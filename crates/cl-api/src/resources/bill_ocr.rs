//! Reading a bill out of an uploaded file with the owner's AI provider
//! (P17 WP2; `POST /bills/extract-from-attachment/{id}`).
//!
//! - Only on the owner's click (Q-73): [`BillReadGate`] must be open or
//!   nothing is sent (`AiUnavailable`). The engine applies its own AI and
//!   consent gates too; those refusals are `AiUnavailable` as well.
//! - Every amount read is owner-unaccepted data (non-negotiable 4): it is
//!   parsed exactly into `Option<Money>` from the engine's text, and a
//!   value that does not parse is `None` with the field listed in
//!   [`BillReading::unreadable`]. Nothing here rounds.
//! - Confidence: before `bill-ocr-v2` the engine sends one document-level
//!   JSON number; it is read from its JSON text into a `Decimal` (no float
//!   anywhere). With `bill-ocr-v2` each field and line has `high`, `medium`
//!   or `low`.
//! - `Debug` shows only counts, flags and the outcome: the document's text,
//!   names and amounts never reach a log.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use cl_core::ids::{AccountId, ContactId};
use cl_core::money::UserInput;
use cl_core::{CalendarDate, Money, Quantity};
use rust_decimal::Decimal;
use serde::Deserialize;
use serde_json::value::RawValue;

use crate::client::{Call, Method};
use crate::errors::ApiError;
use crate::idempotency::IdempotencyKey;
use crate::resources::attachments::AttachmentId;
use crate::resources::operations::BILL_EXTRACT;
use crate::session::Session;

/// The owner-side switches that must all be on before a reading is asked
/// for (P09 preferences: AI on, bill reading on, a provider chosen).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BillReadGate {
    /// AI is turned on.
    pub ai_enabled: bool,
    /// Reading bills is switched on.
    pub bill_reading_on: bool,
    /// A provider is set.
    pub provider_set: bool,
}

impl BillReadGate {
    /// True when every switch is on.
    #[must_use]
    pub const fn is_open(self) -> bool {
        self.ai_enabled && self.bill_reading_on && self.provider_set
    }
}

/// How sure the reader is of a field (`bill-ocr-v2`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Confidence {
    /// Low: mark it strongly.
    Low,
    /// Medium.
    Medium,
    /// High.
    High,
}

impl Confidence {
    fn from_wire(s: &str) -> Option<Confidence> {
        match s {
            "high" => Some(Confidence::High),
            "medium" => Some(Confidence::Medium),
            "low" => Some(Confidence::Low),
            _ => None,
        }
    }
}

/// The document-level confidence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocConfidence {
    /// A score as the engine sent it (a JSON number before `bill-ocr-v2`,
    /// usually 0 to 1), exact from its text.
    Score(Decimal),
    /// A level (`bill-ocr-v2`).
    Level(Confidence),
}

/// A field of the reading that the engine sent but that could not be read
/// as a date or an amount (shown as "check it", never guessed).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ReadField {
    /// The bill date.
    BillDate,
    /// The due date.
    DueDate,
    /// The total.
    Total,
    /// The subtotal.
    Subtotal,
    /// The tax.
    Tax,
    /// A line's amount (0-based).
    LineAmount(usize),
    /// A line's quantity (0-based).
    LineQuantity(usize),
}

/// What the reading found (`status`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ReadStatus {
    /// Fields were read.
    Ok,
    /// Only text was found: show it read-only, fill nothing.
    OcrOnly,
}

/// One line read off the document.
#[derive(Clone, PartialEq, Eq)]
pub struct ReadLine {
    /// Description (document text).
    pub description: Option<String>,
    /// Amount, exact (`None` when absent or unreadable).
    pub amount: Option<Money>,
    /// Quantity, exact (`None` when absent or unreadable).
    pub quantity: Option<Quantity>,
    /// Line confidence (`bill-ocr-v2`).
    pub confidence: Option<Confidence>,
}

impl fmt::Debug for ReadLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReadLine")
            .field("amount", &self.amount.is_some())
            .field("quantity", &self.quantity.is_some())
            .field("confidence", &self.confidence)
            .finish_non_exhaustive()
    }
}

