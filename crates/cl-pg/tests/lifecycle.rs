//! Start, stop, recovery, ports, and loopback-only binding (P03 step 4).

mod common;

use cl_pg::{Cluster, MemoryStore, PasswordStore, PgError};
use std::fs;
use std::net::{Ipv4Addr, TcpListener};
use std::time::Instant;

/// Stops the server when a test ends, pass or fail.
struct Guard<'a>(&'a Cluster);

impl Drop for Guard<'_> {
    fn drop(&mut self) {
        let _ = self.0.stop();
    }
}

#[test]
fn second_start_reuses_the_cluster_and_its_data() {
    let Some(bins) = common::bins() else { return };
    let scratch = common::Scratch::new("reuse");
    let store = MemoryStore::new();

    let cluster = Cluster::ensure(&scratch.data_dir(), bins.clone(), &store).unwrap();
    let first = cluster.start().unwrap();
    let _stop = Guard(&cluster);
    let mut db = common::connect(&cluster, first.port, "postgres");
    db.batch_execute("create table t(id int primary key, v text); insert into t values (1,'a')")
        .unwrap();
    drop(db);

    // Starting a running server adopts it.
    let again = cluster.start().unwrap();
    assert!(again.reused);
    assert_eq!(again.port, first.port);
    assert_eq!(cluster.running_port(), Some(first.port));

    cluster.stop().unwrap();
    assert_eq!(cluster.running_port(), None);
    assert!(!cl_pg::ports::is_ready(first.port));
    cluster.stop().unwrap(); // stopping a stopped server is fine

    // A new Cluster value over the same folder: same password, same data.
    let pw_before = store.get().unwrap().unwrap();
    let cluster2 = Cluster::ensure(&scratch.data_dir(), bins, &store).unwrap();
    assert_eq!(store.get().unwrap().unwrap(), pw_before);
    let second = cluster2.start().unwrap();
    let _stop2 = Guard(&cluster2);
    let mut db = common::connect(&cluster2, second.port, "postgres");
    let n: i64 = db.query_one("select count(*) from t", &[]).unwrap().get(0);
    assert_eq!(n, 1);
}

#[test]
fn recovers_after_the_server_is_killed() {
    let Some(bins) = common::bins() else { return };
    let scratch = common::Scratch::new("kill9");
    let store = MemoryStore::new();
    let cluster = Cluster::ensure(&scratch.data_dir(), bins, &store).unwrap();
    let running = cluster.start().unwrap();
    let _stop = Guard(&cluster);

    let mut db = common::connect(&cluster, running.port, "postgres");
    db.batch_execute(
        "create table t(id int primary key); insert into t select g from generate_series(1,1000) g",
    )
    .unwrap();
    drop(db);

    common::kill_hard(running.pid);
    // The postmaster is gone but its pid file is left behind.
    assert!(scratch.data_dir().join("postmaster.pid").exists());
    assert!(!cl_pg::ports::is_ready(running.port));

    let t0 = Instant::now();
    let restarted = cluster.start().unwrap();
    eprintln!("restart after kill took {:?}", t0.elapsed());
    assert!(!restarted.reused);
    let mut db = common::connect(&cluster, restarted.port, "postgres");
    let n: i64 = db.query_one("select count(*) from t", &[]).unwrap().get(0);
    assert_eq!(n, 1000, "committed rows survive the crash");
}

#[test]
fn a_taken_port_is_skipped() {
    let Some(bins) = common::bins() else { return };
    let scratch = common::Scratch::new("collide");
    let cluster = Cluster::ensure(&scratch.data_dir(), bins, &MemoryStore::new()).unwrap();

    let blocker = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let taken = blocker.local_addr().unwrap().port();
    let free = cl_pg::ports::free_loopback_port().unwrap();

    let running = cluster.start_on_ports([taken, free]).unwrap();
    let _stop = Guard(&cluster);
    assert_eq!(running.port, free);
    assert_ne!(running.port, taken);
    common::connect(&cluster, running.port, "postgres");
}

