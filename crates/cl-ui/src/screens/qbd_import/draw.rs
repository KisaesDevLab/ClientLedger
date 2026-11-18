//! Drawing the import wizard (mockup `QBDImport`): the step list on the
//! left with "Not brought over" and "Undo this import" at its foot, the
//! step on the right, Back and the step's button at the bottom right; the
//! History tab; the undo question; the closing-date password. Every input
//! is labelled (AccessKit, control channel).

use cl_api::imports::{ImportBatch, ImportRefusal};
use cl_model::CommandId;
use cl_model::imports::history::{self, Entry};
use cl_model::imports::wizard::{Blocker, HistoryDepth, Need, Source, Step};
use eframe::egui::{self, Margin, RichText, Ui};

use super::{ImportScreen, Running, Trouble, View, show_date};
use crate::company::CompanyEvent;
use crate::forms::closing_prompt::ClosingClick;
use crate::i18n::{t, t_with};
use crate::layout::{Body, FixedFrame, FrameOpts};
use crate::theme::Tokens;

fn note(ui: &mut Ui, tokens: &Tokens, text: &str) {
    ui.add(egui::Label::new(RichText::new(text).color(tokens.text_secondary)).wrap());
}

fn warn(ui: &mut Ui, tokens: &Tokens, text: &str) {
    ui.add(egui::Label::new(RichText::new(text).color(tokens.warn_text)).wrap());
}

fn shortcut_hint(id: &str) -> String {
    cl_model::commands::find(id)
        .and_then(|c| c.shortcut)
        .map(crate::input::shortcut_text)
        .unwrap_or_default()
}

/// A step's name in the list (the first step says what the source is).
pub(crate) fn step_name(source: Source, s: Step) -> String {
    if s == Step::Source {
        let which = if source.is_conversion() {
            "qbd"
        } else {
            "other"
        };
        return t(&format!("imports.step.source.{which}"));
    }
    t(&format!("imports.step.{}", s.key()))
}

/// A report's name (`imports.slot.<key>.name`).
pub(crate) fn slot_name(key: &str) -> String {
    t(&format!("imports.slot.{key}.name"))
}

/// What keeps the owner on a step, in plain words.
pub(crate) fn blocker_text(b: &Blocker) -> String {
    match b {
        Blocker::Dates(p) => t(&format!("imports.dates.problem.{}", p.key())),
        Blocker::MissingFile { slot }
        | Blocker::NotUploaded { slot }
        | Blocker::RowProblems { slot }
        | Blocker::OneFileOnly { slot } => t_with(
            &format!("imports.blocker.{}", b.key()),
            &[("report", &slot_name(slot))],
        ),
        Blocker::WrongDate { slot, want } => t_with(
            "imports.blocker.wrong_date",
            &[("report", &slot_name(slot)), ("date", &show_date(*want))],
        ),
        _ => t(&format!("imports.blocker.{}", b.key())),
    }
}

/// A batch as History's model row.
pub(crate) fn entry_of(b: &ImportBatch) -> Entry {
    let source = Source::ALL
        .into_iter()
        .find(|s| s.engine_source() == b.source.as_wire())
        .map_or("generic", Source::key);
    Entry {
        id: b.id.to_string(),
        source,
        status: history::BatchStatus::from_wire(b.status.as_wire())
            .unwrap_or(history::BatchStatus::Draft),
        created: b
            .created_at
            .as_deref()
            .and_then(|s| cl_core::CalendarDate::parse_wire(s.get(..10)?).ok()),
        conversion_date: b.conversion_date,
        verified: b.is_verified(),
        backup_ref: b.backup_ref.clone(),
    }
}

impl ImportScreen {
    pub(super) fn draw(&mut self, ui: &mut Ui) {
        let tokens = Tokens::of(ui.ctx());
        egui::Frame::new()
            .fill(tokens.surface_window)
            .inner_margin(Margin::symmetric(20, 12))
            .show(ui, |ui| {
                let mut frame = FixedFrame::begin(ui, "qbd_import", FrameOpts::default());
                frame.top(|ui| self.top_ui(ui, &tokens));
                frame.body(Body::Scroll, |ui| {
                    ui.set_width(ui.available_width());
                    self.trouble_ui(ui, &tokens);
                    if !self.allowed() || !self.supported() {
                        return;
                    }
                    // D6: the questions sit above the step (below its
                    // full-height divider they were off the screen).
                    self.undo_question_ui(ui, &tokens);
                    self.closed_question_ui(ui, &tokens);
                    match self.view {
                        View::History => self.history_ui(ui, &tokens),
                        View::Import => {
                            ui.horizontal_top(|ui| {
                                ui.vertical(|ui| {
                                    ui.set_width(230.0);
                                    self.rail_ui(ui, &tokens);
                                });
                                ui.separator();
                                ui.vertical(|ui| self.step_ui(ui, &tokens));
                            });
                        }
                    }
                });
                frame.bottom(|ui| {
                    ui.add_space(4.0);
                    self.buttons_ui(ui);
                });
                let _ = frame.end();
            });
        if self.closing.dialog_open() {
            match self.closing.window(ui.ctx(), &tokens, "imports") {
                ClosingClick::Submit => self.submit_password(),
                ClosingClick::Cancel => self.cancel_password(),
                ClosingClick::None => {}
            }
        }
    }

