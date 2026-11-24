//! Claude Desktop configuration: connect, remove and status (plan D4).
//!
//! Settings › Claude Desktop (D5) adds the app to Claude Desktop's MCP
//! servers, or takes it out (owner 2026-11-24, Q-A11b; ported from
//! the Trial Balance app's `claude_desktop.rs`, review S14).
//!
//! Claude Desktop's config file (documented at
//! modelcontextprotocol.io/quickstart/user) is `claude_desktop_config.json`
//! in `%APPDATA%\Claude\` on Windows (current versions:
//! `%LOCALAPPDATA%\Claude-Data\claude_<id>\`, the newest when there are
//! several), `~/Library/Application Support/Claude/` on macOS and
//! `~/.config/Claude/` on Linux. The entry runs `clientledger-cli mcp
//! --connection <id>`; it holds no secret and no port (the connection's
//! token is in this computer's keychain, Q-A11b). The file is merged, never
//! replaced: other servers and settings stay as they are; a file that is
//! not JSON is refused, not overwritten; the file as it was before
//! the app first changed it is kept beside it; the new one is written
//! to a temporary file and renamed. No egui here.

use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};

/// The entry's name in `mcpServers`.
pub const SERVER: &str = "clientledger";
/// The file name.
pub const FILE: &str = "claude_desktop_config.json";
/// The earlier file, kept beside it the first time the app changes it.
pub const KEPT: &str = "claude_desktop_config.json.before-clientledger";

/// Why the config could not be changed.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The file is not a config Claude Desktop could read; it was left alone.
    #[error(
        "Claude Desktop's settings file was left as it is: {0}. Fix it in Claude Desktop \
         (Settings \u{203a} Developer \u{203a} Edit Config), then try again."
    )]
    Unreadable(String),
    /// The file or its folder could not be read or written.
    #[error("{what} ({source}).")]
    Io {
        /// What could not be done, in plain words.
        what: String,
        /// The system's reason.
        source: std::io::Error,
    },
    /// No home or app data folder is known on this computer.
    #[error("Claude Desktop's settings folder is not known on this computer.")]
    NoFolder,
}

/// Where Claude Desktop keeps its config on this computer (`None`: no
/// home or app data folder known).
#[must_use]
pub fn config_path() -> Option<PathBuf> {
    if cfg!(windows) {
        let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
        let roaming = std::env::var_os("APPDATA").map(PathBuf::from);
        return windows_config_path(local.as_deref(), roaming.as_deref());
    }
    let dir = if cfg!(target_os = "macos") {
        PathBuf::from(std::env::var_os("HOME")?)
            .join("Library")
            .join("Application Support")
            .join("Claude")
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?
            .join("Claude")
    };
    Some(dir.join(FILE))
}

/// Claude Desktop's config on Windows (TB Q84, found on a Microsoft Store
/// install): current versions keep their settings in
/// `%LOCALAPPDATA%\Claude-Data\claude_<id>\` (its README says
/// `%APPDATA%\Claude` holds only older files), so a config written to
/// `%APPDATA%\Claude` is never read. That folder when it is there (the most
/// recently changed one, if several), else the documented `%APPDATA%\Claude`.
#[must_use]
pub fn windows_config_path(local: Option<&Path>, roaming: Option<&Path>) -> Option<PathBuf> {
    let current = local
        .and_then(|l| std::fs::read_dir(l.join("Claude-Data")).ok())
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|e| {
            e.file_name().to_string_lossy().starts_with("claude_")
                && e.file_type().is_ok_and(|t| t.is_dir())
        })
        .max_by_key(|e| e.metadata().and_then(|m| m.modified()).ok())
        .map(|e| e.path());
    match current {
        Some(dir) => Some(dir.join(FILE)),
        None => Some(roaming?.join("Claude").join(FILE)),
    }
}

/// The entry for a connection: `<program> mcp --connection <id>`, with the
/// data folder only when this copy runs with one other than the default
/// (the keychain entry is scoped to it).
#[must_use]
pub fn entry(program: &Path, connection: &str, data_dir: Option<&Path>) -> Value {
    let mut args = vec![
        Value::from("mcp"),
        Value::from("--connection"),
        Value::from(connection),
    ];
    if let Some(d) = data_dir {
        args.push("--data-dir".into());
        args.push(d.to_string_lossy().into_owned().into());
    }
    json!({"command": program.to_string_lossy(), "args": args})
}

/// The connection id an entry runs with (`--connection <id>`).
#[must_use]
pub fn connection_of(entry: &Value) -> Option<&str> {
    let args = entry.get("args")?.as_array()?;
    let at = args.iter().position(|a| a == "--connection")?;
    args.get(at + 1)?.as_str()
}

