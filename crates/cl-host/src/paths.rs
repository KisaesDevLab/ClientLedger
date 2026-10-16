//! Where `ClientLedger` keeps its files.
//!
//! * Windows: `%LOCALAPPDATA%\ClientLedger`
//! * macOS: `~/Library/Application Support/ClientLedger`
//! * Linux and others: `$XDG_DATA_HOME/clientledger`, else `~/.local/share/clientledger`
//!
//! Every path handed to a child process is absolute (Q-P04b).

use crate::{HostError, Result};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Operating system families that differ in where data lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    /// Windows.
    Windows,
    /// macOS.
    MacOs,
    /// Linux and other Unix systems (XDG layout).
    Unix,
}

impl Os {
    /// The operating system this program was built for.
    #[must_use]
    pub fn current() -> Os {
        if cfg!(windows) {
            Os::Windows
        } else if cfg!(target_os = "macos") {
            Os::MacOs
        } else {
            Os::Unix
        }
    }
}

/// The folders `ClientLedger` uses. All paths are absolute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppPaths {
    root: PathBuf,
}

/// `Ok(None)` when unset or empty, `Err` when set but not absolute.
fn absolute_env(
    env: &dyn Fn(&str) -> Option<OsString>,
    key: &str,
) -> std::result::Result<Option<PathBuf>, ()> {
    match env(key).filter(|v| !v.is_empty()) {
        None => Ok(None),
        Some(v) => {
            let p = PathBuf::from(v);
            if p.is_absolute() {
                Ok(Some(p))
            } else {
                Err(())
            }
        }
    }
}

impl AppPaths {
    /// The data folder of the signed-in user.
    ///
    /// # Errors
    /// Fails when the operating system does not provide a home or data folder.
    pub fn for_current_user() -> Result<AppPaths> {
        Self::resolve(Os::current(), &|k| std::env::var_os(k))
    }

    /// [`AppPaths::for_current_user`] with the operating system and its
    /// environment given explicitly (tests use this).
    ///
    /// # Errors
    /// Fails when the needed variables are missing or not absolute paths.
    pub fn resolve(os: Os, env: &dyn Fn(&str) -> Option<OsString>) -> Result<AppPaths> {
        let home = || {
            absolute_env(
                env,
                if os == Os::Windows {
                    "USERPROFILE"
                } else {
                    "HOME"
                },
            )
            .ok()
            .flatten()
            .ok_or_else(|| HostError::NoDataFolder("the home folder is not set".into()))
        };
        let root = match os {
            Os::Windows => {
                let base = match absolute_env(env, "LOCALAPPDATA") {
                    Ok(Some(p)) => p,
                    Ok(None) => home()?.join("AppData").join("Local"),
                    Err(()) => {
                        return Err(HostError::NoDataFolder(
                            "LOCALAPPDATA is not an absolute path".into(),
                        ));
                    }
                };
                base.join("ClientLedger")
            }
            Os::MacOs => home()?
                .join("Library")
                .join("Application Support")
                .join("ClientLedger"),
            Os::Unix => {
                // The XDG specification says a relative XDG_DATA_HOME is ignored.
                let base = match absolute_env(env, "XDG_DATA_HOME") {
                    Ok(Some(p)) => p,
                    _ => home()?.join(".local").join("share"),
                };
                base.join("clientledger")
            }
        };
        AppPaths::at(root)
    }

    /// Use `root` as the app data folder (tests and portable setups).
    ///
    /// # Errors
    /// Fails when `root` is not absolute.
    pub fn at(root: impl Into<PathBuf>) -> Result<AppPaths> {
        let root = root.into();
        if !root.is_absolute() {
            return Err(HostError::NotAbsolute {
                what: "the app data folder",
                path: root,
            });
        }
        Ok(AppPaths { root })
    }

    /// The app data folder.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The `PostgreSQL` data directory. Its secrets file and startup log sit
    /// beside it, never inside (Q-P03).
    #[must_use]
    pub fn pg_data(&self) -> PathBuf {
        self.root.join("pgdata")
    }

    /// The engine's data directory (`CL_DATA_DIR`).
    #[must_use]
    pub fn engine_data(&self) -> PathBuf {
        self.root.join("engine")
    }

    /// Where the engine unpacks its embedded assets (`CL_ENGINE_RUNTIME_DIR`).
    #[must_use]
    pub fn engine_runtime(&self) -> PathBuf {
        self.root.join("engine-runtime")
    }

    /// The engine's private temporary folder (`TEMP`/`TMPDIR`).
    #[must_use]
    pub fn engine_tmp(&self) -> PathBuf {
        self.root.join("engine-tmp")
    }

    /// A private stand-in for the home folder, so the engine never reads the
    /// owner's own dotfiles.
    #[must_use]
    pub fn engine_home(&self) -> PathBuf {
        self.root.join("engine-home")
    }

    /// Log files (host, engine; rotated).
    #[must_use]
    pub fn logs_dir(&self) -> PathBuf {
        self.root.join("logs")
    }

    /// The host's own log.
    #[must_use]
    pub fn host_log(&self) -> PathBuf {
        self.logs_dir().join("host.log")
    }

    /// The engine's log (`CL_LOG_FILE`).
    #[must_use]
    pub fn engine_log(&self) -> PathBuf {
        self.logs_dir().join("engine.log")
    }

