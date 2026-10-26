//! The intake inbox behind the window (A01 C1): every intake folder of the
//! open company, watched on its own thread, each file copied in as soon as
//! it stopped changing. One per app (kept in the egui context), alive while
//! a company is open, whether or not the inbox window is.
//!
//! - **Folders** come from the engine (`session.intake()`, U-93), read when
//!   the company opens, after every Preferences save
//!   ([`intake_folders::saves`]) and every [`ENGINE_REFRESH`]. A folder is
//!   watched ([`WatchHandle`]) only when this computer confirmed it at its
//!   current path (`CompanyPrefs::intake_here`) and no other company on
//!   this computer uses it; otherwise it shows as `NotConfirmed` or
//!   `MappedByAnother` and is never touched.
//! - **Arrival** (Q-288): a ready file that has no entry is read on its
//!   folder's thread and, unless the same bytes came in before (then it is
//!   one `duplicate` entry, never uploaded on its own), uploaded as a draft
//!   attachment under its own `DraftKey`. One file at a time per company;
//!   the entry is saved as `uploading` (draft key and upload key) before
//!   the upload is sent. After a restart an `uploading` entry is resolved
//!   by listing its draft: found → `arrived`, none → sent again into the
//!   same draft (a fresh idempotency key: keys can't outlive the app).
//! - **Handling**: a saved form that took the arrival's draft, or **Add
//!   to…** (`attachments.link`), makes it `linked` (or `kept` in a folder
//!   that leaves originals where they are) and the original moves to
//!   `Processed/YYYY-MM/`. **Discard** makes it `discarding`: the original
//!   moves to `Discarded/`, then only the engine's copy is deleted. Every
//!   status is saved before the next side effect; a failed move is tried
//!   again and the original stays where it is. Nothing here deletes a file.
//! - **Migration** (C4): on company open, after the session is up and
//!   before the first look, P17's single folder becomes an intake folder
//!   (`cl_company::intake::run_migration` over the company record and
//!   `migrate_inbox::plan`). Should it fail, the P17 folder keeps working
//!   for this session through the legacy service and Home shows a Needs
//!   attention line.
//!
//! Nothing here logs, or puts in `Debug` or the control channel, a folder
//! path, a folder name or a file name.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::{Duration, Instant};

use cl_api::{
    ApiError, AttachTo, Attachment, AttachmentId, ConflictReason, DraftKey, IdempotencyKey,
    IntakeSettings, LocalValidation, OrphanLabel, PageRequest, RequestGroup, UserText,
};
use cl_company::intake::{
    FileError, HandledDir, Health, IntakeEngine, IntakeFile, ItemState, Listing, LocalIntake,
    MigrateError, MigrationOutcome, MoveJob, PlannedMigration, ReadFile, RealFs, Snapshot,
    WatchConfig, WatchHandle, check_path_syntax, run_migration,
};
use cl_core::CalendarDate;
use cl_core::ids::{IntakeFolderId, TenantId};
use cl_core::intake::IntakeFolder;
use cl_settings::inbox_state::{
    EntryStatus, FileIdentity, InboxState, InboxStateStore, company_key, path_hash,
};
use cl_settings::{HereNote, RecentStore};
use eframe::egui::{self, Context};

use super::service::{Upload, upload_of};
use crate::company::{CompanyLink, Task};
use crate::screens::preferences::intake_folders::{self, source};

/// How often the folder list is read again from the engine.
pub(crate) const ENGINE_REFRESH: Duration = Duration::from_secs(60);
/// How often uploads on open forms are checked (and at once after any
/// save anywhere in the app).
const RECONCILE_EVERY: Duration = Duration::from_secs(10);
/// The wait before a failed move, delete or upload is tried again.
const RETRY: Duration = Duration::from_secs(30);
/// Uploads checked per engine request when the company opens.
const SWEEP_BATCH: usize = 10;

/// Milliseconds since 1970, now.
fn now_ms() -> i64 {
    cl_settings::recent::now().unix_millis()
}

/// A file as the inbox state keys it before its bytes are known.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct FileKey {
    path_hash: String,
    size: u64,
    modified_ms: i64,
}

impl FileKey {
    fn of(f: &IntakeFile) -> FileKey {
        FileKey {
            path_hash: path_hash(&f.path),
            size: f.size,
            modified_ms: f.modified_ms,
        }
    }

    fn matches(&self, id: &FileIdentity) -> bool {
        id.matches(&self.path_hash, self.size, self.modified_ms)
    }
}

/// Which form an arrival opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FormKind {
    /// Enter Bill.
    Bill,
    /// Write Check.
    Check,
}

/// Where a row of the inbox stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum RowState {
    /// Still being written (or not yet unchanged for 5 s).
    Settling,
    /// Ready, waiting for its turn to be copied in.
    Waiting,
    /// Being copied in.
    Copying,
    /// Copied in; waiting to be handled.
    Arrived,
    /// Copied in and on a form that is open.
    OnForm,
    /// The same bytes came in before.
    Duplicate,
    /// Handled; the original is on its way to `Processed/` or `Discarded/`.
    Moving,
    /// Older than "only files added from now on": brought in on request.
    BringIn,
    /// Over 10 MB (never read).
    TooLarge,
    /// HEIC/HEIF (never read).
    NotSupported,
    /// A cloud placeholder (never read).
    OnlineOnly,
    /// A hard-linked file (never read).
    LinkedFile,
    /// Not a PDF or picture by its bytes.
    Refused,
}

impl RowState {
    /// A short name for the control channel.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            RowState::Settling => "settling",
            RowState::Waiting => "waiting",
            RowState::Copying => "copying",
            RowState::Arrived => "arrived",
            RowState::OnForm => "on_form",
            RowState::Duplicate => "duplicate",
            RowState::Moving => "moving",
            RowState::BringIn => "bring_in",
            RowState::TooLarge => "too_large",
            RowState::NotSupported => "not_supported",
            RowState::OnlineOnly => "online_only",
            RowState::LinkedFile => "linked_file",
            RowState::Refused => "refused",
        }
    }
}

/// One row of the inbox.
#[derive(Clone)]
pub(crate) struct Row {
    /// The folder (an index into [`Intake::runs`]).
    pub run: usize,
    /// The file.
    pub file: IntakeFile,
    /// Where it stands.
    pub state: RowState,
    /// Its inbox entry, once it has one.
    pub entry: Option<FileIdentity>,
    /// Duplicate: when the first copy came in (ms since 1970).
    pub seen_at: Option<i64>,
}

/// Kind and state only.
impl std::fmt::Debug for Row {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Row")
            .field("run", &self.run)
            .field("state", &self.state)
            .field("kind", &self.file.kind)
            .finish_non_exhaustive()
    }
}

/// One intake folder as the inbox sees it.
pub(crate) struct Run {
    /// The folder as the engine stores it (owner data: never logged).
    pub folder: IntakeFolder,
    watch: Option<WatchHandle>,
    /// The root and pin the watcher runs with.
    watched: Option<(PathBuf, String)>,
    /// The last look.
    pub snapshot: Option<Snapshot>,
    /// Why it isn't watched (`NotConfirmed`, `MappedByAnother`), else
    /// `None`.
    blocked: Option<Health>,
    /// "Only files added from now on" (Q-A01h).
    since_ms: Option<i64>,
}

impl Run {
    /// How the folder is doing.
    pub(crate) fn health(&self) -> Health {
        if let Some(h) = self.blocked {
            return h;
        }
        self.snapshot.as_ref().map_or(Health::Ok, |s| s.health)
    }

    /// True when it is watched.
    pub(crate) fn watched(&self) -> bool {
        self.watch.is_some()
    }

    /// The last listing, when the folder could be listed.
    pub(crate) fn listing(&self) -> Option<&Listing> {
        self.snapshot.as_ref().and_then(|s| s.listing.as_ref().ok())
    }

    /// Look at the folder now.
    pub(crate) fn poke(&self) {
        if let Some(w) = &self.watch {
            w.poke();
        }
    }

    /// Reads `path` on the folder's thread.
    pub(crate) fn read(&self, path: PathBuf) -> Option<Receiver<Result<ReadFile, FileError>>> {
        self.watch.as_ref().map(|w| w.read(path))
    }
}

