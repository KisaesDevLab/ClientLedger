//! What bills and vendor credits share (P17 WP2): their lines, terms, the
//! saved record and its fingerprint, the request bodies, the capability
//! gates and the engine's refusal codes.
//!
//! - Every line amount is `Money` and goes out as `Money::to_wire()`; a
//!   reply decodes money only from strings.
//! - A field the pinned engine cannot store is refused before sending,
//!   `Local(Unsupported)`, with nothing sent: item, quantity, cost,
//!   customer and billable on a line, a tag on a vendor credit line and a
//!   draft attachment on a create (`bill-lines-v1`); discount terms and the
//!   terms codes the pin does not know (`bill-terms-v1`); a bill not yet
//!   received (`bill-received-v1`); a closing-date password
//!   (`ap-closing-v1`). `expectedUpdatedAt` is only a guard: it is sent
//!   with `ap-edit-v1` and left out otherwise.
//! - The fingerprint of a record covers what its form writes: vendor,
//!   date, reference, memo, terms, total, status and the document lines
//!   (the expense side; the A/P line is the engine's). The engine-computed
//!   due date, balances and payment state are left out (review Notes).
//! - `Debug` never prints names, references, memos or amounts.

use std::fmt;
use std::str::FromStr;

use cl_core::ids::{AccountId, ContactId, ItemId, TagId, TransactionId};
use cl_core::{CalendarDate, Money, Quantity};
use rust_decimal::Decimal;
use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize, Serializer};
use serde_json::{Value, json};

use crate::client::{Call, Method};
use crate::errors::{ApiError, LocalValidation};
use crate::idempotency::IdempotencyKey;
use crate::resources::attachments::DraftKey;
use crate::resources::common::remember_created;
use crate::resources::operations::Operation;
use crate::resources::posting::{PostingOptions, posting_body};
use crate::resources::reconcile::{OutcomeMatcher, Probe, Search, row_fp};
use crate::resources::transactions::{MAX_LINES, Transaction, TxnStatus, TxnType, non_empty};
use crate::resources::versioning::{
    EditableFields, GuardedUpdate, Versioned, fingerprint_of, get_versioned, saved_from,
    update_guarded_reply,
};
use crate::session::Session;
use crate::wire::Capabilities;

/// The longest vendor reference number the engine takes (UTF-16 code
/// units, as its schema counts).
pub const VENDOR_REF_MAX: usize = 100;

/// The longest custom net or discount period (days).
pub const TERMS_DAYS_MAX: u32 = 365;

/// The terms codes the pinned engine knows (`custom` takes `termsDays`).
/// Other codes need `bill-terms-v1`.
pub const PIN_TERMS_CODES: [&str; 8] = [
    "due_on_receipt",
    "net_10",
    "net_15",
    "net_30",
    "net_45",
    "net_60",
    "net_90",
    "custom",
];

/// The terms code for a custom number of days.
pub const TERMS_CUSTOM: &str = "custom";

/// Which A/P document.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ApKind {
    /// A bill: expense debits, Accounts Payable credited.
    Bill,
    /// A vendor credit: Accounts Payable debited, expense credits.
    VendorCredit,
}

impl ApKind {
    pub(crate) const fn txn_type(self) -> TxnType {
        match self {
            ApKind::Bill => TxnType::Bill,
            ApKind::VendorCredit => TxnType::VendorCredit,
        }
    }
}

/// A percentage such as a 2% early-payment discount (`"2"`, `"1.5"`):
/// more than 0 and less than 100, at most 4 decimals. Exact; never a float.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Percent(Decimal);

impl Percent {
    /// Strict parse: digits with an optional `.` and 1 to 4 decimals, more
    /// than 0 and less than 100.
    ///
    /// # Errors
    /// `InvalidValue` for anything else.
    pub fn parse(s: &str) -> Result<Percent, LocalValidation> {
        let (int, frac) = s.split_once('.').unwrap_or((s, ""));
        let digits = |p: &str| p.bytes().all(|b| b.is_ascii_digit());
        if int.is_empty()
            || int.len() > 2
            || !digits(int)
            || !digits(frac)
            || frac.len() > 4
            || (s.contains('.') && frac.is_empty())
        {
            return Err(LocalValidation::InvalidValue);
        }
        let d = Decimal::from_str(s).map_err(|_| LocalValidation::InvalidValue)?;
        if d <= Decimal::ZERO || d >= Decimal::ONE_HUNDRED {
            return Err(LocalValidation::InvalidValue);
        }
        Ok(Percent(d.normalize()))
    }

    /// The wire text, trailing zeros removed (`2`, `1.5`).
    #[must_use]
    pub fn to_wire(self) -> String {
        self.0.normalize().to_string()
    }

    /// The exact value.
    #[must_use]
    pub fn as_decimal(self) -> Decimal {
        self.0
    }
}

/// A discount term is owner data only in combination; the value is fine to
/// show in tests.
impl fmt::Debug for Percent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Percent({})", self.to_wire())
    }
}

impl Serialize for Percent {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_wire())
    }
}

/// A JSON string only (a number is refused, as for money).
impl<'de> Deserialize<'de> for Percent {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        // The engine may send numeric(Ã¢â‚¬Â¦,4) text with trailing zeros.
        let trimmed = if s.contains('.') {
            s.trim_end_matches('0').trim_end_matches('.')
        } else {
            s.as_str()
        };
        Percent::parse(trimmed).map_err(|_| de::Error::custom("percent"))
    }
}

/// Payment terms of a bill.
#[derive(Clone, PartialEq, Eq)]
pub struct BillTerms {
    /// The engine's code (`net_30`, `custom`, Ã¢â‚¬Â¦ ; `GET /payment-terms`
    /// lists them with `bill-terms-v1`).
    pub code: String,
    /// Net days, for `custom` only.
    pub days: Option<u32>,
    /// Early-payment discount (`bill-terms-v1`).
    pub discount_percent: Option<Percent>,
    /// Days the discount is open (`bill-terms-v1`; fewer than the net
    /// days).
    pub discount_days: Option<u32>,
}

