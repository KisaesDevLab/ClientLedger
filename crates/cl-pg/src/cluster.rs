//! The database cluster: creation, configuration, start, and stop.

use crate::proc;
use crate::{PgBinaries, PgError, Result, Secret};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Line appended to `postgresql.conf` once, at creation.
pub(crate) const INCLUDE_LINE: &str = "include_if_exists = 'clientledger.conf'";

/// Locale names to try, best first. `C.UTF-8` gives byte-order sorting with
/// UTF-8 aware character handling where the OS has it; plain `C` is the
/// portable fallback (Q-P03b).
const LOCALES: [&str; 2] = ["C.UTF-8", "C"];

/// The only login rules: password (SCRAM) over loopback TCP, the superuser
/// only to the `postgres` maintenance database and the application role only
/// to its own database. The superuser can never open the books.
pub(crate) const HBA: &str = "\
# Managed by ClientLedger. Rewritten on every start; do not edit.
host    postgres          postgres            127.0.0.1/32    scram-sha-256
host    clientledger      clientledger_app    127.0.0.1/32    scram-sha-256
";

/// A `PostgreSQL` cluster `ClientLedger` owns.
#[derive(Debug, Clone)]
pub struct Cluster {
    pub(crate) data_dir: PathBuf,
    pub(crate) bins: PgBinaries,
    pub(crate) password: Secret,
}

impl Cluster {
    /// Open the cluster at `data_dir`, creating it on first use.
    ///
    /// A new cluster gets a random 256-bit superuser password saved through
    /// `store` before the cluster is created. An existing cluster must already
    /// have its password in `store`.
    ///
    /// # Errors
    /// Fails when the folder is unsuitable, the password is missing, or
    /// `initdb` fails.
    pub fn ensure(
        data_dir: &Path,
        bins: PgBinaries,
        store: &dyn crate::PasswordStore,
    ) -> Result<Cluster> {
        let data_dir = absolute(data_dir)?;
        crate::check_data_dir(&data_dir).map_err(|issue| PgError::DataDir { issue })?;
        if is_initialized(&data_dir) {
            let password = store.get()?.ok_or_else(|| PgError::PasswordMissing {
                data_dir: data_dir.clone(),
            })?;
            let cluster = Cluster {
                data_dir,
                bins,
                password,
            };
            cluster.write_static_config()?;
            return Ok(cluster);
        }

        let password = Secret::generate()?;
        store.set(&password)?;
        let cluster = Cluster {
            data_dir,
            bins,
            password,
        };
        cluster.init()?;
        cluster.write_static_config()?;
        tracing::info!(store = store.kind(), "created database cluster");
        Ok(cluster)
    }

    /// The data directory.
    #[must_use]
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// The binaries in use.
    #[must_use]
    pub fn binaries(&self) -> &PgBinaries {
        &self.bins
    }

    fn init(&self) -> Result<()> {
        let dir = &self.data_dir;
        if dir.exists() {
            let empty = fs::read_dir(dir)
                .map_err(|e| PgError::io("read data folder", e))?
                .next()
                .is_none();
            if !empty {
                return Err(PgError::Io {
                    context: format!("create database in {}", dir.display()),
                    source: std::io::Error::new(
                        std::io::ErrorKind::AlreadyExists,
                        "the folder is not empty and is not a ClientLedger database",
                    ),
                });
            }
        }
        let parent = dir
            .parent()
            .ok_or_else(|| PgError::io("data folder", std::io::Error::other("no parent folder")))?;
        fs::create_dir_all(parent).map_err(|e| PgError::io("create data folder parent", e))?;

        remove_stale_password_files(parent);
        let pwfile = PasswordFile::create(parent, &self.password)?;
        let mut last: Option<PgError> = None;
        for locale in LOCALES {
            let mut cmd = proc::command(&self.bins.program("initdb"));
            cmd.arg("-D")
                .arg(dir)
                .args(["-U", "postgres", "-E", "UTF8"])
                .arg(format!("--locale={locale}"))
                .args(["--auth-host=scram-sha-256", "--auth-local=scram-sha-256"])
                .arg(format!("--pwfile={}", pwfile.path().display()))
                .arg("--no-instructions");
            match proc::run(cmd, "initdb", Some(&self.password)) {
                Ok(_) => return Ok(()),
                Err(e) => {
                    let locale_problem = matches!(
                        &e,
                        PgError::Command { detail, .. } if detail.to_ascii_lowercase().contains("locale")
                    );
                    // initdb removes what it created; make sure nothing partial stays
                    // (the folder was empty or missing when we started).
                    let _ = fs::remove_dir_all(dir);
                    if !locale_problem {
                        return Err(e);
                    }
                    last = Some(e);
                }
            }
        }
        Err(last.unwrap_or_else(|| PgError::io("initdb", std::io::Error::other("no locale"))))
    }

