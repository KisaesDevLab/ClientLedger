//! Errors for `cl-host`. Messages never contain the launch token or a
//! database password: those travel only as [`cl_pg::Secret`] values.

use std::path::PathBuf;

// The two master-secret messages are built from these fixed parts around
// the data folder, so `is_master_secret_message` can recognise the whole text.
const MISSING_HEAD: &str = "the engine's master secret for ";
const MISSING_TAIL: &str = " is not in this computer's secure storage. Your books are untouched, but they cannot be opened until it is back. If the keychain was only temporarily unavailable, unlock it and start ClientLedger again; otherwise restore the data folder together with its saved secrets (ClientLedger will not invent a new one, because that would lock you out of data it encrypted earlier)";
const MISMATCH_HEAD: &str =
    "the engine's master secret in this computer's secure storage is not the one used for ";
const MISMATCH_TAIL: &str = ". Restore the matching secret (for example by restoring the data folder with its saved secrets) rather than continuing";

/// Whether `message` is, in its entirety, a [`HostError::MasterSecretMissing`]
/// or [`HostError::MasterSecretMismatch`] message: one of the fixed openings,
/// a single-line folder path, and the matching fixed ending, with nothing
/// before or after. Those messages hold only a folder path and guidance,
/// never a secret, so the UI may show them although they mention "secret".
/// Any added text (an appended value, a second line) makes this `false`.
#[must_use]
pub fn is_master_secret_message(message: &str) -> bool {
    [(MISSING_HEAD, MISSING_TAIL), (MISMATCH_HEAD, MISMATCH_TAIL)]
        .iter()
        .any(|(head, tail)| {
            message
                .strip_prefix(head)
                .and_then(|rest| rest.strip_suffix(tail))
                .is_some_and(|path| {
                    !path.is_empty()
                        && !path.chars().any(char::is_control)
                        && !path.contains(head)
                        && !path.contains(tail)
                })
        })
}

/// Everything that can go wrong while hosting the database and engine.
#[derive(Debug, thiserror::Error)]
pub enum HostError {
    /// The operating system did not say where the user's data folder is.
    #[error("cannot find your data folder: {0}")]
    NoDataFolder(String),

    /// A path that must be absolute was relative.
    #[error("{what} must be an absolute path, got {}", .path.display())]
    NotAbsolute {
        /// Which setting.
        what: &'static str,
        /// The offending value.
        path: PathBuf,
    },

    /// A file or folder operation failed.
    #[error("{context}: {source}")]
    Io {
        /// What we were doing.
        context: String,
        /// The underlying error.
        #[source]
        source: std::io::Error,
    },

    /// The cluster exists but its master secret is not in secure storage.
    #[error("{MISSING_HEAD}{}{MISSING_TAIL}", .data_dir.display())]
    MasterSecretMissing {
        /// The cluster's data directory.
        data_dir: PathBuf,
    },

    /// Secure storage holds a different master secret than the one used before.
    #[error("{MISMATCH_HEAD}{}{MISMATCH_TAIL}", .data_dir.display())]
    MasterSecretMismatch {
        /// The cluster's data directory.
        data_dir: PathBuf,
    },

    /// A test-only engine variable that is not a plain `FAKE_*` switch.
    #[error("engine variable {0:?} is not allowed")]
    BadEngineEnv(String),

    /// The bundled database failed.
    #[error(transparent)]
    Pg(#[from] cl_pg::PgError),

    /// The supervisor gave up (restarts exhausted or a permanent failure).
    #[error("{0}")]
    Fault(crate::HostFault),

    /// Waited for the engine and it did not become ready.
    #[error("the engine did not become ready within {0:?}")]
    Timeout(std::time::Duration),

    /// The engine is not running right now.
    #[error("the engine is not running")]
    NotRunning,

    /// Another program holds the engine's port, so nothing was sent to it.
    #[error("the engine's port is held by another program; nothing was sent")]
    ForeignListener,

    /// A first-run endpoint call failed.
    #[error(transparent)]
    Desktop(#[from] crate::desktop::DesktopError),
}

impl HostError {
    pub(crate) fn io(context: impl Into<String>, source: std::io::Error) -> Self {
        HostError::Io {
            context: context.into(),
            source,
        }
    }
}

/// Convenience alias used across the crate.
pub type Result<T> = std::result::Result<T, HostError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn master_secret_messages_are_recognised_whole() {
        let data_dir = PathBuf::from(r"C:\Users\alex.sample\AppData\Local\ClientLedger\pgdata");
        for e in [
            HostError::MasterSecretMissing {
                data_dir: data_dir.clone(),
            },
            HostError::MasterSecretMismatch { data_dir },
        ] {
            let text = e.to_string();
            assert!(text.starts_with("the engine's master secret "), "{text}");
            assert!(!text.contains("  "), "no runs of spaces: {text}");
            assert!(is_master_secret_message(&text), "{text}");
            // Anything added before, after, or on another line is not one.
            assert!(!is_master_secret_message(&format!(
                "{text} 9f3a2b7c4d5e6f70"
            )));
            assert!(!is_master_secret_message(&format!("{text}\nsecret=abc")));
            assert!(!is_master_secret_message(&format!("x {text}")));
        }
        assert!(!is_master_secret_message(&format!(
            "{MISSING_HEAD}a\nsecret=abc{MISSING_TAIL}"
        )));
        assert!(!is_master_secret_message(&format!(
            "{MISSING_HEAD}{MISSING_TAIL}"
        )));
        assert!(!is_master_secret_message(&format!(
            "{MISSING_HEAD}x{MISSING_TAIL}{MISSING_TAIL}"
        )));
        assert!(!is_master_secret_message(
            "the engine's master secret is abc123"
        ));
    }
}
