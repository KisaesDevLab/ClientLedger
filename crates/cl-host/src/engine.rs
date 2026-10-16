//! Launching the engine executable.
//!
//! The environment is built from scratch (never inherited), the launch token
//! travels only in that environment, and the engine's ready line
//! (`{"event":"listening","host":"127.0.0.1","port":54321}`) tells us the port
//! the operating system picked for `CL_BIND=127.0.0.1:0`.

use crate::procs::{self, TreeKiller};
use crate::secrets::EngineSecrets;
use crate::{AppPaths, HostError, Result};
use cl_pg::Secret;
use serde_json::Value;
use std::collections::VecDeque;
use std::ffi::OsString;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Engine exit code for "configuration problem; restarting will not help"
/// (`EX_CONFIG`, docs/DESKTOP.md).
pub(crate) const EXIT_CONFIG: i32 = 78;

/// Engine log rotation: size of one file and number of rotated files kept.
/// The host deletes closed rotated files by age and total size (Q-118).
pub(crate) const ENGINE_LOG_MAX_BYTES: u64 = 10 * 1024 * 1024;
pub(crate) const ENGINE_LOG_KEEP: u32 = 4;

/// Lines of engine output kept for fault reports.
const OUTPUT_LINES: usize = 40;
/// Longest kept line.
const LINE_CHARS: usize = 400;

/// Everything needed to start one engine process.
pub(crate) struct LaunchPlan<'a> {
    pub exe: &'a Path,
    pub paths: &'a AppPaths,
    pub token: &'a Secret,
    pub db_url: &'a Secret,
    pub secrets: &'a EngineSecrets,
    /// Extra variables for tests (a fake engine's behaviour switches).
    pub extra_env: &'a [(String, String)],
}

fn require_absolute(what: &'static str, path: &Path) -> Result<()> {
    if path.is_absolute() {
        Ok(())
    } else {
        Err(HostError::NotAbsolute {
            what,
            path: path.to_path_buf(),
        })
    }
}

/// The complete environment of the engine process.
///
/// # Errors
/// Fails when a path that must be absolute is not (Q-P04b).
pub(crate) fn build_env(plan: &LaunchPlan<'_>) -> Result<Vec<(String, OsString)>> {
    let p = plan.paths;
    let log = p.engine_log();
    let data = p.engine_data();
    let runtime = p.engine_runtime();
    require_absolute("CL_LOG_FILE", &log)?;
    require_absolute("CL_DATA_DIR", &data)?;
    require_absolute("CL_ENGINE_RUNTIME_DIR", &runtime)?;
    require_absolute("the engine program", plan.exe)?;

    let text = |s: &str| OsString::from(s);
    let path = |p: PathBuf| p.into_os_string();
    let mut env: Vec<(String, OsString)> = vec![
        ("CL_MODE".into(), text("desktop")),
        ("CL_BIND".into(), text("127.0.0.1:0")),
        ("CL_TOKEN".into(), text(plan.token.expose())),
        ("CL_DATA_DIR".into(), path(data)),
        ("CL_LOG_FILE".into(), path(log)),
        (
            "CL_LOG_MAX_BYTES".into(),
            text(&ENGINE_LOG_MAX_BYTES.to_string()),
        ),
        ("CL_LOG_KEEP".into(), text(&ENGINE_LOG_KEEP.to_string())),
        ("CL_OFFLINE".into(), text("auto")),
        ("CL_REQUIRE_NON_SUPERUSER".into(), text("1")),
        ("CL_ENGINE_RUNTIME_DIR".into(), path(runtime)),
        ("DATABASE_URL".into(), text(plan.db_url.expose())),
        ("JWT_SECRET".into(), text(plan.secrets.jwt_secret.expose())),
        (
            "ENCRYPTION_KEY".into(),
            text(plan.secrets.encryption_key.expose()),
        ),
        (
            "PLAID_ENCRYPTION_KEY".into(),
            text(plan.secrets.plaid_encryption_key.expose()),
        ),
        ("NODE_ENV".into(), text("production")),
        ("TEMP".into(), path(p.engine_tmp())),
        ("TMP".into(), path(p.engine_tmp())),
        ("TMPDIR".into(), path(p.engine_tmp())),
        ("HOME".into(), path(p.engine_home())),
        ("USERPROFILE".into(), path(p.engine_home())),
    ];
    if cfg!(windows) {
        let root = std::env::var_os("SystemRoot").unwrap_or_else(|| OsString::from(r"C:\Windows"));
        let root_path = PathBuf::from(&root);
        let minimal = std::env::join_paths([root_path.join("System32"), root_path])
            .unwrap_or_else(|_| OsString::from(r"C:\Windows\System32"));
        env.push(("PATH".into(), minimal));
        env.push(("SystemRoot".into(), root.clone()));
        env.push(("windir".into(), root));
    } else {
        env.push(("PATH".into(), text("/usr/bin:/bin")));
    }
    for (k, _) in plan.extra_env {
        // Test-only switches for the fake engine. They can never override the
        // contract, the PATH, or Node's own options (NODE_OPTIONS and friends).
        let charset_ok = k
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_');
        if !(k.starts_with("FAKE_") && charset_ok) {
            return Err(HostError::BadEngineEnv(k.clone()));
        }
    }
    env.extend(
        plan.extra_env
            .iter()
            .map(|(k, v)| (k.clone(), OsString::from(v))),
    );
    Ok(env)
}

