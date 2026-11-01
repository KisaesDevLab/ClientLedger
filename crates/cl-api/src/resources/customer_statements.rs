//! Customer statements (P22, engine U-112 amended and U-140; contract per
//! the P22 plan §2.6, verified at the D8 pin bump).
//!
//! - The engine computes the statement: lines, running balance, aging
//!   (the same call as the A/R aging summary, Q-177) and amount due. The
//!   UI shows the strings.
//! - A run issues numbered snapshots (Q-P22n): display data only, no ids
//!   inside. Print, PDF, email and the Outbox render the snapshot, so a
//!   reprint is identical.
//! - Names, addresses and emails are owner data: `Debug` shows ids, dates,
//!   counts and the format only.

use std::fmt;
use std::str::FromStr;

use base64::Engine as _;
use cl_core::ids::{ContactId, IdError, TransactionId, parse_uuid};
use cl_core::{CalendarDate, Money, Timestamp};
use serde::de::Deserializer;
use serde::{Deserialize, Serialize, Serializer};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::client::{Call, Method};
use crate::errors::{ApiError, LocalValidation};
use crate::idempotency::IdempotencyKey;
use crate::resources::attachments::uuid_id;
use crate::resources::customer_payments::{
    de_cents, de_cents_opt, de_stored, error_code, null_false, null_vec, require,
};
use crate::resources::invoices::{
    EMAIL_PDF_MAX_BYTES, EmailMessage, OutboxId, OutboxItem, SendEvent, SendRecord,
};
use crate::resources::operations::{
    REPORT_CUSTOMER_STATEMENT, REPORT_CUSTOMER_STATEMENTS, STATEMENT_EMAIL, STATEMENT_EMAIL_LATER,
    STATEMENT_GET, STATEMENT_RUN_CREATE, STATEMENT_SENT, STATEMENTS_LIST,
};
use crate::session::{Session, decode, json_body};
use crate::wire::{Capabilities, Page, PageRequest};

/// Clients one run may take (400 `TOO_MANY_CONTACTS` above it).
pub const MAX_RUN_CONTACTS: usize = 500;
/// Clients the desktop puts in one run (P22 plan §5: chunks of 200).
pub const RUN_CHUNK: usize = 200;

uuid_id!(
    /// One issued customer statement (U-140).
    CustomerStatementId
);

uuid_id!(
    /// One statement run (U-140).
    StatementRunId
);

/// `StatementRefusal`: the engine's refusals of statement runs and sends.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StatementRefusal {
    /// More than 500 clients in one run (400 `TOO_MANY_CONTACTS`).
    TooManyContacts,
    /// No SMTP server set up (400 `SMTP_NOT_CONFIGURED`).
    SmtpNotConfigured,
    /// The SMTP server refused the message (400 `SMTP_SEND_FAILED`).
    SmtpSendFailed,
    /// The Outbox message was sent or removed meanwhile (404
    /// `OUTBOX_ITEM_NOT_FOUND`).
    OutboxItemGone,
    /// The PDF is over 10 MB (400 `ATTACHMENT_TOO_LARGE`).
    AttachmentTooLarge,
}

const STATEMENT_REFUSALS: &[(&str, StatementRefusal)] = &[
    ("TOO_MANY_CONTACTS", StatementRefusal::TooManyContacts),
    (
        crate::resources::invoices::SMTP_NOT_CONFIGURED,
        StatementRefusal::SmtpNotConfigured,
    ),
    (
        crate::resources::invoices::SMTP_SEND_FAILED,
        StatementRefusal::SmtpSendFailed,
    ),
    ("OUTBOX_ITEM_NOT_FOUND", StatementRefusal::OutboxItemGone),
    ("ATTACHMENT_TOO_LARGE", StatementRefusal::AttachmentTooLarge),
];

/// Every engine code [`statement_refusal`] knows.
#[must_use]
pub fn statement_refusal_codes() -> Vec<&'static str> {
    STATEMENT_REFUSALS.iter().map(|(c, _)| *c).collect()
}

/// Classifies `e` (`None` for anything else).
#[must_use]
pub fn statement_refusal(e: &ApiError) -> Option<StatementRefusal> {
    let code = error_code(e)?;
    STATEMENT_REFUSALS
        .iter()
        .find(|(c, _)| *c == code)
        .map(|(_, r)| *r)
}

