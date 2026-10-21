//! Stored transaction templates (P15 lane T, U-62, `templates-v1`):
//! `/templates` CRUD, groups, `from-transaction`, `GET /{id}/draft`,
//! `enter`, `skip` and `resolve`.
//!
//! A template is the exact create body of its form's engine endpoint, minus
//! the date and numbers (`target/plans/P15-engine-templates.md` §3), so
//! [`TemplateDraft`] wraps the same checked drafts the forms already save.
//! Amounts go out as `Money::to_wire()` strings and come back through the
//! strict `Money::parse_wire`; a JSON number is a parse error. The list
//! `total` is display-only and never summed.
//!
//! WP-S scaffold: the draft enum and the warning shape the forms'
//! `TemplateSource` hook needs; lane T adds the calls and the wire mapping.

use crate::resources::bank_postings::{CardChargeDraft, CheckDraft, DepositDraft, TransferDraft};
use crate::resources::daily_sales::DailyEntryDraft;
use crate::resources::transactions::NewJournalEntry;
use crate::session::Session;

/// What a template stores: one form's checked draft (its date and numbers
/// are ignored on the wire). A card credit is a [`TemplateDraft::Deposit`]
/// into the card, told apart by the template's `clientForm`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TemplateDraft {
    /// Write Check (`check`).
    Check(CheckDraft),
    /// Card charge or other non-check payment (`expense`).
    CardCharge(CardChargeDraft),
    /// Make Deposit or card credit (`deposit`; never with grouped payments).
    Deposit(DepositDraft),
    /// Transfer Funds (`transfer`).
    Transfer(TransferDraft),
    /// Make Journal Entry (`journal_entry`; never with Reverse on).
    Journal(NewJournalEntry),
    /// Daily Sales Summary (`daily_sales`; never "Enter automatically").
    DailySales(DailyEntryDraft),
}

/// Why a reference in a template's draft needs the owner's attention
/// (`GET /templates/{id}/draft` `warnings[].code`).
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum DraftWarningCode {
    /// `ACCOUNT_INACTIVE`.
    AccountInactive,
    /// `CONTACT_INACTIVE`.
    ContactInactive,
    /// `TAG_INACTIVE`.
    TagInactive,
    /// `NOT_FOUND`: the reference was removed from the draft.
    NotFound,
    /// A code this version does not know.
    Other(String),
}

/// One warning on a template's draft: the draft field (engine path) and
/// why. The form shows it as a banner; nothing is logged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DraftWarning {
    /// The draft field, as the engine names it (`lines[0].accountId`).
    pub field: String,
    /// Why.
    pub code: DraftWarningCode,
}

/// Template calls of a session (`session.templates()`).
#[derive(Clone, Copy, Debug)]
pub struct Templates<'a>(
    #[allow(dead_code, reason = "P15 lane T adds the calls")] pub(crate) &'a Session,
);

impl Session {
    /// Templates of the open company.
    #[must_use]
    pub fn templates(&self) -> Templates<'_> {
        Templates(self)
    }
}
