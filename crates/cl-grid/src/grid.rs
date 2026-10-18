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
use egui::{
    Align2, Color32, CornerRadius, EventFilter, FontId, Id, Pos2, Rect, Response, Role, Sense,
    Stroke, StrokeKind, Ui, UiBuilder, Vec2, pos2, vec2,
};

use crate::core::codec::cell_text;
use crate::core::keys::{Formats, GridInput};
use crate::core::model::{
    CellKind, CellPos, CellProblem, CellValue, ColumnId, Line, Lookups, MarkKind, ModelRef,
    RowHeight, RowState,
};
use crate::core::rowkey::RowKey;
use crate::core::spec::{GridCommand, GridEvent, GridSpec, Handled, SpecError};
use crate::core::state::{Core, Effects};
use crate::core::viewport::{Heights, RowAlign, Viewport};
use crate::sequencer;
use crate::view::input::{self, CursorInfo};
use crate::view::layout::ColumnLayout;
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
}

impl GridEnv<'_> {
    fn formats(&self) -> Formats {
        Formats {
            today: self.today,
            first_weekday: self.first_weekday,
            date_order: self.date_order,
            money: self.money_style,
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
    seen_rev: Option<u64>,
    ime_active: bool,
    cells: Vec<(Id, CellPos)>,
    rows_drawn: usize,
    row_unit: Option<u64>,
    pending_reset: bool,
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

impl Grid {
    /// A grid with this spec.
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
            seen_rev: None,
            ime_active: false,
            cells: Vec::new(),
            rows_drawn: 0,
            row_unit: None,
            pending_reset: false,
        })
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
        let row = units(if self.two_lines() { 44.0 } else { 30.0 });
        Heights::uniform(self.row_unit.unwrap_or(row), model.rows().row_count())
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
        CursorInfo {
            editable_grid: self.spec().editable,
            kind,
            editing: edit.is_some()
                && !matches!(kind, Some(CellKind::Checkbox | CellKind::Dropdown { .. })),
            buffer_empty: edit.is_none_or(|e| e.buffer.is_empty()),
            selects: self.spec().selection != crate::core::selection::SelectionMode::None,
            ime_active: self.ime_active,
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
        self.layout = ColumnLayout::compute(&self.core.spec().columns, &self.widths, avail_w - BAR);
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
            let mut collector = input::Collector::default();
            let mut kept = Vec::with_capacity(raw.len());
            for event in raw {
                let mut out = Vec::new();
                if let Some(keep) = collector.event(&event, self.cursor_info(), &mut out) {
                    kept.push(keep);
                }
                for gi in out {
                    let (ev, leave) = self.handle(gi, &mut model, env, &ctx);
                    leaving |= leave;
                    events.extend(ev);
                }
            }
            ctx.input_mut(|i| i.events = kept);
            if let Some(p) = collector.paste {
                self.core.show_problem(p);
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

        // AccessKit requests aimed at last frame's cells and the grid (F5).
        let mut presses: Vec<Press> = Vec::new();
        let mut page: Option<bool> = None;
        ctx.input_mut(|i| {
            for (id, pos) in &self.cells {
                i.consume_accesskit_action_requests(*id, |r| {
                    let hit = matches!(
                        r.action,
                        egui::accesskit::Action::Focus | egui::accesskit::Action::Click
                    );
                    if hit {
                        presses.push(Press {
                            pos: *pos,
                            double: false,
                            mods: crate::core::keys::Mods::NONE,
                        });
                    }
                    hit
                });
            }
            i.consume_accesskit_action_requests(self.id, |r| match r.action {
                egui::accesskit::Action::ScrollDown => {
                    page = Some(true);
                    true
                }
                egui::accesskit::Action::ScrollUp => {
                    page = Some(false);
                    true
                }
                _ => false,
            });
        });
        if let Some(forward) = page {
            self.viewport.page(forward, &h, model.rows());
        }

        // The wheel scrolls the grid, not the screen around it (§12 R3).
        if ui.rect_contains_pointer(rect) {
            let dy = ctx.input_mut(|i| {
                let d = i.smooth_scroll_delta.y;
                i.smooth_scroll_delta.y = 0.0;
                d
            });
            if dy != 0.0 {
                self.viewport.scroll_by(wheel_units(dy), &h, model.rows());
            }
        }

        if self.pending_reset {
            self.reset_editor(&ctx);
        }
        let painted = self.paint(ui, rect, header_h, &mut model, env, focused, leaving);
        presses.extend(painted.presses);
        if painted.buffer_changed
            && let Some(text) = self.core.edit().map(|e| e.buffer.clone())
        {
            let (ev, _) = self.handle(GridInput::BufferEdited(text), &mut model, env, &ctx);
            events.extend(ev);
        }
        for p in presses {
            let (ev, _) = self.handle(
                GridInput::Click {
                    pos: p.pos,
                    double: p.double,
                    mods: p.mods,
                },
                &mut model,
                env,
                &ctx,
            );
            events.extend(ev);
            if !focused {
                ctx.memory_mut(|m| m.request_focus(focus_id));
            }
        }
        // egui's Tab traversal hands focus over while widgets register, so a
        // gain shows up only now: hold the next navigation key (§2.4.3).
        if !leaving {
            self.sync_focus(&ctx, &mut model, env, &mut events);
        }
        let range = self.viewport.visible_range(&h);
        model.rows_mut().visible_range(range);
        self.had_focus = ctx.memory(|m| m.has_focus(focus_id));
        GridOutput {
            events,
            focused: self.had_focus,
        }
    }
}

/// What painting saw.
struct Painted {
    presses: Vec<Press>,
    buffer_changed: bool,
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

    fn paint_header(&self, ui: &mut Ui, header: Rect, env: &GridEnv<'_>) {
        let style = env.style;
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
        header_ui.set_clip_rect(header);
        ctx.accesskit_node_builder(header_row, |node| node.set_row_index(0));
        let columns = &self.core.spec().columns;
        for (ci, place) in self.layout.places.iter().enumerate() {
            let Some(col) = columns.iter().find(|c| c.id == place.id) else {
                continue;
            };
            let line = if place.line == Line::Second { 1.0 } else { 0.0 };
            let r = Rect::from_min_size(
                pos2(
                    header.left() + place.x,
                    header.top() + line * style.header_line,
                ),
                vec2(place.width, style.header_line),
            );
            let text = env.t(col.header);
            let id = self.id.with(("header", col.id.0));
            let _ = header_ui.interact(r, id, Sense::hover());
            ctx.accesskit_node_builder(id, |node| {
                node.set_role(Role::ColumnHeader);
                node.set_label(text.clone());
                node.set_column_index(ci);
            });
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
        let n = model.rows().row_count();
        let h = self.heights(model);
        let row_h = self.row_points(style);
        let header = Rect::from_min_size(rect.min, vec2(rect.width(), header_h));
        let body = Rect::from_min_max(
            pos2(rect.left(), header.bottom()),
            pos2(rect.right() - BAR, rect.bottom()),
        );
        let bar = Rect::from_min_max(pos2(rect.right() - BAR, header.bottom()), rect.max);
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
        grid_ui.set_clip_rect(rect);
        ctx.accesskit_node_builder(self.id, |node| {
            node.set_row_count(n + 1);
            node.set_column_count(columns.len());
        });
        self.paint_header(&mut grid_ui, header, env);

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
        let line_h = if self.two_lines() { row_h / 2.0 } else { row_h };
        let places = self.layout.places.clone();
        for i in range.clone() {
            let top = body.top() + points(self.viewport.local_top(i, &h));
            let row_rect = Rect::from_min_size(pos2(body.left(), top), vec2(body.width(), row_h));
            let Some(key) = model.rows().row_key(i) else {
                // Not loaded yet: a skeleton bar.
                painter.rect_filled(
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
            let selected = self.core.selection().contains(key);
            let is_cursor_row = cursor.is_some_and(|c| c.row == key);
            if let Some(f) = Self::row_fill(style, state, is_cursor_row) {
                painter.rect_filled(row_rect.intersect(body), CornerRadius::ZERO, f);
            }
            let bar_rect = Rect::from_min_size(row_rect.min, vec2(3.0, row_h)).intersect(body);
            if state.contains(RowState::ERROR) {
                painter.rect_filled(bar_rect, CornerRadius::ZERO, style.warn);
            }
            if selected {
                painter.rect_filled(bar_rect, CornerRadius::ZERO, style.accent);
                painter.rect_stroke(
                    row_rect.shrink(0.5).intersect(body),
                    CornerRadius::ZERO,
                    Stroke::new(1.0, style.accent_border),
                    StrokeKind::Inside,
                );
            }
            painter.hline(
                row_rect.x_range(),
                row_rect.bottom() - 0.5,
                Stroke::new(1.0, style.row_border),
            );

            let row_id = self.row_id(key);
            let row_label_text = model.rows().row_label(i);
            let row_of = env
                .t(TextKey("grid.a11y.row_of"))
                .replace("{n}", &group(i + 1))
                .replace("{total}", &group(n));
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
            row_ui.set_clip_rect(row_rect.intersect(body));
            ctx.accesskit_node_builder(row_id, |node| {
                node.set_label(label);
                node.set_row_index(i + 1);
                node.set_row_index_text(row_of);
                if selected {
                    node.set_selected(true);
                }
            });
            for (ci, place) in places.iter().enumerate() {
                let Some(col) = columns.iter().find(|c| c.id == place.id).copied() else {
                    continue;
                };
                let y = if place.line == Line::Second {
                    top + line_h
                } else {
                    top
                };
                let cell_rect =
                    Rect::from_min_size(pos2(body.left() + place.x, y), vec2(place.width, line_h));
                let pos = CellPos {
                    row: key,
                    col: col.id,
                };
                let is_cursor = cursor == Some(pos);
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
                    if let Some(edit) = self.core.edit_mut() {
                        let te = if matches!(col.kind, CellKind::Amount(_)) {
                            crate::editors::amount::editor(
                                &mut edit.buffer,
                                focus_id,
                                style,
                                place.width,
                            )
                        } else {
                            crate::editors::text::editor(
                                &mut edit.buffer,
                                focus_id,
                                font,
                                egui_align(col.align),
                                place.width,
                                style.pad,
                                max,
                            )
                        };
                        let resp: Response = row_ui.put(cell_rect, te);
                        buffer_changed |= resp.changed();
                    }
                    ctx.accesskit_node_builder(focus_id, |node| {
                        node.set_label(cell_label.clone());
                        node.set_column_index(ci);
                    });
                    if focused {
                        painter.rect_stroke(
                            cell_rect.intersect(body),
                            CornerRadius::ZERO,
                            Stroke::new(2.0, style.accent),
                            StrokeKind::Inside,
                        );
                    }
                    continue;
                }
                let cell_id = self.cell_id(pos);
                let resp = row_ui.interact(cell_rect.intersect(body), cell_id, Sense::CLICK);
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
                ctx.accesskit_node_builder(cell_id, |node| {
                    node.set_role(if editable { Role::GridCell } else { Role::Cell });
                    node.set_label(cell_label);
                    node.set_value(shown.text.clone());
                    node.set_column_index(ci);
                    node.add_action(egui::accesskit::Action::Focus);
                    node.add_action(egui::accesskit::Action::Click);
                    if state.contains(RowState::LOCKED) {
                        node.set_read_only();
                        node.set_description(env.t(TextKey("grid.a11y.locked")));
                    }
                    if state.contains(RowState::ERROR) {
                        node.set_invalid(egui::accesskit::Invalid::True);
                    }
                });
                Self::paint_cell(
                    row_ui.painter(),
                    cell_rect,
                    &col,
                    &value,
                    &shown,
                    state,
                    style,
                );
                if is_cursor && focused {
                    painter.rect_stroke(
                        cell_rect.intersect(body),
                        CornerRadius::ZERO,
                        Stroke::new(2.0, style.accent),
                        StrokeKind::Inside,
                    );
                }
            }
        }
        self.cells = cells;
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
                node.set_role(Role::ScrollBar);
                node.set_label(env.t(TextKey("grid.a11y.scrollbar")));
                node.set_value(group(top + 1));
            });
        }

        // The caption under the open cell: calculator preview, error, notice.
        if let (Some(pos), Some(i)) = (cursor, cursor_index)
            && range.contains(&i)
            && let Some(place) = self.layout.place(pos.col)
            && let Some(text) = self.caption(pos, env)
        {
            let top = body.top() + points(self.viewport.local_top(i, &h)) + row_h;
            crate::view::overlay::caption(
                &ctx,
                self.id.with("caption"),
                pos2(body.left() + place.x, top),
                &text,
                style,
            );
        }
        Painted {
            presses,
            buffer_changed,
        }
    }

    fn caption(&self, pos: CellPos, env: &GridEnv<'_>) -> Option<String> {
        let edit = self.core.edit()?;
        let rules = match self.spec().column(pos.col).map(|c| c.kind) {
            Some(CellKind::Amount(rules)) => Some(rules),
            _ => None,
        };
        if let Some(p) = edit.error {
            // An inexact calculation names the two nearest values.
            if let Some(rules) = rules
                && let Err(e) = amount_entry::parse(&edit.buffer, rules)
            {
                return Some(crate::editors::calc::error_text(
                    &e,
                    env.money_style,
                    env.text,
                ));
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
                // NN4: an amount cell never shows a suggestion.
                debug_assert!(
                    kind.accepts_suggestions(),
                    "Suggested in a non-picker column"
                );
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
            CellValue::Tags(chips) => plain(
                chips
                    .iter()
                    .map(|c| c.label.as_str())
                    .collect::<Vec<_>>()
                    .join(" · "),
            ),
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

    fn paint_cell(
        painter: &egui::Painter,
        rect: Rect,
        col: &crate::core::model::Column,
        value: &CellValue<'_>,
        shown: &Shown,
        state: RowState,
        style: &GridStyle,
    ) {
        let color = if shown.placeholder {
            style.placeholder
        } else if state.contains(RowState::LOCKED) {
            style.locked_text
        } else if col.line == Line::Second {
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
                return;
            }
            CellValue::Mark(MarkKind::Cleared) => {
                check(
                    painter,
                    Rect::from_center_size(rect.center(), vec2(10.0, 10.0)),
                    style.text_secondary,
                );
                return;
            }
            CellValue::Mark(MarkKind::Reconciled) => {
                painter.text(
                    rect.center(),
                    Align2::CENTER_CENTER,
                    "R",
                    style.mono.clone(),
                    style.text_secondary,
                );
                return;
            }
            CellValue::Mark(MarkKind::None) | CellValue::Loading => return,
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
        }
        let galley = painter.layout_job(job);
        let x = match col.align {
            crate::core::model::Align::Right => inner.right() - galley.size().x,
            crate::core::model::Align::Center => inner.center().x - galley.size().x / 2.0,
            crate::core::model::Align::Left => inner.left(),
        };
        let y = rect.center().y - galley.size().y / 2.0;
        painter.galley(Pos2::new(x, y), galley, color);
    }
}

/// A drawn check mark (Plex has no check glyph, Q-P06h).
fn check(painter: &egui::Painter, r: Rect, color: Color32) {
    let a = pos2(r.left(), r.center().y);
    let b = pos2(r.left() + r.width() * 0.4, r.bottom());
    let c = pos2(r.right(), r.top());
    painter.line_segment([a, b], Stroke::new(1.8, color));
    painter.line_segment([b, c], Stroke::new(1.8, color));
}
