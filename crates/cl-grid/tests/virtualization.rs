//! Virtualization (P10 AC1, plan Rev 2 §6): the work per frame does not
//! depend on the row count (1k, 200k, 10M), Ctrl+End puts the last row's
//! bottom exactly on the grid's bottom, the top row stays put when rows are
//! inserted above, and the wheel scrolls the grid, not the screen.

// AccessKit bounds are f64 screen geometry; never money.
#![allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]

mod support;

use cl_grid::{ModelRef, RowKey};
use egui::{Event, Key, Modifiers, Pos2, Vec2};
use support::browse::Browse;
use support::register::{Register, spec};
use support::rig::key;

#[test]
fn work_per_frame_does_not_depend_on_the_row_count() {
    let mut results = Vec::new();
    for n in [1_000usize, 200_000, 10_000_000] {
        let mut b = Browse::new(Register::new(n), spec());
        b.frame(vec![]);
        b.frame(vec![]);
        b.model.calls.set(0);
        let shapes = b.frame(vec![]);
        results.push((n, b.model.calls.get(), shapes));
    }
    let (_, calls, shapes) = results[0];
    // 940 pt high at 44 pt per two-line row: about 21 rows, 9 columns.
    assert!(calls <= (21 + 2) * 9, "{calls} cell reads in one frame");
    for (n, c, s) in &results {
        assert_eq!((*c, *s), (calls, shapes), "N = {n}");
    }
}

#[test]
fn ctrl_end_lands_exactly_at_the_bottom() {
    for n in [200_000usize, 10_000_000] {
        let mut b = Browse::new(Register::new(n), spec());
        b.focus();
        b.frame(key(Key::End, Modifiers::CTRL));
        b.frame(vec![]);
        let last = b.grid.cell_id(cl_grid::CellPos {
            row: RowKey(n as u64),
            col: support::register::DATE,
        });
        let row_bottom_line1 = b.node_bounds(last).expect("last row drawn").y1;
        let bar = b
            .node_bounds(b.grid.id().with("scrollbar"))
            .expect("scrollbar");
        // The date is on line 1 (22 pt of a 44 pt row): the row's bottom is
        // 22 pt below it, exactly on the body's bottom.
        let row_bottom = row_bottom_line1 + 22.0;
        assert!(
            (row_bottom - bar.y1).abs() < 0.01,
            "N = {n}: {row_bottom} vs {}",
            bar.y1
        );
        let snap = b.grid.snapshot(&ModelRef::Browse(&mut b.model));
        assert_eq!(snap.cursor_row, Some(n - 1));
        let row_node = b
            .node_bounds(b.grid.row_id(RowKey(n as u64)))
            .expect("row node");
        assert!((row_node.y1 - bar.y1).abs() < 0.01);
    }
}

#[test]
fn rows_inserted_above_do_not_move_the_view() {
    let mut keys = support::KeysModel::with_keys(500);
    keys.revision = 1;
    let spec = cl_grid::GridSpec::browse(vec![support::lines::columns()[6]]);
    let mut b = Browse::new(keys, spec);
    b.frame(vec![]);
    b.grid.scroll_to(
        RowKey(200),
        cl_grid::core::viewport::RowAlign::Top,
        &ModelRef::Browse(&mut b.model),
    );
    b.frame(vec![]);
    assert_eq!(
        b.grid.snapshot(&ModelRef::Browse(&mut b.model)).top_row,
        199
    );
    for k in 0..25u64 {
        b.model.rows.insert(0, Some(RowKey(10_000 + k)));
    }
    b.model.revision = 2;
    b.frame(vec![]);
    assert_eq!(
        b.grid.snapshot(&ModelRef::Browse(&mut b.model)).top_row,
        224,
        "the same row is still on top"
    );
}

#[test]
fn the_wheel_scrolls_the_grid_not_the_screen() {
    let mut b = Browse::new(Register::new(200_000), spec());
    b.outer = true;
    b.frame(vec![Event::PointerMoved(Pos2::new(400.0, 300.0))]);
    for _ in 0..4 {
        b.frame(vec![
            Event::PointerMoved(Pos2::new(400.0, 300.0)),
            Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: Vec2::new(0.0, -200.0),
                modifiers: Modifiers::NONE,
                phase: egui::TouchPhase::Move,
                source: egui::MouseWheelSource::Wheel,
            },
        ]);
    }
    let top = b.grid.snapshot(&ModelRef::Browse(&mut b.model)).top_row;
    assert!(top > 0, "the grid scrolled");
    assert_eq!(b.outer_offset, 0.0, "the screen did not");
}
