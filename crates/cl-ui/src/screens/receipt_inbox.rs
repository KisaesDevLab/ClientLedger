//! The receipt inbox (P17, `nav.receipt_inbox`, plan §3 WP5, Q-54,
//! Q-P17p): the files in the owner's inbox folder, a preview, and the
//! actions that turn one into a bill, a check or a bank transaction's
//! attachment.
//!
//! - The folder is chosen in Preferences › Receipt inbox; it is listed by
//!   [`service`] on a background thread (no watcher crate) and the window
//!   polls the result every frame.
//! - A file is offered only when it is ready (unchanged for two listings
//!   ≥ 2 s apart), at most 10 MB and on this computer (not an online-only
//!   cloud placeholder). It is read on a background thread when selected;
//!   those exact bytes are previewed and uploaded.
//! - **New bill** / **New check** upload the file as a draft and open a
//!   fresh Enter Bill / Write Check with it (`OpenPayload::InboxDraft`).
//!   The original stays in the inbox until a saved record holds the upload
//!   (plan amendment S8); an abandoned form leaves it there.
//! - **Add to a bill… / a check… / a bank transaction…** upload straight
//!   onto the record picked. Once the engine confirms the upload (the size
//!   it stored is the size read) the original moves to
//!   `<inbox>/Filed/<YYYY-MM>/`, never overwriting and never deleted.
//! - File names are shown on screen; they are never logged, and the
//!   control channel's state carries counts and states only.

mod demo;
mod picker;
mod service;
#[cfg(test)]
mod tests;

pub(crate) use service::intake_problems;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, TryRecvError};

use cl_api::{
    ApiError, AttachTo, Attachment, AttachmentId, DraftKey, IdempotencyKey, LocalValidation,
    OrphanLabel, RequestGroup,
};
use cl_company::inbox::{FileError, FileState, InboxFile, NamedKind, ReadFile, ScanError};
use cl_core::CalendarDate;
use cl_core::ids::TenantId;
use cl_model::{CommandId, Scope};
use cl_settings::inbox_state::{FileIdentity, path_hash};
use eframe::egui::{self, RichText, Ui};
use serde_json::{Value, json};

use crate::company::{CompanyEvent, CompanyLink, Task};
use crate::i18n::{t, t_count, t_with};
use crate::screens::company::form;
use crate::theme::Tokens;
use crate::widgets::pdf_viewer::{DocSource, DocViewer, ViewerEvent, ViewerState};
use crate::windows::{OpenPayload, Screen};

pub(crate) use picker::{PickKind, PickRow};
pub(crate) use service::{FAST_KINDS, Folder, Mark, badge, tick};

/// The command and window kind.
pub const KIND: &str = "nav.receipt_inbox";

/// Enter Bill's window kind (it takes an inbox draft, WP4).
const ENTER_BILL: &str = "vendors.enter_bill";
/// Write Check's window kind.
const WRITE_CHECK: &str = "vendors.write_check";

/// The preview's height in points.
const PREVIEW_HEIGHT: f32 = 360.0;
/// Below this width the list and the preview stack.
const STACK_WIDTH: f32 = 720.0;

/// The receipt inbox window for `kind`, if it is one.
#[must_use]
pub fn make(
    kind: CommandId,
    link: &Arc<CompanyLink>,
    open: Option<(TenantId, String)>,
    today: CalendarDate,
) -> Option<Box<dyn Screen>> {
    (kind.as_str() == KIND)
        .then(|| Box::new(ReceiptInboxScreen::new(Arc::clone(link), open, today)) as Box<_>)
}

/// An inbox file already uploaded as a draft, handed to a new form.
#[derive(Clone, PartialEq, Eq)]
pub struct InboxDraft {
    /// The draft the file was uploaded under (sent on the create as
    /// `draftAttachmentId`).
    pub draft: DraftKey,
    /// The uploaded file.
    pub attachment: AttachmentId,
    /// Its name and bytes, for the preview (the name is owner data).
    pub file: DocSource,
}

/// Ids only.
impl std::fmt::Debug for InboxDraft {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InboxDraft")
            .field("draft", &self.draft)
            .field("attachment", &self.attachment)
            .finish_non_exhaustive()
    }
}

