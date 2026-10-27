//! The intake inbox's document drafts (A04 D1, D2 Discard, D3 Add to…, D6
//! follow), when the engine has `document-drafts-v1`. Without it nothing
//! here runs and the inbox behaves as in A01.
//!
//! - **Claim on arrival** (Q-289): every arrival is claimed as a document
//!   draft, auto-read or not, one at a time. The entry is saved as
//!   `claimSent` before the claim is sent (A01 D4); a claim whose answer
//!   isn't known counts as claimed (the plain P17 paths are refused). The
//!   answers (plan D1 table, [`ClaimAnswer`]): made → `own`; the same bytes
//!   are another draft → its file is one of **this entry's** uploads →
//!   `own` (adopted), entered already → handled, still waiting → the entry
//!   `follows` it as a duplicate and its extra copy is deleted
//!   ([`JobKind::DeleteCopy`]: only this entry's own uploads, only while
//!   the engine keeps them on a draft key); filed on a record → handled;
//!   gone (a restore) → uploaded again once; refused (403) → shown on the
//!   row, never retried this session; anything else → tried again in 30 s.
//!   A P17 upload sharing a draft key is first linked alone under a fresh
//!   key (convert then moves only it).
//! - **Reading** ([`ReadQueue`], batch lane): auto-read folders read their
//!   arrivals once claimed; **Read all** reads every `new` draft of the
//!   company. One at a time, off the egui thread: the draft is fetched (it
//!   must still be `new`; its `updated_at` is the store's token), its file
//!   downloaded at its turn (at most 10 MB, so no queued job holds bytes),
//!   read by the local reading service (never AI, no connection), and
//!   stored. A stale or closed draft drops the result; "no reader on this
//!   computer" and Cancel store nothing (the draft stays "Waiting to be
//!   read"). Drafts open in a form ([`Intake::hold_draft`]) are skipped.
//! - **Follow** (M1, Q-A04o): every [`FOLLOW_EVERY`], each entry that owns
//!   a draft and is still waiting, and each that follows one, is looked up
//!   by id ([`FOLLOW_BATCH`] per request): entered → handled (the original
//!   moves to `Processed/`), discarded → discarding (an own copy is deleted
//!   after the move; a followed draft deletes nothing of the other
//!   computer's), gone → the link goes and an arrival is claimed again.
//!
//! Nothing here logs a file name, an amount or document text; the control
//! channel gets counts and states.

use cl_api::{
    AttachProblem, Capabilities, ContactFilter, DocumentDraft, DocumentDraftId, DraftKind,
    DraftListQuery, DraftRefusal, DraftStatus, attach_problem,
};
use cl_core::ids::TransactionId;
use cl_core::intake::IntakeKind;
use cl_docread::protocol::JobHint;
use cl_docread::vendor_match::VendorCandidate;
use cl_model::doc_draft::FOLLOW_BATCH;
use cl_settings::inbox_state::{DraftLink, InboxEntry};

use super::{
    AttachmentId, Claim, DraftKey, EntryStatus, FileIdentity, IdempotencyKey, Instant, Intake, Job,
    JobDone, JobKind, OrphanLabel, RETRY, Task,
};
use crate::company::CompanyLink;
use crate::reading::queue::{BatchProgress, Lane, MAX_DOWNLOAD_BYTES, ReadQueue};
use crate::reading::service::{
    ReadContext, ReadFailure, ReadOutcome, ReadRequest, ReadingService, card_accounts_of,
    draft_reading, load_context, with_layouts,
};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Weak};
use std::time::Duration;

/// How often claimed entries are looked up by id (M1).
pub(crate) const FOLLOW_EVERY: Duration = Duration::from_secs(60);

/// Pages of `new` drafts Read all lists (the engine's page cap).
const READ_ALL_PAGE: u32 = cl_api::draft_limits::MAX_PAGE;

/// Read all lists at most this many drafts.
const READ_ALL_MAX: usize = 2000;

/// Resends of an attach whose answer was lost (same key).
const ATTACH_RESENDS: u8 = 3;

// ---------------------------------------------------------------------------
// The claim's answer (pure, so every row of the D1 table is tested).

/// What a claim of one arrival ended as (plan D1 "Claim answers").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ClaimAnswer {
    /// 201, or 409 naming a draft that reads one of this entry's own
    /// uploads (an earlier claim whose reply was lost): `own`.
    Own(DocumentDraftId),
    /// 409 naming another computer's live draft of the same bytes:
    /// `duplicate` + `follows`, the extra copy deleted.
    Elsewhere(DocumentDraftId),
    /// The winner was entered already, or 422 `ATTACHMENT_NOT_DRAFT` (the
    /// upload is on a record): handled.
    Entered,
    /// 404: the upload is gone (a restore, or drafts reset): forget it and
    /// upload again once.
    Gone,
    /// 403: stop; the row says so.
    Refused,
    /// The engine has no document drafts (now): nothing changes.
    Unsupported,
    /// Network, 5xx, a busy key, a winner that just went: try again later.
    Retry,
}

impl ClaimAnswer {
    /// A short name for logs (no owner data).
    pub(crate) fn class(self) -> &'static str {
        match self {
            ClaimAnswer::Own(_) => "own",
            ClaimAnswer::Elsewhere(_) => "elsewhere",
            ClaimAnswer::Entered => "entered",
            ClaimAnswer::Gone => "gone",
            ClaimAnswer::Refused => "refused",
            ClaimAnswer::Unsupported => "unsupported",
            ClaimAnswer::Retry => "retry",
        }
    }
}

/// The create's answer: done, or the winner to look at.
pub(crate) enum FirstAnswer {
    Done(ClaimAnswer),
    Winner(DocumentDraftId),
}

/// Reads the answer of `POST /document-drafts`.
pub(crate) fn first_answer(r: &Result<DocumentDraft, cl_api::ApiError>) -> FirstAnswer {
    use cl_api::ApiError as E;
    FirstAnswer::Done(match r {
        Ok(d) => ClaimAnswer::Own(d.id),
        Err(E::DocumentDraft(DraftRefusal::AlreadyClaimed { draft: Some(id) })) => {
            return FirstAnswer::Winner(*id);
        }
        Err(E::DocumentDraft(DraftRefusal::NotADraftFile)) => ClaimAnswer::Entered,
        Err(E::NotFound { .. }) => ClaimAnswer::Gone,
        Err(E::Forbidden { .. }) => ClaimAnswer::Refused,
        Err(E::Local(cl_api::LocalValidation::Unsupported)) => ClaimAnswer::Unsupported,
        Err(_) => ClaimAnswer::Retry,
    })
}

/// Reads the winner (`GET /document-drafts/{id}`) after a 409: adopted only
/// when it reads one of `ours` (this entry's own uploads, T4).
pub(crate) fn winner_answer(
    r: &Result<DocumentDraft, cl_api::ApiError>,
    ours: &[String],
) -> ClaimAnswer {
    match r {
        Ok(d)
            if d.attachment_id
                .is_some_and(|a| ours.iter().any(|o| *o == a.to_string())) =>
        {
            if d.status == DraftStatus::Converted {
                ClaimAnswer::Entered
            } else if d.status.is_live() {
                ClaimAnswer::Own(d.id)
            } else {
                // Discarded since: the claim is free again.
                ClaimAnswer::Retry
            }
        }
        Ok(d) if d.status == DraftStatus::Converted => ClaimAnswer::Entered,
        Ok(d) if d.status.is_live() => ClaimAnswer::Elsewhere(d.id),
        Err(cl_api::ApiError::Forbidden { .. }) => ClaimAnswer::Refused,
        // Discarded or gone since the 409, or no answer: claim again.
        _ => ClaimAnswer::Retry,
    }
}

