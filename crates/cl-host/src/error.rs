//! Errors for `cl-host`. Messages never contain the launch token or a
//! database password: those travel only as [`cl_pg::Secret`] values.

use std::path::PathBuf;

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
    #[error(
        "the engine's master secret for {} is not in this computer's secure storage.          Your books are untouched, but they cannot be opened until it is back. If the          keychain was only temporarily unavailable, unlock it and start ClientLedger          again; otherwise restore the data folder together with its saved secrets          (ClientLedger will not invent a new one, because that would lock you out of          data it encrypted earlier)",
        .data_dir.display()
    )]
    MasterSecretMissing {
        /// The cluster's data directory.
        data_dir: PathBuf,
    },

    /// Secure storage holds a different master secret than the one used before.
    #[error(
        "the engine's master secret in this computer's secure storage is not the one used          for {}. Restore the matching secret (for example by restoring the data folder          with its saved secrets) rather than continuing",
        .data_dir.display()
    )]
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
