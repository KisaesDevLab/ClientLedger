//! Switch from `QuickBooks` Desktop (P31, `file.import_qbd`), and the same
//! wizard for other accounting programs (`system_import`, Q-275). Layout per
//! `docs/design/mockups/QBDImport.html`: a step list on the left, the step
//! on the right, Back and the step's main button at the bottom right.
//!
//! 1. **Export reports.** Each report to export, in plain words.
//! 2. **Add the files.** One card per report; several journal files (one
//!    per year); the engine reads each file as its slot's report (its
//!    header check) and says what it could not read.
//! 3. **Review matches.** How each account the files name was matched;
//!    an account that matches nothing must be given one (never suspense,
//!    Q-P31c).
//! 4. **History and start date.** The conversion date (from the reference
//!    trial balance), the history depth (Q-142: two years plus opening
//!    balances, preselected), the date checks of the files.
//! 5. **Import.** One batch: a verified backup first (nothing is sent when
//!    it fails), then the commit (one key per exact body; never sent again
//!    on its own), then the batch read each frame with its progress. A
//!    closed period asks for the closing-date password; a lost answer
//!    offers Check status and Try again (the same body and key).
//! 6. **Check that it matches** and 7. **Spot-check** (D4 builds their
//!    views; here they show the engine's tie-out summary).
//!
//! Undo (History or the step list's link): confirm, backup, undo; a 409
//! lists what blocks it in plain words and offers the backup's restore.
//!
//! Control-channel state: steps, counts and flags only; never amounts,
//! names, file names or the backup's path.

// No floating point near imported amounts (plan §3); pixel sums only in
// the drawing.
#![deny(clippy::float_arithmetic)]

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::mpsc::Receiver;

use cl_api::imports::{
    AccountMatch, BatchState, BatchStatus, ImportBatch, ImportBatchId, ImportRefusal,
    ImportSessionId, JobAccepted, MatchStatus, TieOut, UploadOutcome, import_refusal,
};
use cl_api::{
    Account, AccountFilter, ApiError, IdempotencyKey, OrphanLabel, RequestGroup, Session,
};
use cl_core::ids::AccountId;
use cl_core::{CalendarDate, Secret};
use cl_model::CommandId;
use cl_model::imports::wizard::{DroppedFile, Source, Step, Wizard};
use serde_json::{Value, json};

use crate::company::{CompanyLink, Task};
use crate::forms::closing_prompt::ClosingPrompt;
use crate::i18n::t;
use crate::windows::Screen;

mod draw;
pub mod mapping;
pub mod spot_check;
#[cfg(test)]
mod tests;
pub mod tieout;
mod wizard;

/// The command and window kind.
pub const KIND: &str = "file.import_qbd";

/// The QuickBooks Desktop window for `kind`, if it is one.
#[must_use]
pub fn make(
    kind: CommandId,
    link: &Arc<CompanyLink>,
    _open: Option<(cl_core::ids::TenantId, String)>,
    today: CalendarDate,
) -> Option<Box<dyn Screen>> {
    (kind.as_str() == KIND)
        .then(|| Box::new(ImportScreen::new(Arc::clone(link), today, Source::Qbd)) as Box<_>)
}

/// The backup kind taken before an import or an undo. The shared
/// `BackupKind::BeforeImport` lives on `fix/P30-backup-before-post`, not
/// merged into this branch yet: until it is, the regular kind is used with
/// retention off (regular retention could prune a backup a batch still
/// needs, plan §1). Swap here when that branch merges.
pub(crate) fn backup_kind() -> cl_settings::BackupKind {
    cl_settings::BackupKind::Backup
}

/// A request in flight or already answered (the demo answers at once).
pub(crate) enum Job<T> {
    Running(Task<T>),
    Done(Option<Result<T, ApiError>>),
}

impl<T> Job<T> {
    fn take(&mut self) -> Option<Result<T, ApiError>> {
        match self {
            Job::Running(t) => t.try_take(),
            Job::Done(v) => v.take(),
        }
    }
}

fn poll<T>(slot: &mut Option<Job<T>>) -> Option<Result<T, ApiError>> {
    let r = slot.as_mut()?.take()?;
    *slot = None;
    Some(r)
}

/// Which part of the window shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    /// The wizard.
    Import,
    /// Earlier imports.
    History,
}

