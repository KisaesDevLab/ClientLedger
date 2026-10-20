//! Payments waiting to be deposited (P14 lane A):
//! `GET /payments/pending-deposits?exact=1` (string amounts with
//! `deposit-grouping-v1`; numbers from older engines are display-only and
//! never summed) and the deposit detail report.

use crate::session::Session;

/// Payment readers of a session (`session.payments()`).
#[derive(Clone, Copy, Debug)]
pub struct Payments<'a>(
    #[allow(dead_code, reason = "P14 lane A adds the calls")] pub(crate) &'a Session,
);

impl Session {
    /// Payments of the open company.
    #[must_use]
    pub fn payments(&self) -> Payments<'_> {
        Payments(self)
    }
}