/// Where the migration and the first read stand.
enum Stage {
    /// No company.
    Idle,
    /// The company just opened: migrate, then read the folders.
    Start,
    /// The demo is laying out its folders (`--demo` only).
    Seeding(Receiver<super::demo::Seeded>),
    /// P17's folder is moving over.
    Migrating(MigJob),
    /// Watching.
    Running,
}

enum MigJob {
    Queue(Task<Result<MigrationOutcome, MigrateError>>),
    Thread(Receiver<Result<MigrationOutcome, MigrateError>>),
}

/// How the migration ended (control channel).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Migration {
    /// Nothing to migrate.
    None,
    /// Running.
    Running,
    /// P17's folder is now an intake folder.
    Migrated,
    /// It can't be moved over yet (too many folders, a non-UTF-8 path).
    Blocked,
    /// It failed; the P17 folder keeps working this session.
    Failed,
}

impl Migration {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Migration::None => "none",
            Migration::Running => "running",
            Migration::Migrated => "migrated",
            Migration::Blocked => "blocked",
            Migration::Failed => "failed",
        }
    }
}

/// The one copy in flight.
enum Copy {
    /// An `uploading` entry left from before: is its draft there?
    Resolving {
        identity: FileIdentity,
        task: Task<Option<AttachmentId>>,
    },
    /// Reading the file on its folder's thread.
    Reading {
        folder: IntakeFolderId,
        file: IntakeFile,
        rx: Receiver<Result<ReadFile, FileError>>,
        /// The `uploading` entry this read is for (its draft is reused).
        resume: Option<FileIdentity>,
    },
    /// Uploading.
    Uploading {
        identity: FileIdentity,
        size: u64,
        task: Task<Attachment>,
    },
}

/// What a link or delete in flight is for.
enum JobKind {
    /// `Add to…` onto a saved record.
    LinkRecord(FileIdentity),
    /// A P17 upload linked into a fresh draft before its form opens.
    LinkDraft(FileIdentity, DraftKey, FormKind),
    /// Deleting the engine's copy of a discarded file; `keep`: the folder
    /// leaves originals where they are (the entry becomes `kept`).
    Delete(FileIdentity, bool),
}

struct Job {
    kind: JobKind,
    task: Task<()>,
}

/// A form opened with an arrival's draft: the form holds the claim, the
/// inbox only a weak copy, so a closed form frees the file (A01 D4: two
/// forms on one arrival draft are prevented in memory).
pub(crate) type Claim = Arc<()>;

/// A form ready to open (after a P17 upload was linked into a draft).
#[derive(Clone)]
pub(crate) struct FormReady {
    pub kind: FormKind,
    pub entry: FileIdentity,
    pub draft: DraftKey,
    pub attachment: AttachmentId,
}

/// The folder list and the user's role, as read.
type FoldersRead = Result<(source::Stored, Option<String>), ApiError>;

/// Why an action was refused at once.
pub(crate) type ActionResult = Result<(), &'static str>;

/// The intake inbox.
pub(crate) struct Intake {
    tenant: Option<TenantId>,
    company: String,
    store: Option<InboxStateStore>,
    state: InboxState,
    stage: Stage,
    group: Option<RequestGroup>,
    /// The folders as last read (`None` before the first read).
    folders: Option<Vec<IntakeFolder>>,
    folders_task: Option<Task<FoldersRead>>,
    folders_failed: bool,
    refreshed: Option<Instant>,
    saves_seen: u64,
    /// The user's engine role (`owner` is the Admin until P29).
    pub role: Option<String>,
    /// Every folder, in the engine's order.
    pub runs: Vec<Run>,
    copy: Option<Copy>,
    jobs: Vec<Job>,
    reconcile: Option<Task<Vec<(String, Upload)>>>,
    reconciled: Option<Instant>,
    bus_seen: u64,
    /// Arrived uploads still to check after the company opened.
    sweep: Vec<String>,
    claims: BTreeMap<String, Weak<()>>,
    /// Originals whose move is queued (by path hash).
    moving: BTreeSet<String>,
    /// When a failed step may run again (by path hash).
    retry_at: BTreeMap<String, Instant>,
    /// Files refused by their bytes this session (never retried).
    refused: BTreeSet<FileKey>,
    /// "Bring in" asked for these older files.
    bring_in: BTreeSet<FileKey>,
    /// Forms the screen should open now.
    pub ready_forms: Vec<FormReady>,
    /// The migration's end.
    pub migration: Migration,
    /// The migration failed: the P17 folder keeps working this session.
    fallback: bool,
    /// Originals moved to `Processed/` since the company opened.
    pub processed: u32,
    /// Originals moved to `Discarded/` since the company opened.
    pub discarded: u32,
    /// Moves that failed and wait for a retry.
    pub move_failures: u32,
    /// The last problem of a background step (i18n key).
    pub problem: Option<&'static str>,
    today: Option<CalendarDate>,
    /// The watchers' timings (tests shorten them).
    pub(crate) watch_config: WatchConfig,
    /// The demo lays out its folders the next time the window opens.
    demo_reseed: bool,
}

impl Intake {
    fn new() -> Intake {
        Intake {
            tenant: None,
            company: String::new(),
            store: None,
            state: InboxState::default(),
            stage: Stage::Idle,
            group: None,
            folders: None,
            folders_task: None,
            folders_failed: false,
            refreshed: None,
            saves_seen: 0,
            role: None,
            runs: Vec::new(),
            copy: None,
            jobs: Vec::new(),
            reconcile: None,
            reconciled: None,
            bus_seen: 0,
            sweep: Vec::new(),
            claims: BTreeMap::new(),
            moving: BTreeSet::new(),
            retry_at: BTreeMap::new(),
            refused: BTreeSet::new(),
            bring_in: BTreeSet::new(),
            ready_forms: Vec::new(),
            migration: Migration::None,
            fallback: false,
            processed: 0,
            discarded: 0,
            move_failures: 0,
            problem: None,
            today: None,
            watch_config: WatchConfig::default(),
            demo_reseed: false,
        }
    }

    // -----------------------------------------------------------------
    // What the screen reads.

    /// True once the folders were read (and the migration is over).
    pub(crate) fn loaded(&self) -> bool {
        matches!(self.stage, Stage::Running) && self.folders.is_some()
    }

    /// The folder list couldn't be read.
    pub(crate) fn failed(&self) -> bool {
        self.folders_failed && self.folders.is_none()
    }

    /// The migration failed: the P17 service runs instead.
    pub(crate) fn fallback(&self) -> bool {
        self.fallback
    }

    /// True while anything runs in the background.
    pub(crate) fn busy(&self) -> bool {
        self.copy.is_some()
            || !self.jobs.is_empty()
            || self.reconcile.is_some()
            || self.folders_task.is_some()
            || matches!(self.stage, Stage::Seeding(_) | Stage::Migrating(_))
    }

    /// Folders that need the owner (Home's Needs attention line), plus one
    /// for a migration that couldn't run.
    pub(crate) fn problems(&self) -> usize {
        let folders = self
            .runs
            .iter()
            .filter(|r| r.health().needs_attention())
            .count();
        folders
            + usize::from(matches!(
                self.migration,
                Migration::Failed | Migration::Blocked
            ))
    }

    /// Whether the user may change the folders.
    pub(crate) fn is_admin(&self) -> bool {
        self.role.as_deref() == Some(source::ADMIN_ROLE)
    }

    /// Look in every folder now.
    pub(crate) fn poke(&self) {
        for r in &self.runs {
            r.poke();
        }
    }

    /// The upload `attachment` is on an open form.
    fn on_form(&self, attachment: &str) -> bool {
        self.claims
            .get(attachment)
            .is_some_and(|w| w.strong_count() > 0)
    }

    fn keeps_originals(&self, folder: Option<IntakeFolderId>) -> bool {
        folder.is_some_and(|id| {
            self.runs
                .iter()
                .any(|r| r.folder.id == id && r.folder.keep_originals)
        })
    }

