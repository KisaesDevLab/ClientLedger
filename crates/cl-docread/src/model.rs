//! The reading's value types (**frozen** in A03 step 2; plan D3).
//!
//! Everything here crosses the worker pipe (inside [`crate::protocol`]) or
//! the desktop's reading service, so it is plain data with serde and no
//! behaviour beyond construction checks.
//!
//! Integers only (plan D3): positions are [`Bbox`] ten-thousandths of the
//! original page, confidence is basis points ([`Confidence`]), amounts are
//! [`Money`], quantities [`Quantity`], dates [`CalendarDate`].
//!
//! **Privacy (plan §4).** Document text, amounts, dates, card digits and
//! names are owner data: `Debug` on every type that carries them prints
//! only shapes (which fields are present, counts, rule ids, positions,
//! confidence), never values. A test checks that.
//!
//! **Change rule.** Later work packages may add a [`CheckCode`] variant or a
//! [`RuleId`] string without coordination; any other change to this file
//! goes through the orchestrator, because WP-A, WP-B and the desktop build
//! on it in parallel.

use cl_core::{CalendarDate, Money, Quantity};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::fmt;

// ---------------------------------------------------------------------------
// Caps the desktop re-checks on every reading (plan D2 "Host pipes").
// ---------------------------------------------------------------------------

/// Pages read per document (Q-A03p): the first ten.
pub const MAX_PAGES: u16 = 10;
/// Characters in one word.
pub const MAX_WORD_CHARS: usize = 256;
/// Words on one page.
pub const MAX_WORDS_PER_PAGE: usize = 20_000;
/// Characters in one extracted text value (vendor name, reference, ...).
pub const MAX_TEXT_CHARS: usize = 512;
/// Line items in one reading.
pub const MAX_LINES: usize = 500;
/// Checks in one reading.
pub const MAX_CHECKS: usize = 200;
/// Anchor tokens in one fingerprint.
pub const MAX_FINGERPRINT_TOKENS: usize = 400;
/// Field regions in one template.
pub const MAX_TEMPLATE_FIELDS: usize = 64;

// ---------------------------------------------------------------------------
// Positions and confidence
// ---------------------------------------------------------------------------

/// A box on the **original** page in ten-thousandths of the page's width
/// (`left`, `right`) and height (`top`, `bottom`), `0..=10000`, with
/// `left <= right` and `top <= bottom`. OCR boxes are mapped back through
/// the clean-up transform first (`ocr_geom`), so a box lands where the
/// viewer shows the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "BboxWire", into = "BboxWire")]
pub struct Bbox {
    left: u16,
    top: u16,
    right: u16,
    bottom: u16,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BboxWire {
    l: u16,
    t: u16,
    r: u16,
    b: u16,
}

impl TryFrom<BboxWire> for Bbox {
    type Error = ModelError;
    fn try_from(w: BboxWire) -> Result<Self, ModelError> {
        Bbox::new(w.l, w.t, w.r, w.b).ok_or(ModelError::Bbox)
    }
}

impl From<Bbox> for BboxWire {
    fn from(b: Bbox) -> Self {
        BboxWire {
            l: b.left,
            t: b.top,
            r: b.right,
            b: b.bottom,
        }
    }
}

impl Bbox {
    /// The full scale: one page width or height.
    pub const SCALE: u16 = 10_000;
    /// The whole page.
    pub const PAGE: Bbox = Bbox {
        left: 0,
        top: 0,
        right: Self::SCALE,
        bottom: Self::SCALE,
    };

    /// A box, or `None` when a side is past [`Bbox::SCALE`] or the sides are
    /// out of order.
    #[must_use]
    pub const fn new(left: u16, top: u16, right: u16, bottom: u16) -> Option<Bbox> {
        if right > Self::SCALE || bottom > Self::SCALE || left > right || top > bottom {
            return None;
        }
        Some(Bbox {
            left,
            top,
            right,
            bottom,
        })
    }

