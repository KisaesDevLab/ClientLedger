//! Title bar: app name, company name, date and closing date, and the company
//! accent stripe (Q-184). 36 px, `chrome.title`.

use eframe::egui::{self, Align, Frame, Layout, Margin, RichText, Ui};

use super::{Shell, us_date, weekday_date};
use crate::i18n::{t, t_with};
use crate::theme::Tokens;

/// Title bar height in logical points.
pub const HEIGHT: f32 = 36.0;
/// Accent stripe height.
pub const STRIPE: f32 = 3.0;

/// Draw the title bar and the stripe under it.
pub fn show(ui: &mut Ui, shell: &Shell, narrow: bool, tokens: &Tokens) {
    egui::Panel::top("title_bar")
        .exact_size(HEIGHT)
        .show_separator_line(false)
        .frame(
            Frame::new()
                .fill(tokens.chrome_title)
                .inner_margin(Margin::symmetric(16, 0)),
        )
        .show(ui, |ui| {
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                ui.add(
                    egui::Label::new(
                        RichText::new(t("app.name"))
                            .family(crate::fonts::bold())
                            .size(14.0)
                            .color(tokens.chrome_title_text),
                    )
                    .selectable(false),
                );
                ui.add_space(6.0);
                let company = shell
                    .company
                    .as_ref()
                    .map_or_else(|| t("shell.no_company"), |c| c.name.clone());
                // Right side first so the name truncates instead of the dates.
                let right = right_text(shell, narrow);
                let right_width = ui.fonts_mut(|f| {
                    f.layout_no_wrap(
                        right.clone(),
                        egui::FontId::proportional(13.0),
                        tokens.chrome_title_muted,
                    )
                    .size()
                    .x
                });
                let room = (ui.available_width() - right_width - 24.0).max(40.0);
                ui.scope(|ui| {
                    ui.set_max_width(room);
                    ui.add(
                        egui::Label::new(
                            RichText::new(company)
                                .size(13.0)
                                .color(tokens.chrome_title_muted),
                        )
                        .truncate()
                        .selectable(false),
                    );
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.add(
                        egui::Label::new(
                            RichText::new(right)
                                .size(13.0)
                                .color(tokens.chrome_title_muted),
                        )
                        .selectable(false),
                    );
                });
            });
        });
    egui::Panel::top("company_stripe")
        .exact_size(STRIPE)
        .show_separator_line(false)
        .frame(Frame::new().fill(shell.accent(tokens)))
        .show(ui, |_| {});
}

/// `Wed 11/04/2026  Books closed through 09/30/2026`.
#[must_use]
pub fn right_text(shell: &Shell, narrow: bool) -> String {
    let date = weekday_date(shell.today());
    let closed = shell.company.as_ref().and_then(|c| c.closed_through);
    if narrow {
        // Little room: the closing date matters more than today's date.
        return closed.map_or(date, |c| {
            t_with("shell.closed_short", &[("date", &us_date(c))])
        });
    }
    match closed {
        Some(closed) => format!(
            "{date}   {}",
            t_with("shell.closed_through", &[("date", &us_date(closed))])
        ),
        None => date,
    }
}
