//! A draft in its real form (A04 D2, D3, D5): opening a document draft in
//! Enter Bill, Record Card Charge (also as "Expense paid from") or Write
//! Check in *draft mode*, Accept through the form's own Save & Close / Save
//! & New sent to `convert`, every convert answer, attaching to a record
//! already in the books, Page Up/Down through the queue, and learning after
//! a convert. The forms own their fields; [`DraftSession`] owns the draft.
//!
//! # The posting path (non-negotiables 4, 6, 7, 8)
//!
//! - The form checks its values with its own exact gate (`check()`: the
//!   lines equal the total to the cent; read values must be accepted) and
//!   builds its own engine draft (`BillDraft`, `CardChargeDraft`,
//!   `CheckDraft`): [`PostBody`]. The demo posts that draft; the engine
//!   gets the same draft through `BankPosting::{card_charge_with,
//!   check_with}` (`ConvertBody`). Nothing is computed here.
//! - **Keys** ([`DraftSession`]): one idempotency key per (body, the
//!   draft's change token, acknowledged records). The closing-date password
//!   is not part of it (a 423 resent with the password reuses the key); an
//!   unknown outcome or a busy key resends the **identical** request; a
//!   changed body, a new token ("Keep my values and enter") or new
//!   acknowledgements get a new key; `IDEMPOTENCY_KEY_REUSED` reads the
//!   draft and either reports it entered or sends again under a new key.
//! - Every convert answer of the plan's D2 table: 201 (the inbox's entry
//!   is handled and its original moves, then learning), unknown or busy
//!   (resend identical), `SavedUnreadable` (read the draft for the record),
//!   423 (the form's closing-date password flow), 409 duplicate (the attach
//!   dialog, [`super::attach`]), 409 closed with or without a record, 409
//!   stale (keep my values or reload), key reused, 403, validation (the
//!   form focuses the field).
//! - Before the convert, on **every** Accept, the register is searched for
//!   a record that is already this document (Q-282): the owner may attach
//!   instead ([`super::attach`]); Enter never attaches.
//!
//! # The control channel
//!
//! `state.review.form_digest` and `state.review.posted_digest` (plan §4
//! N4): at Accept the form's checked values and the request body as put
//! on the wire are each written as the same canonical text ([`Canon`],
//! two independent readings) and hashed with [`digest`]; a
//! scenario checks they are equal (AC1: what the owner reviewed is what
//! posted). The digest is an HMAC-SHA-256 under a random key made once per
//! process (security review L6): a plain hash of a few values could be
//! reversed by trying likely amounts; the keyed one can only be compared
//! with another digest from the same run. The forms' `state().draft`
//! reports [`DraftSession::state_json`]: states, flags and counts only.

#![allow(
    clippy::single_match_else,
    clippy::manual_let_else,
    reason = "each engine answer reads as its own arm"
)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use cl_api::{
    ApiError, AttachmentId, BankPosting, BillDraft, Capabilities, CardChargeDraft, CheckDraft,
    ClosedPeriodAuthorization, ConflictReason, ConvertBody, ConvertedDraft, DocumentDraft,
    DocumentDraftId, DraftConversion, DraftKind, DraftListQuery, DraftReader, DraftRefusal,
    DraftStatus, IdempotencyKey, LocalValidation, OrphanLabel, PostingOptions, RequestGroup,
};
use cl_core::ids::{AccountId, ContactId, TenantId, TransactionId};
use cl_core::{CalendarDate, MoneyStyle, Secret};
use cl_docread::card::card_account;
use cl_docread::model::{Bbox, Confidence as Bp, Last4, Payment, Reader, Reading, Spot};
use cl_docread::vendor_match::{VendorMatch, VendorTier};
use cl_model::CommandId;
use cl_model::doc_draft::{
    CompareTarget, DocKind, DraftForm, FillKind, PageBox, Step, TargetKey, TargetLevel,
};
use cl_model::forms::SaveIntent;
use cl_model::forms::doc_read::{ReadField, ReadState as ReadMarks, ReadTarget};
use eframe::egui::{self, RichText};
use hmac::{Hmac, KeyInit as _, Mac as _};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::Sha256;

use super::attach::{AttachDialog, DialogAnswer, MatchProbe};
use crate::company::demo::drafts::{self as demo_drafts, DemoConversion, DemoConvertBody};
use crate::company::{CompanyEvent, CompanyLink, Task};
use crate::i18n::{t, t_with};
use crate::reading::learn::{self, CardOutcome, LearnInput, Learned};
use crate::reading::service::{WordCache, doc_words};
use crate::reading::{LocalReading, ReadJob, ReadOutcome, ReadRequest};
use crate::theme::Tokens;
use crate::widgets::compare::{CompareAction, CompareEnv, CompareEvent, CompareTools};
use crate::widgets::pdf_viewer::DocSource;
use crate::windows::OpenPayload;

#[cfg(test)]
#[path = "review_tests.rs"]
mod tests;

#[path = "review_canon.rs"]
mod canon;
pub(crate) use canon::Canon;

#[path = "review_kind_ai.rs"]
mod kind_ai;
pub(crate) use kind_ai::{Carry, DraftAssist, FormValues, publish_carry};

/// A file larger than this is not downloaded for the form (the engine's
/// own attachment limit).
const MAX_FILE_BYTES: usize = 10 * 1024 * 1024;
/// An unknown outcome is resent (identical, same key) this many times
/// before the owner is asked.
const UNKNOWN_RESENDS: u8 = 3;
/// A busy key is resent at most this many times.
const BUSY_RESENDS: u8 = 10;
/// Wait before resending.
#[cfg(not(test))]
const RESEND_AFTER: Duration = Duration::from_secs(1);
#[cfg(test)]
const RESEND_AFTER: Duration = Duration::ZERO;

// ======================================================= digests (frozen)

/// The last Accept's digests (`state.review`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ReviewDigests {
    /// [`digest`] of the form's canonical values at Accept.
    pub form_digest: Option<String>,
    /// [`digest`] of the canonical convert body sent.
    pub posted_digest: Option<String>,
    /// After an attach on the demo engine: counts only (S3: the books
    /// gained no record and the file is on the record picked).
    pub attached: Option<AttachProof>,
}

/// What an attach changed in the demo's books (counts only).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct AttachProof {
    /// Records in the books before the attach.
    pub records_before: usize,
    /// Records after it.
    pub records_after: usize,
    /// Files now filed on the record attached to.
    pub files_on_record: usize,
}

impl ReviewDigests {
    /// The control channel's JSON.
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        let mut v = serde_json::to_value(self).unwrap_or(serde_json::Value::Null);
        // Scenarios compare the two here (AC1): equal and present.
        if let serde_json::Value::Object(m) = &mut v {
            m.insert(
                "match".into(),
                (self.form_digest.is_some() && self.form_digest == self.posted_digest).into(),
            );
        }
        v
    }
}

/// This process's digest key: 32 random bytes from the operating system,
/// made on first use and never stored, logged or shown.
fn key() -> &'static [u8; 32] {
    static KEY: OnceLock<[u8; 32]> = OnceLock::new();
    KEY.get_or_init(|| {
        let mut key = [0u8; 32];
        if getrandom::fill(&mut key).is_err() {
            key = fallback_key();
        }
        key
    })
}

/// When the operating system gives no random bytes: the standard library's
/// per-process random hash seeds (weaker, still unknown outside this run).
fn fallback_key() -> [u8; 32] {
    use std::hash::{BuildHasher as _, Hasher as _};
    let mut key = [0u8; 32];
    for (i, chunk) in key.chunks_mut(8).enumerate() {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_usize(i);
        chunk.copy_from_slice(&h.finish().to_le_bytes());
    }
    key
}

/// HMAC-SHA-256 of `canonical` under this process's key, lowercase hex:
/// equal inputs give equal digests within one run of the app only.
#[must_use]
pub fn digest(canonical: &[u8]) -> String {
    use std::fmt::Write as _;
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(key()) else {
        // A 32-byte key is always accepted (HMAC takes any length).
        return String::new();
    };
    mac.update(canonical);
    mac.finalize()
        .into_bytes()
        .iter()
        .fold(String::with_capacity(64), |mut out, b| {
            let _ = write!(out, "{b:02x}");
            out
        })
}

fn digests_id() -> egui::Id {
    egui::Id::unique("a04/review/digests")
}

/// Records the last Accept's digests (the draft form, at Accept).
pub fn publish_digests(ctx: &egui::Context, digests: ReviewDigests) {
    ctx.data_mut(|d| d.insert_temp(digests_id(), digests));
}

/// The last Accept's digests; both `None` before any.
#[must_use]
pub fn digests(ctx: &egui::Context) -> ReviewDigests {
    ctx.data(|d| d.get_temp::<ReviewDigests>(digests_id()))
        .unwrap_or_default()
}

// ============================================================ post body

/// What a draft is posted as: the form's own engine draft, built by its
/// own `to_draft` / builder from the checked values. The demo posts it as
/// is; the engine gets it through cl-api's checked builders.
#[derive(Clone, PartialEq)]
pub(crate) enum PostBody {
    /// Enter Bill.
    Bill(BillDraft),
    /// Record Card Charge.
    CardCharge(CardChargeDraft),
    /// Record Card Charge as "Expense paid from" (bank or cash).
    CashExpense(CardChargeDraft),
    /// Write Check.
    Check(CheckDraft),
}

impl std::fmt::Debug for PostBody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PostBody({})", self.kind().as_str())
    }
}

impl PostBody {
    /// The kind it converts as.
    #[must_use]
    pub(crate) const fn kind(&self) -> DraftKind {
        match self {
            PostBody::Bill(_) => DraftKind::Bill,
            PostBody::CardCharge(_) => DraftKind::CardCharge,
            PostBody::CashExpense(_) => DraftKind::CashExpense,
            PostBody::Check(_) => DraftKind::Check,
        }
    }

    fn demo(&self) -> DemoConvertBody {
        match self {
            PostBody::Bill(d) => DemoConvertBody::Bill(d.clone()),
            PostBody::CardCharge(d) => DemoConvertBody::CardCharge(d.clone()),
            PostBody::CashExpense(d) => DemoConvertBody::CashExpense(d.clone()),
            PostBody::Check(d) => DemoConvertBody::Check(d.clone()),
        }
    }

    /// cl-api's convert body (its own checks run here: nothing is sent
    /// when they fail).
    fn live(&self, billable: bool) -> Result<ConvertBody, ApiError> {
        Ok(match self {
            PostBody::Bill(d) => ConvertBody::Bill(d.clone()),
            PostBody::CardCharge(d) => ConvertBody::CardCharge(
                BankPosting::card_charge_with(d.clone(), billable).map_err(ApiError::Local)?,
            ),
            PostBody::CashExpense(d) => ConvertBody::CashExpense(
                BankPosting::card_charge_with(d.clone(), billable).map_err(ApiError::Local)?,
            ),
            PostBody::Check(d) => ConvertBody::Check(
                BankPosting::check_with(d.clone(), billable).map_err(ApiError::Local)?,
            ),
        })
    }
}

/// The posted side of AC1: [`Canon::of_wire`]'s digest of the `body` that
/// `live` puts on the wire under `caps` (cl-api's own
/// [`ConvertBody::wire_body`], the function `convert` sends); `None` when
/// it can't be built (the digests then differ).
fn posted_digest(kind: DraftKind, live: &ConvertBody, caps: &Capabilities) -> Option<String> {
    let v = live.wire_body(caps).ok()?;
    Canon::of_wire(kind, &v).map(|c| c.digest())
}

