//! Is the engine up, is it ours, and is it healthy?
//!
//! Two separate questions (security review item L11):
//!
//! * [`verify_listener`]: the operating system says our own child holds the
//!   listening socket, and nobody else does. If the engine dies, another
//!   program could bind its port; before the launch token is sent anywhere we
//!   check who owns the port.
//! * [`check_health`]: `GET /health` with `X-CL-Token` returns 200 with a
//!   healthy desktop-mode body. A wrong answer to the token (401) means
//!   something other than our engine is behind the port.
//!
//! A stronger proof, an HMAC challenge the engine answers with the token, is
//! recorded as an engine follow-up (U-16, proposed).

use crate::http;
use cl_pg::Secret;
use serde_json::Value;
use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;

/// What `/health` reported (desktop mode, `/health` v2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthReport {
    /// The database check passed.
    pub db_ok: bool,
    /// `mode` field (`desktop`).
    pub mode: String,
    /// Engine version string, when reported.
    pub version: Option<String>,
    /// Source commit the engine was built from, when reported.
    pub engine_commit: Option<String>,
    /// Number of applied migrations, when reported.
    pub schema_version: Option<u64>,
    /// Whether the engine's database role is a superuser, when reported.
    pub superuser: Option<bool>,
}

/// Why a health check did not pass.
#[derive(Debug, thiserror::Error)]
pub enum HealthError {
    /// Nothing answered on the port.
    #[error("the engine did not answer: {0}")]
    Unreachable(String),
    /// The launch token was refused: whatever answers is not our engine.
    #[error("the program on the engine's port refused the launch token")]
    TokenRefused,
    /// A status other than 200.
    #[error("the engine answered HTTP {0}")]
    BadStatus(u16),
    /// The body was not what a desktop-mode engine sends.
    #[error("the engine's health answer was not understood")]
    Malformed,
    /// The engine says it is not healthy.
    #[error("the engine reports a problem: database check failed")]
    Unhealthy,
    /// Another process owns the port (or shares it).
    #[error("the engine's port is not held by the engine process we started")]
    NotOurProcess,
}

/// Parse the body of a 200 `/health` response.
pub(crate) fn parse_health(body: &str) -> Result<HealthReport, HealthError> {
    let v: Value = serde_json::from_str(body).map_err(|_| HealthError::Malformed)?;
    let mode = v
        .get("mode")
        .and_then(Value::as_str)
        .ok_or(HealthError::Malformed)?
        .to_string();
    let db_ok = v.get("db").and_then(Value::as_str) == Some("ok");
    let text = |key: &str| v.get(key).and_then(Value::as_str).map(str::to_string);
    Ok(HealthReport {
        db_ok,
        mode,
        version: text("version"),
        engine_commit: text("engineCommit"),
        schema_version: v.get("schemaVersion").and_then(Value::as_u64),
        superuser: v.pointer("/checks/db/superuser").and_then(Value::as_bool),
    })
}

/// `GET /health` with the launch token.
///
/// # Errors
/// See [`HealthError`].
pub fn check_health(
    port: u16,
    token: &Secret,
    timeout: Duration,
) -> Result<HealthReport, HealthError> {
    let r = http::request(port, "GET", "/health", token, None, timeout)
        .map_err(|e| HealthError::Unreachable(e.kind().to_string()))?;
    match r.status {
        200 => {}
        401 => return Err(HealthError::TokenRefused),
        other => return Err(HealthError::BadStatus(other)),
    }
    let report = parse_health(&r.body)?;
    if report.mode != "desktop" {
        return Err(HealthError::Malformed);
    }
    if !report.db_ok {
        return Err(HealthError::Unhealthy);
    }
    Ok(report)
}

/// Check that exactly one program listens on loopback `port`, and that it is
/// process `pid`.
///
/// # Errors
/// [`HealthError::NotOurProcess`] when the owner differs, when more than one
/// process listens, or when the socket is not bound to loopback only. A
/// failure to list sockets is reported as [`HealthError::Unreachable`].
pub fn verify_listener(port: u16, pid: u32) -> Result<(), HealthError> {
    let all = listeners::get_all().map_err(|e| HealthError::Unreachable(e.to_string()))?;
    let mut owners = all.iter().filter(|l| {
        l.socket.port() == port
            && l.protocol == listeners::Protocol::TCP
            && l.state == listeners::SocketState::Listen
    });
    let Some(first) = owners.next() else {
        return Err(HealthError::NotOurProcess);
    };
    let mut all_ours = first.process.pid == pid && is_loopback(first.socket.ip());
    for other in owners {
        all_ours &= other.process.pid == pid && is_loopback(other.socket.ip());
    }
    if all_ours {
        Ok(())
    } else {
        Err(HealthError::NotOurProcess)
    }
}

