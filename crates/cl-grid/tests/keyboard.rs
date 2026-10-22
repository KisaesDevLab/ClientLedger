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
    r.frame(vec![]);
    let labels: Vec<String> = r
        .nodes()
        .iter()
        .filter_map(|(_, n)| n.label().map(str::to_string))
        .collect();
    assert!(labels.iter().any(|l| l == "= 33.33"), "{labels:?}");
    // Rounded to the cent, half away from zero (Q-P10x), and committed.
    r.frame(tab());
    assert_eq!(r.model.lines()[0].amount, Some(money("33.33")));
    assert_eq!(r.at(), (0, MEMO));
    r.frame(shift_tab());
    r.frame(typed("5/0"));
    r.frame(tab());
    assert_eq!(r.at(), (0, AMOUNT), "an invalid buffer keeps the cursor");
    let _ = TextKey("unused");
    assert_eq!(r.model.row_count(), 2);
}

/// Ids registered as focusable in the last pass (what egui adds to its
/// focus cache, `Memory::interested_in_focus`).
fn focusable_ids(r: &Rig) -> std::collections::BTreeSet<u64> {
    r.ctx.viewport(|v| {
        [&v.prev_pass.widgets, &v.this_pass.widgets]
            .into_iter()
            .flat_map(|w| {
                w.layers()
                    .flat_map(|(_, rects)| rects.iter())
                    .filter(|w| w.enabled && w.sense.is_focusable())
                    .map(|w| w.id.value())
                    .collect::<Vec<_>>()
            })
            .collect()
    })
}

/// B1/F4: egui's focus cache keeps every focusable id it sees. Across a
/// full scroll and a thousand moves the grid registers exactly one, its
/// focus id, next to the header and footer fields.
#[test]
fn focus_cache_stays_bounded() {
    let mut r = rig(2_000);
    r.focus_cell(0, ACCOUNT);
    let mut seen = focusable_ids(&r);
    for i in 0..1_000 {
        let k = if i < 400 { Key::PageDown } else { Key::ArrowUp };
        r.frame(key(k, Modifiers::NONE));
        seen.extend(focusable_ids(&r));
    }
    r.frame(key(Key::End, Modifiers::CTRL));
    seen.extend(focusable_ids(&r));
    let want: std::collections::BTreeSet<u64> =
        [r.grid.focus_id(), Rig::header_id(), Rig::footer_id()]
            .iter()
            .map(egui::Id::value)
            .collect();
    assert_eq!(seen, want, "only the grid's focus id and the two fields");
}

/// T25: no column's resize handle is in the Tab order, and Tab never stops
/// on one.
#[test]
fn column_resize_handles_are_not_focusable() {
    let mut r = rig(2);
    r.focus_cell(0, ACCOUNT);
    for col in [ACCOUNT, AMOUNT, MEMO, DATE, TAGS, BILLABLE] {
        if let Some(resp) = r.ctx.read_response(r.grid.id().with(("resize", col.0))) {
            assert!(!resp.sense.is_focusable(), "{col:?} resize handle");
        }
    }
    let handles: Vec<egui::Id> = [ACCOUNT, AMOUNT, MEMO, DATE, TAGS, BILLABLE]
        .iter()
        .map(|c| r.grid.id().with(("resize", c.0)))
        .collect();
    assert!(
        handles.iter().any(|h| r.ctx.read_response(*h).is_some()),
        "the line grid draws resize handles"
    );
    for _ in 0..40 {
        r.frame(tab());
        let f = r.focused();
        assert!(
            f.is_none_or(|f| !handles.contains(&f)),
            "Tab stopped on a handle"
        );
    }
}

