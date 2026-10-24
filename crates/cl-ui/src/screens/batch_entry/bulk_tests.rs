//! Copy down, Columns… and the bulk actions (P16 steps 0f-0h; Q-272,
//! Q-273, Q-274) against the demo engine.

use std::collections::BTreeSet;
use std::sync::Arc;

use cl_grid::{CellInput, CellPos, LookupResult, Lookups, RowKey};
use cl_model::forms::batch::{ColumnChoice, ColumnError, CopyScope, Field, RowDraft, SectionKind};
use cl_model::{Key, Keymap, Mods, Shortcut};

use super::grid::column_of;
use super::*;
use crate::company::demo::account_id;
use crate::screens::register::lists::{NAMES, TAGS};

fn day(mo: u32, d: u32) -> CalendarDate {
    CalendarDate::from_ymd(2026, mo, d).expect("date")
}

fn m(s: &str) -> cl_core::Money {
    cl_core::Money::parse_wire(s).expect("money")
}

fn screen_in(dir: &std::path::Path) -> (BulkEntryScreen, Arc<CompanyLink>) {
    let link = Arc::new(CompanyLink::demo(dir));
    let s = BulkEntryScreen::new(Arc::clone(&link), None, day(11, 4));
    (s, link)
}

/// Three checks: (account, amount, memo).
fn three(s: &mut BulkEntryScreen) -> Vec<RowKey> {
    let rows = [
        (6100, "10.00", "Fuel"),
        (5000, "20.00", "Gravel"),
        (6100, "30.00", "Trees"),
    ];
    let today = s.today;
    let highest = s.grid.adapter().highest.clone();
    let mut keys = std::mem::take(&mut s.grid.adapter_mut().keys);
    let mut out = Vec::new();
    s.grid.edit_step(|b| {
        let sec = &mut b.sections[0];
        sec.rows.retain(|r| !r.is_blank());
        for (n, (account, amount, memo)) in rows.into_iter().enumerate() {
            let mut r = RowDraft::new(keys.next_key());
            r.date = Some(day(11, 2));
            r.account = Some(account_id(account));
            r.amount = Some(m(amount));
            r.memo = memo.into();
            r.number = format!("{}", 2001 + n);
            out.push(r.key);
            sec.rows.push(r);
        }
        grid::ensure_entry(b, 0, &mut keys, today, &highest);
    });
    s.grid.adapter_mut().keys = keys;
    out
}

fn row(s: &BulkEntryScreen, k: RowKey) -> RowDraft {
    s.batch().find(k).expect("row").1.clone()
}

fn first_key(s: &BulkEntryScreen, list: cl_grid::ListId) -> u64 {
    match s.books.lists.all(list) {
        LookupResult::Ready(v) => v.first().expect("an entry").id,
        LookupResult::Loading => panic!("lists not loaded"),
    }
}

#[test]
fn copy_down_sets_every_other_row_and_one_undo_takes_it_back() {
    let dir = tempfile::tempdir().expect("tmp");
    let (mut s, _link) = screen_in(dir.path());
    let k = three(&mut s);
    // Every row checked by the engine first.
    assert!(s.start_check(validate::CheckScope::Everything));
    assert!(s.batch().rows_to_check(&s.checks).is_empty());
    let hashes: Vec<_> = k.iter().map(|k| s.batch().row_hash(*k)).collect();
    let at = CellPos {
        row: k[1],
        col: column_of(Field::Account),
    };
    assert_eq!(s.copy_down_at(at, CopyScope::AllDisplayed), 2);
    assert_eq!(s.state_json()["rows_changed"], 2);
    for key in &k {
        assert_eq!(row(&s, *key).account, Some(account_id(5000)));
    }
    // The changed rows lost their engine answer and only they go again.
    assert_ne!(s.batch().row_hash(k[0]), hashes[0]);
    assert_eq!(s.batch().row_hash(k[1]), hashes[1]);
    assert_ne!(s.batch().row_hash(k[2]), hashes[2]);
    let want: BTreeSet<RowKey> = [k[0], k[2]].into();
    assert_eq!(s.batch().rows_to_check(&s.checks), want);
    assert!(s.touched.contains(&k[0]) && s.touched.contains(&k[2]));
    // No rows were added, the amounts are the owner's.
    assert_eq!(s.batch().filled_count(), 3);
    assert_eq!(row(&s, k[0]).amount, Some(m("10.00")));
    // One undo step.
    assert!(s.form_command(CommandId::new("edit.undo")));
    assert_eq!(row(&s, k[0]).account, Some(account_id(6100)));
    assert_eq!(row(&s, k[2]).account, Some(account_id(6100)));
}