/// A bill read off a document. Every value is a suggestion the owner must
/// accept.
#[derive(Clone, PartialEq, Eq)]
pub struct BillReading {
    /// Fields, or text only.
    pub status: ReadStatus,
    /// The vendor's name as printed.
    pub vendor_name: Option<String>,
    /// The vendor the engine matched by exact name (a vendor contact).
    pub vendor: Option<ContactId>,
    /// That vendor's default expense account.
    pub default_expense_account: Option<AccountId>,
    /// The vendor's invoice number.
    pub vendor_invoice_number: Option<String>,
    /// Bill date.
    pub bill_date: Option<CalendarDate>,
    /// Due date.
    pub due_date: Option<CalendarDate>,
    /// Terms as printed or as a code.
    pub payment_terms: Option<String>,
    /// Total.
    pub total: Option<Money>,
    /// Subtotal.
    pub subtotal: Option<Money>,
    /// Tax.
    pub tax: Option<Money>,
    /// Lines.
    pub lines: Vec<ReadLine>,
    /// Notes.
    pub notes: Option<String>,
    /// The text found (`OcrOnly`).
    pub raw_text: Option<String>,
    /// Document-level confidence.
    pub confidence: Option<DocConfidence>,
    /// Per-field confidence by the engine's field name (`vendor`,
    /// `billDate`, `total`, …; `bill-ocr-v2`). Empty before.
    pub field_confidence: BTreeMap<String, Confidence>,
    /// Engine notes on quality (`tesseract_local_ocr`, …).
    pub quality_warnings: Vec<String>,
    /// Fields sent but not readable as a date or an amount.
    pub unreadable: Vec<ReadField>,
}

