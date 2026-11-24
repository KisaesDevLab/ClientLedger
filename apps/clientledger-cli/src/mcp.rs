//! `clientledger-cli mcp --connection <id>`: the stdio relay an MCP client
//! such as Claude Desktop runs (A11 D4, Q-325, Q-A11b).
//!
//! It speaks MCP on stdin and stdout and finds the running app for every
//! request: the endpoint file in the app's run folder (owner-only, checked
//! by `cl_automation::control_file`) gives the loopback port and the run's
//! key, and this computer's keychain holds the connection's token. Nothing
//! secret and no port is in the MCP client's config. While the app is
//! closed the relay says so in plain words (`cl_mcp::relay`).
//!
//! Only MCP lines go to stdout: this command prints nothing else there.

use std::io::BufReader;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, Mutex, PoisonError};

use cl_automation::control_file::{self, ControlFileError};
use cl_host::AppPaths;
use cl_mcp::relay::{Upstream, UpstreamError};
use cl_mcp::transport::{self, Endpoint, Locate, Loopback};
use cl_settings::{KeyringSecrets, SecretStore};

use crate::host::EXIT_USAGE;

const HELP: &str = "\
Usage: clientledger-cli mcp --connection <id> [--data-dir <path>]

Speaks MCP on stdin and stdout for an AI app such as Claude Desktop, and
passes each request to ClientLedger while it is open on this computer.
ClientLedger's Settings > Claude Desktop writes this command into Claude
Desktop's settings for you; other AI apps can run it the same way.

Options:
  --connection <id>   The AI connection made in ClientLedger (Preferences >
                      AI assistants (MCP)); its key is in this computer's
                      keychain
  --data-dir <path>   App data folder (default: your user data folder)";

/// What a refused token says.
pub(crate) const NO_TOKEN: &str = "This computer has no key for that AI connection. In \
     ClientLedger, open Settings \u{203a} Claude Desktop and choose Connect again.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Parsed {
    Run {
        connection: String,
        data_dir: Option<PathBuf>,
    },
    Help,
}

/// Parse the arguments after `mcp`.
pub(crate) fn parse(args: &[String]) -> Result<Parsed, String> {
    let mut connection = None;
    let mut data_dir = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--help" | "-h" | "help" => return Ok(Parsed::Help),
            "--connection" => {
                let id = it.next().ok_or("--connection needs an id")?;
                if !transport::valid_connection_id(id) {
                    return Err(format!("{id:?} is not a connection id"));
                }
                connection = Some(id.clone());
            }
            "--data-dir" => {
                let d = PathBuf::from(it.next().ok_or("--data-dir needs a path")?);
                if !d.is_absolute() {
                    return Err("--data-dir must be an absolute path".into());
                }
                data_dir = Some(d);
            }
            other => return Err(format!("unknown option {other:?}")),
        }
    }
    Ok(Parsed::Run {
        connection: connection.ok_or("--connection <id> is required")?,
        data_dir,
    })
}

/// The app on this computer: its endpoint file and the keychain.
struct LocalApp {
    endpoint_file: PathBuf,
    connection: String,
    secrets: Box<dyn SecretStore>,
    token: Mutex<Option<String>>,
}

impl Locate for LocalApp {
    fn endpoint(&self) -> Result<Endpoint, UpstreamError> {
        match control_file::read_checked(&self.endpoint_file) {
            Ok(info) => Ok(Endpoint {
                addr: SocketAddr::from((Ipv4Addr::LOCALHOST, info.port)),
                key: info.token,
            }),
            Err(ControlFileError::Unsafe(why)) => Err(UpstreamError::Refused(format!(
                "ClientLedger's connection file is not safe to use ({why}), so nothing was sent."
            ))),
            // Missing (closed), or being written (opening): closed for now.
            Err(_) => Err(UpstreamError::Closed),
        }
    }

    fn connection(&self) -> &str {
        &self.connection
    }

    fn token(&self) -> Result<String, UpstreamError> {
        let mut cached = self.token.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(t) = cached.as_ref() {
            return Ok(t.clone());
        }
        match self.secrets.get(&transport::token_name(&self.connection)) {
            Ok(Some(s)) => {
                let t = s.expose().to_string();
                *cached = Some(t.clone());
                Ok(t)
            }
            Ok(None) => Err(UpstreamError::Refused(NO_TOKEN.into())),
            Err(_) => Err(UpstreamError::Refused(
                "This computer's keychain could not be read, so nothing was sent.".into(),
            )),
        }
    }
}

