//! The document worker (P17, plan §1 "Documents", §3 WP1): one thread that
//! owns PDFium (`pdfium-render`, loaded at run time) and decodes PNG and
//! JPEG with the `image` crate, so the egui thread never blocks.
//!
//! # Handoff (WP0 skeleton, WP1 filled it)
//!
//! Fixed by WP0 (later WPs may add, not change):
//! - [`DocWorker::start`], [`DocWorker::send`], [`DocWorker::try_recv`]:
//!   requests in, replies polled once per frame.
//! - [`DocRequest`]: `Open {doc, bytes}`, `Render {doc, page, px_width}` and
//!   (WP1) `Close {doc}`. `doc` is the caller's number for one opened
//!   document; a newer request for the same `doc` supersedes queued ones.
//! - [`DocReply`]: `Opened {doc, pages, truncated}` | `Protected` |
//!   `Unreadable` | `Unavailable` (no PDFium on this computer; images still
//!   decode) | `Rendered(RenderedPage)` (RGBA, ≤ 4096 px on the long side
//!   and 32 MP).
//!
//! # How it works
//!
//! There is one worker thread per process, started by the first
//! [`DocWorker::start`]: PDFium's bindings are process-wide and initialised
//! once, so every viewer shares it. Each [`DocWorker`] is a handle with its
//! own reply channel; dropping it closes its documents. Before each round the
//! thread takes everything queued and drops what newer requests supersede
//! (an older render of the same document, anything before a re-open or a
//! close), so fast zooming renders only the last size.
//!
//! The library is looked for, in order, at `CL_PDFIUM_PATH` (an absolute
//! path to the library or its folder), `<exe_dir>/pdfium/`, and in debug
//! builds only `target/pdfium/<triple>/` (`cargo xtask pdfium-fetch`).
//! A relative `CL_PDFIUM_PATH` is ignored, and `bind_to_system_library` is
//! never used: either would let a stray library from the working folder or
//! the system path load. Without the library PDFs answer `Unavailable`.
//!
//! Limits: PDFs show at most [`MAX_PAGES`] pages; a drawn page is at most
//! [`MAX_SIDE`] px on its long side and [`MAX_PIXELS`]; images are refused
//! over [`MAX_IMAGE_SIDE`] px a side or [`MAX_IMAGE_ALLOC`] bytes to decode
//! (the decoder checks the header first, so a decompression bomb never
//! allocates), then kept at most [`MAX_SIDE`] px on the long side.
//!
//! Nothing here logs a file name or a document's content.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, OnceLock};

use image::{ImageReader, Limits, RgbaImage};
use pdfium_render::prelude::{
    PdfDocument, PdfRenderConfig, Pdfium, PdfiumError, PdfiumInternalError,
};

/// Pages shown of a longer PDF.
pub const MAX_PAGES: u32 = 500;
/// The long side of a drawn page, in pixels.
pub const MAX_SIDE: u32 = 4096;
/// The pixels of a drawn page.
pub const MAX_PIXELS: u64 = 32_000_000;
/// The largest image side the decoder accepts.
pub const MAX_IMAGE_SIDE: u32 = 12_000;
/// The most memory one image decode may use.
pub const MAX_IMAGE_ALLOC: u64 = 256 * 1024 * 1024;
/// Documents kept open across all viewers (the oldest closes first).
const MAX_OPEN: usize = 16;

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
    /// Forget a document (no reply).
    Close {
        /// The document.
        doc: u64,
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
            DocRequest::Close { doc } => f.debug_struct("Close").field("doc", doc).finish(),
        }
    }
}

