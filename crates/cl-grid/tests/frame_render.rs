//! Rendering (P10 plan Rev 2 §6): two-line registers at 1440×940, 1024×700
//! and 480×320 at 200% draw nothing outside the grid, the widest amount is
//! drawn in full and its column only grows, selection marks show over error
//! and locked fills (T21), and a dark style renders too.

// AccessKit bounds are f64 screen geometry; never money.
#![allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]

mod support;

use cl_grid::{GridStyle, ModelRef, RowKey, RowState};
use egui::epaint::{Shape, TextShape};
use egui::{Color32, Vec2};
use support::browse::Browse;
use support::register::{PAYMENT, Register, spec};

fn shapes(b: &Browse<Register>) -> Vec<(egui::Rect, Shape)> {
    b.last
        .as_ref()
        .expect("frame")
        .shapes
        .iter()
        .map(|c| (c.clip_rect, c.shape.clone()))
        .collect()
}

fn texts(b: &Browse<Register>) -> Vec<TextShape> {
    let mut out = Vec::new();
    for (_, s) in shapes(b) {
        text_shapes(&s, &mut out);
    }
    out
}

fn grid_rect(b: &Browse<Register>) -> egui::Rect {
    let r = b.node_bounds(b.grid.id()).expect("grid node");
    egui::Rect::from_min_max(
        egui::pos2(r.x0 as f32, r.y0 as f32),
        egui::pos2(r.x1 as f32, r.y1 as f32),
    )
}

#[test]
fn nothing_is_drawn_outside_the_grid_at_three_sizes() {
    for (size, ppp) in [
        (Vec2::new(1440.0, 940.0), 1.0),
        (Vec2::new(1024.0, 700.0), 1.0),
        (Vec2::new(480.0, 320.0), 2.0),
    ] {
        let mut b = Browse::new(Register::new(200_000), spec());
        b.size = size;
        b.ctx.set_pixels_per_point(ppp);
        for _ in 0..3 {
            b.frame(vec![]);
        }
        let grid = grid_rect(&b);
        assert!(
            grid.width() <= size.x + 0.5 && grid.height() <= size.y + 0.5,
            "{size:?}"
        );
        for (clip, _) in shapes(&b) {
            let visible = clip.intersect(egui::Rect::from_min_size(egui::Pos2::ZERO, size));
            assert!(
                visible.is_negative() || grid.expand(0.5).contains_rect(visible),
                "{size:?}: a shape is clipped to {visible:?}, outside the grid {grid:?}"
            );
        }
        let snap = b.grid.snapshot(&ModelRef::Browse(&mut b.model));
        assert_eq!(snap.rows, 200_000);
        // At least three rows stay visible at the minimum window (Q-P06e).
        let header = 44.0;
        assert!(
            grid.height() - header >= 3.0 * 44.0 - 0.5,
            "{size:?}: {grid:?}"
        );
    }
}

#[test]
fn the_widest_amount_is_never_cut_and_the_column_only_grows() {
    let mut reg = Register::new(50);
    reg.big = Some(0);
    let mut b = Browse::new(reg, spec());
    for _ in 0..4 {
        b.frame(vec![]);
    }
    let big = cl_core::Money::MIN.format(cl_core::MoneyStyle::default());
    assert_eq!(big, "-999,999,999,999,999.9999");
    let shape = texts(&b)
        .into_iter()
        .find(|t| t.galley.job.text == big)
        .expect("the amount is drawn");
    assert!(!shape.galley.elided, "never elided");
    let cell = b
        .node_bounds(b.grid.cell_id(cl_grid::CellPos {
            row: RowKey(1),
            col: PAYMENT,
        }))
        .expect("cell");
    let drawn = shape.visual_bounding_rect();
    assert!(
        drawn.left() >= cell.x0 as f32 - 0.5 && drawn.right() <= cell.x1 as f32 + 0.5,
        "{drawn:?} inside {cell:?}"
    );
    let width = |b: &Browse<Register>| b.grid.column_layout().place(PAYMENT).expect("col").width;
    let grown = width(&b);
    assert!(grown > 120.0, "the column grew past its preferred width");
    // A narrower value does not shrink it back (no jitter while scrolling).
    b.model.big = None;
    for _ in 0..3 {
        b.frame(vec![]);
    }
    assert!(
        (width(&b) - grown).abs() < f32::EPSILON,
        "the column kept its width"
    );
}

fn has_rect(b: &Browse<Register>, color: Color32, y_range: (f32, f32), x: f32) -> bool {
    fn walk(s: &Shape, color: Color32, y: (f32, f32), x: f32) -> bool {
        match s {
            Shape::Rect(r) => {
                r.fill == color
                    && r.rect.top() >= y.0 - 0.5
                    && r.rect.bottom() <= y.1 + 0.5
                    && (r.rect.left() - x).abs() < 0.5
            }
            Shape::Vec(v) => v.iter().any(|s| walk(s, color, y, x)),
            _ => false,
        }
    }
    shapes(b).iter().any(|(_, s)| walk(s, color, y_range, x))
}