impl BillTerms {
    /// Fixed terms by code (no custom days, no discount).
    #[must_use]
    pub fn code(code: &str) -> BillTerms {
        BillTerms {
            code: code.to_string(),
            days: None,
            discount_percent: None,
            discount_days: None,
        }
    }
}

impl fmt::Debug for BillTerms {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BillTerms")
            .field("code", &self.code)
            .field("days", &self.days)
            .field("discount_percent", &self.discount_percent)
            .field("discount_days", &self.discount_days)
            .finish()
    }
}

/// One line of a bill or vendor credit to save.
#[derive(Clone, PartialEq, Eq)]
pub struct BillLineDraft {
    /// The account (an expense account, or an item's account; D5).
    pub account: AccountId,
    /// The amount (greater than zero). For an item line with quantity and
    /// cost it must be `cost Ãƒâ€” quantity` rounded half away from zero to
    /// cents (D6).
    pub amount: Money,
    /// Line description (owner data: never logged). Blank takes the memo,
    /// as the engine does.
    pub description: Option<String>,
    /// Line tag (on a vendor credit only with `bill-lines-v1`).
    pub tag: Option<TagId>,
    /// The item (`bill-lines-v1`).
    pub item: Option<ItemId>,
    /// Quantity, greater than zero (`bill-lines-v1`).
    pub quantity: Option<Quantity>,
    /// Cost per unit, zero or more, up to 4 decimals; needs a quantity
    /// (`bill-lines-v1`).
    pub unit_price: Option<Money>,
    /// The customer (job) the line is for (`bill-lines-v1`).
    pub customer: Option<ContactId>,
    /// Billable to `customer` (`bill-lines-v1`; needs the customer).
    pub billable: bool,
}

impl BillLineDraft {
    /// A plain expense line.
    #[must_use]
    pub fn expense(account: AccountId, amount: Money) -> BillLineDraft {
        BillLineDraft {
            account,
            amount,
            description: None,
            tag: None,
            item: None,
            quantity: None,
            unit_price: None,
            customer: None,
            billable: false,
        }
    }

    fn uses_line_fields(&self) -> bool {
        self.item.is_some()
            || self.quantity.is_some()
            || self.unit_price.is_some()
            || self.customer.is_some()
            || self.billable
    }
}

/// Ids and flags only.
impl fmt::Debug for BillLineDraft {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BillLineDraft")
            .field("account", &self.account)
            .field("tag", &self.tag)
            .field("item", &self.item)
            .field("customer", &self.customer)
            .field("billable", &self.billable)
            .finish_non_exhaustive()
    }
}

/// The parts of a bill or credit that are checked and sent the same way.
pub(crate) struct DocParts<'a> {
    pub kind: ApKind,
    pub vendor: ContactId,
    pub date: CalendarDate,
    pub vendor_ref: Option<&'a str>,
    pub memo: Option<&'a str>,
    pub amount: Money,
    pub lines: &'a [BillLineDraft],
    pub terms: Option<&'a BillTerms>,
    pub due_date: Option<CalendarDate>,
    pub received: bool,
    pub draft: Option<DraftKey>,
}