impl DocRequest {
    fn doc(&self) -> u64 {
        match self {
            DocRequest::Open { doc, .. }
            | DocRequest::Render { doc, .. }
            | DocRequest::Close { doc } => *doc,
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
    /// The page's own width at 100 % (PDF points, or image pixels), for the
    /// viewer's percent zoom.
    pub natural_width: u32,
    /// The page's own height at 100 %.
    pub natural_height: u32,
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

/// What the thread is asked: a handle's request, or a handle closing.
enum Job {
    Req {
        handle: u64,
        req: DocRequest,
        reply: Sender<DocReply>,
    },
    CloseHandle(u64),
}

/// The handle the viewer keeps.
pub struct DocWorker {
    handle: u64,
    jobs: Sender<Job>,
    tx: Sender<DocReply>,
    rx: Receiver<DocReply>,
}

impl std::fmt::Debug for DocWorker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DocWorker")
            .field("handle", &self.handle)
            .finish_non_exhaustive()
    }
}

static NEXT_HANDLE: AtomicU64 = AtomicU64::new(1);
static SHARED: OnceLock<Sender<Job>> = OnceLock::new();

impl DocWorker {
    /// A handle on the process's worker thread (started on first use).
    #[must_use]
    pub fn start() -> DocWorker {
        let jobs = SHARED
            .get_or_init(|| spawn(library_path().map(Path::to_path_buf)))
            .clone();
        DocWorker::on(jobs)
    }

    /// A handle on a private worker that has no PDFium, whatever this
    /// computer has (tests of the "unavailable" path).
    #[cfg(test)]
    pub(crate) fn start_without_library() -> DocWorker {
        DocWorker::on(spawn(None))
    }

    fn on(jobs: Sender<Job>) -> DocWorker {
        let (tx, rx) = channel();
        DocWorker {
            handle: NEXT_HANDLE.fetch_add(1, Ordering::Relaxed),
            jobs,
            tx,
            rx,
        }
    }

    /// Queue a request (never blocks).
    pub fn send(&self, req: DocRequest) {
        let job = Job::Req {
            handle: self.handle,
            req,
            reply: self.tx.clone(),
        };
        if let Err(std::sync::mpsc::SendError(Job::Req { req, .. })) = self.jobs.send(job) {
            // The worker thread is gone (it panicked): answer as if no
            // library were here, so the viewer shows its fallback.
            if !matches!(req, DocRequest::Close { .. }) {
                let _ = self.tx.send(DocReply::Unavailable { doc: req.doc() });
            }
        }
    }

    /// The next answer, if one is ready (call once per frame).
    #[must_use]
    pub fn try_recv(&self) -> Option<DocReply> {
        self.rx.try_recv().ok()
    }
}

impl Drop for DocWorker {
    fn drop(&mut self) {
        let _ = self.jobs.send(Job::CloseHandle(self.handle));
    }
}

/// Start a worker thread that loads the library at `lib` (none: PDFs are
/// `Unavailable`).
fn spawn(lib: Option<PathBuf>) -> Sender<Job> {
    let (tx, rx) = channel();
    let started = std::thread::Builder::new()
        .name("cl-doc-worker".into())
        .spawn(move || serve(lib.as_deref(), &rx));
    if started.is_err() {
        tracing::warn!("document worker thread could not start");
    }
    tx
}

/// Bind PDFium from `lib`. The bindings are process-wide: a second bind
/// in the same process (tests start private workers) reuses them.
fn bind(lib: &Path) -> Option<Pdfium> {
    match Pdfium::bind_to_library(lib) {
        Ok(b) => Some(Pdfium::new(b)),
        Err(PdfiumError::PdfiumLibraryBindingsAlreadyInitialized) => Some(Pdfium::default()),
        Err(_) => {
            tracing::warn!("PDFium could not be loaded; PDF previews are unavailable");
            None
        }
    }
}

fn serve(lib: Option<&Path>, rx: &Receiver<Job>) {
    let pdfium = lib.and_then(bind);
    let mut docs = Docs::default();
    while let Ok(first) = rx.recv() {
        let mut batch = vec![first];
        while let Ok(more) = rx.try_recv() {
            batch.push(more);
        }
        for job in coalesce(batch) {
            match job {
                Job::CloseHandle(h) => docs.close_handle(h),
                Job::Req { handle, req, reply } => {
                    if let Some(r) = handle_request(pdfium.as_ref(), &mut docs, handle, req) {
                        // A closed viewer no longer listens: nothing to do.
                        let _ = reply.send(r);
                    }
                }
            }
        }
    }
}

/// Drop what a later job in the batch supersedes: an earlier render of the
/// same document, anything before a re-open or a close of that document,
/// and everything of a handle that closed.
fn coalesce(batch: Vec<Job>) -> Vec<Job> {
    let mut rendered = std::collections::HashSet::new();
    let mut reset = std::collections::HashSet::new();
    let mut gone = std::collections::HashSet::new();
    let mut keep = Vec::with_capacity(batch.len());
    for job in batch.into_iter().rev() {
        match &job {
            Job::CloseHandle(h) => {
                gone.insert(*h);
                keep.push(job);
            }
            Job::Req { handle, req, .. } => {
                let key = (*handle, req.doc());
                if gone.contains(handle) || reset.contains(&key) {
                    continue;
                }
                match req {
                    DocRequest::Render { .. } => {
                        if rendered.insert(key) {
                            keep.push(job);
                        }
                    }
                    DocRequest::Open { .. } | DocRequest::Close { .. } => {
                        reset.insert(key);
                        keep.push(job);
                    }
                }
            }
        }
    }
    keep.reverse();
    keep
}

/// One open document.
enum Doc<'a> {
    Pdf(PdfDocument<'a>, u32),
    Image(RgbaImage),
}

/// Open documents by (handle, doc), oldest first.
#[derive(Default)]
struct Docs<'a> {
    open: Vec<((u64, u64), Doc<'a>)>,
}

impl<'a> Docs<'a> {
    fn get(&self, key: (u64, u64)) -> Option<&Doc<'a>> {
        self.open.iter().find(|(k, _)| *k == key).map(|(_, d)| d)
    }

