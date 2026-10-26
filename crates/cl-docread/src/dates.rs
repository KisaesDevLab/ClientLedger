//! Document dates (WP-A, step 8; plan D3 `dates.rs`, S6). Our own grammar
//! (the engine accepts ISO only): US month/day/year by default; a first
//! number over 12 means day-first; both 12 or less → US at medium
//! confidence plus a "could be day-first" check; two-digit years are 20yy;
//! month names, ordinals and ISO are read too.
//!
//! **Consumers:** the rules and the AI gate; the signature of [`parse`] is
//! fixed.

use crate::model::Confidence;
use cl_core::CalendarDate;

/// A date read from text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParsedDate {
    pub date: CalendarDate,
    pub confidence: Confidence,
    /// Both numbers were 12 or less: the caller adds a
    /// `DateCouldBeDayFirst` check.
    pub could_be_day_first: bool,
}

/// The date printed in `text`, or `None`. `today` bounds two-digit years
/// and implausible dates. Stub (step 2): always `None`.
#[must_use]
pub fn parse(text: &str, today: CalendarDate) -> Option<ParsedDate> {
    let _ = (text, today);
    None
}
