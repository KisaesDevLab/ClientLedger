//! Cell editors through real egui frames (P10 plan Rev 2 §6): T12 (Space
//! key and its text), dropdowns, multiline memos, locked checkboxes and
//! clicks that keep the grid's focus; T14 (suggestions) in step 13b.

mod support;

use cl_grid::{CellPos, GridEvent, RowModel};
use egui::accesskit::Role;
use egui::{Event, Key, Modifiers};
use support::browse::Browse;
use support::lines::{
    ACCOUNT, AMOUNT, BILLABLE, DATE, Lines, MEMO, STATUS, TAGS, extras_spec, line_spec, money,
};
use support::register::{self, Register};
use support::rig::{Rig, key, tab, typed};

/// Space as a desktop delivers it: the key press, its text, the release.
fn space() -> Vec<Event> {
    let mut v = key(Key::Space, Modifiers::NONE);
    v.insert(1, Event::Text(" ".into()));
    v
}

fn popup_highlight(r: &Rig) -> Option<usize> {
    r.grid
        .core()
        .edit()
        .and_then(|e| e.popup.as_ref())
        .and_then(|p| p.highlight)
}

#[test]
fn space_key_and_text_pair_toggles_once() {
    let mut r = Rig::new(line_spec(), Lines::new(true, 2));
    r.focus_cell(0, BILLABLE);
    r.frame(space());
    assert!(r.model.lines()[0].billable, "one toggle, not two");
    r.frame(space());
    assert!(!r.model.lines()[0].billable);
    // The control channel types a lone space: that toggles once too.
    r.frame(typed(" "));
    assert!(r.model.lines()[0].billable);
    assert_eq!(r.focused(), Some(r.grid.focus_id()));
    // The focus node speaks for the checkbox.
    let node = r.node(r.grid.focus_id()).expect("focus node");
    assert_eq!(node.role(), Role::CheckBox);
    assert_eq!(node.toggled(), Some(egui::accesskit::Toggled::True));
    assert_eq!(node.label(), Some("Billable, line 1"));
}

#[test]
fn space_in_browse_selects_and_types_nothing() {
    let mut b = Browse::new(Register::new(20), register::spec());
    b.focus();
    b.frame(key(Key::ArrowDown, Modifiers::NONE));
    b.frame(space());
    assert_eq!(b.grid.selected_keys().count(), 1);
    assert!(
        b.events.contains(&GridEvent::SelectionChanged),
        "{:?}",
        b.events
    );
    assert!(
        !b.events
            .iter()
            .any(|e| matches!(e, GridEvent::TypedInBrowse(_))),
        "the space's text is not typing"
    );
}

#[test]
fn locked_checkbox_reports_the_attempt() {
    let mut r = Rig::new(line_spec(), Lines::new(true, 2));
    r.model.form.edit_step(|l| l[0].locked = true);
    r.focus_cell(0, BILLABLE);
    let out = r.frame(space());
    assert!(!r.model.lines()[0].billable);
    let key0 = r.model.key(0);
    assert!(
        out.events
            .contains(&GridEvent::LockedRowEditAttempt { key: key0 }),
        "{:?}",
        out.events
    );
}

#[test]
fn dropdown_opens_jumps_and_picks() {
    let mut r = Rig::new(extras_spec(), Lines::new(true, 2));
    r.focus_cell(0, STATUS);
    assert_eq!(popup_highlight(&r), None, "closed until asked");
    r.frame(space());
    assert_eq!(popup_highlight(&r), Some(0));
    // egui sizes a new Area invisibly first; its nodes come a frame later.
    r.frame(vec![]);
    r.frame(vec![]);
    let list = r.node(r.grid.popup_id()).expect("list node");
    assert_eq!(list.role(), Role::ListBox);
    let first = r
        .node(cl_grid::view::overlay::item_id(r.grid.popup_id(), 0))
        .expect("option");
    assert_eq!(first.role(), Role::ListBoxOption);
    assert_eq!(first.label(), Some("Open"));
    assert!(first.is_selected().unwrap_or(false));
    let focus = r.node(r.grid.focus_id()).expect("focus node");
    assert_eq!(focus.role(), Role::ComboBox);
    assert_eq!(focus.is_expanded(), Some(true));
    // A letter jumps; Enter picks without recording the row.
    r.frame(typed("p"));
    assert_eq!(popup_highlight(&r), Some(2));
    let out = r.frame(key(Key::Enter, Modifiers::NONE));
    assert_eq!(r.model.lines()[0].status, Some(3));
    assert_eq!(popup_highlight(&r), None);
    assert_eq!(r.at(), (0, STATUS));
    assert!(
        !out.events
            .iter()
            .any(|e| matches!(e, GridEvent::RecordRefused { .. })),
        "Enter in the list does not record"
    );
    // Alt+Down opens on the current choice; Up then Tab picks and moves on.
    r.frame(key(Key::ArrowDown, Modifiers::ALT));
    assert_eq!(popup_highlight(&r), Some(2));
    r.frame(key(Key::ArrowUp, Modifiers::NONE));
    r.frame(tab());
    assert_eq!(r.model.lines()[0].status, Some(2));
    assert_eq!(r.at(), (0, TAGS));
    assert_eq!(r.focused(), Some(r.grid.focus_id()));
}

