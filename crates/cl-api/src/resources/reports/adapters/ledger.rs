//! General Ledger and Transaction Detail by Account from the exact route
//! (U-14): per account a heading with the opening balance, one row per
//! line with its running balance (each opens its form), and the account's
//! totals; then the report's totals. Every figure is the engine's. The
//! engine stops at the request's line limit and says how many lines there
//! are (Q-P28l).

use cl_core::ids::{AccountId, TransactionId};
use cl_core::report::{
    Cell, ColumnHead, ColumnKind, DateRange, Drill, ReportMeta, ReportRequest, ReportTable,
    RowKind, RowPath, Truncated,
};
use serde_json::Value;

use super::exact::{
    account_caption, amount, cell, date, id, list, meta_basis, require_exact, text,
};
use super::{col, row};
use crate::errors::ApiError;

/// The most lines the window shows (Q-P28l).
pub const SCREEN_LIMIT: u32 = 50_000;

/// Transaction types with a label (`@reports.txn.<type>`); any other type
/// shows as the engine names it.
const TYPES: [&str; 14] = [
    "invoice",
    "credit_memo",
    "cash_sale",
    "customer_payment",
    "customer_refund",
    "bill",
    "vendor_credit",
    "bill_payment",
    "expense",
    "deposit",
    "transfer",
    "journal_entry",
    "inventory_adjustment",
    "sales_tax_payment",
];

/// The caption of a line of type `t`.
#[must_use]
pub fn type_label(t: &str) -> String {
    if TYPES.contains(&t) {
        format!("@reports.txn.{t}")
    } else {
        t.to_string()
    }
}

fn columns(range: DateRange) -> Vec<ColumnHead> {
    let amount_col = |key: &str, caption: &str| ColumnHead {
        key: key.to_string(),
        caption: caption.to_string(),
        kind: ColumnKind::Amount,
        range: Some(range),
        drill: None,
    };
    vec![
        col("date", "@reports.col.date", ColumnKind::Date),
        col("number", "@reports.col.number", ColumnKind::Text),
        col("name", "@reports.col.name", ColumnKind::Text),
        col("memo", "@reports.col.memo", ColumnKind::Text),
        col("tag", "@reports.col.tag", ColumnKind::Text),
        amount_col("debit", "@reports.col.debit"),
        amount_col("credit", "@reports.col.credit"),
        amount_col("balance", "@reports.col.balance"),
    ]
}

fn opt_text(v: &Value, key: &str) -> Cell {
    text(v, key).map_or(Cell::Empty, |s| Cell::Text(s.to_string()))
}

/// The General Ledger over `range`.
#[allow(
    clippy::too_many_lines,
    reason = "one pass: heading, lines, totals per account"
)]
///
/// # Errors
/// [`ApiError::ReportShape`] when an exact field is missing.
pub fn general_ledger(
    v: &Value,
    req: &ReportRequest,
    range: DateRange,
) -> Result<ReportTable, ApiError> {
    require_exact(v)?;
    let mut rows = Vec::new();
    let mut shown: u64 = 0;
    for a in list(v, "accounts")? {
        let account = id(a, "accountId", AccountId::parse)?;
        let key = account.map_or_else(|| "none".to_string(), |x| x.to_string());
        let caption = account_caption(text(a, "accountNumber"), text(a, "name"));
        let mut head = row(
            RowKind::Header,
            0,
            RowPath::of([key.as_str()]),
            caption.clone(),
            vec![
                Cell::Empty,
                Cell::Empty,
                Cell::Empty,
                Cell::Text("@reports.row.beginning_balance".into()),
                Cell::Empty,
                Cell::Empty,
                Cell::Empty,
                cell(a, "beginningBalanceExact")?,
            ],
        );
        head.expandable = true;
        rows.push(head);
        for (i, l) in list(a, "lines")?.iter().enumerate() {
            shown += 1;
            // Cash-basis line ids are row numbers: the index keeps paths
            // unique either way.
            let line = text(l, "lineId").unwrap_or("");
            let mut r = row(
                RowKind::Detail,
                1,
                RowPath::of([key.as_str(), &format!("{i}:{line}")]),
                type_label(text(l, "txnType").unwrap_or("")),
                vec![
                    Cell::Date(date(l, "date")?),
                    opt_text(l, "txnNumber"),
                    opt_text(l, "contactName"),
                    opt_text(l, "description"),
                    opt_text(l, "tagName"),
                    cell(l, "debitExact")?,
                    cell(l, "creditExact")?,
                    cell(l, "runningBalanceExact")?,
                ],
            );
            r.drill = id(l, "transactionId", TransactionId::parse)?.map(|t| Drill::Transaction {
                id: t,
                txn_type: text(l, "txnType").unwrap_or("").to_string(),
            });
            rows.push(r);
        }
        split_rows(a, req, range, account, &key, &mut rows)?;
        let mut tot = row(
            RowKind::AccountTotal,
            0,
            RowPath::of([format!("{key}~total")]),
            format!("@reports.row.total_of {caption}"),
            vec![
                Cell::Empty,
                Cell::Empty,
                Cell::Empty,
                Cell::Empty,
                Cell::Empty,
                cell(a, "periodDebitsExact")?,
                cell(a, "periodCreditsExact")?,
                cell(a, "endingBalanceExact")?,
            ],
        );
        tot.drill = None;
        rows.push(tot);
    }
    rows.push(row(
        RowKind::Grand,
        0,
        RowPath::of(["total"]),
        "@reports.row.total".into(),
        vec![
            Cell::Empty,
            Cell::Empty,
            Cell::Empty,
            Cell::Empty,
            Cell::Empty,
            Cell::Amount(amount(v, "totalDebitsExact")?),
            Cell::Amount(amount(v, "totalCreditsExact")?),
            Cell::Empty,
        ],
    ));
    let truncated = match (
        v.get("truncated"),
        v.get("rowCount").and_then(Value::as_u64),
    ) {
        (Some(Value::Bool(true)), Some(total)) => Some(Truncated { shown, total }),
        (Some(Value::Bool(_)), Some(_)) => None,
        _ => return Err(super::exact::shape("truncated")),
    };
    Ok(ReportTable {
        columns: columns(range),
        rows,
        meta: ReportMeta {
            basis: meta_basis(v).or(req.basis),
            dates: Some(req.dates.clone()),
            truncated,
            note: None,
            hidden_notice: false,
        },
    })
}

