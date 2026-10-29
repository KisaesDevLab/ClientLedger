//! Deposit summaries (for the owner's records; no window address).

use super::{Column, FormDoc, FormStyle, TotalRow, layout_form};
use crate::PrintError;
use crate::amount::format_amount;
use crate::layout::Document;
use rust_decimal::Decimal;

/// One payment in the deposit.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DepositLine {
    /// Customer or other payer.
    pub received_from: String,
    /// Memo.
    pub memo: String,
    /// Check number (may be empty).
    pub check_no: String,
    /// Payment method.
    pub payment_method: String,
    /// The engine's amount.
    pub amount: Decimal,
}

/// A deposit as the engine reports it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DepositSummaryData {
    /// The bank account deposited to.
    pub account: String,
    /// Deposit date.
    pub date: String,
    /// Payments deposited.
    pub lines: Vec<DepositLine>,
    /// The engine's subtotal.
    pub subtotal: Decimal,
    /// Cash back, if any.
    pub cash_back: Option<Decimal>,
    /// The engine's deposit total.
    pub total: Decimal,
    /// Deposit memo.
    pub memo: String,
}

/// The deposit summary as a template document.
#[must_use]
pub fn form(d: &DepositSummaryData, style: &FormStyle) -> FormDoc {
    let l = &style.labels;
    let mut totals = vec![TotalRow::new(&l.deposit_subtotal, d.subtotal, false)];
    if let Some(c) = d.cash_back {
        totals.push(TotalRow::new(&l.cash_back, c, false));
    }
    totals.push(TotalRow::new(&l.deposit_total, d.total, true));
    FormDoc {
        title: l.deposit_summary.clone(),
        number: String::new(),
        address_label: String::new(),
        address: vec![],
        fields: vec![
            (l.deposit_to.clone(), d.account.clone()),
            (l.date.clone(), d.date.clone()),
        ],
        columns: vec![
            Column::text("received_from", &l.received_from, 55.0),
            Column::text("memo", &l.memo, 55.0),
            Column::code("check_no", &l.check_no, 22.0),
            Column::text("payment_method", &l.payment_method, 28.0),
            Column::number("amount", &l.amount, 30.5),
        ],
        rows: d
            .lines
            .iter()
            .map(|x| {
                vec![
                    x.received_from.clone(),
                    x.memo.clone(),
                    x.check_no.clone(),
                    x.payment_method.clone(),
                    format_amount(x.amount),
                ]
            })
            .collect(),
        totals,
        message: d.memo.clone(),
        boxes: vec![],
    }
}

/// Lay out a deposit summary.
///
/// # Errors
/// As [`layout_form`].
pub fn layout(d: &DepositSummaryData, style: &FormStyle) -> Result<Document, PrintError> {
    layout_form(&form(d, style), style)
}

#[cfg(test)]
mod tests {
    use super::super::tests::style;
    use super::*;
    use crate::layout::resolve;

    #[test]
    fn a_deposit_summary_lists_payments_and_totals() {
        let d = DepositSummaryData {
            account: "Checking".into(),
            date: "10/29/2026".into(),
            lines: vec![
                DepositLine {
                    received_from: "Pat Doe".into(),
                    check_no: "5512".into(),
                    payment_method: "Check".into(),
                    amount: Decimal::new(704, 2),
                    ..DepositLine::default()
                },
                DepositLine {
                    received_from: "Lee Ray".into(),
                    payment_method: "Cash".into(),
                    amount: Decimal::new(2000, 2),
                    ..DepositLine::default()
                },
            ],
            subtotal: Decimal::new(2704, 2),
            cash_back: Some(Decimal::new(500, 2)),
            total: Decimal::new(2204, 2),
            ..DepositSummaryData::default()
        };
        let p = &resolve(&layout(&d, &style()).unwrap()).unwrap()[0];
        let t = |tag: &str| {
            p.runs_of(tag)
                .next()
                .map(|r| r.text.clone())
                .unwrap_or_default()
        };
        assert_eq!(t("title"), "Deposit Summary");
        assert_eq!(t("field0.value"), "Checking");
        assert_eq!(t("row0.check_no"), "5512");
        assert_eq!(t("row1.amount"), "20.00");
        assert_eq!(t("total1.label"), "Less cash back");
        assert_eq!(t("total2.value"), "22.04");
        assert!(p.runs_of("address").next().is_none());
    }
}