/// The capabilities a body is put on the wire with: the engine's, or what
/// the demo engine takes for bills (lines, terms, received).
fn wire_caps(link: &CompanyLink) -> Capabilities {
    let Some(demo) = link.demo_state() else {
        return link.capabilities();
    };
    let b = crate::company::demo::bills::caps(&demo);
    drop(demo);
    let names: Vec<&str> = [
        (b.lines, Capabilities::BILL_LINES),
        (b.terms, Capabilities::BILL_TERMS),
        (b.received, Capabilities::BILL_RECEIVED),
    ]
    .into_iter()
    .filter_map(|(on, name)| on.then_some(name))
    .collect();
    serde_json::from_value(json!(names)).unwrap_or_default()
}

// ============================================================ open hook

/// What a form opens in draft mode (`OpenPayload::DocumentDraft`). Ids
/// only; `Debug` is safe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DraftOpen {
    /// The draft.
    pub id: DocumentDraftId,
    /// The form it opens in.
    pub form: DraftForm,
}

/// The model's kind of an engine draft kind.
#[must_use]
pub fn doc_kind(k: DraftKind) -> DocKind {
    match k {
        DraftKind::Bill => DocKind::Bill,
        DraftKind::CardCharge => DocKind::CardCharge,
        DraftKind::Check => DocKind::Check,
        DraftKind::CashExpense => DocKind::CashExpense,
        DraftKind::Unknown => DocKind::Unknown,
    }
}

/// **The open hook** (WP-F1's queue Enter, the inbox, Home): opens draft
/// `open.id` in its form's window, in draft mode. A draft of an unknown
/// kind has no form: show [`kind_chooser`] first (Q-A04n).
pub fn open(link: &CompanyLink, open: DraftOpen) {
    link.push(CompanyEvent::OpenWith(
        CommandId::new(open.form.command()),
        Box::new(OpenPayload::DocumentDraft(open)),
    ));
}

/// "What is this document?" (Q-A04n): the four forms, no default. The
/// form picked, once.
pub fn kind_chooser(ui: &mut egui::Ui) -> Option<DraftForm> {
    let mut picked = None;
    ui.label(t("doc_form.chooser.title"));
    ui.horizontal_wrapped(|ui| {
        for f in DraftForm::ALL {
            if ui.button(t(form_key(f))).clicked() {
                picked = Some(f);
            }
        }
    });
    picked
}

/// The words for a form ("Enter Bill", "Expense paid from…").
#[must_use]
pub fn form_key(f: DraftForm) -> &'static str {
    match f {
        DraftForm::EnterBill => "doc_form.as.bill",
        DraftForm::CardCharge => "doc_form.as.card_charge",
        DraftForm::CashExpense => "doc_form.as.cash_expense",
        DraftForm::WriteCheck => "doc_form.as.check",
    }
}

fn queue_id() -> egui::Id {
    egui::Id::unique("a04/review/queue_order")
}

/// The queue's rows in the order shown (WP-F1 publishes it each time its
/// list changes): Page Up/Down and Save & New in a draft form step through
/// it. Ids and kinds only.
pub fn publish_queue(ctx: &egui::Context, rows: Vec<(DocumentDraftId, DraftKind)>) {
    ctx.data_mut(|d| d.insert_temp(queue_id(), Arc::new(rows)));
}

/// The queue order last published (empty before any).
#[must_use]
pub fn queue_order(ctx: &egui::Context) -> Arc<Vec<(DocumentDraftId, DraftKind)>> {
    ctx.data(|d| d.get_temp::<Arc<Vec<(DocumentDraftId, DraftKind)>>>(queue_id()))
        .unwrap_or_default()
}

/// The session's word cache (D4: 20 drafts' page words, never stored).
#[derive(Clone, Default)]
struct SharedWords(Arc<Mutex<WordCache>>);

fn words_id() -> egui::Id {
    egui::Id::unique("a04/review/word_cache")
}

fn word_cache(ctx: &egui::Context) -> Arc<Mutex<WordCache>> {
    ctx.data_mut(|d| {
        d.get_temp_mut_or_default::<SharedWords>(words_id())
            .0
            .clone()
    })
}

// ====================================================== stored reading

/// A draft's stored reading as the forms take it.
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum Stored {
    /// Claimed, not read yet ("Waiting to be read").
    Unread,
    /// The reading failed ("Couldn't read").
    Failed,
    /// The stored values.
    Read(Box<LocalReading>),
    /// A stored reading this version can't read (S11): the form opens
    /// empty with "Couldn't read this document, enter it by hand".
    Unreadable,
}

impl std::fmt::Debug for Stored {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Stored::Unread => "Unread",
            Stored::Failed => "Failed",
            Stored::Read(_) => "Read",
            Stored::Unreadable => "Unreadable",
        })
    }
}

fn model_kind(k: DraftKind) -> cl_docread::model::DocKind {
    use cl_docread::model::DocKind as M;
    match k {
        DraftKind::Bill => M::Bill,
        DraftKind::CardCharge => M::CardCharge,
        DraftKind::Check => M::Check,
        DraftKind::CashExpense => M::CashExpense,
        DraftKind::Unknown => M::Unknown,
    }
}

/// The stored reading of `d` (values only: the words are re-read, S11):
/// the header, lines and checks as stored, the vendor the desktop matched
/// when it was read (`suggested_contact_id`), and the card account whose
/// digits match the printed ones among `cards`.
#[must_use]
pub(crate) fn stored_reading(d: &DocumentDraft, cards: &[(AccountId, Last4)]) -> Stored {
    match d.status {
        DraftStatus::New => return Stored::Unread,
        DraftStatus::Failed => return Stored::Failed,
        DraftStatus::Ready | DraftStatus::Converted | DraftStatus::Discarded => {}
    }
    let reader = match d.reader {
        Some(DraftReader::TextPdf) => Reader::TextPdf,
        Some(DraftReader::Ocr) => Reader::Ocr,
        Some(DraftReader::Template) => Reader::Template,
        _ => return Stored::Unreadable,
    };
    let header = match d.fields.clone().map(serde_json::from_value) {
        Some(Ok(h)) => h,
        None => cl_docread::model::Header::default(),
        Some(Err(_)) => return Stored::Unreadable,
    };
    let lines = match d.lines.clone().map(serde_json::from_value) {
        Some(Ok(l)) => l,
        None => Vec::new(),
        Some(Err(_)) => return Stored::Unreadable,
    };
    let checks = match d.checks.clone().map(serde_json::from_value) {
        Some(Ok(c)) => c,
        None => Vec::new(),
        Some(Err(_)) => return Stored::Unreadable,
    };
    let mut reading = Reading::empty(reader, 0);
    reading.kind = model_kind(d.kind);
    reading.header = header;
    reading.lines = lines;
    reading.checks = checks;
    let vendor = d.suggested_contact_id.map(|contact| VendorMatch {
        contact,
        tier: if reader == Reader::Template {
            VendorTier::PastLayout
        } else {
            VendorTier::Name(1)
        },
        confidence: reading
            .header
            .vendor
            .as_ref()
            .map_or(Bp::MEDIUM, |v| v.confidence),
        layout_of: None,
    });
    let card = match reading.header.payment.as_ref().map(|p| &p.value) {
        Some(Payment::Card { last4 }) if !cards.is_empty() => Some(card_account(*last4, cards)),
        _ => None,
    };
    Stored::Read(Box::new(LocalReading {
        reading,
        vendor,
        card,
    }))
}

/// A reading's spot as a compare box.
#[must_use]
pub(crate) fn spot_box(s: Spot) -> Option<PageBox> {
    let b = s.bbox;
    PageBox::new(s.page, b.left(), b.top(), b.right(), b.bottom())
}

/// A compare box as a reading's spot.
#[must_use]
pub(crate) fn box_spot(b: PageBox) -> Option<Spot> {
    Some(Spot {
        page: b.page,
        bbox: Bbox::new(b.l, b.t, b.r, b.b)?,
    })
}

// ============================================================== session

/// What the form asks the session to post at Accept.
#[derive(Clone)]
pub(crate) struct Accept {
    /// The form's own engine draft of its checked values.
    pub body: PostBody,
    /// The engine takes line customers and billable marks.
    pub billable: bool,
    /// [`Canon::digest`] of the form's checked values.
    pub form_digest: String,
    /// Save & Close or Save & New.
    pub intent: SaveIntent,
    /// The closing-date password (never part of the key's identity).
    pub password: Option<Secret>,
    /// The register search's probe (the form's values).
    pub probe: MatchProbe,
    /// What learning needs after a 201.
    pub learn: Option<LearnInput>,
}

impl std::fmt::Debug for Accept {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Accept")
            .field("body", &self.body)
            .field("intent", &self.intent)
            .field("password", &self.password.is_some())
            .finish_non_exhaustive()
    }
}

/// What a key belongs to (never the password).
#[derive(Clone, PartialEq)]
struct Identity {
    body: PostBody,
    expected: String,
    ack: Vec<TransactionId>,
}

/// Why the draft is read again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Why {
    /// The convert committed but its answer couldn't be read.
    Unreadable,
    /// `IDEMPOTENCY_KEY_REUSED`.
    KeyReused,
    /// "Keep my values and enter": a fresh change token.
    KeepMine,
}

/// What the session is doing.
enum Work {
    Idle,
    /// The register search on Accept.
    Matching {
        accept: Box<Accept>,
        task: Task<Vec<cl_docread::register_match::RegisterRow>>,
    },
    /// The register search failed or couldn't run (review B2): nothing
    /// posts until the owner picks Try again or Enter as new.
    SearchFailed {
        accept: Box<Accept>,
    },
    /// The attach dialog is up.
    Choosing {
        accept: Box<Accept>,
    },
    /// The convert is in flight.
    Converting {
        accept: Box<Accept>,
        ack: Vec<TransactionId>,
        task: Task<ConvertedDraft>,
    },
    /// The same request goes again at `at` (unknown outcome, busy key).
    Resend {
        accept: Box<Accept>,
        ack: Vec<TransactionId>,
        at: Instant,
        unknown: u8,
        busy: u8,
    },
    /// The resends ran out: the owner's Try again sends it once more.
    Unknown {
        accept: Box<Accept>,
        ack: Vec<TransactionId>,
    },
    /// The draft is read again.
    Refetch {
        accept: Box<Accept>,
        ack: Vec<TransactionId>,
        why: Why,
        task: Task<DocumentDraft>,
    },
    /// The attach is in flight.
    Attaching {
        accept: Box<Accept>,
        target: TransactionId,
        task: Task<(DocumentDraft, TransactionId)>,
        tries: u8,
    },
    /// Posted or attached: the draft is closed.
    Done,
}

impl Work {
    fn name(&self) -> &'static str {
        match self {
            Work::Idle => "idle",
            Work::Matching { .. } => "matching",
            Work::SearchFailed { .. } => "search_failed",
            Work::Choosing { .. } => "choosing",
            Work::Converting { .. } => "converting",
            Work::Resend { .. } => "resend",
            Work::Unknown { .. } => "unknown",
            Work::Refetch { .. } => "refetch",
            Work::Attaching { .. } => "attaching",
            Work::Done => "done",
        }
    }
}

