//! Errors for `cl-pg`. Messages are written for a log or a support request:
//! they never contain the database password.

use crate::paths::DataDirIssue;
use std::path::PathBuf;

/// Everything that can go wrong while managing the bundled `PostgreSQL`.
#[derive(Debug, thiserror::Error)]
pub enum PgError {
    /// The `PostgreSQL` binaries are not where we looked.
    #[error(
        "PostgreSQL binaries not found (looked in {}); run `cargo xtask pg-fetch` or reinstall ClientLedger",
        .searched.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ")
    )]
    BinariesNotFound {
        /// Directories that were checked.
        searched: Vec<PathBuf>,
    },

    /// The binaries folder exists but a required program is missing.
    #[error("PostgreSQL install at {} is incomplete: {} is missing", .root.display(), .missing)]
    BinariesIncomplete {
        /// The install root.
        root: PathBuf,
        /// Name of the missing program.
        missing: String,
    },

    /// The chosen data folder is not safe for a live database (Q-42).
    #[error("{issue}")]
    DataDir {
        /// What is wrong with the folder.
        issue: DataDirIssue,
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

    /// A helper program exited with an error.
    #[error("{program} failed ({status}): {detail}")]
    Command {
        /// The program name, such as `initdb`.
        program: &'static str,
        /// Exit status text.
        status: String,
        /// Scrubbed tail of its output.
        detail: String,
    },

    /// The cluster exists but its password is not in the keychain.
    #[error(
        "the database password for {} is not in this computer's secure storage; restore it or reset it with Cluster::reset_password",
        .data_dir.display()
    )]
    PasswordMissing {
        /// The data directory that needs its password.
        data_dir: PathBuf,
    },

    /// Reading or writing the stored password failed.
    #[error("secure storage error: {0}")]
    SecretStore(String),

    /// The operating system could not provide random bytes.
    #[error("no random source available: {0}")]
    Random(String),

    /// `PostgreSQL` would not start.
    #[error("PostgreSQL did not start after {attempts} attempt(s): {detail}")]
    StartFailed {
        /// How many ports were tried.
        attempts: u32,
        /// Scrubbed tail of the server log.
        detail: String,
    },

    /// A server for this data directory is running but not answering.
    #[error("a PostgreSQL process (pid {pid}) holds this data directory but is not answering")]
    StuckProcess {
        /// Process id from `postmaster.pid`.
        pid: u32,
    },

    /// The server refused an administrative statement.
    #[error("database setup step failed ({context}): {detail}")]
    Sql {
        /// What we were doing.
        context: String,
        /// The server's short message and SQLSTATE (no statement text).
        detail: String,
    },

    /// A backup file is not usable.
    #[error("backup problem: {0}")]
    Backup(String),
}

impl PgError {
    pub(crate) fn io(context: impl Into<String>, source: std::io::Error) -> Self {
        PgError::Io {
            context: context.into(),
            source,
        }
    }
}
