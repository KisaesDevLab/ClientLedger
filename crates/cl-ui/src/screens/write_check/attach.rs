//! Attachments on a check: the drop zone and file field, the list, and
//! "Save a copy".
//!
//! Before the first save one file uploads as a draft and rides on the check
//! create as `draftAttachmentId`; once the check is saved, files attach to
//! it directly. A file is checked here first (PDF, PNG or JPEG by its first
//! bytes; at most 10 MB) and a refused one is never sent. A failed upload
//! never blocks saving the check.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use cl_api::{
    ATTACHMENT_MAX_BYTES, ApiError, AttachTo, Attachment, AttachmentId, DraftKey, FileKind,
    IdempotencyKey, LocalValidation, OrphanLabel,
};
use cl_model::forms::check::AttachmentRef;
use eframe::egui::{self, RichText, Ui};

use super::WriteCheckScreen;
use crate::company::Task;
use crate::i18n::{t, t_with};
use crate::screens::company::form;
use crate::theme::Tokens;

/// The check's attachments.
#[derive(Default)]
pub(crate) struct AttachState {
    /// The path typed in the file field.
    pub path: String,
    /// The draft the unsaved check's file went to.
    pub draft: Option<DraftKey>,
    /// The files on the check (or its draft).
    pub list: Vec<Attachment>,
    /// What the list is for.
    pub list_for: Option<AttachTo>,
    pub list_task: Option<Task<Vec<Attachment>>>,
    pub upload: Option<Task<Attachment>>,
    pub download: Option<Task<(String, Vec<u8>)>>,
    /// "Preview" (P17): the file's bytes on their way to the document tab.
    pub preview: Option<Task<(String, Vec<u8>)>>,
    /// A plain message (i18n key) about the last attempt.
    pub problem: Option<&'static str>,
    /// Where the last copy was saved.
    pub saved_to: Option<PathBuf>,
    /// The Attach button asked for the keyboard in the file field.
    pub focus: bool,
    /// The receipt-inbox file this new check started from (its name, for
    /// "Attached from … in your receipt inbox.", P17).
    pub from_inbox: Option<String>,
    /// A chosen file being read off the egui thread.
    pub reading: Option<std::sync::mpsc::Receiver<FileRead>>,
}

impl AttachState {
    pub(crate) fn busy(&self) -> bool {
        self.reading.is_some()
            || self.list_task.is_some()
            || self.upload.is_some()
            || self.download.is_some()
            || self.preview.is_some()
    }
}

/// Reads a file the owner chose, refusing it before anything is sent when
/// it is too large or not a PDF or image.
///
/// # Errors
/// An i18n key for the owner.
pub(crate) fn read_file(path: &Path) -> Result<(String, Vec<u8>), &'static str> {
    let meta = std::fs::metadata(path).map_err(|_| "check.attach.not_found")?;
    if !meta.is_file() {
        return Err("check.attach.not_found");
    }
    if meta.len() > ATTACHMENT_MAX_BYTES as u64 {
        return Err("check.attach.too_large");
    }
    let bytes = std::fs::read(path).map_err(|_| "check.attach.not_found")?;
    check_bytes(&bytes)?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    Ok((name, bytes))
}

/// A file read on its way to an upload: the name and bytes, or an i18n key.
pub(crate) type FileRead = Result<(String, Vec<u8>), &'static str>;

/// Reads the file at `path` with `read` on a background thread (up to 10 MB
/// from a possibly slow disk or network share), so the egui thread never
/// waits on it. Poll the answer with [`poll_file_read`].
///
/// # Errors
/// `check.attach.failed` when no thread could be started.
pub(crate) fn read_in_background(
    path: &Path,
    read: fn(&Path) -> FileRead,
) -> Result<std::sync::mpsc::Receiver<FileRead>, &'static str> {
    let (tx, rx) = std::sync::mpsc::channel();
    let path = path.to_path_buf();
    std::thread::Builder::new()
        .name("cl-attach-read".into())
        .spawn(move || {
            let _ = tx.send(read(&path));
        })
        .map(|_| rx)
        .map_err(|_| "check.attach.failed")
}

