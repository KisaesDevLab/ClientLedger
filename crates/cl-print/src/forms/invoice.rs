//! Invoices (P19; P20 adds the ship-to block, the service-date column,
//! multi-line descriptions and [`pdf_bytes`]).

use super::{Column, FormDoc, FormStyle, ItemLine, TotalRow, item_table, layout_form, shown};
use crate::PrintError;
use crate::layout::Document;
use rust_decimal::Decimal;

/// An invoice as the engine reports it (amounts parsed by the caller,
/// dates formatted by the caller).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InvoiceData {
    /// Invoice number (prefix included).
    pub number: String,
    /// Invoice date.
    pub date: String,
    /// Due date (may be empty).
    pub due_date: String,
    /// Terms (may be empty).
    pub terms: String,
    /// Customer's P.O. number (may be empty).
    pub po_number: String,
    /// Bill-to address, name first.
    pub bill_to: Vec<String>,
    /// Ship-to address, name first; printed only when it differs from the
    /// bill-to address (Q-P20m).
    pub ship_to: Vec<String>,
    /// Item lines.
    pub lines: Vec<ItemLine>,
    /// Leave out lines whose amount is zero (Q-201; off by default).
    pub hide_zero_lines: bool,
    /// The engine's subtotal.
    pub subtotal: Decimal,
    /// Sales tax, if any.
    pub tax: Option<Decimal>,
    /// The engine's total.
    pub total: Decimal,
    /// Payments and credits applied, if any.
    pub payments: Option<Decimal>,
    /// The engine's balance due.
    pub balance_due: Decimal,
    /// Message to the customer (printed; the memo never is, Q-P20k).
    pub message: String,
    /// The invoice's print layout (U-130, Q-P20ac); `None` prints the
    /// standard form.
    pub layout: Option<PrintLayout>,
    /// Each line's item name, for a layout's Item column (same order as
    /// `lines`; may be shorter).
    pub item_names: Vec<String>,
}

/// What a print layout changes on the form (U-130): its columns, the
/// ship-to block, the P.O. number and terms fields. Its footer is the
/// caller's `FormStyle::footer`.
#[allow(clippy::struct_excessive_bools)] // one switch per option
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrintLayout {
    /// Item column.
    pub item: bool,
    /// Description column.
    pub description: bool,
    /// Service date column (printed when a line has a date).
    pub service_date: bool,
    /// Quantity column.
    pub quantity: bool,
    /// Rate column.
    pub rate: bool,
    /// Ship-to block.
    pub show_ship_to: bool,
    /// P.O. number field.
    pub show_po_number: bool,
    /// Terms field.
    pub show_terms: bool,
}

impl Default for PrintLayout {
    fn default() -> Self {
        PrintLayout {
            item: false,
            description: true,
            service_date: true,
            quantity: true,
            rate: true,
            show_ship_to: true,
            show_po_number: true,
            show_terms: true,
        }
    }
}

/// The item table as a layout chooses it: Item, Description, Service
/// date, Quantity, Rate (each when on), then Amount (always). The
/// description takes the width the others leave (190.5 mm in all).
fn layout_table(
    l: &super::FormLabels,
    d: &InvoiceData,
    p: PrintLayout,
) -> (Vec<Column>, Vec<Vec<String>>) {
    use crate::amount::{format_amount, format_quantity};
    let dated = p.service_date
        && shown(&d.lines, d.hide_zero_lines).any(|x| !x.service_date.trim().is_empty());
    let mut cols: Vec<Column> = Vec::new();
    let mut fixed = 35.5;
    if p.item {
        fixed += 35.0;
    }
    if dated {
        fixed += 22.0;
    }
    if p.quantity {
        fixed += 22.0;
    }
    if p.rate {
        fixed += 30.0;
    }
    if p.item {
        let w = if p.description {
            35.0
        } else {
            190.5 - fixed + 35.0
        };
        cols.push(Column::text("item", &l.item, w));
    }
    if p.description || !p.item {
        cols.push(Column::text("description", &l.description, 190.5 - fixed));
    }
    if dated {
        cols.push(Column::code("service_date", &l.service_date, 22.0));
    }
    if p.quantity {
        cols.push(Column::number("quantity", &l.quantity, 22.0));
    }
    if p.rate {
        cols.push(Column::number("rate", &l.rate, 30.0));
    }
    cols.push(Column::number("amount", &l.amount, 35.5));
    let index: Vec<usize> = d
        .lines
        .iter()
        .enumerate()
        .filter(|(_, x)| !(d.hide_zero_lines && x.amount.is_some_and(|a| a.is_zero())))
        .map(|(i, _)| i)
        .collect();
    let rows = index
        .into_iter()
        .map(|i| {
            let x = &d.lines[i];
            let mut row = Vec::new();
            if p.item {
                row.push(d.item_names.get(i).cloned().unwrap_or_default());
            }
            if p.description || !p.item {
                row.push(x.description.clone());
            }
            if dated {
                row.push(x.service_date.trim().to_string());
            }
            if p.quantity {
                row.push(x.quantity.map(format_quantity).unwrap_or_default());
            }
            if p.rate {
                row.push(x.rate.map(format_quantity).unwrap_or_default());
            }
            row.push(x.amount.map(format_amount).unwrap_or_default());
            row
        })
        .collect();
    (cols, rows)
}

