//! The ledger grid in the real shell (P10 plan §6, "U" tests): the demo
//! screen opened with `--demo` rules and the input sequencer installed.
//! Most helpers feed one input frame and then idle until the sequencer has
//! delivered everything (up to 22 frames); batched tests put several events
//! in one frame. `split_line_in_back_to_back_frames` enters the split line
//! with one input per frame and no idle frames in between.
//!
//! Values are read back from the AccessKit tree, the same place the control
//! channel reads them, and parsed into `Money` for exact comparison:
//! `Money::format` never rounds, so the displayed value is the model value.
//! The demo's Save refuses a check whose lines total differs from the check
//! amount by any amount, so a save that goes through is itself an exact
//! `Money` equality.

use cl_core::Money;
use cl_core::money::UserInput;
use cl_model::CommandId;
use cl_ui::shell::{Shell, ShellOptions};
use cl_ui::theme::ThemeChoice;
use eframe::egui::{
    self, Context, Event, Key, Modifiers, PointerButton, Pos2, RawInput, Rect, accesskit, vec2,
};

const SIZE: egui::Vec2 = vec2(1440.0, 940.0);

/// One AccessKit node, flattened.
#[derive(Debug, Clone)]
struct Node {
    id: accesskit::NodeId,
    role: accesskit::Role,
    label: String,
    value: Option<String>,
    bounds: Option<Rect>,
}

struct Rig {
    ctx: Context,
    shell: Shell,
    nodes: Vec<Node>,
    focus: Option<accesskit::NodeId>,
}

impl Rig {
    /// The shell with `--demo`, the grid demo in front.
    fn new() -> Rig {
        let ctx = Context::default();
        Shell::install(&ctx);
        cl_grid::sequencer::install(&ctx);
        ctx.enable_accesskit();
        let mut shell = Shell::new(ShellOptions {
            demo: true,
            theme: Some(ThemeChoice::Light),
            zoom_percent: Some(100),
            ..ShellOptions::default()
        });
        shell.set_run_only(true);
        shell.dispatch_now(CommandId::new("dev.grid_demo"));
        let mut rig = Rig {
            ctx,
            shell,
            nodes: Vec::new(),
            focus: None,
        };
        rig.idle();
        assert_eq!(rig.state()["mode"], "lines", "the demo is in front");
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
        if let Some(update) = out.platform_output.accesskit_update.take() {
            check_tree(&update);
            self.focus = Some(update.focus);
            self.nodes = flatten(&update);
        }
    }

    /// Frames until the sequencer has delivered everything, plus two for
    /// egui's popup sizing pass.
    fn idle(&mut self) {
        for _ in 0..20 {
            self.frame(vec![]);
            if !cl_grid::sequencer::pending(&self.ctx, egui::ViewportId::ROOT) {
                break;
            }
        }
        self.frame(vec![]);
        self.frame(vec![]);
    }

    /// Several events in one frame, then idle.
    fn batch(&mut self, events: Vec<Event>) {
        self.frame(events);
        self.idle();
    }

    fn text(&mut self, s: &str) {
        self.batch(vec![Event::Text(s.into())]);
    }

    fn key(&mut self, k: Key, m: Modifiers) {
        self.batch(tap(k, m));
    }

    fn tab(&mut self) {
        self.key(Key::Tab, Modifiers::NONE);
    }

    fn enter(&mut self) {
        self.key(Key::Enter, Modifiers::NONE);
    }

    fn click_at(&mut self, p: Pos2) {
        self.frame(vec![
            Event::PointerMoved(p),
            Event::PointerButton {
                pos: p,
                button: PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            },
        ]);
        self.frame(vec![Event::PointerButton {
            pos: p,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }]);
        self.idle();
    }

    /// Click the node with exactly this label (and this role, if given).
    fn click(&mut self, label: &str, role: Option<accesskit::Role>) {
        let p = self
            .nodes
            .iter()
            .find(|n| n.label == label && role.is_none_or(|r| n.role == r) && n.bounds.is_some())
            .and_then(|n| n.bounds)
            .unwrap_or_else(|| panic!("no `{label}` on screen"))
            .center();
        self.click_at(p);
    }