/// The background read's answer once it is there (taking it out of `slot`).
pub(crate) fn poll_file_read(
    slot: &mut Option<std::sync::mpsc::Receiver<FileRead>>,
) -> Option<FileRead> {
    let rx = slot.as_ref()?;
    let r = match rx.try_recv() {
        Ok(r) => r,
        Err(std::sync::mpsc::TryRecvError::Empty) => return None,
        Err(std::sync::mpsc::TryRecvError::Disconnected) => Err("check.attach.failed"),
    };
    *slot = None;
    Some(r)
}

/// The local checks on a file's bytes.
///
/// # Errors
/// An i18n key for the owner.
pub(crate) fn check_bytes(bytes: &[u8]) -> Result<(), &'static str> {
    match cl_api::check_file(bytes) {
        Ok(_) => Ok(()),
        Err(LocalValidation::AttachmentTooLarge) => Err("check.attach.too_large"),
        Err(_) => Err("check.attach.wrong_type"),
    }
}

/// Where "Save a copy" puts a file: the Downloads folder, else the
/// temporary folder.
fn copy_dir() -> PathBuf {
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"));
    home.map(|h| PathBuf::from(h).join("Downloads"))
        .filter(|p| p.is_dir())
        .unwrap_or_else(std::env::temp_dir)
}

/// Names Windows keeps for devices (with any extension).
const RESERVED: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// The extension a file of `kind` is saved with.
fn extension(kind: FileKind) -> &'static str {
    match kind {
        FileKind::Pdf => "pdf",
        FileKind::Png => "png",
        FileKind::Jpeg => "jpg",
    }
}

/// A safe file stem from the engine's file name: the last path part only,
/// safe characters only, no extension (the bytes decide it), no leading
/// dots, no trailing dots or spaces, never a Windows device name.
pub(crate) fn safe_stem(name: &str) -> String {
    let last = name.rsplit(['/', '\\']).next().unwrap_or_default();
    let stem = match last.rfind('.') {
        Some(i) if i > 0 => &last[..i],
        _ => last,
    };
    let safe: String = stem
        .chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, '.' | '-' | '_' | ' '))
        .take(120)
        .collect();
    let safe = safe
        .trim_start_matches(['.', ' '])
        .trim_end_matches(['.', ' '])
        .to_string();
    if safe.is_empty() {
        return "attachment".to_string();
    }
    let device = safe
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end()
        .to_uppercase();
    if RESERVED.contains(&device.as_str()) {
        return format!("file-{safe}");
    }
    safe
}

/// Saves downloaded bytes into `dir` as a new file: the bytes must be a
/// PDF, PNG or JPEG (checked here, as on upload), the extension is the
/// detected type's, the name is made safe, and an existing file is never
/// overwritten (` (2)`, ` (3)`, … is added instead).
///
/// # Errors
/// An i18n key for the owner.
pub(crate) fn save_copy(dir: &Path, name: &str, bytes: &[u8]) -> Result<PathBuf, &'static str> {
    let kind = cl_api::check_file(bytes).map_err(|_| "check.attach.download_wrong_type")?;
    let stem = safe_stem(name);
    let ext = extension(kind);
    for n in 1..1000 {
        let file = if n == 1 {
            format!("{stem}.{ext}")
        } else {
            format!("{stem} ({n}).{ext}")
        };
        let path = dir.join(file);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut f) => {
                if f.write_all(bytes).and_then(|()| f.sync_all()).is_err() {
                    drop(f);
                    let _ = std::fs::remove_file(&path);
                    return Err("check.attach.download_failed");
                }
                return Ok(path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return Err("check.attach.download_failed"),
        }
    }
    Err("check.attach.download_failed")
}

