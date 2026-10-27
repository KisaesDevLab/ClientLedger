//! **Read a statement** on Match Vendor Statement (A06, ADD-02 steps 3-7,
//! plan decisions b-g).
//!
//! - **The file.** Browse, a file dropped on the window, or a statement
//!   draft opened from Review Documents (U-94). The bytes are checked as an
//!   attachment's are (10 MB, PDF / PNG / JPEG / TIFF) and read in the
//!   sandboxed worker as a statement job (`JobHint::statement`, up to 20
//!   pages, Q-300). Nothing is parsed here and nothing goes online.
//! - **The lines** go through the paste path
//!   (`cl_model::statement_match::rows_from_read`): the box holds them as
//!   typed lines would be, each marked "Read from the document — check
//!   it" until accepted (one Accept for all, or line by line). Match waits
//!   for the accept. Typing in the box makes the lines the owner's.
//! - **The vendor** is matched against the company's vendors. One already
//!   picked that differs is never changed: a warning says so.
//! - **The document pane** shows the statement beside its lines; picking a
//!   line outlines its spot on the page, and clicking a spot picks the line
//!   (the A04 compare tools, with the read lines as targets).
//! - **Checks** the reader raised (lines vs. balances, running balances,
//!   dates, duplicates) are listed; nothing is adjusted (non-negotiable 6).
//! - **Ask AI to help** as in A03, behind the same switches and consent;
//!   it may name the vendor and the closing balance, and an AI amount is
//!   kept only when the same figure is printed on the page (Q-A03j). Both
//!   wait as suggestions until the owner uses them (non-negotiable 4,
//!   review S4): the vendor picker, the balance line and the keep never
//!   see them before. The lines are never the AI's.
//! - **Balance line** (Q-297): the statement's closing balance beside the
//!   books' (`/reports/vendor-statement`, exact), display only.
//! - **Keep the statement** (Q-298): after Match, on by default, the file
//!   is stored on the vendor with its period in the name. For a statement
//!   draft the file is uploaded to the vendor first, then the draft is
//!   discarded and its own copy deleted (review S1): a failure at any step
//!   leaves the file findable, on the vendor or still in Review Documents.
//!   The engine refuses to move a file a live draft reads, so moving it
//!   would have to discard first and could strand it.
//!
//! Logs carry outcome classes only; the control channel counts only.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};

use cl_api::{
    ApiError, AssistSuggestion, AttachTo, AttachmentId, DocumentDraftId, DraftKind, FileKind,
    IdempotencyKey, OrphanLabel, VendorStatementBalances,
};
use cl_core::ids::{ContactId, TenantId};
use cl_core::{CalendarDate, Money, MoneyStyle};
use cl_docread::model::{
    Check, CheckCode, ConfidenceBucket, Found, Header, SignSource, StatementRead, Template,
};
use cl_docread::protocol::JobHint;
use cl_docread::vendor_match::{self, VendorCandidate};
use cl_model::rowkey::RowKey;
use cl_model::statement_match::{
    LineColumn, PastedRow, ReadLine, ReadStatement, StatementKind, rows_from_read,
};
use eframe::egui::{self, RichText, Ui};
use serde_json::{Value, json};

use super::super::doc_read::ReadSwitches;
use super::StatementMatchScreen;
use crate::company::Task;
use crate::i18n::{t, t_with};
use crate::reading::assist::{self, NoRoute, Route};
use crate::reading::{
    LocalReading, ReadCancel, ReadFailure, ReadOutcome, ReadRequest, ReadingService,
};
use crate::screens::company::form::{self, DateInput};
use crate::screens::write_check::attach::{
    FileRead, poll_file_read, read_file, read_in_background,
};
use crate::theme::Tokens;
use crate::widgets::compare::{
    CompareAction, CompareEnv, CompareEvent, CompareTarget, CompareTools, PageBox, TargetKey,
    TargetLevel,
};
use crate::widgets::doc_pane::DocPane;
use crate::widgets::pdf_viewer::{DocSource, ViewerState};

/// The field name of a read line's target (`statement_line:3`).
pub(crate) const LINE_TARGET: &str = "statement_line";

/// "Couldn't read this statement — type or paste the lines instead."
pub(crate) const COULDNT: &str = "statement.read.couldnt";

/// The height of the docked document pane.
const PANE_HEIGHT: f32 = 420.0;

/// A statement draft opened from Review Documents (U-94).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct StatementDraft {
    /// The draft.
    pub id: DocumentDraftId,
    /// Its file.
    pub attachment: Option<AttachmentId>,
}

/// What a kept statement was kept for: one file per vendor and period.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct KeepKey {
    vendor: ContactId,
    from: CalendarDate,
    to: CalendarDate,
    doc: u64,
}

/// The idempotency keys of one keep.
#[derive(Clone)]
pub(crate) struct KeepKeys {
    upload: IdempotencyKey,
    delete: IdempotencyKey,
}

/// What the books' balance was asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BalanceKey {
    vendor: ContactId,
    from: CalendarDate,
    to: CalendarDate,
}

