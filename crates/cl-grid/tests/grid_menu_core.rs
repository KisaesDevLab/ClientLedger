//! P16 D6 in the state machine: the context menu (open, keys, pick,
//! close), the select gutter, Shift+Up/Down, Select all and Clear
//! selection, and selection by key across reorders and undo.

#![allow(clippy::assert_is_empty)]

mod support;

use cl_grid::core::keys::{Formats, GridInput, GridKey, Mods};
use cl_grid::core::state::{Core, Effects};
use cl_grid::{
    CellPos, ContextItem, ContextTarget, GridCommand, GridEvent, GridSpec, ModelRef, RowKey,
    RowModel, SelectionMode,
};
use support::lines::{ACCOUNT, AMOUNT, Lines, MEMO, TestLookups, formats, line_spec};

fn bulk_spec() -> GridSpec {
    GridSpec {
        selection: SelectionMode::Multi,
        select_column: true,
        context_menu: true,
        ..line_spec()
    }
}

fn items() -> Vec<ContextItem> {
    vec![
        ContextItem::new(1, "Copy down").shortcut("Ctrl+D"),
        ContextItem::new(2, "Copy to rows below"),
        ContextItem::new(3, "Paste").enabled(false),
        ContextItem::new(4, "Delete rows"),
    ]
}

struct Rig {
    core: Core,
    m: Lines,
    l: TestLookups,
    fm: Formats,
}

impl Rig {
    /// `rows` lines plus the blank ENTRY line, focused on line 1.
    fn new(spec: GridSpec, rows: usize) -> Rig {
        let mut r = Rig {
            core: Core::new(spec),
            m: Lines::new(true, rows),
            l: TestLookups::default(),
            fm: formats(),
        };
        r.input(GridInput::FocusGained { from_below: false });
        r
    }
    fn bulk(rows: usize) -> Rig {
        Rig::new(bulk_spec(), rows)
    }
    fn input(&mut self, i: GridInput) -> Effects {
        let mut mr = ModelRef::Edit(&mut self.m);
        self.core.handle(i, &mut mr, &self.l, self.fm)
    }
    fn key(&mut self, k: GridKey, m: Mods) -> Effects {
        self.input(GridInput::Key(k, m))
    }
    fn cmd(&mut self, c: GridCommand) -> Effects {
        self.input(GridInput::Command(c))
    }
    fn pos(&self, row: usize, col: cl_grid::ColumnId) -> CellPos {
        CellPos {
            row: self.m.key(row),
            col,
        }
    }
    /// Right-click on a cell: prepare, then open with the screen's items.
    fn menu_on(&mut self, target: ContextTarget) -> (bool, Effects) {
        let mut mr = ModelRef::Edit(&mut self.m);
        let (ready, fx) = self.core.prepare_menu(target, &mut mr, &self.l, self.fm);
        let opened = ready && self.core.open_menu(target, items());
        (opened, fx)
    }
    fn gutter(&mut self, row: usize, mods: Mods) -> Effects {
        let row = self.m.key(row);
        self.input(GridInput::GutterClick { row, mods })
    }
    fn selected(&self) -> Vec<usize> {
        let mut v: Vec<usize> = self
            .core
            .selected_keys()
            .filter_map(|k| self.m.index_of(k))
            .collect();
        v.sort_unstable();
        v
    }
    fn at(&self) -> (usize, cl_grid::ColumnId) {
        let c = self.core.cursor().expect("cursor");
        (self.m.index_of(c.row).expect("row"), c.col)
    }
    fn resync(&mut self) -> Effects {
        let mut mr = ModelRef::Edit(&mut self.m);
        self.core.resync(&mut mr, self.fm, true)
    }
}

fn picked(fx: &Effects) -> Option<(ContextTarget, u16)> {
    fx.events.iter().find_map(|e| match e {
        GridEvent::ContextPicked { target, item } => Some((*target, *item)),
        _ => None,
    })
}

fn changed(fx: &Effects) -> bool {
    fx.events.contains(&GridEvent::SelectionChanged)
}

// ----------------------------------------------------------- the menu

