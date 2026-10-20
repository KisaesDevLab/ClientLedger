//! Daily Sales Summary (P14 lane B): the engine's daily-sales layouts
//! (`/daily-sales/templates`) and days (`/daily-sales/entries`, preview,
//! post and void; `closingPassword` with `daily-sales-lock-v1`).
//!
//! Amounts go out as `Money::to_wire()` strings (the engine refuses a JSON
//! number) and come back as exact `Money`; the engine computes the posting
//! and its over/short, the app only compares. Layouts and days are
//! company-scoped (`X-Company-Id`): another company's are a 404 and are left
//! out of lists.
//!
//! Local refusals (nothing is sent): more than [`MAX_DAILY_VALUES`] values or
//! [`MAX_DAILY_LINES`] layout lines (`TooManyLines`), a negative value or a
//! line twice (`InvalidValue`, Q-P14t), an empty or over-long label or a
//! section the engine does not know (`InvalidValue`), and a closing-date
//! password before the engine advertises `daily-sales-lock-v1`
//! (`DailySalesLockUnavailable`).

use std::collections::BTreeSet;
use std::fmt;

use cl_core::ids::{AccountId, DailyEntryId, DailyLineId, DailyTemplateId, TagId, TransactionId};
use cl_core::{CalendarDate, Money};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Value, json};

use crate::client::{Call, Method};
use crate::errors::{ApiError, LocalValidation};
use crate::idempotency::IdempotencyKey;
use crate::resources::operations::{
    DAILY_ENTRIES, DAILY_ENTRIES_CREATE, DAILY_ENTRY_DELETE, DAILY_ENTRY_GET, DAILY_ENTRY_POST,
    DAILY_ENTRY_PREVIEW, DAILY_ENTRY_PUT, DAILY_ENTRY_VOID, DAILY_TEMPLATE_DELETE,
    DAILY_TEMPLATE_GET, DAILY_TEMPLATE_LINES, DAILY_TEMPLATE_PUT, DAILY_TEMPLATES,
    DAILY_TEMPLATES_CREATE,
};
use crate::resources::posting::{ClosedPeriodAuthorization, PostingOptions, posting_body};
use crate::resources::versioning::saved_unreadable;
use crate::session::{Session, decode, json_body};
use crate::wire::Capabilities;

/// Most values one day takes (the engine's schema).
pub const MAX_DAILY_VALUES: usize = 200;
/// Most lines one layout takes (the engine's schema).
pub const MAX_DAILY_LINES: usize = 200;
/// Longest line label, in characters (the engine's `varchar(120)`).
pub const DAILY_LABEL_MAX_CHARS: usize = 120;
/// Longest layout name, in characters.
pub const DAILY_NAME_MAX_CHARS: usize = 255;
/// Longest note on a day, in characters.
pub const DAILY_NOTES_MAX_CHARS: usize = 2000;

/// Where a daily-sales line sits on the register report.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Section {
    /// Sales by department.
    Sales,
    /// Sales tax collected (Q-260).
    Tax,
    /// Tips owed to staff.
    Tips,
    /// Discounts, comps and returns.
    Discount,
    /// Tenders: cash, cards (to Payments Clearing).
    Payment,
    /// Paid out of the drawer.
    Payout,
    /// Anything else (gift cards sold, …).
    Other,
    /// A section a newer engine sent; shown, never sent back.
    Unknown(String),
}

impl Section {
    /// The section from the engine's word.
    #[must_use]
    pub fn from_wire(s: &str) -> Section {
        match s {
            "sales" => Section::Sales,
            "tax" => Section::Tax,
            "tips" => Section::Tips,
            "discount" => Section::Discount,
            "payment" => Section::Payment,
            "payout" => Section::Payout,
            "other" => Section::Other,
            other => Section::Unknown(other.to_string()),
        }
    }

    /// The engine's word (`None` for an unknown section).
    #[must_use]
    pub fn as_wire(&self) -> Option<&'static str> {
        Some(match self {
            Section::Sales => "sales",
            Section::Tax => "tax",
            Section::Tips => "tips",
            Section::Discount => "discount",
            Section::Payment => "payment",
            Section::Payout => "payout",
            Section::Other => "other",
            Section::Unknown(_) => return None,
        })
    }
}

impl<'de> Deserialize<'de> for Section {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Section, D::Error> {
        String::deserialize(d).map(|s| Section::from_wire(&s))
    }
}

