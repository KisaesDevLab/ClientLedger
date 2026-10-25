//! Local settings for `ClientLedger`.
//!
//! - [`user_prefs`]: per-user look-and-feel and entry choices (P09).
//! - [`company_prefs`]: company-local choices the engine has no column for
//!   (P17 receipt inbox, this computer's intake-folder confirmations,
//!   bills-due window, AI switches and cap, consent note).
//! - [`inbox_state`]: which receipt-inbox files were uploaded (by hash,
//!   never by name), so a file is not offered twice (P17).
//! - [`migrate_inbox`]: the pure plan that moves P17's single folder into
//!   the company's intake folders (A01 D5).
//! - [`ai`] and [`smtp`]: provider names, keychain entry names, the token
//!   cap, the Ollama address check, and SMTP field checks.
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

pub mod ai;
pub mod company_prefs;
pub mod inbox_state;
pub mod migrate_inbox;
pub mod recent;
pub mod secrets;
pub mod smtp;
mod store;
pub mod user_prefs;

pub use ai::{AiLocalPrefs, AiProvider, CapCheck, OllamaUrlError, validate_ollama_url};
pub use company_prefs::{
    AiConsentNote, CompanyPrefs, FolderError, HereNote, check_inbox_folder, path_hash,
};
pub use recent::{
    ACCENT_PALETTE, Accent, BackupKind, BackupRecord, CompanyFlags, CompanyRecord, FiscalCalendar,
    RecentCompanies, RecentError, RecentStore,
};
pub use secrets::{KeyringSecrets, MemorySecrets, SecretError, SecretStore};
pub use smtp::{SmtpFieldError, SmtpForm, SmtpSecurity};
pub use user_prefs::{
    DateFormat, DateWarnings, NegativeStyle, StartScreen, ThemeChoice, UpdateCheck, UserPrefs,
    UserPrefsStore,
};

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
