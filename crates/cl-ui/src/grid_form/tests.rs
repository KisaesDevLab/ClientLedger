//! Every save, leave and record-move path commits the open cell first (B4,
//! plan §2.3), in a real shell with the input sequencer.

use std::borrow::Cow;
use std::cell::RefCell;
use std::rc::Rc;

use cl_core::Money;
use cl_grid::{
    Align, CellInput, CellKind, CellPos, CellProblem, CellValue, Column, ColumnId, EditableModel,
    Grid, GridHeight, GridSpec, Line, NoLookups, RecordOutcome, RowKey, RowKeyGen, RowModel,
    RowState, Source, TextKey, WidthSpec,
};
use cl_model::amount_entry::AmountRules;
use cl_model::forms::{FormState, LeaveDecision, SaveIntent, SaveStart};
use cl_model::{CommandId, Scope};
use eframe::egui::{self, Event, Key, Modifiers, Pos2, RawInput, Rect, Ui, vec2};

use super::{GridForm, LinesAdapter, Routed};
use crate::shell::{Shell, ShellOptions};
use crate::windows::{PromptAnswer, Screen, WindowId};

const AMOUNT: ColumnId = ColumnId(1);
const MEMO: ColumnId = ColumnId(2);

#[derive(Debug, Clone, PartialEq)]
struct Row {
    key: RowKey,
    amount: Option<Money>,
    memo: String,
}

impl Row {
    fn blank(key: RowKey) -> Row {
        Row {
            key,
            amount: None,
            memo: String::new(),
        }
    }
    fn is_blank(&self) -> bool {
        self.amount.is_none() && self.memo.is_empty()
    }
}

struct Adapter {
    keys: RowKeyGen,
    snapshot: Option<Row>,
}

struct View<'a> {
    form: &'a mut FormState<Vec<Row>>,
    adapter: &'a mut Adapter,
}

impl View<'_> {
    fn rows(&self) -> &[Row] {
        self.form.current()
    }
}

impl LinesAdapter<Vec<Row>> for Adapter {
    fn model<'a>(&'a mut self, form: &'a mut FormState<Vec<Row>>) -> Box<dyn EditableModel + 'a> {
        Box::new(View {
            form,
            adapter: self,
        })
    }
}

impl RowModel for View<'_> {
    fn revision(&self) -> u64 {
        self.form.revision()
    }
    fn row_count(&self) -> usize {
        self.rows().len()
    }
    fn row_key(&self, index: usize) -> Option<RowKey> {
        self.rows().get(index).map(|r| r.key)
    }
    fn index_of(&self, key: RowKey) -> Option<usize> {
        self.rows().iter().position(|r| r.key == key)
    }
    fn row_state(&self, index: usize) -> RowState {
        if index + 1 == self.rows().len() && self.rows()[index].is_blank() {
            RowState::ENTRY
        } else {
            RowState::empty()
        }
    }
    fn cell(&self, index: usize, col: ColumnId) -> CellValue<'_> {
        let Some(r) = self.rows().get(index) else {
            return CellValue::Empty;
        };
        match col {
            AMOUNT => r.amount.map_or(CellValue::Empty, CellValue::Money),
            MEMO => CellValue::Text(Cow::Borrowed(&r.memo)),
            _ => CellValue::Empty,
        }
    }
    fn row_label(&self, _index: usize) -> String {
        String::new()
    }
}

