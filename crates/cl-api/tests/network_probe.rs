//! Proof that the network-blocked CI run really is blocked (P05 plan §2.11,
//! B3). Without this probe the blocked job would prove nothing.
//!
//! The Linux `engine` job rejects all non-loopback traffic of the `cltest`
//! user (iptables/ip6tables owner match) and then runs the contract tests as
//! that user with `CL_NETWORK_BLOCKED=1`. Here, with that variable set:
//!
//! - a connect to a public address (1.1.1.1:443 and its IPv6 twin) must fail,
//!   and the IPv4 one must fail fast (a rejected connect, not a timeout:
//!   a silently dropped packet would only look blocked);
//! - a connect to a listener on 127.0.0.1 must still succeed, because the
//!   engine, PostgreSQL and the client all talk over loopback.
//!
//! Without `CL_NETWORK_BLOCKED=1` the test is skipped with a clear message
//! (developer machines and the unrestricted jobs reach the network).

use std::io::ErrorKind;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::time::{Duration, Instant};

const WAIT: Duration = Duration::from_secs(3);

fn blocked_run() -> bool {
    std::env::var("CL_NETWORK_BLOCKED").as_deref() == Ok("1")
}

#[test]
fn public_addresses_are_unreachable_and_loopback_works() {
    if !blocked_run() {
        eprintln!(
            "SKIPPED: network_probe only runs in the network-blocked CI job \
             (set CL_NETWORK_BLOCKED=1 after installing the egress rule)"
        );
        return;
    }

    let v4: SocketAddr = "1.1.1.1:443".parse().expect("literal address");
    let started = Instant::now();
    let err = TcpStream::connect_timeout(&v4, WAIT)
        .expect_err("a public IPv4 address must not be reachable in the blocked run");
    assert_ne!(
        err.kind(),
        ErrorKind::TimedOut,
        "the connect timed out after {:?}: the packet was dropped, not rejected, \
         so the iptables REJECT rule is not what stopped it",
        started.elapsed()
    );
    assert!(
        started.elapsed() < WAIT,
        "a rejected connect must fail fast, took {:?}",
        started.elapsed()
    );

    let v6: SocketAddr = "[2606:4700:4700::1111]:443"
        .parse()
        .expect("literal address");
    TcpStream::connect_timeout(&v6, WAIT)
        .expect_err("a public IPv6 address must not be reachable in the blocked run");

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let local = listener.local_addr().expect("local address");
    TcpStream::connect_timeout(&local, WAIT).expect("loopback must stay reachable");

    eprintln!("network_probe: blocked OK (public IPv4/IPv6 rejected, loopback reachable)");
}
