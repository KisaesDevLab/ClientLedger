//! Local settings for `ClientLedger` (P08 part; P09 adds user preferences,
//! AI and SMTP settings).
//!
//! - [`recent`]: the recent-companies store, a JSON file in the app data
//!   folder. It remembers, per company, what the engine does not store yet:
//!   the accent colour (Q-184), when the company was last opened and backed
//!   up, the backup history (`path`, `sha256`, `archiveId`, `createdAt`),
//!   and the setup flags that wait for U-25 (fiscal calendar, sales tax,
//!   inventory) (Q-P08a, Q-P08c).
//! - [`secrets`]: a small keychain-backed secret store for the backup
//!   passphrase (Q-P02i). Secrets never touch the JSON file.
//!
//! No I/O beyond its own files and the OS keychain; no processes; no
//! database.
#![forbid(unsafe_code)]

pub mod recent;
pub mod secrets;
mod store;

pub use recent::{
    ACCENT_PALETTE, Accent, BackupKind, BackupRecord, CompanyFlags, CompanyRecord, FiscalCalendar,
    RecentCompanies, RecentError, RecentStore,
};
pub use secrets::{KeyringSecrets, MemorySecrets, SecretError, SecretStore};

/// Returns the crate name; placeholder kept from the skeleton.
#[must_use]
pub fn crate_name() -> &'static str {
    "cl-settings"
}

#[cfg(test)]
mod tests {
    #[test]
    fn crate_name_matches() {
        assert_eq!(super::crate_name(), "cl-settings");
    }
}
