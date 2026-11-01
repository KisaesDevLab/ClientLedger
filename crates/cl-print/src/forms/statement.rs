//! Customer statements, laid out against this crate's own data struct
//! (Q-P19d). P22 feeds it the engine's statement (`GET
//! /reports/customer-statement` or an issued snapshot) in one of three
//! layouts (Q-P22h): open item (Date, Number, Due, Amount, Open), balance
//! forward (a forward row, then Date, Transaction, Amount, Balance) and
//! transaction (the same columns; the previous balance in the header).
//! Every figure is the engine's, parsed for display only.

use super::{Column, FormDoc, FormStyle, TotalRow, layout_form};
use crate::PrintError;
use crate::amount::format_amount;
use crate::layout::Document;
use rust_decimal::Decimal;

/// The statement's layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StatementFormat {
    /// Each open item with what is open of it (the default, Q-175).
    #[default]
    OpenItem,
    /// The opening balance as a row, then the period's activity.
    BalanceForward,
    /// The period's activity; the previous balance in the header.
    Transaction,
}

/// One statement line with the engine's running balance.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StatementLine {
    /// Date.
    pub date: String,
    /// The document number ("1091"), if any.
    pub number: String,
    /// What happened ("Invoice 1091", "Payment").
    pub description: String,
    /// Due date (invoices), if any.
    pub due_date: String,
    /// Charge (positive) or payment or credit (negative).
    pub amount: Decimal,
    /// What is open of it (open-item statements).
    pub open: Option<Decimal>,
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
    /// The layout.
    pub format: StatementFormat,
    /// The statement's number (an issued statement), else empty.
    pub number: String,
    /// Statement date.
    pub date: String,
    /// Customer address, name first.
    pub customer: Vec<String>,
    /// Opening balance (date, amount): a forward row (balance forward) or
    /// the header's previous balance (transaction).
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

fn transaction_text(x: &StatementLine) -> String {
    match (x.description.is_empty(), x.number.is_empty()) {
        (false, _) => x.description.clone(),
        (true, false) => x.number.clone(),
        (true, true) => String::new(),
    }
}

