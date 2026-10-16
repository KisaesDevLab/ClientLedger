//! The `host` commands against a throwaway app data folder. They never start
//! the database or engine (that needs the keychain); the running side is played
//! by the single-instance endpoint, which is exactly what the commands talk to.

use cl_host::{Acquired, AppPaths};
use std::path::Path;
use std::process::{Command, Output};

fn cli(data_dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_clientledger-cli"))
        .arg("host")
        .args(args)
        .arg("--data-dir")
        .arg(data_dir)
        .output()
        .expect("run the CLI")
}

fn scratch(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("cl-cli-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn text(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn status_and_stop_report_not_running_with_exit_3() {
    let dir = scratch("none");
    for cmd in ["status", "stop"] {
        let out = cli(&dir, &[cmd]);
        assert_eq!(out.status.code(), Some(3), "{cmd}");
        assert!(text(&out).contains("not running"));
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn status_shows_what_the_running_host_reports_and_stop_asks_it_to_stop() {
    let dir = scratch("running");
    let paths = AppPaths::at(&dir).unwrap();
    let Acquired::First(guard) = cl_host::acquire(&paths).unwrap() else {
        panic!("expected to be first");
    };
    guard.set_status("state=running database_pid=1 engine_pid=2");

    let out = cli(&dir, &["status"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(text(&out).contains("state=running database_pid=1 engine_pid=2"));
    assert!(!text(&out).to_lowercase().contains("token"));

    // `start` refuses to run a second host and says so.
    let out = cli(
        &dir,
        &[
            "start",
            "--engine-exe",
            std::env::current_exe().unwrap().to_str().unwrap(),
        ],
    );
    assert_eq!(out.status.code(), Some(4), "{}", text(&out));
    assert!(text(&out).contains("already running"));

    // `stop` sets the stop request, then waits for the host to go away.
    let waiter = std::thread::spawn({
        let dir = dir.clone();
        move || cli(&dir, &["stop"])
    });
    let start = std::time::Instant::now();
    while !guard.stop_requested() {
        assert!(start.elapsed().as_secs() < 20, "stop was never requested");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    drop(guard); // the host exits
    let out = waiter.join().unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(text(&out).contains("stopped"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn logs_print_the_tail_of_the_host_and_engine_logs() {
    let dir = scratch("logs");
    let paths = AppPaths::at(&dir).unwrap();
    paths.ensure_dirs().unwrap();
    std::fs::write(
        paths.host_log(),
        "host line 1\nhost line 2\nhost line 3\nhost line 4\nhost line 5\n\
         host line 6\nhost line 7\nhost line 8\nhost line 9\nhost line 10\n",
    )
    .unwrap();
    std::fs::write(paths.engine_log(), "engine line A\nengine line B\n").unwrap();

    let out = cli(&dir, &["logs", "--lines", "3"]);
    let shown = text(&out);
    assert_eq!(out.status.code(), Some(0));
    assert!(shown.contains("host line 10") && shown.contains("host line 8"));
    assert!(!shown.contains("host line 7"));

    let out = cli(&dir, &["logs", "--engine"]);
    assert!(text(&out).contains("engine line B"));

    let empty = scratch("logs-empty");
    let out = cli(&empty, &["logs"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(text(&out).contains("empty or missing"));
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(empty);
}

#[test]
fn start_without_an_engine_fails_with_a_plain_message() {
    let dir = scratch("noengine");
    let out = cli(
        &dir,
        &[
            "start",
            "--engine-exe",
            dir.join("missing.exe").to_str().unwrap(),
        ],
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("engine program"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn usage_errors_exit_2() {
    let dir = scratch("usage");
    let out = cli(&dir, &["frobnicate"]);
    assert_eq!(out.status.code(), Some(2));
    let out = Command::new(env!("CARGO_BIN_EXE_clientledger-cli"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let _ = std::fs::remove_dir_all(dir);
}
