//! Drawing Import Transactions from a Spreadsheet: the heading and step bar,
//! each step, the run's progress and its troubles, and the closing-date
//! password window. Every input has a visible label it is `labelled_by`.

use cl_model::imports::csv_txn::{MissingColumn, ProblemKind, RowProblem, TxnField};
use eframe::egui::{self, Margin, RichText, Ui};

use super::{Action, CsvTransactionsScreen, EngineProblem, Phase, Step, Trouble};
use crate::forms::closing_prompt::ClosingClick;
use crate::i18n::{t, t_with};
use crate::layout::{Body, FixedFrame, FrameOpts};
use crate::theme::Tokens;

/// The most problems listed (the rest are counted).
const SHOWN_PROBLEMS: usize = 200;

fn note(ui: &mut Ui, tokens: &Tokens, text: &str) {
    ui.add(egui::Label::new(RichText::new(text).color(tokens.text_secondary)).wrap());
}

fn warn(ui: &mut Ui, tokens: &Tokens, text: &str) {
    ui.add(egui::Label::new(RichText::new(text).color(tokens.warn_text)).wrap());
}

fn field_name(f: TxnField) -> String {
    t(&format!("txn_import.field.{}", f.wire()))
}

/// The engine's column in the owner's words ("A cell" when it named none
/// or one this build doesn't know).
fn engine_field_name(f: Option<&str>) -> String {
    f.and_then(TxnField::from_wire)
        .map_or_else(|| t("txn_import.field.unknown"), field_name)
}

fn row_label(row: usize) -> String {
    if row == 0 {
        t("txn_import.file_wide")
    } else {
        t_with("txn_import.row", &[("row", &row.to_string())])
    }
}

/// A row problem in the owner's words (never cell text).
pub(super) fn problem_text(p: &RowProblem) -> String {
    let field = p.field.map(field_name).unwrap_or_default();
    match p.kind {
        ProblemKind::Unbalanced(d) => t_with(
            "txn_import.problem.unbalanced",
            &[("amount", &d.abs().to_string())],
        ),
        k => t_with(
            &format!("txn_import.problem.{}", k.code()),
            &[("field", &field)],
        ),
    }
}

/// A problem the books found, in the owner's words.
pub(super) fn engine_problem_text(p: &EngineProblem) -> String {
    use cl_api::import_transactions::RefReason;
    let key = match p.reason {
        Some(RefReason::Ambiguous) => format!("txn_import.engine.{}.ambiguous", p.code),
        Some(RefReason::NoAccount) => format!("txn_import.engine.{}.no_account", p.code),
        Some(RefReason::Unknown) | None => format!("txn_import.engine.{}", p.code),
    };
    let field = engine_field_name(p.field.as_deref());
    let text = t_with(&key, &[("field", &field)]);
    if text == key {
        t_with("txn_import.engine.other", &[("code", &p.code)])
    } else {
        text
    }
}

fn gap_text(g: MissingColumn) -> String {
    t(match g {
        MissingColumn::Type => "txn_import.map.gap.type",
        MissingColumn::Date => "txn_import.map.gap.date",
        MissingColumn::AccountOrItem => "txn_import.map.gap.account_or_item",
        MissingColumn::Amount => "txn_import.map.gap.amount",
    })
}