/// Removes secrets from text before it is kept or shown.
#[derive(Debug, Clone)]
pub(crate) struct Scrubber(Vec<Secret>);

impl Scrubber {
    pub(crate) fn new(secrets: impl IntoIterator<Item = Secret>) -> Scrubber {
        Scrubber(secrets.into_iter().collect())
    }

    pub(crate) fn scrub(&self, text: &str) -> String {
        self.0.iter().fold(text.to_string(), |t, s| s.scrub(&t))
    }
}

#[derive(Debug, Default)]
struct Output {
    lines: VecDeque<String>,
    fatal_code: Option<String>,
}

/// A running engine.
#[derive(Debug)]
pub(crate) struct EngineProcess {
    pub child: Child,
    pub killer: TreeKiller,
    pub pid: u32,
    pub port: u16,
    output: Arc<Mutex<Output>>,
    terminated: bool,
}

impl Drop for EngineProcess {
    /// A panic or an early return must never leave the engine running.
    fn drop(&mut self) {
        if !self.terminated {
            self.terminate(Duration::from_secs(2));
        }
    }
}

impl EngineProcess {
    /// Last lines of engine output (scrubbed when kept).
    pub(crate) fn output_tail(&self) -> Vec<String> {
        self.output
            .lock()
            .map(|o| o.lines.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// The `code` of a `{"event":"fatal"}` line, when the engine printed one.
    pub(crate) fn fatal_code(&self) -> Option<String> {
        self.output.lock().ok().and_then(|o| o.fatal_code.clone())
    }

    /// Stop the process tree and wait for it.
    pub(crate) fn terminate(&mut self, grace: Duration) {
        self.terminated = true;
        self.killer.terminate(&mut self.child, grace);
    }
}

/// Why the engine did not become ready.
#[derive(Debug)]
pub(crate) enum LaunchError {
    /// The program could not be started at all.
    Spawn(std::io::Error),
    /// It ended before printing its ready line.
    Exited {
        code: Option<i32>,
        fatal_code: Option<String>,
        tail: Vec<String>,
    },
    /// No ready line within the time limit.
    NotReady { tail: Vec<String> },
    /// The ready line named an address other than loopback.
    NotLoopback,
    /// The caller asked to stop while we were waiting.
    Cancelled,
}

impl LaunchError {
    /// Restarting cannot help (the engine said its configuration is wrong, or
    /// the program is missing).
    pub(crate) fn is_permanent(&self) -> bool {
        match self {
            LaunchError::Spawn(e) => e.kind() == std::io::ErrorKind::NotFound,
            LaunchError::Exited { code, .. } => *code == Some(EXIT_CONFIG),
            LaunchError::NotLoopback => true,
            LaunchError::NotReady { .. } | LaunchError::Cancelled => false,
        }
    }

    /// A one-line description without secrets.
    pub(crate) fn describe(&self) -> String {
        match self {
            LaunchError::Spawn(e) => {
                format!("the engine program could not be started ({})", e.kind())
            }
            LaunchError::Exited {
                code, fatal_code, ..
            } => {
                let how = code.map_or_else(
                    || "was stopped".to_string(),
                    |c| format!("exited with code {c}"),
                );
                match fatal_code {
                    Some(f) => format!("the engine {how} ({f})"),
                    None => format!("the engine {how} before it was ready"),
                }
            }
            LaunchError::NotReady { .. } => "the engine did not become ready in time".into(),
            LaunchError::NotLoopback => "the engine reported a non-loopback address".into(),
            LaunchError::Cancelled => "start cancelled".into(),
        }
    }

    /// Output lines to attach to a fault report.
    pub(crate) fn tail(&self) -> Vec<String> {
        match self {
            LaunchError::Exited { tail, .. } | LaunchError::NotReady { tail } => tail.clone(),
            _ => Vec::new(),
        }
    }
}

/// Longest line kept from the engine; the rest of a longer line is discarded.
const MAX_LINE_BYTES: usize = 64 * 1024;

/// Call `f` for every line of `reader`, however odd the bytes: invalid UTF-8
/// is replaced, a line longer than [`MAX_LINE_BYTES`] is cut (the rest is
/// skipped), and reading continues to the end so the child never blocks on a
/// full pipe.
pub(crate) fn for_each_line(reader: impl std::io::Read, mut f: impl FnMut(&str)) {
    let mut r = BufReader::new(reader);
    let mut line: Vec<u8> = Vec::new();
    let emit = |line: &mut Vec<u8>, f: &mut dyn FnMut(&str)| {
        let text = String::from_utf8_lossy(line);
        f(text.trim_end_matches('\r'));
        line.clear();
    };
    let keep = |line: &mut Vec<u8>, chunk: &[u8]| {
        let room = MAX_LINE_BYTES.saturating_sub(line.len());
        line.extend_from_slice(&chunk[..chunk.len().min(room)]);
    };
    loop {
        let buf = match r.fill_buf() {
            Ok([]) => break,
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        if let Some(i) = buf.iter().position(|&b| b == b'\n') {
            keep(&mut line, &buf[..i]);
            r.consume(i + 1);
            emit(&mut line, &mut f);
        } else {
            let n = buf.len();
            keep(&mut line, buf);
            r.consume(n);
        }
    }
    if !line.is_empty() {
        emit(&mut line, &mut f);
    }
}

/// Read a ready line: `{"event":"listening","host":"127.0.0.1","port":54321}`.
pub(crate) fn parse_ready_line(line: &str) -> Option<(String, u16)> {
    let v: Value = serde_json::from_str(line.trim()).ok()?;
    if v.get("event")?.as_str()? != "listening" {
        return None;
    }
    let port = u16::try_from(v.get("port")?.as_u64()?).ok()?;
    let host = v.get("host").and_then(Value::as_str).unwrap_or("127.0.0.1");
    Some((host.to_string(), port))
}

fn push_line(out: &Mutex<Output>, scrubber: &Scrubber, line: &str) {
    let clean: String = scrubber.scrub(line).chars().take(LINE_CHARS).collect();
    if let Ok(mut o) = out.lock() {
        if let Ok(v) = serde_json::from_str::<Value>(&clean)
            && v.get("event").and_then(Value::as_str) == Some("fatal")
        {
            o.fatal_code = v
                .get("code")
                .and_then(Value::as_str)
                .map(|c| c.chars().take(60).collect());
        }
        if o.lines.len() >= OUTPUT_LINES {
            o.lines.pop_front();
        }
        o.lines.push_back(clean);
    }
}

/// Start the engine and wait for its ready line.
///
/// `cancelled` is polled while waiting so a shutdown request is prompt.
pub(crate) fn launch(
    plan: &LaunchPlan<'_>,
    scrubber: &Scrubber,
    ready_timeout: Duration,
    cancelled: &dyn Fn() -> bool,
) -> std::result::Result<EngineProcess, LaunchError> {
    let env = build_env(plan).map_err(|e| {
        LaunchError::Spawn(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            e.to_string(),
        ))
    })?;
    let mut cmd = Command::new(plan.exe);
    // No arguments at all: nothing secret can ever appear in a process list.
    cmd.env_clear()
        .envs(env)
        .current_dir(plan.paths.engine_data())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    procs::prepare(&mut cmd);
    let mut child = cmd.spawn().map_err(LaunchError::Spawn)?;
    let pid = child.id();
    let killer = TreeKiller::adopt(&child);
    procs::record_child(plan.paths, plan.exe, pid);

    let output = Arc::new(Mutex::new(Output::default()));
    let (ready_tx, ready_rx) = mpsc::channel::<(String, u16)>();
    if let Some(stdout) = child.stdout.take() {
        let (out, scrub) = (Arc::clone(&output), scrubber.clone());
        std::thread::spawn(move || {
            for_each_line(stdout, |line| {
                if let Some(ready) = parse_ready_line(line) {
                    let _ = ready_tx.send(ready);
                }
                push_line(&out, &scrub, line);
            });
        });
    }
    if let Some(stderr) = child.stderr.take() {
        let (out, scrub) = (Arc::clone(&output), scrubber.clone());
        std::thread::spawn(move || for_each_line(stderr, |line| push_line(&out, &scrub, line)));
    }
    let mut proc = EngineProcess {
        child,
        killer,
        pid,
        port: 0,
        output,
        terminated: false,
    };

    let deadline = Instant::now() + ready_timeout;
    loop {
        if cancelled() {
            proc.terminate(Duration::from_secs(1));
            return Err(LaunchError::Cancelled);
        }
        match ready_rx.recv_timeout(Duration::from_millis(100)) {
            Ok((host, port)) => {
                if host != "127.0.0.1" && host != "::1" && host != "localhost" {
                    proc.terminate(Duration::from_secs(1));
                    return Err(LaunchError::NotLoopback);
                }
                proc.port = port;
                return Ok(proc);
            }
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => {}
        }
        if let Ok(Some(status)) = proc.child.try_wait() {
            // Let the reader threads drain what the engine printed last.
            std::thread::sleep(Duration::from_millis(100));
            let err = LaunchError::Exited {
                code: status.code(),
                fatal_code: proc.fatal_code(),
                tail: proc.output_tail(),
            };
            proc.terminate(Duration::from_millis(100));
            procs::forget_child(plan.paths);
            return Err(err);
        }
        if Instant::now() >= deadline {
            let tail = proc.output_tail();
            proc.terminate(Duration::from_secs(2));
            procs::forget_child(plan.paths);
            return Err(LaunchError::NotReady { tail });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan_parts() -> (AppPaths, Secret, Secret, EngineSecrets) {
        let root = std::env::temp_dir().join("cl-host-engine-env-test");
        (
            AppPaths::at(root).unwrap(),
            Secret::new("TOKEN-0123456789abcdef"),
            Secret::new("postgres://app:DBPW@127.0.0.1:5555/clientledger"),
            EngineSecrets::from_master(&Secret::new("m")),
        )
    }

    fn env_map(extra: &[(String, String)]) -> std::collections::BTreeMap<String, OsString> {
        let (paths, token, db, secrets) = plan_parts();
        let exe = std::env::temp_dir().join("engine.exe");
        let plan = LaunchPlan {
            exe: &exe,
            paths: &paths,
            token: &token,
            db_url: &db,
            secrets: &secrets,
            extra_env: extra,
        };
        build_env(&plan).unwrap().into_iter().collect()
    }

    #[test]
    fn the_environment_has_the_desktop_contract() {
        let env = env_map(&[]);
        let get = |k: &str| env.get(k).unwrap().to_string_lossy().into_owned();
        assert_eq!(get("CL_MODE"), "desktop");
        assert_eq!(get("CL_BIND"), "127.0.0.1:0");
        assert_eq!(get("CL_REQUIRE_NON_SUPERUSER"), "1");
        assert_eq!(get("CL_TOKEN"), "TOKEN-0123456789abcdef");
        assert!(get("DATABASE_URL").contains("@127.0.0.1:5555/"));
        assert_eq!(get("NODE_ENV"), "production");
        assert!(Path::new(&get("CL_DATA_DIR")).is_absolute());
        assert!(Path::new(&get("CL_LOG_FILE")).is_absolute());
        assert!(Path::new(&get("CL_ENGINE_RUNTIME_DIR")).is_absolute());
    }

    #[test]
    fn nothing_is_inherited_from_this_process() {
        // PATH exists in this test process; it must be replaced, not passed on.
        let env = env_map(&[]);
        let inherited = std::env::var_os("PATH").unwrap();
        assert_ne!(env.get("PATH").unwrap(), &inherited);
        let allowed: std::collections::BTreeSet<&str> = [
            "CL_MODE",
            "CL_BIND",
            "CL_TOKEN",
            "CL_DATA_DIR",
            "CL_LOG_FILE",
            "CL_LOG_MAX_BYTES",
            "CL_LOG_KEEP",
            "CL_OFFLINE",
            "CL_REQUIRE_NON_SUPERUSER",
            "CL_ENGINE_RUNTIME_DIR",
            "DATABASE_URL",
            "JWT_SECRET",
            "ENCRYPTION_KEY",
            "PLAID_ENCRYPTION_KEY",
            "NODE_ENV",
            "TEMP",
            "TMP",
            "TMPDIR",
            "HOME",
            "USERPROFILE",
            "PATH",
            "SystemRoot",
            "windir",
        ]
        .into_iter()
        .collect();
        for key in env.keys() {
            assert!(allowed.contains(key.as_str()), "unexpected variable {key}");
        }
    }

    #[test]
    fn the_token_is_in_the_environment_only_never_in_an_argument() {
        // The command is built with no arguments at all (see `launch`); this
        // pins the contract for the secret-bearing variables.
        let env = env_map(&[]);
        for secret_var in ["CL_TOKEN", "DATABASE_URL", "JWT_SECRET", "ENCRYPTION_KEY"] {
            assert!(env.contains_key(secret_var));
        }
    }

    #[test]
    fn extra_variables_may_only_be_fake_switches() {
        let (paths, token, db, secrets) = plan_parts();
        let exe = std::env::temp_dir().join("engine.exe");
        for bad in [
            "NODE_OPTIONS",
            "NODE_DEBUG",
            "CL_TOKEN",
            "PATH",
            "DATABASE_URL",
            "fake_lower",
            "FAKE_x",
            "LD_PRELOAD",
            "",
        ] {
            let extra = [(bad.to_string(), "1".to_string())];
            let plan = LaunchPlan {
                exe: &exe,
                paths: &paths,
                token: &token,
                db_url: &db,
                secrets: &secrets,
                extra_env: &extra,
            };
            assert!(
                matches!(build_env(&plan), Err(HostError::BadEngineEnv(_))),
                "{bad:?} must be refused"
            );
        }
        let ok = [("FAKE_MODE".to_string(), "1".to_string())];
        let plan = LaunchPlan {
            exe: &exe,
            paths: &paths,
            token: &token,
            db_url: &db,
            secrets: &secrets,
            extra_env: &ok,
        };
        assert!(build_env(&plan).is_ok());
    }

    #[test]
    fn relative_directories_are_refused() {
        let (paths, token, db, secrets) = plan_parts();
        let plan = LaunchPlan {
            exe: Path::new("engine.exe"),
            paths: &paths,
            token: &token,
            db_url: &db,
            secrets: &secrets,
            extra_env: &[],
        };
        assert!(matches!(
            build_env(&plan),
            Err(HostError::NotAbsolute { .. })
        ));
    }

    fn lines_of(bytes: &[u8]) -> Vec<String> {
        let mut out = Vec::new();
        for_each_line(bytes, |l| out.push(l.to_string()));
        out
    }

    #[test]
    fn lines_survive_invalid_utf8_crlf_and_a_missing_final_newline() {
        let got = lines_of(b"one\r\ntw\xffo\nlast");
        let bad = format!("tw{}o", char::REPLACEMENT_CHARACTER);
        assert_eq!(got, ["one", bad.as_str(), "last"]);
        assert_eq!(lines_of(b""), Vec::<String>::new());
    }

    #[test]
    fn a_huge_line_is_cut_and_reading_continues_to_the_ready_line() {
        let mut data = vec![b'x'; 5 * 1024 * 1024];
        data.extend_from_slice(
            b"\n{\"event\":\"listening\",\"host\":\"127.0.0.1\",\"port\":4242}\n",
        );
        let got = lines_of(&data);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].len(), MAX_LINE_BYTES);
        assert_eq!(parse_ready_line(&got[1]), Some(("127.0.0.1".into(), 4242)));
    }

    #[test]
    fn ready_lines_are_read() {
        assert_eq!(
            parse_ready_line(r#"{"event":"listening","host":"127.0.0.1","port":54321}"#),
            Some(("127.0.0.1".into(), 54321))
        );
        assert_eq!(parse_ready_line(r#"{"event":"fatal","port":1}"#), None);
        assert_eq!(parse_ready_line("plain text"), None);
        assert_eq!(
            parse_ready_line(r#"{"event":"listening","port":70000}"#),
            None
        );
    }

    #[test]
    fn scrubbing_removes_every_secret() {
        let s = Scrubber::new([Secret::new("tok123"), Secret::new("pw456")]);
        assert_eq!(s.scrub("a tok123 b pw456 c"), "a *** b *** c");
    }

    #[test]
    fn fatal_lines_are_recorded() {
        let out = Mutex::new(Output::default());
        let s = Scrubber::new([]);
        push_line(
            &out,
            &s,
            r#"{"event":"fatal","code":"DB_SUPERUSER_FORBIDDEN","message":"x"}"#,
        );
        assert_eq!(
            out.lock().unwrap().fatal_code.as_deref(),
            Some("DB_SUPERUSER_FORBIDDEN")
        );
    }

    #[test]
    fn output_is_bounded() {
        let out = Mutex::new(Output::default());
        let s = Scrubber::new([]);
        for i in 0..200 {
            push_line(&out, &s, &format!("line {i} {}", "x".repeat(1000)));
        }
        let o = out.lock().unwrap();
        assert_eq!(o.lines.len(), OUTPUT_LINES);
        assert!(o.lines.iter().all(|l| l.chars().count() <= LINE_CHARS));
    }

    #[test]
    fn permanent_failures_are_classified() {
        assert!(
            LaunchError::Exited {
                code: Some(78),
                fatal_code: None,
                tail: vec![]
            }
            .is_permanent()
        );
        assert!(
            !LaunchError::Exited {
                code: Some(1),
                fatal_code: None,
                tail: vec![]
            }
            .is_permanent()
        );
        assert!(!LaunchError::NotReady { tail: vec![] }.is_permanent());
        assert!(
            LaunchError::Spawn(std::io::Error::from(std::io::ErrorKind::NotFound)).is_permanent()
        );
    }
}