#[test]
fn dropdown_letter_opens_and_esc_closes() {
    let mut r = Rig::new(extras_spec(), Lines::new(true, 2));
    r.focus_cell(0, STATUS);
    r.frame(typed("b"));
    assert_eq!(popup_highlight(&r), Some(1));
    r.frame(key(Key::Escape, Modifiers::NONE));
    assert_eq!(popup_highlight(&r), None);
    assert_eq!(r.model.lines()[0].status, None, "Esc picks nothing");
}

#[test]
fn clicking_a_choice_picks_it_and_keeps_focus() {
    let mut r = Rig::new(extras_spec(), Lines::new(true, 2));
    r.focus_cell(0, STATUS);
    r.frame(space());
    r.frame(vec![]);
    r.frame(vec![]);
    let paid = r
        .bounds(cl_grid::view::overlay::item_id(r.grid.popup_id(), 2))
        .expect("option bounds");
    r.click_at(paid.center());
    r.frame(vec![]);
    assert_eq!(r.model.lines()[0].status, Some(3));
    assert_eq!(r.focused(), Some(r.grid.focus_id()));
    assert_eq!(popup_highlight(&r), None);
}

#[test]
fn clicking_another_cell_commits_and_keeps_focus() {
    let mut r = Rig::new(line_spec(), Lines::new(true, 2));
    r.focus_cell(0, AMOUNT);
    r.frame(typed("12"));
    let target = r
        .bounds(r.grid.cell_id(CellPos {
            row: r.model.key(1),
            col: MEMO,
        }))
        .expect("cell bounds");
    r.click_at(target.center());
    r.frame(vec![]);
    assert_eq!(r.model.lines()[0].amount, Some(money("12.00")));
    assert_eq!(r.at(), (1, MEMO));
    assert_eq!(r.focused(), Some(r.grid.focus_id()), "focus stayed");
    assert!(!r.grid.core().is_parked());
}

#[test]
fn multiline_memo_takes_shift_enter_and_grows() {
    let mut r = Rig::new(extras_spec(), Lines::new(true, 2));
    r.focus_cell(0, MEMO);
    let row = r.grid.row_id(r.model.key(0));
    let base = r.bounds(row).expect("row").height();
    r.frame(typed("Hardwood"));
    r.frame(key(Key::Enter, Modifiers::SHIFT));
    r.frame(typed("mulch"));
    r.frame(vec![]);
    assert_eq!(
        r.grid.core().edit().map(|e| e.buffer.as_str()),
        Some("Hardwood\nmulch")
    );
    let grown = r.bounds(row).expect("row").height();
    assert!(grown > base + 10.0, "{base} -> {grown}");
    r.frame(tab());
    r.frame(vec![]);
    assert_eq!(r.model.lines()[0].memo, "Hardwood\nmulch");
    assert_eq!(r.at(), (0, DATE));
    assert!(r.bounds(row).expect("row").height() > base + 10.0);
    // Plain Enter records the row (no account yet: refused, back to it).
    r.focus_cell(0, MEMO);
    let out = r.frame(key(Key::Enter, Modifiers::NONE));
    assert_eq!(r.model.lines()[0].memo, "Hardwood\nmulch");
    assert!(
        out.events
            .iter()
            .any(|e| matches!(e, GridEvent::RecordRefused { .. })),
        "{:?}",
        out.events
    );
    assert_eq!(r.at(), (0, ACCOUNT));
}

#[test]
fn single_line_memo_ignores_shift_enter_newlines() {
    let mut r = Rig::new(line_spec(), Lines::new(true, 2));
    r.focus_cell(0, MEMO);
    r.frame(typed("Perennials"));
    let out = r.frame(key(Key::Enter, Modifiers::SHIFT));
    assert!(!r.model.lines()[0].memo.contains('\n'));
    assert!(
        out.events
            .iter()
            .any(|e| matches!(e, GridEvent::RecordRefused { .. })),
        "Shift+Enter is Enter outside a multiline memo"
    );
}

#[test]
fn date_editor_takes_date_keys() {
    let mut r = Rig::new(line_spec(), Lines::new(true, 2));
    r.focus_cell(0, DATE);
    r.frame(typed("t"));
    r.frame(typed("+"));
    r.frame(tab());
    let tomorrow = support::lines::formats().today.add_days(1).expect("date");
    assert_eq!(r.model.lines()[0].date, Some(tomorrow));
    assert_eq!(r.model.row_count(), 3);
}
