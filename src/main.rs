use std::io::Write;
use std::process::{Command, Stdio};
use std::collections::HashMap;

extern crate clap;
use clap::{App, AppSettings, Arg, ArgMatches, SubCommand};

#[macro_use]
extern crate failure;
use failure::Error;

extern crate shellexpand;
use shellexpand::tilde;

use std::env::set_current_dir;
use std::path::Path;

use niri_ipc::{Action, Request, Response };

#[derive(Debug, Fail)]
enum NiriIPCError {
    #[fail(display = "Not handled: {}", err)]
    UnhandledError { err: String },
}

struct ApplicationState<'a> {
    socket: &'a mut niri_ipc::socket::Socket,
    confdir: &'a Path,
}

trait QueryRun {
    fn query(&mut self, request: niri_ipc::Request) -> Result<Option<niri_ipc::Response>, Error>;
    fn run_action(&mut self, request: niri_ipc::Request) -> Result<(), Error>;
}

impl QueryRun for niri_ipc::socket::Socket {
    fn query(&mut self, request: niri_ipc::Request) -> Result<Option<niri_ipc::Response>, Error> {
        match self.send(request)? {
            Ok(niri_ipc::Response::Handled) => Ok(None),
            Ok(x) => Ok(Some(x)),
            Err(err) => Err(NiriIPCError::UnhandledError { err })?,
        }
    }

