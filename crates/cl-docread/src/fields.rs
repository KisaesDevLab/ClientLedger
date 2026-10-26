//! Header fields (WP-A, step 9; plan D3 `fields.rs`, M7): vendor (top
//! block, not "Bill to"/"Ship to"), date, reference, terms, due date,
//! subtotal, tax, fees, total, amount paid, balance, payment (card last 4,
//! PAID, check number, cash). A value is taken right of its label, then
//! below it, each with a spot, rule id and confidence.
//!
//! M7: arithmetic may only break a tie among *unlabelled* candidates (and
//! then adds a `PickedUsingTotals` check); a labelled value is never
//! replaced for failing to reconcile. With a template, each field comes
//! from the template's region when valid (parses, in the region) and falls
//! back to the rules per field on validity only; template and rules both
//! valid but different → the template's value plus a `ReadersDisagree`
//! check.
//!
//! Internal to the rules (WP-A may refine).

use crate::layout::Layout;
use crate::model::{Check, Header, PageWords, Template};
use cl_core::CalendarDate;

/// Extract the header. Stub (step 2): nothing found.
#[must_use]
pub fn extract(
    layout: &Layout,
    pages: &[PageWords],
    template: Option<&Template>,
    today: CalendarDate,
) -> (Header, Vec<Check>) {
    let _ = (layout, pages, template, today);
    (Header::default(), Vec::new())
}