    /// Left edge.
    #[must_use]
    pub const fn left(self) -> u16 {
        self.left
    }
    /// Top edge.
    #[must_use]
    pub const fn top(self) -> u16 {
        self.top
    }
    /// Right edge.
    #[must_use]
    pub const fn right(self) -> u16 {
        self.right
    }
    /// Bottom edge.
    #[must_use]
    pub const fn bottom(self) -> u16 {
        self.bottom
    }
    /// Width in ten-thousandths of the page width.
    #[must_use]
    pub const fn width(self) -> u16 {
        self.right - self.left
    }
    /// Height in ten-thousandths of the page height.
    #[must_use]
    pub const fn height(self) -> u16 {
        self.bottom - self.top
    }

    /// The smallest box holding both.
    #[must_use]
    pub fn union(self, other: Bbox) -> Bbox {
        Bbox {
            left: self.left.min(other.left),
            top: self.top.min(other.top),
            right: self.right.max(other.right),
            bottom: self.bottom.max(other.bottom),
        }
    }

    /// Whether the boxes share any area (touching edges count).
    #[must_use]
    pub const fn overlaps(self, other: Bbox) -> bool {
        self.left <= other.right
            && other.left <= self.right
            && self.top <= other.bottom
            && other.top <= self.bottom
    }
}

/// Confidence in basis points, `0..=10000` (the engine's
/// `bucketConfidence` thresholds: high at 8500, medium at 6000).
/// The default is [`Confidence::NONE`].
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(try_from = "u16", into = "u16")]
pub struct Confidence(u16);

/// The owner-facing confidence tint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConfidenceBucket {
    /// At least [`Confidence::HIGH`].
    High,
    /// At least [`Confidence::MEDIUM`].
    Medium,
    /// Below [`Confidence::MEDIUM`].
    Low,
}

impl Confidence {
    /// Certain (text layer words, owner-confirmed values).
    pub const CERTAIN: Confidence = Confidence(10_000);
    /// The high bucket starts here (engine: 0.85).
    pub const HIGH: Confidence = Confidence(8_500);
    /// The medium bucket starts here (engine: 0.6).
    pub const MEDIUM: Confidence = Confidence(6_000);
    /// No confidence.
    pub const NONE: Confidence = Confidence(0);

    /// A confidence, or `None` above 10000.
    #[must_use]
    pub const fn new(bp: u16) -> Option<Confidence> {
        if bp > 10_000 {
            None
        } else {
            Some(Confidence(bp))
        }
    }

    /// Clamp to `0..=10000`.
    #[must_use]
    pub const fn saturating(bp: u32) -> Confidence {
        if bp > 10_000 {
            Confidence(10_000)
        } else {
            // In range: bp <= 10000 fits u16.
            #[allow(clippy::cast_possible_truncation)]
            Confidence(bp as u16)
        }
    }

    /// Basis points.
    #[must_use]
    pub const fn bp(self) -> u16 {
        self.0
    }

    /// The tint bucket.
    #[must_use]
    pub const fn bucket(self) -> ConfidenceBucket {
        if self.0 >= Self::HIGH.0 {
            ConfidenceBucket::High
        } else if self.0 >= Self::MEDIUM.0 {
            ConfidenceBucket::Medium
        } else {
            ConfidenceBucket::Low
        }
    }
}

impl TryFrom<u16> for Confidence {
    type Error = ModelError;
    fn try_from(bp: u16) -> Result<Self, ModelError> {
        Confidence::new(bp).ok_or(ModelError::Confidence)
    }
}

impl From<Confidence> for u16 {
    fn from(c: Confidence) -> u16 {
        c.0
    }
}

/// Where something is: a page (0-based, in document order) and a box on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spot {
    pub page: u16,
    pub bbox: Bbox,
}

// ---------------------------------------------------------------------------
// Words and pages
// ---------------------------------------------------------------------------

