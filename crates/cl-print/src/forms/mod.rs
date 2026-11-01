//! The built-in form template (Q-92–Q-95) and the documents that use it.
//!
//! One template: logo, company block (in the envelope's top window),
//! bill-to address at the #10 double-window position (Q-93), the document
//! title, number and date fields, a lines table that breaks across pages,
//! totals, and footer payment-instructions text on every page (Q-95; no
//! remittance stub in beta). Each document type ([`invoice`],
//! [`statement`], [`sales_receipt`], [`credit_memo`], [`deposit_summary`])
//! turns its own data into a [`FormDoc`].
//!
//! Totals are the engine's numbers, passed in by the caller; the template
//! never adds amounts up (the engine owns accounting rules).
//!
//! # Geometry
//!
//! A Letter sheet tri-folds at 93.1 mm with the top panel outermost; in a
//! #10 double-window envelope the recipient window then spans 48.6–77.2 mm
//! from the top and 22.2–136.5 mm across ([`WINDOW`]), and the top
//! (return-address) window about 1–26 mm down and 9.5–98 mm across, where
//! the company block goes.

pub mod credit_memo;
pub mod deposit_summary;
pub mod invoice;
pub mod sales_receipt;
pub mod statement;

use crate::PrintError;
use crate::fonts::Font;
use crate::image::Logo;
use crate::layout::{Align, Document, Element, Page, Rect, Shape, TextBox, line_height_mm};
use rust_decimal::Decimal;

/// Left margin.
pub const LEFT: f32 = 12.7;
/// Right margin (page width less 12.7 mm).
pub const RIGHT: f32 = 203.2;
/// The recipient window of a #10 double-window envelope on a tri-folded
/// sheet.
pub const WINDOW: Rect = Rect::new(22.2, 48.6, 114.3, 28.6);
/// The return-address window on a tri-folded sheet.
pub const TOP_WINDOW: Rect = Rect::new(9.5, 1.0, 88.9, 25.4);
/// The company name and address block (inside [`TOP_WINDOW`]).
pub const COMPANY: Rect = Rect::new(15.0, 6.0, 80.0, 19.0);
/// The logo box (top right).
pub const LOGO: Rect = Rect::new(150.0, 6.0, 53.2, 20.0);
/// The document title.
pub const TITLE: Rect = Rect::new(110.0, 30.0, 93.2, 9.0);
/// The bill-to label, just above the window.
pub const ADDRESS_LABEL: Rect = Rect::new(25.4, 45.5, 95.0, 4.0);
/// The bill-to address (inside [`WINDOW`]).
pub const ADDRESS: Rect = Rect::new(25.4, 50.8, 95.0, 22.0);
/// First header field (label, value) row.
pub const FIELDS_TOP: f32 = 50.8;
/// Header field row pitch.
pub const FIELD_PITCH: f32 = 5.0;
/// Top of the lines table on the first page.
pub const TABLE_TOP_FIRST: f32 = 82.0;
/// Top of the lines table on continuation pages.
pub const TABLE_TOP_NEXT: f32 = 25.0;
/// Nothing in the table body goes below this.
pub const TABLE_BOTTOM: f32 = 244.0;
/// The footer (payment instructions), on every page.
pub const FOOTER: Rect = Rect::new(LEFT, 248.0, RIGHT - LEFT, 14.0);
/// The page number.
pub const PAGE_NO: Rect = Rect::new(130.0, 266.0, RIGHT - 130.0, 4.0);
/// Table header row height.
pub const HEAD_H: f32 = 6.0;
/// Body text size.
pub const BODY_PT: f32 = 9.0;
/// Space above and below a row's text.
pub const ROW_PAD: f32 = 1.2;
/// Totals row pitch.
pub const TOTAL_H: f32 = 5.5;
/// Header field rows a form may have.
pub const MAX_FIELDS: usize = 5;

/// The company printed on every form.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CompanyInfo {
    /// Legal or trade name.
    pub name: String,
    /// Address, phone, email: one entry per printed line.
    pub lines: Vec<String>,
}

/// What every form shares: the company, its logo and the footer text.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FormStyle {
    /// The company block.
    pub company: CompanyInfo,
    /// The logo (Q-92), if the owner added one.
    pub logo: Option<Logo>,
    /// Payment instructions (Q-95), printed at the foot of every page.
    pub footer: String,
    /// Fixed words.
    pub labels: FormLabels,
}

