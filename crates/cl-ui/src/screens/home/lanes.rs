//! The four lanes (P16 step 1, D10): Money in, Money out, Banking, Close &
//! report. Each action is a registry command (`cl_model::home::LANES`), so
//! its label and shortcut chip come from the command table and a click
//! opens its form, or the standard placeholder window when its screen
//! arrives in a later phase (Q-P16k). The figure at each lane's foot is an
//! engine figure or a dash (Q-P16i): what clients owe and the bills due
//! need `dashboard-exact-v1` (U-76), sales tax owed waits for the reports
//! (P28), and the last reconciliation reads on every engine.

use cl_core::{CalendarDate, Money, MoneyStyle};
use cl_model::home::{LANES, Lane, LaneFooter};
use eframe::egui::{
    self, Align, CornerRadius, Layout, Margin, Response, RichText, Role, Sense, Stroke, StrokeKind,
    TextStyle, Ui, Vec2, WidgetInfo,
};
use serde_json::{Value, json};

use super::HomeScreen;
use crate::i18n::{has, t, t_with};
use crate::screens::company::form::activated;
use crate::theme::{Tokens, styles};

/// Below this width the lanes go two to a row, and below [`ONE_COLUMN`]
/// one to a row (D10).
const FOUR_COLUMNS: f32 = 1100.0;
const ONE_COLUMN: f32 = 640.0;

/// Lanes per row for a body `width` points wide.
#[must_use]
pub(crate) fn columns(width: f32) -> usize {
    if width >= FOUR_COLUMNS {
        4
    } else if width >= ONE_COLUMN {
        2
    } else {
        1
    }
}

/// What sits at the right end of a lane action.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Trailing {
    /// The command's shortcut chip (`Ctrl W`).
    Chip(String),
    /// A count badge (bank-feed items waiting).
    Badge(u64),
    /// A short muted note (the month, the closing date).
    Note {
        /// The text.
        text: String,
        /// In the figure font (dates).
        mono: bool,
    },
    /// Nothing.
    Nothing,
}

/// The label a lane action shows: Home's own wording where the mockup has
/// one ("Review Bank Feed"), else the command's label.
#[must_use]
pub(crate) fn action_label(id: &str) -> String {
    let own = format!("home.action.{id}");
    if has(&own) {
        t(&own)
    } else {
        t(&format!("cmd.{id}"))
    }
}

/// The month short name ("Oct").
#[must_use]
pub(crate) fn month_short(month: u32) -> String {
    t(&format!("home.month.{month}"))
}

/// The last whole month before `today` (the month the Profit & Loss chip
/// names: "Oct" on 11/04, as the mockup shows).
#[must_use]
pub(crate) fn last_month(today: CalendarDate) -> u32 {
    CalendarDate::from_ymd(today.year(), today.month(), 1)
        .and_then(|first| first.add_days(-1))
        .map_or(today.month(), CalendarDate::month)
}

/// What a lane action shows at its right end.
#[must_use]
pub(crate) fn trailing(home: &HomeScreen, id: &str) -> Trailing {
    match id {
        "banking.feed" => match home.data.counts.value.as_ref().map(|c| &c.feed) {
            Some(Ok(n)) if *n > 0 => Trailing::Badge(*n),
            _ => Trailing::Nothing,
        },
        "reports.profit_loss" => Trailing::Note {
            text: month_short(last_month(home.today)),
            mono: false,
        },
        // "last 10/06" waits for Send to My Accountant (P32, Q-P16i).
        "company.send_to_accountant" => Trailing::Nothing,
        "company.closing_date" => match home.data.closing.value {
            Some(Some(d)) => Trailing::Note {
                text: format!("{:02}/{:02}", d.month(), d.day()),
                mono: true,
            },
            _ => Trailing::Nothing,
        },
        _ => cl_model::commands::find(id)
            .and_then(|c| c.shortcut)
            .map_or(Trailing::Nothing, |s| {
                Trailing::Chip(crate::shell::chip_text(s))
            }),
    }
}

