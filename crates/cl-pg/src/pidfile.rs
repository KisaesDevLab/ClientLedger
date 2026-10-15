//! `postmaster.pid` and process identity checks.

use crate::proc;
use std::path::Path;

/// The parts of `postmaster.pid` we use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PidFile {
    pub pid: u32,
    /// The data directory the server says it runs in.
    pub data_dir: String,
    pub port: Option<u16>,
    /// Directory of the Unix socket; empty when sockets are off.
    pub socket_dir: String,
    /// The `listen_addresses` the server actually bound.
    pub listen_addr: String,
}

/// Parse the file text: pid, data dir, start time, port, socket dir, listen
/// address, shared memory key, status.
pub(crate) fn parse(text: &str) -> Option<PidFile> {
    let mut lines = text.lines();
    let pid = lines.next()?.trim().parse().ok()?;
    let data_dir = lines.next()?.trim().to_string();
    let _started = lines.next()?;
    let port = lines.next()?.trim().parse().ok();
    let socket_dir = lines.next().unwrap_or("").trim().to_string();
    let listen_addr = lines.next().unwrap_or("").trim().to_string();
    Some(PidFile {
        pid,
        data_dir,
        port,
        socket_dir,
        listen_addr,
    })
}

/// Read `postmaster.pid` from a data directory.
pub(crate) fn read(data_dir: &Path) -> Option<PidFile> {
    parse(&std::fs::read_to_string(data_dir.join("postmaster.pid")).ok()?)
}

/// Does the data directory a pid file names match `ours`? Slash direction,
/// case on Windows, and a trailing slash do not matter; symbolic links and
/// `\\?\\` prefixes are resolved when the folders exist.
pub(crate) fn same_dir(named: &str, ours: &Path) -> bool {
    fn norm(text: &str) -> String {
        let t = text.replace('\\', "/");
        let t = t.strip_prefix("//?/").unwrap_or(&t).trim_end_matches('/');
        if cfg!(windows) {
            t.to_lowercase()
        } else {
            t.to_string()
        }
    }
    let named = named.trim();
    if named.is_empty() {
        return false;
    }
    if norm(named) == norm(&ours.to_string_lossy()) {
        return true;
    }
    match (Path::new(named).canonicalize(), ours.canonicalize()) {
        (Ok(a), Ok(b)) => norm(&a.to_string_lossy()) == norm(&b.to_string_lossy()),
        _ => false,
    }
}

/// First column of a `tasklist /FO CSV /NH` row, lowercased.
fn tasklist_name(output: &str) -> Option<String> {
    let line = output.lines().find(|l| l.trim_start().starts_with('"'))?;
    let first = line.trim_start().strip_prefix('"')?.split('"').next()?;
    Some(first.to_lowercase())
}

/// Is process `pid` a `PostgreSQL` server process?
///
/// `Some(true)`: yes. `Some(false)`: a different program has that pid (the
/// pid was reused after a crash or reboot). `None`: no such process or we
/// could not tell.
pub(crate) fn process_is_postgres(pid: u32) -> Option<bool> {
    let name = if cfg!(windows) {
        let mut cmd = proc::command(Path::new("tasklist"));
        cmd.args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"]);
        let f = proc::finish(cmd, "tasklist", None).ok()?;
        if !f.success {
            return None;
        }
        tasklist_name(&f.output)?
    } else {
        let mut cmd = proc::command(Path::new("ps"));
        cmd.args(["-p", &pid.to_string(), "-o", "comm="]);
        let f = proc::finish(cmd, "ps", None).ok()?;
        let name = f.output.trim().to_lowercase();
        if !f.success || name.is_empty() {
            return None;
        }
        name
    };
    Some(name.contains("postgres") || name.contains("postmaster"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str =
        "4242\nC:/data/pgdata\n1760000000\n54321\n\n127.0.0.1\n  1234 5678\nready\n";

    #[test]
    fn parses_the_pid_file() {
        let p = parse(SAMPLE).unwrap();
        assert_eq!(p.pid, 4242);
        assert_eq!(p.port, Some(54321));
        assert_eq!(p.socket_dir, "");
        assert_eq!(p.listen_addr, "127.0.0.1");
    }

    #[test]
    fn data_directory_names_are_compared_loosely_but_not_wrongly() {
        assert!(same_dir("/a/b/pgdata", Path::new("/a/b/pgdata")));
        assert!(same_dir("/a/b/pgdata/", Path::new("/a/b/pgdata")));
        assert!(!same_dir("/a/b/other", Path::new("/a/b/pgdata")));
        assert!(!same_dir("", Path::new("/a/b/pgdata")));
        if cfg!(windows) {
            assert!(same_dir(
                "c:/Users/X/pgdata",
                Path::new("C:\\Users\\x\\pgdata")
            ));
            assert!(same_dir(
                "//?/C:/Users/X/pgdata",
                Path::new("C:\\Users\\X\\pgdata")
            ));
        }
    }

    #[test]
    fn the_data_directory_is_read_from_the_pid_file() {
        assert_eq!(parse(SAMPLE).unwrap().data_dir, "C:/data/pgdata");
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(parse(""), None);
        assert_eq!(parse("not a pid\nx\ny\n"), None);
        assert_eq!(parse("12\n"), None);
    }

    #[test]
    fn tasklist_rows_are_read() {
        let row = "\"postgres.exe\",\"4242\",\"Console\",\"1\",\"21,000 K\"\r\n";
        assert_eq!(tasklist_name(row), Some("postgres.exe".into()));
        let none = "INFO: No tasks are running which match the specified criteria.\r\n";
        assert_eq!(tasklist_name(none), None);
    }

    #[test]
    fn this_test_process_is_not_postgres_and_a_dead_pid_is_unknown() {
        assert_eq!(process_is_postgres(std::process::id()), Some(false));
        // pid values this large are not issued on any supported OS.
        assert_eq!(process_is_postgres(4_000_000_000), None);
    }
}
