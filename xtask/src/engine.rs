//! `cargo xtask engine [--smoke] [--force]`: build the `MyBooks` engine into a
//! Node single executable application (SEA) for the host platform.
//!
//! Pipeline: read the engine's Node requirement, `npm ci` + build the engine,
//! bundle `packages/api/dist` into one `CommonJS` file with esbuild (see
//! `xtask/engine-tools/bundle.mjs`), build the SEA blob, inject it into a copy
//! of the host Node binary, and write `manifest.json`. The engine submodule is
//! never modified. Results are cached by engine commit + Node version + target.

use anyhow::{Context, Result, anyhow, bail};
use std::collections::hash_map::RandomState;
use std::fs;
use std::hash::{BuildHasher, Hasher};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Fuse string baked into every official Node binary (documented by Node's SEA docs).
const SEA_FUSE: &str = "NODE_SEA_FUSE_fce680ab2cc467b6e072b8b5df1996b2";

/// Host platform names used for `target/engine/<target>` and bcrypt prebuilds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// Directory name under `target/engine/`.
    pub dir: &'static str,
    /// Platform tag used by native prebuild folders (`process.platform-arch`).
    pub node_tag: &'static str,
    pub exe_name: &'static str,
}

/// Map `std::env::consts` to a supported target.
///
/// # Errors
/// Returns an error for platforms `ClientLedger` does not ship.
pub fn host_target() -> Result<Target> {
    target_for(std::env::consts::OS, std::env::consts::ARCH)
}

fn target_for(os: &str, arch: &str) -> Result<Target> {
    Ok(match (os, arch) {
        ("windows", "x86_64") => Target {
            dir: "windows-x64",
            node_tag: "win32-x64",
            exe_name: "clientledger-engine.exe",
        },
        ("macos", "aarch64") => Target {
            dir: "macos-arm64",
            node_tag: "darwin-arm64",
            exe_name: "clientledger-engine",
        },
        ("macos", "x86_64") => Target {
            dir: "macos-x64",
            node_tag: "darwin-x64",
            exe_name: "clientledger-engine",
        },
        ("linux", "x86_64") => Target {
            dir: "linux-x64",
            node_tag: "linux-x64",
            exe_name: "clientledger-engine",
        },
        _ => bail!("unsupported host platform {os}/{arch}"),
    })
}

/// Extract the required Node major version from `.nvmrc` (preferred) or the
/// `engines.node` range in `package.json` (first number in the range).
#[must_use]
pub fn required_node_major(nvmrc: Option<&str>, package_json: &str) -> Option<u32> {
    let first_number = |s: &str| -> Option<u32> {
        let digits: String = s
            .trim()
            .trim_start_matches(|c: char| !c.is_ascii_digit())
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        digits.parse().ok()
    };
    if let Some(v) = nvmrc.and_then(first_number) {
        return Some(v);
    }
    let engines = package_json.find("\"engines\"")?;
    let rest = &package_json[engines..];
    let node = rest.find("\"node\"")?;
    let after = &rest[node + "\"node\"".len()..];
    let colon = after.find(':')?;
    let value = after[colon + 1..].trim_start().strip_prefix('"')?;
    first_number(value.split('"').next()?)
}

/// Major component of a `v24.15.0` style version string.
#[must_use]
pub fn node_major(version: &str) -> Option<u32> {
    version
        .trim()
        .trim_start_matches('v')
        .split('.')
        .next()?
        .parse()
        .ok()
}

/// Cache key: engine commit + Node version + target.
#[must_use]
pub fn cache_key(commit: &str, node_version: &str, target: &Target) -> String {
    let short: String = commit.chars().take(12).collect();
    format!(
        "{short}-node{}-{}",
        node_version.trim_start_matches('v'),
        target.dir
    )
}

/// RFC 3339 UTC timestamp from Unix seconds (no external time crate needed).
#[must_use]
pub fn rfc3339_utc(secs: u64) -> String {
    let days = i64::try_from(secs / 86_400).unwrap_or(0);
    let rem = secs % 86_400;
    // Civil-from-days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3_600,
        rem % 3_600 / 60,
        rem % 60
    )
}

/// Render `manifest.json`.
#[must_use]
pub fn manifest_json(
    commit: &str,
    dirty: bool,
    node_version: &str,
    bundle_sha256: &str,
    build_time: &str,
    target: &Target,
) -> String {
    format!(
        "{{\n  \"schema\": 1,\n  \"engineCommit\": \"{commit}\",\n  \"engineDirty\": {dirty},\n  \"nodeVersion\": \"{}\",\n  \"bundleSha256\": \"{bundle_sha256}\",\n  \"buildTime\": \"{build_time}\",\n  \"target\": \"{}\"\n}}\n",
        node_version.trim_start_matches('v'),
        target.dir
    )
}