impl CsvTransactionsScreen {
    pub(super) fn draw(&mut self, ui: &mut Ui) {
        let tokens = Tokens::of(ui.ctx());
        egui::Frame::new()
            .fill(tokens.surface_window)
            .inner_margin(Margin::symmetric(20, 12))
            .show(ui, |ui| {
                let mut frame = FixedFrame::begin(ui, "csv_transactions", FrameOpts::default());
                frame.top(|ui| {
                    ui.add(egui::Label::new(
                        RichText::new(t(&format!("cmd.{}", super::KIND)))
                            .size(22.0)
                            .strong()
                            .color(tokens.text_primary),
                    ));
                    note(ui, &tokens, &t("txn_import.subtitle"));
                    ui.horizontal_wrapped(|ui| {
                        for (i, s) in Step::ALL.iter().enumerate() {
                            let text = format!(
                                "{}. {}",
                                i + 1,
                                t(&format!("txn_import.step.{}", s.key()))
                            );
                            let rich = if *s == self.step {
                                RichText::new(text).strong().color(tokens.accent)
                            } else {
                                RichText::new(text).color(tokens.text_secondary)
                            };
                            ui.add(egui::Label::new(rich));
                        }
                    });
                });
                frame.body(Body::Scroll, |ui| {
                    ui.set_width(ui.available_width());
                    if !self.supported() {
                        warn(ui, &tokens, &t("txn_import.unsupported"));
                        return;
                    }
                    if !self.admin() {
                        warn(ui, &tokens, &t("txn_import.not_admin"));
                    }
                    self.trouble_ui(ui, &tokens);
                    match self.step {
                        Step::Choose => self.choose_ui(ui, &tokens),
                        Step::Map => self.map_ui(ui, &tokens),
                        Step::Preview => self.preview_ui(ui, &tokens),
                        Step::Done => self.done_ui(ui, &tokens),
                    }
                    self.phase_ui(ui, &tokens);
                });
                frame.bottom(|ui| self.buttons_ui(ui));
                let _ = frame.end();
            });
        match self.closing.window(ui.ctx(), &tokens, "txn_import") {
            ClosingClick::Submit => self.submit_password(),
            ClosingClick::Cancel => self.closing.cancel(),
            ClosingClick::None => {}
        }
    }

