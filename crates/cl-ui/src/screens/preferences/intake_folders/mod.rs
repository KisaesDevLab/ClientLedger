//! Preferences › Document intake (A01 WP-A): the company's intake folders
//! (kept in the engine, U-93, Q-A01a) and this computer's confirmation of
//! each one (`CompanyPrefs::intake_here`, never archived, Q-A01b).
//!
//! - The table shows Name · Folder · Kind · Status. An Admin (engine role
//!   `owner` until P29, Q-A01k) can Add a folder, Rename it inline, Remove
//!   it (the files stay where they are), Test it, and switch Leave
//!   originals where they are and Read new documents automatically (shown
//!   with its "coming update" hint, Q-A01j). Everyone else sees the list
//!   read-only.
//! - **Nothing touches a folder until the Admin asks.** Typed paths are
//!   checked by syntax only, on the UI thread. Test runs on its own thread
//!   with a 15 s deadline, and so does the automatic test after Add. A
//!   folder is confirmed for this computer only from a passing test.
//! - **Save ordering.** The panel never writes `intake_here` itself.
//!   Pending confirmations wait in [`IntakeEdits`]. On Save the folder list
//!   is PUT to the engine with its `updatedAt`. Only once the engine accepts
//!   it are the confirmations written to the company record, in one store
//!   update, and the save counter ([`saves`]) moves on so the inbox service
//!   re-reads its folders. A 409 writes nothing locally and shows "Intake
//!   folders were changed elsewhere. Reload to see them."
//!
//! Folder names and paths are owner data: they appear on screen only,
//! never in `state()`, `Debug` or a log line.

pub mod source;
pub mod wp_b;

#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Receiver;

use cl_company::CompanyError;
use cl_core::ids::{IntakeFolderId, TenantId};
use cl_core::intake::{IntakeFolder, IntakeKind};
use cl_settings::HereNote;
use cl_settings::migrate_inbox::{MAX_FOLDERS, same_path_key};
use eframe::egui::{self, RichText, Ui};
use serde_json::{Value, json};

use self::source::{Fetched, Stored};
use self::wp_b::{Health, PathProblem, ProbeReport};
use crate::company::{CompanyLink, Task};
use crate::i18n::{t, t_count};
use crate::screens::company::form;
use crate::theme::Tokens;

/// Files above which a newly confirmed folder asks before bringing
/// everything in (Q-A01h).
pub const BRING_IN_ASK_ABOVE: usize = 50;
/// Longest folder name (engine rule).
pub const MAX_NAME_CHARS: usize = 100;

/// Intake-folder saves so far in this app session. The inbox service
/// re-reads the folders when the number moves on.
static SAVES: AtomicU64 = AtomicU64::new(0);

/// How many intake-folder saves this app session has made.
#[must_use]
pub fn saves() -> u64 {
    SAVES.load(Ordering::SeqCst)
}

/// What the owner can change on the panel. Compared whole, with the rest
/// of the Preferences edits, for the dirty dot.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IntakeEdits {
    /// The company's folders, in order.
    pub folders: Vec<IntakeFolder>,
    /// Confirmations made in this window, written on Save once the engine
    /// accepted the list.
    pub confirm: BTreeMap<IntakeFolderId, HereNote>,
}

/// Where the panel's read stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadState {
    /// No company is open.
    NoCompany,
    /// Reading.
    Loading,
    /// Read.
    Ready,
    /// The engine has no intake settings yet (Q-A01q).
    NeedsEngine,
    /// The read failed.
    Failed,
}

impl LoadState {
    fn key(self) -> &'static str {
        match self {
            LoadState::NoCompany => "no_company",
            LoadState::Loading => "loading",
            LoadState::Ready => "ready",
            LoadState::NeedsEngine => "needs_engine",
            LoadState::Failed => "failed",
        }
    }
}

/// Why the intake folders can't be saved as they are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntakeBlock {
    /// A folder has no name.
    NameEmpty,
    /// A name is over 100 characters.
    NameTooLong,
    /// Two folders share a name.
    NameTaken,
    /// More than 20 folders.
    TooMany,
    /// A path breaks a rule.
    Path(PathProblem),
    /// Two folders share a path.
    PathTaken,
    /// Another company on this computer uses the folder (Q-A01i).
    MappedByAnother,
    /// Only an Admin can change the folders.
    ReadOnly,
    /// The folders have not been read yet.
    NotLoaded,
    /// The engine refused the list.
    Refused,
}