/// Value baked into the SEA banner and reported as `engineCommit` on `/health`:
/// the submodule commit, plus `-dirty` when tracked engine files were modified.
#[must_use]
pub fn banner_commit(commit: &str, dirty: bool) -> String {
    if dirty {
        format!("{commit}-dirty")
    } else {
        commit.to_string()
    }
}

/// Pull a string field out of the flat manifest we wrote ourselves.
#[must_use]
pub fn manifest_field<'a>(manifest: &'a str, key: &str) -> Option<&'a str> {
    let marker = format!("\"{key}\": \"");
    let start = manifest.find(&marker)? + marker.len();
    manifest[start..].split('"').next()
}

pub(crate) fn repo_root() -> Result<PathBuf> {
    // xtask lives at <root>/xtask.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| anyhow!("cannot locate repository root"))
}

fn output_dir(root: &Path, target: &Target) -> PathBuf {
    root.join("target").join("engine").join(target.dir)
}

fn node_cmd(args: &[&str]) -> Command {
    let mut c = Command::new("node");
    c.args(args);
    c
}

pub(crate) fn npx() -> Command {
    if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.args(["/C", "npx"]);
        c
    } else {
        Command::new("npx")
    }
}

fn npm() -> Command {
    if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.args(["/C", "npm"]);
        c
    } else {
        Command::new("npm")
    }
}

pub(crate) fn run(mut cmd: Command, what: &str) -> Result<()> {
    println!("==> {what}");
    let status = cmd
        .status()
        .with_context(|| format!("could not start `{what}`"))?;
    if !status.success() {
        bail!("`{what}` failed ({status})");
    }
    Ok(())
}

