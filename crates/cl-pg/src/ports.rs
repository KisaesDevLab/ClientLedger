//! Loopback ports and the "is the server answering" probe.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::time::Duration;

/// Ask the OS for a free loopback port. There is a small window before the
/// server binds it; the caller retries on a collision.
///
/// # Errors
/// Fails when no loopback socket can be bound.
pub fn free_loopback_port() -> std::io::Result<u16> {
    let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    Ok(l.local_addr()?.port())
}

/// A v3 startup packet for user and database `postgres`. The server answers
/// with an authentication request (ready) or an error (not ready).
fn startup_packet() -> Vec<u8> {
    let mut body: Vec<u8> = Vec::new();
    body.extend_from_slice(&196_608u32.to_be_bytes()); // protocol 3.0
    for part in ["user", "postgres", "database", "postgres"] {
        body.extend_from_slice(part.as_bytes());
        body.push(0);
    }
    body.push(0);
    let len = u32::try_from(body.len() + 4).unwrap_or(u32::MAX);
    let mut packet = len.to_be_bytes().to_vec();
    packet.extend_from_slice(&body);
    packet
}

/// What a server's first reply to a startup packet means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Reply {
    /// It wants to authenticate us: accepting connections.
    Accepting,
    /// Alive but refusing (starting up, recovering, shutting down).
    Rejecting,
    /// Not a `PostgreSQL` reply.
    Unknown,
}

pub(crate) fn classify_reply(bytes: &[u8]) -> Reply {
    match bytes.first() {
        Some(b'R') => Reply::Accepting,
        Some(b'E') => {
            let text = String::from_utf8_lossy(bytes);
            if text.contains("57P03") || text.contains("57P01") || text.contains("57P02") {
                Reply::Rejecting
            } else {
                // Some other error (for example too many clients): alive.
                Reply::Accepting
            }
        }
        _ => Reply::Unknown,
    }
}

/// Like `pg_isready`: true when the server on `port` accepts connections.
#[must_use]
pub fn is_ready(port: u16) -> bool {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let Ok(mut s) = TcpStream::connect_timeout(&addr, Duration::from_millis(500)) else {
        return false;
    };
    let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = s.set_write_timeout(Some(Duration::from_secs(2)));
    if s.write_all(&startup_packet()).is_err() {
        return false;
    }
    let mut buf = [0u8; 256];
    let n = s.read(&mut buf).unwrap_or(0);
    classify_reply(&buf[..n]) == Reply::Accepting
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_port_is_loopback_bindable() {
        // The port is released before we bind it again, so another test or
        // process can take it first; that is the collision the contract says
        // callers retry on.
        let rebound = (0..5).any(|_| {
            let p = free_loopback_port().unwrap();
            p != 0 && TcpListener::bind((Ipv4Addr::LOCALHOST, p)).is_ok()
        });
        assert!(rebound, "no free loopback port could be bound again");
    }

    #[test]
    fn startup_packet_is_well_formed() {
        let p = startup_packet();
        let len = u32::from_be_bytes([p[0], p[1], p[2], p[3]]) as usize;
        assert_eq!(len, p.len());
        assert_eq!(&p[4..8], &196_608u32.to_be_bytes());
        assert_eq!(p.last(), Some(&0));
    }

    #[test]
    fn replies_are_classified() {
        assert_eq!(classify_reply(b"R\0\0\0\x17\0\0\0\x0a"), Reply::Accepting);
        assert_eq!(
            classify_reply(b"EFATAL\0C57P03\0Mthe database system is starting up\0"),
            Reply::Rejecting
        );
        assert_eq!(
            classify_reply(b"EFATAL\0C53300\0Mtoo many clients\0"),
            Reply::Accepting
        );
        assert_eq!(classify_reply(b"HTTP/1.1 400"), Reply::Unknown);
        assert_eq!(classify_reply(b""), Reply::Unknown);
    }

    #[test]
    fn closed_port_is_not_ready() {
        let p = free_loopback_port().unwrap();
        assert!(!is_ready(p));
    }

    #[test]
    fn a_non_postgres_listener_is_not_ready() {
        let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = l.local_addr().unwrap().port();
        let t = std::thread::spawn(move || {
            if let Ok((mut s, _)) = l.accept() {
                let mut b = [0u8; 64];
                let _ = s.read(&mut b);
                let _ = s.write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n");
            }
        });
        assert!(!is_ready(port));
        t.join().unwrap();
    }
}
