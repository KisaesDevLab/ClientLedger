//! The editing state machine, row by row of the plan's §2.5 tables, in the
//! three presets (`line_entry`, `fixed_edit`, `browse`). T24's core part too.

// Terse emptiness checks read better in these tables.
#![allow(clippy::assert_is_empty)]

mod support;

use cl_grid::core::keys::{Formats, GridInput, GridKey, Mods};
use cl_grid::core::state::{CHANGED_UNDERNEATH, Core, Effects, PopupKind, ROW_GONE};
use cl_grid::{
    CellPos, GridCommand, GridEvent, GridSpec, ModelRef, RowKey, RowModel, Source, TextKey,
};
use support::KeysModel;
use support::lines::{
    ACCOUNT, AMOUNT, Account, BILLABLE, DATE, Lines, MEMO, TAGS, TestLookups, fixed_spec, formats,
    line_spec, money, move_formats,
};

struct Rig {
    core: Core,
    m: Lines,
    l: TestLookups,
    fm: Formats,
}

impl Rig {
    fn line(rows: usize) -> Rig {
        let mut r = Rig {
            core: Core::new(line_spec()),
            m: Lines::new(true, rows),
            l: TestLookups::default(),
            fm: formats(),
        };
        r.input(GridInput::FocusGained { from_below: false });
        r
    }

    fn fixed(rows: usize) -> Rig {
        let mut r = Rig {
            core: Core::new(fixed_spec()),
            m: Lines::new(false, rows),
            l: TestLookups::default(),
            fm: formats(),
        };
        r.input(GridInput::FocusGained { from_below: false });
        r
    }

    fn input(&mut self, i: GridInput) -> Effects {
        let mut mr = ModelRef::Edit(&mut self.m);
        self.core.handle(i, &mut mr, &self.l, self.fm)
    }
    fn key(&mut self, k: GridKey) -> Effects {
        self.input(GridInput::Key(k, Mods::NONE))
    }
    fn shift(&mut self, k: GridKey) -> Effects {
        self.input(GridInput::Key(k, Mods::SHIFT))
    }
    fn ctrl(&mut self, k: GridKey) -> Effects {
        self.input(GridInput::Key(k, Mods::CTRL))
    }
    fn typ(&mut self, t: &str) -> Effects {
        self.input(GridInput::Text(t.into()))
    }
    fn cmd(&mut self, c: GridCommand) -> Effects {
        self.input(GridInput::Command(c))
    }
    fn click(&mut self, row: usize, col: cl_grid::ColumnId) -> Effects {
        let pos = CellPos {
            row: self.m.key(row),
            col,
        };
        self.input(GridInput::Click {
            pos,
            double: false,
            mods: Mods::NONE,
        })
    }
    fn at(&self) -> (usize, cl_grid::ColumnId) {
        let c = self.core.cursor().expect("cursor");
        (self.m.index_of(c.row).expect("row"), c.col)
    }
    fn buffer(&self) -> String {
        self.core
            .edit()
            .map(|e| e.buffer.clone())
            .unwrap_or_default()
    }
    fn error(&self) -> Option<TextKey> {
        self.core.edit().and_then(|e| e.error).map(|p| p.key)
    }
    fn resync(&mut self, after_undo: bool) -> Effects {
        let mut mr = ModelRef::Edit(&mut self.m);
        self.core.resync(&mut mr, self.fm, after_undo)
    }
    fn commit_pending(&mut self) -> Result<(), cl_grid::CellProblem> {
        let mut mr = ModelRef::Edit(&mut self.m);
        self.core.commit_pending(&mut mr, &self.l, self.fm).0
    }
    /// Account 5100 and an amount on the cursor row, then back to Account.
    fn fill_row(&mut self, amount: &str) {
        self.typ("5100");
        self.key(GridKey::Tab);
        self.typ(amount);
        self.key(GridKey::Tab);
    }
}

fn has(fx: &Effects, f: impl Fn(&GridEvent) -> bool) -> bool {
    fx.events.iter().any(f)
}

#[test]
fn focus_in_lands_on_the_first_editable_cell() {
    let r = Rig::line(2);
    assert_eq!(r.at(), (0, ACCOUNT));
    assert!(r.core.has_focus());
    assert!(r.core.edit().unwrap().fresh, "text selected on entry");
    let mut below = Rig::line(2);
    below.input(GridInput::FocusLost);
    below.input(GridInput::FocusGained { from_below: true });
    assert_eq!(
        below.at(),
        (2, TAGS),
        "Shift+Tab in: entry line's last editable cell"
    );
}

#[test]
fn tab_commits_picker_then_amount_and_shift_tab_goes_back() {
    let mut r = Rig::line(1);
    r.typ("51");
    let popup = r.core.edit().unwrap().popup.clone().expect("popup");
    assert_eq!(popup.items[0].id, 5100, "5100 first (S6)");
    r.key(GridKey::Tab);
    assert_eq!(r.m.line(r.m.key(0)).account, Account::Id(5100));
    assert_eq!(r.m.set_calls.last().unwrap().1, Source::Picked);
    assert_eq!(r.at(), (0, AMOUNT));
    r.typ("842.15");
    let fx = r.key(GridKey::Tab);
    assert!(has(&fx, |e| matches!(e, GridEvent::CellCommitted { .. })));
    assert_eq!(r.m.line(r.m.key(0)).amount, Some(money("842.15")));
    assert_eq!(r.at(), (0, MEMO));
    r.shift(GridKey::Tab);
    assert_eq!(r.at(), (0, AMOUNT));
    assert_eq!(r.buffer(), "842.15");
}

