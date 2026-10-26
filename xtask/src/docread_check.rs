//! `cargo xtask docread-check`: the reading worker has no network code and
//! starts nothing (A03 plan D2 "Network", S16; security review M4).
//!
//! The static half of the no-network check, on every OS in
//! `cargo xtask ci` (the Linux `docread-netcheck` CI job is the runtime
//! half, [`netcheck_violations`]):
//!
//! 1. `cargo tree -p clientledger-docread -e normal` for every shipped
//!    target has none of [`DENIED_CRATES`] (sockets, HTTP, TLS, async runtimes);
//! 2. no `std::net` / `core::net` path, socket type or `Command` in
//!    [`SCAN_DIRS`], except [`HOOKS`], which only test builds compile
//!    (its `spawn` hook proves the Windows job refuses a second process).
//!    Comments and strings are not tokens.

use anyhow::{Context, Result, bail};
use proc_macro2::{TokenStream, TokenTree};
use std::path::{Path, PathBuf};

/// Crates that can reach the network, by name.
pub const DENIED_CRATES: &[&str] = &[
    "mio",
    "socket2",
    "tokio",
    "async-std",
    "hyper",
    "h2",
    "h3",
    "reqwest",
    "ureq",
    "curl",
    "curl-sys",
    "isahc",
    "attohttpc",
    "rustls",
    "native-tls",
    "openssl",
    "openssl-sys",
    "quinn",
];

/// The worker's sources and the reading rules it runs.
pub const SCAN_DIRS: [&str; 2] = ["apps/clientledger-docread/src", "crates/cl-docread/src"];

/// The one file allowed a `Command` (test hooks, `test-support` only).
pub const HOOKS: &str = "apps/clientledger-docread/src/hooks.rs";

/// Socket and process identifiers that may not appear.
const DENIED_IDENTS: &[&str] = &[
    "Command",
    "TcpStream",
    "TcpListener",
    "UdpSocket",
    "UnixStream",
    "UnixListener",
    "UnixDatagram",
    "ToSocketAddrs",
];