#[test]
fn copy_to_rows_below_and_never_an_amount_or_a_number() {
    let dir = tempfile::tempdir().expect("tmp");
    let (mut s, _link) = screen_in(dir.path());
    let k = three(&mut s);
    let memo = |f| CellPos {
        row: k[1],
        col: column_of(f),
    };
    assert_eq!(s.copy_down_at(memo(Field::Memo), CopyScope::Below), 1);
    assert_eq!(row(&s, k[0]).memo, "Fuel", "above: untouched");
    assert_eq!(row(&s, k[2]).memo, "Gravel");
    assert_eq!(s.state_json()["last_action"], "copy_below");
    for f in [Field::Amount, Field::Number] {
        assert_eq!(s.copy_down_at(memo(f), CopyScope::AllDisplayed), 0);
    }
    assert_eq!(row(&s, k[0]).amount, Some(m("10.00")));
    assert_eq!(row(&s, k[0]).number, "2001");
    assert_eq!(
        s.bulk_note.as_deref(),
        Some("Amounts and check numbers are never copied.")
    );
    // Ctrl+D (the command) copies the cursor cell.
    s.grid
        .edit_step(|b| b.sections[0].rows[1].date = Some(day(11, 3)));
    s.grid
        .focus(memo(Field::Date), &*Arc::clone(&s.books.lists));
    assert!(s.form_command(CommandId::new("grid.copy_down")));
    assert_eq!(row(&s, k[0]).date, Some(day(11, 3)));
    assert_eq!(row(&s, k[2]).date, Some(day(11, 3)));
}

#[test]
fn ctrl_d_copies_down_only_in_the_grid_scope() {
    let km = Keymap::new(cl_model::commands::COMMANDS).expect("keymap");
    let ctrl = |c| Shortcut::new(Mods::CMD, Key::Letter(c));
    let at = |scope, c| km.resolve(scope, ctrl(c)).map(CommandId::as_str);
    assert_eq!(at(Scope::Grid, 'D'), Some("grid.copy_down"));
    // A form, or a grid that keeps the form scope (Write Check's lines).
    assert_eq!(at(Scope::Form, 'D'), Some("form.delete_item"));
    assert_eq!(at(Scope::Grid, 'A'), Some("grid.select_all"));
    // Bulk Entry asks for the grid scope only while its grid has the
    // keyboard.
    let dir = tempfile::tempdir().expect("tmp");
    let (mut s, _link) = screen_in(dir.path());
    let k = three(&mut s);
    assert_eq!(crate::windows::Screen::scope(&s), Scope::Form);
    let ctx = eframe::egui::Context::default();
    crate::theme::apply(&ctx, crate::theme::ThemeChoice::Light);
    crate::fonts::install(&ctx);
    s.grid.focus(
        CellPos {
            row: k[0],
            col: column_of(Field::Memo),
        },
        &*Arc::clone(&s.books.lists),
    );
    for _ in 0..3 {
        let mut out = ctx.run_ui(eframe::egui::RawInput::default(), |ui| {
            crate::windows::Screen::ui(&mut s, ui);
        });
        out.textures_delta.clear();
    }
    assert!(s.grid.grid().has_focus());
    assert_eq!(crate::windows::Screen::scope(&s), Scope::Grid);
    s.columns_open = true;
    assert_eq!(crate::windows::Screen::scope(&s), Scope::Form);
}