impl EditableModel for View<'_> {
    fn cell_editable(&self, _index: usize, _col: ColumnId) -> bool {
        true
    }
    fn row_entered(&mut self, key: RowKey) {
        self.adapter.snapshot = self.rows().iter().find(|r| r.key == key).cloned();
    }
    fn set_cell(
        &mut self,
        pos: CellPos,
        value: CellInput,
        _source: Source,
    ) -> Result<(), CellProblem> {
        let keys = &mut self.adapter.keys;
        let applied = self.form.edit_step(|rows| {
            if let Some(r) = rows.iter_mut().find(|r| r.key == pos.row) {
                match (pos.col, value) {
                    (AMOUNT, CellInput::Money(m)) => r.amount = Some(m),
                    (AMOUNT, CellInput::Clear) => r.amount = None,
                    (MEMO, CellInput::Text(t)) => r.memo = t,
                    (MEMO, CellInput::Clear) => r.memo.clear(),
                    _ => {}
                }
            }
            if rows.last().is_none_or(|r| !r.is_blank()) {
                rows.push(Row::blank(keys.next_key()));
            }
        });
        if applied {
            Ok(())
        } else {
            Err(CellProblem::new(TextKey("grid.form.saving")).at(pos.col))
        }
    }
    fn revert_row(&mut self, key: RowKey) -> bool {
        let Some(snap) = self.adapter.snapshot.clone().filter(|s| s.key == key) else {
            return false;
        };
        if self.rows().contains(&snap) {
            return false;
        }
        self.form.edit_step(|rows| {
            if let Some(r) = rows.iter_mut().find(|r| r.key == key) {
                *r = snap;
            }
        })
    }
    fn insert_line(&mut self, before: Option<RowKey>) -> Option<RowKey> {
        let key = self.adapter.keys.next_key();
        self.form
            .edit_step(|rows| {
                let at = before
                    .and_then(|b| rows.iter().position(|r| r.key == b))
                    .unwrap_or(rows.len());
                rows.insert(at, Row::blank(key));
            })
            .then_some(key)
    }
    fn delete_line(&mut self, key: RowKey) -> bool {
        self.form.edit_step(|rows| rows.retain(|r| r.key != key))
    }
    fn record_row(&mut self, key: RowKey) -> RecordOutcome {
        let i = self.index_of(key).unwrap_or(0);
        RecordOutcome::Recorded {
            next: self.row_key(i + 1),
        }
    }
}

fn column(id: ColumnId, kind: CellKind) -> Column {
    Column {
        id,
        header: TextKey("grid.a11y.line"),
        placeholder: None,
        help: None,
        kind,
        line: Line::First,
        align: Align::Left,
        width: WidthSpec {
            min: 60,
            preferred: 140,
            flex: 1,
        },
        editable: true,
        mono: false,
    }
}

type Form = GridForm<Vec<Row>, Adapter>;

fn grid_form() -> Form {
    grid_form_with(2)
}

fn grid_form_with(n: usize) -> Form {
    let mut keys = RowKeyGen::new();
    let rows: Vec<Row> = (0..n).map(|_| Row::blank(keys.next_key())).collect();
    let spec = GridSpec::line_entry(vec![
        column(AMOUNT, CellKind::Amount(AmountRules::MONEY)),
        column(MEMO, CellKind::Text { max_chars: 80 }),
    ]);
    let grid = Grid::new("grid-form-test", spec).expect("spec");
    GridForm::new(
        FormState::new(rows),
        Adapter {
            keys,
            snapshot: None,
        },
        grid,
        "Test lines",
        GridHeight::Rows { min: 3, max: 6 },
    )
    .with_widths_key("test/lines")
}

#[derive(Default)]
struct Probe {
    saved: Vec<Vec<Row>>,
    left_with: Option<(LeaveDecision, Vec<Row>)>,
    blocked: u32,
    save_rect: Option<Rect>,
    next_rect: Option<Rect>,
    /// Ask the screen to focus line 1's Amount on its next frame.
    refocus: bool,
    /// The grid's positions after the last frame.
    snap: cl_grid::GridSnapshot,
    /// The form revision after the last frame.
    revision: u64,
    /// Lines in the form after the last frame.
    rows: usize,
    /// Keep saves in flight (an engine that has not answered yet).
    hold: bool,
    /// The value of the save in flight.
    held: Option<Vec<Row>>,
    /// Finish the held save on the next frame.
    release: bool,
}

struct TestScreen {
    form: Form,
    probe: Rc<RefCell<Probe>>,
    scope: Scope,
}

impl TestScreen {
    fn finish(&mut self, start: SaveStart<Vec<Row>>) -> bool {
        match start {
            SaveStart::Send(v) if self.probe.borrow().hold => {
                self.probe.borrow_mut().held = Some(v);
                true
            }
            SaveStart::Send(v) => {
                self.probe.borrow_mut().saved.push(v.clone());
                self.form.save_ok(v, &NoLookups);
                true
            }
            SaveStart::Skip(_) => true,
            SaveStart::Busy => false,
        }
    }

    fn next_record(&mut self) {
        match self.form.leave(&NoLookups) {
            Ok(d) => {
                self.probe.borrow_mut().left_with = Some((d, self.form.form().current().clone()));
            }
            Err(_) => self.probe.borrow_mut().blocked += 1,
        }
    }
}

