//! Revenue by Category and Expenses by Category (U-14): one row per
//! account with the engine's total, and the engine's grand total. Each
//! amount opens the lines behind it.

use cl_core::ids::AccountId;
use cl_core::report::{
    Cell, ColumnHead, ColumnKind, DateRange, ReportMeta, ReportRequest, ReportTable, RowKind,
    RowPath,
};
use serde_json::Value;

use super::exact::{account_caption, amount, cell, id, list, meta_basis, require_exact, text};
use super::row;
use super::statements::lines;
use crate::errors::ApiError;

/// The category report over `range`.
///
/// # Errors
/// [`ApiError::ReportShape`] when an exact field is missing.
pub fn by_category(
    v: &Value,
    req: &ReportRequest,
    range: DateRange,
) -> Result<ReportTable, ApiError> {
    require_exact(v)?;
    let mut rows = Vec::new();
    for (i, e) in list(v, "data")?.iter().enumerate() {
        let account = id(e, "account_id", AccountId::parse)?;
        let key = account.map_or_else(|| format!("row{i}"), |a| a.to_string());
        let mut r = row(
            RowKind::Account,
            1,
            RowPath::of(["accounts", key.as_str()]),
            account_caption(text(e, "account_number"), text(e, "category")),
            vec![cell(e, "totalExact")?],
        );
        r.drill = account.and_then(|a| lines(req, vec![a], Some(range.from), range.to));
        rows.push(r);
    }
    let totals = v
        .get("totalsExact")
        .ok_or(super::exact::shape("totalsExact"))?;
    rows.push(row(
        RowKind::Grand,
        0,
        RowPath::of(["total"]),
        "@reports.row.total".into(),
        vec![Cell::Amount(amount(totals, "total")?)],
    ));
    Ok(ReportTable {
        columns: vec![ColumnHead {
            key: "total".into(),
            caption: "@reports.col.total".into(),
            kind: ColumnKind::Amount,
            range: Some(range),
            drill: None,
        }],
        rows,
        meta: ReportMeta {
            basis: meta_basis(v).or(req.basis),
            dates: Some(req.dates.clone()),
            ..ReportMeta::default()
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cl_core::CalendarDate;
    use cl_core::report::{Basis, Drill, ReportDates, Source};

    fn fixture(name: &str) -> Value {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/reports")
            .join(format!("{name}.json"));
        serde_json::from_str(&std::fs::read_to_string(p).expect("fixture")).expect("json")
    }

    #[test]
    fn rows_drill_and_the_total_is_the_engine_s() {
        let range = DateRange {
            from: CalendarDate::from_ymd(2033, 3, 1).expect("d"),
            to: CalendarDate::from_ymd(2033, 3, 31).expect("d"),
        };
        let mut r = ReportRequest::new(Source::ExpenseByCategory, ReportDates::Range(range));
        r.basis = Some(Basis::Accrual);
        let t = by_category(&fixture("exact_expense_by_category"), &r, range).expect("table");
        assert_eq!(t.rows.len(), 3);
        assert!(matches!(t.rows[0].drill, Some(Drill::Lines { .. })));
        assert_eq!(
            t.rows[2].cells,
            vec![Cell::Amount("75.0000".parse().expect("d"))]
        );
        let t = by_category(&fixture("exact_revenue_by_category"), &r, range).expect("table");
        assert_eq!(
            t.rows.last().expect("grand").cells,
            vec![Cell::Amount("0.3000".parse().expect("d"))]
        );
    }
}
