//! Company-local preferences (P09): choices made for one company that the
//! engine has no column for. They sit on the company's `companies.json`
//! record and travel in a backup's `manifest.host` (Q-P09c), never in the
//! engine. Nothing here is a secret or an amount.

use std::path::PathBuf;

use cl_core::Timestamp;
use serde::{Deserialize, Serialize};

use crate::ai::{AiLocalPrefs, AiProvider};
use crate::smtp::SmtpSecurity;

/// Default for "bills due within N days" on Home (Q-169).
pub const BILLS_DUE_DAYS_DEFAULT: u16 = 7;
/// Largest bills-due window accepted.
pub const BILLS_DUE_DAYS_MAX: u16 = 365;

/// The cloud-AI consent as this app recorded it (Q-136): the engine's audit
/// row carries date, user and disclosure version; the provider is kept
/// here until U-30 adds it to the engine row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AiConsentNote {
    /// The provider the owner agreed to send redacted data to.
    pub provider: AiProvider,
    /// Disclosure version accepted.
    pub disclosure_version: u32,
    /// When (UTC).
    pub accepted_at: Option<Timestamp>,
}

impl Default for AiConsentNote {
    fn default() -> Self {
        AiConsentNote {
            provider: AiProvider::Ollama,
            disclosure_version: 1,
            accepted_at: None,
        }
    }
}

/// Everything local to one company.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CompanyPrefs {
    /// Folder watched for new receipts and bills (Q-54); `None` = off.
    pub receipt_inbox_dir: Option<PathBuf>,
    /// Home shows bills due within this many days (Q-169).
    pub bills_due_days: u16,
    /// Local AI switches, cap and count.
    pub ai: AiLocalPrefs,
    /// Recorded cloud consent (see [`AiConsentNote`]).
    pub ai_consent: Option<AiConsentNote>,
    /// SMTP connection security (the rest of SMTP lives in the engine).
    pub smtp_security: SmtpSecurity,
}

impl Default for CompanyPrefs {
    fn default() -> Self {
        CompanyPrefs {
            receipt_inbox_dir: None,
            bills_due_days: BILLS_DUE_DAYS_DEFAULT,
            ai: AiLocalPrefs::default(),
            ai_consent: None,
            smtp_security: SmtpSecurity::default(),
        }
    }
}

impl CompanyPrefs {
    /// Snaps fields into range.
    pub fn normalize(&mut self) {
        self.bills_due_days = self.bills_due_days.clamp(1, BILLS_DUE_DAYS_MAX);
        if self
            .receipt_inbox_dir
            .as_ref()
            .is_some_and(|p| p.as_os_str().is_empty())
        {
            self.receipt_inbox_dir = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_round_trip_and_normalize() {
        let d = CompanyPrefs::default();
        assert_eq!(d.bills_due_days, 7);
        assert_eq!(
            d.ai.monthly_token_cap,
            Some(crate::ai::DEFAULT_MONTHLY_TOKEN_CAP)
        );
        let text = serde_json::to_string(&d).expect("json");
        assert_eq!(
            serde_json::from_str::<CompanyPrefs>(&text).expect("parse"),
            d
        );
        assert_eq!(
            serde_json::from_str::<CompanyPrefs>("{}").expect("empty"),
            d
        );
        let mut p = CompanyPrefs {
            bills_due_days: 0,
            receipt_inbox_dir: Some(PathBuf::new()),
            ai_consent: Some(AiConsentNote {
                provider: AiProvider::Anthropic,
                disclosure_version: 2,
                accepted_at: None,
            }),
            ..CompanyPrefs::default()
        };
        p.normalize();
        assert_eq!(p.bills_due_days, 1);
        assert_eq!(p.receipt_inbox_dir, None);
        p.bills_due_days = 9999;
        p.normalize();
        assert_eq!(p.bills_due_days, BILLS_DUE_DAYS_MAX);
        assert_eq!(AiConsentNote::default().disclosure_version, 1);
    }
}