/// Counts and flags only: the document's content is owner data.
impl fmt::Debug for BillReading {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BillReading")
            .field("status", &self.status)
            .field("vendor", &self.vendor)
            .field("lines", &self.lines.len())
            .field("total", &self.total.is_some())
            .field("field_confidence", &self.field_confidence.len())
            .field("unreadable", &self.unreadable)
            .finish_non_exhaustive()
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LineWire {
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    amount: Option<Box<RawValue>>,
    #[serde(default)]
    quantity: Option<Box<RawValue>>,
    #[serde(default)]
    confidence: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReadingWire {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    vendor: Option<String>,
    #[serde(default)]
    contact_id: Option<ContactId>,
    #[serde(default)]
    default_expense_account_id: Option<AccountId>,
    #[serde(default)]
    vendor_invoice_number: Option<String>,
    #[serde(default)]
    bill_date: Option<String>,
    #[serde(default)]
    due_date: Option<String>,
    #[serde(default)]
    payment_terms: Option<String>,
    #[serde(default)]
    total: Option<Box<RawValue>>,
    #[serde(default)]
    subtotal: Option<Box<RawValue>>,
    #[serde(default)]
    tax: Option<Box<RawValue>>,
    #[serde(default)]
    line_items: Vec<LineWire>,
    #[serde(default)]
    notes: Option<String>,
    #[serde(default)]
    raw_text: Option<String>,
    #[serde(default)]
    confidence: Option<Box<RawValue>>,
    #[serde(default)]
    field_confidence: Option<BTreeMap<String, String>>,
    #[serde(default)]
    quality_warnings: Vec<String>,
}

/// The text of a JSON string or number (`None` for `null`).
fn raw_text(raw: Option<&RawValue>) -> Option<String> {
    let raw = raw?;
    let t = raw.get().trim();
    if t == "null" {
        return None;
    }
    match serde_json::from_str::<String>(t) {
        Ok(s) => Some(s),
        // A number: its exact JSON text.
        Err(_) => Some(t.to_string()),
    }
}

/// An exact decimal from document text: the engine grammar, else what an
/// owner may type (`$1,330.00`), at most 4 decimals, never rounded; or a
/// JSON number in exponent form.
fn exact_decimal(s: &str) -> Option<Decimal> {
    let s = s.trim();
    if let Ok(m) = Money::parse_wire(s) {
        return Some(m.as_decimal());
    }
    if let Ok(m) = Money::parse_user(s, UserInput::FULL) {
        return Some(m.as_decimal());
    }
    Decimal::from_str(s)
        .or_else(|_| Decimal::from_scientific(s))
        .ok()
}

fn money_field(
    raw: Option<&RawValue>,
    field: ReadField,
    bad: &mut Vec<ReadField>,
) -> Option<Money> {
    let text = raw_text(raw)?;
    if text.trim().is_empty() {
        return None;
    }
    let m = exact_decimal(&text).and_then(|d| Money::try_from(d).ok());
    if m.is_none() {
        bad.push(field);
    }
    m
}

fn quantity_field(
    raw: Option<&RawValue>,
    field: ReadField,
    bad: &mut Vec<ReadField>,
) -> Option<Quantity> {
    let text = raw_text(raw)?;
    if text.trim().is_empty() {
        return None;
    }
    let q = exact_decimal(&text).and_then(|d| Quantity::try_from(d).ok());
    if q.is_none() {
        bad.push(field);
    }
    q
}

fn date_field(s: Option<&str>, field: ReadField, bad: &mut Vec<ReadField>) -> Option<CalendarDate> {
    let s = s.map(str::trim).filter(|s| !s.is_empty())?;
    let d = CalendarDate::parse_wire(s).ok();
    if d.is_none() {
        bad.push(field);
    }
    d
}

fn doc_confidence(raw: Option<&RawValue>) -> Option<DocConfidence> {
    let text = raw_text(raw)?;
    Confidence::from_wire(text.trim())
        .map(DocConfidence::Level)
        .or_else(|| exact_decimal(&text).map(DocConfidence::Score))
}

/// Parses `{extraction: {...}}` of the extract answer.
///
/// # Errors
/// `Decode` when the body is not that shape.
pub fn reading_from_json(body: &[u8]) -> Result<BillReading, ApiError> {
    #[derive(Deserialize)]
    struct W {
        extraction: ReadingWire,
    }
    const WHAT: &str = "bill reading";
    let w: W = serde_json::from_slice(body).map_err(|_| ApiError::Decode { what: WHAT })?;
    let r = w.extraction;
    let mut bad = Vec::new();
    let bill_date = date_field(r.bill_date.as_deref(), ReadField::BillDate, &mut bad);
    let due_date = date_field(r.due_date.as_deref(), ReadField::DueDate, &mut bad);
    let total = money_field(r.total.as_deref(), ReadField::Total, &mut bad);
    let subtotal = money_field(r.subtotal.as_deref(), ReadField::Subtotal, &mut bad);
    let tax = money_field(r.tax.as_deref(), ReadField::Tax, &mut bad);
    let lines = r
        .line_items
        .into_iter()
        .enumerate()
        .map(|(i, l)| ReadLine {
            description: l.description,
            amount: money_field(l.amount.as_deref(), ReadField::LineAmount(i), &mut bad),
            quantity: quantity_field(l.quantity.as_deref(), ReadField::LineQuantity(i), &mut bad),
            confidence: l.confidence.as_deref().and_then(Confidence::from_wire),
        })
        .collect();
    Ok(BillReading {
        status: if r.status.as_deref() == Some("ocr_only") {
            ReadStatus::OcrOnly
        } else {
            ReadStatus::Ok
        },
        vendor_name: r.vendor,
        vendor: r.contact_id,
        default_expense_account: r.default_expense_account_id,
        vendor_invoice_number: r.vendor_invoice_number,
        bill_date,
        due_date,
        payment_terms: r.payment_terms,
        total,
        subtotal,
        tax,
        lines,
        notes: r.notes,
        raw_text: r.raw_text,
        confidence: doc_confidence(r.confidence.as_deref()),
        field_confidence: r
            .field_confidence
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(k, v)| Confidence::from_wire(&v).map(|c| (k, c)))
            .collect(),
        quality_warnings: r.quality_warnings,
        unreadable: bad,
    })
}

/// Engine codes of a failed reading (kept as `Validation` with the code;
/// the engine's own codes are lower case, cl-api keeps them upper-cased).
pub const AI_OCR_FAILED: &str = "AI_OCR_FAILED";
/// The provider (or every provider) could not be reached or failed.
pub const AI_ALL_PROVIDERS_FAILED: &str = "AI_ALL_PROVIDERS_FAILED";
/// The model's answer could not be read.
pub const AI_PARSE_FAILED: &str = "AI_PARSE_FAILED";
/// A scan local reading could not read, at a privacy level that never
/// sends images to a cloud provider.
pub const OCR_UNREADABLE_AT_PII_LEVEL: &str = "OCR_UNREADABLE_AT_PII_LEVEL";
/// A PDF with no text layer (a scan) had to be turned into page pictures,
/// which needs poppler's `pdftoppm`: ClientLedger does not ship it (GPL),
/// so a scanned PDF cannot be read yet. Text PDFs are read in the engine.
pub const PDF_RENDER_FAILED: &str = "PDF_RENDER_FAILED";
/// The PDF gave no page pictures (same cause, an empty result).
pub const PDF_RENDER_EMPTY: &str = "PDF_RENDER_EMPTY";

/// The engine's AI gates (any of these is `AiUnavailable`).
const GATE_CODES: [&str; 4] = [
    "AI_DISABLED",
    "AI_DISABLED_GLOBALLY",
    "AI_FUNCTION_DISABLED",
    "AI_CONSENT_BLOCKED",
];

/// Document reading of a session (`session.bill_reading()`).
#[derive(Clone, Copy, Debug)]
pub struct BillOcr<'a>(pub(crate) &'a Session);