/// The statement document and its reading (owner data: shown, never
/// logged; `Debug` is not derived).
pub(crate) struct StatementDoc {
    /// The document pane.
    pub viewer: ViewerState,
    /// Line ↔ spot highlights.
    pub compare: CompareTools,
    /// The document's bytes and kind.
    pub bytes: Option<Arc<[u8]>>,
    pub file_kind: Option<FileKind>,
    /// Bumped for every new document (keep once per document).
    pub doc_no: u64,
    /// The Browse window.
    pub picking: Option<Receiver<Vec<PathBuf>>>,
    /// A file path typed in (owner data; never logged).
    pub path_text: String,
    /// A file being read from disk.
    pub loading: Option<Receiver<FileRead>>,
    /// The reading in the worker.
    pub job: Option<Receiver<ReadOutcome>>,
    pub cancel: Option<ReadCancel>,
    /// A service set by tests instead of the app's.
    pub service: Option<Arc<ReadingService>>,
    /// The company's statement layouts (`stmt:`, Q-299), for the reader.
    pub layouts: Vec<(ContactId, Template)>,
    pub layouts_task: Option<Task<Vec<(ContactId, Template)>>>,
    /// Learning the vendor's statement layout on accept (class only).
    pub learn_task: Option<Task<()>>,
    pub learned: Option<&'static str>,
    /// The last reading, with its page words.
    pub last: Option<Box<LocalReading>>,
    /// The read lines in the box, with their marks.
    pub read: Option<ReadStatement>,
    /// Each mark's line as the paste reader reads it back, by mark: read
    /// once per reading, not every frame (review S7).
    pub read_rows: Vec<Option<PastedRow>>,
    /// What the compare targets were last published for (pending lines,
    /// still as read): unchanged, they aren't built again.
    pub published: Option<(usize, bool)>,
    /// The read lines' spots on the page, by read line.
    pub spots: Vec<Option<PageBox>>,
    /// The read line picked (its spot outlined).
    pub selected: Option<usize>,
    /// Why the last attempt said nothing useful (i18n key).
    pub problem: Option<&'static str>,
    /// The company vendor the statement names (the reader's match, or an
    /// AI suggestion the owner accepted). The wrong-vendor warning, the
    /// keep and the layout learning all compare the picked vendor with it,
    /// every time (review B2).
    pub read_vendor: Option<ContactId>,
    /// The closing balance printed on the statement.
    pub closing: Option<Money>,
    /// The closing balance came from AI help (checked against the page).
    pub closing_by_ai: bool,
    /// A vendor AI help named, waiting for the owner's "Use this vendor".
    pub suggested_vendor: Option<ContactId>,
    /// A closing balance found after Ask AI to help, waiting for the
    /// owner's "Use this balance"; `true` when the AI's (not the reader's).
    pub suggested_closing: Option<(Money, bool)>,
    /// The statement's date (else the period's end).
    pub statement_date: Option<CalendarDate>,
    /// The reader's checks on the statement (shown, never applied).
    pub checks: Vec<Check>,
    /// Statements sent to the reader; the last outcome class.
    pub sent: u32,
    pub outcome: Option<&'static str>,
    /// Opened from Review Documents.
    pub draft: Option<StatementDraft>,
    /// The draft's file being fetched.
    pub draft_file: Option<Task<(cl_api::DocumentDraft, Vec<u8>)>>,
    /// The tenant (the demo engine keys drafts by it).
    pub tenant: Option<TenantId>,
    // ---- Ask AI to help
    pub switches: Option<ReadSwitches>,
    pub switches_task: Option<Task<ReadSwitches>>,
    pub assist: Option<Task<Result<AssistSuggestion, ApiError>>>,
    pub assist_sent: u32,
    pub assist_outcome: Option<&'static str>,
    pub helped: bool,
    // ---- Keep the statement
    /// "Save this statement with the vendor" (default on).
    pub keep: bool,
    pub keep_task: Option<(KeepKey, Task<()>)>,
    /// The keys of a keep, made once per vendor, period and document (S1).
    /// Set from the first attempt on, so Try again knows it is one: it
    /// first looks for the file already on the vendor (an upload that
    /// timed out but was saved; the engine doesn't replay uploads by key).
    pub keep_keys: Option<(KeepKey, KeepKeys)>,
    /// The demo engine has the file on the vendor for this keep already
    /// (its upload isn't keyed).
    pub keep_uploaded: Option<KeepKey>,
    pub kept: Option<KeepKey>,
    pub keep_failed: bool,
    pub keeps: u32,
    // ---- The balance line
    pub balance_task: Option<(BalanceKey, Task<Option<VendorStatementBalances>>)>,
    pub books: Option<(BalanceKey, VendorStatementBalances)>,
}

impl Default for StatementDoc {
    fn default() -> Self {
        StatementDoc {
            viewer: ViewerState::default(),
            compare: CompareTools::new(),
            bytes: None,
            file_kind: None,
            doc_no: 0,
            picking: None,
            path_text: String::new(),
            loading: None,
            job: None,
            cancel: None,
            service: None,
            layouts: Vec::new(),
            layouts_task: None,
            learn_task: None,
            learned: None,
            last: None,
            read: None,
            read_rows: Vec::new(),
            published: None,
            spots: Vec::new(),
            selected: None,
            problem: None,
            read_vendor: None,
            closing: None,
            closing_by_ai: false,
            suggested_vendor: None,
            suggested_closing: None,
            statement_date: None,
            checks: Vec::new(),
            sent: 0,
            outcome: None,
            draft: None,
            draft_file: None,
            tenant: None,
            switches: None,
            switches_task: None,
            assist: None,
            assist_sent: 0,
            assist_outcome: None,
            helped: false,
            keep: true,
            keep_task: None,
            keep_keys: None,
            keep_uploaded: None,
            kept: None,
            keep_failed: false,
            keeps: 0,
            balance_task: None,
            books: None,
        }
    }
}

/// A closed window stops a reading still running.
impl Drop for StatementDoc {
    fn drop(&mut self) {
        if let Some(c) = self.cancel.take() {
            c.cancel();
        }
    }
}

impl StatementDoc {
    /// Reading, fetching or helping.
    pub(crate) fn busy(&self) -> bool {
        self.job.is_some()
            || self.loading.is_some()
            || self.assist.is_some()
            || self.draft_file.is_some()
    }

    /// Forget the reading (a new document, or Clear).
    fn forget_reading(&mut self) {
        if let Some(c) = self.cancel.take() {
            c.cancel();
        }
        self.job = None;
        self.last = None;
        self.read = None;
        self.read_rows.clear();
        self.published = None;
        self.spots.clear();
        self.selected = None;
        self.problem = None;
        self.read_vendor = None;
        self.closing = None;
        self.closing_by_ai = false;
        self.suggested_vendor = None;
        self.suggested_closing = None;
        self.statement_date = None;
        self.checks.clear();
        self.assist = None;
        self.helped = false;
        self.assist_outcome = None;
        self.books = None;
        self.balance_task = None;
        self.learned = None;
        self.learn_task = None;
        self.compare.clear();
        self.compare.set_targets(Vec::new(), Vec::new());
        self.compare.set_words(None);
    }

    /// Everything goes (Clear).
    pub(crate) fn reset(&mut self) {
        self.forget_reading();
        self.viewer.clear();
        self.bytes = None;
        self.file_kind = None;
        self.picking = None;
        self.path_text.clear();
        self.loading = None;
        self.draft = None;
        self.draft_file = None;
        self.keep = true;
        self.keep_task = None;
        self.keep_keys = None;
        self.keep_uploaded = None;
        self.kept = None;
        self.keep_failed = false;
    }

    /// The read lines still need the owner's Accept (and the box still
    /// holds them as read).
    pub(crate) fn awaiting_accept(&self, box_text: &str) -> bool {
        self.read
            .as_ref()
            .is_some_and(|r| r.still_read(box_text) && !r.all_accepted())
    }

    /// The control channel's view: counts and flags only.
    pub(crate) fn state_json(&self, box_text: &str, picked: Option<ContactId>) -> Value {
        let read = self.read.as_ref();
        json!({
            "document": self.bytes.is_some(),
            "busy": self.busy(),
            "requests": self.sent,
            "outcome": self.outcome,
            "problem": self.problem,
            "lines": read.map(|r| r.marks.len()),
            "pending": read.map(ReadStatement::pending),
            "unsure": read.map(|r| r.marks.iter().filter(|m| m.unsure).count()),
            "typed_over": read.is_some_and(|r| !r.still_read(box_text)),
            "spots": self.spots.iter().filter(|s| s.is_some()).count(),
            "selected": self.selected,
            "vendor_differs": self.differs_from(picked),
            "closing_read": self.closing.is_some(),
            "closing_by_ai": self.closing_by_ai,
            "vendor_suggested": self.suggested_vendor.is_some(),
            "closing_suggested": self.suggested_closing.is_some(),
            "checks": self.checks.iter().map(|c| crate::screens::bill::doc_read_local::code_name(c.code)).collect::<Vec<_>>(),
            "draft": self.draft.is_some(),
            "layouts": self.layouts.len(),
            "learned": self.learned,
            "assist": {
                "offer": self.offer().key(),
                "requests": self.assist_sent,
                "outcome": self.assist_outcome,
                "helped": self.helped,
            },
            "keep": self.keep,
            "kept": self.kept.is_some(),
            "keeps": self.keeps,
            "keep_failed": self.keep_failed,
            "keeping": self.keep_task.is_some(),
            "viewer": self.viewer.state_json(),
        })
    }