/// The footer a form prints: the document type's own text when it has
/// any, else the company footer (owner 2026-10-29). Blank (whitespace
/// only) overrides fall back to the company text.
#[must_use]
pub fn resolve_footer(company: &str, override_text: Option<&str>) -> String {
    match override_text {
        Some(t) if !t.trim().is_empty() => t.to_string(),
        _ => company.to_string(),
    }
}

/// A table column.
#[derive(Debug, Clone, PartialEq)]
pub struct Column {
    /// Tag suffix for the column's cells (`row3.amount`).
    pub key: String,
    /// Heading.
    pub header: String,
    /// Width in millimetres; a form's widths add up to 190.5.
    pub width: f32,
    /// Alignment of heading and cells.
    pub align: Align,
    /// Numbers (Plex Mono) rather than text.
    pub numeric: bool,
    /// Wrap long text onto more lines (the row grows).
    pub wrap: bool,
}

impl Column {
    /// A text column that wraps.
    #[must_use]
    pub fn text(key: &str, header: &str, width: f32) -> Column {
        Column {
            key: key.into(),
            header: header.into(),
            width,
            align: Align::Left,
            numeric: false,
            wrap: true,
        }
    }

    /// A right-aligned number column.
    #[must_use]
    pub fn number(key: &str, header: &str, width: f32) -> Column {
        Column {
            key: key.into(),
            header: header.into(),
            width,
            align: Align::Right,
            numeric: true,
            wrap: false,
        }
    }

    /// A left-aligned number column (dates, reference numbers).
    #[must_use]
    pub fn code(key: &str, header: &str, width: f32) -> Column {
        Column {
            key: key.into(),
            header: header.into(),
            width,
            align: Align::Left,
            numeric: true,
            wrap: false,
        }
    }
}

/// A totals row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TotalRow {
    /// Label.
    pub label: String,
    /// Formatted amount.
    pub value: String,
    /// The bottom line (bold, rule above).
    pub strong: bool,
}

impl TotalRow {
    /// A row from an engine amount.
    #[must_use]
    pub fn new(label: &str, v: Decimal, strong: bool) -> TotalRow {
        TotalRow {
            label: label.into(),
            value: crate::amount::format_amount(v),
            strong,
        }
    }
}

/// A document ready for the template.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FormDoc {
    /// Title ("Invoice").
    pub title: String,
    /// The document number, for continuation headers (may be empty).
    pub number: String,
    /// Label above the address ("Bill to"), if any.
    pub address_label: String,
    /// The address block, name first (empty for none).
    pub address: Vec<String>,
    /// Header fields (label, value), at most [`MAX_FIELDS`].
    pub fields: Vec<(String, String)>,
    /// Table columns.
    pub columns: Vec<Column>,
    /// Table rows: one formatted cell per column.
    pub rows: Vec<Vec<String>>,
    /// Totals, top to bottom.
    pub totals: Vec<TotalRow>,
    /// A message beside the totals (customer message, memo).
    pub message: String,
    /// A row of labelled boxes under the totals (statement aging).
    pub boxes: Vec<(String, String)>,
    /// Label above the ship-to block ("Ship to"; P20, Q-P20m).
    pub ship_to_label: String,
    /// The ship-to block, name first; empty for none. Printed in the right
    /// column under the header fields, outside both envelope windows, and
    /// the lines table moves down below it.
    pub ship_to: Vec<String>,
}

/// Fixed words on forms (English defaults; the UI passes its
/// translations).
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(missing_docs)] // Each field is the English word it defaults to.
pub struct FormLabels {
    pub invoice: String,
    pub sales_receipt: String,
    pub credit_memo: String,
    pub statement: String,
    pub deposit_summary: String,
    pub bill_to: String,
    pub sold_to: String,
    pub customer: String,
    pub number: String,
    pub date: String,
    pub due_date: String,
    pub terms: String,
    pub po_number: String,
    pub ship_to: String,
    pub service_date: String,
    pub payment_method: String,
    pub reference: String,
    pub statement_date: String,
    pub deposit_to: String,
    pub item: String,
    pub description: String,
    pub quantity: String,
    pub rate: String,
    pub amount: String,
    pub received_from: String,
    pub memo: String,
    pub check_no: String,
    pub transaction: String,
    pub balance: String,
    pub balance_forward: String,
    pub subtotal: String,
    pub tax: String,
    pub total: String,
    pub payments: String,
    pub balance_due: String,
    pub total_credit: String,
    pub remaining_credit: String,
    pub deposit_subtotal: String,
    pub cash_back: String,
    pub deposit_total: String,
    pub amount_due: String,
    pub current: String,
    pub days_1_30: String,
    pub days_31_60: String,
    pub days_61_90: String,
    pub over_90: String,
    /// Appended to the title on continuation pages.
    pub continued: String,
    /// `{page}` and `{pages}` are replaced.
    pub page_of: String,
}