/// The figure at a lane's foot: its label, the figure (`None` shows a
/// dash) and the dash's tooltip.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FooterView {
    /// "Clients owe you".
    pub label: String,
    /// The engine's figure, formatted; `None` is a dash.
    pub value: Option<String>,
    /// The figure is an amount (figure font).
    pub money: bool,
    /// Why there is a dash.
    pub tip: Option<String>,
}

fn footer_key(f: LaneFooter) -> &'static str {
    match f {
        LaneFooter::ClientsOwe => "clients_owe",
        LaneFooter::BillsDue => "bills_due",
        LaneFooter::LastReconciled => "last_reconciled",
        LaneFooter::SalesTaxOwed => "sales_tax_owed",
    }
}

/// The engine's figure for a money footer, or why there is none.
fn money_figure(
    home: &HomeScreen,
    figure: Option<Money>,
    style: MoneyStyle,
) -> (Option<String>, Option<String>) {
    if !home.exact {
        return (None, Some(t("home.footer.later.tip")));
    }
    match figure {
        Some(m) => (Some(m.format(style)), None),
        None if home.data.summary.error.is_some() => (None, Some(t("home.footer.failed.tip"))),
        None => (None, None),
    }
}

/// A lane's footer as it shows (`style` formats the amounts).
#[must_use]
pub(crate) fn footer_view(home: &HomeScreen, footer: LaneFooter, style: MoneyStyle) -> FooterView {
    let summary = home.data.summary.value.as_ref();
    let days = home.local.bills_due_days;
    match footer {
        LaneFooter::ClientsOwe => {
            let figure = summary
                .and_then(|s| s.receivables.as_ref())
                .map(|r| r.total_outstanding);
            let (value, tip) = money_figure(home, figure, style);
            FooterView {
                label: t("home.footer.clients_owe"),
                value,
                money: true,
                tip,
            }
        }
        LaneFooter::BillsDue => {
            // Only the engine's figure for the company's own window.
            let figure = summary
                .and_then(|s| s.payables.as_ref())
                .filter(|p| p.due_within_days == days)
                .map(|p| p.due_soon_amount);
            let (value, mut tip) = money_figure(home, figure, style);
            if home.exact && value.is_none() && days > cl_api::DUE_WITHIN_DAYS_MAX {
                tip = Some(t("home.footer.window.tip"));
            }
            FooterView {
                label: t_with("home.footer.bills_due", &[("days", &days.to_string())]),
                value,
                money: true,
                tip,
            }
        }
        LaneFooter::LastReconciled => {
            let value = home.data.reconcile.value.as_ref().map(|r| {
                r.last_completed.map_or_else(
                    || t("home.footer.not_yet"),
                    |l| {
                        let date = crate::shell::us_date(l.statement_date);
                        match home.data.account(l.account) {
                            Some(a) => {
                                format!("{} \u{b7} {date}", crate::account_names::shown_name(a))
                            }
                            None => date,
                        }
                    },
                )
            });
            let tip = (value.is_none() && home.data.reconcile.error.is_some())
                .then(|| t("home.footer.failed.tip"));
            FooterView {
                label: t("home.footer.last_reconciled"),
                value,
                money: false,
                tip,
            }
        }
        LaneFooter::SalesTaxOwed => FooterView {
            label: t("home.footer.sales_tax_owed"),
            value: None,
            money: true,
            tip: Some(t("home.footer.sales_tax.tip")),
        },
    }
}

/// Which footers show a figure (the control channel; never the figure).
#[must_use]
pub(crate) fn footers_shown(home: &HomeScreen) -> Value {
    let map: serde_json::Map<String, Value> = LANES
        .iter()
        .map(|l| {
            let v = footer_view(home, l.footer, MoneyStyle::default());
            (footer_key(l.footer).to_string(), json!(v.value.is_some()))
        })
        .collect();
    Value::Object(map)
}

