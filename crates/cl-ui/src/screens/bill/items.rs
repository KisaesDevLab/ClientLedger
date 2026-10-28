//! The Items grid of a bill (P17, plan §3 WP4; shown only with
//! `bill-lines-v1`, Q-P17c).
//!
//! Item · Description · Qty · Cost · Amount · Account · Customer / Project ·
//! Billable · Tags over `cl_model::forms::item_lines` (WP3), on cl-grid's
//! `line_entry` preset. Amount is computed and read-only when Qty and Cost
//! are both set (D6). The grid model is the shared
//! [`crate::forms::item_lines_grid`] (beside the Expenses tab's
//! `lines_grid`); this module names it for the screen.

pub use crate::forms::item_lines_grid::{
    I_ACCOUNT, I_AMOUNT, I_BILLABLE, I_COST, I_CUSTOMER, I_DESC, I_ITEM, I_QTY, I_TAG, ITEMS,
    ItemLists, ItemsGrid,
};