/// The statement layouts (Q-175, Q-P22h).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatementFormat {
    /// Each open item (the default, Q-175).
    #[default]
    OpenItem,
    /// The opening balance, then the period's activity.
    BalanceForward,
    /// The period's activity with the previous balance in the header.
    Transaction,
}

impl StatementFormat {
    /// The wire name.
    #[must_use]
    pub const fn as_wire(self) -> &'static str {
        match self {
            StatementFormat::OpenItem => "open_item",
            StatementFormat::BalanceForward => "balance_forward",
            StatementFormat::Transaction => "transaction",
        }
    }
}

/// Aging buckets as exact strings (2 places for a client's statement; the
/// exact sums, possibly with legacy sub-cent residue, in the summary).
#[derive(Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct AgingBuckets {
    /// Not yet due (and credits dated in the last 0 days).
    #[serde(deserialize_with = "de_stored")]
    pub current: Money,
    /// 1-30 days.
    #[serde(rename = "days1to30", deserialize_with = "de_stored")]
    pub days_1_30: Money,
    /// 31-60 days.
    #[serde(rename = "days31to60", deserialize_with = "de_stored")]
    pub days_31_60: Money,
    /// 61-90 days.
    #[serde(rename = "days61to90", deserialize_with = "de_stored")]
    pub days_61_90: Money,
    /// Over 90 days.
    #[serde(rename = "over90", deserialize_with = "de_stored")]
    pub over_90: Money,
}

impl fmt::Debug for AgingBuckets {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AgingBuckets(..)")
    }
}

/// A statement's aging row (Q-177: equal to the client's A/R aging row).
#[derive(Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct StatementAging {
    /// The buckets.
    #[serde(flatten)]
    pub buckets: AgingBuckets,
    /// Their total.
    #[serde(deserialize_with = "de_stored")]
    pub total: Money,
}

impl fmt::Debug for StatementAging {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("StatementAging(..)")
    }
}

/// Who the statement is for (owner data).
#[derive(Clone, PartialEq, Eq, Deserialize)]
pub struct StatementContact {
    /// The client (absent inside a snapshot: snapshots hold no ids).
    #[serde(default)]
    pub id: Option<ContactId>,
    /// Name.
    pub name: String,
    /// Billing address lines.
    #[serde(default, deserialize_with = "null_vec")]
    pub address: Vec<String>,
    /// Email.
    #[serde(default)]
    pub email: Option<String>,
}

impl fmt::Debug for StatementContact {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StatementContact")
            .field("id", &self.id)
            .field("address", &self.address.len())
            .field("email", &self.email.is_some())
            .finish_non_exhaustive()
    }
}

/// One statement line.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatementLine {
    /// Its date.
    pub date: CalendarDate,
    /// The transaction (absent inside a snapshot).
    #[serde(default)]
    pub txn_id: Option<TransactionId>,
    /// `invoice`, `customer_payment`, `credit_memo`, ...
    #[serde(default)]
    pub txn_type: String,
    /// The document number (owner data).
    #[serde(default)]
    pub number: Option<String>,
    /// Description (owner data).
    #[serde(default)]
    pub description: Option<String>,
    /// Due date (invoices).
    #[serde(default)]
    pub due_date: Option<CalendarDate>,
    /// Its amount (credits negative).
    #[serde(deserialize_with = "de_cents")]
    pub amount: Money,
    /// What is open of it (open-item format).
    #[serde(default, deserialize_with = "de_cents_opt")]
    pub open: Option<Money>,
    /// The running balance.
    #[serde(deserialize_with = "de_stored")]
    pub balance: Money,
}

impl fmt::Debug for StatementLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StatementLine")
            .field("date", &self.date)
            .field("txn_id", &self.txn_id)
            .field("txn_type", &self.txn_type)
            .field("due_date", &self.due_date)
            .finish_non_exhaustive()
    }
}

