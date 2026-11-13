//! `.clreport` report files (A09 WP-A, Q-A09a, plan §3.3): export one
//! memorized report or a whole group, and import a file with a review of
//! its references. The Memorized Reports list and the Report Builder's
//! Report tab call [`start_export`] and [`start_import`]; each opens a
//! "Report File" tab ([`ReportFileWindow`]) that owns the flow, its file
//! dialogs and its engine jobs.
//!
//! - **Export**: the engine (U-220) gives the reports without ids
//!   (`"$ref:N"` tokens and what each named) and audits a `download` row
//!   per report; the file is checked (`cl_model::report_def::clreport`)
//!   and written through a temp file and a rename. Only memorized reports
//!   export (Q-A09l).
//! - **Import**: the file's size is checked before it is read, then its
//!   envelope (a newer version asks the owner to update), then the
//!   engine's preview matches the references. The review shows one card
//!   per report: matched references folded, each unresolved one with its
//!   candidates, the full list, "Leave out" (never for a budget) or Cancel;
//!   a name clash takes Rename or Replace (only for the author or an admin
//!   and the same kind). Every choice asks the preview again.
//! - **After review**: one report opens unsaved (the builder, or its report
//!   window for a standard report) with its name and matched group, and
//!   its Memorize sends `origin: 'import'`. Several reports end with
//!   **Memorize all**: missing groups are created by name, then each report;
//!   the first failure stops and says what wasn't imported. A Replace first
//!   asks, then writes a `.clreport` copy of the report it replaces into the
//!   company's backup folder and reads it back (Q-A09p); only then is it
//!   overwritten (for a single report too, so the copy and the overwrite
//!   can't be separated). Cancel at any point writes nothing.

mod review;
#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::Receiver;

use cl_api::resources::reports::definitions::{
    DefinitionGroup, DefinitionPatch, DefinitionRefusal, DefinitionRow, DefinitionWrite,
    ExportWhat, PreviewReport, TransferReport, definition_refusal,
};
use cl_api::{ApiError, Capabilities, OrphanLabel, RequestGroup};
use cl_model::CommandId;
use cl_model::report_def::clreport::{self, FileError, ReportFile};
use eframe::egui::{self, RichText, Ui};

use crate::company::{CompanyEvent, CompanyLink};
use crate::i18n::{t, t_count, t_with};
use crate::screens::company::form;
use crate::screens::invoice::engine::{self, Job};
use crate::theme::Tokens;
use crate::widgets::file_picker::{self, Filter, PickUse};
use crate::widgets::primary_button;
use crate::windows::{OpenPayload, Screen};

pub(crate) use review::{Clash, Planned, Review};

/// The window's kind.
pub const KIND: &str = "reports.report_file";

/// The file dialog's filter.
const FILTER: Filter = Filter {
    name_key: "picker.filter.clreport",
    extensions: &["clreport"],
};

/// The folder under the backup folder that keeps replaced reports.
const BACKUP_SUBFOLDER: &str = "Replaced reports";

/// What to export.
#[derive(Clone, PartialEq)]
pub enum Target {
    /// One memorized report (the list's selection).
    Report(Box<DefinitionRow>),
    /// A group and its reports, in list order.
    Group {
        /// The group.
        group: DefinitionGroup,
        /// Its reports.
        reports: Vec<DefinitionRow>,
    },
    /// The memorized report an editor shows (the Report tab).
    Saved {
        /// Its id.
        id: String,
        /// Its name (the file name).
        name: String,
    },
}

impl std::fmt::Debug for Target {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Target::Report(_) => f.write_str("Report"),
            Target::Group { reports, .. } => f
                .debug_struct("Group")
                .field("reports", &reports.len())
                .finish_non_exhaustive(),
            Target::Saved { .. } => f.write_str("Saved"),
        }
    }
}

impl Target {
    /// The export request and the default file name (without extension).
    fn request(&self) -> (ExportWhat, String) {
        match self {
            Target::Report(r) => (ExportWhat::Ids(vec![r.id.clone()]), r.name.clone()),
            Target::Group { group, .. } => {
                (ExportWhat::Group(group.id.clone()), group.name.clone())
            }
            Target::Saved { id, name } => (ExportWhat::Ids(vec![id.clone()]), name.clone()),
        }
    }

    fn empty(&self) -> bool {
        matches!(self, Target::Group { reports, .. } if reports.is_empty())
    }
}

/// What a Report File window opens on.
#[derive(Clone, Debug, PartialEq)]
pub enum Start {
    /// Export to a file.
    Export(Target),
    /// Import a file.
    Import,
}

// A definition compares as JSON values do (exactly).
impl Eq for Start {}

/// A standard report read from a file, opened unsaved in a report window
/// (`reports.center`). `Debug` names nothing.
#[derive(Clone, PartialEq)]
pub struct ImportedOpen {
    /// The standard definition (this company's ids).
    pub definition: serde_json::Value,
    /// Its name, for the Memorize dialog.
    pub name: String,
    /// The matched group, for the Memorize dialog.
    pub group_id: Option<String>,
}

impl Eq for ImportedOpen {}

impl std::fmt::Debug for ImportedOpen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImportedOpen").finish_non_exhaustive()
    }
}

/// True when this engine (or the demo) can carry report files.
pub(crate) fn available(link: &CompanyLink) -> bool {
    link.is_demo()
        || (link.capabilities().has(Capabilities::REPORT_DEFS)
            && link.capabilities().has(Capabilities::REPORT_DEFS_TRANSFER))
}

/// Starts exporting `target` to a `.clreport` file. Returns the i18n key
/// of a message to show instead, if any.
pub(crate) fn start_export(link: &CompanyLink, target: Target) -> Option<&'static str> {
    if !available(link) {
        return Some("report_file.unavailable");
    }
    if target.empty() {
        return Some("report_file.export.empty_group");
    }
    link.push(CompanyEvent::OpenWith(
        CommandId::new(KIND),
        Box::new(OpenPayload::ReportFile(Box::new(Start::Export(target)))),
    ));
    None
}

/// Starts importing a `.clreport` file. Returns the i18n key of a message
/// to show instead, if any.
pub(crate) fn start_import(link: &CompanyLink) -> Option<&'static str> {
    if !available(link) {
        return Some("report_file.unavailable");
    }
    link.push(CompanyEvent::OpenWith(
        CommandId::new(KIND),
        Box::new(OpenPayload::ReportFile(Box::new(Start::Import))),
    ));
    None
}

/// The window for this kind.
#[must_use]
pub fn make(link: &Arc<CompanyLink>) -> Box<dyn Screen> {
    Box::new(ReportFileWindow::new(Arc::clone(link)))
}

/// A standard report from a file in a report window: shown unsaved, its
/// Memorize dialog open with the file's name and the matched group, and
/// its Memorize sent with `origin: 'import'`.
pub(crate) fn open_imported(w: &mut super::ReportWindow, open: ImportedOpen) -> bool {
    let Some((spec, options)) = standard_of(&open.definition) else {
        return false;
    };
    w.show_report(spec, options);
    w.memo.current = None;
    w.memo.saved = None;
    w.memo.origin = Some("import".into());
    let mut d = super::memorized::Dialog::default();
    d.name = open.name;
    d.group = open.group_id;
    w.memo.dialog = Some(d);
    true
}

