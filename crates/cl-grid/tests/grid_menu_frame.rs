//! P16 D6 in real egui frames: the context menu opens on a right-click,
//! Shift+F10, the Menu key and AccessKit `ShowContextMenu`, is driven by
//! the keyboard and the mouse, and is off without opt-in; the select gutter
//! (pointer, Shift+click, AccessKit), Shift+Down, and the AccessKit names.

// AccessKit bounds are f64 screen geometry; never money.
#![allow(clippy::cast_possible_truncation, clippy::assert_is_empty)]

mod support;

use cl_grid::{
    CellPos, ColumnId, ContextTarget, GridEvent, GridSpec, RowKey, RowModel, SelectionMode,
};
use egui::accesskit::{Action, ActionRequest, Role, Toggled, TreeId};
use egui::{Event, Key, Modifiers, PointerButton, Pos2};
use support::lines::{ACCOUNT, AMOUNT, Lines, MEMO, line_spec};
use support::rig::{Rig, TestMenu, key};

fn bulk_spec() -> GridSpec {
    GridSpec {
        selection: SelectionMode::Multi,
        select_column: true,
        context_menu: true,
        ..line_spec()
    }
}

/// A grid with `rows` lines plus the ENTRY line, a menu source, drawn.
fn rig(spec: GridSpec, menu: bool) -> Rig {
    let mut r = Rig::new(spec, Lines::new(true, 4));
    if menu {
        r.menu = Some(TestMenu::standard());
    }
    r.frame(vec![]);
    r.frame(vec![]);
    r
}

fn pos(r: &Rig, row: usize, col: ColumnId) -> CellPos {
    CellPos {
        row: r.model.key(row),
        col,
    }
}

fn center(r: &Rig, id: egui::Id) -> Pos2 {
    r.bounds(id).expect("node drawn").center()
}

fn asked(r: &Rig) -> Vec<ContextTarget> {
    r.menu
        .as_ref()
        .map(|m| m.asked.borrow().clone())
        .unwrap_or_default()
}

fn picked(events: &[GridEvent]) -> Option<u16> {
    events.iter().find_map(|e| match e {
        GridEvent::ContextPicked { item, .. } => Some(*item),
        _ => None,
    })
}

fn request(id: egui::Id, action: Action) -> Event {
    Event::AccessKitActionRequest(ActionRequest {
        action,
        target_tree: TreeId::ROOT,
        target_node: id.accesskit_id(),
        data: None,
    })
}

fn nodes_with_role(r: &Rig, role: Role) -> Vec<egui::accesskit::Node> {
    r.nodes()
        .into_iter()
        .map(|(_, n)| n)
        .filter(|n| n.role() == role)
        .collect()
}

#[test]
fn a_right_click_opens_the_menu_on_that_cell() {
    let mut r = rig(bulk_spec(), true);
    let target = pos(&r, 2, MEMO);
    let at = center(&r, r.grid.cell_id(target));
    r.button_at(at, PointerButton::Secondary);
    r.frame(vec![]);
    assert_eq!(asked(&r), [ContextTarget::Cell(target)]);
    assert_eq!(
        r.grid.core().cursor(),
        Some(target),
        "the cursor moved first"
    );
    assert!(r.grid.core().menu().is_some());
    // The menu node and its entries.
    let menu = r.node(r.grid.menu_id()).expect("menu");
    assert_eq!(menu.role(), Role::Menu);
    assert_eq!(menu.label(), Some("Actions"));
    assert_eq!(nodes_with_role(&r, Role::MenuItem).len(), 4);
    let item = |i| {
        r.node(cl_grid::view::context_menu::item_id(r.grid.menu_id(), i))
            .expect("entry")
    };
    let labels: Vec<String> = (0..4)
        .map(|i| item(i).label().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        labels,
        ["Copy down", "Copy to rows below", "Paste", "Delete rows"]
    );
    assert_eq!(item(0).keyboard_shortcut(), Some("Ctrl+D"));
    assert!(item(0).is_selected().unwrap_or(false), "highlighted");
    assert!(item(2).is_disabled());
    assert_eq!(item(2).description(), Some("Nothing to paste"));
    // It sits at the pointer.
    let first = r
        .bounds(cl_grid::view::context_menu::item_id(r.grid.menu_id(), 0))
        .expect("entry");
    assert!(
        (first.left() - at.x).abs() < 6.0 && (first.top() - at.y).abs() < 6.0,
        "{first:?} at {at:?}"
    );
}

