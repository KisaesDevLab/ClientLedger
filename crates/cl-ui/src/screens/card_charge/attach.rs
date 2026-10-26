//! Attachments on a card charge or credit (A02, Q-A02e): Write Check's
//! card (the drop zone, the file field and Browse, the list, "Save a
//! copy") with the document pane, so the pop-out viewer follows it.
//!
//! A saved charge takes files directly (`expense`); a saved credit is a
//! deposit into the card (`deposit`). The engine's card-charge create
//! carries no draft, so before the first save files go to a draft and move
//! onto the record once it is saved (`POST /attachments/{id}/link`, one by
//! one). A failed move leaves the charge saved and offers Try again; it
//! never blocks or undoes the save. A file is checked here first (PDF,
//! PNG, JPEG or TIFF by its first bytes; at most 10 MB) and a refused one
//! is never sent.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cl_api::{
    ApiError, AttachTo, Attachment, AttachmentId, DraftKey, IdempotencyKey, LocalValidation,
    OrphanLabel, RequestGroup, TxnType,
};
use cl_core::ids::TransactionId;
use cl_model::forms::card::CardMode;
use eframe::egui::{self, RichText, Ui};
use serde_json::{Value, json};

use super::CardChargeScreen;
use crate::company::{CompanyEvent, CompanyLink, Task};
use crate::i18n::{t, t_with};
use crate::screens::company::form;
use crate::screens::write_check::attach::{
    FileRead, browse_button, browse_for_file, check_bytes, copy_dir, poll_file_read, read_file,
    read_in_background, save_copy,
};
use crate::theme::Tokens;
use crate::widgets::doc_pane::DocPane;
use crate::widgets::pdf_viewer::{DocSource, ViewerEvent, ViewerState};

/// The pane's height on the card.
const PANE_HEIGHT: f32 = 360.0;

/// Where a saved record of `mode` keeps its files.
#[must_use]
pub(crate) fn record_target(mode: CardMode, id: TransactionId) -> AttachTo {
    match mode {
        CardMode::Charge => AttachTo::Expense(id),
        CardMode::Credit => AttachTo::Txn(TxnType::Deposit, id),
    }
}

/// Draft files that still have to move onto a saved record.
#[derive(Debug, Clone)]
pub(crate) struct PendingMove {
    to: AttachTo,
    files: Vec<AttachmentId>,
}

/// The form's attachments.
#[derive(Default)]
pub(crate) struct CardAttach {
    /// The path typed in the file field.
    pub path: String,
    /// The draft an unsaved charge's files went to.
    pub draft: Option<DraftKey>,
    /// The saved record the files belong to.
    pub record: Option<AttachTo>,
    /// The files on the record (or its draft).
    pub list: Vec<Attachment>,
    list_task: Option<Task<Vec<Attachment>>>,
    upload: Option<Task<Attachment>>,
    /// The name and bytes being uploaded (shown once the engine took them).
    uploading: Option<DocSource>,
    download: Option<Task<(String, Vec<u8>)>>,
    /// The file the pane shows.
    pub shown: Option<AttachmentId>,
    pane_task: Option<Task<(AttachmentId, String, Vec<u8>)>>,
    /// The pane's viewer.
    pub viewer: ViewerState,
    /// A plain message (i18n key) about the last attempt.
    pub problem: Option<&'static str>,
    /// Where the last copy was saved.
    pub saved_to: Option<PathBuf>,
    /// The Attach button asked for the keyboard in the file field.
    pub focus: bool,
    reading: Option<std::sync::mpsc::Receiver<FileRead>>,
    browsing: Option<std::sync::mpsc::Receiver<Vec<PathBuf>>>,
    /// Draft files moving onto the record just saved (kept across a new
    /// form: Save & New starts one at once).
    moving: Option<(PendingMove, Task<()>)>,
    /// The last move failed: the record is saved without them.
    move_failed: Option<PendingMove>,
}

impl CardAttach {
    pub(crate) fn busy(&self) -> bool {
        self.reading.is_some()
            || self.browsing.is_some()
            || self.list_task.is_some()
            || self.upload.is_some()
            || self.download.is_some()
            || self.pane_task.is_some()
            || self.moving.is_some()
    }