/// The catalog report and options of a standard definition, when this
/// build can show it.
fn standard_of(
    definition: &serde_json::Value,
) -> Option<(
    &'static cl_model::reports::ReportSpec,
    cl_model::reports::ReportOptions,
)> {
    let Ok(cl_model::reports::definition::Memorized::Standard(def)) =
        cl_model::reports::definition::read_memorized(definition)
    else {
        return None;
    };
    let spec = cl_model::reports::find(&def.report_key)?;
    Some((spec, def.options))
}

/// The folder replaced reports are copied to: the chosen backup folder, or
/// the default one.
fn backup_folder(link: &CompanyLink) -> PathBuf {
    link.store()
        .load()
        .ok()
        .and_then(|l| l.backup_folder)
        .unwrap_or_else(|| link.backup_dir.clone())
        .join(BACKUP_SUBFOLDER)
}

/// The file text for `reports` (checked: no ids, no figures, caps).
fn file_bytes(reports: &[TransferReport]) -> Result<Vec<u8>, FileError> {
    let file = ReportFile::new(
        &format!("ClientLedger {}", env!("CARGO_PKG_VERSION")),
        &chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
        reports.iter().map(review::file_report_of).collect(),
    );
    clreport::write(&file)
}

/// Writes a copy of `report` (as exported) into `folder` under a name no
/// other file has, reads it back and compares (Q-A09p). The path written.
fn write_verified_copy(folder: &Path, report: &TransferReport) -> Result<PathBuf, &'static str> {
    let bytes = file_bytes(std::slice::from_ref(report)).map_err(|_| "build")?;
    std::fs::create_dir_all(folder).map_err(|_| "folder")?;
    let stamp = chrono::Local::now().format("%Y-%m-%d %H%M%S").to_string();
    let base = cl_print::files::safe_component(&report.name);
    let path = (1..1000)
        .map(|n| {
            let suffix = if n == 1 {
                String::new()
            } else {
                format!(" ({n})")
            };
            folder.join(format!("{base} {stamp}{suffix}.{}", clreport::EXTENSION))
        })
        .find(|p| !p.exists())
        .ok_or("name")?;
    cl_print::report_export::write_atomic(&path, &bytes).map_err(|_| "write")?;
    let back = std::fs::read(&path).map_err(|_| "read")?;
    let (Ok(read), Ok(sent)) = (clreport::read(&back), clreport::read(&bytes)) else {
        return Err("verify");
    };
    if back != bytes || read != sent {
        return Err("verify");
    }
    Ok(path)
}

/// Reads and checks a picked file: a regular file (never a pipe or a
/// device, which could block), its size before reading it, at most one
/// byte over the cap read (a file growing meanwhile, S-L3), then the
/// envelope.
fn read_file(path: &Path) -> Result<ReportFile, FileError> {
    use std::io::Read as _;
    let meta = std::fs::metadata(path).map_err(|_| FileError::NotReportFile)?;
    if !meta.is_file() {
        return Err(FileError::NotReportFile);
    }
    if usize::try_from(meta.len()).map_or(true, |n| n > clreport::MAX_FILE_BYTES) {
        return Err(FileError::TooLarge);
    }
    let f = std::fs::File::open(path).map_err(|_| FileError::NotReportFile)?;
    let mut bytes = Vec::new();
    f.take(clreport::MAX_FILE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| FileError::NotReportFile)?;
    if bytes.len() > clreport::MAX_FILE_BYTES {
        return Err(FileError::TooLarge);
    }
    clreport::read(&bytes)
}

/// The owner's words for a file problem.
const fn file_error_key(e: FileError) -> &'static str {
    match e {
        FileError::NotReportFile => "report_file.error.not_report_file",
        FileError::TooLarge => "report_file.error.too_large",
        FileError::Newer => "report_file.error.newer",
        FileError::Empty => "report_file.error.empty",
        FileError::CompanyData => "report_file.error.company_data",
    }
}

/// The owner's words for an engine refusal of a preview or a save.
fn refusal_key(e: &ApiError) -> &'static str {
    match definition_refusal(e) {
        Some(DefinitionRefusal::Invalid { code: Some(c), .. }) if c == "SCHEMA_NEWER" => {
            "report_file.error.newer"
        }
        Some(DefinitionRefusal::Invalid { .. }) => "report_file.error.invalid",
        Some(DefinitionRefusal::NameTaken) => "report_file.save.name_taken",
        Some(DefinitionRefusal::Stale) => "report_file.save.stale",
        Some(DefinitionRefusal::NotAuthor) => "report_file.save.not_author",
        Some(DefinitionRefusal::TooLarge) => "report_file.error.too_large",
        None if matches!(e, ApiError::NotFound { .. }) => "report_file.error.gone",
        None => "report_file.error.engine",
    }
}

/// The owner's words for a left-out reference's effect.
fn effect_key(effect: &str) -> &'static str {
    match effect {
        "filter_removed" => "report_file.effect.filter_removed",
        "group_by_removed" => "report_file.effect.group_by_removed",
        "tag_split_removed" => "report_file.effect.tag_split_removed",
        "columns_total" => "report_file.effect.columns_total",
        _ => "report_file.effect.removed_from_list",
    }
}

/// The kind's i18n key.
const fn ref_kind_key(k: cl_model::report_def::clreport::RefKind) -> &'static str {
    use cl_model::report_def::clreport::RefKind;
    match k {
        RefKind::Account => "report_file.ref.account",
        RefKind::Contact => "report_file.ref.contact",
        RefKind::Item => "report_file.ref.item",
        RefKind::Tag => "report_file.ref.tag",
        RefKind::TagGroup => "report_file.ref.tag_group",
        RefKind::Budget => "report_file.ref.budget",
    }
}

/// The engine's list of every row of a reference kind (the full-list
/// picker): `(id, label)`.
type PickList = Vec<(String, String)>;

/// An export under way.
#[derive(Default)]
struct Exporting {
    /// What to export, until the path is chosen and the job starts.
    what: Option<ExportWhat>,
    job: Option<Job<Vec<TransferReport>>>,
    /// The file, once the engine answered and it was checked.
    built: Option<Result<Vec<u8>, &'static str>>,
    dialog: Option<Receiver<Option<PathBuf>>>,
    /// The Save dialog's answer.
    path: Option<PathBuf>,
    /// The Save dialog was cancelled.
    cancelled: bool,
    /// The default file name.
    name: String,
    asked: bool,
}

/// A choice to take back: its key and what it held before (None: no
/// choice; Some(None): left out).
#[allow(clippy::option_option, reason = "the choices map holds Option<String>")]
type Undo = (String, Option<Option<String>>);

