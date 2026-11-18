//! Import Transactions from a Spreadsheet (P31 D5, Q-125,
//! `file.import_transactions`): invoices, bills and journal entries from a
//! CSV, in one batch the owner can undo.
//!
//! Choose file → Match columns → Check rows → Import → Done.
//!
//! - Every row is checked here first (`cl_model::imports::csv_txn`): exact
//!   amounts, full dates, required cells, groups that mix documents,
//!   journal entries that don't balance. A file with any problem sends
//!   nothing.
//! - The file goes to the engine as the canonical `transactions` CSV; its
//!   own row problems (an account the books don't have) are shown by row
//!   and code, never by the engine's message.
//! - Import and Undo each make a verified backup first
//!   (the shared [`crate::screens::qbd_import::backup_kind`]); a failed
//!   backup sends nothing.
//! - Commit and undo are sent once with a key. A lost answer stops at
//!   "Check status", which reads the batch; only the owner's Try again
//!   sends again, with the same key. While a commit or undo may be running
//!   the upload is never discarded and closing the window asks first.
//! - No sales tax is worked out for spreadsheet invoices (Q-P31i).

use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use cl_api::import_transactions::{
    BatchRunRequest, BatchState, BatchStatus, ImportBatchId, TxnImportRefusal, UndoBlocker,
    UploadOutcome, txn_import_refusal,
};
use cl_api::{ApiError, Capabilities, ClosedPeriodAuthorization, IdempotencyKey, OrphanLabel};
use cl_api::{RequestGroup, Session};
use cl_core::Secret;
use cl_core::ids::TenantId;
use cl_model::CommandId;
use cl_model::imports::csv_txn::{self, DocCounts, MissingColumn, TxnField, TxnMapping, TxnPlan};
use serde_json::{Value, json};

use crate::company::{CompanyEvent, CompanyLink, Task};
use crate::forms::closing_prompt::ClosingPrompt;
use crate::i18n::t;
use crate::windows::Screen;

mod draw;
#[cfg(test)]
mod tests;

/// The command and window kind.
pub const KIND: &str = "file.import_transactions";

/// How often a running import is read.
const POLL_EVERY: Duration = Duration::from_millis(500);

/// The window for `kind`, if it is this one.
#[must_use]
pub fn make(
    kind: CommandId,
    link: &Arc<CompanyLink>,
    open: Option<(TenantId, String)>,
    _today: cl_core::CalendarDate,
) -> Option<Box<dyn Screen>> {
    (kind.as_str() == KIND)
        .then(|| Box::new(CsvTransactionsScreen::new(Arc::clone(link), open)) as Box<_>)
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

/// The wizard's steps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Choose the file.
    Choose,
    /// Match its columns.
    Map,
    /// The rows checked; Import.
    Preview,
    /// Imported (or undone).
    Done,
}

impl Step {
    /// Every step, in order.
    pub const ALL: [Step; 4] = [Step::Choose, Step::Map, Step::Preview, Step::Done];

    fn key(self) -> &'static str {
        match self {
            Step::Choose => "choose",
            Step::Map => "map",
            Step::Preview => "preview",
            Step::Done => "done",
        }
    }
}

/// What a batch run does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Import the file.
    Commit,
    /// Undo the import.
    Undo,
}

/// Where the import (or undo) is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Nothing under way.
    Idle,
    /// The file is going to the books.
    Uploading,
    /// The batch is being made.
    Batching,
    /// The backup before the run is being made.
    BackingUp(Action),
    /// The commit or undo request is out.
    Sending(Action),
    /// The books are running it; the batch is read each half second.
    Running(Action),
    /// The answer was lost: "Check status".
    Unknown(Action),
    /// "Check status" is reading the batch.
    Checking(Action),
}

impl Phase {
    /// A commit or undo may be running in the books.
    #[must_use]
    pub fn may_be_running(self) -> bool {
        matches!(
            self,
            Phase::Sending(_) | Phase::Running(_) | Phase::Unknown(_) | Phase::Checking(_)
        )
    }

    fn code(self) -> &'static str {
        match self {
            Phase::Idle => "idle",
            Phase::Uploading => "uploading",
            Phase::Batching => "batching",
            Phase::BackingUp(_) => "backing_up",
            Phase::Sending(_) => "sending",
            Phase::Running(_) => "running",
            Phase::Unknown(_) => "unknown",
            Phase::Checking(_) => "checking",
        }
    }
}