/// A question over the form.
#[derive(Clone, Debug)]
pub(crate) enum Dialog {
    /// 409 `STALE`: keep my values and enter, or reload.
    Stale {
        accept: Box<Accept>,
        ack: Vec<TransactionId>,
    },
    /// Converted elsewhere ("Already entered"), with its record if known.
    AlreadyEntered(Option<TransactionId>),
    /// Discarded on another computer: nothing posts.
    DiscardedElsewhere,
    /// **Discard document**: are you sure?
    ConfirmDiscard,
    /// Page Up/Down with unsaved changes: leave them?
    Leave(DraftOpen),
}

impl Dialog {
    fn name(&self) -> &'static str {
        match self {
            Dialog::Stale { .. } => "stale",
            Dialog::AlreadyEntered(_) => "already_entered",
            Dialog::DiscardedElsewhere => "discarded_elsewhere",
            Dialog::ConfirmDiscard => "confirm_discard",
            Dialog::Leave(_) => "leave",
        }
    }
}

/// What the form must do.
#[derive(Debug, Clone)]
pub(crate) enum DraftEvent {
    /// The draft and its file arrived: take [`DraftSession::take_fill`].
    Loaded,
    /// "Read document" on a `new` draft read it: take the fill (not
    /// stored).
    Read,
    /// The document posted as `transaction`: the form is saved; then
    /// `intent` (Save & New asks for the next draft itself).
    Posted {
        transaction: TransactionId,
        intent: SaveIntent,
    },
    /// The document was attached to `transaction` (nothing posted).
    Attached { transaction: TransactionId },
    /// A refusal for the form to show (the closing-date password,
    /// validation with a path, anything else); nothing posted.
    Failed { error: ApiError, intent: SaveIntent },
    /// The owner backed out (the attach dialog's Cancel); nothing posted.
    Cancelled,
    /// "Reload": the draft changed elsewhere; the form fills again from it.
    Reload,
    /// The window should close (discarded, or already entered).
    Close,
    /// Open this draft next (Save & New, Page Up/Down), or close when
    /// there is none.
    Next(Option<DraftOpen>),
    /// Ask AI to help answered: the values not yet accepted give way to
    /// the merged reading ([`DraftSession::take_fill`]); accepted and
    /// changed values stay.
    Helped,
}

/// Learning's state (D5).
#[derive(Default)]
pub(crate) struct LearnState {
    /// Waiting for the words to learn (this session only).
    waiting: Option<LearnInput>,
    layout: Option<Task<()>>,
    card: Option<Task<CardOutcome>>,
    /// What learning did (control channel).
    pub layout_outcome: Option<Learned>,
    /// What remembering the card did.
    pub card_outcome: Option<CardOutcome>,
    /// The intake's reading context must load the layouts again.
    reload_layouts: bool,
}

/// The loaded draft.
struct Loaded {
    draft: DocumentDraft,
    file: Option<DocSource>,
    /// The draft key the file is on (extra files go there, so convert moves
    /// them with it).
    key: Option<cl_api::DraftKey>,
}

/// The draft key of a file still on one.
fn key_of(a: &cl_api::Attachment) -> Option<cl_api::DraftKey> {
    (a.attachable_type == "draft")
        .then(|| cl_api::DraftKey::parse(&a.attachable_id).ok())
        .flatten()
}

/// One draft open in a form (see the module comment).
pub(crate) struct DraftSession {
    link: Arc<CompanyLink>,
    tenant: Option<TenantId>,
    group: Option<RequestGroup>,
    /// The draft.
    pub id: DocumentDraftId,
    /// The form it is open in.
    pub form: DraftForm,
    /// The draft as last read (its `updated_at` guards every change).
    pub draft: Option<DocumentDraft>,
    load: Option<Task<Loaded>>,
    /// Why the load failed (i18n key).
    pub load_problem: Option<&'static str>,
    /// The document (shown locked: its action is Discard document).
    pub file: Option<DocSource>,
    /// The draft key the document is on: extra files go there (convert
    /// moves them with it; no post-save move).
    pub draft_key: Option<cl_api::DraftKey>,
    /// The stored values.
    pub stored: Option<Stored>,
    /// Whether the form took the values.
    fill_taken: bool,
    /// Page words and fingerprint (a re-read in the worker, or the cache).
    words: Option<Arc<LocalReading>>,
    words_job: Option<ReadJob>,
    words_tried: bool,
    /// "Read document" on a `new` draft.
    read_job: Option<ReadJob>,
    read_result: Option<Box<LocalReading>>,
    /// The batch reader skips this draft while the form holds it (S4).
    claim: Option<Arc<()>>,
    work: Work,
    key: Option<(Identity, IdempotencyKey)>,
    /// The last register search was for this body (a resend with the
    /// password does not search again), and what the owner acknowledged.
    matched: Option<(PostBody, Vec<TransactionId>)>,
    /// A question over the form.
    pub dialog: Option<Dialog>,
    /// The attach dialog.
    pub attach: Option<AttachDialog>,
    /// A plain note on the banner (i18n key).
    pub note: Option<&'static str>,
    /// The record it became.
    pub posted: Option<TransactionId>,
    /// The form shows its own "Filled from …" line this frame (Enter
    /// Bill's reading banner): the draft banner leaves its own out, so
    /// there is one (UI check item 3).
    pub hide_source_line: bool,
    /// Discarded on another computer: nothing may post.
    closed_elsewhere: bool,
    /// The compare tools.
    pub compare: CompareTools,
    /// Fields filled by a click: their spot and the value as clicked.
    pub clicked: BTreeMap<ReadTarget, PageBox>,
    /// The read lines' keys, in the reading's order.
    line_rows: Vec<cl_model::rowkey::RowKey>,
    /// Learning.
    pub learn: LearnState,
    /// "Remember card ending 1234" (default on).
    pub remember_card: bool,
    events: Vec<DraftEvent>,
    digests: Option<ReviewDigests>,
    /// The inbox's entry must be handled for this upload (S17).
    handled: Option<AttachmentId>,
    discard: Option<Task<()>>,
    discard_local: bool,
    next: Option<Task<Option<DraftOpen>>>,
    queue: Arc<Vec<(DocumentDraftId, DraftKind)>>,
    accepts: u32,
    /// Resends so far of the request in flight (unknown, busy).
    resends: (u8, u8),
    /// The attach's key: (target, change token, key).
    attach_key: Option<(TransactionId, String, IdempotencyKey)>,
    /// Ask AI to help (card and check forms).
    pub assist: DraftAssist,
    /// What "Enter this as" brought from the other form (taken on the
    /// first frame, applied after the fill).
    carry: Option<Carry>,
    /// The first frame looked for a carry.
    carry_looked: bool,
    /// The form opened with a carry (control channel).
    carried: Option<usize>,
}

impl std::fmt::Debug for DraftSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DraftSession")
            .field("form", &self.form)
            .field("work", &self.work.name())
            .field("stored", &self.stored)
            .finish_non_exhaustive()
    }
}

/// The convert's options: the password at the top level only.
fn options(password: Option<Secret>) -> PostingOptions {
    PostingOptions {
        closed_period: password.map(ClosedPeriodAuthorization::with_password),
    }
}

impl DraftSession {
    /// Opens draft `open` for a form (the load starts now; the demo
    /// answers at once).
    pub(crate) fn new(
        link: Arc<CompanyLink>,
        tenant: Option<TenantId>,
        open: DraftOpen,
    ) -> DraftSession {
        let group = link.group().and_then(Result::ok);
        let mut s = DraftSession {
            link,
            tenant,
            group,
            id: open.id,
            form: open.form,
            draft: None,
            load: None,
            load_problem: None,
            file: None,
            draft_key: None,
            stored: None,
            fill_taken: false,
            words: None,
            words_job: None,
            words_tried: false,
            read_job: None,
            read_result: None,
            claim: None,
            work: Work::Idle,
            key: None,
            matched: None,
            dialog: None,
            attach: None,
            note: None,
            posted: None,
            hide_source_line: false,
            closed_elsewhere: false,
            compare: CompareTools::new(),
            clicked: BTreeMap::new(),
            line_rows: Vec::new(),
            learn: LearnState::default(),
            remember_card: true,
            events: Vec::new(),
            digests: None,
            handled: None,
            discard: None,
            discard_local: false,
            next: None,
            queue: Arc::default(),
            accepts: 0,
            resends: (0, 0),
            attach_key: None,
            assist: DraftAssist::default(),
            carry: None,
            carry_looked: false,
            carried: None,
        };
        s.start_load();
        s
    }

    fn tenant_or_err(&self) -> Result<TenantId, ApiError> {
        self.tenant
            .or_else(|| self.link.demo_state().and_then(|d| d.open))
            .ok_or(ApiError::NoCompanyOpen)
    }

    fn start_load(&mut self) {
        let id = self.id;
        let link = Arc::clone(&self.link);
        if let Some(mut demo) = link.demo_state() {
            let tenant = self.tenant.or(demo.open);
            let out = tenant
                .ok_or(ApiError::NoCompanyOpen)
                .and_then(|t| demo_drafts::get(&demo, t, id))
                .map(|draft| {
                    let meta = draft
                        .attachment_id
                        .and_then(|a| crate::company::demo::checks::find_attachment(&demo, a));
                    let file = draft.attachment_id.and_then(|a| {
                        let bytes = crate::company::demo::checks::download(&demo, a).ok()?;
                        Some(DocSource {
                            name: meta
                                .as_ref()
                                .map(|m| m.file_name.clone())
                                .unwrap_or_default(),
                            bytes: Arc::from(bytes),
                        })
                    });
                    let _ = &mut demo;
                    Loaded {
                        key: meta.as_ref().and_then(key_of),
                        draft,
                        file,
                    }
                });
            drop(demo);
            match out {
                Ok(l) => self.load = Some(Task::ready(l)),
                Err(e) => self.load_failed(&e),
            }
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            self.load_problem = Some("doc_form.load.failed");
            return;
        };
        if let Ok(p) = group.read(&session, move |s| async move {
            let draft = s.document_drafts().get(id).await?;
            let (file, key) = match draft.attachment_id {
                Some(a) => {
                    let meta = s.attachments().get(a).await?;
                    let key = key_of(&meta);
                    let bytes = s.attachments().download(a).await?;
                    let file = (bytes.len() <= MAX_FILE_BYTES).then(|| DocSource {
                        name: meta.file_name,
                        bytes: Arc::from(bytes),
                    });
                    (file, key)
                }
                None => (None, None),
            };
            Ok(Loaded { draft, file, key })
        }) {
            self.load = Some(Task::Pending(p));
        }
    }

    fn load_failed(&mut self, e: &ApiError) {
        self.load_problem = Some(match e {
            ApiError::NotFound { .. } => "doc_form.load.gone",
            _ => "doc_form.load.failed",
        });
    }

    /// The draft and file arrived (or came again after Reload).
    fn loaded(&mut self, l: Loaded, cards: &[(AccountId, Last4)]) {
        if !l.draft.status.is_live() {
            // Entered or discarded since the queue listed it.
            self.dialog = Some(match l.draft.status {
                DraftStatus::Converted => Dialog::AlreadyEntered(l.draft.converted_transaction_id),
                _ => Dialog::DiscardedElsewhere,
            });
            self.closed_elsewhere = l.draft.status == DraftStatus::Discarded;
        }
        self.stored = Some(stored_reading(&l.draft, cards));
        self.draft = Some(l.draft);
        self.carried_values();
        if l.file.is_some() {
            self.file = l.file;
        }
        if l.key.is_some() {
            self.draft_key = l.key;
        }
        self.fill_taken = false;
        self.events.push(DraftEvent::Loaded);
    }