    /// Another record (new, or read from the books): nothing attached,
    /// nothing shown. The viewer's worker and a move under way stay.
    pub(crate) fn reset(&mut self) {
        let mut viewer = std::mem::take(&mut self.viewer);
        viewer.clear();
        let moving = self.moving.take();
        let move_failed = self.move_failed.take();
        *self = CardAttach {
            viewer,
            moving,
            move_failed,
            ..CardAttach::default()
        };
    }

    fn show(&mut self, id: AttachmentId, source: DocSource) {
        self.shown = Some(id);
        self.viewer.set_full_size_button(true);
        self.viewer.open(source);
    }

    /// The control channel's view (no file names).
    pub(crate) fn state_json(&self) -> Value {
        json!({
            "count": self.list.len(),
            "saved_record": self.record.is_some(),
            "draft": self.draft.is_some(),
            "uploading": self.upload.is_some(),
            "moving": self.moving.is_some(),
            "move_failed": self.move_failed.is_some(),
            "problem": self.problem,
            "saved_copy": self.saved_to.is_some(),
            "viewer": self.viewer.state_json(),
        })
    }
}

/// Moves each draft file onto `to`.
async fn move_files(s: &cl_api::Session, m: PendingMove) -> Result<(), ApiError> {
    for f in m.files {
        s.attachments().link(f, m.to, IdempotencyKey::new()).await?;
    }
    Ok(())
}

/// Starts moving `m` (the demo moves at once).
fn start_move(
    link: &Arc<CompanyLink>,
    group: Option<&RequestGroup>,
    draft: DraftKey,
    m: &PendingMove,
) -> Option<Task<()>> {
    if let Some(mut demo) = link.demo_state() {
        crate::company::demo::checks::link_draft(&mut demo, draft, m.to);
        return Some(Task::ready(()));
    }
    let (session, group) = (link.session()?, group?);
    let label = OrphanLabel {
        what: "orphan.card_attach",
        key: None,
    };
    let m = m.clone();
    group
        .write(
            &session,
            label,
            move |s| async move { move_files(&s, m).await },
        )
        .ok()
        .map(Task::Pending)
}

impl CardChargeScreen {
    /// Where new files go: the saved record, else the draft (made on first
    /// use).
    fn attach_target(&mut self) -> AttachTo {
        if let Some(to) = self.attach.record {
            return to;
        }
        AttachTo::Draft(*self.attach.draft.get_or_insert_with(DraftKey::new))
    }

    /// A saved record is open (`id`): its files.
    pub(crate) fn attach_opened(&mut self, id: TransactionId) {
        self.attach.reset();
        self.attach.record = Some(record_target(self.header.current().mode, id));
        self.load_attachments();
    }

    /// The form was saved as `id`: files attached before the first save
    /// move onto it; the open record's files are read again.
    pub(crate) fn attach_saved(&mut self, id: TransactionId) {
        let to = record_target(self.header.current().mode, id);
        let fresh = self.attach.record.is_none();
        self.attach.record = Some(to);
        if fresh
            && let Some(draft) = self.attach.draft.take()
            && !self.attach.list.is_empty()
        {
            let m = PendingMove {
                to,
                files: self.attach.list.iter().map(|a| a.id).collect(),
            };
            match start_move(&self.link, self.group.as_ref(), draft, &m) {
                Some(task) => {
                    self.attach.move_failed = None;
                    self.attach.moving = Some((m, task));
                }
                None => self.attach.move_failed = Some(m),
            }
            self.poll_attach();
        }
    }

    /// Try again after a failed move.
    fn retry_move(&mut self) {
        let Some(m) = self.attach.move_failed.take() else {
            return;
        };
        if self.link.is_demo() {
            // The demo moves its files at once and never fails.
            return;
        }
        let task = self
            .link
            .session()
            .zip(self.group.as_ref())
            .and_then(|(session, group)| {
                let label = OrphanLabel {
                    what: "orphan.card_attach",
                    key: None,
                };
                let m = m.clone();
                group
                    .write(
                        &session,
                        label,
                        move |s| async move { move_files(&s, m).await },
                    )
                    .ok()
            });
        match task {
            Some(p) => self.attach.moving = Some((m, Task::Pending(p))),
            None => self.attach.move_failed = Some(m),
        }
    }

    /// Attach the file at the typed path (or a dropped one).
    pub(crate) fn attach_path(&mut self, path: &Path) {
        self.attach.problem = None;
        self.attach.saved_to = None;
        if self.attach.upload.is_some() || self.attach.reading.is_some() {
            return;
        }
        match read_in_background(path, read_file) {
            Ok(rx) => self.attach.reading = Some(rx),
            Err(key) => self.attach.problem = Some(key),
        }
    }

