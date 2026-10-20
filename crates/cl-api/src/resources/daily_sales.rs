//! Daily Sales Summary (P14 lane B): the engine's daily-sales layouts
//! (`/daily-sales/templates`) and days (`/daily-sales/entries`, preview,
//! post and void; `closingPassword` with `daily-sales-lock-v1`).
//!
//! Amounts go out as `Money::to_wire()` strings (the engine refuses a JSON
//! number) and come back as exact `Money`; the engine computes the posting
//! and its over/short, the app only compares.

use crate::session::Session;

/// Daily-sales layouts and days of a session (`session.daily_sales()`).
#[derive(Clone, Copy, Debug)]
pub struct DailySales<'a>(
    #[allow(dead_code, reason = "P14 lane B adds the calls")] pub(crate) &'a Session,
);

impl Session {
    /// Daily-sales layouts and days of the open company.
    #[must_use]
    pub fn daily_sales(&self) -> DailySales<'_> {
        DailySales(self)
    }
}