/// Draws the lanes; returns how many went in a row.
pub(super) fn show(ui: &mut Ui, home: &mut HomeScreen, tokens: &Tokens) -> usize {
    let cols = columns(ui.available_width());
    let mut style = crate::grid_env::shell_frame(ui.ctx()).money_style;
    style.symbol = true;
    let id = ui.scope_id().with("home-lanes");
    let mut clicked: Option<&'static str> = None;
    {
        let view: &HomeScreen = home;
        super::tiles(ui, id, LANES.len(), cols, |ui, i, min_h| {
            lane(ui, view, tokens, &LANES[i], min_h, style, &mut clicked)
        });
    }
    if let Some(id) = clicked {
        home.open(id);
    }
    cols
}

/// One lane: its title, four actions and the footer pinned to the bottom
/// (every lane in a row is as tall as the tallest). Returns its natural
/// height.
fn lane(
    ui: &mut Ui,
    home: &HomeScreen,
    tokens: &Tokens,
    lane: &Lane,
    min_h: f32,
    style: MoneyStyle,
    clicked: &mut Option<&'static str>,
) -> f32 {
    egui::Frame::new()
        .fill(tokens.surface_window)
        .stroke(Stroke::new(1.0, tokens.border_default))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::same(14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let top = ui.cursor().top();
            ui.add(
                egui::Label::new(
                    RichText::new(t(&format!("home.lane.{}", lane.id.key())).to_uppercase())
                        .text_style(styles::eyebrow())
                        .strong()
                        .color(tokens.accent),
                )
                .selectable(false),
            );
            ui.add_space(6.0);
            ui.spacing_mut().item_spacing.y = 8.0;
            for id in lane.actions {
                let r = action_row(ui, tokens, &action_label(id), &trailing(home, id));
                if activated(&r) {
                    *clicked = Some(id);
                }
            }
            let used = ui.cursor().top() - top;
            let foot_id = ui.scope_id().with(("home-lane-foot", lane.id.key()));
            let foot_h: f32 = ui.data(|d| d.get_temp(foot_id)).unwrap_or(30.0);
            let pad = min_h - used - foot_h;
            if pad > 0.0 {
                ui.add_space(pad);
            }
            let foot_top = ui.cursor().top();
            footer(ui, tokens, &footer_view(home, lane.footer, style));
            let got = ui.cursor().top() - foot_top;
            if (got - foot_h).abs() > 0.5 {
                ui.data_mut(|d| d.insert_temp(foot_id, got));
                ui.ctx().request_discard("home lane footer measured");
            }
            used + got
        })
        .inner
}

/// The label at the left and the figure at the right; when both don't fit
/// on one line, the figure goes under the label, still at the right.
fn footer(ui: &mut Ui, tokens: &Tokens, v: &FooterView) {
    ui.add_space(4.0);
    let text = v.value.clone().unwrap_or_else(|| "\u{2014}".into());
    let family = if v.money {
        egui::FontFamily::Monospace
    } else {
        egui::FontFamily::Proportional
    };
    let width = ui.available_width();
    let measure = |s: &str, font: egui::FontId| {
        ui.painter()
            .layout_no_wrap(s.to_owned(), font, tokens.text_primary)
            .size()
            .x
    };
    let fits = measure(&v.label, egui::FontId::proportional(13.0))
        + measure(&text, egui::FontId::new(13.0, family.clone()))
        + 12.0
        <= width;
    let label = egui::Label::new(
        RichText::new(&v.label)
            .size(13.0)
            .color(tokens.text_secondary),
    )
    .selectable(false);
    let value = |ui: &mut Ui| {
        let rich = RichText::new(text.as_str())
            .size(13.0)
            .family(family.clone())
            .color(if v.value.is_some() {
                tokens.text_primary
            } else {
                tokens.text_tertiary
            });
        let r = ui.add(egui::Label::new(rich).selectable(false).wrap());
        if let Some(tip) = &v.tip {
            r.on_hover_text(tip);
        }
    };
    if fits {
        ui.horizontal(|ui| {
            ui.add(label);
            ui.with_layout(Layout::right_to_left(Align::Center), value);
        });
    } else {
        ui.add(label.wrap());
        ui.with_layout(Layout::top_down(Align::Max), value);
    }
}