/// What went wrong (shown in owner words; never the engine's text, which
/// can name people, U-270).
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Trouble {
    Refused(ImportRefusal),
    /// The answer was lost: it may have gone through.
    Unknown,
    PasswordWait,
    /// Not the company Admin, or an account is hidden from this user.
    Restricted,
    Unsupported,
    EngineDown,
    /// The backup before the import (or undo) failed: nothing was sent.
    BackupFailed,
    /// The import job failed: nothing was posted (the backup is kept).
    Failed,
    /// The batch is gone (discarded elsewhere): nothing was posted.
    Gone,
    Other,
}

impl Trouble {
    fn of(e: &ApiError) -> Trouble {
        if let Some(r) = import_refusal(e) {
            return Trouble::Refused(r);
        }
        match e {
            ApiError::OutcomeUnknown { .. } => Trouble::Unknown,
            ApiError::ClosingPasswordRateLimited { .. } => Trouble::PasswordWait,
            ApiError::Sensitive(_) | ApiError::Forbidden { .. } => Trouble::Restricted,
            ApiError::Local(cl_api::LocalValidation::Unsupported) => Trouble::Unsupported,
            ApiError::EngineUnavailable { .. } => Trouble::EngineDown,
            ApiError::NotFound { .. } => Trouble::Gone,
            _ => Trouble::Other,
        }
    }

    /// A stable key (`imports.trouble.<code>` or `imports.refusal.<code>`).
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Trouble::Refused(r) => refusal_code(r),
            Trouble::Unknown => "unknown",
            Trouble::PasswordWait => "password_wait",
            Trouble::Restricted => "restricted",
            Trouble::Unsupported => "unsupported",
            Trouble::EngineDown => "engine_down",
            Trouble::BackupFailed => "backup_failed",
            Trouble::Failed => "failed",
            Trouble::Gone => "gone",
            Trouble::Other => "other",
        }
    }
}

/// A stable key for a refusal (`imports.refusal.<key>`).
pub(crate) fn refusal_code(r: &ImportRefusal) -> &'static str {
    match r {
        ImportRefusal::AlreadyCommitted { .. } => "already_committed",
        ImportRefusal::PeriodOverlap { .. } => "period_overlap",
        ImportRefusal::Busy => "busy",
        ImportRefusal::UndoBlocked { .. } => "undo_blocked",
        ImportRefusal::TieOutNotZero { .. } => "tieout_not_zero",
        ImportRefusal::CompanyNotEmpty => "company_not_empty",
        ImportRefusal::SessionInBatch => "session_in_batch",
        ImportRefusal::FiscalYearLocked => "fiscal_year_locked",
        ImportRefusal::AdminRequired => "admin_required",
        ImportRefusal::DateMismatch { .. } => "date_mismatch",
        ImportRefusal::AmountInvalid { .. } => "amount_invalid",
        ImportRefusal::Unbalanced => "unbalanced",
        ImportRefusal::UnknownRef { .. } => "unknown_ref",
        ImportRefusal::MixedGroup => "mixed_group",
        ImportRefusal::Interrupted => "interrupted",
        ImportRefusal::BatchUndone => "batch_undone",
        ImportRefusal::BatchNotCommitted => "batch_not_committed",
        ImportRefusal::BatchEmpty => "batch_empty",
        ImportRefusal::SourceMismatch => "source_mismatch",
    }
}

/// Reads one chosen file off the egui thread, refusing it before anything
/// is sent when it is over the engine's limit.
fn read_import_file(path: &std::path::Path) -> crate::screens::write_check::attach::FileRead {
    let meta = std::fs::metadata(path).map_err(|_| "imports.file.open")?;
    if !meta.is_file() {
        return Err("imports.file.open");
    }
    if meta.len() > cl_api::imports::MAX_UPLOAD_BYTES as u64 {
        return Err("imports.file.too_large");
    }
    let bytes = std::fs::read(path).map_err(|_| "imports.file.open")?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    Ok((name, bytes))
}

/// One file the owner added to a report's card.
pub(crate) struct SlotFile {
    /// Its name, shown on the card only (never logged or in state).
    pub(crate) name: String,
    /// Being read from disk.
    pub(crate) reading: Option<Receiver<crate::screens::write_check::attach::FileRead>>,
    /// The upload out.
    pub(crate) job: Option<Job<UploadOutcome>>,
    /// What the engine read.
    pub(crate) outcome: Option<UploadOutcome>,
    /// Why it was not taken (an i18n key).
    pub(crate) problem: Option<&'static str>,
    /// The engine's refusal (a re-import).
    pub(crate) refusal: Option<ImportRefusal>,
}

