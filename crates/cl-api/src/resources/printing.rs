//! Form printing (P19): the check print queue, recording check numbers,
//! requeue and reprint (U-106, U-107), the check style per bank account
//! (U-109), the company logo (U-110), the form footers and document
//! numbering (U-111) and the forms Print later queue (U-113).
//!
//! `cl-print` lays out every page itself (Q-P19c); these calls only read
//! what to print and record that it printed. Amounts arrive as decimal
//! strings and are read into [`Money`]; never floating point.
//!
//! Recording comes first: `POST /checks/print` numbers the checks before
//! anything goes to the printer (B2). Requeue and reprint change a check's
//! number, so a check dated on or before the closing date takes the
//! closing-date password ([`PostingOptions`]; 423 without it, 403 when it is
//! wrong).

use std::collections::BTreeMap;
use std::fmt;

use cl_core::ids::{AccountId, TransactionId};
use cl_core::{CalendarDate, Money};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::client::{Call, Method};
use crate::errors::{ApiError, ErrorCode, LocalValidation};
// 409 on print: a number in the range is held by another check on the
// account (`details.checkNumber`); the same code as Pay Bills' U-103.
use crate::resources::bill_payments::CHECK_NUMBER_IN_USE;
use crate::resources::checks::{CheckSettings, check_settings_from_json};
use crate::resources::operations::{
    CHECK_SETTINGS, CHECK_SETTINGS_PUT, CHECKS_PRINT, CHECKS_PRINT_QUEUE, CHECKS_REPRINT,
    CHECKS_REQUEUE, CHECKS_SPOILED, COMPANY_LOGO, COMPANY_LOGO_UPLOAD, DOCUMENT_NUMBER_CLAIM,
    DOCUMENT_NUMBER_NEXT, DOCUMENT_NUMBER_PUT, DOCUMENT_NUMBERS, FORMS_MARK_PRINTED,
    FORMS_PRINT_QUEUE, INVOICE_TEMPLATE, INVOICE_TEMPLATE_PUT,
};
use crate::resources::posting::{PostingOptions, posting_body};
use crate::session::{Session, decode, json_body};
use crate::wire::Capabilities;

// ---- check style per bank account (U-109) ---------------------------------

/// The check stock of a bank account. `Standard` (three checks per page) is
/// this app's own; the engine stores it but does not draw it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CheckFormat {
    /// Three checks per page.
    Standard,
    /// Check on top, two stubs (the default of a new account, Q-P19q).
    Voucher,
    /// Z-fold self-mailer.
    ZFold,
    /// Stub, check, stub (engine layout; printed here as voucher).
    CheckMiddle,
}

impl CheckFormat {
    /// The wire value of the stored style.
    #[must_use]
    pub const fn as_wire(self) -> &'static str {
        match self {
            CheckFormat::Standard => "standard",
            CheckFormat::Voucher => "voucher",
            CheckFormat::ZFold => "z_fold",
            CheckFormat::CheckMiddle => "check_middle",
        }
    }

    /// Reads a stored style; anything unknown is the default, voucher.
    #[must_use]
    pub fn from_wire(s: &str) -> CheckFormat {
        match s {
            "standard" => CheckFormat::Standard,
            "z_fold" => CheckFormat::ZFold,
            "check_middle" => CheckFormat::CheckMiddle,
            _ => CheckFormat::Voucher,
        }
    }

    /// The `format` of `POST /checks/print`, which takes only the engine's
    /// own layouts: standard is recorded as `voucher` (it is audit text
    /// only; Q-P19am).
    #[must_use]
    pub const fn print_wire(self) -> &'static str {
        match self {
            CheckFormat::Standard | CheckFormat::Voucher => "voucher",
            CheckFormat::ZFold => "z_fold",
            CheckFormat::CheckMiddle => "check_middle",
        }
    }
}

/// A bank account's check style (`GET /checks/settings?bank_account_id=`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CheckLayout {
    /// The stock.
    pub format: CheckFormat,
    /// Print the company name, address and logo on the check (Q-P19k).
    pub print_company_block: bool,
    /// True when the account has its own entry (else the company fallback).
    pub own: bool,
}

impl Default for CheckLayout {
    fn default() -> Self {
        CheckLayout {
            format: CheckFormat::Voucher,
            print_company_block: false,
            own: false,
        }
    }
}

fn layout_from(v: &Value) -> Option<CheckLayout> {
    Some(CheckLayout {
        format: CheckFormat::from_wire(v.get("format")?.as_str()?),
        print_company_block: v
            .get("printCompanyBlock")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        own: v.get("source").and_then(Value::as_str) == Some("bank_account"),
    })
}

// ---- the check print queue (U-107) ----------------------------------------

/// What kind of check waits to print.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum QueuedCheckKind {
    /// A written check.
    Check,
    /// A bill payment (its stub lists the bills paid).
    BillPayment,
}