    /// The statement names a company vendor other than `picked`.
    pub(crate) fn differs_from(&self, picked: Option<ContactId>) -> bool {
        self.read_vendor.is_some_and(|v| picked != Some(v))
    }

    /// What "Ask AI to help" offers now.
    pub(crate) fn offer(&self) -> super::super::doc_read_local::Offer {
        use super::super::doc_read_local::Offer;
        let Some(last) = self.last.as_ref() else {
            return Offer::None;
        };
        if self.helped
            || self.busy()
            || !needs_help(
                last,
                self.closing.is_some() || self.suggested_closing.is_some(),
            )
        {
            return Offer::None;
        }
        let Some(s) = self.switches else {
            return Offer::None;
        };
        match assist::route(s.assist_switches()) {
            Ok(r) => Offer::Ask(r),
            Err(NoRoute::NeedsConsent) => Offer::NeedsConsent,
            Err(NoRoute::Off | NoRoute::Unsupported) => Offer::None,
        }
    }
}

/// The reading left the vendor or the closing balance out.
fn needs_help(local: &LocalReading, closing: bool) -> bool {
    local.vendor.is_none() || !closing
}

fn unsure<T>(f: Option<&Found<T>>) -> bool {
    f.is_some_and(|f| f.confidence.bucket() == ConfidenceBucket::Low)
}

/// The column a read line's amount came from, in the model's words.
fn column_of(s: SignSource) -> LineColumn {
    match s {
        SignSource::SingleColumn => LineColumn::Single,
        SignSource::ChargesColumn => LineColumn::Charges,
        SignSource::CreditsColumn => LineColumn::Credits,
        SignSource::PaymentsColumn => LineColumn::Payments,
    }
}

/// The reader's lines as the model's.
#[must_use]
pub(crate) fn read_lines(st: &StatementRead) -> Vec<ReadLine> {
    st.lines
        .iter()
        .map(|l| ReadLine {
            date: l.date.value,
            reference: l
                .reference
                .as_ref()
                .map(|r| r.value.clone())
                .unwrap_or_default(),
            amount: l.amount.value,
            column: column_of(l.sign_from),
            unsure: unsure(Some(&l.date))
                || unsure(Some(&l.amount))
                || unsure(l.reference.as_ref()),
        })
        .collect()
}

/// Each mark's line read back as the paste reader reads it (by mark).
fn read_rows_of(read: &ReadStatement, today: CalendarDate) -> Vec<Option<PastedRow>> {
    let parsed = cl_model::statement_match::read::read_back(read, today);
    let mut by_line: Vec<Option<PastedRow>> = vec![None; read.marks.len() + 1];
    for r in parsed.rows {
        if let Some(slot) = by_line.get_mut(r.line) {
            *slot = Some(r);
        }
    }
    read.marks
        .iter()
        .map(|m| by_line.get_mut(m.line).and_then(Option::take))
        .collect()
}

/// A spot as a compare box.
fn spot_box(s: cl_docread::model::Spot) -> Option<PageBox> {
    let b = s.bbox;
    PageBox::new(s.page, b.left(), b.top(), b.right(), b.bottom())
}

/// One box over a read line: its date to its amount when both are on one
/// page, else the amount's.
fn line_box(l: &cl_docread::model::StatementLineRead) -> Option<PageBox> {
    let (a, d) = (spot_box(l.amount.spot)?, spot_box(l.date.spot));
    match d {
        Some(d) if d.page == a.page => PageBox::new(
            a.page,
            a.l.min(d.l),
            a.t.min(d.t),
            a.r.max(d.r),
            a.b.max(d.b),
        ),
        _ => Some(a),
    }
}

/// The statement checks the owner is shown (never applied).
fn is_statement_check(code: CheckCode) -> bool {
    matches!(
        code,
        CheckCode::StmtSum
            | CheckCode::StmtRunning
            | CheckCode::StmtDateOutsidePeriod
            | CheckCode::StmtDuplicateLine
            | CheckCode::PagesSkipped
            | CheckCode::PageNotRead
    )
}

/// What a statement check says (owner text; shown, never logged).
pub(crate) fn check_text(c: &Check, style: MoneyStyle) -> String {
    let line = c
        .line
        .map_or_else(String::new, |l| (u32::from(l) + 1).to_string());
    match c.code {
        CheckCode::StmtSum => match c.difference {
            Some(d) => t_with(
                "statement.read.check.sum",
                &[("difference", &d.abs().format(style))],
            ),
            None => t("statement.read.check.sum.plain"),
        },
        CheckCode::StmtRunning => t_with("statement.read.check.running", &[("line", &line)]),
        CheckCode::StmtDateOutsidePeriod => t_with("statement.read.check.date", &[("line", &line)]),
        CheckCode::StmtDuplicateLine => {
            t_with("statement.read.check.duplicate", &[("line", &line)])
        }
        _ => super::super::doc_read_local::check_text(c, style),
    }
}

/// The candidates' vendor for a name printed on a statement.
fn match_name(
    name: Option<&Found<String>>,
    reading: &cl_docread::model::Reading,
    vendors: &[VendorCandidate],
) -> Option<ContactId> {
    let header = Header {
        vendor: name.cloned(),
        ..Header::default()
    };
    vendor_match::match_vendor(&header, reading.fingerprint.as_ref(), vendors).map(|m| m.contact)
}

impl StatementMatchScreen {
    // ------------------------------------------------------------ the file

    /// Browse for a statement file.
    pub(crate) fn browse_statement(&mut self, ctx: &egui::Context) {
        if self.doc.picking.is_none() {
            self.doc.picking = Some(crate::screens::write_check::attach::browse_for_file(ctx));
        }
    }

    /// A statement file at `path` (Browse or dropped): read from disk in
    /// the background, then read.
    pub(crate) fn statement_path(&mut self, path: &std::path::Path) {
        match read_in_background(path, read_file) {
            Ok(rx) => self.doc.loading = Some(rx),
            Err(k) => self.doc.problem = Some(k),
        }
    }

    /// A statement's bytes (from disk, a draft, or tests): shown and read.
    pub(crate) fn statement_bytes(&mut self, name: &str, bytes: Vec<u8>) {
        let Ok(kind) = cl_api::check_file(&bytes) else {
            self.doc.problem = Some("check.attach.type");
            return;
        };
        self.doc.forget_reading();
        // A Match made (or running) for the last document is not this
        // one's: it never keeps this file (review B2).
        self.outcome = None;
        self.reading = None;
        self.books_failed = None;
        self.doc.doc_no += 1;
        self.doc.kept = None;
        self.doc.keep_failed = false;
        let bytes: Arc<[u8]> = Arc::from(bytes);
        self.doc.bytes = Some(Arc::clone(&bytes));
        self.doc.file_kind = Some(kind);
        self.doc.viewer.open(DocSource {
            name: name.to_string(),
            bytes,
        });
        self.start_statement_read();
    }

    /// Files dropped on the window.
    fn take_dropped(&mut self, ui: &Ui) {
        let dropped = ui.ctx().input(|i| i.raw.dropped_files.clone());
        if let Some(f) = dropped.into_iter().next() {
            let p = f.path().to_path_buf();
            self.statement_path(&p);
        }
    }