impl IntakeBlock {
    /// i18n key of the owner message.
    #[must_use]
    pub fn message_key(self) -> &'static str {
        match self {
            IntakeBlock::NameEmpty => "prefs.intake.err.name_empty",
            IntakeBlock::NameTooLong => "prefs.intake.err.name_long",
            IntakeBlock::NameTaken => "prefs.intake.err.name_taken",
            IntakeBlock::TooMany => "prefs.intake.err.too_many",
            IntakeBlock::Path(p) => path_problem_key(p),
            IntakeBlock::PathTaken => "prefs.intake.err.path_taken",
            IntakeBlock::MappedByAnother => "prefs.intake.err.mapped",
            IntakeBlock::ReadOnly => "prefs.intake.err.read_only",
            IntakeBlock::NotLoaded => "prefs.intake.err.not_loaded",
            IntakeBlock::Refused => "prefs.intake.err.refused",
        }
    }
}

/// i18n key for a refused path.
#[must_use]
pub fn path_problem_key(p: PathProblem) -> &'static str {
    match p {
        PathProblem::Empty => "prefs.intake.path.empty",
        PathProblem::TooLong => "prefs.intake.path.too_long",
        PathProblem::ControlChars => "prefs.intake.path.control",
        PathProblem::NotAbsolute => "prefs.intake.path.not_absolute",
        PathProblem::Device => "prefs.intake.path.device",
        PathProblem::Url => "prefs.intake.path.url",
        PathProblem::WebDav => "prefs.intake.path.webdav",
        PathProblem::DotDot => "prefs.intake.path.dot_dot",
        PathProblem::AppFolder => "prefs.intake.path.app_folder",
        PathProblem::DriveRoot => "prefs.intake.path.drive_root",
    }
}

/// i18n key and control-channel word for a folder's health.
#[must_use]
pub fn health_key(h: Health) -> (&'static str, &'static str) {
    match h {
        Health::Ok => ("prefs.intake.health.ok", "ok"),
        Health::NotConfirmed => ("prefs.intake.health.not_confirmed", "not_confirmed"),
        Health::Missing => ("prefs.intake.health.missing", "missing"),
        Health::Unreachable => ("prefs.intake.health.unreachable", "unreachable"),
        Health::NotResponding => ("prefs.intake.health.not_responding", "not_responding"),
        Health::PermissionDenied => ("prefs.intake.health.permission", "permission_denied"),
        Health::NotAFolder => ("prefs.intake.health.not_folder", "not_a_folder"),
        Health::CantMove => ("prefs.intake.health.cant_move", "cant_move"),
        Health::Moved => ("prefs.intake.health.moved", "moved"),
        Health::MappedByAnother => ("prefs.intake.health.mapped", "mapped_by_another"),
        Health::Slow => ("prefs.intake.health.slow", "slow"),
        Health::TooMany => ("prefs.intake.health.too_many", "too_many"),
    }
}

/// i18n key of a kind.
#[must_use]
pub fn kind_key(k: IntakeKind) -> &'static str {
    match k {
        IntakeKind::Bills => "prefs.intake.kind.bills",
        IntakeKind::Receipts => "prefs.intake.kind.receipts",
        IntakeKind::Auto => "prefs.intake.kind.auto",
    }
}

fn kind_word(k: IntakeKind) -> &'static str {
    match k {
        IntakeKind::Bills => "bills",
        IntakeKind::Receipts => "receipts",
        IntakeKind::Auto => "auto",
    }
}

/// Whether a probe passed well enough to confirm the folder.
fn usable(r: &ProbeReport) -> bool {
    matches!(r.health, Health::Ok | Health::TooMany | Health::Slow) && r.canonical_key.is_some()
}

/// A new folder id.
#[must_use]
pub fn new_folder_id() -> IntakeFolderId {
    IntakeFolderId::from_uuid(uuid::Uuid::new_v4())
}

fn now_ms() -> i64 {
    cl_settings::recent::now().unix_millis()
}

/// The Add folder row as typed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AddRow {
    /// Name.
    pub name: String,
    /// Path.
    pub path: String,
    /// Kind.
    pub kind: Option<IntakeKind>,
    /// The last refusal.
    pub problem: Option<IntakeBlock>,
}

/// One Test, running or answered.
#[derive(Default)]
struct Probe {
    rx: Option<Receiver<ProbeReport>>,
    report: Option<ProbeReport>,
    /// The path tested (a report says nothing about another path).
    path: String,
}

/// What the panel asks the screen to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelAction {
    /// Nothing.
    None,
    /// Read the folders again, replacing the typed changes.
    Reload,
}

