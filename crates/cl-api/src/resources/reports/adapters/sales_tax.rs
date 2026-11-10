//! Sales Tax Liability (v2, exact today): a section per agency with a row
//! per tax item and the agency's totals, then the report's totals, all the
//! engine's figures. Bad debts and recoveries are left out on cash basis
//! (always zero there, Q-SPF3).

use cl_core::report::{
    Cell, ColumnHead, ColumnKind, DateRange, ReportDates, ReportMeta, ReportTable, RowKind, RowPath,
};

use super::{amount, col, name_or, row};
use crate::resources::sales_tax_reports::{LiabilityColumns, SalesTaxLiability};

fn columns(hide_bad_debts: bool) -> Vec<ColumnHead> {
    let mut v = vec![
        col(
            "taxable_sales",
            "@reports.col.taxable_sales",
            ColumnKind::Amount,
        ),
        col(
            "collected",
            "@reports.col.tax_collected",
            ColumnKind::Amount,
        ),
        col("discounts", "@reports.col.discounts", ColumnKind::Amount),
    ];
    if !hide_bad_debts {
        v.push(col(
            "bad_debts",
            "@reports.col.bad_debts",
            ColumnKind::Amount,
        ));
        v.push(col(
            "recoveries",
            "@reports.col.recoveries",
            ColumnKind::Amount,
        ));
    }
    v.push(col(
        "adjustments",
        "@reports.col.adjustments",
        ColumnKind::Amount,
    ));
    v.push(col("paid", "@reports.col.tax_paid", ColumnKind::Amount));
    v.push(col("owed", "@reports.col.tax_owed", ColumnKind::Amount));
    v
}

fn cells(taxable: Option<cl_core::Money>, c: &LiabilityColumns, hide_bad_debts: bool) -> Vec<Cell> {
    let mut v = vec![
        taxable.map_or(Cell::Empty, amount),
        amount(c.collected),
        amount(c.discounts),
    ];
    if !hide_bad_debts {
        v.push(amount(c.bad_debts));
        v.push(amount(c.recoveries));
    }
    v.push(amount(c.adjustments));
    v.push(amount(c.paid));
    v.push(amount(c.owed_at_end));
    v
}

/// The liability report over `range`.
#[must_use]
pub fn liability(r: &SalesTaxLiability, range: DateRange) -> ReportTable {
    let hide = r.hides_bad_debts();
    let cols = columns(hide);
    let width = cols.len();
    let mut rows = Vec::new();
    for (ai, a) in r.agencies.iter().enumerate() {
        let path = RowPath::of([format!("agency{ai}")]);
        let mut head = row(
            RowKind::Header,
            0,
            path.clone(),
            name_or(Some(&a.name), "@reports.row.no_agency"),
            vec![Cell::Empty; width],
        );
        head.expandable = !a.items.is_empty();
        rows.push(head);
        for (ii, it) in a.items.iter().enumerate() {
            rows.push(row(
                RowKind::Detail,
                1,
                path.child(format!("item{ii}")),
                name_or(Some(&it.name), "@reports.row.no_tax_item"),
                cells(Some(it.taxable_sales), &it.tax, hide),
            ));
        }
        rows.push(row(
            RowKind::Total,
            0,
            RowPath::of([format!("agency{ai}_total")]),
            "@reports.row.agency_total".into(),
            cells(None, &a.totals, hide),
        ));
    }
    rows.push(row(
        RowKind::Grand,
        0,
        RowPath::of(["total"]),
        "@reports.row.total".into(),
        cells(None, &r.totals, hide),
    ));
    ReportTable {
        columns: cols,
        rows,
        meta: ReportMeta {
            dates: Some(ReportDates::Range(range)),
            ..ReportMeta::default()
        },
    }
}