/// A trimmed reference, `None` when blank.
pub(crate) fn clean_ref(s: Option<&str>) -> Option<String> {
    s.map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// Checks a document before anything is sent.
///
/// # Errors
/// `Local(Ã¢â‚¬Â¦)`: `Unsupported` (a field without its capability),
/// `TooFewLines`, `TooManyLines`, `NonPositiveAmount`, `TotalMismatch`
/// (lines Ã¢â€°Â  amount, exactly), `LineAmountMismatch` (an item line's cost Ãƒâ€”
/// quantity, or a cost without a quantity), `InvalidValue` (a reference
/// over 100 characters, a billable line without a customer, a quantity not
/// above zero, a negative cost, bad terms, an overflowing total).
pub(crate) fn check_doc(p: &DocParts<'_>, caps: &Capabilities) -> Result<(), ApiError> {
    let local = |v| Err(ApiError::Local(v));
    let lines_cap = caps.has(Capabilities::BILL_LINES);
    // What the engine takes at all comes first.
    let needs_lines = p.lines.iter().any(BillLineDraft::uses_line_fields)
        || p.draft.is_some()
        || (p.kind == ApKind::VendorCredit && p.lines.iter().any(|l| l.tag.is_some()));
    if needs_lines && !lines_cap {
        return local(LocalValidation::Unsupported);
    }
    if !p.received && !caps.has(Capabilities::BILL_RECEIVED) {
        return local(LocalValidation::Unsupported);
    }
    if let Some(t) = p.terms {
        let pin_code = PIN_TERMS_CODES.contains(&t.code.as_str());
        let discount = t.discount_percent.is_some() || t.discount_days.is_some();
        if (discount || !pin_code) && !caps.has(Capabilities::BILL_TERMS) {
            return local(LocalValidation::Unsupported);
        }
        let custom = t.code == TERMS_CUSTOM;
        let bad_days = match (custom, t.days) {
            (true, Some(d)) => d > TERMS_DAYS_MAX,
            (true, None) | (false, Some(_)) => true,
            (false, None) => false,
        };
        let bad_discount = match (t.discount_percent, t.discount_days) {
            (None, None) => false,
            (Some(_), Some(dd)) => dd > TERMS_DAYS_MAX || t.days.is_some_and(|n| dd >= n),
            _ => true,
        };
        if t.code.trim().is_empty() || bad_days || bad_discount {
            return local(LocalValidation::InvalidValue);
        }
    }
    if p.kind == ApKind::VendorCredit && (p.terms.is_some() || p.due_date.is_some() || !p.received)
    {
        return local(LocalValidation::InvalidValue);
    }
    if p.lines.is_empty() {
        return local(LocalValidation::TooFewLines);
    }
    if p.lines.len() > MAX_LINES {
        return local(LocalValidation::TooManyLines);
    }
    if p.lines.iter().any(|l| !l.amount.is_positive()) || !p.amount.is_positive() {
        return local(LocalValidation::NonPositiveAmount);
    }
    for l in p.lines {
        if l.billable && l.customer.is_none()
            || l.quantity.is_some_and(|q| !q.is_positive())
            || l.unit_price.is_some_and(Money::is_negative)
        {
            return local(LocalValidation::InvalidValue);
        }
        match (l.quantity, l.unit_price) {
            (None, Some(_)) => return local(LocalValidation::LineAmountMismatch),
            (Some(q), Some(rate)) => {
                let want = rate
                    .mul_round(q, 2)
                    .map_err(|_| ApiError::Local(LocalValidation::InvalidValue))?;
                if want != l.amount {
                    return local(LocalValidation::LineAmountMismatch);
                }
            }
            _ => {}
        }
    }
    let total = Money::sum(p.lines.iter().map(|l| l.amount))
        .map_err(|_| ApiError::Local(LocalValidation::InvalidValue))?;
    if total != p.amount {
        return local(LocalValidation::TotalMismatch);
    }
    if p.vendor_ref.is_some_and(|r| utf16_len(r) > VENDOR_REF_MAX) {
        return local(LocalValidation::InvalidValue);
    }
    Ok(())
}

/// The request body of a create (`update = false`) or an edit. An edit
/// always names the memo and the reference (empty clears them: the engine
/// keeps a missing memo as it was). Line tags always go out on a bill
/// (`null` for none) so the engine never fills the vendor's default tag.
pub(crate) fn doc_body(p: &DocParts<'_>, caps: &Capabilities, update: bool) -> Value {
    let lines_cap = caps.has(Capabilities::BILL_LINES);
    let lines: Vec<Value> = p
        .lines
        .iter()
        .map(|l| {
            let mut v = json!({
                "accountId": l.account,
                "amount": l.amount.to_wire(),
            });
            if let Some(d) = non_empty(l.description.clone()) {
                v["description"] = Value::String(d);
            }
            if p.kind == ApKind::Bill || lines_cap {
                v["tagId"] = json!(l.tag);
            }
            if let Some(i) = l.item {
                v["itemId"] = json!(i);
            }
            if let Some(q) = l.quantity {
                v["quantity"] = Value::String(q.to_wire());
            }
            if let Some(r) = l.unit_price {
                v["unitPrice"] = Value::String(r.to_wire());
            }
            if let Some(c) = l.customer {
                v["contactId"] = json!(c);
            }
            if l.billable {
                v["isBillable"] = Value::Bool(true);
            }
            v
        })
        .collect();
    let mut v = json!({
        "contactId": p.vendor,
        "txnDate": p.date,
        "lines": lines,
    });
    let reference = clean_ref(p.vendor_ref);
    let memo = p.memo.filter(|m| !m.is_empty());
    if update {
        v["vendorInvoiceNumber"] = Value::String(reference.unwrap_or_default());
        v["memo"] = Value::String(memo.unwrap_or_default().to_string());
    } else {
        if let Some(r) = reference {
            v["vendorInvoiceNumber"] = Value::String(r);
        }
        if let Some(m) = memo {
            v["memo"] = Value::String(m.to_string());
        }
    }
    if p.kind == ApKind::Bill {
        if let Some(d) = p.due_date {
            v["dueDate"] = json!(d);
        }
        if let Some(t) = p.terms {
            v["paymentTerms"] = Value::String(t.code.clone());
            if let Some(d) = t.days {
                v["termsDays"] = json!(d);
            }
            if let Some(pct) = t.discount_percent {
                v["discountPercent"] = Value::String(pct.to_wire());
            }
            if let Some(d) = t.discount_days {
                v["discountDays"] = json!(d);
            }
        }
        if caps.has(Capabilities::BILL_RECEIVED) {
            v["billReceived"] = Value::Bool(p.received);
        }
    }
    if !update && let Some(k) = p.draft {
        v["draftAttachmentId"] = Value::String(k.to_string());
    }
    v
}

/// A body with the closing-date password (`ap-closing-v1`, else refused
/// with nothing sent) and, with `ap-edit-v1`, the stale guard.
pub(crate) fn ap_body(
    s: &Session,
    mut body: Value,
    opts: &PostingOptions,
    expected_updated_at: Option<cl_core::Timestamp>,
) -> Result<Vec<u8>, ApiError> {
    let caps = s.client().capabilities();
    if opts.closed_period.is_some() && !caps.has(Capabilities::AP_CLOSING) {
        return Err(ApiError::Local(LocalValidation::Unsupported));
    }
    if let (Some(t), true, Value::Object(map)) = (
        expected_updated_at,
        caps.has(Capabilities::AP_EDIT),
        &mut body,
    ) {
        map.insert("expectedUpdatedAt".into(), Value::String(t.to_wire()));
    }
    posting_body(s, body, opts)
}

/// Options of a void or delete of a bill or vendor credit.
#[derive(Clone, Debug, Default)]
pub struct ApWriteOptions {
    /// The closing-date password (`ap-closing-v1`; refused locally
    /// without it).
    pub posting: PostingOptions,
    /// The record's `updatedAt` when it was read: sent as
    /// `expectedUpdatedAt` with `ap-edit-v1` (409 `STALE` when it moved),
    /// left out otherwise.
    pub expected_updated_at: Option<cl_core::Timestamp>,
}

/// One document line as hashed: the stored line, or the draft line as the
/// engine will store it.
struct HashLine<'a> {
    account: AccountId,
    amount: Money,
    description: Option<&'a str>,
    tag: Option<TagId>,
    customer: Option<ContactId>,
    billable: bool,
    item: Option<ItemId>,
    quantity: Option<Quantity>,
    unit_price: Option<Money>,
}

