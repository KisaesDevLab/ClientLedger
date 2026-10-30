//! The editing state machine (P10 plan Rev 2 §2.5).
//!
//! [`Core`] holds the cursor, the open editor's buffer, popups, selection
//! and pending rows. [`Core::handle`] applies one [`GridInput`]: model
//! changes go through the [`EditableModel`] trait (each commit is one
//! non-merging undo step there), and everything the view must do is returned
//! as [`Effects`]. It is deterministic and has no egui, clock or I/O, so
//! every row of the plan's tables is unit-tested.

use std::collections::BTreeSet;
use std::fmt;

use cl_model::TextKey;
use cl_model::date_entry;
use cl_model::date_rules::DateKey;
use cl_model::forms::EnterMode;
use cl_model::tags::{self, TagAdd, TagChip};

use super::codec::{
    NOT_IN_LIST, PickerResolution, TAG_NOT_IN_LIST, cell_text, parse_buffer, resolve_picker,
};
use super::context::{ContextItem, ContextMenu, ContextTarget};
use super::keys::{Formats, GridInput, GridKey, Mods};
use super::model::{
    CellInput, CellKind, CellPos, CellProblem, CellValue, ColumnId, LookupResult, Lookups,
    ModelRef, RecordOutcome, RowState, Source, Suggestion,
};
use super::rowkey::RowKey;
use super::selection::{Selection, SelectionMode};
use super::spec::{GridCommand, GridEvent, GridSpec};

/// Popup rows shown at once (paging moves by this many).
pub const POPUP_ROWS: usize = 8;
/// Most matches typing offers; the popup shows [`POPUP_ROWS`] at a time
/// and the wheel or the arrows reach the rest.
pub const TYPEAHEAD_MAX: usize = 50;
/// Shown when the cursor's row vanished after a reload.
pub const ROW_GONE: TextKey = TextKey("grid.row.gone");
/// Shown when the model changed a cell while the owner was typing in it.
pub const CHANGED_UNDERNEATH: TextKey = TextKey("grid.cell.changed_underneath");

/// Which list a popup shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PopupKind {
    /// Type-ahead under the cell.
    List,
    /// Ctrl+L: every entry.
    Full,
    /// A click on the cell (Q-250): every entry, starting on the cell's
    /// own. Nothing counts as chosen until the owner moves in the list or
    /// clicks an entry (it then becomes [`PopupKind::Full`]).
    Browse,
    /// A dropdown column's choices.
    Choices,
}

/// An open popup. `Debug` prints counts and flags only (no names).
#[derive(Clone, PartialEq, Eq)]
pub struct Popup {
    /// What it lists.
    pub kind: PopupKind,
    /// The entries.
    pub items: Vec<Suggestion>,
    /// The highlighted entry.
    pub highlight: Option<usize>,
    /// The list is still loading.
    pub loading: bool,
    /// "Add {text}" is offered after the entries.
    pub add_new: Option<String>,
}

/// The open editor on the cursor cell. `Debug` never prints the buffer,
/// the origin text or chip names.
#[derive(Clone, PartialEq, Eq)]
pub struct Edit {
    /// The text being edited.
    pub buffer: String,
    /// The text the cell had when the editor opened (or last resynced).
    pub origin: String,
    /// The model revision `origin` came from.
    pub origin_rev: u64,
    /// The editor was just opened: its text is selected, so the first typed
    /// text replaces it.
    pub fresh: bool,
    /// An open popup.
    pub popup: Option<Popup>,
    /// The problem shown under the cell.
    pub error: Option<CellProblem>,
    /// A caption shown under the cell (tag replaced, changed underneath).
    pub caption: Option<TextKey>,
    /// The cell's chips (tag cells).
    pub chips: Vec<TagChip>,
}

impl fmt::Debug for Popup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Popup")
            .field("kind", &self.kind)
            .field("items", &self.items.len())
            .field("highlight", &self.highlight)
            .field("loading", &self.loading)
            .field("add_new", &self.add_new.is_some())
            .finish()
    }
}

impl fmt::Debug for Edit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Edit")
            .field("dirty", &self.is_dirty())
            .field("origin_rev", &self.origin_rev)
            .field("fresh", &self.fresh)
            .field("popup", &self.popup)
            .field("error", &self.error)
            .field("caption", &self.caption)
            .field("chips", &self.chips.len())
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for Effects {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Effects")
            .field("events", &self.events)
            .field("handled", &self.handled)
            .field("scroll_to", &self.scroll_to)
            .field("leave", &self.leave)
            .field("copy", &self.copy.is_some())
            .field("reset_editor", &self.reset_editor)
            .finish()
    }
}

impl Edit {
    /// The buffer differs from the cell's value.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.buffer != self.origin
    }
}

/// What the view must do after an input. `Debug` never prints the
/// clipboard text.
#[derive(Default, Clone, PartialEq, Eq)]
pub struct Effects {
    /// Events for the screen.
    pub events: Vec<GridEvent>,
    /// The input did something (commands: `Handled`).
    pub handled: bool,
    /// Bring this row into view.
    pub scroll_to: Option<RowKey>,
    /// Leave the grid: Tab (true) or Shift+Tab (false).
    pub leave: Option<bool>,
    /// Put this text on the clipboard.
    pub copy: Option<String>,
    /// A different cell's editor opened: reset the `TextEdit` state (select
    /// all, clear its undo, interrupt IME).
    pub reset_editor: bool,
}

impl Effects {
    fn handled() -> Effects {
        Effects {
            handled: true,
            ..Effects::default()
        }
    }

    /// `later` happened after `self` in the same input: events in order,
    /// the later scroll, leave and copy win.
    fn then(mut self, later: Effects) -> Effects {
        self.events.extend(later.events);
        self.handled |= later.handled;
        self.scroll_to = later.scroll_to.or(self.scroll_to);
        self.leave = later.leave.or(self.leave);
        self.copy = later.copy.or(self.copy);
        self.reset_editor |= later.reset_editor;
        self
    }
}

/// The grid's editing state. `Debug` goes through the redacting `Edit`.
#[derive(Debug, Clone)]
pub struct Core {
    spec: GridSpec,
    cursor: Option<CellPos>,
    edit: Option<Edit>,
    parked: bool,
    pending: BTreeSet<RowKey>,
    selection: Selection,
    focused: bool,
    entered: Option<RowKey>,
    page_rows: usize,
    notice: Option<TextKey>,
    last_index: usize,
    menu: Option<ContextMenu>,
}

/// Where Tab-style commits come from: only Tab accepts an AI suggestion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Accept {
    Tab,
    Other,
}

impl Core {
    /// A grid with no cursor yet.
    #[must_use]
    pub fn new(spec: GridSpec) -> Core {
        Core {
            spec,
            cursor: None,
            edit: None,
            parked: false,
            pending: BTreeSet::new(),
            selection: Selection::new(),
            focused: false,
            entered: None,
            page_rows: 10,
            notice: None,
            last_index: 0,
            menu: None,
        }
    }

    /// The open context menu.
    #[must_use]
    pub fn menu(&self) -> Option<&ContextMenu> {
        self.menu.as_ref()
    }

    /// Close the context menu; true when one was open.
    pub fn close_menu(&mut self) -> bool {
        self.menu.take().is_some()
    }

    /// Get ready to open a context menu on `target`
    /// ([`GridSpec::context_menu`] only): close any menu, commit the open
    /// cell like a click does (so the menu acts on the value the owner
    /// sees), close its list, and for a cell move the cursor there. False
    /// when no menu may open (not opted in, an invalid cell keeps the
    /// cursor, or the row is gone); the view then asks the screen for items
    /// and calls [`Core::open_menu`].
    pub fn prepare_menu(
        &mut self,
        target: ContextTarget,
        model: &mut ModelRef<'_>,
        lookups: &dyn Lookups,
        fmt: Formats,
    ) -> (bool, Effects) {
        if !self.spec.context_menu {
            return (false, Effects::default());
        }
        self.menu = None;
        self.focused = true;
        let mut fx = Effects::handled();
        if self
            .commit(model, lookups, fmt, Accept::Other, &mut fx)
            .is_err()
        {
            return (false, fx); // stay with the error (S11)
        }
        self.close_popup();
        if let ContextTarget::Cell(pos) = target {
            if model.rows().index_of(pos.row).is_none() {
                return (false, fx);
            }
            if self.cursor != Some(pos) {
                self.move_to(pos, model, fmt, &mut fx);
            }
        }
        (true, fx)
    }

    /// Open the context menu with the screen's `items` (after
    /// [`Core::prepare_menu`]). An empty list opens nothing; false then.
    pub fn open_menu(&mut self, target: ContextTarget, items: Vec<ContextItem>) -> bool {
        if !self.spec.context_menu || items.is_empty() {
            return false;
        }
        self.close_popup();
        self.menu = Some(ContextMenu::new(target, items));
        true
    }