impl Screen for TestScreen {
    fn title(&self) -> String {
        "Lines".into()
    }
    fn ui(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            let save = ui.button("Save");
            let next = ui.button("Next");
            self.probe.borrow_mut().save_rect = Some(save.rect);
            self.probe.borrow_mut().next_rect = Some(next.rect);
            if save.clicked() {
                match self.form.begin_save(SaveIntent::Save, &NoLookups) {
                    Ok(s) => {
                        self.finish(s);
                    }
                    Err(_) => self.probe.borrow_mut().blocked += 1,
                }
            }
            if next.clicked() {
                self.next_record();
            }
        });
        if std::mem::take(&mut self.probe.borrow_mut().release) {
            let held = self.probe.borrow_mut().held.take();
            if let Some(v) = held {
                self.probe.borrow_mut().saved.push(v.clone());
                self.form.save_ok(v, &NoLookups);
            }
        }
        if std::mem::take(&mut self.probe.borrow_mut().refocus) {
            let first = self.form.form().current()[0].key;
            self.form.focus(
                CellPos {
                    row: first,
                    col: AMOUNT,
                },
                &NoLookups,
            );
        }
        self.form.ui(ui, &NoLookups);
        let mut p = self.probe.borrow_mut();
        p.snap = self.form.snapshot();
        p.revision = self.form.form().revision();
        p.rows = self.form.form().current().len();
    }
    fn dirty(&self) -> bool {
        self.form.dirty()
    }
    fn scope(&self) -> Scope {
        self.scope
    }
    fn command(&mut self, id: CommandId) -> bool {
        match self.form.command(id, &NoLookups) {
            Routed::NotHandled => false,
            Routed::Handled => true,
            Routed::Save(s) => {
                self.finish(s);
                true
            }
            Routed::Navigate { decision, .. } => {
                self.probe.borrow_mut().left_with =
                    Some((decision, self.form.form().current().clone()));
                true
            }
            Routed::Blocked(_) => {
                self.probe.borrow_mut().blocked += 1;
                true
            }
        }
    }
    fn save(&mut self) -> bool {
        if let Ok(s) = self.form.begin_save(SaveIntent::SaveAndClose, &NoLookups) {
            self.finish(s)
        } else {
            self.probe.borrow_mut().blocked += 1;
            false
        }
    }
}

struct Rig {
    ctx: egui::Context,
    shell: Shell,
    probe: Rc<RefCell<Probe>>,
    window: WindowId,
}

const SIZE: egui::Vec2 = vec2(1200.0, 800.0);

impl Rig {
    fn new() -> Rig {
        Rig::with(2, Scope::Form)
    }

    fn with(rows: usize, scope: Scope) -> Rig {
        let ctx = egui::Context::default();
        Shell::install(&ctx);
        cl_grid::sequencer::install(&ctx);
        let mut shell = Shell::new(ShellOptions::default());
        let probe = Rc::new(RefCell::new(Probe::default()));
        let mut form = grid_form_with(rows);
        let first = form.form().current()[0].key;
        form.focus(
            CellPos {
                row: first,
                col: AMOUNT,
            },
            &NoLookups,
        );
        let window = shell.windows.open_new(
            CommandId::new("vendors.write_check"),
            Box::new(TestScreen {
                form,
                probe: Rc::clone(&probe),
                scope,
            }),
        );
        let mut rig = Rig {
            ctx,
            shell,
            probe,
            window,
        };
        rig.idle();
        rig
    }