fn capture(mut cmd: Command, what: &str) -> Result<String> {
    let out = cmd
        .output()
        .with_context(|| format!("could not start `{what}`"))?;
    if !out.status.success() {
        bail!("`{what}` failed ({})", out.status);
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn engine_commit(engine: &Path) -> Result<(String, bool)> {
    let git = |args: &[&str]| -> Result<String> {
        let mut c = Command::new("git");
        c.arg("-C").arg(engine).args(args);
        capture(c, "git")
    };
    let commit = git(&["rev-parse", "HEAD"])
        .context("engine/ is missing: run `git submodule update --init engine`")?;
    // Tracked-file changes only; build output and node_modules are ignored by the engine.
    let dirty = !git(&["status", "--porcelain", "--untracked-files=no"])?.is_empty();
    Ok((commit, dirty))
}

fn check_node(engine: &Path) -> Result<String> {
    let version = capture(node_cmd(&["--version"]), "node --version").context(
        "Node.js is required to build the engine executable (it is not needed to run it)",
    )?;
    let nvmrc = fs::read_to_string(engine.join(".nvmrc")).ok();
    let pkg = fs::read_to_string(engine.join("package.json")).context("engine/package.json")?;
    let want = required_node_major(nvmrc.as_deref(), &pkg)
        .ok_or_else(|| anyhow!("could not read the engine's Node version (.nvmrc or engines)"))?;
    let have =
        node_major(&version).ok_or_else(|| anyhow!("unexpected node version `{version}`"))?;
    if have < want {
        bail!("engine needs Node >= {want}, found {version}");
    }
    Ok(version)
}

/// Sign hooks. macOS needs a signature after injection or the kernel kills the
/// binary; the identity comes from `CL_CODESIGN_IDENTITY` (ad-hoc by default).
/// On any OS `CL_SIGN_HOOK` may name a program that is run with the executable
/// path as its only argument (release signing lands in P35).
fn sign_hooks(exe: &Path) -> Result<()> {
    if cfg!(target_os = "macos") {
        let identity = std::env::var("CL_CODESIGN_IDENTITY").unwrap_or_else(|_| "-".into());
        let mut c = Command::new("codesign");
        c.args(["--force", "--sign", &identity]).arg(exe);
        run(c, "codesign")?;
    }
    if let Some(hook) = std::env::var_os("CL_SIGN_HOOK") {
        let mut c = Command::new(hook);
        c.arg(exe);
        run(c, "CL_SIGN_HOOK")?;
    }
    Ok(())
}

fn build(root: &Path, target: &Target, force: bool) -> Result<PathBuf> {
    let engine = root.join("engine");
    let out = output_dir(root, target);
    let exe = out.join(target.exe_name);
    let (commit, dirty) = engine_commit(&engine)?;
    let node_version = check_node(&engine)?;
    let key = cache_key(&commit, &node_version, target);
    let cache = root.join("target").join("engine").join(".cache").join(&key);

    if !force
        && !dirty
        && cache.join(target.exe_name).exists()
        && cache.join("manifest.json").exists()
    {
        println!("engine: cache hit {key}");
        fs::create_dir_all(&out)?;
        fs::copy(cache.join(target.exe_name), &exe)?;
        fs::copy(cache.join("manifest.json"), out.join("manifest.json"))?;
        return Ok(exe);
    }

    // Tooling (esbuild, postject), pinned by xtask/engine-tools/package-lock.json.
    let tools = root.join("xtask").join("engine-tools");
    let mut c = npm();
    c.args(["ci", "--no-audit", "--no-fund"])
        .current_dir(&tools);
    run(c, "npm ci (engine-tools)")?;

    // Engine dependencies and build. Lifecycle scripts are skipped: the Chromium
    // download and git hooks are not wanted, and bcrypt ships prebuilt binaries.
    let mut c = npm();
    c.args(["ci", "--ignore-scripts", "--no-audit", "--no-fund"])
        .env("PUPPETEER_SKIP_DOWNLOAD", "1")
        .current_dir(&engine);
    run(c, "npm ci (engine)")?;
    // The API build (tsc) outgrew Node's default 2 GB heap on the hosted macOS
    // runners; an owner-set NODE_OPTIONS still wins.
    let node_options =
        std::env::var("NODE_OPTIONS").unwrap_or_else(|_| "--max-old-space-size=6144".to_owned());
    for ws in ["@kis-books/shared", "@kis-books/api"] {
        let mut c = npm();
        c.args(["run", "build", "--workspace", ws])
            .env("NODE_OPTIONS", &node_options)
            .current_dir(&engine);
        run(c, &format!("npm run build ({ws})"))?;
    }

    let work = root
        .join("target")
        .join("engine")
        .join(".work")
        .join(target.dir);
    let mut c = Command::new("node");
    c.arg(tools.join("bundle.mjs"))
        .arg(&engine)
        .arg(&work)
        .arg(target.node_tag)
        .arg(banner_commit(&commit, dirty));
    run(c, "bundle engine")?;
    let bundle_sha = fs::read_to_string(work.join("bundle.sha256"))?
        .trim()
        .to_string();

    let mut c = Command::new("node");
    c.arg("--experimental-sea-config")
        .arg(work.join("sea-config.json"));
    run(c, "node SEA blob")?;

    // Copy of the host Node binary.
    let node_path = capture(node_cmd(&["-p", "process.execPath"]), "node")?;
    fs::create_dir_all(&out)?;
    if exe.exists() {
        fs::remove_file(&exe)?;
    }
    fs::copy(&node_path, &exe).with_context(|| format!("copy {node_path}"))?;
    if cfg!(target_os = "macos") {
        let mut c = Command::new("codesign");
        c.arg("--remove-signature").arg(&exe);
        run(c, "codesign --remove-signature")?;
    }
    let mut c = Command::new("node");
    c.arg(
        tools
            .join("node_modules")
            .join("postject")
            .join("dist")
            .join("cli.js"),
    )
    .arg(&exe)
    .arg("NODE_SEA_BLOB")
    .arg(work.join("sea-prep.blob"))
    .args(["--sentinel-fuse", SEA_FUSE]);
    if cfg!(target_os = "macos") {
        c.args(["--macho-segment-name", "NODE_SEA"]);
    }
    run(c, "postject")?;
    sign_hooks(&exe)?;

    let secs = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let manifest = manifest_json(
        &commit,
        dirty,
        &node_version,
        &bundle_sha,
        &rfc3339_utc(secs),
        target,
    );
    fs::write(out.join("manifest.json"), &manifest)?;

    if !dirty {
        fs::create_dir_all(&cache)?;
        fs::copy(&exe, cache.join(target.exe_name))?;
        fs::write(cache.join("manifest.json"), &manifest)?;
    }
    println!("engine: built {}", exe.display());
    Ok(exe)
}

// ---------------------------------------------------------------- smoke test

fn free_port() -> Result<u16> {
    let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    Ok(l.local_addr()?.port())
}

/// Minimal HTTP/1.1 GET against loopback (avoids an HTTP client dependency).
/// The `Host` header carries the port because the desktop gate checks it.
fn http_get(port: u16, path: &str, token: Option<&str>) -> Result<(u16, String)> {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_secs(2))?;
    s.set_read_timeout(Some(Duration::from_secs(5)))?;
    let token_header = token.map_or_else(String::new, |t| format!("X-CL-Token: {t}\r\n"));
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{token_header}Connection: close\r\n\r\n"
    );
    s.write_all(request.as_bytes())?;
    let mut raw = Vec::new();
    s.read_to_end(&mut raw)?;
    let text = String::from_utf8_lossy(&raw).into_owned();
    let status = text
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .ok_or_else(|| anyhow!("malformed HTTP response"))?;
    let body = text
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or_default();
    Ok((status, body))
}

/// A 256-bit launch token as 64 hex digits, seeded from the OS through
/// `RandomState` (each hasher gets fresh random keys). Throwaway use only.
fn random_token() -> String {
    use std::fmt::Write as _;
    (0..4).fold(String::new(), |mut acc, _| {
        let _ = write!(acc, "{:016x}", RandomState::new().build_hasher().finish());
        acc
    })
}

