//! The [`Grid`] widget: the egui adapter over [`Core`] (P10 plan Rev 2
//! §2.3-2.4, §2.8-2.11).
//!
//! One egui focus id per grid (B1): while editing it is the cursor cell's
//! `TextEdit`, otherwise an invisible focus target. Cells are non-focusable
//! click targets with explicit AccessKit nodes, so egui's focus traversal
//! never walks the cells and the focus-lock filter survives every cursor
//! move. Input is collected and consumed before anything draws, in order
//! (the sequencer splits batched frames first, B2).

use cl_core::{CalendarDate, MoneyStyle, Weekday};
use cl_model::TextKey;
use cl_model::amount_entry::{self, Preview};
use cl_model::date_entry::DateOrder;
use cl_model::forms::EnterMode;
use egui::{
    Align2, Color32, CornerRadius, EventFilter, FontId, Id, Pos2, Rect, Response, Role, Sense,
    Stroke, StrokeKind, Ui, UiBuilder, Vec2, pos2, vec2,
};

use crate::core::codec::cell_text;
use crate::core::context::{ContextMenuSource, ContextTarget};
use crate::core::keys::{Formats, GridInput, GridKey, Mods};
use crate::core::model::{
    CellKind, CellPos, CellProblem, CellValue, ColumnId, Line, Lookups, MarkKind, ModelRef,
    RowHeight, RowState,
};
use crate::core::rowkey::RowKey;
use crate::core::spec::{GridCommand, GridEvent, GridSpec, Handled, SpecError};
use crate::core::state::{Core, Effects};
use crate::core::viewport::{Heights, RowAlign, Viewport};
use crate::sequencer;
use crate::view::a11y::{self, CellInfo, Request, RowInfo, ak_rect};
use crate::view::input::{self, CursorInfo};
use crate::view::layout::ColumnLayout;
use crate::view::overlay::{PopupHit, PopupView};
use crate::view::style::GridStyle;

/// The focus-lock filter the grid's focus widget uses: Tab, both arrow
/// axes and Esc belong to the grid (F2).
pub const FULL_FILTER: EventFilter = EventFilter {
    tab: true,
    horizontal_arrows: true,
    vertical_arrows: true,
    escape: true,
};

/// How tall the grid is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GridHeight {
    /// Fill the space below, keeping `reserve_below` points free, and show
    /// at least `min_rows` rows.
    Fill {
        /// Points kept free under the grid (footer, buttons).
        reserve_below: f32,
        /// Fewest rows shown (3 by default).
        min_rows: u16,
    },
    /// Fit the rows, between `min` and `max` rows tall.
    Rows {
        /// Fewest rows.
        min: u16,
        /// Most rows before the grid scrolls.
        max: u16,
    },
}

/// What the screen provides each frame.
pub struct GridEnv<'a> {
    /// Colours and fonts (`cl_ui::theme::grid_style`).
    pub style: &'a GridStyle,
    /// The i18n resolver.
    pub text: &'a dyn Fn(TextKey) -> String,
    /// Lists for pickers and tags.
    pub lookups: &'a dyn Lookups,
    /// Today (for `T` and `M/D`).
    pub today: CalendarDate,
    /// First day of the week (for `W` and `K`).
    pub first_weekday: Weekday,
    /// Date display order.
    pub date_order: DateOrder,
    /// Negative-amount style.
    pub money_style: MoneyStyle,
    /// Height rule.
    pub height: GridHeight,
    /// The grid's accessible label ("Expense lines").
    pub label: &'a str,
    /// False while a shell modal is open: the grid gives up focus and
    /// ignores input (B6).
    pub input_enabled: bool,
    /// What Enter does: Save & New style (record the row) or move to the
    /// next cell (the "Enter moves between fields" preference, Q-264).
    pub enter_mode: EnterMode,
    /// The screen's context menu entries ([`GridSpec::context_menu`]);
    /// `None` opens no menu.
    pub context: Option<&'a dyn ContextMenuSource>,
}

impl GridEnv<'_> {
    fn formats(&self) -> Formats {
        Formats {
            today: self.today,
            first_weekday: self.first_weekday,
            date_order: self.date_order,
            money: self.money_style,
            enter_mode: self.enter_mode,
        }
    }

    fn t(&self, key: TextKey) -> String {
        (self.text)(key)
    }
}

/// What happened this frame.
#[derive(Debug, Default)]
pub struct GridOutput {
    /// Events for the screen.
    pub events: Vec<GridEvent>,
    /// The grid has keyboard focus at the end of the frame.
    pub focused: bool,
}

/// Positions and counts for the control channel. Never values (F14).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GridSnapshot {
    /// The cursor's row index.
    pub cursor_row: Option<usize>,
    /// The cursor's column id.
    pub cursor_col: Option<u16>,
    /// Rows in the model.
    pub rows: usize,
    /// The first visible row.
    pub top_row: usize,
    /// A popup is open.
    pub popup_open: bool,
    /// Selected rows.
    pub selected: usize,
    /// The grid has focus.
    pub focused: bool,
    /// The open cell has text not committed yet.
    pub uncommitted: bool,
}

/// The grid. The screen owns it; its state survives frames.
// Frame-to-frame focus bookkeeping flags (B1, B6).
#[allow(clippy::struct_excessive_bools)]
pub struct Grid {
    id: Id,
    core: Core,
    viewport: Viewport,
    widths: Vec<(ColumnId, u16)>,
    layout: ColumnLayout,
    had_focus: bool,
    restore_focus: bool,
    want_focus: bool,
    /// Passes left to retry an explicit [`Grid::focus`]: a grid shown for
    /// the first time in the pass that asked has no focus widget yet, and
    /// egui drops focus on a widget it did not see.
    focus_retries: u8,
    seen_rev: Option<u64>,
    ime_active: bool,
    cells: Vec<(Id, CellPos)>,
    /// Row nodes drawn last frame (AccessKit `ScrollIntoView` targets).
    row_nodes: Vec<(Id, RowKey)>,
    rows_drawn: usize,
    row_unit: Option<u64>,
    note_unit: u64,
    pending_reset: bool,
    /// Horizontal scroll in points when the columns are wider than the grid.
    hscroll: f32,
    /// Amount and date columns grown to fit their widest value this
    /// session; they never shrink (S14).
    grown: Vec<(ColumnId, u16)>,
    /// Measured heights, cached by model revision and row count (N3).
    measured: std::cell::RefCell<Option<MeasuredCache>>,
    /// The open popup's rect last frame (presses there keep focus).
    popup_rect: Option<Rect>,
    /// The popup's first shown entry.
    popup_top: usize,
    /// A popup row's height last frame (the wheel scrolls whole rows).
    popup_row_h: f32,
    /// Wheel points not yet a whole popup row.
    popup_wheel: f32,
    /// The highlight the popup showed last frame: the list follows the
    /// highlight only when it moves, so the wheel can scroll away from it.
    popup_seen: Option<Option<usize>>,
    /// Select checkboxes drawn last frame (AccessKit Click targets).
    gutters: Vec<(Id, RowKey)>,
    /// Where the open context menu sits: the pointer of the right-click,
    /// or `None` for under the cursor cell (Shift+F10, the Menu key).
    menu_at: Option<Pos2>,
    /// The open context menu's rect last frame (presses there keep focus;
    /// a press anywhere else closes it).
    menu_rect: Option<Rect>,
    /// The cursor cell's rect last painted (the keyboard menu's anchor).
    cursor_rect: Option<Rect>,
    /// The cursor's column last frame: when it changes, the columns
    /// scroll sideways to show it.
    follow_col: Option<ColumnId>,
    /// Where the cells (after the gutter, before the scroll bar) and the
    /// header were drawn last frame, in screen points.
    drawn: Option<(Rect, Rect)>,
}

fn group(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

// Geometry only, never money: screen points <-> whole 1/64 pt units.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn units(points: f32) -> u64 {
    let u = (points.max(0.0) * 64.0).round();
    if u >= 1.0e18 { u64::MAX } else { u as u64 }
}

// Geometry only: viewport-local offsets are small (a few thousand points).
#[allow(clippy::cast_precision_loss)]
fn points(units: i64) -> f32 {
    units as f32 / 64.0
}

// Geometry only: a wheel delta in points to signed units.
#[allow(clippy::cast_possible_truncation)]
fn wheel_units(dy: f32) -> i64 {
    -((dy * 64.0).round() as i64)
}

/// The popup's first entry after the wheel moved `wheel` points (positive
/// is up, toward the top) with rows `row_h` tall: the new first entry and
/// the points left over (less than one row).
// Geometry only: a few rows per wheel step.
#[allow(clippy::cast_possible_truncation)]
fn popup_wheel(top: usize, wheel: f32, row_h: f32) -> (usize, f32) {
    let row_h = row_h.max(1.0);
    let rows = (wheel / row_h).trunc();
    let left = wheel - rows * row_h;
    (top.saturating_add_signed(-(rows as isize)), left)
}

impl Grid {
    /// A grid with this spec.
    ///
    /// `id_salt` becomes the grid's egui id (its single focus id and its
    /// memory), so it must be unique among the grids alive at the same
    /// time in every window and viewport: screens that can be open more
    /// than once include their window id (`"{window}/lines"`). Two grids
    /// with one salt would share focus and editor state.
    ///
    /// # Errors
    /// See [`SpecError`].
    pub fn new(id_salt: &str, spec: GridSpec) -> Result<Grid, SpecError> {
        spec.validate()?;
        Ok(Grid {
            id: Id::unique(("cl_grid", id_salt)),
            core: Core::new(spec),
            viewport: Viewport::new(0),
            widths: Vec::new(),
            layout: ColumnLayout::default(),
            had_focus: false,
            restore_focus: false,
            want_focus: false,
            focus_retries: 0,
            seen_rev: None,
            ime_active: false,
            cells: Vec::new(),
            row_nodes: Vec::new(),
            rows_drawn: 0,
            row_unit: None,
            note_unit: units(18.0),
            pending_reset: false,
            hscroll: 0.0,
            grown: Vec::new(),
            measured: std::cell::RefCell::new(None),
            popup_rect: None,
            popup_top: 0,
            popup_row_h: 18.0,
            popup_wheel: 0.0,
            popup_seen: None,
            gutters: Vec::new(),
            menu_at: None,
            menu_rect: None,
            cursor_rect: None,
            follow_col: None,
            drawn: None,
        })
    }

    /// Where `id` was drawn last frame: its left edge and width in screen
    /// points, cut to the part in view (`None` when scrolled out of view
    /// or not drawn yet). Screens align a totals row under the columns
    /// with it.
    #[must_use]
    pub fn column_span(&self, id: ColumnId) -> Option<(f32, f32)> {
        let (cells, _) = self.drawn?;
        let p = self.layout.place(id)?;
        let left = cells.left() + p.x - self.hscroll;
        let l = left.max(cells.left());
        let r = (left + p.width).min(cells.right());
        (r > l).then_some((l, r - l))
    }

    /// Whether all of column `id` was in view last frame.
    #[must_use]
    pub fn column_shown(&self, id: ColumnId) -> bool {
        let (Some((cells, _)), Some(p)) = (self.drawn, self.layout.place(id)) else {
            return false;
        };
        let left = cells.left() + p.x - self.hscroll;
        left >= cells.left() - 0.5 && left + p.width <= cells.right() + 0.5
    }