/// The statement as a template document.
#[must_use]
pub fn form(d: &StatementData, style: &FormStyle) -> FormDoc {
    let l = &style.labels;
    let mut rows = Vec::new();
    let mut fields = Vec::new();
    if !d.number.is_empty() {
        fields.push((l.number.clone(), d.number.clone()));
    }
    fields.push((l.statement_date.clone(), d.date.clone()));
    let columns = if d.format == StatementFormat::OpenItem {
        rows.extend(d.lines.iter().map(|x| {
            vec![
                x.date.clone(),
                if x.number.is_empty() {
                    x.description.clone()
                } else {
                    x.number.clone()
                },
                x.due_date.clone(),
                format_amount(x.amount),
                format_amount(x.open.unwrap_or(x.amount)),
            ]
        }));
        vec![
            Column::code("date", &l.date, 28.0),
            Column::text("number", &l.number, 52.5),
            Column::code("due", &l.due_date, 28.0),
            Column::number("amount", &l.amount, 41.0),
            Column::number("open", &l.open, 41.0),
        ]
    } else {
        if let Some((date, bal)) = &d.balance_forward {
            if d.format == StatementFormat::BalanceForward {
                rows.push(vec![
                    date.clone(),
                    l.balance_forward.clone(),
                    String::new(),
                    format_amount(*bal),
                ]);
            } else {
                fields.push((l.previous_balance.clone(), format_amount(*bal)));
            }
        }
        rows.extend(d.lines.iter().map(|x| {
            vec![
                x.date.clone(),
                transaction_text(x),
                format_amount(x.amount),
                format_amount(x.balance),
            ]
        }));
        vec![
            Column::code("date", &l.date, 28.0),
            Column::text("transaction", &l.transaction, 92.5),
            Column::number("amount", &l.amount, 35.0),
            Column::number("balance", &l.balance, 35.0),
        ]
    };
    fields.push((l.amount_due.clone(), format_amount(d.amount_due)));
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
        address_label: l.customer.clone(),
        address: d.customer.clone(),
        fields,
        columns,
        rows,
        totals: vec![TotalRow::new(&l.amount_due, d.amount_due, true)],
        message: d.message.clone(),
        boxes,
        ..FormDoc::default()
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

    fn text_of(d: &StatementData) -> impl Fn(&str) -> String + use<> {
        let p = resolve(&layout(d, &style()).unwrap()).unwrap().remove(0);
        move |tag: &str| {
            p.runs_of(tag)
                .next()
                .map(|r| r.text.clone())
                .unwrap_or_default()
        }
    }

    fn activity() -> StatementData {
        StatementData {
            format: StatementFormat::BalanceForward,
            date: "10/31/2026".into(),
            customer: vec!["Pat Doe".into(), "5 Oak Lane".into()],
            balance_forward: Some(("10/01/2026".into(), Decimal::new(10_000, 2))),
            lines: vec![
                StatementLine {
                    date: "10/05/2026".into(),
                    number: "INV-1001".into(),
                    description: "Invoice INV-1001".into(),
                    amount: Decimal::new(1204, 2),
                    balance: Decimal::new(11_204, 2),
                    ..StatementLine::default()
                },
                StatementLine {
                    date: "10/20/2026".into(),
                    description: "Payment".into(),
                    amount: Decimal::new(-10_000, 2),
                    balance: Decimal::new(1204, 2),
                    ..StatementLine::default()
                },
            ],
            amount_due: Decimal::new(1204, 2),
            aging: Some(Aging {
                current: Decimal::new(1204, 2),
                ..Aging::default()
            }),
            ..StatementData::default()
        }
    }

    #[test]
    fn a_statement_shows_balance_forward_lines_amount_due_and_aging() {
        let t = text_of(&activity());
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

    #[test]
    fn a_transaction_statement_puts_the_previous_balance_in_the_header() {
        let mut d = activity();
        d.format = StatementFormat::Transaction;
        let t = text_of(&d);
        assert_eq!(t("row0.transaction"), "Invoice INV-1001");
        assert_eq!(t("field1.label"), "Previous balance");
        assert_eq!(t("field1.value"), "100.00");
        assert_eq!(t("field2.value"), "12.04");
    }

    /// §6.3: the open-item layout (golden 5's lines as the demo makes
    /// them: two invoices, a credit memo and a payment credit).
    #[test]
    fn an_open_item_statement_lists_number_due_amount_and_open() {
        let line =
            |date: &str, number: &str, due: &str, amount: i64, open: i64, bal: i64| StatementLine {
                date: date.into(),
                number: number.into(),
                description: String::new(),
                due_date: due.into(),
                amount: Decimal::new(amount, 2),
                open: Some(Decimal::new(open, 2)),
                balance: Decimal::new(bal, 2),
            };
        let d = StatementData {
            format: StatementFormat::OpenItem,
            number: "1001".into(),
            date: "11/30/2026".into(),
            customer: vec!["Bluestem Veterinary Clinic".into()],
            lines: vec![
                line("09/15/2026", "1201", "10/15/2026", 25_000, 25_000, 25_000),
                line("10/20/2026", "1202", "11/19/2026", 50_000, 50_000, 75_000),
                line("11/02/2026", "CM-1", "", -39_975, -39_975, 35_025),
                line("11/30/2026", "", "", -5000, -5000, 30_025),
            ],
            amount_due: Decimal::new(30_025, 2),
            aging: Some(Aging {
                current: Decimal::new(-5000, 2),
                days_1_30: Decimal::new(10_025, 2),
                days_31_60: Decimal::new(25_000, 2),
                ..Aging::default()
            }),
            ..StatementData::default()
        };
        let t = text_of(&d);
        assert_eq!(t("field0.value"), "1001");
        assert_eq!(t("row0.number"), "1201");
        assert_eq!(t("row0.due"), "10/15/2026");
        assert_eq!(t("row1.open"), "500.00");
        assert_eq!(t("row2.amount"), "-399.75");
        assert_eq!(t("row3.open"), "-50.00");
        assert_eq!(t("field1.value"), "11/30/2026");
        assert_eq!(t("field2.value"), "300.25");
        assert_eq!(t("box1.value"), "100.25");
        assert_eq!(t("box2.value"), "250.00");
        assert_eq!(t("total0.value"), "300.25");
    }
}