    fn top_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        ui.horizontal_wrapped(|ui| {
            ui.add(egui::Label::new(
                RichText::new(self.title_text())
                    .size(22.0)
                    .strong()
                    .color(tokens.text_primary),
            ));
            ui.add_space(12.0);
            if ui
                .selectable_label(self.view == View::Import, t("imports.tab.import"))
                .clicked()
            {
                self.view = View::Import;
            }
            if ui
                .selectable_label(self.view == View::History, t("imports.tab.history"))
                .clicked()
            {
                self.show_history();
            }
            if let Some(b) = self.state.as_ref().filter(|b| b.batch.backup_ref.is_some()) {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let when = b
                        .batch
                        .committed_at
                        .as_deref()
                        .and_then(|s| cl_core::CalendarDate::parse_wire(s.get(..10)?).ok())
                        .map(show_date)
                        .unwrap_or_default();
                    note(
                        ui,
                        tokens,
                        &t_with("imports.backup_taken", &[("date", &when)]),
                    );
                });
            }
        });
    }

    fn title_text(&self) -> String {
        let which = if self.wizard.source.is_conversion() {
            "qbd"
        } else {
            "other"
        };
        t(&format!("imports.title.{which}"))
    }

    fn rail_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        ui.label(
            RichText::new(self.title_text().to_uppercase())
                .size(12.0)
                .strong()
                .color(tokens.text_tertiary),
        );
        ui.add_space(6.0);
        let steps: Vec<Step> = self
            .wizard
            .source
            .steps()
            .iter()
            .copied()
            .filter(|s| *s != Step::Done)
            .collect();
        let at = steps.iter().position(|s| *s == self.wizard.step);
        for (i, s) in steps.iter().enumerate() {
            let done = at.is_none_or(|a| i < a) || self.wizard.step == Step::Done;
            let current = Some(i) == at;
            let name = step_name(self.wizard.source, *s);
            // A finished step: a drawn circled check (the bundled fonts have
            // no check glyph, D6); the others their number.
            let text = if done && !current {
                name.clone()
            } else {
                format!("{}  {name}", i + 1)
            };
            let rich = if current {
                RichText::new(text).strong().color(tokens.accent)
            } else {
                RichText::new(text).color(tokens.text_secondary)
            };
            let r = ui
                .horizontal(|ui| {
                    if done && !current {
                        crate::widgets::circled_check(
                            ui,
                            14.0,
                            tokens.accent,
                            tokens.surface_window,
                            &t_with("imports.step.finished", &[("step", &name)]),
                        );
                    }
                    ui.add(egui::Label::new(rich))
                })
                .inner;
            if current {
                r.widget_info(|| {
                    egui::WidgetInfo::labeled(
                        egui::Role::Label,
                        true,
                        t_with(
                            "imports.step.current",
                            &[("step", &step_name(self.wizard.source, *s))],
                        ),
                    )
                });
            }
        }
        ui.add_space(24.0);
        note(ui, tokens, &t("imports.not_brought_over"));
        if self.wizard.imported
            && !self.undone
            && self.allowed()
            && let Some(id) = self.batch
            && ui
                .add_enabled(!self.locked(), egui::Link::new(t("imports.undo.link")))
                .clicked()
        {
            self.ask_undo(id);
        }
    }

    fn step_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        ui.add(egui::Label::new(
            RichText::new(step_name(self.wizard.source, self.wizard.step))
                .size(20.0)
                .strong()
                .color(tokens.text_primary),
        ));
        ui.add_space(6.0);
        match self.wizard.step {
            Step::Source => self.source_ui(ui, tokens),
            Step::Files => self.files_ui(ui, tokens),
            Step::Mapping => super::mapping::mapping_ui(self, ui, tokens),
            Step::Dates => self.dates_ui(ui, tokens),
            Step::Import => self.import_ui(ui, tokens),
            Step::TieOut => super::tieout::tieout_ui(self, ui, tokens),
            Step::SpotCheck => super::spot_check::spot_check_ui(self, ui, tokens),
            Step::Done => {
                note(ui, tokens, &t("imports.done"));
                self.results_ui(ui, tokens);
            }
        }
        if !matches!(self.wizard.step, Step::Import | Step::TieOut | Step::Done) {
            self.sync_dates();
            let blockers = self.wizard.blockers(self.today);
            if !blockers.is_empty() && !self.busy() {
                ui.add_space(8.0);
                for b in &blockers {
                    warn(ui, tokens, &blocker_text(b));
                }
            }
        }
    }

    fn source_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        if self.system {
            let l = ui.label(t("imports.source.pick"));
            ui.horizontal_wrapped(|ui| {
                for s in crate::screens::system_import::SOURCES {
                    let r = ui.radio(
                        self.wizard.source == s,
                        t(&format!("imports.source.{}", s.key())),
                    );
                    if r.clicked() && !self.locked() && self.batch.is_none() {
                        self.release_files_for_source();
                        self.wizard.set_source(s);
                    }
                    r.labelled_by(l.id);
                }
            });
            ui.add_space(8.0);
        }
        note(ui, tokens, &t("imports.export.intro"));
        ui.add_space(6.0);
        for (i, slot) in self.wizard.source.slots().iter().enumerate() {
            ui.label(RichText::new(format!("{}. {}", i + 1, slot_name(slot.key))).strong());
            note(ui, tokens, &t(&format!("imports.slot.{}.how", slot.key)));
            ui.add_space(4.0);
        }
    }

    fn need_text(need: Need) -> String {
        match need {
            Need::Required => t("imports.need.required"),
            Need::Optional => t("imports.need.optional"),
            Need::TwoYearsOnly => t("imports.need.two_years"),
            Need::WithInventory => t("imports.need.inventory"),
        }
    }

    fn files_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        note(ui, tokens, &t("imports.files.intro"));
        ui.add_space(6.0);
        let dropped = ui.ctx().input(|i| i.raw.dropped_files.clone());
        let pinned = self.batch.is_some() || self.locked();
        for slot in self.wizard.source.slots() {
            let name = slot_name(slot.key);
            let card = egui::Frame::new()
                .stroke(egui::Stroke::new(1.0, tokens.border_default))
                .corner_radius(6)
                .inner_margin(Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(&name).strong());
                        note(ui, tokens, &Self::need_text(slot.need));
                        if slot.multiple {
                            note(ui, tokens, &t("imports.files.several"));
                        }
                    });
                    let n = self.uploads.get(slot.key).map_or(0, Vec::len);
                    let mut remove = None;
                    for i in 0..n {
                        let Some(f) = self.uploads.get(slot.key).and_then(|v| v.get(i)) else {
                            continue;
                        };
                        let status = file_status(f);
                        let bad = f.problem.is_some()
                            || f.refusal.is_some()
                            || f.outcome.as_ref().is_some_and(|o| !o.is_clean());
                        ui.horizontal_wrapped(|ui| {
                            ui.label(&f.name);
                            if bad {
                                warn(ui, tokens, &status);
                            } else {
                                note(ui, tokens, &status);
                            }
                            if !pinned
                                && !f.busy()
                                && ui
                                    .small_button(t("imports.files.remove"))
                                    .on_hover_text(t_with(
                                        "imports.files.remove_named",
                                        &[("report", &name)],
                                    ))
                                    .clicked()
                            {
                                remove = Some(i);
                            }
                        });
                    }
                    if let Some(i) = remove {
                        self.remove_file(slot.key, i);
                    }
                    let add = if n > 0 && slot.multiple {
                        t("imports.files.add_another")
                    } else if n > 0 {
                        t("imports.files.replace")
                    } else {
                        t("imports.files.add")
                    };
                    ui.horizontal_wrapped(|ui| {
                        let r = ui
                            .add_enabled(!pinned && self.pick_rx.is_none(), egui::Button::new(add))
                            .on_hover_text(t("imports.files.drop_hint"));
                        r.widget_info(|| {
                            egui::WidgetInfo::labeled(
                                egui::Role::Button,
                                !pinned,
                                t_with("imports.files.add_named", &[("report", &name)]),
                            )
                        });
                        if r.clicked() {
                            self.browse(ui.ctx(), slot.key);
                        }
                        note(ui, tokens, &t("imports.files.drop_hint"));
                    });
                });
            if !pinned && !dropped.is_empty() && card.response.contains_pointer() {
                for d in &dropped {
                    let path = d.path().to_path_buf();
                    self.take_path(slot.key, &path);
                }
            }
            ui.add_space(6.0);
        }
    }

    fn dates_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        note(ui, tokens, &t("imports.dates.intro"));
        ui.add_space(6.0);
        egui::Grid::new("imports/dates")
            .num_columns(2)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                let l = ui.label(t("imports.dates.conversion"));
                ui.add(egui::TextEdit::singleline(&mut self.date_text).desired_width(120.0))
                    .labelled_by(l.id);
                ui.end_row();
                let l = ui.label(t("imports.dates.fiscal_month"));
                let months: Vec<String> =
                    (1..=12).map(|m| t(&format!("imports.month.{m}"))).collect();
                let cur = usize::try_from(self.wizard.fy_start_month.clamp(1, 12) - 1).unwrap_or(0);
                egui::ComboBox::from_id_salt("imports/fy")
                    .selected_text(&months[cur])
                    .show_ui(ui, |ui| {
                        for (i, m) in months.iter().enumerate() {
                            if ui.selectable_label(i == cur, m).clicked() {
                                self.wizard.fy_start_month = u32::try_from(i + 1).unwrap_or(1);
                            }
                        }
                    })
                    .response
                    .labelled_by(l.id);
                ui.end_row();
                if !self.wizard.source.is_conversion() {
                    let l = ui.label(t("imports.dates.opening"));
                    ui.add(egui::TextEdit::singleline(&mut self.opening_text).desired_width(120.0))
                        .labelled_by(l.id);
                    ui.end_row();
                }
            });
        self.sync_dates();
        if self.wizard.source.is_conversion() {
            ui.add_space(8.0);
            let l = ui.label(RichText::new(t("imports.depth.label")).strong());
            for d in [HistoryDepth::TwoYears, HistoryDepth::All] {
                ui.radio_value(
                    &mut self.wizard.depth,
                    d,
                    t(&format!("imports.depth.{}", d.key())),
                )
                .labelled_by(l.id);
            }
            if let Ok(d) = self.wizard.dates()
                && let Some(from) = d.history_from
            {
                note(
                    ui,
                    tokens,
                    &t_with("imports.depth.from", &[("date", &show_date(from))]),
                );
            }
            note(ui, tokens, &t("imports.depth.empty_company"));
        }
    }

    fn import_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let files: usize = self.uploads.values().map(Vec::len).sum();
        note(
            ui,
            tokens,
            &t_with("imports.import.intro", &[("n", &files.to_string())]),
        );
        if self.wizard.source.requires_empty_company() {
            note(ui, tokens, &t("imports.import.empty_company"));
        }
        ui.add_space(8.0);
        if self.waiting.is_some() || self.backup_running() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(t("imports.import.backing_up"));
            });
        } else if self.running.is_some() {
            let p = self.state.as_ref().and_then(|s| s.progress.as_ref());
            let text = p.map_or_else(
                || t("imports.import.running"),
                |p| {
                    t_with(
                        "imports.import.progress",
                        &[
                            ("done", &p.done.to_string()),
                            ("total", &p.total.to_string()),
                        ],
                    )
                },
            );
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(text);
            });
            if let Some(p) = p
                && p.total > 0
            {
                ui.add(egui::ProgressBar::new(ratio(p.done, p.total)).desired_width(320.0));
            }
        } else if self.unsettled {
            if self.status_answer == Some(false) {
                note(ui, tokens, &t("imports.import.not_taken"));
            }
            ui.horizontal(|ui| {
                if ui.button(t("imports.check_status")).clicked() {
                    self.check_status();
                }
                if ui.button(t("imports.try_again")).clicked() {
                    self.try_again();
                }
            });
        } else if self.trouble == Some(Trouble::Failed)
            && self.allowed()
            && self
                .failure
                .as_ref()
                .and_then(super::rebuild_step)
                .is_some()
        {
            // Its options are fixed: a new batch, not the same one again.
            note(ui, tokens, &t("imports.rebuild.note"));
            if ui.button(t("imports.rebuild.button")).clicked() {
                self.rebuild_after_failure();
            }
        } else if self.trouble == Some(Trouble::Failed) && self.allowed() {
            if ui.button(t("imports.retry")).clicked() {
                self.start_import();
            }
        } else if self.trouble == Some(Trouble::BackupFailed)
            && ui.button(t("imports.try_again")).clicked()
        {
            self.start_import();
        }
        self.date_check_ui(ui, tokens);
    }

    /// The batch's date warnings (E2c): shown while and after it posts;
    /// they never block.
    fn date_check_ui(&self, ui: &mut Ui, tokens: &Tokens) {
        let Some(dc) = &self.date_check else {
            return;
        };
        let lines = date_check_lines(dc);
        if lines.is_empty() {
            return;
        }
        ui.add_space(8.0);
        ui.label(RichText::new(t("imports.datecheck.title")).strong());
        for l in &lines {
            note(ui, tokens, l);
        }
    }

    /// What the import brought in (Done).
    fn results_ui(&self, ui: &mut Ui, tokens: &Tokens) {
        let lines = self
            .state
            .as_ref()
            .map(|s| result_lines(&s.batch))
            .unwrap_or_default();
        if !lines.is_empty() {
            ui.add_space(8.0);
            ui.label(RichText::new(t("imports.result.title")).strong());
            for l in &lines {
                note(ui, tokens, l);
            }
        }
        self.date_check_ui(ui, tokens);
    }

    fn trouble_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let Some(tr) = self.trouble.clone() else {
            return;
        };
        let text = match (&tr, &self.failure) {
            (Trouble::Failed, Some(r)) => refusal_text(r),
            _ => trouble_text(&tr),
        };
        egui::Frame::new()
            .fill(tokens.warn_tint)
            .corner_radius(4)
            .inner_margin(Margin::symmetric(10, 6))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.add(egui::Label::new(RichText::new(text).color(tokens.warn_text)).wrap());
                match &tr {
                    Trouble::Refused(ImportRefusal::UndoBlocked { blockers }) => {
                        for b in blockers {
                            ui.label(format!("• {}", blocker_kind_text(&b.kind)));
                        }
                        if let Some(name) = self.blocked_backup() {
                            note(ui, tokens, &t("imports.restore.note"));
                            if ui.button(t("imports.restore.button")).clicked() {
                                self.restore_backup(&name);
                            }
                        }
                    }
                    Trouble::Refused(
                        ImportRefusal::AlreadyCommitted { .. }
                        | ImportRefusal::PeriodOverlap { batch: Some(_), .. },
                    ) if ui.link(t("imports.open_history")).clicked() => {
                        self.show_history();
                    }
                    Trouble::Refused(ImportRefusal::DateMismatch { files, .. }) => {
                        for f in files {
                            ui.label(format!("• {}", self.mismatch_text(f)));
                        }
                    }
                    Trouble::Refused(ImportRefusal::JournalAfterConversion { rows, .. })
                        if !rows.is_empty() =>
                    {
                        let shown: Vec<String> =
                            rows.iter().take(10).map(|(_, r)| r.to_string()).collect();
                        ui.label(t_with(
                            "imports.refusal.journal_after_conversion.rows",
                            &[("rows", &shown.join(", "))],
                        ));
                    }
                    _ => {}
                }
            });
        ui.add_space(8.0);
    }

    fn undo_question_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        if self.confirm_undo.is_none() {
            return;
        }
        ui.add_space(10.0);
        egui::Frame::new()
            .fill(tokens.surface_subtle)
            .stroke(egui::Stroke::new(1.0, tokens.border_strong))
            .corner_radius(6)
            .inner_margin(Margin::symmetric(14, 12))
            .show(ui, |ui| {
                ui.label(RichText::new(t("imports.undo.title")).strong());
                note(ui, tokens, &t("imports.undo.body"));
                ui.horizontal(|ui| {
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new(t("imports.undo.confirm")).color(tokens.on_warn),
                            )
                            .fill(tokens.warn),
                        )
                        .clicked()
                    {
                        self.undo();
                    }
                    if ui.button(t("imports.undo.keep")).clicked() {
                        self.confirm_undo = None;
                    }
                });
            });
    }

    /// A closed period with no closing-date password: the owner confirms
    /// each time (nothing is sent until then).
    fn closed_question_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let Some(ask) = self.ask_closed else {
            return;
        };
        let date = ask.date.map(show_date).unwrap_or_default();
        ui.add_space(10.0);
        egui::Frame::new()
            .fill(tokens.surface_subtle)
            .stroke(egui::Stroke::new(1.0, tokens.border_strong))
            .corner_radius(6)
            .inner_margin(Margin::symmetric(14, 12))
            .show(ui, |ui| {
                ui.label(RichText::new(t("imports.closed.title")).strong());
                note(
                    ui,
                    tokens,
                    &t_with("imports.closed.body", &[("date", &date)]),
                );
                ui.horizontal(|ui| {
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new(t("imports.closed.confirm")).color(tokens.on_warn),
                            )
                            .fill(tokens.warn),
                        )
                        .clicked()
                    {
                        self.confirm_closed();
                    }
                    if ui.button(t("imports.closed.cancel")).clicked() {
                        self.cancel_closed();
                    }
                });
            });
    }

    fn history_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let Some(list) = self.history.clone() else {
            ui.label(t("imports.history.loading"));
            return;
        };
        if list.is_empty() {
            note(ui, tokens, &t("imports.history.empty"));
            return;
        }
        let mut entries: Vec<(Entry, &ImportBatch)> =
            list.iter().map(|b| (entry_of(b), b)).collect();
        entries.sort_by(|a, b| {
            b.0.created
                .cmp(&a.0.created)
                .then_with(|| b.0.id.cmp(&a.0.id))
        });
        let admin = self.allowed();
        let locked = self.locked();
        egui::Grid::new("imports/history")
            .num_columns(5)
            .striped(true)
            .spacing([14.0, 6.0])
            .show(ui, |ui| {
                for k in ["date", "source", "status", "verified", "actions"] {
                    ui.label(RichText::new(t(&format!("imports.history.col.{k}"))).strong());
                }
                ui.end_row();
                for (e, b) in &entries {
                    ui.label(e.created.map(show_date).unwrap_or_default());
                    ui.label(t(&format!("imports.source.{}", e.source)));
                    ui.label(t(&format!("imports.status.{}", e.status.key())));
                    ui.label(if e.verified {
                        t("imports.history.verified")
                    } else {
                        String::new()
                    });
                    let a = history::actions(e, admin);
                    ui.horizontal(|ui| {
                        if a.tieout
                            && ui
                                .add_enabled(!locked, egui::Button::new(t("imports.history.open")))
                                .clicked()
                        {
                            self.open_batch(b);
                        }
                        if a.retry
                            && ui
                                .add_enabled(!locked, egui::Button::new(t("imports.retry")))
                                .clicked()
                        {
                            self.retry_batch(b);
                        }
                        if a.undo
                            && ui
                                .add_enabled(!locked, egui::Button::new(t("imports.undo.button")))
                                .clicked()
                        {
                            self.ask_undo(b.id);
                        }
                        if a.check_status && ui.button(t("imports.check_status")).clicked() {
                            self.load_history();
                        }
                        if a.restore
                            && let Some(name) = &b.backup_ref
                            && ui.button(t("imports.restore.button")).clicked()
                        {
                            self.restore_backup(name);
                        }
                        // D6: a row with nothing to do says why, in plain
                        // words, rather than an empty cell.
                        if !(a.tieout || a.retry || a.undo || a.check_status || a.restore) {
                            let key = format!("imports.history.nothing.{}", e.status.key());
                            let text = if crate::i18n::has(&key) {
                                t(&key)
                            } else {
                                t("imports.history.nothing.other")
                            };
                            ui.label(RichText::new(text).color(tokens.text_secondary));
                        }
                    });
                    ui.end_row();
                    let lines = result_lines(b);
                    if !lines.is_empty() {
                        ui.label("");
                        ui.add(
                            egui::Label::new(
                                RichText::new(lines.join(" ")).color(tokens.text_secondary),
                            )
                            .wrap(),
                        );
                        ui.end_row();
                    }
                }
            });
    }

    fn buttons_ui(&mut self, ui: &mut Ui) {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            if self.view == View::History || self.wizard.step == Step::Done {
                if ui.button(t("imports.close")).clicked() {
                    self.link
                        .push(CompanyEvent::CloseWindow(CommandId::new(self.kind())));
                }
                return;
            }
            let primary = match self.wizard.step {
                Step::Import => t("imports.button.import"),
                Step::TieOut => t("imports.button.verify"),
                Step::SpotCheck => t("imports.button.finish"),
                _ => t("imports.button.continue"),
            };
            let enabled = self.can_next() && self.allowed();
            let r = ui
                .add_enabled_ui(enabled, |ui| crate::widgets::primary_button(ui, &primary))
                .inner
                .on_hover_text(shortcut_hint("form.save_new"));
            if r.clicked() {
                self.next();
            }
            if self.wizard.step != Step::Source
                && ui
                    .add_enabled(self.can_back(), egui::Button::new(t("imports.button.back")))
                    .on_hover_text(shortcut_hint("form.step_back"))
                    .clicked()
            {
                self.back();
            }
            if self.wizard.step == Step::Import
                && self.batch.is_some()
                && !self.wizard.imported
                && ui
                    .add_enabled(
                        !self.locked(),
                        egui::Button::new(t("imports.button.start_over")),
                    )
                    .clicked()
            {
                self.start_over();
            }
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                if self.running.is_some() || self.backup_running() {
                    ui.label(match self.running_kind() {
                        Running::Undo => t("imports.undo.running"),
                        Running::Commit => t("imports.import.running"),
                    });
                }
            });
        });
    }
}

