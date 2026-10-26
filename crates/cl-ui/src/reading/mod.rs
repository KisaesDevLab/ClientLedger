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

pub mod map;
pub mod service;

use std::sync::{Arc, OnceLock};

pub use service::{LocalReading, ReadFailure, ReadOutcome, ReadRequest, Reader, ReadingService};

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