impl SlotFile {
    fn new(name: String) -> SlotFile {
        SlotFile {
            name,
            reading: None,
            job: None,
            outcome: None,
            problem: None,
            refusal: None,
        }
    }

    pub(crate) fn session(&self) -> Option<ImportSessionId> {
        self.outcome.as_ref().map(|o| o.session.id)
    }

    pub(crate) fn busy(&self) -> bool {
        self.reading.is_some() || self.job.is_some()
    }

    /// The model's view of it.
    fn dropped(&self) -> DroppedFile {
        DroppedFile {
            name: self.name.clone(),
            uploaded: self.outcome.is_some(),
            problems: self.outcome.as_ref().map_or(0, |o| {
                u32::try_from(
                    o.problems
                        .len()
                        .max(usize::try_from(o.session.error_count).unwrap_or(0)),
                )
                .unwrap_or(u32::MAX)
            }),
            report_date: self
                .outcome
                .as_ref()
                .and_then(|o| o.session.report_date.or(o.preview.report_date)),
        }
    }
}

/// The running job the batch reports on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Running {
    Commit,
    Undo,
}

/// The exact body (and its key) of the last commit or undo sent: "Try
/// again" after a lost answer sends this again, never a new one. No
/// `Debug`: the password and the backup's name are in it.
#[derive(Clone)]
pub(crate) struct Held {
    pub(crate) what: Running,
    pub(crate) batch: ImportBatchId,
    pub(crate) backup_ref: String,
    pub(crate) password: Option<Secret>,
    /// The owner confirmed importing into a closed period that has no
    /// closing-date password (commit only; never set by default).
    pub(crate) confirm_closed: bool,
    pub(crate) key: IdempotencyKey,
}

/// What the closing-date password is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Locked {
    Commit,
    Undo(ImportBatchId),
}

/// The Import window (QuickBooks Desktop or another program).
#[allow(clippy::struct_excessive_bools, reason = "independent screen flags")]
pub struct ImportScreen {
    pub(crate) link: Arc<CompanyLink>,
    group: Option<RequestGroup>,
    pub(crate) today: CalendarDate,
    pub(crate) view: View,
    /// The step machine (cl-model).
    pub(crate) wizard: Wizard,
    /// The QBD window or the other-programs one (its source picker).
    pub(crate) system: bool,
    // Files.
    pub(crate) pick_rx: Option<Receiver<Vec<std::path::PathBuf>>>,
    /// The card the file window is for.
    pub(crate) pick_slot: Option<&'static str>,
    pub(crate) uploads: BTreeMap<&'static str, Vec<SlotFile>>,
    discard_jobs: Vec<Job<()>>,
    // Matches.
    chart_job: Option<Job<Vec<Account>>>,
    pub(crate) chart: Vec<Account>,
    /// Source key → the company account the owner picked.
    pub(crate) account_map: BTreeMap<String, AccountId>,
    // Dates.
    pub(crate) date_text: String,
    pub(crate) opening_text: String,
    // Import.
    batch_create_job: Option<Job<ImportBatch>>,
    /// The batch (made once on the first Import click; every retry uses
    /// it: its files are pinned to it).
    pub(crate) batch: Option<ImportBatchId>,
    backup_job: Option<Job<Result<String, cl_company::CompanyError>>>,
    /// The commit or undo waiting for its backup.
    waiting: Option<(Running, ImportBatchId)>,
    job_out: Option<Job<JobAccepted>>,
    pub(crate) held: Option<Held>,
    /// The key of the last commit or undo sent (control channel, tests).
    pub(crate) last_key: Option<IdempotencyKey>,
    /// The commit's or undo's answer was lost (B1: nothing discards,
    /// Cancel and Back are off until Check status settles it).
    pub(crate) unsettled: bool,
    /// The batch's job runs (per the last read).
    pub(crate) running: Option<Running>,
    batch_job: Option<Job<BatchState>>,
    pub(crate) state: Option<BatchState>,
    /// Commits and undos sent (control channel: never twice on its own).
    pub(crate) jobs_sent: u32,
    pub(crate) closing: ClosingPrompt<Locked>,
    /// The closing-date password for the next send (dropped once sent).
    password: Option<Secret>,
    /// A closed period with no closing-date password: the question is up
    /// (its closing date).
    pub(crate) ask_closed: Option<Option<CalendarDate>>,
    /// The owner said yes: the next commit carries the confirmation.
    confirm_closed_next: bool,
    pub(crate) status_answer: Option<bool>,
    pub(crate) trouble: Option<Trouble>,
    /// Why the last job failed, when its code has owner words (shown in
    /// place of the plain "stopped partway"; Retry is still offered).
    pub(crate) failure: Option<ImportRefusal>,
    // Tie-out (D4 draws it in full).
    tieout_job: Option<Job<TieOut>>,
    pub(crate) tieout: Option<TieOut>,
    /// The tie-out's rows, tab and filter.
    pub(crate) tie: Option<tieout::TieState>,
    /// Save PDF waiting for its Save dialog.
    pub(crate) pdf: Option<tieout::PdfSave>,
    /// The last Save PDF's outcome (`ok`, words).
    pub(crate) pdf_message: Option<(bool, String)>,
    /// Tie-out PDFs written (control channel).
    pub(crate) pdfs_saved: u32,
    verify_job: Option<Job<ImportBatch>>,
    /// The spot-check list (step 7), saved with the batch.
    pub(crate) spot: spot_check::SpotList,
    // Undo.
    pub(crate) confirm_undo: Option<ImportBatchId>,
    /// The batch an undo (its backup, its password, its send) is for.
    pub(crate) undo_pending: Option<ImportBatchId>,
    pub(crate) undone: bool,
    // History.
    history_job: Option<Job<Vec<ImportBatch>>>,
    pub(crate) history: Option<Vec<ImportBatch>>,
}

