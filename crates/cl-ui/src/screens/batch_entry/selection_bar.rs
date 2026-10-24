//! The selection bar (P16 step 0h, Q-274): with rows ticked in the grid's
//! select column (Shift+click or Shift+Up/Down for a range, Ctrl+click to
//! add or remove one, Ctrl+A or the menu's Select All Rows), it shows the
//! count and Set Account, Set Name, Set Tag, Set Date, Set Memo, Set Print
//! Later (checks only), Delete Rows and Clear Selection.
//!
//! A Set action opens the same cell the grid has for that column (a click
//! opens its list, Q-250; Ctrl+L the full list; date keys in a date), in a
//! one-cell grid; Apply or Enter puts the value on every selected row as
//! one undo step. Nothing is set unasked: no suggestion fills the picker.
//! Only the columns the section type has are offered, and never an amount
//! or a number. Setting a hidden column is allowed and says so.

use std::sync::Arc;

use cl_grid::{
    CellInput, CellPos, CellProblem, CellValue, ColumnId, EditableModel, Grid, GridEvent,
    GridHeight, RecordOutcome, RowKey, RowModel, RowState, Source,
};
use cl_model::forms::FormState;
use cl_model::forms::batch::{Field, FieldValue, RowDraft, SectionKind, is_required, ops};
use eframe::egui::{self, RichText, Ui, Vec2};

use super::BulkEntryScreen;
use super::grid::{apply, cell_value, column_of, field_of, header_key, picker_spec};
use crate::grid_form::{GridForm, LinesAdapter};
use crate::i18n::{t, t_count, t_with};
use crate::screens::register::lists::RegisterLists;
use crate::theme::Tokens;

/// The Set actions in the bar's order, with their commands.
pub(crate) const SET_ACTIONS: [(Field, &str); 6] = [
    (Field::Account, "batch.set_account"),
    (Field::Contact, "batch.set_name"),
    (Field::Tag, "batch.set_tag"),
    (Field::Date, "batch.set_date"),
    (Field::Memo, "batch.set_memo"),
    (Field::PrintLater, "batch.set_print_later"),
];

/// The field a Set command is for.
#[must_use]
pub(crate) fn set_field_of(command: &str) -> Option<Field> {
    SET_ACTIONS
        .iter()
        .find(|(_, c)| *c == command)
        .map(|(f, _)| *f)
}

/// The picker's one row and what it is for.
pub(crate) struct PickAdapter {
    lists: Arc<RegisterLists>,
    kind: SectionKind,
    field: Field,
}

struct PickView<'a> {
    form: &'a mut FormState<RowDraft>,
    a: &'a mut PickAdapter,
}

impl LinesAdapter<RowDraft> for PickAdapter {
    fn model<'a>(&'a mut self, form: &'a mut FormState<RowDraft>) -> Box<dyn EditableModel + 'a> {
        Box::new(PickView { form, a: self })
    }
}

impl RowModel for PickView<'_> {
    fn revision(&self) -> u64 {
        self.form.revision()
    }
    fn row_count(&self) -> usize {
        1
    }
    fn row_key(&self, index: usize) -> Option<RowKey> {
        (index == 0).then(|| self.form.current().key)
    }
    fn index_of(&self, key: RowKey) -> Option<usize> {
        (key == self.form.current().key).then_some(0)
    }
    fn row_state(&self, _index: usize) -> RowState {
        RowState::empty()
    }
    fn cell(&self, index: usize, col: ColumnId) -> CellValue<'_> {
        if index != 0 || field_of(col) != Some(self.a.field) {
            return CellValue::Empty;
        }
        // No suggestion: a picker never shows a value the owner did not
        // choose (non-negotiable 4).
        cell_value(self.form.current(), Some(self.a.field), &self.a.lists, None)
    }
    fn row_label(&self, _index: usize) -> String {
        String::new()
    }
}