/// The review and its jobs.
#[derive(Default)]
struct Reviewing {
    review: Review,
    preview: Option<Job<Vec<PreviewReport>>>,
    /// A choice was made while a preview was under way: ask again.
    again: bool,
    /// The Replace confirm is open.
    confirm: bool,
    lists: BTreeMap<&'static str, PickList>,
    list_jobs: BTreeMap<&'static str, Job<PickList>>,
    /// The last preview after a choice failed (an i18n key): the review
    /// stays, the choice is taken back, Try again asks once more (#6).
    error: Option<&'static str>,
    /// The last choice (its key and the value before it), taken back when
    /// the preview it asked for fails.
    undo: Option<Undo>,
}

/// One step of saving the reviewed reports.
enum Step {
    /// The replaced reports exported for their copies (and how many were
    /// asked for: each must come back, S-L2).
    Backup(Job<Vec<TransferReport>>, usize),
    /// A group being created (its name).
    Group(String, Job<DefinitionGroup>),
    /// A group whose name was taken meanwhile: the groups read again.
    Groups(String, Job<Vec<DefinitionGroup>>),
    /// A report being saved (its plan index).
    Report(usize, Job<DefinitionRow>),
}

/// Saving the reviewed reports.
struct Saving {
    plan: Vec<Planned>,
    /// Groups created or found, by name (lowercase).
    groups: BTreeMap<String, String>,
    /// Backup copies written (Q-A09p).
    backed_up: bool,
    /// The next report to save.
    next: usize,
    step: Option<Step>,
    saved: Vec<DefinitionRow>,
    /// Open the result after: the review had one report.
    single: bool,
    /// The review, to go back to when a name was taken meanwhile (#7).
    review: Review,
}

/// Where the window is.
enum Stage {
    /// Not started (no payload yet).
    Idle,
    Export(Exporting),
    /// The Open dialog is up.
    Pick(Option<Receiver<Vec<PathBuf>>>),
    /// The file is being read (off the UI thread).
    Reading(Receiver<Result<ReportFile, FileError>>),
    /// The memorized names are read, then the first preview.
    Starting(Box<Reviewing>, Option<Job<Vec<String>>>),
    Review(Box<Reviewing>),
    Saving(Box<Saving>),
    /// Finished: what was saved, and what wasn't (the message says why).
    Done {
        saved: Vec<DefinitionRow>,
        not_imported: Vec<String>,
    },
    /// A problem; "Choose another file" for an import.
    Failed {
        import: bool,
    },
}

/// The Report File window.
pub struct ReportFileWindow {
    link: Arc<CompanyLink>,
    group: Option<RequestGroup>,
    stage: Stage,
    /// What the window says (a result or a problem).
    message: Option<String>,
    close: bool,
    /// For the control channel: the last outcome.
    last: Option<&'static str>,
}

impl std::fmt::Debug for ReportFileWindow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReportFileWindow")
            .field("stage", &self.stage_name())
            .finish_non_exhaustive()
    }
}

fn label() -> OrphanLabel {
    OrphanLabel {
        what: "orphan.report_memorize",
        key: None,
    }
}

impl ReportFileWindow {
    /// A window not started yet.
    #[must_use]
    pub fn new(link: Arc<CompanyLink>) -> ReportFileWindow {
        let group = link.group().and_then(Result::ok);
        ReportFileWindow {
            link,
            group,
            stage: Stage::Idle,
            message: None,
            close: false,
            last: None,
        }
    }