/// A customer statement (`GET /reports/customer-statement`, and the
/// snapshot of an issued one).
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomerStatement {
    /// The client.
    pub contact: StatementContact,
    /// The statement date.
    pub as_of: CalendarDate,
    /// The layout.
    pub format: StatementFormat,
    /// The period start (balance forward and transaction).
    #[serde(default)]
    pub start: Option<CalendarDate>,
    /// The balance before the period.
    #[serde(default, deserialize_with = "de_stored_opt")]
    pub opening_balance: Option<Money>,
    /// The lines.
    #[serde(default, deserialize_with = "null_vec")]
    pub lines: Vec<StatementLine>,
    /// The aging row.
    pub aging: StatementAging,
    /// What the client owes (= aging total).
    #[serde(deserialize_with = "de_stored")]
    pub amount_due: Money,
    /// The exact total less the sum of cent figures: 0.00 except with
    /// legacy sub-cent residue (a warning on screen, never printed).
    #[serde(default, deserialize_with = "de_stored_zero")]
    pub difference: Money,
}

fn de_stored_opt<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Money>, D::Error> {
    match Option::<String>::deserialize(d)? {
        Some(s) => Money::parse_wire(&s)
            .map(Some)
            .map_err(|_| serde::de::Error::custom("money")),
        None => Ok(None),
    }
}

fn de_stored_zero<'de, D: Deserializer<'de>>(d: D) -> Result<Money, D::Error> {
    Ok(de_stored_opt(d)?.unwrap_or(Money::ZERO))
}

impl fmt::Debug for CustomerStatement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CustomerStatement")
            .field("contact", &self.contact)
            .field("as_of", &self.as_of)
            .field("format", &self.format)
            .field("start", &self.start)
            .field("lines", &self.lines.len())
            .finish_non_exhaustive()
    }
}

impl CustomerStatement {
    /// True when the exact total and the cent figures differ (legacy
    /// sub-cent residue): the screen shows a plain warning.
    #[must_use]
    pub fn has_difference(&self) -> bool {
        !self.difference.is_zero()
    }
}

/// `GET /reports/customer-statement` query.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StatementQuery {
    /// The client.
    pub contact: ContactId,
    /// The statement date.
    pub as_of: CalendarDate,
    /// The layout.
    pub format: StatementFormat,
    /// The period start (`None`: the first of the as-of month).
    pub start: Option<CalendarDate>,
}

/// The last statement issued to a client.
#[derive(Clone, PartialEq, Eq, Deserialize)]
pub struct LastStatement {
    /// Its date.
    pub date: CalendarDate,
    /// Its number.
    #[serde(default)]
    pub number: Option<String>,
}

impl fmt::Debug for LastStatement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LastStatement")
            .field("date", &self.date)
            .finish_non_exhaustive()
    }
}

/// One row of the batch picker (`GET /reports/customer-statements`).
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatementListRow {
    /// The client.
    pub contact_id: ContactId,
    /// Name (owner data).
    #[serde(default)]
    pub name: String,
    /// What the client owes as of the date.
    #[serde(deserialize_with = "de_stored")]
    pub amount_due: Money,
    /// Days the oldest item is past due (`None`: nothing overdue).
    #[serde(default)]
    pub oldest_days_overdue: Option<u32>,
    /// The client has an email address.
    #[serde(default, deserialize_with = "null_false")]
    pub has_email: bool,
    /// The last statement issued.
    #[serde(default)]
    pub last_statement: Option<LastStatement>,
}

impl fmt::Debug for StatementListRow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StatementListRow")
            .field("contact_id", &self.contact_id)
            .field("oldest_days_overdue", &self.oldest_days_overdue)
            .field("has_email", &self.has_email)
            .finish_non_exhaustive()
    }
}

/// `GET /reports/customer-statements` filters (Q-176).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StatementListQuery {
    /// The statement date.
    pub as_of: CalendarDate,
    /// Only clients with something at least this many days overdue.
    pub min_days_overdue: u32,
    /// Only clients whose amount due is not 0.00 (on by default).
    pub open_only: bool,
}

/// `POST /customer-statement-runs`.
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewStatementRun {
    /// The statement date.
    pub as_of: CalendarDate,
    /// The layout.
    pub format: StatementFormat,
    /// The period start.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start: Option<CalendarDate>,
    /// The clients (1-500).
    pub contact_ids: Vec<ContactId>,
}

impl fmt::Debug for NewStatementRun {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NewStatementRun")
            .field("as_of", &self.as_of)
            .field("format", &self.format)
            .field("start", &self.start)
            .field("contacts", &self.contact_ids.len())
            .finish()
    }
}

