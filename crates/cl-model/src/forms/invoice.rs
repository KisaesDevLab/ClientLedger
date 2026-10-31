//! The Create Invoice form model (P20 plan §3 WP-A step 2).
//!
//! **No money arithmetic here** (Q-P20n): the engine computes every line
//! amount, discount, subtotal, the tax, the total, the due date and the
//! client's balance after the invoice (U-122, U-129). The form holds what
//! the owner typed and shows what the engine answered:
//!
//! - [`InvoiceForm`] is the editable value (the `V` of
//!   [`crate::forms::FormState`]): client, date, number, terms, an owner's
//!   due date, P.O. number, memo, customer message, header tags and the
//!   lines (item, description, service date, quantity, a typed rate or
//!   none, taxable, tag, billable source and markup).
//! - [`check_invoice`] (and [`InvoiceForm::to_new_invoice`]) is the one
//!   gate before a save: it returns an [`InvoiceRequest`], the wire-ready
//!   content the UI maps one to one onto `cl_api::NewInvoice`, or the
//!   [`InvoiceProblem`]s to show.
//! - [`InvoiceForm::preview_request`] is the lenient request for `POST
//!   /invoices/preview`; [`PreviewTracker`] debounces it 250 ms, tags each
//!   one sent with a [`PreviewTicket`], drops stale replies and tells the
//!   screen what to show: the engine's figures, "Updating…" while a newer
//!   request is pending (never an old figure), or "Can't calculate" after a
//!   failure. Save never waits for the preview: posting runs the same
//!   engine code.
//!
//! Debug output never shows names, descriptions, memos or amounts.

use std::fmt;

use cl_core::ids::{AccountId, ContactId, ItemId, JournalLineId, TagId};
use cl_core::{CalendarDate, Money, Quantity, Rate};

use crate::rowkey::{RowKey, RowKeyGen};
use crate::text::TextKey;

/// Most lines one invoice takes (the engine's maximum).
pub const MAX_LINES: usize = 500;
/// The longest invoice number.
pub const NUMBER_MAX: usize = 50;
/// The longest P.O. number.
pub const PO_MAX: usize = 50;
/// The longest line description and customer message.
pub const TEXT_MAX: usize = 4000;
/// The longest custom terms period (days).
pub const TERMS_DAYS_MAX: u16 = 3650;
/// The terms code of a custom number of days.
pub const CUSTOM_TERMS: &str = "custom";
/// How long the form waits after the last edit before asking for a
/// preview.
pub const PREVIEW_DEBOUNCE_MS: u64 = 250;

/// The billable cost a line invoices (U-128, Q-P20b/p).
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct BillableRef {
    /// The source expense line.
    pub source_line: JournalLineId,
    /// Markup in percent (blank: none). The engine computes the amount.
    pub markup_percent: Option<Quantity>,
}

impl fmt::Debug for BillableRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BillableRef")
            .field("source_line", &self.source_line)
            .field("markup", &self.markup_percent.is_some())
            .finish()
    }
}

/// One line of the invoice as typed.
#[derive(Clone, PartialEq, Eq)]
pub struct InvoiceLine {
    /// Row identity (kept through undo, inserts and deletes).
    pub key: RowKey,
    /// The item (supplies the income account and the taxable default).
    pub item: Option<ItemId>,
    /// The income account when there is no item (or to override it).
    pub account: Option<AccountId>,
    /// Description, may span several lines.
    pub description: String,
    /// Service date.
    pub service_date: Option<CalendarDate>,
    /// Quantity; blank sends 1.
    pub quantity: Option<Quantity>,
    /// The rate the owner typed; blank lets the engine price the line from
    /// the item and the client's price level. On a billable line it is the
    /// engine's display rate, shown only and never sent.
    pub rate: Option<Money>,
    /// A percent discount line's stored percent (from the saved invoice;
    /// resent so the line keeps it). Cleared when the item changes.
    pub discount_percent: Option<Quantity>,
    /// Taxable as the owner set it; `None` follows the item.
    pub taxable: Option<bool>,
    /// Line tag.
    pub tag: Option<TagId>,
    /// The billable cost this line invoices (quantity and rate are the
    /// source's and read-only, Q-P20p).
    pub billable: Option<BillableRef>,
}

impl InvoiceLine {
    /// An empty entry line.
    #[must_use]
    pub fn blank(key: RowKey) -> InvoiceLine {
        InvoiceLine {
            key,
            item: None,
            account: None,
            description: String::new(),
            service_date: None,
            quantity: None,
            rate: None,
            discount_percent: None,
            taxable: None,
            tag: None,
            billable: None,
        }
    }

