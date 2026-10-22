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
use crate::i18n::{t, t_count, t_with};
use crate::screens::card_charge::kit::{self, ACTION_HEIGHT};
use crate::screens::company::form;
use crate::theme::Tokens;

/// The most problems listed under the grid (F8 finds the rest).
const LISTED_PROBLEMS: usize = 6;

/// Below this height (points, the window's content) the title row goes.
const COMPACT_BELOW: f32 = 620.0;

/// Below this height the sections and the grid scroll above the actions.
const TIGHT_BELOW: f32 = 360.0;

/// Which measured height (see `BulkEntryScreen::measured`).
#[derive(Clone, Copy)]
enum BelowKind {
    /// Everything under the grid (the usual layout).
    Below,
    /// The totals, messages and actions (the tight layout).
    Footer,
}

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
        let room = ui.available_height();
        if room < TIGHT_BELOW && !self.summary_open && !self.import_open && self.mapping.is_none() {
            self.draw_tight(ui, &tokens);
            return;
        }
        self.toolbar(ui, &tokens);
        // Short windows (the minimum window, large zoom): the tab names the
        // window, so the title row goes and the grid gets its height.
        if room >= COMPACT_BELOW {
            kit::title_only(ui, &tokens, &t("cmd.banking.bulk_entry"));
        } else {
            ui.add_space(6.0);
        }
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
        self.saved_line(ui, &tokens);
        let enabled = !self.busy() && !self.closing_prompt.dialog_open();
        self.strip_ui(ui, &tokens, enabled);
        ui.add_space(4.0);
        self.section_header_ui(ui, &tokens, enabled);
        if self.import_open || self.mapping.is_some() {
            // The panel stands in for the grid until rows are added.
            egui::ScrollArea::vertical()
                .id_salt("batch_entry/panel")
                .auto_shrink([false, true])
                .max_height((ui.available_height() - 70.0).max(120.0))
                .show(ui, |ui| {
                    self.import_ui(ui, &tokens);
                    self.mapping_ui(ui, &tokens);
                });
            ui.add_space(8.0);
            self.actions_ui(ui, false);
            return;
        }
        self.import_ui(ui, &tokens);
        ui.add_space(6.0);
        // Everything under the grid, as tall as it was last frame (first
        // frame: an estimate), stays above the status bar.
        let reserve = self.below_h.unwrap_or_else(|| self.estimate_below());
        self.grid.set_height(cl_grid::GridHeight::Fill {
            reserve_below: reserve,
            min_rows: 3,
        });
        self.grid_ui(ui, enabled);
        let top = ui.cursor().top();
        self.problems_ui(ui, &tokens);
        self.suggestions_ui(ui, &tokens, enabled);
        self.footer_ui(ui, &tokens, enabled, false);
        self.measured(BelowKind::Below, ui.cursor().top() - top);
    }

    /// At a very short height (200% zoom on the minimum window): the
    /// toolbar, the sections, the grid and its problems scroll in the space
    /// above the totals and the actions, which stay on screen (why Save all
    /// is off is in its tooltip).
    fn draw_tight(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let enabled = !self.busy() && !self.closing_prompt.dialog_open();
        let footer = self.footer_h.unwrap_or(ACTION_HEIGHT + 40.0);
        let body = (ui.available_height() - footer).max(48.0);
        self.grid
            .set_height(cl_grid::GridHeight::Rows { min: 3, max: 6 });
        egui::ScrollArea::vertical()
            .id_salt("batch_entry/body")
            .auto_shrink([false, true])
            .max_height(body)
            .show(ui, |ui| {
                self.toolbar(ui, tokens);
                ui.add_space(6.0);
                if !self.caps().can_post() {
                    form::hint(ui, tokens, &t("batch_entry.unavailable"));
                }
                self.saved_line(ui, tokens);
                self.strip_ui(ui, tokens, enabled);
                ui.add_space(4.0);
                self.section_header_ui(ui, tokens, enabled);
                self.import_ui(ui, tokens);
                ui.add_space(6.0);
                self.grid_ui(ui, enabled);
                self.problems_ui(ui, tokens);
                self.suggestions_ui(ui, tokens, enabled);
            });
        let top = ui.cursor().top();
        // Its own container: the scrolled content above is another part
        // of the window, not drawn under the totals.
        ui.scope_builder(
            egui::UiBuilder::new().accessibility_role(egui::accesskit::Role::Group),
            |ui| self.footer_ui(ui, tokens, enabled, true),
        );
        self.measured(BelowKind::Footer, ui.cursor().top() - top);
    }

    /// Remembers how tall the part under the grid was this frame; a change
    /// draws again so the grid takes the right height at once.
    fn measured(&mut self, kind: BelowKind, h: f32) {
        let slot = match kind {
            BelowKind::Below => &mut self.below_h,
            BelowKind::Footer => &mut self.footer_h,
        };
        let h = h.max(0.0) + 4.0;
        if slot.is_none_or(|old| (old - h).abs() > 0.5) {
            *slot = Some(h);
            self.relayout = true;
        }
    }

    /// The first frame's guess at the height under the grid.
    fn estimate_below(&self) -> f32 {
        let listed = self
            .problem_rows_in(self.selected())
            .len()
            .min(LISTED_PROBLEMS + 1);
        let mut reserve = 150.0 + 26.0 * f32::from(u8::try_from(listed).unwrap_or(u8::MAX));
        if !self.open_suggestions().is_empty() || self.suggest.offer.is_some() {
            reserve += 34.0;
        }
        reserve
    }

    /// The grid and what its events start.
    fn grid_ui(&mut self, ui: &mut Ui, enabled: bool) {
        let out = header::locked_scope(ui, enabled, false, |ui| {
            self.grid.ui(ui, &*std::sync::Arc::clone(&self.books.lists))
        });
        self.after_grid_events(&out.events);
        for e in out.events {
            if let GridEvent::BlockPaste { at, text } = e {
                self.block_paste(at, text.as_str());
            }
        }
    }

    /// The totals, the messages and the actions; `brief`: the totals line
    /// only (the tight layout).
    fn footer_ui(&mut self, ui: &mut Ui, tokens: &Tokens, enabled: bool, brief: bool) {
        self.totals_ui(ui, tokens, brief);
        if let Some(text) = self.section_refused_text() {
            form::inline_warning(ui, tokens, &text);
        }
        if let Some(e) = &self.error {
            form::inline_error(ui, tokens, e);
        }
        if let Some(k) = self.notice {
            form::hint(ui, tokens, &t(k));
        }
        if self.busy() {
            form::progress_line(ui, tokens, &t("batch_entry.saving"));
        } else if self.checking.is_some() {
            form::progress_line(ui, tokens, &t("batch_entry.checking"));
        }
        ui.add_space(8.0);
        self.actions_ui(ui, enabled);
    }

    /// "Saved 5 rows. Show the summary" after Save all & New.
    fn saved_line(&mut self, ui: &mut Ui, tokens: &Tokens) {
        if let Some(s) = &self.summary {
            let n = s.rows;
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(t_count("batch_entry.summary.saved", n, &[]))
                        .color(tokens.text_secondary),
                );
                if ui.link(t("batch_entry.summary.show")).clicked() {
                    self.summary_open = true;
                }
            });
        }
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
                self.start_check(super::validate::CheckScope::Everything);
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
    fn totals_ui(&mut self, ui: &mut Ui, tokens: &Tokens, brief: bool) {
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
                t_count(
                    "batch_entry.totals.journal",
                    tot.rows,
                    &[
                        ("debits", &tot.debits.format(style)),
                        ("credits", &tot.credits.format(style)),
                        ("difference", &diff.format(style)),
                    ],
                ),
                !diffs.is_empty(),
            )
        } else {
            (
                t_count(
                    "batch_entry.totals",
                    tot.rows,
                    &[("total", &tot.amount.format(style))],
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
        if brief {
            return;
        }
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
                &t_count("batch_entry.closed.count", locked, &[]),
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
