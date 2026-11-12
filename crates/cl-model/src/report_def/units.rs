//! The unit of a builder run's value cells. The engine sends every value as
//! an exact decimal string without a unit, and cl-api reads them as
//! amounts; the definition knows which sections count rows or items, so
//! the window applies it here before showing the table.

use std::collections::HashMap;

use cl_core::report::{Cell, ColumnHead, ColumnKind, ReportRow, ReportTable};
use rust_decimal::Decimal;

use super::schema::{BuilderDefinition, Unit};
use super::validate::{NodeUnit, node_units};

/// The node id of a row: the first path segment is `n:<id>`.
fn node_of(path: &[String]) -> Option<&str> {
    path.first().and_then(|s| s.strip_prefix("n:"))
}

fn as_count(d: Decimal) -> Cell {
    // A count is whole and not negative; a change in a count can be
    // negative, so it stays a plain number.
    if d.is_sign_negative() || !d.fract().is_zero() {
        return Cell::Quantity(d.normalize());
    }
    u64::try_from(d.trunc()).map_or(Cell::Quantity(d.normalize()), Cell::Count)
}

/// Turns the value cells of count and quantity rows into
/// [`Cell::Count`] / [`Cell::Quantity`]. Detail-field columns (`f:` keys)
/// and percents keep their own kinds; money, debit-credit and mixed rows
/// stay amounts.
pub fn apply_units(def: &BuilderDefinition, table: &mut ReportTable) {
    apply_units_to_rows(def, &table.columns, &mut table.rows);
}

fn value_columns(columns: &[ColumnHead]) -> Vec<bool> {
    columns
        .iter()
        .map(|c| c.kind == ColumnKind::Amount && !c.key.starts_with("f:"))
        .collect()
}

fn converter(units: &HashMap<String, NodeUnit>, node: Option<&str>) -> Option<fn(Decimal) -> Cell> {
    match node.and_then(|n| units.get(n)) {
        Some(NodeUnit::One(Unit::Count)) => Some(as_count),
        Some(NodeUnit::One(Unit::Quantity)) => Some(Cell::Quantity),
        _ => None,
    }
}

fn convert_cells(cells: &mut [Cell], values: &[bool], convert: fn(Decimal) -> Cell) {
    for (cell, is_value) in cells.iter_mut().zip(values) {
        if let (true, Cell::Amount(d)) = (*is_value, &*cell) {
            *cell = convert(*d);
        }
    }
}

/// As [`apply_units`], for rows read against `columns` (a detail page).
pub fn apply_units_to_rows(
    def: &BuilderDefinition,
    columns: &[ColumnHead],
    rows: &mut [ReportRow],
) {
    let units = node_units(&def.nodes);
    let values = value_columns(columns);
    for row in rows {
        if let Some(convert) = converter(&units, node_of(&row.path.0)) {
            convert_cells(&mut row.cells, &values, convert);
        }
    }
}

/// As [`apply_units`], for a detail page's echoed subtotal of `node_id`.
pub fn apply_units_to_cells(
    def: &BuilderDefinition,
    columns: &[ColumnHead],
    node_id: &str,
    cells: &mut [Cell],
) {
    let units = node_units(&def.nodes);
    if let Some(convert) = converter(&units, Some(node_id)) {
        convert_cells(cells, &value_columns(columns), convert);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cl_core::report::{RowKind, RowPath};

    fn col(key: &str, kind: ColumnKind) -> ColumnHead {
        ColumnHead {
            key: key.into(),
            caption: String::new(),
            kind,
            range: None,
            drill: None,
        }
    }

    fn d(s: &str) -> Decimal {
        Decimal::from_str_exact(s).expect("decimal")
    }

    fn row(node: &str, cells: Vec<Cell>) -> ReportRow {
        ReportRow {
            kind: RowKind::Group,
            level: 1,
            path: RowPath::of([format!("n:{node}"), "x:1".into()]),
            caption: String::new(),
            cells,
            drill: None,
            expandable: false,
        }
    }

    #[test]
    fn count_and_quantity_rows_get_their_units() {
        let def = crate::report_def::parse(
            r#"{"schemaVersion":1,"kind":"builder","dates":{"mode":"range"},
                "columns":{"detailFields":["amount"],"compare":[],"headers":{}},
                "nodes":[
                  {"type":"section","id":"n","caption":"Invoices","source":"sales","measure":"count",
                   "filters":[],"groupBy":[{"key":"contact"}],"display":"summary",
                   "defaultExpanded":false,"showTotal":true,"hideZero":false},
                  {"type":"section","id":"q","caption":"Units","source":"item_sales","measure":"quantity",
                   "filters":[],"groupBy":[{"key":"item"}],"display":"summary",
                   "defaultExpanded":false,"showTotal":true,"hideZero":false},
                  {"type":"section","id":"m","caption":"Sales","source":"sales","measure":"amount",
                   "filters":[],"groupBy":[],"display":"summary",
                   "defaultExpanded":false,"showTotal":true,"hideZero":false}]}"#,
        )
        .expect("definition");
        let mut t = ReportTable {
            columns: vec![
                col("f:amount", ColumnKind::Amount),
                col("total", ColumnKind::Amount),
                col("total:c0:change", ColumnKind::Amount),
                col("total:pct", ColumnKind::Percent),
            ],
            rows: vec![
                row(
                    "n",
                    vec![
                        Cell::Amount(d("9.0000")),
                        Cell::Amount(d("3.0000")),
                        Cell::Amount(d("-2.0000")),
                        Cell::Percent(Some(d("50.0"))),
                    ],
                ),
                row(
                    "q",
                    vec![
                        Cell::Empty,
                        Cell::Amount(d("2.5000")),
                        Cell::Empty,
                        Cell::Percent(None),
                    ],
                ),
                row(
                    "m",
                    vec![
                        Cell::Empty,
                        Cell::Amount(d("10.0000")),
                        Cell::Empty,
                        Cell::Percent(None),
                    ],
                ),
            ],
            meta: cl_core::report::ReportMeta::default(),
        };
        apply_units(&def, &mut t);
        assert_eq!(t.rows[0].cells[0], Cell::Amount(d("9.0000")), "a field");
        assert_eq!(t.rows[0].cells[1], Cell::Count(3));
        assert_eq!(t.rows[0].cells[2], Cell::Quantity(d("-2")));
        assert_eq!(t.rows[0].cells[3], Cell::Percent(Some(d("50.0"))));
        assert_eq!(t.rows[1].cells[1], Cell::Quantity(d("2.5000")));
        assert_eq!(t.rows[2].cells[1], Cell::Amount(d("10.0000")));
        // A detail page's echoed subtotal takes its node's unit.
        let mut sub = vec![
            Cell::Empty,
            Cell::Amount(d("4.0000")),
            Cell::Empty,
            Cell::Percent(None),
        ];
        apply_units_to_cells(&def, &t.columns, "n", &mut sub);
        assert_eq!(sub[1], Cell::Count(4));
    }
}