fn is_loopback(ip: IpAddr) -> bool {
    ip.is_loopback() || ip == IpAddr::V4(Ipv4Addr::LOCALHOST)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    const GOOD: &str = r#"{"status":"ok","db":"ok","redis":"disabled","mode":"desktop","version":"v1","engineCommit":"abc","schemaVersion":216,"checks":{"db":{"ok":true,"superuser":false}}}"#;

    #[test]
    fn a_desktop_health_body_is_parsed() {
        let r = parse_health(GOOD).unwrap();
        assert!(r.db_ok);
        assert_eq!(r.mode, "desktop");
        assert_eq!(r.engine_commit.as_deref(), Some("abc"));
        assert_eq!(r.schema_version, Some(216));
        assert_eq!(r.superuser, Some(false));
    }

    #[test]
    fn missing_mode_or_bad_json_is_malformed() {
        assert!(matches!(parse_health("{}"), Err(HealthError::Malformed)));
        assert!(matches!(
            parse_health("<html>"),
            Err(HealthError::Malformed)
        ));
    }

    /// Serve one canned response on a fresh loopback port.
    fn serve_once(response: String) -> u16 {
        let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Ok((mut s, _)) = l.accept() {
                let mut buf = [0u8; 2048];
                let _ = s.read(&mut buf);
                let _ = s.write_all(response.as_bytes());
            }
        });
        port
    }

    fn http_200(body: &str) -> String {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    #[test]
    fn health_passes_for_a_good_body() {
        let port = serve_once(http_200(GOOD));
        let r = check_health(port, &Secret::new("t"), Duration::from_secs(2)).unwrap();
        assert_eq!(r.version.as_deref(), Some("v1"));
    }

    #[test]
    fn a_401_means_the_token_was_refused() {
        let port = serve_once("HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n".into());
        let err = check_health(port, &Secret::new("t"), Duration::from_secs(2)).unwrap_err();
        assert!(matches!(err, HealthError::TokenRefused), "{err}");
    }

    #[test]
    fn a_non_desktop_or_failing_database_is_not_healthy() {
        let port = serve_once(http_200(&GOOD.replace("desktop", "server")));
        assert!(matches!(
            check_health(port, &Secret::new("t"), Duration::from_secs(2)),
            Err(HealthError::Malformed)
        ));
        let port = serve_once(http_200(&GOOD.replace(r#""db":"ok""#, r#""db":"fail""#)));
        assert!(matches!(
            check_health(port, &Secret::new("t"), Duration::from_secs(2)),
            Err(HealthError::Unhealthy)
        ));
    }

    #[test]
    fn nothing_listening_is_unreachable() {
        let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = l.local_addr().unwrap().port();
        drop(l);
        assert!(matches!(
            check_health(port, &Secret::new("t"), Duration::from_millis(500)),
            Err(HealthError::Unreachable(_))
        ));
    }

    #[test]
    fn the_token_is_never_in_an_error_message() {
        let port = serve_once("HTTP/1.1 500 Oops\r\nContent-Length: 0\r\n\r\n".into());
        let err = check_health(port, &Secret::new("sekret-token"), Duration::from_secs(2));
        assert!(!format!("{err:?}").contains("sekret-token"));
    }

    #[test]
    fn this_process_owns_its_own_listener_and_nobody_else_does() {
        let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = l.local_addr().unwrap().port();
        let me = std::process::id();
        verify_listener(port, me).expect("we own our own socket");
        assert!(matches!(
            verify_listener(port, me.wrapping_add(1)),
            Err(HealthError::NotOurProcess)
        ));
        drop(l);
        assert!(matches!(
            verify_listener(port, me),
            Err(HealthError::NotOurProcess)
        ));
    }

    #[test]
    fn a_wildcard_listener_is_not_accepted() {
        let l = TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0)).unwrap();
        let port = l.local_addr().unwrap().port();
        assert!(matches!(
            verify_listener(port, std::process::id()),
            Err(HealthError::NotOurProcess)
        ));
    }
}
