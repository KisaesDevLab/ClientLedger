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

use cl_core::ids::{AccountId, ContactId, JournalLineId, TagId, TransactionId};
use cl_core::{CalendarDate, Money, Timestamp};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::client::{Call, Method};
use crate::errors::{ApiError, LocalValidation};
use crate::idempotency::IdempotencyKey;
use crate::resources::common::{self, Query, SortDir, Spec};
use crate::resources::operations::{TXN_GET, TXN_PUT, TXN_VOID, TXNS_CREATE, TXNS_LIST};
use crate::resources::posting::{PostingOptions, posting_body};
use crate::resources::reconcile::{OutcomeMatcher, Probe, Search, row_fp};
use crate::resources::versioning::{
    EditableFields, GuardedUpdate, RecordVersion, Versioned, fingerprint_of, saved_from,
    update_guarded,
};
use crate::session::Session;
use crate::wire::{Page, PageRequest};

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
    fn as_str(self) -> String {
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
    fn amount(self) -> Money {
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
}

/// A balanced journal entry, checked before sending.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewJournalEntry {
    txn_date: CalendarDate,
    memo: Option<String>,
    basis: Basis,
    lines: Vec<JeLine>,
}

fn non_empty(s: Option<String>) -> Option<String> {
    s.filter(|v| !v.is_empty())
}

impl NewJournalEntry {
    /// Checks and builds an entry. Empty memo and descriptions count as
    /// none (the engine stores them as `null`).
    ///
    /// # Errors
    /// `TooFewLines` (< 2), `TooManyLines` (> 500), `NonPositiveAmount`,
    /// `Unbalanced` (debits ≠ credits exactly), `InvalidValue` when a total
    /// overflows the money range.
    pub fn new(
        txn_date: CalendarDate,
        memo: Option<String>,
        basis: Basis,
        lines: Vec<JeLine>,
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
        })
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
                None,
                i64::try_from(i).unwrap_or(i64::MAX),
            )
        });
        fingerprint_of(&txn_editable(
            TxnType::JournalEntry,
            self.txn_date,
            self.memo.as_deref(),
            self.basis,
            TxnStatus::Posted,
            None,
            &lines.collect::<Vec<_>>(),
        ))
    }
}

fn line_editable(
    account: AccountId,
    debit: &str,
    credit: &str,
    description: Option<&str>,
    tag: Option<TagId>,
    contact: Option<ContactId>,
    order: i64,
) -> Value {
    json!({
        "accountId": account,
        "debit": debit,
        "credit": credit,
        "description": description.filter(|d| !d.is_empty()),
        "tagId": tag,
        "contactId": contact,
        "lineOrder": order,
    })
}

fn txn_editable(
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
    /// Line contact.
    #[serde(default)]
    pub contact_id: Option<ContactId>,
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
        "lines.lineOrder",
    ];

    fn editable(&self) -> Value {
        let mut lines: Vec<&TransactionLine> = self.lines.iter().collect();
        lines.sort_by_key(|l| l.line_order.unwrap_or(0));
        let lines = lines
            .into_iter()
            .map(|l| {
                line_editable(
                    l.account_id,
                    &l.debit.to_wire(),
                    &l.credit.to_wire(),
                    l.description.as_deref(),
                    l.tag_id,
                    l.contact_id,
                    l.line_order.unwrap_or(0),
                )
            })
            .collect::<Vec<_>>();
        txn_editable(
            self.txn_type,
            self.txn_date,
            self.memo.as_deref(),
            self.basis,
            self.status,
            self.contact_id,
            &lines,
        )
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
#[derive(Clone, Debug, Default, PartialEq, Eq)]
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
    /// Any [`ApiError`].
    pub async fn list(
        &self,
        filter: &TransactionFilter,
        page: PageRequest,
    ) -> Result<Page<TransactionSummary>, ApiError> {
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
        let body = posting_body(s, entry.to_value(), opts)?;
        let matcher = OutcomeMatcher::Created {
            search: Search::JournalEntry {
                date: entry.txn_date,
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
        let body = posting_body(self.0, entry.to_value(), opts)?;
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
}