    // --------------------------------------------------------- the reading

    /// Sends the shown statement to the reader.
    pub(crate) fn start_statement_read(&mut self) {
        let Some(bytes) = self.doc.bytes.clone() else {
            return;
        };
        let service = self.doc.service.clone().or_else(crate::reading::shared);
        let Some(service) = service else {
            self.doc.problem = Some(COULDNT);
            self.doc.outcome = Some(ReadFailure::Unavailable.class());
            return;
        };
        if let Some(demo) = self.link.demo_state() {
            self.doc.layouts.clone_from(&demo.drafts.statement_layouts);
        }
        let (vendors, templates) = crate::reading::service::with_layouts(
            crate::reading::map::vendor_candidates(&self.contacts),
            &self.doc.layouts,
        );
        let request = ReadRequest {
            bytes,
            hint: JobHint::statement(),
            today: self.today,
            vendors,
            templates,
            card_accounts: None,
        };
        self.doc.sent += 1;
        self.doc.outcome = None;
        self.doc.problem = None;
        tracing::info!(job = self.doc.sent, "statement reading started");
        let job = service.submit_cancellable(request);
        self.doc.job = Some(job.answer);
        self.doc.cancel = Some(job.cancel);
    }

    fn take_outcome(&mut self) -> Option<ReadOutcome> {
        let rx = self.doc.job.as_ref()?;
        let outcome = match rx.try_recv() {
            Ok(o) => o,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => ReadOutcome::Failed(ReadFailure::Unavailable),
        };
        self.doc.job = None;
        self.doc.cancel = None;
        Some(outcome)
    }

    /// Each frame: the file, the reading, the help, the keep and the
    /// balance, when they answer.
    pub(crate) fn poll_statement(&mut self) {
        if let Some(paths) = crate::widgets::file_picker::poll(&mut self.doc.picking)
            && let Some(p) = paths.into_iter().next()
        {
            self.statement_path(&p);
        }
        if let Some(r) = poll_file_read(&mut self.doc.loading) {
            match r {
                Ok((name, bytes)) => self.statement_bytes(&name, bytes),
                Err(k) => self.doc.problem = Some(k),
            }
        }
        if let Some(r) = self.doc.draft_file.as_mut().and_then(Task::try_take) {
            self.doc.draft_file = None;
            match r {
                Ok((draft, bytes)) => {
                    self.doc.draft = Some(StatementDraft {
                        id: draft.id,
                        attachment: draft.attachment_id,
                    });
                    self.statement_bytes(&t("statement.read.draft_file"), bytes);
                }
                Err(_) => self.doc.problem = Some("statement.read.draft_failed"),
            }
        }
        if let Some(outcome) = self.take_outcome() {
            self.doc.outcome = Some(outcome.class());
            match outcome {
                ReadOutcome::Read(local) => self.fill_statement(local),
                ReadOutcome::Failed(ReadFailure::Cancelled) => {}
                ReadOutcome::Failed(f) => {
                    tracing::info!(
                        job = self.doc.sent,
                        outcome = f.class(),
                        "statement reading failed"
                    );
                    self.couldnt_read();
                }
            }
        }
        if let Some(r) = self.doc.layouts_task.as_mut().and_then(Task::try_take) {
            self.doc.layouts_task = None;
            self.doc.layouts = r.unwrap_or_default();
        }
        if let Some(r) = self.doc.learn_task.as_mut().and_then(Task::try_take) {
            self.doc.learn_task = None;
            self.doc.learned = Some(if r.is_ok() { "stored" } else { "failed" });
        }
        self.poll_statement_switches();
        self.poll_statement_assist();
        self.poll_keep();
        self.poll_balance();
    }

    /// A finished reading into the form: the lines into the box (read, not
    /// accepted), the vendor and the period when known.
    pub(crate) fn fill_statement(&mut self, local: Box<LocalReading>) {
        let Some(st) = local
            .reading
            .statement
            .clone()
            .filter(|s| !s.lines.is_empty())
        else {
            tracing::info!(
                job = self.doc.sent,
                outcome = "no_lines",
                "statement reading"
            );
            self.couldnt_read();
            self.doc.last = Some(local);
            return;
        };
        let lines = read_lines(&st);
        let read = rows_from_read(&lines);
        self.text.clone_from(&read.text);
        self.parsed = None;
        self.outcome = None;
        self.problem = None;
        self.doc.read_rows = read_rows_of(&read, self.today);
        self.doc.published = None;
        self.doc.read = Some(read);
        self.doc.spots = st.lines.iter().map(line_box).collect();
        self.doc.closing = st.closing.as_ref().map(|c| c.value);
        self.doc.statement_date = st
            .statement_date
            .as_ref()
            .or(st.period_to.as_ref())
            .map(|d| d.value);
        if let Some(d) = st.period_from.as_ref() {
            self.from = DateInput::new(d.value);
        }
        if let Some(d) = st.period_to.as_ref().or(st.statement_date.as_ref()) {
            self.to = DateInput::new(d.value);
        }
        // Only a statement date before the shown From: the period is that
        // date's month, never From after To (review, low).
        if let (Some(from), Some(to)) = (self.from.value, self.to.value)
            && from > to
        {
            self.from = DateInput::new(to.start_of_month());
        }
        self.doc.checks = local
            .reading
            .checks
            .iter()
            .filter(|c| is_statement_check(c.code))
            .cloned()
            .collect();
        let found = local.vendor.map(|v| v.contact).or_else(|| {
            match_name(
                st.vendor.as_ref(),
                &local.reading,
                &crate::reading::map::vendor_candidates(&self.contacts),
            )
        });
        self.take_vendor(found);
        self.doc
            .compare
            .set_words(Some(Arc::new(crate::widgets::compare::DocWords {
                pages: local.reading.words.clone(),
            })));
        self.doc.last = Some(local);
        self.ensure_statement_switches();
    }

    /// Nothing was read: the message says type or paste, and the pane
    /// closes (no blank page; A06 UI review). The file itself stays, as
    /// typed lines may still be matched and kept with it.
    fn couldnt_read(&mut self) {
        self.doc.problem = Some(COULDNT);
        self.doc.viewer.clear();
        self.doc.compare.clear();
    }

    /// The vendor the statement names: picked when none is; a different
    /// one already picked stays, and the warning shows while they differ.
    fn take_vendor(&mut self, found: Option<ContactId>) {
        let Some(found) = found.filter(|f| self.vendors.iter().any(|(id, _)| id == f)) else {
            return;
        };
        self.doc.read_vendor = Some(found);
        if self.vendor.is_none() {
            self.vendor = Some(found);
        }
    }

    /// The statement names another company vendor than the one picked now.
    pub(crate) fn vendor_differs(&self) -> bool {
        self.doc.differs_from(self.vendor)
    }

    /// The name of the vendor the statement names, for the warning.
    fn read_vendor_name(&self) -> Option<String> {
        let v = self.doc.read_vendor?;
        self.vendors
            .iter()
            .find(|(id, _)| *id == v)
            .map(|(_, n)| n.clone())
    }

    /// Accept every read line.
    pub(crate) fn accept_read_lines(&mut self) {
        if let Some(r) = self.doc.read.as_mut() {
            r.accept_all();
        }
        self.learn_on_accept();
    }