    /// Input while the context menu is open: Up/Down/Home/End move the
    /// highlight, Enter or Space picks, Esc, Tab and Shift+F10 close it,
    /// typing is ignored. Anything else (a click elsewhere, focus lost,
    /// another command) closes the menu and goes on (`None`).
    fn menu_input(&mut self, input: &GridInput) -> Option<Effects> {
        let menu = self.menu.as_mut()?;
        match input {
            GridInput::Key(GridKey::Up | GridKey::Down, Mods::NONE) => {
                menu.step(matches!(input, GridInput::Key(GridKey::Down, _)));
                Some(Effects::handled())
            }
            GridInput::Key(GridKey::Home | GridKey::End, _) => {
                menu.jump(matches!(input, GridInput::Key(GridKey::End, _)));
                Some(Effects::handled())
            }
            GridInput::Key(GridKey::Enter | GridKey::Space, Mods::NONE) => {
                let at = menu.highlight;
                Some(self.menu_pick(at))
            }
            GridInput::MenuPicked(i) => {
                let at = Some(*i);
                Some(self.menu_pick(at))
            }
            GridInput::Key(GridKey::Escape | GridKey::Tab | GridKey::ContextMenu, _)
            | GridInput::Command(GridCommand::Revert) => {
                self.menu = None;
                Some(Effects::handled())
            }
            GridInput::Text(_) => Some(Effects::handled()),
            _ => {
                self.menu = None;
                None
            }
        }
    }

    /// Pick the entry at `at` when it is enabled: the screen gets
    /// [`GridEvent::ContextPicked`] and the menu closes.
    fn menu_pick(&mut self, at: Option<usize>) -> Effects {
        let mut fx = Effects::handled();
        let Some(menu) = self.menu.as_ref() else {
            return fx;
        };
        if let Some(item) = at.and_then(|i| menu.pickable(i)) {
            fx.events.push(GridEvent::ContextPicked {
                target: menu.target,
                item: item.id,
            });
            self.menu = None;
        }
        fx
    }

    /// The spec.
    #[must_use]
    pub fn spec(&self) -> &GridSpec {
        &self.spec
    }

    /// The cursor cell.
    #[must_use]
    pub fn cursor(&self) -> Option<CellPos> {
        self.cursor
    }

    /// The open editor.
    #[must_use]
    pub fn edit(&self) -> Option<&Edit> {
        self.edit.as_ref()
    }

    /// The open editor, for the view's `TextEdit` (it edits `buffer` in
    /// place and then sends [`GridInput::BufferEdited`]).
    pub fn edit_mut(&mut self) -> Option<&mut Edit> {
        self.edit.as_mut()
    }

    /// An invalid edit waits for the grid to get focus back.
    #[must_use]
    pub fn is_parked(&self) -> bool {
        self.parked
    }

    /// Rows whose save is in flight.
    #[must_use]
    pub fn pending(&self) -> &BTreeSet<RowKey> {
        &self.pending
    }

    /// The selection.
    #[must_use]
    pub fn selection(&self) -> &Selection {
        &self.selection
    }

    /// Logical focus.
    #[must_use]
    pub fn has_focus(&self) -> bool {
        self.focused
    }

    /// A grid-level notice ("that row is gone").
    #[must_use]
    pub fn notice(&self) -> Option<TextKey> {
        self.notice
    }

    /// The editor has text that is not in the model yet.
    #[must_use]
    pub fn has_uncommitted_edit(&self) -> bool {
        self.edit.as_ref().is_some_and(Edit::is_dirty)
    }

    /// Show a problem under the open cell (a refused paste).
    pub fn show_problem(&mut self, p: CellProblem) {
        self.set_error(p);
    }

    /// Whole rows per page (the view sets it from the viewport).
    pub fn set_page_rows(&mut self, rows: usize) {
        self.page_rows = rows.max(1);
    }

    /// Put the cursor on a cell (keyboard focus is the view's job).
    pub fn set_cursor(&mut self, pos: CellPos, model: &mut ModelRef<'_>, fmt: Formats) -> Effects {
        let mut fx = Effects::handled();
        self.move_to(pos, model, fmt, &mut fx);
        fx
    }

    fn column_kind(&self, col: ColumnId) -> Option<CellKind> {
        self.spec.column(col).map(|c| c.kind)
    }

    fn col_order(&self) -> Vec<ColumnId> {
        self.spec.columns.iter().map(|c| c.id).collect()
    }

    fn cell_is_editable(&self, model: &ModelRef<'_>, index: usize, col: ColumnId) -> bool {
        let ModelRef::Edit(m) = model else {
            return false;
        };
        if !self.spec.editable {
            return false;
        }
        let Some(key) = m.row_key(index) else {
            return false;
        };
        !self.pending.contains(&key)
            && !m.row_state(index).contains(RowState::LOCKED)
            && self.spec.column(col).is_some_and(|c| c.editable)
            && m.cell_editable(index, col)
    }

    fn locked(model: &ModelRef<'_>, index: usize) -> bool {
        model.rows().row_state(index).contains(RowState::LOCKED)
    }

    fn cursor_index(&self, model: &ModelRef<'_>) -> Option<usize> {
        self.cursor.and_then(|c| model.rows().index_of(c.row))
    }

    /// Open (or not) the editor on the cursor cell.
    fn open_editor(&mut self, model: &mut ModelRef<'_>, fmt: Formats, fx: &mut Effects) {
        self.edit = None;
        self.parked = false;
        let Some(pos) = self.cursor else { return };
        let Some(index) = model.rows().index_of(pos.row) else {
            return;
        };
        self.last_index = index;
        if !self.cell_is_editable(model, index, pos.col) {
            return;
        }
        let rows = model.rows();
        let value = rows.cell(index, pos.col);
        let text = cell_text(&value, fmt);
        let chips = match value {
            CellValue::Tags(t) => t.to_vec(),
            _ => Vec::new(),
        };
        self.edit = Some(Edit {
            buffer: text.clone(),
            origin: text,
            origin_rev: rows.revision(),
            fresh: true,
            popup: None,
            error: None,
            caption: None,
            chips,
        });
        fx.reset_editor = true;
    }

    fn move_to(&mut self, pos: CellPos, model: &mut ModelRef<'_>, fmt: Formats, fx: &mut Effects) {
        let row_changed = self.entered != Some(pos.row);
        self.cursor = Some(pos);
        self.notice = None;
        if row_changed && let Some(m) = model.edit() {
            m.row_entered(pos.row);
            self.entered = Some(pos.row);
        }
        self.open_editor(model, fmt, fx);
        fx.scroll_to = Some(pos.row);
    }

    /// The first (or last) editable cell of row `index`, else its first
    /// (last) column.
    fn edge_cell(&self, model: &ModelRef<'_>, index: usize, last: bool) -> Option<CellPos> {
        let key = model.rows().row_key(index)?;
        let mut cols = self.col_order();
        if last {
            cols.reverse();
        }
        let col = cols
            .iter()
            .copied()
            .find(|c| self.cell_is_editable(model, index, *c))
            .or_else(|| cols.first().copied())?;
        Some(CellPos { row: key, col })
    }

    /// The next (previous) editable cell after the cursor, across rows.
    fn step_cell(&self, model: &ModelRef<'_>, forward: bool) -> Option<CellPos> {
        let pos = self.cursor?;
        let index = model.rows().index_of(pos.row)?;
        let cols = self.col_order();
        let ci = cols.iter().position(|c| *c == pos.col)?;
        let n = model.rows().row_count();
        let mut row = index;
        let mut col = ci;
        loop {
            if forward {
                if col + 1 < cols.len() {
                    col += 1;
                } else if row + 1 < n {
                    row += 1;
                    col = 0;
                } else {
                    return None;
                }
            } else if col > 0 {
                col -= 1;
            } else if row > 0 {
                row -= 1;
                col = cols.len() - 1;
            } else {
                return None;
            }
            if self.cell_is_editable(model, row, cols[col]) {
                return Some(CellPos {
                    row: model.rows().row_key(row)?,
                    col: cols[col],
                });
            }
        }
    }

    /// No editable cell follows the cursor in its row (Enter there records
    /// the row when Enter moves between fields, Q-264).
    fn at_last_editable_cell(&self, model: &ModelRef<'_>) -> bool {
        let Some(pos) = self.cursor else { return true };
        let Some(index) = model.rows().index_of(pos.row) else {
            return true;
        };
        let cols = self.col_order();
        let Some(ci) = cols.iter().position(|c| *c == pos.col) else {
            return true;
        };
        !cols[ci + 1..]
            .iter()
            .any(|c| self.cell_is_editable(model, index, *c))
    }