/// Where a page's words came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WordSource {
    /// The PDF's own text layer (`PDFium`).
    TextLayer,
    /// Tesseract on the cleaned-up page image.
    Ocr,
}

/// One word on a page.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Word {
    /// The word as printed (at most [`MAX_WORD_CHARS`] characters).
    pub text: String,
    pub bbox: Bbox,
    /// [`Confidence::CERTAIN`] for text-layer words.
    pub confidence: Confidence,
}

impl fmt::Debug for Word {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Word")
            .field("chars", &self.text.chars().count())
            .field("bbox", &self.bbox)
            .field("confidence", &self.confidence)
            .finish()
    }
}

/// One page's words, in reading order (top to bottom, left to right).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PageWords {
    /// 0-based page index in the document.
    pub page: u16,
    /// Height ÷ width × 10000 of the original page (letter portrait is
    /// 12941). Templates compare it.
    pub aspect: u32,
    pub source: WordSource,
    pub words: Vec<Word>,
}

impl fmt::Debug for PageWords {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PageWords")
            .field("page", &self.page)
            .field("aspect", &self.aspect)
            .field("source", &self.source)
            .field("words", &self.words.len())
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Extracted values
// ---------------------------------------------------------------------------

/// The name of the rule that produced a value or a check, such as
/// `"fields.total.label-right"` or `"template"`. Stable strings: A04 shows
/// them in "why this value" and the accuracy report groups by them.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RuleId(Cow<'static, str>);

impl RuleId {
    /// A rule id from a literal.
    #[must_use]
    pub const fn new(id: &'static str) -> RuleId {
        RuleId(Cow::Borrowed(id))
    }

    /// The id text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A value read from the page, where it was, which rule found it and how
/// sure the rule is.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Found<T> {
    pub value: T,
    pub spot: Spot,
    pub rule: RuleId,
    pub confidence: Confidence,
}

impl<T> fmt::Debug for Found<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Found")
            .field("value", &"<redacted>")
            .field("spot", &self.spot)
            .field("rule", &self.rule.as_str())
            .field("confidence", &self.confidence)
            .finish()
    }
}

/// The last four digits of a payment card (`"1234"`). Never logged.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Last4([u8; 4]);

impl Last4 {
    /// Four ASCII digits, or `None`.
    #[must_use]
    pub fn parse(s: &str) -> Option<Last4> {
        let b = s.as_bytes();
        if b.len() == 4 && b.iter().all(u8::is_ascii_digit) {
            Some(Last4([b[0], b[1], b[2], b[3]]))
        } else {
            None
        }
    }

    /// The digits.
    #[must_use]
    pub fn as_str(&self) -> &str {
        // Only ever built from four ASCII digits.
        std::str::from_utf8(&self.0).unwrap_or("")
    }
}

impl TryFrom<String> for Last4 {
    type Error = ModelError;
    fn try_from(s: String) -> Result<Self, ModelError> {
        Last4::parse(&s).ok_or(ModelError::Last4)
    }
}

impl From<Last4> for String {
    fn from(l: Last4) -> String {
        l.as_str().to_string()
    }
}

impl fmt::Debug for Last4 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Last4(<redacted>)")
    }
}

/// How the document says it was paid (receipts).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "camelCase", deny_unknown_fields)]
pub enum Payment {
    /// A card ending in these digits.
    Card { last4: Last4 },
    /// A check with this number.
    Check { number: String },
    /// Cash.
    Cash,
    /// Marked paid, method not printed.
    Paid,
}

impl fmt::Debug for Payment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Payment::Card { .. } => "Card",
            Payment::Check { .. } => "Check",
            Payment::Cash => "Cash",
            Payment::Paid => "Paid",
        })
    }
}

/// Payment terms as printed, and the days they mean when known
/// (`"Net 30"` → 30; `"Due on receipt"` → 0).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Terms {
    pub text: String,
    pub days: Option<u16>,
}

impl fmt::Debug for Terms {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Terms")
            .field("days_known", &self.days.is_some())
            .finish_non_exhaustive()
    }
}