/// N3: an ISO date typed one key per frame commits; `+` typed in the same
/// frame right after digits applies to those digits.
#[test]
fn iso_date_and_same_frame_date_key() {
    let mut r = rig(2);
    r.focus_cell(0, DATE);
    for c in "2026-11-04".chars() {
        r.frame(typed(&c.to_string()));
    }
    r.frame(tab());
    let want = cl_core::CalendarDate::from_ymd(2026, 11, 4).ok();
    assert_eq!(r.model.lines()[0].date, want);
    r.focus_cell(1, DATE);
    r.frame(vec![
        egui::Event::Text("3/1/2026".into()),
        egui::Event::Text("+".into()),
    ]);
    r.frame(tab());
    let next = cl_core::CalendarDate::from_ymd(2026, 3, 2).ok();
    assert_eq!(r.model.lines()[1].date, next);
}

fn paste(s: &str) -> Vec<egui::Event> {
    vec![egui::Event::Paste(s.into())]
}

fn block_rig(rows: usize) -> Rig {
    Rig::new(
        cl_grid::GridSpec {
            block_paste: true,
            ..line_spec()
        },
        Lines::new(true, rows),
    )
}

fn blocks(out: &cl_grid::GridOutput) -> Vec<(cl_grid::CellPos, String)> {
    out.events
        .iter()
        .filter_map(|e| match e {
            cl_grid::GridEvent::BlockPaste { at, text } => Some((*at, text.as_str().to_owned())),
            _ => None,
        })
        .collect()
}

fn shown_problem(r: &Rig) -> Option<TextKey> {
    r.grid
        .core()
        .edit()
        .and_then(|e| e.error.as_ref())
        .map(|p| p.key)
}

#[test]
fn block_paste_off_by_default_refuses_two_values() {
    let mut r = rig(2);
    assert!(!r.grid.spec().block_paste);
    r.focus_cell(0, AMOUNT);
    let out = r.frame(paste("1\t2"));
    assert_eq!(blocks(&out).len(), 0);
    assert_eq!(
        shown_problem(&r),
        Some(TextKey("grid.paste.one_value")),
        "the default grid still refuses a multi-value paste"
    );
}

#[test]
fn block_paste_emits_the_text_at_the_cursor() {
    let mut r = block_rig(2);
    r.focus_cell(1, AMOUNT);
    let text = "Date\tPayee\tAmount\r\n2026-11-04\tAcme\t12.345\r\n";
    let out = r.frame(paste(text));
    let at = cl_grid::CellPos {
        row: r.model.key(1),
        col: AMOUNT,
    };
    assert_eq!(blocks(&out), vec![(at, text.to_owned())]);
    assert_eq!(shown_problem(&r), None, "no one-value refusal");
    assert_eq!(r.model.line(r.model.key(1)).amount, None);
}

#[test]
fn block_paste_takes_several_lines_in_a_text_cell() {
    let mut r = block_rig(1);
    r.focus_cell(0, MEMO);
    let out = r.frame(paste("one\ntwo"));
    assert_eq!(blocks(&out).len(), 1);
    assert_eq!(r.model.lines()[0].memo, "");
}

#[test]
fn block_paste_leaves_a_single_value_alone() {
    let mut r = block_rig(1);
    r.focus_cell(0, AMOUNT);
    let out = r.frame(paste("12.50\r\n"));
    assert_eq!(blocks(&out).len(), 0);
    assert_eq!(shown_problem(&r), None);
    r.frame(tab());
    assert_eq!(r.model.lines()[0].amount, Some(money("12.50")));
}

#[test]
fn pasted_text_debug_hides_the_content() {
    let r = block_rig(1);
    let text = cl_grid::PastedText::new("Acme\t1,234.56");
    let shown = format!("{text:?}");
    assert!(
        !shown.contains("Acme") && !shown.contains("1,234"),
        "{shown}"
    );
    let ev = cl_grid::GridEvent::BlockPaste {
        at: cl_grid::CellPos {
            row: r.model.key(0),
            col: AMOUNT,
        },
        text,
    };
    assert_eq!(format!("{ev:?}"), "BlockPaste");
}

#[test]
fn block_paste_needs_an_editable_grid() {
    let spec = cl_grid::GridSpec {
        block_paste: true,
        ..cl_grid::GridSpec::browse(support::lines::columns())
    };
    assert_eq!(
        spec.validate(),
        Err(cl_grid::SpecError::EditFlagsWithoutEditing)
    );
}
