//! Income and expenses, last 6 months (P16 step 4, D1, D9): one group of
//! two bars per whole month before today, from the engine's own Profit &
//! Loss for that month (`/reports/profit-loss`, six reads in one request
//! group). "October profit" is the report's net income, never income minus
//! expenses worked out here (Non-negotiable 7), so the chart always agrees
//! with the report it drills into.
//!
//! Months are company-local: the fiscal calendar's periods (standard months
//! or a 4-4-5 style retail calendar, `cl_core::fiscal`) ending before the
//! period that holds today. A retail period is named by the month its middle
//! day falls in.
//!
//! Report figures are display-only text (`ReportAmount`, until U-14): a bar's
//! height comes from the figure rounded to whole cents as an integer
//! (geometry only), and every figure shown is the engine's text laid out
//! like money. No amount is logged or put in `state()`.

use cl_api::{ApiError, ProfitLoss, ReportAmount, ReportBasis};
use cl_core::{CalendarDate, FiscalYearSpec, MoneyStyle};
use cl_settings::FiscalCalendar;
use eframe::egui::{self, Align, Layout, Margin, RichText, Sense, Ui, Vec2};

use super::HomeScreen;
use super::balances::{failed, heading, note};
use crate::i18n::{t, t_with};
use crate::screens::lists::center::model::report_text;
use crate::theme::Tokens;
use crate::widgets::bar_chart::{Bar, BarChart, BarGroup};

/// Months on the chart.
pub(crate) const MONTHS: usize = 6;

/// The plot's height (the mockup's 120 px).
const PLOT_H: f32 = 120.0;

/// One period of the chart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Month {
    /// The calendar month it is named by (1-12).
    pub named: u32,
    /// First day.
    pub start: CalendarDate,
    /// Last day.
    pub end: CalendarDate,
}

/// The month a period is named by: the one its middle day falls in.
fn named(start: CalendarDate, end: CalendarDate) -> u32 {
    start
        .add_days(start.days_until(end) / 2)
        .map_or(start.month(), CalendarDate::month)
}

/// The [`MONTHS`] whole periods before the one holding `today`, oldest
/// first (fewer near the ends of the supported dates).
#[must_use]
pub(crate) fn months(spec: FiscalYearSpec, today: CalendarDate) -> Vec<Month> {
    let mut out = Vec::with_capacity(MONTHS);
    let Ok(mut p) = spec.period_containing(today) else {
        return out;
    };
    for _ in 0..MONTHS {
        let Ok(before) = p.start.add_days(-1) else {
            break;
        };
        let Ok(prev) = spec.period_containing(before) else {
            break;
        };
        p = prev;
        out.push(Month {
            named: named(p.start, p.end),
            start: p.start,
            end: p.end,
        });
    }
    out.reverse();
    out
}

/// The fiscal calendar of a company: its fiscal start month (engine
/// settings, January when unset) and the calendar kind kept on this
/// computer (U-25).
#[must_use]
pub(crate) fn spec(start_month: Option<u8>, calendar: FiscalCalendar) -> FiscalYearSpec {
    cl_company::year_end::fiscal_spec(start_month.unwrap_or(1), calendar)
}

/// The report basis of the company's accounting method (accrual unless it
/// says cash).
#[must_use]
pub(crate) fn basis(method: Option<&str>) -> ReportBasis {
    if method == Some("cash") {
        ReportBasis::Cash
    } else {
        ReportBasis::Accrual
    }
}

/// One month's Profit & Loss as the engine answered it.
pub(crate) struct MonthPl {
    /// The period.
    pub range: Month,
    /// The report, or why it is missing (that month draws a dash).
    pub pl: Result<ProfitLoss, ApiError>,
}