/// A commit or undo as sent: kept so Try again after a lost answer sends
/// exactly the same request with the same key.
struct Held {
    action: Action,
    key: IdempotencyKey,
    password: Option<Secret>,
    backup_ref: Option<String>,
}

/// What the window explains.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Trouble {
    /// The backup failed: nothing was sent.
    BackupFailed(Action),
    /// Check status found the run never started (Try again, same key).
    NotStarted(Action),
    /// The commit failed; nothing posted.
    Failed {
        /// The owner's row, when one is at fault.
        row: Option<usize>,
        /// The engine code.
        code: String,
        /// The column, when one cell is at fault.
        field: Option<String>,
    },
    /// The file was imported before.
    Already {
        /// When (engine text).
        when: Option<String>,
    },
    /// Another import is running.
    Busy,
    /// Not an Admin.
    AdminRequired,
    /// Later work uses what the import made.
    UndoBlocked(Vec<UndoBlocker>),
    /// Anything else (network, engine).
    Engine,
}

impl Trouble {
    fn code(&self) -> &'static str {
        match self {
            Trouble::BackupFailed(_) => "backup_failed",
            Trouble::NotStarted(_) => "not_started",
            Trouble::Failed { .. } => "failed",
            Trouble::Already { .. } => "already",
            Trouble::Busy => "busy",
            Trouble::AdminRequired => "admin_required",
            Trouble::UndoBlocked(_) => "undo_blocked",
            Trouble::Engine => "engine",
        }
    }
}

/// One problem the books found, on the owner's row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EngineProblem {
    /// The owner's row (0: the file).
    pub row: usize,
    /// The column (engine name).
    pub field: Option<String>,
    /// The engine code.
    pub code: String,
}

/// The Import Transactions window.
pub struct CsvTransactionsScreen {
    link: Arc<CompanyLink>,
    group: Option<RequestGroup>,
    open: Option<(TenantId, String)>,
    pub(crate) step: Step,
    pick_rx: Option<Receiver<Vec<std::path::PathBuf>>>,
    reading: Option<Receiver<FileRead>>,
    pub(crate) file_name: Option<String>,
    pub(crate) file_error: Option<&'static str>,
    pub(crate) headers: Vec<String>,
    rows: Vec<Vec<String>>,
    pub(crate) mapping: TxnMapping,
    pub(crate) used_saved: bool,
    pub(crate) remember: bool,
    pub(crate) plan: Option<TxnPlan>,
    pub(crate) engine_problems: Vec<EngineProblem>,
    upload: Option<UploadOutcome>,
    batch: Option<ImportBatchId>,
    pub(crate) phase: Phase,
    upload_job: Option<Job<UploadOutcome>>,
    batch_job: Option<Job<ImportBatchId>>,
    backup_job: Option<Job<Result<String, cl_company::CompanyError>>>,
    run_job: Option<Job<()>>,
    poll_job: Option<Job<BatchState>>,
    next_poll: Option<Instant>,
    held: Option<Held>,
    /// A commit was sent at least once: the upload is never discarded.
    commit_sent: bool,
    pub(crate) state: Option<BatchState>,
    pub(crate) backup_ref: Option<String>,
    pub(crate) trouble: Option<Trouble>,
    pub(crate) confirm_undo: bool,
    pub(crate) undone: bool,
    pub(crate) closing: ClosingPrompt<Action>,
}

/// A chosen file read off the egui thread: its name and bytes, or an i18n key.
type FileRead = Result<(String, Vec<u8>), &'static str>;

/// Reads a chosen file off the egui thread (up to 10 MB).
fn read_file(path: &std::path::Path) -> FileRead {
    let meta = std::fs::metadata(path).map_err(|_| "txn_import.file.open")?;
    if meta.len() > cl_api::import_transactions::MAX_UPLOAD_BYTES as u64 {
        return Err("txn_import.file.too_large");
    }
    let bytes = std::fs::read(path).map_err(|_| "txn_import.file.open")?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    Ok((name, bytes))
}

impl CsvTransactionsScreen {
    /// A fresh wizard on step 1.
    #[must_use]
    pub fn new(link: Arc<CompanyLink>, open: Option<(TenantId, String)>) -> CsvTransactionsScreen {
        let group = link.group().and_then(Result::ok);
        CsvTransactionsScreen {
            link,
            group,
            open,
            step: Step::Choose,
            pick_rx: None,
            reading: None,
            file_name: None,
            file_error: None,
            headers: Vec::new(),
            rows: Vec::new(),
            mapping: TxnMapping::new(),
            used_saved: false,
            remember: true,
            plan: None,
            engine_problems: Vec::new(),
            upload: None,
            batch: None,
            phase: Phase::Idle,
            upload_job: None,
            batch_job: None,
            backup_job: None,
            run_job: None,
            poll_job: None,
            next_poll: None,
            held: None,
            commit_sent: false,
            state: None,
            backup_ref: None,
            trouble: None,
            confirm_undo: false,
            undone: false,
            closing: ClosingPrompt::default(),
        }
    }