    /// Every listed file, oldest first within each folder, with where it
    /// stands. Files handled in a folder that keeps its originals are left
    /// out.
    pub(crate) fn rows(&self) -> Vec<Row> {
        let mut out = Vec::new();
        for (i, run) in self.runs.iter().enumerate() {
            let Some(listing) = run.listing() else {
                continue;
            };
            for f in &listing.files {
                let key = FileKey::of(f);
                let entry =
                    self.state
                        .find(&self.company, &key.path_hash, key.size, key.modified_ms);
                let state = match (f.state, entry.map(|e| e.status)) {
                    (_, Some(EntryStatus::Kept)) => continue,
                    (ItemState::Settling, None) => RowState::Settling,
                    (ItemState::TooLarge, _) => RowState::TooLarge,
                    (ItemState::NotSupported, _) => RowState::NotSupported,
                    (ItemState::OnlineOnly, _) => RowState::OnlineOnly,
                    (ItemState::Linked, _) => RowState::LinkedFile,
                    (_, Some(EntryStatus::Uploading)) => RowState::Copying,
                    (_, Some(EntryStatus::Arrived)) => {
                        if entry
                            .and_then(|e| e.attachment())
                            .is_some_and(|a| self.on_form(a))
                        {
                            RowState::OnForm
                        } else {
                            RowState::Arrived
                        }
                    }
                    (_, Some(EntryStatus::Duplicate)) => RowState::Duplicate,
                    (_, Some(EntryStatus::Linked | EntryStatus::Discarding)) => RowState::Moving,
                    (ItemState::Ready, None) => {
                        if self.refused.contains(&key) {
                            RowState::Refused
                        } else if self.copying(&key) {
                            RowState::Copying
                        } else if self.held_back(run, f, &key) {
                            RowState::BringIn
                        } else {
                            RowState::Waiting
                        }
                    }
                };
                let seen_at = (state == RowState::Duplicate)
                    .then(|| entry.and_then(|e| self.state.seen(&self.company, &e.file.sha256)))
                    .flatten()
                    .map(|s| s.at);
                out.push(Row {
                    run: i,
                    file: f.clone(),
                    state,
                    entry: entry.map(|e| e.file.clone()),
                    seen_at,
                });
            }
        }
        out
    }

    fn copying(&self, key: &FileKey) -> bool {
        match &self.copy {
            Some(Copy::Reading { file, .. }) => FileKey::of(file) == *key,
            Some(Copy::Resolving { identity, .. } | Copy::Uploading { identity, .. }) => {
                key.matches(identity)
            }
            None => false,
        }
    }

    /// An older file in a folder confirmed for "only files added from now
    /// on" (and not brought in).
    fn held_back(&self, run: &Run, f: &IntakeFile, key: &FileKey) -> bool {
        run.since_ms.is_some_and(|since| f.modified_ms < since) && !self.bring_in.contains(key)
    }

    /// Files waiting for the owner (the menu's count).
    pub(crate) fn badge(&self) -> usize {
        self.rows()
            .iter()
            .filter(|r| matches!(r.state, RowState::Arrived | RowState::Duplicate))
            .count()
    }

    /// Files queued or being copied in.
    pub(crate) fn queued(&self) -> usize {
        self.rows()
            .iter()
            .filter(|r| matches!(r.state, RowState::Waiting | RowState::Copying))
            .count()
    }

    // -----------------------------------------------------------------
    // Company open and the folder list.

    fn reset_company(&mut self, link: &CompanyLink, tenant: Option<TenantId>) {
        *self = Intake {
            watch_config: self.watch_config,
            demo_reseed: self.demo_reseed,
            ..Intake::new()
        };
        self.tenant = tenant;
        self.company = tenant
            .map(|t| company_key(&t.to_string()))
            .unwrap_or_default();
        self.store = link.store().path().parent().map(InboxStateStore::in_dir);
        self.state = self
            .store
            .as_ref()
            .and_then(|s| s.load().ok())
            .unwrap_or_default();
        self.group = link.group().and_then(Result::ok);
        self.bus_seen = link.bus().generation();
        self.stage = if tenant.is_some() {
            Stage::Start
        } else {
            Stage::Idle
        };
    }

    fn save_state(&self) {
        if let Some(store) = &self.store
            && let Err(e) = store.save(&self.state)
        {
            // R6: dedupe then holds for this session only.
            tracing::warn!(error = %e, "could not save the intake inbox state");
        }
    }

    /// The `--demo` inbox window opened: lay the demo's folders out again
    /// (every run of a scenario starts alike).
    pub(crate) fn demo_window_opened(&mut self, link: &CompanyLink) {
        if !link.is_demo() {
            return;
        }
        self.demo_reseed = true;
        if matches!(self.stage, Stage::Running) {
            self.stage = Stage::Start;
        }
    }

    fn start(&mut self, link: &Arc<CompanyLink>) {
        let Some(tenant) = self.tenant else {
            self.stage = Stage::Idle;
            return;
        };
        if self.demo_reseed
            && let Some(base) = super::demo::intake_base(link)
        {
            self.demo_reseed = false;
            self.stage = Stage::Seeding(super::demo::seed_intake(base));
            return;
        }
        let legacy = link.store().load().ok().and_then(|l| {
            l.get(tenant)
                .and_then(|r| r.prefs.receipt_inbox_dir.clone())
        });
        if legacy.is_none() {
            self.stage = Stage::Running;
            return;
        }
        self.migration = Migration::Running;
        let local = PrefsLocal {
            store: link.store().clone(),
            tenant,
            fresh: intake_folders::new_folder_id(),
        };
        let store = self.store.clone();
        let company = self.company.clone();
        if link.is_demo() {
            let engine = DemoEngine {
                link: Arc::clone(link),
                tenant,
            };
            let (tx, rx) = mpsc::channel();
            let spawned = std::thread::Builder::new()
                .name("cl-intake-migrate".into())
                .spawn(move || {
                    let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                        .enable_time()
                        .build()
                    else {
                        return;
                    };
                    let state = store.as_ref().map(|s| (s, company.as_str()));
                    let _ = tx.send(rt.block_on(run_migration(&engine, &local, state)));
                });
            if spawned.is_ok() {
                self.stage = Stage::Migrating(MigJob::Thread(rx));
            } else {
                self.migration_done(link, &Err(MigrateError::Local("started")));
            }
            return;
        }
        let (Some(session), Some(group)) = (link.session(), self.group.as_ref()) else {
            // No session yet: try again next frame.
            self.migration = Migration::None;
            return;
        };
        let label = OrphanLabel {
            what: "inbox.orphan.migrate",
            key: None,
        };
        match group.write(&session, label, move |s| async move {
            let state = store.as_ref().map(|st| (st, company.as_str()));
            Ok(run_migration(&s, &local, state).await)
        }) {
            Ok(p) => self.stage = Stage::Migrating(MigJob::Queue(Task::Pending(p))),
            Err(_) => self.migration_done(link, &Err(MigrateError::Local("started"))),
        }
    }

