//! The unit of a builder run's value cells. The engine sends every value as
//! an exact decimal string without a unit, and cl-api reads them as
//! amounts; the definition knows which sections count rows or items, so
//! the window applies it here before showing the table.

use cl_core::report::{Cell, ColumnKind, ReportTable};
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
    let units = node_units(&def.nodes);
    let value_cols: Vec<bool> = table
        .columns
        .iter()
        .map(|c| c.kind == ColumnKind::Amount && !c.key.starts_with("f:"))
        .collect();
    for row in &mut table.rows {
        let unit = node_of(&row.path.0).and_then(|n| units.get(n)).copied();
        let convert: fn(Decimal) -> Cell = match unit {
            Some(NodeUnit::One(Unit::Count)) => as_count,
            Some(NodeUnit::One(Unit::Quantity)) => Cell::Quantity,
            _ => continue,
        };
        for (cell, is_value) in row.cells.iter_mut().zip(&value_cols) {
            if let (true, Cell::Amount(d)) = (*is_value, &*cell) {
                *cell = convert(*d);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cl_core::report::{ColumnHead, ReportRow, RowKind, RowPath};

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
    }
}
