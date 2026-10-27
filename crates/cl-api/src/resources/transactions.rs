//! Transactions and journal entries (`/api/v1/transactions`, P05 plan
//! §2.8, §5).
//!
//! - A [`NewJournalEntry`] can only be built through
//!   [`NewJournalEntry::new`], which refuses before anything is sent: fewer
//!   than 2 or more than 500 lines, any amount that is not above zero, and
//!   debits that differ from credits by any amount (no 0.0001 tolerance).
//!   A line has one side by construction ([`Side`]) and `Money` cannot hold
//!   more than 4 decimals, so "both sides" and "5 decimals" cannot be
//!   expressed at all. The engine stays authoritative (NN7).
//! - Create, update and void carry a [`Probe`] (ledger-version stamp plus
//!   a fingerprint matcher) so an ambiguous failure is reconciled before the
//!   owner is told the outcome is unknown (§2.6.1).
//! - A 423 `TB_PERIOD_LOCKED` is `ApiError::ClosedPeriod`; there is no way
//!   to send `overrideConfirmed` ([`PostingOptions`], §2.7).
//! - P15 part A (`target/plans/P15-engine-shapes-a.md`): line names
//!   (`contactId`, `je-names-v1`) with the A/R-A/P rule (Q-206), the entry
//!   number (`txnNumber`, `je-number-v1`), the Adjusting flag
//!   (`isAdjusting`, `je-adjusting-v1`) and Reverse on (`reverseOn`,
//!   `je-reversing-v1`). [`NewJournalEntry::new_with`] refuses what the
//!   engine would refuse; a field set without its capability is refused at
//!   send time, nothing sent.

use std::collections::BTreeSet;

use cl_core::ids::{AccountId, ContactId, JournalLineId, TagId, TransactionId};
use cl_core::{CalendarDate, Money, Timestamp};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::client::{Call, Method};
use crate::errors::{ApiError, LocalValidation};
use crate::idempotency::IdempotencyKey;
use crate::resources::common::{self, Query, SortDir, Spec};
use crate::resources::operations::{
    JE_NEXT_NUMBER, TXN_DELETE, TXN_GET, TXN_PUT, TXN_VOID, TXNS_CREATE, TXNS_LIST,
};
use crate::resources::posting::{PostingOptions, posting_body};
use crate::resources::reconcile::{OutcomeMatcher, Probe, Search, row_fp};
use crate::resources::versioning::{
    EditableFields, GuardedUpdate, RecordVersion, Versioned, fingerprint_of, saved_from,
    update_guarded,
};
use crate::session::Session;
use crate::wire::{Capabilities, Page, PageRequest};

pub(crate) const SPEC: Spec = Spec {
    list: &TXNS_LIST,
    create: &TXNS_CREATE,
    get: &TXN_GET,
    put: &TXN_PUT,
    delete: &TXN_VOID,
    base: "/api/v1/transactions",
    item_key: "transaction",
    list_key: "data",
};

/// The most lines a journal entry may have (engine `MAX_LINES`).
pub const MAX_LINES: usize = 500;

/// The longest journal entry number the engine takes, after trimming
/// (characters, `je-number-v1`).
pub const JE_NUMBER_MAX_CHARS: usize = 20;

/// Kind of transaction (`txnType`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(missing_docs, reason = "the engine's transaction type names")]
pub enum TxnType {
    Invoice,
    CustomerPayment,
    CashSale,
    Expense,
    Deposit,
    Transfer,
    JournalEntry,
    CreditMemo,
    CustomerRefund,
    Bill,
    VendorCredit,
    BillPayment,
    DailySales,
    /// Adjusting entry (Trial Balance module; not editable here).
    Aje,
}

impl TxnType {
    /// True for the kinds the engine's soft delete takes (Q-245): journal
    /// entries, checks/expenses, transfers, deposits and sales receipts.
    /// Everything else (invoices, bills, payments, ...) is voided instead.
    /// The engine still refuses a deposit that groups payments
    /// (`TXN_DELETE_LINKED`).
    #[must_use]
    pub const fn is_deletable(self) -> bool {
        matches!(
            self,
            TxnType::JournalEntry
                | TxnType::Expense
                | TxnType::Transfer
                | TxnType::Deposit
                | TxnType::CashSale
        )
    }

    pub(crate) fn as_str(self) -> String {
        serde_json::to_value(self)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default()
    }
}

/// Status of a transaction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TxnStatus {
    /// Not posted.
    Draft,
    /// Posted to the ledger.
    Posted,
    /// Voided (reversal lines posted).
    Void,
    /// Deleted (R7 soft delete: reversing lines, hidden from registers and
    /// reports, kept in the audit trail).
    Deleted,
}

/// Which report bases a journal entry affects.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Basis {
    /// Cash-basis reports only.
    Cash,
    /// Accrual-basis reports only.
    Accrual,
    /// Both (the default).
    #[default]
    Both,
}

/// One side of a journal line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    /// Debit amount (greater than zero).
    Debit(Money),
    /// Credit amount (greater than zero).
    Credit(Money),
}