impl HashLine<'_> {
    fn value(&self) -> Value {
        let mut v = json!({
            "accountId": self.account,
            "amount": self.amount.to_wire(),
            "description": self.description.filter(|d| !d.is_empty()),
            "tagId": self.tag,
        });
        // Hashed only when set, so a plain line hashes the same from any
        // engine.
        if let Some(c) = self.customer {
            v["contactId"] = json!(c);
        }
        if self.billable {
            v["isBillable"] = Value::Bool(true);
        }
        if let Some(i) = self.item {
            v["itemId"] = json!(i);
        }
        if let Some(q) = self.quantity {
            v["quantity"] = Value::String(q.to_wire());
        }
        if let Some(r) = self.unit_price {
            v["unitPrice"] = Value::String(r.to_wire());
        }
        v
    }
}

/// The hashed header of a document.
struct HashHead<'a> {
    kind: ApKind,
    date: CalendarDate,
    status: TxnStatus,
    vendor: Option<ContactId>,
    memo: Option<&'a str>,
    vendor_ref: Option<&'a str>,
    terms: Option<&'a str>,
    terms_days: Option<u32>,
    total: Option<Money>,
    received: bool,
    discount_percent: Option<Percent>,
    discount_days: Option<u32>,
}

fn doc_editable(h: &HashHead<'_>, lines: &[HashLine<'_>]) -> Value {
    let custom = h.terms == Some(TERMS_CUSTOM);
    let mut v = json!({
        "txnType": h.kind.txn_type(),
        "txnDate": h.date,
        "status": h.status,
        "contactId": h.vendor,
        "memo": h.memo.filter(|m| !m.is_empty()),
        "vendorInvoiceNumber": h.vendor_ref.map(str::trim).filter(|r| !r.is_empty()),
        "paymentTerms": h.terms.filter(|t| !t.is_empty()),
        // Days count only for custom terms: for fixed terms the engine may
        // store the vendor's custom days alongside.
        "termsDays": if custom { h.terms_days } else { None },
        "total": h.total.map(Money::to_wire),
        "lines": lines.iter().map(HashLine::value).collect::<Vec<_>>(),
    });
    // Hashed only when set (U-86/U-87), so older records hash as before.
    if !h.received {
        v["billReceived"] = Value::Bool(false);
    }
    if let Some(p) = h.discount_percent {
        v["discountPercent"] = Value::String(p.to_wire());
    }
    if let Some(d) = h.discount_days {
        v["discountDays"] = json!(d);
    }
    v
}

/// The fingerprint the record has once `p` is saved: the engine stores
/// each line's description, or the memo when it is blank.
pub(crate) fn requested_fingerprint(p: &DocParts<'_>) -> [u8; 32] {
    let memo = p.memo.filter(|m| !m.is_empty());
    let lines: Vec<HashLine<'_>> = p
        .lines
        .iter()
        .map(|l| HashLine {
            account: l.account,
            amount: l.amount,
            description: l.description.as_deref().filter(|d| !d.is_empty()).or(memo),
            tag: l.tag,
            customer: l.customer,
            billable: l.billable,
            item: l.item,
            quantity: l.quantity,
            unit_price: l.unit_price,
        })
        .collect();
    let terms = p.terms;
    fingerprint_of(&doc_editable(
        &HashHead {
            kind: p.kind,
            date: p.date,
            status: TxnStatus::Posted,
            vendor: Some(p.vendor),
            memo,
            vendor_ref: p.vendor_ref,
            terms: terms.map(|t| t.code.as_str()),
            terms_days: terms.and_then(|t| t.days),
            total: Some(p.amount),
            received: p.received,
            discount_percent: terms.and_then(|t| t.discount_percent),
            discount_days: terms.and_then(|t| t.discount_days),
        },
        &lines,
    ))
}

/// The row fingerprint of the create probe (D2): vendor, date, reference
/// and total of a posted document. `with_ref` is false for vendor credit
/// lists that do not carry the reference (before `ap-edit-v1`, S6).
fn row_key(v: &Value, with_ref: bool) -> Option<[u8; 32]> {
    if v.get("status").and_then(Value::as_str) != Some("posted") {
        return None;
    }
    let total = Money::parse_wire(v.get("total")?.as_str()?).ok()?;
    let date = CalendarDate::parse_wire(v.get("txnDate")?.as_str()?).ok()?;
    let reference = if with_ref {
        v.get("vendorInvoiceNumber")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|r| !r.is_empty())
    } else {
        None
    };
    Some(fingerprint_of(&json!({
        "contactId": v.get("contactId")?.as_str()?,
        "txnDate": date,
        "vendorInvoiceNumber": reference,
        "total": total.to_wire(),
    })))
}

#[allow(clippy::needless_pass_by_value, reason = "the RowFp signature")]
pub(crate) fn row_key_with_ref(v: Value) -> Option<[u8; 32]> {
    row_key(&v, true)
}

#[allow(clippy::needless_pass_by_value, reason = "the RowFp signature")]
pub(crate) fn row_key_no_ref(v: Value) -> Option<[u8; 32]> {
    row_key(&v, false)
}

/// The probe's expected row fingerprint for `p`.
pub(crate) fn expected_row_key(p: &DocParts<'_>, with_ref: bool) -> [u8; 32] {
    let reference = if with_ref {
        clean_ref(p.vendor_ref)
    } else {
        None
    };
    fingerprint_of(&json!({
        "contactId": p.vendor.to_string(),
        "txnDate": p.date,
        "vendorInvoiceNumber": reference,
        "total": p.amount.to_wire(),
    }))
}

