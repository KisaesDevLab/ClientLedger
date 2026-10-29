//! Customer statements, laid out against this crate's own data struct
//! (Q-P19d); the engine's statement data and the Statements screen come in
//! P22.

use super::{Column, FormDoc, FormStyle, TotalRow, layout_form};
use crate::PrintError;
use crate::amount::format_amount;
use crate::layout::Document;
use rust_decimal::Decimal;

/// One statement line with the engine's running balance.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StatementLine {
    /// Date.
    pub date: String,
    /// What happened ("Invoice INV-1001", "Payment").
    pub description: String,
    /// Charge (positive) or payment or credit (negative).
    pub amount: Decimal,
    /// Balance after this line.
    pub balance: Decimal,
}

/// Aging of the amount due, as the engine reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Aging {
    /// Not yet due.
    pub current: Decimal,
    /// 1 to 30 days past due.
    pub days_1_30: Decimal,
    /// 31 to 60 days past due.
    pub days_31_60: Decimal,
    /// 61 to 90 days past due.
    pub days_61_90: Decimal,
    /// More than 90 days past due.
    pub over_90: Decimal,
}

/// A statement.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StatementData {
    /// Statement date.
    pub date: String,
    /// Customer address, name first.
    pub customer: Vec<String>,
    /// Opening balance (date, amount), if the statement starts mid-history.
    pub balance_forward: Option<(String, Decimal)>,
    /// Lines.
    pub lines: Vec<StatementLine>,
    /// The engine's amount due.
    pub amount_due: Decimal,
    /// Aging, if shown.
    pub aging: Option<Aging>,
    /// Message.
    pub message: String,
}

/// The statement as a template document.
#[must_use]
pub fn form(d: &StatementData, style: &FormStyle) -> FormDoc {
    let l = &style.labels;
    let mut rows = Vec::new();
    if let Some((date, bal)) = &d.balance_forward {
        rows.push(vec![
            date.clone(),
            l.balance_forward.clone(),
            String::new(),
            format_amount(*bal),
        ]);
    }
    rows.extend(d.lines.iter().map(|x| {
        vec![
            x.date.clone(),
            x.description.clone(),
            format_amount(x.amount),
            format_amount(x.balance),
        ]
    }));
    let boxes = d
        .aging
        .map(|a| {
            vec![
                (l.current.clone(), format_amount(a.current)),
                (l.days_1_30.clone(), format_amount(a.days_1_30)),
                (l.days_31_60.clone(), format_amount(a.days_31_60)),
                (l.days_61_90.clone(), format_amount(a.days_61_90)),
                (l.over_90.clone(), format_amount(a.over_90)),
            ]
        })
        .unwrap_or_default();
    FormDoc {
        title: l.statement.clone(),
        number: String::new(),
        address_label: l.customer.clone(),
        address: d.customer.clone(),
        fields: vec![
            (l.statement_date.clone(), d.date.clone()),
            (l.amount_due.clone(), format_amount(d.amount_due)),
        ],
        columns: vec![
            Column::code("date", &l.date, 28.0),
            Column::text("transaction", &l.transaction, 92.5),
            Column::number("amount", &l.amount, 35.0),
            Column::number("balance", &l.balance, 35.0),
        ],
        rows,
        totals: vec![TotalRow::new(&l.amount_due, d.amount_due, true)],
        message: d.message.clone(),
        boxes,
    }
}

/// Lay out a statement.
///
/// # Errors
/// As [`layout_form`].
pub fn layout(d: &StatementData, style: &FormStyle) -> Result<Document, PrintError> {
    layout_form(&form(d, style), style)
}

#[cfg(test)]
mod tests {
    use super::super::tests::style;
    use super::*;
    use crate::layout::resolve;

    #[test]
    fn a_statement_shows_balance_forward_lines_amount_due_and_aging() {
        let d = StatementData {
            date: "10/31/2026".into(),
            customer: vec!["Pat Doe".into(), "5 Oak Lane".into()],
            balance_forward: Some(("10/01/2026".into(), Decimal::new(10_000, 2))),
            lines: vec![
                StatementLine {
                    date: "10/05/2026".into(),
                    description: "Invoice INV-1001".into(),
                    amount: Decimal::new(1204, 2),
                    balance: Decimal::new(11_204, 2),
                },
                StatementLine {
                    date: "10/20/2026".into(),
                    description: "Payment".into(),
                    amount: Decimal::new(-10_000, 2),
                    balance: Decimal::new(1204, 2),
                },
            ],
            amount_due: Decimal::new(1204, 2),
            aging: Some(Aging {
                current: Decimal::new(1204, 2),
                ..Aging::default()
            }),
            message: String::new(),
        };
        let p = &resolve(&layout(&d, &style()).unwrap()).unwrap()[0];
        let t = |tag: &str| {
            p.runs_of(tag)
                .next()
                .map(|r| r.text.clone())
                .unwrap_or_default()
        };
        assert_eq!(t("title"), "Statement");
        assert_eq!(t("row0.transaction"), "Balance forward");
        assert_eq!(t("row0.balance"), "100.00");
        assert_eq!(t("row2.amount"), "-100.00");
        assert_eq!(t("field1.value"), "12.04");
        assert_eq!(t("total0.value"), "12.04");
        assert_eq!(t("box0.label"), "Current");
        assert_eq!(t("box0.value"), "12.04");
        assert_eq!(t("box4.value"), "0.00");
    }
}
