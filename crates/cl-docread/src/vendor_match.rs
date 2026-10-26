//! Vendor matching (WP-A, step 11; plan D3 `vendor_match.rs`, S21). Runs in
//! the desktop on the extracted strings against the company's and the
//! tenant-wide vendors (U-90). Tiers, strongest first:
//! `vendor_account_number`; phone; past layouts (a similar fingerprint →
//! its contact); the engine's four `matchByName` tiers
//! (`ai-name-match.ts:55-95`, ambiguous → none); address breaks ties.
//! Accounts: the vendor's default only (Q-A03r).
//!
//! **Consumer:** the desktop reading service (WP-G); the signatures here
//! are fixed.

use crate::model::{Confidence, Fingerprint, Header};
use cl_core::ids::ContactId;
use std::fmt;

/// One vendor the document could be from. Built by the desktop from
/// `GET /contacts` and the company's templates.
#[derive(Clone, PartialEq, Eq)]
pub struct VendorCandidate {
    pub contact: ContactId,
    pub display_name: String,
    pub company_name: Option<String>,
    /// `contacts.vendor_account_number`.
    pub account_number: Option<String>,
    pub phone: Option<String>,
    /// Billing address lines joined with `\n`.
    pub address: Option<String>,
    /// Fingerprints of this vendor's saved layout templates.
    pub layouts: Vec<Fingerprint>,
}

impl fmt::Debug for VendorCandidate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VendorCandidate")
            .field("contact", &self.contact)
            .field("layouts", &self.layouts.len())
            .finish_non_exhaustive()
    }
}

/// Which tier matched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum VendorTier {
    AccountNumber,
    Phone,
    PastLayout,
    /// The engine's name tiers, 1 (exact) to 4 (loosest).
    Name(u8),
}

/// A matched vendor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VendorMatch {
    pub contact: ContactId,
    pub tier: VendorTier,
    pub confidence: Confidence,
}

/// The vendor the reading is from, or `None` (the desktop offers a new
/// vendor). Stub (step 2): always `None`.
#[must_use]
pub fn match_vendor(
    header: &Header,
    fingerprint: Option<&Fingerprint>,
    vendors: &[VendorCandidate],
) -> Option<VendorMatch> {
    let _ = (header, fingerprint, vendors);
    None
}