    /// The header cell of column `id` last frame, cut to the part in view.
    #[must_use]
    pub fn header_cell(&self, id: ColumnId) -> Option<Rect> {
        let (_, header) = self.drawn?;
        let (x, w) = self.column_span(id)?;
        let p = self.layout.place(id)?;
        let top = if p.line == Line::Second {
            header.top() + header.height() / 2.0
        } else {
            header.top()
        };
        let h = if self.layout.two_lines {
            header.height() / 2.0
        } else {
            header.height()
        };
        Some(Rect::from_min_size(pos2(x, top), vec2(w, h)))
    }

    /// The open context menu's id (its `Menu` node; entries are
    /// `view::context_menu::item_id(menu_id, index)`).
    #[must_use]
    pub fn menu_id(&self) -> Id {
        self.id.with("menu")
    }

    /// The id of a row's select checkbox ([`GridSpec::select_column`]).
    #[must_use]
    pub fn gutter_id(&self, key: RowKey) -> Id {
        self.id.with(("select", key.0))
    }

    /// Width of the select-checkbox gutter (0 without a select column).
    fn gutter(&self) -> f32 {
        if self.spec().select_column {
            GUTTER
        } else {
            0.0
        }
    }

    /// The grid's egui id (its AccessKit grid node).
    #[must_use]
    pub fn id(&self) -> Id {
        self.id
    }

    /// The single egui focus id (B1).
    #[must_use]
    pub fn focus_id(&self) -> Id {
        self.id.with("focus")
    }

    /// The id of a cell's click target and AccessKit node.
    #[must_use]
    pub fn cell_id(&self, pos: CellPos) -> Id {
        self.id.with(("cell", pos.row.0, pos.col.0))
    }

    /// The id of a row's AccessKit node.
    #[must_use]
    pub fn row_id(&self, key: RowKey) -> Id {
        self.id.with(("row", key.0))
    }

    /// The spec.
    #[must_use]
    pub fn spec(&self) -> &GridSpec {
        self.core.spec()
    }

    /// The state machine (tests and screens read the editor and selection).
    #[must_use]
    pub fn core(&self) -> &Core {
        &self.core
    }

    /// Logical focus.
    #[must_use]
    pub fn has_focus(&self) -> bool {
        self.core.has_focus()
    }

    /// The open cell has text that is not in the model yet (feeds
    /// `Screen::dirty`).
    #[must_use]
    pub fn has_uncommitted_edit(&self) -> bool {
        self.core.has_uncommitted_edit()
    }

    /// The selected rows.
    pub fn selected_keys(&self) -> impl Iterator<Item = RowKey> + '_ {
        self.core.selected_keys()
    }

    /// Owner-resized column widths (persisted per screen, Q-P10k).
    #[must_use]
    pub fn column_widths(&self) -> Vec<(ColumnId, u16)> {
        self.widths.clone()
    }

    /// Restore column widths.
    pub fn set_column_widths(&mut self, widths: &[(ColumnId, u16)]) {
        self.widths = widths.to_vec();
    }

    /// The last layout (P12's entry row lines its fields up with it).
    #[must_use]
    pub fn column_layout(&self) -> &ColumnLayout {
        &self.layout
    }

    /// Move the cursor and take keyboard focus next frame.
    pub fn focus(&mut self, pos: CellPos, mut model: ModelRef<'_>, env: &GridEnv<'_>) {
        self.core.set_cursor(pos, &mut model, env.formats());
        self.want_focus = true;
        self.focus_retries = 3;
    }

    /// Move the cursor without taking keyboard focus (a list shown beside
    /// a form that keeps the focus).
    pub fn set_cursor(&mut self, pos: CellPos, mut model: ModelRef<'_>, env: &GridEnv<'_>) {
        self.core.set_cursor(pos, &mut model, env.formats());
    }

    /// Bring a row into view on the next frame.
    pub fn scroll_to(&mut self, key: RowKey, align: RowAlign, model: &ModelRef<'_>) {
        if let Some(i) = model.rows().index_of(key) {
            let h = self.heights(model);
            self.viewport.scroll_to(i, align, &h, model.rows());
        }
    }

    /// A forwarded command (from `Screen::command`). `NotHandled` lets the
    /// shell run its default.
    pub fn command(
        &mut self,
        cmd: GridCommand,
        mut model: ModelRef<'_>,
        env: &GridEnv<'_>,
    ) -> (Handled, Vec<GridEvent>) {
        let fx = self.core.handle(
            GridInput::Command(cmd),
            &mut model,
            env.lookups,
            env.formats(),
        );
        let handled = if fx.handled {
            Handled::Handled
        } else {
            Handled::NotHandled
        };
        let events = self.apply(fx, &model, None);
        (handled, events)
    }

    /// Commit the open cell before a save, leave or record move (B4).
    ///
    /// # Errors
    /// The open cell is invalid: the cursor stays on it with the problem
    /// shown, and the save must not start.
    pub fn commit_pending(
        &mut self,
        mut model: ModelRef<'_>,
        env: &GridEnv<'_>,
    ) -> Result<(), CellProblem> {
        let (r, fx) = self
            .core
            .commit_pending(&mut model, env.lookups, env.formats());
        if r.is_err() {
            self.want_focus = true;
        }
        self.apply(fx, &model, None);
        r
    }

    /// After undo, redo or a reload: re-read the model under the editor.
    pub fn resync(
        &mut self,
        mut model: ModelRef<'_>,
        env: &GridEnv<'_>,
        after_undo: bool,
    ) -> Vec<GridEvent> {
        let fx = self.core.resync(&mut model, env.formats(), after_undo);
        self.seen_rev = Some(model.rows().revision());
        self.apply(fx, &model, None)
    }

    /// A different record was loaded (Save & New, Prev/Next, Clear): clear
    /// the editor, selection, pending rows and notice, and put the cursor on
    /// row 0's first editable cell. Emits no events.
    pub fn reset(&mut self, mut model: ModelRef<'_>, env: &GridEnv<'_>) {
        let fx = self.core.reset(&mut model, env.formats());
        self.seen_rev = Some(model.rows().revision());
        let _ = self.apply(fx, &model, None);
    }

    /// After a save stored new lines under the same keys (T20).
    pub fn rebind(&mut self, mut model: ModelRef<'_>, env: &GridEnv<'_>) {
        let fx = self.core.rebind(&mut model, env.formats());
        self.seen_rev = Some(model.rows().revision());
        self.apply(fx, &model, None);
    }

    /// An asynchronous record finished.
    pub fn record_finished(
        &mut self,
        key: RowKey,
        result: Result<Option<RowKey>, Vec<CellProblem>>,
    ) -> Vec<GridEvent> {
        self.core.record_finished(key, result).events
    }

    /// Quick-add finished: put the new entry into the cell.
    pub fn resolve_pending(
        &mut self,
        pos: CellPos,
        id: u64,
        mut model: ModelRef<'_>,
        env: &GridEnv<'_>,
    ) {
        let fx = self
            .core
            .resolve_pending(pos, id, &mut model, env.formats());
        self.apply(fx, &model, None);
    }

    /// Positions and counts, no values.
    #[must_use]
    pub fn snapshot(&self, model: &ModelRef<'_>) -> GridSnapshot {
        let rows = model.rows();
        GridSnapshot {
            cursor_row: self.core.cursor().and_then(|c| rows.index_of(c.row)),
            cursor_col: self.core.cursor().map(|c| c.col.0),
            rows: rows.row_count(),
            top_row: self.viewport.top(),
            popup_open: self.core.edit().is_some_and(|e| e.popup.is_some()),
            selected: self.core.selection().len(),
            focused: self.core.has_focus(),
            uncommitted: self.core.has_uncommitted_edit(),
        }
    }

    fn two_lines(&self) -> bool {
        match self.spec().height {
            RowHeight::Uniform { two_lines } => two_lines || self.layout.two_lines,
            RowHeight::Measured { .. } => self.layout.two_lines,
        }
    }

    fn row_points(&self, style: &GridStyle) -> f32 {
        if self.two_lines() {
            style.row_two
        } else {
            style.row_one
        }
    }

    fn heights(&self, model: &ModelRef<'_>) -> Heights {
        let row = self
            .row_unit
            .unwrap_or_else(|| units(if self.two_lines() { 44.0 } else { 30.0 }));
        let rows = model.rows();
        let n = rows.row_count();
        match self.spec().height {
            RowHeight::Measured { max_rows } if n <= max_rows as usize => {
                let rev = rows.revision();
                // The open multiline memo grows its row while typing.
                let editing = self.editing_memo_lines(rows);
                let mut cache = self.measured.borrow_mut();
                if let Some((r, c, u, e, h)) = cache.as_ref()
                    && *r == rev
                    && *c == n
                    && *u == row
                    && *e == editing
                {
                    return h.clone();
                }
                // A row with a note (caption, suggestion, error) gets one
                // more line, and a multiline memo one line per extra text
                // line; everything else is the base height.
                let heights: Vec<u64> = (0..n)
                    .map(|i| {
                        let note = if rows.row_note(i).is_some() {
                            self.note_unit
                        } else {
                            0
                        };
                        let memo = match editing {
                            Some((ei, lines)) if ei == i => lines,
                            _ => self.memo_extra_lines(rows, i),
                        };
                        row + note + memo * self.note_unit
                    })
                    .collect();
                let h = Heights::measured(&heights);
                *cache = Some((rev, n, row, editing, h.clone()));
                h
            }
            // Above the cap (Q-P10m) or uniform: one height for every row.
            _ => Heights::uniform(row, n),
        }
    }

    /// Column widths for layout: the owner's, then grown amount and date
    /// widths where those are wider.
    fn effective_widths(&self) -> Vec<(ColumnId, u16)> {
        let mut out = self.widths.clone();
        for (id, w) in &self.grown {
            match out.iter_mut().find(|(i, _)| i == id) {
                Some(entry) => entry.1 = entry.1.max(*w),
                None => out.push((*id, *w)),
            }
        }
        out
    }

    /// Apply view effects; returns the events.
    fn apply(
        &mut self,
        fx: Effects,
        model: &ModelRef<'_>,
        ctx: Option<&egui::Context>,
    ) -> Vec<GridEvent> {
        if let Some(key) = fx.scroll_to
            && let Some(i) = model.rows().index_of(key)
        {
            let h = self.heights(model);
            self.viewport.ensure_visible(i, &h, model.rows());
        }
        if let Some(ctx) = ctx {
            if let Some(text) = fx.copy {
                ctx.copy_text(text);
            }
            if fx.reset_editor {
                self.reset_editor(ctx);
            }
            if let Some(forward) = fx.leave {
                ctx.memory_mut(|m| {
                    m.set_focus_lock_filter(self.focus_id(), EventFilter::default());
                    m.move_focus(if forward {
                        egui::FocusDirection::Next
                    } else {
                        egui::FocusDirection::Previous
                    });
                });
            }
        } else if fx.reset_editor {
            self.pending_reset = true;
        }
        fx.events
    }

    fn reset_editor(&mut self, ctx: &egui::Context) {
        self.pending_reset = false;
        let id = self.focus_id();
        let text = self
            .core
            .edit()
            .map(|e| e.buffer.clone())
            .unwrap_or_default();
        let mut state = egui::text_edit::TextEditState::load(ctx, id).unwrap_or_default();
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::select_all_str(&text)));
        state.clear_undoer();
        state.store(ctx, id);
        ctx.memory_mut(egui::Memory::interrupt_ime);
    }
}

