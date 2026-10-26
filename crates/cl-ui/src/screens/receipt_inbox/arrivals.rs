//! The receipt inbox window over the company's intake folders (A01 C2,
//! C3): every file the folders hold, with its folder and where it stands,
//! a folder filter, health chips with **Set up…** / **Test**, a preview of
//! the selected file, and what turns an arrival into a bill, a check or a
//! bank transaction's attachment, or discards it.
//!
//! The arrival is already in the company (copied in as a draft attachment
//! when it stopped changing): **New bill** / **New check** open the form
//! with that very draft (no second upload); **Add to…** links it onto the
//! record picked. File and folder names are shown on screen only; the
//! control channel's state carries indexes, counts and states.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};

use cl_api::AttachTo;
use cl_company::intake::{DocKind, FileError, Health, IntakeFile, ReadFile};
use cl_core::ids::IntakeFolderId;
use cl_core::intake::IntakeKind;
use cl_model::CommandId;
use eframe::egui::{self, Margin, RichText, Ui};
use serde_json::{Value, json};

use super::intake::{self, FormKind, FormReady, Migration, Row, RowState};
use super::picker::{self, PickKind, PickRow};
use super::{ENTER_BILL, InboxDraft, STACK_WIDTH, WRITE_CHECK, date_of, size_text};
use crate::company::{CompanyEvent, CompanyLink};
use crate::i18n::{t, t_count, t_with};
use crate::screens::company::form;
use crate::screens::preferences::intake_folders;
use crate::theme::Tokens;
use crate::widgets::pdf_viewer::{DocSource, ViewerEvent};
use crate::windows::OpenPayload;

/// One folder as the window shows it.
#[derive(Clone)]
pub(super) struct RunInfo {
    id: IntakeFolderId,
    /// Owner data (shown on screen only).
    name: String,
    health: Health,
    watched: bool,
    keep_originals: bool,
    kind: IntakeKind,
}

/// What the window reads from the service each frame.
pub(super) struct Frame {
    loaded: bool,
    failed: bool,
    admin: bool,
    runs: Vec<RunInfo>,
    rows: Vec<Row>,
    queued: usize,
    badge: usize,
    processed: u32,
    discarded: u32,
    move_failures: u32,
    problems: usize,
    migration: Migration,
    problem: Option<&'static str>,
    ready_forms: Vec<FormReady>,
}

impl Frame {
    fn read(ctx: &egui::Context) -> Frame {
        let h = intake::handle(ctx);
        let mut s = h.lock();
        Frame {
            loaded: s.loaded(),
            failed: s.failed(),
            admin: s.is_admin(),
            runs: s
                .runs
                .iter()
                .map(|r| RunInfo {
                    id: r.folder.id,
                    name: r.folder.name.clone(),
                    health: r.health(),
                    watched: r.watched(),
                    keep_originals: r.folder.keep_originals,
                    kind: r.folder.default_kind,
                })
                .collect(),
            rows: s.rows(),
            queued: s.queued(),
            badge: s.badge(),
            processed: s.processed,
            discarded: s.discarded,
            move_failures: s.move_failures,
            problems: s.problems(),
            migration: s.migration,
            problem: s.problem,
            ready_forms: std::mem::take(&mut s.ready_forms),
        }
    }
}

/// The window's own state in intake mode.
#[derive(Default)]
pub(super) struct Arrivals {
    /// The folder filter (`None`: all folders).
    filter: Option<IntakeFolderId>,
    /// The selected file (its path hash).
    selected: Option<String>,
    reading: Option<(String, Receiver<Result<ReadFile, FileError>>)>,
    loaded: Option<(IntakeFile, ReadFile)>,
    read_problem: Option<&'static str>,
    confirm_discard: bool,
    picker: Option<picker::Picker>,
    problem: Option<&'static str>,
    notice: Option<String>,
    /// The demo laid its folders out for this window.
    opened: bool,
    /// The last frame's view (for `state()`).
    view: Value,
}