/// The selected file, read.
struct Loaded {
    file: InboxFile,
    read: ReadFile,
}

/// What an upload is for.
#[derive(Clone, Debug)]
enum Goal {
    /// A fresh Enter Bill with the file as its draft.
    NewBill(DraftKey),
    /// A fresh Write Check with the file as its draft.
    NewCheck(DraftKey),
    /// Straight onto a saved record.
    Record(PickKind, PickRow),
}

impl Goal {
    fn target(&self) -> AttachTo {
        match self {
            Goal::NewBill(d) | Goal::NewCheck(d) => AttachTo::Draft(*d),
            Goal::Record(_, row) => row.target(),
        }
    }
}

/// What the window shows of the service (kept for `state()`).
#[derive(Clone, Debug, Default)]
struct View {
    folder: &'static str,
    files: Vec<(FileState, Mark)>,
    /// The files' path hashes, in the same order.
    hashes: Vec<String>,
    more: usize,
    badge: usize,
    filed: u32,
    move_failures: u32,
}

/// The receipt inbox window.
pub struct ReceiptInboxScreen {
    link: Arc<CompanyLink>,
    open: Option<(TenantId, String)>,
    today: CalendarDate,
    group: Option<RequestGroup>,
    /// The selected file (its path hash).
    selected: Option<String>,
    reading: Option<(String, Receiver<Result<ReadFile, FileError>>)>,
    loaded: Option<Loaded>,
    read_problem: Option<&'static str>,
    viewer: ViewerState,
    picker: Option<picker::Picker>,
    upload: Option<(Goal, Task<Attachment>)>,
    /// A plain message (i18n key) about the last action.
    problem: Option<&'static str>,
    /// What the last action did (shown text).
    notice: Option<String>,
    view: View,
    demo_folder_used: bool,
    /// Where "Save a copy" put the selected file.
    saved_copy: Option<Result<PathBuf, &'static str>>,
}

impl ReceiptInboxScreen {
    /// The inbox of the open company.
    #[must_use]
    pub fn new(
        link: Arc<CompanyLink>,
        open: Option<(TenantId, String)>,
        today: CalendarDate,
    ) -> ReceiptInboxScreen {
        let group = link.group().and_then(Result::ok);
        let mut viewer = ViewerState::default();
        viewer.set_full_size_button(true);
        ReceiptInboxScreen {
            link,
            open,
            today,
            group,
            selected: None,
            reading: None,
            loaded: None,
            read_problem: None,
            viewer,
            picker: None,
            upload: None,
            problem: None,
            notice: None,
            view: View::default(),
            demo_folder_used: false,
            saved_copy: None,
        }
    }

    fn tenant(&self) -> Option<TenantId> {
        self.open.as_ref().map(|o| o.0)
    }

    /// One frame of the service, and the window's copy of what it shows.
    fn sync(&mut self, ctx: &egui::Context) -> Vec<(InboxFile, Mark)> {
        let h = service::handle(ctx);
        if !self.demo_folder_used {
            self.demo_folder_used = true;
            let mut s = h.lock();
            s.use_demo_folder(&self.link);
            // A window that opens lists the folder at once.
            s.poke();
        }
        drop(h);
        tick(ctx, &self.link, self.tenant(), self.today, true);
        let h = service::handle(ctx);
        let s = h.lock();
        let files: Vec<(InboxFile, Mark)> = s
            .listing()
            .map(|l| l.files.iter().map(|f| (f.clone(), s.mark(f))).collect())
            .unwrap_or_default();
        self.view = View {
            folder: folder_key(s.folder(), s.snapshot().map(|x| &x.listing)),
            files: files.iter().map(|(f, m)| (f.state, *m)).collect(),
            hashes: files.iter().map(|(f, _)| path_hash(&f.path)).collect(),
            more: s.listing().map_or(0, |l| l.more),
            badge: s.badge(),
            filed: s.filed,
            move_failures: s.move_failures,
        };
        files
    }