    fn poll_stage(&mut self, link: &Arc<CompanyLink>) {
        match &mut self.stage {
            Stage::Seeding(rx) => match rx.try_recv() {
                Ok(seeded) => {
                    if let Some(tenant) = self.tenant {
                        super::demo::apply_seed(link, tenant, &seeded);
                    }
                    // Every demo run starts alike: this company's entries
                    // and its copied-in history go.
                    self.state.companies.remove(&self.company);
                    self.state.seen.remove(&self.company);
                    self.save_state();
                    self.runs.clear();
                    self.folders = None;
                    self.stage = Stage::Start;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => self.stage = Stage::Start,
            },
            Stage::Migrating(job) => {
                let result = match job {
                    MigJob::Queue(t) => match t.try_take() {
                        None => return,
                        Some(Ok(r)) => r,
                        Some(Err(e)) => Err(MigrateError::Engine(e)),
                    },
                    MigJob::Thread(rx) => match rx.try_recv() {
                        Ok(r) => r,
                        Err(TryRecvError::Empty) => return,
                        Err(TryRecvError::Disconnected) => Err(MigrateError::Local("run")),
                    },
                };
                self.migration_done(link, &result);
            }
            Stage::Idle | Stage::Start | Stage::Running => {}
        }
    }

    fn migration_done(&mut self, link: &CompanyLink, r: &Result<MigrationOutcome, MigrateError>) {
        self.migration = match r {
            Ok(MigrationOutcome::Nothing) => Migration::None,
            Ok(MigrationOutcome::Blocked) => Migration::Blocked,
            Ok(MigrationOutcome::Migrated { .. }) => Migration::Migrated,
            Err(_) => Migration::Failed,
        };
        if let Err(e) = r {
            tracing::warn!(error = %e, "the receipt inbox folder could not move to intake folders");
        }
        if matches!(self.migration, Migration::Failed | Migration::Blocked) {
            // The P17 folder keeps working while it is still set.
            let still_set = self.tenant.is_some_and(|t| {
                link.store().load().is_ok_and(|l| {
                    l.get(t)
                        .is_some_and(|r| r.prefs.receipt_inbox_dir.is_some())
                })
            });
            self.fallback = still_set;
        }
        // The migration moved P17 entries in the state file.
        self.state = self
            .store
            .as_ref()
            .and_then(|s| s.load().ok())
            .unwrap_or_default();
        self.stage = Stage::Running;
        self.refreshed = None;
    }

    fn refresh_folders(&mut self, link: &CompanyLink, now: Instant) {
        if self.folders_task.is_some() {
            return;
        }
        let saves = intake_folders::saves();
        let due = self
            .refreshed
            .is_none_or(|t| now.duration_since(t) >= ENGINE_REFRESH)
            || saves != self.saves_seen;
        if !due {
            return;
        }
        let Some(tenant) = self.tenant else {
            return;
        };
        self.refreshed = Some(now);
        self.saves_seen = saves;
        if link.is_demo() {
            match source::demo_get(link, tenant) {
                Some(f) => self.folders_task = Some(Task::ready(Ok((f.stored, f.role)))),
                None => self.folders_failed = true,
            }
            return;
        }
        let (Some(session), Some(group)) = (link.session(), self.group.as_ref()) else {
            self.refreshed = None;
            return;
        };
        if let Ok(p) = group.read(&session, move |s| async move {
            let stored = source::engine_get(&s).await?;
            let role = source::engine_role(&s, tenant).await;
            Ok(Ok((stored, role)))
        }) {
            self.folders_task = Some(Task::Pending(p));
        }
    }

    fn poll_folders(&mut self, link: &CompanyLink) {
        let Some(r) = self.folders_task.as_mut().and_then(Task::poll_result) else {
            return;
        };
        self.folders_task = None;
        match r {
            Ok((stored, role)) => {
                self.folders_failed = false;
                self.role = role;
                self.apply_folders(link, stored.folders);
            }
            Err(e) => {
                tracing::debug!(error = %e, "the intake folders could not be read; retrying later");
                self.folders_failed = true;
            }
        }
    }

    /// The app's own folders, never watched.
    fn app_dirs(link: &CompanyLink) -> Vec<PathBuf> {
        let mut dirs = Vec::new();
        if let Some(settings) = link.store().path().parent() {
            dirs.push(settings.to_path_buf());
        }
        dirs.push(link.backup_dir.clone());
        dirs
    }

    /// Takes the engine's list: a watcher per confirmed folder, started,
    /// kept or stopped (never joined).
    fn apply_folders(&mut self, link: &CompanyLink, folders: Vec<IntakeFolder>) {
        let Some(tenant) = self.tenant else {
            return;
        };
        let recent = link.store().load().unwrap_or_default();
        let here: BTreeMap<IntakeFolderId, HereNote> = recent
            .get(tenant)
            .map(|r| r.prefs.intake_here.clone())
            .unwrap_or_default();
        // Pinned roots other companies on this computer confirmed, with
        // when (the later-confirmed company pauses, Q-A01i).
        let mut others: BTreeMap<String, Option<i64>> = BTreeMap::new();
        for r in recent.live().filter(|r| r.tenant_id != tenant) {
            for n in r.prefs.intake_here.values() {
                if n.canonical_key.is_empty() {
                    continue;
                }
                let at = n.confirmed_at.map(cl_core::Timestamp::unix_millis);
                others
                    .entry(n.canonical_key.clone())
                    .and_modify(|e| *e = (*e).min(at))
                    .or_insert(at);
            }
        }
        let app_dirs = Self::app_dirs(link);
        let dirs: Vec<&Path> = app_dirs.iter().map(PathBuf::as_path).collect();
        let mut old: Vec<Run> = std::mem::take(&mut self.runs);
        for f in &folders {
            let mut run = match old.iter().position(|r| r.folder.id == f.id) {
                Some(i) => old.swap_remove(i),
                None => Run {
                    folder: f.clone(),
                    watch: None,
                    watched: None,
                    snapshot: None,
                    blocked: None,
                    since_ms: None,
                },
            };
            run.folder = f.clone();
            let note = here
                .get(&f.id)
                .filter(|n| n.path_hash == cl_settings::path_hash(&f.path));
            run.since_ms = note.and_then(|n| n.since_ms);
            let mapped = note.is_some_and(|n| {
                others.get(&n.canonical_key).is_some_and(|other_at| {
                    let ours = n.confirmed_at.map(cl_core::Timestamp::unix_millis);
                    match (*other_at, ours) {
                        (Some(o), Some(m)) => o <= m,
                        _ => true,
                    }
                })
            });
            let want = match note {
                None => Err(Health::NotConfirmed),
                Some(_) if mapped => Err(Health::MappedByAnother),
                Some(n) => check_path_syntax(&f.path, &dirs)
                    .map(|root| (root, n.canonical_key.clone()))
                    .map_err(|_| Health::NotConfirmed),
            };
            match want {
                Ok(w) => {
                    run.blocked = None;
                    if run.watched.as_ref() != Some(&w) || run.watch.is_none() {
                        run.snapshot = None;
                        run.watch = Some(WatchHandle::spawn_with(
                            f.id,
                            w.0.clone(),
                            w.1.clone(),
                            Arc::new(RealFs),
                            self.watch_config,
                        ));
                        run.watched = Some(w);
                    }
                }
                Err(h) => {
                    run.blocked = Some(h);
                    run.watch = None;
                    run.watched = None;
                    run.snapshot = None;
                }
            }
            self.runs.push(run);
        }
        // Folders no longer listed: their watchers stop as `old` drops.
        drop(old);
        if self.folders.is_none() {
            // First read since the company opened: check every upload that
            // waits for the owner, paced (D4 "full sweep").
            self.sweep = self
                .state
                .entries(&self.company)
                .iter()
                .filter(|e| e.status == EntryStatus::Arrived)
                .filter_map(|e| e.attachment().map(str::to_owned))
                .collect();
        }
        self.folders = Some(folders);
    }

    // -----------------------------------------------------------------
    // Looks, moves and pruning.

    fn take_snapshots(&mut self) {
        let mut changed = false;
        for i in 0..self.runs.len() {
            let latest = {
                let run = &self.runs[i];
                run.watch
                    .as_ref()
                    .and_then(|w| w.latest_after(run.snapshot.as_ref().map(|s| s.seq)))
            };
            if let Some(s) = latest {
                self.runs[i].snapshot = Some(s);
                changed |= self.prune(i);
            }
        }
        if changed {
            self.save_state();
        }
    }

    /// Drops entries of folder `i` whose file is no longer there (only
    /// when the listing is complete). Entries still owing a move or a
    /// delete are left to [`Intake::drive`].
    fn prune(&mut self, i: usize) -> bool {
        let run = &self.runs[i];
        let Some(listing) = run.listing().filter(|l| l.complete()) else {
            return false;
        };
        let keys: Vec<FileKey> = listing.files.iter().map(FileKey::of).collect();
        let id = run.folder.id;
        let in_flight = self.copy_identity();
        let Some(list) = self.state.companies.get_mut(&self.company) else {
            return false;
        };
        let before = list.len();
        list.retain(|e| {
            e.folder != Some(id)
                || e.status.pending() && e.status != EntryStatus::Uploading
                || in_flight.as_ref() == Some(&e.file)
                || keys.iter().any(|k| k.matches(&e.file))
        });
        let changed = list.len() != before;
        if list.is_empty() {
            self.state.companies.remove(&self.company);
        }
        changed
    }

    fn copy_identity(&self) -> Option<FileIdentity> {
        match &self.copy {
            Some(Copy::Resolving { identity, .. } | Copy::Uploading { identity, .. }) => {
                Some(identity.clone())
            }
            Some(Copy::Reading { resume, .. }) => resume.clone(),
            None => None,
        }
    }

    fn run_of(&self, folder: Option<IntakeFolderId>) -> Option<&Run> {
        folder.and_then(|id| self.runs.iter().find(|r| r.folder.id == id))
    }

    fn due(&self, path_hash: &str, now: Instant) -> bool {
        self.retry_at.get(path_hash).is_none_or(|t| now >= *t)
    }

    /// Pushes the moves and deletes that handled entries still owe.
    fn drive(&mut self, link: &CompanyLink, now: Instant) {
        let today = self.today.unwrap_or_else(crate::shell::today);
        let processed = HandledDir::Processed {
            year: today.year(),
            month: today.month(),
        };
        let mut moves: Vec<(usize, MoveJob)> = Vec::new();
        let mut deletes: Vec<(FileIdentity, AttachmentId, bool)> = Vec::new();
        let mut gone: Vec<FileIdentity> = Vec::new();
        for e in self.state.entries(&self.company) {
            let to = match e.status {
                EntryStatus::Linked => processed,
                EntryStatus::Discarding => HandledDir::Discarded,
                _ => continue,
            };
            let ph = &e.file.path_hash;
            if self.moving.contains(ph) || !self.due(ph, now) || self.deleting(&e.file) {
                continue;
            }
            let Some(ri) = e
                .folder
                .and_then(|id| self.runs.iter().position(|r| r.folder.id == id))
            else {
                continue;
            };
            let run = &self.runs[ri];
            let Some(listing) = run.listing() else {
                continue;
            };
            match listing
                .files
                .iter()
                .find(|f| e.file.matches(&path_hash(&f.path), f.size, f.modified_ms))
            {
                Some(f) => moves.push((
                    ri,
                    MoveJob {
                        path: f.path.clone(),
                        sha256: e.file.sha256.clone(),
                        to,
                        file: e.file.clone(),
                    },
                )),
                None if listing.complete() => {
                    // Moved already (or by the owner).
                    match (
                        e.status,
                        e.attachment().and_then(|a| AttachmentId::parse(a).ok()),
                    ) {
                        (EntryStatus::Discarding, Some(a)) => {
                            deletes.push((e.file.clone(), a, false));
                        }
                        _ => gone.push(e.file.clone()),
                    }
                }
                None => {}
            }
        }
        for (ri, job) in moves {
            if let Some(w) = &self.runs[ri].watch {
                self.moving.insert(job.file.path_hash.clone());
                w.push_move(job);
            }
        }
        for (identity, a, keep) in deletes {
            self.start_delete(link, identity, a, keep);
        }
        if !gone.is_empty() {
            for g in gone {
                self.state.remove(&self.company, &g);
            }
            self.save_state();
        }
    }

    fn deleting(&self, file: &FileIdentity) -> bool {
        self.jobs
            .iter()
            .any(|j| matches!(&j.kind, JobKind::Delete(f, _) if f == file))
    }

    fn apply_moves(&mut self, link: &CompanyLink, now: Instant) {
        let mut done = Vec::new();
        for r in &self.runs {
            if let Some(w) = &r.watch {
                done.extend(w.take_moved());
            }
        }
        if done.is_empty() {
            return;
        }
        for d in done {
            self.moving.remove(&d.file.path_hash);
            let Some(entry) = self.state.entry(&self.company, &d.file).cloned() else {
                continue;
            };
            match d.result {
                Ok(_) | Err(FileError::Gone | FileError::Changed | FileError::NotAFile) => {
                    let moved = d.result.is_ok();
                    if entry.status == EntryStatus::Discarding {
                        if moved {
                            self.discarded += 1;
                        }
                        match entry.attachment().and_then(|a| AttachmentId::parse(a).ok()) {
                            Some(a) => self.start_delete(link, d.file.clone(), a, false),
                            None => self.state.remove(&self.company, &d.file),
                        }
                    } else {
                        if moved {
                            self.processed += 1;
                        }
                        self.state.remove(&self.company, &d.file);
                    }
                    self.retry_at.remove(&d.file.path_hash);
                }
                Err(_) => {
                    self.move_failures += 1;
                    self.retry_at.insert(d.file.path_hash.clone(), now + RETRY);
                    tracing::warn!("an intake file could not be moved yet; it stays in place");
                }
            }
        }
        self.save_state();
        self.poke();
    }

    // -----------------------------------------------------------------
    // Arrival.

    /// Starts the next copy when none runs: an `uploading` entry left from
    /// before first, then the oldest ready file without an entry.
    fn next_copy(&mut self, link: &CompanyLink, now: Instant) {
        if self.copy.is_some() {
            return;
        }
        // Uploading entries whose file is listed.
        let resume = self
            .state
            .entries(&self.company)
            .iter()
            .filter(|e| e.status == EntryStatus::Uploading && self.due(&e.file.path_hash, now))
            .find_map(|e| {
                let run = self.run_of(e.folder).filter(|r| r.watched())?;
                let f = run.listing()?.files.iter().find(|f| {
                    f.state == ItemState::Ready
                        && e.file.matches(&path_hash(&f.path), f.size, f.modified_ms)
                })?;
                Some((e.file.clone(), e.draft.clone(), run.folder.id, f.clone()))
            });
        if let Some((identity, draft, folder, file)) = resume {
            let draft = draft.and_then(|d| DraftKey::parse(&d).ok());
            match draft {
                Some(d) => self.start_resolve(link, identity, d),
                None => self.start_read(folder, file, Some(identity)),
            }
            return;
        }
        let mut best: Option<(i64, IntakeFolderId, IntakeFile)> = None;
        for run in self.runs.iter().filter(|r| r.watched()) {
            let Some(listing) = run.listing() else {
                continue;
            };
            for f in listing.files.iter().filter(|f| f.state == ItemState::Ready) {
                let key = FileKey::of(f);
                if self
                    .state
                    .find(&self.company, &key.path_hash, key.size, key.modified_ms)
                    .is_some()
                    || self.refused.contains(&key)
                    || !self.due(&key.path_hash, now)
                    || self.held_back(run, f, &key)
                {
                    continue;
                }
                if best.as_ref().is_none_or(|b| f.modified_ms < b.0) {
                    best = Some((f.modified_ms, run.folder.id, f.clone()));
                }
            }
        }
        if let Some((_, folder, file)) = best {
            self.start_read(folder, file, None);
        }
    }

    fn start_read(
        &mut self,
        folder: IntakeFolderId,
        file: IntakeFile,
        resume: Option<FileIdentity>,
    ) {
        let Some(rx) = self
            .runs
            .iter()
            .find(|r| r.folder.id == folder)
            .and_then(|r| r.read(file.path.clone()))
        else {
            return;
        };
        self.copy = Some(Copy::Reading {
            folder,
            file,
            rx,
            resume,
        });
    }

    fn start_resolve(&mut self, link: &CompanyLink, identity: FileIdentity, draft: DraftKey) {
        if let Some(demo) = link.demo_state() {
            let found = crate::company::demo::checks::attachments(&demo, AttachTo::Draft(draft))
                .first()
                .map(|a| a.id);
            drop(demo);
            self.copy = Some(Copy::Resolving {
                identity,
                task: Task::ready(found),
            });
            return;
        }
        let (Some(session), Some(group)) = (link.session(), self.group.as_ref()) else {
            return;
        };
        if let Ok(p) = group.read(&session, move |s| async move {
            let page = s
                .attachments()
                .list(AttachTo::Draft(draft), PageRequest::first_max())
                .await?;
            Ok(page.data.first().map(|a| a.id))
        }) {
            self.copy = Some(Copy::Resolving {
                identity,
                task: Task::Pending(p),
            });
        }
    }

    fn poll_copy(&mut self, link: &CompanyLink, now: Instant) {
        let Some(copy) = self.copy.take() else {
            return;
        };
        match copy {
            Copy::Resolving { identity, mut task } => match task.try_take() {
                None => self.copy = Some(Copy::Resolving { identity, task }),
                Some(Ok(Some(a))) => {
                    self.state
                        .arrived(&self.company, &identity, &a.to_string(), now_ms());
                    self.save_state();
                }
                Some(Ok(None)) => {
                    // Not there: read the file again and send it into the
                    // same draft.
                    let target = self
                        .state
                        .entry(&self.company, &identity)
                        .and_then(|e| e.folder)
                        .and_then(|id| {
                            let run = self.runs.iter().find(|r| r.folder.id == id)?;
                            let f = run.listing()?.files.iter().find(|f| {
                                identity.matches(&path_hash(&f.path), f.size, f.modified_ms)
                            })?;
                            Some((id, f.clone()))
                        });
                    if let Some((folder, file)) = target {
                        self.start_read(folder, file, Some(identity));
                    }
                }
                Some(Err(e)) => {
                    tracing::debug!(error = %e, "an intake upload could not be checked; retrying later");
                    self.retry_at
                        .insert(identity.path_hash.clone(), now + RETRY);
                }
            },
            Copy::Reading {
                folder,
                file,
                rx,
                resume,
            } => match rx.try_recv() {
                Err(TryRecvError::Empty) => {
                    self.copy = Some(Copy::Reading {
                        folder,
                        file,
                        rx,
                        resume,
                    });
                }
                Err(TryRecvError::Disconnected) => {}
                Ok(r) => self.read_done(link, folder, &file, resume, r, now),
            },
            Copy::Uploading {
                identity,
                size,
                mut task,
            } => match task.try_take() {
                None => {
                    self.copy = Some(Copy::Uploading {
                        identity,
                        size,
                        task,
                    });
                }
                Some(r) => self.upload_done(&identity, size, r, now),
            },
        }
    }

    fn read_done(
        &mut self,
        link: &CompanyLink,
        folder: IntakeFolderId,
        file: &IntakeFile,
        resume: Option<FileIdentity>,
        r: Result<ReadFile, FileError>,
        now: Instant,
    ) {
        let key = FileKey::of(file);
        let read = match r {
            Ok(read) if read.size == file.size && read.modified_ms == file.modified_ms => read,
            // Changed since it was listed: the next look settles it.
            Ok(_)
            | Err(
                FileError::Unstable | FileError::Changed | FileError::Gone | FileError::NotAFile,
            ) => {
                return;
            }
            Err(FileError::WrongType | FileError::Empty) => {
                self.refused.insert(key);
                return;
            }
            Err(_) => {
                self.retry_at.insert(key.path_hash, now + RETRY);
                return;
            }
        };
        let identity = FileIdentity {
            path_hash: key.path_hash.clone(),
            size: read.size,
            modified_ms: read.modified_ms,
            sha256: read.sha256.clone(),
        };
        let draft = match resume {
            Some(old) if old == identity => self
                .state
                .entry(&self.company, &old)
                .and_then(|e| e.draft.as_deref())
                .and_then(|d| DraftKey::parse(d).ok())
                .unwrap_or_default(),
            Some(old) => {
                // Not the bytes that were going up: start over.
                self.state.remove(&self.company, &old);
                self.save_state();
                return;
            }
            None => {
                if self.state.seen(&self.company, &identity.sha256).is_some() {
                    self.state
                        .mark_duplicate(&self.company, Some(folder), &identity);
                    self.save_state();
                    return;
                }
                DraftKey::new()
            }
        };
        let key = IdempotencyKey::new();
        // Saved before the upload is sent (D4).
        if !self.state.begin_upload(
            &self.company,
            Some(folder),
            &identity,
            &draft.to_string(),
            &key.as_header(),
        ) {
            return;
        }
        self.save_state();
        let name = file.name.clone();
        let bytes = read.bytes;
        let size = read.size;
        if let Some(mut demo) = link.demo_state() {
            let r = crate::company::demo::checks::upload(
                &mut demo,
                AttachTo::Draft(draft),
                &name,
                &bytes,
            );
            drop(demo);
            match r {
                Ok(a) => {
                    self.copy = Some(Copy::Uploading {
                        identity,
                        size,
                        task: Task::ready(a),
                    });
                }
                Err(e) => self.upload_done(&identity, size, Err(e), now),
            }
            return;
        }
        let (Some(session), Some(group)) = (link.session(), self.group.as_ref()) else {
            return;
        };
        let label = OrphanLabel {
            what: "inbox.orphan.upload",
            key: Some(key.clone()),
        };
        match group.write(&session, label, move |s| async move {
            s.attachments()
                .upload(AttachTo::Draft(draft), &name, &bytes, key)
                .await
        }) {
            Ok(p) => {
                self.copy = Some(Copy::Uploading {
                    identity,
                    size,
                    task: Task::Pending(p),
                });
            }
            Err(_) => {
                self.retry_at
                    .insert(identity.path_hash.clone(), now + RETRY);
            }
        }
    }

    fn upload_done(
        &mut self,
        identity: &FileIdentity,
        size: u64,
        r: Result<Attachment, ApiError>,
        now: Instant,
    ) {
        match r {
            Ok(a) if a.file_size == Some(size) && a.attachable_type == "draft" => {
                self.state
                    .arrived(&self.company, identity, &a.id.to_string(), now_ms());
                self.save_state();
                self.problem = None;
            }
            Ok(_) => {
                // The engine didn't store the whole file: send it again
                // later (the draft is resolved first).
                self.problem = Some("inbox.upload.mismatch");
                self.retry_at
                    .insert(identity.path_hash.clone(), now + RETRY);
            }
            Err(ApiError::OutcomeUnknown { .. }) => {
                // Resolved by listing the draft.
                self.retry_at
                    .insert(identity.path_hash.clone(), now + RETRY);
            }
            Err(ApiError::Local(
                LocalValidation::AttachmentType | LocalValidation::AttachmentTooLarge,
            )) => {
                self.state.remove(&self.company, identity);
                self.save_state();
                self.refused.insert(FileKey {
                    path_hash: identity.path_hash.clone(),
                    size: identity.size,
                    modified_ms: identity.modified_ms,
                });
            }
            Err(e) => {
                tracing::debug!(error = %e, "an intake file could not be copied in; retrying later");
                self.problem = Some("inbox.upload.failed");
                self.retry_at
                    .insert(identity.path_hash.clone(), now + RETRY);
            }
        }
    }

    // -----------------------------------------------------------------
    // Handling.

    /// The entry's arrival draft for a new form, claimed for as long as
    /// the form holds the returned claim. `Err` when the file isn't
    /// waiting, or is on an open form already.
    pub(crate) fn claim_for_form(
        &mut self,
        link: &CompanyLink,
        entry: &FileIdentity,
        kind: FormKind,
    ) -> Result<Option<(DraftKey, AttachmentId, Claim)>, &'static str> {
        let e = self
            .state
            .entry(&self.company, entry)
            .filter(|e| e.status == EntryStatus::Arrived)
            .ok_or("inbox.action.not_waiting")?;
        let attachment = e
            .attachment()
            .and_then(|a| AttachmentId::parse(a).ok())
            .ok_or("inbox.action.not_waiting")?;
        if self.on_form(&attachment.to_string()) {
            return Err("inbox.on_form.hint");
        }
        if let Some(draft) = e.draft.as_deref().and_then(|d| DraftKey::parse(d).ok()) {
            return Ok(Some((draft, attachment, self.claim(attachment))));
        }
        // A P17 upload: link it into a fresh draft first (D5).
        if link.is_demo() {
            return Err("inbox.upload.failed");
        }
        let draft = DraftKey::new();
        self.start_link(
            link,
            JobKind::LinkDraft(entry.clone(), draft, kind),
            attachment,
            AttachTo::Draft(draft),
        )?;
        Ok(None)
    }