    fn row_at(model: &ModelRef<'_>, index: usize, col: ColumnId) -> Option<CellPos> {
        let key = model.rows().row_key(index)?;
        Some(CellPos { row: key, col })
    }

    // ---- committing ------------------------------------------------------

    fn set_error(&mut self, p: CellProblem) {
        if let Some(e) = self.edit.as_mut() {
            e.error = Some(p);
        }
    }

    fn commit_value(
        &mut self,
        model: &mut ModelRef<'_>,
        input: CellInput,
        source: Source,
        fx: &mut Effects,
    ) -> Result<(), CellProblem> {
        let Some(pos) = self.cursor else {
            return Ok(());
        };
        let Some(m) = model.edit() else { return Ok(()) };
        match m.set_cell(pos, input, source) {
            Ok(()) => {
                fx.events.push(GridEvent::CellCommitted { pos, source });
                Ok(())
            }
            Err(p) => {
                let p = CellProblem {
                    col: Some(pos.col),
                    ..p
                };
                self.set_error(p);
                Err(p)
            }
        }
    }

    /// The chosen popup entry, when one is highlighted.
    fn highlighted(&self) -> Option<Suggestion> {
        let popup = self.edit.as_ref()?.popup.as_ref()?;
        let i = popup.highlight?;
        popup.items.get(i).cloned()
    }

    // One arm per cell kind keeps the commit rules readable in one place.
    #[allow(clippy::too_many_lines)]
    /// Commit the open editor. `Ok(())` when there was nothing to commit or
    /// the value went into the model; `Err` keeps the editor open with the
    /// problem shown.
    fn commit(
        &mut self,
        model: &mut ModelRef<'_>,
        lookups: &dyn Lookups,
        fmt: Formats,
        accept: Accept,
        fx: &mut Effects,
    ) -> Result<(), CellProblem> {
        let (Some(pos), Some(edit)) = (self.cursor, self.edit.as_ref()) else {
            return Ok(());
        };
        let Some(kind) = self.column_kind(pos.col) else {
            return Ok(());
        };
        let Some(index) = model.rows().index_of(pos.row) else {
            return Ok(());
        };
        let dirty = edit.is_dirty();
        let buffer = edit.buffer.clone();
        // An entry the owner moved to in the full list (Ctrl+L) counts as
        // chosen even though nothing was typed.
        let full_pick = edit
            .popup
            .as_ref()
            .is_some_and(|p| p.kind == PopupKind::Full && p.highlight.is_some());
        let result = match kind {
            CellKind::Picker { list, .. } => {
                let picked = self.highlighted();
                if let Some(s) = picked.filter(|_| dirty || full_pick) {
                    self.commit_value(model, CellInput::Ref(s.id), Source::Picked, fx)
                } else if !dirty {
                    let suggested = match model.rows().cell(index, pos.col) {
                        CellValue::Suggested { id, .. } => Some(id),
                        _ => None,
                    };
                    match suggested {
                        Some(id) if accept == Accept::Tab && buffer.is_empty() => self
                            .commit_value(
                                model,
                                CellInput::Ref(id),
                                Source::SuggestionAccepted,
                                fx,
                            ),
                        _ => Ok(()),
                    }
                } else {
                    match resolve_picker(list, &buffer, lookups) {
                        PickerResolution::Entry(id) => {
                            self.commit_value(model, CellInput::Ref(id), Source::Typed, fx)
                        }
                        PickerResolution::Empty => {
                            self.commit_value(model, CellInput::Clear, Source::Typed, fx)
                        }
                        PickerResolution::Unresolved(t) => {
                            let r = self.commit_value(
                                model,
                                CellInput::Unresolved(t),
                                Source::Typed,
                                fx,
                            );
                            if r.is_ok() {
                                self.set_error(CellProblem::new(NOT_IN_LIST).at(pos.col));
                            }
                            r
                        }
                    }
                }
            }
            CellKind::Tags { list } => {
                if buffer.trim().is_empty() && !full_pick {
                    let suggested = match model.rows().cell(index, pos.col) {
                        CellValue::Suggested { id, display } => Some((id, display.to_string())),
                        _ => None,
                    };
                    match suggested {
                        Some((id, label)) if accept == Accept::Tab => {
                            let chip = TagChip {
                                id,
                                label,
                                group: None,
                            };
                            self.add_chip(model, chip, Source::SuggestionAccepted, fx)
                        }
                        _ => Ok(()),
                    }
                } else {
                    let chosen = self.highlighted().or_else(|| lookups.exact(list, &buffer));
                    if let Some(s) = chosen {
                        let chip = TagChip {
                            id: s.id,
                            label: s.label,
                            group: s.group,
                        };
                        self.add_chip(model, chip, Source::Picked, fx)
                    } else {
                        let p = CellProblem::new(TAG_NOT_IN_LIST).at(pos.col);
                        self.set_error(p);
                        Err(p)
                    }
                }
            }
            CellKind::Dropdown { .. } => {
                let current = match model.rows().cell(index, pos.col) {
                    CellValue::Choice(id) => Some(id),
                    _ => None,
                };
                // Picking the current choice again is not an edit.
                if let Some(s) = self.highlighted().filter(|s| Some(s.id) != current) {
                    self.commit_value(model, CellInput::Choice(s.id), Source::Picked, fx)
                } else {
                    Ok(())
                }
            }
            CellKind::Checkbox | CellKind::Mark | CellKind::Display => Ok(()),
            CellKind::Text { .. }
            | CellKind::Memo { .. }
            | CellKind::Amount(_)
            | CellKind::Date => {
                if !dirty {
                    return Ok(());
                }
                match parse_buffer(kind, &buffer, fmt) {
                    Ok((input, source)) => self.commit_value(model, input, source, fx),
                    Err(p) => {
                        let p = p.at(pos.col);
                        self.set_error(p);
                        Err(p)
                    }
                }
            }
        };
        if result.is_ok() {
            self.refresh_origin(model, fmt);
            fx.reset_editor = true;
        }
        result
    }

    /// After a commit the editor shows the model's value (`1/5/99` becomes
    /// `01/05/2099`, `400+44.25` becomes `444.25`).
    fn refresh_origin(&mut self, model: &ModelRef<'_>, fmt: Formats) {
        let (Some(pos), Some(edit)) = (self.cursor, self.edit.as_mut()) else {
            return;
        };
        let Some(index) = model.rows().index_of(pos.row) else {
            return;
        };
        let value = model.rows().cell(index, pos.col);
        let text = cell_text(&value, fmt);
        if let CellValue::Tags(t) = value {
            edit.chips = t.to_vec();
            edit.buffer.clear();
            edit.origin.clear();
        } else {
            edit.buffer.clone_from(&text);
            edit.origin = text;
        }
        edit.origin_rev = model.rows().revision();
        edit.fresh = true;
        edit.popup = None;
        if edit.error.is_some_and(|e| e.key != NOT_IN_LIST) {
            edit.error = None;
        }
    }

    fn add_chip(
        &mut self,
        model: &mut ModelRef<'_>,
        chip: TagChip,
        source: Source,
        fx: &mut Effects,
    ) -> Result<(), CellProblem> {
        let Some(edit) = self.edit.as_mut() else {
            return Ok(());
        };
        let mut chips = edit.chips.clone();
        let outcome = tags::add(&mut chips, chip);
        let caption = outcome.caption_key();
        if outcome == TagAdd::Duplicate {
            edit.buffer.clear();
            edit.popup = None;
            edit.caption = caption;
            return Ok(());
        }
        let r = self.commit_value(model, CellInput::Tags(chips.clone()), source, fx);
        if let Some(edit) = self.edit.as_mut()
            && r.is_ok()
        {
            edit.chips = chips;
            edit.buffer.clear();
            edit.origin.clear();
            edit.popup = None;
            edit.caption = caption;
        }
        r
    }

    // ---- popups ----------------------------------------------------------

    fn refresh_popup(&mut self, lookups: &dyn Lookups) {
        let Some(pos) = self.cursor else { return };
        let kind = self.column_kind(pos.col);
        let Some(edit) = self.edit.as_mut() else {
            return;
        };
        let (list, add_new) = match kind {
            Some(CellKind::Picker { list, add_new }) => (list, add_new),
            Some(CellKind::Tags { list }) => (list, false),
            _ => return,
        };
        let q = edit.buffer.trim().to_string();
        if q.is_empty() {
            edit.popup = None;
            return;
        }
        edit.popup = Some(match lookups.suggest(list, &q, TYPEAHEAD_MAX) {
            LookupResult::Ready(items) => {
                let exact = lookups.exact(list, &q).is_some();
                Popup {
                    kind: PopupKind::List,
                    highlight: (!items.is_empty()).then_some(0),
                    loading: false,
                    add_new: (add_new && !exact).then_some(q),
                    items,
                }
            }
            LookupResult::Loading => Popup {
                kind: PopupKind::List,
                items: Vec::new(),
                highlight: None,
                loading: true,
                add_new: None,
            },
        });
    }