/// The command line an entry runs, for "Other AI apps" (any MCP client
/// that starts a program on stdio).
#[must_use]
pub fn command_line(entry: &Value) -> String {
    let quote = |s: &str| {
        if s.contains(char::is_whitespace) {
            format!("\"{s}\"")
        } else {
            s.to_string()
        }
    };
    let mut out = quote(entry["command"].as_str().unwrap_or(""));
    for a in entry["args"].as_array().into_iter().flatten() {
        out.push(' ');
        out.push_str(&quote(a.as_str().unwrap_or("")));
    }
    out
}

/// Where things stand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// No config file yet (Claude Desktop not installed, or never set up).
    NoFile,
    /// The file has no entry for the app.
    NotConnected,
    /// Connected, as this copy would write it.
    Connected,
    /// An entry that runs another program or folder (an older install,
    /// another data folder): Connect updates it.
    Different,
    /// The entry's connection was revoked or deleted (review S14): every
    /// call would be refused, so "Connect again".
    Stale,
    /// The file is not JSON Claude Desktop could read; nothing is changed.
    Unreadable(String),
}

impl Status {
    /// the app is in Claude Desktop's servers (this copy or another).
    #[must_use]
    pub fn connected(&self) -> bool {
        matches!(self, Status::Connected | Status::Different | Status::Stale)
    }
}

/// The status of `text` (the file's contents, `None` when there is none)
/// for `want`; `live` says whether a connection id is still in use.
#[must_use]
pub fn status(text: Option<&str>, want: &Value, live: &dyn Fn(&str) -> bool) -> Status {
    let Some(text) = text else {
        return Status::NoFile;
    };
    match parse(text) {
        Err(why) => Status::Unreadable(why),
        Ok(doc) => match doc.get("mcpServers").and_then(|s| s.get(SERVER)) {
            None => Status::NotConnected,
            Some(e) if connection_of(e).is_none_or(|c| !live(c)) => Status::Stale,
            Some(e)
                if e.get("command") == want.get("command") && e.get("args") == want.get("args") =>
            {
                Status::Connected
            }
            Some(_) => Status::Different,
        },
    }
}

fn parse(text: &str) -> Result<Map<String, Value>, String> {
    if text.trim().is_empty() {
        return Ok(Map::new());
    }
    match serde_json::from_str::<Value>(text) {
        Ok(Value::Object(m)) => match m.get("mcpServers") {
            None | Some(Value::Object(_)) => Ok(m),
            Some(_) => Err("its mcpServers is not a list of servers".into()),
        },
        Ok(_) => Err("it is not a JSON object".into()),
        Err(e) => Err(format!("it is not valid JSON ({e})")),
    }
}

/// `text` with the app's entry set to `want` (added or replaced), or
/// taken out when `want` is `None`; everything else as it was.
///
/// # Errors
/// [`ConfigError::Unreadable`]: the text is not a config Claude Desktop
/// could read.
pub fn merged(text: Option<&str>, want: Option<&Value>) -> Result<String, ConfigError> {
    let mut doc = parse(text.unwrap_or("")).map_err(ConfigError::Unreadable)?;
    let servers = doc
        .entry("mcpServers")
        .or_insert_with(|| Value::Object(Map::new()));
    let Value::Object(servers) = servers else {
        return Err(ConfigError::Unreadable(
            "its mcpServers is not a list of servers".into(),
        ));
    };
    match want {
        Some(e) => {
            servers.insert(SERVER.into(), e.clone());
        }
        None => {
            servers.remove(SERVER);
        }
    }
    serde_json::to_string_pretty(&Value::Object(doc))
        .map(|s| s + "\n")
        .map_err(|e| ConfigError::Unreadable(e.to_string()))
}

fn io(what: String) -> impl FnOnce(std::io::Error) -> ConfigError {
    move |source| ConfigError::Io { what, source }
}

/// Write `bytes` to `path` through a temporary file beside it, then rename
/// it over the old one, so Claude Desktop never reads half a file.
///
/// # Errors
/// The temporary file could not be written or renamed.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .map_or_else(|| "config".into(), |n| n.to_string_lossy().into_owned());
    let tmp = dir.join(format!(".{name}.clientledger-{}.tmp", std::process::id()));
    let result = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        std::io::Write::write_all(&mut f, bytes)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Write the change to the file at `path`: the file as it was before