/// One statement a run issued.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IssuedStatement {
    /// The statement.
    pub statement_id: CustomerStatementId,
    /// The client.
    pub contact_id: ContactId,
    /// Its number.
    pub number: String,
    /// Its amount due.
    #[serde(deserialize_with = "de_stored")]
    pub amount_due: Money,
}

impl fmt::Debug for IssuedStatement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IssuedStatement")
            .field("statement_id", &self.statement_id)
            .field("contact_id", &self.contact_id)
            .finish_non_exhaustive()
    }
}

/// What a run answered.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatementRun {
    /// The run.
    pub run_id: StatementRunId,
    /// One per client.
    pub statements: Vec<IssuedStatement>,
}

impl fmt::Debug for StatementRun {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StatementRun")
            .field("run_id", &self.run_id)
            .field("statements", &self.statements.len())
            .finish()
    }
}

/// An issued statement (`GET /customer-statements/{id}`).
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IssuedStatementRecord {
    /// The statement.
    pub id: CustomerStatementId,
    /// Its number.
    pub number: String,
    /// The client.
    pub contact_id: ContactId,
    /// The statement date.
    pub as_of: CalendarDate,
    /// Its amount due.
    #[serde(deserialize_with = "de_stored")]
    pub amount_due: Money,
    /// What was issued (display data only).
    pub snapshot: CustomerStatement,
    /// How it went out.
    #[serde(default, deserialize_with = "null_vec")]
    pub send_history: Vec<SendEvent>,
    /// Issued.
    #[serde(default)]
    pub created_at: Option<Timestamp>,
}

impl fmt::Debug for IssuedStatementRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IssuedStatementRecord")
            .field("id", &self.id)
            .field("contact_id", &self.contact_id)
            .field("as_of", &self.as_of)
            .field("snapshot", &self.snapshot)
            .field("send_history", &self.send_history)
            .finish_non_exhaustive()
    }
}

/// One row of a client's statement history (newest first).
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatementHistoryRow {
    /// The statement.
    pub id: CustomerStatementId,
    /// Its number.
    pub number: String,
    /// The statement date.
    pub as_of: CalendarDate,
    /// Its amount due.
    #[serde(deserialize_with = "de_stored")]
    pub amount_due: Money,
    /// Issued.
    #[serde(default)]
    pub created_at: Option<Timestamp>,
    /// How it went out.
    #[serde(default, deserialize_with = "null_vec")]
    pub send_history: Vec<SendEvent>,
}

impl fmt::Debug for StatementHistoryRow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StatementHistoryRow")
            .field("id", &self.id)
            .field("as_of", &self.as_of)
            .field("send_history", &self.send_history)
            .finish_non_exhaustive()
    }
}

/// The PDF file name of statement `number` (`Statement-1007.pdf`).
#[must_use]
pub fn statement_pdf_name(number: &str) -> String {
    let safe: String = number
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        .collect();
    if safe.is_empty() {
        "Statement.pdf".to_string()
    } else {
        format!("Statement-{safe}.pdf")
    }
}

/// Checks a run before anything is sent.
///
/// # Errors
/// `Local(Unsupported)` without `customer-statement-send-v1`;
/// `Local(TooFewLines)` for no client; `Local(TooManyLines)` over 500.
pub fn check_run(r: &NewStatementRun, caps: &Capabilities) -> Result<(), ApiError> {
    if !caps.has(Capabilities::CUSTOMER_STATEMENT_SEND) {
        return Err(ApiError::Local(LocalValidation::Unsupported));
    }
    if r.contact_ids.is_empty() {
        return Err(ApiError::Local(LocalValidation::TooFewLines));
    }
    if r.contact_ids.len() > MAX_RUN_CONTACTS {
        return Err(ApiError::Local(LocalValidation::TooManyLines));
    }
    Ok(())
}

/// Customer statement operations of a session
/// (`session.customer_statements()`).
#[derive(Clone, Copy, Debug)]
pub struct CustomerStatements<'a>(pub(crate) &'a Session);

impl Session {
    /// Customer statements of the open company (P22).
    #[must_use]
    pub fn customer_statements(&self) -> CustomerStatements<'_> {
        CustomerStatements(self)
    }
}

