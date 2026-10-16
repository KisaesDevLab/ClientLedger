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

    /// The bundled database failed.
    #[error(transparent)]
    Pg(#[from] cl_pg::PgError),
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