/// Which side of the entry a line's amount posts to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum NormalSide {
    /// Debit (tenders, discounts, paid-outs).
    Debit,
    /// Credit (sales, tax, tips, other).
    Credit,
}

/// The engine's starting layouts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Preset {
    /// Retail: departments, returns, tax, cash, cards, other tender.
    Retail,
    /// Restaurant / bar: food, beverage, comps, tax, tips, tenders, gift cards.
    Restaurant,
    /// Blank.
    Custom,
}

impl Preset {
    /// The engine's word.
    #[must_use]
    pub const fn as_wire(self) -> &'static str {
        match self {
            Preset::Retail => "retail",
            Preset::Restaurant => "restaurant",
            Preset::Custom => "custom",
        }
    }
}

fn preset_de<'de, D: Deserializer<'de>>(d: D) -> Result<Preset, D::Error> {
    let s = Option::<String>::deserialize(d)?;
    Ok(match s.as_deref() {
        Some("retail") => Preset::Retail,
        Some("restaurant") => Preset::Restaurant,
        _ => Preset::Custom,
    })
}

fn true_if_null<'de, D: Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    Ok(Option::<bool>::deserialize(d)?.unwrap_or(true))
}

fn false_if_null<'de, D: Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    Ok(Option::<bool>::deserialize(d)?.unwrap_or(false))
}

fn sort_de<'de, D: Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
    // An integer column; never money.
    Ok(Option::<u32>::deserialize(d)?.unwrap_or(0))
}

fn money_or_zero<'de, D: Deserializer<'de>>(d: D) -> Result<Money, D::Error> {
    Ok(Option::<Money>::deserialize(d)?.unwrap_or(Money::ZERO))
}

/// One line of a layout.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyLine {
    /// The line.
    pub id: DailyLineId,
    /// Where it sits on the report.
    pub section: Section,
    /// The owner's label ("Cash", "Plant sales").
    pub label: String,
    /// The account it posts to (`None` until mapped).
    #[serde(rename = "accountId", default)]
    pub account: Option<AccountId>,
    /// The line's own tag.
    #[serde(rename = "tagId", default)]
    pub tag: Option<TagId>,
    /// Debit or credit.
    #[serde(rename = "normalSide")]
    pub side: NormalSide,
    /// The owner must fill it in.
    #[serde(rename = "isRequired", default, deserialize_with = "false_if_null")]
    pub required: bool,
    /// The value may carry its own tag.
    #[serde(default, deserialize_with = "false_if_null")]
    pub allow_tag: bool,
    /// False for a removed line (kept for old days).
    #[serde(
        rename = "isActive",
        default = "yes",
        deserialize_with = "true_if_null"
    )]
    pub active: bool,
    /// Order on the form.
    #[serde(rename = "sortOrder", default, deserialize_with = "sort_de")]
    pub sort: u32,
}

const fn yes() -> bool {
    true
}

/// Hides the label (the owner's words).
impl fmt::Debug for DailyLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DailyLine")
            .field("id", &self.id)
            .field("section", &self.section)
            .field("account", &self.account)
            .field("side", &self.side)
            .field("active", &self.active)
            .finish_non_exhaustive()
    }
}

/// A daily-sales layout. The list ([`DailySales::templates`]) leaves
/// `lines` empty; [`DailySales::template`] fills them, in form order.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyTemplate {
    /// The layout.
    pub id: DailyTemplateId,
    /// The owner's name for it.
    pub name: String,
    /// The preset it started from.
    #[serde(
        rename = "presetType",
        default = "custom",
        deserialize_with = "preset_de"
    )]
    pub preset: Preset,
    /// False once deleted.
    #[serde(default = "yes", deserialize_with = "true_if_null")]
    pub is_active: bool,
    /// The tag a day takes when it has none.
    #[serde(rename = "defaultTagId", default)]
    pub default_tag: Option<TagId>,
    /// Its lines (active and removed), by sort order.
    #[serde(default)]
    pub lines: Vec<DailyLine>,
}

const fn custom() -> Preset {
    Preset::Custom
}

/// Hides the name.
impl fmt::Debug for DailyTemplate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DailyTemplate")
            .field("id", &self.id)
            .field("preset", &self.preset)
            .field("is_active", &self.is_active)
            .field("lines", &self.lines)
            .finish_non_exhaustive()
    }
}