impl Default for FormLabels {
    fn default() -> FormLabels {
        let s = |v: &str| v.to_string();
        FormLabels {
            invoice: s("Invoice"),
            sales_receipt: s("Sales Receipt"),
            credit_memo: s("Credit Memo"),
            statement: s("Statement"),
            deposit_summary: s("Deposit Summary"),
            bill_to: s("Bill to"),
            sold_to: s("Sold to"),
            customer: s("Customer"),
            number: s("Number"),
            date: s("Date"),
            due_date: s("Due date"),
            terms: s("Terms"),
            po_number: s("P.O. number"),
            ship_to: s("Ship to"),
            service_date: s("Service date"),
            payment_method: s("Payment method"),
            reference: s("Reference"),
            statement_date: s("Statement date"),
            deposit_to: s("Deposit to"),
            item: s("Item"),
            description: s("Description"),
            quantity: s("Qty"),
            rate: s("Rate"),
            amount: s("Amount"),
            received_from: s("Received from"),
            memo: s("Memo"),
            check_no: s("Check no."),
            transaction: s("Transaction"),
            balance: s("Balance"),
            balance_forward: s("Balance forward"),
            subtotal: s("Subtotal"),
            tax: s("Sales tax"),
            total: s("Total"),
            payments: s("Payments and credits"),
            balance_due: s("Balance due"),
            total_credit: s("Total credit"),
            remaining_credit: s("Remaining credit"),
            deposit_subtotal: s("Deposit subtotal"),
            cash_back: s("Less cash back"),
            deposit_total: s("Deposit total"),
            amount_due: s("Amount due"),
            current: s("Current"),
            days_1_30: s("1-30 days"),
            days_31_60: s("31-60 days"),
            days_61_90: s("61-90 days"),
            over_90: s("Over 90 days"),
            continued: s("(continued)"),
            page_of: s("Page {page} of {pages}"),
        }
    }
}

/// An item line on invoices, sales receipts and credit memos.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ItemLine {
    /// Description (wraps).
    pub description: String,
    /// Quantity, if the line has one.
    pub quantity: Option<Decimal>,
    /// Rate, if the line has one.
    pub rate: Option<Decimal>,
    /// The engine's line amount; `None` for a text-only line.
    pub amount: Option<Decimal>,
    /// Service date as the caller formatted it (empty for none; P20). A
    /// Service date column is printed when any line has one.
    pub service_date: String,
}

/// The standard item columns (description, qty, rate, amount).
pub(crate) fn item_columns(l: &FormLabels) -> Vec<Column> {
    vec![
        Column::text("description", &l.description, 103.0),
        Column::number("quantity", &l.quantity, 22.0),
        Column::number("rate", &l.rate, 30.0),
        Column::number("amount", &l.amount, 35.5),
    ]
}

/// The item columns with a Service date column after the description
/// (P20), the description narrower by its width.
pub(crate) fn item_columns_dated(l: &FormLabels) -> Vec<Column> {
    vec![
        Column::text("description", &l.description, 81.0),
        Column::code("service_date", &l.service_date, 22.0),
        Column::number("quantity", &l.quantity, 22.0),
        Column::number("rate", &l.rate, 30.0),
        Column::number("amount", &l.amount, 35.5),
    ]
}

pub(crate) fn shown(lines: &[ItemLine], hide_zero: bool) -> impl Iterator<Item = &ItemLine> {
    lines
        .iter()
        .filter(move |l| !(hide_zero && l.amount.is_some_and(|a| a.is_zero())))
}

/// Item lines as rows; with `hide_zero` (Q-201) lines whose amount is
/// exactly zero are left out (text-only lines stay). A description may
/// span several lines (`\n`).
pub(crate) fn item_rows(lines: &[ItemLine], hide_zero: bool) -> Vec<Vec<String>> {
    use crate::amount::{format_amount, format_quantity};
    shown(lines, hide_zero)
        .map(|l| {
            vec![
                l.description.clone(),
                l.quantity.map(format_quantity).unwrap_or_default(),
                l.rate.map(format_quantity).unwrap_or_default(),
                l.amount.map(format_amount).unwrap_or_default(),
            ]
        })
        .collect()
}