/// A bill's payment state (`billStatus`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BillStatus {
    /// Nothing paid or credited.
    Unpaid,
    /// Partly paid or credited.
    Partial,
    /// Paid in full.
    Paid,
    /// Past due (as stored).
    Overdue,
    /// A state this version does not know.
    #[serde(other)]
    Other,
}

/// One document line of a saved bill or vendor credit.
#[derive(Clone, PartialEq, Eq)]
pub struct BillRecordLine {
    /// The account.
    pub account: AccountId,
    /// The amount (the debit of a bill line, the credit of a credit line).
    pub amount: Money,
    /// Description (owner data).
    pub description: Option<String>,
    /// Tag.
    pub tag: Option<TagId>,
    /// Customer (job).
    pub customer: Option<ContactId>,
    /// Billable to the customer.
    pub billable: bool,
    /// Item.
    pub item: Option<ItemId>,
    /// Quantity.
    pub quantity: Option<Quantity>,
    /// Cost per unit.
    pub unit_price: Option<Money>,
}

impl fmt::Debug for BillRecordLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BillRecordLine")
            .field("account", &self.account)
            .field("tag", &self.tag)
            .field("item", &self.item)
            .field("customer", &self.customer)
            .field("billable", &self.billable)
            .finish_non_exhaustive()
    }
}

/// A saved bill or vendor credit (`GET /bills/{id}`, `GET
/// /vendor-credits/{id}` and the save answers).
#[derive(Clone, PartialEq, Eq)]
pub struct BillRecord {
    /// The ledger record (lines include the engine's A/P line).
    pub txn: Transaction,
    /// The document lines (expense side), in order.
    pub lines: Vec<BillRecordLine>,
    /// The vendor's reference number.
    pub vendor_invoice_number: Option<String>,
    /// Due date (engine-computed unless the owner set it).
    pub due_date: Option<CalendarDate>,
    /// Terms code.
    pub payment_terms: Option<String>,
    /// Terms days.
    pub terms_days: Option<u32>,
    /// What is still owed (bill) or open (credit), as the engine sent it.
    pub balance_due: Option<Money>,
    /// Paid so far.
    pub amount_paid: Option<Money>,
    /// Credits applied so far.
    pub credits_applied: Option<Money>,
    /// Payment state.
    pub bill_status: Option<BillStatus>,
    /// Received (`bill-received-v1`; `true` from engines without it).
    pub bill_received: bool,
    /// Discount (`bill-terms-v1`).
    pub discount_percent: Option<Percent>,
    /// Discount days (`bill-terms-v1`).
    pub discount_days: Option<u32>,
    /// Last day of the discount (`bill-terms-v1`).
    pub discount_date: Option<CalendarDate>,
}

impl BillRecord {
    /// Bill or vendor credit.
    #[must_use]
    pub fn kind(&self) -> ApKind {
        if self.txn.txn_type == TxnType::VendorCredit {
            ApKind::VendorCredit
        } else {
            ApKind::Bill
        }
    }

    /// True once payments or credits are applied (vendor, date and total
    /// are then read-only; void is refused). Read from the engine's state.
    #[must_use]
    pub fn has_payments(&self) -> bool {
        matches!(
            self.bill_status,
            Some(BillStatus::Partial | BillStatus::Paid)
        ) || self.amount_paid.is_some_and(Money::is_positive)
            || self.credits_applied.is_some_and(Money::is_positive)
    }
}

impl fmt::Debug for BillRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BillRecord")
            .field("id", &self.txn.id)
            .field("txn_type", &self.txn.txn_type)
            .field("date", &self.txn.txn_date)
            .field("status", &self.txn.status)
            .field("due_date", &self.due_date)
            .field("bill_status", &self.bill_status)
            .field("bill_received", &self.bill_received)
            .field("lines", &self.lines.len())
            .finish_non_exhaustive()
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RecordWire {
    #[serde(default)]
    vendor_invoice_number: Option<String>,
    #[serde(default)]
    due_date: Option<CalendarDate>,
    #[serde(default)]
    payment_terms: Option<String>,
    #[serde(default)]
    terms_days: Option<u32>,
    #[serde(default)]
    balance_due: Option<Money>,
    #[serde(default)]
    amount_paid: Option<Money>,
    #[serde(default)]
    credits_applied: Option<Money>,
    #[serde(default)]
    bill_status: Option<BillStatus>,
    #[serde(default)]
    bill_received: Option<bool>,
    #[serde(default)]
    discount_percent: Option<Percent>,
    #[serde(default)]
    discount_days: Option<u32>,
    #[serde(default)]
    discount_date: Option<CalendarDate>,
    #[serde(default)]
    lines: Vec<LineWire>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LineWire {
    #[serde(default)]
    item_id: Option<ItemId>,
    #[serde(default)]
    quantity: Option<Quantity>,
    #[serde(default)]
    unit_price: Option<Money>,
}

/// Reads the object as the [`Transaction`] and again for the A/P fields.
/// A record that is neither a bill nor a vendor credit is refused.
impl<'de> Deserialize<'de> for BillRecord {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = Value::deserialize(d)?;
        let txn = Transaction::deserialize(&v).map_err(|_| de::Error::custom("transaction"))?;
        let w = RecordWire::deserialize(&v).map_err(|_| de::Error::custom("bill fields"))?;
        let kind = match txn.txn_type {
            TxnType::Bill => ApKind::Bill,
            TxnType::VendorCredit => ApKind::VendorCredit,
            _ => return Err(de::Error::custom("not a bill or vendor credit")),
        };
        if w.lines.len() != txn.lines.len() {
            return Err(de::Error::custom("lines"));
        }
        let mut pairs: Vec<_> = txn.lines.iter().zip(w.lines).collect();
        pairs.sort_by_key(|(l, _)| l.line_order.unwrap_or(0));
        let lines = pairs
            .into_iter()
            .filter_map(|(l, x)| {
                let amount = match kind {
                    ApKind::Bill => l.debit,
                    ApKind::VendorCredit => l.credit,
                };
                amount.is_positive().then(|| BillRecordLine {
                    account: l.account_id,
                    amount,
                    description: l.description.clone(),
                    tag: l.tag_id,
                    customer: l.contact_id,
                    billable: l.is_billable,
                    item: x.item_id,
                    quantity: x.quantity,
                    unit_price: x.unit_price,
                })
            })
            .collect();
        Ok(BillRecord {
            txn,
            lines,
            vendor_invoice_number: w.vendor_invoice_number,
            due_date: w.due_date,
            payment_terms: w.payment_terms,
            terms_days: w.terms_days,
            balance_due: w.balance_due,
            amount_paid: w.amount_paid,
            credits_applied: w.credits_applied,
            bill_status: w.bill_status,
            bill_received: w.bill_received.unwrap_or(true),
            discount_percent: w.discount_percent,
            discount_days: w.discount_days,
            discount_date: w.discount_date,
        })
    }
}