/// The panel's runtime state (not part of the edits).
pub struct IntakePanel {
    /// Where the read stands.
    pub load: LoadState,
    /// The engine's stamp for the folders in the committed edits.
    pub updated_at: Option<String>,
    /// The user's engine role (`None`: not known yet; read-only).
    pub role: Option<String>,
    load_task: Option<Task<Result<Fetched, CompanyError>>>,
    replace_on_load: bool,
    probes: BTreeMap<IntakeFolderId, Probe>,
    /// Folders added in this window: confirmed by their first passing test.
    auto_confirm: BTreeSet<IntakeFolderId>,
    /// The Add folder row, while open.
    pub add: Option<AddRow>,
    renaming: Option<(IntakeFolderId, String)>,
    removing: Option<IntakeFolderId>,
    /// A confirmed-to-be folder holding many files: waiting for "Bring in
    /// now / Only new files".
    ask_bring_in: Option<IntakeFolderId>,
    /// The last save met a `409 STALE`.
    pub stale: bool,
    /// Pinned roots other companies on this computer confirmed (Q-A01i).
    other_keys: BTreeSet<String>,
}

impl std::fmt::Debug for IntakePanel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IntakePanel")
            .field("load", &self.load)
            .field("stale", &self.stale)
            .finish_non_exhaustive()
    }
}

impl Default for IntakePanel {
    fn default() -> Self {
        IntakePanel {
            load: LoadState::NoCompany,
            updated_at: None,
            role: None,
            load_task: None,
            replace_on_load: false,
            probes: BTreeMap::new(),
            auto_confirm: BTreeSet::new(),
            add: None,
            renaming: None,
            removing: None,
            ask_bring_in: None,
            stale: false,
            other_keys: BTreeSet::new(),
        }
    }
}

impl IntakePanel {
    /// Whether the user may change the folders.
    #[must_use]
    pub fn is_admin(&self) -> bool {
        self.role.as_deref() == Some(source::ADMIN_ROLE)
    }

    /// Whether the folders can be edited now.
    #[must_use]
    pub fn editable(&self) -> bool {
        self.load == LoadState::Ready && self.is_admin()
    }

    /// Starts reading the folders and the user's role. The demo answers
    /// at once (returned); the engine answers through [`IntakePanel::poll`].
    pub fn start_load(
        &mut self,
        link: &CompanyLink,
        tenant: Option<TenantId>,
        group: Option<&cl_api::RequestGroup>,
    ) -> Option<Fetched> {
        let Some(tenant) = tenant else {
            self.load = LoadState::NoCompany;
            return None;
        };
        self.other_keys = link
            .store()
            .load()
            .unwrap_or_default()
            .live()
            .filter(|r| r.tenant_id != tenant)
            .flat_map(|r| {
                r.prefs
                    .intake_here
                    .values()
                    .map(|n| n.canonical_key.clone())
            })
            .filter(|k| !k.is_empty())
            .collect();
        if !source::supported(link) {
            self.load = LoadState::NeedsEngine;
            return None;
        }
        if link.is_demo() {
            return match source::demo_get(link, tenant) {
                Some(f) => Some(f),
                None => {
                    self.load = LoadState::Failed;
                    None
                }
            };
        }
        let (Some(session), Some(group)) = (link.session(), group) else {
            self.load = LoadState::Failed;
            return None;
        };
        self.load = LoadState::Loading;
        let pending = group.read(&session, move |s| async move {
            let stored = match source::engine_get(&s).await {
                Ok(v) => v,
                Err(e) => return Ok(Err(CompanyError::from(e))),
            };
            let role = source::engine_role(&s, tenant).await;
            Ok(Ok(Fetched { stored, role }))
        });
        match pending {
            Ok(p) => self.load_task = Some(Task::Pending(p)),
            Err(_) => self.load = LoadState::Failed,
        }
        None
    }

    /// Asks the next read to replace the typed changes (Reload after a 409).
    pub fn replace_next_load(&mut self) {
        self.replace_on_load = true;
    }

    /// Takes what the read found. Typed changes stay unless they are
    /// clean or a Reload asked for the engine's list.
    pub fn apply(&mut self, f: Fetched, edits: &mut IntakeEdits, committed: &mut IntakeEdits) {
        let clean = edits == committed;
        committed.folders = f.stored.folders;
        committed.confirm.clear();
        if clean || self.replace_on_load {
            *edits = committed.clone();
            self.probes.clear();
            self.auto_confirm.clear();
            self.add = None;
            self.renaming = None;
            self.removing = None;
            self.ask_bring_in = None;
            self.stale = false;
        }
        self.replace_on_load = false;
        self.updated_at = f.stored.updated_at;
        self.role = f.role;
        self.load = LoadState::Ready;
    }

    /// The engine accepted `stored`: the panel's baseline moves on.
    pub fn saved(&mut self, stored: &Stored) {
        self.updated_at.clone_from(&stored.updated_at);
        self.stale = false;
        self.auto_confirm.clear();
        SAVES.fetch_add(1, Ordering::SeqCst);
    }

    /// Whether any test is still running.
    #[must_use]
    pub fn busy(&self) -> bool {
        self.load_task.is_some() || self.probes.values().any(|p| p.rx.is_some())
    }

