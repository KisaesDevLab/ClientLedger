//! [`GridForm`]: a form's [`FormState`] and its line [`Grid`] kept together,
//! so every save, leave and record move commits the open cell first (P10
//! plan Rev 2 §2.3, B4).
//!
//! A grid screen keeps its `FormState` **only** inside a `GridForm`; there
//! is no way to reach `FormState::begin_save` past it. The
//! `grid_screens_never_call_form_state_begin_save` test fails a screen
//! module that uses a grid and holds its own `FormState`.
//!
//! Paths that must commit first (one test each below): Alt+N / Alt+S
//! (`command`), a mouse Save button (`begin_save`), the close prompt's Save
//! and the quit prompt (`Screen::save` → `begin_save`), and Prev/Next
//! record buttons or commands when the grid is not focused (`leave`).

use cl_grid::{
    CellPos, CellProblem, EditableModel, Grid, GridCommand, GridHeight, GridOutput, GridSnapshot,
    Lookups, ModelRef,
};
use cl_model::CommandId;
use cl_model::forms::{AfterSave, FormState, LeaveDecision, SaveFailure, SaveIntent, SaveStart};
use eframe::egui::Ui;

use crate::grid_env::{self, EnvParts};

/// Builds the grid's editable model over the form's value. The adapter
/// keeps what the model needs besides the value (row-key generator, the
/// Esc snapshot of the row being edited).
pub trait LinesAdapter<V> {
    /// The model for one call.
    fn model<'a>(&'a mut self, form: &'a mut FormState<V>) -> Box<dyn EditableModel + 'a>;
}

/// What a routed command asks the screen to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Routed<V> {
    /// Not a grid-form command, or nothing to do: let the shell run its
    /// default (Esc closes or prompts).
    NotHandled,
    /// The grid or the form acted.
    Handled,
    /// A save started after the commit: send the value, or follow up.
    Save(SaveStart<V>),
    /// Prev/Next record with the grid not focused: the open cell was
    /// committed; the screen moves when `decision` allows (or prompts).
    Navigate {
        /// Next (true) or previous (false).
        forward: bool,
        /// What `FormState::leave` said.
        decision: LeaveDecision,
    },
    /// The open cell is not valid: the cursor is on it with the problem
    /// shown, and nothing was saved or left.
    Blocked(CellProblem),
}

/// A form with a line grid.
pub struct GridForm<V, A> {
    form: FormState<V>,
    adapter: A,
    grid: Grid,
    parts: EnvParts,
    label: String,
    height: GridHeight,
    widths_key: Option<String>,
    widths_loaded: bool,
    snapshot: GridSnapshot,
}

impl<V: Clone + PartialEq, A: LinesAdapter<V>> GridForm<V, A> {
    /// Put a form and its grid together.
    pub fn new(
        form: FormState<V>,
        adapter: A,
        grid: Grid,
        label: impl Into<String>,
        height: GridHeight,
    ) -> GridForm<V, A> {
        GridForm {
            form,
            adapter,
            grid,
            parts: EnvParts::default(),
            label: label.into(),
            height,
            widths_key: None,
            widths_loaded: false,
            snapshot: GridSnapshot::default(),
        }
    }

    /// Remember column widths under this key (`<screen kind>/<grid>`).
    #[must_use]
    pub fn with_widths_key(mut self, key: impl Into<String>) -> GridForm<V, A> {
        self.widths_key = Some(key.into());
        self
    }

    /// The form (read only: saves go through [`GridForm::begin_save`]).
    #[must_use]
    pub fn form(&self) -> &FormState<V> {
        &self.form
    }

    /// The adapter.
    #[must_use]
    pub fn adapter(&self) -> &A {
        &self.adapter
    }

    /// The adapter, for screen bookkeeping outside the model.
    pub fn adapter_mut(&mut self) -> &mut A {
        &mut self.adapter
    }

    /// The grid (read only).
    #[must_use]
    pub fn grid(&self) -> &Grid {
        &self.grid
    }

    /// Shows the form through another grid (a different column set, P15A
    /// Bulk Entry sections) and returns the one it replaces. The new grid's
    /// cursor is put back on rows of the current value.
    pub fn swap_grid(
        &mut self,
        grid: Grid,
        label: impl Into<String>,
        lookups: &dyn Lookups,
    ) -> Grid {
        let old = std::mem::replace(&mut self.grid, grid);
        self.label = label.into();
        self.widths_loaded = false;
        self.with(lookups, Grid::reset);
        old
    }

