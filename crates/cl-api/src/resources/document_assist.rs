//! A second reading of a document by the company's AI provider (A03 step
//! 21, engine U-92d, capability `document-assist-v1`;
//! `POST /api/v1/document-reading/assist`).
//!
//! The desktop has already read the document on this computer. It sends
//! the page text it read (and, only with the owner's consent, up to three
//! page pictures) and gets field suggestions back. The engine parses no
//! file and stores nothing.
//!
//! - Every amount in the answer is a value the model read: it is parsed
//!   exactly (whole cents, never rounded, as P17's reading) and is only a
//!   suggestion. The desktop keeps an AI amount only when a word on the page
//!   equals it (`cl_docread::ai_gate`, Q-A03j) and the owner accepts it
//!   (non-negotiable 4).
//! - Consent (Q-A03k): a cloud provider needs the company's
//!   `document_text_cloud` switch, pictures `document_images_cloud` too
//!   (403 [`CONSENT_REQUIRED`]) and the engine's cloud-vision setting (403
//!   [`CLOUD_VISION_DISABLED`]). A local Ollama needs neither switch, only
//!   the company's AI opt-in.
//! - The engine's limits are checked here first (nothing sent):
//!   [`ASSIST_MAX_TEXT_CHARS`], [`ASSIST_MAX_IMAGES`],
//!   [`ASSIST_MAX_IMAGE_BYTES`], PNG only, something to read.
//! - The document-level confidence is a JSON number: it is read from its
//!   JSON text into basis points, never as a float.
//! - `Debug` never shows the text, the pictures or a read value.

use std::collections::BTreeMap;
use std::fmt;

use base64::Engine as _;
use cl_core::{CalendarDate, Money, Quantity};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

use crate::client::{Call, Method};
use crate::errors::{ApiError, LocalValidation};
use crate::idempotency::IdempotencyKey;
use crate::resources::bill_ocr::{
    Confidence, GATE_CODES, ReadField, date_field, exact_decimal, money_field, quantity_field,
    raw_text,
};
use crate::resources::document_drafts::DraftKind;
use crate::resources::operations::DOCUMENT_READING_ASSIST;
use crate::session::{Session, json_body};
use crate::wire::Capabilities;

/// The most page-text characters one request carries (the engine's limit).
pub const ASSIST_MAX_TEXT_CHARS: usize = 60_000;
/// The most page pictures one request carries.
pub const ASSIST_MAX_IMAGES: usize = 3;
/// The page pictures together are at most this many bytes (2 MiB).
pub const ASSIST_MAX_IMAGE_BYTES: usize = 2 * 1024 * 1024;

/// 403: the company has not agreed to send this to an online service
/// (`details.task` names the switch), or its AI opt-in is missing or stale.
pub const CONSENT_REQUIRED: &str = "CONSENT_REQUIRED";
/// 403: page pictures never go online at the engine's privacy level.
pub const CLOUD_VISION_DISABLED: &str = "CLOUD_VISION_DISABLED";
/// 429: too many AI requests a minute.
pub const AI_RATE_LIMIT: &str = "AI_RATE_LIMIT";

/// The first bytes of every PNG file.
const PNG_MAGIC: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

/// True when `bytes` start like a PNG file.
#[must_use]
pub fn is_png(bytes: &[u8]) -> bool {
    bytes.starts_with(&PNG_MAGIC)
}

/// What to read. `Debug` shows sizes only.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct AssistRequest {
    /// The page text as the desktop read it.
    pub text: String,
    /// Page pictures (PNG files); empty unless the owner agreed to send
    /// pictures.
    pub page_images: Vec<Vec<u8>>,
    /// What the desktop thinks the document is (only steers the prompt).
    pub kind_hint: Option<DraftKind>,
}

impl fmt::Debug for AssistRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AssistRequest")
            .field("text_chars", &self.text.chars().count())
            .field("images", &self.page_images.len())
            .field("kind_hint", &self.kind_hint)
            .finish()
    }
}

