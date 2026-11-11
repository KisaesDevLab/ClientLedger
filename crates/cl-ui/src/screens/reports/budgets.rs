//! Budgets (Q-65, U-203; P28 WP4): simple P&L budgets of twelve fiscal
//! periods, edited in a panel of the report window beside Budget vs.
//! Actual.
//!
//! - A list of the company's budgets and a new budget for a fiscal year.
//! - An accounts × 12 periods editor. Cells are owner-typed amounts
//!   (`Money::parse_user`, to the cent); a row is saved when the owner
//!   leaves it, with the budget's current version. Saves go one at a time:
//!   each answer carries the next version, which the next save uses.
//! - Every total (a line's, each section's per period, net income) is the
//!   engine's, read back after each save; a row being edited says "Totals
//!   update when saved".
//! - Fill from actuals, adjust by a percent and copy run in the engine,
//!   one Idempotency-Key per click, and wait while a row is unsaved.
//! - A 409 (someone else changed the budget) offers Reload.

use std::collections::{BTreeMap, BTreeSet};

use cl_api::resources::reports::budgets::{
    Budget, BudgetLineWrite, BudgetLines, BudgetRefusal, NewBudget, PERIODS, budget_refusal,
    parse_percent,
};
use cl_api::{ApiError, IdempotencyKey, LocalValidation, OrphanLabel};
use cl_core::ids::AccountId;
use cl_core::money::UserInput;
use cl_core::{Money, MoneyStyle};
use eframe::egui::{self, RichText, Ui};
use rust_decimal::Decimal;

use super::window::ReportWindow;
use crate::i18n::{t, t_with};
use crate::screens::company::form;
use crate::screens::invoice::engine::{self, Job};
use crate::theme::Tokens;

/// One P&L account row of the editor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BudgetAccount {
    pub id: AccountId,
    /// `number · name`.
    pub caption: String,
    /// The engine's type (the section).
    pub kind: String,
}

/// The P&L account types, in report order.
const PL_TYPES: [&str; 5] = [
    "revenue",
    "cogs",
    "expense",
    "other_revenue",
    "other_expense",
];

/// Rows a save sent, with the text they had.
type Sent = Vec<(AccountId, [String; PERIODS])>;

/// The budgets panel's state.
#[derive(Default)]
pub(crate) struct BudgetPanel {
    /// Shown.
    pub open: bool,
    pub(super) list: Option<Vec<Budget>>,
    list_job: Option<Job<Vec<Budget>>>,
    accounts: Option<Vec<BudgetAccount>>,
    accounts_job: Option<Job<Vec<BudgetAccount>>>,
    /// The budget being edited.
    pub selected: Option<String>,
    pub(crate) lines: Option<BudgetLines>,
    lines_job: Option<Job<BudgetLines>>,
    /// The owner's text of rows being edited.
    pub(crate) drafts: BTreeMap<AccountId, [String; PERIODS]>,
    /// Rows the owner left, waiting to be saved.
    pub(crate) pending: BTreeSet<AccountId>,
    /// The save in flight: its rows and the text it sent.
    save: Option<(Sent, Job<BudgetLines>)>,
    /// Fill, adjust or copy in flight.
    action: Option<Job<BudgetLines>>,
    create: Option<Job<Budget>>,
    /// The budget changed elsewhere (409): Reload.
    pub(crate) stale: bool,
    pub(crate) error: Option<ApiError>,
    /// A cell that isn't an amount: (account, period).
    pub(crate) bad_cell: Option<(AccountId, usize)>,
    pub(crate) percent: String,
    pub(crate) new_year: String,
    pub(crate) new_name: String,
    copy_from: Option<String>,
}

impl BudgetPanel {
    /// True while anything is in flight.
    pub(crate) fn busy(&self) -> bool {
        self.list_job.is_some()
            || self.accounts_job.is_some()
            || self.lines_job.is_some()
            || self.save.is_some()
            || self.action.is_some()
            || self.create.is_some()
    }

    /// True when a row has unsaved text (fill, adjust and copy wait).
    pub(crate) fn has_unsaved(&self) -> bool {
        !self.drafts.is_empty() || !self.pending.is_empty() || self.save.is_some()
    }