    /// The signed-in user may commit and undo (Admin, U-263).
    pub(crate) fn admin(&self) -> bool {
        self.link.rights().is_admin()
    }

    /// The engine has the batch and `transactions` capabilities (the demo
    /// plays them).
    pub(crate) fn supported(&self) -> bool {
        self.link.is_demo()
            || cl_api::import_transactions::transactions_supported(&self.link.capabilities())
    }

    fn closing_supported(&self) -> bool {
        self.link.is_demo() || self.link.capabilities().has(Capabilities::CLOSING_PASSWORD)
    }

    // ---- Requests ---------------------------------------------------------

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

    /// Takes every answer that arrived.
    pub(crate) fn poll(&mut self) {
        if let Some(paths) = crate::widgets::file_picker::poll(&mut self.pick_rx)
            && let Some(p) = paths.into_iter().next()
        {
            self.take_path(&p);
        }
        if let Some(r) = crate::screens::write_check::attach::poll_file_read(&mut self.reading) {
            match r {
                Ok((name, bytes)) => self.take_file(name, &bytes),
                Err(key) => self.file_error = Some(key),
            }
        }
        if let Some(r) = poll(&mut self.upload_job) {
            self.after_upload(r);
        }
        if let Some(r) = poll(&mut self.batch_job) {
            self.after_batch(r);
        }
        if let Some(r) = poll(&mut self.backup_job) {
            self.after_backup(r.map_err(cl_company::CompanyError::from).and_then(|x| x));
        }
        if let Some(r) = poll(&mut self.run_job) {
            self.after_run(r);
        }
        if let Some(r) = poll(&mut self.poll_job) {
            self.after_poll(r);
        }
        self.maybe_read_batch();
    }

    /// Something is under way (the frame repaints while it is).
    pub(crate) fn busy(&self) -> bool {
        !matches!(self.phase, Phase::Idle | Phase::Unknown(_))
            || self.reading.is_some()
            || self.pick_rx.is_some()
    }

    // ---- Step 1: the file ---------------------------------------------------

    /// Opens the system file window.
    pub(crate) fn browse(&mut self, ctx: &eframe::egui::Context) {
        use crate::widgets::file_picker::{PickUse, TXN_IMPORT_FILES, pick_files};
        self.pick_rx = Some(pick_files(
            ctx,
            PickUse::TxnImportFile,
            &[TXN_IMPORT_FILES],
            false,
        ));
    }

    fn take_path(&mut self, p: &std::path::Path) {
        self.file_error = None;
        match crate::screens::write_check::attach::read_in_background(p, read_file) {
            Ok(rx) => self.reading = Some(rx),
            Err(_) => self.file_error = Some("txn_import.file.open"),
        }
    }

    /// Reads a file's cells and guesses (or recalls) its columns.
    pub(crate) fn take_file(&mut self, name: String, bytes: &[u8]) {
        if self.phase.may_be_running() || self.step == Step::Done {
            return;
        }
        self.forget_upload();
        self.plan = None;
        self.trouble = None;
        self.engine_problems.clear();
        if bytes.len() > cl_api::import_transactions::MAX_UPLOAD_BYTES {
            self.file_error = Some("txn_import.file.too_large");
            return;
        }
        let text = cl_import::csv::decode(bytes);
        let Ok(mut rows) = cl_import::csv::parse(&text) else {
            self.file_error = Some("txn_import.file.unreadable");
            return;
        };
        let headers = rows.remove(0).fields;
        if !cl_model::forms::batch::looks_like_header(
            &headers.iter().map(String::as_str).collect::<Vec<_>>(),
        ) {
            self.file_error = Some("txn_import.file.no_header");
            return;
        }
        self.file_error = None;
        self.rows = rows.into_iter().map(|r| r.fields).collect();
        let h: Vec<&str> = headers.iter().map(String::as_str).collect();
        let saved = self
            .mappings_store()
            .zip(self.company_key())
            .and_then(|(s, c)| s.load().get(&c, &csv_txn::header_signature(&h)).cloned());
        self.used_saved = saved.is_some();
        self.mapping = saved.map_or_else(
            || csv_txn::guess_mapping(&h),
            |m| csv_txn::mapping_from_saved(&h, &m),
        );
        self.headers = headers;
        self.file_name = Some(name);
        self.step = Step::Map;
        tracing::info!(target: "txn_import", rows = self.rows.len(), "file read");
    }