    fn stage_name(&self) -> &'static str {
        match &self.stage {
            Stage::Idle => "idle",
            Stage::Export(_) => "exporting",
            Stage::Pick(_) => "picking",
            Stage::Reading(_) | Stage::Starting(..) => "reading",
            Stage::Review(_) => "review",
            Stage::Saving(_) => "saving",
            Stage::Done { .. } => "done",
            Stage::Failed { .. } => "failed",
        }
    }

    fn fail(&mut self, key: &'static str, import: bool) {
        self.message = Some(t(key));
        self.last = Some(key);
        self.stage = Stage::Failed { import };
    }

    // ------------------------------------------------------------ export

    /// An export asks for the path first; the engine's export (which
    /// audits a `download` per report) starts only once there is one, so a
    /// cancelled Save dialog leaves no audit row (S-L6).
    fn begin_export(&mut self, target: &Target) {
        let (what, name) = target.request();
        self.stage = Stage::Export(Exporting {
            what: Some(what),
            name: format!(
                "{}.{}",
                cl_print::files::safe_component(&name),
                clreport::EXTENSION
            ),
            ..Exporting::default()
        });
    }

    /// The Save dialog's answer (tests answer it here).
    #[cfg(test)]
    pub(crate) fn answer_save(&mut self, path: Option<PathBuf>) {
        if let Stage::Export(x) = &mut self.stage {
            x.asked = true;
            x.dialog = None;
            x.cancelled = path.is_none();
            x.path = path;
        }
    }

    fn poll_export(&mut self) {
        let Stage::Export(x) = &mut self.stage else {
            return;
        };
        if let Some(chosen) = file_picker::poll(&mut x.dialog) {
            x.cancelled = chosen.is_none();
            x.path = chosen;
        }
        if x.cancelled {
            // Cancelled: nothing exported, nothing written.
            self.close = true;
            return;
        }
        if x.path.is_some()
            && let Some(what) = x.what.take()
        {
            let w2 = what.clone();
            x.job = Some(engine::read(
                &self.link,
                self.group.as_ref(),
                move |d| crate::company::demo::report_transfer::export(d, &w2),
                move |s| async move { s.report_definitions().export(&what).await },
            ));
        }
        if let Some(r) = engine::poll(&mut x.job) {
            x.built = Some(match r {
                Ok(reports) => file_bytes(&reports).map_err(file_error_key),
                Err(e) => Err(refusal_key(&e)),
            });
        }
        let (Some(built), Some(path)) = (x.built.clone(), x.path.clone()) else {
            return;
        };
        let file = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        match built {
            Err(key) => self.fail(key, false),
            Ok(bytes) => {
                if cl_print::report_export::write_atomic(&path, &bytes).is_ok() {
                    self.message = Some(t_with("report_file.export.saved", &[("file", &file)]));
                    self.last = Some("exported");
                    self.stage = Stage::Done {
                        saved: Vec::new(),
                        not_imported: Vec::new(),
                    };
                } else {
                    self.fail("report_file.export.write_failed", false);
                }
            }
        }
    }

    // ------------------------------------------------------------ import

    /// Reads `path` off the UI thread.
    pub(crate) fn read_path(&mut self, path: PathBuf) {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(read_file(&path));
        });
        self.stage = Stage::Reading(rx);
    }

    fn poll_import(&mut self) {
        match &mut self.stage {
            Stage::Pick(rx) => {
                if let Some(paths) = file_picker::poll(rx) {
                    match paths.into_iter().next() {
                        Some(p) => self.read_path(p),
                        // Cancelled: nothing read, nothing written.
                        None => self.close = true,
                    }
                }
            }
            Stage::Reading(rx) => {
                let r = match rx.try_recv() {
                    Ok(r) => r,
                    Err(std::sync::mpsc::TryRecvError::Empty) => return,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        Err(FileError::NotReportFile)
                    }
                };
                match r {
                    Ok(file) => self.begin_review(&file),
                    Err(e) => self.fail(file_error_key(e), true),
                }
            }
            Stage::Starting(rv, names) => {
                let Some(r) = engine::poll(names) else {
                    return;
                };
                let mut rv = std::mem::take(rv);
                rv.review.names = r.unwrap_or_default();
                self.send_preview(&mut rv);
                self.stage = Stage::Review(rv);
            }
            Stage::Review(_) => self.poll_review(),
            Stage::Saving(_) => self.poll_saving(),
            _ => {}
        }
    }

    fn begin_review(&mut self, file: &ReportFile) {
        self.begin_review_of(Review::new(&file.reports, Vec::new()));
    }

    /// Reads this company's report names, then previews `review`.
    fn begin_review_of(&mut self, review: Review) {
        let rv = Box::new(Reviewing {
            review,
            ..Reviewing::default()
        });
        let names = engine::read(
            &self.link,
            self.group.as_ref(),
            |d| {
                Ok(crate::company::demo::report_store::definitions(d)
                    .0
                    .into_iter()
                    .map(|r| r.name)
                    .collect())
            },
            |s| async move {
                Ok(s.report_definitions()
                    .list()
                    .await?
                    .into_iter()
                    .map(|r| r.name)
                    .collect())
            },
        );
        self.stage = Stage::Starting(rv, Some(names));
    }

    fn send_preview(&self, rv: &mut Reviewing) {
        if rv.preview.is_some() {
            rv.again = true;
            return;
        }
        let (file, choices) = (rv.review.file.clone(), rv.review.choices.clone());
        let (f2, c2) = (file.clone(), choices.clone());
        rv.preview = Some(engine::read(
            &self.link,
            self.group.as_ref(),
            move |d| crate::company::demo::report_transfer::preview(d, &f2, &c2),
            move |s| async move { s.report_definitions().import_preview(&file, &choices).await },
        ));
    }

    fn poll_review(&mut self) {
        let Stage::Review(rv) = &mut self.stage else {
            return;
        };
        let mut lists = Vec::new();
        for (k, job) in &mut rv.list_jobs {
            if let Some(r) = job.take() {
                lists.push((*k, r.unwrap_or_default()));
            }
        }
        for (k, l) in lists {
            rv.list_jobs.remove(k);
            rv.lists.insert(k, l);
        }
        let Some(r) = engine::poll(&mut rv.preview) else {
            return;
        };
        match r {
            Ok(reports) => {
                rv.review.take(reports);
                rv.error = None;
                rv.undo = None;
                if std::mem::take(&mut rv.again) {
                    let mut rv = std::mem::take(rv);
                    self.send_preview(&mut rv);
                    self.stage = Stage::Review(rv);
                }
            }
            // The first preview: nothing to go back to.
            Err(e) if rv.review.reports.is_empty() => {
                let key = refusal_key(&e);
                self.fail(key, true);
            }
            // A later one: the review and the other choices stay; the
            // last choice is taken back and the card says why (#6).
            Err(e) => {
                rv.again = false;
                if let Some((k, before)) = rv.undo.take() {
                    match before {
                        Some(v) => rv.review.choices.insert(k, v),
                        None => rv.review.choices.remove(&k),
                    };
                }
                rv.error = Some(refusal_key(&e));
            }
        }
    }

    /// Try again after a failed preview.
    pub(crate) fn retry(&mut self) {
        let Stage::Review(rv) = &mut self.stage else {
            return;
        };
        rv.error = None;
        let mut rv = std::mem::take(rv);
        self.send_preview(&mut rv);
        self.stage = Stage::Review(rv);
    }

    /// A choice in the review: the preview is asked again.
    pub(crate) fn choose(&mut self, i: usize, token: &str, id: Option<String>) {
        let Stage::Review(rv) = &mut self.stage else {
            return;
        };
        let key = cl_api::resources::reports::definitions::choice_key(i, token);
        rv.undo = Some((key.clone(), rv.review.choices.get(&key).cloned()));
        rv.error = None;
        rv.review.choose(i, token, id);
        let mut rv = std::mem::take(rv);
        self.send_preview(&mut rv);
        self.stage = Stage::Review(rv);
    }

    /// Start over on report `i`: its choices go, the preview is asked again.
    pub(crate) fn start_over(&mut self, i: usize) {
        let Stage::Review(rv) = &mut self.stage else {
            return;
        };
        rv.review.start_over(i);
        rv.undo = None;
        rv.error = None;
        let mut rv = std::mem::take(rv);
        self.send_preview(&mut rv);
        self.stage = Stage::Review(rv);
    }

    /// The review (tests and the control channel).
    pub(crate) fn review(&self) -> Option<&Review> {
        match &self.stage {
            Stage::Review(rv) => Some(&rv.review),
            _ => None,
        }
    }

    /// A clash answer (tests; the card sets it on the review).
    #[cfg(test)]
    pub(crate) fn set_clash(&mut self, i: usize, c: Clash) {
        if let Stage::Review(rv) = &mut self.stage {
            rv.review.set_clash(i, c);
        }
    }

    /// Continue (one report) or Memorize all (several). A Replace asks
    /// first; `confirmed` is the confirm's yes.
    pub(crate) fn finish(&mut self, confirmed: bool) {
        let Stage::Review(rv) = &mut self.stage else {
            return;
        };
        if rv.preview.is_some() {
            return;
        }
        let Some(plan) = rv.review.plan() else {
            return;
        };
        if rv.review.replaces() && !confirmed {
            rv.confirm = true;
            return;
        }
        rv.confirm = false;
        let single = rv.review.single();
        if single && plan[0].replace.is_none() {
            self.open_unsaved(&plan[0]);
            return;
        }
        let mut s = Saving {
            plan,
            groups: BTreeMap::new(),
            backed_up: false,
            next: 0,
            step: None,
            saved: Vec::new(),
            single,
            review: rv.review.clone(),
        };
        self.next_step(&mut s);
        self.stage = Stage::Saving(Box::new(s));
    }

    /// Cancel: nothing is written.
    pub(crate) fn cancel(&mut self) {
        if let Stage::Review(rv) = &mut self.stage
            && rv.confirm
        {
            rv.confirm = false;
            return;
        }
        if !matches!(self.stage, Stage::Saving(_)) {
            self.close = true;
        }
    }

    /// One report opens unsaved: the builder, or a report window.
    fn open_unsaved(&mut self, p: &Planned) {
        if p.kind == "builder" {
            let text = p.definition.to_string();
            let Ok(def) = cl_model::report_def::parse(&text) else {
                self.fail("report_file.error.invalid", true);
                return;
            };
            super::builder::open(
                &self.link,
                super::builder::BuilderSeed::Imported {
                    def: Box::new(def),
                    name: p.name.clone(),
                    group_id: p.group_id.clone(),
                },
            );
        } else {
            // Checked here, while the window can still say so: the report
            // window can't open it once this one has closed (#8).
            if standard_of(&p.definition).is_none() {
                self.fail("report_file.error.invalid", true);
                return;
            }
            self.link.push(CompanyEvent::OpenWith(
                CommandId::new(super::KIND),
                Box::new(OpenPayload::ImportedReport(Box::new(ImportedOpen {
                    definition: p.definition.clone(),
                    name: p.name.clone(),
                    group_id: p.group_id.clone(),
                }))),
            ));
        }
        self.last = Some("opened");
        self.close = true;
    }

    /// Starts the next step of `s`, or ends.
    fn next_step(&mut self, s: &mut Saving) {
        // 1. The copies of the reports a Replace overwrites.
        if !s.backed_up {
            let ids: Vec<String> = s
                .plan
                .iter()
                .filter_map(|p| p.replace.as_ref().map(|r| r.0.clone()))
                .collect();
            if !ids.is_empty() {
                let n = ids.len();
                let what = ExportWhat::Ids(ids);
                let w2 = what.clone();
                s.step = Some(Step::Backup(
                    engine::read(
                        &self.link,
                        self.group.as_ref(),
                        move |d| crate::company::demo::report_transfer::export(d, &w2),
                        move |s| async move { s.report_definitions().export(&what).await },
                    ),
                    n,
                ));
                return;
            }
            s.backed_up = true;
        }
        // 2. Groups the file names that this company lacks (several
        // reports only: one report opens with the matched group).
        if !s.single
            && let Some(name) = s
                .plan
                .iter()
                .filter_map(|p| p.new_group.clone())
                .find(|g| !s.groups.contains_key(&g.to_lowercase()))
        {
            let n2 = name.clone();
            let n3 = name.clone();
            s.step = Some(Step::Group(
                name,
                engine::write(
                    &self.link,
                    self.group.as_ref(),
                    label(),
                    move |d| crate::company::demo::report_store::create_group(d, &n2),
                    move |s| async move { s.report_definitions().create_group(&n3).await },
                ),
            ));
            return;
        }
        // 3. The reports, one at a time.
        let Some(p) = s.plan.get(s.next).cloned() else {
            s.step = None;
            return;
        };
        let group_id = p.group_id.clone().or_else(|| {
            p.new_group
                .as_ref()
                .filter(|_| !s.single)
                .and_then(|g| s.groups.get(&g.to_lowercase()).cloned())
        });
        let job = if let Some((id, version)) = p.replace.clone() {
            // A Replace sends the body and the origin only: the report keeps
            // its own name and group (#1, #15). A builder body's title is the
            // report's name (Q-A08r), so it carries the kept name.
            let mut definition = p.definition.clone();
            if p.kind == "builder"
                && let Some(m) = definition.as_object_mut()
                && m.contains_key("title")
            {
                m.insert("title".into(), serde_json::Value::String(p.name.clone()));
            }
            let patch = DefinitionPatch {
                definition: Some(definition),
                origin: Some("import".into()),
                ..DefinitionPatch::default()
            };
            let (p2, id2, kind) = (patch.clone(), id.clone(), p.kind.clone());
            engine::write(
                &self.link,
                self.group.as_ref(),
                label(),
                move |d| {
                    crate::company::demo::report_store::patch_definition(d, &id2, version, &p2)
                },
                move |s| async move {
                    s.report_definitions()
                        .patch(&id, version, &kind, &patch)
                        .await
                },
            )
        } else {
            let w = DefinitionWrite {
                name: p.name.clone(),
                group_id,
                kind: p.kind.clone(),
                definition: p.definition.clone(),
                origin: Some("import".into()),
            };
            let w2 = w.clone();
            // One key per report of this Memorize all: a retried send stores
            // one.
            let key = cl_api::IdempotencyKey::new();
            engine::write(
                &self.link,
                self.group.as_ref(),
                label(),
                move |d| crate::company::demo::report_store::save_definition(d, None, &w2),
                move |s| async move { s.report_definitions().create(&w, key).await },
            )
        };
        s.step = Some(Step::Report(s.next, job));
    }

    fn poll_saving(&mut self) {
        let Stage::Saving(s) = &mut self.stage else {
            return;
        };
        let mut s = std::mem::replace(
            s,
            Box::new(Saving {
                plan: Vec::new(),
                groups: BTreeMap::new(),
                backed_up: true,
                next: 0,
                step: None,
                saved: Vec::new(),
                single: false,
                review: Review::default(),
            }),
        );
        let outcome = self.advance(&mut s);
        match outcome {
            Advance::Waiting => self.stage = Stage::Saving(s),
            Advance::Next => {
                self.next_step(&mut s);
                if s.step.is_none() {
                    self.finished(*s, None);
                } else {
                    self.stage = Stage::Saving(s);
                }
            }
            Advance::Stop(key) => {
                let name = s
                    .plan
                    .get(s.next)
                    .map(|p| p.name.clone())
                    .unwrap_or_default();
                if key == NAME_TAKEN && !s.single {
                    self.review_again(*s, &name);
                } else {
                    self.finished(*s, Some(&(name, key)));
                }
            }
        }
    }

    /// Takes the answer of the step under way.
    fn advance(&mut self, s: &mut Saving) -> Advance {
        let Some(step) = s.step.as_mut() else {
            return Advance::Next;
        };
        match step {
            Step::Backup(job, asked) => {
                let asked = *asked;
                let Some(r) = job.take() else {
                    return Advance::Waiting;
                };
                s.step = None;
                // Every replaced report must have its copy: a short answer
                // replaces nothing (non-negotiable 9, S-L2).
                let Ok(reports) = r
                    .map_err(|_| ())
                    .and_then(|v| if v.len() == asked { Ok(v) } else { Err(()) })
                else {
                    return Advance::Stop("report_file.backup.failed");
                };
                let folder = backup_folder(&self.link);
                for rep in &reports {
                    if let Err(why) = write_verified_copy(&folder, rep) {
                        tracing::warn!(why, "a replaced report's copy could not be written");
                        return Advance::Stop("report_file.backup.failed");
                    }
                }
                s.backed_up = true;
                Advance::Next
            }
            Step::Group(name, job) => {
                let Some(r) = job.take() else {
                    return Advance::Waiting;
                };
                let name = name.clone();
                match r {
                    Ok(g) => {
                        s.groups.insert(name.to_lowercase(), g.id);
                        s.step = None;
                        Advance::Next
                    }
                    // Made meanwhile (another window): read the groups.
                    Err(e) if definition_refusal(&e) == Some(DefinitionRefusal::NameTaken) => {
                        s.step = Some(Step::Groups(
                            name,
                            engine::read(
                                &self.link,
                                self.group.as_ref(),
                                |d| Ok(crate::company::demo::report_store::definitions(d).1),
                                |s| async move { s.report_definitions().groups().await },
                            ),
                        ));
                        Advance::Waiting
                    }
                    Err(_) => {
                        s.step = None;
                        Advance::Stop("report_file.save.group_failed")
                    }
                }
            }
            Step::Groups(name, job) => {
                let Some(r) = job.take() else {
                    return Advance::Waiting;
                };
                let name = name.clone();
                s.step = None;
                match r.ok().and_then(|gs| {
                    gs.into_iter()
                        .find(|g| g.name.trim().eq_ignore_ascii_case(name.trim()))
                }) {
                    Some(g) => {
                        s.groups.insert(name.to_lowercase(), g.id);
                        Advance::Next
                    }
                    None => Advance::Stop("report_file.save.group_failed"),
                }
            }
            Step::Report(i, job) => {
                let Some(r) = job.take() else {
                    return Advance::Waiting;
                };
                let i = *i;
                s.step = None;
                match r {
                    Ok(row) => {
                        s.saved.push(row);
                        s.next = i + 1;
                        self.link.bus().publish(crate::data_bus::Change::Reports);
                        Advance::Next
                    }
                    Err(e) => Advance::Stop(refusal_key(&e)),
                }
            }
        }
    }

    /// A name taken meanwhile (another window, another user) stopped
    /// Memorize all: what was saved leaves the review, the rest comes back
    /// with a fresh preview and its clash choices (plan §5, #7).
    fn review_again(&mut self, s: Saving, name: &str) {
        let saved: Vec<usize> = s.plan.iter().take(s.saved.len()).map(|p| p.index).collect();
        if !s.saved.is_empty() {
            self.link.bus().publish(crate::data_bus::Change::Reports);
        }
        let mut review = s.review;
        review.remove(&saved);
        let mut text = t_with("report_file.save.again", &[("name", name)]);
        if !s.saved.is_empty() {
            text.push(' ');
            text.push_str(&t_count("report_file.save.done", s.saved.len(), &[]));
        }
        self.message = Some(text);
        self.last = Some("review_again");
        self.begin_review_of(review);
    }

    /// Ends the save: one report opens; several are listed.
    fn finished(&mut self, s: Saving, stopped: Option<&(String, &'static str)>) {
        if s.single
            && stopped.is_none()
            && let Some(row) = s.saved.first()
        {
            self.link.push(CompanyEvent::OpenWith(
                CommandId::new(super::KIND),
                Box::new(OpenPayload::Memorized(Box::new(
                    super::memorized_list::MemorizedOpen(row.clone()),
                ))),
            ));
            self.last = Some("replaced");
            self.close = true;
            self.stage = Stage::Done {
                saved: s.saved,
                not_imported: Vec::new(),
            };
            return;
        }
        let not_imported: Vec<String> = s
            .plan
            .iter()
            .skip(s.saved.len())
            .map(|p| p.name.clone())
            .collect();
        self.last = Some(if stopped.is_some() {
            "stopped"
        } else {
            "imported"
        });
        self.message = Some(match stopped {
            // Separate words when nothing was saved (#7).
            Some((name, key)) if s.saved.is_empty() => t_with(
                "report_file.save.stopped_none",
                &[("name", name), ("why", &t(key))],
            ),
            Some((name, key)) => t_with(
                "report_file.save.stopped",
                &[("name", name), ("why", &t(key))],
            ),
            None => t_count("report_file.save.done", s.saved.len(), &[]),
        });
        self.stage = Stage::Done {
            saved: s.saved,
            not_imported,
        };
    }

    // ------------------------------------------------------------ drawing

    fn draw(&mut self, ui: &mut Ui) {
        let tokens = Tokens::of(ui.ctx());
        let ctx = ui.ctx().clone();
        // The file dialogs need the context: they open on the first frame.
        match &mut self.stage {
            Stage::Export(x) if !x.asked => {
                x.asked = true;
                x.dialog = Some(file_picker::save_file_as(
                    &ctx,
                    PickUse::SaveReport,
                    None,
                    &x.name,
                    ("picker.filter.clreport", clreport::EXTENSION),
                ));
            }
            Stage::Pick(rx @ None) => {
                *rx = Some(file_picker::pick_files(
                    &ctx,
                    PickUse::SaveReport,
                    &[FILTER],
                    false,
                ));
            }
            _ => {}
        }
        ui.heading(t("report_file.title"));
        ui.add_space(6.0);
        if let Some(m) = &self.message {
            ui.add(egui::Label::new(m.as_str()).wrap());
            ui.add_space(6.0);
        }
        match self.stage_name() {
            "exporting" => {
                ui.label(
                    RichText::new(t("report_file.export.working")).color(tokens.text_secondary),
                );
            }
            "picking" | "reading" => {
                ui.label(
                    RichText::new(t("report_file.import.reading")).color(tokens.text_secondary),
                );
            }
            "review" => self.draw_review(ui, &tokens),
            "saving" => {
                ui.label(RichText::new(t("report_file.save.working")).color(tokens.text_secondary));
            }
            "done" => self.draw_done(ui, &tokens),
            "failed" => {
                ui.horizontal(|ui| {
                    if matches!(self.stage, Stage::Failed { import: true })
                        && ui.button(t("report_file.import.another")).clicked()
                    {
                        self.message = None;
                        self.stage = Stage::Pick(None);
                    }
                    if ui.button(t("report_file.close")).clicked() {
                        self.close = true;
                    }
                });
            }
            _ => {}
        }
    }

    fn draw_done(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let Stage::Done {
            saved,
            not_imported,
            ..
        } = &self.stage
        else {
            return;
        };
        let mut open = None;
        for row in saved {
            ui.horizontal(|ui| {
                ui.label(row.name.as_str());
                if ui
                    .button(t("report_file.done.open"))
                    .on_hover_text(t("report_file.done.open.tip"))
                    .clicked()
                {
                    open = Some(row.clone());
                }
            });
        }
        if !not_imported.is_empty() {
            ui.add_space(6.0);
            ui.label(RichText::new(t("report_file.done.not_imported")).color(tokens.warn_text));
            for n in not_imported {
                ui.label(format!("\u{2022} {n}"));
            }
        }
        ui.add_space(8.0);
        if ui.button(t("report_file.close")).clicked() {
            self.close = true;
        }
        if let Some(row) = open {
            self.link.push(CompanyEvent::OpenWith(
                CommandId::new(super::KIND),
                Box::new(OpenPayload::Memorized(Box::new(
                    super::memorized_list::MemorizedOpen(row),
                ))),
            ));
        }
    }

    fn draw_review(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let Stage::Review(rv) = &mut self.stage else {
            return;
        };
        let mut chosen: Vec<(usize, String, Option<String>)> = Vec::new();
        let mut again: Vec<usize> = Vec::new();
        let mut want_lists: Vec<&'static str> = Vec::new();
        let busy = rv.preview.is_some();
        let n = rv.review.reports.len();
        ui.label(
            RichText::new(t_count("report_file.review.intro", n, &[])).color(tokens.text_secondary),
        );
        ui.add_space(6.0);
        let mut retry = false;
        if let Some(key) = rv.error {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(format!(
                        "{} {}",
                        t("report_file.review.failed_choice"),
                        t(key)
                    ))
                    .color(tokens.warn_text),
                );
                if ui.button(t("report_file.review.retry")).clicked() {
                    retry = true;
                }
            });
            ui.add_space(6.0);
        }
        // A hostile file can name thousands of rows: only the first
        // MAX_DRAWN_UNRESOLVED needing a choice are drawn (S-L4).
        let mut budget = MAX_DRAWN_UNRESOLVED;
        egui::ScrollArea::vertical()
            .id_salt("report-file-review")
            .auto_shrink([false, true])
            .max_height(ui.available_height() - 48.0)
            .show(ui, |ui| {
                for i in 0..n {
                    if card(ui, tokens, rv, i, &mut budget, &mut chosen, &mut want_lists) {
                        again.push(i);
                    }
                    ui.add_space(8.0);
                }
                let hidden = rv
                    .review
                    .unresolved_total()
                    .saturating_sub(MAX_DRAWN_UNRESOLVED);
                if hidden > 0 {
                    ui.label(
                        RichText::new(t_count("report_file.review.more", hidden, &[]))
                            .color(tokens.text_secondary),
                    );
                }
            });
        let can = rv.review.can_continue() && !busy;
        let single = rv.review.single();
        let confirm = rv.confirm;
        let mut finish = false;
        let mut cancel = false;
        ui.horizontal(|ui| {
            let label = if single {
                t("report_file.review.continue")
            } else {
                t("report_file.review.memorize_all")
            };
            if ui
                .add_enabled(can, egui::Button::new(label))
                .on_disabled_hover_text(t("report_file.review.not_ready"))
                .clicked()
            {
                finish = true;
            }
            if ui.button(t("report_file.review.cancel")).clicked() {
                cancel = true;
            }
            if busy {
                ui.label(
                    RichText::new(t("report_file.review.checking")).color(tokens.text_secondary),
                );
            }
        });
        let mut yes = false;
        if confirm {
            egui::Window::new(t("report_file.replace.confirm_title"))
                .id(egui::Id::unique("report-file-replace-confirm"))
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .collapsible(false)
                .resizable(false)
                .show(ui.ctx(), |ui| {
                    ui.set_width(380.0);
                    ui.add(egui::Label::new(t("report_file.replace.confirm")).wrap());
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if primary_button(ui, &t("report_file.replace.yes")).clicked() {
                            yes = true;
                        }
                        if ui.button(t("report_file.review.cancel")).clicked() {
                            cancel = true;
                        }
                    });
                });
        }
        if retry {
            self.retry();
        }
        for k in want_lists {
            self.load_list(k);
        }
        for (i, token, id) in chosen {
            self.choose(i, &token, id);
        }
        for i in again {
            self.start_over(i);
        }
        if yes {
            self.finish(true);
        } else if finish {
            self.finish(false);
        } else if cancel {
            self.cancel();
        }
    }

    /// Loads the full list of a reference kind for the pickers.
    fn load_list(&mut self, kind: &'static str) {
        let Stage::Review(rv) = &mut self.stage else {
            return;
        };
        if rv.lists.contains_key(kind) || rv.list_jobs.contains_key(kind) {
            return;
        }
        let job = engine::read(
            &self.link,
            self.group.as_ref(),
            move |d| Ok(crate::company::demo::report_transfer::pick_list(d, kind)),
            move |s| async move { live_list(&s, kind).await },
        );
        rv.list_jobs.insert(kind, job);
    }
}