/// The header fields (plan D3 `fields.rs`). `None` means "not on the
/// document" (or not found): the reader never invents a value.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct Header {
    /// The seller's name from the top block (not "Bill to"/"Ship to").
    pub vendor: Option<Found<String>>,
    /// The seller's address lines joined with `\n` (vendor matching).
    pub vendor_address: Option<Found<String>>,
    /// The seller's phone as printed (vendor matching).
    pub vendor_phone: Option<Found<String>>,
    /// "Account #"/"Customer #" the vendor gives the owner (strongest
    /// vendor-matching anchor, `contacts.vendor_account_number`).
    pub account_number: Option<Found<String>>,
    /// Invoice or receipt date.
    pub date: Option<Found<CalendarDate>>,
    /// Invoice number, receipt number or similar.
    pub reference: Option<Found<String>>,
    pub terms: Option<Found<Terms>>,
    pub due_date: Option<Found<CalendarDate>>,
    pub subtotal: Option<Found<Money>>,
    pub tax: Option<Found<Money>>,
    /// Labelled fees added to the total (shipping, delivery, surcharge).
    pub fees: Vec<Found<Money>>,
    pub total: Option<Found<Money>>,
    pub amount_paid: Option<Found<Money>>,
    /// "Balance due" when printed.
    pub balance_due: Option<Found<Money>>,
    pub payment: Option<Found<Payment>>,
}

impl fmt::Debug for Header {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut present: Vec<&str> = Vec::new();
        for (name, has) in [
            ("vendor", self.vendor.is_some()),
            ("vendorAddress", self.vendor_address.is_some()),
            ("vendorPhone", self.vendor_phone.is_some()),
            ("accountNumber", self.account_number.is_some()),
            ("date", self.date.is_some()),
            ("reference", self.reference.is_some()),
            ("terms", self.terms.is_some()),
            ("dueDate", self.due_date.is_some()),
            ("subtotal", self.subtotal.is_some()),
            ("tax", self.tax.is_some()),
            ("total", self.total.is_some()),
            ("amountPaid", self.amount_paid.is_some()),
            ("balanceDue", self.balance_due.is_some()),
            ("payment", self.payment.is_some()),
        ] {
            if has {
                present.push(name);
            }
        }
        f.debug_struct("Header")
            .field("present", &present)
            .field("fees", &self.fees.len())
            .finish()
    }
}

/// The header fields by name: checks, templates and the accuracy report
/// refer to fields with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FieldName {
    Vendor,
    VendorAddress,
    VendorPhone,
    AccountNumber,
    Date,
    Reference,
    Terms,
    DueDate,
    Subtotal,
    Tax,
    Fees,
    Total,
    AmountPaid,
    BalanceDue,
    Payment,
}

/// One line item. The printed amount is always kept; quantity and unit
/// price are dropped (with a check) when they don't multiply to it
/// (plan D3 `lines.rs`).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Line {
    pub description: Option<Found<String>>,
    pub quantity: Option<Found<Quantity>>,
    pub unit_price: Option<Found<Money>>,
    pub amount: Found<Money>,
}