    fn select(&mut self, file: &InboxFile) {
        let hash = path_hash(&file.path);
        if self.selected.as_ref() == Some(&hash) {
            return;
        }
        self.selected = Some(hash.clone());
        self.loaded = None;
        self.read_problem = None;
        self.picker = None;
        self.saved_copy = None;
        self.viewer.clear();
        if !file.state.offerable() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let path = file.path.clone();
        let spawned = std::thread::Builder::new()
            .name("cl-inbox-read".into())
            .spawn(move || {
                let _ = tx.send(cl_company::inbox::read_for_upload(&path));
            });
        if spawned.is_ok() {
            self.reading = Some((hash, rx));
        } else {
            self.read_problem = Some("inbox.read.failed");
        }
    }

    fn clear_selection(&mut self) {
        self.selected = None;
        self.reading = None;
        self.loaded = None;
        self.read_problem = None;
        self.picker = None;
        self.saved_copy = None;
        self.viewer.clear();
    }

    fn poll_read(&mut self, files: &[(InboxFile, Mark)]) {
        let Some((hash, rx)) = &self.reading else {
            return;
        };
        let result = match rx.try_recv() {
            Ok(r) => r,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err(FileError::Io),
        };
        let hash = hash.clone();
        self.reading = None;
        if self.selected.as_ref() != Some(&hash) {
            return;
        }
        let Some((file, _)) = files.iter().find(|(f, _)| path_hash(&f.path) == hash) else {
            return;
        };
        match result {
            Ok(read) if read.size == file.size && read.modified_ms == file.modified_ms => {
                self.viewer.open(DocSource {
                    name: file.name.clone(),
                    bytes: Arc::from(read.bytes.clone()),
                });
                self.loaded = Some(Loaded {
                    file: file.clone(),
                    read,
                });
            }
            // It changed since it was listed: wait for the next listing.
            Ok(_) | Err(FileError::Unstable) => self.read_problem = Some("inbox.read.changed"),
            Err(e) => self.read_problem = Some(read_problem_key(e)),
        }
    }

    /// Upload the selected file for `goal`.
    fn start_upload(&mut self, goal: Goal) {
        self.problem = None;
        self.notice = None;
        if self.upload.is_some() {
            return;
        }
        let Some(loaded) = &self.loaded else {
            return;
        };
        let to = goal.target();
        let name = loaded.file.name.clone();
        let bytes = loaded.read.bytes.clone();
        let link = Arc::clone(&self.link);
        if let Some(mut demo) = link.demo_state() {
            let r = crate::company::demo::checks::upload(&mut demo, to, &name, &bytes);
            drop(demo);
            match r {
                Ok(a) => self.upload = Some((goal, Task::ready(a))),
                Err(e) => self.problem = Some(upload_problem_key(&e)),
            }
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            self.problem = Some("inbox.upload.failed");
            return;
        };
        let key = IdempotencyKey::new();
        let label = OrphanLabel {
            what: "inbox.orphan.upload",
            key: Some(key.clone()),
        };
        match group.write(&session, label, move |s| async move {
            s.attachments().upload(to, &name, &bytes, key).await
        }) {
            Ok(p) => self.upload = Some((goal, Task::Pending(p))),
            Err(_) => self.problem = Some("inbox.upload.failed"),
        }
    }

    fn poll_upload(&mut self, ctx: &egui::Context) {
        let Some(r) = self.upload.as_mut().and_then(|(_, t)| t.try_take()) else {
            return;
        };
        let Some((goal, _)) = self.upload.take() else {
            return;
        };
        let Some(loaded) = self.loaded.as_ref() else {
            return;
        };
        let a = match r {
            Ok(a) => a,
            Err(e) => {
                self.problem = Some(upload_problem_key(&e));
                return;
            }
        };
        // The engine stored what was read: the same size, where it was
        // sent.
        if a.file_size != Some(loaded.read.size)
            || a.attachable_type != goal.target().attachable_type()
        {
            self.problem = Some("inbox.upload.mismatch");
            return;
        }
        let identity = FileIdentity {
            path_hash: path_hash(&loaded.file.path),
            size: loaded.read.size,
            modified_ms: loaded.read.modified_ms,
            sha256: loaded.read.sha256.clone(),
        };
        let linked = matches!(goal, Goal::Record(..));
        service::handle(ctx)
            .lock()
            .record_upload(&loaded.file, &identity, a.id, linked);
        let source = DocSource {
            name: loaded.file.name.clone(),
            bytes: Arc::from(loaded.read.bytes.clone()),
        };
        match goal {
            Goal::NewBill(draft) | Goal::NewCheck(draft) => {
                let kind = if matches!(goal, Goal::NewBill(_)) {
                    ENTER_BILL
                } else {
                    WRITE_CHECK
                };
                self.link.push(CompanyEvent::OpenWith(
                    CommandId::new(kind),
                    Box::new(OpenPayload::InboxDraft(InboxDraft {
                        draft,
                        attachment: a.id,
                        file: source,
                    })),
                ));
                self.notice = Some(t("inbox.notice.draft"));
            }
            Goal::Record(kind, row) => {
                self.notice = Some(t_with(
                    "inbox.notice.attached",
                    &[("record", &record_text(kind, &row))],
                ));
                self.clear_selection();
            }
        }
    }