#[test]
fn required_columns_stay_and_the_choice_is_remembered_per_company() {
    let dir = tempfile::tempdir().expect("tmp");
    let (mut s, _link) = screen_in(dir.path());
    let k = three(&mut s);
    assert_eq!(
        s.hide_column(Field::Amount, true),
        Err(ColumnError::Required(Field::Amount))
    );
    assert_eq!(
        s.hide_column(Field::Contact, true),
        Err(ColumnError::Required(Field::Contact)),
        "a check needs a payee"
    );
    assert_eq!(s.hide_column(Field::Memo, true), Ok(()));
    assert!(s.move_column(Field::Tag, -2));
    let shown: Vec<Field> = s
        .grid
        .grid()
        .spec()
        .columns
        .iter()
        .filter_map(|c| grid::field_of(c.id))
        .collect();
    assert!(!shown.contains(&Field::Memo), "the grid lost the column");
    assert_eq!(shown, s.choice(SectionKind::Checks).visible());
    assert_eq!(
        s.state_json()["columns"]["hidden"],
        serde_json::json!(["memo"])
    );
    // The hidden memo still goes out.
    let out = s.batch().outgoing(None, None);
    let memos: Vec<_> = out.groups[0]
        .rows
        .iter()
        .map(|r| r.memo.as_deref())
        .collect();
    assert_eq!(memos, [Some("Fuel"), Some("Gravel"), Some("Trees")]);
    assert_eq!(row(&s, k[0]).memo, "Fuel");
    // Reopening the window: the same choice.
    let (again, _link) = screen_in(dir.path());
    assert_eq!(
        again.choice(SectionKind::Checks),
        s.choice(SectionKind::Checks)
    );
    assert!(
        !again
            .grid
            .grid()
            .spec()
            .columns
            .iter()
            .any(|c| c.id == column_of(Field::Memo))
    );
    assert_eq!(
        again.choice(SectionKind::Deposits),
        ColumnChoice::default_for(SectionKind::Deposits),
        "per section type"
    );
    // Reset to default, remembered too.
    s.reset_columns();
    let (third, _link) = screen_in(dir.path());
    assert_eq!(
        third.choice(SectionKind::Checks),
        ColumnChoice::default_for(SectionKind::Checks)
    );
}

#[test]
fn a_headerless_paste_maps_over_the_shown_columns_in_order() {
    let dir = tempfile::tempdir().expect("tmp");
    let (mut s, _link) = screen_in(dir.path());
    s.hide_column(Field::Number, true)
        .expect("No. may be hidden");
    s.open_mapping(
        "11/02/2026\t\tHollister Fuel Stop\t6100\t5.00\n",
        paste::Origin::Paste {
            at: None,
            column: 0,
        },
    );
    let p = s.mapping.as_ref().expect("panel");
    assert!(!p.first_row_headers);
    // Date · Print later · Pay to · Account · Amount (No. hidden).
    assert_eq!(p.mapping.get(&Field::Date), Some(&0));
    assert_eq!(p.mapping.get(&Field::Contact), Some(&2));
    assert_eq!(p.mapping.get(&Field::Account), Some(&3));
    assert_eq!(p.mapping.get(&Field::Amount), Some(&4));
    assert_eq!(p.mapping.get(&Field::Number), None);
}

#[test]
fn every_set_action_sets_the_selected_rows_in_one_undo_step() {
    let dir = tempfile::tempdir().expect("tmp");
    let (mut s, _link) = screen_in(dir.path());
    let k = three(&mut s);
    assert!(s.form_command(CommandId::new("grid.select_all")));
    assert_eq!(
        s.selected_rows(),
        k.iter().copied().collect::<BTreeSet<_>>()
    );
    assert_eq!(s.state_json()["selected"], 3, "not the blank entry row");
    let lists = Arc::clone(&s.books.lists);
    let account = lists.account_key(account_id(5200)).expect("5200");
    let name = first_key(&s, NAMES);
    let tag = first_key(&s, TAGS);
    let cases: Vec<(&str, CellInput)> = vec![
        ("batch.set_account", CellInput::Ref(account)),
        ("batch.set_name", CellInput::Ref(name)),
        ("batch.set_tag", CellInput::Ref(tag)),
        ("batch.set_date", CellInput::Date(day(11, 3))),
        ("batch.set_memo", CellInput::Text("Bulk".into())),
        ("batch.set_print_later", CellInput::Bool(true)),
    ];
    for (cmd, value) in cases {
        let before = s.batch().clone();
        assert!(s.form_command(CommandId::new(cmd)), "{cmd}");
        assert!(s.picker.is_some(), "{cmd}");
        s.picker_value(value);
        assert_eq!(s.apply_picker(), 3, "{cmd}");
        assert!(s.picker.is_none());
        for key in &k {
            let r = row(&s, *key);
            match cmd {
                "batch.set_account" => assert_eq!(r.account, Some(account_id(5200))),
                "batch.set_name" => assert_eq!(r.contact, lists.name(name)),
                "batch.set_tag" => assert_eq!(r.tag, lists.tag(tag)),
                "batch.set_date" => assert_eq!(r.date, Some(day(11, 3))),
                "batch.set_memo" => assert_eq!(r.memo, "Bulk"),
                _ => assert!(r.print_later),
            }
            // Never an amount or a number.
            assert_eq!(r.amount, before.find(*key).expect("row").1.amount);
            assert_eq!(r.number, before.find(*key).expect("row").1.number);
        }
        assert!(s.form_command(CommandId::new("edit.undo")), "{cmd}");
        assert_eq!(*s.batch(), before, "{cmd}: one undo step");
        assert!(s.form_command(CommandId::new("edit.redo")));
        assert_eq!(s.selected_rows().len(), 3, "the selection survives undo");
    }
    // Print later on numbered checks: their numbers stay (the rows say so).
    assert_eq!(row(&s, k[0]).number, "2001");
    // An empty account is refused; the picker stays with its reason.
    assert!(s.open_picker(Field::Account));
    assert_eq!(s.apply_picker(), 0);
    assert_eq!(
        s.picker.as_ref().and_then(|p| p.error),
        Some("batch_entry.set.empty")
    );
    // Esc closes it.
    assert!(s.form_command(CommandId::new("form.revert")));
    assert!(s.picker.is_none());
    // Not offered: amounts, and Print later outside checks.
    assert!(!s.open_picker(Field::Amount));
}