/// One layout line to save ([`DailySales::replace_lines`]). A line without
/// an id is new; a saved line left out is removed (kept for old days).
#[derive(Clone, PartialEq, Eq)]
pub struct DailyLineInput {
    /// The saved line, or `None` for a new one.
    pub id: Option<DailyLineId>,
    /// Where it sits (an unknown section is refused locally).
    pub section: Section,
    /// The label (1 to 120 characters).
    pub label: String,
    /// The account it posts to.
    pub account: Option<AccountId>,
    /// The line's own tag.
    pub tag: Option<TagId>,
    /// Debit or credit.
    pub side: NormalSide,
    /// The owner must fill it in.
    pub required: bool,
    /// Active (false removes it but keeps it for old days).
    pub active: bool,
}

/// Hides the label.
impl fmt::Debug for DailyLineInput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DailyLineInput")
            .field("id", &self.id)
            .field("section", &self.section)
            .field("account", &self.account)
            .field("side", &self.side)
            .finish_non_exhaustive()
    }
}

impl From<&DailyLine> for DailyLineInput {
    fn from(l: &DailyLine) -> DailyLineInput {
        DailyLineInput {
            id: Some(l.id),
            section: l.section.clone(),
            label: l.label.clone(),
            account: l.account,
            tag: l.tag,
            side: l.side,
            required: l.required,
            active: l.active,
        }
    }
}

/// Status of a day.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum EntryStatus {
    /// Saved but not posted (a failed post, Q-P14u).
    Draft,
    /// Posted to the books.
    Posted,
    /// Voided.
    Void,
    /// A status a newer engine sent.
    Unknown(String),
}

impl EntryStatus {
    /// The engine's word (`None` for an unknown status).
    #[must_use]
    pub fn as_wire(&self) -> Option<&'static str> {
        Some(match self {
            EntryStatus::Draft => "draft",
            EntryStatus::Posted => "posted",
            EntryStatus::Void => "void",
            EntryStatus::Unknown(_) => return None,
        })
    }
}

impl<'de> Deserialize<'de> for EntryStatus {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<EntryStatus, D::Error> {
        let s = String::deserialize(d)?;
        Ok(match s.as_str() {
            "draft" => EntryStatus::Draft,
            "posted" => EntryStatus::Posted,
            "void" => EntryStatus::Void,
            _ => EntryStatus::Unknown(s),
        })
    }
}

/// One amount of a day.
#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyValue {
    /// The layout line.
    #[serde(rename = "templateLineId")]
    pub line: DailyLineId,
    /// The amount (an exact string both ways; never negative going out).
    pub amount: Money,
    /// The value's own tag.
    #[serde(rename = "tagId", default)]
    pub tag: Option<TagId>,
}

/// Hides the amount.
impl fmt::Debug for DailyValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DailyValue")
            .field("line", &self.line)
            .finish_non_exhaustive()
    }
}

/// A day to save as a draft ([`DailySales::create_draft`],
/// [`DailySales::update_draft`]).
#[derive(Clone, PartialEq, Eq)]
pub struct DailyEntryDraft {
    /// The layout.
    pub template: DailyTemplateId,
    /// The business day.
    pub date: CalendarDate,
    /// The day's tag.
    pub tag: Option<TagId>,
    /// The owner's note.
    pub notes: Option<String>,
    /// The amounts (zero values may be left out).
    pub values: Vec<DailyValue>,
}

/// Hides the notes and amounts.
impl fmt::Debug for DailyEntryDraft {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DailyEntryDraft")
            .field("template", &self.template)
            .field("date", &self.date)
            .field("tag", &self.tag)
            .field("values", &self.values.len())
            .finish_non_exhaustive()
    }
}

/// The engine's totals of a day.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub struct DailyTotals {
    /// Sales lines.
    pub sales: Money,
    /// Sales tax lines.
    pub tax: Money,
    /// Tenders.
    pub payments: Money,
}

/// A saved day with its amounts.
#[derive(Clone, PartialEq, Eq)]
pub struct DailyEntry {
    /// The day.
    pub id: DailyEntryId,
    /// Its layout.
    pub template: DailyTemplateId,
    /// The business day.
    pub date: CalendarDate,
    /// Draft, posted or void.
    pub status: EntryStatus,
    /// The posted journal entry.
    pub transaction: Option<TransactionId>,
    /// The day's tag.
    pub tag: Option<TagId>,
    /// The owner's note.
    pub notes: Option<String>,
    /// Its amounts.
    pub values: Vec<DailyValue>,
    /// Debits minus credits as posted (positive = over; zero until posted).
    pub over_short: Money,
    /// The engine's totals (zero until posted).
    pub totals: DailyTotals,
    /// The layout as the engine read it with the day.
    pub layout: Option<DailyTemplate>,
}