    fn frame(&mut self, events: Vec<Event>) {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SIZE)),
            events,
            ..RawInput::default()
        };
        let shell = &mut self.shell;
        let mut out = self.ctx.run_ui(input, |ui| shell.ui(ui));
        out.textures_delta.clear();
    }

    fn idle(&mut self) {
        for _ in 0..20 {
            self.frame(vec![]);
            if !cl_grid::sequencer::pending(&self.ctx, egui::ViewportId::ROOT)
                && !cl_grid::sequencer::pending(&self.ctx, self.popout_viewport())
            {
                break;
            }
        }
    }

    fn popout_viewport(&self) -> egui::ViewportId {
        egui::ViewportId::from_hash_of(("cl-window", self.window.raw()))
    }

    /// Pop the test window out into its own (test-rendered) viewport.
    fn pop_out(&mut self) {
        self.ctx.set_embed_viewports(false);
        egui::Context::set_immediate_viewport_renderer(|ctx, vp| {
            let events = POPOUT_EVENTS.with(|e| std::mem::take(&mut *e.borrow_mut()));
            let input = RawInput {
                viewport_id: vp.ids.this,
                viewports: std::iter::once((vp.ids.this, egui::ViewportInfo::default())).collect(),
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(900.0, 640.0))),
                events,
                ..RawInput::default()
            };
            let mut cb = vp.viewport_ui_cb;
            let mut out = ctx.run_ui(input, |ui| cb(ui));
            out.textures_delta.clear();
            POPOUT_PASSES.with(|n| *n.borrow_mut() += 1);
        });
        self.shell.windows.pop_out(self.window);
        self.probe.borrow_mut().refocus = true;
        self.idle();
    }

    /// One main-window frame with these events for the popped-out window.
    fn pop_frame(&mut self, events: Vec<Event>) {
        POPOUT_EVENTS.with(|e| e.borrow_mut().extend(events));
        self.frame(vec![]);
        self.idle();
    }

    fn type_text(&mut self, s: &str) {
        self.frame(vec![Event::Text(s.into())]);
        self.idle();
    }

    fn click(&mut self, rect: Option<Rect>) {
        let p = rect.expect("drawn").center();
        self.frame(vec![
            Event::PointerMoved(p),
            Event::PointerButton {
                pos: p,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            },
        ]);
        self.frame(vec![Event::PointerButton {
            pos: p,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }]);
        self.idle();
    }

    fn dirty(&self) -> bool {
        self.shell
            .windows
            .get(self.window)
            .is_some_and(crate::windows::Window::dirty)
    }

    fn last_saved_amount(&self) -> Option<Money> {
        self.probe.borrow().saved.last().and_then(|v| v[0].amount)
    }
}

thread_local! {
    static POPOUT_EVENTS: RefCell<Vec<Event>> = const { RefCell::new(Vec::new()) };
    static POPOUT_PASSES: RefCell<u32> = const { RefCell::new(0) };
}

fn tap(k: Key, modifiers: Modifiers) -> Vec<Event> {
    vec![
        key_event(k, modifiers, true),
        key_event(k, modifiers, false),
    ]
}

fn repeat(k: Key) -> Event {
    Event::Key {
        key: k,
        physical_key: None,
        pressed: true,
        repeat: true,
        modifiers: Modifiers::NONE,
    }
}

const CTRL: Modifiers = Modifiers {
    alt: false,
    ctrl: true,
    shift: false,
    mac_cmd: false,
    command: true,
};

fn money(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}

fn key_event(k: Key, modifiers: Modifiers, pressed: bool) -> Event {
    Event::Key {
        key: k,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers,
    }
}

#[test]
fn an_open_cell_makes_the_form_dirty() {
    let mut rig = Rig::new();
    assert!(!rig.dirty());
    rig.type_text("1250");
    assert!(rig.dirty(), "typed text not committed yet is unsaved");
    assert_eq!(rig.probe.borrow().saved.len(), 0);
}

#[test]
fn alt_s_in_the_same_frame_as_the_text_commits_first() {
    let mut rig = Rig::new();
    rig.frame(vec![
        Event::Text("1250".into()),
        key_event(Key::S, Modifiers::ALT, true),
        key_event(Key::S, Modifiers::ALT, false),
    ]);
    rig.idle();
    assert_eq!(rig.last_saved_amount(), Some(money("1250.00")));
}

#[test]
fn mouse_save_commits_first() {
    let mut rig = Rig::new();
    rig.type_text("842.15");
    let r = rig.probe.borrow().save_rect;
    rig.click(r);
    assert_eq!(rig.last_saved_amount(), Some(money("842.15")));
    assert!(!rig.dirty());
}

/// S2: typing and Tab while a save is in flight never changes the lines
/// behind the owner's back: the grid takes no input until the save ends,
/// then gets focus back.
#[test]
fn typing_while_a_save_is_in_flight_is_not_lost() {
    let mut rig = Rig::new();
    rig.probe.borrow_mut().hold = true;
    rig.type_text("12");
    let r = rig.probe.borrow().save_rect;
    rig.click(r);
    assert!(rig.probe.borrow().held.is_some(), "the save is in flight");
    let before = rig.probe.borrow().revision;
    rig.type_text("34");
    rig.frame(tap(Key::Tab, Modifiers::NONE));
    rig.idle();
    assert_eq!(rig.probe.borrow().revision, before, "nothing changed");
    assert!(
        !rig.probe.borrow().snap.focused,
        "no grid input while saving"
    );
    rig.probe.borrow_mut().release = true;
    rig.idle();
    assert_eq!(rig.last_saved_amount(), Some(money("12.00")));
    assert!(rig.probe.borrow().snap.focused, "focus comes back");
    assert!(!rig.dirty());
}

