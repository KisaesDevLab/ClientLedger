//! The receipt inbox behind the window (P17 WP5): one per app (kept in the
//! egui context), alive while a company is open, whether or not the inbox
//! window is.
//!
//! - A background thread lists the folder with `std::fs` (no watcher
//!   crate): every 5 s while the inbox or Enter Bill is in front (2 s while
//!   a file is settling), otherwise every 60 s. It also files originals
//!   away, so the egui thread never waits on the disk. The UI polls its
//!   results each frame ([`tick`]).
//! - The company's inbox state (`cl_settings::inbox_state`) says which
//!   files went up already. Uploads are checked against the engine after
//!   every listing (plan amendment S8): an upload a saved record now holds
//!   moves the original to `Filed/`; an upload the engine no longer has (a
//!   restore, a cleaned-up draft) is forgotten, so its file is offered
//!   again instead of hidden for ever.
//! - The menu's count is the files ready and not yet uploaded ([`badge`]).
//!
//! Nothing here logs a file name or a path.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

use cl_api::{ApiError, AttachmentId, RequestGroup};
use cl_company::inbox::{self, FileError, FileState, InboxFile, Listing, ScanError, Stability};
use cl_core::CalendarDate;
use cl_core::ids::TenantId;
use cl_settings::FolderError;
use cl_settings::inbox_state::{FileIdentity, InboxState, InboxStateStore, company_key, path_hash};
use eframe::egui::{self, Context};

use crate::company::{CompanyLink, Task};

/// Listing interval while the inbox or Enter Bill is in front.
pub(crate) const FAST: Duration = Duration::from_secs(5);
/// Listing interval otherwise.
pub(crate) const SLOW: Duration = Duration::from_secs(60);
/// How often the chosen folder is read again from the company's
/// preferences.
const FOLDER_RECHECK: Duration = Duration::from_secs(5);

/// The window kinds that list the inbox often.
pub(crate) const FAST_KINDS: [&str; 2] = [super::KIND, "vendors.enter_bill"];

/// What the folder choice is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Folder {
    /// No company is open.
    NoCompany,
    /// The owner has not chosen a folder (Preferences › Receipt inbox).
    NotChosen,
    /// The chosen path cannot be used (not absolute, a network folder).
    Refused(FolderError),
    /// The folder (it may still turn out to be missing).
    Chosen(PathBuf),
}

/// One listing from the thread.
#[derive(Clone, Debug)]
pub(crate) struct Snapshot {
    /// The folder listed.
    pub dir: PathBuf,
    /// What was found.
    pub listing: Result<Listing, ScanError>,
    /// Counts up with every listing.
    pub seq: u64,
}

/// What the inbox knows of one file beyond the folder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mark {
    /// Never uploaded.
    New,
    /// Uploaded to a form that is not saved yet; still offered.
    Uploaded,
    /// A saved record holds it; it moves to `Filed/` (retried).
    Linked,
}

impl Mark {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Mark::New => "new",
            Mark::Uploaded => "uploaded",
            Mark::Linked => "linked",
        }
    }
}

// ---------------------------------------------------------------------------
// The thread.

struct MoveJob {
    inbox: PathBuf,
    path: PathBuf,
    file: FileIdentity,
    year: i32,
    month: u32,
}

struct MoveDone {
    file: FileIdentity,
    result: Result<(), FileError>,
}

#[derive(Default)]
struct PollInner {
    dir: Option<PathBuf>,
    fast: bool,
    poke: bool,
    stop: bool,
    seq: u64,
    latest: Option<Snapshot>,
    moves: Vec<MoveJob>,
    moved: Vec<MoveDone>,
}

#[derive(Default)]
struct PollShared {
    inner: Mutex<PollInner>,
    wake: Condvar,
}

impl PollShared {
    fn lock(&self) -> std::sync::MutexGuard<'_, PollInner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The listing thread's handle; dropping it stops the thread.
struct Poller {
    shared: Arc<PollShared>,
    started: bool,
}

impl Poller {
    fn new() -> Poller {
        Poller {
            shared: Arc::new(PollShared::default()),
            started: false,
        }
    }

    fn ensure_started(&mut self) {
        if self.started {
            return;
        }
        let shared = Arc::clone(&self.shared);
        self.started = std::thread::Builder::new()
            .name("cl-receipt-inbox".into())
            .spawn(move || run(&shared))
            .is_ok();
        if !self.started {
            tracing::warn!("could not start the receipt inbox thread");
        }
    }