    /// Nothing typed on it (the trailing entry line).
    #[must_use]
    pub fn is_blank(&self) -> bool {
        self.item.is_none()
            && self.account.is_none()
            && self.description.trim().is_empty()
            && self.service_date.is_none()
            && self.quantity.is_none()
            && self.rate.is_none()
            && self.discount_percent.is_none()
            && self.taxable.is_none()
            && self.tag.is_none()
            && self.billable.is_none()
    }

    /// Item, quantity and rate come from a billable cost and can't be
    /// edited (the engine refuses them, `BILLABLE_FIELDS_READONLY`).
    #[must_use]
    pub fn amounts_locked(&self) -> bool {
        self.billable.is_some()
    }

    /// The owner picked another item: the old item's price and discount
    /// percent no longer apply (the engine prices the new item).
    pub fn item_changed(&mut self, item: Option<ItemId>) {
        if self.item != item {
            self.item = item;
            self.rate = None;
            self.discount_percent = None;
        }
    }

    fn priceable(&self) -> bool {
        self.item.is_some() || self.account.is_some() || self.billable.is_some()
    }
}

/// Ids and flags only.
impl fmt::Debug for InvoiceLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InvoiceLine")
            .field("key", &self.key)
            .field("item", &self.item)
            .field("account", &self.account)
            .field("service_date", &self.service_date.is_some())
            .field("quantity", &self.quantity.is_some())
            .field("rate", &self.rate.is_some())
            .field("taxable", &self.taxable)
            .field("billable", &self.billable)
            .finish_non_exhaustive()
    }
}

/// A billable cost the owner chose in Add Billable Costs, as it becomes a
/// line (copied from the engine's row; nothing computed).
#[derive(Clone, PartialEq, Eq)]
pub struct BillableSeed {
    /// The source expense line.
    pub source_line: JournalLineId,
    /// The income account the cost is billed to: the owner's pick
    /// (remembered per expense account), else the engine's suggestion
    /// (`suggestedIncomeAccountId`). Required by the engine.
    pub account: Option<AccountId>,
    /// The source's description.
    pub description: String,
    /// The source's quantity (shown only).
    pub quantity: Option<Quantity>,
    /// Markup in percent, defaulted from the client.
    pub markup_percent: Option<Quantity>,
}

impl fmt::Debug for BillableSeed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BillableSeed")
            .field("source_line", &self.source_line)
            .field("account", &self.account)
            .finish_non_exhaustive()
    }
}

/// The whole editable invoice.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct InvoiceForm {
    /// The client.
    pub client: Option<ContactId>,
    /// Invoice date.
    pub date: Option<CalendarDate>,
    /// Invoice number (prefilled from the company's counter; blank lets the
    /// engine assign one).
    pub number: String,
    /// The client's P.O. number.
    pub po_number: String,
    /// Terms code; `None` uses the client's, else the company's (Q-P20g).
    pub terms: Option<String>,
    /// Days of custom terms.
    pub terms_days: Option<u16>,
    /// A due date the owner typed (wins over the terms); `None` shows the
    /// engine's from the preview.
    pub due_date: Option<CalendarDate>,
    /// Internal memo (registers and reports only, Q-P20k).
    pub memo: String,
    /// Printed message.
    pub customer_message: String,
    /// Header tags.
    pub header_tags: Vec<TagId>,
    /// Leave $0 lines off the printout.
    pub hide_zero_lines: bool,
    /// Waiting to be printed.
    pub print_later: bool,
    /// Lines, ending with a blank entry line.
    pub lines: Vec<InvoiceLine>,
}

impl fmt::Debug for InvoiceForm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InvoiceForm")
            .field("client", &self.client.is_some())
            .field("date", &self.date.is_some())
            .field("number", &!self.number.is_empty())
            .field("po_number", &!self.po_number.is_empty())
            .field("terms", &self.terms.is_some())
            .field("due_date", &self.due_date.is_some())
            .field("memo", &!self.memo.is_empty())
            .field("customer_message", &!self.customer_message.is_empty())
            .field("header_tags", &self.header_tags.len())
            .field("hide_zero_lines", &self.hide_zero_lines)
            .field("print_later", &self.print_later)
            .field("lines", &self.lines.len())
            .finish_non_exhaustive()
    }
}

impl InvoiceForm {
    /// A blank invoice dated `date` with one entry line.
    #[must_use]
    pub fn new(date: Option<CalendarDate>, keys: &mut RowKeyGen) -> InvoiceForm {
        InvoiceForm {
            date,
            lines: vec![InvoiceLine::blank(keys.next_key())],
            ..InvoiceForm::default()
        }
    }

    /// Keeps exactly one blank line at the end (the entry line).
    pub fn ensure_entry_line(&mut self, keys: &mut RowKeyGen) {
        while self.lines.len() > 1
            && self.lines.last().is_some_and(InvoiceLine::is_blank)
            && self.lines[self.lines.len() - 2].is_blank()
        {
            self.lines.pop();
        }
        if !self.lines.last().is_some_and(InvoiceLine::is_blank) {
            self.lines.push(InvoiceLine::blank(keys.next_key()));
        }
    }