/// A date as the screen shows and takes it.
pub(crate) fn show_date(d: CalendarDate) -> String {
    format!("{:02}/{:02}/{:04}", d.month(), d.day(), d.year())
}

pub(crate) fn parse_date(s: &str) -> Option<CalendarDate> {
    let s = s.trim();
    if let Ok(d) = CalendarDate::parse_wire(s) {
        return Some(d);
    }
    let mut it = s.split('/');
    let (m, d, y) = (it.next()?, it.next()?, it.next()?);
    CalendarDate::parse_wire(&format!("{y:0>4}-{m:0>2}-{d:0>2}")).ok()
}

impl ImportScreen {
    /// A fresh wizard for `source` (the other-programs window starts on
    /// Accounting Power and offers the others).
    #[must_use]
    pub fn new(link: Arc<CompanyLink>, today: CalendarDate, source: Source) -> ImportScreen {
        let group = link.group().and_then(Result::ok);
        let mut s = ImportScreen {
            link,
            group,
            today,
            view: View::Import,
            wizard: Wizard::new(source, 1),
            system: source != Source::Qbd,
            pick_rx: None,
            pick_slot: None,
            uploads: BTreeMap::new(),
            discard_jobs: Vec::new(),
            chart_job: None,
            chart: Vec::new(),
            account_map: BTreeMap::new(),
            date_text: String::new(),
            opening_text: String::new(),
            batch_create_job: None,
            batch: None,
            backup_job: None,
            waiting: None,
            job_out: None,
            held: None,
            last_key: None,
            unsettled: false,
            running: None,
            batch_job: None,
            state: None,
            jobs_sent: 0,
            closing: ClosingPrompt::default(),
            password: None,
            ask_closed: None,
            confirm_closed_next: false,
            status_answer: None,
            trouble: None,
            failure: None,
            tieout_job: None,
            tieout: None,
            tie: None,
            pdf: None,
            pdf_message: None,
            pdfs_saved: 0,
            verify_job: None,
            spot: spot_check::SpotList::default(),
            confirm_undo: None,
            undo_pending: None,
            undone: false,
            history_job: None,
            history: None,
        };
        if !s.allowed() {
            s.trouble = Some(Trouble::Restricted);
        } else if !s.supported() {
            s.trouble = Some(Trouble::Unsupported);
        } else {
            s.load_chart();
        }
        s
    }