    /// The values for the form, once per load (`None` when there are none:
    /// a `new`, failed or unreadable draft opens empty).
    pub(crate) fn take_fill(&mut self) -> Option<LocalReading> {
        if let Some(r) = self.read_result.take() {
            return Some(*r);
        }
        if std::mem::replace(&mut self.fill_taken, true) {
            return None;
        }
        match &self.stored {
            Some(Stored::Read(r)) => Some((**r).clone()),
            _ => None,
        }
    }

    /// The form filled these targets from the reading (line keys in the
    /// reading's order, for the line spots).
    pub(crate) fn filled(&mut self, targets: &[ReadTarget]) {
        self.line_rows = targets
            .iter()
            .filter_map(|t| match t {
                ReadTarget::Line(k) => Some(*k),
                _ => None,
            })
            .collect();
    }

    /// The card account matched for `cards` when the form's card list
    /// arrives after the draft (a card charge).
    pub(crate) fn rematch_card(&mut self, cards: &[(AccountId, Last4)]) {
        // Only the card: the values may be carried or AI-helped ones.
        if let (Some(Stored::Read(r)), Some(d)) = (&mut self.stored, &self.draft)
            && r.card.is_none()
            && let Stored::Read(again) = stored_reading(d, cards)
            && again.card.is_some()
        {
            r.card = again.card;
        }
    }

    /// The stored values (for spots, the banner and learning).
    fn values(&self) -> Option<&LocalReading> {
        match &self.stored {
            Some(Stored::Read(r)) => Some(r),
            _ => None,
        }
    }

    /// The printed card digits, when the document printed some.
    #[must_use]
    pub(crate) fn printed_card(&self) -> Option<Last4> {
        match self
            .values()?
            .reading
            .header
            .payment
            .as_ref()
            .map(|p| &p.value)
        {
            Some(Payment::Card { last4 }) => Some(*last4),
            _ => None,
        }
    }

    /// Read with the vendor's saved layout and a suggested vendor (D5
    /// banner "Filled from <vendor>'s saved layout").
    #[must_use]
    pub(crate) fn layout_vendor(&self) -> Option<ContactId> {
        let d = self.draft.as_ref()?;
        (d.reader == Some(DraftReader::Template))
            .then_some(d.suggested_contact_id)
            .flatten()
    }

    /// True while nothing may change in the form: a convert or attach in
    /// flight or its outcome unknown (S3), a question open, or the draft
    /// closed.
    #[must_use]
    pub(crate) fn locked(&self) -> bool {
        !matches!(self.work, Work::Idle)
            || self.dialog.is_some()
            || self.attach.is_some()
            || self.discard.is_some()
    }

    /// The draft is closed (posted, attached, entered or discarded
    /// elsewhere): nothing more is sent.
    #[must_use]
    pub(crate) fn closed(&self) -> bool {
        matches!(self.work, Work::Done) || self.closed_elsewhere
    }

    /// True while something runs (the form repaints).
    #[must_use]
    pub(crate) fn busy(&self) -> bool {
        self.load.is_some()
            || self.words_job.is_some()
            || self.read_job.is_some()
            || self.assist.busy()
            || !matches!(
                self.work,
                Work::Idle | Work::Done | Work::Choosing { .. } | Work::SearchFailed { .. }
            )
            || self.learn.layout.is_some()
            || self.learn.card.is_some()
            || self.discard.is_some()
            || self.next.is_some()
            || self.compare.reading_box()
    }

    // -------------------------------------------------------- each frame

    /// Every frame: the load, the words, the convert or attach, learning,
    /// the discard and the next draft. `cards` are the company's card
    /// accounts with their digits (the card match). Returns what the form
    /// must do.
    pub(crate) fn tick(
        &mut self,
        ctx: &egui::Context,
        cards: &[(AccountId, Last4)],
        today: CalendarDate,
    ) -> Vec<DraftEvent> {
        if self.claim.is_none()
            && let Some(h) = crate::screens::receipt_inbox::intake::existing(ctx)
        {
            self.claim = Some(h.lock().hold_draft(self.id));
        }
        self.queue = queue_order(ctx);
        // "Enter this as" left the owner's work for this form (first frame
        // only: the window it came from never takes it back).
        if !std::mem::replace(&mut self.carry_looked, true) {
            self.carry = kind_ai::take_carry(ctx, self.id, self.form);
            self.carried = self.carry.as_ref().map(Carry::kept);
        }
        if let Some(r) = self.load.as_mut().and_then(Task::try_take) {
            self.load = None;
            match r {
                Ok(l) => self.loaded(l, cards),
                Err(e) => self.load_failed(&e),
            }
        }
        self.poll_words(ctx, today);
        self.poll_read();
        self.poll_assist();
        self.poll_work();
        self.poll_learn(today);
        self.poll_discard(ctx);
        if let Some(r) = self.next.as_mut().and_then(Task::try_take) {
            self.next = None;
            self.events.push(DraftEvent::Next(r.ok().flatten()));
        }
        if let Some(d) = self.digests.take() {
            publish_digests(ctx, d);
        }
        if let Some(a) = self.handled.take()
            && let Some(h) = crate::screens::receipt_inbox::intake::existing(ctx)
        {
            // S17: the entry is Linked (or Kept) and its original moves;
            // a crash before this is caught by the inbox's own sweep.
            let _ = h.lock().handled_by_attachment(a);
        }
        if std::mem::take(&mut self.learn.reload_layouts)
            && let Some(h) = crate::screens::receipt_inbox::intake::existing(ctx)
        {
            h.lock().forget_read_context();
        }
        if self.busy() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
        std::mem::take(&mut self.events)
    }