impl CustomerStatements<'_> {
    /// One client's statement as of a date, computed now.
    ///
    /// # Errors
    /// `Local(Unsupported)` without `customer-statement-v1`; `NotFound`;
    /// `Decode`; any [`ApiError`].
    pub async fn statement(&self, q: &StatementQuery) -> Result<CustomerStatement, ApiError> {
        require(self.0, Capabilities::CUSTOMER_STATEMENT)?;
        let mut call = Call::at(Method::Get, REPORT_CUSTOMER_STATEMENT.route());
        call.query = vec![
            ("contactId".into(), q.contact.to_string()),
            ("asOf".into(), q.as_of.to_wire()),
            ("format".into(), q.format.as_wire().into()),
        ];
        if let Some(d) = q.start {
            call.query.push(("start".into(), d.to_wire()));
        }
        let reply = self.0.send(&REPORT_CUSTOMER_STATEMENT, call, None).await?;
        decode(&reply, "customer statement")
    }

    /// The batch picker: clients with their amount due, filtered.
    ///
    /// # Errors
    /// `Local(Unsupported)`; `Decode`; any [`ApiError`].
    pub async fn statement_list(
        &self,
        q: &StatementListQuery,
        page: PageRequest,
    ) -> Result<Page<StatementListRow>, ApiError> {
        require(self.0, Capabilities::CUSTOMER_STATEMENT)?;
        let mut call = Call::at(Method::Get, REPORT_CUSTOMER_STATEMENTS.route());
        call.query = vec![
            ("asOf".into(), q.as_of.to_wire()),
            ("minDaysOverdue".into(), q.min_days_overdue.to_string()),
            ("openOnly".into(), q.open_only.to_string()),
        ];
        call.query.extend(page.query());
        let reply = self.0.send(&REPORT_CUSTOMER_STATEMENTS, call, None).await?;
        Page::from_json(&reply.body, "data")
    }

    /// Issues numbered statement snapshots for up to 500 clients (keyed
    /// under `idempotency-customer-statements-v1`; a replay answers the
    /// same run). No GL effect, no closing check.
    ///
    /// # Errors
    /// Nothing is sent for `Local(…)` ([`check_run`]); `TOO_MANY_CONTACTS`;
    /// any [`ApiError`].
    pub async fn create_run(
        &self,
        r: &NewStatementRun,
        key: IdempotencyKey,
    ) -> Result<StatementRun, ApiError> {
        let s = &self.0.pinned();
        check_run(r, &s.client().capabilities())?;
        let mut call =
            Call::at(Method::Post, STATEMENT_RUN_CREATE.route()).json_body(json_body(r)?);
        call.idempotency_key = Some(key);
        let reply = s.send(&STATEMENT_RUN_CREATE, call, None).await?;
        decode(&reply, "statement run").map_err(crate::resources::versioning::saved_unreadable)
    }

    /// One issued statement with its snapshot.
    ///
    /// # Errors
    /// `Local(Unsupported)`; `NotFound`; `Decode`; any [`ApiError`].
    pub async fn get(&self, id: CustomerStatementId) -> Result<IssuedStatementRecord, ApiError> {
        require(self.0, Capabilities::CUSTOMER_STATEMENT_SEND)?;
        let call = Call::new(
            Method::Get,
            STATEMENT_GET.route(),
            format!("/api/v1/customer-statements/{id}"),
        );
        let reply = self.0.send(&STATEMENT_GET, call, None).await?;
        let v: Value = decode(&reply, "customer statement")?;
        let inner = v.get("statement").cloned().unwrap_or(v);
        serde_json::from_value(inner).map_err(|_| ApiError::Decode {
            what: "customer statement",
        })
    }

    /// A client's statements, newest first.
    ///
    /// # Errors
    /// `Local(Unsupported)`; `Decode`; any [`ApiError`].
    pub async fn history(&self, contact: ContactId) -> Result<Vec<StatementHistoryRow>, ApiError> {
        #[derive(Deserialize)]
        struct W {
            data: Vec<StatementHistoryRow>,
        }
        require(self.0, Capabilities::CUSTOMER_STATEMENT_SEND)?;
        let mut call = Call::at(Method::Get, STATEMENTS_LIST.route());
        call.query = vec![("contactId".into(), contact.to_string())];
        let reply = self.0.send(&STATEMENTS_LIST, call, None).await?;
        decode::<W>(&reply, "statement history").map(|w| w.data)
    }

    /// Records that a statement was printed, emailed or saved as a PDF
    /// (Mark as sent, Q-P22c; printing counts as sent).
    ///
    /// # Errors
    /// `Local(Unsupported)`; `Local(InvalidValue)` for over 50 recipients;
    /// any [`ApiError`].
    pub async fn mark_sent(&self, id: CustomerStatementId, r: &SendRecord) -> Result<(), ApiError> {
        require(self.0, Capabilities::CUSTOMER_STATEMENT_SEND)?;
        if r.recipients.len() > crate::resources::invoices::EMAIL_RECIPIENTS_MAX {
            return Err(ApiError::Local(LocalValidation::InvalidValue));
        }
        let mut body = json!({"method": r.method});
        if !r.recipients.is_empty() {
            body["recipients"] = json!(r.recipients);
        }
        if let Some(t) = r.sent_at {
            body["sentAt"] = json!(t.to_wire());
        }
        let call = Call::new(
            Method::Post,
            STATEMENT_SENT.route(),
            format!("/api/v1/customer-statements/{id}/sent"),
        )
        .json_body(json_body(&body)?);
        self.0.send(&STATEMENT_SENT, call, None).await?;
        Ok(())
    }

    /// Emails the statement with `pdf` attached and records an "email"
    /// send. Keep one key per send and reuse it on a retry. `outbox_id`
    /// (Send all) takes the Outbox message out in the same commit.
    ///
    /// # Errors
    /// `Local(Unsupported)`; `Local(InvalidValue)` for a bad address;
    /// `Local(AttachmentTooLarge)` for a PDF over 10 MB; from the engine
    /// see [`statement_refusal`]; any [`ApiError`].
    pub async fn email(
        &self,
        id: CustomerStatementId,
        message: &EmailMessage,
        pdf_file_name: &str,
        pdf: &[u8],
        outbox_id: Option<OutboxId>,
        key: IdempotencyKey,
    ) -> Result<(), ApiError> {
        require(self.0, Capabilities::CUSTOMER_STATEMENT_SEND)?;
        message.check()?;
        if pdf.is_empty() || pdf.len() > EMAIL_PDF_MAX_BYTES {
            return Err(ApiError::Local(LocalValidation::AttachmentTooLarge));
        }
        let mut m = message.json();
        m.insert(
            "pdfBase64".into(),
            Value::String(base64::engine::general_purpose::STANDARD.encode(pdf)),
        );
        m.insert(
            "pdfFileName".into(),
            Value::String(pdf_file_name.to_string()),
        );
        if let Some(o) = outbox_id {
            m.insert("outboxId".into(), Value::String(o.to_string()));
        }
        let mut call = Call::new(
            Method::Post,
            STATEMENT_EMAIL.route(),
            format!("/api/v1/customer-statements/{id}/email"),
        )
        .json_body(json_body(&Value::Object(m))?);
        call.idempotency_key = Some(key);
        self.0.send(&STATEMENT_EMAIL, call, None).await?;
        Ok(())
    }

    /// Puts the statement's email in the Outbox (sent on Send all).
    ///
    /// # Errors
    /// As [`CustomerStatements::email`] without the PDF checks.
    pub async fn email_later(
        &self,
        id: CustomerStatementId,
        message: &EmailMessage,
        key: IdempotencyKey,
    ) -> Result<OutboxItem, ApiError> {
        #[derive(Deserialize)]
        struct W {
            item: OutboxItem,
        }
        require(self.0, Capabilities::CUSTOMER_STATEMENT_SEND)?;
        message.check()?;
        let mut call = Call::new(
            Method::Post,
            STATEMENT_EMAIL_LATER.route(),
            format!("/api/v1/customer-statements/{id}/email-later"),
        )
        .json_body(json_body(&Value::Object(message.json()))?);
        call.idempotency_key = Some(key);
        let reply = self.0.send(&STATEMENT_EMAIL_LATER, call, None).await?;
        decode::<W>(&reply, "outbox item")
            .map(|w| w.item)
            .map_err(crate::resources::versioning::saved_unreadable)
    }
}