#[test]
fn selection_shows_over_error_and_locked_rows() {
    let mut reg = Register::new(20);
    reg.states = vec![
        (1, RowState::ERROR),
        (2, RowState::LOCKED),
        (3, RowState::NEEDS_REVIEW),
    ];
    let mut b = Browse::new(reg, spec());
    b.focus();
    // Select rows 2..=4 with Space and Shift+Down.
    b.frame(support::rig::key(
        egui::Key::ArrowDown,
        egui::Modifiers::NONE,
    ));
    b.frame(support::rig::key(egui::Key::Space, egui::Modifiers::NONE));
    b.frame(support::rig::key(
        egui::Key::ArrowDown,
        egui::Modifiers::SHIFT,
    ));
    b.frame(support::rig::key(
        egui::Key::ArrowDown,
        egui::Modifiers::SHIFT,
    ));
    b.frame(vec![]);
    assert_eq!(b.grid.core().selection().len(), 3);
    let accent = b.style.accent;
    for i in 1..=3u64 {
        let row = b.node_bounds(b.grid.row_id(RowKey(i + 1))).expect("row");
        assert!(
            has_rect(&b, accent, (row.y0 as f32, row.y1 as f32), row.x0 as f32),
            "row {i}: the selection bar is drawn over its fill"
        );
    }
    let fills = [b.style.warn_surface, b.style.locked_row, b.style.warn_tint];
    for (i, fill) in fills.iter().enumerate() {
        let row = b
            .node_bounds(b.grid.row_id(RowKey(i as u64 + 2)))
            .expect("row");
        assert!(
            has_rect(&b, *fill, (row.y0 as f32, row.y1 as f32), row.x0 as f32),
            "row {}: the state fill is still drawn",
            i + 1
        );
    }
}

#[test]
fn a_dark_style_renders() {
    let mut b = Browse::new(Register::new(1_000), spec());
    b.style = GridStyle {
        surface: Color32::from_rgb(0x15, 0x17, 0x1C),
        text: Color32::from_rgb(0xE6, 0xE8, 0xEC),
        header_fill: Color32::from_rgb(0x20, 0x23, 0x2A),
        ..GridStyle::default()
    };
    b.frame(vec![]);
    let shapes = b.frame(vec![]);
    assert!(shapes > 0);
    assert!(texts(&b).iter().any(|t| {
        t.fallback_color == b.style.text
            || t.galley
                .job
                .sections
                .iter()
                .any(|s| s.format.color == b.style.text)
    }));
}

#[test]
fn measured_rows_grow_for_their_notes() {
    let mut reg = Register::new(10);
    reg.notes = vec![1];
    let mut sp = spec();
    sp.height = cl_grid::RowHeight::Measured { max_rows: 5_000 };
    let mut b = Browse::new(reg, sp);
    b.frame(vec![]);
    b.frame(vec![]);
    let h = |b: &Browse<Register>, k: u64| {
        let r = b.node_bounds(b.grid.row_id(RowKey(k))).expect("row");
        (r.y1 - r.y0) as f32
    };
    assert!((h(&b, 1) - 44.0).abs() < 0.01);
    assert!(
        (h(&b, 2) - (44.0 + b.style.note_line)).abs() < 0.01,
        "a note adds a line"
    );
    assert!(
        texts(&b)
            .iter()
            .any(|t| t.galley.job.text == "Suggested from 4 earlier checks")
    );
}

#[test]
fn dragging_a_header_border_resizes_the_column() {
    let mut b = Browse::new(Register::new(10), spec());
    b.frame(vec![]);
    let before = b
        .grid
        .column_layout()
        .place(support::register::DATE)
        .expect("col")
        .width;
    let x = before; // the Date column starts at 0: its right border
    let y = 11.0; // inside the header's first line
    let at = |x: f32| egui::Event::PointerMoved(egui::pos2(x, y));
    let press = |pressed| egui::Event::PointerButton {
        pos: egui::pos2(x, y),
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    b.frame(vec![at(x)]);
    b.frame(vec![press(true)]);
    b.frame(vec![at(x + 20.0)]);
    b.frame(vec![at(x + 40.0)]);
    b.frame(vec![press(false)]);
    b.frame(vec![]);
    let after = b
        .grid
        .column_layout()
        .place(support::register::DATE)
        .expect("col")
        .width;
    assert!(after > before + 30.0, "{before} -> {after}");
    assert!(
        b.grid
            .column_widths()
            .iter()
            .any(|(c, _)| *c == support::register::DATE)
    );
    let resize = b
        .ctx
        .read_response(b.grid.id().with(("resize", support::register::DATE.0)))
        .expect("handle");
    assert!(
        !resize.sense.is_focusable(),
        "resize handles are not in the Tab order (T25)"
    );
}

/// Every text shape in `s`, nested ones included.
fn text_shapes(s: &Shape, out: &mut Vec<TextShape>) {
    match s {
        Shape::Text(t) => out.push(t.clone()),
        Shape::Vec(v) => v.iter().for_each(|x| text_shapes(x, out)),
        _ => {}
    }
}

#[test]
fn a_row_scrolled_under_the_header_draws_no_half_text() {
    use egui::accesskit::{Action, ActionRequest, TreeId};
    // A height that is not a whole number of rows: bringing the cut last
    // row into view leaves the top row partly under the header.
    let mut b = Browse::new(Register::new(500), spec());
    b.size = Vec2::new(1440.0, 333.0);
    b.frame(vec![]);
    b.frame(vec![]);
    let last = (1..=500u64)
        .filter(|k| b.node_bounds(b.grid.row_id(RowKey(*k))).is_some())
        .last()
        .expect("rows");
    b.frame(vec![egui::Event::AccessKitActionRequest(ActionRequest {
        action: Action::ScrollIntoView,
        target_tree: TreeId::ROOT,
        target_node: b.grid.row_id(RowKey(last)).accesskit_id(),
        data: None,
    })]);
    b.frame(vec![]);
    let mut seen = 0;
    for (clip, s) in shapes(&b) {
        let mut out = Vec::new();
        text_shapes(&s, &mut out);
        for t in out {
            seen += 1;
            assert!(
                t.pos.y >= clip.top() - 0.5,
                "text at {:?} starts above its clip {clip:?} (half drawn)",
                t.pos
            );
        }
    }
    assert!(seen > 0);
}