    fn mappings_store(&self) -> Option<cl_settings::TxnMappingsStore> {
        self.link
            .store()
            .path()
            .parent()
            .map(cl_settings::TxnMappingsStore::in_dir)
    }

    fn company_key(&self) -> Option<String> {
        self.open.as_ref().map(|(t, _)| t.to_string())
    }

    // ---- Step 2: columns ------------------------------------------------------

    /// What the mapping still needs.
    pub(crate) fn gaps(&self) -> Vec<MissingColumn> {
        csv_txn::mapping_gaps(&self.mapping)
    }

    /// Sets (or clears) a field's column.
    pub(crate) fn set_column(&mut self, field: TxnField, column: Option<usize>) {
        match column {
            Some(c) => self.mapping.insert(field, c),
            None => self.mapping.remove(&field),
        };
        self.plan = None;
    }

    /// Checks every row with the chosen columns (the next step).
    pub(crate) fn check_rows(&mut self) {
        if !self.gaps().is_empty() {
            return;
        }
        let plan = csv_txn::build(&self.rows, 2, &self.mapping);
        tracing::info!(
            target: "txn_import",
            documents = plan.documents.len(),
            problems = plan.problems.len(),
            "rows checked"
        );
        self.plan = Some(plan);
        self.engine_problems.clear();
        self.step = Step::Preview;
    }

    fn save_mapping(&self) {
        if !self.remember {
            return;
        }
        let (Some(store), Some(company)) = (self.mappings_store(), self.company_key()) else {
            return;
        };
        let h: Vec<&str> = self.headers.iter().map(String::as_str).collect();
        let mut all = store.load();
        all.put(
            &company,
            &csv_txn::header_signature(&h),
            csv_txn::mapping_to_saved(&h, &self.mapping),
        );
        if store.save(&all).is_err() {
            tracing::warn!(target: "txn_import", "column mapping not saved");
        }
    }

    // ---- Step 3: import -------------------------------------------------------

    /// Import: the file goes up, a batch is made, then backup and commit.
    pub(crate) fn start_import(&mut self) {
        let Some(plan) = self.plan.as_ref().filter(|p| p.can_import()) else {
            return;
        };
        if self.phase != Phase::Idle || !self.admin() || !self.supported() {
            return;
        }
        self.trouble = None;
        self.engine_problems.clear();
        self.save_mapping();
        if self.batch.is_some() {
            // A failed commit, or a refused one: run again, new key.
            self.begin(Action::Commit, None);
            return;
        }
        let csv = plan.canonical_csv().into_bytes();
        let accounts: Vec<String> = crate::company::demo::chart()
            .into_iter()
            .map(|a| a.fields.name)
            .collect();
        let demo_csv = csv.clone();
        self.phase = Phase::Uploading;
        self.upload_job = Some(self.write(
            "orphan.txn_import_upload",
            None,
            move |d| d.txn_import.upload(&demo_csv, &accounts),
            move |s| async move { s.import_transactions().upload(&csv).await },
        ));
    }

    fn after_upload(&mut self, r: Result<UploadOutcome, ApiError>) {
        match r {
            Ok(u) if !u.problems.is_empty() => {
                let plan = self.plan.as_ref();
                self.engine_problems = u
                    .problems
                    .iter()
                    .map(|p| EngineProblem {
                        row: usize::try_from(p.row)
                            .ok()
                            .and_then(|r| plan.and_then(|pl| pl.source_row(r)))
                            .unwrap_or(0),
                        field: p.field.clone(),
                        code: p.code.clone(),
                    })
                    .collect();
                tracing::info!(target: "txn_import", problems = u.problems.len(), "the books found row problems");
                // Nothing can be committed from it: let it go.
                self.upload = Some(u);
                self.forget_upload();
                self.phase = Phase::Idle;
            }
            Ok(u) => {
                let id = u.session;
                self.upload = Some(u);
                self.phase = Phase::Batching;
                self.batch_job = Some(self.write(
                    "orphan.txn_import_upload",
                    None,
                    move |d| d.txn_import.create_batch(id),
                    move |s| async move { s.import_transactions().create_batch(id).await },
                ));
            }
            Err(e) => {
                self.phase = Phase::Idle;
                self.refused(&e);
            }
        }
    }