    fn configure(&self, dir: Option<&Path>, fast: bool) {
        let mut i = self.shared.lock();
        let changed = i.dir.as_deref() != dir || i.fast != fast;
        if changed {
            i.dir = dir.map(Path::to_path_buf);
            i.fast = fast;
            if i.latest
                .as_ref()
                .is_some_and(|s| Some(s.dir.as_path()) != dir)
            {
                i.latest = None;
            }
            drop(i);
            self.shared.wake.notify_all();
        }
    }

    fn poke(&self) {
        self.shared.lock().poke = true;
        self.shared.wake.notify_all();
    }

    /// The latest listing, when it is newer than `seen`.
    fn latest_after(&self, seen: Option<u64>) -> Option<Snapshot> {
        let i = self.shared.lock();
        i.latest.as_ref().filter(|l| Some(l.seq) != seen).cloned()
    }

    fn push_move(&self, job: MoveJob) {
        self.shared.lock().moves.push(job);
        self.shared.wake.notify_all();
    }

    fn take_moved(&self) -> Vec<MoveDone> {
        std::mem::take(&mut self.shared.lock().moved)
    }
}

impl Drop for Poller {
    fn drop(&mut self) {
        self.shared.lock().stop = true;
        self.shared.wake.notify_all();
    }
}

fn run(shared: &PollShared) {
    let mut stability = Stability::default();
    let mut last_dir: Option<PathBuf> = None;
    loop {
        let (dir, moves) = {
            let mut i = shared.lock();
            while !i.stop && i.dir.is_none() && i.moves.is_empty() {
                i = shared.wake.wait(i).unwrap_or_else(PoisonError::into_inner);
            }
            if i.stop {
                return;
            }
            i.poke = false;
            (i.dir.clone(), std::mem::take(&mut i.moves))
        };
        for m in moves {
            let result =
                inbox::file_away(&m.inbox, &m.path, &m.file.sha256, m.year, m.month).map(|_| ());
            shared.lock().moved.push(MoveDone {
                file: m.file,
                result,
            });
        }
        let Some(dir) = dir else {
            continue;
        };
        if last_dir.as_ref() != Some(&dir) {
            stability.clear();
            last_dir = Some(dir.clone());
        }
        let mut listing = inbox::scan(&dir);
        if let Ok(l) = &mut listing {
            stability.apply(l, Instant::now());
        }
        let settling = listing
            .as_ref()
            .is_ok_and(|l| l.files.iter().any(|f| f.state == FileState::Settling));
        let mut i = shared.lock();
        if i.stop {
            return;
        }
        if i.dir.as_ref() == Some(&dir) {
            i.seq += 1;
            let seq = i.seq;
            i.latest = Some(Snapshot {
                dir: dir.clone(),
                listing,
                seq,
            });
        }
        let wait = match (i.fast, settling) {
            (true, true) => inbox::STABLE_FOR,
            (true, false) => FAST,
            (false, _) => SLOW,
        };
        let fast = i.fast;
        let (i, _) = shared
            .wake
            .wait_timeout_while(i, wait, |i| {
                !i.stop
                    && !i.poke
                    && i.moves.is_empty()
                    && i.dir.as_ref() == Some(&dir)
                    && i.fast == fast
            })
            .unwrap_or_else(PoisonError::into_inner);
        drop(i);
    }
}

// ---------------------------------------------------------------------------
// The service.

/// What the engine says about one upload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Upload {
    /// Still in a draft (the form is not saved).
    Draft,
    /// A saved record holds it.
    Linked,
    /// The engine has no such file.
    Gone,
}

/// One entry's uploads as the engine sees them.
#[derive(Clone, Debug)]
pub(crate) struct Checked {
    file: FileIdentity,
    uploads: Vec<(String, Upload)>,
}

/// The inbox behind the window.
pub(crate) struct Service {
    poller: Poller,
    tenant: Option<TenantId>,
    company: String,
    folder: Folder,
    folder_checked: Option<Instant>,
    snapshot: Option<Snapshot>,
    store: Option<InboxStateStore>,
    state: InboxState,
    reconcile: Option<Task<Vec<Checked>>>,
    /// The listing the last check ran after (`None`: not since the company
    /// opened, so the next one is a full revalidation).
    reconciled_seq: Option<u64>,
    group: Option<RequestGroup>,
    /// The demo's own inbox folder, once the inbox window opened in
    /// `--demo`.
    demo_folder: Option<PathBuf>,
    fast_until: Option<Instant>,
    /// Originals filed away since the app started.
    pub filed: u32,
    /// Moves that failed and wait for a retry.
    pub move_failures: u32,
    /// Today's date (the `Filed/<YYYY-MM>` folder).
    today: Option<CalendarDate>,
}

