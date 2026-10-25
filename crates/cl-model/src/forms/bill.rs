//! The Enter Bill form model (P17, plan §3 WP3): one checked form for a bill
//! and a vendor credit.
//!
//! # Handoff (WP0 skeleton, WP3 fills this file and `bill/tests.rs`)
//!
//! Fixed by WP0 because the UI skeletons already name them:
//! - [`BillKind`] (`Bill` / `Credit`, default `Bill`): the Bill / Vendor
//!   credit radio. It switches the posting route and the labels, not the
//!   screen.
//! - [`BillForm`] (`Default`): the whole form. WP3 adds the fields (vendor,
//!   date, ref, terms, custom days, discount fields, amount due, due
//!   override, memo, header tag, bill received, `FormLines` expenses,
//!   `ItemLines` items, attachments, read state). Keep `Default` and
//!   `Clone`.
//!
//! WP3 adds the rest of the plan's API here: `BillContext`, `BillProblem`
//! (each with a `TextKey`), `CheckedBill`,
//! `check(form, ctx) -> Result<CheckedBill, Vec<BillProblem>>`,
//! `duplicate_window(date) -> (from, to)` (± 18 calendar months, the day
//! clamped to the month end) and `ref_matches(a, b)` (trim, Unicode case
//! fold, nothing else). Money is exact (`cl_core::Money`); no floats in this
//! crate.

/// What the form saves: a bill or a vendor credit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum BillKind {
    /// A bill (`POST /bills`): A/P goes up.
    #[default]
    Bill,
    /// A vendor credit (`POST /vendor-credits`): no terms, due date or
    /// received flag.
    Credit,
}

/// The Enter Bill form as typed. WP3 adds its fields.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BillForm {
    /// Bill or vendor credit.
    pub kind: BillKind,
}

#[cfg(test)]
mod skeleton_tests {
    use super::*;

    #[test]
    fn a_new_form_is_a_bill() {
        assert_eq!(BillForm::default().kind, BillKind::Bill);
    }
}