    fn insert(&mut self, key: (u64, u64), doc: Doc<'a>) {
        self.remove(key);
        if self.open.len() >= MAX_OPEN {
            self.open.remove(0);
        }
        self.open.push((key, doc));
    }

    fn remove(&mut self, key: (u64, u64)) {
        self.open.retain(|(k, _)| *k != key);
    }

    fn close_handle(&mut self, handle: u64) {
        self.open.retain(|((h, _), _)| *h != handle);
    }
}

fn handle_request<'a>(
    pdfium: Option<&'a Pdfium>,
    docs: &mut Docs<'a>,
    handle: u64,
    req: DocRequest,
) -> Option<DocReply> {
    match req {
        DocRequest::Close { doc } => {
            docs.remove((handle, doc));
            None
        }
        DocRequest::Open { doc, bytes } => {
            docs.remove((handle, doc));
            let (reply, opened) = open(pdfium, doc, &bytes);
            if let Some(d) = opened {
                docs.insert((handle, doc), d);
            }
            Some(reply)
        }
        DocRequest::Render {
            doc,
            page,
            px_width,
        } => Some(
            docs.get((handle, doc))
                .and_then(|d| render(d, doc, page, px_width))
                .map_or(DocReply::Unreadable { doc }, DocReply::Rendered),
        ),
    }
}

fn open<'a>(pdfium: Option<&'a Pdfium>, doc: u64, bytes: &[u8]) -> (DocReply, Option<Doc<'a>>) {
    match cl_api::FileKind::sniff(bytes) {
        Some(cl_api::FileKind::Pdf) => {
            let Some(pdfium) = pdfium else {
                return (DocReply::Unavailable { doc }, None);
            };
            match pdfium.load_pdf_from_byte_vec(bytes.to_vec(), None) {
                Ok(pdf) => {
                    let all = u32::try_from(pdf.pages().len()).unwrap_or(0);
                    if all == 0 {
                        return (DocReply::Unreadable { doc }, None);
                    }
                    let pages = all.min(MAX_PAGES);
                    (
                        DocReply::Opened {
                            doc,
                            pages,
                            truncated: all > MAX_PAGES,
                        },
                        Some(Doc::Pdf(pdf, pages)),
                    )
                }
                Err(PdfiumError::PdfiumLibraryInternalError(
                    PdfiumInternalError::PasswordError,
                )) => (DocReply::Protected { doc }, None),
                Err(_) => (DocReply::Unreadable { doc }, None),
            }
        }
        Some(cl_api::FileKind::Png | cl_api::FileKind::Jpeg) => match decode_image(bytes) {
            Some(img) => (
                DocReply::Opened {
                    doc,
                    pages: 1,
                    truncated: false,
                },
                Some(Doc::Image(img)),
            ),
            None => (DocReply::Unreadable { doc }, None),
        },
        None => (DocReply::Unreadable { doc }, None),
    }
}