    /// Rewrite the files `ClientLedger` owns: the login rules and the include
    /// line. Runs on every `ensure`, so a hand-edited `pg_hba.conf` cannot
    /// open the server to other machines.
    pub(crate) fn write_static_config(&self) -> Result<()> {
        write_file(&self.data_dir.join("pg_hba.conf"), HBA.as_bytes())?;
        let conf = self.data_dir.join("postgresql.conf");
        let text = fs::read_to_string(&conf).map_err(|e| PgError::io("read postgresql.conf", e))?;
        if !text.lines().any(|l| l.trim() == INCLUDE_LINE) {
            let mut f = fs::OpenOptions::new()
                .append(true)
                .open(&conf)
                .map_err(|e| PgError::io("update postgresql.conf", e))?;
            writeln!(f, "\n# ClientLedger settings (always last)\n{INCLUDE_LINE}")
                .map_err(|e| PgError::io("update postgresql.conf", e))?;
        }
        Ok(())
    }
}

/// True when `dir` holds an initialized cluster.
#[must_use]
pub fn is_initialized(dir: &Path) -> bool {
    dir.join("PG_VERSION").is_file()
}

fn absolute(p: &Path) -> Result<PathBuf> {
    std::path::absolute(p).map_err(|e| PgError::io("resolve data folder", e))
}

pub(crate) fn write_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp-cl");
    fs::write(&tmp, bytes).map_err(|e| PgError::io(format!("write {}", path.display()), e))?;
    fs::rename(&tmp, path).map_err(|e| PgError::io(format!("write {}", path.display()), e))
}

/// Delete `.clpg-init-*` files a crashed earlier run left behind. They hold a
/// password that was never used for a finished cluster, but they should not
/// linger.
fn remove_stale_password_files(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        if e.file_name().to_string_lossy().starts_with(INIT_PREFIX) {
            let _ = fs::remove_file(e.path());
        }
    }
}

/// Name prefix of the temporary `initdb --pwfile` file.
const INIT_PREFIX: &str = ".clpg-init-";

/// A temporary file holding the password for `initdb --pwfile`, removed on drop.
struct PasswordFile {
    path: PathBuf,
}

impl PasswordFile {
    fn create(dir: &Path, password: &Secret) -> Result<Self> {
        let mut tag = [0u8; 6];
        getrandom::fill(&mut tag).map_err(|e| PgError::Random(e.to_string()))?;
        let path = dir.join(format!("{INIT_PREFIX}{}", crate::secret::hex(&tag)));
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts
            .open(&path)
            .map_err(|e| PgError::io("create temporary password file", e))?;
        let file = PasswordFile { path };
        writeln!(f, "{}", password.expose())
            .map_err(|e| PgError::io("write temporary password file", e))?;
        Ok(file)
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for PasswordFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hba_has_two_narrow_scram_loopback_rules() {
        let rules: Vec<&str> = HBA.lines().filter(|l| !l.starts_with('#')).collect();
        assert_eq!(rules.len(), 2);
        for r in &rules {
            assert!(r.contains("127.0.0.1/32"));
            assert!(r.contains("scram-sha-256"));
            assert!(!r.contains(" all "), "{r}");
        }
        assert!(rules[0].contains("postgres") && !rules[0].contains("clientledger"));
        assert!(rules[1].contains("clientledger_app"));
        assert!(!HBA.contains("trust"));
    }

    #[test]
    fn stale_password_files_are_swept() {
        let dir = std::env::temp_dir().join(format!("cl-pg-sweep-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(".clpg-init-abc123"), b"old").unwrap();
        fs::write(dir.join("keep.txt"), b"mine").unwrap();
        remove_stale_password_files(&dir);
        assert!(!dir.join(".clpg-init-abc123").exists());
        assert!(dir.join("keep.txt").exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn password_file_is_removed_on_drop() {
        let dir = std::env::temp_dir().join(format!("cl-pg-pwf-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path;
        {
            let f = PasswordFile::create(&dir, &Secret::new("pw")).unwrap();
            path = f.path().to_path_buf();
            assert!(path.exists());
        }
        assert!(!path.exists());
        let _ = fs::remove_dir_all(dir);
    }
}