    fn choose_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        note(ui, tokens, &t("txn_import.choose.text"));
        ui.add_space(8.0);
        if ui
            .add_enabled(
                self.pick_rx.is_none(),
                egui::Button::new(t("txn_import.choose.browse")),
            )
            .clicked()
        {
            self.browse(ui.ctx());
        }
        if let Some(e) = self.file_error {
            warn(ui, tokens, &t(e));
        }
    }

    fn map_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        if let Some(name) = &self.file_name {
            note(
                ui,
                tokens,
                &t_with(
                    "txn_import.choose.file",
                    &[("name", name), ("rows", &self.rows.len().to_string())],
                ),
            );
        }
        note(ui, tokens, &t("txn_import.map.text"));
        if self.used_saved {
            note(ui, tokens, &t("txn_import.map.used_saved"));
        }
        ui.add_space(6.0);
        let headers = self.headers.clone();
        let mut changes = Vec::new();
        egui::Grid::new("txn_import_map")
            .num_columns(2)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                for f in TxnField::ALL {
                    let label = ui.label(field_name(f));
                    let current = self.mapping.get(&f).copied();
                    let shown = current
                        .and_then(|c| headers.get(c).cloned())
                        .unwrap_or_else(|| t("txn_import.map.none"));
                    egui::ComboBox::from_id_salt(("txn_import_col", f.wire()))
                        .selected_text(shown)
                        .width(240.0)
                        .show_ui(ui, |ui| {
                            if ui
                                .selectable_label(current.is_none(), t("txn_import.map.none"))
                                .clicked()
                            {
                                changes.push((f, None));
                            }
                            for (i, h) in headers.iter().enumerate() {
                                if ui.selectable_label(current == Some(i), h).clicked() {
                                    changes.push((f, Some(i)));
                                }
                            }
                        })
                        .response
                        .labelled_by(label.id);
                    ui.end_row();
                }
            });
        for (f, c) in changes {
            self.set_column(f, c);
        }
        ui.add_space(6.0);
        ui.checkbox(&mut self.remember, t("txn_import.map.remember"));
        for g in self.gaps() {
            warn(ui, tokens, &gap_text(g));
        }
    }

    fn preview_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let Some(plan) = self.plan.as_ref() else {
            return;
        };
        let c = plan.counts();
        ui.label(t_with(
            "txn_import.preview.counts",
            &[
                (
                    "invoices",
                    &counted("txn_import.count.invoices", c.invoices),
                ),
                ("bills", &counted("txn_import.count.bills", c.bills)),
                (
                    "entries",
                    &counted("txn_import.count.entries", c.journal_entries),
                ),
                ("rows", &counted("txn_import.count.rows", plan.rows)),
            ],
        ));
        // U-268: the import adds no names, products, tags or accounts.
        note(ui, tokens, &t("txn_import.preview.nothing_created"));
        if plan.problems.is_empty() && self.engine_problems.is_empty() {
            note(ui, tokens, &t("txn_import.preview.ready"));
        } else {
            warn(ui, tokens, &t("txn_import.preview.problems"));
            ui.add_space(4.0);
            let lines: Vec<String> = plan
                .problems
                .iter()
                .map(|p| format!("{}: {}", row_label(p.row), problem_text(p)))
                .chain(
                    self.engine_problems
                        .iter()
                        .map(|p| format!("{}: {}", row_label(p.row), engine_problem_text(p))),
                )
                .collect();
            for l in lines.iter().take(SHOWN_PROBLEMS) {
                ui.add(egui::Label::new(l).wrap());
            }
            if lines.len() > SHOWN_PROBLEMS {
                note(
                    ui,
                    tokens,
                    &t_with(
                        "txn_import.preview.more",
                        &[("n", &(lines.len() - SHOWN_PROBLEMS).to_string())],
                    ),
                );
            }
        }
    }

    fn done_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        if self.undone {
            ui.label(t("txn_import.undone"));
            return;
        }
        let c = self.counts();
        ui.label(t_with(
            "txn_import.done",
            &[
                (
                    "invoices",
                    &counted("txn_import.count.invoices", c.invoices),
                ),
                ("bills", &counted("txn_import.count.bills", c.bills)),
                (
                    "entries",
                    &counted("txn_import.count.entries", c.journal_entries),
                ),
            ],
        ));
        if let Some(b) = &self.backup_ref {
            note(
                ui,
                tokens,
                &t_with("txn_import.done.backup", &[("file", b)]),
            );
        }
        if self.confirm_undo {
            ui.add_space(8.0);
            warn(ui, tokens, &t("txn_import.undo.confirm"));
            ui.horizontal(|ui| {
                if ui.button(t("txn_import.undo.yes")).clicked() {
                    self.undo();
                }
                if ui.button(t("txn_import.undo.no")).clicked() {
                    self.confirm_undo = false;
                }
            });
        }
    }

    fn phase_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        ui.add_space(8.0);
        let text = match self.phase {
            Phase::Idle => return,
            Phase::Uploading | Phase::Batching => t("txn_import.uploading"),
            Phase::BackingUp(Action::Commit) => t("txn_import.backing_up"),
            Phase::BackingUp(Action::Undo) => t("txn_import.backing_up.undo"),
            Phase::Sending(_) | Phase::Checking(_) => t("txn_import.sending"),
            Phase::Running(Action::Commit) => {
                let (done, total) = self
                    .state
                    .as_ref()
                    .map_or((0, 0), |s| (s.progress.done, s.progress.total));
                t_with(
                    "txn_import.running",
                    &[("done", &done.to_string()), ("total", &total.to_string())],
                )
            }
            Phase::Running(Action::Undo) => t("txn_import.running.undo"),
            Phase::Unknown(_) => {
                warn(ui, tokens, &t("txn_import.unknown"));
                if ui.button(t("txn_import.check_status")).clicked() {
                    self.check_status();
                }
                return;
            }
        };
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(text);
        });
    }

    fn trouble_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let Some(trouble) = self.trouble.clone() else {
            return;
        };
        let mut again = false;
        match &trouble {
            Trouble::BackupFailed(a) => {
                warn(
                    ui,
                    tokens,
                    &t(if *a == Action::Undo {
                        "txn_import.backup_failed.undo"
                    } else {
                        "txn_import.backup_failed"
                    }),
                );
                again = true;
            }
            Trouble::NotStarted(_) => {
                warn(ui, tokens, &t("txn_import.status.not_started"));
                again = true;
            }
            Trouble::Failed { row, code, field } => {
                let why = engine_problem_text(&EngineProblem {
                    row: row.unwrap_or(0),
                    field: field.clone(),
                    code: code.clone(),
                    reason: None,
                });
                match row {
                    Some(r) => warn(
                        ui,
                        tokens,
                        &t_with(
                            "txn_import.failed.row",
                            &[("row", &r.to_string()), ("why", &why)],
                        ),
                    ),
                    None => warn(ui, tokens, &t("txn_import.failed")),
                }
                again = true;
            }
            Trouble::Already { when } => warn(
                ui,
                tokens,
                &t_with(
                    "txn_import.already",
                    &[(
                        "when",
                        when.as_deref().map_or("", |w| w.get(..10).unwrap_or(w)),
                    )],
                ),
            ),
            Trouble::Busy => warn(ui, tokens, &t("txn_import.busy")),
            Trouble::AdminRequired => warn(ui, tokens, &t("txn_import.admin_required")),
            Trouble::UndoBlocked(blockers) => {
                warn(ui, tokens, &t("txn_import.undo_blocked"));
                for b in blockers {
                    ui.label(t_with(
                        "txn_import.undo_blocked.item",
                        &[
                            ("kind", &b.kind.replace('_', " ")),
                            (
                                "reason",
                                &b.reason.as_deref().unwrap_or("").replace('_', " "),
                            ),
                        ],
                    ));
                }
                note(ui, tokens, &t("txn_import.undo_blocked.fix"));
            }
            Trouble::Engine => warn(ui, tokens, &t("txn_import.trouble")),
        }
        if again && self.admin() && ui.button(t("txn_import.try_again")).clicked() {
            self.try_again();
        }
        ui.add_space(6.0);
    }

    fn buttons_ui(&mut self, ui: &mut Ui) {
        let idle = self.phase == Phase::Idle;
        ui.horizontal(|ui| {
            match self.step {
                Step::Choose => {}
                Step::Map => {
                    if ui
                        .add_enabled(idle, egui::Button::new(t("txn_import.back")))
                        .clicked()
                    {
                        self.back();
                    }
                    if ui
                        .add_enabled(
                            self.gaps().is_empty(),
                            egui::Button::new(t("txn_import.next")),
                        )
                        .clicked()
                    {
                        self.check_rows();
                    }
                }
                Step::Preview => {
                    if ui
                        .add_enabled(
                            idle && !self.commit_sent,
                            egui::Button::new(t("txn_import.back")),
                        )
                        .clicked()
                    {
                        self.back();
                    }
                    let ready = self
                        .plan
                        .as_ref()
                        .is_some_and(cl_model::imports::csv_txn::TxnPlan::can_import)
                        && self.engine_problems.is_empty()
                        && self.trouble.is_none();
                    if self.admin()
                        && ui
                            .add_enabled(idle && ready, egui::Button::new(t("txn_import.import")))
                            .clicked()
                    {
                        self.start_import();
                    }
                }
                Step::Done => {
                    if !self.undone
                        && self.admin()
                        && ui
                            .add_enabled(
                                idle && !self.confirm_undo,
                                egui::Button::new(t("txn_import.undo")),
                            )
                            .clicked()
                    {
                        self.ask_undo();
                    }
                    if ui
                        .add_enabled(idle, egui::Button::new(t("txn_import.start_over")))
                        .clicked()
                    {
                        self.start_over();
                    }
                }
            }
            let may_close =
                !self.phase.may_be_running() && !matches!(self.phase, Phase::BackingUp(_));
            if ui
                .add_enabled(may_close, egui::Button::new(t("txn_import.cancel")))
                .clicked()
            {
                self.cancel();
            }
        });
    }
}

/// `n` things in words, singular for one ("1 invoice", "2 invoices").
fn counted<N: TryInto<usize>>(key: &str, n: N) -> String {
    crate::i18n::t_count(key, n.try_into().unwrap_or(usize::MAX), &[])
}
