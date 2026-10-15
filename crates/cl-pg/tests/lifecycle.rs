//! Start, stop, recovery, ports, and loopback-only binding (P03 step 4).

mod common;

use cl_pg::{Cluster, MemoryStore, PasswordStore, PgError};
use std::fs;
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::time::{Duration, Instant};

/// Stops the server when a test ends, pass or fail.
struct Guard<'a>(&'a Cluster);

impl Drop for Guard<'_> {
    fn drop(&mut self) {
        let _ = self.0.stop();
    }
}

fn budget() -> Duration {
    let secs = std::env::var("CL_PG_BUDGET_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10);
    Duration::from_secs(secs)
}

#[test]
fn fresh_ensure_and_start_is_fast_and_loopback_only() {
    let Some(bins) = common::bins() else { return };
    let scratch = common::Scratch::new("fresh");
    let store = MemoryStore::new();

    let t0 = Instant::now();
    let cluster = Cluster::ensure(&scratch.data_dir(), bins, &store).unwrap();
    let running = cluster.start().unwrap();
    let took = t0.elapsed();
    let _stop = Guard(&cluster);
    eprintln!("fresh ensure + start took {took:?}");
    assert!(
        took < budget(),
        "ensure + start took {took:?}, budget {:?}",
        budget()
    );
    assert!(!running.reused);

    // Password login works; a wrong password and no password do not.
    let mut db = common::connect(&cluster, running.port, "postgres");
    let enc: String = db.query_one("show server_encoding", &[]).unwrap().get(0);
    assert_eq!(enc, "UTF8");
    let collate: String = db
        .query_one(
            "select datcollate from pg_database where datname = 'postgres'",
            &[],
        )
        .unwrap()
        .get(0);
    eprintln!("lc_collate = {collate}");
    assert!(collate == "C" || collate.starts_with("C."), "{collate}");
    assert!(common::connect_with(running.port, "postgres", "not-the-password").is_err());

    // Loopback only: the pid file, the OS socket table, and real connections.
    let pf = fs::read_to_string(scratch.data_dir().join("postmaster.pid")).unwrap();
    let lines: Vec<&str> = pf.lines().collect();
    assert_eq!(lines[3].trim(), running.port.to_string());
    assert_eq!(lines[4].trim(), "", "no Unix socket directory: {pf}");
    assert_eq!(lines[5].trim(), "127.0.0.1", "{pf}");
    common::assert_only_loopback(running.port);
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

#[test]
fn nothing_listens_on_another_address_of_this_computer() {
    let Some(bins) = common::bins() else { return };
    let scratch = common::Scratch::new("lan");
    let cluster = Cluster::ensure(&scratch.data_dir(), bins, &MemoryStore::new()).unwrap();
    let running = cluster.start().unwrap();
    let _stop = Guard(&cluster);

    // The address this computer would use to reach the network (no packet is sent).
    let lan_ip = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))
        .and_then(|s| s.connect((Ipv4Addr::new(192, 0, 2, 1), 9)).map(|()| s))
        .and_then(|s| s.local_addr())
        .ok()
        .filter(|a| !a.ip().is_loopback());
    match lan_ip {
        Some(addr) => {
            let target = SocketAddr::new(addr.ip(), running.port);
            let res = TcpStream::connect_timeout(&target, Duration::from_secs(2));
            assert!(res.is_err(), "database reachable on {target}");
        }
        None => eprintln!("no non-loopback address on this machine; nothing to probe"),
    }
    // IPv6 loopback is off too.
    let v6 = SocketAddr::from((std::net::Ipv6Addr::LOCALHOST, running.port));
    assert!(TcpStream::connect_timeout(&v6, Duration::from_secs(2)).is_err());
}
