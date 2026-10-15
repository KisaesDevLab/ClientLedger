//! `cargo xtask engine-test [-- <vitest args>]`: run the engine's own api test
//! suite against a throwaway `PostgreSQL` built from the binaries that
//! `cargo xtask pg-fetch` unpacked into `target/pg/<target>/`.
//!
//! The cluster lives in `target/engine-testdb`, listens on 127.0.0.1:5434 with
//! trust authentication (the engine's CI database, role `kisbooks`), and is
//! stopped and deleted afterwards, even when the tests fail. Redis is not
//! started: `REDIS_URL` points at port 6390 where nothing listens, so suites
//! that need Redis fail or skip by design and the engine's CI stays
//! authoritative for them. Nothing here uses the network.

use crate::engine::{self, Target};
use anyhow::{Context, Result, bail};
use std::fs;
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

/// Port and credentials of the engine CI's test database (`ci.yml`).
const PG_PORT: u16 = 5434;
const PG_USER: &str = "kisbooks";
const TEST_DB: &str = "kisbooks_test";

/// A running throwaway cluster. Stops and deletes itself on drop.
struct TestCluster {
    pg_ctl: PathBuf,
    dir: PathBuf,
}

impl Drop for TestCluster {
    fn drop(&mut self) {
        let _ = Command::new(&self.pg_ctl)
            .args(["stop", "-m", "immediate", "-w", "-D"])
            .arg(&self.dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn exe(bin: &Path, name: &str) -> PathBuf {
    let ext = if cfg!(windows) { ".exe" } else { "" };
    bin.join(format!("{name}{ext}"))
}

/// Split `args` into the vitest arguments: everything after an optional
/// leading `--` (cargo passes it through).
#[must_use]
pub fn vitest_args(args: &[String]) -> &[String] {
    match args.first().map(String::as_str) {
        Some("--") => &args[1..],
        _ => args,
    }
}

fn port_in_use() -> bool {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, PG_PORT));
    TcpStream::connect_timeout(&addr, Duration::from_millis(500)).is_ok()
}

/// `initdb` and `pg_ctl start` a trust-auth cluster, then create the test database.
fn start_cluster(root: &Path, target: &Target) -> Result<TestCluster> {
    let bin = root.join("target").join("pg").join(target.dir).join("bin");
    if !exe(&bin, "initdb").exists() {
        bail!(
            "PostgreSQL binaries not found in {}: run `cargo xtask pg-fetch` first",
            bin.display()
        );
    }
    if port_in_use() {
        bail!("port {PG_PORT} is already in use (the engine tests expect their database there)");
    }
    let dir = root.join("target").join("engine-testdb");
    let _ = fs::remove_dir_all(&dir);
    // Locale `C`: `C.UTF-8` is rejected by PostgreSQL 16 on Windows (Q-P03b).
    let mut c = Command::new(exe(&bin, "initdb"));
    c.args([
        "--auth=trust",
        "-U",
        PG_USER,
        "-E",
        "UTF8",
        "--locale=C",
        "-D",
    ])
    .arg(&dir)
    .stdout(Stdio::null());
    engine::run(c, "initdb (engine test cluster)")?;

    // The guard exists before `start`, so a failed start still cleans up.
    let cluster = TestCluster {
        pg_ctl: exe(&bin, "pg_ctl"),
        dir: dir.clone(),
    };
    // pg_ctl (not postgres directly) so it also works from elevated Windows
    // shells, where postgres.exe refuses to run (Q-P03d).
    let mut c = Command::new(&cluster.pg_ctl);
    c.arg("start")
        .arg("-w")
        .arg("-D")
        .arg(&dir)
        .arg("-l")
        .arg(root.join("target").join("engine-testdb.log"))
        .arg("-o")
        .arg(format!("-p {PG_PORT} -c listen_addresses=127.0.0.1"))
        .stdout(Stdio::null());
    engine::run(c, "pg_ctl start (engine test cluster)")?;
    engine::wait_for_port(PG_PORT, "test postgres", Duration::from_secs(30))?;

    // No psql in the pinned archives (Q-P03a): use the engine's own driver.
    engine::node_sql(
        root,
        &format!("postgres://{PG_USER}@127.0.0.1:{PG_PORT}/postgres"),
        &[&format!("CREATE DATABASE {TEST_DB}")],
    )?;
    Ok(cluster)
}

/// The environment the engine's CI gives its api tests, with Redis pointed at
/// a port nothing listens on.
fn apply_test_env(c: &mut Command) {
    c.env(
        "DATABASE_URL",
        format!("postgresql://{PG_USER}:{PG_USER}@127.0.0.1:{PG_PORT}/{TEST_DB}"),
    )
    .env("REDIS_URL", "redis://127.0.0.1:6390")
    .env("JWT_SECRET", "test-secret-key-for-testing-only");
}

/// `cargo xtask engine-test [-- <vitest args>]`
pub fn engine_test(args: &[String]) -> Result<()> {
    let root = engine::repo_root()?;
    let target = engine::host_target()?;
    if !engine::engine_deps_installed(&root) {
        bail!(
            "engine dependencies are missing: run `cargo xtask engine` (or `npm ci` in engine/) first"
        );
    }
    let _cluster = start_cluster(&root, &target)?;

    let mut c = engine::npx();
    c.args(["tsx", "packages/api/src/migrate.ts"])
        .current_dir(root.join("engine"));
    apply_test_env(&mut c);
    engine::run(c, "migrate the test database")?;

    let mut c = engine::npx();
    c.args(["vitest", "run"])
        .args(vitest_args(args))
        .current_dir(root.join("engine").join("packages").join("api"));
    apply_test_env(&mut c);
    engine::run(c, "vitest run (engine api tests)")
        .context("engine tests failed (Redis suites are expected to fail without Redis)")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn leading_double_dash_is_dropped() {
        let args = strings(&["--", "src/desktop", "-t", "gate"]);
        assert_eq!(
            vitest_args(&args),
            &strings(&["src/desktop", "-t", "gate"])[..]
        );
    }

    #[test]
    fn plain_args_pass_through() {
        let args = strings(&["src/desktop"]);
        assert_eq!(vitest_args(&args), &args[..]);
        assert_eq!(vitest_args(&[]), &Vec::<String>::new()[..]);
    }
}
