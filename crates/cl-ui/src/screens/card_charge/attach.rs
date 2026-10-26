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
//!
//! No file is lost on the way (review B2): the save waits while a file is
//! being read, sent or listed; the move lists the draft from the engine
//! (so it takes every file there, not the list shown) and runs on the
//! write lane, so it finishes even if the window closes; the draft is kept
//! until the move succeeded; a file still on its way into the draft when
//! the save finished moves when it gets there; and a failed move makes the
//! window ask before it closes.

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

/// A draft whose files still have to move onto a saved record: whatever
/// the draft holds when the move runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PendingMove {
    draft: DraftKey,
    to: AttachTo,
}

/// Pages of a draft listed by one move (50 files a page); a move that
/// finds more runs again.
const MOVE_PAGES: usize = 20;

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
    /// Where the upload under way goes.
    upload_to: Option<AttachTo>,
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
    /// The draft handed to the last move: a file that reaches it after the
    /// save moves too (kept across a new form).
    handed_off: Option<PendingMove>,
    /// A file reached the draft while its move ran: move again after it.
    move_again: bool,
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

    /// A file is being read, sent or listed: saving now could leave it
    /// behind in the draft (review B2), so the save waits.
    pub(crate) fn save_waits(&self) -> bool {
        self.reading.is_some() || self.upload.is_some() || self.list_task.is_some()
    }

    /// Files of a saved record that are not on it yet: a move that failed,
    /// or a file still on its way into a draft already handed off. Closing
    /// the window now asks first.
    pub(crate) fn unsettled(&self) -> bool {
        self.move_failed.is_some() || self.late_upload()
    }

    /// The upload under way goes into a draft whose save already happened.
    fn late_upload(&self) -> bool {
        self.upload.is_some()
            && matches!(
                (self.upload_to, self.handed_off),
                (Some(AttachTo::Draft(d)), Some(m)) if d == m.draft
            )
    }

    /// Another record (new, or read from the books): nothing attached,
    /// nothing shown. The viewer's worker, a move under way or failed, and
    /// a file still on its way into a draft already handed off stay.
    pub(crate) fn reset(&mut self) {
        let mut viewer = std::mem::take(&mut self.viewer);
        viewer.clear();
        let late = self.late_upload();
        let (upload, upload_to) = if late {
            (self.upload.take(), self.upload_to)
        } else {
            (None, None)
        };
        *self = CardAttach {
            viewer,
            moving: self.moving.take(),
            move_failed: self.move_failed.take(),
            handed_off: self.handed_off.take(),
            move_again: self.move_again,
            upload,
            upload_to,
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
            "unsettled": self.unsettled(),
            "problem": self.problem,
            "saved_copy": self.saved_to.is_some(),
            "viewer": self.viewer.state_json(),
        })
    }
}

/// Moves every file the engine lists in the draft onto `m.to` (a moved
/// file leaves the draft, so each page starts again from the top).
pub(crate) async fn move_files(s: &cl_api::Session, m: PendingMove) -> Result<(), ApiError> {
    for _ in 0..MOVE_PAGES {
        let page = cl_api::PageRequest::new(50, 0)?;
        let files = s
            .attachments()
            .list(AttachTo::Draft(m.draft), page)
            .await?
            .data;
        let full = files.len() >= 50;
        for f in files {
            s.attachments()
                .link(f.id, m.to, IdempotencyKey::new())
                .await?;
        }
        if !full {
            return Ok(());
        }
    }
    Ok(())
}