impl Side {
    /// The amount (always above zero on a checked entry).
    #[must_use]
    pub const fn amount(self) -> Money {
        match self {
            Side::Debit(m) | Side::Credit(m) => m,
        }
    }

    fn wire(self) -> (String, String) {
        match self {
            Side::Debit(m) => (m.to_wire(), Money::ZERO.to_wire()),
            Side::Credit(m) => (Money::ZERO.to_wire(), m.to_wire()),
        }
    }
}

/// One line of a new journal entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JeLine {
    /// Account.
    pub account: AccountId,
    /// Debit or credit.
    pub side: Side,
    /// Line description (owner data: never logged).
    pub description: Option<String>,
    /// Line tag.
    pub tag: Option<TagId>,
    /// The line's name (customer, vendor or other name; `je-names-v1`).
    /// Every Accounts Receivable or Accounts Payable line needs one; an
    /// entry may have any number of them (Q-206).
    pub contact: Option<ContactId>,
}

/// The header options of a journal entry (P15 part A). The default is an
/// entry without a number, not adjusting and not reversing, which is what
/// every engine takes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct JeOptions {
    /// Entry number (`txnNumber`, free text, at most 20 characters after
    /// trimming; blank is none). Needs `je-number-v1`.
    pub number: Option<String>,
    /// An adjusting entry (`isAdjusting`). Needs `je-adjusting-v1`.
    pub adjusting: bool,
    /// Posts a reversing entry on this date, which must be after the
    /// entry's date (`reverseOn`). Needs `je-reversing-v1`.
    pub reverse_on: Option<CalendarDate>,
}

/// The Accounts Receivable and Accounts Payable accounts of the chart (the
/// caller reads them from each account's system tag or detail type). The
/// A/R-A/P name rule (Q-206) is checked against them before anything is
/// sent.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AccountRoles {
    /// Accounts Receivable accounts.
    pub receivable: BTreeSet<AccountId>,
    /// Accounts Payable accounts.
    pub payable: BTreeSet<AccountId>,
}

impl AccountRoles {
    /// True for an Accounts Receivable account.
    #[must_use]
    pub fn is_receivable(&self, a: AccountId) -> bool {
        self.receivable.contains(&a)
    }

    /// True for an Accounts Payable account.
    #[must_use]
    pub fn is_payable(&self, a: AccountId) -> bool {
        self.payable.contains(&a)
    }

    /// True for an Accounts Receivable or Accounts Payable account.
    #[must_use]
    pub fn is_ar_ap(&self, a: AccountId) -> bool {
        self.is_receivable(a) || self.is_payable(a)
    }
}

/// A balanced journal entry, checked before sending.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewJournalEntry {
    txn_date: CalendarDate,
    memo: Option<String>,
    basis: Basis,
    lines: Vec<JeLine>,
    number: Option<String>,
    adjusting: bool,
    reverse_on: Option<CalendarDate>,
}

/// Engine code: an A/R or A/P line without a name (400, `je-names-v1`).
pub const JE_AR_AP_NAME_REQUIRED: &str = "JE_AR_AP_NAME_REQUIRED";
/// Engine code: more than one A/R or A/P line in one entry (400). Only
/// engines before 33a973fa send it; the client no longer refuses such
/// entries itself (Q-206).
pub const JE_AR_AP_ONE_PER_ENTRY: &str = "JE_AR_AP_ONE_PER_ENTRY";
/// Engine code: a void, delete or edit of an entry whose reversal was
/// changed since it was made (409; void or delete the reversal first).
pub const REVERSAL_EDITED: &str = "REVERSAL_EDITED";

/// True when `e` carries the engine code `code` (validation or conflict).
#[must_use]
pub fn has_code(e: &ApiError, code: &str) -> bool {
    match e {
        ApiError::Validation(v) => v.code().is_some_and(|c| c.as_str() == code),
        ApiError::Conflict { code: Some(c), .. } | ApiError::Forbidden { code: Some(c) } => {
            c.as_str() == code
        }
        _ => false,
    }
}

/// The longest delete reason the engine takes (characters).
pub const DELETE_REASON_MAX_CHARS: usize = 500;

/// Options of a soft delete (R7).
#[derive(Clone, Default)]
pub struct DeleteOptions {
    /// Why it is deleted (optional, at most 500 characters; recorded in
    /// the audit trail).
    pub reason: Option<String>,
    /// The closing-date password, for a transaction in a closed period
    /// (U-15). Never logged.
    pub closing_password: Option<cl_core::Secret>,
}

impl std::fmt::Debug for DeleteOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeleteOptions")
            .field("reason", &self.reason.as_ref().map(|_| ".."))
            .field(
                "closing_password",
                &self.closing_password.as_ref().map(|_| "***"),
            )
            .finish()
    }
}

pub(crate) fn non_empty(s: Option<String>) -> Option<String> {
    s.filter(|v| !v.is_empty())
}