/// The refusal key of a name taken at save.
const NAME_TAKEN: &str = "report_file.save.name_taken";

/// How a save step went.
enum Advance {
    Waiting,
    Next,
    Stop(&'static str),
}

/// The engine's rows of `kind` for the full-list picker.
async fn live_list(s: &cl_api::Session, kind: &str) -> Result<PickList, ApiError> {
    let mut v: PickList = match kind {
        "account" => s
            .accounts()
            .list_all(&cl_api::AccountFilter::default())
            .await?
            .into_iter()
            .map(|a| {
                let label = match a.fields.account_number.as_deref().filter(|n| !n.is_empty()) {
                    Some(n) => format!("{n} \u{b7} {}", a.fields.name),
                    None => a.fields.name.clone(),
                };
                (a.id.to_string(), label)
            })
            .collect(),
        "contact" => s
            .contacts()
            .list_all(&cl_api::ContactFilter::default())
            .await?
            .into_iter()
            .map(|c| (c.id.to_string(), c.fields.display_name))
            .collect(),
        "item" => s
            .items()
            .list_all(&cl_api::ItemFilter::default())
            .await?
            .into_iter()
            .map(|i| (i.id.to_string(), i.fields.name))
            .collect(),
        "tag" => s
            .tags()
            .list_all(&cl_api::TagFilter::default())
            .await?
            .into_iter()
            .map(|t| (t.id.to_string(), t.fields.name))
            .collect(),
        "tagGroup" => s
            .tags()
            .groups()
            .await?
            .into_iter()
            .map(|g| (g.id.to_string(), g.fields.name))
            .collect(),
        "budget" => s
            .budgets()
            .list()
            .await?
            .into_iter()
            .map(|b| (b.id, format!("{} ({})", b.name, b.fiscal_year)))
            .collect(),
        _ => Vec::new(),
    };
    v.sort_by_key(|(_, l)| l.to_lowercase());
    Ok(v)
}

/// The card's words for an unresolved reference: an account the file names
/// by number wasn't found under that number (Q-A09v); anything else wasn't
/// found under its name.
fn unresolved_key(r: Option<&cl_model::report_def::clreport::Reference>) -> &'static str {
    match r {
        Some(x)
            if x.kind == cl_model::report_def::clreport::RefKind::Account
                && x.number.as_deref().is_some_and(|n| !n.trim().is_empty()) =>
        {
            "report_file.card.unresolved_number"
        }
        _ => "report_file.card.unresolved",
    }
}

