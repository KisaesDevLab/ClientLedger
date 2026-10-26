//! Line items (WP-A, step 10; plan D3 `lines.rs`, S7): the header row sets
//! the column bands; rows run to Subtotal/Total; wrapped descriptions
//! merge. Quantity × unit price is checked with `Money::mul_round(qty, 2)`
//! (the engine's `LINE_AMOUNT_MISMATCH`): a failing line keeps its printed
//! amount, drops quantity and unit price, and gains a
//! `LineQuantityTimesPrice` check. Not confident → no lines and a
//! `LinesNotRead` check (one total line to the vendor's default account,
//! Q-285).
//!
//! Internal to the rules (WP-A may refine).

use crate::layout::Layout;
use crate::model::{Check, Line, LineTable, PageWords};

/// Extract the line items. Stub (step 2): none.
#[must_use]
pub fn extract(
    layout: &Layout,
    pages: &[PageWords],
    table: Option<&LineTable>,
) -> (Vec<Line>, Vec<Check>) {
    let _ = (layout, pages, table);
    (Vec::new(), Vec::new())
}