    /// Polls the read and the running tests.
    pub fn poll(&mut self, edits: &mut IntakeEdits, committed: &mut IntakeEdits) {
        if let Some(task) = self.load_task.as_mut()
            && let Some(r) = task.poll_result()
        {
            self.load_task = None;
            match r {
                Ok(f) => self.apply(f, edits, committed),
                Err(_) => self.load = LoadState::Failed,
            }
        }
        let mut answered = Vec::new();
        for (id, p) in &mut self.probes {
            if let Some(rx) = &p.rx {
                match rx.try_recv() {
                    Ok(report) => {
                        p.report = Some(report);
                        p.rx = None;
                        answered.push(*id);
                    }
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        p.report = Some(ProbeReport {
                            health: Health::NotResponding,
                            files: 0,
                            too_large: 0,
                            heic: 0,
                            online_only: 0,
                            writable: None,
                            cloud: false,
                            canonical_key: None,
                        });
                        p.rx = None;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => {}
                }
            }
        }
        for id in answered {
            if self.auto_confirm.contains(&id) {
                self.offer_confirm(edits, id);
            }
        }
    }

    /// Runs Test on `id` (one at a time per folder).
    pub fn test(&mut self, edits: &IntakeEdits, id: IntakeFolderId, app_dirs: &[PathBuf]) {
        let Some(f) = edits.folders.iter().find(|f| f.id == id) else {
            return;
        };
        if self.probes.get(&id).is_some_and(|p| p.rx.is_some()) {
            return;
        }
        let dirs: Vec<&Path> = app_dirs.iter().map(PathBuf::as_path).collect();
        let root = match wp_b::check_path_syntax(&f.path, &dirs) {
            Ok(r) => r,
            Err(_) => return,
        };
        self.probes.insert(
            id,
            Probe {
                rx: Some(wp_b::probe_async(root, !f.keep_originals)),
                report: None,
                path: f.path.clone(),
            },
        );
    }

    fn report(&self, f: &IntakeFolder) -> Option<&ProbeReport> {
        self.probes
            .get(&f.id)
            .filter(|p| p.path == f.path)
            .and_then(|p| p.report.as_ref())
    }

    fn testing(&self, id: IntakeFolderId) -> bool {
        self.probes.get(&id).is_some_and(|p| p.rx.is_some())
    }

    /// After a passing test: confirm at once, or ask first when the folder
    /// already holds many files.
    fn offer_confirm(&mut self, edits: &mut IntakeEdits, id: IntakeFolderId) {
        let Some(f) = edits.folders.iter().find(|f| f.id == id) else {
            return;
        };
        let Some(r) = self.report(f) else { return };
        if !usable(r) {
            return;
        }
        if r.files > BRING_IN_ASK_ABOVE {
            self.ask_bring_in = Some(id);
        } else {
            self.confirm(edits, id, false);
        }
    }

    /// Marks `id` for this computer (written on Save). `only_new`: files
    /// already there are listed, not copied (Q-A01h).
    pub fn confirm(&mut self, edits: &mut IntakeEdits, id: IntakeFolderId, only_new: bool) {
        let Some(f) = edits.folders.iter().find(|f| f.id == id) else {
            return;
        };
        let Some(key) = self.report(f).and_then(|r| r.canonical_key.clone()) else {
            return;
        };
        let note = HereNote {
            path_hash: cl_settings::path_hash(&f.path),
            canonical_key: key,
            since_ms: only_new.then(now_ms),
            confirmed_at: Some(cl_settings::recent::now()),
        };
        edits.confirm.insert(id, note);
        self.auto_confirm.remove(&id);
        if self.ask_bring_in == Some(id) {
            self.ask_bring_in = None;
        }
    }

    /// Adds the typed folder; it is tested at once (the Admin asked for it
    /// on this computer) and confirmed by a passing test.
    ///
    /// # Errors
    /// Why it can't be added.
    pub fn add_folder(
        &mut self,
        edits: &mut IntakeEdits,
        app_dirs: &[PathBuf],
    ) -> Result<IntakeFolderId, IntakeBlock> {
        let row = self.add.clone().unwrap_or_default();
        let name = row.name.trim().to_owned();
        let path = row.path.trim().to_owned();
        let checked = check_new(edits, None, &name, &path, app_dirs);
        if let Err(b) = checked {
            if let Some(a) = self.add.as_mut() {
                a.problem = Some(b);
            }
            return Err(b);
        }
        let id = new_folder_id();
        edits.folders.push(IntakeFolder {
            id,
            name,
            path,
            default_kind: row.kind.unwrap_or(IntakeKind::Auto),
            auto_read: false,
            keep_originals: false,
        });
        self.add = None;
        self.auto_confirm.insert(id);
        self.test(edits, id, app_dirs);
        Ok(id)
    }