    /// The control channel's view.
    pub(crate) fn state_json(&self) -> serde_json::Value {
        serde_json::json!({
            "open": self.open,
            "budgets": self.list.as_ref().map_or(0, Vec::len),
            "selected": self.selected.is_some(),
            "lines": self.lines.as_ref().map_or(0, |l| l.lines.len()),
            "unsaved": self.drafts.len() + self.pending.len(),
            "saving": self.save.is_some(),
            "stale": self.stale,
            "error": self.error.is_some(),
            "bad_cell": self.bad_cell.is_some(),
        })
    }
}

/// A cell's value as the editor shows it (2 places, no separators: the
/// text the owner edits).
pub(crate) fn cell_text(d: Decimal) -> String {
    let mut r = d.round_dp(2);
    r.rescale(2);
    r.to_string()
}

/// The twelve cells of a row from the owner's text, or the first that
/// isn't an amount.
///
/// # Errors
/// The period index (0-11) of the first bad cell.
pub(crate) fn parse_row(cells: &[String; PERIODS]) -> Result<[Money; PERIODS], usize> {
    let mut out = [Money::ZERO; PERIODS];
    for (i, c) in cells.iter().enumerate() {
        let t = c.trim();
        out[i] = if t.is_empty() {
            Money::ZERO
        } else {
            Money::parse_user(t, UserInput::MONEY).map_err(|_| i)?
        };
    }
    Ok(out)
}

/// The engine's cells of `account` (zeros when it has no line yet).
fn values(lines: Option<&BudgetLines>, account: AccountId) -> [Decimal; PERIODS] {
    lines
        .and_then(|l| l.lines.iter().find(|x| x.account_id == account))
        .map_or([Decimal::ZERO; PERIODS], |l| l.periods)
}

fn orphan(what: &'static str) -> OrphanLabel {
    OrphanLabel { what, key: None }
}

/// Opens or closes the panel; opening loads the budgets and accounts.
pub(super) fn toggle(w: &mut ReportWindow) {
    w.budgets.open = !w.budgets.open;
    if w.budgets.open {
        load(w);
    }
}

/// Loads the budget list and the P&L accounts.
pub(super) fn load(w: &mut ReportWindow) {
    let p = &mut w.budgets;
    if p.list_job.is_none() {
        p.list_job = Some(engine::read(
            &w.link,
            w.group.as_ref(),
            |_| Ok(Vec::new()),
            |s| async move { s.budgets().list().await },
        ));
    }
    if p.accounts.is_none() && p.accounts_job.is_none() {
        p.accounts_job = Some(engine::read(
            &w.link,
            w.group.as_ref(),
            |_| Ok(Vec::new()),
            |s| async move {
                let all = s
                    .accounts()
                    .list_all(&cl_api::AccountFilter::default())
                    .await?;
                let mut rows: Vec<(usize, String, BudgetAccount)> = all
                    .into_iter()
                    .filter(|a| a.is_active)
                    .filter_map(|a| {
                        let kind = a.fields.account_type.as_str();
                        let order = PL_TYPES.iter().position(|k| *k == kind)?;
                        let number = a.fields.account_number.clone().unwrap_or_default();
                        let caption = if number.is_empty() {
                            a.fields.name.clone()
                        } else {
                            format!("{number} \u{b7} {}", a.fields.name)
                        };
                        Some((
                            order,
                            caption.to_lowercase(),
                            BudgetAccount {
                                id: a.id,
                                caption,
                                kind: kind.to_string(),
                            },
                        ))
                    })
                    .collect();
                rows.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
                Ok(rows.into_iter().map(|r| r.2).collect())
            },
        ));
    }
}

/// Opens budget `id` in the editor.
pub(super) fn select(w: &mut ReportWindow, id: String) {
    let p = &mut w.budgets;
    p.selected = Some(id.clone());
    p.lines = None;
    p.drafts.clear();
    p.pending.clear();
    p.stale = false;
    p.error = None;
    p.bad_cell = None;
    p.lines_job = Some(engine::read(
        &w.link,
        w.group.as_ref(),
        |_| Err(ApiError::Local(LocalValidation::Unsupported)),
        move |s| async move { s.budgets().lines(&id).await },
    ));
}