    /// Accept the read line on box line `line`.
    pub(crate) fn accept_read_line(&mut self, line: usize) {
        if let Some(r) = self.doc.read.as_mut() {
            r.accept(line);
        }
        self.learn_on_accept();
    }

    /// Every line accepted: the vendor's statement layout learns this
    /// document (Q-299, `stmt:` key), once per reading.
    fn learn_on_accept(&mut self) {
        // Never under a vendor the statement doesn't name (review S3).
        if self.doc.learned.is_some()
            || self.vendor_differs()
            || !self
                .doc
                .read
                .as_ref()
                .is_some_and(ReadStatement::all_accepted)
        {
            return;
        }
        let (Some(vendor), Some(last)) = (self.vendor, self.doc.last.as_ref()) else {
            return;
        };
        let Some(tpl) = cl_docread::templates::learn(&last.reading) else {
            self.doc.learned = Some("nothing");
            return;
        };
        self.doc.learned = Some("saving");
        self.doc.learn_task =
            crate::reading::learn::save_layout(&self.link, self.group.as_ref(), vendor, tpl, true);
        if self.doc.learn_task.is_none() {
            self.doc.learned = Some("failed");
        }
    }

    /// The company's statement layouts (`document-drafts-v1`).
    pub(crate) fn load_statement_layouts(&mut self) {
        if self.link.is_demo()
            || !self
                .link
                .capabilities()
                .has(cl_api::Capabilities::DOCUMENT_DRAFTS)
        {
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        self.doc.layouts_task = group
            .read(&session, |s| async move {
                let rows = s.vendor_templates().list(None).await?;
                Ok(crate::reading::service::statement_templates_of(&rows))
            })
            .ok()
            .map(Task::Pending);
    }

    /// Picks read line `i`: its spot is outlined and its page shown.
    pub(crate) fn select_read_line(&mut self, i: usize) {
        self.doc.selected = Some(i);
        self.publish_targets();
        self.doc
            .compare
            .focus(Some(TargetKey::line(LINE_TARGET, RowKey(i as u64 + 1))));
    }

    /// The read lines as compare targets, with what each shows.
    fn publish_targets(&mut self) {
        let Some(read) = self.doc.read.as_ref() else {
            return;
        };
        let still = read.still_read(&self.text);
        let key = (read.pending(), still);
        if self.doc.published == Some(key) {
            return;
        }
        self.doc.published = Some(key);
        let mut targets = Vec::with_capacity(read.marks.len());
        let mut values = Vec::with_capacity(read.marks.len());
        for (i, m) in read.marks.iter().enumerate() {
            let spot = self.doc.spots.get(m.source).copied().flatten();
            targets.push(CompareTarget {
                key: TargetKey::line(LINE_TARGET, RowKey(m.source as u64 + 1)),
                label: t_with("statement.read.line_label", &[("n", &m.line.to_string())]),
                spot,
                level: if spot.is_none() {
                    TargetLevel::NotFound
                } else if m.unsure && !m.accepted && still {
                    TargetLevel::CheckThis
                } else {
                    TargetLevel::Sure
                },
                fill: None,
            });
            values.push(
                self.doc
                    .read_rows
                    .get(i)
                    .and_then(Option::as_ref)
                    .map(|r| {
                        format!(
                            "{} {} {}",
                            crate::shell::us_date(r.entry.date),
                            r.entry.reference,
                            r.entry.amount.format(self.money_style)
                        )
                    })
                    .unwrap_or_default(),
            );
        }
        self.doc.compare.set_targets(targets, values);
    }

    /// A compare event from the docked viewer or the pop-out.
    pub(crate) fn on_statement_event(&mut self, event: CompareEvent) {
        let service = self.doc.service.clone().or_else(crate::reading::shared);
        let env = CompareEnv {
            today: self.today,
            service: service.as_deref(),
            bytes: self.doc.bytes.clone(),
        };
        if let Some(CompareAction::Focus(key)) = self.doc.compare.on_event(event, &env)
            && key.field == LINE_TARGET
            && let Some(row) = key.row
            && let Ok(i) = usize::try_from(row.0)
            && i > 0
        {
            self.doc.selected = Some(i - 1);
        }
    }

    // ------------------------------------------------------ Ask AI to help

    fn ensure_statement_switches(&mut self) {
        if let Some(s) = ReadSwitches::demo(&self.link) {
            self.doc.switches = Some(s);
            return;
        }
        if self.doc.switches.is_some() || self.doc.switches_task.is_some() {
            return;
        }
        if let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) {
            self.doc.switches_task = super::super::doc_read::fetch_switches(group, &session)
                .ok()
                .map(Task::Pending);
        }
    }

    fn poll_statement_switches(&mut self) {
        if self.link.is_demo() && self.doc.last.is_some() {
            self.doc.switches = ReadSwitches::demo(&self.link);
        }
        if let Some(r) = self.doc.switches_task.as_mut().and_then(Task::try_take) {
            self.doc.switches_task = None;
            self.doc.switches = r.ok();
        }
    }

    /// Ask AI to help with the reading (the owner's click).
    pub(crate) fn start_statement_assist(&mut self) {
        let super::super::doc_read_local::Offer::Ask(route) = self.doc.offer() else {
            return;
        };
        let Some(last) = self.doc.last.as_ref() else {
            return;
        };
        if matches!(route, Route::Cloud { .. })
            && !crate::forms::suggest::under_cap(&self.link, self.doc.tenant)
        {
            self.doc.problem = Some("bill.read.problem.cap");
            self.doc.assist_outcome = Some("cap");
            return;
        }
        let kind = if crate::screens::preferences::intake_folders::source::statements_supported(
            &self.link,
        ) {
            DraftKind::Statement
        } else {
            DraftKind::Unknown
        };
        let bytes = self.doc.bytes.clone().unwrap_or_else(|| Arc::from([]));
        let request = assist::request(last, route, &bytes, kind);
        tracing::info!(job = self.doc.sent, "statement assist started");
        if self.link.is_demo() {
            self.doc.assist_sent += 1;
            self.doc.assist = Some(Task::ready(super::super::doc_read_local::demo_assist(
                &self.link,
            )));
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            self.doc.problem = Some("bill.read.problem.unreachable");
            return;
        };
        match super::super::doc_read::send_assist(group, &session, request) {
            Ok(p) => {
                self.doc.assist_sent += 1;
                self.doc.assist = Some(Task::Pending(p));
            }
            Err(_) => self.doc.problem = Some("bill.read.problem.busy"),
        }
    }