    fn move_highlight(&mut self, by: isize) -> bool {
        let Some(popup) = self.edit.as_mut().and_then(|e| e.popup.as_mut()) else {
            return false;
        };
        if popup.items.is_empty() {
            return true;
        }
        let last = popup.items.len() - 1;
        // Nothing highlighted yet (a full list): the first move lands on
        // the first or last entry.
        let next = match popup.highlight {
            Some(cur) => cur.saturating_add_signed(by).min(last),
            None if by < 0 => last,
            None => 0,
        };
        popup.highlight = Some(next);
        if popup.kind == PopupKind::Browse {
            popup.kind = PopupKind::Full;
        }
        true
    }

    fn popup_open(&self) -> bool {
        self.edit.as_ref().is_some_and(|e| e.popup.is_some())
    }

    fn close_popup(&mut self) {
        if let Some(e) = self.edit.as_mut() {
            e.popup = None;
        }
    }

    // ---- entry points ----------------------------------------------------

    /// Apply one input.
    pub fn handle(
        &mut self,
        input: GridInput,
        model: &mut ModelRef<'_>,
        lookups: &dyn Lookups,
        fmt: Formats,
    ) -> Effects {
        if self
            .cursor
            .is_some_and(|c| model.rows().index_of(c.row).is_none())
        {
            self.recover_cursor(model, fmt);
        }
        if let Some(fx) = self.menu_input(&input) {
            return fx;
        }
        match input {
            GridInput::FocusGained { from_below } => self.focus_gained(from_below, model, fmt),
            GridInput::FocusLost => self.focus_lost(model, lookups, fmt),
            GridInput::Command(c) => self.command(c, model, lookups, fmt),
            GridInput::Click { pos, double, mods } => {
                self.click(pos, double, mods, model, lookups, fmt)
            }
            GridInput::Key(k, m) => {
                if self.spec.editable {
                    self.key_edit(k, m, model, lookups, fmt)
                } else {
                    self.key_browse(k, m, model, fmt)
                }
            }
            GridInput::Text(t) => self.text(&t, model, lookups, fmt),
            GridInput::BufferEdited(t) => {
                if let Some(e) = self.edit.as_mut() {
                    e.buffer = t;
                    e.fresh = false;
                    e.error = None;
                    e.caption = None;
                }
                self.refresh_popup(lookups);
                Effects::handled()
            }
            GridInput::PopupPicked(i) => {
                if let Some(p) = self.edit.as_mut().and_then(|e| e.popup.as_mut())
                    && i < p.items.len()
                {
                    p.highlight = Some(i);
                    if p.kind == PopupKind::Browse {
                        p.kind = PopupKind::Full;
                    }
                }
                let mut fx = Effects::handled();
                let _ = self.commit(model, lookups, fmt, Accept::Other, &mut fx);
                fx
            }
            GridInput::AddNewPicked => self.add_new(model, fmt),
            GridInput::OpenChoices(items) => self.open_choices(items, model),
            GridInput::ToggleList => {
                if self.popup_open() {
                    self.close_popup();
                    Effects::handled()
                } else {
                    self.open_list(model, lookups, PopupKind::Browse)
                }
            }
            GridInput::GutterClick { row, mods } => self.gutter_click(row, mods, model),
            // A menu entry without an open menu (a stale click).
            GridInput::MenuPicked(_) => Effects::default(),
        }
    }

    // ---- selection (P16 D6) ---------------------------------------------

    /// The row at `index` can be selected: loaded and not the blank ENTRY
    /// row.
    fn selectable(model: &ModelRef<'_>, index: usize) -> Option<RowKey> {
        let rows = model.rows();
        let key = rows.row_key(index)?;
        (!rows.row_state(index).contains(RowState::ENTRY)).then_some(key)
    }

    /// A range may run onto the ENTRY row: take it out again.
    fn drop_entry_rows(&mut self, model: &ModelRef<'_>) {
        if !self.spec.entry_line {
            return;
        }
        let rows = model.rows();
        let entry: Vec<RowKey> = self
            .selection
            .iter()
            .filter(|k| {
                rows.index_of(*k)
                    .is_some_and(|i| rows.row_state(i).contains(RowState::ENTRY))
            })
            .collect();
        for k in entry {
            self.selection.remove(k);
        }
    }

    /// The select checkbox of `row`: a click (or Ctrl+click) toggles it;
    /// Shift+click selects from the anchor (the cursor row when there is
    /// none) to it. The cursor and the open cell stay.
    fn gutter_click(&mut self, row: RowKey, mods: Mods, model: &ModelRef<'_>) -> Effects {
        if !self.spec.select_column {
            return Effects::default();
        }
        let Some(index) = model.rows().index_of(row) else {
            return Effects::default();
        };
        if Self::selectable(model, index).is_none() {
            return Effects::default();
        }
        let sel_mode = self.spec.selection;
        if mods.shift {
            if let Some(from) = self.cursor {
                self.selection.anchor_if_none(from.row);
            }
            self.selection.extend_to(model.rows(), row, sel_mode);
            self.drop_entry_rows(model);
        } else {
            self.selection.toggle(row, sel_mode);
        }
        Effects {
            events: vec![GridEvent::SelectionChanged],
            ..Effects::handled()
        }
    }

    /// Select every displayed row but the ENTRY row (rows not loaded yet
    /// are skipped). Multi-row selection only; with no rows it does
    /// nothing.
    fn select_all(&mut self, model: &ModelRef<'_>) -> Effects {
        if self.spec.selection != SelectionMode::Multi {
            return Effects::default();
        }
        let n = model.rows().row_count();
        let keys: Vec<RowKey> = (0..n).filter_map(|i| Self::selectable(model, i)).collect();
        let mut fx = Effects::handled();
        if self.selection.select_only(keys) {
            fx.events.push(GridEvent::SelectionChanged);
        }
        fx
    }

    /// Shift+Up/Down in an editable grid with a select column: move like
    /// the arrow (the open cell commits first) and extend the selection
    /// from the anchor (the row the cursor was on) to the new row.
    fn extend_vertical(
        &mut self,
        by: isize,
        model: &mut ModelRef<'_>,
        lookups: &dyn Lookups,
        fmt: Formats,
    ) -> Effects {
        let Some(from) = self.cursor else {
            return Effects::handled();
        };
        let mut fx = self.vertical(by, model, lookups, fmt);
        let Some(to) = self.cursor.filter(|c| c.row != from.row) else {
            return fx;
        };
        if let Some(i) = model.rows().index_of(from.row)
            && Self::selectable(model, i).is_some()
        {
            self.selection.anchor_if_none(from.row);
        }
        self.selection
            .extend_to(model.rows(), to.row, self.spec.selection);
        self.drop_entry_rows(model);
        fx.events.push(GridEvent::SelectionChanged);
        fx
    }

    fn open_choices(&mut self, items: Vec<Suggestion>, model: &ModelRef<'_>) -> Effects {
        let Some(pos) = self.cursor else {
            return Effects::default();
        };
        if !matches!(self.column_kind(pos.col), Some(CellKind::Dropdown { .. })) {
            return Effects::default();
        }
        let current =
            model
                .rows()
                .index_of(pos.row)
                .and_then(|i| match model.rows().cell(i, pos.col) {
                    CellValue::Choice(id) => items.iter().position(|s| s.id == id),
                    _ => None,
                });
        let Some(edit) = self.edit.as_mut() else {
            return Effects::default();
        };
        edit.popup = Some(Popup {
            kind: PopupKind::Choices,
            highlight: current.or((!items.is_empty()).then_some(0)),
            loading: false,
            add_new: None,
            items,
        });
        Effects::handled()
    }

    fn add_new(&mut self, model: &mut ModelRef<'_>, fmt: Formats) -> Effects {
        let mut fx = Effects::handled();
        let Some(pos) = self.cursor else { return fx };
        let Some(text) = self
            .edit
            .as_ref()
            .and_then(|e| e.popup.as_ref())
            .and_then(|p| p.add_new.clone())
        else {
            return fx;
        };
        self.close_popup();
        fx.events.push(GridEvent::AddNew {
            pos,
            text: text.clone(),
        });
        let _ = self.commit_value(model, CellInput::Unresolved(text), Source::Typed, &mut fx);
        self.refresh_origin(model, fmt);
        self.set_error(CellProblem::new(NOT_IN_LIST).at(pos.col));
        fx
    }

