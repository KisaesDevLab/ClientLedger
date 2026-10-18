//! The grid demo in process: exact amounts end to end, the register and
//! Pay bills modes, and a state with no values in it.

use cl_core::{CalendarDate, Money};
use cl_grid::CellPos;
use eframe::egui::{self, Event, Key, Modifiers, Pos2, RawInput, Rect, vec2};

use super::{GridDemo, Mode};
use crate::screens::dev::lines::{ACCOUNT, Pick};
use crate::shell::Shell;
use crate::windows::Screen;

struct Rig {
    ctx: egui::Context,
    demo: GridDemo,
}

impl Rig {
    fn new() -> Rig {
        let ctx = egui::Context::default();
        Shell::install(&ctx);
        cl_grid::sequencer::install(&ctx);
        let today = CalendarDate::from_ymd(2026, 11, 4).unwrap();
        let mut rig = Rig {
            ctx,
            demo: GridDemo::new(today, None),
        };
        rig.idle();
        rig
    }

    fn frame(&mut self, events: Vec<Event>) {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1440.0, 940.0))),
            events,
            ..RawInput::default()
        };
        let demo = &mut self.demo;
        let mut out = self.ctx.run_ui(input, |ui| {
            egui::CentralPanel::default().show(ui, |ui| demo.ui(ui));
        });
        out.textures_delta.clear();
    }

    /// The open editor's or cell's AccessKit node with this label, after
    /// one more frame: (value, description).
    fn node(&mut self, label: &str) -> Option<(String, String)> {
        self.ctx.enable_accesskit();
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1440.0, 940.0))),
            ..RawInput::default()
        };
        let demo = &mut self.demo;
        let mut out = self.ctx.run_ui(input, |ui| {
            egui::CentralPanel::default().show(ui, |ui| demo.ui(ui));
        });
        out.textures_delta.clear();
        let update = out.platform_output.accesskit_update?;
        let mut found: Vec<_> = update
            .nodes
            .iter()
            .filter(|(_, n)| n.label() == Some(label))
            .map(|(_, n)| {
                (
                    n.role() == egui::accesskit::Role::TextInput,
                    n.value().unwrap_or_default().to_owned(),
                    n.description().unwrap_or_default().to_owned(),
                )
            })
            .collect();
        // The editor first.
        found.sort_by_key(|(editor, _, _)| !editor);
        found.into_iter().next().map(|(_, v, d)| (v, d))
    }

    fn idle(&mut self) {
        for _ in 0..20 {
            self.frame(vec![]);
            if !cl_grid::sequencer::pending(&self.ctx, egui::ViewportId::ROOT) {
                break;
            }
        }
    }

    fn text(&mut self, s: &str) {
        self.frame(vec![Event::Text(s.into())]);
        self.idle();
    }

    fn key(&mut self, k: Key) {
        self.frame(vec![
            Event::Key {
                key: k,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::NONE,
            },
            Event::Key {
                key: k,
                physical_key: None,
                pressed: false,
                repeat: false,
                modifiers: Modifiers::NONE,
            },
        ]);
        self.idle();
    }
}

fn money(s: &str) -> Money {
    Money::parse_wire(s).unwrap()
}

#[test]
fn a_balanced_check_saves_exact_amounts() {
    let mut rig = Rig::new();
    rig.demo.check_text = "1286.40".into();
    rig.idle();
    assert_eq!(
        rig.demo.lines.form().current().amount,
        Some(money("1286.40"))
    );
    let first = rig.demo.lines.form().current().lines[0].key;
    rig.demo.lines.focus(
        CellPos {
            row: first,
            col: ACCOUNT,
        },
        &rig.demo.lists,
    );
    rig.idle();
    rig.text("51");
    rig.key(Key::Tab);
    rig.text("842.15");
    rig.key(Key::Enter);
    rig.text("Mulch");
    rig.key(Key::Tab);
    rig.text("400+44.25");
    rig.key(Key::Tab);
    let check = rig.demo.lines.form().current().clone();
    assert_eq!(check.lines[0].account, Pick::Id(5100));
    assert_eq!(check.lines[0].amount, Some(money("842.15")));
    assert_eq!(check.lines[1].account, Pick::Id(5110));
    assert_eq!(check.lines[1].amount, Some(money("444.25")));
    assert!(super::lines::balanced(&check));

    assert!(rig.demo.save_with(cl_model::forms::SaveIntent::Save));
    assert_eq!(rig.demo.saved.len(), 1);
    assert!(rig.demo.last_save_balanced);
    let total = super::lines::lines_total(&rig.demo.saved[0]).unwrap();
    assert_eq!(total, money("1286.40"));
    assert!(!rig.demo.dirty());
}

#[test]
fn an_unbalanced_check_is_refused_with_a_problem() {
    let mut rig = Rig::new();
    rig.demo.check_text = "10.00".into();
    rig.idle();
    let first = rig.demo.lines.form().current().lines[0].key;
    rig.demo.lines.focus(
        CellPos {
            row: first,
            col: ACCOUNT,
        },
        &rig.demo.lists,
    );
    rig.idle();
    rig.text("6200");
    rig.key(Key::Tab);
    rig.text("9.99");
    // Alt+S arrives as a command; the open cell is committed first.
    assert!(
        rig.demo
            .command(cl_model::CommandId::new("form.save_close"))
    );
    assert_eq!(
        rig.demo.lines.form().current().lines[0].amount,
        Some(money("9.99"))
    );
    assert_eq!(rig.demo.saved.len(), 0);
    assert!(!rig.demo.last_save_balanced);
    assert_eq!(rig.demo.state()["problems"], 1);
}