impl NewJournalEntry {
    /// Checks and builds an entry without a number, not adjusting and not
    /// reversing, with no A/R-A/P accounts known
    /// ([`NewJournalEntry::new_with`] with the defaults).
    ///
    /// # Errors
    /// As [`NewJournalEntry::new_with`].
    pub fn new(
        txn_date: CalendarDate,
        memo: Option<String>,
        basis: Basis,
        lines: Vec<JeLine>,
    ) -> Result<NewJournalEntry, LocalValidation> {
        Self::new_with(
            txn_date,
            memo,
            basis,
            lines,
            JeOptions::default(),
            &AccountRoles::default(),
        )
    }

    /// Checks and builds an entry. Empty memo, descriptions and number
    /// count as none (the engine stores them as `null`).
    ///
    /// # Errors
    /// `TooFewLines` (< 2), `TooManyLines` (> 500), `NonPositiveAmount`,
    /// `Unbalanced` (debits ≠ credits exactly), `InvalidValue` when a total
    /// overflows the money range or the number is over 20 characters,
    /// `ReverseNotAfter` (Reverse on not after the date) and
    /// `ArApNameMissing` (an A/R or A/P line without a name; any number of
    /// named A/R or A/P lines is fine, Q-206).
    pub fn new_with(
        txn_date: CalendarDate,
        memo: Option<String>,
        basis: Basis,
        lines: Vec<JeLine>,
        opts: JeOptions,
        roles: &AccountRoles,
    ) -> Result<NewJournalEntry, LocalValidation> {
        if lines.len() < 2 {
            return Err(LocalValidation::TooFewLines);
        }
        if lines.len() > MAX_LINES {
            return Err(LocalValidation::TooManyLines);
        }
        if lines.iter().any(|l| !l.side.amount().is_positive()) {
            return Err(LocalValidation::NonPositiveAmount);
        }
        let total = |debit: bool| {
            Money::sum(lines.iter().filter_map(|l| match (l.side, debit) {
                (Side::Debit(m), true) | (Side::Credit(m), false) => Some(m),
                _ => None,
            }))
            .map_err(|_| LocalValidation::InvalidValue)
        };
        if total(true)? != total(false)? {
            return Err(LocalValidation::Unbalanced);
        }
        let number = opts
            .number
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty());
        if number
            .as_ref()
            .is_some_and(|n| n.chars().count() > JE_NUMBER_MAX_CHARS)
        {
            return Err(LocalValidation::InvalidValue);
        }
        if opts.reverse_on.is_some_and(|r| r <= txn_date) {
            return Err(LocalValidation::ReverseNotAfter);
        }
        // Q-206 (owner 2026-10-25, engine 33a973fa): any number of A/R or
        // A/P lines, each with a name.
        if lines
            .iter()
            .any(|l| roles.is_ar_ap(l.account) && l.contact.is_none())
        {
            return Err(LocalValidation::ArApNameMissing);
        }
        let lines = lines
            .into_iter()
            .map(|l| JeLine {
                description: non_empty(l.description),
                ..l
            })
            .collect();
        Ok(NewJournalEntry {
            txn_date,
            memo: non_empty(memo),
            basis,
            lines,
            number,
            adjusting: opts.adjusting,
            reverse_on: opts.reverse_on,
        })
    }

    /// Entry number (trimmed; `None` when blank).
    #[must_use]
    pub fn number(&self) -> Option<&str> {
        self.number.as_deref()
    }

    /// True for an adjusting entry.
    #[must_use]
    pub fn adjusting(&self) -> bool {
        self.adjusting
    }

    /// The date of the reversing entry, if any.
    #[must_use]
    pub fn reverse_on(&self) -> Option<CalendarDate> {
        self.reverse_on
    }

    /// The header options this entry was built with.
    #[must_use]
    pub fn options(&self) -> JeOptions {
        JeOptions {
            number: self.number.clone(),
            adjusting: self.adjusting,
            reverse_on: self.reverse_on,
        }
    }

    /// Refuses, before sending, what an engine without the matching
    /// capability cannot store.
    fn require(&self, caps: &Capabilities) -> Result<(), ApiError> {
        let need = |set: bool, cap: &str, refusal: LocalValidation| {
            if set && !caps.has(cap) {
                Err(ApiError::Local(refusal))
            } else {
                Ok(())
            }
        };
        need(
            self.lines.iter().any(|l| l.contact.is_some()),
            Capabilities::JE_NAMES,
            LocalValidation::JeNamesUnavailable,
        )?;
        need(
            self.number.is_some(),
            Capabilities::JE_NUMBER,
            LocalValidation::JeNumberUnavailable,
        )?;
        need(
            self.adjusting,
            Capabilities::JE_ADJUSTING,
            LocalValidation::JeAdjustingUnavailable,
        )?;
        need(
            self.reverse_on.is_some(),
            Capabilities::JE_REVERSING,
            LocalValidation::JeReversingUnavailable,
        )
    }

    /// The update body: the create body plus the explicit "cleared" values
    /// an engine with the capability needs (a `PUT` without `txnNumber`
    /// keeps the old number, without `isAdjusting` keeps the flag, and
    /// without `reverseOn` keeps the stored reversal — so `reverseOn` is
    /// always sent: the date, or `null` to remove the reversal).
    pub(crate) fn to_update_value(&self, caps: &Capabilities) -> Value {
        let mut v = self.to_value();
        if caps.has(Capabilities::JE_NUMBER) && self.number.is_none() {
            v["txnNumber"] = Value::String(String::new());
        }
        if caps.has(Capabilities::JE_ADJUSTING) {
            v["isAdjusting"] = Value::Bool(self.adjusting);
        }
        if caps.has(Capabilities::JE_REVERSING) {
            v["reverseOn"] = self.reverse_on.map_or(Value::Null, |r| json!(r));
        }
        v
    }

    /// Entry date.
    #[must_use]
    pub fn txn_date(&self) -> CalendarDate {
        self.txn_date
    }

    /// Memo.
    #[must_use]
    pub fn memo(&self) -> Option<&str> {
        self.memo.as_deref()
    }

    /// Report basis.
    #[must_use]
    pub fn basis(&self) -> Basis {
        self.basis
    }

    /// Lines in order.
    #[must_use]
    pub fn lines(&self) -> &[JeLine] {
        &self.lines
    }

    /// The request body (both sides as canonical 4-decimal strings).
    pub(crate) fn to_value(&self) -> Value {
        let lines: Vec<Value> = self
            .lines
            .iter()
            .map(|l| {
                let (debit, credit) = l.side.wire();
                let mut v = json!({
                    "accountId": l.account,
                    "debit": debit,
                    "credit": credit,
                });
                if let Some(d) = &l.description {
                    v["description"] = Value::String(d.clone());
                }
                if let Some(t) = l.tag {
                    v["tagId"] = json!(t);
                }
                if let Some(c) = l.contact {
                    v["contactId"] = json!(c);
                }
                v
            })
            .collect();
        let mut v = json!({
            "txnType": "journal_entry",
            "txnDate": self.txn_date,
            "basis": self.basis,
            "lines": lines,
        });
        if let Some(m) = &self.memo {
            v["memo"] = Value::String(m.clone());
        }
        if let Some(n) = &self.number {
            v["txnNumber"] = Value::String(n.clone());
        }
        if self.adjusting {
            v["isAdjusting"] = Value::Bool(true);
        }
        if let Some(r) = self.reverse_on {
            v["reverseOn"] = json!(r);
        }
        v
    }

    /// The fingerprint the posted entry has (same fields as
    /// [`Transaction`]'s editable set).
    #[must_use]
    pub fn fingerprint(&self) -> [u8; 32] {
        let lines = self.lines.iter().enumerate().map(|(i, l)| {
            let (debit, credit) = l.side.wire();
            line_editable(
                l.account,
                &debit,
                &credit,
                l.description.as_deref(),
                l.tag,
                l.contact,
                false,
                i64::try_from(i).unwrap_or(i64::MAX),
            )
        });
        let mut v = txn_editable(
            TxnType::JournalEntry,
            self.txn_date,
            self.memo.as_deref(),
            self.basis,
            TxnStatus::Posted,
            None,
            &lines.collect::<Vec<_>>(),
        );
        je_editable(
            &mut v,
            self.number.as_deref(),
            self.adjusting,
            self.reverse_on,
        );
        fingerprint_of(&v)
    }
}

