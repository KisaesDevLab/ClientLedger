//! Drawing Bulk Entry: the toolbar (Check all rows, Import CSV file…), the
//! title, the section strip and pickers, the mapping panel, the grid, the
//! problems of the shown section, the suggestions, the totals line and the
//! actions (Clear · Save all & New · Save all). No mockup (Q-P15An): the
//! Write Check pattern and DESIGN.md's grid rules.

use cl_core::Money;
use cl_grid::GridEvent;
use cl_model::CommandId;
use cl_model::forms::batch::Field;
use eframe::egui::{self, Align, Layout, RichText, Ui, Vec2};

use super::BulkEntryScreen;
use super::grid::{Note, header_key};
use super::validate::blocker_text;
use crate::forms::closing_prompt::ClosingClick;
use crate::forms::header;
use crate::i18n::{t, t_with};
use crate::screens::card_charge::kit::{self, ACTION_HEIGHT};
use crate::screens::company::form;
use crate::theme::Tokens;

/// The most problems listed under the grid (F8 finds the rest).
const LISTED_PROBLEMS: usize = 6;

impl BulkEntryScreen {
    pub(crate) fn draw(&mut self, ui: &mut Ui) {
        let tokens = Tokens::of(ui.ctx());
        if !self.books.loaded {
            form::progress_line(ui, &tokens, &t("batch_entry.loading"));
            return;
        }
        match self.closing_prompt.window(ui.ctx(), &tokens, "batch_entry") {
            ClosingClick::Submit => self.submit_closing(),
            ClosingClick::Cancel => self.closing_prompt.cancel(),
            ClosingClick::None => {}
        }
        self.toolbar(ui, &tokens);
        kit::title_only(ui, &tokens, &t("cmd.banking.bulk_entry"));
        if self.summary_open {
            egui::ScrollArea::vertical()
                .id_salt("batch_entry/summary")
                .auto_shrink([false, false])
                .show(ui, |ui| self.summary_ui(ui, &tokens));
            return;
        }
        if !self.caps().can_post() {
            form::hint(ui, &tokens, &t("batch_entry.unavailable"));
        }
        if let Some(s) = &self.summary {
            let n = s.rows;
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(t_with(
                        "batch_entry.summary.saved",
                        &[("n", &n.to_string())],
                    ))
                    .color(tokens.text_secondary),
                );
                if ui.link(t("batch_entry.summary.show")).clicked() {
                    self.summary_open = true;
                }
            });
        }
        let enabled = !self.busy() && !self.closing_prompt.dialog_open();
        self.strip_ui(ui, &tokens, enabled);
        ui.add_space(4.0);
        self.section_header_ui(ui, &tokens, enabled);
        self.import_ui(ui, &tokens);
        self.mapping_ui(ui, &tokens);
        ui.add_space(6.0);
        let out = header::locked_scope(ui, enabled, false, |ui| {
            self.grid.ui(ui, &*std::sync::Arc::clone(&self.books.lists))
        });
        self.after_grid_events(&out.events);
        for e in out.events {
            if let GridEvent::BlockPaste { at, text } = e {
                self.block_paste(at, text.as_str());
            }
        }
        self.problems_ui(ui, &tokens);
        self.suggestions_ui(ui, &tokens, enabled);
        self.totals_ui(ui, &tokens);
        if let Some(e) = &self.error {
            form::inline_error(ui, &tokens, e);
        }
        if let Some(k) = self.notice {
            form::hint(ui, &tokens, &t(k));
        }
        if self.busy() {
            form::progress_line(ui, &tokens, &t("batch_entry.saving"));
        } else if self.checking.is_some() {
            form::progress_line(ui, &tokens, &t("batch_entry.checking"));
        }
        ui.add_space(8.0);
        self.actions_ui(ui, enabled);
    }

    fn toolbar(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let dirty = self.is_dirty();
        let idle = !self.busy() && !self.summary_open;
        let mut clicked: Option<&'static str> = None;
        header::toolbar(ui, tokens, dirty, &t("helper.unsaved"), |ui| {
            ui.add_enabled_ui(idle, |ui| {
                if ui
                    .button(t("batch_entry.check_all"))
                    .on_hover_text(t("batch_entry.check_all.tip"))
                    .clicked()
                {
                    clicked = Some("check_all");
                }
                if ui
                    .button(t("batch_entry.import"))
                    .on_hover_text(t("batch_entry.import.tip"))
                    .clicked()
                {
                    clicked = Some("import");
                }
                if ui
                    .button(t("batch_entry.next_problem"))
                    .on_hover_text(kit::chip("batch.next_problem").replace(' ', "+"))
                    .clicked()
                {
                    clicked = Some("batch.next_problem");
                }
            });
        });
        match clicked {
            Some("check_all") => {
                self.start_check(true);
            }
            Some("import") => {
                self.import_open = true;
                self.import_error = None;
            }
            Some(cmd) => self.after_frame = Some(CommandId::new(cmd)),
            None => {}
        }
    }

    /// "Row 2 · Account: Not in your lists" for the shown section.
    fn problems_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let i = self.selected();
        let rows = self.problem_rows_in(i);
        if rows.is_empty() {
            return;
        }
        let kind = self.shown_kind;
        let mut go = None;
        ui.add_space(4.0);
        for key in rows.iter().take(LISTED_PROBLEMS) {
            let Some(n) = self.batch().sections[i]
                .rows
                .iter()
                .position(|r| r.key == *key)
            else {
                continue;
            };
            let look = self
                .grid
                .adapter()
                .looks
                .get(key)
                .cloned()
                .unwrap_or_default();
            let what = match look.note {
                Some(Note::Problem(k) | Note::Caption(k)) => t(k.0),
                Some(Note::Engine(m) | Note::Suggestion(m)) => m,
                None => t("batch_entry.problem.engine"),
            };
            let field = self
                .problem_field(*key)
                .filter(|f| kind.columns().contains(f));
            let text = match field {
                Some(f) => t_with(
                    "batch_entry.problem.line_field",
                    &[
                        ("n", &(n + 1).to_string()),
                        ("field", &t(header_key(kind, f))),
                        ("problem", &what),
                    ],
                ),
                None => t_with(
                    "batch_entry.problem.line",
                    &[("n", &(n + 1).to_string()), ("problem", &what)],
                ),
            };
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(text).color(tokens.warn_text));
                if ui.small_button(t("batch_entry.problem.go")).clicked() {
                    go = Some(*key);
                }
            });
        }
        if rows.len() > LISTED_PROBLEMS {
            form::hint(
                ui,
                tokens,
                &t_with(
                    "batch_entry.problem.more",
                    &[("n", &(rows.len() - LISTED_PROBLEMS).to_string())],
                ),
            );
        }
        if let Some(k) = go {
            self.focus_row(k);
        }
    }

    /// "Checks: 3 rows · Total $1,234.50", and for journal entries
    /// "Debits · Credits · Difference" (warn when an entry is off), then
    /// why Save all is off.
    fn totals_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let i = self.selected();
        let style = self.money_style;
        let Ok(tot) = self.batch().section_totals(i) else {
            ui.label(RichText::new(t("batch_entry.problem.too_large")).color(tokens.warn_text));
            return;
        };
        let diffs = self.unbalanced_differences(i);
        let (text, warn) = if self.shown_kind.is_journal() {
            let diff = diffs
                .iter()
                .try_fold(Money::ZERO, |a, d| a.checked_add(d.abs()))
                .unwrap_or(Money::ZERO);
            (
                t_with(
                    "batch_entry.totals.journal",
                    &[
                        ("n", &tot.rows.to_string()),
                        ("debits", &tot.debits.format(style)),
                        ("credits", &tot.credits.format(style)),
                        ("difference", &diff.format(style)),
                    ],
                ),
                !diffs.is_empty(),
            )
        } else {
            (
                t_with(
                    "batch_entry.totals",
                    &[
                        ("n", &tot.rows.to_string()),
                        ("total", &tot.amount.format(style)),
                    ],
                ),
                false,
            )
        };
        let color = if warn {
            tokens.warn_text
        } else {
            tokens.text_secondary
        };
        ui.add_space(4.0);
        ui.add(egui::Label::new(RichText::new(text).size(13.0).color(color)).selectable(false));
        let closing = self.books.closing;
        if let Err(b) = self.batch().can_save(&self.checks, self.caps(), closing) {
            let text = blocker_text(b);
            ui.label(RichText::new(text).color(tokens.warn_text));
        } else if let Some(b) = self.refused {
            ui.label(RichText::new(blocker_text(b)).color(tokens.warn_text));
        }
        let locked = self.batch().locked_rows(closing).len();
        if locked > 0 {
            form::hint(
                ui,
                tokens,
                &t_with("batch_entry.closed.count", &[("n", &locked.to_string())]),
            );
        }
    }

    /// Clear · Save all & New · Save all. Save all is off (its reason in
    /// the tooltip and above) until every row passes.
    fn actions_ui(&mut self, ui: &mut Ui, enabled: bool) {
        let blocker = self
            .batch()
            .can_save(&self.checks, self.caps(), self.books.closing)
            .err();
        // Rows not checked yet are checked on Save all, then saved.
        let can_save = enabled
            && blocker.is_none_or(|b| matches!(b, cl_model::forms::batch::Blocker::Unchecked(_)));
        let tip = blocker.map(blocker_text).unwrap_or_default();
        let mut clicked = None;
        let row = Vec2::new(ui.available_width(), ACTION_HEIGHT);
        ui.allocate_ui_with_layout(row, Layout::right_to_left(Align::Center), |ui| {
            for (cmd, label, primary) in [
                ("form.save_close", "batch_entry.save_all", true),
                ("form.save_new", "batch_entry.save_all_new", false),
            ] {
                let r = ui
                    .add_enabled_ui(can_save, |ui| {
                        crate::widgets::action_button_with_chip(
                            ui,
                            &t(label),
                            &kit::chip(cmd),
                            primary,
                            ACTION_HEIGHT,
                        )
                    })
                    .inner;
                let r = if can_save {
                    r
                } else {
                    r.on_disabled_hover_text(&tip)
                };
                if r.clicked() {
                    clicked = Some(cmd);
                }
            }
            if ui
                .add_enabled(
                    enabled,
                    egui::Button::new(t("helper.clear")).min_size(Vec2::new(0.0, ACTION_HEIGHT)),
                )
                .on_hover_text(t("batch_entry.clear.tip"))
                .clicked()
            {
                clicked = Some("form.clear");
            }
        });
        if let Some(cmd) = clicked {
            self.after_frame = Some(CommandId::new(cmd));
        }
    }

    /// The field a row's caption is about.
    fn problem_field(&self, key: cl_grid::RowKey) -> Option<Field> {
        self.first_field(key)
    }
}