/// The status line under a file.
fn file_status(f: &super::SlotFile) -> String {
    if f.reading.is_some() {
        return t("imports.files.reading");
    }
    if f.job.is_some() {
        return t("imports.files.uploading");
    }
    if let Some(k) = f.problem {
        return t(k);
    }
    if let Some(r) = &f.refusal {
        return refusal_text(r);
    }
    match &f.outcome {
        Some(o) if !o.is_clean() => {
            let mut rows: Vec<u64> = o.problems.iter().map(|p| p.row_number).collect();
            rows.sort_unstable();
            rows.dedup();
            let shown: Vec<String> = rows.iter().take(5).map(u64::to_string).collect();
            t_with(
                "imports.files.problems",
                &[
                    ("n", &o.problems.len().max(1).to_string()),
                    ("rows", &shown.join(", ")),
                ],
            )
        }
        Some(o) => {
            let rows = o.session.row_count.to_string();
            match o.session.report_date.or(o.preview.report_date) {
                Some(d) => t_with(
                    "imports.files.read_dated",
                    &[("rows", &rows), ("date", &show_date(d))],
                ),
                None => t_with("imports.files.read", &[("rows", &rows)]),
            }
        }
        None => String::new(),
    }
}

/// A refusal in owner words (never the engine's message).
pub(crate) fn refusal_text(r: &ImportRefusal) -> String {
    let key = format!("imports.refusal.{}", super::refusal_code(r));
    match r {
        ImportRefusal::AlreadyCommitted { committed_at, .. } => {
            let when = committed_at
                .as_deref()
                .and_then(|s| cl_core::CalendarDate::parse_wire(s.get(..10)?).ok())
                .map(show_date);
            match when {
                Some(d) => t_with("imports.refusal.already_committed_on", &[("date", &d)]),
                None => t(&key),
            }
        }
        ImportRefusal::PeriodOverlap {
            from: Some(f),
            to: Some(to),
            ..
        } => t_with(
            "imports.refusal.period_overlap_dates",
            &[("from", &show_date(*f)), ("to", &show_date(*to))],
        ),
        ImportRefusal::UndoBlocked { blockers } => {
            t_with(&key, &[("n", &blockers.len().to_string())])
        }
        ImportRefusal::DateMismatch {
            conversion_date: Some(d),
            ..
        } => t_with(
            "imports.refusal.date_mismatch_on",
            &[("date", &show_date(*d))],
        ),
        ImportRefusal::JournalAfterConversion {
            conversion_date: Some(d),
            row_count,
            ..
        } if *row_count > 0 => t_with(
            "imports.refusal.journal_after_conversion_on",
            &[("n", &row_count.to_string()), ("date", &show_date(*d))],
        ),
        ImportRefusal::UnmappedAccount { count, .. } if *count > 0 => t_with(
            "imports.refusal.unmapped_account_n",
            &[("n", &count.to_string())],
        ),
        ImportRefusal::InventoryPeriodClosed {
            closing_date: Some(d),
        } => t_with(
            "imports.refusal.inventory_period_closed_on",
            &[("date", &show_date(*d))],
        ),
        ImportRefusal::TieOutNotZero {
            difference_count, ..
        } if *difference_count > 0 => t_with(
            "imports.refusal.tieout_not_zero_n",
            &[("n", &difference_count.to_string())],
        ),
        ImportRefusal::TieOutReferenceRequired { as_of: Some(d) } => t_with(
            "imports.refusal.tieout_reference_required_on",
            &[("date", &show_date(*d))],
        ),
        _ => t(&key),
    }
}