/// The P15 journal entry fields, hashed only when set so every fingerprint
/// without them stays what it was before P15.
fn je_editable(
    v: &mut Value,
    number: Option<&str>,
    adjusting: bool,
    reverse_on: Option<CalendarDate>,
) {
    if let Some(n) = number.map(str::trim).filter(|n| !n.is_empty()) {
        v["txnNumber"] = Value::String(n.to_string());
    }
    if adjusting {
        v["isAdjusting"] = Value::Bool(true);
    }
    if let Some(r) = reverse_on {
        v["reverseOn"] = json!(r);
    }
}

/// The editable fields of one line. `isBillable` (`line-billable-v1`) is
/// hashed only when set, so the fingerprints of every non-billable line,
/// from any engine, stay what they were before P13.
#[allow(clippy::too_many_arguments, reason = "one argument per hashed field")]
pub(crate) fn line_editable(
    account: AccountId,
    debit: &str,
    credit: &str,
    description: Option<&str>,
    tag: Option<TagId>,
    contact: Option<ContactId>,
    billable: bool,
    order: i64,
) -> Value {
    let mut v = json!({
        "accountId": account,
        "debit": debit,
        "credit": credit,
        "description": description.filter(|d| !d.is_empty()),
        "tagId": tag,
        "contactId": contact,
        "lineOrder": order,
    });
    if billable {
        v["isBillable"] = Value::Bool(true);
    }
    v
}

/// A line's source document (P14: a grouped payment or a refunded vendor
/// credit), hashed only when set so every other fingerprint stays what it
/// was before P14.
pub(crate) fn with_source(mut line: Value, source: Option<TransactionId>) -> Value {
    if let Some(s) = source {
        line["sourceTransactionId"] = json!(s);
    }
    line
}

pub(crate) fn txn_editable(
    txn_type: TxnType,
    date: CalendarDate,
    memo: Option<&str>,
    basis: Basis,
    status: TxnStatus,
    contact: Option<ContactId>,
    lines: &[Value],
) -> Value {
    json!({
        "txnType": txn_type,
        "txnDate": date,
        "memo": memo.filter(|m| !m.is_empty()),
        "basis": basis,
        "status": status,
        "contactId": contact,
        "lines": lines,
    })
}