    /// Claims `attachment` for a form (see [`Claim`]).
    pub(crate) fn claim(&mut self, attachment: AttachmentId) -> Claim {
        let claim = Arc::new(());
        self.claims
            .insert(attachment.to_string(), Arc::downgrade(&claim));
        claim
    }

    /// **Add to…**: links the arrival onto `target`.
    pub(crate) fn add_to(
        &mut self,
        link: &CompanyLink,
        entry: &FileIdentity,
        target: AttachTo,
    ) -> ActionResult {
        let e = self
            .state
            .entry(&self.company, entry)
            .filter(|e| e.status == EntryStatus::Arrived)
            .ok_or("inbox.action.not_waiting")?;
        let attachment = e
            .attachment()
            .and_then(|a| AttachmentId::parse(a).ok())
            .ok_or("inbox.action.not_waiting")?;
        if self.on_form(&attachment.to_string()) {
            return Err("inbox.on_form.hint");
        }
        if link.is_demo() {
            // Each arrival owns its draft: linking the draft moves exactly
            // this file.
            let draft = e
                .draft
                .as_deref()
                .and_then(|d| DraftKey::parse(d).ok())
                .ok_or("inbox.upload.failed")?;
            let mut demo = link.demo_state().ok_or("inbox.upload.failed")?;
            let n = crate::company::demo::checks::link_draft(&mut demo, draft, target);
            drop(demo);
            if n == 0 {
                return Err("inbox.upload.failed");
            }
            self.handled(entry);
            return Ok(());
        }
        self.start_link(link, JobKind::LinkRecord(entry.clone()), attachment, target)
    }