    /// Inserts a blank line before `at` (Ctrl+Insert); returns its key.
    pub fn insert_line(&mut self, at: RowKey, keys: &mut RowKeyGen) -> RowKey {
        let key = keys.next_key();
        let pos = self
            .lines
            .iter()
            .position(|l| l.key == at)
            .unwrap_or(self.lines.len());
        self.lines.insert(pos, InvoiceLine::blank(key));
        key
    }

    /// Deletes a line (Ctrl+Delete); the entry line stays.
    pub fn delete_line(&mut self, key: RowKey, keys: &mut RowKeyGen) -> bool {
        let before = self.lines.len();
        self.lines.retain(|l| l.key != key);
        let removed = self.lines.len() != before;
        self.ensure_entry_line(keys);
        removed
    }

    /// The line with `key`.
    #[must_use]
    pub fn line(&self, key: RowKey) -> Option<&InvoiceLine> {
        self.lines.iter().find(|l| l.key == key)
    }

    /// The line with `key`, to edit.
    pub fn line_mut(&mut self, key: RowKey) -> Option<&mut InvoiceLine> {
        self.lines.iter_mut().find(|l| l.key == key)
    }

    /// Adds the chosen billable costs as lines before the entry line
    /// (nothing is added automatically, Q-P20c). Billable lines default to
    /// taxable (Q-P20p). A cost already on the invoice is skipped.
    pub fn add_billable(&mut self, seeds: &[BillableSeed], keys: &mut RowKeyGen) -> usize {
        let mut added = 0;
        for s in seeds {
            let on_it = self
                .lines
                .iter()
                .any(|l| l.billable.is_some_and(|b| b.source_line == s.source_line));
            if on_it {
                continue;
            }
            let pos = self
                .lines
                .iter()
                .rposition(|l| !l.is_blank())
                .map_or(0, |p| p + 1);
            self.lines.insert(
                pos,
                InvoiceLine {
                    account: s.account,
                    description: s.description.clone(),
                    quantity: s.quantity,
                    taxable: Some(true),
                    billable: Some(BillableRef {
                        source_line: s.source_line,
                        markup_percent: s.markup_percent,
                    }),
                    ..InvoiceLine::blank(keys.next_key())
                },
            );
            added += 1;
        }
        self.ensure_entry_line(keys);
        added
    }

    /// The source lines of the billable costs on the invoice (the side
    /// panel leaves them out of "N unbilled costs").
    #[must_use]
    pub fn billable_sources(&self) -> Vec<JournalLineId> {
        self.lines
            .iter()
            .filter_map(|l| l.billable.map(|b| b.source_line))
            .collect()
    }

    /// The checked request (see [`check_invoice`]).
    ///
    /// # Errors
    /// Every [`InvoiceProblem`] found.
    pub fn to_new_invoice(
        &self,
        ctx: &InvoiceContext,
    ) -> Result<InvoiceRequest, Vec<InvoiceProblem>> {
        check_invoice(self, ctx)
    }

    /// The request for `POST /invoices/preview`, lenient: the used lines
    /// that name an item or an account, as [`check_invoice`] would send
    /// them. What the engine does not price (number, memo, message, P.O.
    /// number, tags, descriptions, print later) is left out, so editing it
    /// keeps the figures on screen. `None` when there is nothing to price
    /// yet (no client, no date or no such line).
    #[must_use]
    pub fn preview_request(&self, caps: InvoiceCaps) -> Option<InvoiceRequest> {
        let client = self.client?;
        let date = self.date?;
        let lines: Vec<RequestLine> = self
            .lines
            .iter()
            .filter(|l| l.priceable())
            .map(|l| RequestLine {
                description: None,
                tag: None,
                ..request_line(l, caps)
            })
            .collect();
        if lines.is_empty() || lines.len() > MAX_LINES {
            return None;
        }
        Some(InvoiceRequest {
            number: None,
            memo: String::new(),
            po_number: None,
            customer_message: None,
            header_tags: None,
            hide_zero_lines: None,
            print_later: false,
            ..self.request(client, date, lines, caps)
        })
    }

    fn request(
        &self,
        client: ContactId,
        date: CalendarDate,
        lines: Vec<RequestLine>,
        caps: InvoiceCaps,
    ) -> InvoiceRequest {
        let custom = self.terms.as_deref() == Some(CUSTOM_TERMS);
        InvoiceRequest {
            client,
            date,
            due_date: self.due_date,
            terms: self.terms.clone(),
            terms_days: if custom { self.terms_days } else { None },
            number: Some(self.number.trim().to_string()).filter(|n| !n.is_empty()),
            memo: self.memo.clone(),
            po_number: caps.fields.then(|| self.po_number.trim().to_string()),
            customer_message: caps.fields.then(|| self.customer_message.clone()),
            header_tags: caps.fields.then(|| self.header_tags.clone()),
            hide_zero_lines: caps.fields.then_some(self.hide_zero_lines),
            print_later: self.print_later,
            lines,
        }
    }
}