#[test]
fn a_menu_on_a_cell_moves_the_cursor_and_commits_first() {
    let mut r = Rig::bulk(3);
    let amount = r.pos(0, AMOUNT);
    r.input(GridInput::Click {
        pos: amount,
        double: false,
        mods: Mods::NONE,
    });
    r.input(GridInput::Text("12+".into()));
    // An unfinished calculation: a menu elsewhere first commits, which
    // refuses it.
    let target = ContextTarget::Cell(r.pos(2, MEMO));
    let (opened, _) = r.menu_on(target);
    assert!(!opened, "an invalid cell keeps the cursor (S11)");
    assert_eq!(r.at(), (0, AMOUNT));
    assert!(r.core.menu().is_none());
    // A valid value commits, then the cursor moves and the menu opens.
    r.input(GridInput::BufferEdited("12.50".into()));
    let (opened, fx) = r.menu_on(target);
    assert!(opened);
    assert!(
        fx.events
            .iter()
            .any(|e| matches!(e, GridEvent::CellCommitted { .. }))
    );
    assert_eq!(r.at(), (2, MEMO));
    let menu = r.core.menu().expect("open");
    assert_eq!(menu.target, target);
    assert_eq!(menu.highlight, Some(0));
    assert!(
        r.core.edit().is_some_and(|e| e.popup.is_none()),
        "no list under the menu"
    );
}

#[test]
fn keys_move_over_disabled_entries_and_enter_picks() {
    let mut r = Rig::bulk(3);
    let target = ContextTarget::Cell(r.pos(1, AMOUNT));
    assert!(r.menu_on(target).0);
    r.key(GridKey::Down, Mods::NONE);
    r.key(GridKey::Down, Mods::NONE);
    assert_eq!(
        r.core.menu().unwrap().highlight,
        Some(3),
        "Paste is skipped"
    );
    r.key(GridKey::Down, Mods::NONE);
    assert_eq!(r.core.menu().unwrap().highlight, Some(0), "wraps around");
    r.key(GridKey::Up, Mods::NONE);
    assert_eq!(r.core.menu().unwrap().highlight, Some(3));
    r.key(GridKey::Home, Mods::NONE);
    assert_eq!(r.core.menu().unwrap().highlight, Some(0));
    r.key(GridKey::End, Mods::NONE);
    assert_eq!(r.core.menu().unwrap().highlight, Some(3));
    // Typing is ignored and the cursor does not move under the menu.
    r.input(GridInput::Text("x".into()));
    assert_eq!(r.at(), (1, AMOUNT));
    assert!(r.core.edit().is_some_and(|e| e.buffer.is_empty()));
    let fx = r.key(GridKey::Enter, Mods::NONE);
    assert_eq!(picked(&fx), Some((target, 4)));
    assert!(r.core.menu().is_none());
}

#[test]
fn a_disabled_entry_cannot_be_picked() {
    let mut r = Rig::bulk(2);
    assert!(r.menu_on(ContextTarget::Cell(r.pos(0, MEMO))).0);
    let fx = r.input(GridInput::MenuPicked(2));
    assert_eq!(picked(&fx), None);
    assert!(r.core.menu().is_some(), "stays open");
    let fx = r.input(GridInput::MenuPicked(1));
    assert_eq!(picked(&fx).map(|p| p.1), Some(2));
    // A stale click with no menu does nothing.
    assert_eq!(r.input(GridInput::MenuPicked(0)), Effects::default());
}

#[test]
fn esc_tab_revert_shift_f10_a_click_and_focus_loss_close_it() {
    let closers: Vec<GridInput> = vec![
        GridInput::Key(GridKey::Escape, Mods::NONE),
        GridInput::Key(GridKey::Tab, Mods::NONE),
        GridInput::Key(GridKey::ContextMenu, Mods::SHIFT),
        GridInput::Command(GridCommand::Revert),
        GridInput::FocusLost,
    ];
    for c in closers {
        let mut r = Rig::bulk(3);
        assert!(r.menu_on(ContextTarget::Cell(r.pos(1, MEMO))).0);
        let fx = r.input(c.clone());
        assert!(r.core.menu().is_none(), "{c:?}");
        assert_eq!(picked(&fx), None);
        // Esc and Tab only close the menu: the cursor stays.
        if !matches!(c, GridInput::FocusLost) {
            assert_eq!(r.at(), (1, MEMO), "{c:?}");
        }
    }
    // A click elsewhere closes it and then acts as a click.
    let mut r = Rig::bulk(3);
    assert!(r.menu_on(ContextTarget::Cell(r.pos(1, MEMO))).0);
    let pos = r.pos(2, MEMO);
    r.input(GridInput::Click {
        pos,
        double: false,
        mods: Mods::NONE,
    });
    assert!(r.core.menu().is_none());
    assert_eq!(r.at(), (2, MEMO));
}