/// The wire kind of a reference kind.
const fn wire_kind(k: cl_model::report_def::clreport::RefKind) -> &'static str {
    use cl_model::report_def::clreport::RefKind;
    match k {
        RefKind::Account => "account",
        RefKind::Contact => "contact",
        RefKind::Item => "item",
        RefKind::Tag => "tag",
        RefKind::TagGroup => "tagGroup",
        RefKind::Budget => "budget",
    }
}

/// The most unresolved references drawn at once (S-L4).
pub(crate) const MAX_DRAWN_UNRESOLVED: usize = 50;

/// One report's card, drawing at most `budget` unresolved references
/// (taken from it). True when the owner pressed Start over.
fn card(
    ui: &mut Ui,
    tokens: &Tokens,
    rv: &mut Reviewing,
    i: usize,
    budget: &mut usize,
    chosen: &mut Vec<(usize, String, Option<String>)>,
    want_lists: &mut Vec<&'static str>,
) -> bool {
    let Some(r) = rv.review.reports.get(i).cloned() else {
        return false;
    };
    let mut again = false;
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            // Owner text: shown as typed, never looked up as a key.
            ui.label(RichText::new(r.name.as_str()).strong());
            let kind = if r.kind == "builder" {
                t("report_file.kind.builder")
            } else {
                t("report_file.kind.standard")
            };
            ui.label(RichText::new(kind).size(12.0).color(tokens.text_secondary));
            let state = if r.ready {
                t("report_file.card.ready")
            } else {
                t("report_file.card.needs_you")
            };
            ui.label(RichText::new(state).size(12.0).color(if r.ready {
                tokens.text_secondary
            } else {
                tokens.warn_text
            }));
        });
        let matched = rv.review.matched(i);
        if matched > 0 {
            ui.label(
                RichText::new(t_count("report_file.card.matched", matched, &[]))
                    .size(12.0)
                    .color(tokens.text_secondary),
            );
        }
        let drawn = r.unresolved.len().min(*budget);
        *budget -= drawn;
        for u in &r.unresolved[..drawn] {
            let reference = rv.review.reference(i, &u.reference_token);
            let (what, kind) = match &reference {
                Some(x) => (
                    x.label().map_or_else(
                        || t("report_file.ref.missing"),
                        |l| t_with(ref_kind_key(x.kind), &[("name", &l)]),
                    ),
                    Some(wire_kind(x.kind)),
                ),
                None => (t("report_file.ref.unknown"), None),
            };
            ui.add_space(4.0);
            ui.label(t_with(
                unresolved_key(reference.as_ref()),
                &[("what", &what)],
            ));
            ui.horizontal_wrapped(|ui| {
                let combo_label = t("report_file.card.pick");
                let l = ui.label(combo_label.as_str());
                // The options (candidates, then the full list) are built
                // only while the combo is open, and the full list is asked
                // for then (S-L4).
                egui::ComboBox::from_id_salt(("report-file-pick", i, u.reference_token.as_str()))
                    .width(260.0)
                    .selected_text(t("report_file.card.pick_one"))
                    .show_ui(ui, |ui| {
                        for c in &u.candidates {
                            if ui.selectable_label(false, c.label.as_str()).clicked() {
                                chosen.push((i, u.reference_token.clone(), Some(c.id.clone())));
                            }
                        }
                        if let Some(k) = kind {
                            want_lists.push(k);
                            for (id, text) in rv.lists.get(k).into_iter().flatten() {
                                if u.candidates.iter().any(|c| c.id == *id) {
                                    continue;
                                }
                                if ui.selectable_label(false, text.as_str()).clicked() {
                                    chosen.push((i, u.reference_token.clone(), Some(id.clone())));
                                }
                            }
                        }
                    })
                    .response
                    .labelled_by(l.id);
                if rv.review.can_leave_out(i, &u.reference_token) {
                    if ui
                        .button(t("report_file.card.leave_out"))
                        .on_hover_text(t("report_file.card.leave_out.tip"))
                        .clicked()
                    {
                        chosen.push((i, u.reference_token.clone(), None));
                    }
                } else {
                    ui.label(
                        RichText::new(t("report_file.card.budget_needed"))
                            .size(12.0)
                            .color(tokens.text_secondary),
                    );
                }
            });
        }
        for d in &r.dropped {
            let what = rv
                .review
                .reference(i, &d.reference_token)
                .and_then(|x| x.label())
                .unwrap_or_else(|| t("report_file.ref.missing"));
            ui.label(
                RichText::new(t_with(effect_key(&d.effect), &[("what", &what)]))
                    .size(12.0)
                    .color(tokens.warn_text),
            );
        }
        if let Some(p) = &r.problem {
            form::inline_note(
                ui,
                tokens,
                &t_with(
                    "report_file.card.problem",
                    &[("code", p.code.as_deref().unwrap_or("SCHEMA"))],
                ),
            );
        }
        // Every choice can be taken back (a leave-out the save check
        // refuses, a wrong pick).
        if rv.review.has_choices(i)
            && ui
                .button(t("report_file.card.start_over"))
                .on_hover_text(t("report_file.card.start_over.tip"))
                .clicked()
        {
            again = true;
        }
        clash_row(ui, tokens, rv, i, &r);
    });
    again
}