/// What the engine can take (from its capabilities).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[allow(clippy::struct_excessive_bools)] // one flag per capability
pub struct InvoiceCaps {
    /// `invoice-fields-v1`: items, service dates and the header fields.
    pub fields: bool,
    /// `invoice-pricing-v1`: lines without a rate, the preview.
    pub pricing: bool,
    /// `billable-costs-v1`.
    pub billable: bool,
    /// `invoice-edit-v1`: the closing-date password on invoices.
    pub closing: bool,
}

/// What an invoice is checked against.
#[derive(Debug, Clone, Copy, Default)]
pub struct InvoiceContext {
    /// The company's closing date, when set.
    pub closing_date: Option<CalendarDate>,
    /// The saved date of the invoice being edited.
    pub original_date: Option<CalendarDate>,
    /// Engine capabilities.
    pub caps: InvoiceCaps,
}

/// The field a problem belongs to (the screen focuses it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InvoiceField {
    /// Client.
    Client,
    /// Date.
    Date,
    /// Invoice number.
    Number,
    /// P.O. number.
    PoNumber,
    /// Terms and their days.
    Terms,
    /// Due date.
    DueDate,
    /// Customer message.
    Message,
    /// The lines as a whole.
    Lines,
    /// One line's item or account.
    LineItem(RowKey),
    /// One line's description.
    LineDescription(RowKey),
    /// One line's rate.
    LineRate(RowKey),
    /// One line's billable markup.
    LineMarkup(RowKey),
}

/// Why an invoice can't be saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InvoiceProblem {
    /// No client.
    ClientMissing,
    /// No date.
    DateMissing,
    /// Dated in a closed period and the engine can't take the closing
    /// password on invoices yet.
    ClosedPeriodNeedsUpdate,
    /// The number is longer than [`NUMBER_MAX`].
    NumberTooLong,
    /// The P.O. number is longer than [`PO_MAX`].
    PoTooLong,
    /// The customer message is longer than [`TEXT_MAX`].
    MessageTooLong,
    /// Custom terms need their days (0 to [`TERMS_DAYS_MAX`]).
    TermsDaysInvalid,
    /// The due date is before the invoice date.
    DueBeforeDate,
    /// Items, service dates, P.O. number, message or tags need an engine
    /// update (`invoice-fields-v1`).
    FieldsNeedUpdate,
    /// No line is used.
    NoLines,
    /// More than [`MAX_LINES`] lines.
    TooManyLines,
    /// A used line has neither item nor account.
    LineItemMissing(RowKey),
    /// A line's description is longer than [`TEXT_MAX`].
    LineDescriptionTooLong(RowKey),
    /// A line has no rate and the engine can't price it yet.
    LineRateMissing(RowKey),
    /// A billable line needs an engine update (`billable-costs-v1`).
    BillableNeedsUpdate(RowKey),
    /// A markup below zero or above 1000%.
    MarkupNegative(RowKey),
    /// A billable line without the income account it is billed to.
    BillableAccountMissing(RowKey),
}

impl InvoiceProblem {
    /// Plain-language message (strings live in `cl-ui::i18n`).
    #[must_use]
    pub const fn key(self) -> TextKey {
        match self {
            InvoiceProblem::ClientMissing => TextKey("invoice.problem.client_missing"),
            InvoiceProblem::DateMissing => TextKey("invoice.problem.date_missing"),
            InvoiceProblem::ClosedPeriodNeedsUpdate => {
                TextKey("invoice.problem.closed_needs_update")
            }
            InvoiceProblem::NumberTooLong => TextKey("invoice.problem.number_too_long"),
            InvoiceProblem::PoTooLong => TextKey("invoice.problem.po_too_long"),
            InvoiceProblem::MessageTooLong => TextKey("invoice.problem.message_too_long"),
            InvoiceProblem::TermsDaysInvalid => TextKey("invoice.problem.terms_days_invalid"),
            InvoiceProblem::DueBeforeDate => TextKey("invoice.problem.due_before_date"),
            InvoiceProblem::FieldsNeedUpdate => TextKey("invoice.problem.fields_need_update"),
            InvoiceProblem::NoLines => TextKey("invoice.problem.no_lines"),
            InvoiceProblem::TooManyLines => TextKey("invoice.problem.too_many_lines"),
            InvoiceProblem::LineItemMissing(_) => TextKey("invoice.problem.line_item_missing"),
            InvoiceProblem::LineDescriptionTooLong(_) => {
                TextKey("invoice.problem.line_description_too_long")
            }
            InvoiceProblem::LineRateMissing(_) => TextKey("invoice.problem.line_rate_missing"),
            InvoiceProblem::BillableNeedsUpdate(_) => {
                TextKey("invoice.problem.billable_needs_update")
            }
            InvoiceProblem::MarkupNegative(_) => TextKey("invoice.problem.markup_negative"),
            InvoiceProblem::BillableAccountMissing(_) => {
                TextKey("invoice.problem.billable_account_missing")
            }
        }
    }