    /// The AI's answer, merged under the gate: it may name the vendor and
    /// the closing balance (kept only when printed on the page). The lines
    /// stay the reader's.
    fn poll_statement_assist(&mut self) {
        let Some(result) = self.doc.assist.as_mut().and_then(Task::poll_result) else {
            return;
        };
        self.doc.assist = None;
        let answer = match result {
            Ok(a) => a,
            Err(e) => {
                let key = super::super::doc_read_local::assist_problem(&e);
                self.doc.problem = Some(key);
                self.doc.assist_outcome = Some(key.rsplit('.').next().unwrap_or(key));
                return;
            }
        };
        let Some(last) = self.doc.last.clone() else {
            return;
        };
        let candidates = crate::reading::map::vendor_candidates(&self.contacts);
        // The gate runs before any value reaches the form (Q-A03j).
        let merged = assist::merge(&last, &answer, &candidates);
        self.doc.helped = true;
        self.doc.assist_outcome = Some("helped");
        let total_of = |h: &Header| h.total.as_ref().or(h.balance_due.as_ref()).map(|f| f.value);
        // Nothing reaches the form here: what was found waits for the
        // owner's Use (S4). An amount is the AI's only when the reader had
        // none; the gate kept it only because it is printed on the page.
        if self.doc.closing.is_none()
            && let Some(total) = total_of(&merged.reading.header)
        {
            let by_ai = total_of(&last.reading.header).is_none();
            self.doc.suggested_closing = Some((total, by_ai));
        }
        if last.vendor.is_none() {
            let found = merged.vendor.map(|v| v.contact).or_else(|| {
                match_name(
                    merged.reading.header.vendor.as_ref(),
                    &merged.reading,
                    &candidates,
                )
            });
            self.doc.suggested_vendor =
                found.filter(|f| self.vendors.iter().any(|(id, _)| id == f));
        }
    }

    /// The owner uses the vendor AI help named: it is picked, and the
    /// statement counts as that vendor's.
    pub(crate) fn use_suggested_vendor(&mut self) {
        if let Some(v) = self.doc.suggested_vendor.take() {
            self.vendor = Some(v);
            self.doc.read_vendor = Some(v);
        }
    }

    /// The owner uses the closing balance found with AI help: the balance
    /// line shows it (after a Match).
    pub(crate) fn use_suggested_closing(&mut self) {
        if let Some((value, by_ai)) = self.doc.suggested_closing.take() {
            self.doc.closing = Some(value);
            self.doc.closing_by_ai = by_ai;
            self.ask_balance();
        }
    }

    // ------------------------------------------------- Keep the statement

    /// Whether "Save this statement with the vendor" can be offered: a
    /// document and a Match for a vendor.
    pub(crate) fn keep_offered(&self) -> bool {
        self.doc.bytes.is_some() && self.outcome.is_some()
    }

    /// After a Match, the statement is kept with the vendor when the box
    /// is on (once per document, vendor and period).
    pub(crate) fn keep_after_match(&mut self) {
        if !self.doc.keep || self.doc.keep_task.is_some() {
            return;
        }
        let (Some(o), Some(bytes), Some(kind)) = (
            self.outcome.as_ref(),
            self.doc.bytes.clone(),
            self.doc.file_kind,
        ) else {
            return;
        };
        let key = KeepKey {
            vendor: o.request.vendor,
            from: o.request.from,
            to: o.request.to,
            doc: self.doc.doc_no,
        };
        if self.doc.kept == Some(key) {
            return;
        }
        // A statement that names another vendor than the Match's is never
        // filed on it, whatever started the keep (review B2). Picking the
        // statement's vendor and matching again keeps it.
        if self.doc.read_vendor.is_some_and(|v| v != key.vendor) {
            return;
        }
        let name = cl_api::statement_file_name(key.from, key.to, kind);
        let to = AttachTo::Contact(key.vendor);
        let draft = self.doc.draft;
        self.doc.keep_failed = false;
        let retry = self.doc.keep_keys.as_ref().is_some_and(|(k, _)| *k == key);
        if !retry {
            self.doc.keep_keys = Some((
                key,
                KeepKeys {
                    upload: IdempotencyKey::new(),
                    delete: IdempotencyKey::new(),
                },
            ));
        }
        let Some(keys) = self.doc.keep_keys.as_ref().map(|(_, k)| k.clone()) else {
            return;
        };
        if let Some(mut demo) = self.link.demo_state() {
            let mut uploaded = self.doc.keep_uploaded == Some(key);
            let r = keep_in_demo(
                &mut demo,
                self.doc.tenant,
                draft,
                (to, &name, &bytes),
                &mut uploaded,
            );
            drop(demo);
            if uploaded {
                self.doc.keep_uploaded = Some(key);
            }
            self.doc.keep_task = Some((key, Task::ready(())));
            if r.is_err() {
                self.doc.keep_task = None;
                self.doc.keep_failed = true;
            }
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            self.doc.keep_failed = true;
            return;
        };
        let label = OrphanLabel {
            what: "orphan.statement_keep",
            key: None,
        };
        let task = group.write(&session, label, move |s| async move {
            // The file is on the vendor before anything else changes. On
            // Try again, a file of this name and size already there is the
            // last attempt's: it isn't filed twice.
            let there = retry && {
                let files = s
                    .attachments()
                    .list(to, cl_api::PageRequest::first_max())
                    .await?;
                files
                    .data
                    .iter()
                    .any(|f| f.file_name == name && f.file_size == u64::try_from(bytes.len()).ok())
            };
            if !there {
                s.attachments()
                    .upload(to, &name, &bytes, keys.upload)
                    .await?;
            }
            if let Some(d) = draft {
                // Then the draft leaves Review Documents (discard is
                // idempotent), and its own copy of the file goes. That copy
                // is the same bytes as the vendor's: failing to delete it
                // loses nothing.
                s.document_drafts().discard(d.id).await?;
                if let Some(file) = d.attachment
                    && s.attachments().delete(file, keys.delete).await.is_err()
                {
                    tracing::info!(outcome = "copy_left", "statement draft file");
                }
            }
            Ok(())
        });
        match task {
            Ok(p) => self.doc.keep_task = Some((key, Task::Pending(p))),
            Err(_) => self.doc.keep_failed = true,
        }
    }

    fn poll_keep(&mut self) {
        let done = self.doc.keep_task.as_mut().and_then(|(_, t)| t.try_take());
        if let Some(r) = done
            && let Some((key, _)) = self.doc.keep_task.take()
        {
            match r {
                Ok(()) => {
                    self.doc.kept = Some(key);
                    self.doc.keeps += 1;
                    self.doc.draft = None;
                }
                Err(_) => self.doc.keep_failed = true,
            }
        }
    }

    // ------------------------------------------------------ the balance line

