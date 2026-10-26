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
//! Logs carry outcome classes only: never a file name, text or an amount.

use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};

use cl_core::CalendarDate;
use cl_core::ids::AccountId;
use cl_docread::card::{self, CardPick};
use cl_docread::model::{Last4, Payment, Reading, Template};
use cl_docread::protocol::{JobFailure, JobHint};
use cl_docread::vendor_match::{self, VendorCandidate, VendorMatch};
use cl_host::{CouldNotRead, DocJob, DocreadConfig, DocreadHost, JobOutcome};

/// Something that reads documents: the worker host, or a stand-in in
/// tests. Blocking.
pub trait Reader: Send {
    /// One outcome per job, in order.
    fn read(&mut self, jobs: &[DocJob], today: CalendarDate) -> Vec<JobOutcome>;
}

impl Reader for DocreadHost {
    fn read(&mut self, jobs: &[DocJob], today: CalendarDate) -> Vec<JobOutcome> {
        DocreadHost::read(self, jobs, today)
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
            | CouldNotRead::BadReply
            | CouldNotRead::Cancelled => ReadFailure::GaveUp,
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

struct Message {
    request: ReadRequest,
    reply: Sender<ReadOutcome>,
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
        let (reply, answer) = mpsc::channel();
        if let Err(mpsc::SendError(m)) = self.tx.send(Message { request, reply }) {
            let _ = m.reply.send(ReadOutcome::Failed(ReadFailure::Unavailable));
        }
        answer
    }
}

fn serve(rx: &Receiver<Message>, make: MakeReader) {
    let mut make = Some(make);
    let mut reader: Option<Box<dyn Reader>> = None;
    while let Ok(Message { request, reply }) = rx.recv() {
        if reader.is_none()
            && let Some(m) = make.take()
        {
            reader = m();
        }
        let outcome = match reader.as_mut() {
            Some(r) => read_one(r.as_mut(), &request),
            None => ReadOutcome::Failed(ReadFailure::Unavailable),
        };
        tracing::info!(outcome = outcome.class(), "document read");
        // The window may have closed meanwhile: nobody waits then.
        let _ = reply.send(outcome);
    }
}

/// Reads one document and adds the desktop's matches.
pub(crate) fn read_one(reader: &mut dyn Reader, request: &ReadRequest) -> ReadOutcome {
    let job = DocJob {
        bytes: request.bytes.to_vec(),
        hint: request.hint,
        templates: request.templates.clone(),
    };
    let outcome = reader
        .read(std::slice::from_ref(&job), request.today)
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
                read_one(&mut r, &request()),
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
}