    /// The words: the session cache, else a re-read in the worker (D4:
    /// "Getting the page ready…"). Values never come from it.
    fn poll_words(&mut self, ctx: &egui::Context, today: CalendarDate) {
        if self.words.is_none() && !self.words_tried {
            let ready = matches!(self.stored, Some(Stored::Read(_)));
            if let (true, Some(file)) = (ready, self.file.clone()) {
                self.words_tried = true;
                let cache = word_cache(ctx);
                let hit = cache
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .get(&self.id.to_string());
                match hit {
                    Some(w) => self.set_words(w),
                    None => {
                        if let Some(service) = crate::reading::shared() {
                            self.words_job = Some(service.submit_cancellable(ReadRequest {
                                bytes: file.bytes,
                                hint: cl_docread::protocol::JobHint { kind: None },
                                today,
                                vendors: Vec::new(),
                                templates: Vec::new(),
                                card_accounts: None,
                            }));
                        }
                    }
                }
            }
        }
        let Some(job) = &self.words_job else {
            return;
        };
        let outcome = match job.answer.try_recv() {
            Ok(o) => o,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                ReadOutcome::Failed(crate::reading::ReadFailure::Unavailable)
            }
        };
        self.words_job = None;
        match outcome {
            ReadOutcome::Read(local) => {
                let w: Arc<LocalReading> = Arc::from(local);
                word_cache(ctx)
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .put(&self.id.to_string(), Arc::clone(&w));
                self.set_words(w);
            }
            ReadOutcome::Failed(f) => {
                tracing::info!(outcome = f.class(), "draft words not read");
                // Learning waiting for them is skipped (D5).
                if self.learn.waiting.take().is_some() {
                    self.learn.layout_outcome = Some(Learned::Nothing);
                }
            }
        }
    }

    fn set_words(&mut self, w: Arc<LocalReading>) {
        self.compare.set_words(Some(Arc::new(doc_words(&w))));
        self.words = Some(w);
    }

    /// The form read the document itself (Enter Bill's Read document on a
    /// `new` draft): its words and spots serve the compare tools and
    /// learning (the values are the form's, never stored).
    pub(crate) fn adopt_reading(&mut self, local: &LocalReading) {
        if self.words.is_some() || !matches!(self.stored, Some(Stored::Unread | Stored::Failed)) {
            return;
        }
        self.set_words(Arc::new(local.clone()));
        self.stored = Some(Stored::Read(Box::new(local.clone())));
        self.fill_taken = true;
    }

    /// "Read document" on a `new` draft (card and check forms; Enter Bill
    /// uses its own): the result fills the form only and is not stored.
    pub(crate) fn read_document(
        &mut self,
        vendors: Vec<cl_docread::vendor_match::VendorCandidate>,
        cards: Option<Vec<(AccountId, Last4)>>,
        today: CalendarDate,
    ) -> bool {
        if self.read_job.is_some() || !matches!(self.stored, Some(Stored::Unread | Stored::Failed))
        {
            return false;
        }
        let (Some(file), Some(service)) = (self.file.clone(), crate::reading::shared()) else {
            self.note = Some("doc_form.read.unavailable");
            return false;
        };
        self.read_job = Some(service.submit_cancellable(ReadRequest {
            bytes: file.bytes,
            hint: cl_docread::protocol::JobHint {
                kind: Some(cl_core::intake::IntakeKind::Receipts),
            },
            today,
            vendors,
            templates: Vec::new(),
            card_accounts: cards,
        }));
        true
    }

    fn poll_read(&mut self) {
        let Some(job) = &self.read_job else {
            return;
        };
        let outcome = match job.answer.try_recv() {
            Ok(o) => o,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                ReadOutcome::Failed(crate::reading::ReadFailure::Unavailable)
            }
        };
        self.read_job = None;
        match outcome {
            ReadOutcome::Read(local) => {
                let w: Arc<LocalReading> = Arc::from(local.clone());
                self.set_words(w);
                self.stored = Some(Stored::Read(local.clone()));
                self.fill_taken = true;
                self.read_result = Some(local);
                self.events.push(DraftEvent::Read);
            }
            // The banner says "Couldn't read this document" now (UI
            // check item 4); Read document stays for another try.
            ReadOutcome::Failed(_) => self.read_failed(),
        }
    }

    /// "Read document" failed (here, or Enter Bill's own): a draft not
    /// read yet now shows as one that couldn't be read.
    pub(crate) fn read_failed(&mut self) {
        if matches!(self.stored, Some(Stored::Unread)) {
            self.stored = Some(Stored::Failed);
        }
    }

    // ------------------------------------------------------------ accept

    /// **Accept** (the form's Save & Close / Save & New, its checks
    /// passed): the register search first (every kind, Q-282), then the
    /// convert. False when nothing was started (a note says why).
    pub(crate) fn accept(&mut self, a: Accept) -> bool {
        if self.closed() {
            self.note = Some(if self.closed_elsewhere {
                "doc_form.discarded_elsewhere"
            } else {
                "doc_form.already_done"
            });
            return false;
        }
        if !matches!(self.work, Work::Idle) || self.dialog.is_some() || self.attach.is_some() {
            return false;
        }
        if self.draft.is_none() {
            self.note = Some("doc_form.load.waiting");
            return false;
        }
        self.note = None;
        self.accepts += 1;
        // The same body again (the closing-date password, a refused try
        // fixed elsewhere): the owner already answered the search.
        if let Some((body, ack)) = &self.matched
            && *body == a.body
        {
            let ack = ack.clone();
            self.send_convert(a, ack);
            return true;
        }
        self.start_search(Box::new(a));
        true
    }

    /// The register search for `a` (Q-282). Only an engine without attach
    /// posts without one; a search that couldn't run or failed never posts
    /// (review B2): the owner chooses Try again or Enter as new.
    fn start_search(&mut self, a: Box<Accept>) {
        use super::attach::Search;
        match super::attach::search(&self.link, self.group.as_ref(), self.tenant, &a.probe) {
            Search::Started(task) => {
                self.work = Work::Matching { accept: a, task };
                self.poll_work();
            }
            // No attach on this engine: post as before (the engine's own
            // duplicate check still runs).
            Search::Unsupported => {
                self.matched = Some((a.body.clone(), Vec::new()));
                self.send_convert(*a, Vec::new());
            }
            Search::Failed => self.search_failed(a),
        }
    }

    fn search_failed(&mut self, accept: Box<Accept>) {
        tracing::info!("register search failed; nothing entered");
        self.note = Some("doc_form.match.failed");
        self.work = Work::SearchFailed { accept };
    }

    /// After a failed search: **Enter as new**, the owner's explicit choice
    /// (never the default button). Posts the reviewed body acknowledging
    /// nothing; the engine's own duplicate check still runs.
    pub(crate) fn enter_as_new(&mut self) {
        if let Work::SearchFailed { accept } = std::mem::replace(&mut self.work, Work::Idle) {
            self.note = None;
            self.matched = Some((accept.body.clone(), Vec::new()));
            self.send_convert(*accept, Vec::new());
        }
    }

    /// After a failed search: back to the form, nothing sent.
    fn search_back(&mut self) {
        if matches!(self.work, Work::SearchFailed { .. }) {
            self.work = Work::Idle;
            self.note = None;
            self.events.push(DraftEvent::Cancelled);
        }
    }

    fn key_for(&mut self, id: Identity) -> IdempotencyKey {
        match &self.key {
            Some((same, k)) if *same == id => k.clone(),
            _ => {
                let k = IdempotencyKey::new();
                self.key = Some((id, k.clone()));
                k
            }
        }
    }

    /// Sends the convert (or the identical request again).
    fn send_convert(&mut self, a: Accept, ack: Vec<TransactionId>) {
        let Some(expected) = self.draft.as_ref().map(|d| d.updated_at.clone()) else {
            return;
        };
        let key = self.key_for(Identity {
            body: a.body.clone(),
            expected: expected.clone(),
            ack: ack.clone(),
        });
        // cl-api's body (its own checks run here: nothing is sent when
        // they fail). What the owner reviewed and what goes on the wire
        // (AC1, N4, review S2): the posted digest reads the request body
        // itself. The demo engine takes the same drafts (`PostBody::demo`).
        let live = a.body.live(a.billable);
        self.digests = Some(ReviewDigests {
            attached: None,
            form_digest: Some(a.form_digest.clone()),
            posted_digest: live
                .as_ref()
                .ok()
                .and_then(|b| posted_digest(a.body.kind(), b, &wire_caps(&self.link))),
        });
        let id = self.id;
        if self.link.is_demo() {
            let conversion = DemoConversion {
                body: a.body.demo(),
                expected_updated_at: expected,
                acknowledge: ack.clone(),
            };
            let out = match (self.tenant_or_err(), self.link.demo_state()) {
                (Ok(tenant), Some(mut demo)) => demo_drafts::convert(
                    &mut demo,
                    tenant,
                    id,
                    &conversion,
                    a.password.as_ref(),
                    key,
                ),
                _ => Err(ApiError::NoCompanyOpen),
            };
            // The demo answers at once.
            match out {
                Ok(c) => self.converted(&a, c),
                Err(e) => self.convert_failed(Box::new(a), ack, e),
            }
            return;
        }
        let body = match live {
            Ok(b) => b,
            Err(e) => {
                self.events.push(DraftEvent::Failed {
                    error: e,
                    intent: a.intent,
                });
                return;
            }
        };
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            self.events.push(DraftEvent::Failed {
                error: ApiError::NoCompanyOpen,
                intent: a.intent,
            });
            return;
        };
        let conversion = DraftConversion {
            body,
            expected_updated_at: expected,
            acknowledge: ack.clone(),
        };
        let opts = options(a.password.clone());
        let label = OrphanLabel {
            what: "orphan.draft_convert",
            key: Some(key.clone()),
        };
        match group.write(&session, label, move |s| async move {
            s.document_drafts()
                .convert(id, &conversion, &opts, key)
                .await
        }) {
            Ok(p) => {
                self.work = Work::Converting {
                    accept: Box::new(a),
                    ack,
                    task: Task::Pending(p),
                };
            }
            Err(_) => self.events.push(DraftEvent::Failed {
                error: ApiError::Local(LocalValidation::InvalidValue),
                intent: a.intent,
            }),
        }
    }

    fn refetch(&mut self, accept: Box<Accept>, ack: Vec<TransactionId>, why: Why) {
        let id = self.id;
        let link = Arc::clone(&self.link);
        if let Some(demo) = link.demo_state() {
            let out = self
                .tenant
                .or(demo.open)
                .ok_or(ApiError::NoCompanyOpen)
                .and_then(|t| demo_drafts::get(&demo, t, id));
            drop(demo);
            self.refetched(accept, ack, why, out);
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            self.work = Work::Unknown { accept, ack };
            return;
        };
        let task = match group.read(&session, move |s| async move {
            s.document_drafts().get(id).await
        }) {
            Ok(p) => Task::Pending(p),
            Err(_) => {
                self.work = Work::Unknown { accept, ack };
                return;
            }
        };
        self.work = Work::Refetch {
            accept,
            ack,
            why,
            task,
        };
    }

    fn poll_work(&mut self) {
        let work = std::mem::replace(&mut self.work, Work::Idle);
        self.work = match work {
            Work::Matching { accept, mut task } => match task.try_take() {
                None => Work::Matching { accept, task },
                Some(r) => {
                    // A failed search never posts (review B2): the
                    // engine's duplicate check skips vendor-less receipts.
                    let rows = match r {
                        Ok(rows) => rows,
                        Err(_) => {
                            self.search_failed(accept);
                            return;
                        }
                    };
                    let probe = accept.probe.probe();
                    let cands = cl_docread::register_match::candidates(&probe, &rows);
                    if cands.is_empty() {
                        self.matched = Some((accept.body.clone(), Vec::new()));
                        self.send_convert(*accept, Vec::new());
                        return;
                    }
                    self.attach = Some(AttachDialog::from_candidates(&accept.probe, &rows, &cands));
                    Work::Choosing { accept }
                }
            },
            Work::Converting {
                accept,
                ack,
                mut task,
            } => match task.try_take() {
                None => Work::Converting { accept, ack, task },
                Some(r) => {
                    match r {
                        Ok(c) => self.converted(&accept, c),
                        Err(e) => self.convert_failed(accept, ack, e),
                    }
                    return;
                }
            },
            Work::Resend {
                accept,
                ack,
                at,
                unknown,
                busy,
            } => {
                if Instant::now() < at {
                    Work::Resend {
                        accept,
                        ack,
                        at,
                        unknown,
                        busy,
                    }
                } else {
                    // Keep the counts with the request in flight.
                    self.resends = (unknown, busy);
                    self.send_convert(*accept, ack);
                    return;
                }
            }
            Work::Refetch {
                accept,
                ack,
                why,
                mut task,
            } => match task.try_take() {
                None => Work::Refetch {
                    accept,
                    ack,
                    why,
                    task,
                },
                Some(r) => {
                    self.refetched(accept, ack, why, r);
                    return;
                }
            },
            Work::Attaching {
                accept,
                target,
                mut task,
                tries,
            } => match task.try_take() {
                None => Work::Attaching {
                    accept,
                    target,
                    task,
                    tries,
                },
                Some(r) => {
                    self.attach_answer(accept, target, tries, r);
                    return;
                }
            },
            other => other,
        };
    }

    /// 201: the inbox's entry is handled, learning starts, the form hears.
    fn converted(&mut self, accept: &Accept, c: ConvertedDraft) {
        self.key = None;
        self.resends = (0, 0);
        self.posted = Some(c.transaction);
        self.handled = c
            .draft
            .attachment_id
            .or_else(|| self.draft.as_ref().and_then(|d| d.attachment_id));
        self.draft = Some(c.draft);
        self.work = Work::Done;
        self.note = None;
        if let Some(l) = accept.learn.clone() {
            self.start_learning(l);
        }
        self.events.push(DraftEvent::Posted {
            transaction: c.transaction,
            intent: accept.intent,
        });
    }

    #[allow(clippy::too_many_lines, reason = "one arm per answer of the D2 table")]
    fn convert_failed(&mut self, accept: Box<Accept>, ack: Vec<TransactionId>, e: ApiError) {
        let (unknown, busy) = std::mem::take(&mut self.resends);
        match e {
            ApiError::OutcomeUnknown { .. } | ApiError::Timeout => {
                if unknown + 1 >= UNKNOWN_RESENDS {
                    self.note = Some("doc_form.unknown");
                    self.work = Work::Unknown { accept, ack };
                } else {
                    self.work = Work::Resend {
                        accept,
                        ack,
                        at: Instant::now() + RESEND_AFTER,
                        unknown: unknown + 1,
                        busy,
                    };
                }
            }
            ApiError::Conflict {
                reason: ConflictReason::Busy,
                ..
            } => {
                if busy + 1 >= BUSY_RESENDS {
                    self.note = Some("doc_form.unknown");
                    self.work = Work::Unknown { accept, ack };
                } else {
                    self.work = Work::Resend {
                        accept,
                        ack,
                        at: Instant::now() + RESEND_AFTER,
                        unknown,
                        busy: busy + 1,
                    };
                }
            }
            ApiError::SavedUnreadable { .. } => self.refetch(accept, ack, Why::Unreadable),
            ApiError::IdempotencyKeyReused { .. } => self.refetch(accept, ack, Why::KeyReused),
            ApiError::DocumentDraft(DraftRefusal::PossibleDuplicate { matches, .. }) => {
                let bill = matches!(accept.body, PostBody::Bill(_));
                let attach_ok =
                    crate::screens::receipt_inbox::intake::drafts::attach_on(&self.link);
                self.attach = Some(AttachDialog::from_duplicates(
                    &accept.probe,
                    &matches,
                    bill,
                    attach_ok,
                ));
                self.work = Work::Choosing { accept };
            }
            ApiError::DocumentDraft(DraftRefusal::Closed { transaction }) => {
                self.key = None;
                self.closed_by(transaction);
                self.events.push(DraftEvent::Cancelled);
            }
            ApiError::Conflict {
                reason: ConflictReason::Stale,
                ..
            } => {
                self.dialog = Some(Dialog::Stale { accept, ack });
                self.events.push(DraftEvent::Cancelled);
            }
            ApiError::Forbidden { .. } => {
                self.key = None;
                self.note = Some("doc_form.forbidden");
                self.events.push(DraftEvent::Cancelled);
            }
            error => {
                // The form's own handling: the closing-date password flow
                // (the key stays: the same body goes with the password),
                // a validation path to focus, anything else.
                if !crate::screens::register::keeps_key(&error) {
                    self.key = None;
                }
                self.events.push(DraftEvent::Failed {
                    error,
                    intent: accept.intent,
                });
            }
        }
    }

    /// Converted (or attached) on another computer, or discarded there.
    fn closed_by(&mut self, transaction: Option<TransactionId>) {
        match transaction {
            Some(t) => {
                // No learning, no card digits (D5).
                self.work = Work::Done;
                self.dialog = Some(Dialog::AlreadyEntered(Some(t)));
            }
            None => {
                self.closed_elsewhere = true;
                self.dialog = Some(Dialog::DiscardedElsewhere);
            }
        }
    }

    fn refetched(
        &mut self,
        accept: Box<Accept>,
        ack: Vec<TransactionId>,
        why: Why,
        r: Result<DocumentDraft, ApiError>,
    ) {
        let d = match r {
            Ok(d) => d,
            Err(_) => {
                self.note = Some("doc_form.unknown");
                self.work = Work::Unknown { accept, ack };
                return;
            }
        };
        match (d.status, why) {
            (DraftStatus::Converted, Why::Unreadable) => {
                let transaction = d.converted_transaction_id;
                match transaction {
                    Some(t) => self.converted(
                        &accept,
                        ConvertedDraft {
                            draft: d,
                            transaction: t,
                            draft_matches: Vec::new(),
                        },
                    ),
                    None => {
                        self.note = Some("doc_form.unknown");
                        self.work = Work::Unknown { accept, ack };
                    }
                }
            }
            (DraftStatus::Converted, _) => {
                self.key = None;
                self.closed_by(d.converted_transaction_id);
                self.draft = Some(d);
                self.events.push(DraftEvent::Cancelled);
            }
            (DraftStatus::Discarded, _) => {
                self.key = None;
                self.closed_by(None);
                self.draft = Some(d);
                self.events.push(DraftEvent::Cancelled);
            }
            (_, Why::KeyReused) => {
                // Live: the key went with another request; a new one.
                self.key = None;
                self.draft = Some(d);
                self.send_convert(*accept, ack);
            }
            (_, Why::KeepMine) => {
                // The same body under the fresh token (a new key).
                self.draft = Some(d);
                self.send_convert(*accept, ack);
            }
            (_, Why::Unreadable) => {
                self.note = Some("doc_form.unknown");
                self.draft = Some(d);
                self.work = Work::Unknown { accept, ack };
            }
        }
    }

    /// **Try again**: an unknown outcome sends the identical request once
    /// more; a failed register search runs the search again.
    pub(crate) fn try_again(&mut self) {
        match std::mem::replace(&mut self.work, Work::Idle) {
            Work::Unknown { accept, ack } => {
                self.note = None;
                self.send_convert(*accept, ack);
            }
            Work::SearchFailed { accept } => {
                self.note = None;
                self.start_search(accept);
            }
            other => self.work = other,
        }
    }

    // ------------------------------------------------------------- attach

    /// The attach dialog's answer.
    fn dialog_answer(&mut self, answer: DialogAnswer) {
        let Work::Choosing { accept } = std::mem::replace(&mut self.work, Work::Idle) else {
            self.attach = None;
            return;
        };
        self.attach = None;
        match answer {
            DialogAnswer::Cancel => {
                self.events.push(DraftEvent::Cancelled);
            }
            DialogAnswer::EnterAsNew(ids) => {
                self.matched = Some((accept.body.clone(), ids.clone()));
                self.send_convert(*accept, ids);
            }
            DialogAnswer::Attach(target) => self.send_attach(accept, target, 0),
        }
    }

    fn send_attach(&mut self, accept: Box<Accept>, target: TransactionId, tries: u8) {
        let Some(expected) = self.draft.as_ref().map(|d| d.updated_at.clone()) else {
            return;
        };
        // One key per (target, token): a resend after a lost answer is a
        // replay.
        let key = match &self.attach_key {
            Some((t, e, k)) if *t == target && *e == expected => k.clone(),
            _ => {
                let k = IdempotencyKey::new();
                self.attach_key = Some((target, expected.clone(), k.clone()));
                k
            }
        };
        let id = self.id;
        if self.link.is_demo() {
            let out = match (self.tenant_or_err(), self.link.demo_state()) {
                (Ok(tenant), Some(mut demo)) => {
                    let records_before = demo.ledger.transactions().len();
                    let out = demo_drafts::attach(&mut demo, tenant, id, target, &expected, key)
                        .map(|a| (a.draft, a.transaction));
                    if out.is_ok() {
                        self.digests = Some(ReviewDigests {
                            form_digest: None,
                            posted_digest: None,
                            attached: Some(AttachProof {
                                records_before,
                                records_after: demo.ledger.transactions().len(),
                                files_on_record: crate::company::demo::checks::files_on(
                                    &demo, target,
                                ),
                            }),
                        });
                    }
                    out
                }
                _ => Err(ApiError::NoCompanyOpen),
            };
            self.attach_answer(accept, target, tries, out);
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            self.events.push(DraftEvent::Cancelled);
            return;
        };
        let label = OrphanLabel {
            what: "orphan.draft_attach",
            key: Some(key.clone()),
        };
        let task = match group.write(&session, label, move |s| async move {
            s.document_drafts()
                .attach(id, target, &expected, key)
                .await
                .map(|a| (a.draft, a.transaction))
        }) {
            Ok(p) => Task::Pending(p),
            Err(_) => {
                self.events.push(DraftEvent::Cancelled);
                return;
            }
        };
        self.work = Work::Attaching {
            accept,
            target,
            task,
            tries,
        };
    }

    fn attach_answer(
        &mut self,
        accept: Box<Accept>,
        target: TransactionId,
        tries: u8,
        r: Result<(DocumentDraft, TransactionId), ApiError>,
    ) {
        use cl_api::AttachProblem as P;
        let e = match r {
            Ok((draft, transaction)) => {
                self.attached(draft, transaction);
                return;
            }
            Err(e) => e,
        };
        // A lost answer or a busy key: the same request again (a replay).
        let resend = matches!(
            e,
            ApiError::OutcomeUnknown { .. }
                | ApiError::Timeout
                | ApiError::Conflict {
                    reason: ConflictReason::Busy,
                    ..
                }
        );
        if resend && tries < UNKNOWN_RESENDS {
            self.send_attach(accept, target, tries + 1);
            return;
        }
        self.attach_key = None;
        self.note = Some(match cl_api::attach_problem(&e) {
            Some(P::Closed { transaction }) if transaction == Some(target) => {
                // Attached already (our lost answer): done.
                let draft = self.draft.clone();
                if let Some(d) = draft {
                    self.attached(d, target);
                }
                return;
            }
            Some(P::Closed { transaction }) => {
                self.closed_by(transaction);
                self.events.push(DraftEvent::Cancelled);
                return;
            }
            Some(P::Stale) => "doc_form.attach.stale",
            Some(P::NotFound) => "doc_form.attach.gone",
            Some(P::NotAttachable) => "doc_form.attach.not_attachable",
            Some(P::NotPosted) => "doc_form.attach.not_posted",
            Some(P::FileMissing) => "doc_form.attach.file_missing",
            Some(P::FileOnAnotherRecord | P::FileInUse) => "doc_form.attach.file_elsewhere",
            Some(P::Forbidden) => "doc_form.forbidden",
            None if resend => "doc_form.unknown",
            None => "doc_form.attach.failed",
        });
        self.events.push(DraftEvent::Cancelled);
    }

    fn attached(&mut self, draft: DocumentDraft, transaction: TransactionId) {
        self.attach_key = None;
        self.key = None;
        self.posted = Some(transaction);
        self.handled = draft
            .attachment_id
            .or_else(|| self.draft.as_ref().and_then(|d| d.attachment_id));
        self.draft = Some(draft);
        self.work = Work::Done;
        self.note = None;
        // No learning after an attach (D5).
        self.events.push(DraftEvent::Attached { transaction });
    }

    // ------------------------------------------------------------- learn

    fn start_learning(&mut self, l: LearnInput) {
        if let Some(card) = l.card.filter(|_| self.remember_card) {
            self.learn.card = learn::remember_card(&self.link, self.group.as_ref(), card);
        }
        if l.vendor.is_none() || self.values().is_none() {
            self.learn.layout_outcome = Some(Learned::Nothing);
            return;
        }
        // Without the words this session can't learn yet: it waits for
        // the re-read (skipped if it fails or the app closes).
        self.learn.waiting = Some(l);
    }

    fn poll_learn(&mut self, today: CalendarDate) {
        if let (Some(words), Some(_)) = (self.words.clone(), self.learn.waiting.as_ref())
            && let Some(l) = self.learn.waiting.take()
        {
            let template = self
                .values()
                .and_then(|v| learn::template_of(&v.reading, &words.reading, &l.values, today));
            match (template, l.vendor) {
                (Some(tpl), Some(vendor)) => {
                    self.learn.layout =
                        learn::save_layout(&self.link, self.group.as_ref(), vendor, tpl);
                    if self.learn.layout.is_none() {
                        self.learn.layout_outcome = Some(Learned::Failed);
                    }
                }
                _ => self.learn.layout_outcome = Some(Learned::Nothing),
            }
        }
        if let Some(r) = self.learn.layout.as_mut().and_then(Task::try_take) {
            self.learn.layout = None;
            let outcome = if r.is_ok() {
                self.learn.reload_layouts = true;
                Learned::Stored
            } else {
                Learned::Failed
            };
            tracing::info!(outcome = outcome.name(), "document layout learning");
            self.learn.layout_outcome = Some(outcome);
        }
        if let Some(r) = self.learn.card.as_mut().and_then(Task::try_take) {
            self.learn.card = None;
            let outcome = r.unwrap_or(CardOutcome::Failed);
            tracing::info!(outcome = outcome.name(), "card digits learning");
            self.learn.card_outcome = Some(outcome);
        }
    }

    // ------------------------------------------------------------ discard

    /// **Discard document** (after the confirmation): through the inbox
    /// when this computer holds the arrival (its D2 rules: an own draft is
    /// discarded in the engine, then the original goes to `Discarded/`; a
    /// followed one locally only), else the draft in the engine.
    fn discard_now(&mut self, ctx: &egui::Context) {
        if self.discard.is_some() {
            return;
        }
        let attachment = self.draft.as_ref().and_then(|d| d.attachment_id);
        if let (Some(a), Some(h)) = (
            attachment,
            crate::screens::receipt_inbox::intake::existing(ctx),
        ) && let Some(r) = h.lock().discard_by_attachment(&self.link, a)
        {
            match r {
                Ok(()) => {
                    self.discard_local = true;
                    self.discard = Some(Task::ready(()));
                }
                Err(key) => self.note = Some(key),
            }
            return;
        }
        let id = self.id;
        let link = Arc::clone(&self.link);
        if let Some(mut demo) = link.demo_state() {
            let out = self
                .tenant
                .or(demo.open)
                .ok_or(ApiError::NoCompanyOpen)
                .and_then(|t| demo_drafts::discard(&mut demo, t, id))
                .map(|_| ());
            drop(demo);
            match out {
                Ok(()) => self.discard = Some(Task::ready(())),
                Err(e) => self.discard_failed(&e),
            }
            return;
        }
        self.discard = Some({
            let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
                self.note = Some("doc_form.discard.failed");
                return;
            };
            let label = OrphanLabel {
                what: "orphan.draft_discard",
                key: None,
            };
            match group.write(&session, label, move |s| async move {
                s.document_drafts().discard(id).await.map(|_| ())
            }) {
                Ok(p) => Task::Pending(p),
                Err(_) => {
                    self.note = Some("doc_form.discard.failed");
                    return;
                }
            }
        });
    }

    fn discard_failed(&mut self, e: &ApiError) {
        self.note = Some(match e {
            ApiError::DocumentDraft(DraftRefusal::Closed { .. }) => "doc_form.already_done",
            _ => "doc_form.discard.failed",
        });
    }

    fn poll_discard(&mut self, _ctx: &egui::Context) {
        if let Some(r) = self.discard.as_mut().and_then(Task::try_take) {
            self.discard = None;
            match r {
                Ok(()) => {
                    self.work = Work::Done;
                    self.closed_elsewhere = true;
                    self.link.push(CompanyEvent::Toast("doc_form.discarded"));
                    self.events.push(DraftEvent::Close);
                }
                Err(e) => self.discard_failed(&e),
            }
        }
    }

    // ------------------------------------------------------------- queue

    /// The draft one `step` from this one in the queue: the order WP-F1
    /// published, else the engine's live drafts (newest first).
    fn neighbour(&self, step: Step) -> Option<DraftOpen> {
        let ids: Vec<DocumentDraftId> = self.queue.iter().map(|(i, _)| *i).collect();
        let mut cursor = cl_model::doc_draft::QueueCursor::new();
        if !cursor.select(&ids, &self.id) {
            return None;
        }
        let to = cursor.neighbour(&ids, step)?;
        let kind = self.queue.iter().find(|(i, _)| *i == to).map(|(_, k)| *k)?;
        Some(DraftOpen {
            id: to,
            form: doc_kind(kind).form()?,
        })
    }

    /// Looks for the next draft (Save & New): answers with
    /// [`DraftEvent::Next`].
    pub(crate) fn find_next(&mut self) {
        if let Some(n) = self
            .neighbour(Step::Next)
            .or_else(|| self.neighbour(Step::Prev))
        {
            self.events.push(DraftEvent::Next(Some(n)));
            return;
        }
        let me = self.id;
        let pick = move |page: cl_api::DraftPage| {
            page.drafts
                .into_iter()
                .filter(|d| d.id != me && d.status.is_live())
                .find_map(|d| {
                    Some(DraftOpen {
                        id: d.id,
                        form: doc_kind(d.kind).form()?,
                    })
                })
        };
        let query = DraftListQuery {
            statuses: vec![DraftStatus::New, DraftStatus::Ready, DraftStatus::Failed],
            limit: cl_model::doc_draft::QUEUE_PAGE,
            summary: true,
            ..DraftListQuery::default()
        };
        if let Some(demo) = self.link.demo_state() {
            let out = self
                .tenant
                .or(demo.open)
                .ok_or(ApiError::NoCompanyOpen)
                .and_then(|t| demo_drafts::list(&demo, t, &query));
            self.next = Some(Task::ready(out.ok().and_then(pick)));
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            self.events.push(DraftEvent::Next(None));
            return;
        };
        match group.read(&session, move |s| async move {
            Ok(pick(s.document_drafts().list(&query).await?))
        }) {
            Ok(p) => self.next = Some(Task::Pending(p)),
            Err(_) => self.events.push(DraftEvent::Next(None)),
        }
    }

    /// Page Up/Down in a draft form: the previous or next draft of the
    /// queue; with unsaved changes the owner is asked first. False when
    /// there is none.
    pub(crate) fn step(&mut self, step: Step, dirty: bool) -> bool {
        if self.locked() {
            return false;
        }
        let Some(to) = self.neighbour(step) else {
            self.note = Some("doc_form.queue.end");
            return false;
        };
        if dirty && !self.closed() {
            self.dialog = Some(Dialog::Leave(to));
        } else {
            self.events.push(DraftEvent::Next(Some(to)));
        }
        true
    }

    // ---------------------------------------------------------- commands

    /// The form's keys while the session has a question up. `Some` when
    /// the key was the session's.
    pub(crate) fn command(&mut self, id: CommandId) -> Option<bool> {
        if let Some(d) = self.attach.as_mut()
            && let Some(answer) = d.command(id)
        {
            if let Some(a) = answer {
                self.dialog_answer(a);
            }
            return Some(true);
        }
        if self.dialog.is_some() && id.as_str() == "form.revert" {
            self.answer_dialog(false, None);
            return Some(true);
        }
        if matches!(self.work, Work::SearchFailed { .. }) {
            match id.as_str() {
                // Alt+E: the explicit choice; Esc: back to the form.
                "form.enter_as_new" => {
                    self.enter_as_new();
                    return Some(true);
                }
                "form.revert" => {
                    self.search_back();
                    return Some(true);
                }
                _ => {}
            }
        }
        match id.as_str() {
            "form.discard_document" if !self.closed() => {
                self.dialog = Some(Dialog::ConfirmDiscard);
                Some(true)
            }
            _ => None,
        }
    }

    /// Asks "Discard this document?" (the banner's Discard document).
    pub(crate) fn ask_discard(&mut self) {
        if !self.closed() && !self.locked() {
            self.dialog = Some(Dialog::ConfirmDiscard);
        }
    }

    /// A dialog's answer: `yes` is its first button.
    fn answer_dialog(&mut self, yes: bool, ctx: Option<&egui::Context>) {
        let Some(d) = self.dialog.take() else {
            return;
        };
        match d {
            Dialog::Stale { accept, ack } => {
                if yes {
                    self.refetch(accept, ack, Why::KeepMine);
                } else {
                    // Reload: the form fills again from the new draft.
                    self.key = None;
                    self.matched = None;
                    self.clicked.clear();
                    self.compare.clear();
                    self.start_load();
                    self.events.push(DraftEvent::Reload);
                }
            }
            Dialog::AlreadyEntered(t) => {
                if yes && let Some(t) = t {
                    let kind = CommandId::new(self.form.command());
                    self.link.push(CompanyEvent::OpenRecord(kind, t));
                }
                self.events.push(DraftEvent::Close);
            }
            Dialog::DiscardedElsewhere => {}
            Dialog::ConfirmDiscard => {
                if yes && let Some(ctx) = ctx {
                    self.discard_now(ctx);
                }
            }
            Dialog::Leave(to) => {
                if yes {
                    self.events.push(DraftEvent::Next(Some(to)));
                }
            }
        }
    }

    // ---------------------------------------------------------------- ui

    /// Draws the session's questions over the form (the attach dialog, the
    /// stale question, "Already entered", the discard confirmation).
    pub(crate) fn dialogs_ui(&mut self, ctx: &egui::Context, style: MoneyStyle) {
        if let Some(d) = self.attach.as_mut()
            && let Some(answer) = d.ui(ctx, style)
        {
            self.dialog_answer(answer);
        }
        let Some(dialog) = self.dialog.clone() else {
            return;
        };
        let (title, body, yes, no): (&str, &str, &str, Option<&str>) = match &dialog {
            Dialog::Stale { .. } => (
                "doc_form.stale.title",
                "doc_form.stale.body",
                "doc_form.stale.keep",
                Some("doc_form.stale.reload"),
            ),
            Dialog::AlreadyEntered(Some(_)) => (
                "doc_form.entered.title",
                "doc_form.entered.body",
                "doc_form.entered.open",
                Some("doc_form.close"),
            ),
            Dialog::AlreadyEntered(None) => (
                "doc_form.entered.title",
                "doc_form.entered.body",
                "doc_form.close",
                None,
            ),
            Dialog::DiscardedElsewhere => (
                "doc_form.discarded_elsewhere.title",
                "doc_form.discarded_elsewhere",
                "doc_form.ok",
                None,
            ),
            Dialog::ConfirmDiscard => (
                "doc_form.discard.title",
                "doc_form.discard.body",
                "doc_form.discard.yes",
                Some("doc_form.cancel"),
            ),
            Dialog::Leave(_) => (
                "doc_form.leave.title",
                "doc_form.leave.body",
                "doc_form.leave.yes",
                Some("doc_form.cancel"),
            ),
        };
        let mut answer = None;
        let width = (ctx.content_rect().width() - 48.0).clamp(240.0, 460.0);
        egui::Window::new(t(title))
            .id(egui::Id::unique(("a04/draft_dialog", dialog.name())))
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.set_width(width);
                ui.add(egui::Label::new(t(body)).wrap());
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let first = if matches!(dialog, Dialog::ConfirmDiscard) {
                        crate::widgets::destructive_button(ui, &t(yes))
                    } else {
                        ui.button(t(yes))
                    };
                    if first.clicked() {
                        answer = Some(true);
                    }
                    if let Some(no) = no {
                        // The safe answer is the default (Enter on it).
                        let r = ui.button(t(no));
                        if !ui.ctx().memory(|m| m.focused().is_some()) {
                            r.request_focus();
                        }
                        if r.clicked() {
                            answer = Some(false);
                        }
                    }
                });
            });
        if let Some(yes) = answer {
            self.answer_dialog(yes, Some(ctx));
        }
    }

    /// The draft banner over the form: where the values came from, the
    /// reading's state, notes, "Remember card ending …" and **Discard
    /// document**. `read`: the card and check forms' Accept / Clear (Enter
    /// Bill has its own). `vendor_name`: the suggested vendor's name for
    /// "Filled from <vendor>'s saved layout".
    pub(crate) fn banner_ui(
        &mut self,
        ui: &mut egui::Ui,
        tokens: &Tokens,
        read: Option<BannerRead>,
        vendor_name: Option<&str>,
    ) -> BannerClick {
        let mut click = BannerClick::None;
        egui::Frame::new()
            .fill(tokens.info_tint)
            .stroke(egui::Stroke::new(1.0, tokens.border_default))
            .corner_radius(egui::CornerRadius::same(6))
            .inner_margin(egui::Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                let file = self
                    .file
                    .as_ref()
                    .map(|f| f.name.clone())
                    .unwrap_or_default();
                let line = match (&self.stored, self.layout_vendor(), vendor_name) {
                    (_, _, _) if self.load.is_some() => t("doc_form.loading"),
                    (Some(Stored::Read(_)), _, _) if self.hide_source_line => String::new(),
                    (_, _, _) if self.load_problem.is_some() => {
                        t(self.load_problem.unwrap_or("doc_form.load.failed"))
                    }
                    (Some(Stored::Read(_)), Some(_), Some(name)) => {
                        t_with("doc_form.banner.layout", &[("vendor", name)])
                    }
                    (Some(Stored::Read(_)), _, _) => {
                        t_with("doc_form.banner.file", &[("file", &file)])
                    }
                    (Some(Stored::Unread), _, _) => t("doc_form.banner.unread"),
                    (Some(Stored::Failed | Stored::Unreadable), _, _) => {
                        t("doc_form.banner.unreadable")
                    }
                    (None, _, _) => t("doc_form.loading"),
                };
                if !line.is_empty() {
                    ui.add(egui::Label::new(RichText::new(line).color(tokens.text_primary)).wrap());
                }
                if let Some(r) = read
                    && r.unaccepted > 0
                {
                    let text = if r.unaccepted == 1 {
                        t("doc_form.banner.unaccepted.one")
                    } else {
                        t_with(
                            "doc_form.banner.unaccepted",
                            &[("n", &r.unaccepted.to_string())],
                        )
                    };
                    ui.add(egui::Label::new(RichText::new(text).color(tokens.warn_text)).wrap());
                    ui.horizontal_wrapped(|ui| {
                        let accept = ui
                            .add(egui::Button::new(
                                RichText::new(t("bill.read.accept")).strong(),
                            ))
                            .on_hover_text(t("bill.read.accept.tip"));
                        crate::widgets::kbd(
                            ui,
                            &crate::screens::card_charge::kit::chip("form.accept_read"),
                        );
                        if r.focus_accept {
                            accept.request_focus();
                        }
                        if accept.clicked() {
                            click = BannerClick::Accept;
                        }
                        if ui
                            .button(t("bill.read.clear"))
                            .on_hover_text(t("bill.read.clear.tip"))
                            .clicked()
                        {
                            click = BannerClick::Clear;
                        }
                    });
                }
                if read.is_some()
                    && matches!(self.stored, Some(Stored::Unread | Stored::Failed))
                    && self.file.is_some()
                {
                    let busy = self.read_job.is_some();
                    if ui
                        .add_enabled(!busy, egui::Button::new(t("bill.read.local.button")))
                        .on_hover_text(t("bill.read.local.button.tip"))
                        .clicked()
                    {
                        click = BannerClick::ReadDocument;
                    }
                    if busy {
                        ui.label(
                            RichText::new(t("bill.read.local.reading"))
                                .color(tokens.text_secondary),
                        );
                    }
                }
                if self.words_job.is_some() {
                    ui.label(
                        RichText::new(t("compare.note.words_loading")).color(tokens.text_secondary),
                    );
                }
                // Ask AI to help (card and check forms; Enter Bill has
                // A03's own on its reading banner).
                if read.is_some() && self.assist_banner(ui, tokens) {
                    click = BannerClick::AskAi;
                }
                if let Some(digits) = self
                    .printed_card()
                    .filter(|_| read.is_some_and(|r| r.card_offer))
                {
                    ui.checkbox(
                        &mut self.remember_card,
                        t_with("doc_form.remember_card", &[("digits", digits.as_str())]),
                    );
                }
                if let Some(n) = self.note {
                    ui.add(egui::Label::new(RichText::new(t(n)).color(tokens.warn_text)).wrap());
                }
                if matches!(self.work, Work::Unknown { .. })
                    && ui.button(t("doc_form.try_again")).clicked()
                {
                    click = BannerClick::TryAgain;
                }
                if matches!(self.work, Work::SearchFailed { .. }) {
                    // Try again first and focused; Enter as new is the
                    // owner's explicit choice, never the default (B2).
                    ui.horizontal_wrapped(|ui| {
                        let again = ui.button(t("doc_form.try_again"));
                        if !ui.ctx().memory(|m| m.focused().is_some()) {
                            again.request_focus();
                        }
                        if again.clicked() {
                            click = BannerClick::TryAgain;
                        }
                        if ui
                            .button(t("doc_form.match.enter_new"))
                            .on_hover_text(t("doc_form.match.enter_new.tip"))
                            .clicked()
                        {
                            click = BannerClick::EnterAsNew;
                        }
                        crate::widgets::kbd(
                            ui,
                            &crate::screens::card_charge::kit::chip("form.enter_as_new"),
                        );
                        if ui.button(t("doc_form.match.back")).clicked() {
                            click = BannerClick::SearchBack;
                        }
                    });
                }
                if matches!(self.work, Work::Matching { .. }) {
                    ui.label(RichText::new(t("doc_form.matching")).color(tokens.text_secondary));
                }
                if matches!(
                    self.work,
                    Work::Converting { .. } | Work::Resend { .. } | Work::Refetch { .. }
                ) {
                    ui.label(RichText::new(t("doc_form.entering")).color(tokens.text_secondary));
                }
                if !self.closed() && self.draft.is_some() {
                    // "Enter this as" (Q-A04n): the other forms.
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            RichText::new(t("doc_form.enter_as")).color(tokens.text_secondary),
                        );
                        for f in DraftForm::ALL {
                            if f != self.form
                                && ui
                                    .add_enabled(!self.locked(), egui::Button::new(t(form_key(f))))
                                    .on_hover_text(t("doc_form.enter_as.tip"))
                                    .clicked()
                            {
                                click = BannerClick::EnterAs(f);
                            }
                        }
                        // One row with Discard document: the lines grid
                        // keeps its room (UI check item 3).
                        ui.separator();
                        if ui
                            .add_enabled(!self.locked(), egui::Button::new(t("doc_form.discard")))
                            .on_hover_text(t("doc_form.discard.tip"))
                            .clicked()
                        {
                            click = BannerClick::Discard;
                        }
                    });
                } else if !self.closed()
                    && ui
                        .add_enabled(!self.locked(), egui::Button::new(t("doc_form.discard")))
                        .on_hover_text(t("doc_form.discard.tip"))
                        .clicked()
                {
                    click = BannerClick::Discard;
                }
            });
        ui.add_space(4.0);
        match click {
            BannerClick::Discard => self.ask_discard(),
            BannerClick::TryAgain => self.try_again(),
            BannerClick::EnterAsNew => self.enter_as_new(),
            BannerClick::SearchBack => self.search_back(),
            _ => {}
        }
        click
    }

    // ----------------------------------------------------------- compare

    /// A target's spot: the click's, else the reading's.
    #[must_use]
    pub(crate) fn spot_of(&self, target: ReadTarget) -> Option<PageBox> {
        if let Some(b) = self.clicked.get(&target) {
            return Some(*b);
        }
        let h = &self.values()?.reading.header;
        let s = match target {
            ReadTarget::Vendor | ReadTarget::Payee => h.vendor.as_ref()?.spot,
            ReadTarget::BillDate | ReadTarget::Date => h.date.as_ref()?.spot,
            ReadTarget::DueDate => h.due_date.as_ref()?.spot,
            ReadTarget::Reference => h.reference.as_ref()?.spot,
            ReadTarget::AmountDue | ReadTarget::Total => {
                h.total.as_ref().or(h.balance_due.as_ref())?.spot
            }
            ReadTarget::CheckNumber | ReadTarget::PaidFrom => h.payment.as_ref()?.spot,
            ReadTarget::Line(k) => {
                let i = self.line_rows.iter().position(|r| *r == k)?;
                self.values()?.reading.lines.get(i)?.amount.spot
            }
        };
        spot_box(s)
    }

    /// The form's compare targets this frame (`fields`: each target, its
    /// label, what a click fills and the value shown).
    pub(crate) fn set_targets(&mut self, marks: &ReadMarks, fields: Vec<TargetIn>) {
        let mut targets = Vec::with_capacity(fields.len());
        let mut values = Vec::with_capacity(fields.len());
        for f in fields {
            let spot = self.spot_of(f.target);
            let state = marks.state(f.target);
            let level = if self.clicked.contains_key(&f.target) {
                TargetLevel::Sure
            } else {
                TargetLevel::of(state, spot.is_some())
            };
            targets.push(CompareTarget {
                key: TargetKey::from(f.target),
                label: f.label,
                spot,
                level,
                fill: f.fill,
            });
            values.push(f.value);
        }
        self.compare.set_targets(targets, values);
    }

    /// The target of a compare key.
    #[must_use]
    #[allow(clippy::unused_self, reason = "the forms ask their session")]
    pub(crate) fn target_of(&self, key: TargetKey) -> Option<ReadTarget> {
        let all = [
            ReadTarget::Vendor,
            ReadTarget::BillDate,
            ReadTarget::DueDate,
            ReadTarget::Reference,
            ReadTarget::AmountDue,
            ReadTarget::Payee,
            ReadTarget::Date,
            ReadTarget::Total,
            ReadTarget::CheckNumber,
            ReadTarget::PaidFrom,
        ];
        if let Some(row) = key.row {
            return (key.field == "line_amount").then_some(ReadTarget::Line(row));
        }
        all.into_iter().find(|t| TargetKey::from(*t) == key)
    }

    /// One viewer event through the compare tools; the action for the form.
    pub(crate) fn compare_event(
        &mut self,
        event: CompareEvent,
        today: CalendarDate,
    ) -> Option<CompareAction> {
        let service = crate::reading::shared();
        let env = CompareEnv {
            today,
            service: service.as_deref(),
            bytes: self.file.as_ref().map(|f| Arc::clone(&f.bytes)),
        };
        let a = self.compare.on_event(event, &env)?;
        self.note_fill(&a);
        Some(a)
    }

    /// A box re-read's answer, when it came.
    pub(crate) fn compare_poll(&mut self, today: CalendarDate) -> Option<CompareAction> {
        let a = self.compare.poll(today)?;
        self.note_fill(&a);
        Some(a)
    }

    fn note_fill(&mut self, a: &CompareAction) {
        if let CompareAction::Fill { key, spot, .. } = a
            && let Some(t) = self.target_of(*key)
        {
            self.clicked.insert(t, *spot);
        }
    }

    /// The final value's entry for learning: clicked (with its spot), read
    /// (still the read value) or typed.
    #[must_use]
    pub(crate) fn entry<T: Clone>(
        &self,
        target: ReadTarget,
        marks: &ReadMarks,
        value: &T,
    ) -> cl_docread::correct::Entry<T> {
        use cl_docread::correct::Entry;
        if let Some(spot) = self.clicked.get(&target).and_then(|b| box_spot(*b)) {
            return Entry::Clicked {
                spot,
                filled: value.clone(),
            };
        }
        match marks.state(target) {
            Some(ReadField::Read { .. } | ReadField::Accepted) => Entry::Read,
            _ => Entry::Typed,
        }
    }

    // ------------------------------------------------------- control channel

    /// `state().draft`: states, flags and counts only (no amounts, names,
    /// file names or document text).
    #[must_use]
    pub(crate) fn state_json(&self) -> Value {
        let stored = match &self.stored {
            None => None,
            Some(Stored::Unread) => Some("unread"),
            Some(Stored::Failed) => Some("failed"),
            Some(Stored::Read(_)) => Some("read"),
            Some(Stored::Unreadable) => Some("unreadable"),
        };
        json!({
            "form": self.form.kind().name(),
            "loaded": self.draft.is_some(),
            "load_problem": self.load_problem,
            "stored": stored,
            "layout_vendor": self.layout_vendor().is_some(),
            "has_file": self.file.is_some(),
            "status": self.draft.as_ref().map(|d| d.status.as_str()),
            "work": self.work.name(),
            "dialog": self.dialog.as_ref().map(Dialog::name),
            "attach": self.attach.as_ref().map(AttachDialog::state_json),
            "note": self.note,
            "posted": self.posted.map(|t| t.to_string()),
            "locked": self.locked(),
            "closed": self.closed(),
            "words": self.words.is_some(),
            "clicked": self.clicked.len(),
            "accepts": self.accepts,
            "printed_card": self.printed_card().is_some(),
            "remember_card": self.remember_card,
            "learn": {
                "waiting": self.learn.waiting.is_some(),
                "layout": self.learn.layout_outcome.map(Learned::name),
                "card": self.learn.card_outcome.map(CardOutcome::name),
            },
            "assist": self.assist_json(),
            "carried": self.carried,
        })
    }
}

