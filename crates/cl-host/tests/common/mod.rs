//! Shared helpers for the supervisor tests.
#![allow(dead_code)]

use cl_host::{AppPaths, HostConfig, RestartPolicy};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

/// A scratch app-data folder removed on drop (`CL_HOST_KEEP=1` keeps it).
pub struct Scratch {
    pub root: PathBuf,
}

impl Scratch {
    pub fn new(name: &str) -> Scratch {
        let root = std::env::temp_dir().join(format!("cl-host-it-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        Scratch { root }
    }

    pub fn paths(&self) -> AppPaths {
        AppPaths::at(self.root.join("data")).unwrap()
    }

    pub fn file(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if std::env::var("CL_HOST_KEEP").as_deref() == Ok("1") {
            eprintln!("kept {}", self.root.display());
            return;
        }
        for _ in 0..20 {
            if std::fs::remove_dir_all(&self.root).is_ok() || !self.root.exists() {
                return;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }
}

/// The `fake_engine` example. `cargo test` builds it next to the test
/// binaries; `cargo llvm-cov` builds examples as test harnesses under a hashed
/// name, so when the plain executable is absent it is built on demand into the
/// same target directory. It must exist afterwards: this never skips.
pub fn fake_engine() -> PathBuf {
    static BUILT: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let exe = std::env::current_exe().unwrap();
    let profile_dir = exe.parent().and_then(Path::parent).unwrap();
    let name = if cfg!(windows) {
        "fake_engine.exe"
    } else {
        "fake_engine"
    };
    let p = profile_dir.join("examples").join(name);
    let _guard = BUILT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if !p.is_file() {
        let target_dir = profile_dir.parent().unwrap();
        let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
        let mut cmd = std::process::Command::new(cargo);
        cmd.args(["build", "--example", "fake_engine", "-p", "cl-host"])
            .arg("--target-dir")
            .arg(target_dir);
        if profile_dir.file_name().is_some_and(|n| n == "release") {
            cmd.arg("--release");
        }
        let status = cmd.status().expect("run cargo build for the fake engine");
        assert!(status.success(), "building the fake engine failed");
    }
    assert!(p.is_file(), "fake engine not built at {}", p.display());
    p
}

/// The reading worker with its test hooks (`test-support`), built on demand
/// in the dev profile into its own target folder (`<target>/docread-test`)
/// so the normal worker is never replaced by it (plan D2 "Test hooks").
/// Built once per test process; it must exist afterwards: this never skips.
pub fn docread_worker() -> PathBuf {
    static BUILT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    BUILT
        .get_or_init(|| {
            let exe = std::env::current_exe().unwrap();
            let profile_dir = exe.parent().and_then(Path::parent).unwrap();
            let target_dir = profile_dir.parent().unwrap().join("docread-test");
            let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
            let status = std::process::Command::new(cargo)
                .args([
                    "build",
                    "-p",
                    "clientledger-docread",
                    "--features",
                    "test-support",
                ])
                .arg("--target-dir")
                .arg(&target_dir)
                .status()
                .expect("run cargo build for the reading worker");
            assert!(status.success(), "building the reading worker failed");
            let p = target_dir.join("debug").join(cl_host::docread_exe_name());
            assert!(p.is_file(), "reading worker not built at {}", p.display());
            p
        })
        .clone()
}

/// Whether `cargo xtask pdfium-fetch` has run; `CL_PDFIUM_REQUIRE=1` makes
/// its absence a failure.
pub fn pdfium_present() -> bool {
    let name = if cfg!(windows) {
        "pdfium.dll"
    } else if cfg!(target_os = "macos") {
        "libpdfium.dylib"
    } else {
        "libpdfium.so"
    };
    let triple = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", _) => "x86_64-pc-windows-msvc",
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("macos", _) => "x86_64-apple-darwin",
        _ => "x86_64-unknown-linux-gnu",
    };
    let found = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/pdfium")
        .join(triple)
        .join(name)
        .is_file();
    assert!(
        found || std::env::var("CL_PDFIUM_REQUIRE").as_deref() != Ok("1"),
        "PDFium is required (CL_PDFIUM_REQUIRE=1) but was not found"
    );
    found
}

/// Tiny synthetic PDFs: Helvetica text on US letter pages.
pub mod pdf {
    use std::fmt::Write as _;

    /// `s` (no parentheses or backslashes) at (`x`, `y`) points.
    pub fn text(x: i32, y: i32, size: i32, s: &str) -> String {
        format!("BT /F1 {size} Tf {x} {y} Td ({s}) Tj ET\n")
    }

    /// A PDF with one letter page per entry of `pages`.
    pub fn build(pages: &[String]) -> Vec<u8> {
        let mut objects: Vec<String> = vec![
            "<< /Type /Catalog /Pages 2 0 R >>".into(),
            format!(
                "<< /Type /Pages /Kids [{}] /Count {} >>",
                (0..pages.len())
                    .map(|i| format!("{} 0 R", 4 + i * 2))
                    .collect::<Vec<_>>()
                    .join(" "),
                pages.len()
            ),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
        ];
        for (i, c) in pages.iter().enumerate() {
            objects.push(format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R >> >> /Contents {} 0 R >>",
                5 + i * 2
            ));
            objects.push(format!("<< /Length {} >>\nstream\n{c}endstream", c.len()));
        }
        let mut out = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
        }
        let xref = out.len();
        let mut t = format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1);
        for off in offsets {
            let _ = writeln!(t, "{off:010} 00000 n ");
        }
        let _ = write!(
            t,
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        );
        out.extend_from_slice(t.as_bytes());
        out
    }
}

