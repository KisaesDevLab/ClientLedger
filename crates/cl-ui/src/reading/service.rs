//! The reading service (A03 step 17, plan D7).
//!
//! One thread owns the reader (`cl_host::DocreadHost`, whose `read`
//! blocks for seconds per page) and takes requests one at a time; the egui
//! thread only sends a request and polls the answer's channel each frame.
//! The reader is made on the first request, so an app that never reads a
//! document never starts the worker.
//!
//! After the worker's reading comes back, the desktop does what needs the
//! company's data (Q-A03o): it matches the vendor against the vendors it
//! was given and, when the engine stores card digits (`document-drafts-v1`,
//! WP-F), picks the card account. Without that capability there are no
//! templates and no card match: rules only (S12). The optional AI readers
//! (step 21) are not part of A03's local path.
//!
//! Step 19a (WP-F) adds what needs the engine's `document-drafts-v1`:
//! the company's saved layouts ([`templates_of`], [`with_layouts`]), the
//! card accounts by their last four digits ([`card_accounts_of`]), both
//! loaded by [`load_context`], and [`claim_and_store`], which claims an
//! uploaded draft file and stores its reading (Q-A03i). Enter Bill's "Read
//! document" uses the layouts and card accounts but never claims (Q-A03l);
//! A04's queue claims on arrival.
//!
//! Logs carry outcome classes only: never a file name, text or an amount.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};

use cl_core::CalendarDate;
use cl_core::ids::{AccountId, ContactId};
use cl_docread::card::{self, CardPick};
use cl_docread::model::{DocKind, Last4, Payment, Reading, Template, Word};
use cl_docread::protocol::{JobFailure, JobHint};
use cl_docread::vendor_match::{self, VendorCandidate, VendorMatch};
use cl_host::{CouldNotRead, DocJob, DocreadConfig, DocreadHost, JobOutcome};
use cl_model::doc_draft::PageBox;

// ---- Step 19a: layouts, card accounts and claims (`document-drafts-v1`) ----

/// Version of the layout JSON stored in `fieldRegions`.
const LAYOUT_VERSION: u64 = 1;

/// The company's saved layouts as the reader takes them, with their
/// vendor: each engine template's `fieldRegions.template`. A row this
/// version can't read is left out (the rules read that document).
#[must_use]
pub fn templates_of(saved: &[cl_api::VendorTemplate]) -> Vec<(ContactId, Template)> {
    saved
        .iter()
        .filter(|t| {
            t.field_regions.get("v").and_then(serde_json::Value::as_u64) == Some(LAYOUT_VERSION)
        })
        .filter_map(|t| {
            let raw = t.field_regions.get("template")?.clone();
            let mut tpl: Template = serde_json::from_value(raw).ok()?;
            tpl.samples = t.samples;
            Some((t.contact_id, tpl))
        })
        .collect()
}

/// The engine row for a layout of `vendor` (A04 learns and saves them):
/// keyed by the layout's tokens and page shape, the whole layout in
/// `fieldRegions`. `expected` is the stored row's `updated_at` when
/// replacing it.
///
/// # Errors
/// `Decode` when the layout can't be written as JSON.
pub fn template_upsert(
    vendor: ContactId,
    template: &Template,
    expected: Option<String>,
) -> Result<cl_api::TemplateUpsert, cl_api::ApiError> {
    let mut parts: Vec<String> = template.fingerprint.tokens.clone();
    parts.push(format!("aspect:{}", template.fingerprint.aspect));
    let mut regions = serde_json::Map::new();
    regions.insert("v".into(), serde_json::Value::from(LAYOUT_VERSION));
    regions.insert(
        "template".into(),
        serde_json::to_value(template).map_err(|_| cl_api::ApiError::Decode {
            what: "layout template",
        })?,
    );
    Ok(cl_api::TemplateUpsert {
        contact: vendor,
        fingerprint: cl_api::layout_key(&parts),
        field_regions: regions,
        line_table: None,
        samples: template.samples.max(1),
        expected_updated_at: expected,
    })
}

/// The vendors with their saved layouts' fingerprints (the "past layout"
/// match), and the layouts for the reader, in one order.
#[must_use]
pub fn with_layouts(
    mut vendors: Vec<VendorCandidate>,
    layouts: &[(ContactId, Template)],
) -> (Vec<VendorCandidate>, Vec<Template>) {
    for v in &mut vendors {
        v.layouts.extend(
            layouts
                .iter()
                .filter(|(c, _)| *c == v.contact)
                .map(|(_, t)| t.fingerprint.clone()),
        );
    }
    let templates = layouts.iter().map(|(_, t)| t.clone()).collect();
    (vendors, templates)
}

/// Active credit card accounts with their last four digits (Q-A03q).
#[must_use]
pub fn card_accounts_of(accounts: &[cl_api::Account]) -> Vec<(AccountId, Last4)> {
    accounts
        .iter()
        .filter(|a| {
            a.is_active && a.fields.detail_type.as_deref() == Some(cl_api::CREDIT_CARD_DETAIL_TYPE)
        })
        .filter_map(|a| Some((a.id, Last4::parse(a.card_last4?.as_str())?)))
        .collect()
}

/// What the desktop matches a reading against, from the engine.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct ReadContext {
    /// Saved layouts with their vendor (empty without the capability).
    pub layouts: Vec<(ContactId, Template)>,
    /// Card accounts; `None` without `document-drafts-v1` (no card match).
    pub card_accounts: Option<Vec<(AccountId, Last4)>>,
}

impl std::fmt::Debug for ReadContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReadContext")
            .field("layouts", &self.layouts.len())
            .field("card_accounts", &self.card_accounts.as_ref().map(Vec::len))
            .finish()
    }
}

/// Loads the layouts and card accounts. Without `document-drafts-v1`
/// nothing is fetched: rules only, no card match (S12).
///
/// # Errors
/// Any [`cl_api::ApiError`] of the two reads.
pub async fn load_context(session: &cl_api::Session) -> Result<ReadContext, cl_api::ApiError> {
    if !session
        .client()
        .capabilities()
        .has(cl_api::Capabilities::DOCUMENT_DRAFTS)
    {
        return Ok(ReadContext::default());
    }
    let saved = session.vendor_templates().list(None).await?;
    let accounts = session
        .accounts()
        .list_all(&cl_api::AccountFilter::default())
        .await?;
    Ok(ReadContext {
        layouts: templates_of(&saved),
        card_accounts: Some(card_accounts_of(&accounts)),
    })
}