/// The lines of an address as compared: trimmed, blank lines dropped,
/// spaces collapsed, letters folded to lower case.
fn address_key(lines: &[String]) -> Vec<String> {
    lines
        .iter()
        .map(|l| {
            l.split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase()
        })
        .filter(|l| !l.is_empty())
        .collect()
}

/// The ship-to block to print: empty when there is none or it is the
/// bill-to address (Q-P20m).
#[must_use]
pub fn ship_to_shown(d: &InvoiceData) -> Vec<String> {
    let ship = address_key(&d.ship_to);
    if ship.is_empty() || ship == address_key(&d.bill_to) {
        Vec::new()
    } else {
        d.ship_to
            .iter()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect()
    }
}

/// The invoice as a template document.
#[must_use]
pub fn form(d: &InvoiceData, style: &FormStyle) -> FormDoc {
    let l = &style.labels;
    let p = d.layout.unwrap_or_default();
    let mut fields = vec![
        (l.number.clone(), d.number.clone()),
        (l.date.clone(), d.date.clone()),
    ];
    for (label, v, on) in [
        (&l.due_date, &d.due_date, true),
        (&l.terms, &d.terms, p.show_terms),
        (&l.po_number, &d.po_number, p.show_po_number),
    ] {
        if on && !v.is_empty() {
            fields.push((label.clone(), v.clone()));
        }
    }
    let mut totals = vec![TotalRow::new(&l.subtotal, d.subtotal, false)];
    if let Some(t) = d.tax {
        totals.push(TotalRow::new(&l.tax, t, false));
    }
    totals.push(TotalRow::new(&l.total, d.total, d.payments.is_none()));
    if let Some(p) = d.payments {
        totals.push(TotalRow::new(&l.payments, p, false));
        totals.push(TotalRow::new(&l.balance_due, d.balance_due, true));
    }
    let (columns, rows) = match d.layout {
        Some(p) => layout_table(l, d, p),
        None => item_table(l, &d.lines, d.hide_zero_lines),
    };
    let ship_to = if p.show_ship_to {
        ship_to_shown(d)
    } else {
        Vec::new()
    };
    FormDoc {
        title: l.invoice.clone(),
        number: d.number.clone(),
        address_label: l.bill_to.clone(),
        address: d.bill_to.clone(),
        fields,
        columns,
        rows,
        totals,
        message: d.message.clone(),
        boxes: vec![],
        ship_to_label: if ship_to.is_empty() {
            String::new()
        } else {
            l.ship_to.clone()
        },
        ship_to,
    }
}

/// Lay out an invoice. `style.footer` is the footer already resolved by
/// the caller ([`super::resolve_footer`]).
///
/// # Errors
/// As [`layout_form`].
pub fn layout(d: &InvoiceData, style: &FormStyle) -> Result<Document, PrintError> {
    layout_form(&form(d, style), style)
}