    /// The window's command id.
    pub(crate) fn kind(&self) -> &'static str {
        if self.system {
            crate::screens::system_import::KIND
        } else {
            KIND
        }
    }

    /// Only the company Admin brings books over (plan S4; the engine
    /// refuses others too).
    pub(crate) fn allowed(&self) -> bool {
        let r = self.link.rights();
        r.is_admin() && r.allows(CommandId::new(self.kind()))
    }

    /// The engine can do this import (the demo plays every one).
    pub(crate) fn supported(&self) -> bool {
        let flow = if self.wizard.source.is_conversion() {
            cl_api::imports::Flow::QbdConversion
        } else {
            cl_api::imports::Flow::OtherSystem
        };
        self.link.is_demo() || flow.supported(&self.link.capabilities())
    }

    // ---- Running requests ------------------------------------------------

    fn read<T, F, Fut>(
        &self,
        demo: impl FnOnce(&mut crate::company::demo::DemoState) -> Result<T, ApiError>,
        real: F,
    ) -> Job<T>
    where
        T: Send + 'static,
        F: FnOnce(Session) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Result<T, ApiError>> + Send + 'static,
    {
        if let Some(mut d) = self.link.demo_state() {
            return Job::Done(Some(demo(&mut d)));
        }
        match (self.link.session(), self.group.as_ref()) {
            (Some(session), Some(group)) => group.read(&session, real).map_or(
                Job::Done(Some(Err(ApiError::EngineUnavailable { permanent: false }))),
                |p| Job::Running(Task::Pending(p)),
            ),
            _ => Job::Done(Some(Err(ApiError::EngineUnavailable { permanent: false }))),
        }
    }

    fn write<T, F, Fut>(
        &self,
        what: &'static str,
        key: Option<IdempotencyKey>,
        demo: impl FnOnce(&mut crate::company::demo::DemoState) -> Result<T, ApiError>,
        real: F,
    ) -> Job<T>
    where
        T: Send + 'static,
        F: FnOnce(Session) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Result<T, ApiError>> + Send + 'static,
    {
        if let Some(mut d) = self.link.demo_state() {
            return Job::Done(Some(demo(&mut d)));
        }
        match (self.link.session(), self.group.as_ref()) {
            (Some(session), Some(group)) => group
                .write(&session, OrphanLabel { what, key }, real)
                .map_or(
                    Job::Done(Some(Err(ApiError::EngineUnavailable { permanent: false }))),
                    |p| Job::Running(Task::Pending(p)),
                ),
            _ => Job::Done(Some(Err(ApiError::EngineUnavailable { permanent: false }))),
        }
    }

    fn load_chart(&mut self) {
        self.chart_job = Some(self.read(
            |d| Ok(crate::company::demo::home::accounts(d)),
            |s| async move { s.accounts().list_all(&AccountFilter::default()).await },
        ));
    }

    /// Takes every answer that arrived; while a job runs, reads its batch
    /// (one read out at a time).
    pub(crate) fn poll(&mut self) {
        if let Some(Ok(v)) = poll(&mut self.chart_job) {
            self.chart = v;
        }
        self.poll_files();
        self.discard_jobs.retain_mut(|j| j.take().is_none());
        if let Some(r) = poll(&mut self.batch_create_job) {
            self.after_batch_created(r);
        }
        // Before the job's own answer: the demo's backup and job both
        // answer within one poll.
        if let Some(r) = poll(&mut self.backup_job) {
            self.after_backup(r.map_err(cl_company::CompanyError::from).and_then(|x| x));
        }
        if let Some(r) = poll(&mut self.job_out) {
            self.after_job_sent(r);
        }
        if let Some(r) = poll(&mut self.batch_job) {
            self.after_batch_read(r);
        }
        if self.running.is_some() && self.batch_job.is_none() {
            self.read_batch();
        }
        if let Some(r) = poll(&mut self.tieout_job) {
            match r {
                Ok(t) => {
                    let mut st = tieout::TieState::new(&t);
                    if let Some(old) = &self.tie {
                        st.tab = old.tab;
                        st.view.differences_only = old.view.differences_only;
                    }
                    self.tie = Some(st);
                    self.tieout = Some(t);
                }
                Err(e) => self.trouble = Some(Trouble::of(&e)),
            }
        }
        if let Some(r) = poll(&mut self.verify_job) {
            match r {
                Ok(b) => {
                    self.wizard.verified = b.is_verified();
                    if self.wizard.verified {
                        tracing::info!(target: "imports", "import verified");
                        self.trouble = None;
                        // The list comes from this tie-out: verify drops
                        // the rows it was made from (E3).
                        self.spot.build(&b, self.tieout.as_ref(), &self.chart);
                        self.save_spot_check();
                        self.wizard.next(self.today);
                    }
                }
                Err(e) => {
                    let t = Trouble::of(&e);
                    if matches!(t, Trouble::Refused(ImportRefusal::TieOutNotZero { .. })) {
                        // The books moved since: show the engine's rows now.
                        self.load_tieout();
                    }
                    self.trouble = Some(t);
                }
            }
        }
        self.poll_pdf();
        self.poll_spot_check();
        if let Some(r) = poll(&mut self.history_job) {
            match r {
                Ok(list) => self.history = Some(list),
                Err(e) => self.trouble = Some(Trouble::of(&e)),
            }
        }
    }

    pub(crate) fn busy(&self) -> bool {
        self.pick_rx.is_some()
            || self.uploads.values().flatten().any(SlotFile::busy)
            || self.chart_job.is_some()
            || self.batch_create_job.is_some()
            || self.backup_job.is_some()
            || self.job_out.is_some()
            || self.batch_job.is_some()
            || self.running.is_some()
            || self.tieout_job.is_some()
            || self.verify_job.is_some()
            || self.history_job.is_some()
    }

    /// A commit or undo is being made, sent, or runs (per the last read of
    /// its batch).
    pub(crate) fn in_flight(&self) -> bool {
        self.backup_job.is_some() || self.job_out.is_some() || self.running.is_some()
    }

    /// Cancel, Back and Start over are off: something is out, or its
    /// answer was lost (B1).
    pub(crate) fn locked(&self) -> bool {
        self.in_flight() || self.unsettled || self.batch_create_job.is_some()
    }

    /// The model's files, from the cards.
    fn sync_files(&mut self) {
        self.wizard.files = self
            .uploads
            .iter()
            .filter(|(_, v)| !v.is_empty())
            .map(|(k, v)| (*k, v.iter().map(SlotFile::dropped).collect()))
            .collect();
        if self.wizard.source.is_conversion()
            && self.date_text.trim().is_empty()
            && let Some(d) = self.wizard.reference_date()
        {
            self.date_text = show_date(d);
        }
        self.sync_mapping();
    }

    /// Every account the files name, once each (by source key).
    pub(crate) fn matches(&self) -> Vec<&AccountMatch> {
        let mut seen = std::collections::BTreeSet::new();
        self.uploads
            .values()
            .flatten()
            .filter_map(|f| f.outcome.as_ref())
            .flat_map(|o| o.preview.account_matches.iter())
            .filter(|m| seen.insert(m.source_key.as_str()))
            .collect()
    }

    /// Unmatched accounts with no account picked yet.
    pub(crate) fn unmapped(&self) -> usize {
        self.matches()
            .iter()
            .filter(|m| {
                m.status == MatchStatus::Unmatched && !self.account_map.contains_key(&m.source_key)
            })
            .count()
    }

    pub(crate) fn sync_mapping(&mut self) {
        let uploaded = self.uploads.values().flatten().all(|f| f.outcome.is_some());
        self.wizard.mapping_complete = uploaded && self.unmapped() == 0;
    }

    /// The dates as typed, into the model.
    pub(crate) fn sync_dates(&mut self) {
        self.wizard.conversion_date = parse_date(&self.date_text);
        self.wizard.opening_date = parse_date(&self.opening_text);
    }

    /// The step's main button is on.
    pub(crate) fn can_next(&self) -> bool {
        match self.wizard.step {
            Step::Import => {
                !self.locked()
                    && !self.wizard.imported
                    && self.held.is_none()
                    && !self.closing.dialog_open()
                    && self.ask_closed.is_none()
            }
            Step::TieOut => {
                self.tieout.as_ref().is_some_and(|t| t.all_zero)
                    && self.verify_job.is_none()
                    && self.allowed()
            }
            Step::Done => false,
            _ => !self.busy() && self.wizard.can_continue(self.today),
        }
    }

    /// The step's main button.
    pub(crate) fn next(&mut self) {
        if !self.can_next() {
            return;
        }
        match self.wizard.step {
            Step::Import => self.start_import(),
            Step::TieOut => self.verify(),
            Step::SpotCheck => {
                if self.spot.unsaved() {
                    self.save_spot_check();
                }
                self.wizard.next(self.today);
            }
            _ => {
                self.sync_dates();
                if self.wizard.next(self.today) {
                    self.trouble = None;
                }
            }
        }
    }

    /// Back a step: never while something is out or unsettled, never once
    /// the batch is made (its files are pinned; Start over instead), never
    /// back past the import.
    pub(crate) fn can_back(&self) -> bool {
        !self.locked()
            && self.wizard.step != Step::Source
            && (self.wizard.step != Step::Import || self.batch.is_none())
            && (!self.wizard.imported || self.wizard.step > Step::TieOut)
    }

    pub(crate) fn back(&mut self) {
        if self.can_back() && self.wizard.back() {
            self.trouble = None;
        }
    }

    /// Start over: every file not in a batch is discarded; a batch made
    /// and not committed stays as a draft (History lists it).
    pub(crate) fn start_over(&mut self) {
        if self.locked() {
            return;
        }
        self.release_files();
        let source = self.wizard.source;
        let fy = self.wizard.fy_start_month;
        self.wizard = Wizard::new(source, fy);
        self.account_map.clear();
        self.date_text.clear();
        self.opening_text.clear();
        self.batch = None;
        self.held = None;
        self.state = None;
        self.tieout = None;
        self.trouble = None;
        self.status_answer = None;
        self.password = None;
        self.ask_closed = None;
        self.confirm_closed_next = false;
        self.undone = false;
        self.tie = None;
        self.spot = spot_check::SpotList::default();
    }

    /// Discards the uploaded files no batch holds (closing, Start over,
    /// a replaced file). Files in a batch are pinned to it (B1).
    fn release_files(&mut self) {
        if self.batch.is_some() || self.locked() {
            self.uploads.clear();
            return;
        }
        let ids: Vec<ImportSessionId> = self
            .uploads
            .values()
            .flatten()
            .filter_map(SlotFile::session)
            .collect();
        self.uploads.clear();
        for id in ids {
            self.discard(id);
        }
    }

    pub(crate) fn discard(&mut self, id: ImportSessionId) {
        let job = self.write(
            "orphan.import_discard",
            None,
            move |d| d.imports.discard(id),
            move |s| async move { s.imports().discard(id).await },
        );
        self.discard_jobs.push(job);
    }

    pub(crate) fn show_history(&mut self) {
        self.view = View::History;
        self.load_history();
    }

    pub(crate) fn load_history(&mut self) {
        self.history_job = Some(self.read(
            |d| Ok(d.imports.batches().batches),
            |s| async move { s.imports().batches_all().await },
        ));
    }

    /// The demo company's closing date (the demo checks it itself).
    pub(crate) fn demo_closing(d: &crate::company::demo::DemoState) -> Option<CalendarDate> {
        d.open
            .and_then(|o| d.companies.iter().find(|c| c.tenant_id == o))
            .and_then(|c| c.closed_through)
    }
}

