//! Starting and stopping the server.

use crate::cluster::write_file;
use crate::{Cluster, PgError, Result, Secret};
use crate::{pidfile, ports, proc};
use std::fs;
use std::time::{Duration, Instant};

/// Name of the file `ClientLedger` owns inside the data directory. It is
/// rewritten before every start and included last from `postgresql.conf`.
const OVERRIDES_FILE: &str = "clientledger.conf";

/// How many ports to try before giving up.
const MAX_START_ATTEMPTS: u32 = 5;

/// How long a freshly started or adopted server may take to answer.
const READY_TIMEOUT: Duration = Duration::from_secs(30);

/// How long to keep retrying while a crashed server's leftovers clear.
const LEFTOVER_RETRIES: u32 = 15;

/// Phrases in a server log that mean "that port is taken".
const PORT_TAKEN: [&str; 4] = [
    "could not bind",
    "could not create any tcp/ip sockets",
    "address already in use",
    "only one usage of each socket address",
];

/// Phrases that mean a crashed server's leftovers are still being cleaned up.
const LEFTOVERS: [&str; 3] = [
    "pre-existing shared memory block",
    "lock file \"postmaster.pid\" already exists",
    "could not create lock file",
];

/// Who a `postmaster.pid` belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Owner {
    /// A `PostgreSQL` process for this data directory.
    Ours,
    /// A leftover: the process is another program, or another data directory's server.
    Foreign,
    /// The process exists but could not be identified.
    Unknown,
}

/// A started server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Running {
    /// The loopback port it listens on.
    pub port: u16,
    /// Operating system process id of the postmaster.
    pub pid: u32,
    /// True when a server was already running and was adopted as is.
    pub reused: bool,
}

/// How to reach the server. The password is a [`Secret`].
#[derive(Debug, Clone)]
pub struct ConnectionInfo {
    /// Always `127.0.0.1`.
    pub host: &'static str,
    /// The port from [`Running`].
    pub port: u16,
    /// The superuser name.
    pub user: &'static str,
    /// The superuser password.
    pub password: Secret,
}

impl ConnectionInfo {
    /// `postgres://postgres:<password>@127.0.0.1:<port>/<database>`.
    /// The password is hexadecimal, so no escaping is needed.
    #[must_use]
    pub fn url(&self, database: &str) -> Secret {
        Secret::new(format!(
            "postgres://{}:{}@{}:{}/{}",
            self.user,
            self.password.expose(),
            self.host,
            self.port,
            database
        ))
    }
}

/// Text of `clientledger.conf` for `port`.
pub(crate) fn overrides_text(port: u16) -> String {
    format!(
        "\
# Managed by ClientLedger. Rewritten on every start.
listen_addresses = '127.0.0.1'
port = {port}
unix_socket_directories = ''
password_encryption = 'scram-sha-256'
logging_collector = on
log_directory = 'log'
log_filename = 'postgresql-%a.log'
log_truncate_on_rotation = on
log_rotation_age = 1d
log_statement = 'none'
log_error_verbosity = terse
shared_preload_libraries = ''
archive_mode = off
ssl = off
log_connections = off
log_min_error_statement = panic
"
    )
}

/// The security-relevant settings, given on the command line so no file in the
/// data directory (which a restored backup or another program could have
/// changed) can override them. Command-line settings beat every configuration
/// file, including `postgresql.auto.conf`.
///
/// Paths are double-quoted because `pg_ctl` hands this text to a shell;
/// `check_data_dir` refuses folder names that could break out of the quotes.
pub(crate) fn server_pins(data_dir: &std::path::Path, port: u16) -> String {
    let file = |name: &str| {
        let p = data_dir.join(name).to_string_lossy().into_owned();
        if cfg!(windows) {
            p.replace('\\', "/")
        } else {
            p
        }
    };
    format!(
        "-c listen_addresses=127.0.0.1 -c port={port}          -c unix_socket_directories= -c shared_preload_libraries=          -c archive_mode=off -c ssl=off -c log_statement=none          -c log_error_verbosity=terse          -c \"hba_file={}\" -c \"ident_file={}\"",
        file("pg_hba.conf"),
        file("pg_ident.conf")
    )
}

