//! Reading documents on this computer (A03, plan D7): the reading service
//! and the mapping of a reading into a form.
//!
//! - [`service::ReadingService`] runs the reader on its own thread (never
//!   the egui thread): `cl_host::DocreadHost` starts the sandboxed worker
//!   and tesseract; the desktop then matches the vendor (and, with the
//!   engine's `document-drafts-v1`, the card account) on the extracted
//!   strings (Q-A03o). No engine is needed and nothing goes online.
//! - [`map`] turns a reading into the form model's `DocReading`: every
//!   value is the document's own, filled as *read, not accepted*; the
//!   reading's checks are shown and never fix anything (AC3).
//!
//! A03 reads in memory and creates no draft (Q-A03l, S11): the engine's
//! document drafts and templates arrive with WP-F (step 19a).
//!
//! The service is installed once by the desktop app ([`install`]); tests
//! build their own with a stand-in reader.

pub mod assist;
// A04 WP-F2: learn on accept (layouts, card digits).
pub mod learn;
pub mod map;
// A04: the reading queue's two lanes (interactive before batch).
pub mod queue;
pub mod service;

use std::sync::{Arc, OnceLock};

pub use service::{
    LocalReading, ReadCancel, ReadFailure, ReadJob, ReadOutcome, ReadRequest, Reader,
    ReadingService,
};

static SHARED: OnceLock<Arc<ReadingService>> = OnceLock::new();

/// Installs the app's reading service (once; a second call is ignored).
pub fn install(service: ReadingService) {
    let _ = SHARED.set(Arc::new(service));
}

/// The app's reading service, when the app installed one.
#[must_use]
pub fn shared() -> Option<Arc<ReadingService>> {
    SHARED.get().cloned()
}

/// The total a draft was read with, its sign kept (A05 QA-7). The engine
/// stores only totals of zero or more as `docTotal` (its schema and a
/// table check refuse a minus), so a credit read as a negative total is
/// stored without one and its sign lives only in the reading's header: a
/// full draft gives it, a summary without a stored total gives `None`.
#[must_use]
pub fn read_total(d: &cl_api::DocumentDraft) -> Option<cl_core::Money> {
    if d.doc_total.is_some() {
        return d.doc_total;
    }
    let h: cl_docread::model::Header = serde_json::from_value(d.fields.clone()?).ok()?;
    h.total.as_ref().or(h.balance_due.as_ref()).map(|f| f.value)
}

/// A summary whose total may be a credit: read (a reader, a date) but no
/// stored total. Only the full draft tells (see [`read_total`]).
#[must_use]
pub fn total_needs_full(d: &cl_api::DocumentDraft) -> bool {
    d.doc_total.is_none() && d.doc_date.is_some() && d.reader.is_some() && d.fields.is_none()
}
