//! Sales receipts.

use super::{FormDoc, FormStyle, ItemLine, TotalRow, item_columns, item_rows, layout_form};
use crate::PrintError;
use crate::layout::Document;
use rust_decimal::Decimal;

/// A sales receipt as the engine reports it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SalesReceiptData {
    /// Receipt number.
    pub number: String,
    /// Sale date.
    pub date: String,
    /// How the customer paid.
    pub payment_method: String,
    /// Check or reference number (may be empty).
    pub reference: String,
    /// Sold-to address, name first (may be empty).
    pub sold_to: Vec<String>,
    /// Item lines.
    pub lines: Vec<ItemLine>,
    /// Leave out lines whose amount is zero (Q-201).
    pub hide_zero_lines: bool,
    /// The engine's subtotal.
    pub subtotal: Decimal,
    /// Sales tax, if any.
    pub tax: Option<Decimal>,
    /// The engine's total.
    pub total: Decimal,
    /// Message to the customer.
    pub message: String,
}

/// The receipt as a template document.
#[must_use]
pub fn form(d: &SalesReceiptData, style: &FormStyle) -> FormDoc {
    let l = &style.labels;
    let mut fields = vec![
        (l.number.clone(), d.number.clone()),
        (l.date.clone(), d.date.clone()),
    ];
    for (label, v) in [
        (&l.payment_method, &d.payment_method),
        (&l.reference, &d.reference),
    ] {
        if !v.is_empty() {
            fields.push((label.clone(), v.clone()));
        }
    }
    let mut totals = vec![TotalRow::new(&l.subtotal, d.subtotal, false)];
    if let Some(t) = d.tax {
        totals.push(TotalRow::new(&l.tax, t, false));
    }
    totals.push(TotalRow::new(&l.total, d.total, true));
    FormDoc {
        title: l.sales_receipt.clone(),
        number: d.number.clone(),
        address_label: if d.sold_to.is_empty() {
            String::new()
        } else {
            l.sold_to.clone()
        },
        address: d.sold_to.clone(),
        fields,
        columns: item_columns(l),
        rows: item_rows(&d.lines, d.hide_zero_lines),
        totals,
        message: d.message.clone(),
        boxes: vec![],
        ..FormDoc::default()
    }
}

/// Lay out a sales receipt.
///
/// # Errors
/// As [`layout_form`].
pub fn layout(d: &SalesReceiptData, style: &FormStyle) -> Result<Document, PrintError> {
    layout_form(&form(d, style), style)
}

#[cfg(test)]
mod tests {
    use super::super::tests::style;
    use super::*;
    use crate::layout::resolve;

    #[test]
    fn a_receipt_shows_payment_details_and_total() {
        let d = SalesReceiptData {
            number: "SR-0042".into(),
            date: "10/29/2026".into(),
            payment_method: "Check".into(),
            reference: "5512".into(),
            lines: vec![ItemLine {
                description: "Seeds".into(),
                quantity: Some(Decimal::TWO),
                rate: Some(Decimal::new(399, 2)),
                amount: Some(Decimal::new(798, 2)),
                ..ItemLine::default()
            }],
            subtotal: Decimal::new(798, 2),
            total: Decimal::new(798, 2),
            ..SalesReceiptData::default()
        };
        let p = &resolve(&layout(&d, &style()).unwrap()).unwrap()[0];
        let t = |tag: &str| {
            p.runs_of(tag)
                .next()
                .map(|r| r.text.clone())
                .unwrap_or_default()
        };
        assert_eq!(t("title"), "Sales Receipt");
        assert_eq!(t("field2.value"), "Check");
        assert_eq!(t("field3.value"), "5512");
        assert_eq!(t("total1.label"), "Total");
        assert_eq!(t("total1.value"), "7.98");
        assert_eq!(t("address_label"), "");
    }
}