    fn choose_folder(&self) {
        crate::screens::preferences::open_at(crate::screens::preferences::Category::ReceiptInbox);
        self.link.push(CompanyEvent::OpenWindow(CommandId::new(
            crate::screens::preferences::COMMAND,
        )));
    }

    fn busy(&self) -> bool {
        self.reading.is_some()
            || self.upload.is_some()
            || self.picker.as_ref().is_some_and(picker::Picker::busy)
    }

    // -----------------------------------------------------------------
    // Drawing.

    fn draw(&mut self, ui: &mut Ui, tokens: &Tokens, files: &[(InboxFile, Mark)]) {
        form::heading(ui, tokens, &t("inbox.title"));
        let h = service::handle(ui.ctx());
        let folder = h.lock().folder().clone();
        drop(h);
        if let Folder::Chosen(dir) = &folder {
            ui.add(
                egui::Label::new(
                    RichText::new(t_with(
                        "inbox.folder",
                        &[("path", &crate::screens::document_view::shown_path(dir))],
                    ))
                    .color(tokens.text_secondary),
                )
                .wrap(),
            )
            .on_hover_text(dir.display().to_string());
        }
        ui.horizontal_wrapped(|ui| {
            if ui
                .button(t("inbox.check_now"))
                .on_hover_text(t("inbox.check_now.tip"))
                .clicked()
            {
                service::handle(ui.ctx()).lock().poke();
            }
            if ui
                .button(t("inbox.choose_folder"))
                .on_hover_text(t("inbox.choose_folder.tip"))
                .clicked()
            {
                self.choose_folder();
            }
        });
        ui.add_space(6.0);
        if self.empty_state(ui, tokens, &folder, files) {
            return;
        }
        self.messages(ui, tokens);
        let stacked = ui.available_width() < STACK_WIDTH;
        if stacked {
            self.file_list(ui, tokens, files);
            ui.add_space(8.0);
            self.detail(ui, tokens, files);
        } else {
            let list_width = (ui.available_width() * 0.4).clamp(260.0, 420.0);
            ui.horizontal_top(|ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(list_width, 0.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| self.file_list(ui, tokens, files),
                );
                ui.add_space(12.0);
                ui.vertical(|ui| self.detail(ui, tokens, files));
            });
        }
    }

    /// The no-folder, missing-folder and empty states. True when one was
    /// drawn (nothing else is).
    fn empty_state(
        &mut self,
        ui: &mut Ui,
        tokens: &Tokens,
        folder: &Folder,
        files: &[(InboxFile, Mark)],
    ) -> bool {
        let (title, hint, choose) = match (folder, self.view.folder) {
            (Folder::NoCompany, _) => ("inbox.empty.no_company", None, false),
            (Folder::NotChosen, _) => (
                "inbox.empty.not_chosen",
                Some("inbox.empty.not_chosen.hint"),
                true,
            ),
            (Folder::Refused(_), _) => (
                "inbox.empty.refused",
                Some("inbox.empty.refused.hint"),
                true,
            ),
            (_, "missing" | "not_a_folder") => (
                "inbox.empty.missing",
                Some("inbox.empty.missing.hint"),
                true,
            ),
            (_, "unreadable") => (
                "inbox.empty.unreadable",
                Some("inbox.empty.unreadable.hint"),
                true,
            ),
            (_, "loading") => ("inbox.empty.loading", None, false),
            _ if files.is_empty() => ("inbox.empty.none", Some("inbox.empty.none.hint"), false),
            _ => return false,
        };
        form::card(ui, tokens, |ui| {
            ui.label(RichText::new(t(title)).strong().color(tokens.text_primary));
            if let Some(h) = hint {
                form::hint(ui, tokens, &t(h));
            }
            if choose && crate::widgets::primary_button(ui, &t("inbox.choose_folder")).clicked() {
                self.choose_folder();
            }
        });
        self.messages(ui, tokens);
        true
    }

    fn messages(&self, ui: &mut Ui, tokens: &Tokens) {
        if let Some(n) = &self.notice {
            ui.add(egui::Label::new(RichText::new(n).color(tokens.text_secondary)).wrap());
        }
        if let Some(key) = self.problem {
            form::inline_warning(ui, tokens, &t(key));
        }
        if self.view.move_failures > 0 {
            form::hint(ui, tokens, &t("inbox.move_failed"));
        }
    }

    fn file_list(&mut self, ui: &mut Ui, tokens: &Tokens, files: &[(InboxFile, Mark)]) {
        let ready = files
            .iter()
            .filter(|(f, m)| f.state.offerable() && *m != Mark::Linked)
            .count();
        ui.label(
            RichText::new(t_count("inbox.count", ready, &[]))
                .strong()
                .color(tokens.text_primary),
        );
        let mut pick: Option<InboxFile> = None;
        for (f, mark) in files {
            let hash = path_hash(&f.path);
            let selected = self.selected.as_ref() == Some(&hash);
            let usable = f.state.offerable() && *mark != Mark::Linked;
            ui.add_space(2.0);
            let r = ui.add_enabled(
                usable,
                egui::Button::selectable(
                    selected,
                    RichText::new(&f.name).color(if usable {
                        tokens.text_primary
                    } else {
                        tokens.text_tertiary
                    }),
                ),
            );
            if r.clicked() {
                pick = Some(f.clone());
            }
            ui.label(
                RichText::new(file_line(f, *mark))
                    .size(12.0)
                    .color(tokens.text_secondary),
            );
        }
        if self.view.more > 0 {
            form::hint(ui, tokens, &t_count("inbox.more", self.view.more, &[]));
        }
        if let Some(f) = pick {
            self.select(&f);
        }
    }

    fn detail(&mut self, ui: &mut Ui, tokens: &Tokens, files: &[(InboxFile, Mark)]) {
        let Some(hash) = self.selected.clone() else {
            form::hint(ui, tokens, &t("inbox.pick_a_file"));
            return;
        };
        let Some((file, mark)) = files.iter().find(|(f, _)| path_hash(&f.path) == hash) else {
            return;
        };
        if self.reading.is_some() {
            form::progress_line(ui, tokens, &t("inbox.reading"));
        }
        if let Some(key) = self.read_problem {
            form::inline_warning(ui, tokens, &t(key));
        }
        if self.loaded.is_some() {
            let height = PREVIEW_HEIGHT.min(ui.available_height().max(200.0));
            let event = ui
                .allocate_ui(egui::vec2(ui.available_width(), height), |ui| {
                    ui.set_min_height(height);
                    DocViewer::show(ui, &mut self.viewer, tokens)
                })
                .inner;
            match event {
                ViewerEvent::OpenFullSize => {
                    if let Some(src) = self.viewer.source() {
                        self.link.push(CompanyEvent::OpenWith(
                            CommandId::new(crate::screens::document_view::KIND),
                            Box::new(OpenPayload::Document(src.clone())),
                        ));
                    }
                }
                ViewerEvent::SaveCopy => {
                    if let Some(src) = self.viewer.source() {
                        self.saved_copy = Some(crate::screens::document_view::save_copy(
                            &downloads(),
                            &src.bytes,
                        ));
                    }
                }
                ViewerEvent::None => {}
            }
            if let Some(saved) = &self.saved_copy {
                let text = match saved {
                    Ok(p) => t_with(
                        "check.attach.saved_to",
                        &[("path", &crate::screens::document_view::shown_path(p))],
                    ),
                    Err(k) => t(k),
                };
                form::hint(ui, tokens, &text);
            }
        }
        if *mark == Mark::Uploaded {
            form::hint(ui, tokens, &t("inbox.mark.uploaded.hint"));
        }
        ui.add_space(6.0);
        if self.picker.is_some() {
            self.picker_ui(ui, tokens);
        } else {
            self.actions(ui, tokens, file);
        }
    }

    fn actions(&mut self, ui: &mut Ui, tokens: &Tokens, file: &InboxFile) {
        let ready = self.loaded.is_some() && self.upload.is_none();
        let _ = file;
        let mut chosen: Option<usize> = None;
        ui.horizontal_wrapped(|ui| {
            let labels = [
                ("inbox.action.new_bill", "inbox.action.new_bill.tip"),
                ("inbox.action.add_bill", "inbox.action.add_bill.tip"),
                ("inbox.action.new_check", "inbox.action.new_check.tip"),
                ("inbox.action.add_check", "inbox.action.add_check.tip"),
                ("inbox.action.add_bank", "inbox.action.add_bank.tip"),
            ];
            for (n, (label, tip)) in labels.iter().enumerate() {
                let r = if n == 0 {
                    ui.add_enabled_ui(ready, |ui| crate::widgets::primary_button(ui, &t(label)))
                        .inner
                } else {
                    ui.add_enabled(ready, egui::Button::new(t(label)))
                };
                if r.on_hover_text(t(tip)).clicked() {
                    chosen = Some(n);
                }
            }
        });
        if self.upload.is_some() {
            form::progress_line(ui, tokens, &t("inbox.uploading"));
        }
        match chosen {
            Some(0) => self.start_upload(Goal::NewBill(DraftKey::new())),
            Some(1) => self.picker = Some(picker::Picker::new(PickKind::Bill)),
            Some(2) => self.start_upload(Goal::NewCheck(DraftKey::new())),
            Some(3) => self.picker = Some(picker::Picker::new(PickKind::Check)),
            Some(4) => self.picker = Some(picker::Picker::new(PickKind::BankTxn)),
            _ => {}
        }
    }

    fn picker_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let link = Arc::clone(&self.link);
        let style = crate::grid_env::shell_frame(ui.ctx()).money_style;
        let Some(p) = self.picker.as_mut() else {
            return;
        };
        p.refresh(&link, self.group.as_ref());
        let mut attach: Option<PickRow> = None;
        let mut cancel = false;
        let kind = p.kind;
        form::card(ui, tokens, |ui| {
            ui.label(
                RichText::new(t(kind.title_key()))
                    .strong()
                    .color(tokens.text_primary),
            );
            if kind == PickKind::BankTxn {
                let current = p
                    .account
                    .and_then(|a| p.accounts.iter().find(|x| x.0 == a))
                    .map(|x| x.1.clone())
                    .unwrap_or_default();
                let l = ui.label(t("inbox.pick.account"));
                egui::ComboBox::from_id_salt("inbox-pick-account")
                    .selected_text(current)
                    .width(280.0)
                    .show_ui(ui, |ui| {
                        for (id, name) in &p.accounts {
                            ui.selectable_value(&mut p.account, Some(*id), name);
                        }
                    })
                    .response
                    .labelled_by(l.id);
            }
            form::field_wide(ui, &t("inbox.pick.search"), &mut p.search);
            if p.failed {
                form::inline_warning(ui, tokens, &t("inbox.pick.failed"));
            } else if p.busy() && !p.loaded {
                form::progress_line(ui, tokens, &t("inbox.pick.loading"));
            } else if p.loaded && p.rows.is_empty() {
                form::hint(ui, tokens, &t("inbox.pick.none"));
            }
            for row in &p.rows {
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .add_enabled(
                            self.upload.is_none(),
                            egui::Button::new(t("inbox.pick.attach")),
                        )
                        .on_hover_text(t_with(
                            "inbox.pick.attach.tip",
                            &[("record", &record_text(kind, row))],
                        ))
                        .clicked()
                    {
                        attach = Some(row.clone());
                    }
                    ui.label(
                        RichText::new(crate::shell::us_date(row.date))
                            .monospace()
                            .color(tokens.text_secondary),
                    );
                    if let Some(n) = &row.number {
                        ui.label(RichText::new(n).color(tokens.text_secondary));
                    }
                    ui.label(
                        RichText::new(row.payee.clone().unwrap_or_default())
                            .color(tokens.text_primary),
                    );
                    if let Some(m) = row.amount {
                        ui.label(RichText::new(m.format(style)).monospace());
                    }
                });
            }
            if ui.button(t("inbox.pick.cancel")).clicked() {
                cancel = true;
            }
        });
        if self.upload.is_some() {
            form::progress_line(ui, tokens, &t("inbox.uploading"));
        }
        if cancel {
            self.picker = None;
        } else if let Some(row) = attach {
            self.start_upload(Goal::Record(kind, row));
        }
    }

    fn state_json(&self) -> Value {
        let count = |s: FileState| self.view.files.iter().filter(|(f, _)| *f == s).count();
        let marked = |m: Mark| self.view.files.iter().filter(|(_, x)| *x == m).count();
        let selected = self.selected.as_ref().and_then(|_| {
            self.loaded
                .as_ref()
                .map(|l| (l.file.state.as_str(), kind_str(l.file.kind)))
        });
        let selected_mark = self.selected.as_ref().and_then(|h| {
            self.view
                .files
                .iter()
                .zip(&self.view.hashes)
                .find(|(_, x)| *x == h)
                .map(|((_, m), _)| m.as_str())
        });
        json!({
            "screen": "receipt_inbox",
            "date": self.today.to_string(),
            "folder": self.view.folder,
            "listed": self.view.files.len(),
            "ready": count(FileState::Ready),
            "settling": count(FileState::Settling),
            "too_large": count(FileState::TooLarge),
            "empty": count(FileState::Empty),
            "online_only": count(FileState::OnlineOnly),
            "uploaded": marked(Mark::Uploaded),
            "linked": marked(Mark::Linked),
            "more": self.view.more,
            "badge": self.view.badge,
            "filed": self.view.filed,
            "move_failures": self.view.move_failures,
            "selected": self.selected.is_some(),
            "loaded": selected.is_some(),
            "selected_kind": selected.map(|s| s.1),
            "selected_mark": selected_mark,
            "reading": self.reading.is_some(),
            "preview": self.viewer.state_json(),
            "picker": self.picker.as_ref().map(|p| json!({
                "kind": p.kind.as_str(),
                "rows": p.rows.len(),
                "loaded": p.loaded,
                "accounts": p.accounts.len(),
            })),
            "uploading": self.upload.is_some(),
            "problem": self.problem,
            "read_problem": self.read_problem,
            "notice": self.notice.is_some(),
        })
    }
}

