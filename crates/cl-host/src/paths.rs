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

    /// Scratch folders for document reading (A03): each reading worker's
    /// empty working folder and tesseract's per-page temporary folder,
    /// created owner-only below this and removed after use.
    #[must_use]
    pub fn docread_dir(&self) -> PathBuf {
        self.root.join("docread")
    }

    /// PDFs handed to the OS to print or view (P19, Q-P19a); old files are
    /// swept by `os_print::sweep_print_dir`.
    #[must_use]
    pub fn print_dir(&self) -> PathBuf {
        self.root.join("print")
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

/// File name of the engine program on this operating system.
#[must_use]
pub fn engine_exe_name() -> &'static str {
    if cfg!(windows) {
        "clientledger-engine.exe"
    } else {
        "clientledger-engine"
    }
}

/// `target/engine/<name>` directory name for this machine (matches xtask).
fn engine_target_dir() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => Some("windows-x64"),
        ("macos", "aarch64") => Some("macos-arm64"),
        ("macos", "x86_64") => Some("macos-x64"),
        ("linux", "x86_64") => Some("linux-x64"),
        _ => None,
    }
}

/// Find the engine executable.
///
/// An `explicit` path is used alone. Otherwise, in this order: the
/// `CL_ENGINE_EXE` variable, next to the running program (installed layout,
/// plain or in `engine/`), and, in debug builds only, the `target/engine/<host>/`
/// folder of the source checkout the program itself was built in. The current
/// folder and arbitrary parent folders are never searched: a program planted in
/// a folder other users can create (`C:\`, `/tmp`) would otherwise run as the
/// owner with the database password and keys (security review M1).
///
/// On Unix a candidate (and its folder) must be owned by the user or root and
/// not writable by anyone else. The result is always absolute.
#[must_use]
pub fn find_engine_exe(explicit: Option<&Path>) -> Option<PathBuf> {
    if let Some(p) = explicit {
        // A path the caller named is used as is, never replaced by a guess.
        return vet_engine_exe(p);
    }
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf));
    let env = std::env::var_os("CL_ENGINE_EXE")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from);
    engine_candidates(env, exe_dir.as_deref(), cfg!(debug_assertions))
        .into_iter()
        .find_map(|c| vet_engine_exe(&c))
}

/// The places [`find_engine_exe`] looks, in order. `include_dev` adds the
/// source-checkout location (debug builds only).
fn engine_candidates(
    env: Option<PathBuf>,
    exe_dir: Option<&Path>,
    include_dev: bool,
) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = env.into_iter().collect();
    if let Some(dir) = exe_dir {
        out.push(dir.join(engine_exe_name()));
        out.push(dir.join("engine").join(engine_exe_name()));
        if include_dev && let Some(target) = engine_target_dir() {
            // Only the cargo `target` folder that contains this very program.
            if let Some(cargo_target) = dir
                .ancestors()
                .find(|d| d.file_name().is_some_and(|n| n == "target"))
            {
                out.push(
                    cargo_target
                        .join("engine")
                        .join(target)
                        .join(engine_exe_name()),
                );
            }
        }
    }
    out
}

/// File name of the document reading worker on this operating system.
#[must_use]
pub fn docread_exe_name() -> &'static str {
    if cfg!(windows) {
        "clientledger-docread.exe"
    } else {
        "clientledger-docread"
    }
}

/// Find the document reading worker (A03): an `explicit` path alone, else
/// `CL_DOCREAD_EXE` (absolute), else next to the running program (the
/// installed layout, and `target/<profile>/` in a source checkout, where
/// cargo builds every app side by side). Vetted like the engine.
#[must_use]
pub fn find_docread_exe(explicit: Option<&Path>) -> Option<PathBuf> {
    if let Some(p) = explicit {
        return vet_engine_exe(p);
    }
    let env = std::env::var_os("CL_DOCREAD_EXE")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute());
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf));
    env.into_iter()
        .chain(exe_dir.map(|d| d.join(docread_exe_name())))
        .find_map(|c| vet_engine_exe(&c))
}

/// File name of the tesseract program on this operating system.
#[must_use]
pub fn tesseract_exe_name() -> &'static str {
    if cfg!(windows) {
        "tesseract.exe"
    } else {
        "tesseract"
    }
}

/// `target/ocr/<triple>/` of the source checkout (debug builds only;
/// `cargo xtask ocr-build` and `ocr-models`).
fn ocr_dev_dir() -> Option<PathBuf> {
    if !cfg!(debug_assertions) {
        return None;
    }
    let triple = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => "x86_64-pc-windows-msvc",
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("macos", "x86_64") => "x86_64-apple-darwin",
        ("linux", "x86_64") => "x86_64-unknown-linux-gnu",
        _ => return None,
    };
    Some(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/ocr")
            .join(triple),
    )
}

/// The folders [`find_ocr_dir`] tries, in order: `env` (`CL_OCR_PATH`, only
/// when absolute), `<exe_dir>/ocr/`, then `dev` (debug builds). Never the
/// working folder or `PATH`.
fn ocr_candidates(
    env: Option<PathBuf>,
    exe_dir: Option<&Path>,
    dev: Option<PathBuf>,
) -> Vec<PathBuf> {
    env.filter(|p| p.is_absolute())
        .into_iter()
        .chain(exe_dir.map(|d| d.join("ocr")))
        .chain(dev)
        .collect()
}

/// A folder holding a usable OCR install: the program and the English
/// model.
#[must_use]
pub fn is_ocr_dir(dir: &Path) -> bool {
    dir.join(tesseract_exe_name()).is_file()
        && dir.join("tessdata").join("eng.traineddata").is_file()
}