/// The columns and rows of item lines: with a Service date column when a
/// shown line has a service date (P20).
pub(crate) fn item_table(
    l: &FormLabels,
    lines: &[ItemLine],
    hide_zero: bool,
) -> (Vec<Column>, Vec<Vec<String>>) {
    let dated = shown(lines, hide_zero).any(|x| !x.service_date.trim().is_empty());
    let rows = item_rows(lines, hide_zero);
    if !dated {
        return (item_columns(l), rows);
    }
    let rows = shown(lines, hide_zero)
        .zip(rows)
        .map(|(line, mut row)| {
            row.insert(1, line.service_date.trim().to_string());
            row
        })
        .collect();
    (item_columns_dated(l), rows)
}

/// Left edge of the ship-to block (right of both envelope windows).
pub const SHIP_TO_X: f32 = 140.0;
/// Most ship-to lines printed.
pub const SHIP_TO_MAX_LINES: usize = 6;
const SHIP_TO_PT: f32 = 9.0;

/// Where the ship-to label and block go, if the form has one.
fn ship_to_boxes(form: &FormDoc) -> Option<(Rect, Rect)> {
    if form.ship_to.is_empty() {
        return None;
    }
    let top = FIELDS_TOP + FIELD_PITCH * as_f32(form.fields.len()) + 2.0;
    let label = Rect::new(SHIP_TO_X, top, RIGHT - SHIP_TO_X, 4.0);
    let lines = form.ship_to.len().min(SHIP_TO_MAX_LINES);
    let body = Rect::new(
        SHIP_TO_X,
        top + 4.5,
        RIGHT - SHIP_TO_X,
        line_height_mm(SHIP_TO_PT) * as_f32(lines),
    );
    Some((label, body))
}

/// Where the lines table starts on the first page: below the ship-to block
/// when there is one.
#[must_use]
pub fn table_top_first(form: &FormDoc) -> f32 {
    ship_to_boxes(form).map_or(TABLE_TOP_FIRST, |(_, body)| {
        TABLE_TOP_FIRST.max(body.bottom() + 3.0)
    })
}

/// Lay a form out on as many pages as its rows need.
///
/// # Errors
/// [`PrintError::TooManyFields`] for more than [`MAX_FIELDS`] header
/// fields.
pub fn layout_form(form: &FormDoc, style: &FormStyle) -> Result<Document, PrintError> {
    if form.fields.len() > MAX_FIELDS {
        return Err(PrintError::TooManyFields);
    }
    let mut pages = vec![first_page(form, style)];
    let mut y = table_top_first(form);
    table_head(pages.last_mut(), form, y);
    y += HEAD_H + 1.0;
    for (i, row) in form.rows.iter().enumerate() {
        let h = row_height(form, row);
        if y + h > TABLE_BOTTOM {
            pages.push(next_page(form, style));
            y = TABLE_TOP_NEXT;
            table_head(pages.last_mut(), form, y);
            y += HEAD_H + 1.0;
        }
        if let Some(page) = pages.last_mut() {
            place_row(page, form, i, row, y);
        }
        y += h;
    }
    // Totals, message and boxes stay together on the last page.
    let need = 3.0 + closing_height(form);
    if y + need > TABLE_BOTTOM {
        pages.push(next_page(form, style));
        y = TABLE_TOP_NEXT;
    }
    if let Some(page) = pages.last_mut() {
        place_closing(page, form, y + 3.0);
    }
    let total = pages.len();
    for (i, page) in pages.iter_mut().enumerate() {
        if !style.footer.is_empty() {
            page.text(
                TextBox::new("footer", FOOTER, Font::Sans, 8.5, style.footer.clone()).wrapped(),
            );
        }
        if total > 1 {
            let text = style
                .labels
                .page_of
                .replace("{page}", &(i + 1).to_string())
                .replace("{pages}", &total.to_string());
            page.text(TextBox::new("page_no", PAGE_NO, Font::Sans, 8.0, text).align(Align::Right));
        }
    }
    Ok(Document {
        size: crate::layout::PageSize::LETTER,
        title: title_line(form),
        pages,
        logo: style.logo.clone(),
    })
}

fn title_line(form: &FormDoc) -> String {
    if form.number.is_empty() {
        form.title.clone()
    } else {
        format!("{} {}", form.title, form.number)
    }
}