/// One check waiting to print (`GET /checks/print-queue`).
#[derive(Clone, PartialEq, Eq)]
pub struct QueuedCheck {
    /// The check.
    pub id: TransactionId,
    /// Date.
    pub date: CalendarDate,
    /// Amount (decimal string on the wire).
    pub amount: Money,
    /// The name printed on the check (else the payee's name).
    pub payee: String,
    /// The memo printed on the check (else the transaction memo).
    pub memo: String,
    /// Written check or bill payment.
    pub kind: QueuedCheckKind,
    /// The bank account it is drawn on (`check-reprint-audit-v1`).
    pub bank_account: Option<AccountId>,
    /// Mailing rows (street / street 2 / `City, ST ZIP`); empty when none.
    pub address: Vec<String>,
}

/// Ids and dates only (no names, amounts or memos).
impl fmt::Debug for QueuedCheck {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("QueuedCheck")
            .field("id", &self.id)
            .field("date", &self.date)
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct QueueRow {
    id: TransactionId,
    txn_date: String,
    amount: Money,
    #[serde(default)]
    memo: Option<String>,
    #[serde(default)]
    payee_name_on_check: Option<String>,
    #[serde(default)]
    printed_memo: Option<String>,
    #[serde(default)]
    contact_name: Option<String>,
    #[serde(default)]
    txn_type: Option<String>,
    #[serde(default)]
    bank_account_id: Option<AccountId>,
    #[serde(default)]
    payee_address: Vec<String>,
}

fn non_blank(s: Option<String>) -> Option<String> {
    s.filter(|v| !v.trim().is_empty())
}

/// The date part of a wire date (`2026-11-04` or `2026-11-04T00:00:00Z`).
fn wire_date(s: &str) -> Option<CalendarDate> {
    CalendarDate::parse_wire(s.get(..10).unwrap_or(s)).ok()
}

/// Parses `{data: [...]}` of `GET /checks/print-queue`.
///
/// # Errors
/// `Decode` when the body or a row can't be read.
pub fn print_queue_from_json(body: &[u8]) -> Result<Vec<QueuedCheck>, ApiError> {
    #[derive(Deserialize)]
    struct W {
        data: Vec<QueueRow>,
    }
    let bad = || ApiError::Decode {
        what: "check print queue",
    };
    let w: W = serde_json::from_slice(body).map_err(|_| bad())?;
    w.data
        .into_iter()
        .map(|r| {
            Ok(QueuedCheck {
                id: r.id,
                date: wire_date(&r.txn_date).ok_or_else(bad)?,
                amount: r.amount,
                payee: non_blank(r.payee_name_on_check)
                    .or(non_blank(r.contact_name))
                    .unwrap_or_default(),
                memo: non_blank(r.printed_memo)
                    .or(non_blank(r.memo))
                    .unwrap_or_default(),
                kind: if r.txn_type.as_deref() == Some("bill_payment") {
                    QueuedCheckKind::BillPayment
                } else {
                    QueuedCheckKind::Check
                },
                bank_account: r.bank_account_id,
                address: r
                    .payee_address
                    .into_iter()
                    .filter(|l| !l.trim().is_empty())
                    .collect(),
            })
        })
        .collect()
}

// ---- print, requeue, reprint, spoiled (U-106) ------------------------------

/// `POST /checks/print`: number `checks` in order from `starting_number` on
/// `bank_account`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrintChecks {
    /// The bank account whose checkbook the numbers come from.
    pub bank_account: AccountId,
    /// Checks in print order.
    pub checks: Vec<TransactionId>,
    /// The first number.
    pub starting_number: u64,
    /// The stock (recorded in the audit trail).
    pub format: CheckFormat,
}

/// The engine recorded a print run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrintedBatch {
    /// The batch (for a whole-batch reprint).
    pub batch_id: Uuid,
    /// How many checks it numbered.
    pub checks_printed: u32,
}

/// 400 on print: a check is drawn on another bank account.
pub const CHECK_NOT_ON_BANK_ACCOUNT: &str = "CHECK_NOT_ON_BANK_ACCOUNT";
/// 400 on requeue: a hand-written check (never printed here).
pub const CHECK_NOT_PRINTED: &str = "CHECK_NOT_PRINTED";
/// 400 on print (U-115): the check is void.
pub const CHECK_VOID: &str = "CHECK_VOID";
/// 400 on requeue or reprint: the check's bank account can't be found.
pub const CHECK_BANK_ACCOUNT_UNKNOWN: &str = "CHECK_BANK_ACCOUNT_UNKNOWN";

/// Why the engine refused a print, requeue or reprint, for the screen's
/// plain-language message (`None` for closed periods, passwords, network
/// and anything else, which the shared handling covers).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PrintProblem {
    /// A number in the range is already used on the bank account: pick
    /// another first number.
    NumberInUse,
    /// The queue changed (a check was printed, voided or moved elsewhere):
    /// reload it.
    QueueChanged,
    /// A hand-written check can't be reprinted.
    NotPrinted,
    /// A check in the run was voided (`CHECK_VOID`, U-115): it is no longer
    /// in the queue and can't be printed.
    Voided,
}