/// Hides the notes and amounts.
impl fmt::Debug for DailyEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DailyEntry")
            .field("id", &self.id)
            .field("template", &self.template)
            .field("date", &self.date)
            .field("status", &self.status)
            .field("transaction", &self.transaction)
            .finish_non_exhaustive()
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EntryWire {
    id: DailyEntryId,
    template_id: DailyTemplateId,
    business_date: CalendarDate,
    status: EntryStatus,
    #[serde(default)]
    transaction_id: Option<TransactionId>,
    #[serde(default)]
    tag_id: Option<TagId>,
    #[serde(default)]
    notes: Option<String>,
    #[serde(default)]
    values: Vec<DailyValue>,
    #[serde(default, deserialize_with = "money_or_zero")]
    over_short_amount: Money,
    #[serde(default, deserialize_with = "money_or_zero")]
    total_sales: Money,
    #[serde(default, deserialize_with = "money_or_zero")]
    total_tax: Money,
    #[serde(default, deserialize_with = "money_or_zero")]
    total_payments: Money,
    #[serde(default)]
    template: Option<DailyTemplate>,
}

impl From<EntryWire> for DailyEntry {
    fn from(w: EntryWire) -> DailyEntry {
        DailyEntry {
            id: w.id,
            template: w.template_id,
            date: w.business_date,
            status: w.status,
            transaction: w.transaction_id,
            tag: w.tag_id,
            notes: w.notes,
            values: w.values,
            over_short: w.over_short_amount,
            totals: DailyTotals {
                sales: w.total_sales,
                tax: w.total_tax,
                payments: w.total_payments,
            },
            layout: w.template,
        }
    }
}

/// One row of the days list.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyEntrySummary {
    /// The day.
    pub id: DailyEntryId,
    /// Its layout.
    #[serde(rename = "templateId")]
    pub template: DailyTemplateId,
    /// The layout's name.
    #[serde(rename = "templateName", default)]
    pub template_name: Option<String>,
    /// The business day.
    #[serde(rename = "businessDate")]
    pub date: CalendarDate,
    /// Draft, posted or void.
    pub status: EntryStatus,
    /// The posted journal entry.
    #[serde(rename = "transactionId", default)]
    pub transaction: Option<TransactionId>,
    /// Debits minus credits as posted (positive = over).
    #[serde(
        rename = "overShortAmount",
        default,
        deserialize_with = "money_or_zero"
    )]
    pub over_short: Money,
    /// Sales as posted.
    #[serde(rename = "totalSales", default, deserialize_with = "money_or_zero")]
    pub sales: Money,
}

/// Hides the layout name and amounts.
impl fmt::Debug for DailyEntrySummary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DailyEntrySummary")
            .field("id", &self.id)
            .field("date", &self.date)
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}

/// Which days to list.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DailyEntryQuery {
    /// Only this status.
    pub status: Option<EntryStatus>,
    /// Only this layout.
    pub template: Option<DailyTemplateId>,
    /// Only this business day (filtered here: the engine ignores dates).
    pub date: Option<CalendarDate>,
    /// At most this many (1 to 500; the engine's default is 100).
    pub limit: Option<u16>,
}

/// The engine's live figures for unsaved values (no writes).
#[derive(Clone, PartialEq, Eq)]
pub struct DailyPreview {
    /// Debit-side total.
    pub debits: Money,
    /// Credit-side total.
    pub credits: Money,
    /// Debits minus credits: positive = over, negative = short.
    pub over_short: Money,
    /// Within the engine's tolerance.
    pub balanced: bool,
    /// Sales lines.
    pub sales: Money,
    /// Tax lines.
    pub tax: Money,
    /// Tenders.
    pub payments: Money,
    /// Labels of lines with an amount but no account.
    pub unmapped: Vec<String>,
}