    /// The field to focus.
    #[must_use]
    pub const fn field(self) -> InvoiceField {
        match self {
            InvoiceProblem::ClientMissing => InvoiceField::Client,
            InvoiceProblem::DateMissing | InvoiceProblem::ClosedPeriodNeedsUpdate => {
                InvoiceField::Date
            }
            InvoiceProblem::NumberTooLong => InvoiceField::Number,
            InvoiceProblem::PoTooLong => InvoiceField::PoNumber,
            InvoiceProblem::MessageTooLong => InvoiceField::Message,
            InvoiceProblem::TermsDaysInvalid => InvoiceField::Terms,
            InvoiceProblem::DueBeforeDate => InvoiceField::DueDate,
            InvoiceProblem::FieldsNeedUpdate
            | InvoiceProblem::NoLines
            | InvoiceProblem::TooManyLines => InvoiceField::Lines,
            InvoiceProblem::LineItemMissing(k)
            | InvoiceProblem::BillableNeedsUpdate(k)
            | InvoiceProblem::BillableAccountMissing(k) => InvoiceField::LineItem(k),
            InvoiceProblem::LineDescriptionTooLong(k) => InvoiceField::LineDescription(k),
            InvoiceProblem::LineRateMissing(k) => InvoiceField::LineRate(k),
            InvoiceProblem::MarkupNegative(k) => InvoiceField::LineMarkup(k),
        }
    }
}

/// One line as it goes out (maps one to one onto `cl_api::InvoiceLine`).
#[derive(Clone, PartialEq, Eq)]
pub struct RequestLine {
    /// The form row it came from.
    pub key: RowKey,
    /// Income account (`None`: the item's).
    pub account: Option<AccountId>,
    /// Item (only with `invoice-fields-v1`).
    pub item: Option<ItemId>,
    /// Description (`None` when blank).
    pub description: Option<String>,
    /// Service date.
    pub service_date: Option<CalendarDate>,
    /// Quantity (`None`: left out; the engine takes 1. Never on a
    /// billable line).
    pub quantity: Option<Quantity>,
    /// The typed rate (`None`: priced by the engine; never on a billable
    /// line).
    pub unit_price: Option<Money>,
    /// A discount line's stored percent (never on a billable line).
    pub discount_percent: Option<Quantity>,
    /// Taxable as set (`None`: the item's).
    pub taxable: Option<bool>,
    /// Line tag.
    pub tag: Option<TagId>,
    /// The billable source line.
    pub billable_source_line: Option<JournalLineId>,
    /// Its markup in percent.
    pub markup_percent: Option<Quantity>,
}

impl fmt::Debug for RequestLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RequestLine")
            .field("key", &self.key)
            .field("account", &self.account)
            .field("item", &self.item)
            .field("taxable", &self.taxable)
            .field("billable_source_line", &self.billable_source_line)
            .finish_non_exhaustive()
    }
}

/// The checked invoice, wire-ready (maps one to one onto
/// `cl_api::NewInvoice`; the tax rate is never sent).
#[derive(Clone, PartialEq, Eq)]
pub struct InvoiceRequest {
    /// The client.
    pub client: ContactId,
    /// Invoice date.
    pub date: CalendarDate,
    /// The owner's due date (`None`: the engine's from the terms).
    pub due_date: Option<CalendarDate>,
    /// Terms code (`None`: the invoice's own on an edit, else the client's,
    /// else the company's).
    pub terms: Option<String>,
    /// Days of custom terms.
    pub terms_days: Option<u16>,
    /// Typed number (`None`: the engine's next).
    pub number: Option<String>,
    /// Internal memo (always sent so an edit can clear it).
    pub memo: String,
    /// P.O. number (`Some("")` clears; `None` without `invoice-fields-v1`).
    pub po_number: Option<String>,
    /// Printed message (as P.O. number).
    pub customer_message: Option<String>,
    /// Header tags (as P.O. number).
    pub header_tags: Option<Vec<TagId>>,
    /// Hide $0 lines (as P.O. number).
    pub hide_zero_lines: Option<bool>,
    /// Print later.
    pub print_later: bool,
    /// The used lines, in order.
    pub lines: Vec<RequestLine>,
}