/// The engine's kind for a reading's kind.
#[must_use]
pub fn draft_kind(k: DocKind) -> cl_api::DraftKind {
    match k {
        DocKind::Bill => cl_api::DraftKind::Bill,
        DocKind::CardCharge => cl_api::DraftKind::CardCharge,
        DocKind::Check => cl_api::DraftKind::Check,
        DocKind::CashExpense => cl_api::DraftKind::CashExpense,
        DocKind::Unknown => cl_api::DraftKind::Unknown,
    }
}

/// A local reading as the engine stores it: the header, lines and checks
/// as JSON (amounts as decimal strings, spots and confidences as integers),
/// the vendor match and the duplicate-check fields. The page words are not
/// stored.
///
/// # Errors
/// `Decode` when the reading can't be written as JSON.
pub fn draft_reading(local: &LocalReading) -> Result<cl_api::DraftReading, cl_api::ApiError> {
    let json_err = |_| cl_api::ApiError::Decode {
        what: "document reading",
    };
    let r = &local.reading;
    let fields = match serde_json::to_value(&r.header).map_err(json_err)? {
        serde_json::Value::Object(m) => m,
        _ => serde_json::Map::new(),
    };
    let to_values = |v: serde_json::Value| match v {
        serde_json::Value::Array(a) => a,
        _ => Vec::new(),
    };
    let reader = match r.reader {
        cl_docread::model::Reader::TextPdf => cl_api::DraftReader::TextPdf,
        cl_docread::model::Reader::Ocr => cl_api::DraftReader::Ocr,
        cl_docread::model::Reader::Template => cl_api::DraftReader::Template,
    };
    let h = &r.header;
    Ok(cl_api::DraftReading {
        kind: draft_kind(r.kind),
        reader,
        fields,
        lines: to_values(serde_json::to_value(&r.lines).map_err(json_err)?),
        checks: to_values(serde_json::to_value(&r.checks).map_err(json_err)?),
        suggested_contact: local.vendor.map(|v| v.contact),
        doc_reference: h.reference.as_ref().map(|f| f.value.clone()),
        doc_total: h.total.as_ref().or(h.balance_due.as_ref()).map(|f| f.value),
        doc_date: h.date.as_ref().map(|f| f.value),
    })
}

/// Claims an uploaded draft file (`file`) as a document draft and stores
/// what reading it gave: the reading (`ready`) or the failure class
/// (`failed`). Keyed: a retry of the claim with `key` gets the same draft.
///
/// # Errors
/// `Local(Unsupported)` without `document-drafts-v1` (nothing sent);
/// [`cl_api::DraftRefusal::AlreadyClaimed`] naming the draft that holds
/// the same file; any [`cl_api::ApiError`] of the claim or the save (a
/// failed save leaves the claimed draft `new`).
pub async fn claim_and_store(
    session: &cl_api::Session,
    file: cl_api::AttachmentId,
    hint: cl_api::DraftKind,
    outcome: &ReadOutcome,
    key: cl_api::IdempotencyKey,
) -> Result<cl_api::DocumentDraft, cl_api::ApiError> {
    let drafts = session.document_drafts();
    let draft = drafts.create(file, hint, key).await?;
    match outcome {
        ReadOutcome::Read(local) => {
            drafts
                .store_reading(draft.id, &draft.updated_at, &draft_reading(local)?)
                .await
        }
        ReadOutcome::Failed(f) => {
            drafts
                .store_failure(draft.id, &draft.updated_at, f.class())
                .await
        }
    }
}

/// Something that reads documents: the worker host, or a stand-in in
/// tests. Blocking.
pub trait Reader: Send {
    /// One outcome per job, in order.
    fn read(&mut self, jobs: &[DocJob], today: CalendarDate) -> Vec<JobOutcome>;

    /// [`Reader::read`], stopped when `cancel` is set (A03 review S2). A
    /// reader that can't stop mid-read still answers nothing once
    /// cancelled.
    fn read_cancellable(
        &mut self,
        jobs: &[DocJob],
        today: CalendarDate,
        cancel: &Arc<AtomicBool>,
    ) -> Vec<JobOutcome> {
        if cancel.load(Ordering::SeqCst) {
            return jobs
                .iter()
                .map(|_| JobOutcome::CouldNotRead(CouldNotRead::Cancelled))
                .collect();
        }
        self.read(jobs, today)
    }

    /// A04 WP-C: the words inside `request.area` (protocol 4, a region
    /// job). Readers that can't read boxes answer
    /// [`RegionOutcome::NotSupported`]; the worker host does once WP-R's
    /// protocol 4 lands (F2 wires `DocreadHost`'s region job here).
    fn read_region(&mut self, request: &RegionRequest, cancel: &Arc<AtomicBool>) -> RegionOutcome {
        let _ = (request, cancel);
        RegionOutcome::NotSupported
    }
}

impl Reader for DocreadHost {
    fn read(&mut self, jobs: &[DocJob], today: CalendarDate) -> Vec<JobOutcome> {
        DocreadHost::read(self, jobs, today)
    }

    fn read_cancellable(
        &mut self,
        jobs: &[DocJob],
        today: CalendarDate,
        cancel: &Arc<AtomicBool>,
    ) -> Vec<JobOutcome> {
        // The worker is killed as soon as the flag is set.
        self.read_with_cancel(jobs, today, Arc::clone(cancel))
    }
}

/// Makes the reader on the service thread; `None` when it can't be run on
/// this computer (no worker program).
pub type MakeReader = Box<dyn FnOnce() -> Option<Box<dyn Reader>> + Send>;

/// One document to read, with what the desktop matches it against.
/// `Debug` shows shapes only.
#[derive(Clone)]
pub struct ReadRequest {
    /// The file's bytes (the worker sniffs the type).
    pub bytes: Arc<[u8]>,
    /// What the owner is entering (Enter Bill: bills).
    pub hint: JobHint,
    /// The owner's today (two-digit years, date checks).
    pub today: CalendarDate,
    /// The company's vendors (from `GET /contacts`).
    pub vendors: Vec<VendorCandidate>,
    /// The company's layout templates (`document-drafts-v1` only; empty in
    /// A03 until WP-F).
    pub templates: Vec<Template>,
    /// Card accounts with their last four digits; `None` without
    /// `document-drafts-v1` (no card match then).
    pub card_accounts: Option<Vec<(AccountId, Last4)>>,
}

