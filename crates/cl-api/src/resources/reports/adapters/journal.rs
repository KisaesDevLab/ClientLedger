//! The Journal (U-202, `GET /general-ledger?group_by=transaction`): the
//! ledger's lines grouped by transaction in date order, each transaction
//! with its lines and the engine's debit and credit totals, then the
//! report's totals. Each transaction opens its form.

use cl_core::ids::TransactionId;
use cl_core::report::{
    Cell, ColumnHead, ColumnKind, DateRange, Drill, ReportMeta, ReportRequest, ReportTable,
    RowKind, RowPath, Truncated,
};
use serde_json::Value;

use super::exact::{
    account_caption, amount, cell, date, id, list, meta_basis, meta_cash_rules, require_exact,
    shape, text,
};
use super::ledger::type_label;
use super::{col, row};
use crate::errors::ApiError;

fn opt_text(v: &Value, key: &str) -> Cell {
    text(v, key).map_or(Cell::Empty, |s| Cell::Text(s.to_string()))
}

/// The Journal over `range`.
///
/// # Errors
/// [`ApiError::ReportShape`] when an exact field is missing.
pub fn journal(v: &Value, req: &ReportRequest, range: DateRange) -> Result<ReportTable, ApiError> {
    require_exact(v)?;
    let amount_col = |key: &str, caption: &str| ColumnHead {
        key: key.to_string(),
        caption: caption.to_string(),
        kind: ColumnKind::Amount,
        range: Some(range),
        drill: None,
    };
    let columns = vec![
        col("date", "@reports.col.date", ColumnKind::Date),
        col("number", "@reports.col.number", ColumnKind::Text),
        col("name", "@reports.col.name", ColumnKind::Text),
        col("memo", "@reports.col.memo", ColumnKind::Text),
        col("tag", "@reports.col.tag", ColumnKind::Text),
        amount_col("debit", "@reports.col.debit"),
        amount_col("credit", "@reports.col.credit"),
    ];
    let mut rows = Vec::new();
    let mut shown: u64 = 0;
    for (ti, t) in list(v, "transactions")?.iter().enumerate() {
        let txn = id(t, "transactionId", TransactionId::parse)?.ok_or(shape("transactionId"))?;
        let ty = text(t, "txnType").unwrap_or("");
        let key = format!("{ti}:{txn}");
        let drill = Some(Drill::Transaction {
            id: txn,
            txn_type: ty.to_string(),
        });
        let mut head = row(
            RowKind::Header,
            0,
            RowPath::of([key.as_str()]),
            type_label(ty),
            vec![
                Cell::Date(date(t, "date")?),
                opt_text(t, "txnNumber"),
                opt_text(t, "contactName"),
                Cell::Empty,
                Cell::Empty,
                Cell::Empty,
                Cell::Empty,
            ],
        );
        head.expandable = true;
        head.drill.clone_from(&drill);
        rows.push(head);
        for (li, l) in list(t, "lines")?.iter().enumerate() {
            shown += 1;
            let mut r = row(
                RowKind::Detail,
                1,
                RowPath::of([key.as_str(), &li.to_string()]),
                account_caption(text(l, "accountNumber"), text(l, "accountName")),
                vec![
                    Cell::Empty,
                    Cell::Empty,
                    Cell::Empty,
                    opt_text(l, "description"),
                    opt_text(l, "tagName"),
                    cell(l, "debitExact")?,
                    cell(l, "creditExact")?,
                ],
            );
            r.drill.clone_from(&drill);
            rows.push(r);
        }
        rows.push(row(
            RowKind::AccountTotal,
            0,
            RowPath::of([format!("{key}~total")]),
            "@reports.row.total".into(),
            vec![
                Cell::Empty,
                Cell::Empty,
                Cell::Empty,
                Cell::Empty,
                Cell::Empty,
                cell(t, "totalDebitsExact")?,
                cell(t, "totalCreditsExact")?,
            ],
        ));
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
        ],
    ));
    let truncated = match (
        v.get("truncated"),
        v.get("rowCount").and_then(Value::as_u64),
    ) {
        (Some(Value::Bool(true)), Some(total)) => Some(Truncated { shown, total }),
        (Some(Value::Bool(_)), Some(_)) => None,
        _ => return Err(shape("truncated")),
    };
    Ok(ReportTable {
        columns,
        rows,
        meta: ReportMeta {
            basis: meta_basis(v).or(req.basis),
            dates: Some(req.dates.clone()),
            cash_basis_rules: meta_cash_rules(v),
            truncated,
            hidden_notice: false,
        },
    })
}