/// One stored line of a transaction (void-reversal lines are not shown).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransactionLine {
    /// Line id (recreated by every update; never part of a fingerprint).
    pub id: JournalLineId,
    /// Account.
    pub account_id: AccountId,
    /// Account name (joined; display only).
    #[serde(default)]
    pub account_name: Option<String>,
    /// Account number (joined; display only).
    #[serde(default)]
    pub account_number: Option<String>,
    /// Debit.
    pub debit: Money,
    /// Credit.
    pub credit: Money,
    /// Description.
    #[serde(default)]
    pub description: Option<String>,
    /// Position.
    #[serde(default)]
    pub line_order: Option<i64>,
    /// Tag.
    #[serde(default)]
    pub tag_id: Option<TagId>,
    /// Tag name (joined; display only).
    #[serde(default)]
    pub tag_name: Option<String>,
    /// Line contact (a deposit's customer; with `line-billable-v1` also
    /// the customer on a check or expense line).
    #[serde(default)]
    pub contact_id: Option<ContactId>,
    /// Line contact name (joined; display only; `je-names-v1`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contact_name: Option<String>,
    /// The line is billable to its customer (`line-billable-v1`; `false`
    /// from engines without it).
    #[serde(default)]
    pub is_billable: bool,
    /// Whether a billable line was invoiced yet (`line-billable-v1`;
    /// engine-maintained, never part of a fingerprint).
    #[serde(default)]
    pub billed_status: Option<BilledStatus>,
    /// The document a deposit line takes in: a payment out of Payments
    /// Clearing (`deposit-grouping-v1`) or a vendor credit refunded
    /// (`vendor-refund-v1`). Hashed only when set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_transaction_id: Option<TransactionId>,
    /// What the line is for inside its document (`cash_back` on a
    /// deposit's cash-back line, `deposit-cashback-v1`). Engine-made,
    /// never part of a fingerprint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_role: Option<String>,
}

/// `line_role` of a deposit's cash-back line.
pub const LINE_ROLE_CASH_BACK: &str = "cash_back";

/// Whether a billable line was invoiced yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BilledStatus {
    /// Billable and not invoiced yet.
    Unbilled,
    /// Invoiced (reserved by the engine for later invoicing).
    Billed,
    /// A status this version does not know.
    #[serde(other)]
    Other,
}

/// A transaction with its lines (`GET /transactions/{id}`, create/update
/// and void answers). Public invoice tokens are never read (N8).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Transaction {
    /// Id.
    pub id: TransactionId,
    /// Kind.
    pub txn_type: TxnType,
    /// Number.
    #[serde(default)]
    pub txn_number: Option<String>,
    /// Check number (checks and printed bill payments).
    #[serde(default)]
    pub check_number: Option<u64>,
    /// Date.
    pub txn_date: CalendarDate,
    /// Status.
    pub status: TxnStatus,
    /// Report basis.
    #[serde(default)]
    pub basis: Basis,
    /// Header contact.
    #[serde(default)]
    pub contact_id: Option<ContactId>,
    /// Header contact name (joined; display only).
    #[serde(default)]
    pub contact_name: Option<String>,
    /// Memo.
    #[serde(default)]
    pub memo: Option<String>,
    /// Document total (null for journal entries).
    #[serde(default)]
    pub total: Option<Money>,
    /// Why it was voided.
    #[serde(default)]
    pub void_reason: Option<String>,
    /// Created.
    #[serde(default)]
    pub created_at: Option<Timestamp>,
    /// Last changed.
    #[serde(default)]
    pub updated_at: Option<Timestamp>,
    /// Lines in `lineOrder` order.
    #[serde(default)]
    pub lines: Vec<TransactionLine>,
    /// An adjusting entry (`je-adjusting-v1`; `false` from older engines).
    #[serde(default)]
    pub is_adjusting: bool,
    /// On a reversing entry: the entry it reverses (`je-reversing-v1`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reverses_transaction_id: Option<TransactionId>,
    /// On a reversed entry: its live reversal (`je-reversing-v1`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reversal_transaction_id: Option<TransactionId>,
    /// On a reversed entry: the live reversal's date (`je-reversing-v1`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reverse_on: Option<CalendarDate>,
}

impl EditableFields for Transaction {
    const EDITABLE: &'static [&'static str] = &[
        "txnType",
        "txnDate",
        "memo",
        "basis",
        "status",
        "contactId",
        "lines.accountId",
        "lines.debit",
        "lines.credit",
        "lines.description",
        "lines.tagId",
        "lines.contactId",
        "lines.isBillable",
        "lines.lineOrder",
        "lines.sourceTransactionId",
        // Journal entries only, each hashed only when set (P15).
        "txnNumber",
        "isAdjusting",
        "reverseOn",
    ];

    fn editable(&self) -> Value {
        let mut lines: Vec<&TransactionLine> = self.lines.iter().collect();
        lines.sort_by_key(|l| l.line_order.unwrap_or(0));
        let lines = lines
            .into_iter()
            .map(|l| {
                with_source(
                    line_editable(
                        l.account_id,
                        &l.debit.to_wire(),
                        &l.credit.to_wire(),
                        l.description.as_deref(),
                        l.tag_id,
                        l.contact_id,
                        l.is_billable,
                        l.line_order.unwrap_or(0),
                    ),
                    l.source_transaction_id,
                )
            })
            .collect::<Vec<_>>();
        let mut v = txn_editable(
            self.txn_type,
            self.txn_date,
            self.memo.as_deref(),
            self.basis,
            self.status,
            self.contact_id,
            &lines,
        );
        if self.txn_type == TxnType::JournalEntry {
            je_editable(
                &mut v,
                self.txn_number.as_deref(),
                self.is_adjusting,
                self.reverse_on,
            );
        }
        v
    }
}

