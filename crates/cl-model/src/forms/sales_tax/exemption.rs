//! A client's exemption certificate (P23 plan §2.7, Q-P23n) and the strip
//! a sales document shows for the engine's exemption warnings.

use std::fmt;

use cl_core::CalendarDate;
use cl_core::ids::{ContactId, TaxExemptionId};

use crate::text::TextKey;

/// Longest certificate number.
pub const NUMBER_MAX: usize = 50;
/// Longest note.
pub const NOTE_MAX: usize = 1000;

/// Why a certificate can't save yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ExemptionProblem {
    /// No client.
    NoClient,
    /// No certificate number.
    NumberRequired,
    /// Number over 50 characters.
    NumberTooLong,
    /// The state is not two letters.
    BadState,
    /// Note over 1,000 characters.
    NoteTooLong,
}

impl ExemptionProblem {
    /// Plain-language message (strings live in `cl-ui::i18n`).
    #[must_use]
    pub const fn key(self) -> TextKey {
        match self {
            ExemptionProblem::NoClient => TextKey("tax_exemptions.problem.no_client"),
            ExemptionProblem::NumberRequired => TextKey("tax_exemptions.problem.number_required"),
            ExemptionProblem::NumberTooLong => TextKey("tax_exemptions.problem.number_too_long"),
            ExemptionProblem::BadState => TextKey("tax_exemptions.problem.bad_state"),
            ExemptionProblem::NoteTooLong => TextKey("tax_exemptions.problem.note_too_long"),
        }
    }
}

/// The certificate editor.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct ExemptionForm {
    /// The saved certificate being edited (`None`: new).
    pub id: Option<TaxExemptionId>,
    /// The client.
    pub client: Option<ContactId>,
    /// The certificate number (owner data).
    pub number: String,
    /// The issuing state (`MO`).
    pub state: String,
    /// When it expires.
    pub expires_on: Option<CalendarDate>,
    /// A note (owner data).
    pub note: String,
}

/// Owner data stays out of logs.
impl fmt::Debug for ExemptionForm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExemptionForm")
            .field("id", &self.id)
            .field("client", &self.client)
            .field("expires_on", &self.expires_on)
            .finish_non_exhaustive()
    }
}

/// What a checked certificate sends.
#[derive(Clone, PartialEq, Eq)]
pub struct ExemptionPlan {
    /// The client.
    pub client: ContactId,
    /// The number (trimmed).
    pub number: String,
    /// The state, upper case (`None` when blank).
    pub state: Option<String>,
    /// When it expires.
    pub expires_on: Option<CalendarDate>,
    /// The note (`None` when blank).
    pub note: Option<String>,
}

impl fmt::Debug for ExemptionPlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExemptionPlan")
            .field("client", &self.client)
            .field("state", &self.state)
            .field("expires_on", &self.expires_on)
            .finish_non_exhaustive()
    }
}

impl ExemptionForm {
    /// The plan to send.
    ///
    /// # Errors
    /// The first [`ExemptionProblem`].
    pub fn plan(&self) -> Result<ExemptionPlan, ExemptionProblem> {
        let client = self.client.ok_or(ExemptionProblem::NoClient)?;
        let number = self.number.trim();
        if number.is_empty() {
            return Err(ExemptionProblem::NumberRequired);
        }
        if number.chars().count() > NUMBER_MAX {
            return Err(ExemptionProblem::NumberTooLong);
        }
        let state = match self.state.trim() {
            "" => None,
            s if s.len() == 2 && s.bytes().all(|b| b.is_ascii_alphabetic()) => {
                Some(s.to_ascii_uppercase())
            }
            _ => return Err(ExemptionProblem::BadState),
        };
        let note = self.note.trim();
        if note.chars().count() > NOTE_MAX {
            return Err(ExemptionProblem::NoteTooLong);
        }
        Ok(ExemptionPlan {
            client,
            number: number.to_string(),
            state,
            expires_on: self.expires_on,
            note: (!note.is_empty()).then(|| note.to_string()),
        })
    }
}

/// The exemption strip on an invoice, sales receipt or credit memo, from
/// the preview's problem codes (`EXEMPTION_MISSING`, `EXEMPTION_EXPIRED`,
/// `EXEMPTION_EXPIRING`). Expired wins over expiring over missing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ExemptionStrip {
    /// A non-taxable client has no certificate on file.
    Missing,
    /// The certificate has expired.
    Expired,
    /// It expires within 30 days.
    Expiring,
}

impl ExemptionStrip {
    /// The strip for these preview problem codes (`None`: no strip).
    #[must_use]
    pub fn from_codes<'a>(codes: impl IntoIterator<Item = &'a str>) -> Option<ExemptionStrip> {
        let mut best: Option<ExemptionStrip> = None;
        for c in codes {
            let s = match c {
                "EXEMPTION_EXPIRED" => ExemptionStrip::Expired,
                "EXEMPTION_EXPIRING" => ExemptionStrip::Expiring,
                "EXEMPTION_MISSING" => ExemptionStrip::Missing,
                _ => continue,
            };
            if best.is_none_or(|b| s.rank() > b.rank()) {
                best = Some(s);
            }
        }
        best
    }

    const fn rank(self) -> u8 {
        match self {
            ExemptionStrip::Missing => 0,
            ExemptionStrip::Expiring => 1,
            ExemptionStrip::Expired => 2,
        }
    }

    /// Plain-language message (strings live in `cl-ui::i18n`).
    #[must_use]
    pub const fn key(self) -> TextKey {
        match self {
            ExemptionStrip::Missing => TextKey("tax_exemptions.strip.missing"),
            ExemptionStrip::Expired => TextKey("tax_exemptions.strip.expired"),
            ExemptionStrip::Expiring => TextKey("tax_exemptions.strip.expiring"),
        }
    }
}
