//! The `clientledger` desktop application.
//!
//! Start-up: find the app data folder, take the single-instance lock (a second
//! launch just brings the first window forward), start the supervisor for the
//! bundled database and the books engine in the background, and open the
//! window. The window never waits for the engine; its status bar shows how the
//! engine is doing.
#![forbid(unsafe_code)]
// A desktop app: no console window on Windows release builds.
#![cfg_attr(all(not(debug_assertions), windows), windows_subsystem = "windows")]

mod args;

use std::sync::Arc;

use anyhow::{Context, Result};
use cl_host::{AppPaths, HostConfig, acquire_or_focus, find_engine_exe};
use cl_ui::app::{LaunchOptions, run};
use cl_ui::engine_link::{HostLink, StaticProbe};
use cl_ui::prefs::UiStore;
use cl_ui::shell::{HostView, ShellOptions};

fn main() -> Result<()> {
    let args = match args::parse(std::env::args().skip(1)) {
        Ok(a) => a,
        Err(message) => {
            eprintln!("error: {message}\n\n{}", args::USAGE);
            std::process::exit(2);
        }
    };
    if args.help {
        println!("{}", args::USAGE);
        return Ok(());
    }

    let paths = match &args.data_dir {
        Some(dir) => AppPaths::at(dir.clone()),
        None => AppPaths::for_current_user(),
    }
    .context("could not find a place for ClientLedger's data")?;

    // Events from the UI go to <logs>/ui.log (rotating); the host keeps host.log.
    let _ = paths.ensure_dirs();
    cl_ui::logging::init(paths.logs_dir().join("ui.log"));

    let mut shell = ShellOptions {
        store: Some(UiStore::new(paths.root().join("ui"))),
        log_path: Some(paths.host_log()),
        demo: args.demo,
        theme: args.theme,
        zoom_percent: args.zoom,
        ..ShellOptions::default()
    };
    let mut options = LaunchOptions::default();
    if let Some(size) = args.size {
        options.inner_size = size;
    }

    if !args.demo && !args.no_engine {
        // Another running copy gets a focus request and we are done.
        let Some(guard) = acquire_or_focus(&paths).context("could not check for a running copy")?
        else {
            return Ok(());
        };
        paths
            .ensure_dirs()
            .context("could not create app folders")?;
        if let Some(exe) = find_engine_exe(args.engine_exe.as_deref()) {
            let link = HostLink::start(HostConfig::new(paths.clone(), exe), guard);
            shell.host = Some(link.probe());
            options.focus_request = Some(Arc::clone(&link.focus));
            options.stop_request = Some(Arc::clone(&link.stop));
            // Dropped when the window closes, which stops the engine and the
            // database.
            options.keep_alive = Some(Box::new(link));
        } else {
            // Still hold the single-instance lock for this run.
            options.keep_alive = Some(Box::new(guard));
            shell.host = Some(Arc::new(StaticProbe(HostView::Failed(cl_ui::i18n::t(
                "status.engine.not_found",
            )))));
        }
    }
    options.shell = shell;

    run(options).map_err(|e| anyhow::anyhow!("the window could not start: {e}"))
}