/// What the card and check forms show on the banner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BannerRead {
    /// Read values not accepted yet.
    pub unaccepted: usize,
    /// Put the keyboard on Accept (a save was refused for it).
    pub focus_accept: bool,
    /// Offer "Remember card ending …" (a card charge whose account has no
    /// digits yet).
    pub card_offer: bool,
}

/// What was clicked on the banner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BannerClick {
    None,
    /// Accept these values (Alt+A).
    Accept,
    /// Clear and type it myself.
    Clear,
    /// Discard document (the confirmation is up now).
    Discard,
    /// Read document (a `new` draft).
    ReadDocument,
    /// Try again (unknown outcome, or a failed register search; already
    /// done).
    TryAgain,
    /// Enter as new after a failed register search (already sent).
    EnterAsNew,
    /// Back to the form after a failed register search (already done).
    SearchBack,
    /// Ask AI to help (the form passes its vendors to
    /// [`DraftSession::ask_ai`]).
    AskAi,
    /// "Enter this as" this form (the form builds the [`Carry`]).
    EnterAs(DraftForm),
}

/// One form field for the compare tools.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TargetIn {
    /// The field.
    pub target: ReadTarget,
    /// Its label as shown.
    pub label: String,
    /// What a click fills (`None`: picked from a list).
    pub fill: Option<FillKind>,
    /// The value shown (owner data, for the check list only).
    pub value: String,
}
