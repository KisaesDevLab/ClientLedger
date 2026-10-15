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