impl std::fmt::Debug for ReadRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReadRequest")
            .field("bytes", &self.bytes.len())
            .field("hint", &self.hint)
            .field("vendors", &self.vendors.len())
            .field("templates", &self.templates.len())
            .field("card_accounts", &self.card_accounts.as_ref().map(Vec::len))
            .finish_non_exhaustive()
    }
}

/// A document read on this computer, with the desktop's matches.
#[derive(Clone, PartialEq, Eq)]
pub struct LocalReading {
    /// The worker's reading (checks included; the card check added here).
    pub reading: Reading,
    /// The vendor it is from, when one matched.
    pub vendor: Option<VendorMatch>,
    /// The card account, when card digits were printed and card accounts
    /// were given.
    pub card: Option<CardPick>,
}

impl std::fmt::Debug for LocalReading {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalReading")
            .field("reading", &self.reading)
            .field("vendor", &self.vendor.map(|v| v.tier))
            .field("card", &self.card.map(|c| matches!(c, CardPick::One(_))))
            .finish()
    }
}

/// Why a document was not read. Every one tells the owner "Couldn't read,
/// enter by hand"; some add a plain reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadFailure {
    /// A password-protected PDF.
    Protected,
    /// Over a size limit.
    TooLarge,
    /// Not a PDF or an image the reader knows.
    Unsupported,
    /// A scan, and scans can't be read on this computer (no OCR).
    NeedsOcr,
    /// Damaged, empty, or nothing on it to read.
    Unreadable,
    /// The reader gave up (crashed, too slow, too much memory).
    GaveUp,
    /// The reader isn't installed or couldn't start.
    Unavailable,
    /// The owner cancelled it (nothing is shown).
    Cancelled,
}

impl ReadFailure {
    /// The class logged and shown on the control channel.
    #[must_use]
    pub fn class(self) -> &'static str {
        match self {
            ReadFailure::Protected => "protected",
            ReadFailure::TooLarge => "too_large",
            ReadFailure::Unsupported => "unsupported",
            ReadFailure::NeedsOcr => "needs_ocr",
            ReadFailure::Unreadable => "unreadable",
            ReadFailure::GaveUp => "gave_up",
            ReadFailure::Unavailable => "unavailable",
            ReadFailure::Cancelled => "cancelled",
        }
    }

    fn of_job(f: JobFailure) -> ReadFailure {
        match f {
            JobFailure::Protected => ReadFailure::Protected,
            JobFailure::TooLarge => ReadFailure::TooLarge,
            JobFailure::UnsupportedType => ReadFailure::Unsupported,
            JobFailure::NeedsOcr => ReadFailure::NeedsOcr,
            JobFailure::Unreadable | JobFailure::Empty => ReadFailure::Unreadable,
        }
    }

    fn of_gave_up(c: CouldNotRead) -> ReadFailure {
        match c {
            CouldNotRead::Unavailable => ReadFailure::Unavailable,
            CouldNotRead::Crashed
            | CouldNotRead::TimedOut
            | CouldNotRead::TooMuchMemory
            | CouldNotRead::BadReply => ReadFailure::GaveUp,
            CouldNotRead::Cancelled => ReadFailure::Cancelled,
        }
    }
}

/// The answer to one request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadOutcome {
    /// Read (it may carry checks for the owner).
    Read(Box<LocalReading>),
    /// Not read.
    Failed(ReadFailure),
}

impl ReadOutcome {
    /// The class logged and shown on the control channel.
    #[must_use]
    pub fn class(&self) -> &'static str {
        match self {
            ReadOutcome::Read(_) => "read",
            ReadOutcome::Failed(f) => f.class(),
        }
    }
}

/// One piece of work for the service thread.
enum Message {
    /// Read a document.
    Read {
        request: ReadRequest,
        reply: Sender<ReadOutcome>,
        cancel: Arc<AtomicBool>,
    },
    /// A04 WP-C: read the words inside a box (protocol 4, interactive).
    Region {
        request: RegionRequest,
        reply: Sender<RegionOutcome>,
        cancel: Arc<AtomicBool>,
    },
}

/// A submitted reading: its answer, and a way to stop it.
#[derive(Debug)]
pub struct ReadJob {
    /// The answer (poll it with `try_recv`).
    pub answer: Receiver<ReadOutcome>,
    /// Stops it.
    pub cancel: ReadCancel,
}

impl ReadJob {
    /// Stops the reading (see [`ReadCancel::cancel`]).
    pub fn cancel(&self) {
        self.cancel.cancel();
    }
}

/// Stops one submitted reading.
#[derive(Debug, Clone, Default)]
pub struct ReadCancel(Arc<AtomicBool>);

impl ReadCancel {
    /// A queued reading is never read; a running one's worker is stopped.
    /// The answer is then `Failed(Cancelled)`.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// Reads documents on its own thread (see the module comment).
pub struct ReadingService {
    tx: Sender<Message>,
}

impl std::fmt::Debug for ReadingService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReadingService").finish_non_exhaustive()
    }
}

impl ReadingService {
    /// A service whose reader `make` builds on the first request.
    #[must_use]
    pub fn start(make: MakeReader) -> ReadingService {
        let (tx, rx) = mpsc::channel::<Message>();
        // A thread that can't start leaves `rx` dropped: every request is
        // then answered "unavailable" by `submit`.
        let _ = std::thread::Builder::new()
            .name("docread-service".into())
            .spawn(move || serve(&rx, make));
        ReadingService { tx }
    }

    /// The app's service: the worker program next to the app (or the
    /// checkout's in a debug build), tesseract when installed, scratch
    /// folders under the app data folder.
    #[must_use]
    pub fn for_app(paths: &cl_host::AppPaths) -> ReadingService {
        let paths = paths.clone();
        ReadingService::start(Box::new(move || {
            let Some(exe) = cl_host::find_docread_exe(None) else {
                tracing::warn!("document reader not found; documents can't be read");
                return None;
            };
            let ocr = cl_host::OcrTools::find();
            tracing::info!(ocr = ocr.is_some(), "document reader ready");
            let host: Box<dyn Reader> =
                Box::new(DocreadHost::new(DocreadConfig::new(&paths, exe, ocr)));
            Some(host)
        }))
    }