    fn recover_cursor(&mut self, model: &mut ModelRef<'_>, fmt: Formats) {
        let n = model.rows().row_count();
        let col = self.cursor.map(|c| c.col);
        self.edit = None;
        self.parked = false;
        self.entered = None;
        let mut fx = Effects::default();
        if n == 0 {
            self.cursor = None;
        } else if let (Some(pos), Some(col)) = (
            Self::row_at(model, self.last_index.min(n - 1), ColumnId(0)),
            col,
        ) {
            self.move_to(CellPos { col, ..pos }, model, fmt, &mut fx);
        }
        self.notice = Some(ROW_GONE);
    }

    fn focus_gained(
        &mut self,
        from_below: bool,
        model: &mut ModelRef<'_>,
        fmt: Formats,
    ) -> Effects {
        self.focused = true;
        let mut fx = Effects::handled();
        if self.parked {
            self.parked = false;
            return fx;
        }
        if self.cursor.is_none() {
            let n = model.rows().row_count();
            if n > 0 {
                let target = if from_below {
                    self.edge_cell(model, n - 1, true)
                } else {
                    self.edge_cell(model, 0, false)
                };
                if let Some(t) = target {
                    self.move_to(t, model, fmt, &mut fx);
                }
            }
        } else if from_below {
            let n = model.rows().row_count();
            if n > 0
                && let Some(t) = self.edge_cell(model, n - 1, true)
            {
                self.move_to(t, model, fmt, &mut fx);
            }
        } else if self.edit.is_none() {
            self.open_editor(model, fmt, &mut fx);
        }
        fx
    }

    fn focus_lost(
        &mut self,
        model: &mut ModelRef<'_>,
        lookups: &dyn Lookups,
        fmt: Formats,
    ) -> Effects {
        self.focused = false;
        let mut fx = Effects::handled();
        // Commit before closing the list: a highlighted entry follows the
        // Tab rule, as on every save path (a mouse Save loses focus first).
        if self
            .commit(model, lookups, fmt, Accept::Other, &mut fx)
            .is_err()
        {
            self.parked = true;
        }
        self.close_popup();
        fx
    }

    /// Commit before a save, leave or record navigation (B4). An open picker
    /// follows the Tab rule except that a suggestion is never accepted.
    ///
    /// # Errors
    /// The open cell's text is invalid: the editor stays on it with the
    /// problem shown and the save must not start.
    pub fn commit_pending(
        &mut self,
        model: &mut ModelRef<'_>,
        lookups: &dyn Lookups,
        fmt: Formats,
    ) -> (Result<(), CellProblem>, Effects) {
        let mut fx = Effects::handled();
        let r = self.commit(model, lookups, fmt, Accept::Other, &mut fx);
        if r.is_err() {
            self.parked = false;
            if let Some(pos) = self.cursor {
                fx.scroll_to = Some(pos.row);
            }
        }
        (r, fx)
    }

    fn command(
        &mut self,
        c: GridCommand,
        model: &mut ModelRef<'_>,
        lookups: &dyn Lookups,
        fmt: Formats,
    ) -> Effects {
        match c {
            GridCommand::Commit => self.commit_pending(model, lookups, fmt).1,
            GridCommand::Revert => self.revert(model, fmt),
            GridCommand::InsertLine => self.insert_line(model, lookups, fmt),
            GridCommand::DeleteLine => self.delete_line(model, fmt),
            GridCommand::OpenList => self.open_list(model, lookups, PopupKind::Full),
            GridCommand::PageUp => self.page(false, model, lookups, fmt),
            GridCommand::PageDown => self.page(true, model, lookups, fmt),
            GridCommand::Undo => match self.edit.as_mut() {
                Some(e) if e.is_dirty() => {
                    e.buffer.clone_from(&e.origin);
                    e.error = None;
                    e.popup = None;
                    e.fresh = true;
                    Effects {
                        reset_editor: true,
                        ..Effects::handled()
                    }
                }
                _ => Effects::default(),
            },
            GridCommand::Redo => Effects::default(),
            GridCommand::Copy => self.copy(model, fmt),
            GridCommand::SelectAll => self.select_all(model),
            GridCommand::ClearSelection => {
                if self.selection.clear() {
                    Effects {
                        events: vec![GridEvent::SelectionChanged],
                        ..Effects::handled()
                    }
                } else {
                    Effects::handled()
                }
            }
        }
    }

    fn copy(&self, model: &ModelRef<'_>, fmt: Formats) -> Effects {
        let Some(pos) = self.cursor else {
            return Effects::default();
        };
        let Some(index) = model.rows().index_of(pos.row) else {
            return Effects::default();
        };
        Effects {
            copy: Some(cell_text(&model.rows().cell(index, pos.col), fmt)),
            ..Effects::handled()
        }
    }

    fn revert(&mut self, model: &mut ModelRef<'_>, fmt: Formats) -> Effects {
        if self.popup_open() {
            self.close_popup();
            return Effects::handled();
        }
        // "Not in the list" stays while the committed value is unresolved:
        // Esc on such a clean cell goes on to the row revert (N5).
        if let Some(e) = self.edit.as_mut()
            && (e.is_dirty() || e.error.is_some_and(|p| p.key != NOT_IN_LIST))
        {
            e.buffer.clone_from(&e.origin);
            e.error = e.error.filter(|p| p.key == NOT_IN_LIST);
            e.caption = None;
            e.fresh = true;
            return Effects {
                reset_editor: true,
                ..Effects::handled()
            };
        }
        if !self.spec.editable {
            return if self.selection.clear() {
                Effects {
                    events: vec![GridEvent::SelectionChanged],
                    ..Effects::handled()
                }
            } else {
                Effects::default()
            };
        }
        let Some(pos) = self.cursor else {
            return Effects::default();
        };
        let reverted = model.edit().is_some_and(|m| m.revert_row(pos.row));
        if reverted {
            let mut fx = Effects::handled();
            self.open_editor(model, fmt, &mut fx);
            fx
        } else if self.selection.clear() {
            Effects {
                events: vec![GridEvent::SelectionChanged],
                ..Effects::handled()
            }
        } else {
            Effects::default()
        }
    }

    fn pristine_entry(&self, model: &ModelRef<'_>, index: usize) -> bool {
        model.rows().row_state(index).contains(RowState::ENTRY)
            && !self.edit.as_ref().is_some_and(Edit::is_dirty)
    }

    fn insert_line(
        &mut self,
        model: &mut ModelRef<'_>,
        lookups: &dyn Lookups,
        fmt: Formats,
    ) -> Effects {
        if !self.spec.insert_delete {
            return Effects::default();
        }
        let Some(pos) = self.cursor else {
            return Effects::default();
        };
        let Some(index) = self.cursor_index(model) else {
            return Effects::default();
        };
        if Self::locked(model, index) {
            return Effects {
                events: vec![GridEvent::LockedRowEditAttempt { key: pos.row }],
                ..Effects::handled()
            };
        }
        let mut fx = Effects::handled();
        // A valid buffer is committed first; an invalid one is discarded.
        if self
            .commit(model, lookups, fmt, Accept::Other, &mut fx)
            .is_err()
        {
            self.edit = None;
        }
        let Some(m) = model.edit() else { return fx };
        if let Some(new) = m.insert_line(Some(pos.row))
            && let Some(ni) = model.rows().index_of(new)
            && let Some(t) = self.edge_cell(model, ni, false)
        {
            self.entered = None;
            self.move_to(t, model, fmt, &mut fx);
        }
        fx
    }

    fn delete_line(&mut self, model: &mut ModelRef<'_>, fmt: Formats) -> Effects {
        if !self.spec.insert_delete {
            return Effects::default();
        }
        let Some(pos) = self.cursor else {
            return Effects::default();
        };
        let Some(index) = self.cursor_index(model) else {
            return Effects::default();
        };
        if Self::locked(model, index) {
            return Effects {
                events: vec![GridEvent::LockedRowEditAttempt { key: pos.row }],
                ..Effects::handled()
            };
        }
        if self.pristine_entry(model, index) {
            return Effects::handled();
        }
        self.edit = None;
        let mut fx = Effects::handled();
        let Some(m) = model.edit() else { return fx };
        if m.delete_line(pos.row) {
            let n = model.rows().row_count();
            if n > 0
                && let Some(t) = Self::row_at(model, index.min(n - 1), pos.col)
            {
                self.entered = None;
                self.move_to(t, model, fmt, &mut fx);
            } else {
                self.cursor = None;
            }
        } else {
            self.open_editor(model, fmt, &mut fx);
        }
        fx
    }

