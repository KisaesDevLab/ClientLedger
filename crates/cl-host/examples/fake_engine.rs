//! A stand-in for the engine executable, for supervisor tests.
//!
//! It follows the desktop contract (`CL_BIND`, `CL_TOKEN`, ready line on
//! stdout, `X-CL-Token` gate, `/health`, `/api/v1/desktop/info`,
//! `/api/v1/desktop/initialize`) and misbehaves on request through
//! `FAKE_*` variables:
//!
//! * `FAKE_REPORT=<file>`: write the process arguments and environment there
//! * `FAKE_COUNTER=<file>`: count starts in this file
//! * `FAKE_FAIL_FIRST=<n>`: exit 1 before the ready line on the first n starts
//! * `FAKE_FATAL=1`: print a fatal line and exit 78
//! * `FAKE_NO_READY=1`: never print the ready line
//! * `FAKE_DIE_AFTER_MS=<ms>`: exit 1 that long after the ready line ...
//! * `FAKE_DIE_FIRST=<n>`: ... but only on the first n starts (default: always)
//! * `FAKE_HANG_HEALTH=1`: accept connections and never answer
//! * `FAKE_SUPERUSER=1`: report a superuser database role
//! * `FAKE_LEAK_TOKEN=1`: print the launch token to stderr before dying
#![forbid(unsafe_code)]

use std::fmt::Write as _;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

fn env(key: &str) -> Option<String> {
    std::env::var(key).ok()
}

fn flag(key: &str) -> bool {
    env(key).as_deref() == Some("1")
}

fn number(key: &str) -> Option<u64> {
    env(key).and_then(|v| v.parse().ok())
}

fn count_start() -> u64 {
    let Some(path) = env("FAKE_COUNTER") else {
        return 1;
    };
    let n = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| t.trim().parse::<u64>().ok())
        .unwrap_or(0)
        + 1;
    let _ = std::fs::write(&path, n.to_string());
    n
}

fn write_report() {
    let Some(path) = env("FAKE_REPORT") else {
        return;
    };
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut text = format!("pid={}\nargs={}\n", std::process::id(), args.join(" "));
    for (k, v) in std::env::vars() {
        let _ = writeln!(text, "env:{k}={v}");
    }
    let _ = std::fs::write(path, text);
}

fn respond(stream: &mut TcpStream, status: &str, body: &str) {
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

fn handle(mut stream: TcpStream, token: &str, port: u16, initialized: &AtomicBool) {
    if flag("FAKE_HANG_HEALTH") {
        std::thread::sleep(Duration::from_hours(1));
        return;
    }
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut reader = BufReader::new(&stream);
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return;
    }
    let mut got_token = String::new();
    let mut host = String::new();
    let mut length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() || line.trim().is_empty() {
            break;
        }
        let lower = line.to_ascii_lowercase();
        if let Some(v) = lower.strip_prefix("content-length:") {
            length = v.trim().parse().unwrap_or(0);
        }
        if let Some((_, v)) = line.split_once(':') {
            let name = line.split(':').next().unwrap_or("").to_ascii_lowercase();
            match name.as_str() {
                "x-cl-token" => got_token = v.trim().to_string(),
                "host" => host = v.trim().to_string(),
                _ => {}
            }
        }
    }
    let mut body = vec![0u8; length];
    let _ = reader.read_exact(&mut body);
    drop(reader);
    if got_token != token {
        respond(
            &mut stream,
            "401 Unauthorized",
            r#"{"error":{"code":"DESKTOP_TOKEN_REQUIRED"}}"#,
        );
        return;
    }
    if host != format!("127.0.0.1:{port}") && host != format!("localhost:{port}") {
        respond(
            &mut stream,
            "421 Misdirected Request",
            r#"{"error":{"code":"HOST_NOT_ALLOWED"}}"#,
        );
        return;
    }
    let mut parts = request_line.split_whitespace();
    let (method, path) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    let superuser = flag("FAKE_SUPERUSER");
    match (method, path) {
        ("GET", "/health") => respond(
            &mut stream,
            "200 OK",
            &format!(
                r#"{{"status":"ok","db":"ok","redis":"disabled","mode":"desktop","version":"fake","engineCommit":"{}","schemaVersion":216,"checks":{{"db":{{"ok":true,"superuser":{superuser}}}}}}}"#,
                env("FAKE_COMMIT").unwrap_or_else(|| "fakecommit".into())
            ),
        ),
        ("GET", "/api/v1/desktop/info") => {
            let init = initialized.load(Ordering::SeqCst);
            let ws = if init { r#""ws-1""# } else { "null" };
            respond(
                &mut stream,
                "200 OK",
                &format!(
                    r#"{{"mode":"desktop","initialized":{init},"workspaceTenantId":{ws},"dbSuperuser":{superuser}}}"#
                ),
            );
        }
        ("POST", "/api/v1/desktop/initialize") => {
            if initialized.swap(true, Ordering::SeqCst) {
                respond(
                    &mut stream,
                    "409 Conflict",
                    r#"{"error":{"code":"ALREADY_INITIALIZED","message":"already"}}"#,
                );
            } else {
                let has_company = String::from_utf8_lossy(&body).contains("\"company\"");
                let company = if has_company { r#""co-1""# } else { "null" };
                respond(
                    &mut stream,
                    "201 Created",
                    &format!(
                        r#"{{"workspaceTenantId":"ws-1","userId":"user-1","companyTenantId":{company},"warnings":[]}}"#
                    ),
                );
            }
        }
        _ => respond(
            &mut stream,
            "404 Not Found",
            r#"{"error":{"code":"NOT_FOUND"}}"#,
        ),
    }
}

fn main() {
    let start = count_start();
    write_report();
    if flag("FAKE_FATAL") {
        eprintln!(r#"{{"event":"fatal","code":"CL_ENV_INVALID","message":"bad configuration"}}"#);
        std::process::exit(78);
    }
    if number("FAKE_FAIL_FIRST").is_some_and(|n| start <= n) {
        eprintln!("fake engine failing on purpose (start {start})");
        std::process::exit(1);
    }
    let token = env("CL_TOKEN").unwrap_or_default();
    let bind = env("CL_BIND").unwrap_or_else(|| "127.0.0.1:0".into());
    let listener = match TcpListener::bind(&bind) {
        Ok(l) => l,
        Err(e) => {
            eprintln!(r#"{{"event":"fatal","code":"BIND_FAILED","message":"{e}"}}"#);
            std::process::exit(1);
        }
    };
    let port = listener.local_addr().map_or(0, |a| a.port());
    if flag("FAKE_NO_READY") {
        std::thread::sleep(Duration::from_hours(1));
    }
    println!(r#"{{"event":"listening","host":"127.0.0.1","port":{port}}}"#);
    let _ = std::io::stdout().flush();
    if let Some(ms) = number("FAKE_DIE_AFTER_MS") {
        let die_first = number("FAKE_DIE_FIRST").unwrap_or(u64::MAX);
        if start <= die_first {
            let leak = flag("FAKE_LEAK_TOKEN");
            let token = token.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(ms));
                if leak {
                    eprintln!("about to die, token={token}");
                }
                eprintln!("fake engine dying on purpose");
                std::process::exit(1);
            });
        }
    }
    let initialized = Arc::new(AtomicBool::new(false));
    for conn in listener.incoming().flatten() {
        let (token, flag) = (token.clone(), Arc::clone(&initialized));
        std::thread::spawn(move || handle(conn, &token, port, &flag));
    }
}