/// Width of the grid's own scrollbar track.
const BAR: f32 = 12.0;
/// Room kept for the lock glyph at the right end of a locked row.
const LOCK_GUTTER: f32 = 22.0;
/// Width of the select-checkbox gutter at the left (P16 D6).
const GUTTER: f32 = 28.0;
/// The select checkbox's size.
const GUTTER_BOX: f32 = 14.0;

/// Measured heights keyed by (revision, rows, row unit, open memo).
type MeasuredCache = (u64, usize, u64, Option<(usize, u64)>, Heights);

impl Grid {
    /// The multiline memo columns.
    fn memo_columns(&self) -> impl Iterator<Item = ColumnId> + '_ {
        self.spec().columns.iter().filter_map(|c| {
            matches!(
                c.kind,
                CellKind::Memo {
                    multiline: true,
                    ..
                }
            )
            .then_some(c.id)
        })
    }

    /// Extra text lines the row's multiline memos need (the tallest).
    fn memo_extra_lines(&self, rows: &dyn crate::core::model::RowModel, index: usize) -> u64 {
        self.memo_columns()
            .map(|col| match rows.cell(index, col) {
                CellValue::Text(t) => crate::editors::memo::extra_lines(&t),
                _ => 0,
            })
            .max()
            .unwrap_or(0)
    }

    /// The open multiline memo's row index
    /// and its extra lines, so the row grows while the owner types.
    fn editing_memo_lines(&self, rows: &dyn crate::core::model::RowModel) -> Option<(usize, u64)> {
        let pos = self.core.cursor()?;
        let edit = self.core.edit()?;
        if !self.memo_columns().any(|c| c == pos.col) {
            return None;
        }
        Some((
            rows.index_of(pos.row)?,
            crate::editors::memo::extra_lines(&edit.buffer),
        ))
    }

    /// The open popup's id (its `ListBox` node; entries are
    /// `view::overlay::item_id(popup_id, index)`).
    #[must_use]
    pub fn popup_id(&self) -> Id {
        self.id.with("popup")
    }

    /// The open popup's id and its highlighted entry's id.
    fn popup_owner(&self) -> Option<(Id, Option<Id>)> {
        let popup = self.core.edit()?.popup.as_ref()?;
        let list = self.popup_id();
        Some((
            list,
            popup
                .highlight
                .map(|i| crate::view::overlay::item_id(list, i)),
        ))
    }

    /// AccessKit `SetValue` on a cell: move there, put the text in and commit
    /// it as one edit. Commit follows the Tab rule but never accepts a
    /// suggestion; checkboxes, dropdowns and tags ignore it.
    fn set_value(
        &mut self,
        pos: CellPos,
        value: String,
        model: &mut ModelRef<'_>,
        env: &GridEnv<'_>,
        ctx: &egui::Context,
    ) -> Vec<GridEvent> {
        let kind = self.spec().column(pos.col).map(|c| c.kind);
        if !matches!(
            kind,
            Some(
                CellKind::Text { .. }
                    | CellKind::Memo { .. }
                    | CellKind::Amount(_)
                    | CellKind::Date
                    | CellKind::Picker { .. }
            )
        ) {
            return Vec::new();
        }
        let click = GridInput::Click {
            pos,
            double: false,
            mods: crate::core::keys::Mods::NONE,
        };
        let (mut events, _) = self.handle(click, model, env, ctx);
        if self.core.cursor() != Some(pos) || self.core.edit().is_none() {
            return events;
        }
        for gi in [
            GridInput::BufferEdited(value),
            GridInput::Command(GridCommand::Commit),
        ] {
            let (ev, _) = self.handle(gi, model, env, ctx);
            events.extend(ev);
        }
        events
    }

    /// What the open popup shows this frame.
    fn popup_view(&self, pos: CellPos, width: f32, env: &GridEnv<'_>) -> Option<PopupView<'_>> {
        let popup = self.core.edit()?.popup.as_ref()?;
        let col = self.spec().column(pos.col)?;
        Some(crate::editors::picker::view(
            popup,
            env.t(col.header),
            width,
            self.popup_top,
            self.popup_seen != Some(popup.highlight),
            env.text,
        ))
    }

    /// Dropdown inputs the core cannot resolve alone: Space, Alt+Down, a
    /// letter or a click on the open cell open the column's choices, whose
    /// labels are i18n keys resolved here.
    fn translate(&self, gi: GridInput, env: &GridEnv<'_>) -> Vec<GridInput> {
        use crate::core::keys::{GridKey, Mods};
        let kind = self
            .core
            .cursor()
            .and_then(|c| self.spec().column(c.col))
            .map(|c| c.kind);
        let Some(CellKind::Dropdown { options }) = kind else {
            return vec![gi];
        };
        let Some(edit) = self.core.edit() else {
            return vec![gi];
        };
        let open = edit.popup.is_some();
        let choices = || GridInput::OpenChoices(crate::editors::dropdown::items(options, env.text));
        let alt = Mods {
            alt: true,
            ..Mods::NONE
        };
        match gi {
            // Space picks the highlighted choice and stays, in both Enter
            // modes (Enter would move on when Enter moves between fields).
            GridInput::Key(GridKey::Space, Mods::NONE)
                if open && env.enter_mode == EnterMode::MoveBetweenFields =>
            {
                match edit.popup.as_ref().and_then(|p| p.highlight) {
                    // The commit closes the list.
                    Some(i) => vec![GridInput::PopupPicked(i)],
                    None => vec![GridInput::ToggleList],
                }
            }
            GridInput::Key(GridKey::Space, Mods::NONE) if open => {
                vec![GridInput::Key(GridKey::Enter, Mods::NONE)]
            }
            GridInput::Key(GridKey::Down, m) if m == alt && open => {
                vec![GridInput::Key(GridKey::Down, Mods::NONE)]
            }
            GridInput::Key(GridKey::Space, Mods::NONE) => vec![choices()],
            GridInput::Key(GridKey::Down, m) if m == alt => vec![choices()],
            GridInput::Text(t) if !open && !t.trim().is_empty() => {
                vec![choices(), GridInput::Text(t)]
            }
            GridInput::Click { pos, .. } if !open && self.core.cursor() == Some(pos) => {
                vec![gi, choices()]
            }
            // A click on the open dropdown closes it (Q-250).
            GridInput::Click { pos, .. } if open && self.core.cursor() == Some(pos) => {
                vec![gi, GridInput::ToggleList]
            }
            GridInput::ToggleList if !open => vec![choices()],
            other => vec![other],
        }
    }
}

/// One cell press seen while painting.
struct Press {
    pos: CellPos,
    double: bool,
    mods: crate::core::keys::Mods,
}

impl Grid {
    fn cursor_info(&self) -> CursorInfo {
        let kind = self
            .core
            .cursor()
            .and_then(|c| self.spec().column(c.col))
            .map(|c| c.kind);
        let edit = self.core.edit();
        // An open calculator keeps Shift+arrows for its text.
        let calculator = match (kind, edit) {
            (Some(CellKind::Amount(rules)), Some(e)) => {
                amount_entry::preview(&e.buffer, rules) != Preview::None
            }
            _ => false,
        };
        CursorInfo {
            context_menu: self.spec().context_menu,
            menu_open: self.core.menu().is_some(),
            extend_rows: self.spec().select_column
                && edit.is_none_or(|e| e.popup.is_none())
                && !calculator,
            editable_grid: self.spec().editable,
            kind,
            editing: edit.is_some()
                && !matches!(kind, Some(CellKind::Checkbox | CellKind::Dropdown { .. })),
            buffer_empty: edit.is_none_or(|e| e.buffer.is_empty()),
            selects: self.spec().selection != crate::core::selection::SelectionMode::None,
            ime_active: self.ime_active,
            multiline: matches!(
                kind,
                Some(CellKind::Memo {
                    multiline: true,
                    ..
                })
            ),
            has_newline: edit.is_some_and(|e| e.buffer.contains('\n')),
        }
    }

    /// Bring the core's logical focus in line with egui's.
    fn sync_focus(
        &mut self,
        ctx: &egui::Context,
        model: &mut ModelRef<'_>,
        env: &GridEnv<'_>,
        events: &mut Vec<GridEvent>,
    ) {
        let egui_focus = ctx.memory(|m| m.has_focus(self.focus_id()));
        if egui_focus && !self.core.has_focus() {
            let from_below = ctx.input(|i| i.modifiers.shift);
            let (ev, _) = self.handle(GridInput::FocusGained { from_below }, model, env, ctx);
            events.extend(ev);
            sequencer::hold_navigation(ctx, ctx.viewport_id());
        } else if !egui_focus && self.core.has_focus() {
            let (ev, _) = self.handle(GridInput::FocusLost, model, env, ctx);
            events.extend(ev);
        }
    }

    fn handle(
        &mut self,
        gi: GridInput,
        model: &mut ModelRef<'_>,
        env: &GridEnv<'_>,
        ctx: &egui::Context,
    ) -> (Vec<GridEvent>, bool) {
        let fx = self.core.handle(gi, model, env.lookups, env.formats());
        let leave = fx.leave.is_some();
        (self.apply(fx, model, Some(ctx)), leave)
    }

    /// Open the context menu on `target` (a right-click at `at`, or
    /// Shift+F10, the Menu key or AccessKit `ShowContextMenu` under the
    /// cursor cell): the core commits the open cell and moves the cursor,
    /// then the screen gives the entries. Nothing happens unless the spec
    /// opts in and the screen provides a source.
    fn open_menu(
        &mut self,
        target: ContextTarget,
        at: Option<Pos2>,
        model: &mut ModelRef<'_>,
        env: &GridEnv<'_>,
        ctx: &egui::Context,
    ) -> Vec<GridEvent> {
        let Some(source) = env.context.filter(|_| self.spec().context_menu) else {
            return Vec::new();
        };
        let (ready, fx) = self
            .core
            .prepare_menu(target, model, env.lookups, env.formats());
        let events = self.apply(fx, model, Some(ctx));
        if ready && self.core.open_menu(target, source.items(target)) {
            self.menu_at = at;
            self.menu_rect = None;
            ctx.request_repaint();
        }
        let focus_id = self.focus_id();
        if !ctx.memory(|m| m.has_focus(focus_id)) {
            ctx.memory_mut(|m| m.request_focus(focus_id));
        }
        events
    }