impl EditableModel for PickView<'_> {
    fn cell_editable(&self, index: usize, col: ColumnId) -> bool {
        index == 0 && field_of(col) == Some(self.a.field)
    }
    fn row_entered(&mut self, _key: RowKey) {}
    fn set_cell(
        &mut self,
        pos: CellPos,
        value: CellInput,
        _source: Source,
    ) -> Result<(), CellProblem> {
        let lists = Arc::clone(&self.a.lists);
        let kind = self.a.kind;
        let mut problem = None;
        self.form
            .edit_step(|r| problem = apply(r, kind, pos.col, value, &lists));
        problem.map_or(Ok(()), |k| Err(CellProblem::new(k).at(pos.col)))
    }
    fn revert_row(&mut self, _key: RowKey) -> bool {
        false
    }
    fn insert_line(&mut self, _before: Option<RowKey>) -> Option<RowKey> {
        None
    }
    fn delete_line(&mut self, _key: RowKey) -> bool {
        false
    }
    fn record_row(&mut self, _key: RowKey) -> RecordOutcome {
        // Enter applies the value (the screen sees `RowRecorded`).
        RecordOutcome::Recorded { next: None }
    }
}

/// An open Set picker.
pub(crate) struct Picker {
    /// The column it sets.
    pub field: Field,
    /// Its one-cell grid.
    pub form: GridForm<RowDraft, PickAdapter>,
    /// Why Apply did nothing (i18n key).
    pub error: Option<&'static str>,
}

impl Picker {
    /// The picker's grid has the keyboard.
    #[must_use]
    pub(crate) fn has_focus(&self) -> bool {
        self.form.grid().has_focus()
    }
}

impl BulkEntryScreen {
    /// A Set action (from the bar, the palette or a command): the picker
    /// for `f` opens with the keyboard in its cell. False when the section
    /// type has no such column, it is protected, or no row is selected.
    pub(crate) fn open_picker(&mut self, f: Field) -> bool {
        let kind = self.shown_kind;
        if self.busy() || !ops::settable(kind, f) {
            return false;
        }
        if self.selected_rows().is_empty() {
            self.bulk_note = Some(t("batch_entry.selection.none"));
            return false;
        }
        let lists = Arc::clone(&self.books.lists);
        let _ = self.grid.commit_pending(&*lists);
        let key = self.grid.adapter_mut().keys.next_key();
        let mut row = RowDraft::new(key);
        // Print later's picker starts ticked: the usual reason to set it.
        row.print_later = f == Field::PrintLater;
        let Ok(g) = Grid::new("batch_entry/set", picker_spec(kind, f)) else {
            return false;
        };
        let adapter = PickAdapter {
            lists: Arc::clone(&lists),
            kind,
            field: f,
        };
        let mut form = GridForm::new(
            FormState::new(row),
            adapter,
            g,
            t_with(
                "batch_entry.set.label",
                &[("column", &t(header_key(kind, f)))],
            ),
            GridHeight::Rows { min: 1, max: 1 },
        );
        form.focus(
            CellPos {
                row: key,
                col: column_of(f),
            },
            &*lists,
        );
        self.picker = Some(Picker {
            field: f,
            form,
            error: None,
        });
        self.confirm_delete = false;
        self.columns_open = false;
        self.last_action = Some("set_open");
        true
    }

    /// Apply (or Enter in the picker): the value goes on every selected
    /// row as one undo step. Returns how many rows changed.
    pub(crate) fn apply_picker(&mut self) -> usize {
        let Some(mut p) = self.picker.take() else {
            return 0;
        };
        let lists = Arc::clone(&self.books.lists);
        if p.form.commit_pending(&*lists).is_err() {
            self.picker = Some(p);
            return 0;
        }
        let kind = self.shown_kind;
        let value = FieldValue::of(p.form.form().current(), p.field);
        let empty = match &value {
            None => true,
            Some(FieldValue::Name { id: None, text } | FieldValue::Account { id: None, text }) => {
                text.trim().is_empty() && is_required(kind, p.field)
            }
            Some(_) => false,
        };
        let Some(value) = value.filter(|_| !empty) else {
            p.error = Some("batch_entry.set.empty");
            self.picker = Some(p);
            return 0;
        };
        let rows = self.selected_rows();
        let n = self.change_section(|s| ops::set_field(s, &rows, &value));
        self.rows_changed = n;
        self.last_action = Some("set");
        self.bulk_note = Some(self.set_note(p.field, n));
        n
    }

    /// Puts `value` in the open picker's cell, as typing and Tab would.
    #[cfg(test)]
    pub(crate) fn picker_value(&mut self, value: CellInput) {
        let lists = Arc::clone(&self.books.lists);
        let kind = self.shown_kind;
        if let Some(p) = self.picker.as_mut() {
            let col = column_of(p.field);
            p.form.edit_step(|r| {
                let _ = apply(r, kind, col, value, &lists);
            });
        }
    }