    fn click_cell(&mut self, column: &str, line: usize) {
        let label = format!("{column}, line {line}");
        let p = self
            .nodes
            .iter()
            .find(|n| n.label == label && is_cell(n.role) && n.bounds.is_some())
            .and_then(|n| n.bounds)
            .unwrap_or_else(|| panic!("no cell `{label}`"))
            .center();
        self.click_at(p);
    }

    /// The text field named by a visible label.
    fn click_field(&mut self, label: &str) {
        self.click(label, Some(accesskit::Role::TextInput));
    }

    /// The displayed value of a cell. The cursor cell is reported by its
    /// editor node (the grid's focus target), others by their cell node.
    fn cell(&self, column: &str, line: usize) -> String {
        let label = format!("{column}, line {line}");
        let matching: Vec<&Node> = self.nodes.iter().filter(|n| n.label == label).collect();
        let pick = matching
            .iter()
            .find(|n| n.role == accesskit::Role::TextInput || Some(n.id) == self.focus)
            .or_else(|| matching.iter().find(|n| is_cell(n.role)))
            .unwrap_or_else(|| panic!("no cell `{label}`"));
        pick.value.clone().unwrap_or_default()
    }

    fn money(&self, column: &str, line: usize) -> Option<Money> {
        let text = self.cell(column, line);
        (!text.trim().is_empty())
            .then(|| Money::parse_user(&text, UserInput::CENTS).expect("a displayed amount parses"))
    }

    /// The "Lines total" footer as `Money`.
    fn lines_total(&self) -> Money {
        let text = self
            .nodes
            .iter()
            .find_map(|n| {
                let s = n.value.as_deref().unwrap_or(&n.label);
                s.strip_prefix("Lines total ").map(str::to_owned)
            })
            .expect("the lines total is shown");
        Money::parse_user(&text, UserInput::CENTS).expect("the total parses")
    }

    fn shows(&self, needle: &str) -> bool {
        self.nodes.iter().any(|n| {
            n.label.contains(needle) || n.value.as_deref().is_some_and(|v| v.contains(needle))
        })
    }

    fn state(&self) -> serde_json::Value {
        self.shell.active_screen_state()
    }

    fn cursor(&self) -> (u64, u64) {
        let c = &self.state()["cursor"];
        (
            c["row"].as_u64().expect("a cursor row"),
            c["col"].as_u64().expect("a cursor column"),
        )
    }

    fn saved(&self) -> u64 {
        self.state()["saved_count"].as_u64().unwrap_or(0)
    }

    fn demo_open(&self) -> bool {
        self.state()["mode"].is_string()
    }

    /// Check amount, then click line 1's Account.
    fn start_check(&mut self, amount: &str) {
        self.click_field("Check amount");
        self.text(amount);
        self.click_cell("Account", 1);
    }

    /// The scenario's two lines: 5100 for 842.15 and Mulch for 400+44.25,
    /// entered with the keyboard only. The cursor ends on line 2's Memo.
    fn two_lines(&mut self) {
        self.start_check("1286.40");
        self.text("51");
        self.tab();
        self.text("842.15");
        self.enter();
        self.text("Mulch");
        self.tab();
        self.text("400+44.25");
        self.tab();
    }
}

fn is_cell(role: accesskit::Role) -> bool {
    matches!(
        role,
        accesskit::Role::Cell | accesskit::Role::GridCell | accesskit::Role::CheckBox
    )
}