#[test]
fn setting_a_hidden_column_is_allowed_and_says_so() {
    let dir = tempfile::tempdir().expect("tmp");
    let (mut s, _link) = screen_in(dir.path());
    let k = three(&mut s);
    s.hide_column(Field::Memo, true).expect("hide");
    s.select_all();
    assert!(s.open_picker(Field::Memo));
    s.picker_value(CellInput::Text("Hidden".into()));
    assert_eq!(s.apply_picker(), 3);
    assert_eq!(row(&s, k[1]).memo, "Hidden");
    assert_eq!(
        s.bulk_note.as_deref(),
        Some("Set Memo on 3 rows. The Memo column is hidden.")
    );
}

#[test]
fn delete_rows_asks_first_and_undo_brings_them_back() {
    let dir = tempfile::tempdir().expect("tmp");
    let (mut s, _link) = screen_in(dir.path());
    let k = three(&mut s);
    // Nothing selected: nothing to ask.
    s.ask_delete();
    assert!(!s.confirm_delete);
    s.select_all();
    assert!(s.form_command(CommandId::new("batch.delete_rows")));
    assert_eq!(s.state_json()["confirm_delete"], true);
    // Esc keeps the rows.
    assert!(s.form_command(CommandId::new("form.revert")));
    assert!(!s.confirm_delete);
    assert_eq!(s.batch().filled_count(), 3);
    s.ask_delete();
    assert_eq!(s.delete_selected(), 3);
    assert_eq!(s.batch().filled_count(), 0);
    assert_eq!(s.selected_rows().len(), 0);
    assert_eq!(s.batch().sections[0].rows.len(), 1, "the entry row stays");
    assert!(s.form_command(CommandId::new("edit.undo")));
    assert_eq!(s.batch().filled_count(), 3);
    assert!(s.batch().find(k[1]).is_some());
}

#[test]
fn select_all_and_copy_down_do_nothing_on_an_empty_section() {
    let dir = tempfile::tempdir().expect("tmp");
    let (mut s, _link) = screen_in(dir.path());
    let rev = s.grid.form().revision();
    s.select_all();
    assert_eq!(s.selected_rows().len(), 0);
    assert_eq!(s.copy_down(CopyScope::AllDisplayed), 0);
    assert_eq!(s.grid.form().revision(), rev, "no undo step");
    let items = cl_grid::ContextMenuSource::items(
        &s.menu(),
        cl_grid::ContextTarget::Cell(CellPos {
            row: s.batch().sections[0].rows[0].key,
            col: column_of(Field::Memo),
        }),
    );
    assert!(items.iter().take(3).all(|i| !i.enabled), "{items:?}");
}

#[test]
fn the_window_draws_columns_and_the_selection_bar() {
    let dir = tempfile::tempdir().expect("tmp");
    let (mut s, _link) = screen_in(dir.path());
    three(&mut s);
    let ctx = eframe::egui::Context::default();
    crate::theme::apply(&ctx, crate::theme::ThemeChoice::Dark);
    crate::fonts::install(&ctx);
    let frame = |s: &mut BulkEntryScreen| {
        let mut out = ctx.run_ui(eframe::egui::RawInput::default(), |ui| {
            crate::windows::Screen::ui(s, ui);
        });
        out.textures_delta.clear();
    };
    s.select_all();
    assert!(s.open_picker(Field::Account));
    frame(&mut s);
    frame(&mut s);
    s.picker = None;
    s.ask_delete();
    frame(&mut s);
    assert!(s.form_command(CommandId::new("batch.columns")));
    assert_eq!(s.state_json()["columns_open"], true);
    frame(&mut s);
    frame(&mut s);
    assert!(s.form_command(CommandId::new("form.revert")));
    assert_eq!(s.state_json()["columns_open"], false);
}