    /// The bar above the grid while rows are selected.
    pub(crate) fn selection_bar_ui(&mut self, ui: &mut Ui, tokens: &Tokens, enabled: bool) {
        let count = self.selected_rows().len();
        if count == 0 && self.picker.is_none() && !self.confirm_delete {
            return;
        }
        let kind = self.shown_kind;
        let choice = self.choice(kind);
        let mut command: Option<&'static str> = None;
        ui.add_space(4.0);
        ui.scope_builder(
            egui::UiBuilder::new().accessibility_role(egui::accesskit::Role::Toolbar),
            |ui| {
                ui.add_enabled_ui(enabled, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            RichText::new(t_count("batch_entry.selection.count", count, &[]))
                                .strong(),
                        );
                        for (f, cmd) in SET_ACTIONS {
                            if !ops::settable(kind, f) {
                                continue;
                            }
                            let mut b = ui.add_enabled(
                                count > 0,
                                egui::Button::new(t(&format!("cmd.{cmd}"))),
                            );
                            if choice.is_hidden(f) {
                                b = b.on_hover_text(t_with(
                                    "batch_entry.set.hidden_tip",
                                    &[("column", &t(header_key(kind, f)))],
                                ));
                            }
                            if b.clicked() {
                                command = Some(cmd);
                            }
                        }
                        if crate::widgets::destructive_button(ui, &t("cmd.batch.delete_rows"))
                            .clicked()
                            && count > 0
                        {
                            command = Some("batch.delete_rows");
                        }
                        if ui.button(t("cmd.grid.clear_selection")).clicked() {
                            command = Some("grid.clear_selection");
                        }
                    });
                });
            },
        );
        self.picker_ui(ui, tokens, enabled);
        self.confirm_delete_ui(ui, tokens);
        if let Some(cmd) = command {
            self.after_frame = Some(cl_model::CommandId::new(cmd));
        }
    }

    /// The open picker: "New account for 2 rows" over its cell, Apply and
    /// Cancel.
    fn picker_ui(&mut self, ui: &mut Ui, tokens: &Tokens, enabled: bool) {
        let count = self.selected_rows().len();
        let lists = Arc::clone(&self.books.lists);
        let mut apply = false;
        let mut cancel = false;
        let Some(p) = self.picker.as_mut() else {
            return;
        };
        let kind = self.shown_kind;
        crate::screens::company::form::card(ui, tokens, |ui| {
            ui.label(t_count(
                "batch_entry.set.title",
                count,
                &[("column", &t(header_key(kind, p.field)))],
            ));
            let width = ui.available_width().min(360.0);
            let out = ui
                .allocate_ui(Vec2::new(width, 64.0), |ui| p.form.ui(ui, &*lists))
                .inner;
            apply |= out
                .events
                .iter()
                .any(|e| matches!(e, GridEvent::RowRecorded { .. }));
            if let Some(k) = p.error {
                ui.label(RichText::new(t(k)).color(tokens.warn_text));
            }
            ui.add_enabled_ui(enabled, |ui| {
                ui.horizontal(|ui| {
                    apply |=
                        crate::widgets::primary_button(ui, &t("batch_entry.set.apply")).clicked();
                    cancel = ui.button(t("batch_entry.set.cancel")).clicked();
                });
            });
        });
        if cancel {
            self.picker = None;
        } else if apply {
            self.apply_picker();
        }
    }

    /// Delete rows asks first.
    fn confirm_delete_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        if !self.confirm_delete {
            return;
        }
        let n = self.selected_rows().len();
        let mut yes = false;
        let mut no = false;
        crate::screens::company::form::card(ui, tokens, |ui| {
            ui.label(t_count("batch_entry.delete.confirm", n, &[]));
            ui.horizontal(|ui| {
                yes = crate::widgets::destructive_button(
                    ui,
                    &t_count("batch_entry.delete.yes", n, &[]),
                )
                .clicked();
                no = ui.button(t("batch_entry.delete.no")).clicked();
            });
        });
        if yes {
            self.delete_selected();
        } else if no {
            self.confirm_delete = false;
        }
    }
}