/// A lane action: a full-width bordered row with the label at the left
/// and its chip, badge or note at the right. The accessible name is the
/// label alone.
fn action_row(ui: &mut Ui, tokens: &Tokens, label: &str, trailing: &Trailing) -> Response {
    const PAD_X: f32 = 10.0;
    let width = ui.available_width();
    let kbd = styles::kbd().resolve(ui.style());
    let (galley_t, room, fill, text_color) = match trailing {
        Trailing::Chip(s) => {
            let g = ui
                .painter()
                .layout_no_wrap(s.clone(), kbd, tokens.text_secondary);
            let w = g.size().x + 10.0;
            (
                Some(g),
                w,
                Some(tokens.surface_window),
                tokens.text_secondary,
            )
        }
        Trailing::Badge(n) => {
            let g = ui
                .painter()
                .layout_no_wrap(n.to_string(), kbd, tokens.on_warn);
            let w = g.size().x + 16.0;
            (Some(g), w, Some(tokens.warn_badge), tokens.on_warn)
        }
        Trailing::Note { text, mono } => {
            let font = if *mono {
                kbd
            } else {
                TextStyle::Small.resolve(ui.style())
            };
            let g = ui
                .painter()
                .layout_no_wrap(text.clone(), font, tokens.text_secondary);
            let w = g.size().x;
            (Some(g), w, None, tokens.text_secondary)
        }
        Trailing::Nothing => (None, 0.0, None, tokens.text_secondary),
    };
    let gap = if room > 0.0 { 8.0 } else { 0.0 };
    let wrap = (width - 2.0 * PAD_X - room - gap).max(40.0);
    let galley = ui.painter().layout(
        label.to_owned(),
        TextStyle::Body.resolve(ui.style()),
        tokens.text_primary,
        wrap,
    );
    let h = (galley.size().y + 18.0).max(40.0);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, h), Sense::click());
    let enabled = ui.is_enabled();
    resp.widget_info(|| WidgetInfo::labeled(Role::Button, enabled, label));
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        let hovered = resp.hovered();
        p.rect(
            rect,
            CornerRadius::same(4),
            if hovered {
                tokens.accent_tint
            } else {
                tokens.surface_window
            },
            Stroke::new(
                1.0,
                if hovered {
                    tokens.accent_border
                } else {
                    tokens.border_default
                },
            ),
            StrokeKind::Inside,
        );
        let pos = egui::pos2(rect.left() + PAD_X, rect.center().y - galley.size().y / 2.0);
        p.galley(pos, galley, tokens.text_primary);
        if let Some(g) = galley_t {
            let size = g.size();
            let (w, hh) = match trailing {
                Trailing::Note { .. } => (size.x, size.y),
                _ => (room, size.y + 2.0),
            };
            let tr = egui::Rect::from_min_size(
                egui::pos2(rect.right() - PAD_X - w, rect.center().y - hh / 2.0),
                Vec2::new(w, hh),
            );
            match (trailing, fill) {
                (Trailing::Chip(_), Some(f)) => {
                    p.rect(
                        tr,
                        CornerRadius::same(3),
                        f,
                        Stroke::new(1.0, tokens.border_strong),
                        StrokeKind::Inside,
                    );
                }
                (Trailing::Badge(_), Some(f)) => {
                    p.rect_filled(tr, CornerRadius::same(10), f);
                }
                _ => {}
            }
            p.galley(tr.center() - size / 2.0, g, text_color);
        }
        if resp.has_focus() {
            p.rect_stroke(
                rect,
                CornerRadius::same(4),
                Stroke::new(2.0, tokens.accent),
                StrokeKind::Inside,
            );
        }
    }
    resp
}