/// Port from the engine's one-line ready message on stdout:
/// `{"event":"listening","host":"127.0.0.1","port":54321}`.
#[must_use]
pub fn parse_ready_port(line: &str) -> Option<u16> {
    if !line.contains("\"event\":\"listening\"") {
        return None;
    }
    let rest = &line[line.find("\"port\":")? + "\"port\":".len()..];
    let digits: String = rest
        .trim_start()
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

/// Value of a flat string field (`"key":"value"`) in a JSON body.
#[must_use]
pub fn json_str_field<'a>(body: &'a str, key: &str) -> Option<&'a str> {
    let marker = format!("\"{key}\":\"");
    let start = body.find(&marker)? + marker.len();
    body[start..].split('"').next()
}

/// Run SQL statements (one per line) with the engine's own `pg` driver, since
/// the pinned `PostgreSQL` archives ship no `psql` (Q-P03a). Needs the engine's
/// npm dependencies (`npm ci` in `engine/`).
pub(crate) fn node_sql(root: &Path, url: &str, statements: &[&str]) -> Result<()> {
    const SCRIPT: &str = "const {Client}=require('pg');(async()=>{const c=new Client({connectionString:process.env.CL_PGURL});await c.connect();for(const q of process.env.CL_SQL.split('\\n'))await c.query(q);await c.end()})().catch(e=>{console.error(e.message);process.exit(1)})";
    let mut c = Command::new("node");
    c.args(["-e", SCRIPT])
        .env("CL_PGURL", url)
        .env("CL_SQL", statements.join("\n"))
        .current_dir(root.join("engine"));
    run(c, "node pg (setup SQL)")
}

/// True when the engine's npm dependencies (including `pg`) are installed.
pub(crate) fn engine_deps_installed(root: &Path) -> bool {
    root.join("engine")
        .join("node_modules")
        .join("pg")
        .join("package.json")
        .exists()
}

/// A throwaway `PostgreSQL`, from local binaries or Docker. Stops on drop.
enum TempPg {
    Local {
        child: Child,
        dir: PathBuf,
    },
    /// Started through `pg_ctl`, which drops administrator rights on Windows.
    /// `postgres.exe` refuses to run elevated, as it is on hosted CI runners.
    PgCtl {
        bin: PathBuf,
        dir: PathBuf,
    },
    Docker {
        id: String,
    },
}

impl Drop for TempPg {
    fn drop(&mut self) {
        match self {
            TempPg::Local { child, dir } => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = fs::remove_dir_all(dir);
            }
            TempPg::PgCtl { bin, dir } => {
                let _ = Command::new(bin.join("pg_ctl.exe"))
                    .args(["stop", "-m", "immediate", "-w", "-D"])
                    .arg(&dir)
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
                let _ = fs::remove_dir_all(dir);
            }
            TempPg::Docker { id } => {
                let _ = Command::new("docker").args(["stop", id]).output();
            }
        }
    }
}

fn pg_bin_candidates() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = Vec::new();
    if let Some(p) = std::env::var_os("CL_SMOKE_PG_BIN") {
        v.push(PathBuf::from(p));
    }
    if let Some(p) = std::env::var_os("PGBIN") {
        v.push(PathBuf::from(p));
    }
    if let Some(path) = std::env::var_os("PATH") {
        v.extend(std::env::split_paths(&path));
    }
    let globs = [
        "/usr/lib/postgresql",
        "C:/Program Files/PostgreSQL",
        "/opt/homebrew/opt",
        "/usr/local/opt",
    ];
    for g in globs {
        let Ok(rd) = fs::read_dir(g) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if g.ends_with("/opt") {
                if e.file_name().to_string_lossy().starts_with("postgresql@") {
                    v.push(p.join("bin"));
                }
            } else {
                v.push(p.join("bin"));
            }
        }
    }
    v
}

fn find_pg_bin() -> Option<PathBuf> {
    let ext = if cfg!(windows) { ".exe" } else { "" };
    pg_bin_candidates().into_iter().find(|d| {
        d.join(format!("initdb{ext}")).exists() && d.join(format!("postgres{ext}")).exists()
    })
}