fn first_page(form: &FormDoc, style: &FormStyle) -> Page {
    let mut p = Page::default();
    let name_h = 5.5;
    p.text(TextBox::new(
        "company.name",
        Rect::new(COMPANY.x, COMPANY.y, COMPANY.w, name_h),
        Font::SansBold,
        11.0,
        style.company.name.clone(),
    ));
    p.text(TextBox::new(
        "company.lines",
        Rect::new(COMPANY.x, COMPANY.y + name_h, COMPANY.w, COMPANY.h - name_h),
        Font::Sans,
        8.5,
        style.company.lines.join("\n"),
    ));
    if style.logo.is_some() {
        p.elements.push(Element::Logo(LOGO));
    }
    p.text(
        TextBox::new("title", TITLE, Font::SansBold, 18.0, form.title.clone()).align(Align::Right),
    );
    if !form.address_label.is_empty() {
        p.text(TextBox::new(
            "address_label",
            ADDRESS_LABEL,
            Font::SansSemiBold,
            8.0,
            form.address_label.clone(),
        ));
    }
    if !form.address.is_empty() {
        p.text(TextBox::new(
            "address",
            ADDRESS,
            Font::Sans,
            10.0,
            form.address.join("\n"),
        ));
    }
    let mut y = FIELDS_TOP;
    for (i, (label, value)) in form.fields.iter().enumerate() {
        p.text(TextBox::new(
            format!("field{i}.label"),
            Rect::new(130.0, y, 33.0, 4.5),
            Font::SansSemiBold,
            8.5,
            label.clone(),
        ));
        // Numbers, dates and codes in the figure face; words (an account or
        // terms name) in the text face.
        let font = if value.chars().any(char::is_lowercase) {
            Font::Sans
        } else {
            Font::Mono
        };
        p.text(
            TextBox::new(
                format!("field{i}.value"),
                Rect::new(163.0, y, RIGHT - 163.0, 4.5),
                font,
                9.0,
                value.clone(),
            )
            .align(Align::Right),
        );
        y += FIELD_PITCH;
    }
    if let Some((label, body)) = ship_to_boxes(form) {
        p.text(TextBox::new(
            "ship_to_label",
            label,
            Font::SansSemiBold,
            8.0,
            form.ship_to_label.clone(),
        ));
        let lines: Vec<&str> = form
            .ship_to
            .iter()
            .take(SHIP_TO_MAX_LINES)
            .map(String::as_str)
            .collect();
        p.text(TextBox::new(
            "ship_to",
            body,
            Font::Sans,
            SHIP_TO_PT,
            lines.join("\n"),
        ));
    }
    p
}

fn next_page(form: &FormDoc, style: &FormStyle) -> Page {
    let mut p = Page::default();
    p.text(TextBox::new(
        "company.name",
        Rect::new(LEFT, 12.7, 100.0, 5.0),
        Font::SansSemiBold,
        9.0,
        style.company.name.clone(),
    ));
    p.text(
        TextBox::new(
            "title",
            Rect::new(RIGHT - 100.0, 12.0, 100.0, 6.0),
            Font::SansBold,
            11.0,
            format!("{} {}", title_line(form), style.labels.continued),
        )
        .align(Align::Right),
    );
    p
}

fn table_head(page: Option<&mut Page>, form: &FormDoc, y: f32) {
    let Some(page) = page else { return };
    page.shape(Shape::Box {
        rect: Rect::new(LEFT, y, RIGHT - LEFT, HEAD_H),
        stroke_pt: None,
        fill_gray: Some(0.92),
    });
    let mut x = LEFT;
    for c in &form.columns {
        page.text(
            TextBox::new(
                format!("head.{}", c.key),
                Rect::new(x + 1.0, y + 1.0, c.width - 2.0, HEAD_H - 1.0),
                Font::SansSemiBold,
                8.5,
                c.header.clone(),
            )
            .align(c.align),
        );
        x += c.width;
    }
}

#[allow(clippy::many_single_char_names)] // x, y, h: geometry.
fn cell_box(c: &Column, x: f32, y: f32, h: f32, tag: String, text: String) -> TextBox {
    let font = if c.numeric { Font::Mono } else { Font::Sans };
    let mut b = TextBox::new(
        tag,
        Rect::new(x + 1.0, y + ROW_PAD / 2.0, c.width - 2.0, h),
        font,
        BODY_PT,
        text,
    )
    .align(c.align);
    if c.wrap {
        b = b.wrapped().unclipped();
    }
    b
}

/// A row's height: its tallest cell plus padding.
#[must_use]
pub fn row_height(form: &FormDoc, row: &[String]) -> f32 {
    let lh = line_height_mm(BODY_PT);
    let lines = form
        .columns
        .iter()
        .zip(row)
        .map(|(c, t)| {
            cell_box(c, 0.0, 0.0, lh, String::new(), t.clone())
                .lines()
                .len()
        })
        .max()
        .unwrap_or(1)
        .max(1);
    lh * as_f32(lines) + ROW_PAD
}