impl Service {
    fn new() -> Service {
        Service {
            poller: Poller::new(),
            tenant: None,
            company: String::new(),
            folder: Folder::NoCompany,
            folder_checked: None,
            snapshot: None,
            store: None,
            state: InboxState::default(),
            reconcile: None,
            reconciled_seq: None,
            group: None,
            demo_folder: None,
            fast_until: None,
            filed: 0,
            move_failures: 0,
            today: None,
        }
    }

    /// The folder choice.
    pub(crate) fn folder(&self) -> &Folder {
        &self.folder
    }

    /// The last listing of the chosen folder.
    pub(crate) fn snapshot(&self) -> Option<&Snapshot> {
        self.snapshot
            .as_ref()
            .filter(|s| matches!(&self.folder, Folder::Chosen(d) if *d == s.dir))
    }

    /// The listed files, when the folder could be listed.
    pub(crate) fn listing(&self) -> Option<&Listing> {
        self.snapshot().and_then(|s| s.listing.as_ref().ok())
    }

    /// What the inbox knows of `file`.
    pub(crate) fn mark(&self, file: &InboxFile) -> Mark {
        match self.state.find(
            &self.company,
            &path_hash(&file.path),
            file.size,
            file.modified_ms,
        ) {
            None => Mark::New,
            Some(e) if e.linked => Mark::Linked,
            Some(_) => Mark::Uploaded,
        }
    }

    /// Files ready and never uploaded (the menu's count).
    pub(crate) fn badge(&self) -> usize {
        self.listing().map_or(0, |l| {
            l.files
                .iter()
                .filter(|f| f.state.offerable() && self.mark(f) == Mark::New)
                .count()
        })
    }

    /// List the folder now.
    pub(crate) fn poke(&self) {
        self.poller.poke();
    }

    /// The inbox window opened in `--demo`: use (and fill) the demo's own
    /// folder when the company has none.
    pub(crate) fn use_demo_folder(&mut self, link: &CompanyLink) {
        if !link.is_demo() {
            return;
        }
        let Some(dir) = super::demo::folder(link) else {
            return;
        };
        super::demo::seed(&dir);
        self.demo_folder = Some(dir);
        self.folder_checked = None;
    }

    /// Records an upload of `file` (`read` is what was sent). A `linked`
    /// upload (straight onto a saved record) files the original away now;
    /// a draft's waits until a saved record holds it.
    pub(crate) fn record_upload(
        &mut self,
        file: &InboxFile,
        identity: &FileIdentity,
        attachment: AttachmentId,
        linked: bool,
    ) {
        self.state
            .record_upload(&self.company, identity, &attachment.to_string());
        if linked {
            self.state.mark_linked(&self.company, identity);
        }
        self.save_state();
        if linked {
            self.file_away(file.path.clone(), identity.clone());
        }
        // Check the uploads again on the next frame.
        self.reconciled_seq = None;
    }

    fn file_away(&self, path: PathBuf, file: FileIdentity) {
        let Folder::Chosen(inbox) = &self.folder else {
            return;
        };
        let today = self.today.unwrap_or_else(crate::shell::today);
        self.poller.push_move(MoveJob {
            inbox: inbox.clone(),
            path,
            file,
            year: today.year(),
            month: today.month(),
        });
    }

    fn save_state(&self) {
        if let Some(store) = &self.store
            && let Err(e) = store.save(&self.state)
        {
            tracing::warn!(error = %e, "could not save the receipt inbox state");
        }
    }

    fn reset_company(&mut self, link: &CompanyLink, tenant: Option<TenantId>) {
        self.tenant = tenant;
        self.company = tenant
            .map(|t| company_key(&t.to_string()))
            .unwrap_or_default();
        self.snapshot = None;
        self.reconcile = None;
        self.reconciled_seq = None;
        self.folder_checked = None;
        self.store = link.store().path().parent().map(InboxStateStore::in_dir);
        self.state = self
            .store
            .as_ref()
            .and_then(|s| s.load().ok())
            .unwrap_or_default();
        self.group = link.group().and_then(Result::ok);
    }