    /// Every entry of the cursor cell's list: Ctrl+L ([`PopupKind::Full`])
    /// or a click ([`PopupKind::Browse`]).
    fn open_list(
        &mut self,
        model: &ModelRef<'_>,
        lookups: &dyn Lookups,
        kind: PopupKind,
    ) -> Effects {
        let Some(pos) = self.cursor else {
            return Effects::default();
        };
        // The full list starts on the cell's current entry, or on nothing:
        // a save or Tab right after Ctrl+L must not pick the first entry
        // the owner never chose (B4).
        let current =
            model
                .rows()
                .index_of(pos.row)
                .and_then(|i| match model.rows().cell(i, pos.col) {
                    CellValue::Ref { id, .. } => Some(id),
                    _ => None,
                });
        let Some(CellKind::Picker { list, .. } | CellKind::Tags { list }) =
            self.column_kind(pos.col)
        else {
            return Effects::default();
        };
        let Some(edit) = self.edit.as_mut() else {
            return Effects::default();
        };
        edit.popup = Some(match lookups.all(list) {
            LookupResult::Ready(items) => Popup {
                kind,
                highlight: current.and_then(|id| items.iter().position(|s| s.id == id)),
                loading: false,
                add_new: None,
                items,
            },
            LookupResult::Loading => Popup {
                kind,
                items: Vec::new(),
                highlight: None,
                loading: true,
                add_new: None,
            },
        });
        Effects::handled()
    }

    /// A click opens a list cell's list at once (Q-250); Tab and the arrows
    /// never do. Locked cells have no editor, so nothing opens there.
    fn open_on_click(&mut self, model: &ModelRef<'_>, lookups: &dyn Lookups, fx: &mut Effects) {
        let Some(pos) = self.cursor else { return };
        let list_cell = matches!(
            self.column_kind(pos.col),
            Some(CellKind::Picker { .. } | CellKind::Tags { .. })
        );
        if list_cell && self.edit.is_some() && !self.popup_open() && !self.parked {
            let opened = self.open_list(model, lookups, PopupKind::Browse);
            fx.handled |= opened.handled;
        }
    }

    fn page(
        &mut self,
        forward: bool,
        model: &mut ModelRef<'_>,
        lookups: &dyn Lookups,
        fmt: Formats,
    ) -> Effects {
        if self.popup_open() {
            let by = isize::try_from(POPUP_ROWS).unwrap_or(8);
            self.move_highlight(if forward { by } else { -by });
            return Effects::handled();
        }
        let rows = isize::try_from(self.page_rows).unwrap_or(isize::MAX);
        self.vertical(if forward { rows } else { -rows }, model, lookups, fmt)
    }

    /// Up/Down/Page: commit, then the same column `by` rows away (clamped).
    fn vertical(
        &mut self,
        by: isize,
        model: &mut ModelRef<'_>,
        lookups: &dyn Lookups,
        fmt: Formats,
    ) -> Effects {
        let mut fx = Effects::handled();
        let Some(pos) = self.cursor else {
            return fx;
        };
        let Some(index) = self.cursor_index(model) else {
            return fx;
        };
        if self
            .commit(model, lookups, fmt, Accept::Other, &mut fx)
            .is_err()
        {
            return fx;
        }
        let n = model.rows().row_count();
        if n == 0 {
            return fx;
        }
        let target = index.saturating_add_signed(by).min(n - 1);
        if target != index
            && let Some(t) = Self::row_at(model, target, pos.col)
        {
            self.move_to(t, model, fmt, &mut fx);
        }
        fx
    }

    fn jump_to_row(
        &mut self,
        last: bool,
        model: &mut ModelRef<'_>,
        lookups: &dyn Lookups,
        fmt: Formats,
    ) -> Effects {
        let n = model.rows().row_count();
        let Some(index) = self.cursor_index(model) else {
            return Effects::handled();
        };
        let target = if last { n.saturating_sub(1) } else { 0 };
        let by =
            isize::try_from(target).unwrap_or(isize::MAX) - isize::try_from(index).unwrap_or(0);
        self.vertical(by, model, lookups, fmt)
    }

    fn tab(
        &mut self,
        forward: bool,
        model: &mut ModelRef<'_>,
        lookups: &dyn Lookups,
        fmt: Formats,
    ) -> Effects {
        let mut fx = Effects::handled();
        let accept = if forward { Accept::Tab } else { Accept::Other };
        if self.commit(model, lookups, fmt, accept, &mut fx).is_err() {
            return fx;
        }
        if let Some(t) = self.step_cell(model, forward) {
            self.move_to(t, model, fmt, &mut fx);
        } else {
            self.focused = false;
            fx.leave = Some(forward);
            fx.events.push(GridEvent::LeftGrid { forward });
        }
        fx
    }

    fn enter(&mut self, model: &mut ModelRef<'_>, lookups: &dyn Lookups, fmt: Formats) -> Effects {
        let mut fx = Effects::handled();
        let Some(pos) = self.cursor else { return fx };
        if self.popup_open() {
            let _ = self.commit(model, lookups, fmt, Accept::Other, &mut fx);
            self.close_popup();
            return fx;
        }
        let Some(index) = self.cursor_index(model) else {
            return fx;
        };
        if self.pristine_entry(model, index) {
            fx.events.push(GridEvent::EnterOnEmptyEntry);
            return fx;
        }
        if self
            .commit(model, lookups, fmt, Accept::Other, &mut fx)
            .is_err()
        {
            return fx;
        }
        let Some(m) = model.edit() else { return fx };
        match m.record_row(pos.row) {
            RecordOutcome::Recorded { next } => {
                fx.events.push(GridEvent::RowRecorded { key: pos.row });
                let target = next
                    .and_then(|k| model.rows().index_of(k))
                    .or_else(|| model.rows().index_of(pos.row).map(|i| i + 1))
                    .filter(|i| *i < model.rows().row_count());
                if let Some(t) = target.and_then(|i| self.edge_cell(model, i, false)) {
                    self.move_to(t, model, fmt, &mut fx);
                }
            }
            RecordOutcome::Invalid(problems) => {
                fx.events.push(GridEvent::RecordRefused { key: pos.row });
                if let Some(first) = problems.first() {
                    if let Some(col) = first.col {
                        self.move_to(CellPos { row: pos.row, col }, model, fmt, &mut fx);
                    }
                    self.set_error(*first);
                }
            }
            RecordOutcome::Pending => {
                self.pending.insert(pos.row);
                let next = model.rows().index_of(pos.row).map(|i| i + 1);
                if let Some(t) = next
                    .filter(|i| *i < model.rows().row_count())
                    .and_then(|i| self.edge_cell(model, i, false))
                {
                    self.move_to(t, model, fmt, &mut fx);
                } else {
                    self.edit = None;
                }
            }
        }
        fx
    }

    /// Enter (Shift+Enter when `back`) when Enter moves between fields
    /// (Q-264): Tab (Shift+Tab), taking an open list's highlighted entry
    /// like Tab does (Q-P10c). In the row's last editable cell Enter commits
    /// and records the row like [`Core::enter`], which then puts the cursor
    /// on the next line's first cell; on the untouched entry line it moves
    /// on like Tab instead, so Enter never saves the form in this mode
    /// (Ctrl+Enter does).
    fn enter_move(
        &mut self,
        back: bool,
        model: &mut ModelRef<'_>,
        lookups: &dyn Lookups,
        fmt: Formats,
    ) -> Effects {
        if back || !self.at_last_editable_cell(model) {
            return self.tab(!back, model, lookups, fmt);
        }
        let mut fx = Effects::handled();
        if self.cursor.is_none() {
            return fx;
        }
        if self
            .commit(model, lookups, fmt, Accept::Tab, &mut fx)
            .is_err()
        {
            return fx;
        }
        self.close_popup();
        let pristine = self
            .cursor_index(model)
            .is_some_and(|i| self.pristine_entry(model, i));
        let next = if pristine {
            self.tab(true, model, lookups, fmt)
        } else {
            self.enter(model, lookups, fmt)
        };
        fx.then(next)
    }

    fn toggle_checkbox(&mut self, model: &mut ModelRef<'_>, fx: &mut Effects) -> bool {
        let Some(pos) = self.cursor else { return false };
        if self.column_kind(pos.col) != Some(CellKind::Checkbox) {
            return false;
        }
        let Some(index) = self.cursor_index(model) else {
            return false;
        };
        if Self::locked(model, index) {
            fx.events
                .push(GridEvent::LockedRowEditAttempt { key: pos.row });
            return true;
        }
        if !self.cell_is_editable(model, index, pos.col) {
            return false;
        }
        let now = matches!(model.rows().cell(index, pos.col), CellValue::Bool(true));
        let _ = self.commit_value(model, CellInput::Bool(!now), Source::Typed, fx);
        true
    }