impl Screen for ReceiptInboxScreen {
    fn title(&self) -> String {
        t(&format!("cmd.{KIND}"))
    }

    fn ui(&mut self, ui: &mut Ui) {
        let tokens = Tokens::of(ui.ctx());
        let ctx = ui.ctx().clone();
        let files = self.sync(&ctx);
        self.poll_read(&files);
        self.poll_upload(&ctx);
        if let Some(sel) = &self.selected
            && self.upload.is_none()
            && !files.iter().any(|(f, _)| path_hash(&f.path) == *sel)
        {
            // Filed away, or gone from the folder.
            self.clear_selection();
        }
        if let Some(p) = self.picker.as_mut() {
            p.poll();
        }
        // The window scrolls up and down only, so long names and paths wrap
        // at its width instead of running off it.
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
                self.draw(ui, &tokens, &files);
            });
        if self.busy() {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    fn scope(&self) -> Scope {
        if self.viewer.has_focus() {
            Scope::Viewer
        } else {
            Scope::Window
        }
    }

    fn command(&mut self, id: CommandId) -> bool {
        self.viewer.command(id)
    }

    fn state(&self) -> Value {
        self.state_json()
    }

    fn fixed_layout(&self) -> bool {
        // It scrolls itself, vertically only (see `ui`).
        true
    }
}