impl EditableFields for BillRecord {
    const EDITABLE: &'static [&'static str] = &[
        "txnType",
        "txnDate",
        "status",
        "contactId",
        "memo",
        "vendorInvoiceNumber",
        "paymentTerms",
        "termsDays",
        "total",
        "lines.accountId",
        "lines.amount",
        "lines.description",
        "lines.tagId",
        "lines.contactId",
        "lines.isBillable",
        "lines.itemId",
        "lines.quantity",
        "lines.unitPrice",
        // Each hashed only when set (U-86, U-87).
        "billReceived",
        "discountPercent",
        "discountDays",
    ];

    fn editable(&self) -> Value {
        let lines: Vec<HashLine<'_>> = self
            .lines
            .iter()
            .map(|l| HashLine {
                account: l.account,
                amount: l.amount,
                description: l.description.as_deref(),
                tag: l.tag,
                customer: l.customer,
                billable: l.billable,
                item: l.item,
                quantity: l.quantity,
                unit_price: l.unit_price,
            })
            .collect();
        doc_editable(
            &HashHead {
                kind: self.kind(),
                date: self.txn.txn_date,
                status: self.txn.status,
                vendor: self.txn.contact_id,
                memo: self.txn.memo.as_deref(),
                vendor_ref: self.vendor_invoice_number.as_deref(),
                terms: self.payment_terms.as_deref(),
                terms_days: self.terms_days,
                total: self.txn.total,
                received: self.bill_received,
                discount_percent: self.discount_percent,
                discount_days: self.discount_days,
            },
            &lines,
        )
    }
}

// ---- Create, edit and delete, shared by bills and vendor credits ----

/// Where one kind of A/P document lives on the engine.
pub(crate) struct DocRoutes {
    pub create: &'static Operation,
    pub list: &'static Operation,
    pub get: &'static Operation,
    pub put: &'static Operation,
    /// `/api/v1/bills`
    pub base: &'static str,
    /// Key of one record in an answer (`bill`, `credit`).
    pub key: &'static str,
}

/// Creates a document: checked first, the closing-date password with
/// `ap-closing-v1`, and a lost reply reconciled (D2) by a document of this
/// vendor, date, reference and total created after the send and not by
/// this session. Vendor credit rows carry the reference only with
/// `ap-edit-v1` (S6); before that the reference is left out of the match.
pub(crate) async fn create_doc(
    session: &Session,
    r: &DocRoutes,
    p: &DocParts<'_>,
    opts: &PostingOptions,
    key: IdempotencyKey,
) -> Result<Versioned<BillRecord>, ApiError> {
    // One company for the stamp read, the probe and the write.
    let s = &session.pinned();
    let caps = s.client().capabilities();
    check_doc(p, &caps)?;
    let body = ap_body(s, doc_body(p, &caps, false), opts, None)?;
    let with_ref = p.kind == ApKind::Bill || caps.has(Capabilities::AP_EDIT);
    let matcher = OutcomeMatcher::Created {
        search: Search::Rows {
            list: r.list,
            list_key: "data",
            query: vec![
                ("contactId".into(), p.vendor.to_string()),
                ("startDate".into(), p.date.to_wire()),
                ("endDate".into(), p.date.to_wire()),
            ],
            get: r.get,
            base: r.base,
        },
        fp: if with_ref {
            row_key_with_ref
        } else {
            row_key_no_ref
        },
        fingerprint: expected_row_key(p, with_ref),
    };
    let probe = Probe::new(s, matcher, true).await;
    let mut call = Call::at(Method::Post, r.create.route()).json_body(body);
    call.idempotency_key = Some(key);
    let reply = s.send(r.create, call, Some(&probe)).await?;
    remember_created(s, &reply.body, r.key);
    saved_from(&reply.body, r.key)
}