    /// Uploads checked bytes to the record or its draft.
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
            let r = crate::company::demo::checks::upload(&mut demo, to, &name, &bytes);
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
            what: "orphan.card_attach",
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

    /// Reads the files on the record (or the draft) again.
    pub(crate) fn load_attachments(&mut self) {
        let of = match (self.attach.record, self.attach.draft) {
            (Some(to), _) => to,
            (None, Some(d)) => AttachTo::Draft(d),
            (None, None) => {
                self.list_arrived(Vec::new());
                return;
            }
        };
        let link = Arc::clone(&self.link);
        if let Some(demo) = link.demo_state() {
            let list = crate::company::demo::checks::attachments(&demo, of);
            drop(demo);
            self.list_arrived(list);
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

    /// The pane keeps its file while it is still attached, else shows the
    /// first one (or nothing).
    fn list_arrived(&mut self, list: Vec<Attachment>) {
        self.attach.list = list;
        let kept = self
            .attach
            .shown
            .is_some_and(|id| self.attach.list.iter().any(|a| a.id == id));
        if kept {
            return;
        }
        if let Some(first) = self.attach.list.first() {
            let (id, name) = (first.id, first.file_name.clone());
            self.fetch_bytes(id, name, true);
        } else {
            self.attach.shown = None;
            self.attach.pane_task = None;
            self.attach.viewer.clear();
        }
    }

    /// A file's bytes: for the pane (`pane`), else for "Save a copy".
    fn fetch_bytes(&mut self, id: AttachmentId, name: String, pane: bool) {
        if pane {
            if self.attach.shown == Some(id)
                && (self.attach.pane_task.is_some() || self.attach.viewer.source().is_some())
            {
                return;
            }
            self.attach.shown = Some(id);
        }
        let link = Arc::clone(&self.link);
        if let Some(demo) = link.demo_state() {
            let r = crate::company::demo::checks::download(&demo, id);
            drop(demo);
            match r {
                Ok(bytes) if pane => self.attach.pane_task = Some(Task::ready((id, name, bytes))),
                Ok(bytes) => self.attach.download = Some(Task::ready((name, bytes))),
                Err(_) => self.attach.problem = Some("check.attach.download_failed"),
            }
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        if pane {
            if let Ok(p) = group.read(&session, move |s| async move {
                Ok((id, name, s.attachments().download(id).await?))
            }) {
                self.attach.pane_task = Some(Task::Pending(p));
            }
        } else if let Ok(p) = group.read(&session, move |s| async move {
            Ok((name, s.attachments().download(id).await?))
        }) {
            self.attach.download = Some(Task::Pending(p));
        }
    }

    /// Polls the file window, reads, uploads, the list, moves and
    /// downloads.
    pub(crate) fn poll_attach(&mut self) {
        if let Some(files) = crate::widgets::file_picker::poll(&mut self.attach.browsing)
            && let Some(path) = files.into_iter().next()
        {
            self.attach_path(&path);
        }
        if let Some(r) = poll_file_read(&mut self.attach.reading) {
            match r {
                Ok((name, bytes)) => self.upload_bytes(&name, bytes),
                Err(key) => self.attach.problem = Some(key),
            }
        }
        if let Some(r) = self.attach.upload.as_mut().and_then(Task::try_take) {
            self.attach.upload = None;
            let source = self.attach.uploading.take();
            match r {
                Ok(a) => {
                    self.attach.path.clear();
                    if let Some(src) = source {
                        self.attach.pane_task = None;
                        self.attach.show(a.id, src);
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
                self.list_arrived(list);
            }
        }
        if let Some(r) = self
            .attach
            .moving
            .as_mut()
            .and_then(|(_, task)| task.try_take())
            && let Some((m, _)) = self.attach.moving.take()
        {
            match r {
                Ok(()) => {
                    // The open record (Save) shows them from the books now.
                    if self.attach.record == Some(m.to) {
                        self.load_attachments();
                    }
                }
                Err(_) => self.attach.move_failed = Some(m),
            }
        }
        if let Some(r) = self.attach.pane_task.as_mut().and_then(Task::try_take) {
            self.attach.pane_task = None;
            match r {
                Ok((id, name, bytes)) if self.attach.shown == Some(id) => {
                    let bytes = Arc::from(bytes);
                    self.attach.show(id, DocSource { name, bytes });
                }
                Ok(_) => {}
                Err(_) => self.attach.problem = Some("check.attach.download_failed"),
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

    /// Files dropped on the window go to the charge.
    fn take_dropped(&mut self, ui: &Ui) {
        let dropped = ui.ctx().input(|i| i.raw.dropped_files.clone());
        let Some(f) = dropped.into_iter().next() else {
            return;
        };
        let path = f.path().to_path_buf();
        self.attach_path(&path);
    }

    /// The Attachments card (the side column).
    pub(crate) fn attachments_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        self.take_dropped(ui);
        let mut attach = false;
        let mut browse = false;
        let mut retry = false;
        let mut copy_of = None;
        let mut event = ViewerEvent::None;
        form::card_tight(ui, tokens, |ui| {
            ui.label(
                RichText::new(t("check.panel.attachments"))
                    .strong()
                    .color(tokens.text_primary),
            );
            if self.attach.viewer.source().is_some() {
                if crate::windows::doc_viewer::is_popped(ui.ctx()) {
                    event = DocPane::show(ui, &mut self.attach.viewer, tokens);
                } else {
                    let w = ui.available_width();
                    ui.allocate_ui(egui::vec2(w, PANE_HEIGHT), |ui| {
                        event = DocPane::show(ui, &mut self.attach.viewer, tokens);
                    });
                }
            } else if self.attach.pane_task.is_some() {
                form::progress_line(ui, tokens, &t("bill.source.loading"));
            }
            for a in &self.attach.list {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(&a.file_name).color(tokens.text_primary));
                    if ui
                        .button(t("check.attach.save_copy"))
                        .on_hover_text(t("check.attach.save_copy.tip"))
                        .clicked()
                    {
                        copy_of = Some((a.id, a.file_name.clone()));
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
                    let browsing = self.attach.browsing.is_some() || self.attach.reading.is_some();
                    ui.horizontal_wrapped(|ui| {
                        attach = ui
                            .add_enabled(typed && !busy, egui::Button::new(t("check.attach.add")))
                            .clicked()
                            || (enter && typed && !busy);
                        browse = browse_button(ui, !busy && !browsing);
                    });
                });
            crate::widgets::paint_dashed_border(
                ui.painter(),
                zone.response.rect,
                6.0,
                egui::Stroke::new(1.0, tokens.border_strong),
            );
            if self.attach.upload.is_some() || self.attach.moving.is_some() {
                form::progress_line(ui, tokens, &t("check.attach.uploading"));
            }
            if self.attach.move_failed.is_some() {
                ui.add(
                    egui::Label::new(
                        RichText::new(t("card.attach.move_failed"))
                            .size(12.0)
                            .color(tokens.warn_text),
                    )
                    .wrap(),
                );
                retry = ui.button(t("card.attach.try_again")).clicked();
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
                        &[("path", &crate::screens::document_view::shown_path(p))],
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
                    match save_copy(&copy_dir(), &src.name, &src.bytes) {
                        Ok(path) => self.attach.saved_to = Some(path),
                        Err(key) => self.attach.problem = Some(key),
                    }
                }
            }
            // Pop out and Dock went to the shell through the pane.
            ViewerEvent::None | ViewerEvent::PopOut | ViewerEvent::Dock => {}
        }
        if attach {
            let path = PathBuf::from(self.attach.path.trim().trim_matches('"'));
            self.attach_path(&path);
        }
        if browse && self.attach.browsing.is_none() {
            self.attach.browsing = Some(browse_for_file(ui.ctx()));
        }
        if retry {
            self.retry_move();
        }
        if let Some((id, name)) = copy_of {
            self.attach.problem = None;
            self.attach.saved_to = None;
            self.fetch_bytes(id, name, false);
            self.poll_attach();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_charge_keeps_files_as_an_expense_and_a_credit_as_a_deposit() {
        let id = TransactionId::parse("00000000-0000-4000-8000-000000000001").expect("id");
        assert_eq!(
            record_target(CardMode::Charge, id).attachable_type(),
            "expense"
        );
        assert_eq!(
            record_target(CardMode::Credit, id).attachable_type(),
            "deposit"
        );
        assert!(crate::i18n::has("card.attach.move_failed"));
        assert!(crate::i18n::has("orphan.card_attach"));
    }
}