#[test]
fn unchanged_cells_commit_nothing() {
    let mut r = Rig::line(1);
    r.key(GridKey::Tab);
    r.key(GridKey::Tab);
    assert!(r.m.set_calls.is_empty());
    assert!(!r.m.form.can_undo());
}

#[test]
fn invalid_buffer_stays_on_every_exit() {
    let mut r = Rig::line(2);
    r.key(GridKey::Tab);
    r.typ("5/0");
    for k in [GridKey::Tab, GridKey::Enter, GridKey::Down, GridKey::Up] {
        r.key(k);
        assert_eq!(r.at(), (0, AMOUNT), "{k:?}");
        assert_eq!(r.error(), Some(TextKey("grid.calc.divide_by_zero")));
    }
    r.shift(GridKey::Tab);
    assert_eq!(r.at(), (0, AMOUNT));
    r.click(1, MEMO);
    assert_eq!(r.at(), (0, AMOUNT), "a click elsewhere stays too (S11)");
    r.ctrl(GridKey::End);
    assert_eq!(r.at(), (0, AMOUNT));
    assert!(r.m.set_calls.is_empty());
    // Esc restores the origin and clears the error.
    r.cmd(GridCommand::Revert);
    assert_eq!((r.buffer(), r.error()), (String::new(), None));
}

#[test]
fn enter_records_and_moves_to_the_next_row() {
    let mut r = Rig::line(0);
    r.fill_row("10");
    let fx = r.key(GridKey::Enter);
    assert!(has(&fx, |e| matches!(e, GridEvent::RowRecorded { .. })));
    assert_eq!(r.at(), (1, ACCOUNT));
    assert_eq!(
        r.m.row_count(),
        2,
        "the entry line became a line; a new entry line follows"
    );
}

#[test]
fn enter_on_a_pristine_entry_line_asks_for_save_and_new() {
    let mut r = Rig::line(0);
    let fx = r.key(GridKey::Enter);
    assert!(has(&fx, |e| *e == GridEvent::EnterOnEmptyEntry));
    assert!(r.m.set_calls.is_empty());
}

#[test]
fn enter_on_an_invalid_row_goes_to_the_problem() {
    let mut r = Rig::line(1);
    r.key(GridKey::Tab);
    r.typ("5");
    let fx = r.key(GridKey::Enter);
    assert!(has(&fx, |e| matches!(e, GridEvent::RecordRefused { .. })));
    assert_eq!(r.at(), (0, ACCOUNT));
    assert_eq!(r.error(), Some(TextKey("test.account_needed")));
}

#[test]
fn enter_in_an_open_list_accepts_and_does_not_record() {
    let mut r = Rig::line(0);
    r.typ("Mulch");
    let fx = r.key(GridKey::Enter);
    assert!(!has(&fx, |e| matches!(e, GridEvent::RowRecorded { .. })));
    assert_eq!(r.m.line(r.m.key(0)).account, Account::Id(5110));
    assert_eq!(r.at(), (0, ACCOUNT));
    assert!(r.core.edit().unwrap().popup.is_none());
}

#[test]
fn shift_enter_records_too_when_enter_saves() {
    let mut r = Rig::line(0);
    r.fill_row("10");
    let fx = r.shift(GridKey::Enter);
    assert!(has(&fx, |e| matches!(e, GridEvent::RowRecorded { .. })));
    assert_eq!(r.at(), (1, ACCOUNT));
}

impl Rig {
    /// A line grid with "Enter moves between fields" on (Q-264).
    fn line_move(rows: usize) -> Rig {
        let mut r = Rig::line(rows);
        r.fm = move_formats();
        r
    }
}

#[test]
fn move_mode_enter_moves_across_cells_and_shift_enter_goes_back() {
    let mut r = Rig::line_move(1);
    r.typ("5100");
    let fx = r.key(GridKey::Enter);
    assert!(!has(&fx, |e| matches!(e, GridEvent::RowRecorded { .. })));
    assert_eq!(r.m.line(r.m.key(0)).account, Account::Id(5100));
    assert_eq!(r.at(), (0, AMOUNT));
    r.typ("10");
    r.key(GridKey::Enter);
    assert_eq!(r.m.line(r.m.key(0)).amount, Some(money("10")));
    assert_eq!(r.at(), (0, MEMO));
    r.shift(GridKey::Enter);
    assert_eq!(r.at(), (0, AMOUNT), "Shift+Enter is Shift+Tab");
    assert_eq!(r.buffer(), "10.00");
    // An invalid cell still keeps the cursor.
    r.typ("5/0");
    r.key(GridKey::Enter);
    assert_eq!(r.at(), (0, AMOUNT));
    assert_eq!(r.error(), Some(TextKey("grid.calc.divide_by_zero")));
}

#[test]
fn move_mode_enter_in_the_last_cell_records_and_opens_a_new_line() {
    let mut r = Rig::line_move(0);
    r.fill_row("10");
    let mut recorded_from = None;
    for _ in 0..10 {
        let before = r.at();
        let fx = r.key(GridKey::Enter);
        if has(&fx, |e| matches!(e, GridEvent::RowRecorded { .. })) {
            recorded_from = Some(before);
            break;
        }
    }
    assert_eq!(
        recorded_from,
        Some((0, TAGS)),
        "only the row's last editable cell records"
    );
    assert_eq!(r.at(), (1, ACCOUNT), "the new line's first cell");
    assert_eq!(r.m.row_count(), 2);
}