    /// Removes `id` from the list; its files stay where they are.
    pub fn remove(&mut self, edits: &mut IntakeEdits, id: IntakeFolderId) {
        edits.folders.retain(|f| f.id != id);
        edits.confirm.remove(&id);
        self.probes.remove(&id);
        self.auto_confirm.remove(&id);
        if self.removing == Some(id) {
            self.removing = None;
        }
        if self.ask_bring_in == Some(id) {
            self.ask_bring_in = None;
        }
    }

    /// Renames `id`.
    ///
    /// # Errors
    /// The name is empty, too long or taken.
    pub fn rename(
        &mut self,
        edits: &mut IntakeEdits,
        id: IntakeFolderId,
        name: &str,
    ) -> Result<(), IntakeBlock> {
        let name = name.trim();
        check_name(edits, Some(id), name)?;
        if let Some(f) = edits.folders.iter_mut().find(|f| f.id == id) {
            name.clone_into(&mut f.name);
        }
        self.renaming = None;
        Ok(())
    }

    /// Checks the folders before a save.
    ///
    /// # Errors
    /// The first [`IntakeBlock`].
    pub fn validate(
        &self,
        edits: &IntakeEdits,
        committed: &IntakeEdits,
        app_dirs: &[PathBuf],
    ) -> Result<(), IntakeBlock> {
        if edits == committed {
            return Ok(());
        }
        if self.load != LoadState::Ready {
            return Err(IntakeBlock::NotLoaded);
        }
        if !self.is_admin() {
            return Err(IntakeBlock::ReadOnly);
        }
        if edits.folders.len() > MAX_FOLDERS {
            return Err(IntakeBlock::TooMany);
        }
        let dirs: Vec<&Path> = app_dirs.iter().map(PathBuf::as_path).collect();
        let mut names = BTreeSet::new();
        let mut paths = BTreeSet::new();
        for f in &edits.folders {
            let name = f.name.trim();
            if name.is_empty() {
                return Err(IntakeBlock::NameEmpty);
            }
            if name.chars().count() > MAX_NAME_CHARS {
                return Err(IntakeBlock::NameTooLong);
            }
            if !names.insert(name.to_lowercase()) {
                return Err(IntakeBlock::NameTaken);
            }
            wp_b::check_path_syntax(&f.path, &dirs).map_err(IntakeBlock::Path)?;
            if !paths.insert(same_path_key(&f.path)) {
                return Err(IntakeBlock::PathTaken);
            }
        }
        let mut keys = BTreeSet::new();
        for note in edits.confirm.values() {
            if self.other_keys.contains(&note.canonical_key) || !keys.insert(&note.canonical_key) {
                return Err(IntakeBlock::MappedByAnother);
            }
        }
        Ok(())
    }

    /// The control channel's view: indexes, kinds and states, never a
    /// name or a path.
    #[must_use]
    pub fn state(
        &self,
        edits: &IntakeEdits,
        committed: &IntakeEdits,
        here: &BTreeMap<IntakeFolderId, HereNote>,
    ) -> Value {
        let folders: Vec<Value> = edits
            .folders
            .iter()
            .enumerate()
            .map(|(i, f)| {
                json!({
                    "index": i,
                    "kind": kind_word(f.default_kind),
                    "keep_originals": f.keep_originals,
                    "auto_read": f.auto_read,
                    "new": !committed.folders.iter().any(|c| c.id == f.id),
                    "here": confirmed(here, f),
                    "pending": edits.confirm.contains_key(&f.id),
                    "testing": self.testing(f.id),
                    "probe": self.report(f).map(|r| health_key(r.health).1),
                    "files": self.report(f).map(|r| r.files),
                })
            })
            .collect();
        json!({
            "load": self.load.key(),
            "admin": self.is_admin(),
            "editable": self.editable(),
            "stale": self.stale,
            "adding": self.add.is_some(),
            "add_problem": self.add.as_ref().and_then(|a| a.problem).map(|b| b.message_key()),
            "renaming": self.renaming.is_some(),
            "removing": self.removing.is_some(),
            "asking_bring_in": self.ask_bring_in.is_some(),
            "dirty": edits != committed,
            "folders": folders,
        })
    }
}

/// Whether `f` is confirmed on this computer at its current path.
fn confirmed(here: &BTreeMap<IntakeFolderId, HereNote>, f: &IntakeFolder) -> bool {
    here.get(&f.id)
        .is_some_and(|n| n.path_hash == cl_settings::path_hash(&f.path))
}