/// What a followed draft is now (D6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Followed {
    /// Still waiting.
    Live,
    /// Entered (converted or attached).
    Entered,
    /// Discarded.
    Discarded,
    /// Not there (a restore, or drafts reset).
    Gone,
}

pub(crate) fn followed(r: &Result<DocumentDraft, cl_api::ApiError>) -> Result<Followed, ()> {
    match r {
        Ok(d) => Ok(match d.status {
            DraftStatus::Converted => Followed::Entered,
            DraftStatus::Discarded => Followed::Discarded,
            _ => Followed::Live,
        }),
        Err(cl_api::ApiError::NotFound { .. }) => Ok(Followed::Gone),
        Err(_) => Err(()),
    }
}

/// The record an Add to… target is (U-95 attaches to transactions only).
pub(crate) fn target_transaction(to: cl_api::AttachTo) -> Option<TransactionId> {
    use cl_api::AttachTo as A;
    match to {
        A::Expense(id) | A::Bill(id) | A::VendorCredit(id) | A::Txn(_, id) => Some(id),
        A::Draft(_) => None,
    }
}

/// The message an attach refusal shows (i18n key).
pub(crate) fn attach_problem_key(e: &cl_api::ApiError) -> &'static str {
    match attach_problem(e) {
        Some(AttachProblem::Closed { .. }) => "inbox.attach.closed",
        Some(AttachProblem::Stale) => "inbox.attach.stale",
        Some(AttachProblem::NotFound) => "inbox.attach.not_found",
        Some(AttachProblem::NotAttachable) => "inbox.attach.not_attachable",
        Some(AttachProblem::NotPosted) => "inbox.attach.not_posted",
        Some(AttachProblem::FileMissing | AttachProblem::FileOnAnotherRecord) => {
            "inbox.attach.file_gone"
        }
        Some(AttachProblem::FileInUse) => "inbox.attach.in_use",
        Some(AttachProblem::Forbidden) => "inbox.attach.forbidden",
        None => match e {
            cl_api::ApiError::OutcomeUnknown { .. } => "inbox.upload.unknown",
            _ => "inbox.link.failed",
        },
    }
}

/// The kind a folder's arrivals are claimed as: a bills folder's are
/// bills; anything else is asked (Q-A04n).
pub(crate) fn kind_hint(kind: Option<IntakeKind>) -> DraftKind {
    match kind {
        Some(IntakeKind::Bills) => DraftKind::Bill,
        _ => DraftKind::Unknown,
    }
}

/// What the reader is told the document is.
fn job_hint(kind: DraftKind, folder: Option<IntakeKind>) -> JobHint {
    JobHint {
        kind: match kind {
            DraftKind::Bill => Some(IntakeKind::Bills),
            DraftKind::CardCharge | DraftKind::Check | DraftKind::CashExpense => {
                Some(IntakeKind::Receipts)
            }
            DraftKind::Unknown => folder,
        },
    }
}

/// Only what the document or the reader said is stored as a failure: no
/// reader on this computer, or Cancel, leaves the draft waiting to be read.
pub(crate) fn storable(f: ReadFailure) -> bool {
    !matches!(f, ReadFailure::Unavailable | ReadFailure::Cancelled)
}

// ---------------------------------------------------------------------------
// The work in flight.

/// A claim in flight (one at a time).
enum ClaimStep {
    /// The `claimSent` entry is on its way to disk (save number `saved`).
    Saving {
        saved: u64,
        identity: FileIdentity,
        attachment: AttachmentId,
        hint: DraftKind,
        auto_read: bool,
    },
    /// Sent.
    Sending {
        identity: FileIdentity,
        auto_read: bool,
        task: Task<ClaimAnswer>,
    },
}

/// One document to read (batch lane).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ReadJob {
    draft: DocumentDraftId,
    folder: Option<IntakeKind>,
}

/// The vendors and layouts a batch reads against.
#[derive(Clone)]
struct ReadCtx {
    vendors: Vec<VendorCandidate>,
    context: ReadContext,
}

/// The fetched draft: its token and its file's bytes (`None`: too large).
type Fetched = Option<Box<(DocumentDraft, Option<Vec<u8>>)>>;

/// A follow request: the ids it asked for, and their answers.
type FollowTask = (Vec<String>, Task<Vec<(String, Followed)>>);

/// The read in flight.
enum ReadStep {
    Context(ReadJob, Task<ReadCtx>),
    Fetch(ReadJob, Task<Fetched>),
    Reading {
        job: ReadJob,
        updated_at: String,
        answer: std::sync::mpsc::Receiver<ReadOutcome>,
        cancel: crate::reading::service::ReadCancel,
    },
    Store(Task<StoreAnswer>),
}

/// How a store ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StoreAnswer {
    Stored,
    Failed,
    /// Stale or closed: the result is dropped.
    Dropped,
}

/// An attach whose answer was lost: sent again with the same key.
struct AttachResend {
    entry: FileIdentity,
    draft: DocumentDraftId,
    txn: TransactionId,
    key: IdempotencyKey,
    left: u8,
}

/// The inbox's draft work (reset with the company).
#[derive(Default)]
pub(crate) struct DraftWork {
    claim: Option<ClaimStep>,
    /// Each claim's key for the session (a retry after a lost reply
    /// reuses it; keys are never stored).
    keys: BTreeMap<String, (FileIdentity, IdempotencyKey)>,
    /// Rows whose claim was refused (by path hash): the problem shown.
    refused: BTreeMap<String, &'static str>,
    /// Files uploaded again after their upload was gone (by SHA-256).
    reuploaded: BTreeSet<String>,
    follow: Option<FollowTask>,
    follow_queue: Vec<String>,
    followed_at: Option<Instant>,
    follow_after: Option<Instant>,
    /// Drafts open in a form (F2 holds the claim).
    open: BTreeMap<String, Weak<()>>,
    queue: ReadQueue<ReadJob>,
    step: Option<ReadStep>,
    /// The batch job being read ("Reading 3 of 12").
    current: Option<BatchProgress>,
    context: Option<ReadCtx>,
    listing: Option<Task<Vec<DocumentDraftId>>>,
    /// Read all is running: drafts claimed meanwhile are read too.
    read_all: bool,
    attach_resend: Option<AttachResend>,
    /// The reading service (tests give their own; else the app's).
    pub(crate) reader: Option<Arc<ReadingService>>,
    /// Readings stored since the company opened (control channel).
    pub(crate) stored: u32,
    /// Documents that couldn't be read since the company opened.
    pub(crate) unreadable: u32,
}

impl DraftWork {
    /// A fresh one for a new company, keeping the test's reader.
    pub(crate) fn fresh(old: &DraftWork) -> DraftWork {
        DraftWork {
            reader: old.reader.clone(),
            ..DraftWork::default()
        }
    }

    pub(crate) fn busy(&self) -> bool {
        self.claim.is_some()
            || self.follow.is_some()
            || self.step.is_some()
            || self.listing.is_some()
            || !self.queue.is_empty()
    }

    /// Tests: follow at the next tick.
    #[cfg(test)]
    pub(crate) fn follow_now(&mut self) {
        self.followed_at = None;
        self.follow_after = None;
    }

    fn is_open(&self, draft: &str) -> bool {
        self.open.get(draft).is_some_and(|w| w.strong_count() > 0)
    }
}

/// The engine (or the demo) has document drafts now.
pub(crate) fn drafts_on(link: &CompanyLink) -> bool {
    if let Some(demo) = link.demo_state() {
        return demo.drafts.supported();
    }
    link.session()
        .is_some_and(|s| s.client().capabilities().has(Capabilities::DOCUMENT_DRAFTS))
}

/// The engine (or the demo) can attach a draft to a record (U-95).
pub(crate) fn attach_on(link: &CompanyLink) -> bool {
    if let Some(demo) = link.demo_state() {
        return demo.drafts.supported();
    }
    link.session().is_some_and(|s| {
        let caps = s.client().capabilities();
        caps.has(Capabilities::DOCUMENT_DRAFTS) && caps.has(Capabilities::DOCUMENT_DRAFTS_ATTACH)
    })
}

