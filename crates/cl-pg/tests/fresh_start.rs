//! Fresh creation and first start, alone in its own test program so the 10 s
//! acceptance budget is not measured while other tests compete for the CPU.

mod common;

use cl_pg::{Cluster, MemoryStore};
use std::fs;
use std::net::{Ipv4Addr, SocketAddr, TcpStream, UdpSocket};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// The tests here run one at a time so the timing one has the machine to itself.
static SERIAL: Mutex<()> = Mutex::new(());

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
    let _one_at_a_time = SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
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
fn nothing_listens_on_another_address_of_this_computer() {
    let _one_at_a_time = SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
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