#[test]
fn without_opt_in_or_a_source_a_right_click_does_nothing() {
    for (spec, menu) in [(line_spec(), true), (bulk_spec(), false)] {
        let mut r = rig(spec, menu);
        let before = r.grid.core().cursor();
        let target = pos(&r, 2, MEMO);
        let at = center(&r, r.grid.cell_id(target));
        r.button_at(at, PointerButton::Secondary);
        r.frame(vec![]);
        assert!(r.grid.core().menu().is_none());
        assert!(asked(&r).is_empty());
        assert_eq!(
            r.grid.core().cursor(),
            before,
            "a right-click is not a click"
        );
        assert!(nodes_with_role(&r, Role::Menu).is_empty());
        // Shift+F10 and the Menu key stay unused too.
        r.focus_cell(1, MEMO);
        r.frame(key(Key::F10, Modifiers::SHIFT));
        r.frame(key(Key::Menu, Modifiers::NONE));
        assert!(r.grid.core().menu().is_none());
    }
}

#[test]
fn shift_f10_opens_it_and_the_keys_pick() {
    let mut r = rig(bulk_spec(), true);
    r.focus_cell(1, MEMO);
    r.frame(key(Key::F10, Modifiers::SHIFT));
    r.frame(vec![]);
    let target = pos(&r, 1, MEMO);
    assert_eq!(asked(&r), [ContextTarget::Cell(target)]);
    // Under the cursor cell (its editor speaks for it).
    let first = r
        .bounds(cl_grid::view::context_menu::item_id(r.grid.menu_id(), 0))
        .expect("entry");
    let cell = r.bounds(r.grid.focus_id()).expect("editor");
    assert!(
        (first.top() - cell.bottom()).abs() < 6.0 && (first.left() - cell.left()).abs() < 6.0,
        "{first:?} under {cell:?}"
    );
    // Typing goes nowhere while it is open.
    r.frame(vec![Event::Text("x".into())]);
    assert!(r.grid.core().edit().is_some_and(|e| e.buffer.is_empty()));
    r.frame(key(Key::ArrowDown, Modifiers::NONE));
    r.frame(key(Key::ArrowDown, Modifiers::NONE));
    assert_eq!(r.grid.core().menu().and_then(|m| m.highlight), Some(3));
    let out = r.frame(key(Key::Enter, Modifiers::NONE));
    assert_eq!(picked(&out.events), Some(4));
    assert!(r.grid.core().menu().is_none());
    assert_eq!(
        r.grid.core().cursor(),
        Some(target),
        "Enter did not move on"
    );
}

#[test]
fn the_menu_key_opens_it_and_esc_closes_it() {
    let mut r = rig(bulk_spec(), true);
    r.focus_cell(0, AMOUNT);
    r.frame(key(Key::Menu, Modifiers::NONE));
    assert!(r.grid.core().menu().is_some());
    r.frame(key(Key::Escape, Modifiers::NONE));
    r.frame(vec![]);
    assert!(r.grid.core().menu().is_none());
    assert!(nodes_with_role(&r, Role::Menu).is_empty());
    assert_eq!(r.grid.core().cursor(), Some(pos(&r, 0, AMOUNT)));
}

#[test]
fn clicking_an_entry_picks_it_and_a_click_outside_closes() {
    let mut r = rig(bulk_spec(), true);
    let target = pos(&r, 1, ACCOUNT);
    r.button_at(center(&r, r.grid.cell_id(target)), PointerButton::Secondary);
    r.frame(vec![]);
    let item = cl_grid::view::context_menu::item_id(r.grid.menu_id(), 1);
    let events = r.click_at(center(&r, item));
    assert_eq!(picked(&events), Some(2));
    assert!(r.grid.core().menu().is_none());
    // A disabled entry does nothing (the cursor cell now has its editor:
    // right-click another cell).
    let target = pos(&r, 2, MEMO);
    r.button_at(center(&r, r.grid.cell_id(target)), PointerButton::Secondary);
    r.frame(vec![]);
    let paste = cl_grid::view::context_menu::item_id(r.grid.menu_id(), 2);
    let events = r.click_at(center(&r, paste));
    assert_eq!(picked(&events), None);
    assert!(r.grid.core().menu().is_some());
    // A click on another cell (above the menu) closes it and moves there.
    let other = pos(&r, 0, AMOUNT);
    r.click_at(center(&r, r.grid.cell_id(other)));
    assert!(r.grid.core().menu().is_none());
    assert_eq!(r.grid.core().cursor(), Some(other));
}

#[test]
fn a_header_right_click_asks_about_the_column() {
    let mut r = rig(bulk_spec(), true);
    let before = r.grid.core().cursor();
    let header = r.grid.id().with(("header", AMOUNT.0));
    r.button_at(center(&r, header), PointerButton::Secondary);
    r.frame(vec![]);
    assert_eq!(asked(&r), [ContextTarget::Header(AMOUNT)]);
    assert_eq!(r.grid.core().cursor(), before);
    let item = cl_grid::view::context_menu::item_id(r.grid.menu_id(), 0);
    let events = r.click_at(center(&r, item));
    assert!(events.contains(&GridEvent::ContextPicked {
        target: ContextTarget::Header(AMOUNT),
        item: 1,
    }));
}