/// Reads the six months: the company's settings (fiscal start, basis), then
/// one Profit & Loss per month. A month that fails stays missing; the
/// others still show.
///
/// # Errors
/// The settings read's error (nothing to place the months by).
pub(crate) async fn read(
    s: &cl_api::Session,
    today: CalendarDate,
    calendar: FiscalCalendar,
) -> Result<Vec<MonthPl>, ApiError> {
    let settings = s.company_settings().await?;
    let spec = spec(settings.fiscal_year_start_month, calendar);
    let basis = basis(settings.accounting_method.as_deref());
    let mut out = Vec::with_capacity(MONTHS);
    for range in months(spec, today) {
        let pl = s.reports().profit_loss(range.start, range.end, basis).await;
        out.push(MonthPl { range, pl });
    }
    Ok(out)
}

/// A report figure rounded to whole cents, as an integer: a bar's height
/// only (geometry, never money).
#[must_use]
pub(crate) fn cents(a: &ReportAmount) -> Option<i64> {
    let shown = a.display(2)?;
    let (negative, digits) = match shown.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, shown.as_str()),
    };
    let whole: String = digits.chars().filter(|c| *c != '.').collect();
    let v: i64 = whole.parse().ok()?;
    Some(if negative { -v } else { v })
}

/// One month as the chart shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MonthView {
    /// The period.
    pub range: Month,
    /// The report failed: a dash, not a zero.
    pub missing: bool,
    /// Income, laid out like money ("21,400.00").
    pub income: Option<String>,
    /// Expenses.
    pub expenses: Option<String>,
    /// The report's net income.
    pub net: Option<String>,
    /// Bar heights in whole cents (geometry only).
    pub income_cents: i64,
    /// Bar heights in whole cents (geometry only).
    pub expense_cents: i64,
}

/// The months as shown, money laid out in `style`.
#[must_use]
pub(crate) fn views(months: &[MonthPl], style: MoneyStyle) -> Vec<MonthView> {
    months
        .iter()
        .map(|m| match &m.pl {
            Ok(pl) => MonthView {
                range: m.range,
                missing: false,
                income: pl.total_income.as_ref().and_then(|a| report_text(a, style)),
                expenses: pl
                    .total_expenses
                    .as_ref()
                    .and_then(|a| report_text(a, style)),
                net: report_text(&pl.net_income, style),
                income_cents: pl.total_income.as_ref().and_then(cents).unwrap_or(0),
                expense_cents: pl.total_expenses.as_ref().and_then(cents).unwrap_or(0),
            },
            Err(_) => MonthView {
                range: m.range,
                missing: true,
                income: None,
                expenses: None,
                net: None,
                income_cents: 0,
                expense_cents: 0,
            },
        })
        .collect()
}

/// The full month name ("October").
#[must_use]
pub(crate) fn month_long(month: u32) -> String {
    t(&format!("home.month_long.{month}"))
}

/// What a screen reader says for a month ("October: income 21,400.00,
/// expenses 7,534.86").
#[must_use]
pub(crate) fn accessible(v: &MonthView) -> String {
    let month = month_long(v.range.named);
    if v.missing {
        return t_with("home.chart.group.missing", &[("month", &month)]);
    }
    let dash = "\u{2014}".to_string();
    t_with(
        "home.chart.group",
        &[
            ("month", &month),
            ("income", v.income.as_ref().unwrap_or(&dash)),
            ("expenses", v.expenses.as_ref().unwrap_or(&dash)),
        ],
    )
}

/// The bar groups.
#[must_use]
pub(crate) fn groups(views: &[MonthView], tokens: &Tokens) -> Vec<BarGroup> {
    views
        .iter()
        .map(|v| BarGroup {
            label: super::lanes::month_short(v.range.named),
            accessible: accessible(v),
            bars: (!v.missing).then(|| {
                vec![
                    Bar {
                        value: v.income_cents,
                        color: tokens.accent,
                    },
                    Bar {
                        value: v.expense_cents,
                        color: tokens.warn_badge,
                    },
                ]
            }),
        })
        .collect()
}

/// What the card asks Home to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    /// Read again.
    Retry,
    /// Open the Profit & Loss of this month.
    Drill(Month),
}