/// The tag split of one account (U-201, `accounts[].tagSplit`): its lines
/// without a tag of the group, one row per tag with activity, and the
/// account's total for the period, each with the engine's debits and
/// credits and a drill to its lines.
fn split_rows(
    a: &Value,
    req: &ReportRequest,
    range: DateRange,
    account: Option<AccountId>,
    key: &str,
    rows: &mut Vec<cl_core::report::ReportRow>,
) -> Result<(), ApiError> {
    use cl_core::ids::TagId;
    use cl_core::report::TagScope;
    let Some(group) = req.tag_split else {
        return Ok(());
    };
    let drill = |tag: TagScope| {
        account.map(|acc| Drill::Lines {
            account_ids: vec![acc],
            from: Some(range.from),
            to: range.to,
            contact: None,
            tag,
            basis: req.basis,
            adjusting: req.filters.adjusting,
        })
    };
    let money = |e: &Value| -> Result<Vec<Cell>, ApiError> {
        Ok(vec![
            Cell::Empty,
            Cell::Empty,
            Cell::Empty,
            Cell::Empty,
            Cell::Empty,
            cell(e, "periodDebitsExact")?,
            cell(e, "periodCreditsExact")?,
            Cell::Empty,
        ])
    };
    for e in list(a, "tagSplit")? {
        match text(e, "rowKind") {
            Some("tag") => {
                let tag = id(e, "tagId", TagId::parse)?;
                let mut r = row(
                    RowKind::TagRow,
                    1,
                    RowPath::of([
                        key,
                        format!("t:{}", tag.map(|t| t.to_string()).unwrap_or_default()).as_str(),
                    ]),
                    account_caption(text(e, "tagNumber"), text(e, "tagName")),
                    money(e)?,
                );
                r.drill = tag.and_then(|t| drill(TagScope::Tag(t)));
                rows.push(r);
            }
            Some("account_total") => {
                let mut r = row(
                    RowKind::AccountTotal,
                    1,
                    RowPath::of([key, "split~split"]),
                    "@reports.row.split_total".into(),
                    money(e)?,
                );
                r.drill = drill(TagScope::Any);
                rows.push(r);
            }
            _ => {
                let mut r = row(
                    RowKind::Subtotal,
                    1,
                    RowPath::of([key, "untagged"]),
                    "@reports.row.untagged".into(),
                    money(e)?,
                );
                r.drill = drill(TagScope::UntaggedInGroup(group));
                rows.push(r);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cl_core::CalendarDate;
    use cl_core::report::{Basis, ReportDates, Source};

    fn fixture(name: &str) -> Value {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/reports")
            .join(format!("{name}.json"));
        serde_json::from_str(&std::fs::read_to_string(p).expect("fixture")).expect("json")
    }

    fn march() -> DateRange {
        DateRange {
            from: CalendarDate::from_ymd(2033, 3, 1).expect("d"),
            to: CalendarDate::from_ymd(2033, 3, 31).expect("d"),
        }
    }

    fn req() -> ReportRequest {
        let mut r = ReportRequest::new(Source::GeneralLedger, ReportDates::Range(march()));
        r.basis = Some(Basis::Accrual);
        r
    }

    #[test]
    fn lines_open_their_forms_and_totals_are_the_engine_s() {
        let t =
            general_ledger(&fixture("exact_general_ledger_accrual"), &req(), march()).expect("gl");
        assert_eq!(t.columns.len(), 8);
        assert_eq!(t.rows[0].kind, RowKind::Header);
        let line = &t.rows[1];
        assert_eq!(line.kind, RowKind::Detail);
        assert_eq!(line.caption, "@reports.txn.journal_entry");
        assert!(matches!(line.drill, Some(Drill::Transaction { .. })));
        assert!(line.path.is_under(&t.rows[0].path));
        assert_eq!(line.cells[5], Cell::Amount("0.1000".parse().expect("d")));
        let grand = t.rows.last().expect("grand");
        assert_eq!(grand.cells[5], Cell::Amount("75.3000".parse().expect("d")));
        assert_eq!(grand.cells[6], Cell::Amount("50.0000".parse().expect("d")));
        assert!(t.meta.truncated.is_none());
    }

    #[test]
    fn a_limited_ledger_says_how_many_lines_there_are() {
        let t =
            general_ledger(&fixture("exact_general_ledger_limit"), &req(), march()).expect("gl");
        let tr = t.meta.truncated.expect("truncated");
        assert_eq!((tr.shown, tr.total), (2, 5));
        // The account's totals still cover every line.
        let tot = t
            .rows
            .iter()
            .find(|r| r.kind == RowKind::AccountTotal)
            .expect("total");
        assert_eq!(tot.cells[7], Cell::Amount("-49.7000".parse().expect("d")));
    }
}
