//! Vendor layout templates (WP-A, step 11; plan D3 `templates.rs`, M7).
//! Fingerprint = masked anchor tokens from the top 35 % of page 1 (digit
//! runs of 4+ masked) + aspect ratio; a template matches at Jaccard ≥ 0.6
//! (integer: `intersection × 10 ≥ union × 6`). `learn` builds a template
//! from an accepted (corrected) reading, `merge` folds a new sample into a
//! saved one with integer arithmetic, `apply` reads the template's regions.
//!
//! **Consumers:** the pipeline (`fingerprint`, `best_match`, `apply`) and
//! the desktop / A04 (`fingerprint`, `learn`, `merge`); these signatures
//! are fixed.

use crate::model::{Fingerprint, Header, PageWords, Reading, Template};
use cl_core::CalendarDate;

/// Jaccard similarity at which a template applies, in basis points.
pub const MATCH_BP: u16 = 6_000;

/// The layout fingerprint of a document, or `None` with no words on page 1.
/// Stub (step 2): `None`.
#[must_use]
pub fn fingerprint(pages: &[PageWords]) -> Option<Fingerprint> {
    let _ = pages;
    None
}

/// Jaccard similarity of two fingerprints' tokens, in basis points (0 when
/// the aspect ratios differ too much). Stub (step 2): 0.
#[must_use]
pub fn similarity(a: &Fingerprint, b: &Fingerprint) -> u16 {
    let _ = (a, b);
    0
}

/// The index of the most similar template at or above [`MATCH_BP`].
/// Stub (step 2): `None`.
#[must_use]
pub fn best_match(fingerprint: &Fingerprint, templates: &[Template]) -> Option<usize> {
    let _ = (fingerprint, templates);
    None
}

/// Read the template's regions. Fields that don't parse or fall outside
/// their region are left `None` for the rules to fill (per field, validity
/// only). Stub (step 2): nothing.
#[must_use]
pub fn apply(template: &Template, pages: &[PageWords], today: CalendarDate) -> Header {
    let _ = (template, pages, today);
    Header::default()
}

/// A template learned from an accepted reading (the owner's corrected
/// values with the spots of the matching words). Stub (step 2): `None`.
#[must_use]
pub fn learn(reading: &Reading) -> Option<Template> {
    let _ = reading;
    None
}

/// Fold a new sample into a saved template. Stub (step 2): the saved one.
#[must_use]
pub fn merge(saved: &Template, sample: &Template) -> Template {
    let _ = sample;
    saved.clone()
}