/// A row of the transaction list (no lines).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransactionSummary {
    /// Id.
    pub id: TransactionId,
    /// Kind.
    pub txn_type: TxnType,
    /// Number.
    #[serde(default)]
    pub txn_number: Option<String>,
    /// Date.
    pub txn_date: CalendarDate,
    /// Status.
    pub status: TxnStatus,
    /// Contact.
    #[serde(default)]
    pub contact_id: Option<ContactId>,
    /// Contact name (display only).
    #[serde(default)]
    pub contact_name: Option<String>,
    /// Memo.
    #[serde(default)]
    pub memo: Option<String>,
    /// Document total.
    #[serde(default)]
    pub total: Option<Money>,
    /// Engine-computed amount column (the total, or the sum of debits).
    #[serde(default)]
    pub display_total: Option<Money>,
    /// Created.
    #[serde(default)]
    pub created_at: Option<Timestamp>,
    /// Check number of a check (P13 recent checks). `None` when the engine
    /// list does not carry it yet (plan E5) or the row is not a check.
    #[serde(default)]
    pub check_number: Option<u64>,
}

/// Basis lens of the transaction list.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ReportBasis {
    /// Cash basis (`both` and `cash` entries).
    Cash,
    /// Accrual basis (`both` and `accrual` entries).
    Accrual,
}

/// Sort keys of the transaction list.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[allow(missing_docs, reason = "column names of the transaction list")]
pub enum TxnSort {
    Date,
    Type,
    Number,
    Payee,
    Memo,
    Category,
    Amount,
    Status,
}

impl TxnSort {
    const fn as_str(self) -> &'static str {
        match self {
            TxnSort::Date => "date",
            TxnSort::Type => "type",
            TxnSort::Number => "number",
            TxnSort::Payee => "payee",
            TxnSort::Memo => "memo",
            TxnSort::Category => "category",
            TxnSort::Amount => "amount",
            TxnSort::Status => "status",
        }
    }
}

/// Filters of the transaction list (camelCase on the wire).
#[derive(Clone, Default, PartialEq, Eq)]
#[allow(missing_docs, reason = "engine filter names")]
pub struct TransactionFilter {
    pub txn_type: Option<TxnType>,
    pub status: Option<TxnStatus>,
    pub contact_id: Option<ContactId>,
    pub account_id: Option<AccountId>,
    pub tag_id: Option<TagId>,
    pub basis: Option<ReportBasis>,
    pub start_date: Option<CalendarDate>,
    pub end_date: Option<CalendarDate>,
    /// Memo, number, payee or amount contains (owner data: never logged).
    pub search: Option<String>,
    pub sort: Option<(TxnSort, SortDir)>,
    /// Only transactions whose total is exactly this (A04, engine U-95,
    /// `document-drafts-attach-v1`): compared as SQL `numeric` (`10.1` =
    /// `10.10`, never `10.01`); records without a total (journal entries,
    /// transfers) never match. Sent as a decimal string; without the
    /// capability [`Transactions::list`] refuses it (an older engine would
    /// drop the filter and answer every transaction). Owner data: never
    /// logged.
    pub amount: Option<Money>,
}

/// Hides the search text and the amount (owner data).
impl std::fmt::Debug for TransactionFilter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TransactionFilter")
            .field("txn_type", &self.txn_type)
            .field("status", &self.status)
            .field("contact_id", &self.contact_id)
            .field("account_id", &self.account_id)
            .field("tag_id", &self.tag_id)
            .field("basis", &self.basis)
            .field("start_date", &self.start_date)
            .field("end_date", &self.end_date)
            .field("has_search", &self.search.is_some())
            .field("sort", &self.sort)
            .field("has_amount", &self.amount.is_some())
            .finish()
    }
}

impl TransactionFilter {
    /// Query pairs.
    #[must_use]
    pub fn to_query(&self) -> Vec<(String, String)> {
        self.query().0
    }