/// Find the OCR folder (`tesseract` and `tessdata/`, A03 plan D1).
#[must_use]
pub fn find_ocr_dir() -> Option<PathBuf> {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf));
    let env = std::env::var_os("CL_OCR_PATH").map(PathBuf::from);
    ocr_candidates(env, exe_dir.as_deref(), ocr_dev_dir())
        .into_iter()
        .filter(|d| is_ocr_dir(d))
        .find_map(|d| {
            let exe = vet_engine_exe(&d.join(tesseract_exe_name()))?;
            exe.parent().map(Path::to_path_buf)
        })
}

/// The candidate as an absolute path, if it is a file we are willing to run.
fn vet_engine_exe(p: &Path) -> Option<PathBuf> {
    if !p.is_file() {
        return None;
    }
    let abs = std::path::absolute(p).ok()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let euid = nix::unistd::geteuid().as_raw();
        let file = std::fs::metadata(&abs).ok()?;
        let dir = std::fs::metadata(abs.parent()?).ok()?;
        if !unix_trusted(file.uid(), file.mode(), euid)
            || !unix_trusted(dir.uid(), dir.mode(), euid)
        {
            tracing::warn!("refusing an engine program with unsafe ownership or permissions");
            return None;
        }
    }
    Some(abs)
}

/// Owned by the user or root, and not writable by group or others. (A sticky
/// shared folder such as `/tmp` is therefore never accepted.)
#[cfg(unix)]
fn unix_trusted(uid: u32, mode: u32, euid: u32) -> bool {
    (uid == euid || uid == 0) && mode & 0o022 == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn candidates_never_include_the_current_folder_or_far_parents() {
        let cwd = std::env::current_dir().unwrap();
        let exe_dir = std::env::temp_dir().join("some").join("app");
        for dev in [false, true] {
            let list = engine_candidates(None, Some(&exe_dir), dev);
            assert!(
                list.iter()
                    .all(|c| !c.starts_with(&cwd) || c.starts_with(&exe_dir))
            );
            // Without a `target` folder above the program there is no search upward.
            assert_eq!(list.len(), 2, "{list:?}");
        }
    }

    #[test]
    fn release_builds_skip_the_source_checkout_search() {
        let dir = std::env::temp_dir()
            .join("work")
            .join("target")
            .join("debug");
        let release = engine_candidates(None, Some(&dir), false);
        assert_eq!(release.len(), 2);
        assert!(
            release
                .iter()
                .all(|c| !c.to_string_lossy().contains("windows-x64")
                    && !c.starts_with(dir.parent().unwrap().join("engine")))
        );
        let dev = engine_candidates(None, Some(&dir), true);
        if engine_target_dir().is_some() {
            assert_eq!(dev.len(), 3);
            assert!(dev[2].starts_with(dir.parent().unwrap().join("engine")));
        }
    }

    #[test]
    fn ocr_is_never_looked_for_in_the_working_folder_or_path() {
        let exe_dir = std::env::temp_dir().join("app");
        let list = ocr_candidates(Some(PathBuf::from("relative/ocr")), Some(&exe_dir), None);
        assert_eq!(
            list,
            [exe_dir.join("ocr")],
            "a relative CL_OCR_PATH is ignored"
        );
        let chosen = std::env::temp_dir().join("chosen");
        let list = ocr_candidates(
            Some(chosen.clone()),
            Some(&exe_dir),
            Some(PathBuf::from("/dev")),
        );
        assert_eq!(list[0], chosen);
        assert_eq!(list.len(), 3);
        assert!(!is_ocr_dir(&std::env::temp_dir().join("no-such-ocr-dir")));
        assert!(docread_exe_name().starts_with("clientledger-docread"));
        assert!(find_docread_exe(Some(&exe_dir.join("missing"))).is_none());
    }

    #[test]
    fn the_environment_override_comes_first() {
        let env = PathBuf::from("/chosen/engine");
        let list = engine_candidates(Some(env.clone()), Some(Path::new("/app")), false);
        assert_eq!(list[0], env);
    }

    #[cfg(unix)]
    #[test]
    fn unix_trust_rules() {
        assert!(unix_trusted(1000, 0o755, 1000));
        assert!(unix_trusted(0, 0o755, 1000));
        assert!(!unix_trusted(2000, 0o755, 1000), "someone else's file");
        assert!(!unix_trusted(1000, 0o775, 1000), "group writable");
        assert!(!unix_trusted(1000, 0o757, 1000), "world writable");
        assert!(!unix_trusted(0, 0o1777, 1000), "sticky shared folder");
    }

    #[cfg(unix)]
    #[test]
    fn a_world_writable_program_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("cl-host-vet-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let exe = dir.join(engine_exe_name());
        std::fs::write(&exe, b"x").unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(find_engine_exe(Some(&exe)).is_some());
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(find_engine_exe(Some(&exe)).is_none());
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(find_engine_exe(Some(&exe)).is_none(), "writable folder");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn an_explicit_engine_path_wins_and_is_made_absolute() {
        let dir = std::env::temp_dir().join(format!("cl-host-find-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join(engine_exe_name());
        std::fs::write(&exe, b"x").unwrap();
        assert_eq!(find_engine_exe(Some(&exe)).as_deref(), Some(exe.as_path()));
        assert!(find_engine_exe(Some(&exe)).unwrap().is_absolute());
        let _ = std::fs::remove_dir_all(dir);
    }

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
            p.docread_dir(),
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
