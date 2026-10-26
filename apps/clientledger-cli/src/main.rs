//! `clientledger-cli`: headless company operations and support commands.
//!
//! Output here is for the person at the terminal (an owner helping support, or
//! a developer); it is English only and never contains secrets.
#![forbid(unsafe_code)]

mod host;
mod read;
mod ui;

use std::process::ExitCode;

const USAGE: &str = "\
clientledger-cli

Usage:
  clientledger-cli host status|start|stop|logs [options]
  clientledger-cli read <file> [--json]
  clientledger-cli ui --control <path> <method> [arguments]
  clientledger-cli tools

Run `clientledger-cli host --help` for the host options,
`clientledger-cli read --help` for reading a document on this computer and
`clientledger-cli ui --help` for driving a running app.";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("host") => host::run(&args[1..]),
        Some("read") => read::run(&args[1..]),
        Some("ui") => ui::run_ui(&args[1..]),
        Some("tools") => ui::run_tools(),
        Some("--help" | "-h" | "help") => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(host::EXIT_USAGE)
        }
    }
}