    fn query(&self) -> Query {
        let mut q = Query::default();
        if let Some(t) = self.txn_type {
            q.put("txnType", t.as_str());
        }
        if let Some(s) = self.status {
            q.put(
                "status",
                match s {
                    TxnStatus::Draft => "draft",
                    TxnStatus::Posted => "posted",
                    TxnStatus::Void => "void",
                    TxnStatus::Deleted => "deleted",
                },
            );
        }
        let id = |k: &str, v: Option<String>, q: &mut Query| {
            if let Some(v) = v {
                q.put(k, v);
            }
        };
        id("contactId", self.contact_id.map(|v| v.to_string()), &mut q);
        id("accountId", self.account_id.map(|v| v.to_string()), &mut q);
        id("tagId", self.tag_id.map(|v| v.to_string()), &mut q);
        if let Some(b) = self.basis {
            q.put(
                "basis",
                match b {
                    ReportBasis::Cash => "cash",
                    ReportBasis::Accrual => "accrual",
                },
            );
        }
        id(
            "startDate",
            self.start_date.map(CalendarDate::to_wire),
            &mut q,
        );
        id("endDate", self.end_date.map(CalendarDate::to_wire), &mut q);
        q.opt_str("search", self.search.as_deref());
        id("amount", self.amount.map(Money::to_wire), &mut q);
        q.sort(self.sort.map(|(s, d)| (s.as_str(), d)));
        q
    }
}

/// Transaction operations of a session (`session.transactions()`).
#[derive(Clone, Copy, Debug)]
pub struct Transactions<'a>(pub(crate) &'a Session);

impl Session {
    /// Transactions of the open company.
    #[must_use]
    pub fn transactions(&self) -> Transactions<'_> {
        Transactions(self)
    }
}

impl Transactions<'_> {
    /// One page of transactions.
    ///
    /// # Errors
    /// `Local(Unsupported)` for an `amount` filter without
    /// `document-drafts-attach-v1` (nothing sent); any [`ApiError`].
    pub async fn list(
        &self,
        filter: &TransactionFilter,
        page: PageRequest,
    ) -> Result<Page<TransactionSummary>, ApiError> {
        if filter.amount.is_some()
            && !self
                .0
                .client()
                .capabilities()
                .has(Capabilities::DOCUMENT_DRAFTS_ATTACH)
        {
            return Err(ApiError::Local(LocalValidation::Unsupported));
        }
        common::list_page(self.0, &SPEC, filter.query(), page).await
    }

    /// One transaction with its lines and version.
    ///
    /// # Errors
    /// `NotFound`; any [`ApiError`].
    pub async fn get(&self, id: TransactionId) -> Result<Versioned<Transaction>, ApiError> {
        common::get(self.0, &SPEC, &id.to_string()).await
    }

    /// Posts a journal entry.
    ///
    /// # Errors
    /// `ClosedPeriod`, `Validation`, `Local(ClosedPeriodOverrideUnavailable)`
    /// (nothing sent), `OutcomeUnknown` when reconciliation cannot tell, or
    /// any [`ApiError`].
    pub async fn create_journal_entry(
        &self,
        entry: &NewJournalEntry,
        opts: &PostingOptions,
        key: IdempotencyKey,
    ) -> Result<Versioned<Transaction>, ApiError> {
        // One company for the stamp read, the probe and the write.
        let s = &self.0.pinned();
        entry.require(&s.client().capabilities())?;
        let body = posting_body(s, entry.to_value(), opts)?;
        let matcher = OutcomeMatcher::Created {
            search: Search::Txn {
                kind: TxnType::JournalEntry,
                date: entry.txn_date,
                account: None,
            },
            fp: row_fp::<Transaction>,
            fingerprint: entry.fingerprint(),
        };
        let probe = Probe::new(s, matcher, true).await;
        common::create(s, &SPEC, body, key, Some(&probe)).await
    }

    /// Replaces a journal entry if it is still at `expected` (§2.5). The
    /// engine checks the closing date for both the old and the new date.
    ///
    /// # Errors
    /// `Conflict { Stale }` (nothing sent), `ClosedPeriod`, or as
    /// [`Transactions::create_journal_entry`].
    pub async fn update_journal_entry(
        &self,
        id: TransactionId,
        expected: &RecordVersion,
        entry: &NewJournalEntry,
        opts: &PostingOptions,
        key: IdempotencyKey,
    ) -> Result<Versioned<Transaction>, ApiError> {
        let caps = self.0.client().capabilities();
        entry.require(&caps)?;
        let body = posting_body(self.0, entry.to_update_value(&caps), opts)?;
        let u = GuardedUpdate {
            get: &TXN_GET,
            put: &TXN_PUT,
            path: SPEC.path(&id.to_string()),
            key: SPEC.item_key,
            body,
            idempotency_key: key,
            requested: entry.fingerprint(),
            fp: row_fp::<Transaction>,
            stamp_covered: true,
        };
        update_guarded(self.0, expected, u).await
    }

    /// Voids a transaction. A reason is required.
    ///
    /// # Errors
    /// `Local(MissingReason)` for a blank reason (nothing sent),
    /// `ClosedPeriod`, `Validation` (already void), or any [`ApiError`].
    pub async fn void(
        &self,
        id: TransactionId,
        reason: &str,
        opts: &PostingOptions,
        key: IdempotencyKey,
    ) -> Result<Versioned<Transaction>, ApiError> {
        // One company for the stamp read, the probe and the write.
        let s = &self.0.pinned();
        let reason = reason.trim();
        if reason.is_empty() {
            return Err(ApiError::Local(LocalValidation::MissingReason));
        }
        let body = posting_body(s, json!({ "reason": reason }), opts)?;
        let path = format!("{}/{id}/void", SPEC.base);
        let matcher = OutcomeMatcher::Field {
            get: &TXN_GET,
            path: SPEC.path(&id.to_string()),
            key: SPEC.item_key,
            field: "status",
            value: Value::String("void".into()),
        };
        let probe = Probe::new(s, matcher, true).await;
        let mut call = Call::new(Method::Post, TXN_VOID.route(), path).json_body(body);
        call.idempotency_key = Some(key);
        let reply = s.send(&TXN_VOID, call, Some(&probe)).await?;
        saved_from(&reply.body, SPEC.item_key)
    }

    /// Soft-deletes a transaction (R7, `DELETE /transactions/{id}`): the
    /// engine posts reversing lines, hides it from registers and reports,
    /// and keeps it, with all its lines, in the audit trail. Returns the
    /// deleted transaction (`status` = deleted).
    ///
    /// # Errors
    /// Nothing is sent for: `Local(TxnDeleteUnavailable)` without
    /// `txn-delete-v1`; `Local(ClosedPeriodOverrideUnavailable)` for a
    /// closing password without `closing-password-v1`; `Local(InvalidValue)`
    /// for a reason over 500 characters. From the engine: `Conflict` with
    /// `Reconciled`, `AlreadyDeleted`, `DeleteUnsupported` (Q-245: void it
    /// instead) or `DeleteLinked`; `ClosedPeriod`; `ClosingPasswordRejected`;
    /// `ClosingPasswordRateLimited`; `NotFound`; `OutcomeUnknown`; any
    /// [`ApiError`].
    pub async fn delete(
        &self,
        id: TransactionId,
        opts: &DeleteOptions,
        key: IdempotencyKey,
    ) -> Result<Versioned<Transaction>, ApiError> {
        // One company for the stamp read, the probe and the write.
        let s = &self.0.pinned();
        let caps = s.client().capabilities();
        if !caps.has(crate::wire::Capabilities::TXN_DELETE) {
            return Err(ApiError::Local(LocalValidation::TxnDeleteUnavailable));
        }
        let reason = opts
            .reason
            .as_deref()
            .map(str::trim)
            .filter(|r| !r.is_empty());
        if reason.is_some_and(|r| r.chars().count() > DELETE_REASON_MAX_CHARS) {
            return Err(ApiError::Local(LocalValidation::InvalidValue));
        }
        let mut body = serde_json::Map::new();
        if let Some(r) = reason {
            body.insert("reason".into(), Value::String(r.to_string()));
        }
        let opts = PostingOptions {
            closed_period: opts
                .closing_password
                .clone()
                .map(crate::resources::posting::ClosedPeriodAuthorization::with_password),
        };
        let body = posting_body(s, Value::Object(body), &opts)?;
        let path = SPEC.path(&id.to_string());
        let matcher = OutcomeMatcher::Field {
            get: &TXN_GET,
            path: path.clone(),
            key: SPEC.item_key,
            field: "status",
            value: Value::String("deleted".into()),
        };
        let probe = Probe::new(s, matcher, true).await;
        let mut call = Call::new(Method::Delete, TXN_DELETE.route(), path).json_body(body);
        call.idempotency_key = Some(key);
        let reply = s.send(&TXN_DELETE, call, Some(&probe)).await?;
        saved_from(&reply.body, SPEC.item_key)
    }
}

