//! The background LAN host (P33 step 2, Q-P33f, Q-P33w).
//!
//! The same `clientledger` program runs headless with `--lan-serve`, so the
//! macOS keychain items' access list (which names the app binary) matches.
//! It starts when the owner signs in to the computer:
//!
//! - Windows: a scheduled task with an "at log on" trigger for this user;
//! - macOS: a LaunchAgent in `~/Library/LaunchAgents`;
//! - Linux: a `systemd --user` unit.
//!
//! A system service that runs with nobody signed in is not part of P33 (a
//! separate Tier H addendum, Q-P33f).
//!
//! Hand-over: opening the app on the host while the background host runs
//! sends `stop` over `instance.ctl` and becomes the host itself
//! ([`crate::acquire_or_take_over`]); the background host reports
//! [`BACKGROUND_TAG`] in its `status` line so the app knows it may.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::{HostError, Result};

/// The word the background host puts at the start of its `status` reply.
pub const BACKGROUND_TAG: &str = "role=lan-serve";

/// The command-line flag for the headless host.
pub const SERVE_FLAG: &str = "--lan-serve";

/// The Windows task name, the macOS label and the Linux unit stem.
pub const TASK_NAME: &str = "ClientLedger LAN host";
const MAC_LABEL: &str = "net.clientledger.lan-host";
const LINUX_UNIT: &str = "clientledger-lan-host.service";

/// Where the start-at-sign-in entry lives and how it is managed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    /// Task Scheduler.
    Windows,
    /// launchd.
    MacOs,
    /// systemd --user.
    Linux,
}

impl Platform {
    /// The platform this program runs on.
    #[must_use]
    pub fn current() -> Platform {
        if cfg!(windows) {
            Platform::Windows
        } else if cfg!(target_os = "macos") {
            Platform::MacOs
        } else {
            Platform::Linux
        }
    }
}

/// The program and arguments the entry starts.
#[must_use]
pub fn serve_args(exe: &Path, data_dir: Option<&Path>) -> Vec<OsString> {
    let mut v = vec![exe.as_os_str().to_owned(), OsString::from(SERVE_FLAG)];
    if let Some(d) = data_dir {
        v.push(OsString::from("--data-dir"));
        v.push(d.as_os_str().to_owned());
    }
    v
}

fn quote_windows(s: &str) -> String {
    format!("\"{}\"", s.replace('"', ""))
}

/// The `/TR` value of the Windows task.
#[must_use]
pub fn windows_task_command(exe: &Path, data_dir: Option<&Path>) -> String {
    let mut s = format!("{} {SERVE_FLAG}", quote_windows(&exe.to_string_lossy()));
    if let Some(d) = data_dir {
        s.push_str(" --data-dir ");
        s.push_str(&quote_windows(&d.to_string_lossy()));
    }
    s
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The macOS `LaunchAgent` property list.
#[must_use]
pub fn mac_plist(exe: &Path, data_dir: Option<&Path>) -> String {
    let mut args = String::new();
    for a in serve_args(exe, data_dir) {
        args.push_str("    <string>");
        args.push_str(&xml_escape(&a.to_string_lossy()));
        args.push_str("</string>\n");
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
<plist version=\"1.0\">\n<dict>\n  <key>Label</key>\n  <string>{MAC_LABEL}</string>\n\
  <key>ProgramArguments</key>\n  <array>\n{args}  </array>\n\
  <key>RunAtLoad</key>\n  <true/>\n  <key>KeepAlive</key>\n  <false/>\n\
  <key>ProcessType</key>\n  <string>Background</string>\n</dict>\n</plist>\n"
    )
}

fn systemd_quote(s: &str) -> String {
    format!(
        "\"{}\"",
        s.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('%', "%%")
    )
}

/// The Linux user unit.
#[must_use]
pub fn linux_unit(exe: &Path, data_dir: Option<&Path>) -> String {
    let exec: Vec<String> = serve_args(exe, data_dir)
        .iter()
        .map(|a| systemd_quote(&a.to_string_lossy()))
        .collect();
    format!(
        "[Unit]\nDescription={TASK_NAME}\n\n[Service]\nType=simple\nExecStart={}\nRestart=on-failure\nRestartSec=5\n\n[Install]\nWantedBy=default.target\n",
        exec.join(" ")
    )
}

/// Where the entry file goes (macOS and Linux), under `home`.
#[must_use]
pub fn entry_path(platform: Platform, home: &Path) -> Option<PathBuf> {
    match platform {
        Platform::Windows => None,
        Platform::MacOs => Some(
            home.join("Library")
                .join("LaunchAgents")
                .join(format!("{MAC_LABEL}.plist")),
        ),
        Platform::Linux => Some(
            home.join(".config")
                .join("systemd")
                .join("user")
                .join(LINUX_UNIT),
        ),
    }
}

fn home() -> Result<PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .map(PathBuf::from)
        .ok_or_else(|| HostError::io("find the home folder", std::io::Error::other("no home")))
}

fn run(program: &str, args: &[&str]) -> Result<bool> {
    let status = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| HostError::io("run the system scheduler", e))?;
    Ok(status.success())
}

fn write_entry(path: &Path, text: &str) -> Result<()> {
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p).map_err(|e| HostError::io("create the start-up folder", e))?;
    }
    std::fs::write(path, text).map_err(|e| HostError::io("write the start-up entry", e))
}

