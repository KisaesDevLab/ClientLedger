//! Keyboard through real egui frames (P10 plan Rev 2 §6): T1, T2, T10, T11
//! and T25. Input arrives in consecutive frames with no idle frames unless a
//! test says otherwise, the condition settle-frame harnesses could not see.

mod support;

use cl_grid::{RowModel, TextKey};
use egui::{Key, Modifiers};
use support::lines::{
    ACCOUNT, AMOUNT, BILLABLE, DATE, Lines, MEMO, TAGS, formats, line_spec, money,
};
use support::rig::{Rig, key, shift_tab, tab, typed};

fn rig(rows: usize) -> Rig {
    Rig::new(line_spec(), Lines::new(true, rows))
}

#[test]
fn nav_keys_in_consecutive_frames() {
    let mut r = rig(3);
    r.focus_cell(0, ACCOUNT);
    let focus = r.grid.focus_id();
    r.frame(tab());
    assert_eq!((r.at(), r.focused()), ((0, AMOUNT), Some(focus)));
    r.frame(tab());
    assert_eq!((r.at(), r.focused()), ((0, MEMO), Some(focus)));
    r.frame(key(Key::ArrowDown, Modifiers::NONE));
    assert_eq!((r.at(), r.focused()), ((1, MEMO), Some(focus)));
    let out = r.frame(key(Key::Escape, Modifiers::NONE));
    assert_eq!(
        r.focused(),
        Some(focus),
        "Esc did not drop egui focus (FULL filter)"
    );
    assert!(out.events.contains(&cl_grid::GridEvent::EscapeUnhandled));
    // Two Tabs in back-to-back frames: no cell skipped, none lost.
    r.frame(tab());
    r.frame(tab());
    assert_eq!(r.at(), (1, BILLABLE));
}

#[test]
fn gain_then_tab_in_the_next_frame_stays_in_the_grid() {
    let mut r = rig(2);
    r.focus_header();
    r.frame(tab()); // into the grid
    assert_eq!(r.focused(), Some(r.grid.focus_id()));
    assert_eq!(r.at(), (0, ACCOUNT));
    // The very next frame: egui has no lock filter for the grid yet, so the
    // sequencer holds this Tab for one frame.
    r.frame(tab());
    r.frame(vec![]);
    assert_eq!(
        r.focused(),
        Some(r.grid.focus_id()),
        "focus stayed in the grid"
    );
    assert_eq!(r.at(), (0, AMOUNT));
}

#[test]
fn batched_frame_preserves_order() {
    let mut r = rig(1);
    r.focus_cell(0, AMOUNT);
    let mut events = typed("842.15");
    events.extend(tab());
    events.extend(typed("Perennials"));
    events.extend(tab());
    events.extend(typed("t"));
    events.extend(tab());
    r.frame(events); // all in one RawInput
    r.idle();
    let line = &r.model.lines()[0];
    assert_eq!(line.amount, Some(money("842.15")));
    assert_eq!(line.memo, "Perennials");
    assert_eq!(line.date, Some(formats().today));
    assert_eq!(r.at(), (0, BILLABLE));
}

#[test]
fn shortcut_text_lands_in_the_cell_it_was_typed_in() {
    let mut r = rig(2);
    r.focus_cell(0, AMOUNT);
    let mut events = typed("12");
    events.extend(key(Key::ArrowDown, Modifiers::NONE));
    events.extend(typed("34"));
    r.frame(events);
    r.idle();
    assert_eq!(r.model.lines()[0].amount, Some(money("12")));
    assert_eq!(r.grid.core().edit().unwrap().buffer, "34");
    assert_eq!(r.at(), (1, AMOUNT));
}