fn docker_ready() -> bool {
    Command::new("docker")
        .args(["info"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn start_pg(work: &Path, port: u16) -> Result<Option<TempPg>> {
    if let Some(bin) = find_pg_bin() {
        println!("smoke: using local PostgreSQL in {}", bin.display());
        let ext = if cfg!(windows) { ".exe" } else { "" };
        let dir = work.join("pgdata");
        let mut c = Command::new(bin.join(format!("initdb{ext}")));
        c.args(["-A", "trust", "-U", "postgres", "-E", "UTF8", "-D"])
            .arg(&dir)
            .stdout(Stdio::null());
        run(c, "initdb")?;
        if cfg!(windows) {
            let mut c = Command::new(bin.join("pg_ctl.exe"));
            c.arg("start")
                .arg("-D")
                .arg(&dir)
                .arg("-l")
                .arg(work.join("postgres.log"))
                .arg("-o")
                .arg(format!("-p {port} -c listen_addresses=127.0.0.1"))
                .stdout(Stdio::null());
            run(c, "pg_ctl start")?;
            return Ok(Some(TempPg::PgCtl { bin, dir }));
        }
        let log = fs::File::create(work.join("postgres.log"))?;
        let mut c = Command::new(bin.join(format!("postgres{ext}")));
        c.arg("-D")
            .arg(&dir)
            .args(["-p", &port.to_string(), "-c", "listen_addresses=127.0.0.1"])
            .stdin(Stdio::null())
            .stdout(Stdio::from(log.try_clone()?))
            .stderr(Stdio::from(log));
        if cfg!(unix) {
            c.arg("-c")
                .arg(format!("unix_socket_directories={}", dir.display()));
        }
        let child = c.spawn().context("start postgres")?;
        return Ok(Some(TempPg::Local { child, dir }));
    }
    if docker_ready() {
        println!("smoke: using Docker postgres:16");
        let id = capture(
            {
                let mut c = Command::new("docker");
                c.args([
                    "run",
                    "-d",
                    "--rm",
                    "-e",
                    "POSTGRES_HOST_AUTH_METHOD=trust",
                    "-p",
                ])
                .arg(format!("127.0.0.1:{port}:5432"))
                .arg("postgres:16-alpine");
                c
            },
            "docker run postgres",
        )?;
        return Ok(Some(TempPg::Docker { id }));
    }
    Ok(None)
}

pub(crate) fn wait_for_port(port: u16, what: &str, limit: Duration) -> Result<()> {
    let start = Instant::now();
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    while start.elapsed() < limit {
        if TcpStream::connect_timeout(&addr, Duration::from_millis(500)).is_ok() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    bail!("{what} did not open port {port} within {limit:?}")
}

/// Run the built executable against a temporary Postgres and poll `/health`.
fn smoke(root: &Path, target: &Target) -> Result<()> {
    let out = output_dir(root, target);
    let exe = out.join(target.exe_name);
    let manifest = fs::read_to_string(out.join("manifest.json")).context("manifest.json")?;
    let commit = manifest_field(&manifest, "engineCommit")
        .unwrap_or_default()
        .to_string();

    let required = std::env::var_os("CL_SMOKE_REQUIRE").is_some_and(|v| v == "1");
    let work = root.join("target").join("engine").join(".smoke");
    let _ = fs::remove_dir_all(&work);
    fs::create_dir_all(&work)?;
    let pg_port = free_port()?;
    let Some(pg) = start_pg(&work, pg_port)? else {
        let msg = "no PostgreSQL available (install PostgreSQL 16 or start Docker; set CL_SMOKE_PG_BIN to a bin directory)";
        if required {
            bail!("engine smoke: {msg}");
        }
        println!("engine smoke: SKIPPED: {msg}. CI runs it (CL_SMOKE_REQUIRE=1).");
        return Ok(());
    };
    if let Err(e) = wait_for_port(pg_port, "postgres", Duration::from_mins(1)) {
        if let Ok(log) = fs::read_to_string(work.join("postgres.log")) {
            eprintln!(
                "--- postgres.log ---
{log}
--- end postgres.log ---"
            );
        }
        return Err(e);
    }
    std::thread::sleep(Duration::from_secs(2)); // let recovery finish for Docker's init restart

    let restricted = engine_deps_installed(root);
    let database_url = smoke_database_url(root, pg_port, restricted)?;

    let token = random_token();
    let log = fs::File::create(work.join("engine.log"))?;
    let mut cmd = Command::new(&exe);
    // Minimal environment: no PATH, so Node cannot be found even if installed.
    // No REDIS_URL and no PORT: desktop mode needs neither.
    cmd.env_clear()
        .env("CL_MODE", "desktop")
        .env("CL_BIND", "127.0.0.1:0")
        .env("CL_TOKEN", &token)
        .env("CL_DATA_DIR", work.join("data"))
        .env("DATABASE_URL", database_url)
        .env("JWT_SECRET", "s".repeat(40))
        .env("ENCRYPTION_KEY", "e".repeat(64))
        .env("PLAID_ENCRYPTION_KEY", "p".repeat(64))
        .env("NODE_ENV", "production")
        .env("CL_ENGINE_RUNTIME_DIR", work.join("runtime"))
        .env("TEMP", &work)
        .env("TMP", &work)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::from(log.try_clone()?));
    if restricted {
        cmd.env("CL_REQUIRE_NON_SUPERUSER", "1");
    }
    if let Some(root_dir) = std::env::var_os("SystemRoot") {
        cmd.env("SystemRoot", root_dir);
    }
    let mut child = cmd
        .spawn()
        .with_context(|| format!("start {}", exe.display()))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("engine stdout is not piped"))?;
    let ready_rx = forward_stdout(stdout, log);

    let expected = banner_commit(&commit, manifest.contains("\"engineDirty\": true"));
    let result = wait_ready(&mut child, &ready_rx)
        .and_then(|port| check_health(port, &token, &expected, restricted));
    let _ = child.kill();
    let _ = child.wait();
    drop(pg);
    if result.is_err() {
        if let Ok(log) = fs::read_to_string(work.join("engine.log")) {
            let tail: Vec<&str> = log.lines().rev().take(30).collect();
            eprintln!("--- engine log (last lines) ---");
            for l in tail.iter().rev() {
                eprintln!("{l}");
            }
        }
    } else {
        let _ = fs::remove_dir_all(&work);
    }
    result
}

/// Database URL for the engine. When the engine's npm dependencies are
/// installed it is a non-superuser role that owns its own database, the way P04
/// will run it (E1 supports `CL_REQUIRE_NON_SUPERUSER=1`). Creating the role
/// needs the engine's `pg` driver (no psql in the pinned archives, Q-P03a), so
/// without `engine/node_modules` the superuser is used and a note is printed.
fn smoke_database_url(root: &Path, pg_port: u16, restricted: bool) -> Result<String> {
    if !restricted {
        println!(
            "smoke: note: engine/node_modules missing, so the engine runs as the PostgreSQL superuser (run `npm ci` in engine/ to test a restricted role)"
        );
        return Ok(format!("postgres://postgres@127.0.0.1:{pg_port}/postgres"));
    }
    node_sql(
        root,
        &format!("postgres://postgres@127.0.0.1:{pg_port}/postgres"),
        &[
            "CREATE ROLE cl_smoke LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE",
            "CREATE DATABASE cl_smoke OWNER cl_smoke",
        ],
    )?;
    Ok(format!("postgres://cl_smoke@127.0.0.1:{pg_port}/cl_smoke"))
}

/// Copy the engine's stdout into `sink` (engine.log) on a thread and report
/// the port from its ready line.
fn forward_stdout(stdout: impl Read + Send + 'static, mut sink: fs::File) -> mpsc::Receiver<u16> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            let _ = writeln!(sink, "{line}");
            if let Some(port) = parse_ready_port(&line) {
                let _ = tx.send(port);
            }
        }
    });
    rx
}