    /// Runtime state: instance lock, control endpoint, child records.
    #[must_use]
    pub fn run_dir(&self) -> PathBuf {
        self.root.join("run")
    }

    /// Create the folders (owner-only permissions on Unix).
    ///
    /// # Errors
    /// Fails when a folder cannot be created.
    pub fn ensure_dirs(&self) -> Result<()> {
        for dir in [
            self.root.clone(),
            self.engine_data(),
            self.engine_runtime(),
            self.engine_tmp(),
            self.engine_home(),
            self.logs_dir(),
            self.run_dir(),
        ] {
            create_private_dir(&dir)?;
        }
        Ok(())
    }
}

/// `create_dir_all`, then (Unix) mode 0700 on the last component.
pub(crate) fn create_private_dir(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir)
        .map_err(|e| HostError::io(format!("create folder {}", dir.display()), e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| HostError::io(format!("protect folder {}", dir.display()), e))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let map: HashMap<String, OsString> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), OsString::from(v)))
            .collect();
        move |k| map.get(k).cloned()
    }

    /// An absolute path on whichever OS the test runs on.
    fn abs(unix: &str, win: &str) -> String {
        if cfg!(windows) { win } else { unix }.to_string()
    }

    #[test]
    fn windows_uses_local_app_data() {
        let local = abs("/x/Local", r"C:\Users\Ann\AppData\Local");
        let p = AppPaths::resolve(Os::Windows, &env(&[("LOCALAPPDATA", &local)])).unwrap();
        assert_eq!(p.root(), Path::new(&local).join("ClientLedger"));
    }

    #[test]
    fn windows_falls_back_to_the_profile_folder() {
        let home = abs("/h/ann", r"C:\Users\Ann");
        let p = AppPaths::resolve(Os::Windows, &env(&[("USERPROFILE", &home)])).unwrap();
        assert_eq!(
            p.root(),
            Path::new(&home)
                .join("AppData")
                .join("Local")
                .join("ClientLedger")
        );
    }

    #[test]
    fn windows_refuses_a_relative_local_app_data() {
        let home = abs("/h/ann", r"C:\Users\Ann");
        let r = AppPaths::resolve(
            Os::Windows,
            &env(&[("LOCALAPPDATA", "relative"), ("USERPROFILE", &home)]),
        );
        assert!(matches!(r, Err(HostError::NoDataFolder(_))), "{r:?}");
    }

    #[test]
    fn macos_uses_application_support() {
        let home = abs("/Users/ann", r"C:\Users\ann");
        let p = AppPaths::resolve(Os::MacOs, &env(&[("HOME", &home)])).unwrap();
        assert_eq!(
            p.root(),
            Path::new(&home)
                .join("Library")
                .join("Application Support")
                .join("ClientLedger")
        );
    }

    #[test]
    fn linux_prefers_xdg_data_home() {
        let xdg = abs("/data/xdg", r"C:\xdg");
        let home = abs("/home/ann", r"C:\home\ann");
        let p =
            AppPaths::resolve(Os::Unix, &env(&[("XDG_DATA_HOME", &xdg), ("HOME", &home)])).unwrap();
        assert_eq!(p.root(), Path::new(&xdg).join("clientledger"));
    }

    #[test]
    fn linux_ignores_a_relative_xdg_data_home() {
        let home = abs("/home/ann", r"C:\home\ann");
        let p = AppPaths::resolve(
            Os::Unix,
            &env(&[("XDG_DATA_HOME", "rel/path"), ("HOME", &home)]),
        )
        .unwrap();
        assert_eq!(
            p.root(),
            Path::new(&home)
                .join(".local")
                .join("share")
                .join("clientledger")
        );
    }

    #[test]
    fn missing_home_is_an_error() {
        let r = AppPaths::resolve(Os::Unix, &env(&[]));
        assert!(matches!(r, Err(HostError::NoDataFolder(_))), "{r:?}");
    }

    #[test]
    fn a_relative_root_is_refused() {
        let r = AppPaths::at("some/where");
        assert!(matches!(r, Err(HostError::NotAbsolute { .. })), "{r:?}");
    }

    #[test]
    fn every_derived_path_is_absolute_and_inside_the_root() {
        let root = std::env::temp_dir().join("cl-host-paths-test");
        let p = AppPaths::at(&root).unwrap();
        for path in [
            p.pg_data(),
            p.engine_data(),
            p.engine_runtime(),
            p.engine_tmp(),
            p.engine_home(),
            p.logs_dir(),
            p.host_log(),
            p.engine_log(),
            p.run_dir(),
        ] {
            assert!(path.is_absolute(), "{}", path.display());
            assert!(path.starts_with(&root), "{}", path.display());
        }
    }

    #[test]
    fn ensure_dirs_creates_the_folders() {
        let root = std::env::temp_dir().join(format!("cl-host-ensure-{}", std::process::id()));
        let p = AppPaths::at(&root).unwrap();
        p.ensure_dirs().unwrap();
        assert!(p.logs_dir().is_dir() && p.run_dir().is_dir() && p.engine_data().is_dir());
        p.ensure_dirs().unwrap(); // idempotent
        let _ = std::fs::remove_dir_all(root);
    }
}