/// Review P16-part1 #1: the real Ctrl+A key, through the shell's key
/// handling, selects every row from an empty cell; in a cell with text the
/// first Ctrl+A selects the text and the second selects the rows.
#[test]
fn ctrl_a_through_the_shell_selects_all_rows() {
    use eframe::egui::{self, Event, Modifiers};
    let ctrl = Modifiers {
        ctrl: true,
        command: true,
        ..Modifiers::NONE
    };
    let tap = |k: egui::Key| {
        vec![
            Event::Key {
                key: k,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: ctrl,
            },
            Event::Key {
                key: k,
                physical_key: None,
                pressed: false,
                repeat: false,
                modifiers: ctrl,
            },
        ]
    };
    for (cell, presses) in [("empty", 1), ("text", 2)] {
        let dir = tempfile::tempdir().expect("tmp");
        let (mut s, _link) = screen_in(dir.path());
        let k = three(&mut s);
        let entry = s.batch().sections[0].rows.last().expect("entry row").key;
        let row = if cell == "empty" { entry } else { k[0] };
        s.grid.focus(
            CellPos {
                row,
                col: column_of(Field::Memo),
            },
            &*Arc::clone(&s.books.lists),
        );
        let mut shell = crate::shell::Shell::new(crate::shell::ShellOptions {
            demo: true,
            ..crate::shell::ShellOptions::default()
        });
        let id = shell
            .windows
            .open_new(CommandId::new("banking.bulk_entry"), Box::new(s));
        let ctx = egui::Context::default();
        crate::shell::Shell::install(&ctx);
        cl_grid::sequencer::install(&ctx);
        let frame = |shell: &mut crate::shell::Shell, events: Vec<Event>| {
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1440.0, 940.0),
                )),
                events,
                ..egui::RawInput::default()
            };
            let mut out = ctx.run_ui(raw, |ui| shell.ui(ui));
            out.textures_delta.clear();
        };
        for _ in 0..4 {
            frame(&mut shell, Vec::new());
        }
        let state =
            |shell: &crate::shell::Shell| shell.windows.get(id).expect("window").screen.state();
        assert_eq!(state(&shell)["grid"]["focused"], true, "{cell}");
        assert_eq!(state(&shell)["selected"], 0);
        for n in 1..=presses {
            frame(&mut shell, tap(egui::Key::A));
            frame(&mut shell, Vec::new());
            let want = if n == presses { 3 } else { 0 };
            assert_eq!(state(&shell)["selected"], want, "{cell}: Ctrl+A #{n}");
        }
        assert_eq!(state(&shell)["last_action"], "select_all");
    }
}

/// Review P16-part1 #2: Ctrl+D on the blank entry row (its date carried
/// over) does nothing, greys the menu items and says why.
#[test]
fn copy_down_from_the_blank_entry_row_is_refused_in_plain_words() {
    let dir = tempfile::tempdir().expect("tmp");
    let (mut s, _link) = screen_in(dir.path());
    let k = three(&mut s);
    let entry = s.batch().sections[0].rows.last().expect("entry row").key;
    assert!(
        row(&s, entry).date.is_some(),
        "the entry row carries a date"
    );
    let before = s.batch().clone();
    let rev = s.grid.form().revision();
    for f in [Field::Date, Field::Memo, Field::Account] {
        let at = CellPos {
            row: entry,
            col: column_of(f),
        };
        for scope in [CopyScope::AllDisplayed, CopyScope::Below] {
            assert_eq!(s.copy_down_at(at, scope), 0, "{f:?}");
            assert_eq!(
                s.bulk_note.as_deref(),
                Some("This is the empty line for a new entry. Copy from a line that is filled in.")
            );
        }
        let items = cl_grid::ContextMenuSource::items(&s.menu(), cl_grid::ContextTarget::Cell(at));
        assert!(!items[0].enabled && !items[1].enabled, "{f:?}");
        assert!(
            items[0]
                .tooltip
                .as_deref()
                .is_some_and(|t| t.contains("empty line"))
        );
    }
    // Ctrl+D (the command) on the entry row's date: the same.
    s.grid.focus(
        CellPos {
            row: entry,
            col: column_of(Field::Date),
        },
        &*Arc::clone(&s.books.lists),
    );
    assert!(s.form_command(CommandId::new("grid.copy_down")));
    assert_eq!(*s.batch(), before, "no date or column changed");
    assert_eq!(s.grid.form().revision(), rev, "no undo step");
    assert_eq!(s.state_json()["last_action"], "copy_refused");
    // A row without an amount, and an empty cell, are refused too.
    s.grid.edit_step(|b| b.sections[0].rows[0].amount = None);
    let at = |f| CellPos {
        row: k[0],
        col: column_of(f),
    };
    assert_eq!(s.copy_down_at(at(Field::Memo), CopyScope::AllDisplayed), 0);
    assert_eq!(
        s.bulk_note.as_deref(),
        Some("Enter this line's amount first, then copy from it.")
    );
    assert_eq!(
        s.copy_down_at(
            CellPos {
                row: k[1],
                col: column_of(Field::Tag)
            },
            CopyScope::AllDisplayed
        ),
        0
    );
    assert_eq!(
        s.bulk_note.as_deref(),
        Some("This cell is empty, so there is nothing to copy.")
    );
}

