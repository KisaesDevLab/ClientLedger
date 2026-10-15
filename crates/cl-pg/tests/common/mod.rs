//! Shared helpers for the integration tests.
//!
//! The tests use the binaries unpacked by `cargo xtask pg-fetch` and never
//! touch the network. Without them a test prints a skip notice and passes,
//! unless `CL_PG_REQUIRE=1` (set in CI), which turns the skip into a failure.
#![allow(dead_code)]

use cl_pg::PgBinaries;
use std::path::{Path, PathBuf};

/// The fetched binaries, or `None` (skip) when they are absent and not required.
pub fn bins() -> Option<PgBinaries> {
    match PgBinaries::discover(None) {
        Ok(b) => Some(b),
        Err(e) => {
            assert!(
                std::env::var("CL_PG_REQUIRE").as_deref() != Ok("1"),
                "CL_PG_REQUIRE=1 but PostgreSQL binaries are missing: {e}"
            );
            eprintln!("SKIPPED: {e} (run `cargo xtask pg-fetch`; CI sets CL_PG_REQUIRE=1)");
            None
        }
    }
}

/// A scratch folder removed on drop.
pub struct Scratch {
    pub root: PathBuf,
}

impl Scratch {
    pub fn new(name: &str) -> Scratch {
        let root = std::env::temp_dir().join(format!("cl-pg-it-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        Scratch { root }
    }

    pub fn data_dir(&self) -> PathBuf {
        self.root.join("pgdata")
    }

    pub fn path(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if std::env::var("CL_PG_KEEP").as_deref() == Ok("1") {
            eprintln!("kept {}", self.root.display());
            return;
        }
        // Windows can hold files for a moment after a server exits.
        for _ in 0..10 {
            if std::fs::remove_dir_all(&self.root).is_ok() || !self.root.exists() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(300));
        }
    }
}

/// All regular files under `dir`, recursively.
pub fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                stack.push(p);
            } else {
                out.push(p);
            }
        }
    }
    out
}

/// Connect as the superuser with the cluster's own password.
pub fn connect(cluster: &cl_pg::Cluster, port: u16, db: &str) -> postgres::Client {
    let info = cluster.connection(port);
    connect_with(port, db, info.password.expose()).expect("connect with the real password")
}

pub fn connect_with(
    port: u16,
    db: &str,
    password: &str,
) -> Result<postgres::Client, postgres::Error> {
    let mut cfg = postgres::Config::new();
    cfg.host("127.0.0.1")
        .port(port)
        .user("postgres")
        .password(password)
        .dbname(db)
        .connect_timeout(std::time::Duration::from_secs(10));
    cfg.connect(postgres::NoTls)
}

/// Hard-kill a process (`SIGKILL` or `TerminateProcess`), like a crash or power loss.
pub fn kill_hard(pid: u32) {
    let status = if cfg!(windows) {
        std::process::Command::new("taskkill")
            .args(["/F", "/PID", &pid.to_string()])
            .output()
    } else {
        std::process::Command::new("kill")
            .args(["-9", &pid.to_string()])
            .output()
    };
    assert!(
        status.is_ok_and(|o| o.status.success()),
        "could not kill {pid}"
    );
    // Wait for the process to disappear.
    for _ in 0..50 {
        if !process_alive(pid) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    panic!("process {pid} did not exit");
}

fn process_alive(pid: u32) -> bool {
    if cfg!(windows) {
        std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
            .output()
            .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).contains(&format!("\"{pid}\"")))
    } else {
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .output()
            .is_ok_and(|o| o.status.success())
    }
}

/// Every listening socket on `port` is on 127.0.0.1 (uses `netstat`; skipped
/// with a notice when the tool is missing).
pub fn assert_only_loopback(port: u16) {
    let Ok(out) = std::process::Command::new("netstat").arg("-an").output() else {
        eprintln!("netstat not available; skipped the socket table check");
        return;
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let mut seen = 0;
    for line in text.lines().filter(|l| l.to_uppercase().contains("LISTEN")) {
        let Some(local) = line
            .split_whitespace()
            .find(|t| t.ends_with(&format!(":{port}")) || t.ends_with(&format!(".{port}")))
        else {
            continue;
        };
        seen += 1;
        assert!(
            local.starts_with("127.0.0.1"),
            "listening on {local}: {line}"
        );
    }
    assert!(
        seen >= 1,
        "no listening socket found for port {port}:\n{text}"
    );
}

/// Row count and an order-independent checksum for every table in the
/// `public` schema of `db`, keyed by `db.table`.
pub fn table_checksums(
    cluster: &cl_pg::Cluster,
    port: u16,
    db: &str,
) -> std::collections::BTreeMap<String, (i64, String)> {
    let mut client = connect(cluster, port, db);
    let tables: Vec<String> = client
        .query(
            "select table_name::text from information_schema.tables \
             where table_schema = 'public' and table_type = 'BASE TABLE' order by 1",
            &[],
        )
        .unwrap()
        .iter()
        .map(|r| r.get(0))
        .collect();
    let mut out = std::collections::BTreeMap::new();
    for t in tables {
        let sql = format!(
            "select count(*), coalesce(md5(string_agg(x::text, '|' order by x::text)), '') from \"{t}\" x"
        );
        let row = client.query_one(&sql, &[]).unwrap();
        out.insert(
            format!("{db}.{t}"),
            (row.get::<_, i64>(0), row.get::<_, String>(1)),
        );
    }
    out
}