#[test]
fn move_mode_enter_never_saves_from_the_untouched_entry_line() {
    let mut r = Rig::line_move(0);
    let fx = r.key(GridKey::Enter);
    assert!(!has(&fx, |e| *e == GridEvent::EnterOnEmptyEntry));
    assert_eq!(r.at(), (0, AMOUNT));
    // The last cell of the untouched entry line leaves the grid like Tab.
    r.input(GridInput::FocusLost);
    r.input(GridInput::FocusGained { from_below: true });
    assert_eq!(r.at(), (0, TAGS));
    let fx = r.key(GridKey::Enter);
    assert!(!has(&fx, |e| *e == GridEvent::EnterOnEmptyEntry));
    assert!(!has(&fx, |e| matches!(e, GridEvent::RowRecorded { .. })));
    assert_eq!(fx.leave, Some(true));
    assert!(r.m.set_calls.is_empty());
}

#[test]
fn move_mode_enter_in_an_open_list_takes_the_entry_and_moves_on() {
    let mut r = Rig::line_move(0);
    r.typ("Mulch");
    let fx = r.key(GridKey::Enter);
    assert!(!has(&fx, |e| matches!(e, GridEvent::RowRecorded { .. })));
    assert_eq!(r.m.line(r.m.key(0)).account, Account::Id(5110));
    assert_eq!(r.at(), (0, AMOUNT), "like Tab (Q-P10c)");
}

#[test]
fn arrows_move_the_highlight_or_the_row() {
    let mut r = Rig::line(2);
    r.typ("5");
    r.key(GridKey::Down);
    assert_eq!(
        r.core.edit().unwrap().popup.as_ref().unwrap().highlight,
        Some(1)
    );
    r.key(GridKey::Up);
    r.key(GridKey::Up);
    assert_eq!(
        r.core.edit().unwrap().popup.as_ref().unwrap().highlight,
        Some(0)
    );
    r.cmd(GridCommand::Revert); // close the list
    r.cmd(GridCommand::Revert); // and the text
    r.key(GridKey::Down);
    assert_eq!(r.at(), (1, ACCOUNT));
    r.key(GridKey::Up);
    assert_eq!(r.at(), (0, ACCOUNT));
    r.key(GridKey::Up);
    assert_eq!(r.at(), (0, ACCOUNT), "clamped at the top");
}

#[test]
fn ctrl_home_end_and_paging() {
    let mut r = Rig::line(30);
    r.core.set_page_rows(10);
    r.ctrl(GridKey::End);
    assert_eq!(r.at(), (30, ACCOUNT));
    r.ctrl(GridKey::Home);
    assert_eq!(r.at().0, 0);
    r.cmd(GridCommand::PageDown);
    assert_eq!(r.at().0, 10);
    r.key(GridKey::PageDown);
    assert_eq!(r.at().0, 20);
    r.cmd(GridCommand::PageUp);
    assert_eq!(r.at().0, 10);
    // In an open list, paging moves the highlight.
    r.typ("6");
    r.cmd(GridCommand::PageDown);
    let p = r.core.edit().unwrap().popup.clone().unwrap();
    assert_eq!(p.highlight, Some(p.items.len() - 1));
    assert_eq!(r.at().0, 10);
}

#[test]
fn esc_chain_popup_cell_row_then_not_handled() {
    let mut r = Rig::line(1);
    r.key(GridKey::Tab);
    r.typ("12");
    r.key(GridKey::Tab); // commits 12 on row 0
    r.typ("memo");
    assert!(r.cmd(GridCommand::Revert).handled, "cell reverted");
    assert_eq!(r.buffer(), "");
    assert!(
        r.cmd(GridCommand::Revert).handled,
        "row reverted to its snapshot"
    );
    assert_eq!(r.m.line(r.m.key(0)).amount, None);
    assert!(
        !r.cmd(GridCommand::Revert).handled,
        "nothing left: the shell closes or prompts"
    );
    r.typ("x");
    // Popup first.
    r.click(0, ACCOUNT);
    r.typ("5");
    assert!(r.cmd(GridCommand::Revert).handled);
    assert!(r.core.edit().unwrap().popup.is_none());
    assert_eq!(r.buffer(), "5");
}

#[test]
fn raw_escape_falls_back_to_escape_unhandled() {
    let mut r = Rig::line(0);
    let fx = r.key(GridKey::Escape);
    assert!(has(&fx, |e| *e == GridEvent::EscapeUnhandled));
}

#[test]
fn insert_and_delete_lines() {
    let mut r = Rig::line(2);
    r.key(GridKey::Down); // row 1
    let before = r.m.key(1);
    assert!(r.cmd(GridCommand::InsertLine).handled);
    assert_eq!(r.at(), (1, ACCOUNT));
    assert_ne!(r.m.key(1), before);
    assert_eq!(r.m.key(2), before);
    assert!(r.cmd(GridCommand::DeleteLine).handled);
    assert_eq!(r.m.key(1), before);
    assert_eq!(r.at(), (1, ACCOUNT));
    // Delete on the pristine entry line does nothing.
    r.ctrl(GridKey::End);
    let n = r.m.row_count();
    assert!(r.cmd(GridCommand::DeleteLine).handled);
    assert_eq!(r.m.row_count(), n);
}

#[test]
fn insert_commits_a_valid_buffer_and_discards_an_invalid_one() {
    let mut r = Rig::line(1);
    r.key(GridKey::Tab);
    r.typ("7");
    r.cmd(GridCommand::InsertLine);
    assert_eq!(r.m.lines()[1].amount, Some(money("7")));
    r.key(GridKey::Tab);
    r.typ("1/0");
    r.cmd(GridCommand::InsertLine);
    assert_eq!(
        r.m.lines()[1].amount,
        None,
        "the invalid text was discarded"
    );
}