impl Session {
    /// Reading bills out of uploaded files.
    #[must_use]
    pub fn bill_reading(&self) -> BillOcr<'_> {
        BillOcr(self)
    }
}

impl BillOcr<'_> {
    /// Reads `attachment` (a PDF or image already uploaded, usually to the
    /// form's draft). Nothing is stored on the bill.
    ///
    /// # Errors
    /// `AiUnavailable` when `gate` is not open (nothing sent) or the
    /// engine's AI gate refuses (AI off, the function off, no consent, or
    /// its code-less "not enabled"); `Validation` with [`AI_OCR_FAILED`],
    /// [`AI_ALL_PROVIDERS_FAILED`], [`AI_PARSE_FAILED`] or
    /// [`OCR_UNREADABLE_AT_PII_LEVEL`] when it could not read the file,
    /// [`PDF_RENDER_FAILED`] or [`PDF_RENDER_EMPTY`] for a scanned PDF;
    /// `NotFound`; `OfflineUnavailable`; `Decode`; any [`ApiError`]. The
    /// owner types the bill in by hand in every case.
    pub async fn extract(
        &self,
        gate: BillReadGate,
        attachment: AttachmentId,
        key: IdempotencyKey,
    ) -> Result<BillReading, ApiError> {
        if !gate.is_open() {
            return Err(ApiError::AiUnavailable);
        }
        let mut call = Call::new(
            Method::Post,
            BILL_EXTRACT.route(),
            format!("/api/v1/bills/extract-from-attachment/{attachment}"),
        );
        call.idempotency_key = Some(key);
        let reply = match self.0.send(&BILL_EXTRACT, call, None).await {
            Ok(r) => r,
            Err(ApiError::Validation(v))
                if v.code().is_none_or(|c| GATE_CODES.contains(&c.as_str())) =>
            {
                return Err(ApiError::AiUnavailable);
            }
            Err(e) => return Err(e),
        };
        reading_from_json(&reply.body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::testkit::{company_session, json, requests};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer};

    const ATT: &str = "5c0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
    const OPEN: BillReadGate = BillReadGate {
        ai_enabled: true,
        bill_reading_on: true,
        provider_set: true,
    };

    fn att() -> AttachmentId {
        AttachmentId::parse(ATT).expect("id")
    }

    /// Before `bill-ocr-v2`: amounts as the engine stringified them, a
    /// document confidence that is a JSON number.
    const PRE_V2: &str = r#"{"extraction":{"jobId":"j1","vendor":"Canary Rental","vendorAddress":null,
        "vendorInvoiceNumber":"INV-55871","billDate":"2026-11-03","dueDate":"2026/11/18",
        "paymentTerms":"net_15","total":"1480.00","subtotal":"1,330.00","tax":"$150","lineItems":[
        {"description":"Excavator day rate","amount":"1330.00","quantity":"1"},
        {"description":"Delivery","amount":"one fifty","quantity":null}],
        "notes":null,"confidence":0.85,"contactId":"3a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01",
        "defaultExpenseAccountId":null,"qualityWarnings":["tesseract_local_ocr"],"status":"ok"}}"#;

    #[test]
    fn pre_v2_reads_exact_strings_and_a_number_confidence_without_floats() {
        let r = reading_from_json(PRE_V2.as_bytes()).expect("reading");
        assert_eq!(r.status, ReadStatus::Ok);
        assert_eq!(r.total.map(Money::to_wire).as_deref(), Some("1480.0000"));
        assert_eq!(r.subtotal.map(Money::to_wire).as_deref(), Some("1330.0000"));
        assert_eq!(r.tax.map(Money::to_wire).as_deref(), Some("150.0000"));
        assert_eq!(r.bill_date, CalendarDate::from_ymd(2026, 11, 3).ok());
        assert_eq!(r.due_date, None, "not a wire date");
        assert_eq!(r.lines.len(), 2);
        assert_eq!(
            r.lines[0].amount.map(Money::to_wire).as_deref(),
            Some("1330.0000")
        );
        assert_eq!(
            r.lines[0].quantity.map(Quantity::to_wire).as_deref(),
            Some("1.0000")
        );
        assert_eq!(r.lines[1].amount, None);
        assert_eq!(
            r.unreadable,
            vec![ReadField::DueDate, ReadField::LineAmount(1)]
        );
        assert_eq!(
            r.confidence,
            Some(DocConfidence::Score(Decimal::from_str("0.85").expect("d")))
        );
        assert!(r.field_confidence.is_empty());
        let shown = format!("{r:?}");
        for canary in ["Canary", "1480", "55871", "Excavator"] {
            assert!(!shown.contains(canary), "{shown}");
        }
    }

    #[test]
    fn v2_levels_and_number_amounts_stay_exact() {
        let body = r#"{"extraction":{"vendor":null,"total":1480.5,"lineItems":[
            {"description":"x","amount":1e2,"confidence":"low"}],"confidence":"high",
            "fieldConfidence":{"total":"medium","vendor":"low","odd":"maybe"},"status":"ocr_only",
            "rawText":"text"}}"#;
        let r = reading_from_json(body.as_bytes()).expect("reading");
        assert_eq!(r.status, ReadStatus::OcrOnly);
        assert_eq!(r.total.map(Money::to_wire).as_deref(), Some("1480.5000"));
        assert_eq!(
            r.lines[0].amount.map(Money::to_wire).as_deref(),
            Some("100.0000")
        );
        assert_eq!(r.lines[0].confidence, Some(Confidence::Low));
        assert_eq!(r.confidence, Some(DocConfidence::Level(Confidence::High)));
        assert_eq!(r.field_confidence.len(), 2, "unknown levels are dropped");
        assert_eq!(r.field_confidence.get("total"), Some(&Confidence::Medium));
        assert_eq!(r.raw_text.as_deref(), Some("text"));
        // Five decimals are never rounded: unreadable instead.
        let r = reading_from_json(br#"{"extraction":{"total":"1.00001"}}"#).expect("r");
        assert_eq!((r.total, r.unreadable), (None, vec![ReadField::Total]));
    }

    #[tokio::test]
    async fn a_closed_gate_sends_nothing_and_engine_gates_are_ai_unavailable() {
        let server = MockServer::start().await;
        let s = company_session(&server);
        let e = s
            .bill_reading()
            .extract(BillReadGate::default(), att(), IdempotencyKey::new())
            .await
            .expect_err("gate");
        assert_eq!(e, ApiError::AiUnavailable);
        assert!(server.received_requests().await.expect("r").is_empty());

        for body in [
            r#"{"error":{"message":"AI processing is not enabled."}}"#,
            r#"{"error":{"message":"off","code":"ai_function_disabled"}}"#,
            r#"{"error":{"message":"consent","code":"ai_consent_blocked"}}"#,
        ] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path(format!("/api/v1/bills/extract-from-attachment/{ATT}")))
                .respond_with(json(400, body))
                .mount(&server)
                .await;
            let s = company_session(&server);
            let e = s
                .bill_reading()
                .extract(OPEN, att(), IdempotencyKey::new())
                .await
                .expect_err("engine gate");
            assert_eq!(e, ApiError::AiUnavailable, "{body}");
        }
    }

    #[tokio::test]
    async fn a_failed_reading_keeps_its_code() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(format!("/api/v1/bills/extract-from-attachment/{ATT}")))
            .respond_with(json(
                400,
                r#"{"error":{"message":"could not read","code":"ai_ocr_failed"}}"#,
            ))
            .mount(&server)
            .await;
        let s = company_session(&server);
        let e = s
            .bill_reading()
            .extract(OPEN, att(), IdempotencyKey::new())
            .await
            .expect_err("failed");
        assert!(crate::has_code(&e, AI_OCR_FAILED), "{e:?}");

        // A scanned PDF: the engine's 422 keeps its upper-case code.
        for code in [PDF_RENDER_FAILED, PDF_RENDER_EMPTY] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path(format!("/api/v1/bills/extract-from-attachment/{ATT}")))
                .respond_with(json(
                    422,
                    &format!(r#"{{"error":{{"message":"Failed to render PDF","code":"{code}"}}}}"#),
                ))
                .mount(&server)
                .await;
            let s = company_session(&server);
            let e = s
                .bill_reading()
                .extract(OPEN, att(), IdempotencyKey::new())
                .await
                .expect_err("scanned");
            assert!(crate::has_code(&e, code), "{e:?}");
        }

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(format!("/api/v1/bills/extract-from-attachment/{ATT}")))
            .respond_with(json(200, PRE_V2))
            .mount(&server)
            .await;
        let s = company_session(&server);
        let r = s
            .bill_reading()
            .extract(OPEN, att(), IdempotencyKey::new())
            .await
            .expect("read");
        assert_eq!(r.vendor_invoice_number.as_deref(), Some("INV-55871"));
        assert_eq!(requests(&server, "POST").await.len(), 1);
    }
}
