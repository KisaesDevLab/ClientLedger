//! A grouped bar chart drawn with the painter (P16 D1, Q-P16h): one group
//! per period (the months on Home), side-by-side bars in each, the group's
//! label under it. No plotting crate: each group is its own click target and
//! AccessKit node (a button named by the caller, "October: income …"), so
//! the chart can be read and driven from the keyboard and drills into its
//! report.
//!
//! Money rule (Non-negotiable 6): a bar's value is an integer in the
//! caller's units (whole cents on Home), never a float. Its height is the
//! integer ratio `value / max` in parts of ten thousand, and only that ratio
//! becomes an `f32`, for pixels: geometry only, never money. Every figure a
//! reader sees is text the caller formatted from the engine's own value.

use eframe::egui::{
    self, Color32, CornerRadius, Id, Rect, Role, Sense, Stroke, StrokeKind, Ui, Vec2, WidgetInfo,
};

use crate::theme::Tokens;

/// Parts of ten thousand: the resolution of a bar's height.
const SCALE: i128 = 10_000;

/// One bar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bar {
    /// Its value in the caller's integer units (cents); a value of zero or
    /// less draws no bar.
    pub value: i64,
    /// Its colour (a theme token).
    pub color: Color32,
}

/// One group of bars (a month).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BarGroup {
    /// The label under the group ("Oct").
    pub label: String,
    /// What a screen reader says for the group ("October: income 21,400.00,
    /// expenses 7,534.86"); also its tooltip.
    pub accessible: String,
    /// The bars, left to right; `None` when the group's figures are missing
    /// (a failed read draws a dash, never a zero).
    pub bars: Option<Vec<Bar>>,
}

/// The chart's size.
#[derive(Clone, Copy, Debug)]
pub struct BarChart {
    /// Salt for the groups' ids.
    pub id: Id,
    /// Height of the plot (the bars' area), in points.
    pub plot_height: f32,
}

/// Space between groups (the mockup's 14 px) and between bars (3 px).
const GROUP_GAP: f32 = 14.0;
const BAR_GAP: f32 = 3.0;
/// Height of the label row under the plot.
const LABEL_H: f32 = 20.0;

/// The height of a bar worth `value` when the tallest is worth `max`, in a
/// plot `plot` points tall. Zero or less (or no tallest) gives no bar.
#[must_use]
pub fn bar_height(value: i64, max: i64, plot: f32) -> f32 {
    if value <= 0 || max <= 0 {
        return 0.0;
    }
    let parts = (i128::from(value) * SCALE / i128::from(max)).clamp(0, SCALE);
    // Geometry only, never money: an integer ratio becomes pixels.
    let parts = u16::try_from(parts).unwrap_or(10_000);
    plot * f32::from(parts) / 10_000.0
}

/// The tallest bar's value across `groups` (0 when none is above zero).
#[must_use]
pub fn tallest(groups: &[BarGroup]) -> i64 {
    groups
        .iter()
        .filter_map(|g| g.bars.as_ref())
        .flatten()
        .map(|b| b.value)
        .max()
        .unwrap_or(0)
        .max(0)
}

impl BarChart {
    /// Draws the chart across the available width. Returns the group that
    /// was clicked, or activated with Enter or Space while focused.
    pub fn show(&self, ui: &mut Ui, groups: &[BarGroup]) -> Option<usize> {
        let tokens = Tokens::of(ui.ctx());
        let width = ui.available_width();
        let (rect, _) =
            ui.allocate_exact_size(Vec2::new(width, self.plot_height + LABEL_H), Sense::hover());
        let plot = Rect::from_min_size(rect.min, Vec2::new(width, self.plot_height));
        let n = groups.len().max(1);
        // Geometry only (a count of groups), never money.
        let n_f = f32::from(u16::try_from(n).unwrap_or(u16::MAX));
        let group_w = ((width - GROUP_GAP * (n_f - 1.0)) / n_f).max(8.0);
        let max = tallest(groups);
        let mut picked = None;
        for (i, g) in groups.iter().enumerate() {
            let i_f = f32::from(u16::try_from(i).unwrap_or(u16::MAX));
            let left = plot.left() + i_f * (group_w + GROUP_GAP);
            let column = Rect::from_min_size(
                egui::pos2(left, plot.top()),
                Vec2::new(group_w, self.plot_height + LABEL_H),
            );
            let r = ui.interact(column, self.id.with(("group", i)), Sense::click());
            r.widget_info(|| WidgetInfo::labeled(Role::Button, true, &g.accessible));
            let hovered = r.hovered() || r.has_focus();
            if ui.is_rect_visible(column) {
                paint_group(ui, &tokens, g, column, self.plot_height, max, hovered);
                if r.has_focus() {
                    ui.painter().rect_stroke(
                        column.expand(2.0),
                        CornerRadius::same(3),
                        Stroke::new(2.0, tokens.accent),
                        StrokeKind::Outside,
                    );
                }
            }
            // `clicked` is also Enter or Space on the focused group.
            if r.clicked() {
                picked = Some(i);
            }
            r.on_hover_text(&g.accessible);
        }
        // The axis under the bars.
        ui.painter().hline(
            plot.x_range(),
            plot.bottom(),
            Stroke::new(1.0, tokens.border_default),
        );
        picked
    }
}