impl Intake {
    // -----------------------------------------------------------------
    // What the screen and the forms read.

    /// Whether the inbox claims its arrivals as document drafts.
    pub(crate) fn drafts_enabled(&self) -> bool {
        self.drafts_on
    }

    /// The claim problem of the file at `path_hash`, if refused.
    pub(crate) fn claim_problem(&self, path_hash: &str) -> Option<&'static str> {
        self.drafts.refused.get(path_hash).copied()
    }

    /// Read all's progress: "Reading `done` of `total`".
    pub(crate) fn read_progress(&self) -> Option<BatchProgress> {
        self.drafts.current
    }

    /// Readings stored and documents that couldn't be read this session.
    pub(crate) fn read_counts(&self) -> (u32, u32) {
        (self.drafts.stored, self.drafts.unreadable)
    }

    /// A form shows draft `draft` (F2's draft mode): batch reads skip it
    /// for as long as the form holds the returned claim (S4).
    pub(crate) fn hold_draft(&mut self, draft: DocumentDraftId) -> Claim {
        let claim = Arc::new(());
        self.drafts
            .open
            .insert(draft.to_string(), Arc::downgrade(&claim));
        claim
    }

    /// A draft's convert (or attach) succeeded elsewhere in the app and
    /// moved `attachment` onto a record (S17): its entry is handled (the
    /// original moves to `Processed/`, or is kept). False when no entry
    /// holds that upload.
    pub(crate) fn handled_by_attachment(&mut self, attachment: AttachmentId) -> bool {
        let Some(e) = self
            .state
            .by_attachment(&self.company, &attachment.to_string())
            .filter(|e| matches!(e.status, EntryStatus::Arrived | EntryStatus::Duplicate))
            .map(|e| e.file.clone())
        else {
            return false;
        };
        self.handled(&e);
        true
    }

    /// A04 WP-F2: a draft form's **Discard document** for the upload
    /// `attachment`. When this computer's inbox holds the arrival, it is
    /// discarded by the inbox's own rules (D2: an own draft in the engine
    /// first, a followed one locally only) and `Some` says how that went;
    /// `None` when no entry holds it (the form discards the draft in the
    /// engine itself; the computer that has the file moves it when it
    /// follows the draft).
    pub(crate) fn discard_by_attachment(
        &mut self,
        link: &CompanyLink,
        attachment: AttachmentId,
    ) -> Option<super::ActionResult> {
        let file = self
            .state
            .by_attachment(&self.company, &attachment.to_string())
            .filter(|e| matches!(e.status, EntryStatus::Arrived | EntryStatus::Duplicate))
            .map(|e| e.file.clone())?;
        Some(self.discard(link, &file))
    }

    /// A04 WP-F2: the draft this computer claimed for the entry, when it
    /// is still waiting (New bill / New check open it in draft mode).
    pub(crate) fn own_draft(&self, entry: &FileIdentity) -> Option<DocumentDraftId> {
        let e = self
            .state
            .entry(&self.company, entry)
            .filter(|e| e.status == EntryStatus::Arrived)?;
        match &e.doc_draft {
            DraftLink::Own(id) => DocumentDraftId::parse(id).ok(),
            _ => None,
        }
    }

    /// A04 WP-F2: a vendor's layout was learned: the next reading loads the
    /// layouts again.
    pub(crate) fn forget_read_context(&mut self) {
        self.drafts.context = None;
    }

    // -----------------------------------------------------------------
    // Claims.

    /// Starts the next claim: an arrival with no draft (or an unanswered
    /// claim), due, not refused, not on an open P17 form.
    pub(super) fn next_claim(&mut self, link: &CompanyLink, now: Instant) {
        if !self.drafts_on || self.drafts.claim.is_some() {
            return;
        }
        let pick = self
            .state
            .entries(&self.company)
            .iter()
            .filter(|e| {
                e.status == EntryStatus::Arrived
                    && matches!(e.doc_draft, DraftLink::None | DraftLink::ClaimSent)
                    && self.due(&e.file.path_hash, now)
                    && !self.drafts.refused.contains_key(&e.file.path_hash)
                    && !e.attachment().is_some_and(|a| self.on_form(a))
                    && !self.rekeying(&e.file)
            })
            .find_map(|e| {
                let a = AttachmentId::parse(e.attachment()?).ok()?;
                Some((e.clone(), a))
            });
        let Some((entry, attachment)) = pick else {
            return;
        };
        // A P17 upload shares its form's draft key: linked alone under a
        // fresh key first, so a convert moves only it (D1 "Legacy").
        if entry
            .draft
            .as_deref()
            .and_then(|d| DraftKey::parse(d).ok())
            .is_none()
        {
            self.start_rekey(link, entry.file.clone(), attachment, now);
            return;
        }
        let run = self.run_of(entry.folder);
        let folder_kind = run.map(|r| r.folder.default_kind);
        let auto_read = run.is_some_and(|r| r.folder.auto_read);
        if !self.state.begin_claim(&self.company, &entry.file) {
            return;
        }
        let hint = kind_hint(folder_kind);
        match self.save_state() {
            Some(saved) => {
                self.drafts.claim = Some(ClaimStep::Saving {
                    saved,
                    identity: entry.file,
                    attachment,
                    hint,
                    auto_read,
                });
            }
            None => self.send_claim(link, entry.file, attachment, hint, auto_read, now),
        }
    }

    fn rekeying(&self, file: &FileIdentity) -> bool {
        self.jobs
            .iter()
            .any(|j| matches!(&j.kind, JobKind::Rekey(f, _) if f == file))
    }

    fn start_rekey(
        &mut self,
        link: &CompanyLink,
        entry: FileIdentity,
        attachment: AttachmentId,
        now: Instant,
    ) {
        let draft = DraftKey::new();
        let to = cl_api::AttachTo::Draft(draft);
        if let Some(mut demo) = link.demo_state() {
            let r = crate::company::demo::checks::link_attachment(&mut demo, attachment, to);
            drop(demo);
            if r.is_err() {
                self.retry_at.insert(entry.path_hash.clone(), now + RETRY);
                return;
            }
            let task = Task::ready(JobDone::Done);
            self.jobs.push(Job {
                kind: JobKind::Rekey(entry, draft),
                task,
            });
            return;
        }
        let (Some(session), Some(group)) = (link.session(), self.group.as_ref()) else {
            return;
        };
        let key = IdempotencyKey::new();
        let label = OrphanLabel {
            what: "inbox.orphan.link",
            key: Some(key.clone()),
        };
        match group.write(&session, label, move |s| async move {
            s.attachments()
                .link(attachment, to, key)
                .await
                .map(|()| JobDone::Done)
        }) {
            Ok(p) => self.jobs.push(Job {
                kind: JobKind::Rekey(entry, draft),
                task: Task::Pending(p),
            }),
            Err(_) => {
                self.retry_at.insert(entry.path_hash.clone(), now + RETRY);
            }
        }
    }

    fn send_claim(
        &mut self,
        link: &CompanyLink,
        identity: FileIdentity,
        attachment: AttachmentId,
        hint: DraftKind,
        auto_read: bool,
        now: Instant,
    ) {
        let ours: Vec<String> = self
            .state
            .entry(&self.company, &identity)
            .map(|e| e.attachments.clone())
            .unwrap_or_default();
        // The same claim sent again this session keeps its key.
        let key = match self.drafts.keys.get(&identity.path_hash) {
            Some((same, k)) if *same == identity => k.clone(),
            _ => IdempotencyKey::new(),
        };
        self.drafts
            .keys
            .insert(identity.path_hash.clone(), (identity.clone(), key.clone()));
        if let (Some(mut demo), Some(tenant)) = (link.demo_state(), self.tenant) {
            use crate::company::demo::drafts as d;
            let first = d::create(&mut demo, tenant, attachment, hint, key);
            let answer = match first_answer(&first) {
                FirstAnswer::Done(a) => a,
                FirstAnswer::Winner(id) => winner_answer(&d::get(&demo, tenant, id), &ours),
            };
            drop(demo);
            self.drafts.claim = Some(ClaimStep::Sending {
                identity,
                auto_read,
                task: Task::ready(answer),
            });
            return;
        }
        let (Some(session), Some(group)) = (link.session(), self.group.as_ref()) else {
            self.retry_at
                .insert(identity.path_hash.clone(), now + RETRY);
            return;
        };
        let label = OrphanLabel {
            what: "inbox.orphan.claim",
            key: Some(key.clone()),
        };
        match group.write(&session, label, move |s| async move {
            let drafts = s.document_drafts();
            let first = drafts.create(attachment, hint, key).await;
            Ok(match first_answer(&first) {
                FirstAnswer::Done(a) => a,
                FirstAnswer::Winner(id) => winner_answer(&drafts.get(id).await, &ours),
            })
        }) {
            Ok(p) => {
                self.drafts.claim = Some(ClaimStep::Sending {
                    identity,
                    auto_read,
                    task: Task::Pending(p),
                });
            }
            Err(_) => {
                self.retry_at
                    .insert(identity.path_hash.clone(), now + RETRY);
            }
        }
    }

    pub(super) fn poll_claim(&mut self, link: &CompanyLink, now: Instant) {
        let Some(step) = self.drafts.claim.take() else {
            return;
        };
        match step {
            ClaimStep::Saving {
                saved,
                identity,
                attachment,
                hint,
                auto_read,
            } => {
                if self.saved(saved) {
                    self.send_claim(link, identity, attachment, hint, auto_read, now);
                } else {
                    self.drafts.claim = Some(ClaimStep::Saving {
                        saved,
                        identity,
                        attachment,
                        hint,
                        auto_read,
                    });
                }
            }
            ClaimStep::Sending {
                identity,
                auto_read,
                mut task,
            } => match task.try_take() {
                None => {
                    self.drafts.claim = Some(ClaimStep::Sending {
                        identity,
                        auto_read,
                        task,
                    });
                }
                Some(r) => {
                    let answer = r.unwrap_or(ClaimAnswer::Retry);
                    self.claim_done(link, &identity, answer, auto_read, now);
                }
            },
        }
    }

    fn claim_done(
        &mut self,
        link: &CompanyLink,
        identity: &FileIdentity,
        answer: ClaimAnswer,
        auto_read: bool,
        now: Instant,
    ) {
        tracing::debug!(answer = answer.class(), "an intake file's claim came back");
        let c = self.company.clone();
        match answer {
            ClaimAnswer::Own(id) => {
                self.drafts.keys.remove(&identity.path_hash);
                if self.state.claimed(&c, identity, &id.to_string()) {
                    self.save_state();
                    if auto_read || self.drafts.read_all {
                        self.queue_read(id, self.folder_kind_of(identity));
                    }
                }
            }
            ClaimAnswer::Elsewhere(id) => {
                self.drafts.keys.remove(&identity.path_hash);
                if self.state.claimed_elsewhere(&c, identity, &id.to_string()) {
                    self.save_state();
                    // `drive` deletes the extra copy (restart-safe).
                }
            }
            ClaimAnswer::Entered => {
                self.drafts.keys.remove(&identity.path_hash);
                self.handled(identity);
            }
            ClaimAnswer::Gone => {
                self.drafts.keys.remove(&identity.path_hash);
                let attachment = self
                    .state
                    .entry(&c, identity)
                    .and_then(|e| e.attachment().map(str::to_owned));
                if self.drafts.reuploaded.insert(identity.sha256.clone()) {
                    // Gone from the engine: the file is offered (and
                    // uploaded) again, once.
                    if let Some(a) = attachment {
                        self.state.drop_attachment(&c, &a);
                    }
                    self.state.remove(&c, identity);
                    self.save_state();
                } else {
                    self.drafts
                        .refused
                        .insert(identity.path_hash.clone(), "inbox.claim.gone");
                }
            }
            ClaimAnswer::Refused => {
                self.drafts
                    .refused
                    .insert(identity.path_hash.clone(), "inbox.claim.forbidden");
            }
            ClaimAnswer::Unsupported => {
                self.drafts_on = drafts_on(link);
            }
            ClaimAnswer::Retry => {
                self.retry_at
                    .insert(identity.path_hash.clone(), now + RETRY);
            }
        }
    }

    fn folder_kind_of(&self, identity: &FileIdentity) -> Option<IntakeKind> {
        let e = self.state.entry(&self.company, identity)?;
        self.run_of(e.folder).map(|r| r.folder.default_kind)
    }

    // -----------------------------------------------------------------
    // The extra copy of a followed draft (`DeleteCopy`).

    /// Entries that follow another computer's draft and still hold their
    /// own copy in the engine: deleted (restart-safe, from `drive`).
    pub(super) fn delete_copies(&mut self, link: &CompanyLink, now: Instant) {
        let owed: Vec<(FileIdentity, AttachmentId)> = self
            .state
            .entries(&self.company)
            .iter()
            .filter(|e| matches!(e.doc_draft, DraftLink::Follows(_)))
            .filter(|e| self.due(&e.file.path_hash, now) && !self.deleting(&e.file))
            .filter_map(|e| Some((e.file.clone(), AttachmentId::parse(e.attachment()?).ok()?)))
            .collect();
        for (file, a) in owed {
            self.start_delete_copy(link, file, a);
        }
    }

    /// Deletes this entry's own upload `a`, and only while the engine
    /// keeps it on a draft key and no live draft reads it.
    fn start_delete_copy(&mut self, link: &CompanyLink, entry: FileIdentity, a: AttachmentId) {
        // Only ever one of this entry's own uploads (never a winner's).
        let ours = self
            .state
            .entry(&self.company, &entry)
            .is_some_and(|e| e.attachments.iter().any(|x| *x == a.to_string()));
        if !ours {
            return;
        }
        if let Some(mut demo) = link.demo_state() {
            use super::super::service::{Upload, upload_of};
            let done = match crate::company::demo::checks::find_attachment(&demo, a) {
                None => JobDone::Done,
                Some(_) if demo.drafts.reader_of(a).is_some() => JobDone::InUse,
                Some(f) if upload_of(&f.attachable_type) != Upload::Draft => JobDone::OnRecord,
                Some(_) => {
                    crate::company::demo::checks::delete_attachment(&mut demo, a);
                    JobDone::Done
                }
            };
            drop(demo);
            self.jobs.push(Job {
                kind: JobKind::DeleteCopy(entry),
                task: Task::ready(done),
            });
            return;
        }
        let (Some(session), Some(group)) = (link.session(), self.group.as_ref()) else {
            return;
        };
        let key = IdempotencyKey::new();
        let label = OrphanLabel {
            what: "inbox.orphan.delete",
            key: Some(key.clone()),
        };
        if let Ok(p) = group.write(&session, label, move |s| async move {
            use super::super::service::{Upload, upload_of};
            match s.attachments().get(a).await {
                Ok(found) if upload_of(&found.attachable_type) != Upload::Draft => {
                    return Ok(JobDone::OnRecord);
                }
                Ok(_) => {}
                Err(cl_api::ApiError::NotFound { .. }) => return Ok(JobDone::Done),
                Err(e) => return Err(e),
            }
            match s.attachments().delete(a, key).await {
                Ok(()) | Err(cl_api::ApiError::NotFound { .. }) => Ok(JobDone::Done),
                // A live draft reads it after all: never deleted.
                Err(cl_api::ApiError::Conflict {
                    reason: cl_api::ConflictReason::InUse,
                    ..
                }) => Ok(JobDone::InUse),
                Err(e) => Err(e),
            }
        }) {
            self.jobs.push(Job {
                kind: JobKind::DeleteCopy(entry),
                task: Task::Pending(p),
            });
        }
    }

    // -----------------------------------------------------------------
    // Discard and Add to… on claimed entries.

    /// Discard of an arrival that owns draft `id`: the engine discards the
    /// draft first; only then does the original move and the copy go. A
    /// refusal stops there.
    pub(super) fn start_discard_draft(
        &mut self,
        link: &CompanyLink,
        entry: FileIdentity,
        id: DocumentDraftId,
    ) -> super::ActionResult {
        if self.jobs.iter().any(
            |j| matches!(&j.kind, JobKind::DiscardDraft(f) | JobKind::AttachDraft(f) if *f == entry),
        ) {
            return Err("inbox.claim.pending");
        }
        let discarded = |r: Result<DocumentDraft, cl_api::ApiError>| match r {
            Ok(_) | Err(cl_api::ApiError::NotFound { .. }) => Ok(JobDone::Done),
            Err(cl_api::ApiError::DocumentDraft(DraftRefusal::Closed { .. })) => {
                Ok(JobDone::OnRecord)
            }
            Err(e) => Err(e),
        };
        if let (Some(mut demo), Some(tenant)) = (link.demo_state(), self.tenant) {
            let r = discarded(crate::company::demo::drafts::discard(&mut demo, tenant, id));
            drop(demo);
            let task = match r {
                Ok(done) => Task::ready(done),
                Err(_) => return Err("inbox.discard.failed"),
            };
            self.jobs.push(Job {
                kind: JobKind::DiscardDraft(entry),
                task,
            });
            return Ok(());
        }
        let (Some(session), Some(group)) = (link.session(), self.group.as_ref()) else {
            return Err("inbox.discard.failed");
        };
        let label = OrphanLabel {
            what: "inbox.orphan.discard",
            key: None,
        };
        let p = group
            .write(&session, label, move |s| async move {
                discarded(s.document_drafts().discard(id).await)
            })
            .map_err(|_| "inbox.discard.failed")?;
        self.jobs.push(Job {
            kind: JobKind::DiscardDraft(entry),
            task: Task::Pending(p),
        });
        Ok(())
    }

    /// The engine discarded the entry's draft (here or elsewhere): the
    /// original moves to `Discarded/`, then this entry's own copy goes; in
    /// a folder that keeps originals only the copy goes.
    pub(super) fn draft_discarded(&mut self, link: &CompanyLink, entry: &FileIdentity) {
        let Some(e) = self.state.entry(&self.company, entry).cloned() else {
            return;
        };
        if self.keeps_originals(e.folder) {
            match e.attachment().and_then(|a| AttachmentId::parse(a).ok()) {
                Some(a) if e.status == EntryStatus::Arrived => {
                    self.start_delete(link, entry.clone(), a, true);
                }
                _ => {
                    self.state
                        .set_status(&self.company, entry, EntryStatus::Kept);
                    self.save_state();
                }
            }
            return;
        }
        self.state
            .set_status(&self.company, entry, EntryStatus::Discarding);
        self.save_state();
    }

    /// **Add to…** of an arrival that owns draft `id`: one atomic engine
    /// call (U-95) files the document on the record and closes the draft.
    /// Never a plain link.
    pub(super) fn start_attach(
        &mut self,
        link: &CompanyLink,
        entry: FileIdentity,
        id: DocumentDraftId,
        target: cl_api::AttachTo,
    ) -> super::ActionResult {
        if !attach_on(link) {
            return Err("inbox.claim.review");
        }
        let txn = target_transaction(target).ok_or("inbox.link.failed")?;
        if self.jobs.iter().any(
            |j| matches!(&j.kind, JobKind::DiscardDraft(f) | JobKind::AttachDraft(f) if *f == entry),
        ) {
            return Err("inbox.claim.pending");
        }
        let key = IdempotencyKey::new();
        self.send_attach(
            link,
            AttachResend {
                entry,
                draft: id,
                txn,
                key,
                left: ATTACH_RESENDS,
            },
        )
    }

    fn send_attach(&mut self, link: &CompanyLink, a: AttachResend) -> super::ActionResult {
        let (id, txn, key) = (a.draft, a.txn, a.key.clone());
        // Already on that record (a lost reply that committed): done.
        let attached = move |d: &DocumentDraft| {
            d.status == DraftStatus::Converted && d.converted_transaction_id == Some(txn)
        };
        if let (Some(mut demo), Some(tenant)) = (link.demo_state(), self.tenant) {
            use crate::company::demo::drafts as d;
            let r = match d::get(&demo, tenant, id) {
                Ok(draft) if attached(&draft) => Ok(JobDone::Done),
                Ok(draft) => d::attach(&mut demo, tenant, id, txn, &draft.updated_at, key)
                    .map(|_| JobDone::Done),
                Err(e) => Err(e),
            };
            drop(demo);
            let task = match r {
                Ok(done) => Task::ready(done),
                Err(e) => return Err(attach_problem_key(&e)),
            };
            self.jobs.push(Job {
                kind: JobKind::AttachDraft(a.entry.clone()),
                task,
            });
            self.drafts.attach_resend = Some(a);
            return Ok(());
        }
        let (Some(session), Some(group)) = (link.session(), self.group.as_ref()) else {
            return Err("inbox.link.failed");
        };
        let label = OrphanLabel {
            what: "inbox.orphan.attach",
            key: Some(key.clone()),
        };
        let p = group
            .write(&session, label, move |s| async move {
                let drafts = s.document_drafts();
                let draft = drafts.get(id).await?;
                if attached(&draft) {
                    return Ok(JobDone::Done);
                }
                drafts
                    .attach(id, txn, &draft.updated_at, key)
                    .await
                    .map(|_| JobDone::Done)
            })
            .map_err(|_| "inbox.link.failed")?;
        self.jobs.push(Job {
            kind: JobKind::AttachDraft(a.entry.clone()),
            task: Task::Pending(p),
        });
        self.drafts.attach_resend = Some(a);
        Ok(())
    }

    /// An attach whose answer was lost is sent again with the same key
    /// (it replays, or finds the draft on that record already).
    pub(super) fn attach_lost(&mut self, link: &CompanyLink, entry: &FileIdentity) -> bool {
        let Some(mut a) = self.drafts.attach_resend.take() else {
            return false;
        };
        if a.entry != *entry || a.left == 0 {
            return false;
        }
        a.left -= 1;
        self.send_attach(link, a).is_ok()
    }

    pub(super) fn attach_settled(&mut self, entry: &FileIdentity) {
        if self
            .drafts
            .attach_resend
            .as_ref()
            .is_some_and(|a| a.entry == *entry)
        {
            self.drafts.attach_resend = None;
        }
    }

    // -----------------------------------------------------------------
    // Follow (D6, M1).

    pub(super) fn start_follow(&mut self, link: &CompanyLink, now: Instant) {
        if !self.drafts_on || self.drafts.follow.is_some() || self.folders.is_none() {
            return;
        }
        if self.drafts.follow_after.is_some_and(|t| now < t) {
            return;
        }
        if self.drafts.follow_queue.is_empty() {
            let due = self
                .drafts
                .followed_at
                .is_none_or(|t| now.duration_since(t) >= FOLLOW_EVERY);
            if !due {
                return;
            }
            self.drafts.followed_at = Some(now);
            self.drafts.follow_queue = self
                .state
                .entries(&self.company)
                .iter()
                .filter(|e| match &e.doc_draft {
                    DraftLink::Own(_) => e.status == EntryStatus::Arrived,
                    DraftLink::Follows(_) => e.status == EntryStatus::Duplicate,
                    _ => false,
                })
                .filter_map(|e| e.doc_draft.id().map(str::to_owned))
                .collect();
        }
        if self.drafts.follow_queue.is_empty() {
            return;
        }
        let n = self.drafts.follow_queue.len().min(FOLLOW_BATCH);
        let ids: Vec<String> = self.drafts.follow_queue.drain(..n).collect();
        if let (Some(demo), Some(tenant)) = (link.demo_state(), self.tenant) {
            let mut out = Vec::new();
            for id in &ids {
                let r = DocumentDraftId::parse(id)
                    .map_err(|_| cl_api::ApiError::NotFound { code: None })
                    .and_then(|d| crate::company::demo::drafts::get(&demo, tenant, d));
                if let Ok(f) = followed(&r) {
                    out.push((id.clone(), f));
                }
            }
            drop(demo);
            self.drafts.follow = Some((ids, Task::ready(out)));
            return;
        }
        let (Some(session), Some(group)) = (link.session(), self.group.as_ref()) else {
            self.unfollow(ids, now);
            return;
        };
        let asked = ids.clone();
        match group.read(&session, move |s| async move {
            let mut out = Vec::with_capacity(asked.len());
            for id in asked {
                let Ok(d) = DocumentDraftId::parse(&id) else {
                    out.push((id, Followed::Gone));
                    continue;
                };
                let r = s.document_drafts().get(d).await;
                match followed(&r) {
                    Ok(f) => out.push((id, f)),
                    Err(()) => return Err(r.err().unwrap_or(cl_api::ApiError::Timeout)),
                }
            }
            Ok(out)
        }) {
            Ok(p) => self.drafts.follow = Some((ids, Task::Pending(p))),
            Err(_) => self.unfollow(ids, now),
        }
    }

    /// A follow request failed: its ids go back first, and the next waits.
    fn unfollow(&mut self, mut ids: Vec<String>, now: Instant) {
        ids.append(&mut self.drafts.follow_queue);
        self.drafts.follow_queue = ids;
        self.drafts.follow_after = Some(now + RETRY);
    }

    pub(super) fn poll_follow(&mut self, link: &CompanyLink, now: Instant) {
        let Some(r) = self.drafts.follow.as_mut().and_then(|(_, t)| t.try_take()) else {
            return;
        };
        let ids = self
            .drafts
            .follow
            .take()
            .map(|(i, _)| i)
            .unwrap_or_default();
        let answers = match r {
            Ok(a) => a,
            Err(e) => {
                tracing::debug!(error = %e, "followed drafts could not be read; retrying later");
                self.unfollow(ids, now);
                return;
            }
        };
        self.drafts.follow_after = None;
        for (id, f) in answers {
            self.followed_done(link, &id, f);
        }
    }

    fn followed_done(&mut self, link: &CompanyLink, id: &str, f: Followed) {
        let Some(e): Option<InboxEntry> = self
            .state
            .entries(&self.company)
            .iter()
            .find(|e| e.doc_draft.id() == Some(id))
            .cloned()
        else {
            return;
        };
        let follows = matches!(e.doc_draft, DraftLink::Follows(_));
        match f {
            Followed::Live => {}
            Followed::Entered => {
                if matches!(e.status, EntryStatus::Arrived | EntryStatus::Duplicate) {
                    self.drafts.queue.forget(id);
                    self.handled(&e.file);
                }
            }
            Followed::Discarded => {
                self.drafts.queue.forget(id);
                if follows && e.status == EntryStatus::Duplicate
                    || !follows && e.status == EntryStatus::Arrived
                {
                    // A followed draft: the other computer's is untouched;
                    // only this entry's own copy (if still there) goes.
                    self.draft_discarded(link, &e.file);
                }
            }
            Followed::Gone => {
                self.drafts.queue.forget(id);
                self.state.unclaim(&self.company, &e.file);
                self.save_state();
            }
        }
    }

    // -----------------------------------------------------------------
    // Reading: auto-read and Read all.

    fn queue_read(&mut self, draft: DocumentDraftId, folder: Option<IntakeKind>) {
        let text = draft.to_string();
        let waiting = self.drafts.queue.forget(&text);
        let running = matches!(
            &self.drafts.step,
            Some(ReadStep::Context(j, _) | ReadStep::Fetch(j, _) | ReadStep::Reading { job: j, .. })
                if j.draft == draft
        );
        if running {
            return;
        }
        let folder = folder.or_else(|| waiting.first().and_then(|q| q.job.folder));
        self.drafts
            .queue
            .push(Lane::Batch, Some(text), ReadJob { draft, folder });
    }

    /// **Read all**: every `new` draft of the company is read, oldest
    /// first, and arrivals claimed meanwhile too. False when the engine has
    /// no drafts (nothing to read) or a list is already being made.
    pub(crate) fn read_all(&mut self, link: &CompanyLink) -> bool {
        if !self.drafts_on || self.drafts.listing.is_some() {
            return false;
        }
        self.drafts.read_all = true;
        let query = |offset: u32| DraftListQuery {
            statuses: vec![DraftStatus::New],
            kind: None,
            limit: READ_ALL_PAGE,
            offset,
            summary: true,
        };
        if let (Some(demo), Some(tenant)) = (link.demo_state(), self.tenant) {
            let mut out = Vec::new();
            let mut offset = 0;
            while let Ok(page) = crate::company::demo::drafts::list(&demo, tenant, &query(offset)) {
                let n = page.drafts.len();
                out.extend(page.drafts.into_iter().map(|d| d.id));
                offset = offset.saturating_add(READ_ALL_PAGE);
                if n == 0 || out.len() >= READ_ALL_MAX || offset > cl_api::draft_limits::MAX_OFFSET
                {
                    break;
                }
            }
            drop(demo);
            self.drafts.listing = Some(Task::ready(out));
            return true;
        }
        let (Some(session), Some(group)) = (link.session(), self.group.as_ref()) else {
            return false;
        };
        match group.read(&session, move |s| async move {
            let mut out = Vec::new();
            let mut offset = 0;
            loop {
                let page = s.document_drafts().list(&query(offset)).await?;
                let n = page.drafts.len();
                out.extend(page.drafts.into_iter().map(|d| d.id));
                offset = offset.saturating_add(READ_ALL_PAGE);
                if n == 0 || out.len() >= READ_ALL_MAX || offset > cl_api::draft_limits::MAX_OFFSET
                {
                    return Ok(out);
                }
            }
        }) {
            Ok(p) => {
                self.drafts.listing = Some(Task::Pending(p));
                true
            }
            Err(_) => false,
        }
    }

    /// Cancel: the batch's waiting documents are dropped and the one being
    /// read is stopped (nothing is stored for it).
    pub(crate) fn cancel_reads(&mut self) {
        self.drafts.read_all = false;
        self.drafts.listing = None;
        self.drafts.queue.cancel_batch();
        self.drafts.current = None;
        if let Some(ReadStep::Reading { cancel, .. }) = &self.drafts.step {
            cancel.cancel();
        }
        self.drafts.step = None;
    }

    fn poll_listing(&mut self) {
        let Some(r) = self.drafts.listing.as_mut().and_then(Task::try_take) else {
            return;
        };
        self.drafts.listing = None;
        match r {
            Ok(mut ids) => {
                // Oldest first (the list is newest first).
                ids.reverse();
                let own: BTreeMap<String, Option<IntakeKind>> = self
                    .state
                    .entries(&self.company)
                    .iter()
                    .filter_map(|e| {
                        let id = e.doc_draft.id()?;
                        Some((
                            id.to_owned(),
                            self.run_of(e.folder).map(|r| r.folder.default_kind),
                        ))
                    })
                    .collect();
                for id in ids {
                    let folder = own.get(&id.to_string()).copied().flatten();
                    self.queue_read(id, folder);
                }
            }
            Err(e) => {
                tracing::debug!(error = %e, "the drafts to read could not be listed");
                self.problem = Some("inbox.read_all.failed");
                self.drafts.read_all = false;
            }
        }
    }

    /// One step of the reading pipeline (never blocks).
    pub(super) fn drive_reads(&mut self, link: &CompanyLink) {
        self.poll_listing();
        if self.drafts.step.is_none() {
            self.next_read(link);
        }
        let Some(step) = self.drafts.step.take() else {
            return;
        };
        self.drafts.step = self.advance_read(link, step);
        if self.drafts.step.is_none() && self.drafts.queue.is_empty() {
            self.drafts.current = None;
            if self.drafts.listing.is_none() {
                self.drafts.read_all = false;
                // The next batch reads the vendors again.
                self.drafts.context = None;
            }
        }
    }

    fn next_read(&mut self, link: &CompanyLink) {
        let before = self.drafts.queue.progress();
        let open: BTreeSet<String> = self
            .drafts
            .open
            .keys()
            .filter(|d| self.drafts.is_open(d))
            .cloned()
            .collect();
        let Some(q) = self.drafts.queue.next(|d| open.contains(d)) else {
            return;
        };
        self.drafts.current = before.map(|p| BatchProgress {
            done: p.done + 1,
            total: p.total,
        });
        let job = q.job;
        // No session to ask (signed out): the job is dropped, the draft
        // stays waiting to be read.
        self.drafts.step = match self.drafts.context.clone() {
            Some(_) => self.fetch(link, job.draft).map(|t| ReadStep::Fetch(job, t)),
            None => self.load_ctx(link).map(|t| ReadStep::Context(job, t)),
        };
    }

    fn load_ctx(&self, link: &CompanyLink) -> Option<Task<ReadCtx>> {
        if let Some(mut demo) = link.demo_state() {
            let chart = demo
                .chart
                .get_or_insert_with(crate::company::demo::chart)
                .clone();
            let contacts = demo.contacts_mut().clone();
            return Some(Task::ready(ReadCtx {
                vendors: crate::reading::map::vendor_candidates(&contacts),
                context: ReadContext {
                    // A04 WP-F2: the layouts learned on accept.
                    layouts: demo.drafts.layouts.clone(),
                    card_accounts: Some(card_accounts_of(&chart)),
                },
            }));
        }
        let (Some(session), Some(group)) = (link.session(), self.group.as_ref()) else {
            return None;
        };
        match group.read(&session, |s| async move {
            let contacts = s.contacts().list_all(&ContactFilter::default()).await?;
            let context = load_context(&s).await?;
            Ok(ReadCtx {
                vendors: crate::reading::map::vendor_candidates(&contacts),
                context,
            })
        }) {
            Ok(p) => Some(Task::Pending(p)),
            Err(_) => None,
        }
    }

    fn fetch(&self, link: &CompanyLink, id: DocumentDraftId) -> Option<Task<Fetched>> {
        let cap = usize::try_from(MAX_DOWNLOAD_BYTES).unwrap_or(usize::MAX);
        if let (Some(demo), Some(tenant)) = (link.demo_state(), self.tenant) {
            let out = match crate::company::demo::drafts::get(&demo, tenant, id) {
                Ok(d) if d.status == DraftStatus::New => d.attachment_id.and_then(|a| {
                    let bytes = crate::company::demo::checks::download(&demo, a).ok()?;
                    Some(Box::new((d, (bytes.len() <= cap).then_some(bytes))))
                }),
                _ => None,
            };
            return Some(Task::ready(out));
        }
        let (Some(session), Some(group)) = (link.session(), self.group.as_ref()) else {
            return None;
        };
        match group.read(&session, move |s| async move {
            let d = match s.document_drafts().get(id).await {
                Ok(d) => d,
                Err(cl_api::ApiError::NotFound { .. }) => return Ok(None),
                Err(e) => return Err(e),
            };
            if d.status != DraftStatus::New {
                return Ok(None);
            }
            let Some(a) = d.attachment_id else {
                return Ok(None);
            };
            // Over the cap: not read (the engine refuses larger files).
            match s.attachments().get(a).await {
                Ok(meta) if meta.file_size.is_some_and(|n| n > MAX_DOWNLOAD_BYTES) => {
                    return Ok(Some(Box::new((d, None))));
                }
                Ok(_) => {}
                Err(cl_api::ApiError::NotFound { .. }) => return Ok(None),
                Err(e) => return Err(e),
            }
            let bytes = match s.attachments().download(a).await {
                Ok(b) => b,
                Err(cl_api::ApiError::NotFound { .. }) => return Ok(None),
                Err(e) => return Err(e),
            };
            let fits = bytes.len() <= cap;
            Ok(Some(Box::new((d, fits.then_some(bytes)))))
        }) {
            Ok(p) => Some(Task::Pending(p)),
            Err(_) => None,
        }
    }

    fn advance_read(&mut self, link: &CompanyLink, step: ReadStep) -> Option<ReadStep> {
        match step {
            ReadStep::Context(job, mut task) => match task.try_take() {
                None => Some(ReadStep::Context(job, task)),
                Some(Ok(ctx)) => {
                    self.drafts.context = Some(ctx);
                    self.fetch(link, job.draft).map(|t| ReadStep::Fetch(job, t))
                }
                Some(Err(e)) => {
                    tracing::debug!(error = %e, "the vendors to read against could not be loaded");
                    None
                }
            },
            ReadStep::Fetch(job, mut task) => match task.try_take() {
                None => Some(ReadStep::Fetch(job, task)),
                // Read elsewhere, entered, discarded or gone: skipped.
                Some(Ok(None)) => None,
                Some(Ok(Some(fetched))) => {
                    let (draft, bytes) = *fetched;
                    self.submit_read(link, job, &draft, bytes)
                }
                Some(Err(e)) => {
                    tracing::debug!(error = %e, "a draft to read could not be fetched");
                    None
                }
            },
            ReadStep::Reading {
                job,
                updated_at,
                answer,
                cancel,
            } => match answer.try_recv() {
                Err(std::sync::mpsc::TryRecvError::Empty) => Some(ReadStep::Reading {
                    job,
                    updated_at,
                    answer,
                    cancel,
                }),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => None,
                Ok(outcome) => self.store(link, job.draft, &updated_at, &outcome),
            },
            ReadStep::Store(mut task) => match task.try_take() {
                None => Some(ReadStep::Store(task)),
                Some(Ok(StoreAnswer::Stored)) => {
                    self.drafts.stored += 1;
                    None
                }
                Some(Ok(StoreAnswer::Failed)) => {
                    self.drafts.unreadable += 1;
                    None
                }
                Some(Ok(StoreAnswer::Dropped)) => None,
                Some(Err(e)) => {
                    tracing::debug!(error = %e, "a reading could not be stored");
                    None
                }
            },
        }
    }

    fn submit_read(
        &mut self,
        link: &CompanyLink,
        job: ReadJob,
        draft: &DocumentDraft,
        bytes: Option<Vec<u8>>,
    ) -> Option<ReadStep> {
        let Some(bytes) = bytes else {
            return self.store(
                link,
                job.draft,
                &draft.updated_at,
                &ReadOutcome::Failed(ReadFailure::TooLarge),
            );
        };
        let service = self.drafts.reader.clone().or_else(crate::reading::shared)?;
        let ctx = self.drafts.context.clone()?;
        let (vendors, templates) = with_layouts(ctx.vendors, &ctx.context.layouts);
        let request = ReadRequest {
            bytes: Arc::from(bytes),
            hint: job_hint(draft.kind, job.folder),
            today: self.today.unwrap_or_else(crate::shell::today),
            vendors,
            templates,
            card_accounts: ctx.context.card_accounts,
        };
        let rj = service.submit_cancellable(request);
        Some(ReadStep::Reading {
            job,
            updated_at: draft.updated_at.clone(),
            answer: rj.answer,
            cancel: rj.cancel,
        })
    }

    fn store(
        &mut self,
        link: &CompanyLink,
        id: DocumentDraftId,
        updated_at: &str,
        outcome: &ReadOutcome,
    ) -> Option<ReadStep> {
        tracing::info!(outcome = outcome.class(), "a waiting document was read");
        let reading = match outcome {
            ReadOutcome::Read(local) => match draft_reading(local) {
                Ok(r) => Some(r),
                Err(_) => return None,
            },
            ReadOutcome::Failed(f) if storable(*f) => None,
            ReadOutcome::Failed(_) => return None,
        };
        let failure = match outcome {
            ReadOutcome::Failed(f) => f.class(),
            ReadOutcome::Read(_) => "",
        };
        let answer = |r: Result<DocumentDraft, cl_api::ApiError>, read: bool| match r {
            Ok(_) if read => Ok(StoreAnswer::Stored),
            Ok(_) => Ok(StoreAnswer::Failed),
            // Changed or closed since it was fetched: the result goes.
            Err(
                cl_api::ApiError::Conflict {
                    reason: cl_api::ConflictReason::Stale,
                    ..
                }
                | cl_api::ApiError::DocumentDraft(DraftRefusal::Closed { .. })
                | cl_api::ApiError::NotFound { .. },
            ) => Ok(StoreAnswer::Dropped),
            Err(e) => Err(e),
        };
        if let (Some(mut demo), Some(tenant)) = (link.demo_state(), self.tenant) {
            use crate::company::demo::drafts as d;
            let r = match &reading {
                Some(r) => answer(d::store_reading(&mut demo, tenant, id, updated_at, r), true),
                None => answer(
                    d::store_failure(&mut demo, tenant, id, updated_at, failure),
                    false,
                ),
            };
            drop(demo);
            return match r {
                Ok(a) => Some(ReadStep::Store(Task::ready(a))),
                Err(_) => None,
            };
        }
        let (Some(session), Some(group)) = (link.session(), self.group.as_ref()) else {
            return None;
        };
        let token = updated_at.to_owned();
        let label = OrphanLabel {
            what: "inbox.orphan.read",
            key: None,
        };
        group
            .write(&session, label, move |s| async move {
                let drafts = s.document_drafts();
                match &reading {
                    Some(r) => answer(drafts.store_reading(id, &token, r).await, true),
                    None => answer(drafts.store_failure(id, &token, failure).await, false),
                }
            })
            .ok()
            .map(|p| ReadStep::Store(Task::Pending(p)))
    }
}