/// The batch's date warnings and history skip in plain words (codes,
/// dates and counts; never the engine's message). Warnings never block.
pub(crate) fn date_check_lines(dc: &cl_api::imports::DateCheck) -> Vec<String> {
    use cl_api::imports::DateWarningCode;
    let d = |x: Option<cl_core::CalendarDate>| x.map(show_date).unwrap_or_default();
    let mut out: Vec<String> = dc
        .warnings
        .iter()
        .map(|w| match w.code {
            DateWarningCode::HistoryGap if w.days == Some(1) => t_with(
                "imports.datecheck.gap_one",
                &[
                    ("first", &d(w.first_journal_date)),
                    ("opening", &d(w.opening_balance_date)),
                ],
            ),
            DateWarningCode::HistoryGap => t_with(
                "imports.datecheck.gap",
                &[
                    ("first", &d(w.first_journal_date)),
                    ("days", &w.days.unwrap_or(0).to_string()),
                    ("opening", &d(w.opening_balance_date)),
                ],
            ),
            DateWarningCode::JournalBeforeOpening => t_with(
                "imports.datecheck.before_opening",
                &[("opening", &d(w.opening_balance_date))],
            ),
        })
        .collect();
    if dc.history_skipped > 0
        && let Some(from) = dc.history_from
    {
        out.push(t_with(
            "imports.datecheck.skipped",
            &[
                ("n", &dc.history_skipped.to_string()),
                ("from", &show_date(from)),
            ],
        ));
    }
    out
}