// ---------------------------------------------------------------------------
// Helpers.

fn folder_key(
    folder: &Folder,
    listing: Option<&Result<cl_company::inbox::Listing, ScanError>>,
) -> &'static str {
    match folder {
        Folder::NoCompany => "no_company",
        Folder::NotChosen => "not_chosen",
        Folder::Refused(_) => "refused",
        Folder::Chosen(_) => match listing {
            None => "loading",
            Some(Ok(_)) => "ok",
            Some(Err(ScanError::Missing)) => "missing",
            Some(Err(ScanError::NotAFolder)) => "not_a_folder",
            Some(Err(ScanError::Unreadable)) => "unreadable",
        },
    }
}

fn kind_str(k: NamedKind) -> &'static str {
    match k {
        NamedKind::Pdf => "pdf",
        NamedKind::Png => "png",
        NamedKind::Jpeg => "jpeg",
    }
}

/// "PDF · 1.2 MB · 11/03/2026 · Ready".
fn file_line(f: &InboxFile, mark: Mark) -> String {
    let kind = match f.kind {
        NamedKind::Pdf => "PDF",
        NamedKind::Png => "PNG",
        NamedKind::Jpeg => "JPEG",
    };
    let state = match (f.state, mark) {
        (FileState::Ready, Mark::Uploaded) => t("inbox.mark.uploaded"),
        (FileState::Ready, Mark::Linked) => t("inbox.mark.linked"),
        (FileState::Ready, Mark::New) => t("inbox.state.ready"),
        (FileState::Settling, _) => t("inbox.state.settling"),
        (FileState::TooLarge, _) => t("inbox.state.too_large"),
        (FileState::Empty, _) => t("inbox.state.empty"),
        (FileState::OnlineOnly, _) => t("inbox.state.online_only"),
    };
    let mut parts = vec![kind.to_string(), size_text(f.size)];
    if let Some(d) = date_of(f.modified_ms) {
        parts.push(crate::shell::us_date(d));
    }
    parts.push(state);
    parts.join(" \u{b7} ")
}