    /// Reads `request` off this thread; the answer arrives on the channel
    /// returned (poll it with `try_recv`).
    #[must_use]
    pub fn submit(&self, request: ReadRequest) -> Receiver<ReadOutcome> {
        self.submit_cancellable(request).answer
    }

    /// [`ReadingService::submit`] with a Cancel (A03 review S2).
    #[must_use]
    pub fn submit_cancellable(&self, request: ReadRequest) -> ReadJob {
        let (reply, answer) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let message = Message::Read {
            request,
            reply,
            cancel: Arc::clone(&cancel),
        };
        if let Err(mpsc::SendError(Message::Read { reply, .. })) = self.tx.send(message) {
            let _ = reply.send(ReadOutcome::Failed(ReadFailure::Unavailable));
        }
        ReadJob {
            answer,
            cancel: ReadCancel(cancel),
        }
    }

    /// A04 WP-C: reads the words inside `request.area` off this thread (a
    /// box dragged on a scan, D4). The service takes one job at a time, so
    /// a box sent while Read all runs is read before the next document the
    /// batch hands it (T13). The decoding, cropping and OCR happen in the
    /// sandboxed worker only; the desktop gets words back.
    #[must_use]
    pub fn submit_region(&self, request: RegionRequest) -> RegionJob {
        let (reply, answer) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let message = Message::Region {
            request,
            reply,
            cancel: Arc::clone(&cancel),
        };
        if let Err(mpsc::SendError(Message::Region { reply, .. })) = self.tx.send(message) {
            let _ = reply.send(RegionOutcome::Failed(ReadFailure::Unavailable));
        }
        RegionJob {
            answer,
            cancel: ReadCancel(cancel),
        }
    }
}

// ---- A04 WP-C: box re-reads and the session's page words ----

/// A box on one page to read again in the worker (A04 D4 "Drag a box",
/// protocol 4). `Debug` shows the box only.
#[derive(Clone)]
pub struct RegionRequest {
    /// The document's bytes.
    pub bytes: Arc<[u8]>,
    /// The box, on the original page (the worker applies the page's
    /// detected turn and shear, S9).
    pub area: PageBox,
    /// The owner's today.
    pub today: CalendarDate,
}

impl std::fmt::Debug for RegionRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RegionRequest")
            .field("bytes", &self.bytes.len())
            .field("area", &self.area)
            .finish_non_exhaustive()
    }
}

/// The answer to a box re-read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegionOutcome {
    /// The words found in the box, placed on the original page (`Word`'s
    /// `Debug` shows no text).
    Words(Vec<Word>),
    /// This reader can't read boxes (a worker before protocol 4).
    NotSupported,
    /// Not read.
    Failed(ReadFailure),
}

impl RegionOutcome {
    /// The class logged.
    #[must_use]
    pub fn class(&self) -> &'static str {
        match self {
            RegionOutcome::Words(_) => "read",
            RegionOutcome::NotSupported => "not_supported",
            RegionOutcome::Failed(f) => f.class(),
        }
    }
}

/// A submitted box re-read: its answer, and a way to stop it.
#[derive(Debug)]
pub struct RegionJob {
    /// The answer (poll it with `try_recv`).
    pub answer: Receiver<RegionOutcome>,
    /// Stops it.
    pub cancel: ReadCancel,
}

impl RegionJob {
    /// Stops the re-read (a newer pick or box replaced it).
    pub fn cancel(&self) {
        self.cancel.cancel();
    }
}

/// The page words of the drafts read this session (A04 D4): at most
/// [`cl_model::doc_draft::WORD_CACHE_DRAFTS`] readings, the least recently
/// used dropped first. Words aren't stored with a draft; a miss reads the
/// document again in the worker. Keyed by the engine draft id as text.
#[derive(Default)]
pub struct WordCache {
    /// Most recently used last.
    entries: Vec<(String, Arc<LocalReading>)>,
}

impl std::fmt::Debug for WordCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WordCache")
            .field("drafts", &self.entries.len())
            .finish()
    }
}

impl WordCache {
    /// Empty.
    #[must_use]
    pub fn new() -> WordCache {
        WordCache::default()
    }

    /// Keeps `reading` for `draft`, dropping the least recently used over
    /// the limit.
    pub fn put(&mut self, draft: &str, reading: Arc<LocalReading>) {
        self.entries.retain(|(k, _)| k != draft);
        self.entries.push((draft.to_string(), reading));
        let over = self
            .entries
            .len()
            .saturating_sub(cl_model::doc_draft::WORD_CACHE_DRAFTS);
        self.entries.drain(..over);
    }

    /// The reading kept for `draft` (now the most recently used).
    pub fn get(&mut self, draft: &str) -> Option<Arc<LocalReading>> {
        let at = self.entries.iter().position(|(k, _)| k == draft)?;
        let e = self.entries.remove(at);
        let r = Arc::clone(&e.1);
        self.entries.push(e);
        Some(r)
    }

    /// Forgets `draft` (entered, discarded).
    pub fn forget(&mut self, draft: &str) {
        self.entries.retain(|(k, _)| k != draft);
    }

