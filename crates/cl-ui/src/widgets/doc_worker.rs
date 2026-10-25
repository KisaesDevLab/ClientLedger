//! The document worker (P17, plan §1 "Documents", §3 WP1): one thread that
//! owns PDFium (`pdfium-render`, loaded at run time) and decodes PNG and
//! JPEG with the `image` crate, so the egui thread never blocks.
//!
//! # Handoff (WP0 skeleton, WP1 fills this file)
//!
//! Fixed by WP0 (WP1 may add, not change):
//! - [`DocWorker::start`], [`DocWorker::send`], [`DocWorker::try_recv`]:
//!   requests in, replies polled once per frame.
//! - [`DocRequest`]: `Open {doc, bytes}` and `Render {doc, page, px_width}`.
//!   `doc` is the caller's number for one opened document; a newer request
//!   for the same `doc` supersedes queued ones.
//! - [`DocReply`]: `Opened {doc, pages, truncated}` | `Protected` |
//!   `Unreadable` | `Unavailable` (no PDFium on this computer; images still
//!   decode) | `Rendered(RenderedPage)` (RGBA, ≤ 4096 px on the long side
//!   and 32 MP).
//!
//! WP1 adds the thread, the library search order (`CL_PDFIUM_PATH`, then
//! `<exe_dir>/pdfium/`, then `target/pdfium/<triple>/` in debug builds only;
//! never `bind_to_system_library`), the 500-page cap and the image decode
//! limits (12000×12000, 256 MB). Until then every PDF answers
//! `Unavailable` and images answer `Unreadable`, so the viewer shows its
//! "Preview isn't available" state. Nothing here logs a file name.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

/// A request to the worker.
#[derive(Clone, PartialEq, Eq)]
pub enum DocRequest {
    /// Read a document: its page count (images have one page).
    Open {
        /// The caller's number for this document.
        doc: u64,
        /// The file's bytes (a PDF, PNG or JPEG by its first bytes).
        bytes: Arc<[u8]>,
    },
    /// Draw one page `px_width` pixels wide.
    Render {
        /// The document, as opened.
        doc: u64,
        /// Page number, from 0.
        page: u32,
        /// Wanted width in physical pixels (clamped by the worker).
        px_width: u32,
    },
}

/// Sizes only: never the bytes.
impl std::fmt::Debug for DocRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DocRequest::Open { doc, bytes } => f
                .debug_struct("Open")
                .field("doc", doc)
                .field("len", &bytes.len())
                .finish(),
            DocRequest::Render {
                doc,
                page,
                px_width,
            } => f
                .debug_struct("Render")
                .field("doc", doc)
                .field("page", page)
                .field("px_width", px_width)
                .finish(),
        }
    }
}

/// One drawn page, RGBA8, `width × height × 4` bytes.
#[derive(Clone, PartialEq, Eq)]
pub struct RenderedPage {
    /// The document.
    pub doc: u64,
    /// Page number, from 0.
    pub page: u32,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// The pixels.
    pub rgba: Vec<u8>,
}

/// Sizes only.
impl std::fmt::Debug for RenderedPage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RenderedPage")
            .field("doc", &self.doc)
            .field("page", &self.page)
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

/// The worker's answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DocReply {
    /// The document opened.
    Opened {
        /// The document.
        doc: u64,
        /// Pages shown (at most 500).
        pages: u32,
        /// The file has more pages than are shown.
        truncated: bool,
    },
    /// A password-protected PDF.
    Protected {
        /// The document.
        doc: u64,
    },
    /// Not a readable PDF or image (or over the decode limits).
    Unreadable {
        /// The document.
        doc: u64,
    },
    /// PDFium is not on this computer: PDFs cannot be previewed.
    Unavailable {
        /// The document.
        doc: u64,
    },
    /// A page was drawn.
    Rendered(RenderedPage),
}

/// The handle the viewer keeps.
pub struct DocWorker {
    tx: Sender<DocReply>,
    rx: Receiver<DocReply>,
}

impl std::fmt::Debug for DocWorker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DocWorker").finish_non_exhaustive()
    }
}

impl DocWorker {
    /// Start the worker.
    #[must_use]
    pub fn start() -> DocWorker {
        let (tx, rx) = channel();
        DocWorker { tx, rx }
    }

    /// Queue a request (never blocks).
    pub fn send(&self, req: DocRequest) {
        // Skeleton (WP1 replaces): nothing is decoded yet.
        let reply = match req {
            DocRequest::Open { doc, bytes } => match cl_api::FileKind::sniff(&bytes) {
                Some(cl_api::FileKind::Pdf) => DocReply::Unavailable { doc },
                _ => DocReply::Unreadable { doc },
            },
            DocRequest::Render { doc, .. } => DocReply::Unreadable { doc },
        };
        // The receiver lives in `self`, so the send cannot fail.
        let _ = self.tx.send(reply);
    }

    /// The next answer, if one is ready (call once per frame).
    #[must_use]
    pub fn try_recv(&self) -> Option<DocReply> {
        self.rx.try_recv().ok()
    }
}

#[cfg(test)]
mod skeleton_tests {
    use super::*;

    #[test]
    fn a_pdf_without_pdfium_is_unavailable() {
        let w = DocWorker::start();
        w.send(DocRequest::Open {
            doc: 1,
            bytes: Arc::from(&b"%PDF-1.7"[..]),
        });
        assert_eq!(w.try_recv(), Some(DocReply::Unavailable { doc: 1 }));
        assert_eq!(w.try_recv(), None);
        let dbg = format!(
            "{:?}",
            DocRequest::Open {
                doc: 2,
                bytes: Arc::from(&b"secret"[..])
            }
        );
        assert!(!dbg.contains("secret") && dbg.contains("len"));
    }
}