    fn after_batch(&mut self, r: Result<ImportBatchId, ApiError>) {
        match r {
            Ok(id) => {
                self.batch = Some(id);
                self.phase = Phase::Idle;
                self.begin(Action::Commit, None);
            }
            Err(e) => {
                self.phase = Phase::Idle;
                self.refused(&e);
            }
        }
    }

    /// A commit or undo: a new key, then the backup, then the request.
    fn begin(&mut self, action: Action, password: Option<Secret>) {
        if self.batch.is_none() || !matches!(self.phase, Phase::Idle) {
            return;
        }
        self.trouble = None;
        self.held = Some(Held {
            action,
            key: IdempotencyKey::new(),
            password,
            backup_ref: None,
        });
        self.start_backup(action);
    }

    /// The verified backup before a run, off the egui thread. Anything that
    /// stops it from starting is a failed backup, never a run without one.
    fn start_backup(&mut self, action: Action) {
        self.phase = Phase::BackingUp(action);
        let company = self.open.as_ref().map_or_else(String::new, |o| o.1.clone());
        if let Some(mut d) = self.link.demo_state() {
            let r =
                d.txn_import
                    .backup(&company)
                    .map_err(|()| cl_company::CompanyError::JobFailed {
                        code: "DEMO".into(),
                    });
            self.backup_job = Some(Job::Done(Some(Ok(r))));
            return;
        }
        let unavailable =
            || cl_company::CompanyError::Api(ApiError::EngineUnavailable { permanent: false });
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            self.after_backup(Err(unavailable()));
            return;
        };
        let store = self.link.store().clone();
        let secrets = self.link.secrets();
        let default_dir = self.link.backup_dir.clone();
        let task = crate::company::spawn_lifecycle(&self.link, group, move || async move {
            let cl_api::Scope::Company { tenant_id, .. } = session.scope() else {
                return Err(cl_company::CompanyError::UnknownCompany);
            };
            let dest_dir = store
                .load()
                .ok()
                .and_then(|l| l.backup_folder)
                .unwrap_or(default_dir);
            cl_company::backup(
                &session,
                &store,
                secrets.as_ref(),
                &cl_company::BackupRequest {
                    tenant_id,
                    kind: crate::screens::qbd_import::backup_kind(),
                    dest_dir,
                    apply_retention: crate::screens::qbd_import::BACKUP_RETENTION,
                },
                &cl_company::backup::silent(),
            )
            .await
            .map(|b| {
                b.record
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default()
            })
        });
        match task {
            Ok(t) => self.backup_job = Some(Job::Running(t)),
            Err(_) => self.after_backup(Err(unavailable())),
        }
    }

    fn after_backup(&mut self, r: Result<String, cl_company::CompanyError>) {
        let Phase::BackingUp(action) = self.phase else {
            return;
        };
        match r {
            Ok(file) if !file.is_empty() => {
                tracing::info!(target: "txn_import", "backup made before the run");
                if let Some(h) = self.held.as_mut() {
                    h.backup_ref = Some(file);
                }
                self.send();
            }
            Ok(_) | Err(_) => {
                tracing::warn!(target: "txn_import", "backup before the run failed; nothing sent");
                self.held = None;
                self.phase = Phase::Idle;
                self.trouble = Some(Trouble::BackupFailed(action));
            }
        }
    }

    /// Sends the held run (exactly as held: Try again uses the same key).
    fn send(&mut self) {
        let (Some(h), Some(batch)) = (self.held.as_ref(), self.batch) else {
            self.phase = Phase::Idle;
            return;
        };
        let Some(backup_ref) = h.backup_ref.clone() else {
            self.phase = Phase::Idle;
            self.trouble = Some(Trouble::BackupFailed(h.action));
            return;
        };
        let action = h.action;
        let key = h.key.clone();
        let password = h.password.clone();
        let req = BatchRunRequest {
            backup_ref: backup_ref.clone(),
            closed_period: password
                .clone()
                .map(ClosedPeriodAuthorization::with_password),
        };
        let (k_demo, k_real) = (key.clone(), key.clone());
        self.phase = Phase::Sending(action);
        if action == Action::Commit {
            self.commit_sent = true;
            self.backup_ref = Some(backup_ref.clone());
        }
        self.run_job = Some(match action {
            Action::Commit => self.write(
                "orphan.txn_import_commit",
                Some(key),
                move |d| {
                    let closing = crate::company::demo::home::closing_date(d);
                    d.txn_import
                        .commit(batch, &backup_ref, &k_demo, password.as_ref(), closing)
                },
                move |s| async move { s.import_transactions().commit(batch, &req, k_real).await },
            ),
            Action::Undo => self.write(
                "orphan.txn_import_undo",
                Some(key),
                move |d| {
                    let closing = crate::company::demo::home::closing_date(d);
                    d.txn_import
                        .undo(batch, &k_demo, password.as_ref(), closing)
                },
                move |s| async move { s.import_transactions().undo(batch, &req, k_real).await },
            ),
        });
    }

    fn after_run(&mut self, r: Result<(), ApiError>) {
        let Phase::Sending(action) = self.phase else {
            return;
        };
        match r {
            Ok(()) => {
                self.closing.saved();
                self.phase = Phase::Running(action);
                self.next_poll = Some(Instant::now());
            }
            Err(ApiError::OutcomeUnknown { .. }) => {
                tracing::warn!(target: "txn_import", "answer lost; waiting for Check status");
                self.closing.nothing_sent();
                self.phase = Phase::Unknown(action);
            }
            Err(e) => {
                self.held = None;
                self.phase = Phase::Idle;
                let supported = self.closing_supported();
                if !self.closing.failed(&e, action, supported, None) {
                    self.refused(&e);
                }
            }
        }
    }

    fn refused(&mut self, e: &ApiError) {
        tracing::info!(target: "txn_import", detail = %cl_core::OwnerFacing::scrubbed_detail(e), "refused");
        self.trouble = Some(match txn_import_refusal(e) {
            Some(TxnImportRefusal::AlreadyCommitted { committed_at, .. }) => {
                Trouble::Already { when: committed_at }
            }
            Some(TxnImportRefusal::Busy) => Trouble::Busy,
            Some(TxnImportRefusal::AdminRequired) => Trouble::AdminRequired,
            Some(TxnImportRefusal::UndoBlocked { blockers }) => Trouble::UndoBlocked(blockers),
            Some(TxnImportRefusal::SessionInBatch) | None => Trouble::Engine,
        });
    }

    fn maybe_read_batch(&mut self) {
        let reading = matches!(self.phase, Phase::Running(_) | Phase::Checking(_));
        let due = self.next_poll.is_some_and(|t| Instant::now() >= t);
        let Some(id) = self
            .batch
            .filter(|_| reading && due && self.poll_job.is_none())
        else {
            return;
        };
        self.next_poll = None;
        self.poll_job = Some(self.read(
            move |d| d.txn_import.batch(id),
            move |s| async move { s.import_transactions().batch(id).await },
        ));
    }

    fn after_poll(&mut self, r: Result<BatchState, ApiError>) {
        let (action, checking) = match self.phase {
            Phase::Running(a) => (a, false),
            Phase::Checking(a) => (a, true),
            _ => return,
        };
        let st = match r {
            Ok(st) => st,
            Err(_) if checking => {
                self.phase = Phase::Unknown(action);
                self.trouble = Some(Trouble::Engine);
                return;
            }
            Err(_) => {
                // A read that failed while running: read again shortly.
                self.next_poll = Some(Instant::now() + POLL_EVERY);
                return;
            }
        };
        let status = st.status;
        self.state = Some(st.clone());
        match (action, status) {
            (_, s) if s.running() => {
                self.phase = Phase::Running(action);
                self.next_poll = Some(Instant::now() + POLL_EVERY);
            }
            (Action::Commit, BatchStatus::Committed) => {
                tracing::info!(target: "txn_import", "import committed");
                self.held = None;
                self.phase = Phase::Idle;
                self.step = Step::Done;
                self.publish_changes();
            }
            (Action::Undo, BatchStatus::Undone) => {
                tracing::info!(target: "txn_import", "import undone");
                self.held = None;
                self.phase = Phase::Idle;
                self.undone = true;
                self.publish_changes();
            }
            (Action::Commit, BatchStatus::Failed) => {
                self.held = None;
                self.phase = Phase::Idle;
                let e = st.error.clone();
                self.trouble = Some(Trouble::Failed {
                    row: e
                        .as_ref()
                        .and_then(|e| e.row)
                        .and_then(|r| usize::try_from(r).ok())
                        .and_then(|r| self.plan.as_ref().and_then(|p| p.source_row(r))),
                    code: e.as_ref().map_or_else(String::new, |e| e.code.clone()),
                    field: e.and_then(|e| e.field),
                });
            }
            // Check status found the run never started: Try again sends
            // the same request with the same key.
            (Action::Commit, BatchStatus::Draft) | (Action::Undo, BatchStatus::Committed)
                if checking =>
            {
                self.phase = Phase::Unknown(action);
                self.trouble = Some(Trouble::NotStarted(action));
            }
            _ => {
                self.held = None;
                self.phase = Phase::Idle;
                self.trouble = Some(Trouble::Engine);
            }
        }
    }

    /// The import posted or removed documents (and maybe names).
    fn publish_changes(&self) {
        let bus = self.link.bus();
        bus.publish(crate::data_bus::Change::Ledger(None));
        bus.publish(crate::data_bus::Change::Contacts);
    }

    /// "Check status" after a lost answer: reads the batch.
    pub(crate) fn check_status(&mut self) {
        if let Phase::Unknown(a) = self.phase {
            self.trouble = None;
            self.phase = Phase::Checking(a);
            self.next_poll = Some(Instant::now());
        }
    }

    /// Try again: after a failed backup, a new backup and run; after
    /// "not started", the same request with the same key.
    pub(crate) fn try_again(&mut self) {
        match (self.phase, self.trouble.clone()) {
            (Phase::Unknown(_), Some(Trouble::NotStarted(_))) if self.held.is_some() => {
                self.trouble = None;
                self.send();
            }
            (Phase::Idle, Some(Trouble::BackupFailed(a))) => {
                self.trouble = None;
                self.begin(a, None);
            }
            (Phase::Idle, Some(Trouble::Failed { .. })) => {
                self.trouble = None;
                self.begin(Action::Commit, None);
            }
            _ => {}
        }
    }

    /// The closing-date password was given: a new backup and run with it.
    pub(crate) fn submit_password(&mut self) {
        let Some((action, secret)) = self.closing.submit() else {
            return;
        };
        if self.phase != Phase::Idle {
            self.closing.nothing_sent();
            return;
        }
        self.begin(action, Some(secret));
        if !matches!(self.phase, Phase::BackingUp(_) | Phase::Sending(_)) {
            self.closing.nothing_sent();
        }
    }

    // ---- Done: undo --------------------------------------------------------------

    /// "Undo this import": asks first.
    pub(crate) fn ask_undo(&mut self) {
        if self.step == Step::Done && !self.undone && self.phase == Phase::Idle && self.admin() {
            self.confirm_undo = true;
        }
    }

    /// The owner confirmed Undo: backup, then undo.
    pub(crate) fn undo(&mut self) {
        if !self.confirm_undo {
            return;
        }
        self.confirm_undo = false;
        self.begin(Action::Undo, None);
    }

    // ---- Navigation ----------------------------------------------------------------

    /// Next (Alt+N).
    pub(crate) fn next(&mut self) {
        match self.step {
            Step::Map => self.check_rows(),
            Step::Preview => self.start_import(),
            Step::Choose | Step::Done => {}
        }
    }

    /// Back (Alt+B): never while a run may be under way.
    pub(crate) fn back(&mut self) {
        if self.phase != Phase::Idle || self.commit_sent {
            return;
        }
        self.step = match self.step {
            Step::Map => Step::Choose,
            Step::Preview => Step::Map,
            s => s,
        };
        self.trouble = None;
    }

    /// Starts over with another file (after Done, or before importing).
    pub(crate) fn start_over(&mut self) {
        if self.phase.may_be_running() || matches!(self.phase, Phase::BackingUp(_)) {
            return;
        }
        let keep = (Arc::clone(&self.link), self.open.clone());
        self.forget_upload();
        *self = CsvTransactionsScreen::new(keep.0, keep.1);
    }

    /// Lets go of an upload that was never committed (never once a commit
    /// was sent: it may be in the books).
    fn forget_upload(&mut self) {
        if self.commit_sent {
            return;
        }
        if let Some(u) = self.upload.take() {
            let id = u.session;
            if let Some(mut d) = self.link.demo_state() {
                d.txn_import.discard(id);
            } else if let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref())
            {
                let _ = group.write(
                    &session,
                    OrphanLabel {
                        what: "orphan.txn_import_upload",
                        key: None,
                    },
                    move |s| async move { s.import_transactions().discard(id).await },
                );
            }
        }
        self.batch = None;
    }

    /// Cancel: closes, unless a run may be under way.
    pub(crate) fn cancel(&mut self) {
        if self.phase.may_be_running() || matches!(self.phase, Phase::BackingUp(_)) {
            return;
        }
        self.link
            .push(CompanyEvent::CloseWindow(CommandId::new(KIND)));
    }

    /// The counts the Done step reports.
    pub(crate) fn counts(&self) -> DocCounts {
        self.plan.as_ref().map(TxnPlan::counts).unwrap_or_default()
    }

    fn demo_file(&mut self, stem: &str) -> bool {
        let Some(bytes) = crate::company::demo::import_transactions::fixture_bytes(stem) else {
            return false;
        };
        self.take_file(format!("{stem}.csv"), bytes);
        true
    }

    fn dev_command(&mut self, s: &str) -> Option<bool> {
        if let Some(stem) = s.strip_prefix("dev.txn_import_demo_file_") {
            return Some(self.link.is_demo() && self.demo_file(stem));
        }
        let mut d = self.link.demo_state()?;
        match s {
            "dev.txn_import_demo_reset" => {
                d.txn_import.reset();
                if let Some(open) = d.open
                    && let Some(c) = d.companies.iter_mut().find(|c| c.tenant_id == open)
                {
                    c.closed_through = None;
                }
                drop(d);
                self.start_over();
            }
            "dev.txn_import_demo_close_books" => {
                if let Some(open) = d.open
                    && let Some(c) = d.companies.iter_mut().find(|c| c.tenant_id == open)
                {
                    c.closed_through = cl_core::CalendarDate::from_ymd(2026, 10, 31).ok();
                }
            }
            "dev.txn_import_demo_lose_answer" => d.txn_import.lose_next_answer = true,
            "dev.txn_import_demo_backup_fails" => d.txn_import.fail_next_backup = true,
            _ => return None,
        }
        Some(true)
    }
}