/// "820 KB", "1.2 MB" (display only; whole numbers, no floats).
fn size_text(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * 1024;
    if bytes >= MB {
        let tenths = (bytes * 10 + MB / 2) / MB;
        t_with(
            "inbox.size.mb",
            &[("size", &format!("{}.{}", tenths / 10, tenths % 10))],
        )
    } else {
        let kb = bytes.div_ceil(KB).max(1);
        t_with("inbox.size.kb", &[("size", &kb.to_string())])
    }
}

/// The local date of a modified time (milliseconds since 1970).
fn date_of(ms: i64) -> Option<CalendarDate> {
    use chrono::{Datelike as _, TimeZone as _};
    let dt = chrono::Local.timestamp_millis_opt(ms).single()?;
    CalendarDate::from_ymd(dt.year(), dt.month(), dt.day()).ok()
}

/// "check 4417 (Ozark Feed & Seed, 10/28/2026)".
fn record_text(kind: PickKind, row: &PickRow) -> String {
    let what = match kind {
        PickKind::Bill => t("inbox.record.bill"),
        PickKind::Check => t("inbox.record.check"),
        PickKind::BankTxn => t("inbox.record.txn"),
    };
    let mut s = what;
    if let Some(n) = &row.number {
        s.push(' ');
        s.push_str(n);
    }
    let mut extra = Vec::new();
    if let Some(p) = row.payee.as_ref().filter(|p| !p.is_empty()) {
        extra.push(p.clone());
    }
    extra.push(crate::shell::us_date(row.date));
    format!("{s} ({})", extra.join(", "))
}

fn read_problem_key(e: FileError) -> &'static str {
    match e {
        FileError::Gone => "inbox.read.gone",
        FileError::OnlineOnly => "inbox.state.online_only",
        FileError::TooLarge => "inbox.state.too_large",
        FileError::Empty => "inbox.state.empty",
        FileError::WrongType => "inbox.read.wrong_type",
        FileError::Changed | FileError::Unstable => "inbox.read.changed",
        FileError::NotAFile | FileError::Io => "inbox.read.failed",
    }
}

fn upload_problem_key(e: &ApiError) -> &'static str {
    match e {
        ApiError::Local(LocalValidation::AttachmentTooLarge) => "inbox.state.too_large",
        ApiError::Local(LocalValidation::AttachmentType) => "inbox.read.wrong_type",
        ApiError::OutcomeUnknown { .. } => "inbox.upload.unknown",
        _ => "inbox.upload.failed",
    }
}

/// Where "Save a copy" puts a file: the Downloads folder, else the
/// temporary folder.
fn downloads() -> PathBuf {
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"));
    home.map(|h| PathBuf::from(h).join("Downloads"))
        .filter(|p| p.is_dir())
        .unwrap_or_else(std::env::temp_dir)
}
