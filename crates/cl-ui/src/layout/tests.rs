use eframe::egui::{self, Context, Pos2, RawInput, Rect, Vec2};

use super::{Body, FixedFrame, FrameOpts, FrameReport};

/// What one run saw: the report, the bottom bar's rect and the body's
/// room.
struct Seen {
    report: FrameReport,
    bottom: Rect,
    body: Rect,
    area: Rect,
}

/// Runs a few frames (the bars are measured on the first) of a frame with
/// `top_rows` rows on top, a 44-point action row at the bottom and a tall
/// body, in a `size` window.
fn frame(size: Vec2, top_rows: usize, body: Body) -> Seen {
    frame_with(size, top_rows, 0, body)
}

/// Like `frame`, with `bottom_rows` 24-point rows above the action row.
fn frame_with(size: Vec2, top_rows: usize, bottom_rows: usize, body: Body) -> Seen {
    let ctx = Context::default();
    let mut seen = None;
    for _ in 0..4 {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
            ..RawInput::default()
        };
        let mut out = ctx.run_ui(input, |ui| {
            let area = ui.available_rect_before_wrap();
            let mut f = FixedFrame::begin(ui, "test", FrameOpts::default());
            f.top(|ui| {
                for i in 0..top_rows {
                    ui.add_sized([200.0, 24.0], egui::Label::new(format!("row {i}")));
                }
            });
            let body_rect = f.body(body, |ui| {
                let r = ui.max_rect();
                for i in 0..60 {
                    ui.label(format!("line {i}"));
                }
                r
            });
            let bottom = f.bottom(|ui| {
                for i in 0..bottom_rows {
                    ui.add_sized([200.0, 24.0], egui::Label::new(format!("entry {i}")));
                }
                ui.add_sized([120.0, 44.0], egui::Button::new("Save & Close"))
                    .rect
            });
            let report = f.end();
            seen = Some(Seen {
                report,
                bottom,
                body: body_rect,
                area,
            });
        });
        out.textures_delta.clear();
    }
    seen.expect("ran")
}

#[test]
fn bottom_bar_stays_inside_the_window_at_480_by_320() {
    for body in [Body::Scroll, Body::Fill] {
        let s = frame(Vec2::new(480.0, 320.0), 3, body);
        assert!(
            s.bottom.bottom() <= s.area.bottom() + 0.5,
            "{:?} in {:?}",
            s.bottom,
            s.area
        );
        assert!(
            s.bottom.top() >= s.body.bottom() - 0.5,
            "bars don't overlap"
        );
        assert!(!s.report.top_scrolls);
    }
}

#[test]
fn body_keeps_three_rows_and_the_top_scrolls_when_tall() {
    let s = frame(Vec2::new(480.0, 320.0), 12, Body::Scroll);
    assert!(s.report.top_scrolls, "{:?}", s.report);
    assert!(s.report.body_h >= 3.0 * 30.0 - 0.5, "{:?}", s.report);
    assert!(s.bottom.bottom() <= s.area.bottom() + 0.5);
    let fixed = s.report.top_h + s.report.bottom_h;
    assert!(fixed < s.area.height(), "{:?}", s.report);
}

#[test]
fn a_bottom_too_tall_for_a_tiny_window_scrolls_with_its_actions_in_view() {
    let s = frame_with(Vec2::new(480.0, 200.0), 2, 6, Body::Fill);
    assert!(s.report.bottom_scrolls, "{:?}", s.report);
    assert!(s.report.body_h >= 2.0 * 30.0 - 0.5, "{:?}", s.report);
    // The action row (the bar's end) is in the window.
    assert!(s.bottom.bottom() <= s.area.bottom() + 0.5, "{:?}", s.bottom);
    assert!(s.bottom.top() >= s.body.bottom() - 0.5, "{:?}", s.bottom);
    // At a normal size the same bar shows whole.
    let s = frame_with(Vec2::new(1000.0, 700.0), 2, 6, Body::Fill);
    assert!(!s.report.bottom_scrolls, "{:?}", s.report);
}

#[test]
fn a_short_top_shows_whole() {
    let s = frame(Vec2::new(1000.0, 700.0), 4, Body::Fill);
    assert!(!s.report.top_scrolls);
    assert!(s.report.top_h >= 4.0 * 24.0, "{:?}", s.report);
    assert!(
        (s.report.body_h - s.body.height()).abs() < 0.5,
        "the body gets the room between the bars"
    );
    assert!((s.report.bottom_h - 44.0).abs() < 1.0, "{:?}", s.report);
}

#[test]
fn report_json_has_whole_points() {
    let r = FrameReport {
        top_h: 40.4,
        bottom_h: 44.0,
        body_h: 100.6,
        top_scrolls: false,
        bottom_scrolls: false,
    };
    let v = r.json();
    assert_eq!(v["top_h"], 40.0);
    assert_eq!(v["body_h"], 101.0);
    assert_eq!(v["top_scrolls"], false);
}