#[test]
fn accesskit_show_context_menu_opens_it() {
    let mut r = rig(bulk_spec(), true);
    let target = pos(&r, 2, AMOUNT);
    let cell = r.grid.cell_id(target);
    assert!(
        r.node(cell)
            .is_some_and(|n| n.supports_action(Action::ShowContextMenu))
    );
    r.frame(vec![request(cell, Action::ShowContextMenu)]);
    r.frame(vec![]);
    assert_eq!(asked(&r), [ContextTarget::Cell(target)]);
    // Without opt-in the cell does not offer it.
    let plain = rig(line_spec(), true);
    let cell = plain.grid.cell_id(pos(&plain, 2, AMOUNT));
    assert!(
        plain
            .node(cell)
            .is_some_and(|n| !n.supports_action(Action::ShowContextMenu))
    );
}

// ---------------------------------------------------------- the gutter

fn selected(r: &Rig) -> Vec<usize> {
    let mut v: Vec<usize> = r
        .grid
        .selected_keys()
        .filter_map(|k| r.model.index_of(k))
        .collect();
    v.sort_unstable();
    v
}

#[test]
fn gutter_checkboxes_are_named_and_toggle() {
    let mut r = rig(bulk_spec(), false);
    let entry = r.model.row_count() - 1;
    for i in 0..entry {
        let n = r.node(r.grid.gutter_id(r.model.key(i))).expect("checkbox");
        assert_eq!(n.role(), Role::CheckBox);
        assert_eq!(n.label(), Some(format!("Select row {}", i + 1).as_str()));
        assert_eq!(n.toggled(), Some(Toggled::False));
    }
    assert!(
        r.node(r.grid.gutter_id(r.model.key(entry))).is_none(),
        "the ENTRY row has no checkbox"
    );
    let row1 = r.grid.gutter_id(r.model.key(1));
    let events = r.click_at(center(&r, row1));
    assert!(events.contains(&GridEvent::SelectionChanged));
    assert_eq!(selected(&r), [1]);
    r.frame(vec![]);
    assert_eq!(r.node(row1).and_then(|n| n.toggled()), Some(Toggled::True));
    let row_node = r.node(r.grid.row_id(r.model.key(1))).expect("row");
    assert!(row_node.is_selected().unwrap_or(false));
    // Shift+click is a range.
    r.modifiers = Modifiers::SHIFT;
    r.click_at(center(&r, r.grid.gutter_id(r.model.key(3))));
    r.modifiers = Modifiers::NONE;
    assert_eq!(selected(&r), [1, 2, 3]);
    // AccessKit Click toggles too.
    r.frame(vec![request(
        r.grid.gutter_id(r.model.key(2)),
        Action::Click,
    )]);
    assert_eq!(selected(&r), [1, 3]);
    // The first column starts after the gutter.
    let cell = r.bounds(r.grid.cell_id(pos(&r, 3, ACCOUNT))).expect("cell");
    let grid = r.bounds(r.grid.id()).expect("grid");
    assert!(cell.left() >= grid.left() + 27.5, "{cell:?} in {grid:?}");
}

#[test]
fn without_the_select_column_there_is_no_gutter() {
    let r = rig(line_spec(), false);
    assert!(r.node(r.grid.gutter_id(r.model.key(0))).is_none());
    let cell = r.bounds(r.grid.cell_id(pos(&r, 3, ACCOUNT))).expect("cell");
    let grid = r.bounds(r.grid.id()).expect("grid");
    assert!((cell.left() - grid.left()).abs() < 0.5);
}

#[test]
fn shift_down_extends_and_ctrl_click_on_the_gutter_toggles() {
    let mut r = rig(bulk_spec(), false);
    r.focus_cell(0, MEMO);
    r.frame(key(Key::ArrowDown, Modifiers::SHIFT));
    r.frame(key(Key::ArrowDown, Modifiers::SHIFT));
    assert_eq!(selected(&r), [0, 1, 2]);
    assert_eq!(r.grid.core().cursor(), Some(pos(&r, 2, MEMO)));
    r.modifiers = Modifiers::COMMAND;
    r.click_at(center(&r, r.grid.gutter_id(r.model.key(1))));
    r.modifiers = Modifiers::NONE;
    assert_eq!(selected(&r), [0, 2]);
    let snap = r.with_env(|g, m, _| g.snapshot(&m));
    assert_eq!(snap.selected, 2);
    // The selection survives the rows moving (a sort by the screen).
    r.model.form.edit_step(|lines| {
        let entry = lines.pop();
        lines.reverse();
        lines.extend(entry);
    });
    r.frame(vec![]);
    assert_eq!(selected(&r), [1, 3]);
    let keys: Vec<RowKey> = r.grid.selected_keys().collect();
    assert_eq!(keys.len(), 2);
}