fn check_name(
    edits: &IntakeEdits,
    except: Option<IntakeFolderId>,
    name: &str,
) -> Result<(), IntakeBlock> {
    if name.is_empty() {
        return Err(IntakeBlock::NameEmpty);
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(IntakeBlock::NameTooLong);
    }
    let want = name.to_lowercase();
    if edits
        .folders
        .iter()
        .any(|f| Some(f.id) != except && f.name.trim().to_lowercase() == want)
    {
        return Err(IntakeBlock::NameTaken);
    }
    Ok(())
}

/// The inline checks of a new folder: no I/O.
fn check_new(
    edits: &IntakeEdits,
    except: Option<IntakeFolderId>,
    name: &str,
    path: &str,
    app_dirs: &[PathBuf],
) -> Result<(), IntakeBlock> {
    if except.is_none() && edits.folders.len() >= MAX_FOLDERS {
        return Err(IntakeBlock::TooMany);
    }
    check_name(edits, except, name)?;
    let dirs: Vec<&Path> = app_dirs.iter().map(PathBuf::as_path).collect();
    wp_b::check_path_syntax(path, &dirs).map_err(IntakeBlock::Path)?;
    let key = same_path_key(path);
    if edits
        .folders
        .iter()
        .any(|f| Some(f.id) != except && same_path_key(&f.path) == key)
    {
        return Err(IntakeBlock::PathTaken);
    }
    Ok(())
}

/// The folder's Status column text.
fn status_text(
    panel: &IntakePanel,
    edits: &IntakeEdits,
    committed: &IntakeEdits,
    here: &BTreeMap<IntakeFolderId, HereNote>,
    f: &IntakeFolder,
) -> String {
    if panel.testing(f.id) {
        return t("prefs.intake.testing");
    }
    if let Some(r) = panel.report(f)
        && !usable(r)
    {
        return t(health_key(r.health).0);
    }
    if edits.confirm.contains_key(&f.id) {
        return t("prefs.intake.status.ready");
    }
    if !committed.folders.iter().any(|c| c.id == f.id) {
        return t("prefs.intake.status.new");
    }
    if confirmed(here, f) {
        return t("prefs.intake.status.here");
    }
    t("prefs.intake.status.not_here")
}

/// The Test report's details under a folder.
fn report_lines(r: &ProbeReport) -> Vec<String> {
    let mut out = Vec::new();
    if !usable(r) {
        return out;
    }
    out.push(t_count("prefs.intake.test.ok", r.files, &[]));
    if r.too_large > 0 {
        out.push(t_count("prefs.intake.test.too_large", r.too_large, &[]));
    }
    if r.heic > 0 {
        out.push(t_count("prefs.intake.test.heic", r.heic, &[]));
    }
    if r.online_only > 0 {
        out.push(t_count("prefs.intake.test.online_only", r.online_only, &[]));
    }
    if r.writable == Some(false) {
        out.push(t("prefs.intake.test.not_writable"));
    }
    if r.cloud {
        out.push(t("prefs.intake.test.cloud"));
    }
    if r.health != Health::Ok {
        out.push(t(health_key(r.health).0));
    }
    out
}

/// Everything the panel draws from.
pub struct View<'a> {
    /// The runtime state.
    pub panel: &'a mut IntakePanel,
    /// The edits.
    pub edits: &'a mut IntakeEdits,
    /// The engine's list as last read or saved.
    pub committed: &'a IntakeEdits,
    /// This computer's confirmations (the company record).
    pub here: &'a BTreeMap<IntakeFolderId, HereNote>,
    /// A P17 folder is still set (shown while the engine can't keep
    /// intake folders).
    pub legacy: bool,
    /// ClientLedger's own folders (refused as intake folders).
    pub app_dirs: &'a [PathBuf],
}

fn kind_combo(ui: &mut Ui, salt: egui::IdSalt, kind: &mut IntakeKind) {
    let l = ui.add(egui::Label::new(t("prefs.intake.col.kind")).selectable(false));
    egui::ComboBox::from_id_salt(salt)
        .selected_text(t(kind_key(*kind)))
        .show_ui(ui, |ui| {
            for k in [IntakeKind::Bills, IntakeKind::Receipts, IntakeKind::Auto] {
                ui.selectable_value(kind, k, t(kind_key(k)));
            }
        })
        .response
        .labelled_by(l.id)
        .on_hover_text(t("prefs.intake.kind.tip"));
}

