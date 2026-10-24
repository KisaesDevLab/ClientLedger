//! Condensed entry rows (Q-267, DESIGN.md "Window anatomy" 5): forms don't
//! stretch label · account · amount rows across a wide window. The rows sit
//! in a compact table at most [`MAX_WIDTH`] wide, with zebra stripes
//! (`surface.subtle`, the colour of every striped table), a row separator
//! (`border.row`), a whole-row highlight on hover (`surface.form`) and focus
//! (`accent.tint`), and amounts right-aligned in mono. Totals sit in the
//! amount column. The form's header card and its action row use
//! [`capped`] so they share the width.
//!
//! Under [`MAX_WIDTH`] the columns are exactly what Daily Sales drew before
//! (P16 step 0d): only the stripes and the highlight are new. This module
//! only lays rows out; amounts are shown and typed by the caller's widgets
//! (`Money::format`, `amount_entry::parse`), never re-derived here.

use eframe::egui::{self, Color32, Rect, Response, RichText, Shape, Stroke, Ui};

use crate::theme::Tokens;

/// The widest a condensed table, its form's header card and action row
/// get (Q-267: about 760 px).
pub const MAX_WIDTH: f32 = 760.0;
/// The amount column's width.
pub const AMOUNT_WIDTH: f32 = 140.0;
/// A row's label and detail boxes are this tall at least.
const CELL_HEIGHT: f32 = 22.0;

/// A condensed table's settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CondensedTable {
    /// The widest the table gets.
    pub max_width: f32,
    /// The amount column's width.
    pub amount_width: f32,
}

impl Default for CondensedTable {
    fn default() -> Self {
        Self {
            max_width: MAX_WIDTH,
            amount_width: AMOUNT_WIDTH,
        }
    }
}

impl CondensedTable {
    /// The table's width in `available` points.
    #[must_use]
    pub fn width_in(&self, available: f32) -> f32 {
        available.clamp(0.0, self.max_width.max(0.0))
    }

    /// The label (and detail) column's width in a table `width` wide.
    #[must_use]
    pub fn label_width(&self, width: f32) -> f32 {
        ((width - self.amount_width - 24.0) * 0.45).max(0.0)
    }

    /// Draws the rows `rows` adds, in a table as wide as the room allows up
    /// to `max_width`. Returns what `rows` returned and where each row went.
    pub fn show<R>(
        &self,
        ui: &mut Ui,
        id: &str,
        rows: impl FnOnce(&mut CondensedUi<'_>) -> R,
    ) -> (R, CondensedReport) {
        let width = self.width_in(ui.available_width());
        let tokens = Tokens::of(ui.ctx());
        let label_w = self.label_width(width);
        let amount_w = self.amount_width;
        let table = |ui: &mut Ui| {
            ui.set_width(width);
            let left = ui.max_rect().left();
            let mut c = CondensedUi {
                ui,
                tokens,
                x: egui::Rangef::new(left, left + width),
                label_w,
                amount_w,
                parity: 0,
                count: 0,
                totals: false,
                report: CondensedReport {
                    width,
                    rows: Vec::new(),
                },
            };
            let r = rows(&mut c);
            (r, c.report)
        };
        ui.push_id(id, |ui| ui.vertical(table).inner).inner
    }
}

/// Draws a condensed table with the default settings.
pub fn condensed_rows<R>(
    ui: &mut Ui,
    id: &str,
    rows: impl FnOnce(&mut CondensedUi<'_>) -> R,
) -> (R, CondensedReport) {
    CondensedTable::default().show(ui, id, rows)
}

/// Runs `add` in a column as wide as the room allows up to [`MAX_WIDTH`]
/// (the header card and the action row of a condensed form). With less
/// room than that, `add` gets all of it, as before.
pub fn capped<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let width = ui.available_width().clamp(0.0, MAX_WIDTH);
    ui.vertical(|ui| {
        ui.set_width(width);
        add(ui)
    })
    .inner
}

/// Where one row went (for draw tests and the control channel; sizes only).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RowReport {
    /// The label's box.
    pub label: Rect,
    /// The amount's box (input or figure).
    pub amount: Rect,
    /// Drawn on a stripe.
    pub striped: bool,
    /// Highlighted (hovered or holding the focus).
    pub highlighted: bool,
    /// A total row.
    pub total: bool,
}