    fn start_link(
        &mut self,
        link: &CompanyLink,
        kind: JobKind,
        attachment: AttachmentId,
        to: AttachTo,
    ) -> ActionResult {
        let (Some(session), Some(group)) = (link.session(), self.group.as_ref()) else {
            return Err("inbox.upload.failed");
        };
        let key = IdempotencyKey::new();
        let label = OrphanLabel {
            what: "inbox.orphan.link",
            key: Some(key.clone()),
        };
        let p = group
            .write(&session, label, move |s| async move {
                s.attachments().link(attachment, to, key).await
            })
            .map_err(|_| "inbox.upload.failed")?;
        self.jobs.push(Job {
            kind,
            task: Task::Pending(p),
        });
        Ok(())
    }

    /// A saved record holds the arrival: `linked` (its original moves to
    /// `Processed/`), or `kept` in a folder that leaves originals.
    fn handled(&mut self, entry: &FileIdentity) {
        let folder = self
            .state
            .entry(&self.company, entry)
            .and_then(|e| e.folder);
        let next = if self.keeps_originals(folder) {
            EntryStatus::Kept
        } else {
            EntryStatus::Linked
        };
        self.state.set_status(&self.company, entry, next);
        self.save_state();
    }

    /// **Discard**: the original moves to `Discarded/` (never deleted),
    /// then the engine's copy is deleted. In a folder that keeps its
    /// originals, only the engine's copy goes and the file is hidden.
    pub(crate) fn discard(&mut self, link: &CompanyLink, entry: &FileIdentity) -> ActionResult {
        let e = self
            .state
            .entry(&self.company, entry)
            .filter(|e| matches!(e.status, EntryStatus::Arrived | EntryStatus::Duplicate))
            .cloned()
            .ok_or("inbox.action.not_waiting")?;
        if e.attachment().is_some_and(|a| self.on_form(a)) {
            return Err("inbox.on_form.hint");
        }
        let attachment = e.attachment().and_then(|a| AttachmentId::parse(a).ok());
        if self.keeps_originals(e.folder) {
            match attachment {
                Some(a) if e.status == EntryStatus::Arrived => {
                    self.start_delete(link, entry.clone(), a, true);
                }
                _ => {
                    // A duplicate has no copy in the engine: it is only
                    // hidden (the original stays where it is).
                    if let Some(x) = self
                        .state
                        .companies
                        .get_mut(&self.company)
                        .and_then(|l| l.iter_mut().find(|x| x.file == *entry))
                    {
                        x.status = EntryStatus::Kept;
                        x.linked = false;
                    }
                    self.save_state();
                }
            }
            return Ok(());
        }
        self.state
            .set_status(&self.company, entry, EntryStatus::Discarding);
        self.save_state();
        Ok(())
    }