/// The owner left row `account`: it saves when its cells are amounts.
pub(crate) fn commit_row(p: &mut BudgetPanel, account: AccountId) {
    let Some(cells) = p.drafts.get(&account) else {
        return;
    };
    match parse_row(cells) {
        Ok(_) => {
            p.bad_cell = None;
            p.pending.insert(account);
        }
        Err(i) => p.bad_cell = Some((account, i)),
    }
}

/// Starts the next save: every row waiting, at the budget's current
/// version, one save at a time.
fn start_save(w: &mut ReportWindow) {
    let p = &mut w.budgets;
    if p.save.is_some() || p.stale || p.pending.is_empty() {
        return;
    }
    let (Some(id), Some(lines)) = (p.selected.clone(), p.lines.as_ref()) else {
        return;
    };
    let version = lines.budget.version;
    let mut sent = Vec::new();
    let mut writes = Vec::new();
    for a in std::mem::take(&mut p.pending) {
        let Some(text) = p.drafts.get(&a).cloned() else {
            continue;
        };
        let Ok(periods) = parse_row(&text) else {
            continue;
        };
        writes.push(BudgetLineWrite {
            account_id: a,
            periods,
        });
        sent.push((a, text));
    }
    if writes.is_empty() {
        return;
    }
    let key = IdempotencyKey::new();
    let job = engine::write(
        &w.link,
        w.group.as_ref(),
        orphan("orphan.budget_save"),
        |_| Err(ApiError::Local(LocalValidation::Unsupported)),
        move |s| async move { s.budgets().save_lines(&id, version, &writes, key).await },
    );
    p.save = Some((sent, job));
}

/// What a toolbar click asks the engine to do to the whole budget.
enum Whole {
    Fill,
    Adjust(Decimal),
    Copy(String),
}

fn start_whole(w: &mut ReportWindow, what: Whole) {
    let p = &mut w.budgets;
    if p.has_unsaved() || p.action.is_some() || p.stale {
        return;
    }
    let (Some(id), Some(lines)) = (p.selected.clone(), p.lines.as_ref()) else {
        return;
    };
    let version = lines.budget.version;
    let basis = w
        .options
        .basis
        .map_or("accrual", cl_core::report::Basis::as_str);
    // One key per click: a retried adjust is replayed, never applied twice.
    let key = IdempotencyKey::new();
    p.error = None;
    p.action = Some(engine::write(
        &w.link,
        w.group.as_ref(),
        orphan("orphan.budget_change"),
        |_| Err(ApiError::Local(LocalValidation::Unsupported)),
        move |s| async move {
            let b = s.budgets();
            match what {
                Whole::Fill => b.fill_from_actuals(&id, version, Some(basis), key).await,
                Whole::Adjust(pct) => b.adjust_by_percent(&id, version, pct, key).await,
                Whole::Copy(from) => b.copy_from(&id, version, &from, key).await,
            }
        },
    ));
}

fn start_create(w: &mut ReportWindow) {
    let p = &mut w.budgets;
    let Ok(year) = p.new_year.trim().parse::<i32>() else {
        p.error = Some(ApiError::Local(LocalValidation::InvalidValue));
        return;
    };
    let name = if p.new_name.trim().is_empty() {
        t_with(
            "reports.budget.default_name",
            &[("year", &year.to_string())],
        )
    } else {
        p.new_name.trim().to_string()
    };
    let key = IdempotencyKey::new();
    p.error = None;
    p.create = Some(engine::write(
        &w.link,
        w.group.as_ref(),
        orphan("orphan.budget_create"),
        |_| Err(ApiError::Local(LocalValidation::Unsupported)),
        move |s| async move {
            s.budgets()
                .create(
                    &NewBudget {
                        name,
                        fiscal_year: year,
                    },
                    key,
                )
                .await
        },
    ));
}

/// A finished save: adopt the engine's lines and version; rows edited
/// again meanwhile stay as typed.
pub(crate) fn saved(
    p: &mut BudgetPanel,
    sent: Vec<(AccountId, [String; PERIODS])>,
    r: Result<BudgetLines, ApiError>,
) {
    match r {
        Ok(lines) => {
            for (a, text) in sent {
                if p.drafts.get(&a) == Some(&text) {
                    p.drafts.remove(&a);
                }
            }
            p.lines = Some(lines);
            p.error = None;
        }
        Err(e) => {
            // The rows stay unsaved, so nothing typed is lost.
            for (a, _) in sent {
                p.pending.insert(a);
            }
            if budget_refusal(&e) == Some(BudgetRefusal::Stale) {
                p.stale = true;
                p.pending.clear();
            }
            p.error = Some(e);
        }
    }
}

