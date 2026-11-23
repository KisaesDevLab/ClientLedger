//! Only cl-lan binds a non-loopback address (P33, Q-P33x, review S10).
//!
//! A text scan of every `src/**/*.rs` under `crates/` and `apps/`:
//!
//! - socket binding calls (`TcpListener::bind`, `UdpSocket::bind`,
//!   `TcpSocket::new_v4/new_v6/bind`, an mDNS `ServiceDaemon`, `socket2`)
//!   may appear outside `crates/cl-lan` only in [`LOOPBACK_USERS`], the
//!   existing loopback listeners;
//! - in those files, every binding line before the file's first
//!   `#[cfg(test)]` must name a loopback address (`LOCALHOST`,
//!   `127.0.0.1`, `loopback(`).
//!
//! cl-lan's own listen addresses go through `cl_lan::policy` (private
//! ranges only, Q-P33c), which its unit tests cover.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Existing socket users outside cl-lan, all loopback-only.
pub const LOOPBACK_USERS: [&str; 6] = [
    "crates/cl-host/src/health.rs",
    "crates/cl-host/src/instance_lock.rs",
    "crates/cl-pg/src/ports.rs",
    "crates/cl-automation/src/server.rs",
    "crates/cl-ui/src/engine_link.rs",
    "crates/cl-api/src/client.rs",
];

const LAN_CRATE: &str = "crates/cl-lan/";

fn is_bind_line(line: &str) -> bool {
    let l = line.trim_start();
    if l.starts_with("//") {
        return false;
    }
    [
        "TcpListener::bind",
        "UdpSocket::bind",
        "TcpSocket::new_v",
        "ServiceDaemon",
        "socket2::",
    ]
    .iter()
    .any(|p| l.contains(p))
}

fn names_loopback(line: &str) -> bool {
    ["LOCALHOST", "127.0.0.1", "loopback("]
        .iter()
        .any(|p| line.contains(p))
}

/// Violations in one file (`rel` uses forward slashes).
#[must_use]
pub fn check_file(rel: &str, src: &str) -> Vec<String> {
    if rel.starts_with(LAN_CRATE) {
        return Vec::new();
    }
    let allowed = LOOPBACK_USERS.contains(&rel);
    let mut out = Vec::new();
    let mut in_tests = false;
    for (i, line) in src.lines().enumerate() {
        if line.contains("#[cfg(test)]") {
            in_tests = true;
        }
        if !is_bind_line(line) {
            continue;
        }
        if !allowed {
            out.push(format!(
                "{rel}:{}: binds a socket; only cl-lan and the loopback listeners in xtask/src/bind_check.rs may (Q-P33x)",
                i + 1
            ));
        } else if !in_tests && !names_loopback(line) {
            out.push(format!(
                "{rel}:{}: binds a socket without naming a loopback address; only cl-lan may bind a LAN address (Q-P33x)",
                i + 1
            ));
        }
    }
    out
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))? {
        let p = entry?.path();
        if p.is_dir() {
            rust_files(&p, out)?;
        } else if p.extension().is_some_and(|e| e == "rs") {
            out.push(p);
        }
    }
    Ok(())
}

/// Scans the workspace at `root`.
///
/// # Errors
/// I/O errors reading the tree.
pub fn check(root: &Path) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for top in ["crates", "apps"] {
        let base = root.join(top);
        if !base.exists() {
            continue;
        }
        for entry in std::fs::read_dir(&base)? {
            let pkg = entry?.path();
            let mut files = Vec::new();
            rust_files(&pkg.join("src"), &mut files)?;
            for f in files {
                let rel = f
                    .strip_prefix(root)
                    .unwrap_or(&f)
                    .to_string_lossy()
                    .replace('\\', "/");
                let src = std::fs::read_to_string(&f).with_context(|| format!("read {rel}"))?;
                out.extend(check_file(&rel, &src));
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cl_lan_may_bind_anything() {
        assert_eq!(
            check_file(
                "crates/cl-lan/src/host.rs",
                "TcpListener::bind(SocketAddr::new(ip, port))"
            ),
            Vec::<String>::new()
        );
    }

    #[test]
    fn other_crates_may_not_bind() {
        for src in [
            "let l = TcpListener::bind((ip, 0))?;",
            "let s = UdpSocket::bind(\"0.0.0.0:5353\")?;",
            "let d = mdns_sd::ServiceDaemon::new()?;",
            "let s = socket2::Socket::new(d, t, None)?;",
        ] {
            assert_eq!(check_file("crates/cl-ui/src/x.rs", src).len(), 1, "{src}");
        }
        // Comments are not code.
        assert_eq!(
            check_file("crates/cl-ui/src/x.rs", "// TcpListener::bind(x)"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn loopback_users_must_name_loopback_outside_tests() {
        let f = "crates/cl-pg/src/ports.rs";
        assert_eq!(
            check_file(f, "TcpListener::bind((Ipv4Addr::LOCALHOST, 0))"),
            Vec::<String>::new()
        );
        assert_eq!(
            check_file(f, "TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0))").len(),
            1
        );
        assert_eq!(
            check_file(
                f,
                "fn a() {}\n#[cfg(test)]\nmod t { fn b() { TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0)); } }"
            ),
            Vec::<String>::new()
        );
    }

    #[test]
    fn the_workspace_passes() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        assert_eq!(check(&root).expect("scan"), Vec::<String>::new());
    }
}