    /// After a Match with a read closing balance: the books' balance with
    /// the vendor on the statement's date.
    pub(crate) fn ask_balance(&mut self) {
        let (Some(o), Some(_)) = (self.outcome.as_ref(), self.doc.closing) else {
            return;
        };
        let to = self.doc.statement_date.unwrap_or(o.request.to);
        let key = BalanceKey {
            vendor: o.request.vendor,
            from: o.request.from.min(to),
            to,
        };
        if self.doc.books.is_some_and(|(k, _)| k == key)
            || self
                .doc
                .balance_task
                .as_ref()
                .is_some_and(|(k, _)| *k == key)
        {
            return;
        }
        if self.link.is_demo() {
            let b = super::demo_balances(key.from, key.to);
            self.doc.balance_task = Some((key, Task::ready(b)));
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        self.doc.balance_task = group
            .read(&session, move |s| async move {
                s.reports()
                    .vendor_statement_balances(key.vendor, key.from, key.to)
                    .await
            })
            .ok()
            .map(|p| (key, Task::Pending(p)));
    }

    fn poll_balance(&mut self) {
        let done = self
            .doc
            .balance_task
            .as_mut()
            .and_then(|(_, t)| t.try_take());
        if let Some(r) = done
            && let Some((key, _)) = self.doc.balance_task.take()
        {
            self.doc.books = r.ok().flatten().map(|b| (key, b));
        }
    }

    /// The statement's closing balance, the books' and the difference, when
    /// both are known exactly (display only).
    pub(crate) fn balance_line(&self) -> Option<(Money, Money, Money)> {
        let o = self.outcome.as_ref()?;
        let (key, books) = self.doc.books?;
        if key.vendor != o.request.vendor {
            return None;
        }
        let stmt = self.doc.closing?;
        let diff = stmt.checked_sub(books.closing).ok()?;
        Some((stmt, books.closing, diff))
    }

    // ------------------------------------------------------------ drawing

    /// The Read a statement row: Browse, the file, the reading's state.
    pub(crate) fn statement_file_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        self.take_dropped(ui);
        ui.horizontal_wrapped(|ui| {
            let busy = self.doc.busy();
            if ui
                .add_enabled(!busy, egui::Button::new(t("statement.read.button")))
                .on_hover_text(t("statement.read.button.tip"))
                .clicked()
            {
                self.browse_statement(ui.ctx());
            }
            form::hint(ui, tokens, &t("statement.read.drop_hint"));
        });
        let mut read_typed = false;
        ui.horizontal_wrapped(|ui| {
            let l = ui.add(egui::Label::new(t("statement.read.file")).selectable(false));
            let r = ui
                .add(egui::TextEdit::singleline(&mut self.doc.path_text).desired_width(320.0))
                .labelled_by(l.id);
            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let busy = self.doc.busy();
            read_typed = (ui
                .add_enabled(
                    !busy && !self.doc.path_text.trim().is_empty(),
                    egui::Button::new(t("statement.read.file_button")),
                )
                .clicked()
                || enter)
                && !self.doc.path_text.trim().is_empty();
        });
        if read_typed {
            let p = PathBuf::from(self.doc.path_text.trim());
            self.statement_path(&p);
        }
        if self.doc.job.is_some() || self.doc.loading.is_some() || self.doc.draft_file.is_some() {
            ui.horizontal_wrapped(|ui| {
                form::progress_line(ui, tokens, &t("statement.read.reading"));
                if self.doc.job.is_some() && ui.button(t("statement.read.cancel")).clicked() {
                    if let Some(c) = self.doc.cancel.take() {
                        c.cancel();
                    }
                    self.doc.job = None;
                    self.doc.outcome = Some(ReadFailure::Cancelled.class());
                }
            });
        }
        if let Some(k) = self.doc.problem {
            ui.label(RichText::new(t(k)).color(tokens.warn_text));
        }
        if self.vendor_differs() {
            let name = self.read_vendor_name().unwrap_or_default();
            ui.label(
                RichText::new(t_with("statement.read.vendor_differs", &[("name", &name)]))
                    .color(tokens.warn_text),
            );
        }
        self.suggestions_ui(ui, tokens);
        if self.doc.assist.is_some() {
            form::progress_line(ui, tokens, &t("statement.read.helping"));
        }
        let mut ask = false;
        match self.doc.offer() {
            super::super::doc_read_local::Offer::Ask(route) => {
                let tip = match route {
                    Route::Local => "bill.read.assist.button.tip.local",
                    Route::Cloud { .. } => "bill.read.assist.button.tip.cloud",
                };
                ask = ui
                    .button(t("bill.read.assist.button"))
                    .on_hover_text(t(tip))
                    .clicked();
            }
            super::super::doc_read_local::Offer::NeedsConsent => {
                form::hint(ui, tokens, &t("bill.read.assist.consent"));
            }
            super::super::doc_read_local::Offer::None => {}
        }
        if ask {
            self.start_statement_assist();
        }
    }

