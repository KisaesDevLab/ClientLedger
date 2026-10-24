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