/// Review P16-part1 #5: Set Date that would join two journal entries
/// leaves those rows and says so; the rest is one undo step.
#[test]
fn set_date_keeps_journal_entries_apart_and_says_so() {
    let dir = tempfile::tempdir().expect("tmp");
    let (mut s, _link) = screen_in(dir.path());
    let today = s.today;
    let highest = s.grid.adapter().highest.clone();
    let mut keys = std::mem::take(&mut s.grid.adapter_mut().keys);
    let mut k = Vec::new();
    s.grid.edit_step(|b| {
        let sec = &mut b.sections[0];
        sec.kind = SectionKind::JournalEntries;
        sec.context = None;
        sec.rows.clear();
        for (d, debit) in [(1, true), (1, false), (2, true), (2, false)] {
            let mut r = RowDraft::new(keys.next_key());
            r.date = Some(day(11, d));
            r.number = "1".into();
            r.account = Some(account_id(if debit { 6100 } else { 1010 }));
            if debit {
                r.debit = Some(m("5.00"));
            } else {
                r.credit = Some(m("5.00"));
            }
            k.push(r.key);
            sec.rows.push(r);
        }
        grid::ensure_entry(b, 0, &mut keys, today, &highest);
    });
    s.grid.adapter_mut().keys = keys;
    s.sync_shown_section();
    // All four rows: 11/01's lines already have the date; 11/02's would join.
    s.select_all();
    assert_eq!(s.selected_rows().len(), 4);
    let rev = s.grid.form().revision();
    assert!(s.open_picker(Field::Date));
    s.picker_value(CellInput::Date(day(11, 1)));
    assert_eq!(s.apply_picker(), 0);
    assert_eq!(s.state_json()["kept_apart"], 2);
    assert_eq!(
        row(&s, k[2]).date,
        Some(day(11, 2)),
        "entry 1 on 11/02 stays"
    );
    assert_eq!(
        s.grid.form().revision(),
        rev,
        "nothing changed, no undo step"
    );
    assert!(
        s.bulk_note
            .as_deref()
            .is_some_and(|n| n.contains("This would join two journal entries")),
        "{:?}",
        s.bulk_note
    );
}

/// Review P16-part1 #4: F8 to a problem in a hidden column shows the
/// column again and puts the cursor on the cell.
#[test]
fn f8_to_a_problem_in_a_hidden_column_shows_the_column() {
    let dir = tempfile::tempdir().expect("tmp");
    let (mut s, _link) = screen_in(dir.path());
    let k = three(&mut s);
    // Two checks numbered 2001: a problem on No.
    s.grid
        .edit_step(|b| b.sections[0].rows[1].number = "2001".into());
    s.hide_column(Field::Number, true)
        .expect("No. may be hidden");
    assert!(s.choice(SectionKind::Checks).is_hidden(Field::Number));
    s.touched.extend(k.iter().copied());
    s.cursor_row = None;
    s.refresh_looks();
    assert!(s.state_json()["problems"].as_u64().is_some_and(|n| n >= 1));
    assert!(s.form_command(CommandId::new("batch.next_problem")));
    assert!(
        !s.choice(SectionKind::Checks).is_hidden(Field::Number),
        "shown again"
    );
    let v = s.state_json();
    assert_eq!(v["grid"]["column"], "number");
    assert_eq!(v["columns"]["hidden"], serde_json::json!([]));
    assert_eq!(
        s.bulk_note.as_deref(),
        Some("The No. column is shown again: a row has a problem in it.")
    );
}