    fn refresh_folder(&mut self, link: &CompanyLink) {
        self.folder_checked = Some(Instant::now());
        let Some(tenant) = self.tenant else {
            self.folder = Folder::NoCompany;
            return;
        };
        let chosen = link
            .store()
            .load()
            .ok()
            .and_then(|l| {
                l.get(tenant)
                    .and_then(|r| r.prefs.receipt_inbox_dir.clone())
            })
            .filter(|p| !p.as_os_str().is_empty());
        self.folder = match chosen {
            Some(p) => match cl_settings::check_inbox_folder(&p) {
                // A missing folder is listed (and reported) like any other.
                Ok(()) | Err(FolderError::Missing) => Folder::Chosen(p),
                Err(e) => Folder::Refused(e),
            },
            None => self
                .demo_folder
                .clone()
                .map_or(Folder::NotChosen, Folder::Chosen),
        };
    }

    /// Drop entries whose file is no longer in the folder (moved or
    /// deleted by the owner, or changed since it went up).
    fn prune(&mut self) {
        let Some(listing) = self.listing() else {
            return;
        };
        if listing.more > 0 {
            return;
        }
        let present: Vec<(String, u64, i64)> = listing
            .files
            .iter()
            .map(|f| (path_hash(&f.path), f.size, f.modified_ms))
            .collect();
        let before = self.state.entries(&self.company).len();
        if let Some(list) = self.state.companies.get_mut(&self.company) {
            list.retain(|e| present.iter().any(|(p, s, m)| e.file.matches(p, *s, *m)));
            if list.is_empty() {
                self.state.companies.remove(&self.company);
            }
        }
        if self.state.entries(&self.company).len() != before {
            self.save_state();
        }
    }

    fn apply_moves(&mut self) {
        let done = self.poller.take_moved();
        if done.is_empty() {
            return;
        }
        for d in done {
            match d.result {
                Ok(()) => {
                    self.filed += 1;
                    self.state.remove(&self.company, &d.file);
                }
                // Gone, or not the file that went up: nothing to file.
                Err(FileError::Gone | FileError::Changed | FileError::NotAFile) => {
                    self.state.remove(&self.company, &d.file);
                }
                Err(_) => {
                    self.move_failures += 1;
                    tracing::warn!("a receipt inbox file could not be filed away; retrying later");
                }
            }
        }
        self.save_state();
        self.poke();
    }

