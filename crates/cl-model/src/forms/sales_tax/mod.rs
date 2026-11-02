//! Sales tax form state (P23 WP-1; no egui, no money arithmetic).
//!
//! - [`setup`]: the sales tax item, group and tax code editors.
//! - [`pay`]: Pay Sales Tax and "Match a payment you already made" (link).
//! - [`exemption`]: a client's exemption certificate.
//! - [`recovery`]: recovering part of a written-off invoice.
//!
//! Every rate is a [`cl_core::TaxRate`] parsed from the owner's percent;
//! every amount the engine computes (owed, the check total, recovered tax)
//! is held as the engine's [`cl_core::Money`] and copied, never summed.

pub mod exemption;
pub mod pay;
pub mod recovery;
pub mod setup;

#[cfg(test)]
mod tests;