impl WriteCheckScreen {
    /// A fresh check that starts from a receipt-inbox file the inbox
    /// already uploaded as this check's draft (P17 "New check"). The draft
    /// rides on the first save as `draftAttachmentId`, exactly like a file
    /// attached here; the inbox files the original away once the saved
    /// check holds it. Returns false for a check already saved or one that
    /// has a draft file already.
    pub(crate) fn accept_inbox_draft(
        &mut self,
        draft: crate::screens::receipt_inbox::InboxDraft,
    ) -> bool {
        if self.editing.is_some()
            || self.attach.draft.is_some()
            || self.header.current().draft_attachment.is_some()
        {
            return false;
        }
        self.attach.draft = Some(draft.draft);
        let file = AttachmentRef {
            id: draft.draft.to_string(),
            name: draft.file.name.clone(),
        };
        self.header.edit_step(|h| h.draft_attachment = Some(file));
        self.attach.from_inbox = Some(draft.file.name);
        self.load_attachments();
        true
    }

    /// Where new files go: the saved check, else the draft (made on first
    /// use).
    fn attach_target(&mut self) -> AttachTo {
        if let Some(e) = &self.editing {
            return AttachTo::Expense(e.id);
        }
        AttachTo::Draft(*self.attach.draft.get_or_insert_with(DraftKey::new))
    }

    /// Attach the file at the typed path (or a dropped one).
    pub(crate) fn attach_path(&mut self, path: &Path) {
        self.attach.problem = None;
        self.attach.saved_to = None;
        if self.attach.upload.is_some() || self.attach.reading.is_some() {
            return;
        }
        if self.editing.is_none() && self.header.current().draft_attachment.is_some() {
            // One file rides on the first save; more attach once it is saved.
            self.attach.problem = Some("check.attach.one_before_save");
            return;
        }
        // Read off the egui thread; `poll_attach` uploads it.
        match read_in_background(path, read_file) {
            Ok(rx) => self.attach.reading = Some(rx),
            Err(key) => self.attach.problem = Some(key),
        }
    }