    fn key_edit(
        &mut self,
        k: GridKey,
        m: Mods,
        model: &mut ModelRef<'_>,
        lookups: &dyn Lookups,
        fmt: Formats,
    ) -> Effects {
        match (k, m) {
            (GridKey::Tab, Mods::NONE) => self.tab(true, model, lookups, fmt),
            (GridKey::Tab, Mods::SHIFT) => self.tab(false, model, lookups, fmt),
            (GridKey::Enter, Mods::NONE | Mods::SHIFT)
                if fmt.enter_mode == EnterMode::MoveBetweenFields =>
            {
                self.enter_move(m.shift, model, lookups, fmt)
            }
            // Shift+Enter is Enter here (Q-190); in a multiline memo the
            // editor keeps it as a line break before it reaches the grid.
            (GridKey::Enter, Mods::NONE | Mods::SHIFT) => self.enter(model, lookups, fmt),
            (GridKey::Up | GridKey::Down, Mods::NONE) => {
                if self.move_highlight(if k == GridKey::Up { -1 } else { 1 }) {
                    return Effects::handled();
                }
                self.vertical(if k == GridKey::Up { -1 } else { 1 }, model, lookups, fmt)
            }
            // Shift+Up/Down extends the selection (P16 D6); an open list
            // keeps the arrows.
            (GridKey::Up | GridKey::Down, Mods::SHIFT)
                if self.spec.select_column && !self.popup_open() =>
            {
                self.extend_vertical(if k == GridKey::Up { -1 } else { 1 }, model, lookups, fmt)
            }
            (GridKey::Home, Mods::CTRL) => self.jump_to_row(false, model, lookups, fmt),
            (GridKey::End, Mods::CTRL) => self.jump_to_row(true, model, lookups, fmt),
            (GridKey::PageUp, Mods::NONE) => self.page(false, model, lookups, fmt),
            (GridKey::PageDown, Mods::NONE) => self.page(true, model, lookups, fmt),
            (GridKey::Escape, Mods::NONE) => {
                let fx = self.revert(model, fmt);
                if fx.handled {
                    fx
                } else {
                    Effects {
                        events: vec![GridEvent::EscapeUnhandled],
                        ..Effects::default()
                    }
                }
            }
            (GridKey::Space, Mods::NONE) => {
                let mut fx = Effects::handled();
                if self.toggle_checkbox(model, &mut fx) {
                    return fx;
                }
                if self.edit.is_some() {
                    return self.text(" ", model, lookups, fmt);
                }
                if self.spec.selection != SelectionMode::None
                    && let Some(pos) = self.cursor
                {
                    self.selection.toggle(pos.row, self.spec.selection);
                    fx.events.push(GridEvent::SelectionChanged);
                    return fx;
                }
                Effects::default()
            }
            (GridKey::Backspace, Mods::NONE) => {
                let Some(pos) = self.cursor else {
                    return Effects::default();
                };
                let is_tags = matches!(self.column_kind(pos.col), Some(CellKind::Tags { .. }));
                let empty = self.edit.as_ref().is_some_and(|e| e.buffer.is_empty());
                if is_tags && empty {
                    let mut chips = self
                        .edit
                        .as_ref()
                        .map(|e| e.chips.clone())
                        .unwrap_or_default();
                    if tags::remove_last(&mut chips).is_some() {
                        let mut fx = Effects::handled();
                        if self
                            .commit_value(
                                model,
                                CellInput::Tags(chips.clone()),
                                Source::Typed,
                                &mut fx,
                            )
                            .is_ok()
                            && let Some(e) = self.edit.as_mut()
                        {
                            e.chips = chips;
                        }
                        return fx;
                    }
                }
                Effects::default()
            }
            _ => Effects::default(),
        }
    }

    fn key_browse(
        &mut self,
        k: GridKey,
        m: Mods,
        model: &mut ModelRef<'_>,
        fmt: Formats,
    ) -> Effects {
        let n = model.rows().row_count();
        let index = self.cursor_index(model);
        let mode = self.spec.selection;
        let mut fx = Effects::handled();
        let mut go = |core: &mut Core, target: usize, extend: bool, fx: &mut Effects| {
            if n == 0 {
                return;
            }
            let target = target.min(n - 1);
            let col = core.cursor.map_or_else(
                || core.spec.columns.first().map_or(ColumnId(0), |c| c.id),
                |c| c.col,
            );
            if let Some(t) = Self::row_at(model, target, col) {
                if extend && let Some(from) = core.cursor {
                    core.selection.anchor_if_none(from.row);
                }
                core.move_to(t, model, fmt, fx);
                if extend {
                    core.selection.extend_to(model.rows(), t.row, mode);
                    fx.events.push(GridEvent::SelectionChanged);
                }
            }
        };
        let i = index.unwrap_or(0);
        let page = self.page_rows;
        match (k, m) {
            (GridKey::Up, Mods::NONE | Mods::SHIFT) => {
                go(self, i.saturating_sub(1), m.shift, &mut fx);
            }
            (GridKey::Down, Mods::NONE | Mods::SHIFT) => {
                go(
                    self,
                    if index.is_some() { i + 1 } else { 0 },
                    m.shift,
                    &mut fx,
                );
            }
            (GridKey::PageUp, Mods::NONE) => go(self, i.saturating_sub(page), false, &mut fx),
            (GridKey::PageDown, Mods::NONE) => go(self, i.saturating_add(page), false, &mut fx),
            (GridKey::Home, Mods::CTRL | Mods::CTRL_SHIFT) => go(self, 0, m.shift, &mut fx),
            (GridKey::End, Mods::CTRL | Mods::CTRL_SHIFT) => {
                go(self, n.saturating_sub(1), m.shift, &mut fx);
            }
            (GridKey::Left | GridKey::Right, Mods::NONE) => {
                let cols = self.col_order();
                if let Some(pos) = self.cursor
                    && let Some(ci) = cols.iter().position(|c| *c == pos.col)
                {
                    let ni = if k == GridKey::Left {
                        ci.saturating_sub(1)
                    } else {
                        (ci + 1).min(cols.len() - 1)
                    };
                    self.cursor = Some(CellPos {
                        col: cols[ni],
                        ..pos
                    });
                }
            }
            (GridKey::Enter, Mods::NONE | Mods::SHIFT) => {
                if let Some(pos) = self.cursor {
                    fx.events.push(GridEvent::Activate { key: pos.row });
                }
            }
            (GridKey::Space, Mods::NONE) => {
                if mode != SelectionMode::None
                    && let Some(pos) = self.cursor
                {
                    self.selection.toggle(pos.row, mode);
                    fx.events.push(GridEvent::SelectionChanged);
                } else {
                    return Effects::default();
                }
            }
            (GridKey::Tab, Mods::NONE | Mods::SHIFT) => {
                self.focused = false;
                fx.leave = Some(!m.shift);
                fx.events.push(GridEvent::LeftGrid { forward: !m.shift });
            }
            (GridKey::Escape, Mods::NONE) => {
                if self.selection.clear() {
                    fx.events.push(GridEvent::SelectionChanged);
                } else {
                    return Effects {
                        events: vec![GridEvent::EscapeUnhandled],
                        ..Effects::default()
                    };
                }
            }
            _ => return Effects::default(),
        }
        fx
    }