/// Crates from `cargo tree --prefix none` output (`name vX.Y.Z ...`) that
/// are in [`DENIED_CRATES`].
#[must_use]
pub fn denied_in_tree(tree: &str) -> Vec<String> {
    let mut out: Vec<String> = tree
        .lines()
        .filter_map(|l| l.split_whitespace().next())
        .filter(|name| DENIED_CRATES.contains(name))
        .map(|name| {
            format!("clientledger-docread depends on `{name}`, which can reach the network")
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Lines (1-based) of denied tokens in `src`: a [`DENIED_IDENTS`] name or a
/// `std::net` / `core::net` path. Line 0 when `src` does not parse.
#[must_use]
pub fn denied_tokens(src: &str) -> Vec<(usize, String)> {
    let Ok(ts) = src.parse::<TokenStream>() else {
        return vec![(0, "unparsable file".into())];
    };
    let mut out = Vec::new();
    walk(ts, &mut out);
    out
}

fn walk(ts: TokenStream, out: &mut Vec<(usize, String)>) {
    // The last two tokens: `std` `::` / `core` `::` before `net`.
    let mut path_root: Option<String> = None;
    let mut colons = 0;
    for tt in ts {
        match tt {
            TokenTree::Ident(i) => {
                let name = i.to_string();
                let line = i.span().start().line;
                if DENIED_IDENTS.contains(&name.as_str()) {
                    out.push((line, name.clone()));
                }
                if name == "net"
                    && colons == 2
                    && matches!(path_root.as_deref(), Some("std" | "core"))
                {
                    out.push((line, format!("{}::net", path_root.unwrap_or_default())));
                }
                path_root = Some(name);
                colons = 0;
            }
            TokenTree::Punct(p) if p.as_char() == ':' && path_root.is_some() => {
                colons += 1;
            }
            TokenTree::Group(g) => {
                walk(g.stream(), out);
                path_root = None;
                colons = 0;
            }
            _ => {
                path_root = None;
                colons = 0;
            }
        }
    }
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
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

/// Source findings in the repository at `root`.
///
/// # Errors
/// A folder or file could not be read.
pub fn source_violations(root: &Path) -> Result<Vec<String>> {
    let mut files = Vec::new();
    for d in SCAN_DIRS {
        rust_files(&root.join(d), &mut files)?;
    }
    let mut out = Vec::new();
    for f in files {
        let rel = f
            .strip_prefix(root)
            .unwrap_or(&f)
            .to_string_lossy()
            .replace('\\', "/");
        if rel == HOOKS {
            continue;
        }
        let src = std::fs::read_to_string(&f).with_context(|| format!("read {rel}"))?;
        for (line, what) in denied_tokens(&src) {
            out.push(format!(
                "{rel}:{line}: `{what}` in the reading worker (no network, no child processes)"
            ));
        }
    }
    Ok(out)
}

/// Every violation: the dependency tree and the sources.
///
/// # Errors
/// `cargo tree` failed, or a source could not be read.
pub fn check(root: &Path) -> Result<Vec<String>> {
    let out =
        std::process::Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
            .current_dir(root)
            .args([
                "tree",
                "-p",
                "clientledger-docread",
                "-e",
                "normal",
                // The platforms the app ships on (`all` would add wasm-only
                // dependencies of pdfium-render, such as tokio's timer).
                "--target",
                "x86_64-pc-windows-msvc",
                "--target",
                "x86_64-unknown-linux-gnu",
                "--target",
                "aarch64-apple-darwin",
                "--target",
                "x86_64-apple-darwin",
                "--prefix",
                "none",
            ])
            .output()?;
    if !out.status.success() {
        bail!("cargo tree failed ({})", out.status);
    }
    let mut v = denied_in_tree(&String::from_utf8_lossy(&out.stdout));
    v.extend(source_violations(root)?);
    Ok(v)
}

// ---------------------------------------------------------------------------
// The runtime half: an strace log from the Linux CI job
// ---------------------------------------------------------------------------

/// Network calls in an `strace -f -e trace=socket,connect,sendto,sendmsg`
/// log (`pid syscall(args) = result` lines) that reach beyond the machine:
/// any `AF_INET`/`AF_INET6` socket, connect or send to an address that is
/// not loopback, and any `AF_PACKET` socket. `AF_UNIX` and loopback pass
/// (the test harness's own use, plan §6 "No network").
#[must_use]
pub fn netcheck_violations(log: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in log.lines() {
        let Some(call) = line
            .split_whitespace()
            .find(|w| {
                ["socket(", "connect(", "sendto(", "sendmsg("]
                    .iter()
                    .any(|c| w.starts_with(c))
            })
            .map(|w| w.split('(').next().unwrap_or(""))
        else {
            continue;
        };
        let bad = if call == "socket" {
            // `AF_INET` matches `AF_INET6` too.
            (line.contains("AF_INET") || line.contains("AF_PACKET")) && !line.contains("AF_UNIX")
        } else {
            let inet = line.contains("sa_family=AF_INET");
            let loopback =
                line.contains("inet_addr(\"127.") || line.contains("inet_pton(AF_INET6, \"::1\"");
            inet && !loopback
        };
        if bad {
            out.push(format!("network call: {}", line.trim()));
        }
    }
    out
}

/// `cargo xtask docread-netcheck <strace log>`.
///
/// # Errors
/// The log could not be read, or it shows a network call.
pub fn netcheck(args: &[String]) -> Result<()> {
    let [log] = args else {
        bail!("usage: cargo xtask docread-netcheck <strace log>");
    };
    let text = std::fs::read_to_string(log).with_context(|| format!("read {log}"))?;
    let execs = text.matches("execve(").count();
    let v = netcheck_violations(&text);
    if !text.contains("clientledger-docread") {
        bail!("the log shows no reading worker; the check proved nothing");
    }
    println!("docread-netcheck: {execs} program starts traced");
    crate::report(
        "docread-netcheck",
        "no network call beyond loopback or local sockets",
        &v,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn denied_crates_are_found_by_name() {
        let tree = "clientledger-docread v0.1.0 (C:\\x)\nserde v1.0.0\nmio v1.0.2\nmio v1.0.2\nmiow v0.6.0\ntokio-util v0.7.0\nrustls v0.23.0\n";
        let v = denied_in_tree(tree);
        assert_eq!(v.len(), 2, "{v:?}");
        assert!(v[0].contains("`mio`") && v[1].contains("`rustls`"));
        assert_eq!(
            denied_in_tree("image v0.25.0\npdfium-render v0.9.4\n"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn denied_tokens_ignore_comments_and_strings() {
        let src = "// std::net and Command in a comment\nfn a() { let s = \"std::net::TcpStream\"; }\nuse std::net::TcpStream;\nfn b() { std::process::Command::new(\"x\"); }\nfn c() { core :: net :: Ipv4Addr::LOCALHOST; }\nfn d(net: u8) { let x = self::net; }\n";
        let lines: Vec<usize> = denied_tokens(src).iter().map(|(l, _)| *l).collect();
        assert_eq!(lines, [3, 3, 4, 5]);
        assert_eq!(denied_tokens("fn (")[0].0, 0);
    }

    #[test]
    fn the_worker_and_rules_have_no_network_or_process_code() {
        let root = crate::engine::repo_root().unwrap();
        assert_eq!(source_violations(&root).unwrap(), Vec::<String>::new());
    }

    #[test]
    fn netcheck_flags_only_traffic_beyond_the_machine() {
        let ok = r#"1234  socket(AF_UNIX, SOCK_STREAM|SOCK_CLOEXEC, 0) = 3
1234  connect(3, {sa_family=AF_UNIX, sun_path="/var/run/nscd/socket"}, 110) = -1 ENOENT
1240  connect(5, {sa_family=AF_INET, sin_port=htons(5432), sin_addr=inet_addr("127.0.0.1")}, 16) = 0
1240  sendto(5, "x", 1, 0, NULL, 0) = 1
1241  execve("/w/target/debug/clientledger-docread", ["clientledger-docread"], 0x0 /* 9 vars */) = 0
"#;
        assert_eq!(netcheck_violations(ok), Vec::<String>::new());
        let bad = r#"1250  socket(AF_INET6, SOCK_DGRAM, IPPROTO_IP) = 4
1251  connect(4, {sa_family=AF_INET, sin_port=htons(443), sin_addr=inet_addr("93.184.216.34")}, 16) = -1
1252  sendto(4, "q", 1, 0, {sa_family=AF_INET6, sin6_port=htons(53), sin6_flowinfo=0, inet_pton(AF_INET6, "2001:db8::1", &sin6_addr)}, 28) = 1
1253  socket(AF_PACKET, SOCK_RAW, 768) = 5
"#;
        assert_eq!(netcheck_violations(bad).len(), 4);
    }
}
