//! A bill's files and its source-document card (P17, plan §3 WP4).
//!
//! Before the first save every file uploads under one draft key; with
//! `bill-lines-v1` the create carries the draft (`draftAttachmentId`), and
//! before it the files move onto the saved bill afterwards (`attach-draft`,
//! or `link` per file for a credit; a failure leaves the bill saved and
//! offers Try again, plan §5). Once the bill is saved, files attach to it
//! directly. A file is checked here first (PDF, PNG or JPEG by its first
//! bytes; at most 10 MB) and a refused one is never sent. A failed upload
//! never blocks saving the bill.
//!
//! The card shows the file through the document viewer (WP1) with "Open
//! full size", and "Read with AI" (WP7's button). An incoming receipt-inbox
//! file ([`crate::screens::receipt_inbox::InboxDraft`], WP5) is already
//! uploaded as the draft: the card shows it at once.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use cl_api::{
    ATTACHMENT_MAX_BYTES, ApiError, AttachTo, Attachment, AttachmentId, DraftKey, FileKind,
    IdempotencyKey, LocalValidation, OrphanLabel,
};
use cl_model::forms::bill::{BillForm, BillKind};
use cl_model::forms::check::AttachmentRef;
use eframe::egui::{self, RichText, Ui};
use serde_json::{Value, json};

use super::EnterBillScreen;
use crate::company::{CompanyEvent, Task};
use crate::i18n::{t, t_with};
use crate::screens::company::form;
use crate::theme::Tokens;
use crate::widgets::pdf_viewer::{DocSource, DocViewer, ViewerEvent, ViewerState};

/// The preview's height on the card (mockup: 230 px of page, plus the
/// viewer's own bar).
const PREVIEW_HEIGHT: f32 = 300.0;

/// The bill's files and the source-document card.
#[derive(Default)]
pub(crate) struct AttachState {
    /// The path typed in the file field.
    pub path: String,
    /// The draft the unsaved bill's files went to.
    pub draft: Option<DraftKey>,
    /// The files on the bill (or its draft).
    pub list: Vec<Attachment>,
    pub list_task: Option<Task<Vec<Attachment>>>,
    pub upload: Option<Task<Attachment>>,
    /// The name and bytes being uploaded (shown once the engine took them).
    uploading: Option<DocSource>,
    pub download: Option<Task<(String, Vec<u8>)>>,
    /// A file's bytes on their way to the preview.
    pub preview: Option<Task<(AttachmentId, String, Vec<u8>)>>,
    /// The file the card shows.
    pub shown: Option<AttachmentId>,
    /// The card's viewer.
    pub viewer: ViewerState,
    /// The shown file came from the receipt inbox (the banner says so).
    pub from_inbox: bool,
    /// A plain message (i18n key) about the last attempt.
    pub problem: Option<&'static str>,
    /// Where the last copy was saved.
    pub saved_to: Option<PathBuf>,
    /// The Attach button asked for the keyboard in the file field.
    pub focus: bool,
}

impl AttachState {
    pub(crate) fn busy(&self) -> bool {
        self.list_task.is_some()
            || self.upload.is_some()
            || self.download.is_some()
            || self.preview.is_some()
    }

    /// A new bill or another record: no files, nothing shown (the viewer's
    /// worker stays).
    pub(crate) fn reset(&mut self) {
        self.path.clear();
        self.draft = None;
        self.list.clear();
        self.list_task = None;
        self.upload = None;
        self.uploading = None;
        self.download = None;
        self.preview = None;
        self.shown = None;
        self.viewer.clear();
        self.from_inbox = false;
        self.problem = None;
        self.saved_to = None;
        self.focus = false;
    }