    /// Draw the open context menu and act on a click on an entry.
    fn show_menu(
        &mut self,
        ctx: &egui::Context,
        model: &mut ModelRef<'_>,
        env: &GridEnv<'_>,
        grid_rect: Rect,
    ) -> Vec<GridEvent> {
        self.menu_rect = None;
        let Some((items, highlight)) = self.core.menu().map(|m| (m.items.clone(), m.highlight))
        else {
            self.menu_at = None;
            return Vec::new();
        };
        let at = self
            .menu_at
            .or_else(|| self.cursor_rect.map(|r| r.left_bottom()))
            .unwrap_or(grid_rect.min);
        let menu = self.menu_id();
        let view = crate::view::context_menu::MenuView {
            items: &items,
            highlight,
            label: env.t(TextKey("grid.a11y.menu")),
        };
        let (hit, r) = crate::view::context_menu::show(ctx, menu, at, &view, env.style);
        self.menu_rect = Some(r);
        // The focus widget controls the menu; the highlighted entry is its
        // active descendant (only nodes that exist this pass, as for the
        // list popup).
        if ctx.has_accesskit_node(menu) {
            let item = highlight
                .map(|i| crate::view::context_menu::item_id(menu, i))
                .filter(|i| ctx.has_accesskit_node(*i));
            ctx.accesskit_node_builder(self.focus_id(), |node| {
                a11y::popup_owner(node, menu, item);
            });
        }
        match hit {
            Some(i) => self.handle(GridInput::MenuPicked(i), model, env, ctx).0,
            None => Vec::new(),
        }
    }

    /// Draw the grid and handle this frame's input.
    // The frame's steps in order (size, model watch, focus, input, a11y
    // requests, wheel, paint, clicks) read best as one function.
    #[allow(clippy::too_many_lines)]
    pub fn show(&mut self, ui: &mut Ui, mut model: ModelRef<'_>, env: &GridEnv<'_>) -> GridOutput {
        let ctx = ui.ctx().clone();
        let style = env.style;
        let mut events = Vec::new();
        // Layout and size.
        let avail_w = ui.available_width().max(120.0);
        let gutter = self.gutter();
        self.layout = ColumnLayout::compute(
            &self.core.spec().columns,
            &self.effective_widths(),
            avail_w - BAR - gutter,
        );
        self.note_unit = units(style.note_line);
        let row_h = self.row_points(style);
        self.row_unit = Some(units(row_h));
        let header_h = style.header_line * if self.layout.two_lines { 2.0 } else { 1.0 };
        let n = model.rows().row_count();
        let body_h = match env.height {
            GridHeight::Fill {
                reserve_below,
                min_rows,
            } => (ui.clip_rect().bottom() - ui.cursor().top() - header_h - reserve_below)
                .max(f32::from(min_rows.max(1)) * row_h),
            GridHeight::Rows { min, max } => {
                let rows = n.clamp(usize::from(min.max(1)), usize::from(max.max(min).max(1)));
                f32::from(u16::try_from(rows).unwrap_or(u16::MAX)) * row_h
            }
        };
        let (rect, _) = ui.allocate_exact_size(vec2(avail_w, header_h + body_h), Sense::hover());
        let h = self.heights(&model);
        let body_units = units(body_h);
        if self.viewport.height() != body_units {
            self.viewport.set_height(body_units, &h, model.rows());
        }
        self.viewport.resync(&h, model.rows());
        self.core.set_page_rows(self.viewport.page_rows(&h));

        // The model changed outside the grid (undo, recompute, reload).
        let rev = model.rows().revision();
        if self.seen_rev.is_some_and(|r| r != rev) {
            let fx = self.core.resync(&mut model, env.formats(), false);
            events.extend(self.apply(fx, &model, Some(&ctx)));
        }
        self.seen_rev = Some(rev);

        let focus_id = self.focus_id();
        if !env.input_enabled {
            if ctx.memory(|m| m.has_focus(focus_id)) {
                ctx.memory_mut(|m| m.surrender_focus(focus_id));
                self.restore_focus = true;
            }
            if self.core.has_focus() {
                let fx = self.core.suspend(&mut model, env.lookups, env.formats());
                events.extend(self.apply(fx, &model, Some(&ctx)));
            }
            self.had_focus = false;
            let _ = self.paint(ui, rect, header_h, &mut model, env, false, true);
            return GridOutput {
                events,
                focused: false,
            };
        }
        if self.restore_focus || self.want_focus {
            ctx.memory_mut(|m| m.request_focus(focus_id));
            self.restore_focus = false;
            self.want_focus = false;
        }
        self.sync_focus(&ctx, &mut model, env, &mut events);
        let focused = ctx.memory(|m| m.has_focus(focus_id));
        let popup_rect = self.popup_rect;
        let menu_rect = self.menu_rect;
        let in_overlay = |p: Pos2| {
            popup_rect.is_some_and(|r| r.contains(p)) || menu_rect.is_some_and(|r| r.contains(p))
        };
        let press_inside = ctx.input(|i| {
            (i.pointer.any_pressed() || i.pointer.any_click())
                && i.pointer
                    .interact_pos()
                    .is_some_and(|p| rect.contains(p) || in_overlay(p))
        });
        // A press anywhere but on the open menu closes it (a press on a
        // cell then acts as usual).
        if self.core.menu().is_some()
            && ctx.input(|i| {
                i.pointer.any_pressed()
                    && i.pointer
                        .interact_pos()
                        .is_some_and(|p| !menu_rect.is_some_and(|r| r.contains(p)))
            })
        {
            self.core.close_menu();
        }
        ctx.input(|i| {
            for e in &i.events {
                match e {
                    egui::Event::Ime(egui::ImeEvent::Preedit { text, .. }) => {
                        self.ime_active = !text.is_empty();
                    }
                    egui::Event::Ime(egui::ImeEvent::Commit(_)) => self.ime_active = false,
                    _ => {}
                }
            }
        });

        // Input, consumed before anything draws (B2).
        let mut leaving = false;
        if focused {
            let raw = ctx.input_mut(|i| std::mem::take(&mut i.events));
            let mut collector = input::Collector::new(self.spec().block_paste);
            let mut kept = Vec::with_capacity(raw.len());
            for event in raw {
                let mut out = Vec::new();
                // The text typing appends to (none while the first key
                // replaces the selected text).
                let buffer = self
                    .core
                    .edit()
                    .filter(|e| !e.fresh)
                    .map_or_else(String::new, |e| e.buffer.clone());
                if let Some(keep) = collector.event(&event, self.cursor_info(), &buffer, &mut out) {
                    kept.push(keep);
                }
                for gi in out {
                    // Shift+F10 or the Menu key: the screen's menu on the
                    // cursor cell (with a menu open, the core closes it).
                    if matches!(gi, GridInput::Key(GridKey::ContextMenu, _))
                        && self.core.menu().is_none()
                    {
                        if let Some(pos) = self.core.cursor() {
                            events.extend(self.open_menu(
                                ContextTarget::Cell(pos),
                                None,
                                &mut model,
                                env,
                                &ctx,
                            ));
                        }
                        continue;
                    }
                    for gi in self.translate(gi, env) {
                        let (ev, leave) = self.handle(gi, &mut model, env, &ctx);
                        leaving |= leave;
                        events.extend(ev);
                    }
                }
            }
            kept.extend(collector.finish());
            ctx.input_mut(|i| i.events = kept);
            if let Some(p) = collector.paste {
                self.core.show_problem(p);
            }
            if let Some(at) = self.core.cursor() {
                events.extend(
                    collector
                        .blocks
                        .drain(..)
                        .map(|text| GridEvent::BlockPaste { at, text }),
                );
            }
            // Typing into an editor whose row scrolled away brings the row
            // back before the text lands.
            let typing = ctx.input(|i| {
                i.events
                    .iter()
                    .any(|e| matches!(e, egui::Event::Text(_) | egui::Event::Paste(_)))
            });
            if typing
                && self.core.edit().is_some()
                && let Some(c) = self.core.cursor()
                && let Some(i) = model.rows().index_of(c.row)
            {
                self.viewport.ensure_visible(i, &h, model.rows());
            }
        }

        // AccessKit requests aimed at last frame's cells, rows and the grid
        // (F5, §2.11).
        let mut presses: Vec<Press> = Vec::new();
        let mut set_values: Vec<(CellPos, String)> = Vec::new();
        let mut gutter_presses: Vec<(RowKey, Mods)> = Vec::new();
        let mut menu_reqs: Vec<(ContextTarget, Option<Pos2>)> = Vec::new();
        let offers_menu = self.spec().context_menu && env.context.is_some();
        let requests = ctx.input_mut(|i| {
            a11y::take_requests(
                i,
                self.id,
                &self.row_nodes,
                &self.cells,
                &self.gutters,
                offers_menu,
            )
        });
        for r in requests {
            match r {
                Request::Press(pos) => presses.push(Press {
                    pos,
                    double: false,
                    mods: crate::core::keys::Mods::NONE,
                }),
                Request::IntoView(key) => {
                    if let Some(i) = model.rows().index_of(key) {
                        self.viewport.ensure_visible(i, &h, model.rows());
                    }
                }
                Request::SetValue(pos, v) => set_values.push((pos, v)),
                Request::Page(forward) => self.viewport.page(forward, &h, model.rows()),
                Request::Gutter(key) => gutter_presses.push((key, Mods::NONE)),
                Request::Menu(pos) => menu_reqs.push((ContextTarget::Cell(pos), None)),
            }
        }

        // The wheel over an open popup scrolls its list, not the grid
        // (`popup_rect` is last frame's; the list's end is clamped when
        // it is drawn).
        if let Some(r) = self.popup_rect
            && ctx.input(|i| i.pointer.hover_pos()).is_some_and(|p| r.contains(p))
        {
            let dy = ctx.input_mut(|i| {
                let d = i.smooth_scroll_delta.y;
                i.smooth_scroll_delta = Vec2::ZERO;
                d
            });
            (self.popup_top, self.popup_wheel) =
                popup_wheel(self.popup_top, self.popup_wheel + dy, self.popup_row_h);
        }

        // The wheel scrolls the grid, not the screen around it (§12 R3).
        let overflow = (self.layout.content_width - (avail_w - BAR - gutter)).max(0.0);
        if ui.rect_contains_pointer(rect) {
            let (dx, dy) = ctx.input_mut(|i| {
                let d = i.smooth_scroll_delta;
                i.smooth_scroll_delta = Vec2::ZERO;
                (d.x, d.y)
            });
            // Columns wider than the grid scroll sideways, header and body
            // together.
            if overflow > 0.0 {
                self.hscroll = (self.hscroll - dx).clamp(0.0, overflow);
            }
            if dy != 0.0 {
                self.viewport.scroll_by(wheel_units(dy), &h, model.rows());
            }
        }

        // The cursor moved to another column: scroll sideways so it shows
        // (columns wider than the grid, a small window or a large zoom).
        let cursor_col = self.core.cursor().map(|c| c.col);
        if cursor_col != self.follow_col {
            self.follow_col = cursor_col;
            if let Some(p) = cursor_col.and_then(|c| self.layout.place(c)) {
                let view = avail_w - BAR - gutter;
                if p.x < self.hscroll {
                    self.hscroll = p.x;
                } else if p.x + p.width > self.hscroll + view {
                    self.hscroll = (p.x + p.width - view).min(p.x);
                }
                self.hscroll = self.hscroll.clamp(0.0, overflow);
            }
        }
        if self.pending_reset {
            self.reset_editor(&ctx);
        }
        let painted = self.paint(ui, rect, header_h, &mut model, env, focused, leaving);
        // A press anywhere else makes egui's focused widget give up focus
        // while it registers. A press on a cell, the scrollbar or the popup
        // is the grid's own: take focus straight back, filter included, so
        // the click commits like Tab instead of parking the editor (S11).
        if focused && press_inside && !ctx.memory(|m| m.has_focus(focus_id)) {
            ctx.memory_mut(|m| {
                m.request_focus(focus_id);
                m.set_focus_lock_filter(focus_id, FULL_FILTER);
            });
        }
        presses.extend(painted.presses);
        match painted.popup_hit {
            Some(PopupHit::Item(i)) => {
                let (ev, _) = self.handle(GridInput::PopupPicked(i), &mut model, env, &ctx);
                events.extend(ev);
            }
            Some(PopupHit::AddNew) => {
                let (ev, _) = self.handle(GridInput::AddNewPicked, &mut model, env, &ctx);
                events.extend(ev);
            }
            None => {}
        }
        if painted.resized {
            events.push(GridEvent::ColumnsResized);
        }
        if painted.toggles {
            for gi in self.translate(GridInput::ToggleList, env) {
                let (ev, _) = self.handle(gi, &mut model, env, &ctx);
                events.extend(ev);
            }
        }
        if painted.buffer_changed
            && let Some(text) = self.core.edit().map(|e| e.buffer.clone())
        {
            let (ev, _) = self.handle(GridInput::BufferEdited(text), &mut model, env, &ctx);
            events.extend(ev);
        }
        for p in presses {
            let click = GridInput::Click {
                pos: p.pos,
                double: p.double,
                mods: p.mods,
            };
            let was_cursor = self.core.cursor() == Some(p.pos);
            for gi in self.translate(click, env) {
                let (ev, _) = self.handle(gi, &mut model, env, &ctx);
                events.extend(ev);
            }
            // A click that moved onto a dropdown cell opens its choices at
            // once (Q-250); picker and tag cells open in the core.
            if !was_cursor
                && self.core.cursor() == Some(p.pos)
                && self.core.edit().is_some_and(|e| e.popup.is_none())
            {
                for gi in self.translate(GridInput::ToggleList, env) {
                    if matches!(gi, GridInput::OpenChoices(_)) {
                        let (ev, _) = self.handle(gi, &mut model, env, &ctx);
                        events.extend(ev);
                    }
                }
            }
            if !focused {
                ctx.memory_mut(|m| m.request_focus(focus_id));
            }
        }
        for (pos, value) in set_values {
            events.extend(self.set_value(pos, value, &mut model, env, &ctx));
            if !focused {
                ctx.memory_mut(|m| m.request_focus(focus_id));
            }
        }
        // Select checkboxes (P16 D6).
        gutter_presses.extend(painted.gutter_presses);
        for (row, mods) in gutter_presses {
            let (ev, _) = self.handle(GridInput::GutterClick { row, mods }, &mut model, env, &ctx);
            events.extend(ev);
            if !focused {
                ctx.memory_mut(|m| m.request_focus(focus_id));
            }
        }
        // Right-clicks and AccessKit ShowContextMenu open the menu.
        menu_reqs.extend(painted.menu_req.map(|(t, at)| (t, Some(at))));
        if let Some((target, at)) = menu_reqs.pop() {
            events.extend(self.open_menu(target, at, &mut model, env, &ctx));
        }
        events.extend(self.show_menu(&ctx, &mut model, env, rect));
        // egui's Tab traversal hands focus over while widgets register, so a
        // gain shows up only now: hold the next navigation key (§2.4.3).
        if !leaving {
            self.sync_focus(&ctx, &mut model, env, &mut events);
        }
        self.hscroll = self.hscroll.clamp(0.0, overflow);
        let range = self.viewport.visible_range(&h);
        model.rows_mut().visible_range(range);
        self.had_focus = ctx.memory(|m| m.has_focus(focus_id));
        if self.focus_retries > 0 {
            if self.had_focus {
                self.focus_retries = 0;
            } else {
                self.focus_retries -= 1;
                self.want_focus = true;
                ctx.request_repaint();
            }
        }
        GridOutput {
            events,
            focused: self.had_focus,
        }
    }
}

