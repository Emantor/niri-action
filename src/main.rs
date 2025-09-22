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
    socket: &'a mut niri_ipc::socket::Socket,
}

trait QueryRun {
    fn query(&mut self, request: niri_ipc::Request) -> Result<Option<niri_ipc::Response>, Error>;
    fn run_action(&mut self, request: niri_ipc::Request) -> Result<(), Error>;
}

impl QueryRun for niri_ipc::socket::Socket {
    fn query(&mut self, request: niri_ipc::Request) -> Result<Option<niri_ipc::Response>, Error> {
        match self.send(request)? {
            Ok(niri_ipc::Response::Handled) => return Ok(None),
            Ok(x) => return Ok(Some(x)),
            Err(err) => Err(NiriIPCError::UnhandledError { err })?,
        }
    }

    fn run_action(&mut self, request: niri_ipc::Request) -> Result<(), Error> {
        match self.send(request)? {
            Ok(niri_ipc::Response::Handled) => return Ok(()),
            Ok(x) => Err(NiriIPCError::UnhandledError { err: format!("Got result for {:?}", x).to_string() })?,
            Err(err) => Err(NiriIPCError::UnhandledError { err })?,
        }
    }
}

fn main() -> Result<(), Error> {
    let matches = App::new("niri-action")
        .version("v0.1.7")
        .author("Rouven Czerwinski <rouven@czerwinskis.de>")
        .about("Provides selections of niri $things via fuzzel")
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
        socket: &mut niri_ipc::socket::Socket::connect()?,
    };

    match matches.subcommand_name() {
        Some("focus-container") => state.focus_container_by_id(),
        Some("steal-container") => state.steal_container_by_id(),
        Some("focus-workspace") => state.focus_workspace_by_name(),
        Some("move-to-workspace") => state.move_to_workspace_by_name(),
        Some("move-workspace-to-output") => state.move_workspace_to_output(),
        _ => Ok({}),
    }
}

impl ApplicationState<'_> {
    fn focus_container_by_id(&mut self) -> Result<(), Error> {
        let windows = get_windows(&mut self.socket)?;

        let id = fuzzel_get_selection_id(&windows).parse::<u64>()?;
        return self.socket.run_action(Request::Action(Action::FocusWindow { id: id }))
    }

    fn steal_container_by_id(&mut self) -> Result<(), Error> {
        let windows = get_windows(&mut self.socket)?;
        let ws = get_current_workspace(&mut self.socket)?;

        let id = fuzzel_get_selection_id(&windows).parse::<u64>()?;
        return self.socket.run_action(Request::Action(Action::MoveWindowToWorkspace { window_id: Some(id), reference: niri_ipc::WorkspaceReferenceArg::Id(ws), focus: false } ))
    }

    fn focus_workspace_by_name(&mut self) -> Result<(), Error> {
        let work_names = get_workspaces(&mut self.socket)?;


        let ws = fuzzel_get_selection_id_or_entry(&work_names);
        println!("{ws:?} for {work_names:?}");
        match work_names.contains(&ws) {
            true => {
                let id = ws.parse::<u64>()?;

                return self.socket.run_action(Request::Action(Action::FocusWorkspace { reference: niri_ipc::WorkspaceReferenceArg::Id(id) }))
            }
            false => {
                let id = work_names.last().expect("No workspaces").split(":").next().expect("Can't split out id").to_string().parse::<u64>()?;
                self.socket.run_action(Request::Action(Action::FocusWorkspace { reference: niri_ipc::WorkspaceReferenceArg::Id(id) }))?;
                self.socket.run_action(Request::Action(Action::SetWorkspaceName { name: ws, workspace: Some(niri_ipc::WorkspaceReferenceArg::Id(id)) }))
            }
        }
    }

    fn move_to_workspace_by_name(&mut self) -> Result<(), Error> {
        let work_names = get_workspaces(&mut self.socket)?;

        let space = fuzzel_get_selection_id(&work_names).parse::<u64>()?;
        return self.socket.run_action(Request::Action(Action::MoveWindowToWorkspace { window_id: None, reference: niri_ipc::WorkspaceReferenceArg::Id(space), focus: false } ))
    }

    fn move_workspace_to_output(&mut self) -> Result<(), Error> {
        let outputs = get_outputs(&mut self.socket)?;
        let output = fuzzel_get_selection_id(&outputs);
        return self.socket.run_action(Request::Action(Action::MoveWorkspaceToMonitor { output: output, reference: None }))
    }
}

fn get_outputs(socket: &mut niri_ipc::socket::Socket) -> Result<Vec<String>, Error> {
    match socket.query(Request::Outputs)? {
        Some( Response::Outputs(s) ) => return Ok(s.values().map(|x| format!("{}: {} {} {}", x.name, x.make, x.model, x.serial.clone().unwrap_or("<unknown>".to_string())).to_string()).collect()),
        None => return Ok(Vec::new()),
        _ => return Ok(Vec::new())
    };
}

fn get_windows(socket: &mut niri_ipc::socket::Socket) -> Result<Vec<String>, Error> {
    match socket.query(Request::Windows)? {
        Some( Response::Windows(s) ) => return Ok(s.iter().map(|x| format!("{}: {}", x.id, x.title.clone().unwrap_or("Unknown".to_string()))).collect()),
        None => return Ok(Vec::new()),
        _ => return Ok(Vec::new())
    };
}

fn get_workspaces(socket: &mut niri_ipc::socket::Socket) -> Result<Vec<String>, Error> {
    match socket.query(Request::Workspaces)? {
        Some( Response::Workspaces(s) ) => {
            let mut si = s.clone();
            si.sort_by(|a, b| a.idx.cmp(&b.idx));
            let spaces = si.iter().map(|x| format!("{}: {} ({})", x.id, x.name.clone().unwrap_or("<unnamed>".to_string()), x.idx)).collect();
            return Ok(spaces)
        },
        None => return Ok(Vec::new()),
        _ => return Ok(Vec::new())
    };
}

fn get_current_workspace(socket: &mut niri_ipc::socket::Socket) -> Result<u64, Error> {
    match socket.query(Request::Workspaces)? {
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

fn fuzzel_get_selection_id_or_entry(input: &Vec<String>) -> String {
    let fuzzel_out = fuzzel_run(&input);
    match fuzzel_out.contains(":") {
        true => return fuzzel_out
            .split(":")
            .next()
            .expect("Can't split out id")
            .to_string(),
        false => return fuzzel_out.strip_suffix('\n').expect("Failed to strip newline").to_string()
    };
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