#[test]
fn register_and_bills_modes() {
    let mut rig = Rig::new();
    rig.demo.show_register();
    rig.idle();
    assert_eq!(rig.demo.mode, Mode::Register);
    let s = rig.demo.state();
    assert_eq!(s["register_rows"], 200_000);
    assert_eq!(s["rows"], 200_000);
    rig.demo.one_line = true;
    rig.demo.show_register();
    rig.idle();
    assert_eq!(rig.demo.state()["rows"], 200_000);
    assert!(
        rig.demo
            .command(cl_model::CommandId::new("form.next_record"))
    );
    rig.idle();
    assert!(rig.demo.state()["cursor"]["row"].as_u64().unwrap() > 0);
    assert!(
        rig.demo
            .command(cl_model::CommandId::new("form.next_record"))
    );
    rig.idle();
    assert!(
        rig.demo.state()["top_row"].as_u64().unwrap() > 0,
        "{}",
        rig.demo.state()
    );

    rig.demo.show_bills();
    rig.idle();
    assert_eq!(rig.demo.state()["rows"], 6);
    assert_eq!(rig.demo.state()["mode"], "bills");
}

fn strings(v: &serde_json::Value, out: &mut Vec<String>) {
    match v {
        serde_json::Value::String(s) => out.push(s.clone()),
        serde_json::Value::Array(a) => a.iter().for_each(|x| strings(x, out)),
        serde_json::Value::Object(o) => o.values().for_each(|x| strings(x, out)),
        _ => {}
    }
}

/// T18: the screen's state holds positions, counts and flags only.
#[test]
fn state_has_no_values() {
    let mut rig = Rig::new();
    rig.demo.check_text = "1286.40".into();
    rig.idle();
    let mut found = Vec::new();
    strings(&rig.demo.state(), &mut found);
    assert_eq!(found, ["lines"], "only the mode is text");
    let text = rig.demo.state().to_string();
    assert!(!text.contains("1286") && !text.contains("1,286"));
}

fn focus_amount(rig: &mut Rig) {
    let first = rig.demo.lines.form().current().lines[0].key;
    rig.demo.lines.focus(
        CellPos {
            row: first,
            col: super::lines::AMOUNT,
        },
        &rig.demo.lists,
    );
    rig.idle();
}

/// T23: the saved record never holds the blank entry line, and Enter on a
/// pristine entry line is Save & New.
#[test]
fn entry_line_never_in_snapshot() {
    let mut rig = Rig::new();
    rig.demo.check_text = "842.15".into();
    rig.idle();
    let first = rig.demo.lines.form().current().lines[0].key;
    rig.demo.lines.focus(
        CellPos {
            row: first,
            col: ACCOUNT,
        },
        &rig.demo.lists,
    );
    rig.idle();
    rig.text("5100");
    rig.key(Key::Tab);
    rig.text("842.15");
    rig.key(Key::Enter);
    assert_eq!(rig.demo.lines.form().current().lines.len(), 2);
    // On the pristine entry line: Enter saves and starts a new check.
    rig.key(Key::Enter);
    assert_eq!(rig.demo.saved.len(), 1);
    let saved = &rig.demo.saved[0];
    assert_eq!(saved.lines.len(), 1, "the entry line is not saved");
    assert!(saved.lines.iter().all(|l| !l.is_blank()));
    assert_eq!(saved.lines[0].amount, Some(money("842.15")));
    assert_eq!(saved.amount, Some(money("842.15")));
    assert_eq!(rig.demo.lines.form().current().lines.len(), 1);
    assert!(rig.demo.lines.form().current().lines[0].is_blank());
}

/// T27: the screen changes a cell under the open editor. A clean editor
/// shows the new value; a dirty one keeps the typed text and says so.
#[test]
fn model_change_under_editor() {
    let mut rig = Rig::new();
    focus_amount(&mut rig);
    let (value, _) = rig.node("Amount, line 1").expect("the editor");
    assert_eq!(value, "");
    // A recompute (P20 qty x rate) writes Amount while its editor is open.
    rig.demo
        .lines
        .edit_step(|c| c.lines[0].amount = Some(money("500.00")));
    rig.idle();
    let (value, _) = rig.node("Amount, line 1").expect("the editor");
    assert_eq!(value, "500.00", "a clean editor follows the model");

    rig.text("12");
    rig.demo
        .lines
        .edit_step(|c| c.lines[0].amount = Some(money("600.00")));
    rig.idle();
    let (value, description) = rig.node("Amount, line 1").expect("the editor");
    assert!(value.ends_with("12"), "the typed text is kept: {value}");
    let shown = crate::i18n::t("grid.cell.changed_underneath");
    assert!(
        description.contains(&shown) || rig_shows(&mut rig, &shown),
        "the owner is told the cell changed"
    );
    // Esc shows the new value; Tab would have committed the typed one.
    assert!(rig.demo.command(cl_model::CommandId::new("form.revert")));
    rig.idle();
    let (value, _) = rig.node("Amount, line 1").expect("the editor");
    assert_eq!(value, "600.00");
    assert_eq!(
        rig.demo.lines.form().current().lines[0].amount,
        Some(money("600.00"))
    );
}

/// Any node shows `text` as its label, value or description.
fn rig_shows(rig: &mut Rig, text: &str) -> bool {
    rig.ctx.enable_accesskit();
    let input = RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1440.0, 940.0))),
        ..RawInput::default()
    };
    let demo = &mut rig.demo;
    let out = rig.ctx.run_ui(input, |ui| {
        egui::CentralPanel::default().show(ui, |ui| demo.ui(ui));
    });
    out.platform_output.accesskit_update.is_some_and(|u| {
        u.nodes.iter().any(|(_, n)| {
            [n.label(), n.value(), n.description()]
                .into_iter()
                .flatten()
                .any(|s| s.contains(text))
        })
    })
}
