//! "Columns…" (P16 step 0g, Q-273): which columns a section type shows and
//! in which order, from the toolbar button, the `batch.columns` command or
//! a right-click on a column heading.
//!
//! Each column has a show/hide checkbox and Move up / Move down (Alt+Up /
//! Alt+Down move the focused one). The columns a type needs to save
//! (`cl_model::forms::batch::required_fields`, never a list of the UI's
//! own) stay ticked and disabled, with a tooltip saying why. The choice is
//! remembered per company and section type (`prefs::BulkColumnsBook`);
//! Reset to default forgets it. Hidden columns keep their values and still
//! save; only the grid's drawing changes.

use std::sync::Arc;

use cl_model::forms::batch::{ColumnChoice, ColumnError, Field, SectionKind, is_required};
use eframe::egui::{self, Key, Modifiers, Ui};

use super::grid::{self, header_key};
use super::{BulkEntryScreen, sections};
use crate::i18n::{t, t_with};
use crate::prefs::BulkColumnsBook;
use crate::theme::Tokens;

/// Why a column can't be hidden ("Needed to save checks.").
#[must_use]
pub(crate) const fn needed_key(kind: SectionKind) -> &'static str {
    match kind {
        SectionKind::Checks => "batch_entry.columns.needed.checks",
        SectionKind::Expenses => "batch_entry.columns.needed.expenses",
        SectionKind::CardCharges => "batch_entry.columns.needed.card_charges",
        SectionKind::CardCredits => "batch_entry.columns.needed.card_credits",
        SectionKind::Deposits => "batch_entry.columns.needed.deposits",
        SectionKind::JournalEntries => "batch_entry.columns.needed.journal_entries",
    }
}

impl BulkEntryScreen {
    /// The owner's column choice for `kind` (every column, in the default
    /// order, when none is remembered).
    #[must_use]
    pub(crate) fn choice(&self, kind: SectionKind) -> ColumnChoice {
        self.columns
            .get(&kind)
            .cloned()
            .unwrap_or_else(|| ColumnChoice::default_for(kind))
    }

    /// Reads this company's choices (when the window opens) and shows the
    /// grid with them.
    pub(crate) fn load_columns(&mut self) {
        let Some(dir) = self.settings_dir() else {
            return;
        };
        let book = BulkColumnsBook::load(&dir);
        let company = self.company_key();
        self.columns = SectionKind::ALL
            .iter()
            .filter_map(|k| book.get(&company, *k).map(|c| (*k, c)))
            .collect();
        self.spare.clear();
        if self.columns.contains_key(&self.shown_kind) {
            self.rebuild_grid();
        }
    }

    /// Puts a changed choice in force: kept, remembered for the company,
    /// and the grid of that type rebuilt.
    pub(crate) fn set_choice(&mut self, kind: SectionKind, choice: ColumnChoice) {
        if self.choice(kind) == choice {
            return;
        }
        if let Some(dir) = self.settings_dir() {
            let mut book = BulkColumnsBook::load(&dir);
            book.set(&self.company_key(), kind, &choice);
            if book.save(&dir).is_err() {
                tracing::debug!("the bulk entry columns were not written");
            }
        }
        self.columns.insert(kind, choice);
        self.spare.remove(&kind);
        if kind == self.shown_kind {
            self.rebuild_grid();
        }
        self.last_action = Some("columns");
    }

    /// The shown section's grid again, with its type's columns as chosen
    /// (the open cell goes in first; widths stay, they are kept by column).
    fn rebuild_grid(&mut self) {
        let kind = self.shown_kind;
        let lists = Arc::clone(&self.books.lists);
        let _ = self.grid.commit_pending(&*lists);
        let g = grid::new_grid(kind, &self.choice(kind));
        let _old = self.grid.swap_grid(g, sections::grid_label(kind), &*lists);
    }

    /// Hide or show a column of the shown type.
    ///
    /// # Errors
    /// The column is needed to save the type, or the type has no such
    /// column.
    pub(crate) fn hide_column(&mut self, f: Field, hide: bool) -> Result<(), ColumnError> {
        let kind = self.shown_kind;
        let mut c = self.choice(kind);
        c.set_hidden(kind, f, hide)?;
        self.set_choice(kind, c);
        Ok(())
    }

    /// Move a column of the shown type up (`-1`) or down (`1`).
    pub(crate) fn move_column(&mut self, f: Field, delta: i8) -> bool {
        let kind = self.shown_kind;
        let mut c = self.choice(kind);
        let moved = c.move_by(f, delta);
        if moved {
            self.set_choice(kind, c);
        }
        moved
    }