/// Classifies a refusal of the print calls.
#[must_use]
pub fn print_problem(e: &ApiError) -> Option<PrintProblem> {
    let code = match e {
        ApiError::Conflict { code: Some(c), .. } => c.as_str(),
        // Older engines' duplicate check number is a code-less 409.
        ApiError::Conflict { .. } => return Some(PrintProblem::NumberInUse),
        ApiError::Validation(v) => v.code().map_or("", ErrorCode::as_str),
        ApiError::NotFound { .. } => return Some(PrintProblem::QueueChanged),
        _ => return None,
    };
    match code {
        CHECK_NUMBER_IN_USE => Some(PrintProblem::NumberInUse),
        CHECK_NOT_PRINTED => Some(PrintProblem::NotPrinted),
        CHECK_VOID => Some(PrintProblem::Voided),
        CHECK_NOT_ON_BANK_ACCOUNT | CHECK_BANK_ACCOUNT_UNKNOWN | "" => {
            Some(PrintProblem::QueueChanged)
        }
        _ => None,
    }
}

/// A check sent back to the queue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequeuedCheck {
    /// The check.
    pub id: TransactionId,
    /// The number it carried, now spoiled and logged (reusable); `None`
    /// when it had none.
    pub spoiled_number: Option<u64>,
    /// Its bank account.
    pub bank_account: Option<AccountId>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RequeuedRow {
    id: TransactionId,
    #[serde(default)]
    spoiled_check_number: Option<u64>,
    #[serde(default)]
    bank_account_id: Option<AccountId>,
}

/// What a requeue or batch reprint did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Requeued {
    /// The checks sent back to the queue.
    pub checks: Vec<RequeuedCheck>,
    /// Checks the engine left alone because they are void or deleted (U-117;
    /// empty from engines before it, which refuse the whole set instead).
    pub skipped: Vec<TransactionId>,
}

fn requeued_from(reply: &crate::client::EngineReply) -> Result<Requeued, ApiError> {
    #[derive(Deserialize)]
    struct Skipped {
        id: TransactionId,
    }
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum SkippedRow {
        Id(TransactionId),
        Row(Skipped),
    }
    #[derive(Deserialize)]
    struct W {
        #[serde(default)]
        checks: Vec<RequeuedRow>,
        #[serde(default)]
        skipped: Vec<SkippedRow>,
    }
    let w: W = decode(reply, "requeued checks")?;
    Ok(Requeued {
        checks: w
            .checks
            .into_iter()
            .map(|r| RequeuedCheck {
                id: r.id,
                spoiled_number: r.spoiled_check_number,
                bank_account: r.bank_account_id,
            })
            .collect(),
        skipped: w
            .skipped
            .into_iter()
            .map(|s| match s {
                SkippedRow::Id(id) => id,
                SkippedRow::Row(r) => r.id,
            })
            .collect(),
    })
}

/// Why a number was spoiled.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SpoiledReason {
    /// "Did all checks print correctly?" named it bad, or Reprint check.
    Requeue,
    /// A whole batch went back.
    Reprint,
    /// An edit of a printed check chose Reprint.
    EditReprint,
}

/// A spoiled check number (`GET /checks/spoiled`); it may be used again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpoiledNumber {
    /// The number.
    pub number: u64,
    /// The check that carried it.
    pub check: TransactionId,
    /// The bank account.
    pub bank_account: AccountId,
    /// Why.
    pub reason: SpoiledReason,
    /// The check's date.
    pub date: Option<CalendarDate>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SpoiledRow {
    check_number: u64,
    check_id: TransactionId,
    bank_account_id: AccountId,
    reason: String,
    #[serde(default)]
    txn_date: Option<String>,
}

// ---- footers (U-111) -------------------------------------------------------

/// A form type with its own footer and numbering (U-111).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DocType {
    /// Invoice.
    Invoice,
    /// Sales receipt (`cash_sale`).
    SalesReceipt,
    /// Credit memo.
    CreditMemo,
    /// Statement.
    Statement,
}

impl DocType {
    /// Every type, in the engine's order.
    pub const ALL: [DocType; 4] = [
        DocType::Invoice,
        DocType::SalesReceipt,
        DocType::CreditMemo,
        DocType::Statement,
    ];

    /// The wire value.
    #[must_use]
    pub const fn as_wire(self) -> &'static str {
        match self {
            DocType::Invoice => "invoice",
            DocType::SalesReceipt => "cash_sale",
            DocType::CreditMemo => "credit_memo",
            DocType::Statement => "statement",
        }
    }

    /// Reads a wire value.
    #[must_use]
    pub fn from_wire(s: &str) -> Option<DocType> {
        DocType::ALL.into_iter().find(|t| t.as_wire() == s)
    }
}

