//! Item lines of a bill or vendor credit (P17, plan §3 WP3, D5, D6).
//!
//! # Handoff (WP0 skeleton, WP3 fills this file)
//!
//! WP3 adds `ItemLine {key, item, description, qty: Option<Quantity>,
//! rate: Option<Money>, amount: Option<Money>, account, customer, billable,
//! tag}`, the `ItemLines` collection, `line_amount()` (=
//! `rate.mul_round(qty, 2)`, half away from zero, when both are set; the
//! Amount is then read-only) and `items_total`. An item line always carries
//! an account (D5). Exact money only.