impl Drop for CsvTransactionsScreen {
    /// Closing lets go of an upload no commit was sent for.
    fn drop(&mut self) {
        self.forget_upload();
    }
}

impl Screen for CsvTransactionsScreen {
    fn title(&self) -> String {
        t(&format!("cmd.{KIND}"))
    }

    fn fixed_layout(&self) -> bool {
        true
    }

    /// Only while a commit or undo may be running (P30 lesson): closing
    /// then asks, and keeps the import.
    fn dirty(&self) -> bool {
        self.phase.may_be_running() || matches!(self.phase, Phase::BackingUp(_))
    }

    fn save_unconfirmed(&self) -> bool {
        matches!(self.phase, Phase::Unknown(_) | Phase::Sending(_))
    }

    fn can_save(&self) -> bool {
        false
    }

    fn close_words(&self) -> Option<crate::windows::CloseWords> {
        Some(crate::windows::CloseWords {
            message: t("txn_import.close_prompt"),
            close: t("txn_import.close_prompt.discard"),
            keep: t("txn_import.close_prompt.keep"),
        })
    }

    fn ui(&mut self, ui: &mut eframe::egui::Ui) {
        self.poll();
        if self.busy() {
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }
        self.draw(ui);
    }

    fn scope(&self) -> cl_model::Scope {
        cl_model::Scope::Form
    }

