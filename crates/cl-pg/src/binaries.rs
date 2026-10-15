//! Locating the bundled `PostgreSQL` programs.

use crate::{PgError, Result};
use std::path::{Path, PathBuf};

/// The programs `cl-pg` runs. All three ship in the pinned archive.
const REQUIRED: [&str; 3] = ["postgres", "initdb", "pg_ctl"];

/// A `PostgreSQL` install: `<root>/bin`, `<root>/lib`, `<root>/share`.
#[derive(Debug, Clone)]
pub struct PgBinaries {
    root: PathBuf,
}

/// Directory name used under `target/pg/` for this machine (matches xtask).
#[must_use]
pub fn host_dir_name() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => Some("windows-x64"),
        ("macos", "aarch64") => Some("macos-arm64"),
        ("macos", "x86_64") => Some("macos-x64"),
        ("linux", "x86_64") => Some("linux-x64"),
        _ => None,
    }
}

fn exe(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

impl PgBinaries {
    /// Use the install at `root`.
    ///
    /// # Errors
    /// Fails when `postgres`, `initdb`, or `pg_ctl` is missing.
    pub fn from_root(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        for name in REQUIRED {
            if !root.join("bin").join(exe(name)).is_file() {
                return Err(PgError::BinariesIncomplete {
                    root,
                    missing: exe(name),
                });
            }
        }
        Ok(PgBinaries { root })
    }

    /// Find the install without being told where it is.
    ///
    /// Order: `explicit`, the `CL_PG_ROOT` environment variable, `pg/` next to
    /// the running program (installed layout), then `target/pg/<host>` in the
    /// repository (development and tests), searching upward from the program
    /// and from the current directory.
    ///
    /// # Errors
    /// [`PgError::BinariesNotFound`] lists everything that was checked.
    pub fn discover(explicit: Option<&Path>) -> Result<Self> {
        let mut searched: Vec<PathBuf> = Vec::new();
        let mut candidates: Vec<PathBuf> = Vec::new();
        if let Some(p) = explicit {
            candidates.push(p.to_path_buf());
        }
        if let Some(p) = std::env::var_os("CL_PG_ROOT") {
            candidates.push(PathBuf::from(p));
        }
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(Path::to_path_buf));
        if let Some(dir) = &exe_dir {
            candidates.push(dir.join("pg"));
        }
        if let Some(host) = host_dir_name() {
            let starts = exe_dir.into_iter().chain(std::env::current_dir().ok());
            for start in starts {
                for dir in start.ancestors() {
                    candidates.push(dir.join("target").join("pg").join(host));
                }
            }
        }
        for c in candidates {
            if searched.contains(&c) {
                continue;
            }
            match Self::from_root(&c) {
                Ok(found) => return Ok(found),
                Err(_) => searched.push(c),
            }
        }
        Err(PgError::BinariesNotFound { searched })
    }

    /// The install root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Full path of a program in `bin/`, with `.exe` added on Windows.
    #[must_use]
    pub fn program(&self, name: &str) -> PathBuf {
        self.root.join("bin").join(exe(name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn fake_install(dir: &Path, skip: Option<&str>) {
        fs::create_dir_all(dir.join("bin")).unwrap();
        for n in REQUIRED {
            if Some(n) != skip {
                fs::write(dir.join("bin").join(exe(n)), b"x").unwrap();
            }
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("cl-pg-bin-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn complete_install_is_accepted() {
        let d = scratch("ok");
        fake_install(&d, None);
        let b = PgBinaries::from_root(&d).unwrap();
        assert_eq!(b.program("initdb"), d.join("bin").join(exe("initdb")));
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn missing_program_is_reported_by_name() {
        let d = scratch("missing");
        fake_install(&d, Some("pg_ctl"));
        let err = PgBinaries::from_root(&d).unwrap_err();
        assert!(err.to_string().contains(&exe("pg_ctl")), "{err}");
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn explicit_path_wins_in_discovery() {
        let d = scratch("explicit");
        fake_install(&d, None);
        let b = PgBinaries::discover(Some(&d)).unwrap();
        assert_eq!(b.root(), d);
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn host_dir_name_is_known_on_supported_hosts() {
        if cfg!(any(windows, target_os = "macos", target_os = "linux")) {
            assert!(host_dir_name().is_some());
        }
    }
}