    /// The control channel's view (no file names).
    pub(crate) fn state_json(&self, form: &BillForm) -> Value {
        json!({
            "count": self.list.len(),
            "draft_files": form.attachments.len(),
            "uploading": self.upload.is_some(),
            "problem": self.problem,
            "saved_copy": self.saved_to.is_some(),
            "from_inbox": self.from_inbox,
            "viewer": self.viewer.state_json(),
        })
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

/// The local checks on a file's bytes (10 MB exactly is accepted).
///
/// # Errors
/// An i18n key for the owner.
pub(crate) fn check_bytes(bytes: &[u8]) -> Result<(), &'static str> {
    if bytes.is_empty() {
        return Err("bill.attach.empty");
    }
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

/// Saves bytes into `dir` as a new file named "Bill document.<ext>" (the
/// type checked from the bytes; the owner's file name is never used for a
/// path), never overwriting: " (2)", " (3)", … is added instead.
///
/// # Errors
/// An i18n key for the owner.
pub(crate) fn save_copy(dir: &Path, bytes: &[u8]) -> Result<PathBuf, &'static str> {
    let kind = cl_api::check_file(bytes).map_err(|_| "check.attach.download_wrong_type")?;
    let ext = match kind {
        FileKind::Pdf => "pdf",
        FileKind::Png => "png",
        FileKind::Jpeg => "jpg",
    };
    let stem = t("bill.attach.copy_name");
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

impl EnterBillScreen {
    /// Where new files go: the saved bill or credit, else the draft (made
    /// on first use).
    fn attach_target(&mut self) -> AttachTo {
        if let Some(e) = &self.editing {
            return match e.kind {
                BillKind::Bill => AttachTo::Bill(e.id),
                BillKind::Credit => AttachTo::VendorCredit(e.id),
            };
        }
        AttachTo::Draft(*self.attach.draft.get_or_insert_with(DraftKey::new))
    }

    /// Attach the file at the typed path (or a dropped one).
    pub(crate) fn attach_path(&mut self, path: &Path) {
        self.attach.problem = None;
        self.attach.saved_to = None;
        if self.attach.upload.is_some() {
            return;
        }
        match read_file(path) {
            Ok((name, bytes)) => self.upload_bytes(&name, bytes),
            Err(key) => self.attach.problem = Some(key),
        }
    }

    /// Uploads checked bytes to the bill or its draft.
    pub(crate) fn upload_bytes(&mut self, name: &str, bytes: Vec<u8>) {
        if let Err(key) = check_bytes(&bytes) {
            self.attach.problem = Some(key);
            return;
        }
        let to = self.attach_target();
        let name = name.to_string();
        self.attach.uploading = Some(DocSource {
            name: name.clone(),
            bytes: Arc::from(bytes.clone()),
        });
        let link = Arc::clone(&self.link);
        if let Some(mut demo) = link.demo_state() {
            let r = crate::company::demo::bills::upload(&mut demo, to, &name, &bytes);
            drop(demo);
            if let Ok(a) = r {
                self.attach.upload = Some(Task::ready(a));
                self.poll_attach();
            } else {
                self.attach.uploading = None;
                self.attach.problem = Some("check.attach.failed");
            }
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        let key = IdempotencyKey::new();
        let label = OrphanLabel {
            what: "orphan.bill_attach",
            key: Some(key.clone()),
        };
        if let Ok(p) = group.write(&session, label, move |s| async move {
            s.attachments().upload(to, &name, &bytes, key).await
        }) {
            self.attach.upload = Some(Task::Pending(p));
        } else {
            self.attach.uploading = None;
            self.attach.problem = Some("check.attach.failed");
        }
    }

    /// A receipt-inbox file already uploaded as the draft (WP5's "New
    /// bill"): it rides on the save like a file attached here.
    pub(crate) fn take_inbox_draft(&mut self, d: crate::screens::receipt_inbox::InboxDraft) {
        self.attach.draft = Some(d.draft);
        let file = AttachmentRef {
            id: d.attachment.to_string(),
            name: d.file.name.clone(),
        };
        self.header.edit_step(|h| h.attachments.push(file));
        self.attach.from_inbox = true;
        self.attach.shown = Some(d.attachment);
        self.attach.viewer.set_full_size_button(true);
        self.attach.viewer.open(d.file);
        self.load_attachments();
    }

    /// Reads the files on the saved bill (or the draft) again.
    pub(crate) fn load_attachments(&mut self) {
        let of = match (&self.editing, self.attach.draft) {
            (Some(e), _) => match e.kind {
                BillKind::Bill => AttachTo::Bill(e.id),
                BillKind::Credit => AttachTo::VendorCredit(e.id),
            },
            (None, Some(d)) => AttachTo::Draft(d),
            (None, None) => {
                self.attach.list.clear();
                return;
            }
        };
        let link = Arc::clone(&self.link);
        if let Some(demo) = link.demo_state() {
            let list = crate::company::demo::bills::attachments(&demo, of);
            drop(demo);
            self.attach.list_task = Some(Task::ready(list));
            self.poll_attach();
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

    fn fetch_bytes(&mut self, id: AttachmentId, name: String, preview: bool) {
        self.attach.problem = None;
        let link = Arc::clone(&self.link);
        if let Some(demo) = link.demo_state() {
            let r = crate::company::demo::bills::download(&demo, id);
            drop(demo);
            match r {
                Ok(bytes) if preview => self.attach.preview = Some(Task::ready((id, name, bytes))),
                Ok(bytes) => self.attach.download = Some(Task::ready((name, bytes))),
                Err(_) => self.attach.problem = Some("check.attach.download_failed"),
            }
            self.poll_attach();
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        if preview {
            if let Ok(p) = group.read(&session, move |s| async move {
                Ok((id, name, s.attachments().download(id).await?))
            }) {
                self.attach.preview = Some(Task::Pending(p));
            }
        } else if let Ok(p) = group.read(&session, move |s| async move {
            Ok((name, s.attachments().download(id).await?))
        }) {
            self.attach.download = Some(Task::Pending(p));
        }
    }

    /// Polls uploads, the list, downloads and the preview.
    pub(crate) fn poll_attach(&mut self) {
        if let Some(r) = self.attach.upload.as_mut().and_then(Task::try_take) {
            self.attach.upload = None;
            let source = self.attach.uploading.take();
            match r {
                Ok(a) => {
                    self.attach.path.clear();
                    if self.editing.is_none() {
                        let file = AttachmentRef {
                            id: a.id.to_string(),
                            name: a.file_name.clone(),
                        };
                        self.header.edit_step(|h| h.attachments.push(file));
                    }
                    if let Some(src) = source {
                        self.attach.shown = Some(a.id);
                        self.attach.from_inbox = false;
                        self.attach.viewer.set_full_size_button(true);
                        self.attach.viewer.open(src);
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
                // A saved bill opens on its first file.
                if self.attach.shown.is_none()
                    && let Some(first) = self.attach.list.first()
                {
                    let (id, name) = (first.id, first.file_name.clone());
                    self.attach.shown = Some(id);
                    self.fetch_bytes(id, name, true);
                }
            }
        }
        if let Some(r) = self.attach.preview.as_mut().and_then(Task::try_take) {
            self.attach.preview = None;
            match r {
                Ok((id, name, bytes)) => {
                    self.attach.shown = Some(id);
                    self.attach.viewer.set_full_size_button(true);
                    self.attach.viewer.open(DocSource {
                        name,
                        bytes: Arc::from(bytes),
                    });
                }
                Err(_) => self.attach.problem = Some("check.attach.download_failed"),
            }
        }
        if let Some(r) = self.attach.download.as_mut().and_then(Task::try_take) {
            self.attach.download = None;
            match r {
                Ok((_, bytes)) => match save_copy(&copy_dir(), &bytes) {
                    Ok(path) => self.attach.saved_to = Some(path),
                    Err(key) => self.attach.problem = Some(key),
                },
                Err(_) => self.attach.problem = Some("check.attach.download_failed"),
            }
        }
    }

    /// Files dropped on the window go to the bill.
    fn take_dropped(&mut self, ui: &Ui) {
        let dropped = ui.ctx().input(|i| i.raw.dropped_files.clone());
        let Some(f) = dropped.into_iter().next() else {
            return;
        };
        let path = f.path().to_path_buf();
        self.attach_path(&path);
    }

    /// The Source document card: the preview, Read with AI, the files and
    /// the file field.
    pub(crate) fn source_card_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        self.take_dropped(ui);
        let mut attach = false;
        let mut save = None;
        let mut preview = None;
        let mut event = ViewerEvent::None;
        form::card_tight(ui, tokens, |ui| {
            ui.label(
                RichText::new(t("bill.source.title"))
                    .strong()
                    .color(tokens.text_primary),
            );
            if self.attach.viewer.source().is_some() {
                let w = ui.available_width();
                ui.allocate_ui(egui::vec2(w, PREVIEW_HEIGHT), |ui| {
                    event = DocViewer::show(ui, &mut self.attach.viewer, tokens);
                });
            } else if self.attach.preview.is_some() {
                form::progress_line(ui, tokens, &t("bill.source.loading"));
            } else {
                form::hint(ui, tokens, &t("bill.source.none"));
            }
            let name = self.attach.viewer.source().map(|s| s.name.clone());
            let input = super::doc_read::ReadInput {
                link: &self.link,
                file_name: name.as_deref(),
                attachment: self.attach.shown,
                from_inbox: self.attach.from_inbox,
            };
            let _ = super::doc_read::read_button(ui, &mut self.read, &input, tokens);
            for a in &self.attach.list {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(&a.file_name).color(tokens.text_primary));
                    if Some(a.id) != self.attach.shown
                        && ui
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
                        save = Some((a.id, a.file_name.clone()));
                    }
                });
            }
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
        match event {
            ViewerEvent::OpenFullSize => {
                if let Some(src) = self.attach.viewer.source().cloned() {
                    self.link.push(CompanyEvent::OpenWith(
                        cl_model::CommandId::new(crate::screens::document_view::KIND),
                        Box::new(crate::windows::OpenPayload::Document(src)),
                    ));
                }
            }
            ViewerEvent::SaveCopy => {
                if let Some(src) = self.attach.viewer.source() {
                    match save_copy(&copy_dir(), &src.bytes) {
                        Ok(path) => self.attach.saved_to = Some(path),
                        Err(key) => self.attach.problem = Some(key),
                    }
                }
            }
            ViewerEvent::None => {}
        }
        if attach {
            let path = PathBuf::from(self.attach.path.trim().trim_matches('"'));
            self.attach_path(&path);
        }
        if let Some((id, name)) = save {
            self.fetch_bytes(id, name, false);
        }
        if let Some((id, name)) = preview {
            self.fetch_bytes(id, name, true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ten_megabytes_exactly_is_taken_and_one_more_byte_is_not() {
        let mut ok = b"%PDF-1.4\n".to_vec();
        ok.resize(ATTACHMENT_MAX_BYTES, b' ');
        assert_eq!(check_bytes(&ok), Ok(()));
        ok.push(b' ');
        assert_eq!(check_bytes(&ok), Err("check.attach.too_large"));
        assert_eq!(check_bytes(b"not a pdf"), Err("check.attach.wrong_type"));
        assert_eq!(check_bytes(b""), Err("bill.attach.empty"));
        assert!(crate::i18n::has("bill.attach.empty"));
    }

    #[test]
    fn a_copy_never_overwrites_and_takes_its_type_from_the_bytes() {
        let dir = tempfile::tempdir().expect("tmp");
        let pdf = b"%PDF-1.4\n%%EOF\n";
        let a = save_copy(dir.path(), pdf).expect("first");
        let b = save_copy(dir.path(), pdf).expect("second");
        assert_ne!(a, b);
        assert!(a.to_string_lossy().ends_with(".pdf"));
        assert!(b.to_string_lossy().ends_with(" (2).pdf"));
        assert_eq!(
            save_copy(dir.path(), b"plain"),
            Err("check.attach.download_wrong_type")
        );
        let missing = dir.path().join("nope.pdf");
        assert_eq!(read_file(&missing), Err("check.attach.not_found"));
    }
}
