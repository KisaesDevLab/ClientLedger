//! Invoices.

use super::{FormDoc, FormStyle, ItemLine, TotalRow, item_columns, item_rows, layout_form};
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
    /// Message to the customer.
    pub message: String,
}

/// The invoice as a template document.
#[must_use]
pub fn form(d: &InvoiceData, style: &FormStyle) -> FormDoc {
    let l = &style.labels;
    let mut fields = vec![
        (l.number.clone(), d.number.clone()),
        (l.date.clone(), d.date.clone()),
    ];
    for (label, v) in [
        (&l.due_date, &d.due_date),
        (&l.terms, &d.terms),
        (&l.po_number, &d.po_number),
    ] {
        if !v.is_empty() {
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
    FormDoc {
        title: l.invoice.clone(),
        number: d.number.clone(),
        address_label: l.bill_to.clone(),
        address: d.bill_to.clone(),
        fields,
        columns: item_columns(l),
        rows: item_rows(&d.lines, d.hide_zero_lines),
        totals,
        message: d.message.clone(),
        boxes: vec![],
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
                },
                ItemLine {
                    description: "Delivery (waived)".into(),
                    quantity: Some(Decimal::ONE),
                    rate: Some(Decimal::ZERO),
                    amount: Some(Decimal::ZERO),
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
}