/// Replaces a document if it is still as `expected` was read: the
/// fingerprint is compared just before sending (`Conflict { Stale }`,
/// nothing sent), and with `ap-edit-v1` the engine also checks
/// `expectedUpdatedAt` inside its row lock (409 `STALE`). Before
/// `ap-edit-v1` the engine answers with the record as it was before its
/// last write (review B3), so the saved record is read again.
pub(crate) async fn update_doc(
    session: &Session,
    r: &DocRoutes,
    id: TransactionId,
    expected: &Versioned<BillRecord>,
    p: &DocParts<'_>,
    opts: &PostingOptions,
    key: IdempotencyKey,
) -> Result<Versioned<BillRecord>, ApiError> {
    let s = &session.pinned();
    let caps = s.client().capabilities();
    check_doc(p, &caps)?;
    let body = ap_body(
        s,
        doc_body(p, &caps, true),
        opts,
        expected.record.txn.updated_at,
    )?;
    let path = format!("{}/{id}", r.base);
    let u = GuardedUpdate {
        get: r.get,
        put: r.put,
        path: path.clone(),
        key: r.key,
        body,
        idempotency_key: key,
        requested: requested_fingerprint(p),
        fp: row_fp::<BillRecord>,
        stamp_covered: true,
    };
    let reply = update_guarded_reply::<BillRecord>(s, &expected.version, u, None).await?;
    if caps.has(Capabilities::AP_EDIT) {
        saved_from(&reply.body, r.key)
    } else {
        // Saved: a failed read-back is "saved, reload to see it".
        get_versioned(s, r.get, path, r.key)
            .await
            .map_err(|_| ApiError::SavedUnreadable { what: r.key })
    }
}

/// Deletes an unpaid bill or an unapplied vendor credit through
/// `DELETE /transactions/{id}` (`ap-edit-v1`, U-84b; the engine posts
/// reversing lines and keeps it in the audit trail).
///
/// # Errors
/// Nothing is sent for: `Local(Unsupported)` without `ap-edit-v1` (void it
/// instead); `Local(TxnDeleteUnavailable)` without `txn-delete-v1`;
/// `Local(InvalidValue)` for a reason over 500 characters;
/// `Local(ClosedPeriodOverrideUnavailable)`. From the engine: `Conflict`
/// (`BILL_HAS_PAYMENTS`, `CREDIT_APPLIED`, `TXN_DEPOSITED`,
/// `TXN_DELETE_LINKED`, `STALE`; see [`ap_refusal`]), `ClosedPeriod`,
/// `ClosingPasswordRejected`, `OutcomeUnknown`, any [`ApiError`].
pub(crate) async fn delete_doc(
    session: &Session,
    id: TransactionId,
    reason: Option<&str>,
    opts: &ApWriteOptions,
    key: IdempotencyKey,
) -> Result<(), ApiError> {
    use crate::resources::operations::{TXN_DELETE, TXN_GET};
    use crate::resources::transactions::{DELETE_REASON_MAX_CHARS, SPEC};
    let s = &session.pinned();
    let caps = s.client().capabilities();
    if !caps.has(Capabilities::AP_EDIT) {
        return Err(ApiError::Local(LocalValidation::Unsupported));
    }
    if !caps.has(Capabilities::TXN_DELETE) {
        return Err(ApiError::Local(LocalValidation::TxnDeleteUnavailable));
    }
    let reason = reason.map(str::trim).filter(|r| !r.is_empty());
    if reason.is_some_and(|r| r.chars().count() > DELETE_REASON_MAX_CHARS) {
        return Err(ApiError::Local(LocalValidation::InvalidValue));
    }
    let mut body = serde_json::Map::new();
    if let Some(r) = reason {
        body.insert("reason".into(), Value::String(r.to_string()));
    }
    if let Some(t) = opts.expected_updated_at {
        body.insert("expectedUpdatedAt".into(), Value::String(t.to_wire()));
    }
    let body = posting_body(s, Value::Object(body), &opts.posting)?;
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
    s.send(&TXN_DELETE, call, Some(&probe)).await?;
    Ok(())
}

/// Every row of a document list in pages of `page_size`, at most
/// `max_pages` pages (D3: 20 Ã— 50 for the duplicate check). `complete` is
/// false when rows were left unread.
pub(crate) async fn list_capped<T: serde::de::DeserializeOwned>(
    s: &Session,
    list: &'static Operation,
    query: &[(String, String)],
    page_size: u16,
    max_pages: u32,
) -> Result<Capped<T>, ApiError> {
    let mut rows = Vec::new();
    let mut page = crate::wire::PageRequest::new(page_size, 0)?;
    for _ in 0..max_pages {
        let mut call = Call::at(Method::Get, list.route());
        call.query = query.to_vec();
        call.query.extend(page.query());
        let reply = s.send(list, call, None).await?;
        let p: crate::wire::Page<T> = crate::wire::Page::from_json(&reply.body, "data")?;
        let got = p.data.len();
        rows.extend(p.data);
        if got == 0 || u64::try_from(rows.len()).unwrap_or(u64::MAX) >= p.total {
            return Ok(Capped {
                rows,
                complete: true,
            });
        }
        page = page.next();
    }
    Ok(Capped {
        rows,
        complete: false,
    })
}

/// Rows read with a page cap.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Capped<T> {
    /// The rows read.
    pub rows: Vec<T>,
    /// False when the cap left rows unread ("Couldn't check every bill").
    pub complete: bool,
}

// ---- The engine's refusals of A/P documents ----

/// 409: the record changed since it was read (U-17 shape, `ap-edit-v1`).
pub const STALE: &str = "STALE";
/// 409: the vendor credit is applied to bills (or refunded) and can't be
/// changed or deleted.
pub const CREDIT_APPLIED: &str = "CREDIT_APPLIED";
/// 409: the bill has payments or credits applied and can't be deleted.
pub const BILL_HAS_PAYMENTS: &str = "BILL_HAS_PAYMENTS";
/// 409: the credit was deposited as a vendor refund.
pub const TXN_DEPOSITED: &str = "TXN_DEPOSITED";
/// 409: other records depend on it (a refunded credit, review Notes).
pub const TXN_DELETE_LINKED: &str = "TXN_DELETE_LINKED";
/// 409: this kind can't be deleted; void it instead.
pub const TXN_DELETE_UNSUPPORTED: &str = "TXN_DELETE_UNSUPPORTED";
/// 400: an item line's amount is not its cost Ãƒâ€” quantity (`bill-lines-v1`).
pub const LINE_AMOUNT_MISMATCH: &str = "LINE_AMOUNT_MISMATCH";
/// 400: a cost without a quantity (`bill-lines-v1`).
pub const LINE_QTY_RATE_INCOMPLETE: &str = "LINE_QTY_RATE_INCOMPLETE";