impl fmt::Debug for Line {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Line")
            .field("description", &self.description.is_some())
            .field("quantity", &self.quantity.is_some())
            .field("unit_price", &self.unit_price.is_some())
            .field("amount", &self.amount)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Checks
// ---------------------------------------------------------------------------

/// What a check is about. Additive: a later work package may add a variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CheckCode {
    /// Line amounts don't add up to the subtotal.
    LinesVsSubtotal,
    /// Subtotal + tax (+ labelled fees) is not the total.
    SubtotalTaxVsTotal,
    /// Total − amount paid is not the balance due.
    TotalPaidVsBalance,
    /// The due date is not the date plus the terms.
    DueDateVsTerms,
    /// A line's quantity × unit price is not its amount (engine
    /// `LINE_AMOUNT_MISMATCH`); quantity and unit price were dropped.
    LineQuantityTimesPrice,
    /// Both date numbers are 12 or less: read as month/day, could be
    /// day/month.
    DateCouldBeDayFirst,
    /// Several unlabelled candidates; the totals picked this one (M7).
    PickedUsingTotals,
    /// The template and the rules found different valid values; the
    /// template's is used.
    ReadersDisagree,
    /// An optional reader (Ollama, cloud) gave a different value; an AI
    /// amount without an equal page word is never used (Q-A03j).
    AiDisagrees,
    /// No card account, or more than one, ends in the printed digits.
    PickTheCard,
    /// The line table could not be read with confidence: one total line
    /// to the vendor's default account (Q-A03m).
    LinesNotRead,
    /// A page could not be read (OCR timed out or failed).
    PageNotRead,
    /// The document has more pages than are read (Q-A03p).
    PagesSkipped,
    /// The kind (bill, card charge, check, cash expense) is unclear.
    KindUnclear,
}

/// A value shown in a check.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "camelCase")]
pub enum CheckValue {
    Money(Money),
    Date(CalendarDate),
    Text(String),
}

/// A problem the owner should look at. Checks never change a value
/// (AC3): they only point at it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Check {
    pub code: CheckCode,
    /// The rule that raised it.
    pub rule: RuleId,
    /// The header field it is about, if one.
    pub field: Option<FieldName>,
    /// The 0-based line item it is about, if one.
    pub line: Option<u16>,
    /// What the other numbers say it should be.
    pub expected: Option<CheckValue>,
    /// What the document says.
    pub found: Option<CheckValue>,
    /// `found − expected` for amounts.
    pub difference: Option<Money>,
    /// Where to look.
    pub spot: Option<Spot>,
}

impl fmt::Debug for Check {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Check")
            .field("code", &self.code)
            .field("rule", &self.rule.as_str())
            .field("field", &self.field)
            .field("line", &self.line)
            .field("spot", &self.spot)
            .finish_non_exhaustive()
    }
}

// ---------------------------------------------------------------------------
// The reading
// ---------------------------------------------------------------------------

/// What the document becomes (engine `document_drafts.kind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DocKind {
    Bill,
    CardCharge,
    Check,
    CashExpense,
    Unknown,
}

/// Which reader produced the reading (engine `document_drafts.reader`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Reader {
    /// Every page had a usable text layer.
    TextPdf,
    /// At least one page went through OCR.
    Ocr,
    /// A vendor layout template supplied the fields.
    Template,
}

/// Stable masked anchor tokens of a layout plus the page shape (plan D3
/// `templates.rs`): digit runs of 4+ masked, top 35 % of page 1.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fingerprint {
    /// Sorted, de-duplicated, at most [`MAX_FINGERPRINT_TOKENS`].
    pub tokens: Vec<String>,
    /// Page 1 height ÷ width × 10000.
    pub aspect: u32,
}

impl fmt::Debug for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Fingerprint")
            .field("tokens", &self.tokens.len())
            .field("aspect", &self.aspect)
            .finish()
    }
}

/// Where one field sits in a vendor's layout.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TemplateField {
    pub field: FieldName,
    pub page: u16,
    /// The region the value is in.
    pub region: Bbox,
    /// The label printed next to it, normalized, if any (an anchor).
    pub label: Option<String>,
}

impl fmt::Debug for TemplateField {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TemplateField")
            .field("field", &self.field)
            .field("page", &self.page)
            .field("region", &self.region)
            .field("label", &self.label.is_some())
            .finish()
    }
}

/// What a line-table column holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ColumnKind {
    Description,
    Quantity,
    UnitPrice,
    Amount,
    Other,
}

/// One column band of a line table, in page ten-thousandths.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ColumnBand {
    pub kind: ColumnKind,
    pub left: u16,
    pub right: u16,
}