/// Wait for the engine's ready line (it prints it after migrations and bind).
fn wait_ready(child: &mut Child, ready: &mpsc::Receiver<u16>) -> Result<u16> {
    let start = Instant::now();
    let limit = Duration::from_mins(2);
    loop {
        if let Some(status) = child.try_wait()? {
            bail!("engine exited early ({status})");
        }
        match ready.recv_timeout(Duration::from_millis(500)) {
            Ok(port) => {
                println!(
                    "smoke: engine ready on port {port} after {:?}",
                    start.elapsed()
                );
                return Ok(port);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                bail!("engine closed stdout before printing its ready line")
            }
        }
        if start.elapsed() > limit {
            bail!("engine did not print its ready line within {limit:?}");
        }
    }
}

/// The checks the smoke run makes against a ready engine. `/health` must
/// refuse a request without the launch token, then return HTTP 200 with the
/// database up, the desktop mode, and exactly the pinned engine commit.
fn check_health(port: u16, token: &str, expected_commit: &str, restricted: bool) -> Result<()> {
    check_health_within(port, token, expected_commit, restricted, QUEUE_WARMUP)
}

/// How long the smoke run waits for the job queue to come up after the
/// ready line (pg-boss initialises after the listener on a slow runner).
const QUEUE_WARMUP: Duration = Duration::from_secs(30);

/// A 503 that is only the job queue still starting: the database is ok
/// and the `queue` check is the one that is not.
fn queue_still_warming(code: u16, body: &str) -> bool {
    code == 503
        && body.contains("\"db\":\"ok\"")
        && json_str_field(body, "queue").is_some_and(|q| q != "ok")
}

fn check_health_within(
    port: u16,
    token: &str,
    expected_commit: &str,
    restricted: bool,
    warmup: Duration,
) -> Result<()> {
    let (code, _) = http_get(port, "/health", None)?;
    if code != 401 {
        bail!("/health without the launch token returned {code}, expected 401");
    }
    let started = Instant::now();
    let (code, body) = loop {
        let (code, body) = http_get(port, "/health", Some(token))?;
        if !queue_still_warming(code, &body) || started.elapsed() >= warmup {
            break (code, body);
        }
        println!("smoke: job queue still starting, waiting");
        std::thread::sleep(Duration::from_secs(1));
    };
    let shown: String = body.chars().take(400).collect();
    if code != 200 {
        bail!("/health returned {code}, expected 200: {shown}");
    }
    if !body.contains("\"db\":\"ok\"") {
        bail!("/health does not report the database as ok: {shown}");
    }
    if !body.contains("\"mode\":\"desktop\"") {
        bail!("/health does not report desktop mode: {shown}");
    }
    let reported = json_str_field(&body, "engineCommit").unwrap_or_default();
    if reported != expected_commit {
        bail!("/health engineCommit `{reported}` is not the pinned commit `{expected_commit}`");
    }
    if restricted && !body.contains("\"superuser\":false") {
        bail!("engine is not running as a non-superuser role: {shown}");
    }
    let (pc, _) = http_get(port, "/ping", Some(token))?;
    if pc != 200 {
        bail!("/ping returned {pc}");
    }
    println!("smoke: /health HTTP 200, database ok, engineCommit {reported}");
    Ok(())
}