impl AssistRequest {
    /// The engine's limits, checked before anything is sent.
    ///
    /// # Errors
    /// `Local(InvalidValue)` for text over [`ASSIST_MAX_TEXT_CHARS`], more
    /// than [`ASSIST_MAX_IMAGES`] pictures, pictures over
    /// [`ASSIST_MAX_IMAGE_BYTES`] together, a picture that is not a PNG, or
    /// nothing to read.
    pub fn check(&self) -> Result<(), ApiError> {
        let bad = Err(ApiError::Local(LocalValidation::InvalidValue));
        if self.text.chars().count() > ASSIST_MAX_TEXT_CHARS
            || self.page_images.len() > ASSIST_MAX_IMAGES
            || self.page_images.iter().map(Vec::len).sum::<usize>() > ASSIST_MAX_IMAGE_BYTES
            || self.page_images.iter().any(|p| !is_png(p))
            || (self.text.trim().is_empty() && self.page_images.is_empty())
        {
            return bad;
        }
        Ok(())
    }
}

/// Where the second reading ran.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssistReader {
    /// On this computer or network (Ollama).
    SelfHosted,
    /// An online provider, with the company's consent.
    Cloud,
}

/// One line the AI read.
#[derive(Clone, PartialEq, Eq)]
pub struct AssistLine {
    /// Description as printed.
    pub description: Option<String>,
    /// Amount, exact (`None` when absent or unreadable).
    pub amount: Option<Money>,
    /// Quantity, exact.
    pub quantity: Option<Quantity>,
    /// The printed rate per unit, exact.
    pub unit_price: Option<Money>,
    /// The line's confidence.
    pub confidence: Option<Confidence>,
}

impl fmt::Debug for AssistLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AssistLine")
            .field("amount", &self.amount.is_some())
            .field("quantity", &self.quantity.is_some())
            .field("unit_price", &self.unit_price.is_some())
            .field("confidence", &self.confidence)
            .finish_non_exhaustive()
    }
}

/// The AI's suggestions for a document. Every value is a suggestion: the
/// desktop's gate and the owner's accept decide.
#[derive(Clone, PartialEq, Eq)]
pub struct AssistSuggestion {
    /// Where it ran.
    pub reader: AssistReader,
    /// Page pictures were sent.
    pub used_images: bool,
    /// The vendor's name as printed.
    pub vendor: Option<String>,
    /// The vendor's invoice number.
    pub vendor_invoice_number: Option<String>,
    /// Bill date.
    pub bill_date: Option<CalendarDate>,
    /// Due date.
    pub due_date: Option<CalendarDate>,
    /// Terms as printed.
    pub payment_terms: Option<String>,
    /// Total.
    pub total: Option<Money>,
    /// Subtotal.
    pub subtotal: Option<Money>,
    /// Tax.
    pub tax: Option<Money>,
    /// Lines.
    pub lines: Vec<AssistLine>,
    /// The model's own document confidence in basis points (0-10000), read
    /// exactly from its JSON number.
    pub confidence_bp: Option<u16>,
    /// Per-field confidence by the engine's field name.
    pub field_confidence: BTreeMap<String, Confidence>,
    /// Fields sent but not readable as a date or an exact amount.
    pub unreadable: Vec<ReadField>,
}

impl fmt::Debug for AssistSuggestion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AssistSuggestion")
            .field("reader", &self.reader)
            .field("used_images", &self.used_images)
            .field("lines", &self.lines.len())
            .field("total", &self.total.is_some())
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
    unit_price: Option<Box<RawValue>>,
    #[serde(default)]
    confidence: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SuggestionWire {
    reader: String,
    #[serde(default)]
    used_images: bool,
    #[serde(default)]
    vendor: Option<String>,
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
    confidence: Option<Box<RawValue>>,
    #[serde(default)]
    field_confidence: Option<BTreeMap<String, String>>,
}

/// A 0-1 score as basis points, from its exact text (truncated, clamped).
fn basis_points(raw: Option<&RawValue>) -> Option<u16> {
    let d = exact_decimal(&raw_text(raw)?)?;
    let bp = (d * Decimal::from(10_000))
        .trunc()
        .max(Decimal::ZERO)
        .min(Decimal::from(10_000));
    i64::try_from(bp).ok().and_then(|v| u16::try_from(v).ok())
}