/// The decode limits: [`MAX_IMAGE_SIDE`] a side, [`MAX_IMAGE_ALLOC`] bytes.
fn limits() -> Limits {
    let mut l = Limits::default();
    l.max_image_width = Some(MAX_IMAGE_SIDE);
    l.max_image_height = Some(MAX_IMAGE_SIDE);
    l.max_alloc = Some(MAX_IMAGE_ALLOC);
    l
}

/// Decode a PNG or JPEG within [`limits`], then shrink it to at most
/// [`MAX_SIDE`] on its long side. `None` for anything refused.
pub(crate) fn decode_image(bytes: &[u8]) -> Option<RgbaImage> {
    let mut reader = ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    reader.limits(limits());
    let img = reader.decode().ok()?;
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 {
        return None;
    }
    let (tw, th) = fit_pixels(w, w, h)?;
    if (tw, th) == (w, h) {
        Some(img.into_rgba8())
    } else {
        Some(image::imageops::thumbnail(&img.into_rgba8(), tw, th))
    }
}

/// The drawn size for a page `natural_w × natural_h` (any unit) asked
/// `want_w` pixels wide: the aspect kept, at least 1 px a side, at most
/// [`MAX_SIDE`] on the long side and [`MAX_PIXELS`] in all. `None` for a
/// page without a usable size.
#[must_use]
pub fn clamp_size(want_w: u32, natural_w: f32, natural_h: f32) -> Option<(u32, u32)> {
    if !(natural_w.is_finite() && natural_h.is_finite()) || natural_w <= 0.0 || natural_h <= 0.0 {
        return None;
    }
    let aspect = f64::from(natural_h) / f64::from(natural_w);
    let side = f64::from(MAX_SIDE);
    let mut w = f64::from(want_w.clamp(1, MAX_SIDE));
    let mut h = (w * aspect).round().max(1.0);
    if h > side {
        h = side;
        w = (side / aspect).round().max(1.0);
    }
    #[allow(clippy::cast_precision_loss)] // 32 M is exact in f64.
    let max = MAX_PIXELS as f64;
    if w * h > max {
        let s = (max / (w * h)).sqrt();
        w = (w * s).floor().max(1.0);
        h = (h * s).floor().max(1.0);
    }
    // Both are in 1..=4096 here.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Some((w as u32, h as u32))
}

/// [`clamp_size`] for whole-pixel sizes.
fn fit_pixels(want_w: u32, w: u32, h: u32) -> Option<(u32, u32)> {
    #[allow(clippy::cast_precision_loss)] // ≤ 12000.
    clamp_size(want_w, w as f32, h as f32)
}

fn render(d: &Doc<'_>, doc: u64, page: u32, px_width: u32) -> Option<RenderedPage> {
    match d {
        Doc::Pdf(pdf, pages) => {
            if page >= *pages {
                return None;
            }
            let p = pdf.pages().get(i32::try_from(page).ok()?).ok()?;
            let (nw, nh) = (p.width().value, p.height().value);
            let (w, h) = clamp_size(px_width, nw, nh)?;
            let config = PdfRenderConfig::new()
                .set_target_size(i32::try_from(w).ok()?, i32::try_from(h).ok()?)
                .render_form_data(true);
            let bitmap = p.render_with_config(&config).ok()?;
            let width = u32::try_from(bitmap.width()).ok()?;
            let height = u32::try_from(bitmap.height()).ok()?;
            let rgba = bitmap.as_rgba_bytes();
            if rgba.len() != (width as usize) * (height as usize) * 4 {
                return None;
            }
            // Points to whole units: page sizes are far below u32::MAX.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let (natural_width, natural_height) = (nw.round() as u32, nh.round() as u32);
            Some(RenderedPage {
                doc,
                page,
                width,
                height,
                rgba,
                natural_width,
                natural_height,
            })
        }
        Doc::Image(img) => {
            if page != 0 {
                return None;
            }
            let (w, h) = fit_pixels(px_width, img.width(), img.height())?;
            let scaled = if (w, h) == img.dimensions() {
                img.clone()
            } else {
                image::imageops::thumbnail(img, w, h)
            };
            Some(RenderedPage {
                doc,
                page,
                width: scaled.width(),
                height: scaled.height(),
                rgba: scaled.into_raw(),
                natural_width: img.width(),
                natural_height: img.height(),
            })
        }
    }
}