    fn text(
        &mut self,
        t: &str,
        model: &mut ModelRef<'_>,
        lookups: &dyn Lookups,
        fmt: Formats,
    ) -> Effects {
        if t.is_empty() {
            return Effects::default();
        }
        if !self.spec.editable {
            return Effects {
                events: vec![GridEvent::TypedInBrowse(t.to_string())],
                ..Effects::handled()
            };
        }
        let Some(pos) = self.cursor else {
            return Effects::default();
        };
        let Some(index) = self.cursor_index(model) else {
            return Effects::default();
        };
        if Self::locked(model, index) || self.pending.contains(&pos.row) {
            return Effects {
                events: vec![GridEvent::LockedRowEditAttempt { key: pos.row }],
                ..Effects::handled()
            };
        }
        let kind = self.column_kind(pos.col);
        if kind == Some(CellKind::Checkbox) {
            let mut fx = Effects::handled();
            if t == " " {
                self.toggle_checkbox(model, &mut fx);
            }
            return fx;
        }
        if matches!(kind, Some(CellKind::Dropdown { .. })) {
            // First-letter jump in an open choice list.
            let first = t
                .chars()
                .next()
                .map(|c| c.to_lowercase().collect::<String>());
            if let (Some(first), Some(popup)) =
                (first, self.edit.as_mut().and_then(|e| e.popup.as_mut()))
                && let Some(i) = popup
                    .items
                    .iter()
                    .position(|s| s.label.to_lowercase().starts_with(&first))
            {
                popup.highlight = Some(i);
            }
            return Effects::handled();
        }
        let Some(edit) = self.edit.as_mut() else {
            return Effects::default();
        };
        // A single date-key character changes the date (Q-P10f).
        if kind == Some(CellKind::Date) {
            let mut chars = t.chars();
            if let (Some(c), None) = (chars.next(), chars.next())
                && let Some(key) = DateKey::from_char(c)
            {
                let current = date_entry::parse(&edit.origin, fmt.date_order, fmt.today).ok();
                let base_text = if edit.fresh {
                    edit.origin.clone()
                } else {
                    edit.buffer.clone()
                };
                if let Some(d) = date_entry::apply_key(
                    key,
                    &base_text,
                    current,
                    fmt.date_order,
                    fmt.today,
                    fmt.first_weekday,
                ) {
                    edit.buffer = date_entry::format(d, fmt.date_order);
                    edit.fresh = false;
                    edit.error = None;
                }
                return Effects::handled();
            }
        }
        if edit.fresh {
            edit.buffer = t.to_string();
            edit.fresh = false;
        } else {
            edit.buffer.push_str(t);
        }
        edit.error = None;
        edit.caption = None;
        self.refresh_popup(lookups);
        Effects::handled()
    }

    fn click(
        &mut self,
        pos: CellPos,
        double: bool,
        mods: Mods,
        model: &mut ModelRef<'_>,
        lookups: &dyn Lookups,
        fmt: Formats,
    ) -> Effects {
        let mut fx = Effects::handled();
        self.focused = true;
        if !self.spec.editable {
            if mods.shift
                && !mods.ctrl
                && let Some(from) = self.cursor
            {
                self.selection.anchor_if_none(from.row);
            }
            self.move_to(pos, model, fmt, &mut fx);
            let sel_mode = self.spec.selection;
            if mods.ctrl {
                self.selection.toggle(pos.row, sel_mode);
                fx.events.push(GridEvent::SelectionChanged);
            } else if mods.shift {
                self.selection.extend_to(model.rows(), pos.row, sel_mode);
                fx.events.push(GridEvent::SelectionChanged);
            }
            if double {
                fx.events.push(GridEvent::Activate { key: pos.row });
            }
            return fx;
        }
        if self.cursor == Some(pos) {
            if self.edit.is_none() && !self.parked {
                self.open_editor(model, fmt, &mut fx);
            }
            if let Some(index) = self.cursor_index(model)
                && Self::locked(model, index)
            {
                fx.events
                    .push(GridEvent::LockedRowEditAttempt { key: pos.row });
            } else {
                self.open_on_click(model, lookups, &mut fx);
            }
            return fx;
        }
        if self
            .commit(model, lookups, fmt, Accept::Other, &mut fx)
            .is_err()
        {
            return fx; // stay with the error (S11)
        }
        self.move_to(pos, model, fmt, &mut fx);
        if let Some(index) = model.rows().index_of(pos.row) {
            if Self::locked(model, index) {
                fx.events
                    .push(GridEvent::LockedRowEditAttempt { key: pos.row });
            } else if self.column_kind(pos.col) == Some(CellKind::Checkbox) {
                self.toggle_checkbox(model, &mut fx);
            } else {
                self.open_on_click(model, lookups, &mut fx);
            }
        }
        fx
    }

    /// The model changed outside the grid (undo, redo, recompute, reload).
    /// `after_undo` refreshes the cursor row's Esc snapshot.
    pub fn resync(&mut self, model: &mut ModelRef<'_>, fmt: Formats, after_undo: bool) -> Effects {
        let mut fx = Effects::default();
        if self.selection.retain_existing(model.rows()) {
            fx.events.push(GridEvent::SelectionChanged);
        }
        // A menu about a row that is gone closes.
        if let Some(ContextTarget::Cell(p)) = self.menu.as_ref().map(|m| m.target)
            && model.rows().index_of(p.row).is_none()
        {
            self.menu = None;
        }
        let rows = model.rows();
        self.pending.retain(|k| rows.index_of(*k).is_some());
        let Some(pos) = self.cursor else { return fx };
        let Some(index) = model.rows().index_of(pos.row) else {
            fx.events.push(GridEvent::RowGone { key: pos.row });
            self.recover_cursor(model, fmt);
            return fx;
        };
        self.last_index = index;
        if after_undo && let Some(m) = model.edit() {
            m.row_entered(pos.row);
            self.entered = Some(pos.row);
        }
        let rev = model.rows().revision();
        let value = model.rows().cell(index, pos.col);
        let text = cell_text(&value, fmt);
        let chips = match value {
            CellValue::Tags(t) => Some(t.to_vec()),
            _ => None,
        };
        let editable = self.cell_is_editable(model, index, pos.col);
        match self.edit.as_mut() {
            Some(_) if !editable => self.edit = None,
            Some(e) if e.origin_rev != rev => {
                if let Some(c) = chips {
                    e.chips = c;
                }
                if e.is_dirty() {
                    if text != e.origin {
                        e.origin = text;
                        e.caption = Some(CHANGED_UNDERNEATH);
                    }
                } else if text != e.origin {
                    e.buffer.clone_from(&text);
                    e.origin = text;
                    e.fresh = true;
                    fx.reset_editor = true;
                }
                e.origin_rev = rev;
            }
            None if editable && self.focused => self.open_editor(model, fmt, &mut fx),
            Some(_) | None => {}
        }
        fx
    }

    /// A different record was loaded (Save & New, Prev/Next, Clear): drop
    /// the editor, a parked edit, the selection, pending rows and any
    /// notice, and put the cursor on row 0's first editable cell. Nothing
    /// is reported: the old rows did not vanish, the record changed.
    pub fn reset(&mut self, model: &mut ModelRef<'_>, fmt: Formats) -> Effects {
        self.edit = None;
        self.parked = false;
        self.menu = None;
        self.selection.clear();
        self.pending.clear();
        self.notice = None;
        self.entered = None;
        self.cursor = None;
        self.last_index = 0;
        let mut fx = Effects::default();
        if let Some(pos) = self.edge_cell(model, 0, false) {
            self.move_to(pos, model, fmt, &mut fx);
        }
        fx.events.clear();
        fx.reset_editor = true;
        fx
    }

    /// After a save stored new lines with the same keys (B5, T20): keep the
    /// cursor and selection and say nothing about vanished rows.
    pub fn rebind(&mut self, model: &mut ModelRef<'_>, fmt: Formats) -> Effects {
        let fx = self.resync(model, fmt, false);
        if self.notice == Some(ROW_GONE) && self.cursor.is_some() {
            self.notice = None;
        }
        fx
    }

    /// An asynchronous record finished (P12 entry row).
    pub fn record_finished(
        &mut self,
        key: RowKey,
        result: Result<Option<RowKey>, Vec<CellProblem>>,
    ) -> Effects {
        self.pending.remove(&key);
        let mut fx = Effects::handled();
        match result {
            Ok(_) => fx.events.push(GridEvent::RowRecorded { key }),
            Err(problems) => {
                fx.events.push(GridEvent::RecordRefused { key });
                if self.cursor.is_some_and(|c| c.row == key)
                    && let Some(p) = problems.first()
                {
                    self.set_error(*p);
                }
            }
        }
        fx
    }

    /// Quick-add finished: the picker cell becomes the new entry.
    pub fn resolve_pending(
        &mut self,
        pos: CellPos,
        id: u64,
        model: &mut ModelRef<'_>,
        fmt: Formats,
    ) -> Effects {
        let mut fx = Effects::handled();
        let Some(m) = model.edit() else { return fx };
        if m.set_cell(pos, CellInput::Ref(id), Source::Picked).is_ok() {
            fx.events.push(GridEvent::CellCommitted {
                pos,
                source: Source::Picked,
            });
            if self.cursor == Some(pos) {
                if let Some(e) = self.edit.as_mut() {
                    e.error = None;
                }
                self.refresh_origin(model, fmt);
            }
        }
        fx
    }

    /// A shell modal opened or the grid's input was disabled: close popups,
    /// commit a valid buffer, park an invalid one (B6).
    pub fn suspend(
        &mut self,
        model: &mut ModelRef<'_>,
        lookups: &dyn Lookups,
        fmt: Formats,
    ) -> Effects {
        self.focus_lost(model, lookups, fmt)
    }

    /// The selected rows.
    pub fn selected_keys(&self) -> impl Iterator<Item = RowKey> + '_ {
        self.selection.iter()
    }
}