#[test]
fn locked_rows_never_edit() {
    let mut r = Rig::line(3);
    r.m.form.edit_step(|l| l[1].locked = true);
    r.key(GridKey::Down);
    assert_eq!(r.at(), (1, ACCOUNT));
    assert!(r.core.edit().is_none(), "no editor on a locked row");
    for fx in [
        r.typ("x"),
        r.cmd(GridCommand::InsertLine),
        r.cmd(GridCommand::DeleteLine),
    ] {
        assert!(has(&fx, |e| matches!(
            e,
            GridEvent::LockedRowEditAttempt { .. }
        )));
    }
    // Tab skips the locked row.
    r.key(GridKey::Up);
    for _ in 0..6 {
        r.key(GridKey::Tab);
    }
    assert_eq!(r.at(), (2, ACCOUNT));
    let fx = r.click(1, BILLABLE);
    assert!(has(&fx, |e| matches!(
        e,
        GridEvent::LockedRowEditAttempt { .. }
    )));
    assert!(!r.m.lines()[1].billable);
}

#[test]
fn open_list_only_for_pickers_and_tags() {
    let mut r = Rig::line(0);
    assert!(r.cmd(GridCommand::OpenList).handled);
    let p = r.core.edit().unwrap().popup.clone().unwrap();
    assert_eq!((p.kind, p.items.len()), (PopupKind::Full, 4));
    r.cmd(GridCommand::Revert);
    r.key(GridKey::Tab);
    assert!(!r.cmd(GridCommand::OpenList).handled);
}

/// Q-250: a click on a list cell opens its whole list at once, starting on
/// the cell's entry; Tab in never opens it; Esc closes it and changes
/// nothing; typing filters; the chevron toggles it.
#[test]
fn a_click_opens_the_list_and_tab_does_not() {
    let popup = |r: &Rig| r.core.edit().and_then(|e| e.popup.clone());
    let mut r = Rig::line(1);
    assert_eq!(r.at(), (0, ACCOUNT));
    assert!(popup(&r).is_none(), "focus in: no list");
    r.typ("5100");
    r.key(GridKey::Tab);
    assert_eq!(r.m.line(r.m.key(0)).account, Account::Id(5100));
    assert!(popup(&r).is_none(), "Tab onto Amount: no list");
    r.shift(GridKey::Tab);
    assert_eq!(r.at(), (0, ACCOUNT));
    assert!(popup(&r).is_none(), "Shift+Tab onto Account: no list");

    // A click elsewhere then on Account opens every entry on 5100.
    r.click(0, AMOUNT);
    assert!(popup(&r).is_none(), "amount cells have no list");
    r.click(0, ACCOUNT);
    let p = popup(&r).expect("a click opens the list");
    assert_eq!((p.kind, p.items.len()), (PopupKind::Browse, 4));
    assert_eq!(p.highlight.map(|i| p.items[i].id), Some(5100));

    // Esc closes it; the value and the model are untouched.
    let calls = r.m.set_calls.len();
    assert!(r.key(GridKey::Escape).handled);
    assert!(popup(&r).is_none());
    assert_eq!(r.m.line(r.m.key(0)).account, Account::Id(5100));
    assert_eq!(r.m.set_calls.len(), calls, "Esc changed nothing");

    // Opened by a click and left with Tab: nothing was chosen.
    r.click(0, AMOUNT);
    r.click(0, ACCOUNT);
    r.key(GridKey::Tab);
    assert_eq!(r.m.set_calls.len(), calls, "an unmoved list picks nothing");
    assert_eq!(r.at(), (0, AMOUNT));

    // Typing filters the open list.
    r.click(0, ACCOUNT);
    r.typ("Mul");
    assert_eq!(popup(&r).expect("list").kind, PopupKind::List);
    r.key(GridKey::Escape);
    r.key(GridKey::Escape);
    assert!(popup(&r).is_none());
    assert_eq!(r.m.set_calls.len(), calls, "Esc reverted the typing");

    // The chevron toggles; moving in the list makes it a choice.
    r.input(GridInput::ToggleList);
    assert_eq!(popup(&r).expect("list").kind, PopupKind::Browse);
    r.input(GridInput::ToggleList);
    assert!(popup(&r).is_none(), "the chevron closes an open list");
    r.input(GridInput::ToggleList);
    r.key(GridKey::Down);
    assert_eq!(popup(&r).expect("list").kind, PopupKind::Full);
}

#[test]
fn undo_restores_the_buffer_first_then_defers_to_the_form() {
    let mut r = Rig::line(0);
    r.key(GridKey::Tab);
    r.typ("9");
    assert!(r.cmd(GridCommand::Undo).handled);
    assert_eq!(r.buffer(), "");
    assert!(!r.cmd(GridCommand::Undo).handled);
    assert!(!r.cmd(GridCommand::Redo).handled);
}

#[test]
fn commit_pending_follows_the_tab_rule_but_never_accepts_a_suggestion() {
    let mut r = Rig::line(1);
    r.typ("Mul");
    assert_eq!(r.commit_pending(), Ok(()));
    assert_eq!(r.m.lines()[0].account, Account::Id(5110));
    r.key(GridKey::Tab);
    r.typ("1/0");
    assert_eq!(
        r.commit_pending().unwrap_err().key,
        TextKey("grid.calc.divide_by_zero")
    );
    assert_eq!(r.at(), (0, AMOUNT));
    // A suggested account is never taken by a save.
    let mut s = Rig::line(1);
    s.m.form.edit_step(|l| l[0].suggested = Some(6200));
    s.resync(false);
    assert_eq!(s.commit_pending(), Ok(()));
    assert_eq!(s.m.lines()[0].account, Account::None);
}