#[test]
fn header_menus_keep_the_cursor() {
    let mut r = Rig::bulk(3);
    let (opened, _) = r.menu_on(ContextTarget::Header(AMOUNT));
    assert!(opened);
    assert_eq!(r.at(), (0, ACCOUNT));
    let fx = r.key(GridKey::Enter, Mods::NONE);
    assert_eq!(picked(&fx), Some((ContextTarget::Header(AMOUNT), 1)));
}

#[test]
fn without_opt_in_or_items_no_menu_opens() {
    let mut r = Rig::new(line_spec(), 3);
    let target = ContextTarget::Cell(r.pos(2, MEMO));
    let (opened, fx) = r.menu_on(target);
    assert!(!opened);
    assert_eq!(fx, Effects::default());
    assert_eq!(r.at(), (0, ACCOUNT), "the cursor did not move");
    let mut r = Rig::bulk(3);
    let mut mr = ModelRef::Edit(&mut r.m);
    let (ready, _) = r.core.prepare_menu(target, &mut mr, &r.l, r.fm);
    assert!(ready);
    assert!(
        !r.core.open_menu(target, Vec::new()),
        "an empty list opens nothing"
    );
    assert!(r.core.menu().is_none());
}

#[test]
fn a_menu_about_a_deleted_row_closes_on_resync() {
    let mut r = Rig::bulk(3);
    let key = r.m.key(1);
    assert!(r.menu_on(ContextTarget::Cell(r.pos(1, MEMO))).0);
    r.m.form.edit_step(|lines| lines.retain(|l| l.key != key));
    r.resync();
    assert!(r.core.menu().is_none());
}

// ---------------------------------------------------------- selection

#[test]
fn the_gutter_toggles_ranges_and_ctrl_toggles() {
    let mut r = Rig::bulk(6);
    assert!(changed(&r.gutter(1, Mods::NONE)));
    assert_eq!(r.selected(), [1]);
    assert_eq!(r.at(), (0, ACCOUNT), "the cursor stays");
    r.gutter(4, Mods::SHIFT);
    assert_eq!(r.selected(), [1, 2, 3, 4], "range from the anchor");
    r.gutter(2, Mods::SHIFT);
    assert_eq!(
        r.selected(),
        [1, 2],
        "the range shrinks from the same anchor"
    );
    r.gutter(5, Mods::CTRL);
    assert_eq!(r.selected(), [1, 2, 5]);
    r.gutter(1, Mods::NONE);
    assert_eq!(r.selected(), [2, 5], "a click toggles off");
    // The ENTRY row has no checkbox; a range never takes it.
    let entry = r.m.row_count() - 1;
    assert_eq!(r.gutter(entry, Mods::NONE), Effects::default());
    r.gutter(entry - 1, Mods::NONE);
    r.gutter(entry, Mods::SHIFT);
    assert!(!r.selected().contains(&entry));
}

#[test]
fn shift_click_without_an_anchor_ranges_from_the_cursor_row() {
    let mut r = Rig::bulk(5);
    let pos = r.pos(1, MEMO);
    r.input(GridInput::Click {
        pos,
        double: false,
        mods: Mods::NONE,
    });
    r.gutter(3, Mods::SHIFT);
    assert_eq!(r.selected(), [1, 2, 3]);
}

#[test]
fn the_gutter_needs_the_select_column() {
    let mut r = Rig::new(
        GridSpec {
            selection: SelectionMode::Multi,
            ..line_spec()
        },
        3,
    );
    assert_eq!(r.gutter(1, Mods::NONE), Effects::default());
    assert!(r.selected().is_empty());
}