/// Journal entry calls of a session (`session.journal()`).
#[derive(Clone, Copy, Debug)]
pub struct JournalEntries<'a>(pub(crate) &'a Session);

impl Session {
    /// Journal entries of the open company.
    #[must_use]
    pub fn journal(&self) -> JournalEntries<'_> {
        JournalEntries(self)
    }
}

impl JournalEntries<'_> {
    /// The suggested next entry number (`GET /journal-entries/next-number`,
    /// `je-number-v1`): the highest all-digit number + 1, `"1"` if none.
    /// Not reserved; the owner may change it.
    ///
    /// # Errors
    /// `Local(JeNumberUnavailable)` without `je-number-v1` (nothing sent);
    /// `Decode` when `next` is not a string; any [`ApiError`].
    pub async fn next_number(&self) -> Result<String, ApiError> {
        if !self.0.client().capabilities().has(Capabilities::JE_NUMBER) {
            return Err(ApiError::Local(LocalValidation::JeNumberUnavailable));
        }
        let call = Call::at(Method::Get, JE_NEXT_NUMBER.route());
        let reply = self.0.send(&JE_NEXT_NUMBER, call, None).await?;
        next_number_from_json(&reply.body)
    }
}

/// Parses `{"next": "42"}` of `GET /journal-entries/next-number`.
///
/// # Errors
/// `Decode` when `next` is missing or not a string.
pub fn next_number_from_json(body: &[u8]) -> Result<String, ApiError> {
    const WHAT: &str = "next journal entry number";
    let v: Value = serde_json::from_slice(body).map_err(|_| ApiError::Decode { what: WHAT })?;
    v.get("next")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or(ApiError::Decode { what: WHAT })
}