/// S2: the model contract: a commit or line edit during a save is
/// refused, never silently dropped.
#[test]
fn model_refuses_edits_while_saving() {
    let mut form = grid_form();
    let key = form.form.current()[0].key;
    form.form
        .edit_step(|rows| rows[0].amount = Some(money("1.00")));
    let _ = form.form.begin_save(SaveIntent::Save);
    assert!(form.form.is_saving());
    let mut model = form.adapter.model(&mut form.form);
    let pos = CellPos {
        row: key,
        col: MEMO,
    };
    let got = model.set_cell(pos, CellInput::Text("x".into()), Source::Typed);
    assert_eq!(got.map_err(|p| p.key), Err(TextKey("grid.form.saving")));
    assert_eq!(model.insert_line(None), None);
    assert!(!model.delete_line(key));
}

#[test]
fn mouse_save_with_an_invalid_cell_is_blocked() {
    let mut rig = Rig::new();
    rig.type_text("5.5.5");
    let r = rig.probe.borrow().save_rect;
    rig.click(r);
    assert_eq!(rig.probe.borrow().saved.len(), 0);
    assert_eq!(rig.probe.borrow().blocked, 1);
    assert!(rig.dirty());
}

#[test]
fn close_prompt_save_commits_first() {
    let mut rig = Rig::new();
    rig.type_text("1286.40");
    let id = rig.window;
    assert_eq!(
        rig.shell.close_window(id),
        crate::windows::CloseOutcome::Prompt
    );
    // Answered before another frame runs: the cell is still open.
    rig.shell.windows.answer_prompt(PromptAnswer::Save);
    assert_eq!(rig.last_saved_amount(), Some(money("1286.40")));
    assert!(rig.shell.windows.get(id).is_none(), "saved, so it closed");
}

#[test]
fn typed_amount_with_extra_decimals_rounds_to_the_cent() {
    // Q-232: no refusal and no hint; half away from zero.
    let mut rig = Rig::new();
    rig.type_text("5.555");
    let id = rig.window;
    rig.shell.close_window(id);
    rig.shell.windows.answer_prompt(PromptAnswer::Save);
    assert_eq!(rig.last_saved_amount(), Some(money("5.56")));
}

#[test]
fn close_prompt_save_with_an_invalid_cell_keeps_the_window() {
    let mut rig = Rig::new();
    rig.type_text("5.5.5");
    let id = rig.window;
    rig.shell.close_window(id);
    rig.shell.windows.answer_prompt(PromptAnswer::Save);
    assert_eq!(rig.probe.borrow().saved.len(), 0);
    assert!(rig.shell.windows.get(id).is_some());
}

#[test]
fn quit_prompt_save_commits_first() {
    let mut rig = Rig::new();
    rig.type_text("444.25");
    rig.shell.dispatch_now(CommandId::new("file.quit"));
    assert!(rig.shell.windows.prompt().is_some_and(|p| p.quitting));
    let progress = rig.shell.windows.answer_prompt(PromptAnswer::Save);
    assert_eq!(progress, Some(crate::windows::QuitProgress::Ready));
    assert_eq!(rig.last_saved_amount(), Some(money("444.25")));
}

#[test]
fn next_record_button_commits_first() {
    let mut rig = Rig::new();
    rig.type_text("19.99");
    let r = rig.probe.borrow().next_rect;
    rig.click(r);
    let left = rig.probe.borrow().left_with.clone().expect("left");
    assert_eq!(left.0, LeaveDecision::Prompt, "dirty: the screen asks");
    assert_eq!(left.1[0].amount, Some(money("19.99")));
}

#[test]
fn next_record_command_off_the_grid_commits_or_blocks() {
    let mut rig = Rig::new();
    rig.type_text("5.5.5");
    // Leave the grid: the invalid text is parked, still uncommitted.
    let r = rig.probe.borrow().next_rect;
    rig.click(r);
    assert_eq!(rig.probe.borrow().blocked, 1, "the button was blocked");
    assert!(rig.dirty());
    let w = rig.shell.windows.get_mut(rig.window).expect("window");
    assert!(w.screen.command(CommandId::new("form.next_record")));
    assert_eq!(rig.probe.borrow().blocked, 2, "the command was blocked");
    assert!(rig.probe.borrow().left_with.is_none());
}