    /// Uploads checked bytes to the check or its draft.
    pub(crate) fn upload_bytes(&mut self, name: &str, bytes: Vec<u8>) {
        if let Err(key) = check_bytes(&bytes) {
            self.attach.problem = Some(key);
            return;
        }
        let to = self.attach_target();
        let name = name.to_string();
        let link = std::sync::Arc::clone(&self.link);
        if let Some(mut demo) = link.demo_state() {
            let r = crate::company::demo::checks::upload(&mut demo, to, &name, &bytes);
            drop(demo);
            if let Ok(a) = r {
                self.attach.upload = Some(Task::ready(a));
                self.poll_attach();
            } else {
                self.attach.problem = Some("check.attach.failed");
            }
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        let key = IdempotencyKey::new();
        let label = OrphanLabel {
            what: "orphan.check_attach",
            key: Some(key.clone()),
        };
        match group.write(&session, label, move |s| async move {
            s.attachments().upload(to, &name, &bytes, key).await
        }) {
            Ok(p) => self.attach.upload = Some(Task::Pending(p)),
            Err(_) => self.attach.problem = Some("check.attach.failed"),
        }
    }

    /// Reads the files on the saved check (or the draft) again.
    pub(crate) fn load_attachments(&mut self) {
        let of = match (&self.editing, self.attach.draft) {
            (Some(e), _) => AttachTo::Expense(e.id),
            (None, Some(d)) => AttachTo::Draft(d),
            (None, None) => {
                self.attach.list.clear();
                self.attach.list_for = None;
                return;
            }
        };
        self.attach.list_for = Some(of);
        let link = std::sync::Arc::clone(&self.link);
        if let Some(demo) = link.demo_state() {
            let list = crate::company::demo::checks::attachments(&demo, of);
            drop(demo);
            self.attach.list = list;
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        if let Ok(p) = group.read(&session, move |s| async move {
            let page = cl_api::PageRequest::new(50, 0)?;
            Ok(s.attachments().list(of, page).await?.data)
        }) {
            self.attach.list_task = Some(Task::Pending(p));
        }
    }

    /// "Save a copy": the file's bytes into the Downloads folder.
    pub(crate) fn download(&mut self, id: AttachmentId, name: String) {
        self.attach.problem = None;
        self.attach.saved_to = None;
        let link = std::sync::Arc::clone(&self.link);
        if let Some(demo) = link.demo_state() {
            let r = crate::company::demo::checks::download(&demo, id);
            drop(demo);
            match r {
                Ok(bytes) => self.attach.download = Some(Task::ready((name, bytes))),
                Err(_) => self.attach.problem = Some("check.attach.download_failed"),
            }
            self.poll_attach();
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        if let Ok(p) = group.read(&session, move |s| async move {
            Ok((name, s.attachments().download(id).await?))
        }) {
            self.attach.download = Some(Task::Pending(p));
        }
    }

    /// "Preview" (P17): the file in its own document tab
    /// (`nav.document_view`), read the same way as "Save a copy".
    pub(crate) fn preview(&mut self, id: AttachmentId, name: String) {
        self.attach.problem = None;
        let link = std::sync::Arc::clone(&self.link);
        if let Some(demo) = link.demo_state() {
            let r = crate::company::demo::checks::download(&demo, id);
            drop(demo);
            match r {
                Ok(bytes) => self.attach.preview = Some(Task::ready((name, bytes))),
                Err(_) => self.attach.problem = Some("check.attach.download_failed"),
            }
            self.poll_attach();
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        if let Ok(p) = group.read(&session, move |s| async move {
            Ok((name, s.attachments().download(id).await?))
        }) {
            self.attach.preview = Some(Task::Pending(p));
        }
    }

    /// Polls uploads, the list and downloads.
    pub(crate) fn poll_attach(&mut self) {
        if let Some(r) = poll_file_read(&mut self.attach.reading) {
            match r {
                Ok((name, bytes)) => self.upload_bytes(&name, bytes),
                Err(key) => self.attach.problem = Some(key),
            }
        }
        if let Some(r) = self.attach.preview.as_mut().and_then(Task::try_take) {
            self.attach.preview = None;
            match r {
                Ok((name, bytes)) => {
                    let source = crate::widgets::pdf_viewer::DocSource {
                        name,
                        bytes: std::sync::Arc::from(bytes),
                    };
                    self.link.push(crate::company::CompanyEvent::OpenWith(
                        cl_model::CommandId::new(crate::screens::document_view::KIND),
                        Box::new(crate::windows::OpenPayload::Document(source)),
                    ));
                }
                Err(_) => self.attach.problem = Some("check.attach.download_failed"),
            }
        }
        if let Some(r) = self.attach.upload.as_mut().and_then(Task::try_take) {
            self.attach.upload = None;
            match r {
                Ok(a) => {
                    self.attach.path.clear();
                    if self.editing.is_none()
                        && let Some(d) = self.attach.draft
                    {
                        let file = AttachmentRef {
                            id: d.to_string(),
                            name: a.file_name.clone(),
                        };
                        self.header.edit_step(|h| h.draft_attachment = Some(file));
                    }
                    self.load_attachments();
                }
                Err(e) => {
                    self.attach.problem = Some(match e {
                        ApiError::Local(LocalValidation::AttachmentTooLarge) => {
                            "check.attach.too_large"
                        }
                        ApiError::Local(LocalValidation::AttachmentType) => {
                            "check.attach.wrong_type"
                        }
                        _ => "check.attach.failed",
                    });
                }
            }
        }
        if let Some(r) = self.attach.list_task.as_mut().and_then(Task::try_take) {
            self.attach.list_task = None;
            if let Ok(list) = r {
                self.attach.list = list;
            }
        }
        if let Some(r) = self.attach.download.as_mut().and_then(Task::try_take) {
            self.attach.download = None;
            match r {
                Ok((name, bytes)) => match save_copy(&copy_dir(), &name, &bytes) {
                    Ok(path) => self.attach.saved_to = Some(path),
                    Err(key) => self.attach.problem = Some(key),
                },
                Err(_) => self.attach.problem = Some("check.attach.download_failed"),
            }
        }
    }

    /// Files dropped on the window go to the check.
    fn take_dropped(&mut self, ui: &Ui) {
        let dropped = ui.ctx().input(|i| i.raw.dropped_files.clone());
        let Some(f) = dropped.into_iter().next() else {
            return;
        };
        let path = f.path().to_path_buf();
        self.attach_path(&path);
    }

    /// The Attachments card.
    pub(crate) fn attachments_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        self.take_dropped(ui);
        let mut attach = false;
        let mut save_copy = None;
        let mut preview = None;
        form::card_tight(ui, tokens, |ui| {
            ui.label(
                RichText::new(t("check.panel.attachments"))
                    .strong()
                    .color(tokens.text_primary),
            );
            if let Some(name) = &self.attach.from_inbox {
                form::hint(
                    ui,
                    tokens,
                    &t_with("inbox.attached_from", &[("name", name)]),
                );
            }
            for a in &self.attach.list {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(&a.file_name).color(tokens.text_primary));
                    if ui
                        .add_enabled(
                            self.attach.preview.is_none(),
                            egui::Button::new(t("viewer.preview")),
                        )
                        .on_hover_text(t("viewer.preview.tip"))
                        .clicked()
                    {
                        preview = Some((a.id, a.file_name.clone()));
                    }
                    if ui
                        .button(t("check.attach.save_copy"))
                        .on_hover_text(t("check.attach.save_copy.tip"))
                        .clicked()
                    {
                        save_copy = Some((a.id, a.file_name.clone()));
                    }
                });
            }
            // The drop zone: a dashed border (mockup), painted after.
            let zone = egui::Frame::new()
                .corner_radius(egui::CornerRadius::same(6))
                .inner_margin(egui::Margin::same(10))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new(t("check.attach.drop")).color(tokens.text_secondary));
                    ui.label(
                        RichText::new(t("check.attach.kinds"))
                            .size(12.0)
                            .color(tokens.text_tertiary),
                    );
                    let l = ui.label(
                        RichText::new(t("check.attach.file"))
                            .size(12.0)
                            .color(tokens.text_secondary),
                    );
                    let r = ui
                        .add(
                            egui::TextEdit::singleline(&mut self.attach.path)
                                .desired_width(ui.available_width()),
                        )
                        .labelled_by(l.id);
                    if std::mem::take(&mut self.attach.focus) {
                        r.request_focus();
                    }
                    let typed = !self.attach.path.trim().is_empty();
                    let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    let busy = self.attach.upload.is_some();
                    attach = ui
                        .add_enabled(typed && !busy, egui::Button::new(t("check.attach.add")))
                        .clicked()
                        || (enter && typed && !busy);
                });
            crate::widgets::paint_dashed_border(
                ui.painter(),
                zone.response.rect,
                6.0,
                egui::Stroke::new(1.0, tokens.border_strong),
            );
            if self.attach.upload.is_some() {
                form::progress_line(ui, tokens, &t("check.attach.uploading"));
            }
            if let Some(key) = self.attach.problem {
                ui.add(
                    egui::Label::new(RichText::new(t(key)).size(12.0).color(tokens.warn_text))
                        .wrap(),
                );
            }
            if let Some(p) = &self.attach.saved_to {
                form::hint(
                    ui,
                    tokens,
                    &t_with(
                        "check.attach.saved_to",
                        &[("path", &p.display().to_string())],
                    ),
                );
            }
        });
        if attach {
            let path = PathBuf::from(self.attach.path.trim().trim_matches('"'));
            self.attach_path(&path);
        }
        if let Some((id, name)) = save_copy {
            self.download(id, name);
        }
        if let Some((id, name)) = preview {
            self.preview(id, name);
        }
    }
}