/// Hides the amounts and labels.
impl fmt::Debug for DailyPreview {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DailyPreview")
            .field("balanced", &self.balanced)
            .field("unmapped", &self.unmapped.len())
            .finish_non_exhaustive()
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PreviewWire {
    total_debits: Money,
    total_credits: Money,
    over_short: Money,
    #[serde(default)]
    balanced: bool,
    #[serde(default, deserialize_with = "money_or_zero")]
    total_sales: Money,
    #[serde(default, deserialize_with = "money_or_zero")]
    total_tax: Money,
    #[serde(default, deserialize_with = "money_or_zero")]
    total_payments: Money,
    #[serde(default)]
    unmapped_labels: Vec<String>,
}

/// Options of a post or a void.
#[derive(Clone, Debug, Default)]
pub struct DailyPostOptions {
    /// The closing-date password for a day on or before the closing date
    /// (needs `daily-sales-lock-v1`).
    pub closed_period: Option<ClosedPeriodAuthorization>,
}

/// Parses a daily-sales layout answer (`{template}`), a list
/// (`{templates}`), a day (`{entry}`), a days list (`{entries}`) or a
/// preview; used by the demo twins' tests and the golden fixtures.
///
/// # Errors
/// `Decode` when the body is not one.
pub fn template_from_json(body: &[u8]) -> Result<DailyTemplate, ApiError> {
    #[derive(Deserialize)]
    struct W {
        template: DailyTemplate,
    }
    serde_json::from_slice::<W>(body)
        .map(|w| w.template)
        .map_err(|_| ApiError::Decode {
            what: "daily-sales layout",
        })
}

/// See [`template_from_json`].
///
/// # Errors
/// `Decode`.
pub fn entry_from_json(body: &[u8]) -> Result<DailyEntry, ApiError> {
    #[derive(Deserialize)]
    struct W {
        entry: EntryWire,
    }
    serde_json::from_slice::<W>(body)
        .map(|w| w.entry.into())
        .map_err(|_| ApiError::Decode {
            what: "daily-sales day",
        })
}

/// See [`template_from_json`].
///
/// # Errors
/// `Decode`.
pub fn preview_from_json(body: &[u8]) -> Result<DailyPreview, ApiError> {
    let w: PreviewWire = serde_json::from_slice(body).map_err(|_| ApiError::Decode {
        what: "daily-sales preview",
    })?;
    Ok(DailyPreview {
        debits: w.total_debits,
        credits: w.total_credits,
        over_short: w.over_short,
        balanced: w.balanced,
        sales: w.total_sales,
        tax: w.total_tax,
        payments: w.total_payments,
        unmapped: w.unmapped_labels,
    })
}

fn local(v: LocalValidation) -> ApiError {
    ApiError::Local(v)
}

/// The `values` array of a draft or preview, checked: at most 200, each
/// line once, none negative. Amounts are `to_wire()` strings.
fn values_json(values: &[DailyValue]) -> Result<Value, ApiError> {
    if values.len() > MAX_DAILY_VALUES {
        return Err(local(LocalValidation::TooManyLines));
    }
    let mut seen = BTreeSet::new();
    for v in values {
        if v.amount.is_negative() || !seen.insert(v.line) {
            return Err(local(LocalValidation::InvalidValue));
        }
    }
    Ok(Value::Array(
        values
            .iter()
            .map(|v| {
                json!({
                    "templateLineId": v.line.to_string(),
                    "amount": v.amount.to_wire(),
                    "tagId": v.tag.map(|t| t.to_string()),
                })
            })
            .collect(),
    ))
}

/// The body of a new or replaced draft.
pub(crate) fn draft_body(d: &DailyEntryDraft, create: bool) -> Result<Value, ApiError> {
    if d.notes
        .as_ref()
        .is_some_and(|n| n.chars().count() > DAILY_NOTES_MAX_CHARS)
    {
        return Err(local(LocalValidation::InvalidValue));
    }
    let mut body = json!({
        "businessDate": d.date.to_wire(),
        "tagId": d.tag.map(|t| t.to_string()),
        "notes": d.notes,
        "values": values_json(&d.values)?,
    });
    if create && let Value::Object(map) = &mut body {
        map.insert("templateId".into(), json!(d.template.to_string()));
    }
    Ok(body)
}

/// The body of a layout's lines.
pub(crate) fn lines_body(lines: &[DailyLineInput]) -> Result<Value, ApiError> {
    if lines.len() > MAX_DAILY_LINES {
        return Err(local(LocalValidation::TooManyLines));
    }
    let mut out = Vec::with_capacity(lines.len());
    for (i, l) in lines.iter().enumerate() {
        let label = l.label.trim();
        let n = label.chars().count();
        let Some(section) = l.section.as_wire() else {
            return Err(local(LocalValidation::InvalidValue));
        };
        if n == 0 || n > DAILY_LABEL_MAX_CHARS {
            return Err(local(LocalValidation::InvalidValue));
        }
        let mut line = json!({
            "section": section,
            "label": label,
            "accountId": l.account.map(|a| a.to_string()),
            "tagId": l.tag.map(|t| t.to_string()),
            "normalSide": l.side,
            "sortOrder": i,
            "isRequired": l.required,
            "allowTag": false,
            "isActive": l.active,
        });
        if let (Some(id), Value::Object(map)) = (l.id, &mut line) {
            map.insert("id".into(), json!(id.to_string()));
        }
        out.push(line);
    }
    Ok(json!({ "lines": out }))
}

fn name_ok(name: &str) -> Result<&str, ApiError> {
    let n = name.trim();
    let len = n.chars().count();
    if len == 0 || len > DAILY_NAME_MAX_CHARS {
        return Err(local(LocalValidation::InvalidValue));
    }
    Ok(n)
}

/// Daily-sales layouts and days of a session (`session.daily_sales()`).
#[derive(Clone, Copy, Debug)]
pub struct DailySales<'a>(pub(crate) &'a Session);