/// The invoice as PDF bytes (Print, Save PDF, and the email attachment
/// `Invoice-<number>.pdf`, Q-P20q).
///
/// # Errors
/// As [`layout`] and [`crate::render`].
pub fn pdf_bytes(d: &InvoiceData, style: &FormStyle) -> Result<Vec<u8>, PrintError> {
    crate::render(&layout(d, style)?)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::super::tests::style;
    use super::super::{ADDRESS, WINDOW};
    use super::*;
    use crate::fonts::{Font, MM_PER_PT};
    use crate::layout::{LETTER_H_MM, resolve};
    use crate::pdf::read_back::{content_streams, text_origins};
    use crate::pdf::{pt, render_resolved};

    pub(crate) fn sample() -> InvoiceData {
        InvoiceData {
            number: "INV-1001".into(),
            date: "10/29/2026".into(),
            due_date: "11/28/2026".into(),
            terms: "Net 30".into(),
            bill_to: vec![
                "Pat Doe".into(),
                "5 Oak Lane".into(),
                "Columbus, OH 43004".into(),
            ],
            lines: vec![
                ItemLine {
                    description: "Mulch, 2 cu ft".into(),
                    quantity: Some(Decimal::new(25, 1)),
                    rate: Some(Decimal::new(450, 2)),
                    amount: Some(Decimal::new(1125, 2)),
                    ..ItemLine::default()
                },
                ItemLine {
                    description: "Delivery (waived)".into(),
                    quantity: Some(Decimal::ONE),
                    rate: Some(Decimal::ZERO),
                    amount: Some(Decimal::ZERO),
                    ..ItemLine::default()
                },
            ],
            subtotal: Decimal::new(1125, 2),
            tax: Some(Decimal::new(79, 2)),
            total: Decimal::new(1204, 2),
            payments: Some(Decimal::new(500, 2)),
            balance_due: Decimal::new(704, 2),
            message: "Thank you for your business.".into(),
            ..InvoiceData::default()
        }
    }

    fn text(p: &crate::ResolvedPage, tag: &str) -> String {
        p.runs_of(tag)
            .map(|r| r.text.clone())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn a_layout_chooses_the_columns_and_switches() {
        let mut d = sample();
        d.po_number = "PO-7".into();
        d.ship_to = vec!["Pat Doe".into(), "9 Elm St".into()];
        d.item_names = vec!["Mulch".into(), "Delivery".into()];
        d.layout = Some(PrintLayout {
            item: true,
            quantity: false,
            rate: false,
            show_po_number: false,
            show_ship_to: false,
            ..PrintLayout::default()
        });
        let f = form(&d, &style());
        let keys: Vec<&str> = f.columns.iter().map(|c| c.key.as_str()).collect();
        assert_eq!(keys, ["item", "description", "amount"]);
        let width: f32 = f.columns.iter().map(|c| c.width).sum();
        assert!((width - 190.5).abs() < 0.01, "{width}");
        assert_eq!(f.rows[0][0], "Mulch");
        assert!(f.fields.iter().all(|(_, v)| v != "PO-7"));
        assert_eq!(f.ship_to, Vec::<String>::new());
        // Without a layout the standard form prints.
        d.layout = None;
        let f = form(&d, &style());
        assert_eq!(f.columns.len(), 4);
        assert!(f.fields.iter().any(|(_, v)| v == "PO-7"));
        assert!(layout(&d, &style()).is_ok());
    }

    #[test]
    fn an_invoice_prints_header_lines_and_engine_totals() {
        let doc = layout(&sample(), &style()).unwrap();
        let p = &resolve(&doc).unwrap()[0];
        assert_eq!(text(p, "title"), "Invoice");
        assert_eq!(text(p, "field0.value"), "INV-1001");
        assert_eq!(text(p, "field3.label"), "Terms");
        assert_eq!(text(p, "row0.quantity"), "2.5");
        assert_eq!(text(p, "row0.rate"), "4.5");
        assert_eq!(text(p, "row0.amount"), "11.25");
        assert_eq!(text(p, "row1.amount"), "0.00");
        assert_eq!(text(p, "total2.label"), "Total");
        assert_eq!(text(p, "total4.label"), "Balance due");
        assert_eq!(text(p, "total4.value"), "7.04");
        assert_eq!(text(p, "message"), "Thank you for your business.");
        assert_eq!(doc.title, "Invoice INV-1001");
    }

    #[test]
    fn hide_zero_lines_is_off_by_default_and_hides_when_on() {
        assert!(!InvoiceData::default().hide_zero_lines);
        let mut d = sample();
        d.hide_zero_lines = true;
        let p = &resolve(&layout(&d, &style()).unwrap()).unwrap()[0];
        assert!(p.runs_of("row1.amount").next().is_none());
        assert_eq!(text(p, "row0.amount"), "11.25");
    }

    #[test]
    fn the_pdf_draws_the_invoice_where_the_layout_put_it() {
        let doc = layout(&sample(), &style()).unwrap();
        let pages = resolve(&doc).unwrap();
        let bytes = render_resolved(&pages, None, &doc.title).unwrap();
        let streams = content_streams(&bytes);
        assert_eq!(streams.len(), 1);
        let origins = text_origins(&streams[0]);
        assert_eq!(origins.len(), pages[0].runs.len());
        for ((x, y), run) in origins.iter().zip(&pages[0].runs) {
            let ym = (pt(LETTER_H_MM) - y) * MM_PER_PT;
            assert!(
                (x * MM_PER_PT - run.x).abs() < 0.01 && (ym - run.baseline).abs() < 0.01,
                "{}",
                run.tag
            );
        }
        // The window address against the spec, from the PDF itself.
        let i = pages[0]
            .runs
            .iter()
            .position(|r| r.tag == "address")
            .unwrap();
        let (x, y) = origins[i];
        let (xm, ym) = (x * MM_PER_PT, (pt(LETTER_H_MM) - y) * MM_PER_PT);
        assert!((xm - ADDRESS.x).abs() <= 0.5);
        assert!((ym - (ADDRESS.y + Font::Sans.ascent_mm(10.0))).abs() <= 0.5);
        assert!(WINDOW.contains(ADDRESS));
    }

    fn y_of(p: &crate::ResolvedPage, tag: &str) -> f32 {
        p.runs_of(tag).map(|r| r.baseline).fold(f32::MAX, f32::min)
    }

    #[test]
    fn ship_to_prints_only_when_it_differs_and_outside_the_windows() {
        use super::super::{SHIP_TO_X, TABLE_TOP_FIRST, TOP_WINDOW, table_top_first};
        let mut d = sample();
        d.ship_to = vec![
            " pat doe ".into(),
            "5  Oak Lane".into(),
            String::new(),
            "COLUMBUS, OH 43004".into(),
        ];
        assert!(
            ship_to_shown(&d).is_empty(),
            "the same address, written differently"
        );
        let p = &resolve(&layout(&d, &style()).unwrap()).unwrap()[0];
        assert!(p.runs_of("ship_to").next().is_none());
        d.ship_to = vec![
            "Hartley job site".into(),
            "88 River Road".into(),
            "Columbus, OH 43004".into(),
        ];
        let f = form(&d, &style());
        assert_eq!(f.ship_to_label, "Ship to");
        assert!(table_top_first(&f) >= TABLE_TOP_FIRST);
        let p = &resolve(&layout(&d, &style()).unwrap()).unwrap()[0];
        assert_eq!(text(p, "ship_to_label"), "Ship to");
        assert_eq!(
            text(p, "ship_to"),
            "Hartley job site\n88 River Road\nColumbus, OH 43004"
        );
        for r in p.runs_of("ship_to") {
            assert!(r.x >= SHIP_TO_X - 0.01, "right of the windows");
            assert!(r.x > WINDOW.x + WINDOW.w);
            assert!(r.x > TOP_WINDOW.x + TOP_WINDOW.w);
        }
        // The lines table starts below the block.
        let last_ship = p
            .runs_of("ship_to")
            .map(|r| r.baseline)
            .fold(0.0_f32, f32::max);
        assert!(y_of(p, "head.description") > last_ship);
    }

    #[test]
    fn service_dates_get_a_column_and_descriptions_keep_their_lines() {
        let mut d = sample();
        let p = &resolve(&layout(&d, &style()).unwrap()).unwrap()[0];
        assert!(p.runs_of("head.service_date").next().is_none());
        d.lines[0].service_date = "10/31/2026".into();
        d.lines[0].description = "Mulch, 2 cu ft\nspread on the north beds".into();
        let p = &resolve(&layout(&d, &style()).unwrap()).unwrap()[0];
        assert_eq!(text(p, "head.service_date"), "Service date");
        assert_eq!(text(p, "row0.service_date"), "10/31/2026");
        assert!(p.runs_of("row1.service_date").next().is_none());
        assert_eq!(text(p, "row0.amount"), "11.25");
        assert_eq!(
            text(p, "row0.description"),
            "Mulch, 2 cu ft\nspread on the north beds"
        );
        assert!(y_of(p, "row1.description") - y_of(p, "row0.description") > 6.0);
    }

    #[test]
    fn pdf_bytes_are_a_pdf_with_the_customer_message() {
        let bytes = pdf_bytes(&sample(), &style()).unwrap();
        assert!(bytes.starts_with(b"%PDF"));
        assert!(bytes.len() > 1000);
        let p = &resolve(&layout(&sample(), &style()).unwrap()).unwrap()[0];
        assert_eq!(text(p, "message"), "Thank you for your business.");
    }
}