/// Draws the Document intake panel.
pub fn show(ui: &mut Ui, tokens: &Tokens, v: View<'_>) -> PanelAction {
    let mut action = PanelAction::None;
    form::heading(ui, tokens, &t("prefs.cat.intake"));
    form::hint(ui, tokens, &t("prefs.intake.intro"));
    ui.add_space(8.0);
    match v.panel.load {
        LoadState::NoCompany => return action,
        LoadState::Loading => {
            form::progress_line(ui, tokens, &t("prefs.intake.loading"));
            return action;
        }
        LoadState::NeedsEngine => {
            form::card(ui, tokens, |ui| {
                form::inline_warning(ui, tokens, &t("prefs.intake.needs_engine"));
                if v.legacy {
                    form::hint(ui, tokens, &t("prefs.intake.legacy"));
                }
                ui.add(egui::Label::new(t("prefs.intake.where")).selectable(false));
            });
            return action;
        }
        LoadState::Failed => {
            form::card(ui, tokens, |ui| {
                form::inline_warning(ui, tokens, &t("prefs.intake.load_failed"));
                if ui.button(t("prefs.intake.retry")).clicked() {
                    action = PanelAction::Reload;
                }
            });
            return action;
        }
        LoadState::Ready => {}
    }
    if v.panel.stale {
        form::card(ui, tokens, |ui| {
            form::inline_warning(ui, tokens, &t("prefs.intake.stale"));
            form::hint(ui, tokens, &t("prefs.intake.reload.hint"));
            if ui.button(t("prefs.intake.reload")).clicked() {
                action = PanelAction::Reload;
            }
        });
        ui.add_space(6.0);
    }
    let editable = v.panel.editable();
    if !v.panel.is_admin() {
        form::hint(ui, tokens, &t("prefs.intake.read_only"));
    }
    form::card(ui, tokens, |ui| {
        if v.edits.folders.is_empty() {
            ui.add(egui::Label::new(t("prefs.intake.empty")).selectable(false));
            if !editable {
                form::hint(ui, tokens, &t("prefs.intake.ask_admin"));
            }
        } else {
            folder_table(ui, tokens, &v, editable);
            folder_rows(
                ui,
                tokens,
                &mut *v.panel,
                &mut *v.edits,
                v.app_dirs,
                editable,
            );
        }
        if editable {
            ui.add_space(8.0);
            add_row(ui, tokens, &mut *v.panel, &mut *v.edits, v.app_dirs);
        }
        ui.add_space(6.0);
        ui.add(egui::Label::new(t("prefs.intake.where")).selectable(false));
    });
    if v.panel.busy() {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(200));
    }
    action
}

/// The Name · Folder · Kind · Status table.
fn folder_table(ui: &mut Ui, tokens: &Tokens, v: &View<'_>, _editable: bool) {
    egui::Grid::new("prefs-intake-table")
        .num_columns(4)
        .striped(true)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            for key in [
                "prefs.intake.col.name",
                "prefs.intake.col.folder",
                "prefs.intake.col.kind",
                "prefs.intake.col.status",
            ] {
                ui.add(
                    egui::Label::new(RichText::new(t(key)).color(tokens.text_secondary).strong())
                        .selectable(false),
                );
            }
            ui.end_row();
            for f in &v.edits.folders {
                ui.add(egui::Label::new(&f.name).selectable(false));
                ui.add(egui::Label::new(&f.path).truncate().selectable(false));
                ui.add(egui::Label::new(t(kind_key(f.default_kind))).selectable(false));
                let status = status_text(v.panel, v.edits, v.committed, v.here, f);
                ui.add(
                    egui::Label::new(RichText::new(status).color(tokens.text_secondary))
                        .selectable(false),
                );
                ui.end_row();
            }
        });
}