/// A vendor's line table: the header row's region and its columns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LineTable {
    pub page: u16,
    pub header: Bbox,
    pub columns: Vec<ColumnBand>,
}

/// A vendor layout template (engine `vendor_layout_templates`, U-92):
/// stored as opaque JSON by the engine, shaped here. No contact id: the
/// desktop keeps the template-to-vendor map, and contacts never cross the
/// worker pipe (plan D2).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Template {
    pub fingerprint: Fingerprint,
    pub fields: Vec<TemplateField>,
    pub line_table: Option<LineTable>,
    /// Accepted documents merged into it.
    pub samples: u32,
}

impl fmt::Debug for Template {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Template")
            .field("fingerprint", &self.fingerprint)
            .field("fields", &self.fields.len())
            .field("line_table", &self.line_table.is_some())
            .field("samples", &self.samples)
            .finish()
    }
}

/// A page's outcome in the reading.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PageSummary {
    pub page: u16,
    /// `None` when the page could not be read (see the `PageNotRead` check).
    pub source: Option<WordSource>,
    pub words: u32,
}

/// Everything read from one document (`JobDone`). Values only; the desktop
/// matches vendor and card and runs the AI gate afterwards.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Reading {
    pub reader: Reader,
    pub kind: DocKind,
    pub kind_confidence: Confidence,
    pub header: Header,
    pub lines: Vec<Line>,
    pub checks: Vec<Check>,
    /// Pages in the document (may exceed [`MAX_PAGES`]).
    pub page_count: u16,
    pub pages: Vec<PageSummary>,
    /// The pages' words: the AI gate needs them (an AI amount must equal a
    /// page word) and so does template learning (A04).
    pub words: Vec<PageWords>,
    /// The layout's fingerprint, for template lookup and learning.
    pub fingerprint: Option<Fingerprint>,
    /// Index into the batch's templates of the one applied, if any.
    pub template: Option<u16>,
}

impl fmt::Debug for Reading {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Reading")
            .field("reader", &self.reader)
            .field("kind", &self.kind)
            .field("header", &self.header)
            .field("lines", &self.lines.len())
            .field("checks", &self.checks.len())
            .field("page_count", &self.page_count)
            .field("pages", &self.pages.len())
            .field("template", &self.template)
            .finish_non_exhaustive()
    }
}

impl Reading {
    /// An empty reading of `page_count` pages: no fields, kind unknown.
    #[must_use]
    pub fn empty(reader: Reader, page_count: u16) -> Reading {
        Reading {
            reader,
            kind: DocKind::Unknown,
            kind_confidence: Confidence::NONE,
            header: Header::default(),
            lines: Vec::new(),
            checks: Vec::new(),
            page_count,
            pages: Vec::new(),
            words: Vec::new(),
            fingerprint: None,
            template: None,
        }
    }