    /// The model's rows changed under the grid without an edit (another
    /// section is shown): the cursor goes back to a row that exists.
    pub fn reset_view(&mut self, lookups: &dyn Lookups) {
        self.with(lookups, Grid::reset);
    }

    /// How tall the grid is from now on (a screen whose footer grows).
    pub fn set_height(&mut self, height: GridHeight) {
        self.height = height;
    }

    /// Remember column widths under this key from now on.
    pub fn set_widths_key(&mut self, key: impl Into<String>) {
        self.widths_key = Some(key.into());
    }

    /// Positions and counts from the last frame or command (no values).
    #[must_use]
    pub fn snapshot(&self) -> GridSnapshot {
        self.snapshot
    }

    /// Unsaved edits: the form is dirty or the open cell has text that is
    /// not committed yet.
    #[must_use]
    pub fn dirty(&self) -> bool {
        self.form.is_dirty() || self.grid.has_uncommitted_edit()
    }

    /// A header-field edit (merges with the field's previous edit).
    pub fn edit(&mut self, field: u64, f: impl FnOnce(&mut V)) -> bool {
        self.form.edit(field, f)
    }

    /// A header edit that is its own undo step.
    pub fn edit_step(&mut self, f: impl FnOnce(&mut V)) -> bool {
        self.form.edit_step(f)
    }

    /// Run `f` with the grid, its model and an environment built from the
    /// parts captured last frame.
    fn with<R>(
        &mut self,
        lookups: &dyn Lookups,
        f: impl FnOnce(&mut Grid, ModelRef<'_>, &cl_grid::GridEnv<'_>) -> R,
    ) -> R {
        let env = self.parts.env(lookups, &self.label, self.height);
        let mut model = self.adapter.model(&mut self.form);
        let r = f(&mut self.grid, ModelRef::Edit(&mut *model), &env);
        self.snapshot = self.grid.snapshot(&ModelRef::Edit(&mut *model));
        r
    }

    /// The grid model's whole-grid rules (`EditableModel::validate_grid`).
    pub fn validate(&mut self) -> Vec<cl_grid::GridProblem> {
        self.adapter.model(&mut self.form).validate_grid()
    }

    /// Commit the open cell (B4).
    ///
    /// # Errors
    /// The open cell is not valid; the cursor stays on it.
    pub fn commit_pending(&mut self, lookups: &dyn Lookups) -> Result<(), CellProblem> {
        self.with(lookups, Grid::commit_pending)
    }

    /// Commit the open cell, then start a save.
    ///
    /// # Errors
    /// The open cell is not valid: nothing is saved.
    pub fn begin_save(
        &mut self,
        intent: SaveIntent,
        lookups: &dyn Lookups,
    ) -> Result<SaveStart<V>, CellProblem> {
        self.commit_pending(lookups)?;
        Ok(self.form.begin_save(intent))
    }

    /// Commit the open cell, then ask whether leaving needs a prompt.
    ///
    /// # Errors
    /// The open cell is not valid: the owner stays on it.
    pub fn leave(&mut self, lookups: &dyn Lookups) -> Result<LeaveDecision, CellProblem> {
        self.commit_pending(lookups)?;
        Ok(self.form.leave())
    }

    /// The save finished; the grid keeps its cursor on the stored lines.
    pub fn save_ok(&mut self, stored: V, lookups: &dyn Lookups) -> AfterSave {
        let after = self.form.save_ok(stored);
        self.with(lookups, Grid::rebind);
        after
    }

    /// The save failed; the edits stay.
    pub fn save_failed(&mut self, failure: SaveFailure) {
        self.form.save_failed(failure);
    }

    /// The save's outcome is unknown: Save stays blocked until a recheck
    /// answers or the owner confirms (B2).
    pub fn save_unknown(&mut self, failure: SaveFailure) {
        self.form.save_unknown(failure);
    }

    /// The form state, for the unknown-save rule (B2).
    pub fn form_mut(&mut self) -> &mut FormState<V> {
        &mut self.form
    }

    /// Show another record (Prev/Next, Find, Save & New).
    pub fn load(&mut self, record: V, lookups: &dyn Lookups) {
        self.form.load(record);
        self.with(lookups, Grid::reset);
    }

    /// Throw the unsaved edits away (Clear, Discard).
    pub fn revert_form(&mut self, lookups: &dyn Lookups) {
        self.form.revert_form();
        self.with(lookups, Grid::reset);
    }

