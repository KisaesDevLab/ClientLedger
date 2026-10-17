//! Company lifecycle (P08): what the Open Company window, the setup
//! interview, Back Up / Restore / Portable copy, Export All Data and
//! Delete Company do, built on the typed engine client (`cl-api`) and the
//! local recent-companies store (`cl-settings`).
//!
//! Rules this crate keeps (CLAUDE.md non-negotiables):
//! - The engine owns the books. Nothing here computes a balance; restore
//!   verification is the engine's own report, and the trial balance used
//!   for equality checks is the engine's CSV text.
//! - Nothing destructive without a verified backup: delete and
//!   "replace with an older backup" take a fresh archive first and refuse
//!   to run without its path and SHA-256 (Q-185, Q-P02i).
//! - Secrets (backup passphrase, closing-date password, recovery key)
//!   travel in `Secret` and are never logged or written to settings.
//! - Every call is `async` over a `cl_api::Session`; the UI runs them
//!   through its request queue and polls, so the egui thread never
//!   blocks.
#![forbid(unsafe_code)]

pub mod backup;
pub mod delete;
pub mod error;
pub mod export_all;
pub mod lifecycle;
pub mod restore;
pub mod retention;
pub mod setup;
pub mod templates;
pub mod year_end;
mod zipfile;

pub use backup::{BackupOutcome, BackupRequest, HostMetadata, Progress, ProgressSink, backup};
pub use delete::{DeleteOutcome, DeleteRequest, delete_company};
pub use error::CompanyError;
pub use export_all::{ExportAllOutcome, export_all};
pub use lifecycle::{CompanyEntry, close_company, list_companies, new_from_existing, open_company};
pub use restore::{ArchiveInfo, RestoreOutcome, RestoreRequest, inspect_archive, restore};
pub use setup::{CompanySetup, Created, SetupIssue, create_company};
pub use templates::{StarterChart, Template};
pub use year_end::{YearEndPrompt, year_end_prompt};

/// The app version written into archive host metadata.
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Returns the crate name; placeholder kept from the skeleton.
#[must_use]
pub fn crate_name() -> &'static str {
    "cl-company"
}

#[cfg(test)]
mod tests {
    #[test]
    fn crate_name_matches() {
        assert_eq!(super::crate_name(), "cl-company");
    }
}