    /// What Ask AI to help found, each with its Use button (S4).
    fn suggestions_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let mut use_vendor = false;
        let mut use_closing = false;
        if let Some(v) = self.doc.suggested_vendor {
            let name = self
                .vendors
                .iter()
                .find(|(id, _)| *id == v)
                .map(|(_, n)| n.clone())
                .unwrap_or_default();
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(t_with("statement.suggest.vendor", &[("name", &name)]))
                        .color(tokens.warn_text),
                );
                use_vendor = ui.button(t("statement.suggest.vendor.use")).clicked();
            });
        }
        if let Some((value, by_ai)) = self.doc.suggested_closing {
            let key = if by_ai {
                "statement.suggest.closing.ai"
            } else {
                "statement.suggest.closing"
            };
            let amount = value.format(self.money_style);
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(t_with(key, &[("amount", &amount)])).color(tokens.warn_text),
                );
                use_closing = ui.button(t("statement.suggest.closing.use")).clicked();
            });
        }
        if use_vendor {
            self.use_suggested_vendor();
        }
        if use_closing {
            self.use_suggested_closing();
        }
    }

    /// The read lines with their marks and Accept, and the checks.
    pub(crate) fn read_lines_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let Some(read) = self.doc.read.clone() else {
            return;
        };
        if !read.still_read(&self.text) {
            form::hint(ui, tokens, &t("statement.read.typed_over"));
            return;
        }
        let mut accept_all = false;
        let mut accept = None;
        let mut select = None;
        form::card(ui, tokens, |ui| {
            form::heading(
                ui,
                tokens,
                &t_with(
                    "statement.read.lines",
                    &[("n", &read.marks.len().to_string())],
                ),
            );
            ui.label(
                RichText::new(t("statement.read.check_it"))
                    .color(tokens.text_secondary)
                    .size(12.0),
            );
            if read.pending() > 0 {
                ui.horizontal_wrapped(|ui| {
                    accept_all =
                        crate::widgets::primary_button(ui, &t("statement.read.accept_all"))
                            .clicked();
                    crate::widgets::kbd(
                        ui,
                        &crate::screens::card_charge::kit::chip("form.accept_read"),
                    );
                    form::hint(
                        ui,
                        tokens,
                        &t_with(
                            "statement.read.pending",
                            &[("n", &read.pending().to_string())],
                        ),
                    );
                });
            } else {
                form::hint(ui, tokens, &t("statement.read.accepted"));
            }
            egui::ScrollArea::horizontal()
                .id_salt("statement/read_lines")
                .show(ui, |ui| {
                    egui::Grid::new("statement/read_lines")
                        .striped(true)
                        .spacing([16.0, 4.0])
                        .show(ui, |ui| {
                            for k in [
                                "statement.col.line",
                                "statement.col.date",
                                "statement.col.ref",
                                "statement.col.type",
                                "statement.col.amount",
                                "",
                            ] {
                                let text = if k.is_empty() { String::new() } else { t(k) };
                                ui.label(
                                    RichText::new(text).size(12.0).color(tokens.text_secondary),
                                );
                            }
                            ui.end_row();
                            for (i, m) in read.marks.iter().enumerate() {
                                let picked = self.doc.selected == Some(m.source);
                                let n = m.line.to_string();
                                let r = ui
                                    .selectable_label(picked, &n)
                                    .on_hover_text(t("statement.read.find_on_page"));
                                if r.clicked() {
                                    select = Some(m.source);
                                }
                                if let Some(row) =
                                    self.doc.read_rows.get(i).and_then(Option::as_ref)
                                {
                                    // Signed as the box shows it: a line
                                    // without its kind keeps its minus.
                                    let mut row = row.clone();
                                    if row.entry.kind == StatementKind::CreditOrPayment
                                        && let Ok(neg) = row.entry.amount.checked_neg()
                                    {
                                        row.entry.amount = neg;
                                    }
                                    Self::line_cells(ui, tokens, &row, self.money_style);
                                } else {
                                    ui.label("");
                                    ui.label("");
                                    ui.label("");
                                    ui.label(
                                        RichText::new(t("statement.read.fix_line"))
                                            .color(tokens.warn_text),
                                    );
                                }
                                if m.accepted {
                                    ui.label(
                                        RichText::new(t("statement.read.line_accepted"))
                                            .size(12.0)
                                            .color(tokens.text_secondary),
                                    );
                                } else {
                                    ui.horizontal(|ui| {
                                        let note = if m.unsure {
                                            RichText::new(t("statement.read.unsure"))
                                                .color(tokens.warn_text)
                                                .strong()
                                        } else {
                                            RichText::new(t("statement.read.caption"))
                                                .color(tokens.text_secondary)
                                        };
                                        ui.label(note.size(12.0));
                                        if ui
                                            .button(t("statement.read.accept"))
                                            .on_hover_text(t_with(
                                                "statement.read.accept.tip",
                                                &[("n", &n)],
                                            ))
                                            .clicked()
                                        {
                                            accept = Some(m.line);
                                        }
                                    });
                                }
                                ui.end_row();
                            }
                        });
                });
            if !self.doc.checks.is_empty() {
                ui.add_space(4.0);
                ui.label(RichText::new(t("statement.read.checks")).strong());
                for c in &self.doc.checks {
                    ui.label(
                        RichText::new(check_text(c, self.money_style)).color(tokens.warn_text),
                    );
                }
                form::hint(ui, tokens, &t("statement.read.checks.never_fixed"));
            }
        });
        if accept_all {
            self.accept_read_lines();
        }
        if let Some(l) = accept {
            self.accept_read_line(l);
        }
        if let Some(i) = select {
            self.select_read_line(i);
        }
    }

    /// The document pane (docked) beside the lines.
    pub(crate) fn statement_pane_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        if self.doc.viewer.source().is_none() {
            return;
        }
        self.publish_targets();
        let popped = crate::windows::doc_viewer::is_popped(ui.ctx());
        if !popped {
            self.doc.viewer.set_compare(Some(self.doc.compare.view()));
        }
        form::card_tight(ui, tokens, |ui| {
            if popped {
                DocPane::show(ui, &mut self.doc.viewer, tokens);
            } else {
                let w = ui.available_width();
                ui.allocate_ui(egui::vec2(w, PANE_HEIGHT), |ui| {
                    DocPane::show(ui, &mut self.doc.viewer, tokens);
                });
            }
        });
        for e in self.doc.viewer.take_compare_events() {
            self.on_statement_event(e);
        }
    }

    /// "Save this statement with the vendor", the balance line.
    pub(crate) fn after_match_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        if let Some((stmt, books, diff)) = self.balance_line() {
            form::card(ui, tokens, |ui| {
                let style = self.money_style;
                ui.label(
                    RichText::new(t_with(
                        "statement.balance.line",
                        &[
                            ("statement", &stmt.format(style)),
                            ("books", &books.format(style)),
                        ],
                    ))
                    .color(tokens.text_primary),
                );
                let diff_text = if diff.is_zero() {
                    t("statement.balance.same")
                } else {
                    t_with(
                        "statement.balance.difference",
                        &[("difference", &diff.format(style))],
                    )
                };
                ui.label(RichText::new(diff_text).strong());
                if self.doc.closing_by_ai {
                    form::hint(ui, tokens, &t("statement.balance.by_ai"));
                }
                // Plain words, with the accounting term in the tooltip.
                ui.add(
                    egui::Label::new(
                        RichText::new(t("statement.balance.note"))
                            .size(12.0)
                            .color(tokens.text_secondary),
                    )
                    .wrap(),
                )
                .on_hover_text(t("statement.balance.note.tip"));
            });
            ui.add_space(6.0);
        }
        if self.keep_offered() {
            ui.horizontal_wrapped(|ui| {
                let before = self.doc.keep;
                ui.checkbox(&mut self.doc.keep, t("statement.keep"))
                    .on_hover_text(t("statement.keep.tip"));
                if self.doc.keep && !before {
                    self.keep_after_match();
                }
                if self.doc.keep_task.is_some() {
                    form::progress_line(ui, tokens, &t("statement.keep.saving"));
                } else if self.doc.kept.is_some() {
                    form::hint(ui, tokens, &t("statement.keep.saved"));
                } else if self.doc.keep_failed {
                    ui.label(RichText::new(t("statement.keep.failed")).color(tokens.warn_text));
                    if ui.button(t("statement.try_again")).clicked() {
                        self.keep_after_match();
                    }
                }
            });
        }
    }

    // -------------------------------------------------- statement drafts

    /// A statement draft from Review Documents: its file is fetched and
    /// read here.
    pub(crate) fn open_statement_draft(&mut self, id: DocumentDraftId) {
        self.doc.reset();
        if let Some(demo) = self.link.demo_state() {
            let r = self
                .doc
                .tenant
                .ok_or(ApiError::NoCompanyOpen)
                .and_then(|tenant| {
                    let d = crate::company::demo::drafts::get(&demo, tenant, id)?;
                    let file = d.attachment_id.ok_or(ApiError::NotFound { code: None })?;
                    let bytes = crate::company::demo::checks::download(&demo, file)?;
                    Ok((d, bytes))
                });
            drop(demo);
            if let Ok(v) = r {
                self.doc.draft_file = Some(Task::ready(v));
            } else {
                self.doc.problem = Some("statement.read.draft_failed");
            }
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            self.doc.problem = Some("statement.read.draft_failed");
            return;
        };
        self.doc.draft_file = group
            .read(&session, move |s| async move {
                let d = s.document_drafts().get(id).await?;
                let file = d.attachment_id.ok_or(ApiError::NotFound { code: None })?;
                let bytes = s.attachments().download(file).await?;
                Ok((d, bytes))
            })
            .ok()
            .map(Task::Pending);
    }
}

/// The demo engine's keep, in the live order: the file is uploaded to the
/// vendor (once: `uploaded` says it is there already), then a statement
/// draft is discarded and its own copy deleted.
fn keep_in_demo(
    demo: &mut crate::company::demo::DemoState,
    tenant: Option<TenantId>,
    draft: Option<StatementDraft>,
    (to, name, bytes): (AttachTo, &str, &[u8]),
    uploaded: &mut bool,
) -> Result<(), ApiError> {
    use crate::company::demo::{checks, drafts};
    if !*uploaded {
        checks::upload(demo, to, name, bytes)?;
        *uploaded = true;
    }
    if let Some(d) = draft {
        let tenant = tenant.ok_or(ApiError::NoCompanyOpen)?;
        drafts::discard(demo, tenant, d.id)?;
        if let Some(file) = d.attachment {
            checks::delete_attachment(demo, file);
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "read_tests.rs"]
mod tests;