/// `cargo xtask engine [--smoke] [--force]`
pub fn engine(args: &[String]) -> Result<()> {
    let mut smoke_only = false;
    let mut force = false;
    for a in args {
        match a.as_str() {
            "--smoke" => smoke_only = true,
            "--force" => force = true,
            other => bail!("unknown engine option `{other}` (use --smoke, --force)"),
        }
    }
    let root = repo_root()?;
    let target = host_target()?;
    build(&root, &target, force)?;
    if smoke_only {
        smoke(&root, &target)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nvmrc_wins_over_engines() {
        assert_eq!(required_node_major(Some("v22.4.1\n"), "{}"), Some(22));
    }

    #[test]
    fn engines_range_is_parsed() {
        let pkg = r#"{ "name": "x", "engines": { "node": ">=24.0.0" } }"#;
        assert_eq!(required_node_major(None, pkg), Some(24));
        assert_eq!(required_node_major(None, "{}"), None);
    }

    #[test]
    fn node_major_parses() {
        assert_eq!(node_major("v24.15.0\n"), Some(24));
        assert_eq!(node_major("garbage"), None);
    }

    #[test]
    fn cache_key_has_commit_node_and_target() {
        let t = target_for("linux", "x86_64").unwrap();
        let k = cache_key("13ebd2ba50db0e3e68dd473329caf96d1f039e4e", "v24.15.0", &t);
        assert_eq!(k, "13ebd2ba50db-node24.15.0-linux-x64");
    }

    #[test]
    fn timestamps_are_rfc3339() {
        assert_eq!(rfc3339_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339_utc(1_700_000_000), "2023-11-14T22:13:20Z");
        assert_eq!(rfc3339_utc(1_709_164_800), "2024-02-29T00:00:00Z");
    }

    #[test]
    fn manifest_round_trips() {
        let t = target_for("windows", "x86_64").unwrap();
        let m = manifest_json("abc", false, "v24.1.0", "ff00", "2026-01-01T00:00:00Z", &t);
        assert_eq!(manifest_field(&m, "engineCommit"), Some("abc"));
        assert_eq!(manifest_field(&m, "nodeVersion"), Some("24.1.0"));
        assert_eq!(manifest_field(&m, "bundleSha256"), Some("ff00"));
        assert_eq!(manifest_field(&m, "target"), Some("windows-x64"));
    }

    #[test]
    fn banner_commit_marks_dirty_trees() {
        assert_eq!(banner_commit("abc123", false), "abc123");
        assert_eq!(banner_commit("abc123", true), "abc123-dirty");
    }

    #[test]
    fn ready_line_is_parsed() {
        let line = r#"{"event":"listening","host":"127.0.0.1","port":54321}"#;
        assert_eq!(parse_ready_port(line), Some(54321));
        assert_eq!(parse_ready_port(r#"{"event":"fatal","port":1}"#), None);
        assert_eq!(parse_ready_port("plain log line"), None);
    }

    #[test]
    fn json_string_fields_are_read() {
        let body = r#"{"status":"ok","engineCommit":"abc-dirty","mode":"desktop"}"#;
        assert_eq!(json_str_field(body, "engineCommit"), Some("abc-dirty"));
        assert_eq!(json_str_field(body, "missing"), None);
    }

    #[test]
    fn tokens_are_256_bit_hex_and_distinct() {
        let (a, b) = (random_token(), random_token());
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    /// Serve `handler(request)` -> `(status, body)` to every connection until the test ends.
    fn fixture_server(handler: fn(&str) -> (u16, String)) -> u16 {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let mut request = String::new();
                let mut buf = [0_u8; 512];
                while !request.contains("\r\n\r\n") {
                    let n = stream.read(&mut buf).unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    request.push_str(&String::from_utf8_lossy(&buf[..n]));
                }
                let (status, body) = handler(&request);
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        port
    }

    const GOOD: &str = r#"{"status":"ok","db":"ok","mode":"desktop","engineCommit":"abc","checks":{"db":{"superuser":false}}}"#;

    fn gated(request: &str, health: &str) -> (u16, String) {
        if !request.contains("X-CL-Token: tok") {
            return (401, "{}".into());
        }
        if request.starts_with("GET /ping") {
            return (200, "pong".into());
        }
        (200, health.to_string())
    }

    #[test]
    fn health_check_passes_for_a_good_engine() {
        let port = fixture_server(|r| gated(r, GOOD));
        check_health(port, "tok", "abc", true).unwrap();
    }

    #[test]
    fn health_check_rejects_a_commit_mismatch() {
        let port = fixture_server(|r| gated(r, GOOD));
        let err = check_health(port, "tok", "def", true).unwrap_err();
        assert!(err.to_string().contains("pinned commit"), "{err}");
    }

    #[test]
    fn health_check_waits_while_only_the_queue_is_starting() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static CALLS: AtomicUsize = AtomicUsize::new(0);
        const WARMING: &str = r#"{"status":"degraded","db":"ok","queue":"fail","mode":"desktop","engineCommit":"abc","checks":{"db":{"superuser":false}}}"#;
        let port = fixture_server(|r| {
            if !r.contains("X-CL-Token: tok") {
                return (401, "{}".into());
            }
            if r.starts_with("GET /ping") {
                return (200, "pong".into());
            }
            if CALLS.fetch_add(1, Ordering::SeqCst) < 2 {
                (503, WARMING.into())
            } else {
                (200, GOOD.into())
            }
        });
        check_health_within(port, "tok", "abc", true, Duration::from_secs(10)).unwrap();
        assert!(
            CALLS.load(Ordering::SeqCst) >= 3,
            "two 503s were waited out"
        );
    }

    #[test]
    fn health_check_gives_up_on_a_queue_that_never_starts() {
        const WARMING: &str = r#"{"status":"degraded","db":"ok","queue":"fail","mode":"desktop","engineCommit":"abc","checks":{"db":{"superuser":false}}}"#;
        let port = fixture_server(|r| {
            if r.contains("X-CL-Token: tok") {
                (503, WARMING.to_string())
            } else {
                (401, "{}".to_string())
            }
        });
        let err =
            check_health_within(port, "tok", "abc", true, Duration::from_secs(2)).unwrap_err();
        assert!(err.to_string().contains("expected 200"), "{err}");
        assert!(queue_still_warming(503, WARMING));
        assert!(!queue_still_warming(200, WARMING));
        assert!(!queue_still_warming(
            503,
            &WARMING.replace("\"db\":\"ok\"", "\"db\":\"fail\"")
        ));
        assert!(
            !queue_still_warming(503, &GOOD.replace("ok\",\"mode", "degraded\",\"mode")),
            "no queue key is not a warm-up"
        );
    }

    #[test]
    fn health_check_rejects_degraded_503() {
        let port = fixture_server(|r| {
            if r.contains("X-CL-Token: tok") {
                (503, GOOD.replace("ok\",\"mode", "degraded\",\"mode"))
            } else {
                (401, "{}".into())
            }
        });
        let err = check_health(port, "tok", "abc", true).unwrap_err();
        assert!(err.to_string().contains("expected 200"), "{err}");
    }

    #[test]
    fn health_check_requires_the_token_to_be_enforced() {
        let port = fixture_server(|_| (200, GOOD.to_string()));
        let err = check_health(port, "tok", "abc", true).unwrap_err();
        assert!(err.to_string().contains("expected 401"), "{err}");
    }

    #[test]
    fn health_check_rejects_a_superuser_role() {
        let port = fixture_server(|r| gated(r, &GOOD.replace("false", "true")));
        let err = check_health(port, "tok", "abc", true).unwrap_err();
        assert!(err.to_string().contains("non-superuser"), "{err}");
    }

    #[test]
    fn unsupported_hosts_are_rejected() {
        assert!(target_for("freebsd", "x86_64").is_err());
        assert!(target_for("linux", "aarch64").is_err());
    }

    /// The executable must start with no PATH at all, so Node is not on it.
    /// Skips when the engine has not been built (CI sets `CL_REQUIRE_ENGINE_EXE=1`).
    #[test]
    fn built_executable_runs_without_node_on_path() {
        let root = repo_root().unwrap();
        let target = host_target().unwrap();
        let exe = output_dir(&root, &target).join(target.exe_name);
        if !exe.exists() {
            assert!(
                std::env::var_os("CL_REQUIRE_ENGINE_EXE").is_none(),
                "engine executable required but not built: run `cargo xtask engine`"
            );
            println!(
                "skipped: {} not built (run `cargo xtask engine`)",
                exe.display()
            );
            return;
        }
        let mut cmd = Command::new(&exe);
        cmd.env_clear().stdin(Stdio::null());
        if let Some(sr) = std::env::var_os("SystemRoot") {
            cmd.env("SystemRoot", sr);
        }
        let out = cmd.output().unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        // With no DATABASE_URL the engine's own preflight message proves the
        // bundled JavaScript ran inside the single executable.
        assert!(
            stderr.contains("missing required environment variables"),
            "unexpected output: {stderr}"
        );
        assert!(!out.status.success(), "{:?}", out.status);
    }
}