    fn start_delete(
        &mut self,
        link: &CompanyLink,
        entry: FileIdentity,
        a: AttachmentId,
        keep: bool,
    ) {
        if self.deleting(&entry) {
            return;
        }
        if let Some(mut demo) = link.demo_state() {
            crate::company::demo::checks::delete_attachment(&mut demo, a);
            drop(demo);
            self.jobs.push(Job {
                kind: JobKind::Delete(entry, keep),
                task: Task::ready(()),
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
            match s.attachments().delete(a, key).await {
                Ok(()) | Err(ApiError::NotFound { .. }) => Ok(()),
                Err(e) => Err(e),
            }
        }) {
            self.jobs.push(Job {
                kind: JobKind::Delete(entry, keep),
                task: Task::Pending(p),
            });
        }
    }

    /// **Copy anyway** for a duplicate: it goes up like a new file.
    pub(crate) fn copy_anyway(&mut self, entry: &FileIdentity) -> ActionResult {
        let e = self
            .state
            .entry(&self.company, entry)
            .filter(|e| e.status == EntryStatus::Duplicate)
            .ok_or("inbox.action.not_waiting")?;
        let folder = e.folder;
        let draft = DraftKey::new();
        if !self.state.begin_upload(
            &self.company,
            folder,
            entry,
            &draft.to_string(),
            &IdempotencyKey::new().as_header(),
        ) {
            return Err("inbox.action.not_waiting");
        }
        self.save_state();
        Ok(())
    }

    /// **Bring in** an older file in an "only new files" folder.
    pub(crate) fn bring_in(&mut self, file: &IntakeFile) {
        self.bring_in.insert(FileKey::of(file));
    }

    fn poll_jobs(&mut self, now: Instant) {
        let mut i = 0;
        while i < self.jobs.len() {
            let Some(r) = self.jobs[i].task.try_take() else {
                i += 1;
                continue;
            };
            let job = self.jobs.swap_remove(i);
            match (job.kind, r) {
                (JobKind::LinkRecord(entry), Ok(())) => self.handled(&entry),
                (JobKind::LinkDraft(entry, draft, kind), Ok(())) => {
                    if let Some(e) = self
                        .state
                        .companies
                        .get_mut(&self.company)
                        .and_then(|l| l.iter_mut().find(|e| e.file == entry))
                    {
                        e.draft = Some(draft.to_string());
                    }
                    self.save_state();
                    let attachment = self
                        .state
                        .entry(&self.company, &entry)
                        .and_then(|e| e.attachment())
                        .and_then(|a| AttachmentId::parse(a).ok());
                    if let Some(attachment) = attachment {
                        self.ready_forms.push(FormReady {
                            kind,
                            entry,
                            draft,
                            attachment,
                        });
                    }
                }
                (JobKind::Delete(entry, keep), Ok(())) => {
                    if keep {
                        self.state
                            .set_status(&self.company, &entry, EntryStatus::Kept);
                    } else {
                        self.state.remove(&self.company, &entry);
                    }
                    self.save_state();
                }
                (JobKind::LinkRecord(_) | JobKind::LinkDraft(..), Err(e)) => {
                    tracing::debug!(error = %e, "an intake file could not be attached");
                    self.problem = Some(match e {
                        ApiError::OutcomeUnknown { .. } => "inbox.upload.unknown",
                        _ => "inbox.link.failed",
                    });
                }
                (JobKind::Delete(entry, _), Err(e)) => {
                    tracing::debug!(error = %e, "an intake file's copy could not be deleted; retrying later");
                    self.retry_at.insert(entry.path_hash.clone(), now + RETRY);
                }
            }
        }
    }

    // -----------------------------------------------------------------
    // Uploads on forms, and the sweep after opening.

    fn start_reconcile(&mut self, link: &CompanyLink, now: Instant) {
        if self.reconcile.is_some() {
            return;
        }
        let generation = link.bus().generation();
        let bus_moved = generation != self.bus_seen;
        let timer = self
            .reconciled
            .is_none_or(|t| now.duration_since(t) >= RECONCILE_EVERY);
        let mut ids: Vec<String> = Vec::new();
        if !self.claims.is_empty() && (bus_moved || timer) {
            ids.extend(self.claims.keys().cloned());
        }
        if ids.is_empty() && !self.sweep.is_empty() {
            let n = self.sweep.len().min(SWEEP_BATCH);
            ids.extend(self.sweep.drain(..n));
        }
        if ids.is_empty() {
            return;
        }
        self.bus_seen = generation;
        self.reconciled = Some(now);
        if let Some(demo) = link.demo_state() {
            let checked = ids
                .into_iter()
                .map(|id| {
                    let u = AttachmentId::parse(&id)
                        .ok()
                        .and_then(|a| crate::company::demo::checks::find_attachment(&demo, a))
                        .map_or(Upload::Gone, |a| upload_of(&a.attachable_type));
                    (id, u)
                })
                .collect();
            drop(demo);
            self.reconcile = Some(Task::ready(checked));
            return;
        }
        let (Some(session), Some(group)) = (link.session(), self.group.as_ref()) else {
            return;
        };
        if let Ok(p) = group.read(&session, move |s| async move {
            let mut out = Vec::with_capacity(ids.len());
            for id in ids {
                let u = match AttachmentId::parse(&id) {
                    Err(_) => Upload::Gone,
                    Ok(a) => match s.attachments().get(a).await {
                        Ok(a) => upload_of(&a.attachable_type),
                        Err(ApiError::NotFound { .. }) => Upload::Gone,
                        Err(e) => return Err(e),
                    },
                };
                out.push((id, u));
            }
            Ok(out)
        }) {
            self.reconcile = Some(Task::Pending(p));
        }
    }

    fn poll_reconcile(&mut self) {
        let Some(r) = self.reconcile.as_mut().and_then(Task::try_take) else {
            return;
        };
        self.reconcile = None;
        let checked = match r {
            Ok(c) => c,
            Err(e) => {
                tracing::debug!(error = %e, "intake uploads could not be checked; retrying later");
                return;
            }
        };
        let mut changed = false;
        for (id, u) in checked {
            let entry = self
                .state
                .entries(&self.company)
                .iter()
                .find(|e| e.attachments.contains(&id))
                .map(|e| (e.file.clone(), e.status));
            match u {
                Upload::Linked => {
                    self.claims.remove(&id);
                    if let Some((file, EntryStatus::Arrived)) = entry {
                        self.handled(&file);
                        changed = true;
                    }
                }
                Upload::Gone => {
                    self.claims.remove(&id);
                    if matches!(entry, Some((_, EntryStatus::Arrived))) {
                        self.state.drop_attachment(&self.company, &id);
                        changed = true;
                    }
                }
                Upload::Draft => {
                    // Still on its draft: a closed form frees it.
                    if self.claims.get(&id).is_some_and(|w| w.strong_count() == 0) {
                        self.claims.remove(&id);
                    }
                }
            }
        }
        if changed {
            self.save_state();
        }
    }
}

// ---------------------------------------------------------------------------
// The migration's two sides.

/// The company record on this computer (`LocalIntake` for the migration).
struct PrefsLocal {
    store: RecentStore,
    tenant: TenantId,
    /// The id a migrated folder gets (minted once).
    fresh: IntakeFolderId,
}

impl LocalIntake for PrefsLocal {
    fn legacy_dir(&self) -> Result<Option<PathBuf>, MigrateError> {
        let l = self.store.load().map_err(|_| MigrateError::Local("read"))?;
        Ok(l.get(self.tenant)
            .and_then(|r| r.prefs.receipt_inbox_dir.clone()))
    }