/// The company's footer text and the per-type overrides
/// (`GET /company/invoice-template`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FormFooters {
    /// The company footer (`None` when not set: the engine default prints).
    pub footer_text: Option<String>,
    /// Per-type overrides; a blank or missing one uses the company footer.
    pub overrides: BTreeMap<DocType, String>,
    /// The footer each type prints, as the engine resolved it.
    pub resolved: BTreeMap<DocType, String>,
}

impl FormFooters {
    /// The footer `kind` prints.
    #[must_use]
    pub fn resolved_for(&self, kind: DocType) -> &str {
        self.resolved.get(&kind).map_or("", String::as_str)
    }
}

fn footers_from(reply: &crate::client::EngineReply) -> Result<FormFooters, ApiError> {
    let v: Value = decode(reply, "form footers")?;
    let template = v.get("template").filter(|t| t.is_object());
    let text_map = |m: Option<&Value>| -> BTreeMap<DocType, String> {
        m.and_then(Value::as_object)
            .map(|o| {
                o.iter()
                    .filter_map(|(k, v)| Some((DocType::from_wire(k)?, v.as_str()?.to_string())))
                    .collect()
            })
            .unwrap_or_default()
    };
    Ok(FormFooters {
        footer_text: template
            .and_then(|t| t.get("footerText"))
            .and_then(Value::as_str)
            .map(str::to_string),
        overrides: text_map(template.and_then(|t| t.get("footerOverrides")))
            .into_iter()
            .filter(|(_, v)| !v.trim().is_empty())
            .collect(),
        resolved: text_map(v.get("resolvedFooters")),
    })
}

/// Longest footer the engine keeps.
pub const MAX_FOOTER_CHARS: usize = 1000;

// ---- document numbers (U-111) ----------------------------------------------

/// A type's prefix and next number.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocumentNumber {
    /// The type.
    pub kind: DocType,
    /// Prefix (may be empty).
    pub prefix: String,
    /// The next number.
    pub next_number: u64,
    /// `prefix` + `next_number`: what the next document gets.
    pub next: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NumberRow {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    prefix: String,
    next_number: u64,
    #[serde(default)]
    next: String,
}

fn number_from(r: NumberRow) -> Option<DocumentNumber> {
    Some(DocumentNumber {
        kind: DocType::from_wire(&r.kind)?,
        next: if r.next.is_empty() {
            format!("{}{}", r.prefix, r.next_number)
        } else {
            r.next
        },
        prefix: r.prefix,
        next_number: r.next_number,
    })
}

/// A number taken with `POST /document-numbers/claim`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClaimedNumber {
    /// The type.
    pub kind: DocType,
    /// The integer taken.
    pub number: u64,
    /// Prefix + number.
    pub txn_number: String,
}

/// A duplicate document number the engine let through with `warnOnly`
/// (on form creates, U-111). Absent on an idempotent replay.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DocumentNumberWarning {
    /// `DOCUMENT_NUMBER_IN_USE`.
    pub code: String,
    /// The number.
    pub txn_number: String,
    /// The document already holding it.
    pub transaction_id: Option<TransactionId>,
}

/// Reads `warnings` from a form create reply; none when absent (an
/// idempotent replay leaves it out).
#[must_use]
pub fn number_warnings(body: &Value) -> Vec<DocumentNumberWarning> {
    body.get("warnings")
        .and_then(|w| serde_json::from_value(w.clone()).ok())
        .unwrap_or_default()
}

/// What `PUT /document-numbers/{type}` changes (at least one).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DocumentNumberUpdate {
    /// New prefix (at most 20 characters, no spaces).
    pub prefix: Option<String>,
    /// New next number (1 to 999,999,999).
    pub next_number: Option<u64>,
}

/// Longest prefix.
pub const MAX_PREFIX_CHARS: usize = 20;
/// Largest next number.
pub const MAX_NEXT_NUMBER: u64 = 999_999_999;

impl DocumentNumberUpdate {
    fn body(&self) -> Result<Value, ApiError> {
        let invalid = || ApiError::Local(LocalValidation::InvalidValue);
        let mut map = Map::new();
        if let Some(p) = &self.prefix {
            if p.chars().count() > MAX_PREFIX_CHARS || p.chars().any(char::is_whitespace) {
                return Err(invalid());
            }
            map.insert("prefix".into(), Value::String(p.clone()));
        }
        if let Some(n) = self.next_number {
            if !(1..=MAX_NEXT_NUMBER).contains(&n) {
                return Err(invalid());
            }
            map.insert("nextNumber".into(), json!(n));
        }
        if map.is_empty() {
            return Err(invalid());
        }
        Ok(Value::Object(map))
    }
}

// ---- forms print queue (U-113) ---------------------------------------------