#[test]
fn focus_id_stays_the_same_for_a_thousand_moves() {
    let mut r = rig(2_000);
    r.focus_cell(0, ACCOUNT);
    let focus = r.grid.focus_id();
    let moves = [
        key(Key::ArrowDown, Modifiers::NONE),
        tab(),
        key(Key::ArrowDown, Modifiers::NONE),
        shift_tab(),
        key(Key::PageDown, Modifiers::NONE),
        key(Key::ArrowUp, Modifiers::NONE),
    ];
    for i in 0..1_000 {
        r.frame(moves[i % moves.len()].clone());
        assert_eq!(r.focused(), Some(focus), "move {i}");
    }
    r.frame(key(Key::End, Modifiers::CTRL));
    assert_eq!(r.focused(), Some(focus));
    assert_eq!(r.at().0, 2_000, "the entry line");
    r.frame(key(Key::Home, Modifiers::CTRL));
    assert_eq!(r.at().0, 0);
    // Nothing else the grid registers can take focus (B1, T25).
    let key0 = r.model.key(0);
    for col in [ACCOUNT, MEMO, DATE, TAGS] {
        let id = r.grid.cell_id(cl_grid::CellPos {
            row: r.model.key(1),
            col,
        });
        let resp = r.ctx.read_response(id).expect("cell drawn");
        assert!(!resp.sense.is_focusable(), "{col:?} cell is focusable");
    }
    let bar = r
        .ctx
        .read_response(r.grid.id().with("scrollbar"))
        .expect("scrollbar");
    assert!(
        !bar.sense.is_focusable(),
        "the scrollbar is not in the Tab order"
    );
    let _ = key0;
}

#[test]
fn tab_in_and_out() {
    let mut r = rig(1);
    r.focus_header();
    r.frame(tab());
    assert_eq!(r.at(), (0, ACCOUNT), "Tab in: line 1, first editable cell");
    // To the entry line's last editable cell, then out.
    // This arrow comes in the frame right after the gain: the sequencer
    // holds it one frame (§2.4.3), so let that frame pass.
    r.frame(key(Key::ArrowDown, Modifiers::NONE));
    r.frame(vec![]);
    assert_eq!(r.at(), (1, ACCOUNT));
    for _ in 0..5 {
        r.frame(tab());
    }
    assert_eq!(r.at(), (1, TAGS));
    let out = r.frame(tab());
    assert!(
        out.events
            .contains(&cl_grid::GridEvent::LeftGrid { forward: true })
    );
    r.frame(vec![]);
    assert_eq!(
        r.focused(),
        Some(Rig::footer_id()),
        "Tab out reaches the next widget"
    );
    // Shift+Tab from the footer lands on the entry line's last cell.
    r.frame(shift_tab());
    r.frame(vec![]);
    r.frame(vec![]);
    assert_eq!(r.focused(), Some(r.grid.focus_id()));
    assert_eq!(r.at(), (1, TAGS));
    // Shift+Tab from row 0 column 0 reaches the header field.
    r.frame(key(Key::Home, Modifiers::CTRL));
    assert_eq!(r.at(), (0, TAGS));
    for _ in 0..5 {
        r.frame(shift_tab());
    }
    assert_eq!(r.at(), (0, ACCOUNT));
    let out = r.frame(shift_tab());
    assert!(
        out.events
            .contains(&cl_grid::GridEvent::LeftGrid { forward: false })
    );
    r.frame(vec![]);
    r.frame(vec![]);
    assert_eq!(r.focused(), Some(Rig::header_id()));
}

#[test]
fn calculator_caption_and_error_are_readable() {
    let mut r = rig(1);
    r.focus_cell(0, AMOUNT);
    r.frame(typed("400+44.25"));
    r.frame(vec![]);
    let labels: Vec<String> = r
        .nodes()
        .iter()
        .filter_map(|(_, n)| n.label().map(str::to_string))
        .collect();
    assert!(labels.iter().any(|l| l == "= 444.25"), "{labels:?}");
    r.frame(tab());
    assert_eq!(r.model.lines()[0].amount, Some(money("444.25")));
    r.frame(shift_tab());
    r.frame(typed("100/3"));
    r.frame(tab());
    r.frame(vec![]);
    let labels: Vec<String> = r
        .nodes()
        .iter()
        .filter_map(|(_, n)| n.label().map(str::to_string))
        .collect();
    assert!(
        labels.iter().any(|l| l.contains("33.33 or 33.34")),
        "{labels:?}"
    );
    assert_eq!(r.at(), (0, AMOUNT), "the invalid buffer kept the cursor");
    let _ = TextKey("unused");
    assert_eq!(r.model.row_count(), 2);
}