/// Poll until the server answers or `limit` passes.
fn wait_ready(port: u16, limit: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < limit {
        if ports::is_ready(port) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

impl Cluster {
    /// How to connect once the server is running on `port`.
    #[must_use]
    pub fn connection(&self, port: u16) -> ConnectionInfo {
        ConnectionInfo {
            host: "127.0.0.1",
            port,
            user: "postgres",
            password: self.password.clone(),
        }
    }

    /// Start the server on a free loopback port, or adopt one that is already
    /// running for this data directory.
    ///
    /// A leftover `postmaster.pid` from a crash is handled safely: if its
    /// process is gone `PostgreSQL` replaces it and replays its log; if another
    /// program now owns that process id the file is removed; if a real server
    /// is alive but silent we refuse rather than start a second one.
    ///
    /// # Errors
    /// Fails when the folder is no longer acceptable, a stuck server holds the
    /// data directory, or no port works.
    pub fn start(&self) -> Result<Running> {
        self.start_on_ports(std::iter::from_fn(|| ports::free_loopback_port().ok()))
    }

    /// [`Cluster::start`] with a chosen port sequence (tests use this to force
    /// a collision). A port that is taken is replaced by the next one.
    ///
    /// # Errors
    /// Same as [`Cluster::start`].
    #[doc(hidden)]
    pub fn start_on_ports(&self, candidates: impl IntoIterator<Item = u16>) -> Result<Running> {
        crate::check_data_dir(&self.data_dir).map_err(|issue| PgError::DataDir { issue })?;
        self.write_static_config()?;
        if let Some(running) = self.adopt_existing()? {
            return Ok(running);
        }
        let mut attempts = 0u32;
        let mut leftover_waits = 0u32;
        let mut last_detail = String::from("no free port was available");
        let mut candidates = candidates.into_iter();
        let mut port = candidates.next();
        while let Some(p) = port {
            attempts += 1;
            self.write_overrides(p)?;
            let _ = fs::remove_file(self.startup_log());
            let err = match self.pg_ctl_start(p) {
                Ok(()) => return self.finish_start(p),
                Err(e) => e,
            };
            let log = self.settled_log_tail();
            last_detail = format!("{err} {log}");
            let lower = log.to_ascii_lowercase();
            if LEFTOVERS.iter().any(|m| lower.contains(m)) && leftover_waits < LEFTOVER_RETRIES {
                // Backends of a crashed server may still be exiting: same port again.
                leftover_waits += 1;
                attempts -= 1;
                std::thread::sleep(Duration::from_secs(1));
                continue;
            }
            if !PORT_TAKEN.iter().any(|m| lower.contains(m)) || attempts >= MAX_START_ATTEMPTS {
                break;
            }
            tracing::warn!(port = p, "port was taken; trying another");
            port = candidates.next();
        }
        Err(PgError::StartFailed {
            attempts,
            detail: proc::tail(&last_detail, Some(&self.password)),
        })
    }

    /// Where `pg_ctl` sends the server's first messages. It must live outside
    /// the data directory: after a crash the server fsyncs every file in the
    /// data directory, and on Windows that fails with a sharing violation on a
    /// log file that is still open.
    fn startup_log(&self) -> std::path::PathBuf {
        let mut name = self
            .data_dir
            .file_name()
            .map_or_else(|| "pgdata".into(), std::ffi::OsStr::to_os_string);
        name.push(".startup.log");
        self.data_dir.with_file_name(name)
    }

    pub(crate) fn write_overrides(&self, port: u16) -> Result<()> {
        write_file(
            &self.data_dir.join(OVERRIDES_FILE),
            overrides_text(port).as_bytes(),
        )
    }

    fn pg_ctl_start(&self, port: u16) -> Result<()> {
        let mut cmd = proc::command(&self.bins.program("pg_ctl"));
        cmd.arg("start")
            .arg("-D")
            .arg(&self.data_dir)
            .arg("-l")
            .arg(self.startup_log())
            .args(["-w", "-t", "60", "-s", "-o"])
            .arg(server_pins(&self.data_dir, port));
        // The server inherits pg_ctl handles; with pipes, Command::output() waits
        // for the server to exit. Its messages go to startup.log instead.
        cmd.stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        let status = cmd.status().map_err(|e| PgError::io("start pg_ctl", e))?;
        if status.success() {
            Ok(())
        } else {
            Err(PgError::Command {
                program: "pg_ctl",
                status: status
                    .code()
                    .map_or_else(|| "killed".into(), |c| format!("exit {c}")),
                detail: String::new(),
            })
        }
    }

    fn finish_start(&self, port: u16) -> Result<Running> {
        if !wait_ready(port, READY_TIMEOUT) {
            return Err(PgError::StartFailed {
                attempts: 1,
                detail: format!("not accepting connections. {}", self.log_tail()),
            });
        }
        let pid = pidfile::read(&self.data_dir).map_or(0, |p| p.pid);
        tracing::info!(port, "database server started");
        Ok(Running {
            port,
            pid,
            reused: false,
        })
    }

    /// `pg_ctl status`: true when a server process exists for this data directory.
    fn is_running(&self) -> Result<bool> {
        let mut cmd = proc::command(&self.bins.program("pg_ctl"));
        cmd.arg("status").arg("-D").arg(&self.data_dir);
        let f = proc::finish(cmd, "pg_ctl", Some(&self.password))?;
        Ok(f.code == Some(0))
    }

    /// Is the process named in `pf` really this data directory's server? The
    /// number alone proves nothing: after a crash or reboot the operating
    /// system may have given it to another program, even another `PostgreSQL`.
    fn owner_of(&self, pf: &pidfile::PidFile) -> Owner {
        if !pidfile::same_dir(&pf.data_dir, &self.data_dir) {
            return Owner::Foreign;
        }
        match pidfile::process_is_postgres(pf.pid) {
            Some(true) => Owner::Ours,
            Some(false) => Owner::Foreign,
            None => Owner::Unknown,
        }
    }

    /// Delete a `postmaster.pid` that belongs to nobody we manage.
    fn remove_foreign_pid_file(&self) {
        tracing::warn!("removing postmaster.pid owned by an unrelated program");
        let _ = fs::remove_file(self.data_dir.join("postmaster.pid"));
    }

    /// If a server for this data directory is up (or coming up), return it.
    /// A pid file whose process now belongs to another program is deleted.
    fn adopt_existing(&self) -> Result<Option<Running>> {
        let Some(pf) = pidfile::read(&self.data_dir) else {
            return Ok(None);
        };
        if !self.is_running()? {
            // Stale file from a crash: PostgreSQL replaces it itself and
            // replays its write-ahead log.
            return Ok(None);
        }
        if self.owner_of(&pf) == Owner::Foreign {
            // A crash or reboot left the file and another program reused the pid.
            self.remove_foreign_pid_file();
            return Ok(None);
        }
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(port) = pf.port.filter(|p| ports::is_ready(*p)) {
                return Ok(Some(Running {
                    port,
                    pid: pf.pid,
                    reused: true,
                }));
            }
            if Instant::now() >= deadline || !self.is_running()? {
                break;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        if !self.is_running()? {
            return Ok(None);
        }
        Err(PgError::StuckProcess { pid: pf.pid })
    }

    /// Stop the server with `pg_ctl stop -m fast` and wait for it. Succeeds
    /// when nothing is running.
    ///
    /// # Errors
    /// Fails when `pg_ctl` cannot stop the server.
    pub fn stop(&self) -> Result<()> {
        if !self.is_running()? {
            return Ok(());
        }
        if let Some(pf) = pidfile::read(&self.data_dir)
            && self.owner_of(&pf) == Owner::Foreign
        {
            // `pg_ctl stop` would signal whatever owns that pid.
            self.remove_foreign_pid_file();
            return Ok(());
        }
        let mut cmd = proc::command(&self.bins.program("pg_ctl"));
        cmd.arg("stop")
            .arg("-D")
            .arg(&self.data_dir)
            .args(["-m", "fast", "-w", "-t", "60", "-s"]);
        proc::run(cmd, "pg_ctl", Some(&self.password))?;
        tracing::info!("database server stopped");
        Ok(())
    }

    /// The running server's port, when one is up for this data directory.
    #[must_use]
    pub fn running_port(&self) -> Option<u16> {
        let port = pidfile::read(&self.data_dir)?.port?;
        (self.is_running().unwrap_or(false) && ports::is_ready(port)).then_some(port)
    }

    /// [`Cluster::log_tail`] after giving the logging collector a moment to
    /// write the reason the server stopped (it runs in another process, so the
    /// message can trail `pg_ctl`'s exit by a fraction of a second).
    fn settled_log_tail(&self) -> String {
        let known = |t: &str| {
            let lower = t.to_ascii_lowercase();
            PORT_TAKEN
                .iter()
                .chain(LEFTOVERS.iter())
                .any(|m| lower.contains(m))
        };
        let mut log = self.log_tail();
        for _ in 0..20 {
            if known(&log) {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
            log = self.log_tail();
        }
        log
    }

    /// Tail of the newest server logs, for error reports.
    fn log_tail(&self) -> String {
        let mut text = fs::read_to_string(self.startup_log()).unwrap_or_default();
        let newest = fs::read_dir(self.data_dir.join("log"))
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
            .max_by_key(|(t, _)| *t);
        if let Some((_, path)) = newest {
            text.push('\n');
            text.push_str(&fs::read_to_string(path).unwrap_or_default());
        }
        proc::tail(&text, Some(&self.password))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overrides_bind_loopback_only_and_use_the_port() {
        let t = overrides_text(54321);
        assert!(t.contains("listen_addresses = '127.0.0.1'"));
        assert!(t.contains("port = 54321"));
        assert!(t.contains("unix_socket_directories = ''"));
        assert!(!t.contains('*'));
    }

    #[test]
    fn server_pins_fix_the_security_settings_on_the_command_line() {
        let dir = std::path::PathBuf::from(if cfg!(windows) {
            "C:/Users/Some One/pgdata"
        } else {
            "/home/some one/pgdata"
        });
        let pins = server_pins(&dir, 5432);
        for want in [
            "-c listen_addresses=127.0.0.1",
            "-c port=5432",
            "-c unix_socket_directories= ",
            "-c shared_preload_libraries= ",
            "-c archive_mode=off",
            "-c ssl=off",
            "-c log_statement=none",
            "pgdata/pg_hba.conf\"",
            "pgdata/pg_ident.conf\"",
            "-c \"hba_file=",
            "-c \"ident_file=",
        ] {
            assert!(pins.contains(want), "{want} missing from {pins}");
        }
    }

    #[test]
    fn connection_url_hides_its_password_in_debug_output() {
        let info = ConnectionInfo {
            host: "127.0.0.1",
            port: 5555,
            user: "postgres",
            password: Secret::new("abc"),
        };
        let url = info.url("books");
        assert_eq!(url.expose(), "postgres://postgres:abc@127.0.0.1:5555/books");
        assert!(!format!("{info:?}{url:?}").contains("abc"));
    }
}