/// A form marked Print later (`GET /forms/print-queue`).
#[derive(Clone, PartialEq, Eq)]
pub struct QueuedForm {
    /// The form.
    pub id: TransactionId,
    /// Invoice, sales receipt or credit memo.
    pub kind: DocType,
    /// Its number.
    pub number: Option<String>,
    /// Date.
    pub date: CalendarDate,
    /// The customer.
    pub customer: Option<String>,
    /// Total (decimal string on the wire).
    pub total: Option<Money>,
}

/// Ids, types and dates only.
impl fmt::Debug for QueuedForm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("QueuedForm")
            .field("id", &self.id)
            .field("kind", &self.kind)
            .field("date", &self.date)
            .finish_non_exhaustive()
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FormRow {
    id: TransactionId,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    txn_number: Option<String>,
    txn_date: String,
    #[serde(default)]
    contact_name: Option<String>,
    #[serde(default)]
    total: Option<Money>,
}

/// `POST /forms/mark-printed` answer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MarkedPrinted {
    /// Forms whose mark was cleared now.
    pub marked: u32,
    /// Forms that were already cleared.
    pub already_printed: u32,
}

/// Most ids one mark-printed call takes.
pub const MAX_MARK_PRINTED: usize = 500;

// ---- logo (U-110) ----------------------------------------------------------

/// Largest logo the engine takes (it refuses more than 5 MB; this app
/// prints up to 2 MB, `cl_print::image::MAX_LOGO_BYTES`).
pub const MAX_LOGO_UPLOAD_BYTES: usize = 5 * 1024 * 1024;

fn logo_mime(bytes: &[u8]) -> Option<(&'static str, &'static str)> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(("image/png", "png"))
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(("image/jpeg", "jpg"))
    } else {
        None
    }
}

fn logo_multipart(bytes: &[u8], mime: &str, ext: &str) -> (String, Vec<u8>) {
    let boundary = loop {
        let b = format!("cl-{}", Uuid::new_v4().simple());
        if !bytes.windows(b.len()).any(|w| w == b.as_bytes()) {
            break b;
        }
    };
    let mut body = Vec::with_capacity(bytes.len() + 256);
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"logo\"; filename=\"logo.{ext}\"\r\nContent-Type: {mime}\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    (boundary, body)
}

// ---- the calls -------------------------------------------------------------

/// Printing calls of a session (`session.printing()`).
#[derive(Clone, Copy, Debug)]
pub struct Printing<'a>(pub(crate) &'a Session);

impl Session {
    /// Check and form printing of the open company.
    #[must_use]
    pub fn printing(&self) -> Printing<'_> {
        Printing(self)
    }
}

fn require(s: &Session, cap: &str) -> Result<(), ApiError> {
    if s.client().capabilities().has(cap) {
        Ok(())
    } else {
        Err(ApiError::Local(LocalValidation::Unsupported))
    }
}