fn place_row(page: &mut Page, form: &FormDoc, i: usize, row: &[String], y: f32) {
    let lh = line_height_mm(BODY_PT);
    let mut x = LEFT;
    for (c, text) in form.columns.iter().zip(row) {
        if !text.is_empty() {
            page.text(cell_box(
                c,
                x,
                y,
                lh,
                format!("row{i}.{}", c.key),
                text.clone(),
            ));
        }
        x += c.width;
    }
    let bottom = y + row_height(form, row);
    page.shape(Shape::Line {
        x1: LEFT,
        y1: bottom,
        x2: RIGHT,
        y2: bottom,
        width_pt: 0.25,
    });
}

const MESSAGE_W: f32 = 100.0;

fn message_lines(form: &FormDoc) -> usize {
    if form.message.is_empty() {
        return 0;
    }
    TextBox::new(
        "",
        Rect::new(0.0, 0.0, MESSAGE_W, 100.0),
        Font::Sans,
        BODY_PT,
        form.message.clone(),
    )
    .wrapped()
    .unclipped()
    .lines()
    .len()
}

/// Height of totals (or message, if taller) plus the boxes row.
fn closing_height(form: &FormDoc) -> f32 {
    let totals = TOTAL_H * as_f32(form.totals.len()) + 1.0;
    let msg = line_height_mm(BODY_PT) * as_f32(message_lines(form).min(8));
    let boxes = if form.boxes.is_empty() { 0.0 } else { 16.0 };
    totals.max(msg) + boxes
}

fn place_closing(page: &mut Page, form: &FormDoc, top: f32) {
    let mut y = top;
    for (i, t) in form.totals.iter().enumerate() {
        if t.strong {
            page.shape(Shape::Line {
                x1: 120.0,
                y1: y,
                x2: RIGHT,
                y2: y,
                width_pt: 0.75,
            });
        }
        let font = if t.strong { Font::SansBold } else { Font::Sans };
        page.text(
            TextBox::new(
                format!("total{i}.label"),
                Rect::new(110.0, y + 0.6, 53.0, 5.0),
                font,
                BODY_PT,
                t.label.clone(),
            )
            .align(Align::Right),
        );
        page.text(
            TextBox::new(
                format!("total{i}.value"),
                Rect::new(165.0, y + 0.6, RIGHT - 166.0, 5.0),
                Font::Mono,
                BODY_PT,
                t.value.clone(),
            )
            .align(Align::Right),
        );
        y += TOTAL_H;
    }
    if !form.message.is_empty() {
        let h = line_height_mm(BODY_PT) * 8.0;
        page.text(
            TextBox::new(
                "message",
                Rect::new(LEFT, top + 0.6, MESSAGE_W, h),
                Font::Sans,
                BODY_PT,
                form.message.clone(),
            )
            .wrapped(),
        );
    }
    if !form.boxes.is_empty() {
        let by = top
            + TOTAL_H
                * as_f32(form.totals.len())
                    .max(as_f32(message_lines(form).min(8)) * line_height_mm(BODY_PT) / TOTAL_H)
            + 3.0;
        let w = (RIGHT - LEFT) / as_f32(form.boxes.len());
        for (i, (label, value)) in form.boxes.iter().enumerate() {
            let x = LEFT + w * as_f32(i);
            page.shape(Shape::Box {
                rect: Rect::new(x, by, w, 12.0),
                stroke_pt: Some(0.5),
                fill_gray: None,
            });
            page.text(
                TextBox::new(
                    format!("box{i}.label"),
                    Rect::new(x + 1.0, by + 1.0, w - 2.0, 4.0),
                    Font::SansSemiBold,
                    7.5,
                    label.clone(),
                )
                .align(Align::Center),
            );
            page.text(
                TextBox::new(
                    format!("box{i}.value"),
                    Rect::new(x + 1.0, by + 6.0, w - 2.0, 5.0),
                    Font::Mono,
                    9.0,
                    value.clone(),
                )
                .align(Align::Center),
            );
        }
    }
}

