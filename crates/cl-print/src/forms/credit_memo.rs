//! Credit memos.

use super::{FormDoc, FormStyle, ItemLine, TotalRow, item_columns, item_rows, layout_form};
use crate::PrintError;
use crate::layout::Document;
use rust_decimal::Decimal;

/// A credit memo as the engine reports it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CreditMemoData {
    /// Credit memo number.
    pub number: String,
    /// Date.
    pub date: String,
    /// Customer address, name first.
    pub customer: Vec<String>,
    /// Item lines.
    pub lines: Vec<ItemLine>,
    /// Leave out lines whose amount is zero (Q-201).
    pub hide_zero_lines: bool,
    /// The engine's subtotal.
    pub subtotal: Decimal,
    /// Sales tax, if any.
    pub tax: Option<Decimal>,
    /// The engine's total credit.
    pub total: Decimal,
    /// Credit not yet used, if the engine reports it.
    pub remaining: Option<Decimal>,
    /// Message to the customer.
    pub message: String,
}

/// The credit memo as a template document.
#[must_use]
pub fn form(d: &CreditMemoData, style: &FormStyle) -> FormDoc {
    let l = &style.labels;
    let mut totals = vec![TotalRow::new(&l.subtotal, d.subtotal, false)];
    if let Some(t) = d.tax {
        totals.push(TotalRow::new(&l.tax, t, false));
    }
    totals.push(TotalRow::new(&l.total_credit, d.total, true));
    if let Some(r) = d.remaining {
        totals.push(TotalRow::new(&l.remaining_credit, r, false));
    }
    FormDoc {
        title: l.credit_memo.clone(),
        number: d.number.clone(),
        address_label: l.customer.clone(),
        address: d.customer.clone(),
        fields: vec![
            (l.number.clone(), d.number.clone()),
            (l.date.clone(), d.date.clone()),
        ],
        columns: item_columns(l),
        rows: item_rows(&d.lines, d.hide_zero_lines),
        totals,
        message: d.message.clone(),
        boxes: vec![],
    }
}

/// Lay out a credit memo.
///
/// # Errors
/// As [`layout_form`].
pub fn layout(d: &CreditMemoData, style: &FormStyle) -> Result<Document, PrintError> {
    layout_form(&form(d, style), style)
}

#[cfg(test)]
mod tests {
    use super::super::tests::style;
    use super::*;
    use crate::layout::resolve;

    #[test]
    fn a_credit_memo_shows_total_and_remaining_credit() {
        let d = CreditMemoData {
            number: "CM-7".into(),
            date: "10/29/2026".into(),
            customer: vec!["Pat Doe".into()],
            lines: vec![ItemLine {
                description: "Returned rake".into(),
                quantity: Some(Decimal::ONE),
                rate: Some(Decimal::new(2500, 2)),
                amount: Some(Decimal::new(2500, 2)),
            }],
            subtotal: Decimal::new(2500, 2),
            total: Decimal::new(2500, 2),
            remaining: Some(Decimal::new(1000, 2)),
            ..CreditMemoData::default()
        };
        let p = &resolve(&layout(&d, &style()).unwrap()).unwrap()[0];
        let t = |tag: &str| {
            p.runs_of(tag)
                .next()
                .map(|r| r.text.clone())
                .unwrap_or_default()
        };
        assert_eq!(t("title"), "Credit Memo");
        assert_eq!(t("address_label"), "Customer");
        assert_eq!(t("total1.label"), "Total credit");
        assert_eq!(t("total2.value"), "10.00");
    }
}