impl Printing<'_> {
    /// Checks waiting to print, of `bank` (all when `None`).
    ///
    /// # Errors
    /// `Decode`; any [`ApiError`].
    pub async fn check_queue(&self, bank: Option<AccountId>) -> Result<Vec<QueuedCheck>, ApiError> {
        let mut call = Call::at(Method::Get, CHECKS_PRINT_QUEUE.route());
        if let Some(b) = bank {
            call.query.push(("bank_account_id".into(), b.to_string()));
        }
        let reply = self.0.send(&CHECKS_PRINT_QUEUE, call, None).await?;
        print_queue_from_json(&reply.body)
    }

    /// The next check numbers and `bank`'s check style.
    ///
    /// # Errors
    /// `Decode`; any [`ApiError`].
    pub async fn check_layout(
        &self,
        bank: AccountId,
    ) -> Result<(CheckSettings, CheckLayout), ApiError> {
        let mut call = Call::at(Method::Get, CHECK_SETTINGS.route());
        call.query
            .push(("bank_account_id".into(), bank.to_string()));
        let reply = self.0.send(&CHECK_SETTINGS, call, None).await?;
        let settings = check_settings_from_json(&reply.body)?;
        let v: Value = serde_json::from_slice(&reply.body).map_err(|_| ApiError::Decode {
            what: "check settings",
        })?;
        let layout = v.get("layout").and_then(layout_from).unwrap_or_default();
        Ok((settings, layout))
    }

    /// Saves `bank`'s check style. Only `bank`'s entry of `layouts` is sent
    /// (PR-2 S3, U-117): the engine merges the keys it is sent under a lock,
    /// so there is no read-merge-write here that could race another save or
    /// a print run's counter.
    ///
    /// # Errors
    /// `Local(Unsupported)` without `check-reprint-audit-v1` (nothing
    /// sent); `Validation` (not a bank account of this company); any
    /// [`ApiError`].
    pub async fn set_check_layout(
        &self,
        bank: AccountId,
        layout: CheckLayout,
    ) -> Result<(), ApiError> {
        require(self.0, Capabilities::CHECK_REPRINT_AUDIT)?;
        let mut layouts = serde_json::Map::new();
        layouts.insert(
            bank.to_string(),
            json!({
                "format": layout.format.as_wire(),
                "printCompanyBlock": layout.print_company_block,
            }),
        );
        let body = json!({ "layouts": layouts });
        let call = Call::at(Method::Put, CHECK_SETTINGS_PUT.route()).json_body(json_body(&body)?);
        self.0.send(&CHECK_SETTINGS_PUT, call, None).await.map(drop)
    }

    /// Records the numbers of a print run (before anything is printed).
    ///
    /// # Errors
    /// `Local(InvalidValue)` for no checks or a zero first number (nothing
    /// sent); `Conflict` code `CHECK_NUMBER_IN_USE` (a number in the range
    /// is used: [`print_problem`]); `NotFound` or `Validation` (the queue
    /// changed); `ClosedPeriod`, `ClosingPasswordRejected`; any
    /// [`ApiError`].
    pub async fn print_checks(
        &self,
        req: &PrintChecks,
        opts: &PostingOptions,
    ) -> Result<PrintedBatch, ApiError> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct W {
            batch_id: String,
            #[serde(default)]
            checks_printed: u32,
        }
        if req.checks.is_empty() || req.starting_number == 0 {
            return Err(ApiError::Local(LocalValidation::InvalidValue));
        }
        let ids: Vec<String> = req.checks.iter().map(ToString::to_string).collect();
        let value = json!({
            "bankAccountId": req.bank_account.to_string(),
            "checkIds": ids,
            "startingCheckNumber": req.starting_number,
            "format": req.format.print_wire(),
        });
        let body = posting_body(self.0, value, opts)?;
        let call = Call::at(Method::Post, CHECKS_PRINT.route()).json_body(body);
        let reply = self.0.send(&CHECKS_PRINT, call, None).await?;
        let w: W = decode(&reply, "printed checks")?;
        let batch_id = Uuid::parse_str(&w.batch_id).map_err(|_| ApiError::Decode {
            what: "printed checks",
        })?;
        Ok(PrintedBatch {
            batch_id,
            checks_printed: w.checks_printed,
        })
    }

    /// Sends printed checks back to the queue: their numbers are cleared,
    /// logged as spoiled (reusable) and audited; the payments stay.
    ///
    /// # Errors
    /// `Local(InvalidValue)` for no checks; `Validation` code
    /// `CHECK_NOT_PRINTED` (hand-written) or `CHECK_VOID`; `NotFound`;
    /// `ClosedPeriod` (423: ask for the closing-date password),
    /// `ClosingPasswordRejected` (403); any [`ApiError`].
    pub async fn requeue(
        &self,
        checks: &[TransactionId],
        opts: &PostingOptions,
    ) -> Result<Requeued, ApiError> {
        if checks.is_empty() {
            return Err(ApiError::Local(LocalValidation::InvalidValue));
        }
        let ids: Vec<String> = checks.iter().map(ToString::to_string).collect();
        let body = posting_body(self.0, json!({ "checkIds": ids }), opts)?;
        let call = Call::at(Method::Post, CHECKS_REQUEUE.route()).json_body(body);
        let reply = self.0.send(&CHECKS_REQUEUE, call, None).await?;
        requeued_from(&reply)
    }

    /// Sends a whole printed batch back to the queue.
    ///
    /// # Errors
    /// As [`Printing::requeue`]; `NotFound` when the batch has no check in
    /// this company or was already sent back.
    pub async fn reprint_batch(
        &self,
        batch: Uuid,
        opts: &PostingOptions,
    ) -> Result<Requeued, ApiError> {
        let body = posting_body(self.0, json!({}), opts)?;
        let call = Call::new(
            Method::Post,
            CHECKS_REPRINT.route(),
            CHECKS_REPRINT
                .route()
                .replace("{batchId}", &batch.to_string()),
        )
        .json_body(body);
        let reply = self.0.send(&CHECKS_REPRINT, call, None).await?;
        requeued_from(&reply)
    }

    /// Spoiled check numbers of `bank` (all when `None`).
    ///
    /// # Errors
    /// `Local(Unsupported)` without `check-reprint-audit-v1`; any
    /// [`ApiError`].
    pub async fn spoiled(&self, bank: Option<AccountId>) -> Result<Vec<SpoiledNumber>, ApiError> {
        #[derive(Deserialize)]
        struct W {
            data: Vec<SpoiledRow>,
        }
        require(self.0, Capabilities::CHECK_REPRINT_AUDIT)?;
        let mut call = Call::at(Method::Get, CHECKS_SPOILED.route());
        if let Some(b) = bank {
            call.query.push(("bank_account_id".into(), b.to_string()));
        }
        let reply = self.0.send(&CHECKS_SPOILED, call, None).await?;
        let w: W = decode(&reply, "spoiled check numbers")?;
        Ok(w.data
            .into_iter()
            .map(|r| SpoiledNumber {
                number: r.check_number,
                check: r.check_id,
                bank_account: r.bank_account_id,
                reason: match r.reason.as_str() {
                    "reprint" => SpoiledReason::Reprint,
                    "edit_reprint" => SpoiledReason::EditReprint,
                    _ => SpoiledReason::Requeue,
                },
                date: r.txn_date.as_deref().and_then(wire_date),
            })
            .collect())
    }

    /// The company logo's bytes; `None` when there is none.
    ///
    /// # Errors
    /// `Local(Unsupported)` without `company-logo-v1`; any [`ApiError`].
    pub async fn logo(&self) -> Result<Option<Vec<u8>>, ApiError> {
        require(self.0, Capabilities::COMPANY_LOGO)?;
        let call = Call::at(Method::Get, COMPANY_LOGO.route());
        match self.0.send(&COMPANY_LOGO, call, None).await {
            Ok(reply) => Ok(Some(reply.body.to_vec())),
            Err(ApiError::NotFound { .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Uploads a PNG or JPEG as the company logo.
    ///
    /// # Errors
    /// `Local(InvalidValue)` for an empty file, one over 5 MB or not a PNG
    /// or JPEG (nothing sent); `Validation`; any [`ApiError`].
    pub async fn upload_logo(&self, bytes: &[u8]) -> Result<(), ApiError> {
        let (mime, ext) = logo_mime(bytes)
            .filter(|_| bytes.len() <= MAX_LOGO_UPLOAD_BYTES)
            .ok_or(ApiError::Local(LocalValidation::InvalidValue))?;
        let (boundary, body) = logo_multipart(bytes, mime, ext);
        let call =
            Call::at(Method::Post, COMPANY_LOGO_UPLOAD.route()).multipart_body(&boundary, body);
        self.0
            .send(&COMPANY_LOGO_UPLOAD, call, None)
            .await
            .map(drop)
    }

    /// The company footer, the per-type overrides and what each type prints.
    ///
    /// # Errors
    /// `Decode`; any [`ApiError`].
    pub async fn footers(&self) -> Result<FormFooters, ApiError> {
        let call = Call::at(Method::Get, INVOICE_TEMPLATE.route());
        let reply = self.0.send(&INVOICE_TEMPLATE, call, None).await?;
        footers_from(&reply)
    }

    /// Saves the company footer and every override (a blank override uses
    /// the company footer; the overrides map is replaced whole).
    ///
    /// # Errors
    /// `Local(Unsupported)` without `document-numbers-v1`;
    /// `Local(InvalidValue)` for text over 1,000 characters (nothing sent);
    /// any [`ApiError`].
    pub async fn set_footers(
        &self,
        footer_text: &str,
        overrides: &BTreeMap<DocType, String>,
    ) -> Result<FormFooters, ApiError> {
        require(self.0, Capabilities::DOCUMENT_NUMBERS)?;
        let too_long = |s: &str| s.chars().count() > MAX_FOOTER_CHARS;
        if too_long(footer_text) || overrides.values().any(|v| too_long(v)) {
            return Err(ApiError::Local(LocalValidation::InvalidValue));
        }
        let mut map = Map::new();
        for t in DocType::ALL {
            let v = overrides
                .get(&t)
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .map_or(Value::Null, |s| Value::String(s.to_string()));
            map.insert(t.as_wire().into(), v);
        }
        let footer = footer_text.trim();
        let body = json!({
            "footerText": if footer.is_empty() { Value::Null } else { Value::String(footer.to_string()) },
            "footerOverrides": map,
        });
        let call = Call::at(Method::Put, INVOICE_TEMPLATE_PUT.route()).json_body(json_body(&body)?);
        let reply = self.0.send(&INVOICE_TEMPLATE_PUT, call, None).await?;
        footers_from(&reply)
    }

    /// Prefix and next number of every type.
    ///
    /// # Errors
    /// `Local(Unsupported)` without `document-numbers-v1`; any
    /// [`ApiError`].
    pub async fn document_numbers(&self) -> Result<Vec<DocumentNumber>, ApiError> {
        #[derive(Deserialize)]
        struct W {
            numbers: Vec<NumberRow>,
        }
        require(self.0, Capabilities::DOCUMENT_NUMBERS)?;
        let call = Call::at(Method::Get, DOCUMENT_NUMBERS.route());
        let reply = self.0.send(&DOCUMENT_NUMBERS, call, None).await?;
        let w: W = decode(&reply, "document numbers")?;
        Ok(w.numbers.into_iter().filter_map(number_from).collect())
    }

    /// What the next document of `kind` would get (nothing taken).
    ///
    /// # Errors
    /// As [`Printing::document_numbers`].
    pub async fn next_number(&self, kind: DocType) -> Result<DocumentNumber, ApiError> {
        require(self.0, Capabilities::DOCUMENT_NUMBERS)?;
        let mut call = Call::at(Method::Get, DOCUMENT_NUMBER_NEXT.route());
        call.query.push(("type".into(), kind.as_wire().into()));
        let reply = self.0.send(&DOCUMENT_NUMBER_NEXT, call, None).await?;
        let row: NumberRow = decode(&reply, "document number")?;
        number_from(row).ok_or(ApiError::Decode {
            what: "document number",
        })
    }

    /// Sets `kind`'s prefix and/or next number.
    ///
    /// # Errors
    /// `Local(InvalidValue)` for nothing to change, a prefix over 20
    /// characters or with spaces, or a next number outside 1 to 999,999,999
    /// (nothing sent); as [`Printing::document_numbers`].
    pub async fn set_document_number(
        &self,
        kind: DocType,
        update: &DocumentNumberUpdate,
    ) -> Result<DocumentNumber, ApiError> {
        require(self.0, Capabilities::DOCUMENT_NUMBERS)?;
        let body = update.body()?;
        let call = Call::new(
            Method::Put,
            DOCUMENT_NUMBER_PUT.route(),
            DOCUMENT_NUMBER_PUT
                .route()
                .replace("{type}", kind.as_wire()),
        )
        .json_body(json_body(&body)?);
        let reply = self.0.send(&DOCUMENT_NUMBER_PUT, call, None).await?;
        let row: NumberRow = decode(&reply, "document number")?;
        number_from(row).ok_or(ApiError::Decode {
            what: "document number",
        })
    }

    /// Takes the next number of `kind` (statements, which have no create).
    ///
    /// # Errors
    /// As [`Printing::document_numbers`].
    pub async fn claim_number(&self, kind: DocType) -> Result<ClaimedNumber, ApiError> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct W {
            number: u64,
            txn_number: String,
        }
        require(self.0, Capabilities::DOCUMENT_NUMBERS)?;
        let body = json!({ "type": kind.as_wire() });
        let call =
            Call::at(Method::Post, DOCUMENT_NUMBER_CLAIM.route()).json_body(json_body(&body)?);
        let reply = self.0.send(&DOCUMENT_NUMBER_CLAIM, call, None).await?;
        let w: W = decode(&reply, "claimed number")?;
        Ok(ClaimedNumber {
            kind,
            number: w.number,
            txn_number: w.txn_number,
        })
    }

    /// Forms marked Print later, of `kind` (all when `None`).
    ///
    /// # Errors
    /// `Local(Unsupported)` without `forms-print-queue-v1`;
    /// `Local(InvalidValue)` for statements; any [`ApiError`].
    pub async fn form_queue(&self, kind: Option<DocType>) -> Result<Vec<QueuedForm>, ApiError> {
        #[derive(Deserialize)]
        struct W {
            data: Vec<FormRow>,
        }
        require(self.0, Capabilities::FORMS_PRINT_QUEUE)?;
        let mut call = Call::at(Method::Get, FORMS_PRINT_QUEUE.route());
        if let Some(k) = kind {
            if k == DocType::Statement {
                return Err(ApiError::Local(LocalValidation::InvalidValue));
            }
            call.query.push(("type".into(), k.as_wire().into()));
        }
        let reply = self.0.send(&FORMS_PRINT_QUEUE, call, None).await?;
        let w: W = decode(&reply, "forms print queue")?;
        let bad = || ApiError::Decode {
            what: "forms print queue",
        };
        w.data
            .into_iter()
            .map(|r| {
                Ok(QueuedForm {
                    id: r.id,
                    kind: DocType::from_wire(&r.kind).ok_or_else(bad)?,
                    number: non_blank(r.txn_number),
                    date: wire_date(&r.txn_date).ok_or_else(bad)?,
                    customer: non_blank(r.contact_name),
                    total: r.total,
                })
            })
            .collect()
    }

    /// Clears the Print later mark of printed forms.
    ///
    /// # Errors
    /// `Local(InvalidValue)` for none or more than 500 ids (nothing sent);
    /// `NotFound` (an id is not a form of this company; nothing changed);
    /// as [`Printing::form_queue`].
    pub async fn mark_forms_printed(
        &self,
        ids: &[TransactionId],
    ) -> Result<MarkedPrinted, ApiError> {
        require(self.0, Capabilities::FORMS_PRINT_QUEUE)?;
        if ids.is_empty() || ids.len() > MAX_MARK_PRINTED {
            return Err(ApiError::Local(LocalValidation::InvalidValue));
        }
        let ids: Vec<String> = ids.iter().map(ToString::to_string).collect();
        let body = json!({ "ids": ids });
        let call = Call::at(Method::Post, FORMS_MARK_PRINTED.route()).json_body(json_body(&body)?);
        let reply = self.0.send(&FORMS_MARK_PRINTED, call, None).await?;
        decode(&reply, "marked printed")
    }
}

#[cfg(test)]
#[path = "printing_tests.rs"]
mod tests;