/// What a platform adapter (`accesskit_consumer`) needs from every update:
/// each relation points at a node that is reachable from the root, or the
/// adapter panics inside the app (found by `ui-check grid_demo`).
fn check_tree(update: &accesskit::TreeUpdate) {
    let by_id: std::collections::HashMap<_, _> =
        update.nodes.iter().map(|(id, n)| (*id, n)).collect();
    let root = update.tree.as_ref().expect("a full tree").root;
    let mut reachable = std::collections::HashSet::new();
    let mut stack = vec![root];
    while let Some(id) = stack.pop() {
        if reachable.insert(id)
            && let Some(n) = by_id.get(&id)
        {
            stack.extend(n.children().iter().copied());
        }
    }
    for (id, n) in &update.nodes {
        if !reachable.contains(id) {
            continue;
        }
        for c in n.controls() {
            assert!(
                reachable.contains(c),
                "{:?} `{}` controls a node that is not in the tree (in the update: {:?})",
                n.role(),
                n.label().unwrap_or_default(),
                by_id.get(c).map(|x| x.role())
            );
        }
    }
    assert!(
        reachable.contains(&update.focus),
        "the focused node is not in the tree"
    );
}

fn flatten(update: &accesskit::TreeUpdate) -> Vec<Node> {
    let by_id: std::collections::HashMap<_, _> =
        update.nodes.iter().map(|(id, n)| (*id, n)).collect();
    update
        .nodes
        .iter()
        .map(|(id, n)| {
            let mut label = n.label().map(str::to_owned);
            if label.is_none() {
                // `labelled_by`: name the input after its visible label.
                label = n
                    .labelled_by()
                    .iter()
                    .filter_map(|l| by_id.get(l))
                    .find_map(|l| l.label().or(l.value()).map(str::to_owned));
            }
            #[allow(clippy::cast_possible_truncation)]
            let bounds = n.bounds().map(|r| {
                Rect::from_min_max(
                    Pos2::new(r.x0 as f32, r.y0 as f32),
                    Pos2::new(r.x1 as f32, r.y1 as f32),
                )
            });
            Node {
                id: *id,
                role: n.role(),
                label: label.unwrap_or_default(),
                value: n.value().map(str::to_owned),
                bounds,
            }
        })
        .collect()
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

fn tap(k: Key, m: Modifiers) -> Vec<Event> {
    vec![key_event(k, m, true), key_event(k, m, false)]
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

/// AC2 in consecutive frames: a split line with autofill, a calculator
/// amount and a tag, with exact values.
#[test]
fn keyboard_only_split_line_with_autofill_and_a_tag() {
    let mut rig = Rig::new();
    rig.start_check("1286.40");
    rig.text("51");
    assert!(rig.shows("5100"), "the account list opened on `51`");
    rig.tab();
    rig.text("842.15");
    rig.tab();
    rig.text("Perennials");
    rig.tab();
    rig.text("t");
    rig.tab();
    rig.text("Fall");
    rig.tab();
    rig.key(Key::Space, Modifiers::NONE);
    rig.tab();
    rig.text("Install");
    rig.enter();
    rig.enter();
    assert!(
        rig.cell("Account", 1).contains("5100"),
        "{}",
        rig.cell("Account", 1)
    );
    assert_eq!(rig.money("Amount", 1), Some(money("842.15")));
    assert_eq!(rig.cell("Memo", 1), "Perennials");
    assert!(rig.cell("Tags", 1).contains("Install"));

    rig.text("Mulch");
    rig.tab();
    rig.text("400+44.25");
    assert!(rig.shows("= 444.25"), "the calculator previews the result");
    rig.tab();
    assert_eq!(rig.money("Amount", 2), Some(money("444.25")));
    assert_eq!(rig.lines_total(), money("1286.40"));
    assert_eq!(rig.state()["rows"], 3, "two lines and the entry line");

    rig.click("Save", Some(accesskit::Role::Button));
    assert_eq!(rig.saved(), 1);
    assert_eq!(rig.state()["last_save_balanced"], true);
}

/// AC2 / T2 in the shell: the whole first line arrives in one frame (a
/// wedge scanner or a remote-desktop burst) and lands in the right cells.
#[test]
fn batched_frame_preserves_order_in_the_shell() {
    let mut rig = Rig::new();
    rig.start_check("1286.40");
    let mut events = vec![Event::Text("51".into())];
    for (text, k) in [
        ("", Key::Tab),
        ("842.15", Key::Tab),
        ("Perennials", Key::Tab),
        ("t", Key::Tab),
        ("Fall", Key::Tab),
    ] {
        if !text.is_empty() {
            events.push(Event::Text(text.into()));
        }
        events.extend(tap(k, Modifiers::NONE));
    }
    rig.batch(events);
    assert!(rig.cell("Account", 1).contains("5100"));
    assert_eq!(rig.money("Amount", 1), Some(money("842.15")));
    assert_eq!(rig.cell("Memo", 1), "Perennials");
    assert!(!rig.cell("Service date", 1).is_empty(), "`t` is today");
    assert!(rig.cell("Customer:Job", 1).contains("Fall"));
}

/// T3, first half: text and Alt+N in the same frame save the typed amount.
/// T7: Alt+N is a save path that commits the open cell.
#[test]
fn shortcut_after_text_same_frame_saves_the_typed_amount() {
    let mut rig = Rig::new();
    rig.start_check("1250");
    rig.text("6200");
    rig.tab();
    let mut events = vec![Event::Text("1250".into())];
    events.extend(tap(Key::N, Modifiers::ALT));
    rig.batch(events);
    // Saved only if the line equals the check amount exactly.
    assert_eq!(rig.saved(), 1, "{}", rig.state());
    assert_eq!(rig.state()["last_save_balanced"], true);
    assert_eq!(rig.state()["rows"], 1, "Save & New starts a fresh check");
}

/// T3 with Alt+S: the window closes only because the save went through.
#[test]
fn alt_s_in_the_same_frame_saves_and_closes() {
    let mut rig = Rig::new();
    rig.start_check("1250");
    rig.text("6200");
    rig.tab();
    let mut events = vec![Event::Text("1250".into())];
    events.extend(tap(Key::S, Modifiers::ALT));
    rig.batch(events);
    assert!(!rig.demo_open(), "Save & Close closed the demo");
}

/// Exactness the other way: one cent off is refused, and the cell shows
/// the amount that was typed.
#[test]
fn a_cent_off_is_refused_and_the_typed_amount_is_kept() {
    let mut rig = Rig::new();
    rig.start_check("1250.01");
    rig.text("6200");
    rig.tab();
    let mut events = vec![Event::Text("1250".into())];
    events.extend(tap(Key::N, Modifiers::ALT));
    rig.batch(events);
    assert_eq!(rig.saved(), 0);
    assert_eq!(rig.state()["last_save_balanced"], false);
    assert_eq!(rig.money("Amount", 1), Some(money("1250.00")));
}

/// T3, second half: text and Esc in the same frame put the cell back.
#[test]
fn text_then_esc_in_the_same_frame_reverts_the_cell() {
    let mut rig = Rig::new();
    rig.start_check("100");
    rig.text("6200");
    rig.tab();
    let before = rig.cursor();
    let mut events = vec![Event::Text("100".into())];
    events.extend(tap(Key::Escape, Modifiers::NONE));
    rig.batch(events);
    assert_eq!(rig.money("Amount", 1), None);
    assert_eq!(rig.cursor(), before, "the cursor stays on the cell");
    assert!(rig.demo_open(), "Esc did not close the window");
}

/// T4: Ctrl+Z after moving to another row resyncs the open editor; a Tab
/// then commits nothing (Ctrl+Y still redoes), and Ctrl+Y brings it back.
#[test]
fn undo_across_rows_resyncs_editor() {
    let mut rig = Rig::new();
    rig.two_lines();
    rig.key(Key::Z, CTRL);
    assert_eq!(rig.money("Amount", 2), None, "line 2's amount was undone");
    assert_eq!(rig.lines_total(), money("842.15"));
    rig.tab();
    rig.key(Key::Y, CTRL);
    assert_eq!(rig.money("Amount", 2), Some(money("444.25")));
    assert_eq!(rig.lines_total(), money("1286.40"));
}

/// T5: leaving and re-entering a cell makes two undo steps.
#[test]
fn reentered_cell_commits_are_separate_undo_steps() {
    let mut rig = Rig::new();
    rig.start_check("7");
    rig.text("6200");
    rig.tab();
    rig.text("5");
    rig.tab();
    rig.key(Key::Tab, Modifiers::SHIFT);
    rig.text("7");
    rig.tab();
    assert_eq!(rig.money("Amount", 1), Some(money("7.00")));
    rig.key(Key::Z, CTRL);
    assert_eq!(rig.money("Amount", 1), Some(money("5.00")));
    rig.key(Key::Z, CTRL);
    assert_eq!(rig.money("Amount", 1), None);
}

/// T6: Ctrl+Delete on line 2, then Ctrl+Z: the same line comes back, the
/// total is exact both ways and the cursor stays anchored on it.
#[test]
fn delete_line_then_undo() {
    let mut rig = Rig::new();
    rig.two_lines();
    rig.key(Key::ArrowUp, Modifiers::NONE);
    rig.key(Key::ArrowDown, Modifiers::NONE);
    let on_line_2 = rig.cursor();
    assert_eq!(on_line_2.0, 1);
    rig.key(Key::Delete, CTRL);
    assert_eq!(rig.lines_total(), money("842.15"));
    assert_eq!(rig.state()["rows"], 2);
    // The cursor moved to the next row, the entry line, now line 2.
    let on_entry = rig.cursor();
    assert_eq!(on_entry, (1, on_line_2.1));
    rig.key(Key::Z, CTRL);
    assert_eq!(rig.lines_total(), money("1286.40"));
    assert_eq!(rig.state()["rows"], 3);
    assert_eq!(rig.money("Amount", 2), Some(money("444.25")));
    assert!(rig.cell("Account", 2).contains("Mulch"));
    // Anchored by row key: still on the entry line, which moved down.
    assert_eq!(rig.cursor(), (2, on_line_2.1));
}

/// T7: an invalid buffer blocks the mouse Save and keeps the cell focused.
#[test]
fn an_invalid_buffer_blocks_save_and_keeps_the_cell() {
    let mut rig = Rig::new();
    rig.start_check("5.55");
    rig.text("6200");
    rig.tab();
    let amount_cell = rig.cursor();
    rig.text("5.555");
    rig.click("Save", Some(accesskit::Role::Button));
    assert_eq!(rig.saved(), 0);
    assert_eq!(rig.cursor(), amount_cell);
    assert_eq!(rig.state()["focused"], true, "the grid keeps focus");
    assert_eq!(rig.cell("Amount", 1), "5.555", "the text is kept");
}

/// T7: an open picker on "Mul" resolves on Save exactly as Tab would.
#[test]
fn an_open_picker_follows_the_tab_rule_on_save() {
    let by_tab = {
        let mut rig = Rig::new();
        rig.start_check("1");
        rig.text("Mul");
        assert_eq!(rig.state()["popup_open"], true);
        rig.tab();
        rig.cell("Account", 1)
    };
    let mut rig = Rig::new();
    rig.start_check("1");
    rig.text("Mul");
    rig.click("Save", Some(accesskit::Role::Button));
    assert_eq!(rig.cell("Account", 1), by_tab);
    assert_eq!(rig.state()["popup_open"], false);
}

/// S4: the two split lines typed with one input per frame, back to back,
/// with no idle frames between keys (a fast typist or the control channel).
#[test]
fn split_line_in_back_to_back_frames() {
    let mut rig = Rig::new();
    rig.start_check("1286.40");
    let inputs: Vec<Vec<Event>> = vec![
        vec![Event::Text("51".into())],
        tap(Key::Tab, Modifiers::NONE),
        vec![Event::Text("842.15".into())],
        tap(Key::Enter, Modifiers::NONE),
        vec![Event::Text("Mulch".into())],
        tap(Key::Tab, Modifiers::NONE),
        vec![Event::Text("400+44.25".into())],
        tap(Key::Tab, Modifiers::NONE),
    ];
    for events in inputs {
        rig.frame(events);
    }
    assert!(rig.cell("Account", 1).contains("5100"));
    assert!(rig.cell("Account", 2).contains("Mulch"));
    assert_eq!(rig.money("Amount", 1), Some(money("842.15")));
    assert_eq!(rig.money("Amount", 2), Some(money("444.25")));
    assert_eq!(rig.lines_total(), money("1286.40"));
    assert_eq!(rig.cursor().0, 1, "still on line 2 (its Memo)");
}

/// T20: a save keeps the rows: no "line is no longer here" and the cursor
/// stays put.
#[test]
fn rowkeys_survive_save_ok() {
    let mut rig = Rig::new();
    rig.two_lines();
    let before = rig.cursor();
    rig.click("Save", Some(accesskit::Role::Button));
    assert_eq!(rig.saved(), 1);
    assert!(!rig.shows("no longer here"));
    assert_eq!(rig.cursor(), before);
    assert_eq!(rig.state()["dirty"], false);
}

/// T23: Enter on the pristine entry line is Save & New.
#[test]
fn enter_on_the_empty_entry_line_saves_and_starts_new() {
    let mut rig = Rig::new();
    rig.two_lines();
    rig.enter();
    assert_eq!(rig.cursor().0, 2, "on the entry line");
    assert_eq!(rig.saved(), 0);
    rig.enter();
    assert_eq!(rig.saved(), 1, "{}", rig.state());
    assert_eq!(rig.state()["rows"], 1);
    // S1: the new record is not a vanished row.
    assert!(!rig.shows("no longer here"), "{}", rig.state());
    assert_eq!(
        rig.cursor(),
        (0, 1),
        "line 1's Account (the first editable cell)"
    );
}

/// The 200,000-row register: Ctrl+End reaches the last row exactly.
#[test]
fn register_ctrl_end_reaches_the_last_row() {
    let mut rig = Rig::new();
    rig.click("Show 200,000 rows", Some(accesskit::Role::Button));
    assert_eq!(rig.state()["rows"], 200_000);
    assert_eq!(rig.state()["focused"], true, "the switch focuses row 1");
    let row1 = rig
        .nodes
        .iter()
        .find(|n| n.role == accesskit::Role::Row && n.label.ends_with("row 1 of 200,000"))
        .and_then(|n| n.bounds)
        .expect("row 1 is drawn");
    rig.click_at(row1.center());
    assert_eq!(rig.state()["focused"], true);
    rig.key(Key::End, CTRL);
    assert!(rig.shows("row 200,000 of 200,000"), "the last row is drawn");
    assert_eq!(rig.cursor().0, 199_999);
}

/// Pay bills (fixed rows): the switch puts keyboard focus on row 1, and
/// Space selects the bill and fills the amount to pay with the amount due.
#[test]
fn pay_bills_space_selects_and_fills_the_amount() {
    let mut rig = Rig::new();
    rig.click("Pay bills example", Some(accesskit::Role::Button));
    assert_eq!(rig.state()["mode"], "bills");
    assert_eq!(rig.state()["focused"], true, "row 1 has keyboard focus");
    rig.key(Key::Space, Modifiers::NONE);
    assert!(rig.shows("1 selected"));
    let filled: Vec<(String, String)> = rig
        .nodes
        .iter()
        .filter_map(|n| {
            let row = n.label.strip_prefix("Amount to pay, ")?;
            let v = n.value.clone().filter(|v| !v.is_empty())?;
            Some((row.to_owned(), v))
        })
        .collect();
    assert_eq!(filled.len(), 1, "{filled:?}");
    let (row, value) = &filled[0];
    let to_pay = Some(value.clone());
    let amount_due = rig
        .nodes
        .iter()
        .find(|n| n.label == format!("Amount due, {row}"))
        .and_then(|n| n.value.clone());
    assert_eq!(to_pay, amount_due, "selecting fills the amount due exactly");
}