/// The name clash's Rename / Replace choice.
fn clash_row(ui: &mut Ui, tokens: &Tokens, rv: &mut Reviewing, i: usize, r: &PreviewReport) {
    if r.name_taken.is_none() {
        return;
    }
    ui.add_space(4.0);
    ui.label(RichText::new(t("report_file.clash.taken")).color(tokens.warn_text));
    let refused = Review::replace_refused(r);
    let current = rv.review.clashes.get(i).cloned().unwrap_or(Clash::None);
    ui.horizontal(|ui| {
        let renaming = matches!(current, Clash::Rename(_));
        if ui.radio(renaming, t("report_file.clash.rename")).clicked() && !renaming {
            rv.review.set_clash(i, Clash::Rename(r.name.clone()));
            // A free name again.
            let reports = rv.review.reports.clone();
            rv.review.set_clash(i, Clash::None);
            rv.review.take(reports);
        }
        if let Clash::Rename(mut name) = current.clone() {
            let l = ui.label(t("report_file.clash.new_name"));
            if ui
                .add(egui::TextEdit::singleline(&mut name).desired_width(240.0))
                .labelled_by(l.id)
                .changed()
            {
                rv.review.set_clash(i, Clash::Rename(name));
            }
        }
    });
    let replace = ui.add_enabled(
        refused.is_none(),
        egui::RadioButton::new(current == Clash::Replace, t("report_file.clash.replace")),
    );
    let replace = match refused {
        Some(why) => replace.on_disabled_hover_text(t(why)),
        None => replace.on_hover_text(t("report_file.clash.replace.tip")),
    };
    if replace.clicked() {
        rv.review.set_clash(i, Clash::Replace);
    }
    if let Some(why) = refused {
        ui.label(
            RichText::new(t(why))
                .size(12.0)
                .color(tokens.text_secondary),
        );
    }
    if let Some(p) = rv.review.clash_problem(i) {
        form::inline_note(ui, tokens, &t(p));
    }
}