#[cfg(test)]
mod unit {
    use super::*;
    use cl_api::{ApiError, AttachTo, TxnType};

    fn draft(id: &str, status: DraftStatus, file: &str) -> DocumentDraft {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "attachmentId": file,
            "kind": "bill",
            "status": status.as_str(),
            "updatedAt": "t",
        }))
        .expect("draft")
    }

    const D1: &str = "d0000000-0000-4000-8000-0000000dd001";
    const OURS: &str = "d0000000-0000-4000-8000-000000000a01";
    const THEIRS: &str = "d0000000-0000-4000-8000-000000000b01";

    fn id() -> DocumentDraftId {
        DocumentDraftId::parse(D1).expect("id")
    }

    /// Every row of the D1 claim table (T4).
    #[test]
    fn every_claim_answer() {
        let ok = Ok(draft(D1, DraftStatus::New, OURS));
        assert!(matches!(first_answer(&ok), FirstAnswer::Done(ClaimAnswer::Own(d)) if d == id()));
        let claimed = Err(ApiError::DocumentDraft(DraftRefusal::AlreadyClaimed {
            draft: Some(id()),
        }));
        assert!(matches!(first_answer(&claimed), FirstAnswer::Winner(d) if d == id()));
        let nameless = Err(ApiError::DocumentDraft(DraftRefusal::AlreadyClaimed {
            draft: None,
        }));
        let not_draft = Err(ApiError::DocumentDraft(DraftRefusal::NotADraftFile));
        let gone = Err(ApiError::NotFound { code: None });
        let unsupported = Err(ApiError::Local(cl_api::LocalValidation::Unsupported));
        let down = Err(ApiError::Timeout);
        for (r, want) in [
            (nameless, ClaimAnswer::Retry),
            (not_draft, ClaimAnswer::Entered),
            (gone, ClaimAnswer::Gone),
            (unsupported, ClaimAnswer::Unsupported),
            (down, ClaimAnswer::Retry),
        ] {
            assert!(
                matches!(first_answer(&r), FirstAnswer::Done(a) if a == want),
                "{want:?}"
            );
        }
        let ours = vec![OURS.to_string()];
        // Adopted only from this entry's own uploads.
        let mine = Ok(draft(D1, DraftStatus::Ready, OURS));
        assert_eq!(winner_answer(&mine, &ours), ClaimAnswer::Own(id()));
        let theirs = Ok(draft(D1, DraftStatus::New, THEIRS));
        assert_eq!(winner_answer(&theirs, &ours), ClaimAnswer::Elsewhere(id()));
        assert_eq!(
            winner_answer(&theirs, &[]),
            ClaimAnswer::Elsewhere(id()),
            "no uploads: never adopted"
        );
        let entered = Ok(draft(D1, DraftStatus::Converted, THEIRS));
        assert_eq!(winner_answer(&entered, &ours), ClaimAnswer::Entered);
        let discarded = Ok(draft(D1, DraftStatus::Discarded, THEIRS));
        assert_eq!(winner_answer(&discarded, &ours), ClaimAnswer::Retry);
        assert_eq!(
            winner_answer(&Err(ApiError::NotFound { code: None }), &ours),
            ClaimAnswer::Retry
        );
        assert_eq!(ClaimAnswer::Own(id()).class(), "own");
    }

    #[test]
    fn follow_targets_and_hints() {
        assert_eq!(
            followed(&Ok(draft(D1, DraftStatus::Converted, OURS))),
            Ok(Followed::Entered)
        );
        assert_eq!(
            followed(&Ok(draft(D1, DraftStatus::Discarded, OURS))),
            Ok(Followed::Discarded)
        );
        assert_eq!(
            followed(&Ok(draft(D1, DraftStatus::Failed, OURS))),
            Ok(Followed::Live)
        );
        assert_eq!(
            followed(&Err(ApiError::NotFound { code: None })),
            Ok(Followed::Gone)
        );
        assert_eq!(followed(&Err(ApiError::Timeout)), Err(()));
        let t = TransactionId::parse("d0000000-0000-4000-8000-0000000000f1").expect("t");
        assert_eq!(target_transaction(AttachTo::Bill(t)), Some(t));
        assert_eq!(
            target_transaction(AttachTo::Txn(TxnType::Deposit, t)),
            Some(t)
        );
        assert_eq!(target_transaction(AttachTo::Draft(DraftKey::new())), None);
        assert_eq!(kind_hint(Some(IntakeKind::Bills)), DraftKind::Bill);
        assert_eq!(kind_hint(Some(IntakeKind::Receipts)), DraftKind::Unknown);
        assert_eq!(kind_hint(None), DraftKind::Unknown);
        assert_eq!(
            job_hint(DraftKind::CardCharge, None).kind,
            Some(IntakeKind::Receipts)
        );
        assert_eq!(
            job_hint(DraftKind::Unknown, Some(IntakeKind::Bills)).kind,
            Some(IntakeKind::Bills)
        );
        // No reader and Cancel are never stored as a failure.
        assert!(!storable(ReadFailure::Unavailable));
        assert!(!storable(ReadFailure::Cancelled));
        assert!(storable(ReadFailure::Unreadable));
        assert_eq!(
            attach_problem_key(&ApiError::NotFound { code: None }),
            "inbox.attach.not_found"
        );
    }
}