impl fmt::Debug for InvoiceRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InvoiceRequest")
            .field("client", &self.client)
            .field("date", &self.date)
            .field("due_date", &self.due_date)
            .field("terms", &self.terms)
            .field("lines", &self.lines)
            .finish_non_exhaustive()
    }
}

fn request_line(l: &InvoiceLine, caps: InvoiceCaps) -> RequestLine {
    let one = Quantity::parse_wire("1").unwrap_or(Quantity::ZERO);
    // A billable cost sends only its source, account, markup, description,
    // taxable, service date and tag: its item, quantity and price are the
    // source's (U-128; the engine refuses them).
    let billable = l.billable.is_some();
    // Before invoice-pricing-v1 the engine needs a quantity on every line.
    let quantity = if caps.pricing {
        l.quantity
    } else {
        Some(l.quantity.unwrap_or(one))
    };
    RequestLine {
        key: l.key,
        account: l.account,
        item: if caps.fields && !billable {
            l.item
        } else {
            None
        },
        description: Some(l.description.trim_end().to_string()).filter(|d| !d.trim().is_empty()),
        service_date: if caps.fields { l.service_date } else { None },
        quantity: if billable { None } else { quantity },
        unit_price: if billable { None } else { l.rate },
        discount_percent: if billable { None } else { l.discount_percent },
        taxable: l.taxable,
        tag: l.tag,
        billable_source_line: l.billable.map(|b| b.source_line),
        markup_percent: l.billable.and_then(|b| b.markup_percent),
    }
}

/// Checks the invoice before anything is sent and builds the request.
///
/// Blank lines are skipped. Nothing is added up: the engine computes the
/// amounts (U-122) and refuses what it can't post.
///
/// # Errors
/// Every [`InvoiceProblem`] found, in form order.
pub fn check_invoice(
    form: &InvoiceForm,
    ctx: &InvoiceContext,
) -> Result<InvoiceRequest, Vec<InvoiceProblem>> {
    let caps = ctx.caps;
    let mut problems = Vec::new();
    if form.client.is_none() {
        problems.push(InvoiceProblem::ClientMissing);
    }
    match form.date {
        None => problems.push(InvoiceProblem::DateMissing),
        Some(d) => {
            let closed = ctx
                .closing_date
                .is_some_and(|c| d <= c || ctx.original_date.is_some_and(|o| o <= c));
            if closed && !caps.closing {
                problems.push(InvoiceProblem::ClosedPeriodNeedsUpdate);
            }
            if form.due_date.is_some_and(|due| due < d) {
                problems.push(InvoiceProblem::DueBeforeDate);
            }
        }
    }
    if form.number.trim().chars().count() > NUMBER_MAX {
        problems.push(InvoiceProblem::NumberTooLong);
    }
    if form.po_number.trim().chars().count() > PO_MAX {
        problems.push(InvoiceProblem::PoTooLong);
    }
    if form.customer_message.chars().count() > TEXT_MAX {
        problems.push(InvoiceProblem::MessageTooLong);
    }
    if form.terms.as_deref() == Some(CUSTOM_TERMS)
        && form.terms_days.is_none_or(|d| d > TERMS_DAYS_MAX)
    {
        problems.push(InvoiceProblem::TermsDaysInvalid);
    }
    let used: Vec<&InvoiceLine> = form.lines.iter().filter(|l| !l.is_blank()).collect();
    let header_fields = !form.po_number.trim().is_empty()
        || !form.customer_message.trim().is_empty()
        || !form.header_tags.is_empty()
        || form.hide_zero_lines;
    let line_fields = used
        .iter()
        .any(|l| l.item.is_some() || l.service_date.is_some());
    if !caps.fields && (header_fields || line_fields) {
        problems.push(InvoiceProblem::FieldsNeedUpdate);
    }
    if used.is_empty() {
        problems.push(InvoiceProblem::NoLines);
    } else if used.len() > MAX_LINES {
        problems.push(InvoiceProblem::TooManyLines);
    }
    for l in &used {
        if !l.priceable() {
            problems.push(InvoiceProblem::LineItemMissing(l.key));
        }
        if l.description.chars().count() > TEXT_MAX {
            problems.push(InvoiceProblem::LineDescriptionTooLong(l.key));
        }
        if l.rate.is_none() && l.billable.is_none() && !caps.pricing {
            problems.push(InvoiceProblem::LineRateMissing(l.key));
        }
        if let Some(b) = l.billable {
            if !caps.billable {
                problems.push(InvoiceProblem::BillableNeedsUpdate(l.key));
            }
            if l.account.is_none() {
                problems.push(InvoiceProblem::BillableAccountMissing(l.key));
            }
            let max = Quantity::parse_wire("1000").unwrap_or(Quantity::ZERO);
            if b.markup_percent.is_some_and(|m| m.is_negative() || m > max) {
                problems.push(InvoiceProblem::MarkupNegative(l.key));
            }
        }
    }
    if !problems.is_empty() {
        return Err(problems);
    }
    let (Some(client), Some(date)) = (form.client, form.date) else {
        return Err(vec![InvoiceProblem::ClientMissing]);
    };
    let lines = used.into_iter().map(|l| request_line(l, caps)).collect();
    Ok(form.request(client, date, lines, caps))
}