/// Where a condensed table's rows went.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CondensedReport {
    /// The table's width.
    pub width: f32,
    /// Every row, top to bottom.
    pub rows: Vec<RowReport>,
}

impl CondensedReport {
    /// The largest distance from a row's label to its amount box's left
    /// edge (the rule: within the table's width).
    #[must_use]
    pub fn widest_gap(&self) -> f32 {
        self.rows
            .iter()
            .map(|r| r.amount.left() - r.label.left())
            .fold(0.0, f32::max)
    }

    /// The control-channel view (sizes only, never a label or amount).
    #[must_use]
    pub fn state_json(&self) -> serde_json::Value {
        serde_json::json!({
            "width": self.width,
            "rows": self.rows.len(),
            "striped": self.rows.iter().filter(|r| r.striped).count(),
            "widest_gap": self.widest_gap(),
            // The table is at its cap (the window is wide).
            "capped": self.width >= MAX_WIDTH - 0.5,
            // Every amount box is within the cap of its label.
            "fits": self.widest_gap() <= MAX_WIDTH,
        })
    }
}

/// Adds rows to a condensed table.
pub struct CondensedUi<'a> {
    ui: &'a mut Ui,
    tokens: Tokens,
    x: egui::Rangef,
    label_w: f32,
    amount_w: f32,
    /// Rows since the last section heading (the stripe parity).
    parity: usize,
    count: usize,
    totals: bool,
    report: CondensedReport,
}

