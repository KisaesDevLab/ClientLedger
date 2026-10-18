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
use cl_api::{ApiQueue, Client, ClientConfig, QueueConfig};
use cl_host::{AppPaths, HostConfig, acquire_or_focus, find_engine_exe};
use cl_settings::{KeyringSecrets, RecentStore};
use cl_ui::app::{LaunchOptions, run};
use cl_ui::company::CompanyLink;
use cl_ui::engine_link::{HostLink, StaticProbe};
use cl_ui::prefs::UiStore;
use cl_ui::shell::{HostView, ShellOptions};

/// The engine link for the company screens: a client over the verified
/// endpoint, the request queue, the recent-companies store and the
/// keychain, all under the app data folder (P08).
fn company_link(link: &HostLink, paths: &AppPaths) -> Result<Arc<CompanyLink>> {
    let client = Client::new(link.endpoint_source(), ClientConfig::default())
        .context("the engine client could not start")?;
    let queue = ApiQueue::start(Arc::new(|| {}), QueueConfig::default())
        .context("the request queue could not start")?;
    Ok(Arc::new(CompanyLink::live(
        client,
        queue,
        Arc::clone(link.supervisor()),
        RecentStore::in_dir(&paths.root().join("settings")),
        Arc::new(KeyringSecrets::for_root(paths.root())),
        paths.root().join("backups"),
    )))
}

/// The control channel exists only when `--control <path>` was given: no
/// flag, no listener (P07). A refused control file stops the start-up.
fn start_control(path: Option<&std::path::Path>) -> Result<Option<cl_automation::ControlServer>> {
    path.map(|p| {
        cl_automation::ControlServer::start(p).context("the control channel could not start")
    })
    .transpose()
}

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
            shell.company = Some(company_link(&link, &paths)?);
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
    if args.demo {
        shell.company = Some(Arc::new(CompanyLink::demo(&paths.root().join("demo"))));
    }
    options.shell = shell;
    options.control = start_control(args.control.as_deref())?;

    run(options).map_err(|e| anyhow::anyhow!("the window could not start: {e}"))
}

#[cfg(test)]
mod tests {
    use super::start_control;

    #[test]
    fn no_flag_means_no_server_and_no_file() {
        assert!(start_control(None).unwrap().is_none());
    }

    #[test]
    fn flag_starts_a_loopback_server_and_stop_removes_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("c.json");
        let server = start_control(Some(&path)).unwrap().unwrap();
        assert!(server.addr().ip().is_loopback());
        assert!(path.exists());
        drop(server);
        assert!(!path.exists());
    }
}