    fn start_reconcile(&mut self, link: &Arc<CompanyLink>) {
        let entries: Vec<(FileIdentity, Vec<String>)> = self
            .state
            .entries(&self.company)
            .iter()
            .map(|e| (e.file.clone(), e.attachments.clone()))
            .collect();
        if entries.is_empty() {
            return;
        }
        if let Some(demo) = link.demo_state() {
            let checked = entries
                .into_iter()
                .map(|(file, ids)| Checked {
                    file,
                    uploads: ids
                        .into_iter()
                        .map(|id| {
                            let state = AttachmentId::parse(&id)
                                .ok()
                                .and_then(|a| {
                                    crate::company::demo::checks::find_attachment(&demo, a)
                                })
                                .map_or(Upload::Gone, |a| upload_of(&a.attachable_type));
                            (id, state)
                        })
                        .collect(),
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
            let mut out = Vec::with_capacity(entries.len());
            for (file, ids) in entries {
                let mut uploads = Vec::with_capacity(ids.len());
                for id in ids {
                    let state = match AttachmentId::parse(&id) {
                        Err(_) => Upload::Gone,
                        Ok(a) => match s.attachments().get(a).await {
                            Ok(a) => upload_of(&a.attachable_type),
                            Err(ApiError::NotFound { .. }) => Upload::Gone,
                            Err(e) => return Err(e),
                        },
                    };
                    uploads.push((id, state));
                }
                out.push(Checked { file, uploads });
            }
            Ok(out)
        }) {
            self.reconcile = Some(Task::Pending(p));
        }
    }

    fn apply_reconcile(&mut self, checked: Vec<Checked>) {
        for c in checked {
            for (id, state) in &c.uploads {
                if *state == Upload::Gone {
                    self.state.drop_attachment(&self.company, id);
                }
            }
            if c.uploads.iter().any(|(_, s)| *s == Upload::Linked) {
                self.state.mark_linked(&self.company, &c.file);
            }
        }
        self.save_state();
        // Every linked entry whose file is here moves (again) now.
        let Some(listing) = self.listing() else {
            return;
        };
        let jobs: Vec<(PathBuf, FileIdentity)> = self
            .state
            .entries(&self.company)
            .iter()
            .filter(|e| e.linked)
            .filter_map(|e| {
                listing
                    .files
                    .iter()
                    .find(|f| e.file.matches(&path_hash(&f.path), f.size, f.modified_ms))
                    .map(|f| (f.path.clone(), e.file.clone()))
            })
            .collect();
        for (path, file) in jobs {
            self.file_away(path, file);
        }
    }

    fn poll_reconcile(&mut self) {
        let Some(r) = self.reconcile.as_mut().and_then(Task::try_take) else {
            return;
        };
        self.reconcile = None;
        match r {
            Ok(checked) => self.apply_reconcile(checked),
            Err(e) => {
                tracing::debug!(error = %e, "receipt inbox upload check failed; retrying later");
            }
        }
    }

    fn busy(&self) -> bool {
        self.reconcile.is_some()
    }
}

fn upload_of(attachable_type: &str) -> Upload {
    if attachable_type == "draft" {
        Upload::Draft
    } else {
        Upload::Linked
    }
}

/// The app's inbox service.
#[derive(Clone)]
pub(crate) struct Handle(Arc<Mutex<Service>>);

impl Default for Handle {
    fn default() -> Self {
        Handle(Arc::new(Mutex::new(Service::new())))
    }
}

impl Handle {
    pub(crate) fn lock(&self) -> std::sync::MutexGuard<'_, Service> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn handle_id() -> egui::Id {
    egui::Id::unique("cl.receipt_inbox")
}

/// The app's inbox service (made on first use).
pub(crate) fn handle(ctx: &Context) -> Handle {
    ctx.data_mut(|d| d.get_temp_mut_or_default::<Handle>(handle_id()).clone())
}

/// One frame of the inbox: the shell calls it every frame with the open
/// company (`fast` when the inbox or Enter Bill is in front); the inbox
/// window calls it too. Cheap when nothing changed.
pub(crate) fn tick(
    ctx: &Context,
    link: &Arc<CompanyLink>,
    tenant: Option<TenantId>,
    today: CalendarDate,
    fast: bool,
) {
    let h = handle(ctx);
    let mut s = h.lock();
    tick_service(&mut s, link, tenant, today, fast, Instant::now());
    let wait = if s.busy() {
        Duration::from_millis(100)
    } else if s.fast_until.is_some_and(|u| u > Instant::now()) {
        Duration::from_millis(500)
    } else {
        SLOW
    };
    drop(s);
    if tenant.is_some() {
        ctx.request_repaint_after(wait);
    }
}

pub(crate) fn tick_service(
    s: &mut Service,
    link: &Arc<CompanyLink>,
    tenant: Option<TenantId>,
    today: CalendarDate,
    fast: bool,
    now: Instant,
) {
    s.poller.ensure_started();
    s.today = Some(today);
    if s.tenant != tenant || s.store.is_none() {
        s.reset_company(link, tenant);
    }
    if s.folder_checked
        .is_none_or(|t| now.duration_since(t) >= FOLDER_RECHECK)
    {
        s.refresh_folder(link);
    }
    if fast {
        s.fast_until = Some(now + Duration::from_secs(1));
    }
    let fast_now = s.fast_until.is_some_and(|u| u > now);
    let dir = match &s.folder {
        Folder::Chosen(d) => Some(d.clone()),
        _ => None,
    };
    s.poller.configure(dir.as_deref(), fast_now);
    if let Some(latest) = s.poller.latest_after(s.snapshot.as_ref().map(|x| x.seq)) {
        s.snapshot = Some(latest);
        s.prune();
    }
    s.apply_moves();
    s.poll_reconcile();
    let seq = s.snapshot.as_ref().map(|x| x.seq);
    if s.reconcile.is_none()
        && seq.is_some()
        && s.reconciled_seq != seq
        && !s.state.entries(&s.company).is_empty()
    {
        s.reconciled_seq = seq;
        s.start_reconcile(link);
    }
}

/// The menu's count: ready files never uploaded (0 without a company).
#[must_use]
pub(crate) fn badge(ctx: &Context) -> usize {
    ctx.data(|d| d.get_temp::<Handle>(handle_id()))
        .map_or(0, |h| h.lock().badge())
}

/// Intake folders that need the owner (Home's Needs attention line, A01 D6).
/// A01 WP-A stub: 0 until WP-C's multi-folder service counts them.
#[must_use]
pub(crate) fn intake_problems(_ctx: &Context) -> usize {
    0
}