/// Each folder's controls under the table.
fn folder_rows(
    ui: &mut Ui,
    tokens: &Tokens,
    panel: &mut IntakePanel,
    edits: &mut IntakeEdits,
    app_dirs: &[PathBuf],
    editable: bool,
) {
    if !editable {
        return;
    }
    let ids: Vec<IntakeFolderId> = edits.folders.iter().map(|f| f.id).collect();
    for id in ids {
        ui.add_space(8.0);
        ui.separator();
        let Some(f) = edits.folders.iter().find(|f| f.id == id).cloned() else {
            continue;
        };
        ui.add(egui::Label::new(RichText::new(&f.name).strong()).selectable(false));
        // Rename inline.
        if let Some((rid, text)) = panel.renaming.as_mut()
            && *rid == id
        {
            let mut typed = std::mem::take(text);
            let mut done = false;
            let mut cancel = false;
            ui.horizontal(|ui| {
                let r = form::field(ui, &t("prefs.intake.rename.label"), &mut typed);
                if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    done = true;
                }
                if ui.button(t("prefs.intake.rename.done")).clicked() {
                    done = true;
                }
                if ui.button(t("prefs.intake.rename.cancel")).clicked() {
                    cancel = true;
                }
            });
            if cancel {
                panel.renaming = None;
            } else if done {
                if let Err(b) = panel.rename(edits, id, &typed) {
                    form::hint(ui, tokens, &t(b.message_key()));
                    panel.renaming = Some((id, typed));
                }
            } else {
                panel.renaming = Some((id, typed));
            }
        }
        // Kind and the switches.
        if let Some(fm) = edits.folders.iter_mut().find(|f| f.id == id) {
            ui.horizontal(|ui| {
                kind_combo(
                    ui,
                    egui::IdSalt::new(("prefs-intake-kind", id)),
                    &mut fm.default_kind,
                );
            });
            ui.checkbox(&mut fm.keep_originals, t("prefs.intake.keep_originals"))
                .on_hover_text(t("prefs.intake.keep_originals.tip"));
            ui.checkbox(&mut fm.auto_read, t("prefs.intake.auto_read"));
            form::hint(ui, tokens, &t("prefs.intake.auto_read.hint"));
        }
        // Test report.
        if let Some(r) = panel.report(&f).cloned() {
            for line in report_lines(&r) {
                ui.add(egui::Label::new(RichText::new(line).color(tokens.text_secondary)).wrap());
            }
        }
        // Many files already there: ask first (Q-A01h).
        if panel.ask_bring_in == Some(id) {
            let n = panel.report(&f).map_or(0, |r| r.files);
            ui.add(egui::Label::new(t_count("prefs.intake.bring_in.ask", n, &[])).wrap());
            ui.horizontal(|ui| {
                if ui
                    .button(t_count("prefs.intake.bring_in.all", n, &[]))
                    .clicked()
                {
                    panel.confirm(edits, id, false);
                }
                if ui.button(t("prefs.intake.bring_in.new")).clicked() {
                    panel.confirm(edits, id, true);
                }
            });
        }
        // Remove confirm.
        if panel.removing == Some(id) {
            form::inline_warning(ui, tokens, &t("prefs.intake.remove.ask"));
            ui.horizontal(|ui| {
                if ui.button(t("prefs.intake.remove.yes")).clicked() {
                    panel.remove(edits, id);
                }
                if ui.button(t("prefs.intake.remove.no")).clicked() {
                    panel.removing = None;
                }
            });
            continue;
        }
        ui.horizontal(|ui| {
            if ui.button(t("prefs.intake.rename")).clicked() {
                panel.renaming = Some((id, f.name.clone()));
            }
            if ui.button(t("prefs.intake.remove")).clicked() {
                panel.removing = Some(id);
            }
            let testing = panel.testing(id);
            if ui
                .add_enabled(!testing, egui::Button::new(t("prefs.intake.test")))
                .on_hover_text(t("prefs.intake.test.tip"))
                .clicked()
            {
                panel.test(edits, id, app_dirs);
            }
            let can_use = !edits.confirm.contains_key(&id)
                && panel.ask_bring_in != Some(id)
                && panel.report(&f).is_some_and(usable);
            if can_use
                && ui
                    .button(t("prefs.intake.use_here"))
                    .on_hover_text(t("prefs.intake.use_here.tip"))
                    .clicked()
            {
                panel.offer_confirm(edits, id);
            }
        });
    }
}

/// The Add folder button and row.
fn add_row(
    ui: &mut Ui,
    tokens: &Tokens,
    panel: &mut IntakePanel,
    edits: &mut IntakeEdits,
    app_dirs: &[PathBuf],
) {
    let Some(mut row) = panel.add.take() else {
        if ui.button(t("prefs.intake.add")).clicked() {
            panel.add = Some(AddRow::default());
        }
        return;
    };
    form::field(ui, &t("prefs.intake.add.name"), &mut row.name);
    form::field_wide(ui, &t("prefs.intake.add.path"), &mut row.path);
    form::hint(ui, tokens, &t("prefs.intake.add.path_hint"));
    let mut kind = row.kind.unwrap_or(IntakeKind::Auto);
    ui.horizontal(|ui| kind_combo(ui, egui::IdSalt::new("prefs-intake-add-kind"), &mut kind));
    row.kind = Some(kind);
    // Inline refusal of the path as typed (syntax only, no I/O).
    let dirs: Vec<&Path> = app_dirs.iter().map(PathBuf::as_path).collect();
    let typed = row.path.trim();
    if !typed.is_empty()
        && let Err(p) = wp_b::check_path_syntax(typed, &dirs)
    {
        form::hint(ui, tokens, &t(path_problem_key(p)));
    } else if let Some(b) = row.problem {
        form::hint(ui, tokens, &t(b.message_key()));
    }
    let mut add = false;
    let mut cancel = false;
    ui.horizontal(|ui| {
        if ui.button(t("prefs.intake.add.confirm")).clicked() {
            add = true;
        }
        if ui.button(t("prefs.intake.add.cancel")).clicked() {
            cancel = true;
        }
    });
    if cancel {
        return;
    }
    panel.add = Some(row);
    if add {
        let _ = panel.add_folder(edits, app_dirs);
    }
}
