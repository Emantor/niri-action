use std::io::Write;
use std::process::{Command, Stdio};

extern crate clap;
use clap::{App, AppSettings, Arg, SubCommand};

#[macro_use]
extern crate failure;
use failure::Error;

use niri_ipc::{Action, Request, Response };

#[derive(Debug, Fail)]
enum NiriIPCError {
    #[fail(display = "Not handled: {}", err)]
    UnhandledError { err: String },
}

struct ApplicationState<'a> {
    conn: &'a mut niri_ipc::socket::Socket,
}

fn main() -> Result<(), Error> {
    let matches = App::new("niri-action")
        .version("v0.1.7")
        .author("Rouven Czerwinski <rouven@czerwinskis.de>")
        .about("Provides selections of sway $things via fuzzel")
        .setting(AppSettings::ArgRequiredElseHelp)
        .setting(AppSettings::TrailingVarArg)
        .subcommand(
            SubCommand::with_name("focus-container").about("Focus window by name using fuzzel"),
        )
        .subcommand(
            SubCommand::with_name("steal-container").about("Steal window into current workspace"),
        )
        .subcommand(
            SubCommand::with_name("focus-workspace").about("Focus workspace by name using fuzzel"),
        )
        .subcommand(
            SubCommand::with_name("move-to-workspace")
                .about("Move Currently focused container to workspace"),
        )
        .subcommand(
            SubCommand::with_name("move-workspace-to-output")
                .about("Move current workspace to output by name"),
        )
        .subcommand(
            SubCommand::with_name("workspace-exec")
                .about("execute command in workspace")
                .arg(Arg::with_name("args").multiple(true)),
        )
        .get_matches();

    // establish a connection to i3 over a unix socket
    let mut state = ApplicationState {
        conn: &mut niri_ipc::socket::Socket::connect()?,
    };

    match matches.subcommand_name() {
        Some("focus-container") => focus_container_by_id(&mut state),
        Some("steal-container") => steal_container_by_id(&mut state),
        Some("focus-workspace") => focus_workspace_by_name(&mut state),
        Some("move-to-workspace") => move_to_workspace_by_name(&mut state),
        Some("move-workspace-to-output") => move_workspace_to_output(&mut state),
        _ => Ok({}),
    }
}

fn run_niri_query(conn: &mut niri_ipc::socket::Socket, request: niri_ipc::Request) -> Result<Option<niri_ipc::Response>, Error> {
    match conn.send(request)? {
        Ok(niri_ipc::Response::Handled) => return Ok(None),
        Ok(x) => return Ok(Some(x)),
        Err(err) => Err(NiriIPCError::UnhandledError { err })?,
    }
}

fn focus_container_by_id(state: &mut ApplicationState) -> Result<(), Error> {
    let windows = get_windows(&mut state.conn)?;

    let id = fuzzel_get_selection_id(&windows).parse::<u64>()?;
    match run_niri_query(state.conn, Request::Action(Action::FocusWindow { id: id }))? {
        Some(_) => Ok(()),
        None => Ok(()),
    }
}

fn steal_container_by_id(state: &mut ApplicationState) -> Result<(), Error> {
    let windows = get_windows(&mut state.conn)?;
    let ws = get_current_workspace(&mut state.conn)?;

    let id = fuzzel_get_selection_id(&windows).parse::<u64>()?;
    match run_niri_query(state.conn, Request::Action(Action::MoveWindowToWorkspace { window_id: Some(id), reference: niri_ipc::WorkspaceReferenceArg::Id(ws), focus: false } ))? {
        Some(_) => Ok(()),
        None => Ok(()),
    }
}

fn focus_workspace_by_name(state: &mut ApplicationState) -> Result<(), Error> {
    let work_names = get_workspaces(&mut state.conn)?;

    let id = fuzzel_get_selection_id(&work_names).parse::<u8>()?;

    match run_niri_query(state.conn, Request::Action(Action::FocusWorkspace { reference: niri_ipc::WorkspaceReferenceArg::Index(id) }))? {
        Some(_) => Ok(()),
        None => Ok(()),
    }
}

fn move_to_workspace_by_name(state: &mut ApplicationState) -> Result<(), Error> {
    let work_names = get_workspaces(&mut state.conn)?;

    let space = fuzzel_get_selection_id(&work_names).parse::<u8>()?;
    match run_niri_query(state.conn, Request::Action(Action::MoveWindowToWorkspace { window_id: None, reference: niri_ipc::WorkspaceReferenceArg::Index(space), focus: false } ))? {
        Some(_) => Ok(()),
        None => Ok(()),
    }
}

fn move_workspace_to_output(state: &mut ApplicationState) -> Result<(), Error> {
    let outputs = get_outputs(&mut state.conn)?;
    let output = fuzzel_get_selection_id(&outputs);
    match run_niri_query(state.conn, Request::Action(Action::MoveWorkspaceToMonitor { output: output, reference: None }))? {
        Some(_) => Ok(()),
        None => Ok(()),
    }
}

fn get_outputs(conn: &mut niri_ipc::socket::Socket) -> Result<Vec<String>, Error> {
    match run_niri_query(conn, Request::Outputs)? {
        Some( Response::Outputs(s) ) => return Ok(s.values().map(|x| format!("{}: {} {} {}", x.name, x.make, x.model, x.serial.clone().unwrap_or("<unknown>".to_string())).to_string()).collect()),
        None => return Ok(Vec::new()),
        _ => return Ok(Vec::new())
    };
}

fn get_windows(conn: &mut niri_ipc::socket::Socket) -> Result<Vec<String>, Error> {
    match run_niri_query(conn, Request::Windows)? {
        Some( Response::Windows(s) ) => return Ok(s.iter().map(|x| format!("{}: {}", x.id, x.title.clone().unwrap_or("Unknown".to_string()))).collect()),
        None => return Ok(Vec::new()),
        _ => return Ok(Vec::new())
    };
}

fn get_workspaces(conn: &mut niri_ipc::socket::Socket) -> Result<Vec<String>, Error> {
    match run_niri_query(conn, Request::Workspaces)? {
        Some( Response::Workspaces(s) ) => return Ok(s.iter().map(|x| format!("{}: {}", x.idx, x.name.clone().unwrap_or("<none>".to_string()))).collect()),
        None => return Ok(Vec::new()),
        _ => return Ok(Vec::new())
    };
}

fn get_current_workspace(conn: &mut niri_ipc::socket::Socket) -> Result<u64, Error> {
    match run_niri_query(conn, Request::Workspaces)? {
        Some( Response::Workspaces(s) ) => return Ok(s.into_iter().filter(|x| x.is_focused == true).next().unwrap().id),
        None => return Ok(0),
        _ => return Ok(0),
    };
}

fn fuzzel_get_selection_id(input: &Vec<String>) -> String {
    let fuzzel_out = fuzzel_run(&input);
    fuzzel_out
        .split(":")
        .next()
        .expect("Can't split out id")
        .to_string()
}

fn fuzzel_run(input: &Vec<String>) -> String {
    let mut child = Command::new("fuzzel")
        .arg("--dmenu")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("Can't open fuzzel");
    {
        let stdin = child.stdin.as_mut().expect("failed to get stdin");
        stdin
            .write_all(input.join("\n").as_bytes())
            .expect("failed to write to fuzzel");
    }
    let output = child.wait_with_output().expect("failed to wait on child");
    String::from_utf8(output.stdout).expect("Can't read output")
}