#[test]
fn suggestions_are_accepted_only_by_tab() {
    for (name, act) in [
        ("enter", 0),
        ("shift-tab", 1),
        ("down", 2),
        ("click", 3),
        ("focus-lost", 4),
    ] {
        let mut r = Rig::line(2);
        r.m.form.edit_step(|l| l[0].suggested = Some(6200));
        r.resync(false);
        match act {
            0 => drop(r.key(GridKey::Enter)),
            1 => drop(r.shift(GridKey::Tab)),
            2 => drop(r.key(GridKey::Down)),
            3 => drop(r.click(1, MEMO)),
            _ => drop(r.input(GridInput::FocusLost)),
        }
        assert_eq!(r.m.lines()[0].account, Account::None, "{name}");
    }
    let mut r = Rig::line(1);
    r.m.form.edit_step(|l| l[0].suggested = Some(6200));
    r.resync(false);
    r.key(GridKey::Tab);
    assert_eq!(r.m.lines()[0].account, Account::Id(6200));
    assert_eq!(r.m.set_calls.last().unwrap().1, Source::SuggestionAccepted);
}

#[test]
fn focus_loss_commits_or_parks() {
    let mut r = Rig::line(0);
    r.key(GridKey::Tab);
    r.typ("3");
    r.input(GridInput::FocusLost);
    assert_eq!(r.m.lines()[0].amount, Some(money("3")));
    assert!(!r.core.has_focus());
    r.input(GridInput::FocusGained { from_below: false });
    r.typ("3+");
    r.input(GridInput::FocusLost);
    assert!(r.core.is_parked());
    r.input(GridInput::FocusGained { from_below: false });
    assert!(!r.core.is_parked());
    assert_eq!(r.buffer(), "3+", "the parked text is back");
    assert_eq!(r.at(), (0, AMOUNT));
}

#[test]
fn undo_across_rows_resyncs_the_editor() {
    let mut r = Rig::line(2);
    r.key(GridKey::Tab);
    r.typ("5");
    r.key(GridKey::Down); // commits 5 on row 0, editor on row 1 amount
    r.typ("6");
    r.key(GridKey::Up); // commits 6 on row 1, editor on row 0 showing 5.00
    assert_eq!(r.buffer(), "5.00");
    // Form undo removes row 1's value; the editor on row 0 is unchanged.
    assert!(r.m.form.undo());
    let fx = r.resync(true);
    assert!(!fx.reset_editor);
    assert!(r.m.form.undo()); // removes 5 on row 0, under the open editor
    let fx = r.resync(true);
    assert!(fx.reset_editor);
    assert_eq!(
        r.buffer(),
        "",
        "the editor shows the model value after undo"
    );
    // Tab now commits nothing.
    let calls = r.m.set_calls.len();
    r.key(GridKey::Tab);
    assert_eq!(r.m.set_calls.len(), calls);
    assert!(r.m.form.redo());
    r.shift(GridKey::Tab);
    r.resync(true);
    assert_eq!(r.buffer(), "5.00");
}

#[test]
fn a_dirty_buffer_survives_a_change_underneath() {
    let mut r = Rig::line(1);
    r.key(GridKey::Tab);
    r.typ("7");
    r.m.form.edit_step(|l| l[0].amount = Some(money("99")));
    r.resync(false);
    let e = r.core.edit().unwrap();
    assert_eq!(e.buffer, "7");
    assert_eq!(e.caption, Some(CHANGED_UNDERNEATH));
    assert_eq!(e.origin, "99.00");
}

#[test]
fn a_vanished_cursor_row_moves_to_the_nearest_and_says_so() {
    let mut r = Rig::line(3);
    r.key(GridKey::Down);
    r.key(GridKey::Down);
    let gone = r.m.key(2);
    r.m.form.edit_step(|l| l.retain(|x| x.key != gone));
    let fx = r.resync(false);
    assert!(has(&fx, |e| matches!(e, GridEvent::RowGone { .. })));
    assert!(!has(&fx, |e| matches!(e, GridEvent::RecordRefused { .. })));
    assert_eq!(r.core.notice(), Some(ROW_GONE));
    assert_eq!(r.at().0, 2);
}

#[test]
fn rebind_after_save_keeps_the_cursor_quietly() {
    let mut r = Rig::line(2);
    r.key(GridKey::Down);
    let stored = r.m.lines().to_vec();
    r.m.form.save_ok(stored);
    let mut mr = ModelRef::Edit(&mut r.m);
    let fx = r.core.rebind(&mut mr, formats());
    assert!(fx.events.is_empty());
    assert_eq!(r.core.notice(), None);
    assert_eq!(r.at(), (1, ACCOUNT));
}

