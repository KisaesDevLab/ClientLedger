//! Cell editors through real egui frames (P10 plan Rev 2 §6): T12 (Space
//! key and its text), dropdowns, multiline memos, locked checkboxes and
//! clicks that keep the grid's focus; pickers, the full list, "Add …",
//! tag chips and T14 (suggestions are accepted only by Tab).

mod support;

use cl_grid::{CellPos, GridCommand, GridEvent, Handled, RowModel, Source};
use egui::accesskit::Role;
use egui::{Event, Key, Modifiers};
use support::browse::Browse;
use support::lines::{
    ACCOUNT, AMOUNT, Account, BILLABLE, DATE, Lines, MEMO, STATUS, TAGS, extras_spec, line_spec,
    money,
};
use support::register::{self, Register};
use support::rig::{Rig, key, shift_tab, tab, typed};

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
fn the_editor_keeps_the_cell_padding() {
    fn walk(s: &egui::Shape, out: &mut Vec<egui::epaint::TextShape>) {
        match s {
            egui::Shape::Text(t) => out.push(t.clone()),
            egui::Shape::Vec(v) => v.iter().for_each(|x| walk(x, out)),
            _ => {}
        }
    }
    let mut r = Rig::new(line_spec(), Lines::new(true, 2));
    r.focus_cell(0, DATE);
    r.frame(typed("7"));
    r.frame(vec![]);
    let header = support::rig::text(line_spec().column(DATE).expect("date").header);
    let left = r
        .nodes()
        .into_iter()
        .find(|(_, n)| n.role() == Role::ColumnHeader && n.label() == Some(&header))
        .and_then(|(_, n)| n.bounds())
        .expect("date header")
        .x0;
    let mut texts = Vec::new();
    for c in &r.last.as_ref().expect("frame").shapes {
        walk(&c.shape, &mut texts);
    }
    let typed_text = texts
        .iter()
        .find(|t| t.galley.text() == "7")
        .expect("the typed date text");
    #[allow(clippy::cast_possible_truncation)]
    let left = left as f32;
    assert!(
        typed_text.pos.x >= left + r.style.pad - 0.5,
        "the text at {} touches the cell edge at {left}",
        typed_text.pos.x
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

// ---- pickers, full list, tags, suggestions (step 13b) -------------------

fn suggested_rig() -> Rig {
    let mut r = Rig::new(line_spec(), Lines::new(true, 2));
    r.model.form.edit_step(|l| l[0].suggested = Some(5100));
    r.focus_cell(0, ACCOUNT);
    r
}

fn accepted(r: &Rig) -> bool {
    r.model
        .set_calls
        .iter()
        .any(|(_, s)| *s == Source::SuggestionAccepted)
}

fn popup_items(r: &Rig) -> Vec<(u64, String)> {
    r.grid
        .core()
        .edit()
        .and_then(|e| e.popup.as_ref())
        .map(|p| p.items.iter().map(|s| (s.id, s.label.clone())).collect())
        .unwrap_or_default()
}

fn has_label(r: &Rig, role: Role, text: &str) -> bool {
    r.nodes()
        .iter()
        .any(|(_, n)| n.role() == role && n.label().is_some_and(|l| l.contains(text)))
}

fn texts(s: &egui::Shape) -> Vec<String> {
    match s {
        egui::Shape::Text(t) => vec![t.galley.text().to_string()],
        egui::Shape::Vec(v) => v.iter().flat_map(texts).collect(),
        _ => Vec::new(),
    }
}

#[test]
fn suggestion_accept_only_by_tab() {
    let check = |r: &Rig, how: &str| {
        assert_eq!(r.model.lines()[0].account, Account::None, "{how}");
        assert!(!accepted(r), "{how} accepted the suggestion");
    };
    // The suggestion shows as a placeholder in the cell, not a value.
    let mut r = suggested_rig();
    r.frame(key(Key::ArrowDown, Modifiers::NONE));
    check(&r, "Down");
    let cell = r
        .node(r.grid.cell_id(CellPos {
            row: r.model.key(0),
            col: ACCOUNT,
        }))
        .expect("cell");
    assert_eq!(cell.value(), Some("5100 · Materials:Plants"));

    let mut r = suggested_rig();
    let out = r.frame(key(Key::Enter, Modifiers::NONE));
    check(&r, "Enter");
    assert!(
        out.events
            .iter()
            .any(|e| matches!(e, GridEvent::RecordRefused { .. })),
        "a row with only a suggested account fails validation"
    );

    let mut r = suggested_rig();
    r.frame(shift_tab());
    check(&r, "Shift+Tab");

    let mut r = suggested_rig();
    let memo = r
        .bounds(r.grid.cell_id(CellPos {
            row: r.model.key(1),
            col: MEMO,
        }))
        .expect("memo");
    r.click_at(memo.center());
    check(&r, "a click elsewhere");

    let mut r = suggested_rig();
    assert_eq!(r.commit_pending(), Ok(()));
    check(&r, "commit_pending");

    let mut r = suggested_rig();
    r.frame(tab());
    assert_eq!(r.model.lines()[0].account, Account::Id(5100));
    assert!(accepted(&r));
    assert_eq!(r.at(), (0, AMOUNT));
}

#[test]
fn an_amount_suggestion_never_shows_or_commits() {
    let mut r = Rig::new(line_spec(), Lines::new(true, 2));
    r.model.suggest_amount = true;
    r.focus_cell(0, ACCOUNT);
    r.frame(vec![]);
    let cell = r
        .node(r.grid.cell_id(CellPos {
            row: r.model.key(1),
            col: AMOUNT,
        }))
        .expect("cell");
    assert_eq!(cell.value(), Some(""));
    let painted: Vec<String> = r
        .last
        .as_ref()
        .expect("frame")
        .shapes
        .iter()
        .flat_map(|c| texts(&c.shape))
        .collect();
    assert!(painted.iter().all(|t| !t.contains("999")), "{painted:?}");
    r.focus_cell(0, AMOUNT);
    r.frame(tab());
    assert_eq!(r.model.lines()[0].amount, None);
    assert!(!accepted(&r));
}

#[test]
fn type_ahead_ranks_and_tab_takes_the_highlight() {
    let mut r = Rig::new(line_spec(), Lines::new(true, 2));
    r.focus_cell(0, ACCOUNT);
    r.frame(typed("51"));
    r.frame(vec![]);
    r.frame(vec![]);
    let items = popup_items(&r);
    assert_eq!(items.first().map(|i| i.0), Some(5100), "{items:?}");
    assert!(has_label(
        &r,
        Role::ListBoxOption,
        "5100 · Materials:Plants"
    ));
    r.frame(key(Key::ArrowDown, Modifiers::NONE));
    r.frame(tab());
    assert_eq!(r.model.lines()[0].account, Account::Id(5110));
    assert!(
        r.model
            .set_calls
            .iter()
            .any(|(p, s)| p.col == ACCOUNT && *s == Source::Picked)
    );
}

#[test]
fn a_loading_list_says_so() {
    let mut r = Rig::new(line_spec(), Lines::new(true, 2));
    r.lookups.loading = true;
    r.focus_cell(0, ACCOUNT);
    r.frame(typed("51"));
    r.frame(vec![]);
    r.frame(vec![]);
    assert!(has_label(&r, Role::ListBoxOption, "grid.popup.loading"));
}

#[test]
fn full_list_starts_on_the_entry_and_picks_only_when_moved() {
    let mut r = Rig::new(line_spec(), Lines::new(true, 2));
    r.focus_cell(0, ACCOUNT);
    assert_eq!(r.command(GridCommand::OpenList), Handled::Handled);
    assert_eq!(popup_items(&r).len(), 4);
    assert_eq!(popup_highlight(&r), None, "nothing chosen yet");
    // A save right after Ctrl+L picks nothing.
    assert_eq!(r.commit_pending(), Ok(()));
    assert_eq!(r.model.lines()[0].account, Account::None);
    r.command(GridCommand::OpenList);
    r.frame(key(Key::ArrowDown, Modifiers::NONE));
    r.frame(key(Key::ArrowDown, Modifiers::NONE));
    assert_eq!(popup_highlight(&r), Some(1));
    let want = popup_items(&r)[1].0;
    r.frame(key(Key::Enter, Modifiers::NONE));
    assert_eq!(r.model.lines()[0].account, Account::Id(want));
    assert_eq!(popup_highlight(&r), None);
    assert_eq!(r.at(), (0, ACCOUNT), "Enter in the list does not record");
    // Reopened, the list starts on the cell's entry.
    r.command(GridCommand::OpenList);
    assert_eq!(popup_highlight(&r), Some(1));
    r.frame(vec![]);
    r.frame(vec![]);
    assert!(has_label(&r, Role::ListBox, "grid.popup.full_list"));
    // Not a list cell: the shell keeps Ctrl+L.
    r.frame(key(Key::Escape, Modifiers::NONE));
    r.focus_cell(0, AMOUNT);
    assert_eq!(r.command(GridCommand::OpenList), Handled::NotHandled);
}

#[test]
fn add_new_asks_the_screen_and_keeps_focus() {
    let mut r = Rig::new(line_spec(), Lines::new(true, 2));
    r.focus_cell(0, ACCOUNT);
    r.frame(typed("Zebra"));
    r.frame(vec![]);
    r.frame(vec![]);
    let add = r
        .bounds(cl_grid::view::overlay::add_new_id(r.grid.popup_id()))
        .expect("Add row");
    assert!(has_label(&r, Role::ListBoxOption, "Add \"Zebra\""));
    let events = r.click_at(add.center());
    assert!(
        events
            .iter()
            .any(|e| matches!(e, GridEvent::AddNew { text, .. } if text == "Zebra")),
        "{events:?}"
    );
    assert_eq!(
        r.model.lines()[0].account,
        Account::Unresolved("Zebra".into())
    );
    assert_eq!(r.focused(), Some(r.grid.focus_id()));
}

#[test]
fn tags_show_chips_and_replace_within_a_group() {
    let mut r = Rig::new(line_spec(), Lines::new(true, 2));
    r.focus_cell(0, TAGS);
    for t in ["Install", "Crew A", "Crew B"] {
        r.frame(typed(t));
        r.frame(key(Key::Enter, Modifiers::NONE));
    }
    r.frame(vec![]);
    let labels: Vec<&str> = r.model.lines()[0]
        .tags
        .iter()
        .map(|c| c.label.as_str())
        .collect();
    assert_eq!(labels, ["Install", "Crew B"]);
    let focus = r.node(r.grid.focus_id()).expect("focus node");
    assert_eq!(focus.description(), Some("Install · Crew B"));
    assert!(has_label(&r, Role::Status, "grid.tags.replaced"));
    // Backspace in the empty editor removes the last chip.
    r.frame(key(Key::Backspace, Modifiers::NONE));
    assert_eq!(r.model.lines()[0].tags.len(), 1);
    // Enter on the empty editor records the row (no account: refused).
    let out = r.frame(key(Key::Enter, Modifiers::NONE));
    assert!(
        out.events
            .iter()
            .any(|e| matches!(e, GridEvent::RecordRefused { .. }))
    );
}
