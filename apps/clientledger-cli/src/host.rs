//! `clientledger-cli host status|start|stop|logs`.
//!
//! `start` runs the supervisor in the foreground until Ctrl+C or `host stop`.
//! The other commands talk to that running process through the same
//! single-instance endpoint the desktop app uses (`cl_host::send`), so the CLI
//! and the app can never run two supervisors for one user at once.

use cl_host::{
    AppPaths, HostConfig, HostState, Request, SendError, Supervisor, acquire, engine_exe_name,
    find_engine_exe, send, tail_lines,
};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Exit code: wrong arguments.
pub(crate) const EXIT_USAGE: u8 = 2;
/// Exit code: nothing is running (`status`, `stop`).
pub(crate) const EXIT_NOT_RUNNING: u8 = 3;
/// Exit code: another instance already runs (`start`).
pub(crate) const EXIT_ALREADY_RUNNING: u8 = 4;
const EXIT_FAILURE: u8 = 1;

const HELP: &str = "\
Usage: clientledger-cli host <command> [options]

Commands:
  status   Show whether the database and engine are running (exit 3 if not)
  start    Start them and keep running until Ctrl+C or `host stop`
  stop     Ask the running host to shut down
  logs     Show the end of the host log

Options:
  --data-dir <path>     App data folder (default: your user data folder)
  --engine-exe <path>   Engine program (start only)
  --lines <n>           Lines to show (logs only, default 50)
  --engine              Show the engine log instead of the host log (logs only)";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Command {
    Status,
    Start,
    Stop,
    Logs { lines: usize, engine: bool },
    Help,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Options {
    pub command: Command,
    pub data_dir: Option<PathBuf>,
    pub engine_exe: Option<PathBuf>,
}

/// Parse the arguments after `host`.
pub(crate) fn parse(args: &[String]) -> Result<Options, String> {
    let mut it = args.iter();
    let command = match it.next().map(String::as_str) {
        Some("status") => Command::Status,
        Some("start") => Command::Start,
        Some("stop") => Command::Stop,
        Some("logs") => Command::Logs {
            lines: 50,
            engine: false,
        },
        Some("--help" | "-h" | "help") => {
            return Ok(Options {
                command: Command::Help,
                data_dir: None,
                engine_exe: None,
            });
        }
        Some(other) => return Err(format!("unknown command `{other}`")),
        None => return Err("missing command".into()),
    };
    let mut opts = Options {
        command,
        data_dir: None,
        engine_exe: None,
    };
    while let Some(arg) = it.next() {
        let mut value = |name: &str| {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{name} needs a value"))
        };
        match arg.as_str() {
            "--data-dir" => opts.data_dir = Some(PathBuf::from(value("--data-dir")?)),
            "--engine-exe" => opts.engine_exe = Some(PathBuf::from(value("--engine-exe")?)),
            "--lines" => {
                let n: usize = value("--lines")?
                    .parse()
                    .map_err(|_| "--lines needs a number".to_string())?;
                match &mut opts.command {
                    Command::Logs { lines, .. } => *lines = n.clamp(1, 5000),
                    _ => return Err("--lines only applies to `logs`".into()),
                }
            }
            "--engine" => match &mut opts.command {
                Command::Logs { engine, .. } => *engine = true,
                _ => return Err("--engine only applies to `logs`".into()),
            },
            other => return Err(format!("unknown option `{other}`")),
        }
    }
    Ok(opts)
}

fn paths_for(opts: &Options) -> Result<AppPaths, String> {
    match &opts.data_dir {
        Some(dir) => {
            let abs = std::path::absolute(dir).map_err(|e| format!("bad --data-dir: {e}"))?;
            AppPaths::at(abs).map_err(|e| e.to_string())
        }
        None => AppPaths::for_current_user().map_err(|e| e.to_string()),
    }
}

pub(crate) fn run(args: &[String]) -> ExitCode {
    let opts = match parse(args) {
        Ok(o) => o,
        Err(msg) => {
            eprintln!("error: {msg}\n\n{HELP}");
            return ExitCode::from(EXIT_USAGE);
        }
    };
    if opts.command == Command::Help {
        println!("{HELP}");
        return ExitCode::SUCCESS;
    }
    let paths = match paths_for(&opts) {
        Ok(p) => p,
        Err(msg) => {
            eprintln!("error: {msg}");
            return ExitCode::from(EXIT_FAILURE);
        }
    };
    match &opts.command {
        Command::Status => status(&paths),
        Command::Start => start(&paths, &opts),
        Command::Stop => stop(&paths),
        Command::Logs { lines, engine } => logs(&paths, *lines, *engine),
        Command::Help => ExitCode::SUCCESS,
    }
}

fn status(paths: &AppPaths) -> ExitCode {
    match send(paths, Request::Status) {
        Ok(line) => {
            println!("{line}");
            ExitCode::SUCCESS
        }
        Err(SendError::NotRunning) => {
            println!("not running");
            ExitCode::from(EXIT_NOT_RUNNING)
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(EXIT_FAILURE)
        }
    }
}