/// S1: loading a different record (Save & New, Prev/Next, Clear) puts the
/// cursor on row 0's first editable cell with no notice and no events, even
/// though every old row key is gone.
#[test]
fn reset_after_loading_another_record_is_quiet() {
    let mut r = Rig::line(3);
    r.key(GridKey::Down);
    r.key(GridKey::Down);
    r.typ("12");
    let fresh: Vec<_> = (0..2)
        .map(|_| support::lines::LineValue::blank(r.m.keygen.next_key()))
        .collect();
    r.m.form.load(fresh);
    let mut mr = ModelRef::Edit(&mut r.m);
    let fx = r.core.reset(&mut mr, formats());
    assert!(fx.events.is_empty(), "{:?}", fx.events);
    assert_eq!(r.core.notice(), None);
    assert_eq!(r.at(), (0, ACCOUNT));
    assert!(!r.core.has_uncommitted_edit());
    assert!(r.core.pending().is_empty());
    assert!(r.core.selection().is_empty());
}

#[test]
fn keys_typed_while_a_row_is_pending_land_in_the_next_row() {
    let mut spec = line_spec();
    spec.async_record = true;
    let mut r = Rig {
        core: Core::new(spec),
        m: Lines::new(true, 0),
        l: TestLookups::default(),
        fm: formats(),
    };
    r.m.async_records = true;
    r.input(GridInput::FocusGained { from_below: false });
    r.fill_row("10");
    let first = r.m.key(0);
    r.key(GridKey::Enter);
    assert!(r.core.pending().contains(&first));
    assert_eq!(r.at(), (1, ACCOUNT), "the cursor moved on optimistically");
    r.typ("6200");
    r.key(GridKey::Tab);
    assert_eq!(r.m.lines()[1].account, Account::Id(6200));
    // The pending row is locked meanwhile.
    r.click(0, MEMO);
    let fx = r.typ("x");
    assert!(has(&fx, |e| matches!(
        e,
        GridEvent::LockedRowEditAttempt { .. }
    )));
    let fx = r.core.record_finished(first, Ok(None));
    assert!(has(&fx, |e| matches!(e, GridEvent::RowRecorded { .. })));
    assert!(r.core.pending().is_empty());
    let fx = r
        .core
        .record_finished(first, Err(vec![cl_grid::CellProblem::new(TextKey("e"))]));
    assert!(has(&fx, |e| matches!(e, GridEvent::RecordRefused { .. })));
}

#[test]
fn tab_out_and_shift_tab_out() {
    let mut r = Rig::line(0);
    r.shift(GridKey::Tab);
    assert!(!r.core.has_focus(), "Shift+Tab at row 0 col 0 leaves");
    let mut r = Rig::line(0);
    for _ in 0..5 {
        r.key(GridKey::Tab);
    }
    assert_eq!(r.at(), (0, TAGS));
    let fx = r.key(GridKey::Tab);
    assert_eq!(
        fx.leave,
        Some(true),
        "Tab on the pristine entry line's last cell leaves"
    );
    assert!(has(&fx, |e| *e == GridEvent::LeftGrid { forward: true }));
    // A filled entry line wraps to the new entry line instead.
    let mut r = Rig::line(0);
    r.typ("6200");
    for _ in 0..5 {
        r.key(GridKey::Tab);
    }
    let fx = r.key(GridKey::Tab);
    assert_eq!(fx.leave, None);
    assert_eq!(r.at(), (1, ACCOUNT));
}

#[test]
fn date_keys_and_short_years() {
    let mut r = Rig::line(0);
    r.click(0, DATE);
    r.typ("t");
    assert_eq!(r.buffer(), "11/04/2026");
    r.typ("+");
    assert_eq!(r.buffer(), "11/05/2026");
    r.typ("h");
    assert_eq!(r.buffer(), "11/30/2026");
    r.input(GridInput::BufferEdited("1/5/99".into()));
    r.cmd(GridCommand::Commit);
    assert_eq!(
        r.buffer(),
        "01/05/2099",
        "the full year shows after commit (N2)"
    );
    r.typ("y");
    assert_eq!(r.buffer(), "01/01/2099");
}

#[test]
fn calculator_commits_the_result_with_its_source() {
    let mut r = Rig::line(0);
    r.key(GridKey::Tab);
    r.typ("400+44.25");
    r.cmd(GridCommand::Commit);
    assert_eq!(r.buffer(), "444.25");
    assert_eq!(r.m.lines()[0].amount, Some(money("444.25")));
    assert_eq!(r.m.set_calls.last().unwrap().1, Source::Calculator);
}

#[test]
fn tags_replace_within_a_group_and_backspace_removes() {
    let mut r = Rig::line(0);
    r.click(0, TAGS);
    r.typ("Crew A");
    r.key(GridKey::Enter);
    r.typ("Crew B");
    r.key(GridKey::Enter);
    let tags: Vec<String> = r.m.lines()[0]
        .tags
        .iter()
        .map(|t| t.label.clone())
        .collect();
    assert_eq!(tags, ["Crew B"]);
    assert_eq!(
        r.core.edit().unwrap().caption,
        Some(TextKey("grid.tags.replaced"))
    );
    r.typ("Install");
    r.key(GridKey::Tab); // exact match on Tab adds it
    r.shift(GridKey::Tab);
    assert_eq!(r.m.lines()[0].tags.len(), 2);
    r.key(GridKey::Backspace);
    assert_eq!(r.m.lines()[0].tags.len(), 1);
    r.typ("Nope");
    r.key(GridKey::Tab);
    assert_eq!(r.error(), Some(TextKey("grid.tags.not_in_list")));
}

#[test]
fn checkbox_toggles_by_space_and_click() {
    let mut r = Rig::line(1);
    r.click(0, BILLABLE);
    assert!(r.m.lines()[0].billable, "a click toggles");
    r.key(GridKey::Space);
    assert!(!r.m.lines()[0].billable);
    r.typ(" ");
    assert!(
        r.m.lines()[0].billable,
        "the Space text event toggles once too"
    );
}