fn key_of(f: &IntakeFile) -> String {
    cl_settings::inbox_state::path_hash(&f.path)
}

fn kind_label(k: DocKind) -> &'static str {
    match k {
        DocKind::Pdf => "PDF",
        DocKind::Png => "PNG",
        DocKind::Jpeg => "JPEG",
        DocKind::Tiff => "TIFF",
        DocKind::Heic => "HEIC",
    }
}

fn state_key(s: RowState) -> &'static str {
    match s {
        RowState::Settling => "inbox.state.settling",
        RowState::Waiting => "inbox.state.waiting",
        RowState::Copying => "inbox.state.copying",
        RowState::Arrived => "inbox.state.arrived",
        RowState::OnForm => "inbox.state.on_form",
        RowState::Duplicate => "inbox.state.duplicate",
        RowState::Moving => "inbox.state.moving",
        RowState::BringIn => "inbox.state.bring_in",
        RowState::TooLarge => "inbox.state.too_large.intake",
        RowState::NotSupported => "inbox.state.heic",
        RowState::OnlineOnly => "inbox.state.online_only",
        RowState::LinkedFile => "inbox.state.linked_file",
        RowState::Refused => "inbox.state.refused",
    }
}

/// True when a row can be picked (previewed and acted on).
fn pickable(s: RowState) -> bool {
    matches!(
        s,
        RowState::Arrived | RowState::OnForm | RowState::Duplicate | RowState::BringIn
    )
}

/// "Bills · PDF · 1.2 MB · 11/03/2026 · Copied in".
fn row_line(run: Option<&RunInfo>, r: &Row) -> String {
    let mut parts = Vec::new();
    if let Some(run) = run {
        parts.push(run.name.clone());
    }
    parts.push(kind_label(r.file.kind).to_owned());
    parts.push(size_text(r.file.size));
    if let Some(d) = date_of(r.file.modified_ms) {
        parts.push(crate::shell::us_date(d));
    }
    parts.push(t(state_key(r.state)));
    parts.join(" \u{b7} ")
}

/// Whether the folder's chip offers Set up… (this computer must confirm
/// it) rather than Test (look again).
fn needs_set_up(h: Health) -> bool {
    matches!(
        h,
        Health::NotConfirmed | Health::Moved | Health::MappedByAnother
    )
}

fn open_document_intake(link: &CompanyLink) {
    crate::screens::preferences::open_at(crate::screens::preferences::Category::ReceiptInbox);
    link.push(CompanyEvent::OpenWindow(CommandId::new(
        crate::screens::preferences::COMMAND,
    )));
}

