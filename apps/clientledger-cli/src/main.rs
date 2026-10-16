//! `clientledger-cli`: headless company operations and support commands.
//!
//! Output here is for the person at the terminal (an owner helping support, or
//! a developer); it is English only and never contains secrets.
#![forbid(unsafe_code)]

mod host;

use std::process::ExitCode;

const USAGE: &str = "\
clientledger-cli

Usage:
  clientledger-cli host status|start|stop|logs [options]

Run `clientledger-cli host --help` for the host options.";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("host") => host::run(&args[1..]),
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