impl CondensedUi<'_> {
    /// The table's `Ui` (for a hint under a heading and the like).
    pub fn ui(&mut self) -> &mut Ui {
        self.ui
    }

    /// The label column's width.
    #[must_use]
    pub fn label_width(&self) -> f32 {
        self.label_w
    }

    /// A section heading (strong, with the accent rule under it); the
    /// stripes start again under it.
    pub fn section(&mut self, title: &str) -> Response {
        self.ui.add_space(6.0);
        let r = self.ui.add(
            egui::Label::new(
                RichText::new(title)
                    .strong()
                    .color(self.tokens.text_primary),
            )
            .selectable(false),
        );
        self.ui.painter().hline(
            r.rect.x_range(),
            r.rect.bottom() + 2.0,
            Stroke::new(2.0, self.tokens.accent),
        );
        self.parity = 0;
        r
    }

    /// One row: `label`, `detail` (an account, a memo; secondary, small)
    /// and the amount `amount` draws (it gets the label's response, to be
    /// `labelled_by`, and returns the input's response). Returns that.
    pub fn row(
        &mut self,
        label: &str,
        detail: &str,
        amount: impl FnOnce(&mut Ui, &Response) -> Response,
    ) -> Response {
        let primary = self.tokens.text_primary;
        let secondary = self.tokens.text_secondary;
        self.add_row(false, |ui, w| {
            let l = cell(
                ui,
                w,
                egui::Label::new(RichText::new(label).color(primary)).truncate(),
            );
            cell(
                ui,
                w,
                egui::Label::new(RichText::new(detail).size(12.0).color(secondary)).truncate(),
            );
            let a = amount(ui, &l);
            (l, a)
        })
    }

    /// A row whose amount is a figure to read (mono, right-aligned).
    pub fn figure(&mut self, label: &str, detail: &str, figure: &str) -> Response {
        let w = self.amount_w;
        let color = self.tokens.text_primary;
        self.row(label, detail, |ui, _| {
            figure_box(ui, w, figure, color, false)
        })
    }

    /// A total: its label, and the figure right-aligned in the amount
    /// column (strong). Totals are not striped; a rule sits above the
    /// first one.
    pub fn total(&mut self, label: &str, figure: &str) -> Response {
        let w = self.amount_w;
        let color = self.tokens.text_primary;
        if !self.totals {
            self.totals = true;
            let y = self.ui.cursor().top();
            self.ui
                .painter()
                .hline(self.x, y, Stroke::new(1.0, self.tokens.border_default));
            self.ui.add_space(2.0);
        }
        self.add_row(true, |ui, lw| {
            let l = cell(
                ui,
                lw * 2.0 + ui.spacing().item_spacing.x,
                egui::Label::new(RichText::new(label).strong().color(color)).truncate(),
            );
            let a = figure_box(ui, w, figure, color, true);
            (l, a)
        })
    }

    /// Lays out one row and paints its background behind it.
    fn add_row(
        &mut self,
        total: bool,
        cells: impl FnOnce(&mut Ui, f32) -> (Response, Response),
    ) -> Response {
        let background = self.ui.painter().add(Shape::Noop);
        let label_w = self.label_w;
        let inner = self
            .ui
            .push_id(self.count, |ui| {
                ui.horizontal(|ui| cells(ui, label_w)).inner
            })
            .inner;
        let (label, amount) = inner;
        let half = self.ui.spacing().item_spacing.y / 2.0;
        let y = label.rect.y_range().union(amount.rect.y_range());
        let rect = Rect::from_x_y_ranges(self.x, egui::Rangef::new(y.min - half, y.max + half));
        let striped = !total && self.parity % 2 == 1;
        let focused = amount.has_focus() || label.has_focus();
        let hovered = !total && self.ui.rect_contains_pointer(rect);
        let fill = if focused {
            Some(self.tokens.accent_tint)
        } else if hovered {
            Some(self.tokens.surface_form)
        } else if striped {
            Some(self.tokens.surface_subtle)
        } else {
            None
        };
        let mut shapes = Vec::new();
        if let Some(c) = fill {
            shapes.push(Shape::rect_filled(rect, 0.0, c));
        }
        if !total {
            shapes.push(Shape::hline(
                self.x,
                rect.bottom(),
                Stroke::new(1.0, self.tokens.border_row),
            ));
        }
        self.ui.painter().set(background, Shape::Vec(shapes));
        if !total {
            self.parity += 1;
        }
        self.count += 1;
        self.report.rows.push(RowReport {
            label: label.rect,
            amount: amount.rect,
            striped,
            highlighted: focused || hovered,
            total,
        });
        amount
    }
}

/// A label left-aligned in a box `width` wide. The response is the
/// label's (its id names the row's input) with the box's rect.
fn cell(ui: &mut Ui, width: f32, label: egui::Label) -> Response {
    let r = ui.allocate_ui_with_layout(
        egui::vec2(width, CELL_HEIGHT),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.set_min_size(egui::vec2(width, CELL_HEIGHT));
            ui.add(label)
        },
    );
    let mut l = r.inner;
    l.rect = r.response.rect;
    l
}