/// Parses the assist answer `{suggestion: {...}}`.
///
/// # Errors
/// `Decode` when the body is not that shape or names an unknown reader.
pub fn suggestion_from_json(body: &[u8]) -> Result<AssistSuggestion, ApiError> {
    #[derive(Deserialize)]
    struct W {
        suggestion: SuggestionWire,
    }
    const WHAT: &str = "document assist";
    let w: W = serde_json::from_slice(body).map_err(|_| ApiError::Decode { what: WHAT })?;
    let s = w.suggestion;
    let reader = match s.reader.as_str() {
        "self_hosted" => AssistReader::SelfHosted,
        "cloud" => AssistReader::Cloud,
        _ => return Err(ApiError::Decode { what: WHAT }),
    };
    let mut bad = Vec::new();
    let bill_date = date_field(s.bill_date.as_deref(), ReadField::BillDate, &mut bad);
    let due_date = date_field(s.due_date.as_deref(), ReadField::DueDate, &mut bad);
    let total = money_field(s.total.as_deref(), ReadField::Total, &mut bad);
    let subtotal = money_field(s.subtotal.as_deref(), ReadField::Subtotal, &mut bad);
    let tax = money_field(s.tax.as_deref(), ReadField::Tax, &mut bad);
    let lines = s
        .line_items
        .into_iter()
        .enumerate()
        .map(|(i, l)| {
            // An unreadable rate is simply no rate (the gate needs none).
            let mut ignored = Vec::new();
            AssistLine {
                description: l.description.filter(|d| !d.trim().is_empty()),
                amount: money_field(l.amount.as_deref(), ReadField::LineAmount(i), &mut bad),
                quantity: quantity_field(
                    l.quantity.as_deref(),
                    ReadField::LineQuantity(i),
                    &mut bad,
                ),
                unit_price: money_field(
                    l.unit_price.as_deref(),
                    ReadField::LineAmount(i),
                    &mut ignored,
                ),
                confidence: l.confidence.as_deref().and_then(Confidence::from_wire),
            }
        })
        .collect();
    Ok(AssistSuggestion {
        reader,
        used_images: s.used_images,
        vendor: s.vendor.filter(|v| !v.trim().is_empty()),
        vendor_invoice_number: s.vendor_invoice_number.filter(|v| !v.trim().is_empty()),
        bill_date,
        due_date,
        payment_terms: s.payment_terms,
        total,
        subtotal,
        tax,
        lines,
        confidence_bp: basis_points(s.confidence.as_deref()),
        field_confidence: s
            .field_confidence
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(k, v)| Confidence::from_wire(&v).map(|c| (k, c)))
            .collect(),
        unreadable: bad,
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Body<'a> {
    text: &'a str,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    page_images: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    kind_hint: Option<DraftKind>,
}

impl Session {
    /// Asks the company's AI provider for a second reading of `request`
    /// (U-92d). Nothing is stored or posted. Only on the owner's click.
    ///
    /// # Errors
    /// `Local(Unsupported)` without `document-assist-v1` and
    /// `Local(InvalidValue)` over the engine's limits (nothing sent in
    /// either case); `AiUnavailable` when AI or document reading is off in
    /// the engine; `Forbidden` with [`CONSENT_REQUIRED`] or
    /// [`CLOUD_VISION_DISABLED`]; `RateLimited`; `Validation` with
    /// `AI_OCR_FAILED` or `AI_PARSE_FAILED` when the provider could not
    /// read it; `OfflineUnavailable`; `Decode`; any [`ApiError`].
    pub async fn document_assist(
        &self,
        request: &AssistRequest,
        key: IdempotencyKey,
    ) -> Result<AssistSuggestion, ApiError> {
        if !self
            .client()
            .capabilities()
            .has(Capabilities::DOCUMENT_ASSIST)
        {
            return Err(ApiError::Local(LocalValidation::Unsupported));
        }
        request.check()?;
        let b64 = base64::engine::general_purpose::STANDARD;
        let body = Body {
            text: &request.text,
            page_images: request.page_images.iter().map(|p| b64.encode(p)).collect(),
            kind_hint: request.kind_hint,
        };
        let mut call =
            Call::at(Method::Post, DOCUMENT_READING_ASSIST.route()).json_body(json_body(&body)?);
        call.idempotency_key = Some(key);
        let reply = match self.send(&DOCUMENT_READING_ASSIST, call, None).await {
            Ok(r) => r,
            Err(ApiError::Validation(v))
                if v.code().is_some_and(|c| GATE_CODES.contains(&c.as_str())) =>
            {
                return Err(ApiError::AiUnavailable);
            }
            Err(e) => return Err(e),
        };
        suggestion_from_json(&reply.body)
    }
}

#[cfg(test)]
#[path = "document_assist_tests.rs"]
mod tests;