#[test]
fn page_keys_page_the_grid_while_it_has_focus() {
    let mut rig = Rig::new();
    let w = rig.shell.windows.get_mut(rig.window).expect("window");
    assert!(w.screen.command(CommandId::new("form.next_record")));
    assert!(
        rig.probe.borrow().left_with.is_none(),
        "no record move while the grid has focus"
    );
}

#[test]
fn undo_falls_back_to_the_form() {
    let mut f = grid_form();
    let first = f.form().current()[0].key;
    f.edit_step(|rows| rows[0].memo = "Mulch".into());
    assert!(f.form().is_dirty());
    assert_eq!(
        f.command(CommandId::new("edit.undo"), &NoLookups),
        Routed::Handled
    );
    assert_eq!(f.form().current()[0].memo, "");
    assert_eq!(f.form().current()[0].key, first);
    assert_eq!(
        f.command(CommandId::new("edit.redo"), &NoLookups),
        Routed::Handled
    );
    assert_eq!(f.form().current()[0].memo, "Mulch");
    assert_eq!(
        f.command(CommandId::new("edit.find"), &NoLookups),
        Routed::NotHandled
    );
}

/// No screen that uses an editable grid holds its own `FormState` save path: they
/// all go through `GridForm::begin_save` (plan §2.3).
#[test]
fn grid_screens_never_call_form_state_begin_save() {
    fn files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                files(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/screens");
    let mut all = Vec::new();
    files(&root, &mut all);
    // A screen's folder is its first directory under `screens/` (a top-level
    // file is its own screen). An `EditableModel` impl anywhere in the folder
    // (say a `model.rs` beside the screen) makes every file in it a grid user.
    let screen_of = |p: &std::path::Path| -> std::path::PathBuf {
        let rel = p.strip_prefix(&root).unwrap();
        root.join(rel.components().next().unwrap())
    };
    let texts: Vec<_> = all
        .iter()
        .map(|f| (f.clone(), std::fs::read_to_string(f).unwrap()))
        .collect();
    let editable_screens: std::collections::HashSet<_> = texts
        .iter()
        .filter(|(_, t)| implements_editable_model(t))
        .map(|(f, _)| screen_of(f))
        .collect();
    let mut bad = Vec::new();
    for (f, text) in &texts {
        let folder_edits = editable_screens.contains(&screen_of(f));
        if let Some(why) = direct_save(text, folder_edits) {
            bad.push(format!("{}: {why}", f.display()));
        }
    }
    assert!(bad.is_empty(), "{bad:#?}");

    // The check itself catches the mistake.
    let wrong = "use cl_grid::EditableModel;\nfn s(&mut self) { let _ = self.state.begin_save(SaveIntent::Save); }";
    assert!(direct_save(wrong, false).is_some());
    let right = "use cl_grid::EditableModel;\nfn s(&mut self) { let _ = self.form.begin_save(SaveIntent::Save, &self.lookups); }";
    assert_eq!(direct_save(right, false), None);
    let no_grid = "fn s(&mut self) { let _ = self.state.begin_save(SaveIntent::Save); }";
    assert_eq!(direct_save(no_grid, false), None);
    let browse = "use cl_grid::Grid;\nfn s(&mut self) { show(ModelRef::Browse(m)); let _ = self.state.begin_save(SaveIntent::Save); }";
    assert_eq!(direct_save(browse, false), None);
    // The model lives in a sibling file of the same screen folder.
    assert!(direct_save(no_grid, true).is_some());
    assert!(implements_editable_model(
        "impl cl_grid::EditableModel for Lines {}"
    ));
    assert!(!implements_editable_model("use cl_grid::EditableModel;"));
    // `ModelRef` imported and its `Edit` variant used unqualified.
    let bare = "use cl_grid::ModelRef::{self, Edit};\nfn s(&mut self) { show(Edit(m)); let _ = self.state.begin_save(SaveIntent::Save); }";
    assert!(direct_save(bare, false).is_some());
    let other_edit = "fn s(&mut self) { let _ = Mode::Edit(1); let _ = self.state.begin_save(SaveIntent::Save); }";
    assert_eq!(direct_save(other_edit, false), None);
}

/// Whether `source` implements `EditableModel` for some type.
fn implements_editable_model(source: &str) -> bool {
    let code = source.split("#[cfg(test)]").next().unwrap_or("");
    code.contains("EditableModel for")
}

/// Whether `code` calls `Edit(` unqualified (not `X::Edit(`, `.Edit(` or
/// part of a longer name).
fn bare_edit_call(code: &str) -> bool {
    code.match_indices("Edit(").any(|(at, _)| {
        code[..at]
            .chars()
            .next_back()
            .is_none_or(|c| !(c.is_alphanumeric() || c == '_' || c == ':' || c == '.'))
    })
}

/// Why `source` (one screen file) saves around its grid, if it does: the
/// file uses a grid and calls a one-argument `begin_save` (that is
/// `FormState::begin_save`; `GridForm::begin_save` also takes the lookups).
/// `folder_edits` says another file of the same screen implements
/// `EditableModel`.
fn direct_save(source: &str, folder_edits: bool) -> Option<String> {
    let code = source.split("#[cfg(test)]").next().unwrap_or("");
    // Only editable grids hold uncommitted cell edits; a browse-only grid
    // beside a plain field form (the P11 lists) saves through `FormState`.
    let uses_grid = folder_edits
        || code.contains("GridForm")
        || code.contains("ModelRef::Edit")
        || code.contains("EditableModel")
        || (code.contains("ModelRef") && bare_edit_call(code));
    if !uses_grid {
        return None;
    }
    if code.contains("FormState::begin_save") {
        return Some("names FormState::begin_save".into());
    }
    let mut rest = code;
    while let Some(at) = rest.find("begin_save(") {
        let before = &rest[..at];
        rest = &rest[at + "begin_save(".len()..];
        if before.trim_end().ends_with("fn") {
            continue;
        }
        let mut depth = 0usize;
        let mut top_level_comma = false;
        for c in rest.chars() {
            match c {
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' if depth == 0 => break,
                ')' | ']' | '}' => depth -= 1,
                ',' if depth == 0 => top_level_comma = true,
                _ => {}
            }
        }
        if !top_level_comma {
            return Some("calls a one-argument begin_save".into());
        }
    }
    None
}

/// T9 (B6, A4): with the close prompt open, keys never reach the grid;
/// Esc answers Keep and the grid takes focus back.
#[test]
fn modal_blocks_grid_input() {
    let mut rig = Rig::new();
    rig.type_text("12");
    rig.frame(tap(Key::Enter, Modifiers::NONE));
    rig.idle();
    assert_eq!(
        rig.probe.borrow().snap.cursor_row,
        Some(1),
        "on the entry line"
    );
    assert!(rig.dirty());
    // Esc on the pristine entry line: nothing to revert, so the window
    // asks before closing.
    rig.frame(tap(Key::Escape, Modifiers::NONE));
    rig.idle();
    assert!(rig.shell.windows.prompt().is_some(), "the prompt is open");
    let before = (rig.probe.borrow().revision, rig.probe.borrow().snap);
    assert!(!before.1.focused, "the grid gave up focus");
    for events in [
        vec![Event::Text("y".into())],
        tap(Key::Tab, Modifiers::NONE),
        tap(Key::ArrowDown, Modifiers::NONE),
        vec![Event::Text("9".into())],
    ] {
        rig.frame(events);
        rig.idle();
        let p = rig.probe.borrow();
        assert_eq!(p.revision, before.0, "the model is unchanged");
        assert_eq!(p.snap.cursor_row, before.1.cursor_row);
        assert!(!p.snap.focused && !p.snap.uncommitted);
        drop(p);
        assert!(rig.shell.windows.prompt().is_some(), "still asking");
    }
    rig.frame(tap(Key::Escape, Modifiers::NONE));
    rig.idle();
    assert!(rig.shell.windows.prompt().is_none(), "Esc answers Keep");
    assert!(rig.shell.windows.get(rig.window).is_some());
    assert!(rig.probe.borrow().snap.focused, "the grid has focus back");
    assert_eq!(rig.probe.borrow().revision, before.0);
    assert_eq!(rig.probe.borrow().saved.len(), 0);
}

/// T9: the prompt's Save button has focus when the prompt opens, so Enter
/// saves.
#[test]
fn close_prompt_focuses_save() {
    let mut rig = Rig::new();
    rig.type_text("12");
    rig.frame(tap(Key::Enter, Modifiers::NONE));
    rig.idle();
    rig.frame(tap(Key::Escape, Modifiers::NONE));
    rig.idle();
    assert!(rig.shell.windows.prompt().is_some());
    rig.frame(tap(Key::Enter, Modifiers::NONE));
    rig.idle();
    assert_eq!(rig.last_saved_amount(), Some(money("12.00")));
    assert!(
        rig.shell.windows.get(rig.window).is_none(),
        "saved and closed"
    );
}

/// T13 (S3), in Form and Window scope: Page Down pages the grid, its
/// repeats (the key held down) keep paging, and Esc repeats are dropped.
#[test]
fn pagedown_repeat_pages_and_esc_repeat_ignored() {
    for scope in [Scope::Form, Scope::Window] {
        let mut rig = Rig::with(60, scope);
        assert_eq!(rig.probe.borrow().snap.top_row, 0);
        rig.frame(vec![key_event(Key::PageDown, Modifiers::NONE, true)]);
        rig.idle();
        let first = rig.probe.borrow().snap;
        assert!(first.top_row > 0, "{scope:?}: the first press pages");
        assert!(rig.probe.borrow().left_with.is_none(), "no record move");
        // Still held: egui reports the next press as a repeat.
        rig.frame(vec![repeat(Key::PageDown)]);
        rig.idle();
        let second = rig.probe.borrow().snap;
        assert!(second.top_row > first.top_row, "{scope:?}: a repeat pages");
        assert!(second.cursor_row > first.cursor_row);
        rig.frame(vec![key_event(Key::PageDown, Modifiers::NONE, false)]);
        rig.idle();

        // Esc held down: the first press reverts the open cell, the
        // repeats do nothing (a second Esc would close the window).
        rig.type_text("5");
        assert!(rig.dirty());
        rig.frame(vec![key_event(Key::Escape, Modifiers::NONE, true)]);
        rig.idle();
        if scope == Scope::Form {
            assert!(!rig.dirty(), "the first Esc reverted the cell");
        }
        let rev = rig.probe.borrow().revision;
        rig.frame(vec![repeat(Key::Escape)]);
        rig.frame(vec![repeat(Key::Escape)]);
        rig.idle();
        rig.frame(vec![key_event(Key::Escape, Modifiers::NONE, false)]);
        rig.idle();
        assert!(rig.shell.windows.prompt().is_none(), "{scope:?}");
        assert!(rig.shell.windows.get(rig.window).is_some(), "{scope:?}");
        assert_eq!(rig.probe.borrow().revision, rev);
        assert!(rig.probe.borrow().snap.focused, "{scope:?}");
    }
}

/// T17 (S2, A18): a popped-out window handles its own keys even when
/// another tab is in front of the main window.
#[test]
fn popout_grid_receives_commands() {
    let mut rig = Rig::with(3, Scope::Form);
    rig.pop_out();
    rig.shell.dispatch_now(CommandId::new("nav.home"));
    assert_ne!(rig.shell.active_window(), Some(rig.window));
    rig.idle();
    assert!(
        POPOUT_PASSES.with(|n| *n.borrow()) > 0,
        "drawn in its viewport"
    );
    assert!(rig.probe.borrow().snap.focused, "the grid has focus there");

    // Esc reverts the open cell.
    rig.pop_frame(vec![Event::Text("5".into())]);
    assert!(rig.dirty());
    rig.pop_frame(tap(Key::Escape, Modifiers::NONE));
    assert!(!rig.dirty(), "Esc reverted the cell");
    assert!(rig.shell.windows.prompt().is_none());

    // Ctrl+Delete deletes the line.
    assert_eq!(rig.probe.borrow().rows, 3);
    rig.pop_frame(tap(Key::Delete, CTRL));
    assert_eq!(rig.probe.borrow().rows, 2, "Ctrl+Delete deleted line 1");

    // Keys in the main window go to the tab in front there, not here.
    let mut main = vec![Event::Text("1".into())];
    main.extend(tap(Key::S, Modifiers::ALT));
    rig.frame(main);
    rig.idle();
    assert_eq!(rig.probe.borrow().saved.len(), 0);

    // Alt+S saves, committing the open cell first.
    let mut pop = vec![Event::Text("7".into())];
    pop.extend(tap(Key::S, Modifiers::ALT));
    rig.pop_frame(pop);
    let saved = rig.probe.borrow().saved.last().cloned().expect("saved");
    assert_eq!(saved[0].amount, Some(money("7.00")));
    assert_eq!(saved.len(), 2);
}