/// Starts moving `m` on the write lane, so it finishes even if the window
/// closes (the demo moves at once). `None` when it could not start.
fn start_move(
    link: &Arc<CompanyLink>,
    group: Option<&RequestGroup>,
    m: PendingMove,
) -> Option<Task<()>> {
    if let Some(mut demo) = link.demo_state() {
        crate::company::demo::checks::link_draft(&mut demo, m.draft, m.to);
        return Some(Task::ready(()));
    }
    let (session, group) = (link.session()?, group?);
    let label = OrphanLabel {
        what: "orphan.card_attach",
        key: None,
    };
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
        // The draft stays until its files moved (`draft` keeps it for
        // this form, the move for after a new one).
        if fresh && let Some(draft) = self.attach.draft {
            let m = PendingMove { draft, to };
            self.attach.handed_off = Some(m);
            self.run_move(m);
            self.poll_attach();
        }
    }

    /// Move `m`'s files now, or once the move under way ends.
    fn run_move(&mut self, m: PendingMove) {
        if self.attach.moving.is_some() {
            self.attach.move_again = true;
            return;
        }
        match start_move(&self.link, self.group.as_ref(), m) {
            Some(task) => {
                self.attach.move_failed = None;
                self.attach.moving = Some((m, task));
            }
            None => self.attach.move_failed = Some(m),
        }
    }

    /// Try again after a failed move (it lists the draft again).
    pub(crate) fn retry_move(&mut self) {
        if let Some(m) = self.attach.move_failed.take() {
            self.run_move(m);
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
                self.attach.upload_to = Some(to);
                self.poll_attach();
            } else {
                self.attach.uploading = None;
                self.attach.problem = Some("check.attach.failed");
            }
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            // No company open: nothing was sent.
            self.attach.uploading = None;
            self.attach.problem = Some("check.attach.failed");
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
            self.attach.upload_to = Some(to);
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
            let went_to = self.attach.upload_to.take();
            let source = self.attach.uploading.take();
            match r {
                Ok(a) => {
                    self.attach.path.clear();
                    if let Some(src) = source {
                        self.attach.pane_task = None;
                        self.attach.show(a.id, src);
                    }
                    // It reached a draft whose record is already saved:
                    // it moves too.
                    if let (Some(AttachTo::Draft(d)), Some(m)) = (went_to, self.attach.handed_off)
                        && d == m.draft
                    {
                        self.run_move(m);
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
            let again = std::mem::take(&mut self.attach.move_again);
            match r {
                Ok(()) if again => self.run_move(m),
                Ok(()) => {
                    // Every file moved: the draft is done with.
                    if self.attach.draft == Some(m.draft) {
                        self.attach.draft = None;
                    }
                    // The open record (Save) shows them from the books now.
                    if self.attach.record == Some(m.to) {
                        self.load_attachments();
                    }
                }
                // Try again lists the draft afresh, late files too.
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
        assert!(crate::i18n::has("card.attach.wait_save"));
    }

    use super::super::tests::{fill, link, posted, screen};
    use crate::windows::Screen as _;
    use cl_model::forms::SaveIntent;

    const PDF: &[u8] = b"%PDF-1.4\n%synthetic\n";

    /// An upload on the queue (not the demo's at-once one) that ends when
    /// the test sends its answer.
    struct HeldUpload {
        _queue: cl_api::ApiQueue,
        task: Task<Attachment>,
        answer: tokio::sync::oneshot::Sender<Attachment>,
    }

    fn held_upload() -> HeldUpload {
        let queue = cl_api::ApiQueue::start(Arc::new(|| {}), cl_api::QueueConfig::default())
            .expect("queue");
        let group = queue.group().expect("group");
        let session = cl_api::testing::stub_session(9);
        let (answer, rx) = tokio::sync::oneshot::channel::<Attachment>();
        let label = OrphanLabel {
            what: "orphan.card_attach",
            key: None,
        };
        let p = group
            .write(&session, label, move |_s| async move {
                rx.await.map_err(|_| ApiError::Internal)
            })
            .expect("queued");
        HeldUpload {
            _queue: queue,
            task: Task::Pending(p),
            answer,
        }
    }

    /// The file lands in the demo's draft, as the engine would have it.
    fn file_in(link: &Arc<CompanyLink>, to: AttachTo) -> Attachment {
        let mut demo = link.demo_state().expect("demo");
        crate::company::demo::checks::upload(&mut demo, to, "receipt.pdf", PDF).expect("upload")
    }

    fn files_on(link: &Arc<CompanyLink>, of: AttachTo) -> usize {
        let demo = link.demo_state().expect("demo");
        crate::company::demo::checks::attachments(&demo, of).len()
    }

    fn until_uploaded(s: &mut CardChargeScreen) {
        let end = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while s.attach.upload.is_some() {
            assert!(std::time::Instant::now() < end, "the upload never ended");
            std::thread::sleep(std::time::Duration::from_millis(5));
            s.poll_attach();
        }
    }

    /// Review B2: while a file is on its way to the draft no save goes out
    /// (a plain message says why, and Save & Close never closes); once it
    /// is there the save moves it onto the charge and the draft is done.
    #[test]
    fn the_save_waits_for_a_file_on_its_way_then_moves_it() {
        let (_d, link) = link();
        let mut s = screen(&link);
        let draft = DraftKey::new();
        s.attach.draft = Some(draft);
        let held = held_upload();
        s.attach.upload = Some(held.task);
        s.attach.upload_to = Some(AttachTo::Draft(draft));
        fill(&mut s, "12.00", &[("6250", "12.00")]);
        let before = posted(&link);
        for intent in [
            SaveIntent::Save,
            SaveIntent::SaveAndNew,
            SaveIntent::SaveAndClose,
        ] {
            assert!(!s.start_save(intent, None));
            assert_eq!(s.flow.notice, Some("card.attach.wait_save"));
        }
        assert_eq!(posted(&link), before, "nothing sent");
        assert!(s.is_dirty(), "the form is as it was");
        // A read under way or a list waiting holds the save too.
        assert!(s.attach.save_waits());

        let a = file_in(&link, AttachTo::Draft(draft));
        held.answer.send(a).expect("send");
        until_uploaded(&mut s);
        assert!(!s.attach.save_waits());
        assert!(s.start_save(SaveIntent::SaveAndNew, None), "{}", s.state());
        let id = s.flow.last.expect("saved").id;
        s.poll();
        assert_eq!(files_on(&link, AttachTo::Expense(id)), 1, "on the charge");
        assert_eq!(files_on(&link, AttachTo::Draft(draft)), 0);
        assert!(!s.attach.unsettled());
        assert_eq!(s.state()["attachments"]["count"], 0, "a new form");
    }

    /// Review B2: a file that reaches the draft after the save (it was sent
    /// while the save ran) still moves, after Save & New too; until then
    /// the window asks before closing.
    #[test]
    fn a_file_reaching_the_draft_after_the_save_moves_too() {
        let (_d, link) = link();
        let mut s = screen(&link);
        let draft = DraftKey::new();
        s.attach.draft = Some(draft);
        let first = file_in(&link, AttachTo::Draft(draft));
        s.list_arrived(vec![first]);
        fill(&mut s, "12.00", &[("6250", "12.00")]);
        assert!(s.start_save(SaveIntent::Save, None), "{}", s.state());
        let id = s.flow.last.expect("saved").id;
        s.poll();
        assert_eq!(files_on(&link, AttachTo::Expense(id)), 1);
        assert_eq!(s.attach.draft, None, "moved: the draft is done with");
        // A second file was on its way into the draft when the save ended.
        let held = held_upload();
        s.attach.upload = Some(held.task);
        s.attach.upload_to = Some(AttachTo::Draft(draft));
        s.new_form(true);
        assert!(s.attach.upload.is_some(), "a new form keeps it");
        assert!(s.attach.unsettled());
        assert!(s.dirty() && !s.is_dirty(), "the window asks before closing");
        let a = file_in(&link, AttachTo::Draft(draft));
        held.answer.send(a).expect("send");
        until_uploaded(&mut s);
        s.poll_attach();
        assert_eq!(
            files_on(&link, AttachTo::Expense(id)),
            2,
            "both on the charge"
        );
        assert_eq!(files_on(&link, AttachTo::Draft(draft)), 0);
        assert!(!s.attach.unsettled() && !s.dirty());
        assert_eq!(
            s.state()["attachments"]["count"],
            0,
            "not shown on the new form"
        );
    }

    /// Review B2: after a failed move the window asks before it closes; the
    /// prompt's Save tries the move again (listing the draft afresh) and
    /// keeps the window open.
    #[test]
    fn a_failed_move_asks_before_closing_and_save_tries_again() {
        let (_d, link) = link();
        let mut s = screen(&link);
        let id =
            cl_core::ids::TransactionId::parse("00000000-0000-4000-8000-0000000000aa").expect("id");
        let draft = DraftKey::new();
        let _ = file_in(&link, AttachTo::Draft(draft));
        let _ = file_in(&link, AttachTo::Draft(draft));
        s.attach.move_failed = Some(PendingMove {
            draft,
            to: AttachTo::Expense(id),
        });
        assert!(s.dirty() && !s.is_dirty());
        assert_eq!(s.state()["attachments"]["unsettled"], true);
        let mut windows = crate::windows::Windows::new();
        let w = windows.open_new(cl_model::CommandId::new(super::super::KIND), Box::new(s));
        assert_eq!(
            windows.request_close(w),
            crate::windows::CloseOutcome::Prompt,
            "it asks"
        );
        let progress = windows.answer_prompt(crate::windows::PromptAnswer::Save);
        assert!(progress.is_none());
        assert!(windows.get(w).is_some(), "Save keeps the window open");
        assert_eq!(files_on(&link, AttachTo::Expense(id)), 2, "moved on Save");
        assert!(!windows.get(w).expect("window").dirty());
        assert_eq!(
            windows.request_close(w),
            crate::windows::CloseOutcome::Closed
        );
    }

    /// The move lists the draft from the engine and links every file it
    /// finds there, not the list the form showed.
    #[test]
    fn the_move_lists_the_draft_from_the_engine() {
        use wiremock::matchers::{method, path, path_regex};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let draft = DraftKey::new();
        let id =
            cl_core::ids::TransactionId::parse("4b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01").expect("id");
        let row = |n: u8| {
            format!(
                r#"{{"id":"5c0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a0{n}","tenantId":"x","fileName":"a.pdf","filePath":"/p","fileSize":17,"mimeType":"application/pdf","attachableType":"draft","attachableId":"{draft}","ocrTotal":null,"createdAt":"2026-10-20T10:00:00.000Z"}}"#
            )
        };
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        rt.block_on(async {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/api/v1/attachments"))
                .respond_with(ResponseTemplate::new(200).set_body_raw(
                    format!(r#"{{"data":[{},{}],"total":2}}"#, row(1), row(2)),
                    "application/json",
                ))
                .mount(&server)
                .await;
            Mock::given(method("POST"))
                .and(path_regex(r"^/api/v1/attachments/[0-9a-f-]+/link$"))
                .respond_with(ResponseTemplate::new(200).set_body_raw(
                    format!(r#"{{"attachment":{}}}"#, row(1)),
                    "application/json",
                ))
                .mount(&server)
                .await;
            let session = cl_api::testing::stub_session(server.address().port());
            move_files(
                &session,
                PendingMove {
                    draft,
                    to: AttachTo::Expense(id),
                },
            )
            .await
            .expect("moved");
            let seen = server.received_requests().await.expect("recorded");
            let gets: Vec<_> = seen.iter().filter(|r| r.method.as_str() == "GET").collect();
            assert_eq!(gets.len(), 1);
            let q = gets[0].url.query().unwrap_or_default();
            assert!(q.contains(&format!("attachable_id={draft}")), "{q}");
            assert!(q.contains("attachable_type=draft"), "{q}");
            let links: Vec<String> = seen
                .iter()
                .filter(|r| r.method.as_str() == "POST")
                .map(|r| r.url.path().to_string())
                .collect();
            assert_eq!(links.len(), 2, "{links:?}");
            for r in seen.iter().filter(|r| r.method.as_str() == "POST") {
                let body = String::from_utf8_lossy(&r.body);
                assert!(body.contains("\"attachableType\":\"expense\""), "{body}");
            }
        });
    }
}