/// The library's file name on this platform.
fn lib_name() -> std::ffi::OsString {
    Pdfium::pdfium_platform_library_name()
}

/// The place `cargo xtask pdfium-fetch` puts the library (debug builds).
#[cfg(debug_assertions)]
fn dev_dir() -> Option<PathBuf> {
    let triple = if cfg!(all(windows, target_arch = "x86_64")) {
        "x86_64-pc-windows-msvc"
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "aarch64-apple-darwin"
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        "x86_64-apple-darwin"
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        "x86_64-unknown-linux-gnu"
    } else {
        return None;
    };
    Some(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/pdfium")
            .join(triple),
    )
}

#[cfg(not(debug_assertions))]
fn dev_dir() -> Option<PathBuf> {
    None
}

/// Where to look for the library, in order: `env` (`CL_PDFIUM_PATH`, only
/// when absolute; a folder or the library itself), `<exe_dir>/pdfium/`,
/// then `dev` (debug builds).
#[must_use]
pub fn library_candidates(
    env: Option<PathBuf>,
    exe_dir: Option<PathBuf>,
    dev: Option<PathBuf>,
) -> Vec<PathBuf> {
    let name = lib_name();
    let mut out = Vec::new();
    if let Some(p) = env.filter(|p| p.is_absolute()) {
        if p.is_dir() {
            out.push(p.join(&name));
        } else {
            out.push(p);
        }
    }
    if let Some(d) = exe_dir {
        out.push(d.join("pdfium").join(&name));
    }
    if let Some(d) = dev {
        out.push(d.join(&name));
    }
    out
}

/// The first candidate that is a file.
#[must_use]
pub fn find_library(candidates: &[PathBuf]) -> Option<&Path> {
    candidates
        .iter()
        .map(PathBuf::as_path)
        .find(|p| p.is_file())
}