/// Fast timings so failure paths finish in a second or two.
pub fn quick_policy() -> RestartPolicy {
    RestartPolicy {
        max_restarts: 3,
        backoff: vec![Duration::from_millis(50)],
        stable_after: Duration::from_mins(5),
        ready_timeout: Duration::from_secs(10),
        health_ready_timeout: Duration::from_secs(10),
        health_interval: Duration::from_millis(200),
        health_failures: 2,
        poll_interval: Duration::from_millis(50),
        listener_check_every: Duration::from_millis(500),
        stop_grace: Duration::from_secs(3),
        // An upper bound: the drain ends when the output pipes close, so a
        // loaded CI runner still keeps the engine's last lines.
        exit_drain: Duration::from_secs(2),
    }
}

/// Config for the fake engine with secrets kept in memory.
pub fn fake_config(scratch: &Scratch) -> HostConfig {
    let mut cfg = cl_host::in_memory_stores(HostConfig::new(scratch.paths(), fake_engine()));
    cfg.policy = quick_policy();
    cfg
}

fn refresh(pid: u32) -> System {
    let mut sys = System::new();
    sys.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[Pid::from_u32(pid)]),
        true,
        ProcessRefreshKind::nothing()
            .with_cmd(UpdateKind::Always)
            .with_exe(UpdateKind::Always),
    );
    sys
}

pub fn process_alive(pid: u32) -> bool {
    refresh(pid).process(Pid::from_u32(pid)).is_some()
}

/// The process's argument list, program name included.
pub fn command_line(pid: u32) -> Vec<String> {
    refresh(pid)
        .process(Pid::from_u32(pid))
        .map(|p| {
            p.cmd()
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default()
}

pub fn kill_hard(pid: u32) {
    let sys = refresh(pid);
    let p = sys
        .process(Pid::from_u32(pid))
        .expect("process to kill exists");
    assert!(p.kill(), "could not kill {pid}");
    wait_until(Duration::from_secs(10), || !process_alive(pid));
}

pub fn wait_until(limit: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < limit {
        if cond() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    cond()
}

/// All processes whose command line or program path mentions `needle`.
pub fn processes_mentioning(needle: &str) -> Vec<(u32, String)> {
    let mut sys = System::new();
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing()
            .with_cmd(UpdateKind::Always)
            .with_exe(UpdateKind::Always),
    );
    let needle = needle.to_lowercase();
    sys.processes()
        .iter()
        .filter(|(_, p)| {
            let cmd: String = p
                .cmd()
                .iter()
                .map(|a| a.to_string_lossy().to_lowercase())
                .collect::<Vec<_>>()
                .join(" ");
            let exe = p
                .exe()
                .map(|e| e.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            cmd.contains(&needle) || exe.contains(&needle)
        })
        .map(|(pid, p)| (pid.as_u32(), p.name().to_string_lossy().into_owned()))
        .collect()
}