impl Screen for ReportFileWindow {
    fn title(&self) -> String {
        t("report_file.title")
    }

    fn ui(&mut self, ui: &mut Ui) {
        // Opened without a payload (Go to, a restored layout): an import.
        if matches!(self.stage, Stage::Idle) {
            self.stage = Stage::Pick(None);
        }
        self.poll_export();
        self.poll_import();
        self.draw(ui);
        if !matches!(
            self.stage,
            Stage::Done { .. } | Stage::Failed { .. } | Stage::Idle
        ) {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        }
    }

    fn open_with(&mut self, payload: OpenPayload) -> bool {
        let OpenPayload::ReportFile(start) = payload else {
            return false;
        };
        match *start {
            Start::Export(target) => self.begin_export(&target),
            Start::Import => self.stage = Stage::Pick(None),
        }
        true
    }

    fn command(&mut self, id: CommandId) -> bool {
        match id.as_str() {
            "form.revert" => {
                self.cancel();
                true
            }
            _ => false,
        }
    }

    fn take_close_request(&mut self) -> bool {
        std::mem::take(&mut self.close)
    }

    fn state(&self) -> serde_json::Value {
        let review = self.review();
        serde_json::json!({
            "screen": "report_file",
            "stage": self.stage_name(),
            "last": self.last,
            "reports": review.map(|r| r.reports.len()),
            "ready": review.map(Review::can_continue),
            "unresolved": review.map(|r| r.reports.iter().map(|x| x.unresolved.len()).sum::<usize>()),
            "dropped": review.map(|r| r.reports.iter().map(|x| x.dropped.len()).sum::<usize>()),
            "clashes": review.map(|r| r.clashes.iter().filter(|c| **c != Clash::None).count()),
            "saved": match &self.stage {
                Stage::Done { saved, .. } => Some(saved.len()),
                _ => None,
            },
        })
    }
}
