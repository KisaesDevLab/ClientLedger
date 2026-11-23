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
use cl_host::{AppPaths, HostConfig, acquire_or_take_over, find_engine_exe};
use cl_lan::settings::{LanSettings, Mode};
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

/// The host process's LAN parts, stopped before the engine (field order).
struct Running {
    /// The only strong reference: the setup window holds a weak one
    /// (`set_running_host`), so dropping this stops the proxy first.
    _lan: Option<Arc<cl_ui::lan_link::LanHost>>,
    _link: HostLink,
}

/// Host mode turns automatic sign-in off and forgets the saved password:
/// the books are open to the office network (plan D2 step 5).
fn clear_auto_sign_in(paths: &AppPaths) {
    let auto = cl_settings::AutoSignIn::in_dir(&paths.root().join("settings"));
    if auto.is_on()
        && let Err(e) = auto.clear(&KeyringSecrets::for_root(paths.root()))
    {
        tracing::warn!(error = %e, "automatic sign-in could not be cleared");
    }
}

/// Starts LAN host mode over a running supervisor when `lan.json` says so.
fn start_lan_host(
    link: &HostLink,
    paths: &AppPaths,
    lan: &LanSettings,
) -> Option<cl_ui::lan_link::LanHost> {
    let host = lan.host.as_ref().filter(|_| lan.mode == Mode::Host)?;
    let started = cl_ui::lan_link::LanHost::start(
        Arc::clone(link.supervisor()) as Arc<dyn cl_ui::engine_link::SnapshotSource>,
        host,
        paths.root(),
        &KeyringSecrets::for_root(paths.root()),
        Arc::new(cl_lan::policy::OsInterfaces),
        Arc::new(cl_lan::discovery::MdnsDiscovery::default()),
    );
    match started {
        Ok(h) => Some(h),
        Err(e) => {
            tracing::error!(error = %e, "LAN host mode could not start");
            // The Office Network window explains it and offers the fix
            // (review M2: a lost certificate is never silently replaced).
            cl_ui::lan_link::host_start_failed(&e);
            None
        }
    }
}

/// An office-network workstation (P33): only the loopback forwarder runs
/// (in this process); cl-api talks to it, and it pins and forwards to the
/// host. If the link can't start (not paired, certificate trouble), the
/// status bar says so and the Office Network window can fix it; nothing
/// local starts in its place.
fn start_workstation(
    paths: &AppPaths,
    lan: &LanSettings,
    guard: cl_host::InstanceGuard,
    shell: &mut ShellOptions,
    options: &mut LaunchOptions,
) -> Result<()> {
    use cl_ui::lan_link::{LanStartError, NoHost, WorkstationRun};
    let store = KeyringSecrets::for_root(paths.root());
    let started = match lan.workstation.as_ref() {
        Some(ws) => WorkstationRun::start(ws, paths.root(), &store, guard),
        None => Err((LanStartError::NotPaired, guard)),
    };
    let endpoint: Arc<dyn cl_api::EndpointSource> = match started {
        Ok(run) => {
            shell.host = Some(run.probe());
            options.focus_request = Some(Arc::clone(&run.focus));
            options.stop_request = Some(Arc::clone(&run.stop));
            let endpoint = run.link.endpoint_source();
            options.keep_alive = Some(Box::new(run));
            endpoint
        }
        Err((e, guard)) => {
            tracing::error!(error = %e, "the office network link could not start");
            shell.host = Some(Arc::new(StaticProbe(HostView::Failed(cl_ui::i18n::t(
                "lan.ws.start_failed",
            )))));
            // Still hold the single-instance lock for this run.
            options.keep_alive = Some(Box::new(guard));
            Arc::new(NoHost)
        }
    };
    // A company link even when the host can't be reached: the Office
    // Network window (to fix the address or forget the host) needs one.
    let client = Client::new(endpoint, ClientConfig::default())
        .context("the engine client could not start")?;
    let queue = ApiQueue::start(Arc::new(|| {}), QueueConfig::default())
        .context("the request queue could not start")?;
    shell.company = Some(Arc::new(CompanyLink::workstation(
        client,
        queue,
        RecentStore::in_dir(&paths.root().join("settings")),
        Arc::new(store),
        paths.root().join("backups"),
    )));
    Ok(())
}

/// `--lan-serve`: the headless background host (Q-P33f). Runs until
/// `instance.ctl` asks it to stop (the app taking over, or the CLI).
fn lan_serve(paths: &AppPaths, args: &args::Args, lan: &LanSettings) -> Result<()> {
    if lan.mode != Mode::Host || lan.host.is_none() {
        anyhow::bail!("LAN host mode is off on this computer");
    }
    let cl_host::Acquired::First(guard) =
        cl_host::acquire(paths).context("could not check for a running copy")?
    else {
        // Something else already serves these books.
        return Ok(());
    };
    let exe = find_engine_exe(args.engine_exe.as_deref())
        .context("the books engine program was not found")?;
    let mut cfg = HostConfig::new(paths.clone(), exe);
    cfg.lan_host = true;
    let link = HostLink::start_tagged(cfg, guard, Some(cl_host::service::BACKGROUND_TAG));
    let lan_host = start_lan_host(&link, paths, lan);
    while !link.stop.load(std::sync::atomic::Ordering::SeqCst) {
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    drop(lan_host);
    drop(link);
    Ok(())
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
    // Reading documents on this computer (A03): the worker starts on the
    // first "Read document", demo included.
    cl_ui::reading::install(cl_ui::reading::ReadingService::for_app(&paths));

    let lan = LanSettings::load(&cl_ui::lan_link::settings_path(paths.root()));
    // The Office Network window reads lan.json and the keychain through
    // this (before any window can open).
    cl_ui::lan_link::install(
        paths.root().to_path_buf(),
        Arc::new(KeyringSecrets::for_root(paths.root())),
    );
    if lan.mode == Mode::Host {
        clear_auto_sign_in(&paths);
    }
    if args.lan_serve {
        return lan_serve(&paths, &args, &lan);
    }

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
        // Another running copy gets a focus request and we are done; a
        // background LAN host hands over to this window (Q-P33w).
        let Some(guard) =
            acquire_or_take_over(&paths).context("could not check for a running copy")?
        else {
            return Ok(());
        };
        paths
            .ensure_dirs()
            .context("could not create app folders")?;
        if lan.mode == Mode::Workstation {
            // P33: the books are the host's. No local database or engine
            // starts, even when the link can't (never an empty local book).
            start_workstation(&paths, &lan, guard, &mut shell, &mut options)?;
        } else if let Some(exe) = find_engine_exe(args.engine_exe.as_deref()) {
            let mut cfg = HostConfig::new(paths.clone(), exe);
            cfg.lan_host = lan.mode == Mode::Host && lan.host.is_some();
            let link = HostLink::start(cfg, guard);
            let lan_host = start_lan_host(&link, &paths, &lan).map(Arc::new);
            if let Some(h) = &lan_host {
                cl_ui::lan_link::set_running_host(h);
            }
            shell.host = Some(link.probe());
            shell.company = Some(company_link(&link, &paths)?);
            options.focus_request = Some(Arc::clone(&link.focus));
            options.stop_request = Some(Arc::clone(&link.stop));
            // Dropped when the window closes, which stops the engine and the
            // database.
            options.keep_alive = Some(Box::new(Running {
                _lan: lan_host,
                _link: link,
            }));
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
