//! The Items grid of a bill (P17, plan §3 WP4; shown only with
//! `bill-lines-v1`, Q-P17c).
//!
//! # Handoff (WP0 skeleton, WP4 fills this file)
//!
//! cl-grid's `line_entry` preset with Item · Description · Qty · Cost ·
//! Amount · Account · Customer / Job · Billable · Tags over
//! `cl_model::forms::item_lines` (WP3). Amount is computed and read-only
//! when Qty and Cost are both set (D6).