impl Drop for ImportScreen {
    /// Closing the window discards the files no batch holds; a commit or
    /// undo out (or whose answer was lost) keeps everything (B1).
    fn drop(&mut self) {
        // The spot-check list is saved on close (its write outlives the
        // window, as the discards do).
        if self.spot.unsaved() {
            self.save_spot_check();
        }
        self.release_files();
    }
}

impl Screen for ImportScreen {
    fn title(&self) -> String {
        t(&format!("cmd.{}", self.kind()))
    }

    fn fixed_layout(&self) -> bool {
        true
    }

    /// Asks before closing only while a commit or undo runs or its answer
    /// was lost, or files were added and nothing imported yet. The batch
    /// is read every frame while its job runs, so this is the engine's
    /// latest word: a job that ended closes without a question.
    fn dirty(&self) -> bool {
        self.in_flight()
            || self.unsettled
            || (!self.wizard.imported && self.uploads.values().any(|v| !v.is_empty()))
    }

    fn save_unconfirmed(&self) -> bool {
        self.unsettled
    }

    fn can_save(&self) -> bool {
        false
    }

    fn close_words(&self) -> Option<crate::windows::CloseWords> {
        let (message, close) = match (self.in_flight(), self.running_kind()) {
            (true, Running::Undo) => ("imports.close.undo_running", "imports.close.quit_anyway"),
            (true, Running::Commit) => ("imports.close.running", "imports.close.quit_anyway"),
            _ => ("imports.close.files", "imports.close.discard"),
        };
        Some(crate::windows::CloseWords {
            message: t(message),
            close: t(close),
            keep: t("imports.close.keep"),
        })
    }