/// Polls the panel's jobs.
pub(super) fn poll(w: &mut ReportWindow) {
    let mut reload = None;
    {
        let p = &mut w.budgets;
        if let Some(r) = engine::poll(&mut p.list_job) {
            match r {
                Ok(v) => p.list = Some(v),
                Err(e) => p.error = Some(e),
            }
        }
        if let Some(r) = engine::poll(&mut p.accounts_job) {
            p.accounts = Some(r.unwrap_or_default());
        }
        if let Some(r) = engine::poll(&mut p.lines_job) {
            match r {
                Ok(l) => {
                    p.lines = Some(l);
                    p.stale = false;
                }
                Err(e) => p.error = Some(e),
            }
        }
        if let Some((_, job)) = p.save.as_mut()
            && let Some(r) = job.take()
            && let Some((sent, _)) = p.save.take()
        {
            saved(p, sent, r);
        }
        if let Some(r) = engine::poll(&mut p.action) {
            match r {
                Ok(l) => {
                    p.lines = Some(l);
                    p.error = None;
                }
                Err(e) => {
                    p.stale = budget_refusal(&e) == Some(BudgetRefusal::Stale);
                    p.error = Some(e);
                }
            }
        }
        if let Some(r) = engine::poll(&mut p.create) {
            match r {
                Ok(b) => {
                    p.new_name.clear();
                    p.list.get_or_insert_with(Vec::new).push(b.clone());
                    reload = Some(b.id);
                }
                Err(e) => p.error = Some(e),
            }
        }
    }
    if let Some(id) = reload {
        select(w, id);
    }
    start_save(w);
}

fn refusal_text(e: &ApiError) -> Option<String> {
    Some(t(match budget_refusal(e)? {
        BudgetRefusal::Stale => "reports.budget.stale",
        BudgetRefusal::YearTaken => "reports.budget.year_taken",
        BudgetRefusal::LineInvalid => "reports.budget.line_invalid",
    }))
}

/// A short labelled text input.
fn small_field(ui: &mut Ui, label: &str, value: &mut String, width: f32) {
    let l = ui.label(RichText::new(label).size(12.0));
    ui.add(egui::TextEdit::singleline(value).desired_width(width))
        .labelled_by(l.id);
}

const CELL_W: f32 = 78.0;
const CAPTION_W: f32 = 200.0;