    /// The caps the desktop re-checks on every reading from the worker
    /// (plan D2 "Host pipes"); boxes, confidence and amounts are already
    /// checked while parsing.
    ///
    /// # Errors
    /// The first cap exceeded.
    pub fn validate(&self, templates_sent: usize) -> Result<(), ModelError> {
        let pages = usize::from(MAX_PAGES);
        if self.pages.len() > pages || self.words.len() > pages {
            return Err(ModelError::TooMany("pages"));
        }
        if self.lines.len() > MAX_LINES {
            return Err(ModelError::TooMany("lines"));
        }
        if self.checks.len() > MAX_CHECKS {
            return Err(ModelError::TooMany("checks"));
        }
        for p in &self.words {
            if p.page >= self.page_count || p.words.len() > MAX_WORDS_PER_PAGE {
                return Err(ModelError::TooMany("words"));
            }
            if p.words
                .iter()
                .any(|w| w.text.chars().count() > MAX_WORD_CHARS)
            {
                return Err(ModelError::TooLong("word"));
            }
        }
        if let Some(fp) = &self.fingerprint
            && (fp.tokens.len() > MAX_FINGERPRINT_TOKENS
                || fp.tokens.iter().any(|t| t.chars().count() > MAX_WORD_CHARS))
        {
            return Err(ModelError::TooMany("fingerprint tokens"));
        }
        if self
            .template
            .is_some_and(|t| usize::from(t) >= templates_sent)
        {
            return Err(ModelError::Template);
        }
        let h = &self.header;
        let texts = [
            h.vendor.as_ref().map(|f| f.value.as_str()),
            h.vendor_address.as_ref().map(|f| f.value.as_str()),
            h.vendor_phone.as_ref().map(|f| f.value.as_str()),
            h.account_number.as_ref().map(|f| f.value.as_str()),
            h.reference.as_ref().map(|f| f.value.as_str()),
            h.terms.as_ref().map(|f| f.value.text.as_str()),
        ];
        let line_texts = self
            .lines
            .iter()
            .filter_map(|l| l.description.as_ref().map(|d| d.value.as_str()));
        if texts
            .into_iter()
            .flatten()
            .chain(line_texts)
            .any(|t| t.chars().count() > MAX_TEXT_CHARS)
        {
            return Err(ModelError::TooLong("text"));
        }
        let spot_pages = self
            .checks
            .iter()
            .filter_map(|c| c.spot)
            .chain(self.lines.iter().map(|l| l.amount.spot));
        if spot_pages.into_iter().any(|s| s.page >= self.page_count) {
            return Err(ModelError::Spot);
        }
        Ok(())
    }
}