    fn run_action(&mut self, request: niri_ipc::Request) -> Result<(), Error> {
        match self.send(request)? {
            Ok(niri_ipc::Response::Handled) => Ok(()),
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
        .arg(Arg::with_name("confdir").default_value("~/.config/niri-action/"))
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
    let config = tilde(matches.value_of("confdir").unwrap()).to_string();
    let mut state = ApplicationState {
        socket: &mut niri_ipc::socket::Socket::connect()?,
        confdir: Path::new(&config),
    };

    match matches.subcommand_name() {
        Some("focus-container") => state.focus_container_by_id(),
        Some("steal-container") => state.steal_container_by_id(),
        Some("focus-workspace") => state.focus_workspace_by_name(),
        Some("move-to-workspace") => state.move_to_workspace_by_name(),
        Some("move-workspace-to-output") => state.move_workspace_to_output(),
        Some("workspace-exec") => state.workspace_exec(&matches),
        _ => Ok(()),
    }
}

impl ApplicationState<'_> {
    fn focus_container_by_id(&mut self) -> Result<(), Error> {
        let windows = get_windows(self.socket)?;

        let id = fuzzel_get_selection_id(&windows).parse::<u64>()?;
        self.socket.run_action(Request::Action(Action::FocusWindow { id }))
    }

    fn steal_container_by_id(&mut self) -> Result<(), Error> {
        let windows = get_windows(self.socket)?;
        let ws = get_current_workspace(self.socket)?;

        let id = fuzzel_get_selection_id(&windows).parse::<u64>()?;
        self.socket.run_action(Request::Action(Action::MoveWindowToWorkspace { window_id: Some(id), reference: niri_ipc::WorkspaceReferenceArg::Id(ws), focus: false } ))
    }

    fn focus_workspace_by_name(&mut self) -> Result<(), Error> {
        let work_names = get_workspaces(self.socket)?;


        let ws = fuzzel_get_selection_id_or_entry(&work_names);
        println!("{ws:?} for {work_names:?}");
        match ws.id {
            Some(s) => {
                self.socket.run_action(Request::Action(Action::FocusWorkspace { reference: niri_ipc::WorkspaceReferenceArg::Id(s) }))
            }
            None => {
                let id = work_names.last().expect("No workspaces").split(":").next().expect("Can't split out id").to_string().parse::<u64>()?;
                self.socket.run_action(Request::Action(Action::FocusWorkspace { reference: niri_ipc::WorkspaceReferenceArg::Id(id) }))?;
                self.socket.run_action(Request::Action(Action::SetWorkspaceName { name: ws.entry, workspace: Some(niri_ipc::WorkspaceReferenceArg::Id(id)) }))
            }
        }
    }

    fn move_to_workspace_by_name(&mut self) -> Result<(), Error> {
        let work_names = get_workspaces(self.socket)?;

        let space = fuzzel_get_selection_id(&work_names).parse::<u64>()?;
        self.socket.run_action(Request::Action(Action::MoveWindowToWorkspace { window_id: None, reference: niri_ipc::WorkspaceReferenceArg::Id(space), focus: false } ))
    }

    fn move_workspace_to_output(&mut self) -> Result<(), Error> {
        let outputs = get_outputs(self.socket)?;
        let output = fuzzel_get_selection_id(&outputs);
        self.socket.run_action(Request::Action(Action::MoveWorkspaceToMonitor { output, reference: None }))
    }

    fn workspace_exec(&mut self, matches: &ArgMatches) -> Result<(), Error> {
        let matches = matches.subcommand_matches("workspace-exec").unwrap();
        let mapping_path = self.confdir.join("mapping");
        let workspace = get_current_workspace_name(self.socket)?;
        if workspace.is_empty() {
            return Ok(())
        }
        let map = std::fs::read_to_string(mapping_path)?
            .lines()
            .map(|s| s.split(": "))
            .fold(HashMap::new(), |mut acc, x| {
                acc.insert(
                    x.clone().next().unwrap().to_string(),
                    x.clone().nth(1).unwrap_or("~").to_string(),
                );
                acc
            });

        let dir = match map.get(&workspace[..]) {
            Some(s) => tilde(&s).to_string(),
            None => tilde("~").to_string(),
        };

        let path = Path::new(&dir);

        if !path.exists() {
            return Ok(());
        }

        println!("Switching to {dir}");
        set_current_dir(dir)?;
        let args = matches
            .values_of("args").ok_or(NiriIPCError::UnhandledError { err: "No args found".to_string() })?;
        let mut args: std::vec::Vec<String> = args
            .collect::<Vec<_>>()
            .into_iter()
            .map(|s| s.to_owned())
            .collect();
        let binary = args.remove(0);
        std::process::Command::new(binary).args(&args).spawn()?;
        Ok(())
    }
}

fn get_current_workspace_name(socket: &mut niri_ipc::socket::Socket) -> Result<String, Error> {
    match socket.query(Request::Workspaces)? {
        Some( Response::Workspaces(s) ) => Ok::<std::string::String, Error>(s.into_iter().find(|x| x.is_focused).unwrap().name.unwrap_or("".to_string())),
        None => Ok("".to_string()),
        _ => Ok("".to_string())
    }
}

fn get_outputs(socket: &mut niri_ipc::socket::Socket) -> Result<Vec<String>, Error> {
    match socket.query(Request::Outputs)? {
        Some( Response::Outputs(s) ) => Ok::<std::vec::Vec<std::string::String>, Error>(s.values().map(|x| format!("{}: {} {} {}", x.name, x.make, x.model, x.serial.clone().unwrap_or("<unknown>".to_string())).to_string()).collect()),
        None => Ok(Vec::new()),
        _ => Ok(Vec::new())
    }
}

fn get_windows(socket: &mut niri_ipc::socket::Socket) -> Result<Vec<String>, Error> {
    match socket.query(Request::Windows)? {
        Some( Response::Windows(s) ) => Ok::<std::vec::Vec<std::string::String>, Error>(s.iter().map(|x| format!("{}: {}", x.id, x.title.clone().unwrap_or("Unknown".to_string()))).collect()),
        None => Ok(Vec::new()),
        _ => Ok(Vec::new())
    }
}

fn get_workspaces(socket: &mut niri_ipc::socket::Socket) -> Result<Vec<String>, Error> {
    match socket.query(Request::Workspaces)? {
        Some( Response::Workspaces(s) ) => {
            let mut si = s.clone();
            si.sort_by(|a, b| a.idx.cmp(&b.idx));
            let spaces = si.iter().map(|x| format!("{}: {} ({})", x.id, x.name.clone().unwrap_or("<unnamed>".to_string()), x.idx)).collect();
            Ok::<std::vec::Vec<std::string::String>, Error>(spaces)
        },
        None => Ok(Vec::new()),
        _ => Ok(Vec::new())
    }
}

fn get_current_workspace(socket: &mut niri_ipc::socket::Socket) -> Result<u64, Error> {
    match socket.query(Request::Workspaces)? {
        Some( Response::Workspaces(s) ) => Ok::<u64, Error>(s.into_iter().find(|x| x.is_focused).unwrap().id),
        None => Ok(0),
        _ => Ok(0),
    }
}

fn fuzzel_get_selection_id(input: &[String]) -> String {
    let fuzzel_out = fuzzel_run(input);
    fuzzel_out
        .split(":")
        .next()
        .expect("Can't split out id")
        .to_string()
}

#[derive(Debug)]
struct IDorEntry {
    id: Option<u64>,
    entry: String,
}

fn fuzzel_get_selection_id_or_entry(input: &[String]) -> IDorEntry {
    let fuzzel_out = fuzzel_run(input);
    let mut entry = IDorEntry {
        id: None,
        entry: fuzzel_out.strip_suffix('\n').expect("Failed to strip newline").to_string()
    };
    match fuzzel_out.contains(":") {
        true => {
            entry.id = Some(fuzzel_out.split(":") .next() .expect("Can't split out id").parse::<u64>().expect("Failed to convert ID to u64"));
            entry
        }
        false => entry
    }
}

fn fuzzel_run(input: &[String]) -> String {
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