/// What painting saw.
struct Painted {
    presses: Vec<Press>,
    popup_hit: Option<PopupHit>,
    buffer_changed: bool,
    /// The list chevron on the open cell was clicked (Q-250).
    toggles: bool,
    resized: bool,
    /// Presses on select checkboxes (P16 D6).
    gutter_presses: Vec<(RowKey, Mods)>,
    /// A right-click on a cell or header, at the pointer.
    menu_req: Option<(ContextTarget, Pos2)>,
}

/// A cell's text as drawn.
struct Shown {
    text: String,
    placeholder: bool,
    mono: bool,
}

fn egui_align(a: crate::core::model::Align) -> Align2 {
    match a {
        crate::core::model::Align::Right => Align2::RIGHT_CENTER,
        crate::core::model::Align::Center => Align2::CENTER_CENTER,
        crate::core::model::Align::Left => Align2::LEFT_CENTER,
    }
}

impl Grid {
    fn cell_label(&self, env: &GridEnv<'_>, header: &str, index: usize, row_label: &str) -> String {
        let row = if self.spec().editable || row_label.is_empty() {
            env.t(TextKey("grid.a11y.line"))
                .replace("{n}", &group(index + 1))
        } else {
            row_label.to_string()
        };
        env.t(TextKey("grid.a11y.cell"))
            .replace("{column}", header)
            .replace("{row}", &row)
    }