impl super::ReceiptInboxScreen {
    /// One frame in intake mode.
    pub(super) fn ui_intake(&mut self, ui: &mut Ui) {
        let tokens = Tokens::of(ui.ctx());
        let ctx = ui.ctx().clone();
        if !self.arrivals.opened {
            self.arrivals.opened = true;
            let h = intake::handle(&ctx);
            let mut s = h.lock();
            s.demo_window_opened(&self.link);
            s.poke();
        }
        super::tick(&ctx, &self.link, self.tenant(), self.today, true);
        let f = Frame::read(&ctx);
        self.poll_intake_read(&f);
        self.open_ready_forms(&ctx, &f);
        if let Some(sel) = &self.arrivals.selected
            && !f
                .rows
                .iter()
                .any(|r| key_of(&r.file) == *sel && pickable(r.state))
        {
            // Handled, or gone from the folder.
            self.clear_intake_selection();
        }
        if let Some(p) = self.arrivals.picker.as_mut() {
            p.poll();
        }
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
                self.draw_intake(ui, &tokens, &f);
            });
        self.arrivals.view = self.intake_state(&f);
        if self.arrivals.reading.is_some()
            || self
                .arrivals
                .picker
                .as_ref()
                .is_some_and(picker::Picker::busy)
        {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    /// The control channel's view in intake mode.
    pub(super) fn intake_state_json(&self) -> Value {
        if self.arrivals.view.is_null() {
            // Not drawn yet.
            return json!({
                "screen": "receipt_inbox",
                "mode": "intake",
                "date": self.today.to_string(),
                "folder": "loading",
            });
        }
        self.arrivals.view.clone()
    }

    fn clear_intake_selection(&mut self) {
        let a = &mut self.arrivals;
        a.selected = None;
        a.reading = None;
        a.loaded = None;
        a.read_problem = None;
        a.confirm_discard = false;
        a.picker = None;
        self.viewer.clear();
    }

    fn select_row(&mut self, ctx: &egui::Context, r: &Row) {
        let key = key_of(&r.file);
        if self.arrivals.selected.as_ref() == Some(&key) {
            return;
        }
        self.clear_intake_selection();
        self.arrivals.selected = Some(key.clone());
        let rx = {
            let h = intake::handle(ctx);
            let s = h.lock();
            s.runs
                .get(r.run)
                .and_then(|run| run.read(r.file.path.clone()))
        };
        match rx {
            Some(rx) => self.arrivals.reading = Some((key, rx)),
            None => self.arrivals.read_problem = Some("inbox.read.failed"),
        }
    }

    fn poll_intake_read(&mut self, f: &Frame) {
        let Some((key, rx)) = &self.arrivals.reading else {
            return;
        };
        let result = match rx.try_recv() {
            Ok(r) => r,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err(FileError::Io),
        };
        let key = key.clone();
        self.arrivals.reading = None;
        if self.arrivals.selected.as_ref() != Some(&key) {
            return;
        }
        let Some(row) = f.rows.iter().find(|r| key_of(&r.file) == key) else {
            return;
        };
        match result {
            Ok(read) if read.size == row.file.size && read.modified_ms == row.file.modified_ms => {
                self.viewer.open(DocSource {
                    name: row.file.name.clone(),
                    bytes: Arc::from(read.bytes.clone()),
                });
                self.arrivals.loaded = Some((row.file.clone(), read));
            }
            Ok(_) | Err(FileError::Unstable | FileError::Changed) => {
                self.arrivals.read_problem = Some("inbox.read.changed");
            }
            Err(e) => self.arrivals.read_problem = Some(super::read_problem_key(e)),
        }
    }

    /// The loaded file's bytes, when they are `entry`'s file.
    fn source_for(&self, row: &Row) -> Option<DocSource> {
        let (file, read) = self.arrivals.loaded.as_ref()?;
        (key_of(file) == key_of(&row.file)).then(|| DocSource {
            name: file.name.clone(),
            bytes: Arc::from(read.bytes.clone()),
        })
    }

    fn push_form(&self, kind: FormKind, draft: InboxDraft) {
        let cmd = match kind {
            FormKind::Bill => ENTER_BILL,
            FormKind::Check => WRITE_CHECK,
        };
        self.link.push(CompanyEvent::OpenWith(
            CommandId::new(cmd),
            Box::new(OpenPayload::InboxDraft(draft)),
        ));
    }

    fn keep_of(f: &Frame, row: &Row) -> bool {
        f.runs.get(row.run).is_some_and(|r| r.keep_originals)
    }

    fn new_form(&mut self, ctx: &egui::Context, f: &Frame, row: &Row, kind: FormKind) {
        self.arrivals.problem = None;
        self.arrivals.notice = None;
        let (Some(entry), Some(file)) = (row.entry.clone(), self.source_for(row)) else {
            return;
        };
        let r = intake::handle(ctx)
            .lock()
            .claim_for_form(&self.link, &entry, kind);
        match r {
            Ok(Some((draft, attachment, claim))) => {
                self.push_form(
                    kind,
                    InboxDraft {
                        draft,
                        attachment,
                        file,
                        claim: Some(claim),
                    },
                );
                self.arrivals.notice = Some(t(if Self::keep_of(f, row) {
                    "inbox.notice.draft.keep"
                } else {
                    "inbox.notice.draft.intake"
                }));
            }
            Ok(None) => self.arrivals.notice = Some(t("inbox.notice.linking")),
            Err(k) => self.arrivals.problem = Some(k),
        }
    }

    /// Forms whose P17 upload was linked into a draft just now.
    fn open_ready_forms(&mut self, ctx: &egui::Context, f: &Frame) {
        for ready in &f.ready_forms {
            let row = f
                .rows
                .iter()
                .find(|r| r.entry.as_ref() == Some(&ready.entry));
            let Some(file) = row.and_then(|r| self.source_for(r)) else {
                self.arrivals.problem = Some("inbox.read.changed");
                continue;
            };
            let claim = intake::handle(ctx).lock().claim(ready.attachment);
            self.push_form(
                ready.kind,
                InboxDraft {
                    draft: ready.draft,
                    attachment: ready.attachment,
                    file,
                    claim: Some(claim),
                },
            );
            self.arrivals.notice = Some(t("inbox.notice.draft.intake"));
        }
    }

    // -----------------------------------------------------------------
    // Drawing.

    fn draw_intake(&mut self, ui: &mut Ui, tokens: &Tokens, f: &Frame) {
        // The window title's size, as Preferences (22/600, UI check S8).
        ui.heading(t("inbox.title"));
        ui.horizontal_wrapped(|ui| {
            if ui
                .button(t("inbox.check_now"))
                .on_hover_text(t("inbox.check_now.tip"))
                .clicked()
            {
                intake::handle(ui.ctx()).lock().poke();
            }
            if ui
                .button(t("inbox.setup_folders"))
                .on_hover_text(t("inbox.setup_folders.tip"))
                .clicked()
            {
                open_document_intake(&self.link);
            }
            if f.runs.len() > 1 {
                self.filter_combo(ui, f);
            }
        });
        self.health_chips(ui, tokens, f);
        ui.add_space(6.0);
        if self.intake_empty_state(ui, tokens, f) {
            return;
        }
        self.intake_messages(ui, tokens, f);
        let rows: Vec<&Row> = f
            .rows
            .iter()
            .filter(|r| {
                self.arrivals
                    .filter
                    .is_none_or(|id| f.runs.get(r.run).is_some_and(|x| x.id == id))
            })
            .collect();
        if rows.is_empty() {
            form::card(ui, tokens, |ui| {
                ui.label(
                    RichText::new(t("inbox.empty.nothing_new"))
                        .strong()
                        .color(tokens.text_primary),
                );
                form::hint(ui, tokens, &t("inbox.empty.nothing_new.hint"));
            });
            return;
        }
        let stacked = ui.available_width() < STACK_WIDTH;
        if stacked {
            self.intake_list(ui, tokens, f, &rows);
            ui.add_space(8.0);
            self.intake_detail(ui, tokens, f);
        } else {
            let list_width = (ui.available_width() * 0.4).clamp(260.0, 420.0);
            ui.horizontal_top(|ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(list_width, 0.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| self.intake_list(ui, tokens, f, &rows),
                );
                ui.add_space(12.0);
                ui.vertical(|ui| self.intake_detail(ui, tokens, f));
            });
        }
    }

    fn filter_combo(&mut self, ui: &mut Ui, f: &Frame) {
        let current = self
            .arrivals
            .filter
            .and_then(|id| f.runs.iter().find(|r| r.id == id))
            .map_or_else(|| t("inbox.filter.all"), |r| r.name.clone());
        let l = ui.label(t("inbox.filter"));
        egui::ComboBox::from_id_salt("inbox-folder-filter")
            .selected_text(current)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut self.arrivals.filter, None, t("inbox.filter.all"));
                for r in &f.runs {
                    ui.selectable_value(&mut self.arrivals.filter, Some(r.id), &r.name);
                }
            })
            .response
            .labelled_by(l.id);
    }

    /// A chip per folder that isn't fine, with what fixes it.
    fn health_chips(&mut self, ui: &mut Ui, tokens: &Tokens, f: &Frame) {
        let unwell: Vec<&RunInfo> = f.runs.iter().filter(|r| r.health != Health::Ok).collect();
        if unwell.is_empty() {
            return;
        }
        ui.add_space(4.0);
        for r in unwell {
            let attention = r.health.needs_attention();
            let (fill, text) = if attention {
                (tokens.warn_tint, tokens.warn_text)
            } else {
                (tokens.neutral_chip, tokens.neutral_chip_text)
            };
            ui.horizontal_wrapped(|ui| {
                egui::Frame::new()
                    .fill(fill)
                    .corner_radius(10.0)
                    .inner_margin(Margin::symmetric(8, 3))
                    .show(ui, |ui| {
                        ui.add(
                            egui::Label::new(
                                RichText::new(t_with(
                                    "inbox.chip",
                                    &[
                                        ("name", &r.name),
                                        ("health", &t(intake_folders::health_key(r.health).0)),
                                    ],
                                ))
                                .color(text),
                            )
                            .wrap(),
                        );
                    });
                if !attention {
                    return;
                }
                if needs_set_up(r.health) {
                    if ui
                        .button(t("inbox.chip.set_up"))
                        .on_hover_text(t("inbox.chip.set_up.tip"))
                        .clicked()
                    {
                        open_document_intake(&self.link);
                    }
                } else if ui
                    .button(t("inbox.chip.test"))
                    .on_hover_text(t("inbox.chip.test.tip"))
                    .clicked()
                {
                    let h = intake::handle(ui.ctx());
                    let s = h.lock();
                    if let Some(run) = s.runs.iter().find(|x| x.folder.id == r.id) {
                        run.poke();
                    }
                }
            });
        }
    }

    /// Loading, no folders. True when one was drawn (nothing else is).
    fn intake_empty_state(&mut self, ui: &mut Ui, tokens: &Tokens, f: &Frame) -> bool {
        let (title, hint, add) = if f.failed {
            ("inbox.empty.failed", None, false)
        } else if !f.loaded {
            ("inbox.empty.loading_folders", None, false)
        } else if f.runs.is_empty() {
            (
                "inbox.empty.no_folders",
                Some(if f.admin {
                    "inbox.empty.no_folders.hint"
                } else {
                    "inbox.empty.ask_admin"
                }),
                f.admin,
            )
        } else {
            return false;
        };
        form::card(ui, tokens, |ui| {
            ui.label(RichText::new(t(title)).strong().color(tokens.text_primary));
            if let Some(h) = hint {
                form::hint(ui, tokens, &t(h));
            }
            if add && crate::widgets::primary_button(ui, &t("inbox.add_folder")).clicked() {
                open_document_intake(&self.link);
            }
        });
        true
    }

    fn intake_messages(&self, ui: &mut Ui, tokens: &Tokens, f: &Frame) {
        if f.queued > 0 {
            form::progress_line(ui, tokens, &t_count("inbox.copying", f.queued, &[]));
        }
        if let Some(n) = &self.arrivals.notice {
            ui.add(egui::Label::new(RichText::new(n).color(tokens.text_secondary)).wrap());
        }
        if let Some(key) = self.arrivals.problem.or(f.problem) {
            form::inline_warning(ui, tokens, &t(key));
        }
        if f.move_failures > 0 {
            form::hint(ui, tokens, &t("inbox.move_failed.intake"));
        }
        if f.migration == Migration::Blocked {
            form::hint(ui, tokens, &t("inbox.migration.blocked"));
        }
    }

    fn intake_list(&mut self, ui: &mut Ui, tokens: &Tokens, f: &Frame, rows: &[&Row]) {
        let waiting = rows
            .iter()
            .filter(|r| matches!(r.state, RowState::Arrived | RowState::Duplicate))
            .count();
        ui.label(
            RichText::new(t_count("inbox.count.waiting", waiting, &[]))
                .strong()
                .color(tokens.text_primary),
        );
        let mut pick: Option<Row> = None;
        for r in rows {
            let key = key_of(&r.file);
            let selected = self.arrivals.selected.as_ref() == Some(&key);
            let usable = pickable(r.state);
            ui.add_space(2.0);
            // A file that can't be picked (HEIC, too large, still
            // arriving) stays a disabled button, but isn't faded: its name
            // keeps at least 4.5:1 in both themes (UI check S6).
            let resp = ui
                .scope(|ui| {
                    if !usable {
                        ui.style_mut().visuals.disabled_alpha = 1.0;
                    }
                    ui.add_enabled(
                        usable,
                        egui::Button::selectable(
                            selected,
                            RichText::new(&r.file.name).color(if usable {
                                tokens.text_primary
                            } else {
                                tokens.text_secondary
                            }),
                        ),
                    )
                })
                .inner;
            if resp.clicked() {
                pick = Some((*r).clone());
            }
            ui.label(
                RichText::new(row_line(f.runs.get(r.run), r))
                    .size(12.0)
                    .color(tokens.text_secondary),
            );
        }
        if let Some(r) = pick {
            self.select_row(ui.ctx(), &r);
        }
    }

    fn intake_detail(&mut self, ui: &mut Ui, tokens: &Tokens, f: &Frame) {
        let Some(key) = self.arrivals.selected.clone() else {
            form::hint(ui, tokens, &t("inbox.pick_a_file"));
            return;
        };
        let Some(row) = f.rows.iter().find(|r| key_of(&r.file) == key).cloned() else {
            return;
        };
        if self.arrivals.reading.is_some() {
            form::progress_line(ui, tokens, &t("inbox.reading"));
        }
        if let Some(k) = self.arrivals.read_problem {
            form::inline_warning(ui, tokens, &t(k));
        }
        if self.arrivals.loaded.is_some() {
            let event = super::preview_pane(ui, &mut self.viewer, tokens);
            if event == ViewerEvent::OpenFullSize
                && let Some(src) = self.viewer.source()
            {
                self.link.push(CompanyEvent::OpenWith(
                    CommandId::new(crate::screens::document_view::KIND),
                    Box::new(OpenPayload::Document(src.clone())),
                ));
            }
        }
        ui.add_space(6.0);
        match row.state {
            RowState::OnForm => form::hint(ui, tokens, &t("inbox.on_form.hint")),
            RowState::Duplicate => {
                let when = row
                    .seen_at
                    .and_then(date_of)
                    .map(crate::shell::us_date)
                    .unwrap_or_default();
                form::hint(
                    ui,
                    tokens,
                    &t_with("inbox.duplicate.detail", &[("date", &when)]),
                );
            }
            RowState::BringIn => form::hint(ui, tokens, &t("inbox.bring_in.hint")),
            _ => {}
        }
        if self.arrivals.confirm_discard {
            self.discard_confirm(ui, tokens, f, &row);
        } else if self.arrivals.picker.is_some() {
            self.intake_picker(ui, tokens, f, &row);
        } else {
            self.intake_actions(ui, f, &row);
        }
    }

    fn intake_actions(&mut self, ui: &mut Ui, f: &Frame, row: &Row) {
        let ctx = ui.ctx().clone();
        let keep = Self::keep_of(f, row);
        match row.state {
            RowState::Arrived => {
                let ready = self.source_for(row).is_some();
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
                        let enabled = ready || matches!(n, 1 | 3 | 4);
                        let r = if n == 0 {
                            ui.add_enabled_ui(enabled, |ui| {
                                crate::widgets::primary_button(ui, &t(label))
                            })
                            .inner
                        } else {
                            ui.add_enabled(enabled, egui::Button::new(t(label)))
                        };
                        if r.on_hover_text(t(tip)).clicked() {
                            chosen = Some(n);
                        }
                    }
                    let tip = if keep {
                        "inbox.action.discard.keep.tip"
                    } else {
                        "inbox.action.discard.tip"
                    };
                    if ui
                        .button(t("inbox.action.discard"))
                        .on_hover_text(t(tip))
                        .clicked()
                    {
                        chosen = Some(5);
                    }
                });
                match chosen {
                    Some(0) => self.new_form(&ctx, f, row, FormKind::Bill),
                    Some(1) => self.arrivals.picker = Some(picker::Picker::new(PickKind::Bill)),
                    Some(2) => self.new_form(&ctx, f, row, FormKind::Check),
                    Some(3) => self.arrivals.picker = Some(picker::Picker::new(PickKind::Check)),
                    Some(4) => {
                        self.arrivals.picker = Some(picker::Picker::new(PickKind::BankTxn));
                    }
                    Some(5) => self.arrivals.confirm_discard = true,
                    _ => {}
                }
            }
            RowState::Duplicate => {
                let mut discard = false;
                let mut copy = false;
                ui.horizontal_wrapped(|ui| {
                    discard = ui
                        .button(t("inbox.action.discard"))
                        .on_hover_text(t(if keep {
                            "inbox.action.discard.keep.tip"
                        } else {
                            "inbox.action.discard.tip"
                        }))
                        .clicked();
                    copy = ui
                        .button(t("inbox.action.copy_anyway"))
                        .on_hover_text(t("inbox.action.copy_anyway.tip"))
                        .clicked();
                });
                if discard {
                    self.arrivals.confirm_discard = true;
                } else if copy && let Some(entry) = &row.entry {
                    self.arrivals.notice = None;
                    match intake::handle(&ctx).lock().copy_anyway(entry) {
                        Ok(()) => self.arrivals.notice = Some(t("inbox.notice.copy_anyway")),
                        Err(k) => self.arrivals.problem = Some(k),
                    }
                }
            }
            RowState::BringIn => {
                let bring = ui
                    .button(t("inbox.action.bring_in"))
                    .on_hover_text(t("inbox.action.bring_in.tip"))
                    .clicked();
                if bring {
                    intake::handle(&ctx).lock().bring_in(&row.file);
                }
            }
            _ => {}
        }
    }

    fn discard_confirm(&mut self, ui: &mut Ui, tokens: &Tokens, f: &Frame, row: &Row) {
        let keep = Self::keep_of(f, row);
        let mut yes = false;
        let mut no = false;
        form::card(ui, tokens, |ui| {
            form::inline_warning(
                ui,
                tokens,
                &t(if keep {
                    "inbox.discard.ask.keep"
                } else {
                    "inbox.discard.ask"
                }),
            );
            ui.horizontal_wrapped(|ui| {
                yes = ui.button(t("inbox.discard.yes")).clicked();
                no = ui.button(t("inbox.discard.no")).clicked();
            });
        });
        if no {
            self.arrivals.confirm_discard = false;
        } else if yes {
            self.arrivals.confirm_discard = false;
            self.arrivals.problem = None;
            let Some(entry) = &row.entry else {
                return;
            };
            let r = intake::handle(ui.ctx()).lock().discard(&self.link, entry);
            match r {
                Ok(()) => {
                    self.arrivals.notice = Some(t(if keep {
                        "inbox.notice.discarded.keep"
                    } else {
                        "inbox.notice.discarded"
                    }));
                }
                Err(k) => self.arrivals.problem = Some(k),
            }
        }
    }

    fn intake_picker(&mut self, ui: &mut Ui, tokens: &Tokens, f: &Frame, row: &Row) {
        let link = Arc::clone(&self.link);
        let Some(mut p) = self.arrivals.picker.take() else {
            return;
        };
        p.refresh(&link, self.group.as_ref());
        let (attach, cancel) = super::picker_card(ui, tokens, &mut p, false);
        self.arrivals.picker = Some(p);
        if cancel {
            self.arrivals.picker = None;
            return;
        }
        let Some((kind, pick)) = attach else {
            return;
        };
        self.attach_to(ui.ctx(), f, row, kind, &pick);
    }

    fn attach_to(
        &mut self,
        ctx: &egui::Context,
        f: &Frame,
        row: &Row,
        kind: PickKind,
        pick: &PickRow,
    ) {
        self.arrivals.problem = None;
        self.arrivals.notice = None;
        let Some(entry) = &row.entry else {
            return;
        };
        let target: AttachTo = pick.target();
        let r = intake::handle(ctx).lock().add_to(&self.link, entry, target);
        match r {
            Ok(()) => {
                let key = if Self::keep_of(f, row) {
                    "inbox.notice.attached.keep"
                } else {
                    "inbox.notice.attached.intake"
                };
                self.arrivals.notice =
                    Some(t_with(key, &[("record", &super::record_text(kind, pick))]));
                self.arrivals.picker = None;
            }
            Err(k) => self.arrivals.problem = Some(k),
        }
    }

    fn intake_state(&self, f: &Frame) -> Value {
        let mut counts: BTreeMap<&'static str, usize> = BTreeMap::new();
        for r in &f.rows {
            *counts.entry(r.state.as_str()).or_default() += 1;
        }
        let n = |s: RowState| counts.get(s.as_str()).copied().unwrap_or(0);
        let folders: Vec<Value> = f
            .runs
            .iter()
            .enumerate()
            .map(|(i, r)| {
                json!({
                    "index": i,
                    "health": r.health.as_str(),
                    "watched": r.watched,
                    "keep_originals": r.keep_originals,
                    "kind": match r.kind {
                        IntakeKind::Bills => "bills",
                        IntakeKind::Receipts => "receipts",
                        IntakeKind::Auto => "auto",
                    },
                    "listed": f.rows.iter().filter(|x| x.run == i).count(),
                })
            })
            .collect();
        let selected = self
            .arrivals
            .selected
            .as_ref()
            .and_then(|k| f.rows.iter().find(|r| key_of(&r.file) == *k));
        let folder = if f.failed {
            "failed"
        } else if !f.loaded {
            "loading"
        } else if f.runs.is_empty() {
            "none"
        } else {
            "ok"
        };
        json!({
            "screen": "receipt_inbox",
            "mode": "intake",
            "date": self.today.to_string(),
            "folder": folder,
            "folders": folders,
            "filter": self.arrivals.filter.and_then(|id| f.runs.iter().position(|r| r.id == id)),
            "admin": f.admin,
            "listed": f.rows.len(),
            "settling": n(RowState::Settling),
            "waiting": n(RowState::Waiting),
            "copying": n(RowState::Copying),
            "arrived": n(RowState::Arrived),
            "on_form": n(RowState::OnForm),
            "duplicate": n(RowState::Duplicate),
            "moving": n(RowState::Moving),
            "bring_in": n(RowState::BringIn),
            "too_large": n(RowState::TooLarge),
            "not_supported": n(RowState::NotSupported),
            "online_only": n(RowState::OnlineOnly),
            "refused": n(RowState::Refused),
            "queued": f.queued,
            "badge": f.badge,
            "processed": f.processed,
            "discarded": f.discarded,
            "move_failures": f.move_failures,
            "problems": f.problems,
            "migration": f.migration.as_str(),
            "selected": selected.is_some(),
            "selected_state": selected.map(|r| r.state.as_str()),
            "selected_kind": selected.map(|r| r.file.kind.as_str()),
            "selected_folder": selected.map(|r| r.run),
            "loaded": self.arrivals.loaded.is_some(),
            "reading": self.arrivals.reading.is_some(),
            "preview": self.viewer.state_json(),
            "picker": self.arrivals.picker.as_ref().map(|p| json!({
                "kind": p.kind.as_str(),
                "rows": p.rows.len(),
                "loaded": p.loaded,
                "accounts": p.accounts.len(),
            })),
            "confirm_discard": self.arrivals.confirm_discard,
            "problem": self.arrivals.problem.or(f.problem),
            "read_problem": self.arrivals.read_problem,
            "notice": self.arrivals.notice.is_some(),
        })
    }
}

#[cfg(test)]
mod tests;
