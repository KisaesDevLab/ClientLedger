//! Inventory Value, Inventory Value Detail and Stock Status (U-160, exact
//! today). The value total is the engine's `totalValue`; the detail's
//! running quantity and value are the engine's.

use std::collections::HashMap;

use cl_core::ids::ItemId;
use cl_core::report::{
    Cell, ColumnKind, DateRange, Drill, ReportDates, ReportMeta, ReportTable, RowKind, RowPath,
};
use cl_core::{CalendarDate, Quantity};

use super::{amount, amount_or_empty, col, name_or, row};
use crate::resources::inventory::{StockStatusRow, ValuationDetailRow, ValuationSummary};

fn qty(q: Quantity) -> Cell {
    Cell::Quantity(q.as_decimal())
}

fn pct(s: Option<&str>) -> Cell {
    Cell::Percent(s.and_then(|s| s.trim().parse().ok()))
}

/// Inventory Value as of its date.
#[must_use]
pub fn valuation(v: &ValuationSummary) -> ReportTable {
    let columns = vec![
        col("on_hand", "@reports.col.on_hand", ColumnKind::Quantity),
        col(
            "average_cost",
            "@reports.col.average_cost",
            ColumnKind::Amount,
        ),
        col("value", "@reports.col.value", ColumnKind::Amount),
        col(
            "pct_total",
            "@reports.col.pct_of_total",
            ColumnKind::Percent,
        ),
    ];
    let mut rows: Vec<_> = v
        .rows
        .iter()
        .map(|r| {
            row(
                RowKind::Detail,
                1,
                RowPath::of(["items", r.item_id.to_string().as_str()]),
                name_or(r.name.as_deref(), "@reports.row.unnamed_item"),
                vec![
                    qty(r.on_hand),
                    r.average_cost
                        .map_or(Cell::Empty, |c| Cell::Amount(c.as_decimal())),
                    amount(r.value),
                    pct(r.percent_of_total.as_deref()),
                ],
            )
        })
        .collect();
    rows.push(row(
        RowKind::Grand,
        0,
        RowPath::of(["total"]),
        "@reports.row.total".into(),
        vec![
            Cell::Empty,
            Cell::Empty,
            amount_or_empty(v.total_value),
            Cell::Empty,
        ],
    ));
    ReportTable {
        columns,
        rows,
        meta: ReportMeta {
            dates: Some(ReportDates::AsOf(v.as_of)),
            note: v.basis_note.clone(),
            ..ReportMeta::default()
        },
    }
}

/// Inventory Value Detail over `range`; `names` gives each part's name.
#[must_use]
pub fn valuation_detail<S: std::hash::BuildHasher>(
    rows_in: &[ValuationDetailRow],
    names: &HashMap<ItemId, String, S>,
    range: DateRange,
) -> ReportTable {
    let columns = vec![
        col("date", "@reports.col.date", ColumnKind::Date),
        col("kind", "@reports.col.movement", ColumnKind::Text),
        col("quantity", "@reports.col.quantity", ColumnKind::Quantity),
        col("value", "@reports.col.value", ColumnKind::Amount),
        col(
            "running_quantity",
            "@reports.col.on_hand",
            ColumnKind::Quantity,
        ),
        col(
            "running_value",
            "@reports.col.running_value",
            ColumnKind::Amount,
        ),
        col(
            "average_cost",
            "@reports.col.average_cost",
            ColumnKind::Amount,
        ),
    ];
    let rows = rows_in
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let mut x = row(
                RowKind::Detail,
                1,
                RowPath::of([
                    "items",
                    r.item_id.to_string().as_str(),
                    i.to_string().as_str(),
                ]),
                name_or(
                    names.get(&r.item_id).map(String::as_str),
                    "@reports.row.unnamed_item",
                ),
                vec![
                    Cell::Date(r.date),
                    r.kind.clone().map_or(Cell::Empty, Cell::Text),
                    qty(r.quantity),
                    amount(r.value),
                    qty(r.running_quantity),
                    amount(r.running_value),
                    r.average_cost
                        .map_or(Cell::Empty, |c| Cell::Amount(c.as_decimal())),
                ],
            );
            x.drill = Some(Drill::Transaction {
                id: r.transaction_id,
                txn_type: r
                    .txn_type
                    .map(super::super::run::txn_name)
                    .unwrap_or_default(),
            });
            x
        })
        .collect();
    ReportTable {
        columns,
        rows,
        meta: ReportMeta {
            dates: Some(ReportDates::Range(range)),
            ..ReportMeta::default()
        },
    }
}

/// Stock Status as of `as_of`.
#[must_use]
pub fn stock_status(rows_in: &[StockStatusRow], as_of: CalendarDate) -> ReportTable {
    let columns = vec![
        col("description", "@reports.col.description", ColumnKind::Text),
        col("on_hand", "@reports.col.on_hand", ColumnKind::Quantity),
        col(
            "reorder_point",
            "@reports.col.reorder_point",
            ColumnKind::Quantity,
        ),
        col(
            "reorder_quantity",
            "@reports.col.reorder_quantity",
            ColumnKind::Quantity,
        ),
        col("reorder", "@reports.col.reorder", ColumnKind::Text),
    ];
    let rows = rows_in
        .iter()
        .map(|r| {
            row(
                RowKind::Detail,
                1,
                RowPath::of(["items", r.item_id.to_string().as_str()]),
                name_or(r.name.as_deref(), "@reports.row.unnamed_item"),
                vec![
                    r.description.clone().map_or(Cell::Empty, Cell::Text),
                    qty(r.on_hand),
                    r.reorder_point.map_or(Cell::Empty, qty),
                    r.reorder_quantity.map_or(Cell::Empty, qty),
                    if r.reorder == Some(true) {
                        Cell::Text("@reports.cell.reorder_now".into())
                    } else {
                        Cell::Empty
                    },
                ],
            )
        })
        .collect();
    ReportTable {
        columns,
        rows,
        meta: ReportMeta {
            dates: Some(ReportDates::AsOf(as_of)),
            ..ReportMeta::default()
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value() -> ValuationSummary {
        serde_json::from_str(include_str!(
            "../../../../tests/fixtures/reports/inventory_valuation.json"
        ))
        .expect("fixture")
    }

    #[test]
    fn valuation_keeps_the_engine_total_and_percents() {
        let t = valuation(&value());
        assert_eq!(
            t.to_canonical_csv(),
            "kind,level,path,caption,on_hand,average_cost,value,pct_total\n\
             detail,1,items/11111111-1111-4111-8111-111111111111,Mulch (bag),40,3.125,125,62.5\n\
             detail,1,items/22222222-2222-4222-8222-222222222222,Edging,10,7.5,75,37.5\n\
             grand,0,total,@reports.row.total,,,200,\n"
        );
        assert_eq!(
            t.meta.dates,
            Some(ReportDates::AsOf(
                CalendarDate::from_ymd(2026, 10, 31).expect("d")
            ))
        );
    }
}