    fn command(&mut self, id: CommandId) -> bool {
        if let Some(handled) = self.closing.command(id) {
            return handled;
        }
        let s = id.as_str();
        if let Some(r) = self.dev_command(s) {
            return r;
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
                if self.confirm_undo {
                    self.confirm_undo = false;
                    return true;
                }
                match self.step {
                    Step::Map | Step::Preview if self.phase == Phase::Idle => {
                        self.back();
                        true
                    }
                    _ => false,
                }
            }
            _ => false,
        }
    }

    fn state(&self) -> Value {
        let plan = self.plan.as_ref();
        let c = self.counts();
        json!({
            "screen": KIND,
            "step": self.step.key(),
            "phase": self.phase.code(),
            "file": self.file_name.is_some(),
            "file_error": self.file_error,
            "columns": self.headers.len(),
            "mapped": self.mapping.len(),
            "gaps": self.gaps().len(),
            "used_saved": self.used_saved,
            "documents": plan.map_or(0, |p| p.documents.len()),
            "invoices": c.invoices,
            "bills": c.bills,
            "journal_entries": c.journal_entries,
            "problems": plan.map_or(0, |p| p.problems.len()),
            "engine_problems": self.engine_problems.len(),
            "can_import": plan.is_some_and(TxnPlan::can_import) && self.admin(),
            "admin": self.admin(),
            "batch_status": self.state.as_ref().map(|s| s.status.as_wire()),
            "trouble": self.trouble.as_ref().map(Trouble::code),
            "backup_made": self.backup_ref.is_some(),
            "confirm_undo": self.confirm_undo,
            "undone": self.undone,
            "closing": self.closing.state_json(self.closing_supported()),
        })
    }
}