/// A read-only figure in a box `width` wide, right-aligned, mono.
fn figure_box(ui: &mut Ui, width: f32, figure: &str, color: Color32, strong: bool) -> Response {
    let mut text = RichText::new(figure)
        .text_style(egui::TextStyle::Monospace)
        .color(color);
    if strong {
        text = text.strong();
    }
    ui.allocate_ui_with_layout(
        egui::vec2(width, CELL_HEIGHT),
        egui::Layout::right_to_left(egui::Align::Center),
        |ui| {
            ui.set_min_size(egui::vec2(width, CELL_HEIGHT));
            ui.add(egui::Label::new(text).selectable(false));
        },
    )
    .response
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{Pos2, RawInput, vec2};

    /// Draws a two-section table with `n` input rows each and a total in
    /// a window `width` wide; returns the report of the last frame.
    fn draw(width: f32, n: usize) -> CondensedReport {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx, crate::theme::ThemeChoice::Light);
        crate::fonts::install(&ctx);
        let mut texts = vec![String::from("1,000.00"); n * 2];
        let mut report = CondensedReport::default();
        for _ in 0..3 {
            let input = RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(width, 900.0))),
                ..RawInput::default()
            };
            let mut out = ctx.run_ui(input, |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    let ((), r) = condensed_rows(ui, "test", |c| {
                        let mut it = texts.iter_mut();
                        for s in ["Sales", "Payments"] {
                            c.section(s);
                            for i in 0..n {
                                let text = it.next().expect("text");
                                c.row(&format!("Line {i}"), "40000 Sales", |ui, l| {
                                    ui.add(
                                        egui::TextEdit::singleline(text)
                                            .desired_width(AMOUNT_WIDTH)
                                            .font(egui::TextStyle::Monospace)
                                            .align(egui::Align2::RIGHT_CENTER),
                                    )
                                    .labelled_by(l.id)
                                });
                            }
                        }
                        c.figure("Cash back", "", "20.00");
                        c.total("Total", "1,980.00");
                    });
                    report = r;
                });
            });
            out.textures_delta.clear();
        }
        report
    }

    #[test]
    fn wide_windows_keep_the_amount_near_its_label_and_stripe_every_other_row() {
        for width in [1440.0, 1920.0] {
            let r = draw(width, 4);
            assert!((r.width - MAX_WIDTH).abs() < 0.5, "{width}: {}", r.width);
            assert_eq!(r.rows.len(), 2 * 4 + 2);
            for row in &r.rows {
                let gap = row.amount.left() - row.label.left();
                assert!(gap > 0.0 && gap <= MAX_WIDTH, "{width}: gap {gap}");
                assert!(row.amount.right() <= row.label.left() + MAX_WIDTH + 0.5);
            }
            // Stripes alternate within each section and start again.
            let stripes: Vec<bool> = r.rows.iter().map(|x| x.striped).collect();
            assert_eq!(
                stripes,
                [
                    false, true, false, true, false, true, false, true, false, false
                ],
                "{width}"
            );
            assert!(r.rows.last().is_some_and(|x| x.total && !x.striped));
            // Totals and figures sit in the amount column, right edges level.
            let right = r.rows[0].amount.right();
            for row in &r.rows {
                assert!((row.amount.right() - right).abs() < 1.5, "{row:?}");
            }
        }
    }

    #[test]
    fn under_the_cap_the_columns_are_the_old_ones() {
        let table = CondensedTable::default();
        for available in [480.0, 700.0, 760.0] {
            assert!((table.width_in(available) - available).abs() < f32::EPSILON);
        }
        // Daily Sales' old split: (width - amount - 24) * 0.45 per column.
        assert!((table.label_width(700.0) - (700.0 - 140.0 - 24.0) * 0.45).abs() < 0.01);
        assert!((table.width_in(1600.0) - MAX_WIDTH).abs() < f32::EPSILON);
        let r = draw(600.0, 2);
        assert!(r.width < MAX_WIDTH && r.width > 500.0, "{}", r.width);
        let expect = table.label_width(r.width);
        let row = r.rows[0];
        assert!((row.label.width() - expect).abs() < 1.0, "{row:?} {expect}");
    }

    #[test]
    fn the_report_names_sizes_only() {
        let r = draw(1920.0, 1);
        let v = r.state_json();
        assert_eq!(v["rows"], 4);
        // One line per section, then Cash back as the second row under
        // Payments: the only stripe.
        assert_eq!(v["striped"], 1);
        assert!(
            v["widest_gap"]
                .as_f64()
                .is_some_and(|g| g <= f64::from(MAX_WIDTH))
        );
        assert!(!v.to_string().contains("Sales"));
    }
}