/// the app first changed it kept as [`KEPT`], the new one through a
/// temporary file.
///
/// # Errors
/// The file could not be read as a config, or written.
pub fn apply(path: &Path, want: Option<&Value>) -> Result<(), ConfigError> {
    let before = match std::fs::read_to_string(path) {
        Ok(t) => Some(t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(io(format!("{} could not be read", path.display()))(e)),
    };
    let text = merged(before.as_deref(), want)?;
    let dir = path.parent().ok_or(ConfigError::NoFolder)?;
    std::fs::create_dir_all(dir).map_err(io(format!("{} could not be made", dir.display())))?;
    // Kept once, never overwritten by a later change.
    if let Some(b) = &before
        && !dir.join(KEPT).exists()
    {
        std::fs::write(dir.join(KEPT), b)
            .map_err(io("A copy of the settings file could not be kept".into()))?;
    }
    write_atomic(path, text.as_bytes())
        .map_err(io(format!("{} could not be written", path.display())))
}

/// What Connect or Remove said.
pub const CONNECTED: &str =
    "Added to Claude Desktop. Quit Claude Desktop completely and open it again to connect.";
/// What Remove said.
pub const REMOVED: &str = "Taken out of Claude Desktop. Quit and reopen Claude Desktop for it to \
     stop offering ClientLedger.";

/// Settings › Claude Desktop, as a model (D5 draws it).
#[derive(Debug, Clone, Default)]
pub struct ClaudeDesktop {
    /// The config file (`None`: unknown on this computer).
    pub path: Option<PathBuf>,
    /// The entry this copy writes.
    pub want: Value,
    /// Where things stand (read when the card is shown).
    pub status: Option<Status>,
    /// The last change's result.
    pub message: Option<Result<&'static str, String>>,
}

impl ClaudeDesktop {
    /// For this program, this connection and how it runs.
    #[must_use]
    pub fn new(program: &Path, connection: &str, data_dir: Option<&Path>) -> ClaudeDesktop {
        ClaudeDesktop {
            path: config_path(),
            want: entry(program, connection, data_dir),
            status: None,
            message: None,
        }
    }

    /// Read the file again (when the card is shown).
    pub fn refresh(&mut self, live: &dyn Fn(&str) -> bool) {
        self.status = self.path.as_ref().map(|p| {
            let text = std::fs::read_to_string(p).ok();
            status(text.as_deref(), &self.want, live)
        });
    }

    /// Add or update the entry.
    pub fn connect(&mut self, live: &dyn Fn(&str) -> bool) {
        self.change(true, live);
    }

    /// Take the entry out.
    pub fn remove(&mut self, live: &dyn Fn(&str) -> bool) {
        self.change(false, live);
    }

    fn change(&mut self, on: bool, live: &dyn Fn(&str) -> bool) {
        let Some(path) = self.path.clone() else {
            self.message = Some(Err(ConfigError::NoFolder.to_string()));
            return;
        };
        let want = on.then_some(&self.want);
        self.message = Some(
            apply(&path, want)
                .map(|()| if on { CONNECTED } else { REMOVED })
                .map_err(|e| e.to_string()),
        );
        self.refresh(live);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "0b7e2a52-6f1d-4c0e-9d1e-3f1f2a9c0d11";

    fn want() -> Value {
        entry(
            Path::new(r"C:\Program Files\ClientLedger\clientledger-cli.exe"),
            ID,
            None,
        )
    }

    fn live(c: &str) -> bool {
        c == ID
    }

    #[test]
    fn windows_finds_the_current_data_folder_before_the_old_one() {
        let root = tempfile::tempdir().expect("tempdir");
        let (local, roaming) = (root.path().join("Local"), root.path().join("Roaming"));
        std::fs::create_dir_all(&roaming).expect("mkdir");
        assert_eq!(
            windows_config_path(Some(&local), Some(&roaming)),
            Some(roaming.join("Claude").join(FILE))
        );
        let current = local.join("Claude-Data").join("claude_pzs8sxrjxfjjc");
        std::fs::create_dir_all(&current).expect("mkdir");
        std::fs::write(local.join("Claude-Data").join("README.txt"), "x").expect("write");
        assert_eq!(
            windows_config_path(Some(&local), Some(&roaming)),
            Some(current.join(FILE))
        );
        assert_eq!(windows_config_path(None, None), None);
    }

    #[test]
    fn the_entry_names_the_connection_and_holds_no_secret_or_port() {
        let e = want();
        assert_eq!(e["args"], json!(["mcp", "--connection", ID]));
        assert!(e.get("env").is_none(), "no secret, no port");
        assert_eq!(connection_of(&e), Some(ID));
        let e = entry(
            Path::new("/x/clientledger-cli"),
            ID,
            Some(Path::new("/data")),
        );
        assert_eq!(
            e["args"],
            json!(["mcp", "--connection", ID, "--data-dir", "/data"])
        );
        assert_eq!(connection_of(&json!({"args": ["mcp"]})), None);
        assert_eq!(
            command_line(&want()),
            format!(
                "\"C:\\Program Files\\ClientLedger\\clientledger-cli.exe\" mcp --connection {ID}"
            )
        );
    }

    #[test]
    fn other_servers_and_settings_stay() {
        let before = r#"{"globalShortcut":"Ctrl+Space","mcpServers":{"filesystem":{"command":"npx","args":["-y","x"]}}}"#;
        let after: Value =
            serde_json::from_str(&merged(Some(before), Some(&want())).expect("merged"))
                .expect("json");
        assert_eq!(after["globalShortcut"], "Ctrl+Space");
        assert_eq!(after["mcpServers"]["filesystem"]["command"], "npx");
        assert_eq!(after["mcpServers"][SERVER], want());
        let removed: Value =
            serde_json::from_str(&merged(Some(&after.to_string()), None).expect("merged"))
                .expect("json");
        assert!(removed["mcpServers"].get(SERVER).is_none());
        assert_eq!(removed["mcpServers"]["filesystem"]["command"], "npx");
    }

    #[test]
    fn status_reads_the_file() {
        let w = want();
        assert_eq!(status(None, &w, &live), Status::NoFile);
        assert_eq!(status(Some(""), &w, &live), Status::NotConnected);
        assert_eq!(status(Some("{}"), &w, &live), Status::NotConnected);
        let on = merged(None, Some(&w)).expect("merged");
        assert_eq!(status(Some(&on), &w, &live), Status::Connected);
        let other = entry(Path::new(r"D:\old\clientledger-cli.exe"), ID, None);
        assert_eq!(status(Some(&on), &other, &live), Status::Different);
        assert!(matches!(
            status(Some("{nope"), &w, &live),
            Status::Unreadable(_)
        ));
        assert!(matches!(
            status(Some("[1]"), &w, &live),
            Status::Unreadable(_)
        ));
        assert!(matches!(
            status(Some(r#"{"mcpServers":[]}"#), &w, &live),
            Status::Unreadable(_)
        ));
        assert!(!Status::NotConnected.connected());
        assert!(Status::Different.connected());
    }

    /// Review S14: a revoked or deleted connection's entry is stale, even
    /// when it is the one this copy would write.
    #[test]
    fn a_revoked_connection_is_stale() {
        let w = want();
        let on = merged(None, Some(&w)).expect("merged");
        assert_eq!(status(Some(&on), &w, &|_| false), Status::Stale);
        let no_id = merged(None, Some(&json!({"command": "x", "args": ["mcp"]}))).expect("m");
        assert_eq!(status(Some(&no_id), &w, &live), Status::Stale);
        assert!(Status::Stale.connected());
    }

    #[test]
    fn a_file_that_is_not_json_is_left_alone_and_a_good_one_is_kept() {
        let root = tempfile::tempdir().expect("tempdir");
        let dir = root.path().join("Claude");
        let path = dir.join(FILE);
        // No folder yet: made, file written, nothing kept.
        apply(&path, Some(&want())).expect("apply");
        assert!(!dir.join(KEPT).exists());
        assert_eq!(
            status(
                std::fs::read_to_string(&path).ok().as_deref(),
                &want(),
                &live
            ),
            Status::Connected
        );
        // A change keeps the earlier file, once; later changes leave it.
        let first = std::fs::read_to_string(&path).expect("read");
        apply(&path, None).expect("apply");
        assert_eq!(
            std::fs::read_to_string(dir.join(KEPT)).expect("kept"),
            first
        );
        apply(&path, Some(&want())).expect("apply");
        assert_eq!(
            std::fs::read_to_string(dir.join(KEPT)).expect("kept"),
            first,
            "not overwritten"
        );
        // No temporary file is left behind.
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .expect("dir")
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert_eq!(leftovers.len(), 0);
        // Broken: refused, untouched.
        std::fs::write(&path, "{ broken").expect("write");
        let err = apply(&path, Some(&want())).expect_err("refused");
        assert!(err.to_string().contains("left as it is"), "{err}");
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "{ broken");
    }

    #[test]
    fn the_model_connects_and_removes() {
        let root = tempfile::tempdir().expect("tempdir");
        let mut m = ClaudeDesktop {
            path: Some(root.path().join(FILE)),
            want: want(),
            ..ClaudeDesktop::default()
        };
        m.refresh(&live);
        assert_eq!(m.status, Some(Status::NoFile));
        m.connect(&live);
        assert_eq!(m.message, Some(Ok(CONNECTED)));
        assert_eq!(m.status, Some(Status::Connected));
        m.remove(&live);
        assert_eq!(m.message, Some(Ok(REMOVED)));
        assert_eq!(m.status, Some(Status::NotConnected));
        let mut nowhere = ClaudeDesktop::default();
        nowhere.connect(&live);
        assert!(matches!(nowhere.message, Some(Err(_))));
    }
}