/// The engine's refusal of a bill or vendor credit save, void or delete,
/// for the form's plain-language message.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ApRefusal {
    /// Someone changed it since it was read (`STALE`, or the fingerprint
    /// check before sending).
    Stale,
    /// The credit is applied or refunded (`CREDIT_APPLIED`).
    CreditApplied,
    /// The bill has payments or credits (`BILL_HAS_PAYMENTS`).
    BillHasPayments,
    /// A line was already billed to a customer (`LINE_BILLED`).
    LineBilled,
    /// Deposited as a vendor refund (`TXN_DEPOSITED`).
    Deposited,
    /// Other records depend on it (`TXN_DELETE_LINKED`).
    DeleteLinked,
    /// Can't be deleted; void it (`TXN_DELETE_UNSUPPORTED`).
    DeleteUnsupported,
    /// An item line's amount is not its cost Ãƒâ€” quantity.
    LineAmountMismatch,
    /// A cost without a quantity.
    LineQtyRateIncomplete,
}

/// Classifies `e` (`None` for anything else: closed periods, passwords,
/// network, plain validation).
#[must_use]
pub fn ap_refusal(e: &ApiError) -> Option<ApRefusal> {
    use crate::errors::ConflictReason;
    let code = match e {
        ApiError::Conflict {
            reason: ConflictReason::Stale,
            ..
        } => return Some(ApRefusal::Stale),
        ApiError::Conflict { code: Some(c), .. } => c.as_str(),
        ApiError::Validation(v) => v.code().map_or("", |c| c.as_str()),
        _ => return None,
    };
    Some(match code {
        STALE => ApRefusal::Stale,
        CREDIT_APPLIED => ApRefusal::CreditApplied,
        BILL_HAS_PAYMENTS => ApRefusal::BillHasPayments,
        crate::resources::checks::LINE_BILLED => ApRefusal::LineBilled,
        TXN_DEPOSITED => ApRefusal::Deposited,
        TXN_DELETE_LINKED => ApRefusal::DeleteLinked,
        TXN_DELETE_UNSUPPORTED => ApRefusal::DeleteUnsupported,
        LINE_AMOUNT_MISMATCH => ApRefusal::LineAmountMismatch,
        LINE_QTY_RATE_INCOMPLETE => ApRefusal::LineQtyRateIncomplete,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_is_exact_and_bounded() {
        assert_eq!(Percent::parse("2").expect("2").to_wire(), "2");
        assert_eq!(Percent::parse("1.5").expect("1.5").to_wire(), "1.5");
        assert_eq!(Percent::parse("0.0001").expect("tiny").to_wire(), "0.0001");
        assert_eq!(Percent::parse("99.9999").expect("max").to_wire(), "99.9999");
        for bad in [
            "0", "100", "-1", "1e1", "1.", ".5", "1.23456", " 2", "2%", "", "100.0",
        ] {
            assert_eq!(
                Percent::parse(bad),
                Err(LocalValidation::InvalidValue),
                "{bad}"
            );
        }
        let p: Percent = serde_json::from_str(r#""2.0000""#).expect("engine text");
        assert_eq!(p.to_wire(), "2");
        assert!(serde_json::from_str::<Percent>("2").is_err(), "a number");
    }

    #[test]
    fn refusals_are_classified_by_code() {
        use crate::errors::{ConflictReason, ErrorCode, UserText, ValidationError};
        let conflict = |c: &str| ApiError::Conflict {
            reason: ConflictReason::Unspecified,
            current_version: None,
            code: ErrorCode::parse(c),
            message: UserText::default(),
        };
        assert_eq!(
            ap_refusal(&conflict(CREDIT_APPLIED)),
            Some(ApRefusal::CreditApplied)
        );
        assert_eq!(
            ap_refusal(&conflict(BILL_HAS_PAYMENTS)),
            Some(ApRefusal::BillHasPayments)
        );
        assert_eq!(
            ap_refusal(&conflict("LINE_BILLED")),
            Some(ApRefusal::LineBilled)
        );
        assert_eq!(
            ap_refusal(&conflict(TXN_DEPOSITED)),
            Some(ApRefusal::Deposited)
        );
        assert_eq!(
            ap_refusal(&conflict(TXN_DELETE_LINKED)),
            Some(ApRefusal::DeleteLinked)
        );
        assert_eq!(
            ap_refusal(&conflict(TXN_DELETE_UNSUPPORTED)),
            Some(ApRefusal::DeleteUnsupported)
        );
        assert_eq!(ap_refusal(&conflict("OTHER")), None);
        let stale = ApiError::Conflict {
            reason: ConflictReason::Stale,
            current_version: None,
            code: None,
            message: UserText::default(),
        };
        assert_eq!(ap_refusal(&stale), Some(ApRefusal::Stale));
        let v = ApiError::Validation(ValidationError::for_field(
            LINE_AMOUNT_MISMATCH,
            "lines",
            "x",
        ));
        assert_eq!(ap_refusal(&v), Some(ApRefusal::LineAmountMismatch));
        let v = ApiError::Validation(ValidationError::for_field(
            LINE_QTY_RATE_INCOMPLETE,
            "lines",
            "x",
        ));
        assert_eq!(ap_refusal(&v), Some(ApRefusal::LineQtyRateIncomplete));
        assert_eq!(ap_refusal(&ApiError::Timeout), None);
    }
}