/// A token re-read after a refusal (Connect again or Confirm again mints
/// a new one while the relay runs).
struct Fresh(Loopback<AppRef>, Arc<LocalApp>);

/// The shared app, as the loopback upstream's locator.
struct AppRef(Arc<LocalApp>);

impl Locate for AppRef {
    fn endpoint(&self) -> Result<Endpoint, UpstreamError> {
        self.0.endpoint()
    }
    fn connection(&self) -> &str {
        self.0.connection()
    }
    fn token(&self) -> Result<String, UpstreamError> {
        self.0.token()
    }
}

impl Fresh {
    fn retry<T>(&self, f: impl Fn() -> Result<T, UpstreamError>) -> Result<T, UpstreamError> {
        match f() {
            Err(UpstreamError::Refused(_)) => {
                *self.1.token.lock().unwrap_or_else(PoisonError::into_inner) = None;
                f()
            }
            other => other,
        }
    }
}

impl Upstream for Fresh {
    fn probe(&self) -> Result<cl_mcp::relay::Seen, UpstreamError> {
        self.retry(|| self.0.probe())
    }
    fn send(&self, msg: &serde_json::Value) -> Result<Option<serde_json::Value>, UpstreamError> {
        // A refused message was never acted on, so it may be sent again.
        self.retry(|| self.0.send(msg))
    }
}

/// `clientledger-cli mcp …`.
pub(crate) fn run(args: &[String]) -> ExitCode {
    let (connection, data_dir) = match parse(args) {
        Ok(Parsed::Help) => {
            eprintln!("{HELP}");
            return ExitCode::SUCCESS;
        }
        Ok(Parsed::Run {
            connection,
            data_dir,
        }) => (connection, data_dir),
        Err(e) => {
            eprintln!("{e}\n\n{HELP}");
            return ExitCode::from(EXIT_USAGE);
        }
    };
    let paths = match data_dir.map_or_else(AppPaths::for_current_user, AppPaths::at) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("The app data folder is not known: {e}");
            return ExitCode::FAILURE;
        }
    };
    let app = Arc::new(LocalApp {
        endpoint_file: paths.run_dir().join(transport::ENDPOINT_FILE),
        connection,
        secrets: Box::new(KeyringSecrets::for_root(paths.root())),
        token: Mutex::new(None),
    });
    let up: Arc<dyn Upstream> = Arc::new(Fresh(Loopback::new(AppRef(Arc::clone(&app))), app));
    let stdin = std::io::stdin();
    cl_mcp::relay::relay(
        &up,
        BufReader::new(stdin.lock()),
        Box::new(std::io::stdout()),
    );
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;
    use cl_core::Secret;
    use cl_settings::MemorySecrets;

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn parses_the_connection_and_data_folder() {
        let data = if cfg!(windows) { r"C:\data" } else { "/data" };
        assert_eq!(
            parse(&args(&["--connection", "c-1", "--data-dir", data])),
            Ok(Parsed::Run {
                connection: "c-1".into(),
                data_dir: Some(PathBuf::from(data)),
            })
        );
        assert_eq!(parse(&args(&["--help"])), Ok(Parsed::Help));
        assert!(parse(&args(&[])).is_err(), "the connection is required");
        assert!(parse(&args(&["--connection"])).is_err());
        assert!(parse(&args(&["--connection", "../x"])).is_err());
        assert!(parse(&args(&["--connection", "c", "--data-dir", "rel"])).is_err());
        assert!(parse(&args(&["--connection", "c", "--read-only"])).is_err());
    }

    fn app(dir: &std::path::Path, secrets: MemorySecrets) -> LocalApp {
        LocalApp {
            endpoint_file: dir.join(transport::ENDPOINT_FILE),
            connection: "c-1".into(),
            secrets: Box::new(secrets),
            token: Mutex::new(None),
        }
    }

    #[test]
    fn no_endpoint_file_is_closed_and_no_token_says_connect_again() {
        let dir = tempfile::tempdir().expect("tempdir");
        let a = app(dir.path(), MemorySecrets::new());
        assert!(matches!(
            a.endpoint(),
            Err(UpstreamError::Closed | UpstreamError::Refused(_))
        ));
        assert_eq!(a.token(), Err(UpstreamError::Refused(NO_TOKEN.into())));
        let s = MemorySecrets::new();
        s.set("mcp-c-1", &Secret::new("tok")).expect("set");
        let a = app(dir.path(), s);
        assert_eq!(a.token(), Ok("tok".into()));
    }
}