/// The panel.
#[allow(clippy::too_many_lines, reason = "one panel, top to bottom")]
pub(super) fn ui(w: &mut ReportWindow, ui: &mut Ui, tokens: &Tokens) {
    if !w.budgets.open {
        return;
    }
    let style = MoneyStyle {
        symbol: false,
        ..crate::grid_env::shell_frame(ui.ctx()).money_style
    };
    let mut pick: Option<String> = None;
    let mut whole: Option<Whole> = None;
    let mut create = false;
    let mut reload = false;
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.label(RichText::new(t("reports.budget.title")).strong());
        let p = &mut w.budgets;
        ui.horizontal_wrapped(|ui| {
            let l = ui.label(RichText::new(t("reports.budget.pick")).size(12.0));
            let list = p.list.clone().unwrap_or_default();
            let current = p
                .selected
                .as_ref()
                .and_then(|s| list.iter().find(|b| &b.id == s))
                .map_or_else(|| t("reports.budget.choose"), |b| b.name.clone());
            egui::ComboBox::from_id_salt("budget_pick")
                .selected_text(current)
                .show_ui(ui, |ui| {
                    for b in &list {
                        if ui
                            .selectable_label(p.selected.as_ref() == Some(&b.id), &b.name)
                            .clicked()
                        {
                            pick = Some(b.id.clone());
                        }
                    }
                })
                .response
                .labelled_by(l.id);
            ui.separator();
            small_field(ui, &t("reports.budget.new_year"), &mut p.new_year, 60.0);
            small_field(ui, &t("reports.budget.new_name"), &mut p.new_name, 160.0);
            if ui
                .add_enabled(
                    p.create.is_none(),
                    egui::Button::new(t("reports.budget.create")),
                )
                .clicked()
            {
                create = true;
            }
        });
        if let Some(e) = &p.error {
            match refusal_text(e) {
                Some(s) => form::inline_warning(ui, tokens, &s),
                None => form::inline_error(ui, tokens, e),
            }
        }
        if p.stale && ui.button(t("reports.budget.reload")).clicked() {
            reload = true;
        }
        let Some(lines) = p.lines.clone() else {
            if p.selected.is_some() {
                form::progress_line(ui, tokens, &t("reports.loading"));
            }
            return;
        };
        let can_whole = !p.has_unsaved() && p.action.is_none() && !p.stale;
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(can_whole, egui::Button::new(t("reports.budget.fill")))
                .on_hover_text(t("reports.budget.fill.hint"))
                .clicked()
            {
                whole = Some(Whole::Fill);
            }
            small_field(ui, &t("reports.budget.percent"), &mut p.percent, 50.0);
            if ui
                .add_enabled(can_whole, egui::Button::new(t("reports.budget.adjust")))
                .clicked()
            {
                match parse_percent(&p.percent) {
                    Ok(d) => whole = Some(Whole::Adjust(d)),
                    Err(e) => p.error = Some(e),
                }
            }
            let others: Vec<Budget> = p
                .list
                .iter()
                .flatten()
                .filter(|b| Some(&b.id) != p.selected.as_ref())
                .cloned()
                .collect();
            let shown = p
                .copy_from
                .as_ref()
                .and_then(|c| others.iter().find(|b| &b.id == c))
                .map_or_else(|| t("reports.budget.copy_from"), |b| b.name.clone());
            egui::ComboBox::from_id_salt("budget_copy_from")
                .selected_text(shown)
                .show_ui(ui, |ui| {
                    for b in &others {
                        ui.selectable_value(&mut p.copy_from, Some(b.id.clone()), &b.name);
                    }
                });
            if ui
                .add_enabled(
                    can_whole && p.copy_from.is_some(),
                    egui::Button::new(t("reports.budget.copy")),
                )
                .clicked()
                && let Some(c) = p.copy_from.clone()
            {
                whole = Some(Whole::Copy(c));
            }
            if !can_whole && p.has_unsaved() {
                ui.label(
                    RichText::new(t("reports.budget.save_first"))
                        .size(12.0)
                        .color(tokens.text_secondary),
                );
            }
        });
        // The grid: accounts x 12 periods, the engine's total.
        let accounts = p.accounts.clone().unwrap_or_default();
        egui::ScrollArea::both()
            .id_salt("budget_grid")
            .max_height(360.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.add_sized(
                        [CAPTION_W, 18.0],
                        egui::Label::new(t("reports.budget.account")),
                    );
                    for i in 0..PERIODS {
                        ui.add_sized(
                            [CELL_W, 18.0],
                            egui::Label::new(t_with(
                                "reports.budget.period",
                                &[("n", &(i + 1).to_string())],
                            )),
                        );
                    }
                    ui.add_sized(
                        [CELL_W + 20.0, 18.0],
                        egui::Label::new(t("reports.budget.total")),
                    );
                });
                for a in &accounts {
                    let engine_values = values(Some(&lines), a.id);
                    let mut left_row = false;
                    ui.horizontal(|ui| {
                        ui.add_sized([CAPTION_W, 20.0], egui::Label::new(&a.caption).truncate());
                        for i in 0..PERIODS {
                            let mut text = p
                                .drafts
                                .get(&a.id)
                                .map_or_else(|| cell_text(engine_values[i]), |d| d[i].clone());
                            let bad = p.bad_cell == Some((a.id, i));
                            let mut edit = egui::TextEdit::singleline(&mut text)
                                .desired_width(CELL_W)
                                .align(egui::Align2::RIGHT_CENTER);
                            if bad {
                                edit = edit.text_color(tokens.warn_text);
                            }
                            let r = ui.add(edit);
                            let name = t_with(
                                "reports.budget.cell",
                                &[("account", &a.caption), ("n", &(i + 1).to_string())],
                            );
                            r.widget_info(|| {
                                egui::WidgetInfo::labeled(egui::Role::TextInput, true, &name)
                            });
                            if r.changed() {
                                let row = p
                                    .drafts
                                    .entry(a.id)
                                    .or_insert_with(|| engine_values.map(cell_text));
                                row[i] = text;
                            }
                            if r.lost_focus() {
                                left_row = true;
                            }
                        }
                        let total = if p.drafts.contains_key(&a.id) || p.pending.contains(&a.id) {
                            t("reports.budget.totals_on_save")
                        } else {
                            lines
                                .lines
                                .iter()
                                .find(|l| l.account_id == a.id)
                                .map_or_else(String::new, |l| {
                                    cl_core::report::display_amount(l.total, style)
                                })
                        };
                        ui.add_sized(
                            [CELL_W + 20.0, 20.0],
                            egui::Label::new(
                                RichText::new(total).family(egui::FontFamily::Monospace),
                            ),
                        );
                    });
                    if left_row {
                        commit_row(p, a.id);
                    }
                }
                ui.separator();
                for (key, row) in [
                    ("reports.pl.total_income", &lines.totals.revenue),
                    ("reports.pl.total_cogs", &lines.totals.cogs),
                    ("reports.pl.total_expenses", &lines.totals.expenses),
                    ("reports.pl.total_other_income", &lines.totals.other_revenue),
                    (
                        "reports.pl.total_other_expenses",
                        &lines.totals.other_expenses,
                    ),
                    ("reports.pl.net_income", &lines.totals.net_income),
                ] {
                    ui.horizontal(|ui| {
                        ui.add_sized(
                            [CAPTION_W, 18.0],
                            egui::Label::new(RichText::new(t(key)).strong()),
                        );
                        for v in row.periods.iter().chain(std::iter::once(&row.total)) {
                            ui.add_sized(
                                [CELL_W, 18.0],
                                egui::Label::new(
                                    RichText::new(cl_core::report::display_amount(*v, style))
                                        .family(egui::FontFamily::Monospace),
                                ),
                            );
                        }
                    });
                }
            });
        if p.bad_cell.is_some() {
            form::inline_warning(ui, tokens, &t("reports.budget.bad_cell"));
        }
    });
    if let Some(id) = pick {
        select(w, id);
    }
    if create {
        start_create(w);
    }
    if reload && let Some(id) = w.budgets.selected.clone() {
        select(w, id);
    }
    if let Some(what) = whole {
        start_whole(w, what);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "11111111-1111-4111-8111-111111111111";

    fn row(first: &str) -> [String; PERIODS] {
        let mut r: [String; PERIODS] = Default::default();
        r[0] = first.to_string();
        r
    }

    #[test]
    fn cells_are_owner_amounts_to_the_cent() {
        let ok = parse_row(&row("1,234.5")).expect("row");
        assert_eq!(ok[0].to_wire(), "1234.5000");
        assert_eq!(ok[1], Money::ZERO, "blank is zero");
        assert_eq!(parse_row(&row("12x")), Err(0));
        assert_eq!(cell_text(Decimal::new(12_345, 1)), "1234.50");
    }

    #[test]
    fn a_row_saves_only_when_its_cells_are_amounts() {
        let a = AccountId::parse(A).expect("id");
        let mut p = BudgetPanel::default();
        p.drafts.insert(a, row("nope"));
        commit_row(&mut p, a);
        assert!(p.pending.is_empty());
        assert_eq!(p.bad_cell, Some((a, 0)));
        p.drafts.insert(a, row("10"));
        commit_row(&mut p, a);
        assert!(p.pending.contains(&a));
        assert!(p.bad_cell.is_none());
        assert!(p.has_unsaved(), "fill, adjust and copy wait");
    }

    #[test]
    fn a_stale_save_keeps_the_text_and_asks_for_reload() {
        let a = AccountId::parse(A).expect("id");
        let mut p = BudgetPanel::default();
        p.drafts.insert(a, row("10"));
        let stale = cl_api::engine_refusal(
            409,
            &serde_json::json!({"error": {"message": "x", "code": "STALE_VERSION", "details": {"current": 4}}}),
        );
        saved(&mut p, vec![(a, row("10"))], Err(stale));
        assert!(p.stale);
        assert_eq!(p.drafts.get(&a), Some(&row("10")), "nothing typed is lost");
    }
}