#[test]
fn giving_only_taken_ports_fails_clearly() {
    let Some(bins) = common::bins() else { return };
    let scratch = common::Scratch::new("collide-all");
    let cluster = Cluster::ensure(&scratch.data_dir(), bins, &MemoryStore::new()).unwrap();
    let blocker = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let taken = blocker.local_addr().unwrap().port();
    let err = cluster.start_on_ports([taken]).unwrap_err();
    assert!(matches!(err, PgError::StartFailed { .. }), "{err:?}");
    assert_eq!(cluster.running_port(), None);
}

#[test]
fn a_stale_pid_file_owned_by_another_program_is_cleared() {
    let Some(bins) = common::bins() else { return };
    let scratch = common::Scratch::new("pidreuse");
    let cluster = Cluster::ensure(&scratch.data_dir(), bins, &MemoryStore::new()).unwrap();

    // Pretend a crash left a pid file whose number now belongs to this test.
    let me = std::process::id();
    fs::write(
        scratch.data_dir().join("postmaster.pid"),
        format!(
            "{me}\n{}\n1700000000\n5432\n\n127.0.0.1\n  1 1\nready\n",
            scratch.data_dir().display()
        ),
    )
    .unwrap();

    let running = cluster.start().unwrap();
    let _stop = Guard(&cluster);
    assert!(!running.reused);
    assert_ne!(running.pid, me);
    common::connect(&cluster, running.port, "postgres");
}

/// A long-running program that is certainly not `PostgreSQL`.
fn spawn_sleeper() -> std::process::Child {
    let mut cmd = if cfg!(windows) {
        let mut c = std::process::Command::new("ping");
        c.args(["-n", "60", "127.0.0.1"]);
        c
    } else {
        let mut c = std::process::Command::new("sleep");
        c.arg("60");
        c
    };
    cmd.stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap()
}

#[test]
fn stop_never_signals_an_unrelated_program_that_reused_the_pid() {
    let Some(bins) = common::bins() else { return };
    let scratch = common::Scratch::new("stopreuse");
    let cluster = Cluster::ensure(&scratch.data_dir(), bins, &MemoryStore::new()).unwrap();

    let mut other = spawn_sleeper();
    fs::write(
        scratch.data_dir().join("postmaster.pid"),
        format!(
            "{}\n{}\n1700000000\n5432\n\n127.0.0.1\n  1 1\nready\n",
            other.id(),
            scratch.data_dir().display()
        ),
    )
    .unwrap();

    cluster.stop().unwrap();
    assert!(
        other.try_wait().unwrap().is_none(),
        "stop() ended an unrelated program"
    );
    // On Windows `pg_ctl status` already says "not running" for a pid that has
    // no PostgreSQL signal pipe, so the file is simply left for the next start
    // to replace; on Unix `pg_ctl` would have signalled the process.
    if cfg!(unix) {
        assert!(
            !scratch.data_dir().join("postmaster.pid").exists(),
            "the leftover pid file should be removed"
        );
    }
    other.kill().unwrap();
    let _ = other.wait();
}

#[test]
fn a_pid_file_naming_another_data_directory_is_not_ours() {
    let Some(bins) = common::bins() else { return };
    let scratch = common::Scratch::new("otherdir");
    let first = Cluster::ensure(&scratch.path("one"), bins.clone(), &MemoryStore::new()).unwrap();
    let second = Cluster::ensure(&scratch.path("two"), bins, &MemoryStore::new()).unwrap();
    let running = first.start().unwrap();
    let _stop = Guard(&first);

    // `two` has a pid file naming a real PostgreSQL process, but one that
    // belongs to `one`: stopping `two` must leave that server alone.
    fs::write(
        scratch.path("two").join("postmaster.pid"),
        format!(
            "{}\n{}\n1700000000\n5432\n\n127.0.0.1\n  1 1\nready\n",
            running.pid,
            scratch.path("one").display()
        ),
    )
    .unwrap();
    second.stop().unwrap();
    assert_eq!(first.running_port(), Some(running.port));
    assert!(!scratch.path("two").join("postmaster.pid").exists());
}