    /// Readings kept.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Nothing kept.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// The compare tools' words from a local reading.
#[must_use]
pub fn doc_words(local: &LocalReading) -> crate::widgets::compare::DocWords {
    crate::widgets::compare::DocWords {
        pages: local.reading.words.clone(),
    }
}

fn serve(rx: &Receiver<Message>, make: MakeReader) {
    let mut make = Some(make);
    let mut reader: Option<Box<dyn Reader>> = None;
    while let Ok(message) = rx.recv() {
        let (request, reply, cancel) = match message {
            Message::Read {
                request,
                reply,
                cancel,
            } => (request, reply, cancel),
            Message::Region {
                request,
                reply,
                cancel,
            } => {
                serve_region(&mut reader, &mut make, &request, &reply, &cancel);
                continue;
            }
        };
        if cancel.load(Ordering::SeqCst) {
            // Cancelled while waiting: never read.
            let _ = reply.send(ReadOutcome::Failed(ReadFailure::Cancelled));
            continue;
        }
        if reader.is_none()
            && let Some(m) = make.take()
        {
            reader = m();
        }
        let outcome = match reader.as_mut() {
            Some(r) => read_one(r.as_mut(), &request, &cancel),
            None => ReadOutcome::Failed(ReadFailure::Unavailable),
        };
        // A Cancel that came in while reading wins over its answer.
        let outcome = if cancel.load(Ordering::SeqCst) {
            ReadOutcome::Failed(ReadFailure::Cancelled)
        } else {
            outcome
        };
        tracing::info!(outcome = outcome.class(), "document read");
        // The window may have closed meanwhile: nobody waits then.
        let _ = reply.send(outcome);
    }
}

/// One box re-read on the service thread (the reader is made on first use,
/// as for a read).
fn serve_region(
    reader: &mut Option<Box<dyn Reader>>,
    make: &mut Option<MakeReader>,
    request: &RegionRequest,
    reply: &Sender<RegionOutcome>,
    cancel: &Arc<AtomicBool>,
) {
    if cancel.load(Ordering::SeqCst) {
        let _ = reply.send(RegionOutcome::Failed(ReadFailure::Cancelled));
        return;
    }
    if reader.is_none()
        && let Some(m) = make.take()
    {
        *reader = m();
    }
    let outcome = match reader.as_mut() {
        Some(r) => r.read_region(request, cancel),
        None => RegionOutcome::Failed(ReadFailure::Unavailable),
    };
    let outcome = if cancel.load(Ordering::SeqCst) {
        RegionOutcome::Failed(ReadFailure::Cancelled)
    } else {
        outcome
    };
    tracing::info!(outcome = outcome.class(), "box re-read");
    let _ = reply.send(outcome);
}

/// Reads one document and adds the desktop's matches.
pub(crate) fn read_one(
    reader: &mut dyn Reader,
    request: &ReadRequest,
    cancel: &Arc<AtomicBool>,
) -> ReadOutcome {
    let job = DocJob {
        bytes: request.bytes.to_vec(),
        hint: request.hint,
        templates: request.templates.clone(),
    };
    let outcome = reader
        .read_cancellable(std::slice::from_ref(&job), request.today, cancel)
        .into_iter()
        .next();
    match outcome {
        Some(JobOutcome::Read(reading)) => ReadOutcome::Read(Box::new(matched(*reading, request))),
        Some(JobOutcome::Failed(f)) => ReadOutcome::Failed(ReadFailure::of_job(f)),
        Some(JobOutcome::CouldNotRead(c)) => ReadOutcome::Failed(ReadFailure::of_gave_up(c)),
        None => ReadOutcome::Failed(ReadFailure::GaveUp),
    }
}

/// The vendor and card matches on the extracted strings (Q-A03o). The
/// reading's values are never changed; a card the owner must pick adds a
/// check.
pub(crate) fn matched(mut reading: Reading, request: &ReadRequest) -> LocalReading {
    let vendor = vendor_match::match_vendor(
        &reading.header,
        reading.fingerprint.as_ref(),
        &request.vendors,
    );
    // A printed name that won over a past layout is checked (review S4).
    if let Some(check) = vendor
        .as_ref()
        .and_then(|m| vendor_match::layout_check(m, &reading.header, &request.vendors))
        && reading.checks.len() < cl_docread::model::MAX_CHECKS
    {
        reading.checks.push(check);
    }
    let card = match (&request.card_accounts, &reading.header.payment) {
        (Some(accounts), Some(p)) => match &p.value {
            Payment::Card { last4 } => Some(card::card_account(*last4, accounts)),
            _ => None,
        },
        _ => None,
    };
    if let Some(pick) = card
        && let Some(check) = card::pick_the_card_check(&reading, pick)
        && reading.checks.len() < cl_docread::model::MAX_CHECKS
    {
        reading.checks.push(check);
    }
    LocalReading {
        reading,
        vendor,
        card,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cl_core::Money;
    use cl_core::ids::ContactId;
    use cl_docread::model::{Bbox, CheckCode, Confidence, Found, Reader as Source, RuleId, Spot};
    use std::time::Duration;

    fn today() -> CalendarDate {
        CalendarDate::from_ymd(2026, 11, 4).unwrap()
    }

    fn found<T>(value: T) -> Found<T> {
        Found {
            value,
            spot: Spot {
                page: 0,
                bbox: Bbox::new(10, 10, 20, 20).unwrap(),
            },
            rule: RuleId::new("test"),
            confidence: Confidence::HIGH,
        }
    }

    fn reading() -> Reading {
        let mut r = Reading::empty(Source::TextPdf, 1);
        r.header.vendor = Some(found("Midwest Equipment Rental".to_string()));
        r.header.total = Some(found(Money::parse_wire("1480.00").unwrap()));
        r
    }

    fn vendor() -> VendorCandidate {
        VendorCandidate {
            contact: ContactId::parse("00000000-0000-0000-0000-000000000007").unwrap(),
            display_name: "Midwest Equipment Rental".into(),
            company_name: None,
            account_number: None,
            phone: None,
            address: None,
            layouts: Vec::new(),
        }
    }

    fn request() -> ReadRequest {
        ReadRequest {
            bytes: Arc::from(b"%PDF-1.4".to_vec()),
            hint: JobHint::default(),
            today: today(),
            vendors: vec![vendor()],
            templates: Vec::new(),
            card_accounts: None,
        }
    }

    /// Answers every job with the same outcome and counts the calls.
    struct Fixed(JobOutcome, Arc<std::sync::atomic::AtomicU32>);

    impl Reader for Fixed {
        fn read(&mut self, jobs: &[DocJob], _today: CalendarDate) -> Vec<JobOutcome> {
            self.1.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            jobs.iter().map(|_| self.0.clone()).collect()
        }
    }

    fn wait(rx: &Receiver<ReadOutcome>) -> ReadOutcome {
        rx.recv_timeout(Duration::from_secs(10)).expect("an answer")
    }

    #[test]
    fn a_reading_comes_back_with_the_vendor_matched_on_the_desktop() {
        let calls = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let c = Arc::clone(&calls);
        let s = ReadingService::start(Box::new(move || {
            Some(Box::new(Fixed(JobOutcome::Read(Box::new(reading())), c)) as Box<dyn Reader>)
        }));
        let ReadOutcome::Read(local) = wait(&s.submit(request())) else {
            panic!("read");
        };
        assert_eq!(local.vendor.map(|v| v.contact), Some(vendor().contact));
        assert_eq!(local.card, None, "no card accounts without the capability");
        assert_eq!(local.reading.header.total, reading().header.total);
        // The reader is made once and reused.
        let _ = wait(&s.submit(request()));
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    /// Review S4: another vendor's saved layout, this vendor's name printed:
    /// the name wins and the banner asks the owner to check.
    #[test]
    fn a_printed_name_over_a_shared_layout_is_checked() {
        let fp = cl_docread::model::Fingerprint {
            tokens: vec!["invoice".into(), "remit".into()],
            aspect: 12_941,
        };
        let mut other = vendor();
        other.contact = ContactId::parse("00000000-0000-0000-0000-000000000008").unwrap();
        other.display_name = "Hilltop Feed".into();
        other.layouts = vec![fp.clone()];
        let mut r = reading();
        r.fingerprint = Some(fp);
        let mut req = request();
        req.vendors.push(other);
        let local = matched(r, &req);
        assert_eq!(local.vendor.map(|v| v.contact), Some(vendor().contact));
        assert!(local.reading.checks.iter().any(|c| c.code == CheckCode::ReadersDisagree
            && c.rule.as_str() == "vendor.layout"));
    }

    /// Reads until cancelled; counts the reads started.
    struct UntilCancelled(Arc<std::sync::atomic::AtomicU32>);

    impl Reader for UntilCancelled {
        fn read(&mut self, _jobs: &[DocJob], _today: CalendarDate) -> Vec<JobOutcome> {
            unreachable!("read through read_cancellable")
        }

        fn read_cancellable(
            &mut self,
            jobs: &[DocJob],
            _today: CalendarDate,
            cancel: &Arc<AtomicBool>,
        ) -> Vec<JobOutcome> {
            self.0.fetch_add(1, Ordering::SeqCst);
            while !cancel.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(5));
            }
            jobs.iter()
                .map(|_| JobOutcome::CouldNotRead(CouldNotRead::Cancelled))
                .collect()
        }
    }

    /// Review S2: Cancel stops a running reading and a queued one is never
    /// read; both answer Cancelled.
    #[test]
    fn cancel_stops_the_running_and_the_queued_reading() {
        let started = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let n = Arc::clone(&started);
        let s = ReadingService::start(Box::new(move || {
            Some(Box::new(UntilCancelled(n)) as Box<dyn Reader>)
        }));
        let running = s.submit_cancellable(request());
        let queued = s.submit_cancellable(request());
        while started.load(Ordering::SeqCst) == 0 {
            std::thread::sleep(Duration::from_millis(5));
        }
        queued.cancel();
        running.cancel();
        assert_eq!(
            wait(&running.answer),
            ReadOutcome::Failed(ReadFailure::Cancelled)
        );
        assert_eq!(
            wait(&queued.answer),
            ReadOutcome::Failed(ReadFailure::Cancelled)
        );
        assert_eq!(
            started.load(Ordering::SeqCst),
            1,
            "the queued one never started"
        );
        assert_eq!(ReadFailure::Cancelled.class(), "cancelled");
    }

    #[test]
    fn failures_map_to_plain_classes() {
        for (job, want) in [
            (
                JobOutcome::Failed(JobFailure::Protected),
                ReadFailure::Protected,
            ),
            (
                JobOutcome::Failed(JobFailure::Empty),
                ReadFailure::Unreadable,
            ),
            (
                JobOutcome::Failed(JobFailure::NeedsOcr),
                ReadFailure::NeedsOcr,
            ),
            (
                JobOutcome::CouldNotRead(CouldNotRead::Crashed),
                ReadFailure::GaveUp,
            ),
            (
                JobOutcome::CouldNotRead(CouldNotRead::Unavailable),
                ReadFailure::Unavailable,
            ),
        ] {
            let calls = Arc::new(std::sync::atomic::AtomicU32::new(0));
            let mut r = Fixed(job, calls);
            assert_eq!(
                read_one(&mut r, &request(), &Arc::default()),
                ReadOutcome::Failed(want),
                "{want:?}"
            );
        }
    }

    #[test]
    fn no_reader_answers_unavailable() {
        let s = ReadingService::start(Box::new(|| None));
        assert_eq!(
            wait(&s.submit(request())),
            ReadOutcome::Failed(ReadFailure::Unavailable)
        );
    }

    #[test]
    fn card_digits_pick_one_account_or_ask() {
        let mut r = reading();
        r.header.payment = Some(found(Payment::Card {
            last4: Last4::parse("4821").unwrap(),
        }));
        let card = AccountId::parse("00000000-0000-0000-0000-0000000000c1").unwrap();
        let mut req = request();
        req.card_accounts = Some(vec![(card, Last4::parse("4821").unwrap())]);
        let one = matched(r.clone(), &req);
        assert_eq!(one.card, Some(CardPick::One(card)));
        assert_eq!(one.reading.checks.len(), 0);
        req.card_accounts = Some(Vec::new());
        let none = matched(r.clone(), &req);
        assert_eq!(none.card, Some(CardPick::None));
        assert_eq!(none.reading.checks[0].code, CheckCode::PickTheCard);
        // Without the capability nothing is matched and nothing added.
        req.card_accounts = None;
        let off = matched(r, &req);
        assert_eq!(off.card, None);
        assert_eq!(off.reading.checks.len(), 0);
    }

    #[test]
    fn debug_shows_no_document_data() {
        let local = matched(reading(), &request());
        let text = format!("{local:?} {:?}", request());
        assert!(
            !text.contains("Midwest") && !text.contains("1480"),
            "{text}"
        );
    }

    // ---- Step 19a ----

    /// Records how many layouts the job carried.
    struct Seen(Arc<std::sync::Mutex<usize>>);
    impl Reader for Seen {
        fn read(&mut self, jobs: &[DocJob], _today: CalendarDate) -> Vec<JobOutcome> {
            *self.0.lock().unwrap() = jobs[0].templates.len();
            vec![JobOutcome::Read(Box::new(reading()))]
        }
    }

    fn layout() -> Template {
        use cl_docread::model::{FieldName, Fingerprint, TemplateField};
        Template {
            fingerprint: Fingerprint {
                tokens: vec!["invoice".into(), "midwest".into(), "#".into()],
                aspect: 12_941,
            },
            fields: vec![TemplateField {
                field: FieldName::Total,
                page: 0,
                region: Bbox::new(7000, 8000, 9000, 8300).unwrap(),
                label: Some("total".into()),
            }],
            line_table: None,
            samples: 3,
        }
    }

    fn saved(vendor: ContactId, t: &Template) -> cl_api::VendorTemplate {
        let up = template_upsert(vendor, t, None).expect("upsert");
        serde_json::from_value(serde_json::json!({
            "id": "00000000-0000-4000-8000-0000000000b1",
            "contactId": vendor,
            "fingerprint": up.fingerprint,
            "fieldRegions": up.field_regions,
            "lineTable": null,
            "samples": 4,
            "updatedAt": "2026-11-04T10:00:00.000Z",
        }))
        .expect("engine row")
    }

    #[test]
    fn saved_layouts_round_trip_and_reach_the_vendor_match() {
        let v = vendor().contact;
        let t = layout();
        let up = template_upsert(v, &t, None).expect("upsert");
        assert!(up.fingerprint.starts_with("l1:"));
        assert_eq!(
            up.fingerprint,
            template_upsert(v, &t, None).unwrap().fingerprint,
            "the same layout gets the same key"
        );
        let rows = vec![
            saved(v, &t),
            // A row this version can't read is left out.
            serde_json::from_value(serde_json::json!({
                "id": "00000000-0000-4000-8000-0000000000b2",
                "contactId": v, "fingerprint": "l9:unknown-layout",
                "fieldRegions": {"v": 9}, "samples": 1,
                "updatedAt": "2026-11-04T10:00:00.000Z",
            }))
            .unwrap(),
        ];
        let layouts = templates_of(&rows);
        assert_eq!(layouts.len(), 1);
        assert_eq!(layouts[0].0, v);
        assert_eq!(layouts[0].1.fields, t.fields);
        assert_eq!(layouts[0].1.samples, 4, "the engine's sample count");
        let (vendors, templates) = with_layouts(vec![vendor()], &layouts);
        assert_eq!(vendors[0].layouts, vec![t.fingerprint.clone()]);
        assert_eq!(templates.len(), 1);
        // The layouts travel to the reader with the job.
        let mut req = request();
        req.vendors = vendors;
        req.templates = templates;
        let n = Arc::new(std::sync::Mutex::new(0));
        let _ = read_one(&mut Seen(Arc::clone(&n)), &req, &Arc::default());
        assert_eq!(*n.lock().unwrap(), 1);
    }

    fn account(id: &str, detail: &str, digits: Option<&str>, active: bool) -> cl_api::Account {
        serde_json::from_value(serde_json::json!({
            "id": id, "name": "Card", "accountType": "liability", "detailType": detail,
            "accountNumber": null, "description": null, "parentId": null,
            "isActive": active, "isSystem": false, "systemTag": null, "balance": "0.0000",
            "cardLast4": digits,
        }))
        .unwrap()
    }

    #[test]
    fn card_accounts_are_active_cards_with_digits() {
        let one = "00000000-0000-0000-0000-0000000000c1";
        let accounts = vec![
            account(one, "credit_card", Some("4821"), true),
            account(
                "00000000-0000-0000-0000-0000000000c2",
                "credit_card",
                None,
                true,
            ),
            account(
                "00000000-0000-0000-0000-0000000000c3",
                "credit_card",
                Some("4821"),
                false,
            ),
            account(
                "00000000-0000-0000-0000-0000000000c4",
                "bank",
                Some("1111"),
                true,
            ),
        ];
        let cards = card_accounts_of(&accounts);
        assert_eq!(
            cards,
            vec![(
                AccountId::parse(one).unwrap(),
                Last4::parse("4821").unwrap()
            )]
        );
        // The pick by the printed digits.
        let mut r = reading();
        r.header.payment = Some(found(Payment::Card {
            last4: Last4::parse("4821").unwrap(),
        }));
        let mut req = request();
        req.card_accounts = Some(cards);
        assert_eq!(
            matched(r, &req).card,
            Some(CardPick::One(AccountId::parse(one).unwrap()))
        );
    }

    #[test]
    fn a_reading_is_stored_as_strings_and_integers() {
        let mut r = reading();
        r.kind = DocKind::Bill;
        r.header.reference = Some(found("INV-55871".to_string()));
        r.header.date = Some(found(today()));
        let local = matched(r, &request());
        let d = draft_reading(&local).expect("draft reading");
        assert_eq!(d.kind, cl_api::DraftKind::Bill);
        assert_eq!(d.reader, cl_api::DraftReader::TextPdf);
        assert_eq!(d.suggested_contact, Some(vendor().contact));
        assert_eq!(d.doc_reference.as_deref(), Some("INV-55871"));
        assert_eq!(d.doc_total, Some(Money::parse_wire("1480.00").unwrap()));
        assert_eq!(d.doc_date, Some(today()));
        assert_eq!(
            d.fields["total"]["value"], "1480.0000",
            "amounts are strings"
        );
        assert!(d.fields["total"]["confidence"].is_u64(), "basis points");
    }

    /// A session on `server` that learned `caps` from `/desktop/info`.
    async fn caps_session(server: &wiremock::MockServer, caps: &[&str]) -> cl_api::Session {
        use wiremock::matchers::{method, path};
        wiremock::Mock::given(method("GET"))
            .and(path("/api/v1/desktop/info"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(
                serde_json::json!({"mode": "desktop", "initialized": true, "capabilities": caps}),
            ))
            .mount(server)
            .await;
        let s = cl_api::testing::stub_session(server.address().port());
        s.client().desktop_info().await.expect("desktop info");
        s
    }

    #[tokio::test]
    async fn claim_and_store_claims_then_stores_the_reading_or_the_failure() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        const DRAFT: &str = "00000000-0000-4000-8000-0000000000d1";
        let draft = |status: &str| {
            serde_json::json!({"draft": {"id": DRAFT, "kind": "unknown", "status": status,
                "contentSha256": "ab", "updatedAt": "2026-11-04T10:00:00.000Z"}})
        };
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/document-drafts"))
            .respond_with(ResponseTemplate::new(201).set_body_json(draft("new")))
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path(format!("/api/v1/document-drafts/{DRAFT}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(draft("ready")))
            .mount(&server)
            .await;
        let s = caps_session(&server, &[cl_api::Capabilities::DOCUMENT_DRAFTS]).await;
        let file = cl_api::AttachmentId::parse("00000000-0000-4000-8000-0000000000f1").unwrap();
        let read = ReadOutcome::Read(Box::new(matched(reading(), &request())));
        let d = claim_and_store(
            &s,
            file,
            cl_api::DraftKind::Bill,
            &read,
            cl_api::IdempotencyKey::new(),
        )
        .await
        .expect("stored");
        assert_eq!(d.status, cl_api::DraftStatus::Ready);
        claim_and_store(
            &s,
            file,
            cl_api::DraftKind::Bill,
            &ReadOutcome::Failed(ReadFailure::Protected),
            cl_api::IdempotencyKey::new(),
        )
        .await
        .expect("failure stored");
        let puts: Vec<serde_json::Value> = server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|r| r.method.as_str() == "PUT")
            .map(|r| serde_json::from_slice(&r.body).unwrap())
            .collect();
        assert_eq!(puts[0]["status"], "ready");
        assert_eq!(puts[0]["expectedUpdatedAt"], "2026-11-04T10:00:00.000Z");
        assert_eq!(puts[0]["docTotal"], "1480.0000");
        assert_eq!(puts[1]["status"], "failed");
        assert_eq!(puts[1]["failureReason"], "protected");
    }

    #[tokio::test]
    async fn without_the_capability_the_context_is_rules_only() {
        let server = wiremock::MockServer::start().await;
        let s = caps_session(&server, &[]).await;
        assert_eq!(load_context(&s).await, Ok(ReadContext::default()));
        let sent = server.received_requests().await.unwrap();
        assert!(
            sent.iter().all(|r| r.url.path() == "/api/v1/desktop/info"),
            "nothing else is fetched"
        );
    }

    // ---- A04 WP-C ----

    fn region() -> RegionRequest {
        RegionRequest {
            bytes: Arc::from(b"%PDF-1.4".to_vec()),
            area: PageBox::new(0, 1000, 1000, 3000, 2000).unwrap(),
            today: today(),
        }
    }

    fn wait_region(job: &RegionJob) -> RegionOutcome {
        job.answer
            .recv_timeout(Duration::from_secs(10))
            .expect("an answer")
    }

    /// Reads boxes: one word at the box's corner; counts the boxes read.
    struct Boxes(Arc<std::sync::atomic::AtomicU32>);

    impl Reader for Boxes {
        fn read(&mut self, jobs: &[DocJob], _today: CalendarDate) -> Vec<JobOutcome> {
            jobs.iter()
                .map(|_| JobOutcome::Read(Box::new(reading())))
                .collect()
        }

        fn read_region(&mut self, request: &RegionRequest, _c: &Arc<AtomicBool>) -> RegionOutcome {
            self.0.fetch_add(1, Ordering::SeqCst);
            let a = request.area;
            RegionOutcome::Words(vec![Word {
                text: "12.50".into(),
                bbox: Bbox::new(a.l, a.t, a.l + 100, a.t + 100).unwrap(),
                confidence: Confidence::HIGH,
            }])
        }
    }

    #[test]
    fn a_reader_without_boxes_says_so_and_no_reader_is_unavailable() {
        let calls = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let c = Arc::clone(&calls);
        let s = ReadingService::start(Box::new(move || {
            Some(Box::new(Fixed(JobOutcome::Read(Box::new(reading())), c)) as Box<dyn Reader>)
        }));
        assert_eq!(
            wait_region(&s.submit_region(region())),
            RegionOutcome::NotSupported
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "no document read for a box"
        );
        let none = ReadingService::start(Box::new(|| None));
        assert_eq!(
            wait_region(&none.submit_region(region())),
            RegionOutcome::Failed(ReadFailure::Unavailable)
        );
        assert_eq!(RegionOutcome::NotSupported.class(), "not_supported");
    }

    #[test]
    fn a_box_is_read_on_the_service_thread() {
        let boxes = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let b = Arc::clone(&boxes);
        let s = ReadingService::start(Box::new(
            move || Some(Box::new(Boxes(b)) as Box<dyn Reader>),
        ));
        let RegionOutcome::Words(words) = wait_region(&s.submit_region(region())) else {
            panic!("words");
        };
        assert_eq!(words.len(), 1);
        assert_eq!(words[0].bbox.left(), 1000);
        // A read and a box share the one reader, in the order sent.
        assert!(matches!(wait(&s.submit(request())), ReadOutcome::Read(_)));
        assert_eq!(boxes.load(Ordering::SeqCst), 1);
        let debug = format!("{:?}", region());
        assert!(!debug.contains("PDF"), "{debug}");
    }

    #[test]
    fn a_box_cancelled_while_queued_is_never_read() {
        let started = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let n = Arc::clone(&started);
        let s = ReadingService::start(Box::new(move || {
            Some(Box::new(UntilCancelled(n)) as Box<dyn Reader>)
        }));
        let running = s.submit_cancellable(request());
        let queued = s.submit_region(region());
        while started.load(Ordering::SeqCst) == 0 {
            std::thread::sleep(Duration::from_millis(5));
        }
        queued.cancel();
        running.cancel();
        assert_eq!(
            wait_region(&queued),
            RegionOutcome::Failed(ReadFailure::Cancelled)
        );
    }

    #[test]
    fn the_word_cache_keeps_the_twenty_most_recent_drafts() {
        let local = Arc::new(LocalReading {
            reading: reading(),
            vendor: None,
            card: None,
        });
        let mut cache = WordCache::new();
        assert!(cache.is_empty());
        for i in 0..25 {
            cache.put(&format!("d{i}"), Arc::clone(&local));
        }
        assert_eq!(cache.len(), cl_model::doc_draft::WORD_CACHE_DRAFTS);
        assert!(cache.get("d0").is_none(), "the oldest went first");
        // Using d5 keeps it past the next put.
        assert!(cache.get("d5").is_some());
        cache.put("new", Arc::clone(&local));
        assert!(cache.get("d5").is_some());
        assert!(cache.get("d6").is_none());
        cache.put("new", Arc::clone(&local));
        assert_eq!(
            cache.len(),
            cl_model::doc_draft::WORD_CACHE_DRAFTS,
            "no duplicate"
        );
        cache.forget("new");
        assert!(cache.get("new").is_none());
        assert_eq!(format!("{cache:?}"), "WordCache { drafts: 19 }");
        assert_eq!(doc_words(&local).pages, local.reading.words);
    }
}