// ---- The preview ----------------------------------------------------------------

/// Where a line's rate came from (the engine's `unitPriceSource`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RateSource {
    /// The owner typed it (`input`).
    Typed,
    /// The item's price (`item`).
    Item,
    /// The price the client's level sets for this item (`price_level`).
    PriceLevel,
    /// The client's level's % of the item's price (`price_level_percent`).
    PriceLevelPercent,
    /// No price: a subtotal line (`none`).
    Unpriced,
    /// A billable cost with any markup (`billable_cost`).
    Billable,
    /// Unknown or not sent.
    Other,
}

impl RateSource {
    /// The tooltip naming the source (Rate source shown, plan §6).
    #[must_use]
    pub const fn key(self) -> TextKey {
        match self {
            RateSource::Typed => TextKey("invoice.rate_source.typed"),
            RateSource::Item => TextKey("invoice.rate_source.item"),
            RateSource::PriceLevel => TextKey("invoice.rate_source.price_level"),
            RateSource::PriceLevelPercent => TextKey("invoice.rate_source.price_level_percent"),
            RateSource::Unpriced => TextKey("invoice.rate_source.unpriced"),
            RateSource::Billable => TextKey("invoice.rate_source.billable"),
            RateSource::Other => TextKey("invoice.rate_source.other"),
        }
    }
}

/// The engine's figures for one line.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct PreviewLineFigures {
    /// The rate it used.
    pub unit_price: Option<Money>,
    /// Where the rate came from.
    pub source: RateSource,
    /// The line amount.
    pub amount: Money,
    /// The line's share of the tax.
    pub tax_amount: Option<Money>,
    /// A billable line's rate as shown (amount / quantity).
    pub display_rate: Option<Money>,
    /// Another invoice already bills this line's cost (U-128).
    pub billed_elsewhere: bool,
}

impl fmt::Debug for PreviewLineFigures {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PreviewLineFigures")
            .field("source", &self.source)
            .finish_non_exhaustive()
    }
}

/// The engine's figures for the whole invoice.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct PreviewTotals {
    /// Sum of the line amounts.
    pub subtotal: Money,
    /// The document tax.
    pub tax_total: Money,
    /// The tax rate (the label; no city until P23).
    pub tax_rate: Option<Rate>,
    /// Total.
    pub total: Money,
    /// Due date from the terms.
    pub due_date: Option<CalendarDate>,
    /// The client's open balance once this invoice posts.
    pub balance_after: Option<Money>,
}

impl fmt::Debug for PreviewTotals {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PreviewTotals")
            .field("due_date", &self.due_date)
            .finish_non_exhaustive()
    }
}

/// A preview reply (the UI copies `cl_api::InvoicePreview` into it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewFigures {
    /// One per request line, in order.
    pub lines: Vec<PreviewLineFigures>,
    /// The totals.
    pub totals: PreviewTotals,
    /// The numbers of the invoices that already bill a cost on this one
    /// (preview `lines[].error`, U-128).
    pub billed_on: Vec<String>,
}

impl PreviewTracker {
    /// The invoice numbers that already bill a cost on the form (empty
    /// unless the shown figures say so).
    #[must_use]
    pub fn billed_on(&self) -> &[String] {
        match &self.applied {
            Some(a) if self.wanted.is_none() => &a.figures.billed_on,
            _ => &[],
        }
    }
}

/// The tag of one preview request: a reply is applied only if its ticket is
/// the latest one sent for the request still on the form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PreviewTicket(u64);

/// What the totals area shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TotalsView<'a> {
    /// Nothing to price yet (no client, date or line): dashes.
    Nothing,
    /// A newer request is pending: "Updating…", never an old figure.
    Updating,
    /// The engine couldn't compute it: "Can't calculate" (Save still works).
    CantCalculate,
    /// The engine's figures for what is on the form now.
    Ready(&'a PreviewTotals),
}

/// What one line's Rate, Amount and Tax cells show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineView<'a> {
    /// Not priced (blank or unusable line).
    Nothing,
    /// "Updating…".
    Updating,
    /// "Can't calculate".
    CantCalculate,
    /// The engine's figures.
    Ready(&'a PreviewLineFigures),
}