fn stop(paths: &AppPaths) -> ExitCode {
    match send(paths, Request::Stop) {
        Ok(_) => {}
        Err(SendError::NotRunning) => {
            println!("not running");
            return ExitCode::from(EXIT_NOT_RUNNING);
        }
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(EXIT_FAILURE);
        }
    }
    // Wait for the host to finish shutting down (the database can take a while).
    let deadline = Instant::now() + Duration::from_mins(2);
    while Instant::now() < deadline {
        if matches!(send(paths, Request::Status), Err(SendError::NotRunning)) {
            println!("stopped");
            return ExitCode::SUCCESS;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    eprintln!("error: the host did not stop within two minutes");
    ExitCode::from(EXIT_FAILURE)
}

fn logs(paths: &AppPaths, lines: usize, engine: bool) -> ExitCode {
    let file = if engine {
        paths.engine_log()
    } else {
        paths.host_log()
    };
    println!("# {}", file.display());
    let tail = tail_lines(&file, lines);
    if tail.is_empty() {
        println!("# (empty or missing)");
    }
    for line in tail {
        println!("{line}");
    }
    ExitCode::SUCCESS
}

fn start(paths: &AppPaths, opts: &Options) -> ExitCode {
    let Some(engine_exe) = find_engine_exe(opts.engine_exe.as_deref()) else {
        eprintln!(
            "error: the engine program ({}) was not found; pass --engine-exe <path> or set CL_ENGINE_EXE",
            engine_exe_name()
        );
        return ExitCode::from(EXIT_FAILURE);
    };
    if let Err(e) = paths.ensure_dirs() {
        eprintln!("error: {e}");
        return ExitCode::from(EXIT_FAILURE);
    }
    let guard = match acquire(paths) {
        Ok(cl_host::Acquired::First(g)) => g,
        Ok(cl_host::Acquired::Second) => {
            println!("ClientLedger is already running (use `host status`)");
            return ExitCode::from(EXIT_ALREADY_RUNNING);
        }
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(EXIT_FAILURE);
        }
    };

    let interrupted = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&interrupted);
    if let Err(e) = ctrlc::set_handler(move || flag.store(true, Ordering::SeqCst)) {
        eprintln!("warning: Ctrl+C will not shut down cleanly: {e}");
    }

    println!("starting (engine: {})", engine_exe.display());
    let sup = Supervisor::spawn(HostConfig::new(paths.clone(), engine_exe), guard);
    let mut announced = false;
    let code = loop {
        let status = sup.status();
        sup.instance().set_status(&status.summary());
        if !announced {
            for notice in &status.notices {
                println!("notice: {notice}");
            }
        }
        match &status.state {
            HostState::Running if !announced => {
                announced = true;
                println!("ready: database and engine are running");
            }
            HostState::Fault(fault) => {
                eprintln!("error: {fault}");
                break ExitCode::from(EXIT_FAILURE);
            }
            HostState::Stopped => break ExitCode::SUCCESS,
            _ => {}
        }
        if interrupted.load(Ordering::SeqCst) || sup.instance().stop_requested() {
            break ExitCode::SUCCESS;
        }
        std::thread::sleep(Duration::from_millis(250));
    };
    sup.instance().set_status("state=stopping");
    println!("stopping");
    sup.stop();
    println!("stopped");
    code
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn commands_are_parsed() {
        assert_eq!(parse(&args(&["status"])).unwrap().command, Command::Status);
        assert_eq!(parse(&args(&["start"])).unwrap().command, Command::Start);
        assert_eq!(parse(&args(&["stop"])).unwrap().command, Command::Stop);
        assert_eq!(parse(&args(&["--help"])).unwrap().command, Command::Help);
    }

    #[test]
    fn logs_options_are_parsed_and_limited() {
        let o = parse(&args(&["logs", "--lines", "7", "--engine"])).unwrap();
        assert_eq!(
            o.command,
            Command::Logs {
                lines: 7,
                engine: true
            }
        );
        let big = parse(&args(&["logs", "--lines", "999999"])).unwrap();
        assert_eq!(
            big.command,
            Command::Logs {
                lines: 5000,
                engine: false
            }
        );
    }

    #[test]
    fn paths_and_exe_options_are_parsed() {
        let o = parse(&args(&["start", "--data-dir", "/d", "--engine-exe", "/e"])).unwrap();
        assert_eq!(o.data_dir, Some(PathBuf::from("/d")));
        assert_eq!(o.engine_exe, Some(PathBuf::from("/e")));
    }

    #[test]
    fn bad_input_is_rejected_with_a_reason() {
        for bad in [
            vec![],
            vec!["frobnicate"],
            vec!["status", "--lines", "3"],
            vec!["start", "--engine"],
            vec!["logs", "--lines"],
            vec!["logs", "--lines", "many"],
            vec!["status", "--nope"],
        ] {
            let err = parse(&args(&bad)).unwrap_err();
            assert!(!err.is_empty(), "{bad:?}");
        }
    }
}
