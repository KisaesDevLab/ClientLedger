//! One error type for every lifecycle operation, owner-facing through
//! `cl_core::OwnerFacing` (i18n keys only; the text lives in `cl-ui`).

use cl_api::ApiError;
use cl_core::{OwnerFacing, OwnerMessage};

use crate::setup::SetupIssue;

/// Why a company operation failed.
#[derive(Debug, thiserror::Error)]
pub enum CompanyError {
    /// The engine call failed.
    #[error("engine: {0}")]
    Api(#[from] ApiError),
    /// A file could not be read or written (`what` names the step, never
    /// the content).
    #[error("file ({what}): {source}")]
    Io {
        /// Step name (`copy backup`, `read archive`).
        what: &'static str,
        /// The error.
        #[source]
        source: std::io::Error,
    },
    /// The recent-companies file could not be read or saved.
    #[error("settings: {0}")]
    Settings(#[from] cl_settings::RecentError),
    /// The keychain could not be used.
    #[error("secrets: {0}")]
    Secrets(#[from] cl_settings::SecretError),
    /// The setup interview has something to fix.
    #[error("setup: {0:?}")]
    Setup(Vec<SetupIssue>),
    /// No backup passphrase is set yet (Q-P02i).
    #[error("no backup passphrase")]
    NoPassphrase,
    /// An archive job (export or import) failed or disappeared (engine
    /// restarted).
    #[error("archive job failed ({code})")]
    JobFailed {
        /// The engine's code (`ARCHIVE_PASSPHRASE_INVALID`, `ARCHIVE_CORRUPT`,
        /// `ARCHIVE_TOO_NEW`, ...), or `JOB_UNKNOWN` / `TIMEOUT`.
        code: String,
    },
    /// The file is not a `ClientLedger` backup.
    #[error("not a backup file")]
    NotABackup,
    /// The file's hash differs from the one recorded when it was written.
    #[error("backup file changed since it was written")]
    Tampered,
    /// The copied file does not match the engine's hash.
    #[error("copy verification failed")]
    CopyMismatch,
    /// The typed name does not match the company (Q-185).
    #[error("name mismatch")]
    NameMismatch,
    /// The company is not in the list.
    #[error("unknown company")]
    UnknownCompany,
    /// The engine does not offer this yet (capability named).
    #[error("needs engine capability {0}")]
    Unavailable(&'static str),
    /// The archive host metadata could not be built.
    #[error("archive metadata")]
    Metadata,
}

impl CompanyError {
    pub(crate) fn io(what: &'static str) -> impl FnOnce(std::io::Error) -> CompanyError {
        move |source| CompanyError::Io { what, source }
    }
}

impl OwnerFacing for CompanyError {
    fn owner_message(&self) -> OwnerMessage {
        match self {
            CompanyError::Api(e) => e.owner_message(),
            CompanyError::Io { .. } => {
                OwnerMessage::with_fix("error.company.file", "fix.company.file")
            }
            CompanyError::Settings(_) => OwnerMessage::plain("error.company.settings"),
            CompanyError::Secrets(_) => {
                OwnerMessage::with_fix("error.company.keychain", "fix.company.keychain")
            }
            CompanyError::Setup(_) => {
                OwnerMessage::with_fix("error.company.setup", "fix.company.setup")
            }
            CompanyError::NoPassphrase => {
                OwnerMessage::with_fix("error.company.no_passphrase", "fix.company.no_passphrase")
            }
            CompanyError::JobFailed { code } => match code.as_str() {
                "ARCHIVE_PASSPHRASE_INVALID" => {
                    OwnerMessage::with_fix("error.company.passphrase", "fix.company.passphrase")
                }
                "ARCHIVE_CORRUPT" | "ARCHIVE_UNSUPPORTED" => {
                    OwnerMessage::with_fix("error.company.tampered", "fix.company.tampered")
                }
                "ARCHIVE_TOO_NEW" => {
                    OwnerMessage::with_fix("error.company.too_new", "fix.company.too_new")
                }
                _ => OwnerMessage::with_fix("error.company.job_failed", "fix.api.try_again_soon"),
            },
            CompanyError::NotABackup => {
                OwnerMessage::with_fix("error.company.not_a_backup", "fix.company.not_a_backup")
            }
            CompanyError::Tampered => {
                OwnerMessage::with_fix("error.company.tampered", "fix.company.tampered")
            }
            CompanyError::CopyMismatch => {
                OwnerMessage::with_fix("error.company.copy_mismatch", "fix.company.copy_mismatch")
            }
            CompanyError::NameMismatch => {
                OwnerMessage::with_fix("error.company.name_mismatch", "fix.company.name_mismatch")
            }
            CompanyError::UnknownCompany => OwnerMessage::plain("error.company.unknown"),
            CompanyError::Unavailable(_) => {
                OwnerMessage::with_fix("error.company.unavailable", "fix.company.unavailable")
            }
            CompanyError::Metadata => OwnerMessage::plain("error.internal"),
        }
    }

    fn scrubbed_detail(&self) -> String {
        match self {
            CompanyError::Api(e) => e.scrubbed_detail(),
            CompanyError::Io { what, source } => format!("io {what}: {:?}", source.kind()),
            CompanyError::Settings(_) => "settings file".into(),
            CompanyError::Secrets(e) => match e {
                cl_settings::SecretError::InvalidName => "secret name".into(),
                cl_settings::SecretError::Keychain(_) => "keychain".into(),
            },
            CompanyError::Setup(issues) => format!("setup issues: {issues:?}"),
            CompanyError::JobFailed { code } => format!("job {code}"),
            CompanyError::Unavailable(cap) => format!("capability {cap}"),
            other => format!("{other}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn details_never_carry_paths_or_names() {
        let e = CompanyError::Io {
            what: "copy backup",
            source: std::io::Error::new(std::io::ErrorKind::NotFound, "C:/secret/Ozark.clbackup"),
        };
        assert_eq!(e.scrubbed_detail(), "io copy backup: NotFound");
        assert_eq!(e.owner_message().key, "error.company.file");
        let e = CompanyError::Secrets(cl_settings::SecretError::Keychain(
            "entry for Ozark Ridge".into(),
        ));
        assert_eq!(e.scrubbed_detail(), "keychain");
        let e = CompanyError::NameMismatch;
        assert_eq!(
            e.owner_message().suggestion_key,
            Some("fix.company.name_mismatch")
        );
    }
}