/// What a committed batch brought in, beyond the plain counts: names on
/// both lists, jobs, inactive names, items left out, classes, the opening
/// stock and the history skip. Counts add across files; the valuation's
/// total is the engine's own, one line per valuation file (amounts are
/// never added here).
pub(crate) fn result_lines(b: &ImportBatch) -> Vec<String> {
    let results: Vec<&cl_api::imports::CommitSummary> =
        b.files.iter().filter_map(|f| f.result.as_ref()).collect();
    let sum = |f: fn(&cl_api::imports::CommitSummary) -> u64| -> u64 {
        results.iter().map(|r| f(r)).sum()
    };
    let mut out = Vec::new();
    let mut count = |key: &str, n: u64| {
        if n > 0 {
            out.push(t_with(key, &[("n", &n.to_string())]));
        }
    };
    count("imports.result.merged", sum(|r| r.merged_to_both));
    count("imports.result.jobs", sum(|r| r.jobs_folded));
    count("imports.result.inactive", sum(|r| r.inactive));
    count(
        "imports.result.items_skipped",
        sum(|r| r.items_skipped.map_or(0, |s| s.total())),
    );
    count("imports.result.classes", sum(|r| r.classes_mapped));
    count(
        "imports.result.skipped_history",
        sum(|r| r.skipped_before_history.unwrap_or(0)),
    );
    for r in &results {
        if r.items_valued > 0
            && let Some(total) = r.valuation_total
        {
            out.push(t_with(
                "imports.result.valued",
                &[
                    ("n", &r.items_valued.to_string()),
                    ("total", &total.format(cl_core::MoneyStyle::default())),
                ],
            ));
        }
    }
    out
}