fn as_f32(n: usize) -> f32 {
    f32::from(u16::try_from(n).unwrap_or(u16::MAX))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::layout::resolve;

    pub(crate) fn style() -> FormStyle {
        FormStyle {
            company: CompanyInfo {
                name: "Kisaes Garden Supply".into(),
                lines: vec![
                    "12 Elm Street".into(),
                    "Bloomington, IN 47401".into(),
                    "(812) 555-0100".into(),
                ],
            },
            logo: None,
            footer: "Please pay by check to Kisaes Garden Supply or by bank transfer.".into(),
            labels: FormLabels::default(),
        }
    }

    pub(crate) fn rows(n: usize) -> Vec<Vec<String>> {
        (0..n)
            .map(|i| {
                vec![
                    format!("Item {i}"),
                    "1".into(),
                    "10.00".into(),
                    "10.00".into(),
                ]
            })
            .collect()
    }

    fn form(n: usize) -> FormDoc {
        FormDoc {
            title: "Invoice".into(),
            number: "1001".into(),
            address_label: "Bill to".into(),
            address: vec![
                "Pat Doe".into(),
                "5 Oak Lane".into(),
                "Columbus, OH 43004".into(),
            ],
            fields: vec![
                ("Number".into(), "1001".into()),
                ("Date".into(), "10/29/2026".into()),
            ],
            columns: item_columns(&FormLabels::default()),
            rows: rows(n),
            totals: vec![TotalRow::new("Total", Decimal::new(1000, 2), true)],
            message: String::new(),
            boxes: vec![],
            ..FormDoc::default()
        }
    }

    #[test]
    fn the_bill_to_address_sits_in_the_envelope_window_within_half_a_millimetre() {
        assert!(WINDOW.contains(ADDRESS));
        assert!(TOP_WINDOW.contains(COMPANY));
        let doc = layout_form(&form(3), &style()).unwrap();
        let page = &resolve(&doc).unwrap()[0];
        let lines: Vec<_> = page.runs_of("address").collect();
        assert_eq!(lines.len(), 3);
        let first = lines[0];
        assert!((first.x - 25.4).abs() <= 0.5);
        assert!((first.baseline - (50.8 + Font::Sans.ascent_mm(10.0))).abs() <= 0.5);
        for l in &lines {
            assert!(
                l.x >= WINDOW.x
                    && l.baseline <= WINDOW.bottom()
                    && l.baseline - Font::Sans.ascent_mm(10.0) >= WINDOW.y
            );
        }
        let name = page.runs_of("company.name").next().unwrap();
        assert!(TOP_WINDOW.contains(Rect::new(name.x, name.baseline - 4.0, 1.0, 4.0)));
    }

    /// Whole rows of height `one` that fit in `space`.
    fn fits(space: f32, one: f32) -> usize {
        let mut n = 0;
        let mut used = one;
        while used <= space {
            n += 1;
            used += one;
        }
        n
    }

    #[test]
    fn rows_break_onto_continuation_pages_with_totals_on_the_last() {
        let one = row_height(&form(1), &form(1).rows[0]);
        let first_fit = fits(TABLE_BOTTOM - TABLE_TOP_FIRST - HEAD_H - 1.0, one);
        let next_fit = fits(TABLE_BOTTOM - TABLE_TOP_NEXT - HEAD_H - 1.0, one);
        let n = first_fit + next_fit + 3;
        let doc = layout_form(&form(n), &style()).unwrap();
        assert_eq!(doc.pages.len(), 3);
        let pages = resolve(&doc).unwrap();
        assert!(
            pages[0]
                .runs_of(&format!("row{}.description", first_fit - 1))
                .next()
                .is_some()
        );
        assert!(
            pages[0]
                .runs_of(&format!("row{first_fit}.description"))
                .next()
                .is_none()
        );
        assert!(
            pages[1]
                .runs_of(&format!("row{first_fit}.description"))
                .next()
                .is_some()
        );
        assert!(
            pages[2]
                .runs_of(&format!("row{}.description", n - 1))
                .next()
                .is_some()
        );
        // Every row is somewhere, exactly once.
        for i in 0..n {
            let count: usize = pages
                .iter()
                .map(|p| p.runs_of(&format!("row{i}.amount")).count())
                .sum();
            assert_eq!(count, 1, "row {i}");
        }
        for (i, p) in pages.iter().enumerate() {
            assert_eq!(
                p.runs_of("page_no").next().unwrap().text,
                format!("Page {} of 3", i + 1)
            );
            assert!(p.runs_of("footer").next().is_some());
            assert!(
                p.runs
                    .iter()
                    .filter(|r| r.tag.starts_with("row"))
                    .all(|r| r.baseline < TABLE_BOTTOM)
            );
        }
        assert_eq!(
            pages[1].runs_of("title").next().unwrap().text,
            "Invoice 1001 (continued)"
        );
        assert!(pages[2].runs_of("total0.value").next().is_some());
        assert!(pages[0].runs_of("total0.value").next().is_none());
    }

    #[test]
    fn totals_that_do_not_fit_move_to_a_new_page() {
        let one = row_height(&form(1), &form(1).rows[0]);
        let first_fit = fits(TABLE_BOTTOM - TABLE_TOP_FIRST - HEAD_H - 1.0, one);
        let doc = layout_form(&form(first_fit), &style()).unwrap();
        let pages = resolve(&doc).unwrap();
        assert_eq!(pages.len(), 2);
        assert!(pages[1].runs.iter().all(|r| !r.tag.starts_with("row")));
        assert!(pages[1].runs_of("total0.value").next().is_some());
    }

    #[test]
    fn a_single_page_form_has_no_page_number_and_wrapped_rows_grow() {
        let mut f = form(1);
        f.rows[0][0] = "A long description that certainly needs more than one line in a column one hundred millimetres wide, maybe three".into();
        assert!(row_height(&f, &f.rows[0]) > 2.0 * line_height_mm(BODY_PT));
        let pages = resolve(&layout_form(&f, &style()).unwrap()).unwrap();
        assert_eq!(pages.len(), 1);
        assert!(pages[0].runs_of("page_no").next().is_none());
        assert!(pages[0].runs_of("row0.description").count() >= 2);
    }

    #[test]
    fn a_blank_footer_override_falls_back_to_the_company_footer() {
        assert_eq!(resolve_footer("Pay by check.", None), "Pay by check.");
        assert_eq!(resolve_footer("Pay by check.", Some("  ")), "Pay by check.");
        assert_eq!(resolve_footer("Pay by check.", Some("Net 30.")), "Net 30.");
        let mut s = style();
        s.footer = resolve_footer(&s.footer, Some("Thanks!"));
        let pages = resolve(&layout_form(&form(1), &s).unwrap()).unwrap();
        assert_eq!(pages[0].runs_of("footer").next().unwrap().text, "Thanks!");
    }

    #[test]
    fn numeric_cells_shrink_to_fit_and_refuse_what_cannot_fit() {
        let mut f = form(1);
        f.rows[0][1] = "1,234,567.8901".into(); // qty: wider than 20 mm at 9 pt
        f.rows[0][3] = "9,999,999,999.99".into();
        let pages = resolve(&layout_form(&f, &style()).unwrap()).unwrap();
        let q = pages[0].runs_of("row0.quantity").next().unwrap();
        assert_eq!(q.text, "1,234,567.8901");
        assert!(q.size_pt < BODY_PT);
        assert!(
            q.x >= LEFT + 103.0 && q.x + Font::Mono.width_mm(&q.text, q.size_pt) <= LEFT + 125.0
        );
        assert_eq!(
            pages[0].runs_of("row0.amount").next().unwrap().text,
            "9,999,999,999.99"
        );
        f.rows[0][1] = "123,456,789,012,345.678".into();
        let doc = layout_form(&f, &style()).unwrap();
        assert_eq!(
            resolve(&doc),
            Err(PrintError::DoesNotFit("row0.quantity".into()))
        );
    }

    #[test]
    fn too_many_header_fields_are_refused() {
        let mut f = form(1);
        f.fields = vec![("a".into(), "b".into()); 6];
        assert_eq!(layout_form(&f, &style()), Err(PrintError::TooManyFields));
    }

    #[test]
    fn hide_zero_lines_drops_zero_amounts_but_keeps_text_lines() {
        let lines = vec![
            ItemLine {
                description: "Paid".into(),
                quantity: Some(Decimal::ONE),
                rate: Some(Decimal::TEN),
                amount: Some(Decimal::TEN),
                ..ItemLine::default()
            },
            ItemLine {
                description: "Free".into(),
                quantity: Some(Decimal::ONE),
                rate: Some(Decimal::ZERO),
                amount: Some(Decimal::new(0, 2)),
                ..ItemLine::default()
            },
            ItemLine {
                description: "Note".into(),
                quantity: None,
                rate: None,
                amount: None,
                ..ItemLine::default()
            },
        ];
        assert_eq!(item_rows(&lines, false).len(), 3);
        let shown = item_rows(&lines, true);
        assert_eq!(
            shown.iter().map(|r| r[0].as_str()).collect::<Vec<_>>(),
            ["Paid", "Note"]
        );
        assert_eq!(shown[0][3], "10.00");
        assert_eq!(shown[1][3], "");
    }
}