#[test]
fn unresolved_picker_text_and_add_new() {
    let mut r = Rig::line(0);
    r.typ("Gas");
    let p = r.core.edit().unwrap().popup.clone().unwrap();
    assert_eq!(p.add_new.as_deref(), Some("Gas"));
    let fx = r.input(GridInput::AddNewPicked);
    assert!(has(&fx, |e| matches!(e, GridEvent::AddNew { .. })));
    assert_eq!(r.m.lines()[0].account, Account::Unresolved("Gas".into()));
    assert_eq!(r.error(), Some(TextKey("grid.picker.not_in_list")));
    // N5: Esc on the clean cell does not hide the problem while the value
    // is unresolved (here the row has nothing to revert to).
    let mut esc = Rig::line(0);
    esc.typ("Gas");
    esc.input(GridInput::AddNewPicked);
    esc.m.snapshot = Some((esc.m.key(0), esc.m.lines()[0].clone()));
    esc.cmd(GridCommand::Revert);
    assert_eq!(esc.m.lines()[0].account, Account::Unresolved("Gas".into()));
    assert_eq!(esc.error(), Some(TextKey("grid.picker.not_in_list")));
    let pos = r.core.cursor().unwrap();
    let mut mr = ModelRef::Edit(&mut r.m);
    r.core.resolve_pending(pos, 6210, &mut mr, formats());
    assert_eq!(r.m.lines()[0].account, Account::Id(6210));
    assert_eq!(r.error(), None);
    // Tab with no match keeps the text unresolved and moves on.
    let mut r = Rig::line(1);
    r.typ("zzz");
    r.key(GridKey::Tab);
    assert_eq!(r.m.lines()[0].account, Account::Unresolved("zzz".into()));
    assert_eq!(r.at(), (0, AMOUNT));
    // While the list is loading, Tab keeps the text too.
    let mut r = Rig::line(1);
    r.l.loading = true;
    r.typ("Fuel");
    assert!(r.core.edit().unwrap().popup.as_ref().unwrap().loading);
    r.key(GridKey::Tab);
    assert_eq!(
        r.m.lines()[0].account,
        Account::Id(6200),
        "an exact match still resolves"
    );
}

#[test]
fn fixed_edit_has_no_entry_line_or_insert_delete() {
    let mut r = Rig::fixed(3);
    assert!(!r.cmd(GridCommand::InsertLine).handled);
    assert!(!r.cmd(GridCommand::DeleteLine).handled);
    assert_eq!(r.m.row_count(), 3);
    r.ctrl(GridKey::End);
    for _ in 0..5 {
        r.key(GridKey::Tab);
    }
    let fx = r.key(GridKey::Tab);
    assert_eq!(fx.leave, Some(true), "Tab after the last cell leaves");
    // Space on a checkbox toggles it; selection is a separate thing.
    let mut r = Rig::fixed(2);
    r.click(0, BILLABLE);
    r.key(GridKey::Space);
    assert!(!r.m.lines()[0].billable);
}

// ---- Browse ---------------------------------------------------------------

struct Browse {
    core: Core,
    m: KeysModel,
}

impl Browse {
    fn new(n: u64) -> Browse {
        let spec = GridSpec::browse(vec![
            support::lines::columns()[6], // the read-only Balance column
        ]);
        let mut b = Browse {
            core: Core::new(spec),
            m: KeysModel::with_keys(n),
        };
        b.go(GridInput::FocusGained { from_below: false });
        b
    }
    fn go(&mut self, i: GridInput) -> Effects {
        let mut mr = ModelRef::Browse(&mut self.m);
        self.core.handle(i, &mut mr, &cl_grid::NoLookups, formats())
    }
    fn row(&self) -> u64 {
        self.core.cursor().unwrap().row.0
    }
}

#[test]
fn browse_navigation_selection_and_events() {
    let mut b = Browse::new(50);
    assert_eq!(b.row(), 1);
    b.go(GridInput::Key(GridKey::Down, Mods::NONE));
    assert_eq!(b.row(), 2);
    b.go(GridInput::Key(GridKey::Space, Mods::NONE));
    b.go(GridInput::Key(GridKey::Down, Mods::SHIFT));
    b.go(GridInput::Key(GridKey::Down, Mods::SHIFT));
    assert_eq!(b.core.selection().len(), 3);
    b.core.set_page_rows(10);
    b.go(GridInput::Key(GridKey::PageDown, Mods::NONE));
    assert_eq!(b.row(), 14);
    b.go(GridInput::Key(GridKey::End, Mods::CTRL));
    assert_eq!(b.row(), 50);
    let fx = b.go(GridInput::Key(GridKey::Enter, Mods::NONE));
    assert!(has(&fx, |e| *e == GridEvent::Activate { key: RowKey(50) }));
    let fx = b.go(GridInput::Text("j".into()));
    assert!(has(&fx, |e| *e == GridEvent::TypedInBrowse("j".into())));
    let fx = b.go(GridInput::Command(GridCommand::Revert));
    assert!(fx.handled, "Esc clears the selection");
    assert!(b.core.selection().is_empty());
    let fx = b.go(GridInput::Command(GridCommand::Revert));
    assert!(!fx.handled, "then the shell decides");
    let fx = b.go(GridInput::Key(GridKey::Escape, Mods::NONE));
    assert!(has(&fx, |e| *e == GridEvent::EscapeUnhandled));
    let fx = b.go(GridInput::Command(GridCommand::Copy));
    assert_eq!(fx.copy.as_deref(), Some(""));
    let fx = b.go(GridInput::Click {
        pos: CellPos {
            row: RowKey(7),
            col: support::lines::columns()[6].id,
        },
        double: true,
        mods: Mods::CTRL,
    });
    assert!(has(&fx, |e| *e == GridEvent::Activate { key: RowKey(7) }));
    assert!(b.core.selection().contains(RowKey(7)));
    b.go(GridInput::Click {
        pos: CellPos {
            row: RowKey(10),
            col: support::lines::columns()[6].id,
        },
        double: false,
        mods: Mods::SHIFT,
    });
    assert_eq!(b.core.selection().len(), 4);
    let fx = b.go(GridInput::Key(GridKey::Tab, Mods::SHIFT));
    assert_eq!(fx.leave, Some(false));
    assert!(b.core.edit().is_none(), "Browse never edits");
}