    /// Draw the header; returns true when a column was resized, and a
    /// right-click on a column header (grids with a context menu).
    // One pass over the columns: resize handle, node, menu and text.
    #[allow(clippy::too_many_lines)]
    fn paint_header(
        &mut self,
        ui: &mut Ui,
        header: Rect,
        env: &GridEnv<'_>,
    ) -> (bool, Option<(ContextTarget, Pos2)>) {
        let style = env.style;
        let mut resized = false;
        let mut menu_req = None;
        let menu = self.spec().context_menu && env.context.is_some();
        let gutter = self.gutter();
        // Column headers start after the select gutter and never draw over
        // it when the columns scroll sideways.
        let columns_area =
            Rect::from_min_max(pos2(header.left() + gutter, header.top()), header.max);
        let ctx = ui.ctx().clone();
        let header_row = self.id.with("header");
        ui.painter()
            .rect_filled(header, CornerRadius::ZERO, style.header_fill);
        // Every Ui is an AccessKit node: this child Ui is the header row's
        // node, so the column headers are its direct children (R7).
        let mut header_ui = ui.new_child(
            UiBuilder::new()
                .scope_id(header_row)
                .max_rect(header)
                .accessibility_role(Role::Row),
        );
        header_ui.shrink_clip_rect(columns_area);
        // Bounds are what is visible: a grid scrolled partly out of its
        // panel reports only the part on screen.
        let vis = ui.clip_rect();
        let header_label = env.t(TextKey("grid.a11y.header_row"));
        ctx.accesskit_node_builder(header_row, |node| {
            node.set_row_index(0);
            node.set_label(header_label);
            node.set_bounds(ak_rect(header.intersect(vis)));
        });
        let columns = self.core.spec().columns.clone();
        let places = self.layout.places.clone();
        for (ci, place) in places.iter().enumerate() {
            let Some(col) = columns.iter().find(|c| c.id == place.id) else {
                continue;
            };
            let line = if place.line == Line::Second { 1.0 } else { 0.0 };
            let r = Rect::from_min_size(
                pos2(
                    columns_area.left() + place.x - self.hscroll,
                    header.top() + line * style.header_line,
                ),
                vec2(place.width, style.header_line),
            );
            // A resize handle on the right border: drag only, never in the
            // Tab order (B1, T25).
            if place.line == Line::First {
                let handle = Rect::from_center_size(
                    pos2(r.right(), r.center().y),
                    vec2(6.0, style.header_line),
                );
                let hid = self.id.with(("resize", col.id.0));
                let resp = header_ui.interact(handle, hid, Sense::DRAG);
                if resp.hovered() || resp.dragged() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeColumn);
                }
                let dx = resp.drag_delta().x;
                if resp.dragged() && dx != 0.0 {
                    let w = (place.width + dx)
                        .max(f32::from(col.width.min))
                        .min(4_000.0);
                    let w = u16::try_from(units(w) / 64).unwrap_or(u16::MAX);
                    match self.widths.iter_mut().find(|(i, _)| *i == col.id) {
                        Some(e) => e.1 = w,
                        None => self.widths.push((col.id, w)),
                    }
                    self.grown.retain(|(i, _)| *i != col.id);
                    resized = true;
                }
            }
            let text = env.t(col.header);
            let help = col.help.map(|k| env.t(k));
            let id = self.id.with(("header", col.id.0));
            // A header answers a right-click only where a menu is offered.
            let sense = if menu { Sense::CLICK } else { Sense::hover() };
            let resp = header_ui.interact(r, id, sense);
            if menu
                && resp.secondary_clicked()
                && let Some(p) = ctx.input(|i| i.pointer.latest_pos())
            {
                menu_req = Some((ContextTarget::Header(col.id), p));
            }
            if let Some(h) = &help {
                let _ = resp.on_hover_text(h.as_str());
            }
            ctx.accesskit_node_builder(id, |node| {
                node.set_bounds(ak_rect(r.intersect(columns_area).intersect(vis)));
                node.set_role(Role::ColumnHeader);
                node.set_label(text.clone());
                node.set_column_index(ci);
                if let Some(h) = help {
                    node.set_description(h);
                }
            });
            // The mark column's heading is the drawn check the cells use
            // (Plex's U+2713 reads as a radical); its name stays the
            // AccessKit label.
            if matches!(col.kind, CellKind::Mark) {
                check(
                    header_ui.painter(),
                    Rect::from_center_size(r.center(), vec2(9.0, 9.0)),
                    style.header_text,
                );
                continue;
            }
            let (anchor, x) = match col.align {
                crate::core::model::Align::Right => (Align2::RIGHT_CENTER, r.right() - style.pad),
                crate::core::model::Align::Center => (Align2::CENTER_CENTER, r.center().x),
                crate::core::model::Align::Left => (Align2::LEFT_CENTER, r.left() + style.pad),
            };
            header_ui.painter().text(
                pos2(x, r.center().y),
                anchor,
                text,
                style.header_font.clone(),
                style.header_text,
            );
        }
        (resized, menu_req)
    }

    fn row_fill(style: &GridStyle, state: RowState, is_cursor_row: bool) -> Option<Color32> {
        if state.contains(RowState::ERROR) {
            Some(style.warn_surface)
        } else if state.contains(RowState::LOCKED) {
            Some(style.locked_row)
        } else if state.contains(RowState::NEEDS_REVIEW) || state.contains(RowState::SUGGESTED) {
            Some(style.warn_tint)
        } else if is_cursor_row {
            Some(style.accent_tint)
        } else {
            None
        }
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    fn paint(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        header_h: f32,
        model: &mut ModelRef<'_>,
        env: &GridEnv<'_>,
        focused: bool,
        no_editor: bool,
    ) -> Painted {
        let style = env.style;
        let ctx = ui.ctx().clone();
        let fmt = env.formats();
        let mut presses = Vec::new();
        let mut buffer_changed = false;
        let mut toggles = false;
        let total = model.rows().row_count();
        let h = self.heights(model);
        let row_h = self.row_points(style);
        let header = Rect::from_min_size(rect.min, vec2(rect.width(), header_h));
        let body = Rect::from_min_max(
            pos2(rect.left(), header.bottom()),
            pos2(rect.right() - BAR, rect.bottom()),
        );
        let bar = Rect::from_min_max(pos2(rect.right() - BAR, header.bottom()), rect.max);
        // Cells start after the select gutter (P16 D6) and never draw over
        // it when the columns scroll sideways.
        let gutter = self.gutter();
        let cells_body = Rect::from_min_max(pos2(body.left() + gutter, body.top()), body.max);
        let cells_left = cells_body.left();
        self.drawn = Some((cells_body, header));
        let mut gutter_presses = Vec::new();
        let mut gutters = Vec::new();
        let mut menu_req = None;
        let offers_menu = self.spec().context_menu && env.context.is_some();
        let pointer_at = || ctx.input(|i| i.pointer.latest_pos());
        self.cursor_rect = None;
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, CornerRadius::ZERO, style.surface);

        // The grid's child Ui is the grid node; row Uis are the row nodes and
        // cells their direct children (R7: no intermediate containers).
        let columns = self.core.spec().columns.clone();
        let mut grid_ui = ui.new_child(
            UiBuilder::new()
                .scope_id(self.id)
                .max_rect(rect)
                .accessibility_role(Role::Grid)
                .accessibility_label(env.label),
        );
        grid_ui.shrink_clip_rect(rect);
        // What is on screen: node bounds never reach past the panel the
        // grid sits in (a scrolled screen body, the status bar).
        let vis = grid_ui.clip_rect();
        ctx.accesskit_node_builder(self.id, |node| {
            a11y::grid_node(node, rect.intersect(vis), total, columns.len());
        });
        let (resized, header_menu) = self.paint_header(&mut grid_ui, header, env);
        menu_req = menu_req.or(header_menu);

        // Rows.
        let range = self.viewport.visible_range(&h);
        let cursor = self.core.cursor();
        let cursor_index = cursor.and_then(|c| model.rows().index_of(c.row));
        let cursor_kind = cursor
            .and_then(|c| self.spec().column(c.col))
            .map(|c| c.kind);
        let editor_visible = !no_editor
            && self.core.edit().is_some()
            && cursor_index.is_some_and(|i| range.contains(&i))
            && !matches!(
                cursor_kind,
                Some(CellKind::Checkbox | CellKind::Dropdown { .. })
            );
        // The focus target when no editor is drawn (B1): the grid's first
        // focusable widget, a zero-size rect at the top of the body.
        if !editor_visible {
            let target = Rect::from_min_size(body.min, Vec2::ZERO);
            let r = grid_ui.interact(target, self.focus_id(), Sense::FOCUSABLE);
            if r.has_focus() && !no_editor {
                ctx.memory_mut(|m| m.set_focus_lock_filter(self.focus_id(), FULL_FILTER));
            }
        }
        let mut cells = Vec::new();
        let mut row_nodes = Vec::new();
        let mut grew = false;
        let line_h = if self.two_lines() { row_h / 2.0 } else { row_h };
        let places = self.layout.places.clone();
        // Rows never draw over the sticky header.
        let rows_painter = painter.with_clip_rect(body);
        for i in range.clone() {
            let top = body.top() + points(self.viewport.local_top(i, &h));
            // Measured rows may be taller than the base (a note line).
            let this_h = points(i64::try_from(h.height_of(i)).unwrap_or(0)).max(row_h);
            let row_rect = Rect::from_min_size(pos2(body.left(), top), vec2(body.width(), this_h));
            let Some(key) = model.rows().row_key(i) else {
                // Not loaded yet: a skeleton bar.
                rows_painter.rect_filled(
                    Rect::from_min_size(
                        row_rect.min + vec2(style.pad, row_h * 0.35),
                        vec2(row_rect.width() * 0.5, row_h * 0.3),
                    ),
                    CornerRadius::same(3),
                    style.placeholder.gamma_multiply(0.35),
                );
                continue;
            };
            let state = model.rows().row_state(i);
            // A multiline memo's extra lines (measured rows only).
            let note_pts = if model.rows().row_note(i).is_some() {
                style.note_line
            } else {
                0.0
            };
            let memo_pts = (this_h - row_h - note_pts).max(0.0);
            let memo_lines = if memo_pts > 0.5 {
                self.memo_extra_lines(model.rows(), i)
            } else {
                0
            };
            let selected = self.core.selection().contains(key);
            let is_cursor_row = cursor.is_some_and(|c| c.row == key);
            if let Some(f) = Self::row_fill(style, state, is_cursor_row) {
                rows_painter.rect_filled(row_rect.intersect(body), CornerRadius::ZERO, f);
            }
            let bar_rect = Rect::from_min_size(row_rect.min, vec2(3.0, this_h)).intersect(body);
            if state.contains(RowState::ERROR) {
                rows_painter.rect_filled(bar_rect, CornerRadius::ZERO, style.warn);
            }
            if selected {
                rows_painter.rect_filled(bar_rect, CornerRadius::ZERO, style.accent);
                rows_painter.rect_stroke(
                    row_rect.shrink(0.5).intersect(body),
                    CornerRadius::ZERO,
                    Stroke::new(1.0, style.accent_border),
                    StrokeKind::Inside,
                );
            }
            rows_painter.hline(
                row_rect.x_range(),
                row_rect.bottom() - 0.5,
                Stroke::new(1.0, style.row_border),
            );
            // Drawn marks (Plex has no lock glyph, Q-P06h): a lock at the
            // right end of the row, a 6 pt dirty dot in the left gutter.
            if state.contains(RowState::LOCKED) {
                lock(
                    &rows_painter,
                    pos2(row_rect.right() - 14.0, top + row_h / 2.0),
                    style.locked_text,
                );
            }
            if state.contains(RowState::DIRTY) {
                rows_painter.circle_filled(
                    pos2(row_rect.left() + 6.0, top + row_h / 2.0),
                    3.0,
                    style.accent,
                );
            }
            if let Some(note) = model.rows().row_note(i)
                && this_h > row_h
            {
                let (text, color) = match note {
                    crate::core::model::RowNote::Caption(t) => {
                        (t.to_string(), style.text_secondary)
                    }
                    crate::core::model::RowNote::Suggestion(t) => (t.to_string(), style.warn_text),
                    crate::core::model::RowNote::Error(k) => (env.t(k), style.warn_text),
                };
                rows_painter.text(
                    pos2(
                        cells_left + style.pad + 4.0 - self.hscroll,
                        top + row_h + memo_pts + style.note_line / 2.0,
                    ),
                    Align2::LEFT_CENTER,
                    text,
                    style.small.clone(),
                    color,
                );
            }

            let row_id = self.row_id(key);
            let row_label_text = model.rows().row_label(i);
            let row_of = env
                .t(TextKey("grid.a11y.row_of"))
                .replace("{n}", &group(i + 1))
                .replace("{total}", &group(total));
            let label = if row_label_text.is_empty() {
                row_of.clone()
            } else {
                format!("{row_label_text}, {row_of}")
            };
            let mut row_ui = grid_ui.new_child(
                UiBuilder::new()
                    .scope_id(row_id)
                    .max_rect(row_rect)
                    .accessibility_role(Role::Row),
            );
            row_ui.shrink_clip_rect(row_rect.intersect(body));
            let problem = match model.rows().row_note(i) {
                Some(crate::core::model::RowNote::Error(k)) => Some(env.t(k)),
                _ if state.contains(RowState::ERROR) => {
                    Some(env.t(TextKey("grid.a11y.row_problem")))
                }
                _ => None,
            };
            let row_info = RowInfo {
                label,
                row_of,
                index: i + 1,
                selected,
                problem,
            };
            ctx.accesskit_node_builder(row_id, |node| {
                a11y::row_node(node, row_rect.intersect(body).intersect(vis), &row_info);
            });
            row_nodes.push((row_id, key));
            // The select checkbox in the gutter (not on the ENTRY row): a
            // child of the row node, outside the cells' clip.
            if gutter > 0.0 && !state.contains(RowState::ENTRY) {
                let box_rect = Rect::from_center_size(
                    pos2(body.left() + gutter / 2.0 + 2.0, top + row_h / 2.0),
                    vec2(GUTTER_BOX, GUTTER_BOX),
                );
                let gid = self.gutter_id(key);
                let resp = row_ui.interact(box_rect.intersect(body), gid, Sense::CLICK);
                if resp.clicked() {
                    gutter_presses.push((key, ctx.input(|i| input::mods(i.modifiers))));
                }
                if box_rect.top() >= body.top() - 0.5 {
                    row_ui.painter().rect_stroke(
                        box_rect,
                        CornerRadius::same(2),
                        Stroke::new(
                            1.0,
                            if selected {
                                style.accent
                            } else {
                                style.text_secondary
                            },
                        ),
                        StrokeKind::Inside,
                    );
                    if selected {
                        check(row_ui.painter(), box_rect.shrink(3.0), style.accent);
                    }
                }
                let label = env
                    .t(TextKey("grid.a11y.select_row"))
                    .replace("{n}", &group(i + 1));
                ctx.accesskit_node_builder(gid, |node| {
                    a11y::gutter_node(
                        node,
                        box_rect.intersect(body).intersect(vis),
                        label,
                        selected,
                    );
                });
                gutters.push((gid, key));
            }
            row_ui.shrink_clip_rect(cells_body);
            let locked_text = state
                .contains(RowState::LOCKED)
                .then(|| env.t(TextKey("grid.a11y.locked")));
            let suggested_text = env.t(TextKey("grid.a11y.suggested"));
            // The lock glyph sits at the right end of a locked row: the
            // rightmost first-line cell keeps its text clear of it.
            let lock_col = state
                .contains(RowState::LOCKED)
                .then(|| {
                    places
                        .iter()
                        .filter(|p| p.line == Line::First)
                        .max_by(|a, b| (a.x + a.width).total_cmp(&(b.x + b.width)))
                        .map(|p| p.id)
                })
                .flatten();
            for (ci, place) in places.iter().enumerate() {
                let Some(col) = columns.iter().find(|c| c.id == place.id).copied() else {
                    continue;
                };
                let y = if place.line == Line::Second {
                    top + line_h
                } else {
                    top
                };
                let is_memo = matches!(
                    col.kind,
                    CellKind::Memo {
                        multiline: true,
                        ..
                    }
                );
                let cell_rect = Rect::from_min_size(
                    pos2(cells_left + place.x - self.hscroll, y),
                    vec2(place.width, line_h + if is_memo { memo_pts } else { 0.0 }),
                );
                let pos = CellPos {
                    row: key,
                    col: col.id,
                };
                let is_cursor = cursor == Some(pos);
                if is_cursor {
                    self.cursor_rect = Some(cell_rect);
                }
                let header_text = env.t(col.header);
                let cell_label = self.cell_label(env, &header_text, i, &row_label_text);
                if is_cursor && editor_visible {
                    let font = if col.mono {
                        style.mono.clone()
                    } else {
                        style.font.clone()
                    };
                    let max = match col.kind {
                        CellKind::Text { max_chars } | CellKind::Memo { max_chars, .. } => {
                            usize::try_from(max_chars).ok()
                        }
                        _ => None,
                    };
                    let focus_id = self.focus_id();
                    let mut cell_rect = cell_rect;
                    let mut chips_text = None;
                    // Picker and tag cells show the list chevron (Q-250).
                    let list_cell =
                        matches!(col.kind, CellKind::Picker { .. } | CellKind::Tags { .. });
                    let list_open = self.core.edit().is_some_and(|e| e.popup.is_some());
                    // An unaccepted suggestion stays visible in the open
                    // editor as a hint (Tab takes it); never the value, and
                    // only in cells that take suggestions (NN4).
                    let ghost = match model.rows().cell(i, col.id) {
                        CellValue::Suggested { display, .. } if col.kind.accepts_suggestions() => {
                            Some(display.to_string())
                        }
                        _ => None,
                    };
                    if let Some(edit) = self.core.edit_mut() {
                        let ghost = ghost.filter(|_| edit.buffer.is_empty());
                        // Tag chips sit before the text being typed.
                        let mut edit_rect = cell_rect;
                        if matches!(col.kind, CellKind::Tags { .. }) {
                            let used = crate::editors::tags::paint_chips(
                                row_ui.painter(),
                                cell_rect,
                                &edit.chips,
                                style,
                            );
                            edit_rect.min.x = (edit_rect.min.x + used).min(edit_rect.max.x - 24.0);
                            chips_text = Some(crate::editors::tags::chips_text(&edit.chips));
                        }
                        let te = match col.kind {
                            CellKind::Amount(_) => crate::editors::amount::editor(
                                &mut edit.buffer,
                                focus_id,
                                style,
                                place.width,
                            ),
                            CellKind::Date => crate::editors::date::editor(
                                &mut edit.buffer,
                                focus_id,
                                style,
                                place.width,
                            ),
                            CellKind::Memo {
                                multiline: true,
                                max_chars,
                            } => {
                                let extra = crate::editors::memo::extra_lines(&edit.buffer);
                                cell_rect.max.y +=
                                    points(i64::try_from(extra * self.note_unit).unwrap_or(0));
                                crate::editors::memo::editor(
                                    &mut edit.buffer,
                                    focus_id,
                                    font,
                                    place.width,
                                    usize::try_from(extra + 1).unwrap_or(1),
                                    usize::try_from(max_chars).unwrap_or(usize::MAX),
                                )
                            }
                            _ => crate::editors::text::editor_with_room(
                                &mut edit.buffer,
                                focus_id,
                                font,
                                egui_align(col.align),
                                edit_rect.width(),
                                style.pad,
                                if list_cell {
                                    crate::view::chevron::WIDTH
                                } else {
                                    0.0
                                },
                                max,
                            ),
                        };
                        let te = match ghost {
                            Some(g) => {
                                // A suggestion is text to read, not a placeholder: secondary
                                // text keeps it at 4.5:1 or more (A03 UI review 7).
                                te.hint_text(egui::RichText::new(g).color(style.text_secondary))
                            }
                            None => te,
                        };
                        let at = if matches!(col.kind, CellKind::Tags { .. }) {
                            edit_rect
                        } else {
                            cell_rect
                        };
                        let resp: Response = row_ui.put(at, te);
                        buffer_changed |= resp.changed();
                        if offers_menu
                            && resp.secondary_clicked()
                            && let Some(p) = pointer_at()
                        {
                            menu_req = Some((ContextTarget::Cell(pos), p));
                        }
                        if list_cell {
                            crate::view::chevron::paint(
                                row_ui.painter(),
                                cell_rect,
                                style.text_secondary,
                                list_open,
                            );
                            // A click on the open cell opens its list; a
                            // click on the chevron opens or closes it.
                            match crate::view::chevron::click(&resp, cell_rect) {
                                Some(crate::view::chevron::Click::Chevron) => toggles = true,
                                Some(crate::view::chevron::Click::Field) => {
                                    presses.push(Press {
                                        pos,
                                        double: false,
                                        mods: ctx.input(|i| input::mods(i.modifiers)),
                                    });
                                }
                                None => {}
                            }
                        }
                    }
                    ctx.accesskit_node_builder(focus_id, |node| {
                        node.set_label(cell_label.clone());
                        node.set_column_index(ci);
                        if let Some(chips) = chips_text.take().filter(|c| !c.is_empty()) {
                            node.set_description(chips);
                        }
                    });
                    if focused {
                        rows_painter.rect_stroke(
                            cell_rect.intersect(cells_body),
                            CornerRadius::ZERO,
                            Stroke::new(2.0, style.accent),
                            StrokeKind::Inside,
                        );
                    }
                    continue;
                }
                let cell_id = self.cell_id(pos);
                let mut resp =
                    row_ui.interact(cell_rect.intersect(cells_body), cell_id, Sense::CLICK);
                // A right-click opens the menu only where one is offered.
                if offers_menu
                    && resp.secondary_clicked()
                    && let Some(p) = pointer_at()
                {
                    menu_req = Some((ContextTarget::Cell(pos), p));
                }
                if let Some(h) = col.help {
                    resp = resp.on_hover_text(env.t(h));
                }
                if resp.clicked() || resp.double_clicked() {
                    let m = ctx.input(|i| input::mods(i.modifiers));
                    presses.push(Press {
                        pos,
                        double: resp.double_clicked(),
                        mods: m,
                    });
                }
                cells.push((cell_id, pos));
                let value = model.rows().cell(i, col.id);
                let shown = Self::display_text(&value, col.kind, state, &col, env, fmt);
                let editable = self.spec().editable && col.editable;
                // An empty cell's placeholder is a hint, not its value.
                let hint = (shown.placeholder && matches!(value, CellValue::Empty))
                    .then_some(shown.text.as_str());
                let help = col.help.map(|k| env.t(k));
                let info = CellInfo {
                    label: &cell_label,
                    value: if hint.is_some() { "" } else { &shown.text },
                    hint,
                    help: help.as_deref(),
                    column: ci,
                    editable,
                    toggled: match &value {
                        CellValue::Bool(b) => Some(*b),
                        _ => None,
                    },
                    locked: locked_text.as_deref(),
                    problem: row_info.problem.as_deref(),
                    suggested: (matches!(value, CellValue::Suggested { .. })
                        && col.kind.accepts_suggestions())
                    .then_some(suggested_text.as_str()),
                    menu: offers_menu,
                };
                ctx.accesskit_node_builder(cell_id, |node| {
                    a11y::cell_node(node, cell_rect.intersect(cells_body).intersect(vis), &info);
                });
                let suggested_desc = info.suggested.map(|s| s.replace("{value}", &shown.text));
                // The cursor cell without a text editor: the focus target's
                // node speaks for it (a checkbox, a combo box, a cell).
                if is_cursor && !no_editor {
                    let popup_open = self.core.edit().is_some_and(|e| e.popup.is_some());
                    ctx.accesskit_node_builder(self.focus_id(), |node| {
                        node.set_label(cell_label);
                        node.set_column_index(ci);
                        match (col.kind, &value) {
                            (CellKind::Checkbox, CellValue::Bool(b)) => {
                                crate::editors::checkbox::node(node, *b);
                            }
                            (CellKind::Dropdown { .. }, _) => {
                                crate::editors::dropdown::node(node, &shown.text, popup_open);
                            }
                            _ => {
                                node.set_role(if editable { Role::GridCell } else { Role::Cell });
                                match (hint, suggested_desc) {
                                    (Some(h), _) => {
                                        node.set_value("");
                                        node.set_description(h);
                                    }
                                    // An unaccepted suggestion is never the value.
                                    (None, Some(s)) => {
                                        node.set_value("");
                                        node.set_description(s);
                                    }
                                    (None, None) => node.set_value(shown.text.clone()),
                                }
                            }
                        }
                    });
                }
                let paint_rect = if lock_col == Some(col.id) {
                    let mut r = cell_rect;
                    r.max.x = (r.max.x - LOCK_GUTTER).max(r.min.x);
                    r
                } else {
                    cell_rect
                };
                // A line scrolled partly under the header is left blank
                // rather than drawn as half a line of text.
                let cut = cell_rect.top() < body.top() - 0.5;
                // An editable list cell shows its chevron under the pointer
                // and on the cursor (Q-250): a click opens the list. Its text
                // keeps clear of it (a long name ends in "…" before it).
                let chevron = !cut
                    && editable
                    && !state.contains(RowState::LOCKED)
                    && matches!(
                        col.kind,
                        CellKind::Picker { .. } | CellKind::Tags { .. } | CellKind::Dropdown { .. }
                    )
                    && (is_cursor || resp.hovered());
                let text_rect = if chevron {
                    let mut r = paint_rect;
                    r.max.x = (r.max.x - crate::view::chevron::WIDTH).max(r.min.x);
                    r
                } else {
                    paint_rect
                };
                let needed = if cut {
                    0.0
                } else {
                    Self::paint_cell(
                        row_ui.painter(),
                        text_rect,
                        &col,
                        &value,
                        &shown,
                        state,
                        style,
                        if is_memo { memo_lines + 1 } else { 1 },
                    ) + if paint_rect == cell_rect {
                        0.0
                    } else {
                        LOCK_GUTTER
                    }
                };
                if chevron {
                    let open = is_cursor && self.core.edit().is_some_and(|e| e.popup.is_some());
                    crate::view::chevron::paint(
                        row_ui.painter(),
                        paint_rect,
                        style.text_secondary,
                        open,
                    );
                }
                // Numbers are never cut short: the column grows to fit and
                // never shrinks this session (Q-P10o, S14).
                if shown.mono && needed > place.width + 0.5 {
                    let w = u16::try_from(units(needed.ceil()) / 64).unwrap_or(u16::MAX);
                    match self.grown.iter_mut().find(|(c, _)| *c == col.id) {
                        Some(e) => e.1 = e.1.max(w),
                        None => self.grown.push((col.id, w)),
                    }
                    grew = true;
                }
                if is_cursor && focused {
                    rows_painter.rect_stroke(
                        cell_rect.intersect(cells_body),
                        CornerRadius::ZERO,
                        Stroke::new(2.0, style.accent),
                        StrokeKind::Inside,
                    );
                }
            }
        }
        self.cells = cells;
        self.row_nodes = row_nodes;
        self.gutters = gutters;
        self.rows_drawn = range.len();

        // Scrollbar (integer thumb maths).
        let track = units(bar.height());
        let (start, len) = self.viewport.thumb(track, units(24.0), &h);
        if len < track {
            painter.rect_filled(bar, CornerRadius::ZERO, style.header_fill);
            let thumb = Rect::from_min_size(
                pos2(
                    bar.left() + 2.0,
                    bar.top() + points(i64::try_from(start).unwrap_or(0)),
                ),
                vec2(bar.width() - 4.0, points(i64::try_from(len).unwrap_or(0))),
            );
            painter.rect_filled(thumb, CornerRadius::same(4), style.placeholder);
            let bar_id = self.id.with("scrollbar");
            let r = grid_ui.interact(bar, bar_id, Sense::CLICK | Sense::DRAG);
            if let Some(p) = r.interact_pointer_pos()
                && (r.dragged() || r.clicked())
            {
                let at = units(p.y - bar.top() - points(i64::try_from(len).unwrap_or(0)) / 2.0);
                let pos = self.viewport.pos_for_thumb(at, track, len, &h);
                self.viewport.set_pos(pos, &h, model.rows());
            }
            let top = self.viewport.top();
            ctx.accesskit_node_builder(bar_id, |node| {
                a11y::scrollbar_node(
                    node,
                    bar.intersect(vis),
                    env.t(TextKey("grid.a11y.scrollbar")),
                    top,
                    total,
                );
                node.set_value(group(top + 1));
            });
        }

        // The popup under the open cell (choices, type-ahead, full list).
        let mut popup_hit = None;
        self.popup_rect = None;
        let mut caption_top = None;
        if let (Some(pos), Some(i)) = (cursor, cursor_index)
            && range.contains(&i)
            && let Some(place) = self.layout.place(pos.col)
        {
            let mut top = body.top() + points(self.viewport.local_top(i, &h)) + row_h;
            if !no_editor && let Some(view) = self.popup_view(pos, place.width, env) {
                let (hit, r) = crate::view::overlay::popup(
                    &ctx,
                    self.popup_id(),
                    pos2(cells_left + place.x - self.hscroll, top),
                    &view,
                    style,
                );
                let (shown_top, shown_highlight) = (view.top, view.highlight);
                self.popup_top = shown_top;
                self.popup_seen = Some(shown_highlight);
                self.popup_row_h = style.row_one.max(18.0);
                popup_hit = hit;
                self.popup_rect = Some(r);
                top = r.bottom();
                // Relations only to nodes that exist this pass: egui gives a
                // popup no nodes in its first (invisible, sizing) pass, and a
                // relation to a missing node makes the platform adapter
                // panic (accesskit_consumer `controls`).
                if let Some((list, item)) = self.popup_owner()
                    && ctx.has_accesskit_node(list)
                {
                    let item = item.filter(|i| ctx.has_accesskit_node(*i));
                    ctx.accesskit_node_builder(self.focus_id(), |node| {
                        a11y::popup_owner(node, list, item);
                    });
                }
            }
            caption_top = Some((top, place.x));
        }
        // No popup this frame: the next one opens on its highlight.
        if self.popup_rect.is_none() {
            self.popup_top = 0;
            self.popup_wheel = 0.0;
            self.popup_seen = None;
        }
        // The caption under the open cell: calculator preview, error, notice.
        if let (Some(pos), Some((top, x))) = (cursor, caption_top)
            && let Some(text) = self.caption(pos, env)
        {
            crate::view::overlay::caption(
                &ctx,
                self.id.with("caption"),
                pos2(cells_left + x - self.hscroll, top),
                &text,
                style,
            );
        }
        if grew {
            ctx.request_repaint();
        }
        Painted {
            presses,
            popup_hit,
            buffer_changed,
            toggles,
            resized,
            gutter_presses,
            menu_req,
        }
    }

    fn caption(&self, pos: CellPos, env: &GridEnv<'_>) -> Option<String> {
        let edit = self.core.edit()?;
        let rules = match self.spec().column(pos.col).map(|c| c.kind) {
            Some(CellKind::Amount(rules)) => Some(rules),
            _ => None,
        };
        if let Some(p) = edit.error {
            // An amount error: the message and its fix.
            if let Some(rules) = rules
                && let Err(e) = amount_entry::parse(&edit.buffer, rules)
            {
                return Some(crate::editors::calc::error_text(&e, env.text));
            }
            let mut s = env.t(p.key);
            if let Some(fix) = p.fix {
                s.push_str(". ");
                s.push_str(&env.t(fix));
            }
            return Some(s);
        }
        if let Some(c) = edit.caption {
            return Some(env.t(c));
        }
        if let Some(rules) = rules
            && edit.is_dirty()
            && amount_entry::preview(&edit.buffer, rules) != Preview::None
        {
            return crate::editors::calc::caption(&edit.buffer, rules, env.money_style, env.text);
        }
        self.core.notice().map(|k| env.t(k))
    }

    fn display_text(
        value: &CellValue<'_>,
        kind: CellKind,
        state: RowState,
        col: &crate::core::model::Column,
        env: &GridEnv<'_>,
        fmt: Formats,
    ) -> Shown {
        let plain = |text: String| Shown {
            text,
            placeholder: false,
            mono: col.mono,
        };
        match value {
            CellValue::Suggested { display, .. } => {
                // NN4: only picker and tag cells show a suggestion. Anywhere
                // else (an amount) it renders empty, so a model bug can never
                // put an AI amount on screen.
                if kind.accepts_suggestions() {
                    Shown {
                        text: display.to_string(),
                        placeholder: true,
                        mono: false,
                    }
                } else {
                    plain(String::new())
                }
            }
            CellValue::Empty if state.contains(RowState::ENTRY) => Shown {
                text: col.placeholder.map(|k| env.t(k)).unwrap_or_default(),
                placeholder: true,
                mono: false,
            },
            CellValue::Empty if col.empty.is_some() => Shown {
                text: col.empty.map(|k| env.t(k)).unwrap_or_default(),
                placeholder: true,
                mono: false,
            },
            CellValue::Choice(id) => {
                let label = match kind {
                    CellKind::Dropdown { options } => options
                        .iter()
                        .find(|o| o.id == *id)
                        .map(|o| env.t(o.label))
                        .unwrap_or_default(),
                    _ => String::new(),
                };
                plain(label)
            }
            CellValue::Tags(chips) => plain(crate::editors::tags::chips_text(chips)),
            CellValue::Bool(b) => plain(env.t(if *b {
                TextKey("grid.a11y.checked")
            } else {
                TextKey("grid.a11y.unchecked")
            })),
            CellValue::Mark(m) => plain(match m {
                MarkKind::None => String::new(),
                MarkKind::Cleared => env.t(TextKey("grid.a11y.cleared")),
                MarkKind::Reconciled => env.t(TextKey("grid.a11y.reconciled")),
            }),
            CellValue::Loading => Shown {
                text: env.t(TextKey("grid.a11y.loading")),
                placeholder: true,
                mono: false,
            },
            other => plain(cell_text(other, fmt)),
        }
    }

    // The cell, its value and how it is shown, plus the row it sits in.
    #[allow(clippy::too_many_arguments)]
    fn paint_cell(
        painter: &egui::Painter,
        rect: Rect,
        col: &crate::core::model::Column,
        value: &CellValue<'_>,
        shown: &Shown,
        state: RowState,
        style: &GridStyle,
        max_rows: u64,
    ) -> f32 {
        let color = if shown.placeholder && matches!(value, CellValue::Suggested { .. }) {
            // A suggestion is read, not a placeholder: secondary text keeps
            // it at 4.5:1 or more (A03 UI review 7).
            style.text_secondary
        } else if shown.placeholder {
            style.placeholder
        } else if state.contains(RowState::LOCKED) {
            style.locked_text
        } else if col.line == Line::Second || state.contains(RowState::MUTED) {
            style.text_secondary
        } else {
            style.text
        };
        match value {
            CellValue::Bool(b) => {
                let r = Rect::from_center_size(rect.center(), vec2(14.0, 14.0));
                painter.rect_stroke(
                    r,
                    CornerRadius::same(2),
                    Stroke::new(1.0, style.text_secondary),
                    StrokeKind::Inside,
                );
                if *b {
                    check(painter, r.shrink(3.0), style.accent);
                }
                return 0.0;
            }
            CellValue::Mark(MarkKind::Cleared) => {
                check(
                    painter,
                    Rect::from_center_size(rect.center(), vec2(10.0, 10.0)),
                    style.text_secondary,
                );
                return 0.0;
            }
            CellValue::Mark(MarkKind::Reconciled) => {
                painter.text(
                    rect.center(),
                    Align2::CENTER_CENTER,
                    "R",
                    style.mono.clone(),
                    style.text_secondary,
                );
                return 0.0;
            }
            CellValue::Mark(MarkKind::None) | CellValue::Loading => return 0.0,
            _ => {}
        }
        let font: FontId = if col.line == Line::Second {
            style.small.clone()
        } else if shown.mono {
            style.mono.clone()
        } else {
            style.font.clone()
        };
        let inner = rect.shrink2(vec2(style.pad, 0.0));
        let mut job = egui::text::LayoutJob::simple_singleline(shown.text.clone(), font, color);
        // Amounts, dates and numbers are never cut short (Q-P10o); text gets
        // an ellipsis.
        if !shown.mono {
            job.wrap = egui::text::TextWrapping::truncate_at_width(inner.width().max(1.0));
            // A multiline memo shows its lines, up to the row's height.
            job.wrap.max_rows = usize::try_from(max_rows.max(1)).unwrap_or(1);
        }
        let galley = painter.layout_job(job);
        let x = match col.align {
            crate::core::model::Align::Right => inner.right() - galley.size().x,
            crate::core::model::Align::Center => inner.center().x - galley.size().x / 2.0,
            crate::core::model::Align::Left => inner.left(),
        };
        let y = rect.center().y - galley.size().y / 2.0;
        let needed = galley.size().x + 2.0 * style.pad;
        painter.galley(Pos2::new(x, y), galley, color);
        needed
    }
}