/// Starts the background host at every sign-in of this user.
///
/// # Errors
/// The scheduler refused or the entry could not be written.
pub fn install(exe: &Path, data_dir: Option<&Path>) -> Result<()> {
    let ok = match Platform::current() {
        Platform::Windows => {
            let tr = windows_task_command(exe, data_dir);
            run(
                "schtasks",
                &[
                    "/Create", "/F", "/SC", "ONLOGON", "/RL", "LIMITED", "/TN", TASK_NAME, "/TR",
                    &tr,
                ],
            )?
        }
        Platform::MacOs => {
            let path = entry_path(Platform::MacOs, &home()?).unwrap_or_default();
            write_entry(&path, &mac_plist(exe, data_dir))?;
            run("launchctl", &["load", "-w", &path.to_string_lossy()])?
        }
        Platform::Linux => {
            let path = entry_path(Platform::Linux, &home()?).unwrap_or_default();
            write_entry(&path, &linux_unit(exe, data_dir))?;
            run("systemctl", &["--user", "daemon-reload"])?
                && run("systemctl", &["--user", "enable", LINUX_UNIT])?
        }
    };
    if ok {
        Ok(())
    } else {
        Err(HostError::io(
            "register the background host",
            std::io::Error::other("the system scheduler refused"),
        ))
    }
}

/// Removes the start-at-sign-in entry (missing is fine).
///
/// # Errors
/// The entry file could not be removed.
pub fn uninstall() -> Result<()> {
    match Platform::current() {
        Platform::Windows => {
            let _ = run("schtasks", &["/Delete", "/F", "/TN", TASK_NAME])?;
        }
        Platform::MacOs => {
            let path = entry_path(Platform::MacOs, &home()?).unwrap_or_default();
            let _ = run("launchctl", &["unload", "-w", &path.to_string_lossy()]);
            remove_if_present(&path)?;
        }
        Platform::Linux => {
            let _ = run("systemctl", &["--user", "disable", LINUX_UNIT]);
            let path = entry_path(Platform::Linux, &home()?).unwrap_or_default();
            remove_if_present(&path)?;
            let _ = run("systemctl", &["--user", "daemon-reload"]);
        }
    }
    Ok(())
}

fn remove_if_present(p: &Path) -> Result<()> {
    match std::fs::remove_file(p) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(HostError::io("remove the start-up entry", e)),
    }
}

/// True when the start-at-sign-in entry exists.
///
/// # Errors
/// The scheduler could not be asked.
pub fn installed() -> Result<bool> {
    match Platform::current() {
        Platform::Windows => run("schtasks", &["/Query", "/TN", TASK_NAME]),
        p => Ok(entry_path(p, &home()?).is_some_and(|f| f.exists())),
    }
}

/// Starts the background host now, detached from this process ("Keep
/// serving in the background" when the app closes).
///
/// # Errors
/// The program could not be started.
pub fn spawn_background(exe: &Path, data_dir: Option<&Path>) -> Result<()> {
    let args = serve_args(exe, data_dir);
    let mut cmd = Command::new(&args[0]);
    cmd.args(&args[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW
        cmd.creation_flags(0x0000_0008 | 0x0000_0200 | 0x0800_0000);
    }
    cmd.spawn()
        .map(|_| ())
        .map_err(|e| HostError::io("start the background host", e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_command_quotes_paths() {
        let exe = Path::new(r"C:\Program Files\ClientLedger\clientledger.exe");
        assert_eq!(
            windows_task_command(exe, None),
            r#""C:\Program Files\ClientLedger\clientledger.exe" --lan-serve"#
        );
        assert_eq!(
            windows_task_command(exe, Some(Path::new(r"D:\Books Data"))),
            r#""C:\Program Files\ClientLedger\clientledger.exe" --lan-serve --data-dir "D:\Books Data""#
        );
    }

    #[test]
    fn mac_plist_runs_the_same_binary_headless() {
        let p = mac_plist(
            Path::new("/Applications/ClientLedger.app/Contents/MacOS/clientledger"),
            None,
        );
        assert!(p.contains(
            "<string>/Applications/ClientLedger.app/Contents/MacOS/clientledger</string>"
        ));
        assert!(p.contains("<string>--lan-serve</string>"));
        assert!(p.contains("<key>RunAtLoad</key>\n  <true/>"));
        let esc = mac_plist(Path::new("/a&b/<x>"), None);
        assert!(esc.contains("/a&amp;b/&lt;x&gt;"));
    }

    #[test]
    fn linux_unit_quotes_and_escapes() {
        let u = linux_unit(
            Path::new("/opt/client ledger/clientledger"),
            Some(Path::new("/home/a/100%")),
        );
        assert!(u.contains(r#"ExecStart="/opt/client ledger/clientledger" "--lan-serve" "--data-dir" "/home/a/100%%""#), "{u}");
        assert!(u.contains("WantedBy=default.target"));
    }

    #[test]
    fn entry_paths() {
        let h = Path::new("/home/a");
        assert_eq!(entry_path(Platform::Windows, h), None);
        assert!(
            entry_path(Platform::MacOs, h).is_some_and(
                |p| p.ends_with("Library/LaunchAgents/net.clientledger.lan-host.plist")
            )
        );
        assert!(
            entry_path(Platform::Linux, h)
                .is_some_and(|p| p.ends_with(".config/systemd/user/clientledger-lan-host.service"))
        );
    }
}