    /// Reset to default: every column, in the usual order.
    pub(crate) fn reset_columns(&mut self) {
        let kind = self.shown_kind;
        self.set_choice(kind, ColumnChoice::default_for(kind));
    }

    /// Opens "Columns…" (in place of the grid until Done).
    pub(crate) fn open_columns(&mut self) {
        if self.summary_open {
            return;
        }
        let lists = Arc::clone(&self.books.lists);
        let _ = self.grid.commit_pending(&*lists);
        self.picker = None;
        self.confirm_delete = false;
        self.columns_open = true;
        self.last_action = Some("columns_open");
    }

    /// The control channel's view of the shown type's columns.
    pub(crate) fn columns_json(&self) -> serde_json::Value {
        let kind = self.shown_kind;
        let c = self.choice(kind);
        let hidden: Vec<Field> = c
            .all()
            .iter()
            .copied()
            .filter(|f| c.is_hidden(*f))
            .collect();
        serde_json::json!({
            "visible": c.visible(),
            "hidden": hidden,
            "default": c == ColumnChoice::default_for(kind),
        })
    }

    /// The column chooser.
    pub(crate) fn columns_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let kind = self.shown_kind;
        let choice = self.choice(kind);
        let mut toggle: Option<(Field, bool)> = None;
        let mut moves: Option<(Field, i8)> = None;
        let mut focused: Option<Field> = None;
        let mut reset = false;
        let mut done = false;
        let all = choice.all().to_vec();
        let last = all.len().saturating_sub(1);
        crate::screens::company::form::card(ui, tokens, |ui| {
            crate::screens::company::form::heading(
                ui,
                tokens,
                &t_with(
                    "batch_entry.columns.title",
                    &[("kind", &t(sections::kind_key(kind)))],
                ),
            );
            crate::screens::company::form::hint(ui, tokens, &t("batch_entry.columns.explain"));
            ui.add_space(4.0);
            for (i, f) in all.iter().enumerate() {
                let name = t(header_key(kind, *f));
                ui.horizontal(|ui| {
                    let required = is_required(kind, *f);
                    let mut shown = !choice.is_hidden(*f);
                    let r = ui
                        .push_id(("batch_entry/columns", *f), |ui| {
                            ui.add_enabled(!required, egui::Checkbox::new(&mut shown, &name))
                        })
                        .inner;
                    if required {
                        r.clone().on_disabled_hover_text(t(needed_key(kind)));
                    } else if r.changed() {
                        toggle = Some((*f, !shown));
                    }
                    let move_button = |ui: &mut Ui, up: bool, enabled: bool| {
                        let key = if up {
                            "batch_entry.columns.up"
                        } else {
                            "batch_entry.columns.down"
                        };
                        let label = t_with(key, &[("column", &name)]);
                        let text = t(if up {
                            "batch_entry.columns.up.short"
                        } else {
                            "batch_entry.columns.down.short"
                        });
                        let b = ui
                            .push_id(("batch_entry/columns/move", *f, up), |ui| {
                                ui.add_enabled(enabled, egui::Button::new(text).small())
                            })
                            .inner;
                        b.widget_info(|| {
                            egui::WidgetInfo::labeled(egui::Role::Button, enabled, &label)
                        });
                        b.on_hover_text(label)
                    };
                    let up = move_button(ui, true, i > 0);
                    let down = move_button(ui, false, i < last);
                    if up.clicked() {
                        moves = Some((*f, -1));
                    }
                    if down.clicked() {
                        moves = Some((*f, 1));
                    }
                    if r.has_focus() || up.has_focus() || down.has_focus() {
                        focused = Some(*f);
                    }
                });
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                done = crate::widgets::primary_button(ui, &t("batch_entry.columns.done")).clicked();
                reset = ui
                    .add_enabled(
                        choice != ColumnChoice::default_for(kind),
                        egui::Button::new(t("batch_entry.columns.reset")),
                    )
                    .clicked();
            });
        });
        // Alt+Up / Alt+Down move the focused column (its widgets keep
        // their ids, so the focus stays on it).
        if let Some(f) = focused {
            let (up, down) = ui.input_mut(|i| {
                (
                    i.consume_key(Modifiers::ALT, Key::ArrowUp),
                    i.consume_key(Modifiers::ALT, Key::ArrowDown),
                )
            });
            if up {
                moves = Some((f, -1));
            } else if down {
                moves = Some((f, 1));
            }
        }
        if let Some((f, hide)) = toggle {
            let _ = self.hide_column(f, hide);
        }
        if let Some((f, d)) = moves {
            self.move_column(f, d);
        }
        if reset {
            self.reset_columns();
        }
        if done {
            self.columns_open = false;
        }
    }
}