/// What blocks an undo, in plain words.
pub(crate) fn blocker_kind_text(kind: &str) -> String {
    let key = format!("imports.undo_blocker.{kind}");
    if crate::i18n::has(&key) {
        t(&key)
    } else {
        t("imports.undo_blocker.other")
    }
}

pub(crate) fn trouble_text(tr: &Trouble) -> String {
    match tr {
        Trouble::Refused(r) => refusal_text(r),
        other => t(&format!("imports.trouble.{}", other.code())),
    }
}

impl ImportScreen {
    /// One file of a date mismatch, by the report's name on its card (the
    /// engine's file name is never read).
    pub(crate) fn mismatch_text(&self, f: &cl_api::imports::DateMismatchFile) -> String {
        use cl_api::imports::DateMismatchReason;
        let report = f
            .session
            .and_then(|id| {
                self.uploads
                    .iter()
                    .find(|(_, v)| v.iter().any(|x| x.session() == Some(id)))
                    .map(|(k, _)| slot_name(k))
            })
            .unwrap_or_else(|| t("imports.slot.unknown"));
        let date = f.report_date.map(show_date).unwrap_or_default();
        let key = match f.reason {
            Some(DateMismatchReason::NoDate) => "imports.refusal.date_mismatch.file_no_date",
            Some(DateMismatchReason::AfterConversion) => "imports.refusal.date_mismatch.file_after",
            _ => "imports.refusal.date_mismatch.file",
        };
        t_with(key, &[("report", &report), ("date", &date)])
    }

    fn backup_running(&self) -> bool {
        self.waiting.is_some()
    }

    /// The other-programs picker changed source: its files go.
    fn release_files_for_source(&mut self) {
        self.start_over();
        self.uploads.clear();
    }
}

/// A progress bar's fill (pixels only; never an amount).
#[allow(clippy::float_arithmetic, clippy::cast_precision_loss)]
fn ratio(done: u64, total: u64) -> f32 {
    done.min(total) as f32 / total.max(1) as f32
}