/// A value outside the model's rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ModelError {
    #[error("a box is outside the page or its sides are out of order")]
    Bbox,
    #[error("a confidence is above 10000 basis points")]
    Confidence,
    #[error("card digits must be exactly four digits")]
    Last4,
    #[error("too many {0}")]
    TooMany(&'static str),
    #[error("a {0} is too long")]
    TooLong(&'static str),
    #[error("a template index is outside the batch")]
    Template,
    #[error("a spot names a page the document does not have")]
    Spot,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spot() -> Spot {
        Spot {
            page: 0,
            bbox: Bbox::new(100, 200, 300, 400).unwrap(),
        }
    }

    fn found<T>(value: T) -> Found<T> {
        Found {
            value,
            spot: spot(),
            rule: RuleId::new("test.rule"),
            confidence: Confidence::HIGH,
        }
    }

    fn sample() -> Reading {
        let mut r = Reading::empty(Reader::TextPdf, 1);
        r.header.vendor = Some(found("Riverbend Supply Co.".to_string()));
        r.header.reference = Some(found("INV-20417".to_string()));
        r.header.total = Some(found(Money::parse_wire("544.82").unwrap()));
        r.header.date = Some(found(CalendarDate::from_ymd(2026, 7, 14).unwrap()));
        r.header.payment = Some(found(Payment::Card {
            last4: Last4::parse("4821").unwrap(),
        }));
        r.lines.push(Line {
            description: Some(found("Canvas tarp".to_string())),
            quantity: Some(found(Quantity::parse_wire("12").unwrap())),
            unit_price: Some(found(Money::parse_wire("24.50").unwrap())),
            amount: found(Money::parse_wire("294.00").unwrap()),
        });
        r.checks.push(Check {
            code: CheckCode::SubtotalTaxVsTotal,
            rule: RuleId::new("validate.total"),
            field: Some(FieldName::Total),
            line: None,
            expected: Some(CheckValue::Money(Money::parse_wire("544.81").unwrap())),
            found: Some(CheckValue::Text("Ozark Ridge".into())),
            difference: Some(Money::parse_wire("0.01").unwrap()),
            spot: Some(spot()),
        });
        r.words.push(PageWords {
            page: 0,
            aspect: 12_941,
            source: WordSource::TextLayer,
            words: vec![Word {
                text: "Riverbend".into(),
                bbox: spot().bbox,
                confidence: Confidence::CERTAIN,
            }],
        });
        r.fingerprint = Some(Fingerprint {
            tokens: vec!["invoice".into(), "riverbend".into()],
            aspect: 12_941,
        });
        r
    }

    #[test]
    fn debug_never_prints_owner_data() {
        let text = format!("{:?}", sample());
        for secret in [
            "Riverbend",
            "INV-20417",
            "544.8",
            "294",
            "24.5",
            "2026",
            "4821",
            "Ozark",
            "Canvas",
            "0.01",
        ] {
            assert!(!text.contains(secret), "{secret} leaked: {text}");
        }
        assert!(
            text.contains("Reading") && text.contains("vendor"),
            "{text}"
        );
    }

    #[test]
    fn reading_round_trips_through_json_with_amounts_as_strings() {
        let r = sample();
        let json = serde_json::to_string(&r).unwrap();
        assert!(
            json.contains("\"544.8200\"") || json.contains("\"544.82\""),
            "{json}"
        );
        let back: Reading = serde_json::from_str(&json).unwrap();
        assert_eq!(back, r);
        back.validate(0).unwrap();
    }

    #[test]
    fn boxes_and_confidence_are_checked_when_parsing() {
        assert!(Bbox::new(0, 0, 10_001, 5).is_none());
        assert!(Bbox::new(5, 0, 4, 5).is_none());
        assert!(serde_json::from_str::<Bbox>(r#"{"l":0,"t":0,"r":10001,"b":1}"#).is_err());
        assert!(serde_json::from_str::<Bbox>(r#"{"l":9,"t":0,"r":1,"b":1}"#).is_err());
        assert!(serde_json::from_str::<Confidence>("10001").is_err());
        assert_eq!(
            serde_json::from_str::<Confidence>("8500").unwrap(),
            Confidence::HIGH
        );
        assert!(serde_json::from_str::<Last4>(r#""12a4""#).is_err());
        assert!(
            serde_json::from_str::<Money>("12.5").is_err(),
            "amounts are strings"
        );
    }

    #[test]
    fn confidence_buckets_match_the_engine() {
        assert_eq!(
            Confidence::new(8_500).unwrap().bucket(),
            ConfidenceBucket::High
        );
        assert_eq!(
            Confidence::new(8_499).unwrap().bucket(),
            ConfidenceBucket::Medium
        );
        assert_eq!(
            Confidence::new(6_000).unwrap().bucket(),
            ConfidenceBucket::Medium
        );
        assert_eq!(
            Confidence::new(5_999).unwrap().bucket(),
            ConfidenceBucket::Low
        );
        assert_eq!(Confidence::saturating(70_000), Confidence::CERTAIN);
    }

    #[test]
    fn validate_enforces_the_caps() {
        let mut r = sample();
        r.header.vendor = Some(found("x".repeat(MAX_TEXT_CHARS + 1)));
        assert_eq!(r.validate(0), Err(ModelError::TooLong("text")));
        let mut r = sample();
        r.template = Some(3);
        assert_eq!(r.validate(3), Err(ModelError::Template));
        assert_eq!(r.validate(4), Ok(()));
        let mut r = sample();
        r.checks[0].spot = Some(Spot {
            page: 1,
            bbox: Bbox::PAGE,
        });
        assert_eq!(r.validate(0), Err(ModelError::Spot));
        let mut r = sample();
        r.lines = vec![r.lines[0].clone(); MAX_LINES + 1];
        assert_eq!(r.validate(0), Err(ModelError::TooMany("lines")));
    }

    #[test]
    fn bbox_geometry() {
        let a = Bbox::new(0, 0, 10, 10).unwrap();
        let b = Bbox::new(20, 5, 30, 40).unwrap();
        assert_eq!(a.union(b), Bbox::new(0, 0, 30, 40).unwrap());
        assert!(!a.overlaps(b));
        assert!(a.overlaps(Bbox::new(10, 10, 12, 12).unwrap()));
        assert_eq!((b.width(), b.height()), (10, 35));
    }
}