fn paint_group(
    ui: &Ui,
    tokens: &Tokens,
    group: &BarGroup,
    column: Rect,
    plot_h: f32,
    max: i64,
    hovered: bool,
) {
    let painter = ui.painter();
    let plot = Rect::from_min_size(column.min, Vec2::new(column.width(), plot_h));
    if hovered {
        painter.rect_filled(plot, CornerRadius::same(2), tokens.accent_tint);
    }
    match &group.bars {
        Some(bars) if !bars.is_empty() => {
            let k = f32::from(u16::try_from(bars.len()).unwrap_or(u16::MAX));
            let bar_w = ((column.width() - BAR_GAP * (k - 1.0)) / k).max(1.0);
            for (j, b) in bars.iter().enumerate() {
                let h = bar_height(b.value, max, plot_h);
                if h <= 0.0 {
                    continue;
                }
                let j_f = f32::from(u16::try_from(j).unwrap_or(u16::MAX));
                let x = column.left() + j_f * (bar_w + BAR_GAP);
                let bar = Rect::from_min_max(
                    egui::pos2(x, plot.bottom() - h),
                    egui::pos2(x + bar_w, plot.bottom()),
                );
                painter.rect_filled(bar, CornerRadius::ZERO, b.color);
            }
        }
        _ => {
            // Missing figures: a dash in the middle of the plot, not a zero.
            painter.text(
                plot.center(),
                egui::Align2::CENTER_CENTER,
                "\u{2014}",
                egui::FontId::proportional(13.0),
                tokens.text_tertiary,
            );
        }
    }
    painter.text(
        egui::pos2(column.center().x, plot.bottom() + LABEL_H / 2.0 + 1.0),
        egui::Align2::CENTER_CENTER,
        &group.label,
        egui::FontId::proportional(11.0),
        tokens.text_tertiary,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::accesskit;

    #[test]
    fn the_tallest_bar_fills_the_plot_and_zero_gives_no_bar() {
        assert!((bar_height(2_140_000, 2_140_000, 120.0) - 120.0).abs() < 1e-3);
        assert!((bar_height(1_070_000, 2_140_000, 120.0) - 60.0).abs() < 1e-3);
        assert!(bar_height(0, 2_140_000, 120.0).abs() < f32::EPSILON);
        assert!(bar_height(-500, 2_140_000, 120.0).abs() < f32::EPSILON);
        assert!(bar_height(100, 0, 120.0).abs() < f32::EPSILON);
        // Never taller than the plot, and exact at the extremes of i64.
        assert!((bar_height(i64::MAX, 1, 120.0) - 120.0).abs() < 1e-3);
        assert!((bar_height(i64::MAX, i64::MAX, 50.0) - 50.0).abs() < 1e-3);
        let g = |vals: Option<Vec<i64>>| BarGroup {
            label: String::new(),
            accessible: String::new(),
            bars: vals.map(|v| {
                v.into_iter()
                    .map(|value| Bar {
                        value,
                        color: Color32::BLACK,
                    })
                    .collect()
            }),
        };
        assert_eq!(
            tallest(&[g(Some(vec![5, 9])), g(None), g(Some(vec![-20, 3]))]),
            9
        );
        assert_eq!(tallest(&[g(None), g(Some(vec![-1]))]), 0);
    }

    /// Each group is a button named by its accessible text; a click picks it
    /// and the tallest bar reaches the top of the plot.
    #[test]
    fn groups_are_named_buttons_and_a_click_picks_one() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx, crate::theme::ThemeChoice::Light);
        crate::fonts::install(&ctx);
        ctx.enable_accesskit();
        let groups: Vec<BarGroup> = ["Sep", "Oct"]
            .iter()
            .enumerate()
            .map(|(i, m)| BarGroup {
                label: (*m).to_string(),
                accessible: format!("{m}: income"),
                bars: (i == 1).then(|| {
                    vec![Bar {
                        value: 10,
                        color: Color32::RED,
                    }]
                }),
            })
            .collect();
        let chart = BarChart {
            id: Id::NULL.with("test-chart"),
            plot_height: 100.0,
        };
        let mut picked = None;
        let mut names = Vec::new();
        let frame = |events: Vec<egui::Event>, picked: &mut Option<usize>| {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400.0, 300.0),
                )),
                events,
                ..egui::RawInput::default()
            };
            let mut out = ctx.run_ui(input, |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    if let Some(i) = chart.show(ui, &groups) {
                        *picked = Some(i);
                    }
                });
            });
            out.textures_delta.clear();
            out
        };
        let out = frame(Vec::new(), &mut picked);
        if let Some(u) = out.platform_output.accesskit_update {
            for (_, n) in u.nodes {
                if n.role() == accesskit::Role::Button {
                    names.push(n.label().unwrap_or_default().to_string());
                }
            }
        }
        assert_eq!(names, ["Sep: income", "Oct: income"]);
        // The second group's column starts past the middle of 400 points.
        let at = egui::pos2(300.0, 60.0);
        frame(vec![egui::Event::PointerMoved(at)], &mut picked);
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame(vec![button(true)], &mut picked);
        frame(vec![button(false)], &mut picked);
        assert_eq!(picked, Some(1));
    }
}