    fn ui(&mut self, ui: &mut eframe::egui::Ui) {
        self.poll();
        if self.busy() || self.spot.saving() || self.spot.unsaved() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        }
        self.draw(ui);
    }

    fn scope(&self) -> cl_model::Scope {
        cl_model::Scope::Form
    }

    fn command(&mut self, id: CommandId) -> bool {
        if let Some(done) = self.closing.command(id) {
            return done;
        }
        let s = id.as_str();
        if let Some(rest) = s.strip_prefix("dev.imports_demo_") {
            return self.link.is_demo() && self.demo_command(rest);
        }
        match s {
            "form.save_new" => {
                self.next();
                true
            }
            "form.step_back" => {
                self.back();
                true
            }
            "form.revert" => {
                if self.confirm_undo.take().is_some() {
                    return true;
                }
                if self.ask_closed.is_some() {
                    self.cancel_closed();
                    return true;
                }
                if self.view == View::History {
                    self.view = View::Import;
                    return true;
                }
                if self.can_back() {
                    self.back();
                    return true;
                }
                false
            }
            _ => false,
        }
    }

    fn state(&self) -> Value {
        let files: usize = self.uploads.values().map(Vec::len).sum();
        let mut s = json!({
            "screen": self.kind(),
            "view": match self.view { View::Import => "import", View::History => "history" },
            "source": self.wizard.source.key(),
            "step": self.wizard.step.key(),
            "steps": self.wizard.source.steps().iter().map(|s| s.key()).collect::<Vec<_>>(),
            "blockers": self.wizard.blockers(self.today).iter().map(cl_model::imports::wizard::Blocker::key).collect::<Vec<_>>(),
            "can_next": self.can_next(),
            "can_back": self.can_back(),
            "busy": self.busy(),
            "files": files,
            "files_uploaded": self.uploads.values().flatten().filter(|f| f.outcome.is_some()).count(),
            "files_refused": self.uploads.values().flatten().filter(|f| f.problem.is_some() || f.refusal.is_some()).count(),
            "matches": self.matches().len(),
            "unmapped": self.unmapped(),
            "depth": self.wizard.depth.key(),
            "conversion_date_set": self.wizard.conversion_date.is_some(),
            "batch": self.batch.is_some(),
            "backing_up": self.backup_job.is_some(),
            "running": self.running.map(|r| match r { Running::Commit => "commit", Running::Undo => "undo" }),
            "progress": self.state.as_ref().and_then(|b| b.progress.as_ref()).map(|p| json!({"done": p.done, "total": p.total})),
            "batch_status": self.state.as_ref().map(|b| b.batch.status.as_wire()),
            "jobs_sent": self.jobs_sent,
            "unsettled": self.unsettled,
            "in_flight": self.in_flight(),
            "imported": self.wizard.imported,
            "verified": self.wizard.verified,
            "tieout_all_zero": self.tieout.as_ref().map(|t| t.all_zero),
            "tieout_accounts": self.tieout.as_ref().map(|t| t.accounts.len()),
            "status_answer": self.status_answer,
            "trouble": self.trouble.as_ref().map(Trouble::code),
            "blocked_by": match &self.trouble {
                Some(Trouble::Refused(ImportRefusal::UndoBlocked { blockers })) => blockers.len(),
                _ => 0,
            },
            "confirm_undo": self.confirm_undo.is_some(),
            "undone": self.undone,
            "allowed": self.allowed(),
            "history": self.history.as_ref().map(|h| json!({
                "rows": h.len(),
                "committed": h.iter().filter(|b| b.status == BatchStatus::Committed).count(),
                "undone": h.iter().filter(|b| b.status == BatchStatus::Undone).count(),
            })),
        });
        if let Value::Object(m) = &mut s
            && let Value::Object(c) = self.closing.state_json(true)
        {
            m.extend(c);
        }
        // Tie-out, spot-check and the D4 flags (one more object: the macro's
        // recursion limit).
        let d4 = json!({
            "tieout_tab": self.tie.as_ref().map(|x| x.tab.key()),
            "tieout_tabs": self.tie.as_ref().map(|x| x.tabs().iter().map(|t| t.key()).collect::<Vec<_>>()),
            "tieout_differences_only": self.tie.as_ref().map(|x| x.view.differences_only),
            "tieout_rows_shown": self.tie.as_ref().map(|x| x.rows(x.tab).len() + x.checks(x.tab).len()),
            "tieout_differences": self.tie.as_ref().map(|x| x.view.differences().len()),
            "pdfs_saved": self.pdfs_saved,
            "spot_items": self.spot.items.len(),
            "spot_done": self.spot.items.iter().filter(|i| i.done).count(),
            "spot_complete": !self.spot.items.is_empty() && self.spot.complete(),
            "spot_unsaved": self.spot.unsaved(),
            "spot_saves": self.spot.saves,
            "ask_closed": self.ask_closed.is_some(),
            "failure": self.failure.as_ref().map(refusal_code),
        });
        if let (Value::Object(m), Value::Object(d)) = (&mut s, d4) {
            m.extend(d);
        }
        s
    }
}
