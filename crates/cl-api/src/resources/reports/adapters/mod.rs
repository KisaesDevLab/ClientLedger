//! Engine report answers to the neutral [`ReportTable`] (P28 plan §0 step
//! 3), one module per route family. Pure functions on the typed answers:
//! they copy the engine's exact amounts into cells and never add,
//! subtract or divide (plan §6). Fixed captions are `@` i18n keys the
//! window translates.
//!
//! WP1 covers the routes that are exact today: A/R aging, sales tax v2 and
//! inventory. The `*Exact` routes of U-14 (`reports-exact-v1`) come with
//! WP2 and WP3.

pub mod ar_aging;
pub mod categories;
pub(crate) mod exact;
pub mod inventory;
pub mod ledger;
pub mod sales_tax;
pub mod statements;

use cl_core::report::{Cell, ColumnHead, ColumnKind, ReportRow, RowKind, RowPath};

/// A column with no dates of its own.
pub(crate) fn col(key: &str, caption: &str, kind: ColumnKind) -> ColumnHead {
    ColumnHead {
        key: key.to_string(),
        caption: caption.to_string(),
        kind,
        range: None,
        drill: None,
    }
}

/// A row with no drill.
pub(crate) fn row(
    kind: RowKind,
    level: u8,
    path: RowPath,
    caption: String,
    cells: Vec<Cell>,
) -> ReportRow {
    ReportRow {
        kind,
        level,
        path,
        caption,
        cells,
        drill: None,
        expandable: false,
    }
}

/// An exact amount cell.
pub(crate) fn amount(m: cl_core::Money) -> Cell {
    Cell::Amount(m.as_decimal())
}

/// An optional exact amount cell.
pub(crate) fn amount_or_empty(m: Option<cl_core::Money>) -> Cell {
    m.map_or(Cell::Empty, amount)
}

/// Owner text or an `@` label when blank.
pub(crate) fn name_or(name: Option<&str>, fallback: &str) -> String {
    match name.map(str::trim) {
        Some(n) if !n.is_empty() => n.to_string(),
        _ => fallback.to_string(),
    }
}