#[derive(Debug, Clone)]
struct Wanted {
    request: InvoiceRequest,
    since_ms: u64,
    sent: Option<u64>,
    failed: bool,
}

#[derive(Debug, Clone)]
struct Applied {
    request: InvoiceRequest,
    figures: PreviewFigures,
}

/// Debounces previews and decides what the form shows (plan §1: no stale
/// figure is ever shown).
///
/// Call [`PreviewTracker::poll`] every frame with the current
/// [`InvoiceForm::preview_request`]; send what it returns; hand the reply to
/// [`PreviewTracker::apply_preview`] (or [`PreviewTracker::fail`]).
#[derive(Debug, Clone, Default)]
pub struct PreviewTracker {
    wanted: Option<Wanted>,
    applied: Option<Applied>,
    seq: u64,
}

impl PreviewTracker {
    /// A tracker with nothing asked yet.
    #[must_use]
    pub fn new() -> PreviewTracker {
        PreviewTracker::default()
    }

    /// Notes the request on the form at `now_ms` (any monotonic clock in
    /// milliseconds) and returns the one to send, if any: the request has
    /// stood for [`PREVIEW_DEBOUNCE_MS`] and was not sent yet. An edit that
    /// changes nothing the engine prices keeps the figures on screen.
    pub fn poll(
        &mut self,
        request: Option<InvoiceRequest>,
        now_ms: u64,
    ) -> Option<(PreviewTicket, InvoiceRequest)> {
        let Some(request) = request else {
            self.wanted = None;
            self.applied = None;
            return None;
        };
        if self.applied.as_ref().is_some_and(|a| a.request == request) {
            self.wanted = None;
            return None;
        }
        match &mut self.wanted {
            Some(w) if w.request == request => {}
            _ => {
                self.wanted = Some(Wanted {
                    request,
                    since_ms: now_ms,
                    sent: None,
                    failed: false,
                });
                return None;
            }
        }
        let w = self.wanted.as_mut()?;
        if w.sent.is_some() || now_ms.saturating_sub(w.since_ms) < PREVIEW_DEBOUNCE_MS {
            return None;
        }
        self.seq += 1;
        w.sent = Some(self.seq);
        Some((PreviewTicket(self.seq), w.request.clone()))
    }

    /// Applies a reply. Returns false (and changes nothing) for a stale
    /// ticket or a reply with another line count.
    pub fn apply_preview(&mut self, ticket: PreviewTicket, figures: PreviewFigures) -> bool {
        let fresh = self.wanted.as_ref().is_some_and(|w| {
            w.sent == Some(ticket.0) && w.request.lines.len() == figures.lines.len()
        });
        if !fresh {
            return false;
        }
        if let Some(w) = self.wanted.take() {
            self.applied = Some(Applied {
                request: w.request,
                figures,
            });
        }
        true
    }

    /// Records that the request of `ticket` failed ("Can't calculate").
    /// A stale ticket changes nothing.
    pub fn fail(&mut self, ticket: PreviewTicket) {
        if let Some(w) = &mut self.wanted
            && w.sent == Some(ticket.0)
        {
            w.failed = true;
        }
    }

    /// True while a sent request has no answer yet.
    #[must_use]
    pub fn in_flight(&self) -> bool {
        self.wanted
            .as_ref()
            .is_some_and(|w| w.sent.is_some() && !w.failed)
    }

    /// What the totals area shows.
    #[must_use]
    pub fn totals(&self) -> TotalsView<'_> {
        match (&self.wanted, &self.applied) {
            (Some(w), _) if w.failed => TotalsView::CantCalculate,
            (Some(_), _) => TotalsView::Updating,
            (None, Some(a)) => TotalsView::Ready(&a.figures.totals),
            (None, None) => TotalsView::Nothing,
        }
    }

    /// What the line `key` shows.
    #[must_use]
    pub fn line(&self, key: RowKey) -> LineView<'_> {
        match (&self.wanted, &self.applied) {
            (Some(w), _) if w.request.lines.iter().any(|l| l.key == key) => {
                if w.failed {
                    LineView::CantCalculate
                } else {
                    LineView::Updating
                }
            }
            (None, Some(a)) => a
                .request
                .lines
                .iter()
                .position(|l| l.key == key)
                .and_then(|i| a.figures.lines.get(i))
                .map_or(LineView::Nothing, LineView::Ready),
            _ => LineView::Nothing,
        }
    }

    /// The due date to show: the owner's, else the engine's from the
    /// current preview.
    #[must_use]
    pub fn shown_due_date(&self, form: &InvoiceForm) -> Option<CalendarDate> {
        form.due_date.or(match self.totals() {
            TotalsView::Ready(t) => t.due_date,
            _ => None,
        })
    }
}

#[cfg(test)]
#[path = "invoice_tests.rs"]
mod tests;