/// A drawn padlock (Plex has no lock glyph, Q-P06h).
fn lock(painter: &egui::Painter, center: Pos2, color: Color32) {
    let body = Rect::from_center_size(center + vec2(0.0, 2.0), vec2(9.0, 7.0));
    painter.rect_filled(body, CornerRadius::same(1), color);
    let shackle = Rect::from_center_size(center + vec2(0.0, -2.5), vec2(6.0, 6.0));
    painter.rect_stroke(
        shackle,
        CornerRadius::same(3),
        Stroke::new(1.4, color),
        StrokeKind::Inside,
    );
}

/// A drawn check mark (Plex's U+2713 reads as a radical, Q-P06h). Public so
/// screens draw the same mark outside the grid (the register legend).
pub fn check(painter: &egui::Painter, r: Rect, color: Color32) {
    let a = pos2(r.left(), r.center().y);
    let b = pos2(r.left() + r.width() * 0.4, r.bottom());
    let c = pos2(r.right(), r.top());
    painter.line_segment([a, b], Stroke::new(1.8, color));
    painter.line_segment([b, c], Stroke::new(1.8, color));
}

#[cfg(test)]
mod popup_wheel_tests {
    use super::popup_wheel;

    #[test]
    fn the_wheel_moves_the_popup_whole_rows_at_a_time() {
        // Up three rows and a bit: three rows nearer the top, the bit kept.
        let (top, left) = popup_wheel(10, 3.5 * 20.0, 20.0);
        assert_eq!(top, 7);
        assert!((left - 10.0).abs() < 0.01);
        // Down two rows.
        let (top, left) = popup_wheel(10, -40.0, 20.0);
        assert_eq!(top, 12);
        assert!(left.abs() < 0.01);
        // Never above the first entry.
        assert_eq!(popup_wheel(1, 200.0, 20.0).0, 0);
        // Less than a row moves nothing yet.
        assert_eq!(popup_wheel(5, 12.0, 20.0), (5, 12.0));
    }
}