#[test]
fn shift_arrows_extend_from_the_cursor_row() {
    let mut r = Rig::bulk(5);
    let fx = r.key(GridKey::Down, Mods::SHIFT);
    assert!(changed(&fx));
    assert_eq!(r.selected(), [0, 1]);
    assert_eq!(r.at(), (1, ACCOUNT), "the cursor moves like the arrow");
    r.key(GridKey::Down, Mods::SHIFT);
    r.key(GridKey::Down, Mods::SHIFT);
    assert_eq!(r.selected(), [0, 1, 2, 3]);
    r.key(GridKey::Up, Mods::SHIFT);
    assert_eq!(r.selected(), [0, 1, 2], "it shrinks back");
    // Down onto the ENTRY row never selects it.
    for _ in 0..4 {
        r.key(GridKey::Down, Mods::SHIFT);
    }
    let entry = r.m.row_count() - 1;
    assert_eq!(r.at().0, entry);
    assert_eq!(r.selected(), [0, 1, 2, 3, 4]);
}

#[test]
fn shift_arrows_leave_an_open_list_alone_and_need_the_select_column() {
    let mut r = Rig::bulk(4);
    r.input(GridInput::Text("5".into())); // the Account list opens
    assert!(r.core.edit().is_some_and(|e| e.popup.is_some()));
    let fx = r.key(GridKey::Down, Mods::SHIFT);
    assert!(!changed(&fx));
    assert!(r.selected().is_empty());
    assert_eq!(r.at(), (0, ACCOUNT));
    // A plain editable grid with multi-selection ignores Shift+Down.
    let mut plain = Rig::new(
        GridSpec {
            selection: SelectionMode::Multi,
            ..line_spec()
        },
        4,
    );
    plain.key(GridKey::Down, Mods::SHIFT);
    assert!(plain.selected().is_empty());
    assert_eq!(plain.at(), (0, ACCOUNT));
}

#[test]
fn select_all_skips_the_entry_row_and_clear_empties() {
    let mut r = Rig::bulk(4);
    let fx = r.cmd(GridCommand::SelectAll);
    assert!(fx.handled && changed(&fx));
    assert_eq!(r.selected(), [0, 1, 2, 3]);
    let again = r.cmd(GridCommand::SelectAll);
    assert!(again.handled && !changed(&again), "nothing new");
    let fx = r.cmd(GridCommand::ClearSelection);
    assert!(fx.handled && changed(&fx));
    assert!(r.selected().is_empty());
    assert!(!changed(&r.cmd(GridCommand::ClearSelection)));
    // No rows but the ENTRY row: a no-op.
    let mut empty = Rig::bulk(0);
    let fx = empty.cmd(GridCommand::SelectAll);
    assert!(fx.handled && !changed(&fx));
    assert!(empty.selected().is_empty());
    // Without multi-selection it is not the grid's.
    let mut line = Rig::new(line_spec(), 3);
    assert!(!line.cmd(GridCommand::SelectAll).handled);
}

#[test]
fn selection_survives_a_reorder_and_undo_prunes_it() {
    let mut r = Rig::bulk(5);
    r.gutter(1, Mods::NONE);
    r.gutter(3, Mods::CTRL);
    let keys: Vec<RowKey> = r.core.selected_keys().collect();
    // The screen reorders the rows (a sort): the same rows stay selected.
    r.m.form.edit_step(|lines| {
        let entry = lines.pop();
        lines.reverse();
        lines.extend(entry);
    });
    assert!(!changed(&r.resync()));
    assert_eq!(r.core.selected_keys().collect::<Vec<_>>(), keys);
    assert_eq!(r.selected(), [1, 3]);
    // Deleting one selected row (as Delete rows, then undo/redo) prunes it.
    let gone = keys[0];
    r.m.form.edit_step(|lines| lines.retain(|l| l.key != gone));
    assert!(changed(&r.resync()));
    assert_eq!(r.core.selected_keys().collect::<Vec<_>>(), [keys[1]]);
    assert!(r.m.form.undo());
    r.resync();
    assert_eq!(
        r.core.selected_keys().collect::<Vec<_>>(),
        [keys[1]],
        "a pruned key does not come back on undo"
    );
}

#[test]
fn reset_closes_the_menu_and_clears_the_selection() {
    let mut r = Rig::bulk(3);
    r.gutter(1, Mods::NONE);
    assert!(r.menu_on(ContextTarget::Cell(r.pos(1, MEMO))).0);
    let mut mr = ModelRef::Edit(&mut r.m);
    r.core.reset(&mut mr, r.fm);
    assert!(r.core.menu().is_none());
    assert!(r.selected().is_empty());
}
