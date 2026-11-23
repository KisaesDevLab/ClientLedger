//! `clientledger-cli lan install|uninstall|status` (P33, Q-P33f): the
//! background LAN host that starts when you sign in to this computer.

use std::path::PathBuf;
use std::process::ExitCode;

use crate::host::EXIT_USAGE;

const HELP: &str = "\
Usage: clientledger-cli lan <command> [options]

Commands:
  install     Start the background LAN host each time you sign in
  uninstall   Stop starting it at sign-in
  status      Show whether it starts at sign-in

Options:
  --data-dir <path>   App data folder (default: your user data folder)
  --app-exe <path>    The ClientLedger program (default: next to this one)";

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Install,
    Uninstall,
    Status,
    Help,
}

#[derive(Debug, PartialEq, Eq)]
struct Options {
    command: Command,
    data_dir: Option<PathBuf>,
    app_exe: Option<PathBuf>,
}

fn parse(args: &[String]) -> Result<Options, String> {
    let mut it = args.iter();
    let command = match it.next().map(String::as_str) {
        Some("install") => Command::Install,
        Some("uninstall") => Command::Uninstall,
        Some("status") => Command::Status,
        Some("--help" | "-h" | "help") | None => Command::Help,
        Some(other) => return Err(format!("unknown lan command `{other}`")),
    };
    let mut o = Options {
        command,
        data_dir: None,
        app_exe: None,
    };
    while let Some(a) = it.next() {
        let mut value = |n: &str| {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{n} needs a value"))
        };
        match a.as_str() {
            "--data-dir" => o.data_dir = Some(PathBuf::from(value("--data-dir")?)),
            "--app-exe" => o.app_exe = Some(PathBuf::from(value("--app-exe")?)),
            other => return Err(format!("unknown option `{other}`")),
        }
    }
    Ok(o)
}

fn default_app_exe() -> Option<PathBuf> {
    let me = std::env::current_exe().ok()?;
    let exe = me
        .parent()?
        .join(format!("clientledger{}", std::env::consts::EXE_SUFFIX));
    exe.exists().then_some(exe)
}

/// Runs `lan ...`.
pub(crate) fn run(args: &[String]) -> ExitCode {
    let o = match parse(args) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("error: {e}\n\n{HELP}");
            return ExitCode::from(EXIT_USAGE);
        }
    };
    let result = match o.command {
        Command::Help => {
            println!("{HELP}");
            return ExitCode::SUCCESS;
        }
        Command::Install => {
            let Some(exe) = o.app_exe.or_else(default_app_exe) else {
                eprintln!("The ClientLedger program was not found. Use --app-exe <path>.");
                return ExitCode::FAILURE;
            };
            cl_host::service::install(&exe, o.data_dir.as_deref())
                .map(|()| "The background LAN host now starts when you sign in.".to_string())
        }
        Command::Uninstall => cl_host::service::uninstall()
            .map(|()| "The background LAN host no longer starts at sign-in.".to_string()),
        Command::Status => cl_host::service::installed().map(|on| {
            if on {
                "The background LAN host starts when you sign in.".to_string()
            } else {
                "The background LAN host is not set to start at sign-in.".to_string()
            }
        }),
    };
    match result {
        Ok(msg) => {
            println!("{msg}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(a: &[&str]) -> Result<Options, String> {
        parse(&a.iter().map(|s| (*s).to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn parses_commands_and_options() {
        assert_eq!(p(&[]).map(|o| o.command), Ok(Command::Help));
        let o = p(&["install", "--data-dir", "D:/x", "--app-exe", "C:/a.exe"]).expect("ok");
        assert_eq!(o.command, Command::Install);
        assert_eq!(o.data_dir, Some(PathBuf::from("D:/x")));
        assert_eq!(o.app_exe, Some(PathBuf::from("C:/a.exe")));
        assert!(p(&["frob"]).is_err());
        assert!(p(&["status", "--data-dir"]).is_err());
    }
}