/// The library this process uses (searched once).
#[must_use]
pub fn library_path() -> Option<&'static Path> {
    static FOUND: OnceLock<Option<PathBuf>> = OnceLock::new();
    FOUND
        .get_or_init(|| {
            let exe_dir = std::env::current_exe()
                .ok()
                .and_then(|e| e.parent().map(Path::to_path_buf));
            let c = library_candidates(
                std::env::var_os("CL_PDFIUM_PATH").map(PathBuf::from),
                exe_dir,
                dev_dir(),
            );
            find_library(&c).map(Path::to_path_buf)
        })
        .as_deref()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn fixture(name: &str) -> Arc<[u8]> {
        let p = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/docs")
            .join(name);
        Arc::from(std::fs::read(p).expect("fixture"))
    }

    /// Wait (bounded) for the next reply.
    fn reply(w: &DocWorker) -> DocReply {
        let end = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(r) = w.try_recv() {
                return r;
            }
            assert!(Instant::now() < end, "no reply from the worker");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// The shared worker when PDFium is here; `None` (test skipped) when it
    /// is not, unless `CL_PDFIUM_REQUIRE=1` (CI) makes that a failure.
    fn with_pdfium() -> Option<DocWorker> {
        if library_path().is_some() {
            return Some(DocWorker::start());
        }
        assert!(
            std::env::var("CL_PDFIUM_REQUIRE").as_deref() != Ok("1"),
            "PDFium is required (CL_PDFIUM_REQUIRE=1): run `cargo xtask pdfium-fetch`"
        );
        eprintln!("PDFium not present: skipped (run `cargo xtask pdfium-fetch`)");
        None
    }

    /// A worker that has no library, whatever this computer has.
    fn without_pdfium() -> DocWorker {
        let c = library_candidates(
            None,
            None,
            Some(std::env::temp_dir().join("cl-no-pdfium-here")),
        );
        assert!(find_library(&c).is_none());
        DocWorker::on(spawn(find_library(&c).map(Path::to_path_buf)))
    }

    #[test]
    fn clamp_keeps_the_aspect_and_the_limits() {
        // Letter at 800 px wide.
        assert_eq!(clamp_size(800, 612.0, 792.0), Some((800, 1035)));
        // Too wide: 4096 at most.
        assert_eq!(clamp_size(10_000, 612.0, 792.0), Some((3165, 4096)));
        // Very tall: the long side stops at 4096.
        assert_eq!(clamp_size(4096, 100.0, 10_000.0), Some((41, 4096)));
        // Very wide: the width stops at 4096, the height at least 1.
        assert_eq!(clamp_size(9000, 100_000.0, 1.0), Some((4096, 1)));
        assert_eq!(clamp_size(0, 612.0, 792.0), Some((1, 1)));
        for bad in [
            (0.0, 1.0),
            (1.0, 0.0),
            (-1.0, 1.0),
            (f32::NAN, 1.0),
            (1.0, f32::INFINITY),
        ] {
            assert_eq!(clamp_size(800, bad.0, bad.1), None, "{bad:?}");
        }
        for w in [1, 17, 999, 4096, 5000] {
            let (a, b) = clamp_size(w, 3.0, 2.0).unwrap();
            assert!(a <= MAX_SIDE && b <= MAX_SIDE);
            assert!(u64::from(a) * u64::from(b) <= MAX_PIXELS);
        }
    }

    #[test]
    fn the_search_order_is_env_then_exe_then_dev() {
        let tmp = tempfile::tempdir().unwrap();
        let name = lib_name();
        let env_dir = tmp.path().join("env");
        std::fs::create_dir_all(&env_dir).unwrap();
        let exe = tmp.path().join("exe");
        let dev = tmp.path().join("dev");
        let c = library_candidates(Some(env_dir.clone()), Some(exe.clone()), Some(dev.clone()));
        assert_eq!(
            c,
            vec![
                env_dir.join(&name),
                exe.join("pdfium").join(&name),
                dev.join(&name)
            ]
        );
        // Nothing there: no library, and a worker answers Unavailable.
        assert_eq!(find_library(&c), None);
        // A file path is taken as is; a relative one is ignored.
        let file = tmp.path().join("custom").join(&name);
        assert_eq!(
            library_candidates(Some(file.clone()), None, None),
            vec![file]
        );
        assert_eq!(
            library_candidates(Some(PathBuf::from("pdfium")), None, None),
            Vec::<PathBuf>::new()
        );
        // The first that exists wins.
        std::fs::create_dir_all(&dev).unwrap();
        std::fs::write(dev.join(&name), b"x").unwrap();
        assert_eq!(find_library(&c), Some(dev.join(&name).as_path()));
        std::fs::create_dir_all(exe.join("pdfium")).unwrap();
        std::fs::write(exe.join("pdfium").join(&name), b"x").unwrap();
        assert_eq!(
            find_library(&c),
            Some(exe.join("pdfium").join(&name).as_path())
        );
    }

    #[test]
    fn without_pdfium_a_pdf_is_unavailable_and_images_still_open() {
        let w = without_pdfium();
        w.send(DocRequest::Open {
            doc: 1,
            bytes: fixture("two_pages.pdf"),
        });
        assert_eq!(reply(&w), DocReply::Unavailable { doc: 1 });
        w.send(DocRequest::Open {
            doc: 2,
            bytes: fixture("receipt.png"),
        });
        assert_eq!(
            reply(&w),
            DocReply::Opened {
                doc: 2,
                pages: 1,
                truncated: false
            }
        );
        w.send(DocRequest::Render {
            doc: 2,
            page: 0,
            px_width: 160,
        });
        let DocReply::Rendered(p) = reply(&w) else {
            panic!("not rendered")
        };
        assert_eq!((p.width, p.height, p.natural_width), (160, 100, 320));
        assert_eq!(p.rgba.len(), 160 * 100 * 4);
        // A page that isn't there, a document that isn't open.
        w.send(DocRequest::Render {
            doc: 2,
            page: 1,
            px_width: 160,
        });
        assert_eq!(reply(&w), DocReply::Unreadable { doc: 2 });
        w.send(DocRequest::Close { doc: 2 });
        w.send(DocRequest::Render {
            doc: 2,
            page: 0,
            px_width: 160,
        });
        assert_eq!(reply(&w), DocReply::Unreadable { doc: 2 });
        let dbg = format!(
            "{:?}",
            DocRequest::Open {
                doc: 3,
                bytes: Arc::from(&b"secret"[..])
            }
        );
        assert!(!dbg.contains("secret") && dbg.contains("len"));
    }

    #[test]
    fn png_and_jpeg_fixtures_decode() {
        for name in ["receipt.png", "receipt.jpg"] {
            let img = decode_image(&fixture(name)).expect(name);
            assert_eq!(img.dimensions(), (320, 200), "{name}");
            // Not blank: the synthetic drawing has dark pixels.
            assert!(img.pixels().any(|p| p.0[0] < 100), "{name}");
        }
        assert!(decode_image(b"\x89PNG\r\n\x1a\nnot really").is_none());
        assert!(decode_image(b"").is_none());
    }

    #[test]
    fn a_decompression_bomb_is_refused_at_the_header() {
        let bomb = fixture("bomb_30000.png");
        assert!(bomb.len() < 100, "the fixture is a header, not pixels");
        assert!(decode_image(&bomb).is_none());
        let w = without_pdfium();
        w.send(DocRequest::Open {
            doc: 9,
            bytes: bomb,
        });
        assert_eq!(reply(&w), DocReply::Unreadable { doc: 9 });
    }

    #[test]
    fn newer_requests_supersede_queued_ones() {
        let (tx, _rx) = channel();
        let r = |handle, req| Job::Req {
            handle,
            req,
            reply: tx.clone(),
        };
        let render = |doc, px_width| DocRequest::Render {
            doc,
            page: 0,
            px_width,
        };
        let open = |doc| DocRequest::Open {
            doc,
            bytes: Arc::from(&b"x"[..]),
        };
        let kept: Vec<String> = coalesce(vec![
            r(1, render(1, 100)),
            r(1, open(1)),
            r(1, render(1, 200)),
            r(1, render(1, 300)),
            r(1, render(2, 50)),
            r(2, render(1, 10)),
            Job::CloseHandle(2),
            r(1, DocRequest::Close { doc: 3 }),
        ])
        .into_iter()
        .map(|j| match j {
            Job::CloseHandle(h) => format!("close-handle {h}"),
            Job::Req { handle, req, .. } => format!("{handle} {req:?}"),
        })
        .collect();
        assert_eq!(
            kept,
            vec![
                "1 Open { doc: 1, len: 1 }",
                "1 Render { doc: 1, page: 0, px_width: 300 }",
                "1 Render { doc: 2, page: 0, px_width: 50 }",
                "close-handle 2",
                "1 Close { doc: 3 }",
            ]
        );
    }

    #[test]
    fn a_two_page_pdf_opens_and_renders_with_pdfium() {
        let Some(w) = with_pdfium() else { return };
        w.send(DocRequest::Open {
            doc: 1,
            bytes: fixture("two_pages.pdf"),
        });
        assert_eq!(
            reply(&w),
            DocReply::Opened {
                doc: 1,
                pages: 2,
                truncated: false
            }
        );
        w.send(DocRequest::Render {
            doc: 1,
            page: 1,
            px_width: 400,
        });
        let DocReply::Rendered(p) = reply(&w) else {
            panic!("not rendered")
        };
        assert_eq!((p.page, p.width), (1, 400));
        assert!((517..=518).contains(&p.height), "{}", p.height);
        assert_eq!((p.natural_width, p.natural_height), (612, 792));
        assert_eq!(p.rgba.len(), 400 * p.height as usize * 4);
        // Not blank: page 2 has a filled green square.
        let dark = p
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|px| px[0] < 128 && px[3] > 0)
            .count();
        assert!(dark > 1000, "page looks blank ({dark} dark pixels)");
        // A huge request is clamped.
        w.send(DocRequest::Render {
            doc: 1,
            page: 0,
            px_width: 50_000,
        });
        let DocReply::Rendered(p) = reply(&w) else {
            panic!("not rendered")
        };
        assert!(p.width.max(p.height) <= MAX_SIDE);
    }

    #[test]
    fn an_encrypted_pdf_is_protected_with_pdfium() {
        let Some(w) = with_pdfium() else { return };
        w.send(DocRequest::Open {
            doc: 5,
            bytes: fixture("encrypted.pdf"),
        });
        assert_eq!(reply(&w), DocReply::Protected { doc: 5 });
        w.send(DocRequest::Open {
            doc: 6,
            bytes: Arc::from(&b"%PDF-1.7 not a real file"[..]),
        });
        assert_eq!(reply(&w), DocReply::Unreadable { doc: 6 });
    }
}