/// Whether there is nothing to draw yet: every month read, all zero.
#[must_use]
pub(crate) fn empty(views: &[MonthView]) -> bool {
    views.is_empty()
        || views
            .iter()
            .all(|v| !v.missing && v.income_cents == 0 && v.expense_cents == 0)
}

/// The Income and expenses card.
pub(super) fn show(ui: &mut Ui, home: &HomeScreen, tokens: &Tokens) -> Option<Action> {
    heading(ui, tokens, &t("home.chart.title"));
    let slot = &home.data.chart;
    let Some(months) = slot.value.as_ref() else {
        if let Some(e) = &slot.error {
            return failed(ui, tokens, "home.chart.failed", e).then_some(Action::Retry);
        }
        note(ui, tokens, &t("home.chart.loading"));
        return None;
    };
    let plain = crate::grid_env::shell_frame(ui.ctx()).money_style;
    let views = views(months, plain);
    if let Some(e) = months
        .iter()
        .all(|m| m.pl.is_err())
        .then(|| months.iter().find_map(|m| m.pl.as_ref().err()))
        .flatten()
    {
        return failed(ui, tokens, "home.chart.failed", e).then_some(Action::Retry);
    }
    if empty(&views) {
        note(ui, tokens, &t("home.chart.empty"));
        return None;
    }
    let mut action = None;
    egui::Frame::new()
        .inner_margin(Margin::same(14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let chart = BarChart {
                id: ui.scope_id().with("home-chart"),
                plot_height: PLOT_H,
            };
            if let Some(i) = chart.show(ui, &groups(&views, tokens)) {
                action = views.get(i).map(|v| Action::Drill(v.range));
            }
            ui.add_space(10.0);
            let mut style = plain;
            style.symbol = true;
            legend(ui, tokens, months.last().map(|m| profit(m, style)));
        });
    action
}

/// The month's name and the engine's net income for it, with the `$`
/// (`None` when its report is missing).
fn profit(m: &MonthPl, style: MoneyStyle) -> (u32, Option<String>) {
    let figure =
        m.pl.as_ref()
            .ok()
            .and_then(|pl| report_text(&pl.net_income, style));
    (m.range.named, figure)
}

/// Income and Expenses swatches, then the last month's profit at the right.
fn legend(ui: &mut Ui, tokens: &Tokens, last: Option<(u32, Option<String>)>) {
    let small = |text: String| {
        egui::Label::new(RichText::new(text).size(12.0).color(tokens.text_secondary))
            .selectable(false)
    };
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        for (key, color) in [
            ("home.chart.income", tokens.accent),
            ("home.chart.expenses", tokens.warn_badge),
        ] {
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
            ui.painter().rect_filled(rect, 0.0, color);
            ui.add(small(t(key)));
            ui.add_space(10.0);
        }
        let Some((month, figure)) = last else { return };
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.add(
                egui::Label::new(
                    RichText::new(figure.unwrap_or_else(|| "\u{2014}".into()))
                        .size(12.0)
                        .family(egui::FontFamily::Monospace)
                        .color(tokens.text_primary),
                )
                .selectable(false),
            );
            ui.add(small(t_with(
                "home.chart.profit",
                &[("month", &month_long(month))],
            )));
        });
    });
}

/// The months read (the control channel: counts only, never figures).
#[must_use]
pub(crate) fn state(home: &HomeScreen) -> serde_json::Value {
    let months = home.data.chart.value.as_ref();
    serde_json::json!({
        "months": months.map(Vec::len),
        "missing": months.map(|m| m.iter().filter(|m| m.pl.is_err()).count()),
        "labels": months.map(|m| m.iter().map(|m| super::lanes::month_short(m.range.named)).collect::<Vec<_>>()),
        "drill": home.chart_drill.map(|m| format!("{}..{}", m.start.to_wire(), m.end.to_wire())),
    })
}