    fn plan(&self, legacy: &Path, engine: &[IntakeFolder]) -> PlannedMigration {
        let here = self
            .store
            .load()
            .ok()
            .and_then(|l| l.get(self.tenant).map(|r| r.prefs.intake_here.clone()))
            .unwrap_or_default();
        let p = cl_settings::migrate_inbox::plan(Some(legacy), engine, &here, self.fresh);
        PlannedMigration {
            add_folder: p.add_folder,
            confirm: p.confirm,
            clear_legacy: p.clear_legacy,
            blocked: p.blocked.is_some(),
        }
    }

    fn confirm(&self, folder: &IntakeFolder, canonical_key: &str) -> Result<(), MigrateError> {
        self.store
            .update(|l| {
                if let Some(r) = l.get_mut(self.tenant) {
                    r.prefs.intake_here.insert(
                        folder.id,
                        HereNote {
                            path_hash: cl_settings::path_hash(&folder.path),
                            canonical_key: canonical_key.to_owned(),
                            since_ms: None,
                            confirmed_at: Some(cl_settings::recent::now()),
                        },
                    );
                }
            })
            .map_err(|_| MigrateError::Local("written"))
    }

    fn clear_legacy(&self) -> Result<(), MigrateError> {
        self.store
            .update(|l| {
                if let Some(r) = l.get_mut(self.tenant) {
                    r.prefs.receipt_inbox_dir = None;
                }
            })
            .map_err(|_| MigrateError::Local("written"))
    }
}

/// The demo engine's intake settings (`IntakeEngine` for the migration).
struct DemoEngine {
    link: Arc<CompanyLink>,
    tenant: TenantId,
}

fn demo_conflict(reason: ConflictReason) -> ApiError {
    ApiError::Conflict {
        reason,
        current_version: None,
        code: None,
        message: UserText::default(),
    }
}

impl DemoEngine {
    fn get_now(&self) -> Result<IntakeSettings, ApiError> {
        let demo = self
            .link
            .demo_state()
            .ok_or(ApiError::Local(LocalValidation::Unsupported))?;
        let s = demo.intake.get(self.tenant);
        Ok(IntakeSettings {
            folders: s.folders,
            updated_at: s.updated_at,
        })
    }

    fn put_now(
        &self,
        folders: &[IntakeFolder],
        expected: Option<&str>,
    ) -> Result<IntakeSettings, ApiError> {
        let mut demo = self
            .link
            .demo_state()
            .ok_or(ApiError::Local(LocalValidation::Unsupported))?;
        match demo.intake.put(self.tenant, folders.to_vec(), expected) {
            Ok(s) => Ok(IntakeSettings {
                folders: s.folders,
                updated_at: s.updated_at,
            }),
            Err(crate::company::demo::intake::DemoIntakeError::Stale) => {
                Err(demo_conflict(ConflictReason::Stale))
            }
            Err(crate::company::demo::intake::DemoIntakeError::Invalid(_)) => {
                Err(demo_conflict(ConflictReason::Unspecified))
            }
        }
    }
}

/// The demo answers at once.
impl IntakeEngine for DemoEngine {
    fn get(&self) -> impl std::future::Future<Output = Result<IntakeSettings, ApiError>> + Send {
        std::future::ready(self.get_now())
    }

    fn put(
        &self,
        folders: &[IntakeFolder],
        expected: Option<&str>,
    ) -> impl std::future::Future<Output = Result<IntakeSettings, ApiError>> + Send {
        std::future::ready(self.put_now(folders, expected))
    }
}

// ---------------------------------------------------------------------------
// The app's handle.

/// The app's intake inbox.
#[derive(Clone)]
pub(crate) struct Handle(Arc<Mutex<Intake>>);

impl Default for Handle {
    fn default() -> Self {
        Handle(Arc::new(Mutex::new(Intake::new())))
    }
}

impl Handle {
    pub(crate) fn lock(&self) -> std::sync::MutexGuard<'_, Intake> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn handle_id() -> egui::Id {
    egui::Id::unique("cl.intake_inbox")
}

/// The app's intake inbox (made on first use).
pub(crate) fn handle(ctx: &Context) -> Handle {
    ctx.data_mut(|d| d.get_temp_mut_or_default::<Handle>(handle_id()).clone())
}

/// The intake inbox if it was made.
pub(crate) fn existing(ctx: &Context) -> Option<Handle> {
    ctx.data(|d| d.get_temp::<Handle>(handle_id()))
}

/// One frame of the intake inbox (cheap when nothing changed).
pub(crate) fn tick_service(
    s: &mut Intake,
    link: &Arc<CompanyLink>,
    tenant: Option<TenantId>,
    today: CalendarDate,
    now: Instant,
) {
    s.today = Some(today);
    if s.tenant != tenant || (s.store.is_none() && tenant.is_some()) {
        s.reset_company(link, tenant);
    }
    if matches!(s.stage, Stage::Start) {
        s.start(link);
    }
    s.poll_stage(link);
    if !matches!(s.stage, Stage::Running) || s.fallback {
        return;
    }
    s.refresh_folders(link, now);
    s.poll_folders(link);
    s.take_snapshots();
    s.apply_moves(link, now);
    s.poll_jobs(now);
    s.poll_copy(link, now);
    s.next_copy(link, now);
    s.drive(link, now);
    s.poll_reconcile();
    s.start_reconcile(link, now);
}

/// Folders that need the owner (Home's Needs attention line, A01 D6).
#[must_use]
pub(crate) fn intake_problems(ctx: &Context) -> usize {
    existing(ctx).map_or(0, |h| h.lock().problems())
}

#[cfg(test)]
mod tests;