proptest::proptest! {
    /// S3: Shift+Down n times then Shift+Up m times selects exactly the
    /// rows between the starting row and the cursor: |n - m| + 1 of them.
    #[test]
    fn shift_ranges_grow_and_shrink(start in 0usize..10, n in 0usize..20, m in 0usize..20) {
        proptest::prop_assume!(n + m > 0);
        let mut b = Browse::new(60);
        for _ in 0..(20 + start) {
            b.go(GridInput::Key(GridKey::Down, Mods::NONE));
        }
        let origin = b.row();
        for _ in 0..n {
            b.go(GridInput::Key(GridKey::Down, Mods::SHIFT));
        }
        for _ in 0..m {
            b.go(GridInput::Key(GridKey::Up, Mods::SHIFT));
        }
        proptest::prop_assert_eq!(b.core.selection().len(), n.abs_diff(m) + 1);
        let cursor = b.row();
        let (lo, hi) = (origin.min(cursor), origin.max(cursor));
        for k in lo..=hi {
            proptest::prop_assert!(b.core.selection().contains(RowKey(k)));
        }
    }
}

/// S3: a Shift extension never deselects rows chosen before it, and a
/// Shift+click with no anchor starts the range at the cursor row.
#[test]
fn shift_extension_keeps_earlier_picks() {
    let mut b = Browse::new(30);
    b.go(GridInput::Key(GridKey::Space, Mods::NONE)); // row 1, anchor 1
    for _ in 0..4 {
        b.go(GridInput::Key(GridKey::Down, Mods::NONE));
    }
    b.go(GridInput::Key(GridKey::Space, Mods::NONE)); // row 5, anchor 5
    b.go(GridInput::Key(GridKey::Up, Mods::SHIFT));
    b.go(GridInput::Key(GridKey::Up, Mods::SHIFT)); // 3..=5
    b.go(GridInput::Key(GridKey::Down, Mods::SHIFT)); // 4..=5
    let got: Vec<u64> = b.core.selection().iter().map(|k| k.0).collect();
    assert_eq!(got, [1, 4, 5]);
    // No anchor: Shift+Down from row 10 selects rows 10 and 11.
    let mut c = Browse::new(30);
    for _ in 0..9 {
        c.go(GridInput::Key(GridKey::Down, Mods::NONE));
    }
    c.go(GridInput::Key(GridKey::Down, Mods::SHIFT));
    let got: Vec<u64> = c.core.selection().iter().map(|k| k.0).collect();
    assert_eq!(got, [10, 11]);
}

/// A read-only computed money column (a register's running balance, fed
/// by the screen or the engine): never opens an editor, Tab skips it, and
/// typing on it changes nothing.
#[test]
fn computed_money_column_is_locked() {
    let mut spec = line_spec();
    for c in &mut spec.columns {
        if c.id == support::lines::BALANCE {
            c.kind = cl_grid::CellKind::Amount(cl_model::amount_entry::AmountRules::MONEY);
            c.align = cl_grid::Align::Right;
            c.mono = true;
        }
    }
    let mut r = Rig {
        core: Core::new(spec),
        m: Lines::new(true, 2),
        l: TestLookups::default(),
        fm: formats(),
    };
    r.input(GridInput::FocusGained { from_below: false });
    r.click(0, TAGS);
    r.key(GridKey::Tab);
    assert_eq!(r.at(), (1, ACCOUNT), "Tab skips the balance");
    r.click(0, support::lines::BALANCE);
    assert!(r.core.edit().is_none(), "no editor on the balance");
    r.typ("5");
    assert!(r.m.set_calls.is_empty());
}

/// N2: Debug output never carries typed text, memos, amounts or names.
#[test]
fn debug_output_is_redacted() {
    let mut r = Rig::line(1);
    r.key(GridKey::Tab);
    r.key(GridKey::Tab);
    r.typ("Hollister 842.15 secret");
    let core = format!("{:?}", r.core);
    assert!(
        !core.contains("Hollister") && !core.contains("842"),
        "{core}"
    );
    let input = format!("{:?}", GridInput::Text("Hollister 842.15".into()));
    assert_eq!(input, "Text(..)");
    let edited = format!("{:?}", GridInput::BufferEdited("842.15".into()));
    assert!(!edited.contains("842"));
    let fx = Effects {
        copy: Some("Hollister 842.15".into()),
        ..Effects::default()
    };
    assert!(!format!("{fx:?}").contains("Hollister"));
    let choices = GridInput::OpenChoices(vec![cl_grid::Suggestion {
        id: 1,
        label: "Hollister Family".into(),
        group: None,
    }]);
    assert!(!format!("{choices:?}").contains("Hollister"));
}