impl Session {
    /// Daily-sales layouts and days of the open company.
    #[must_use]
    pub fn daily_sales(&self) -> DailySales<'_> {
        DailySales(self)
    }
}

fn write_call(method: Method, route: &'static str, path: String, body: Vec<u8>) -> Call {
    let mut call = Call::new(method, route, path).json_body(body);
    call.idempotency_key = Some(IdempotencyKey::new());
    call
}

impl DailySales<'_> {
    /// The company's active layouts by name (without their lines).
    ///
    /// # Errors
    /// `Decode`; any [`ApiError`].
    pub async fn templates(&self) -> Result<Vec<DailyTemplate>, ApiError> {
        #[derive(Deserialize)]
        struct W {
            templates: Vec<DailyTemplate>,
        }
        let call = Call::at(Method::Get, DAILY_TEMPLATES.route());
        let reply = self.0.send(&DAILY_TEMPLATES, call, None).await?;
        decode::<W>(&reply, "daily-sales layouts").map(|w| w.templates)
    }

    /// One layout with its lines (active and removed) in form order.
    ///
    /// # Errors
    /// `NotFound` (also another company's); `Decode`; any [`ApiError`].
    pub async fn template(&self, id: DailyTemplateId) -> Result<DailyTemplate, ApiError> {
        let call = Call::new(
            Method::Get,
            DAILY_TEMPLATE_GET.route(),
            format!("/api/v1/daily-sales/templates/{id}"),
        );
        let reply = self.0.send(&DAILY_TEMPLATE_GET, call, None).await?;
        let mut t = template_from_json(&reply.body)?;
        t.lines.sort_by_key(|l| l.sort);
        Ok(t)
    }

    /// Creates a layout, blank or from a preset (the engine maps the
    /// tenders, tax and tips lines to its own accounts and leaves the
    /// rest for the owner).
    ///
    /// # Errors
    /// `Local(InvalidValue)` for an empty or over-long name (nothing sent);
    /// any [`ApiError`].
    pub async fn create_template(
        &self,
        name: &str,
        preset: Option<Preset>,
    ) -> Result<DailyTemplate, ApiError> {
        let mut body = json!({ "name": name_ok(name)? });
        if let (Some(p), Value::Object(map)) = (preset, &mut body) {
            map.insert("presetType".into(), json!(p.as_wire()));
        }
        let call = write_call(
            Method::Post,
            DAILY_TEMPLATES_CREATE.route(),
            DAILY_TEMPLATES_CREATE.route().to_string(),
            json_body(&body)?,
        );
        let reply = self.0.send(&DAILY_TEMPLATES_CREATE, call, None).await?;
        template_from_json(&reply.body).map_err(saved_unreadable)
    }

    /// Renames a layout.
    ///
    /// # Errors
    /// `Local(InvalidValue)` for an empty or over-long name; any [`ApiError`].
    pub async fn rename(&self, id: DailyTemplateId, name: &str) -> Result<DailyTemplate, ApiError> {
        let body = json!({ "name": name_ok(name)? });
        let call = write_call(
            Method::Put,
            DAILY_TEMPLATE_PUT.route(),
            format!("/api/v1/daily-sales/templates/{id}"),
            json_body(&body)?,
        );
        let reply = self.0.send(&DAILY_TEMPLATE_PUT, call, None).await?;
        template_from_json(&reply.body).map_err(saved_unreadable)
    }

    /// Deletes a layout (the engine keeps it for its old days).
    ///
    /// # Errors
    /// Any [`ApiError`].
    pub async fn delete_template(&self, id: DailyTemplateId) -> Result<(), ApiError> {
        let mut call = Call::new(
            Method::Delete,
            DAILY_TEMPLATE_DELETE.route(),
            format!("/api/v1/daily-sales/templates/{id}"),
        );
        call.idempotency_key = Some(IdempotencyKey::new());
        self.0.send(&DAILY_TEMPLATE_DELETE, call, None).await?;
        Ok(())
    }

    /// Replaces a layout's lines, in order: saved lines by id, new lines
    /// added, saved lines left out removed (kept for old days).
    ///
    /// # Errors
    /// Nothing is sent for: `Local(TooManyLines)` (over 200),
    /// `Local(InvalidValue)` (an empty or over-long label, an unknown
    /// section). Any [`ApiError`].
    pub async fn replace_lines(
        &self,
        id: DailyTemplateId,
        lines: &[DailyLineInput],
    ) -> Result<DailyTemplate, ApiError> {
        let body = lines_body(lines)?;
        let call = write_call(
            Method::Put,
            DAILY_TEMPLATE_LINES.route(),
            format!("/api/v1/daily-sales/templates/{id}/lines"),
            json_body(&body)?,
        );
        let reply = self.0.send(&DAILY_TEMPLATE_LINES, call, None).await?;
        let mut t = template_from_json(&reply.body).map_err(saved_unreadable)?;
        t.lines.sort_by_key(|l| l.sort);
        Ok(t)
    }

    /// Days, newest first, filtered by `q` (the date filter runs here).
    ///
    /// # Errors
    /// `Local(PageLimit)` for a limit outside 1 to 500; `Decode`; any
    /// [`ApiError`].
    pub async fn list(&self, q: &DailyEntryQuery) -> Result<Vec<DailyEntrySummary>, ApiError> {
        #[derive(Deserialize)]
        struct W {
            entries: Vec<DailyEntrySummary>,
        }
        let mut call = Call::at(Method::Get, DAILY_ENTRIES.route());
        if let Some(st) = q.status.as_ref().and_then(EntryStatus::as_wire) {
            call.query.push(("status".into(), st.into()));
        }
        if let Some(t) = q.template {
            call.query.push(("templateId".into(), t.to_string()));
        }
        if let Some(l) = q.limit {
            if !(1..=500).contains(&l) {
                return Err(local(LocalValidation::PageLimit));
            }
            call.query.push(("limit".into(), l.to_string()));
        }
        let reply = self.0.send(&DAILY_ENTRIES, call, None).await?;
        let mut rows = decode::<W>(&reply, "daily-sales days")?.entries;
        if let Some(d) = q.date {
            rows.retain(|r| r.date == d);
        }
        Ok(rows)
    }

    /// One day with its amounts.
    ///
    /// # Errors
    /// `NotFound`; `Decode`; any [`ApiError`].
    pub async fn entry(&self, id: DailyEntryId) -> Result<DailyEntry, ApiError> {
        let call = Call::new(
            Method::Get,
            DAILY_ENTRY_GET.route(),
            format!("/api/v1/daily-sales/entries/{id}"),
        );
        let reply = self.0.send(&DAILY_ENTRY_GET, call, None).await?;
        entry_from_json(&reply.body)
    }

    /// Saves a new draft day (not posted).
    ///
    /// # Errors
    /// Nothing is sent for `Local(TooManyLines)` and `Local(InvalidValue)`
    /// (a negative amount, a line twice, notes over 2000 characters).
    /// `OutcomeUnknown` when the reply was lost: list the drafts of that
    /// day ([`DailySales::list`]) before trying again. Any [`ApiError`].
    pub async fn create_draft(
        &self,
        d: &DailyEntryDraft,
        key: IdempotencyKey,
    ) -> Result<DailyEntry, ApiError> {
        let body = draft_body(d, true)?;
        let mut call =
            Call::at(Method::Post, DAILY_ENTRIES_CREATE.route()).json_body(json_body(&body)?);
        call.idempotency_key = Some(key);
        let reply = self.0.send(&DAILY_ENTRIES_CREATE, call, None).await?;
        entry_from_json(&reply.body).map_err(saved_unreadable)
    }

    /// Replaces a draft day's date, tag, notes and amounts (its layout
    /// stays).
    ///
    /// # Errors
    /// As [`DailySales::create_draft`]; `Validation` for a day that is no
    /// longer a draft.
    pub async fn update_draft(
        &self,
        id: DailyEntryId,
        d: &DailyEntryDraft,
        key: IdempotencyKey,
    ) -> Result<DailyEntry, ApiError> {
        let body = draft_body(d, false)?;
        let mut call = Call::new(
            Method::Put,
            DAILY_ENTRY_PUT.route(),
            format!("/api/v1/daily-sales/entries/{id}"),
        )
        .json_body(json_body(&body)?);
        call.idempotency_key = Some(key);
        let reply = self.0.send(&DAILY_ENTRY_PUT, call, None).await?;
        entry_from_json(&reply.body).map_err(saved_unreadable)
    }

    /// The engine's figures for `values` on `template` (nothing saved).
    ///
    /// # Errors
    /// As [`DailySales::create_draft`]'s local refusals; `Decode`; any
    /// [`ApiError`].
    pub async fn preview(
        &self,
        template: DailyTemplateId,
        values: &[DailyValue],
    ) -> Result<DailyPreview, ApiError> {
        let body = json!({
            "templateId": template.to_string(),
            "values": values_json(values)?,
        });
        let call = Call::at(Method::Post, DAILY_ENTRY_PREVIEW.route()).json_body(json_body(&body)?);
        let reply = self.0.send(&DAILY_ENTRY_PREVIEW, call, None).await?;
        preview_from_json(&reply.body)
    }

    fn options_body(self, opts: &DailyPostOptions) -> Result<Vec<u8>, ApiError> {
        if opts.closed_period.is_some()
            && !self
                .0
                .client()
                .capabilities()
                .has(Capabilities::DAILY_SALES_LOCK)
        {
            return Err(local(LocalValidation::DailySalesLockUnavailable));
        }
        posting_body(
            self.0,
            json!({}),
            &PostingOptions {
                closed_period: opts.closed_period.clone(),
            },
        )
    }

    /// Posts a draft day: the engine builds the entry from the layout and
    /// adds the cash over/short line itself.
    ///
    /// # Errors
    /// `Local(DailySalesLockUnavailable)` for a password before the engine
    /// takes one (nothing sent). From the engine: `Validation` (an unmapped
    /// line with an amount, no amounts, already posted), `ClosedPeriod`,
    /// `ClosingPasswordRejected`, `ClosingPasswordRateLimited`,
    /// `OutcomeUnknown` (read the day before trying again); any
    /// [`ApiError`].
    pub async fn post(
        &self,
        id: DailyEntryId,
        opts: &DailyPostOptions,
        key: IdempotencyKey,
    ) -> Result<DailyEntry, ApiError> {
        let body = self.options_body(opts)?;
        let mut call = Call::new(
            Method::Post,
            DAILY_ENTRY_POST.route(),
            format!("/api/v1/daily-sales/entries/{id}/post"),
        )
        .json_body(body);
        call.idempotency_key = Some(key);
        let reply = self.0.send(&DAILY_ENTRY_POST, call, None).await?;
        entry_from_json(&reply.body).map_err(saved_unreadable)
    }

    /// Voids a posted day (its journal entry is voided).
    ///
    /// # Errors
    /// As [`DailySales::post`]; `Validation` for a day that is not posted.
    pub async fn void(
        &self,
        id: DailyEntryId,
        opts: &DailyPostOptions,
        key: IdempotencyKey,
    ) -> Result<DailyEntry, ApiError> {
        let body = self.options_body(opts)?;
        let mut call = Call::new(
            Method::Post,
            DAILY_ENTRY_VOID.route(),
            format!("/api/v1/daily-sales/entries/{id}/void"),
        )
        .json_body(body);
        call.idempotency_key = Some(key);
        let reply = self.0.send(&DAILY_ENTRY_VOID, call, None).await?;
        entry_from_json(&reply.body).map_err(saved_unreadable)
    }

    /// Deletes a draft day (a posted day is voided instead).
    ///
    /// # Errors
    /// `Validation` for a posted day; any [`ApiError`].
    pub async fn delete_draft(&self, id: DailyEntryId) -> Result<(), ApiError> {
        let mut call = Call::new(
            Method::Delete,
            DAILY_ENTRY_DELETE.route(),
            format!("/api/v1/daily-sales/entries/{id}"),
        );
        call.idempotency_key = Some(IdempotencyKey::new());
        self.0.send(&DAILY_ENTRY_DELETE, call, None).await?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "daily_sales_tests.rs"]
mod tests;