    /// Move the cursor to a cell and focus the grid next frame.
    pub fn focus(&mut self, pos: CellPos, lookups: &dyn Lookups) {
        self.with(lookups, |g, m, env| g.focus(pos, m, env));
    }

    /// Run a grid command (Select all, Clear selection, P16 D6) with the
    /// environment captured last frame; true when the grid acted.
    pub fn grid_command(&mut self, cmd: GridCommand, lookups: &dyn Lookups) -> bool {
        self.with(lookups, |g, m, env| g.command(cmd, m, env).0.yes())
    }

    fn undo_redo(&mut self, cmd: GridCommand, lookups: &dyn Lookups) -> Routed<V> {
        if self.grid_command(cmd, lookups) {
            return Routed::Handled;
        }
        let done = if cmd == GridCommand::Undo {
            self.form.undo()
        } else {
            self.form.redo()
        };
        if done {
            self.with(lookups, |g, m, env| {
                g.resync(m, env, true);
            });
            Routed::Handled
        } else {
            Routed::NotHandled
        }
    }

    /// The single routing table for keymap commands (plan §2.3).
    pub fn command(&mut self, id: CommandId, lookups: &dyn Lookups) -> Routed<V> {
        let simple = |handled: bool| {
            if handled {
                Routed::Handled
            } else {
                Routed::NotHandled
            }
        };
        match id.as_str() {
            "form.revert" => simple(self.grid_command(GridCommand::Revert, lookups)),
            "form.insert_line" => simple(self.grid_command(GridCommand::InsertLine, lookups)),
            "form.delete_line" => simple(self.grid_command(GridCommand::DeleteLine, lookups)),
            "edit.open_list" => simple(self.grid_command(GridCommand::OpenList, lookups)),
            "edit.undo" => self.undo_redo(GridCommand::Undo, lookups),
            "edit.redo" => self.undo_redo(GridCommand::Redo, lookups),
            "form.prev_record" | "form.next_record" => {
                let forward = id.as_str() == "form.next_record";
                if self.grid.has_focus() {
                    let cmd = if forward {
                        GridCommand::PageDown
                    } else {
                        GridCommand::PageUp
                    };
                    self.grid_command(cmd, lookups);
                    return Routed::Handled;
                }
                match self.leave(lookups) {
                    Ok(decision) => Routed::Navigate { forward, decision },
                    Err(p) => Routed::Blocked(p),
                }
            }
            "form.save_new" | "form.save_close" => {
                let intent = if id.as_str() == "form.save_new" {
                    SaveIntent::SaveAndNew
                } else {
                    SaveIntent::SaveAndClose
                };
                match self.begin_save(intent, lookups) {
                    Ok(start) => Routed::Save(start),
                    Err(p) => Routed::Blocked(p),
                }
            }
            _ => Routed::NotHandled,
        }
    }

    /// Draw the grid. Captures this frame's environment for the commands
    /// that arrive before the next frame, and keeps column widths.
    pub fn ui(&mut self, ui: &mut Ui, lookups: &dyn Lookups) -> GridOutput {
        self.ui_with(ui, lookups, None)
    }

    /// [`GridForm::ui`] with the screen's context menu entries (grids whose
    /// spec opts in, P16 D6).
    pub fn ui_with(
        &mut self,
        ui: &mut Ui,
        lookups: &dyn Lookups,
        context: Option<&dyn cl_grid::ContextMenuSource>,
    ) -> GridOutput {
        self.parts = EnvParts::capture(ui.ctx());
        if !self.widths_loaded {
            self.widths_loaded = true;
            if let Some(w) = self
                .widths_key
                .as_deref()
                .and_then(|k| grid_env::load_widths(ui.ctx(), k))
            {
                self.grid.set_column_widths(&w);
            }
        }
        let mut env = self.parts.env(lookups, &self.label, self.height);
        // While a save is in flight the grid takes no input (its edits
        // would be refused); it gets focus back when the save finishes.
        env.input_enabled &= !self.form.is_saving();
        env.context = context;
        let mut model = self.adapter.model(&mut self.form);
        let out = self.grid.show(ui, ModelRef::Edit(&mut *model), &env);
        self.snapshot = self.grid.snapshot(&ModelRef::Edit(&mut *model));
        drop(model);
        if out
            .events
            .iter()
            .any(|e| matches!(e, cl_grid::GridEvent::ColumnsResized))
            && let Some(key) = &self.widths_key
        {
            grid_env::store_widths(ui.ctx(), key, &self.grid.column_widths());
        }
        out
    }
}

#[cfg(test)]
mod tests;
