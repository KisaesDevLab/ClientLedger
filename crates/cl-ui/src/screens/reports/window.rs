//! The report window's state machine and drawing (plan §4):
//!
//! ```text
//! Empty -> Running{gen} -> Shown | Failed
//! Shown --option change--> Stale (dimmed) -> Running
//! Failed --Retry--> Running
//! ```
//!
//! A response from an older generation is dropped. Presets, basis and
//! column changes run at once.

use std::sync::Arc;

use cl_api::{ApiError, RequestGroup};
use cl_core::fiscal::FiscalYearSpec;
use cl_core::report::{Cell, ReportTable, RowKind, RowPath, display_amount, display_percent};
use cl_core::{CalendarDate, MoneyStyle};
use cl_model::reports::{ReportOptions, ReportSpec, SortMode, resolve};
use cl_model::{CommandId, Scope};
use eframe::egui::{self, Align, Layout, RichText, Ui, Vec2};
use serde_json::{Value, json};

use super::{ReportOpen, available, demo_run, label};
use crate::company::CompanyLink;
use crate::i18n::{t, t_with};
use crate::screens::company::form;
use crate::screens::invoice::engine::{self, Job};
use crate::theme::Tokens;
use crate::windows::{OpenPayload, Screen};

/// Where the run stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RunState {
    /// Nothing asked yet (the fiscal calendar is being read).
    Empty,
    /// Asked; waiting for generation `gen`.
    Running,
    /// The table is current.
    Shown,
    /// Options changed since the table was made (drawn dimmed).
    Stale,
    /// The last run failed.
    Failed,
    /// The engine can't run this report yet (Q-220).
    Unavailable,
}

impl RunState {
    const fn name(self) -> &'static str {
        match self {
            RunState::Empty => "empty",
            RunState::Running => "running",
            RunState::Shown => "shown",
            RunState::Stale => "stale",
            RunState::Failed => "failed",
            RunState::Unavailable => "unavailable",
        }
    }
}

const ROW_H: f32 = 22.0;
/// The caption column's bounds; a longer caption ends in an ellipsis.
const CAPTION_MIN: f32 = 200.0;
const CAPTION_MAX: f32 = 420.0;
/// A text column's bounds (names, memos); amounts are never cut (QA-5).
const TEXT_MIN: f32 = 56.0;
const TEXT_MAX: f32 = 260.0;
const NUM_MIN: f32 = 72.0;
/// Room around a cell's text.
const PAD: f32 = 14.0;

/// What the column widths were measured for. The table's serial changes
/// with every table shown, not with every run asked: the dimmed old table
/// drawn while a run is pending must not lend its widths to the new one
/// (PR6-QA-1).
#[derive(Clone, Copy, PartialEq, Eq)]
struct WidthKey {
    table: u64,
    columns: usize,
    rows: usize,
    collapsed: usize,
    zoom_bits: u32,
}

/// Column widths of the table on screen (caption first), measured once per
/// table (QA-2, QA-3, QA-5).
#[derive(Clone, PartialEq)]
pub(crate) struct Widths {
    key: WidthKey,
    caption: f32,
    cells: Vec<f32>,
    /// Text columns whose longest text is a short code or status (a
    /// number, "Below $2,000"): [`fit`] doesn't cut them.
    short: Vec<bool>,
}

/// A text column with no text longer than this keeps its width.
const SHORT_TEXT: usize = 14;

/// The pick lists: (id text, name).
#[derive(Default)]
pub(crate) struct Picks {
    pub(super) vendors: Option<Vec<(String, String)>>,
    vendors_job: Option<Job<Vec<(String, String)>>>,
    pub(super) banks: Option<Vec<(String, String)>>,
    banks_job: Option<Job<Vec<(String, String)>>>,
}

/// A report window.
pub struct ReportWindow {
    pub(super) link: Arc<CompanyLink>,
    pub(super) group: Option<RequestGroup>,
    kind: CommandId,
    pub(super) today: CalendarDate,
    /// The company's name (header).
    pub(super) company: String,
    pub(super) fiscal: FiscalYearSpec,
    calendar: cl_settings::FiscalCalendar,
    fiscal_job: Option<Job<Option<u8>>>,
    /// The company's tag groups (id, name), for the tag split and the
    /// Tag group columns.
    pub(super) tag_groups: Vec<(cl_core::ids::TagGroupId, String)>,
    tag_groups_job: Option<Job<Vec<(cl_core::ids::TagGroupId, String)>>>,
    /// Vendors and bank accounts (id text, name) for the reports that list
    /// one of them (Transactions by Vendor, Vendor Statement, Check
    /// Register); loaded once, on the first such report.
    pub(super) picks: Picks,
    /// The filters' pick lists and the amount filter's text (B3).
    pub(super) filter_lists: super::filter_lists::FilterLists,
    /// The budgets panel (Budget vs. Actual).
    pub(super) budgets: super::budgets::BudgetPanel,
    /// The ask-a-question bar.
    pub(super) ask: super::ask::AskBar,
    /// The Email form (U-207).
    pub(super) mailer: super::email::Mailer,
    pub(super) spec: &'static ReportSpec,
    pub(super) options: ReportOptions,
    pub(crate) state: RunState,
    generation: u64,
    job: Option<(u64, Job<ReportTable>)>,
    pub(super) table: Option<ReportTable>,
    /// Counts the tables shown (the width cache's key).
    table_serial: u64,
    pub(super) error: Option<ApiError>,
    pub(super) problem: Option<&'static str>,
    pub(super) ran_at: Option<String>,
    /// The From/To (As of) inputs.
    pub(super) dates: super::filters::DateInputs,
    /// The catalog's search text.
    pub(super) search: String,
    /// The options panel (Customize).
    pub(super) customize: bool,
    pub(super) export: super::export::Exporter,
    pub(super) memo: super::memorized::Memo,
    /// A plain message (drill-down that has no target yet, export done).
    pub(super) notice: Option<String>,
    /// A drill's read of the transaction it opens (QA-1).
    pub(super) drill_job: Option<Job<super::drill::Opens>>,
    pub(super) drill_id: Option<cl_core::ids::TransactionId>,
    /// The measured column widths and the rows' horizontal scroll, which
    /// the heading row follows.
    widths: Option<Widths>,
    /// The fiscal year's read failed; Refresh reads it again (S8).
    fiscal_failed: bool,
    h_offset: f32,
    /// The report list beside the report: `None` follows the window's
    /// width (hidden when narrow, as at 200% zoom, QA-14).
    pub(super) catalog_open: Option<bool>,
    pub(super) drill_error: Option<ApiError>,
    /// The Report Builder editor's Export or Print, done once this tab's
    /// run is shown (Q-A08m).
    pub(super) after_run: Option<super::builder_ref::AfterRun>,
    pub(super) bus: crate::data_bus::Subscription,
    /// A Report Builder report shown under the stand-in spec (A07).
    pub(super) builder: Option<Box<super::builder_run::Builder>>,
    /// The Report Builder starters (catalog column).
    pub(super) starters: super::builder_run::Starters,
    /// A Customize in waiting (A08).
    pub(super) edit: super::builder_run::Edit,
    /// Which report is on screen: unique per window and per report shown
    /// (A07 QA-7, QA-11). Scroll positions and the export message belong
    /// to it, so a new report or a new tab opens at the top with no
    /// "Saved ..." line from the report before.
    pub(super) view: u64,
}

/// Hands out [`ReportWindow::view`] numbers.
static VIEWS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn next_view() -> u64 {
    VIEWS.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

impl ReportWindow {
    /// A window on `spec` with its defaults.
    #[must_use]
    pub fn new(
        link: Arc<CompanyLink>,
        kind: CommandId,
        open: Option<(cl_core::ids::TenantId, String)>,
        spec: &'static ReportSpec,
        today: CalendarDate,
    ) -> ReportWindow {
        let group = link.group().and_then(Result::ok);
        let calendar = company_calendar(&link, open.as_ref());

        let fiscal_job = Some(fiscal_read(&link, group.as_ref()));
        let tag_groups_job = Some(engine::read(
            &link,
            group.as_ref(),
            |d| {
                Ok(d.tag_groups
                    .clone()
                    .unwrap_or_else(crate::company::demo::lists::tag_groups)
                    .into_iter()
                    .map(|g| (g.id, g.fields.name))
                    .collect())
            },
            |s| async move {
                Ok(s.tags()
                    .groups()
                    .await?
                    .into_iter()
                    .map(|g| (g.id, g.fields.name))
                    .collect())
            },
        ));
        let mut w = ReportWindow {
            picks: Picks::default(),
            filter_lists: super::filter_lists::FilterLists::default(),
            budgets: super::budgets::BudgetPanel::default(),
            ask: super::ask::AskBar::default(),
            mailer: super::email::Mailer::default(),
            tag_groups: Vec::new(),
            tag_groups_job,
            group,
            kind,
            today,
            company: open.map(|(_, n)| n).unwrap_or_default(),
            fiscal: FiscalYearSpec::CALENDAR_YEAR,
            calendar,
            fiscal_job,
            spec,
            options: ReportOptions::for_spec(spec),
            state: RunState::Empty,
            generation: 0,
            job: None,
            table: None,
            table_serial: 0,
            error: None,
            problem: None,
            ran_at: None,
            dates: super::filters::DateInputs::new(today),
            search: String::new(),
            customize: false,
            export: super::export::Exporter::default(),
            memo: super::memorized::Memo::default(),
            notice: None,
            drill_job: None,
            drill_id: None,
            widths: None,
            fiscal_failed: false,
            h_offset: 0.0,
            catalog_open: None,
            drill_error: None,
            after_run: None,
            bus: crate::data_bus::Subscription::detached(),
            builder: None,
            starters: super::builder_run::Starters::default(),
            edit: super::builder_run::Edit::default(),
            view: next_view(),
            link,
        };
        w.bus = w.link.bus().subscribe();
        super::memorized::load(&mut w);
        w.poll();
        w
    }

    /// Switches to `spec` with `options` (catalog click, drill-down,
    /// memorized report) and runs it.
    pub(crate) fn show_report(&mut self, spec: &'static ReportSpec, options: ReportOptions) {
        self.new_view();
        self.builder = None;
        self.spec = spec;
        self.options = options;
        self.table = None;
        self.error = None;
        self.filter_lists.sync(&self.options);
        self.memo.current = None;
        self.run();
    }

    /// Another report is shown in this window (standard, builder, detail):
    /// it opens at the top, with none of the last report's messages
    /// (A07 QA-7, QA-11). Every switch goes through here.
    pub(crate) fn new_view(&mut self) {
        self.view = next_view();
        self.notice = None;
        self.drill_error = None;
        self.export.clear_message();
        self.h_offset = 0.0;
    }

    /// New options: run again when the engine's answer would change;
    /// collapse, sort and header changes are local.
    pub(crate) fn set_options(&mut self, next: ReportOptions) {
        let rerun = self.options.needs_run(&next);
        self.options = next;
        if rerun {
            // "Saved <file>." was about the report before (QA-15).
            self.export.clear_message();
            if self.table.is_some() {
                self.state = RunState::Stale;
            }
            self.run();
        }
    }

    /// The From/To inputs show the dates the choice stands for.
    fn sync_dates(&mut self) {
        use cl_model::reports::DateChoice;
        let span = match self.options.dates {
            DateChoice::Preset(p) => p.range(&self.fiscal, self.today).ok(),
            DateChoice::Fixed { from, to } => Some(cl_core::report::DateRange { from, to }),
            DateChoice::AsOf(d) => Some(cl_core::report::DateRange { from: d, to: d }),
        };
        if let Some(s) = span {
            self.dates.from.set(s.from);
            self.dates.to.set(s.to);
        }
    }

    /// Starts loading the vendor or bank account list a report needs.
    fn load_picks(&mut self) {
        use cl_model::reports::FilterKind;
        let wants = |k| self.spec.filters.contains(&k);
        if wants(FilterKind::Budget) && self.budgets.list.is_none() {
            super::budgets::load(self);
        }
        let wants = |k| self.spec.filters.contains(&k);
        if wants(FilterKind::Vendor)
            && self.picks.vendors.is_none()
            && self.picks.vendors_job.is_none()
        {
            self.picks.vendors_job = Some(engine::read(
                &self.link,
                self.group.as_ref(),
                |_| Ok(Vec::new()),
                |s| async move {
                    let f = cl_api::ContactFilter {
                        contact_type: Some(cl_api::ContactType::Vendor),
                        is_active: Some(true),
                        ..cl_api::ContactFilter::default()
                    };
                    Ok(s.contacts()
                        .list_all(&f)
                        .await?
                        .into_iter()
                        .map(|c| (c.id.to_string(), c.fields.display_name))
                        .collect())
                },
            ));
        }
        if wants(FilterKind::BankAccount)
            && self.picks.banks.is_none()
            && self.picks.banks_job.is_none()
        {
            self.picks.banks_job = Some(engine::read(
                &self.link,
                self.group.as_ref(),
                |_| Ok(Vec::new()),
                |s| async move {
                    let all = s
                        .accounts()
                        .list_all(&cl_api::AccountFilter::default())
                        .await?;
                    Ok(crate::screens::receipt_inbox::picker::bank_accounts(&all)
                        .into_iter()
                        .map(|(id, n)| (id.to_string(), n))
                        .collect())
                },
            ));
        }
    }

    /// Runs the report (Refresh, F5).
    pub(crate) fn run(&mut self) {
        self.problem = None;
        self.error = None;
        // The fiscal year couldn't be read: read it again first (S8).
        if self.fiscal_failed {
            self.fiscal_failed = false;
            self.fiscal_job = Some(fiscal_read(&self.link, self.group.as_ref()));
        }
        if self.fiscal_job.is_some() {
            self.state = RunState::Empty;
            return;
        }
        if self.builder.is_some() {
            self.sync_dates();
            super::builder_run::run(self);
            return;
        }
        if !available(&self.link, self.spec) {
            self.state = RunState::Unavailable;
            self.table = None;
            return;
        }
        self.sync_dates();
        self.load_picks();
        if let Some(k) = self.spec.missing_pick_in(&self.options) {
            self.problem = Some(match k {
                cl_model::reports::FilterKind::BankAccount => "reports.pick.choose_bank_account",
                cl_model::reports::FilterKind::Budget => "reports.pick.choose_budget",
                _ => "reports.pick.choose_vendor",
            });
            self.table = None;
            self.state = RunState::Shown;
            return;
        }
        let req = match resolve(self.spec, &self.options, &self.fiscal, self.today) {
            Ok(r) => r,
            Err(e) => {
                self.problem = Some(problem_key(&e));
                self.state = RunState::Failed;
                return;
            }
        };
        if self.budget_year_differs(&req) {
            self.problem = Some("reports.budget.other_year");
            self.table = None;
            self.state = RunState::Failed;
            return;
        }
        if !super::request_runs(&self.link, &req) {
            self.state = RunState::Unavailable;
            self.table = None;
            return;
        }
        self.generation += 1;
        let live = req.clone();
        let job = engine::read(
            &self.link,
            self.group.as_ref(),
            move |d| demo_run(d, &req),
            move |s| async move { s.reports().run(&live).await },
        );
        self.job = Some((self.generation, job));
        self.state = RunState::Running;
        self.poll();
    }

    /// Budget vs. Actual on dates outside the chosen budget's fiscal year:
    /// the engine would set another year's actuals against its periods.
    fn budget_year_differs(&self, req: &cl_core::report::ReportRequest) -> bool {
        let (Some(id), Some(list)) = (req.budget_id.as_ref(), self.budgets.list.as_ref()) else {
            return false;
        };
        let Some(start) = list
            .iter()
            .find(|b| &b.id == id)
            .and_then(|b| b.fiscal_year_start)
        else {
            return false;
        };
        let Some(first) = req.dates.span().map(|r| r.from) else {
            return false;
        };
        self.fiscal.year_containing(start).ok() != self.fiscal.year_containing(first).ok()
    }

    pub(crate) fn poll(&mut self) {
        if let Some(r) = engine::poll(&mut self.picks.vendors_job) {
            self.picks.vendors = Some(r.unwrap_or_default());
        }
        if let Some(r) = engine::poll(&mut self.picks.banks_job) {
            self.picks.banks = Some(r.unwrap_or_default());
        }
        if let Some(r) = engine::poll(&mut self.tag_groups_job) {
            // Without the list the pickers stay empty; the report still runs.
            self.tag_groups = r.unwrap_or_default();
        }
        if let Some(r) = engine::poll(&mut self.fiscal_job) {
            match r {
                Ok(start) => {
                    self.fiscal = crate::screens::home::charts::spec(start, self.calendar);
                    self.run();
                }
                // Never a silent January year on a July company (S8): the
                // error shows with Try again.
                Err(e) => {
                    self.fiscal_failed = true;
                    self.error = Some(e);
                    self.state = RunState::Failed;
                }
            }
        }
        if let Some((generation, job)) = self.job.as_mut()
            && let Some(r) = job.take()
        {
            let generation = *generation;
            self.job = None;
            // A late answer to an older run is dropped.
            if generation != self.generation {
                return;
            }
            match r {
                Ok(table) => {
                    self.set_table(table);
                    self.state = RunState::Shown;
                    self.ran_at = Some(
                        chrono::Local::now()
                            .format("%m/%d/%Y %-I:%M %p")
                            .to_string(),
                    );
                }
                Err(e) => {
                    self.error = Some(e);
                    self.state = RunState::Failed;
                }
            }
        }
        super::builder_run::poll(self);
        super::filter_lists::poll(self);
        super::drill::poll_probe(self);
        super::memorized::poll(self);
        super::budgets::poll(self);
        super::ask::poll(self);
        super::email::poll(self);
    }

    /// Shows `table`; its columns are measured afresh (PR6-QA-1).
    pub(crate) fn set_table(&mut self, table: ReportTable) {
        self.table = Some(table);
        self.table_serial += 1;
        self.widths = None;
    }

    /// The measured caption and cell widths (tests).
    #[cfg(test)]
    pub(crate) fn widths_for_test(&self) -> Option<(f32, Vec<f32>)> {
        self.widths.as_ref().map(|w| (w.caption, w.cells.clone()))
    }

    /// A row's caption: a tag row under the tag split in the owner's label
    /// style (Q-271), anything else as the table says.
    pub(crate) fn caption_of(&self, table: &ReportTable, i: usize) -> String {
        cl_model::reports::tag_caption(table, i, &self.options)
            .unwrap_or_else(|| label(&table.rows[i].caption))
    }

    /// The rows shown: collapse (Q-P28j), then sort within groups
    /// (Q-P28k).
    pub(crate) fn shown_rows(&self) -> Vec<usize> {
        let Some(table) = &self.table else {
            return Vec::new();
        };
        if let Some(b) = &self.builder {
            return super::builder_run::shown_rows(b, table, self.table_serial);
        }
        let mut rows = table.visible(&self.options.collapsed);
        // The tag split's account totals show only when asked for (Q-271).
        rows.retain(|&i| !cl_model::reports::hidden(&table.rows[i], &self.options));
        // An aging lists names with something open; a name whose every
        // column is zero shows only when asked for (QA-7). A test, not a
        // sum: the figures are the engine's.
        if self.spec.layout == cl_model::reports::Layout::Aging && !self.options.show_zero_rows {
            rows.retain(|&i| !all_zero(&table.rows[i]));
        }
        if let (Some(sort), SortMode::WithinGroups) = (&self.options.sort, self.spec.sort)
            && let Some(col) = table.columns.iter().position(|c| c.key == sort.column)
        {
            sort_within_groups(table, &mut rows, col, sort.descending);
        }
        rows
    }

    /// Collapses or expands the row at `path`.
    pub(crate) fn toggle_collapse(&mut self, path: &RowPath) {
        let mut next = self.options.clone();
        if let Some(i) = next.collapsed.iter().position(|p| p == path) {
            next.collapsed.remove(i);
        } else {
            next.collapsed.push(path.clone());
        }
        self.set_options(next);
    }

    /// True when the Collapse button would expand everything instead (the
    /// same test [`Self::toggle_collapse_all`] makes), so its label says
    /// "Expand all" (A07 QA-9).
    pub(crate) fn collapse_expands(&self) -> bool {
        match &self.builder {
            Some(b) => !super::builder_run::collapses(b),
            None => !self.options.collapsed.is_empty(),
        }
    }

    /// The Collapse button's label key.
    pub(crate) fn collapse_label(&self) -> &'static str {
        if self.collapse_expands() {
            "reports.tool.expand_all"
        } else {
            "reports.tool.collapse"
        }
    }

    /// Collapse every expandable row, or expand all when any is collapsed.
    pub(crate) fn toggle_collapse_all(&mut self) {
        if self.builder.is_some() {
            super::builder_run::toggle_all(self);
            return;
        }
        let mut next = self.options.clone();
        if next.collapsed.is_empty() {
            next.collapsed = self
                .table
                .iter()
                .flat_map(|t| t.rows.iter())
                .filter(|r| r.expandable)
                .map(|r| r.path.clone())
                .collect();
        } else {
            next.collapsed.clear();
        }
        self.set_options(next);
    }

    /// True when the table on screen is the answer to the options on
    /// screen: only then can it be printed, exported or emailed (B2).
    pub(crate) fn exportable(&self) -> bool {
        self.state == RunState::Shown && self.table.is_some()
    }

    /// The report's title as shown (the owner's rename first).
    pub(crate) fn title_text(&self) -> String {
        if let Some(b) = &self.builder {
            return self
                .options
                .header
                .title
                .clone()
                .unwrap_or_else(|| b.title());
        }
        self.options
            .header
            .title
            .clone()
            .unwrap_or_else(|| t(self.spec.title))
    }

    /// The header's dates and basis line (Q-107).
    pub(crate) fn subtitle(&self) -> String {
        let mut parts = Vec::new();
        if let Some(span) = self
            .table
            .as_ref()
            .and_then(|t| t.meta.dates.as_ref())
            .and_then(cl_core::report::ReportDates::span)
        {
            parts.push(if span.from == span.to {
                t_with(
                    "reports.header.as_of",
                    &[("date", &crate::shell::us_date(span.to))],
                )
            } else {
                format!(
                    "{}\u{2013}{}",
                    crate::shell::us_date(span.from),
                    crate::shell::us_date(span.to)
                )
            });
        }
        // The basis the table ran on, never the options' (B2): a failed or
        // pending run must not label the old figures with the new basis.
        let basis = self
            .table
            .as_ref()
            .and_then(|t| t.meta.basis)
            .or(self.options.basis.filter(|_| self.state == RunState::Shown));
        if let Some(b) = basis {
            parts.push(t(match b {
                cl_core::report::Basis::Accrual => "reports.basis.accrual_basis",
                cl_core::report::Basis::Cash => "reports.basis.cash_basis",
            }));
        }
        if !self.options.header.hide_run_time
            && let Some(at) = &self.ran_at
        {
            parts.push(t_with("reports.header.run", &[("time", at)]));
        }
        parts.join(" \u{b7} ")
    }

    // ------------------------------------------------------------ drawing

    fn draw(&mut self, ui: &mut Ui) {
        let tokens = Tokens::of(ui.ctx());
        // At a large zoom the list would take a third of the window: it
        // folds away behind a button (QA-14).
        let narrow = ui.available_width() < 900.0;
        let show_list = self.catalog_open.unwrap_or(!narrow);
        ui.horizontal_top(|ui| {
            if show_list {
                ui.vertical(|ui| {
                    ui.set_width(if narrow { 180.0 } else { 230.0 });
                    super::catalog::show(self, ui, &tokens);
                });
                ui.separator();
            }
            ui.vertical(|ui| {
                let key = if show_list {
                    "reports.list.hide"
                } else {
                    "reports.list.show"
                };
                if (narrow || self.catalog_open.is_some()) && ui.small_button(t(key)).clicked() {
                    self.catalog_open = Some(!show_list);
                }
                super::filters::toolbar(self, ui, &tokens);
                super::ask::ui(self, ui, &tokens);
                super::filters::filter_bar(self, ui, &tokens);
                // The dialogs sit above the editor, so they open in view
                // (QA-12).
                super::memorized::ui(self, ui, &tokens);
                super::email::ui(self, ui, &tokens);
                if self.customize {
                    super::filters::options_panel(self, ui, &tokens);
                }
                super::budgets::ui(self, ui, &tokens);
                self.body(ui, &tokens);
            });
        });
    }

    /// The widths of the table's columns: amounts as wide as their widest
    /// figure, text up to a cap (then an ellipsis), the caption column as
    /// wide as its longest caption up to a cap.
    fn measure(&mut self, ui: &Ui, rows: &[usize]) -> Widths {
        let key = WidthKey {
            table: self.table_serial,
            columns: self.table.as_ref().map_or(0, |t| t.columns.len()),
            rows: rows.len(),
            collapsed: self.options.collapsed.len(),
            zoom_bits: ui.ctx().pixels_per_point().to_bits(),
        };
        if let Some(w) = &self.widths
            && w.key == key
        {
            return w.clone();
        }
        let Some(table) = self.table.as_ref() else {
            return Widths {
                key,
                caption: CAPTION_MIN,
                cells: Vec::new(),
                short: Vec::new(),
            };
        };
        let style = crate::grid_env::shell_frame(ui.ctx()).money_style;
        let body = egui::TextStyle::Body.resolve(ui.style());
        // The cells' own font: the body size in the monospace family.
        let mono = egui::FontId::new(body.size, egui::FontFamily::Monospace);
        let head = egui::FontId::proportional(12.0);
        let glyph = ui.fonts_mut(|f| f.glyph_width(&mono, '0'));
        let measure = |s: &str, font: &egui::FontId| {
            ui.fonts_mut(|f| {
                f.layout_no_wrap(s.to_string(), font.clone(), egui::Color32::WHITE)
                    .size()
                    .x
            })
        };
        // Cells are monospace: the most characters is the widest; that
        // text is laid out too, in case a glyph comes from a fallback font.
        let mut chars = vec![0usize; table.columns.len()];
        let mut widest = vec![String::new(); table.columns.len()];
        let mut longest: (usize, f32, String) = (0, 0.0, String::new());
        for &i in rows {
            let r = &table.rows[i];
            for (ci, c) in r.cells.iter().enumerate().take(chars.len()) {
                let s = cell_text(c, style);
                let n = s.chars().count();
                if n > chars[ci] {
                    chars[ci] = n;
                    widest[ci] = s;
                }
            }
            let caption = self.caption_of(table, i);
            let indent = f32::from(r.level) * 16.0 + if r.expandable { 22.0 } else { 0.0 };
            if caption.chars().count() + usize::from(r.level) * 2 > longest.0 {
                longest = (
                    caption.chars().count() + usize::from(r.level) * 2,
                    indent,
                    caption,
                );
            }
        }
        let caption =
            (measure(&longest.2, &body) * 1.08 + longest.1 + PAD).clamp(CAPTION_MIN, CAPTION_MAX);
        #[allow(clippy::cast_precision_loss, reason = "a character count")]
        let cells = table
            .columns
            .iter()
            .zip(chars.iter().zip(&widest))
            .map(|(c, (n, text))| {
                let heading = measure(&label(&c.caption), &head) + PAD;
                let cell = (*n as f32 * glyph).max(measure(text, &mono)) + PAD;
                if c.kind.is_numeric() {
                    cell.max(heading.min(140.0)).max(NUM_MIN)
                } else {
                    cell.max(heading.min(TEXT_MAX)).clamp(TEXT_MIN, TEXT_MAX)
                }
            })
            .collect();
        let short = chars.iter().map(|n| *n <= SHORT_TEXT).collect();
        let w = Widths {
            key,
            caption,
            cells,
            short,
        };
        self.widths = Some(w.clone());
        w
    }

    fn body(&mut self, ui: &mut Ui, tokens: &Tokens) {
        if let Some(n) = &self.notice {
            form::hint(ui, tokens, n);
        }
        if let Some(n) = self.export.message() {
            form::hint(ui, tokens, &n);
        }
        // A09: a builder export reads every row first; Cancel stops it.
        if self.export.filling() {
            ui.horizontal(|ui| {
                form::progress_line(ui, tokens, &t("reports.export.filling"));
                if ui.button(t("reports.export.cancel")).clicked() {
                    self.export.cancel();
                }
            });
        }
        if let Some(e) = &self.drill_error {
            form::inline_error_details(ui, tokens, e, "reports.drill");
        }
        match self.state {
            RunState::Unavailable => {
                form::inline_warning(ui, tokens, &t("reports.needs_update"));
                return;
            }
            RunState::Empty | RunState::Running if self.table.is_none() => {
                form::progress_line(ui, tokens, &t("reports.loading"));
                return;
            }
            _ => {}
        }
        if let Some(p) = self.problem {
            form::inline_warning(ui, tokens, &t(p));
        }
        super::builder_run::banner(self, ui, tokens);
        if let Some(e) = &self.error {
            form::inline_error_details(ui, tokens, e, "reports.run");
            if ui.button(t("reports.retry")).clicked() {
                self.run();
            }
        }
        if self.table.is_none() {
            return;
        }
        self.header(ui, tokens);
        // A table older than the options on screen is drawn dimmed, also
        // when the run for the new options failed (B2).
        let dim = matches!(
            self.state,
            RunState::Stale | RunState::Running | RunState::Failed
        );
        if dim {
            ui.multiply_opacity(0.55);
        }
        self.grid(ui, tokens);
    }

    fn header(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let mut follow = None;
        ui.add_space(8.0);
        ui.vertical_centered(|ui| {
            if !self.company.is_empty() {
                ui.label(RichText::new(&self.company).strong());
            }
            ui.label(RichText::new(self.title_text()).size(18.0).strong());
            ui.label(RichText::new(self.subtitle()).color(tokens.text_secondary));
            if let Some(meta) = self.table.as_ref().map(|t| &t.meta)
                && let Some(note) = &meta.note
            {
                let text = RichText::new(super::label(note))
                    .size(12.0)
                    .color(tokens.text_secondary);
                // S5: the note opens the entries it is about, through a
                // link that looks like one (PR6-QA-3).
                match &meta.note_drill {
                    Some(d) => {
                        ui.label(text);
                        let link = ui
                            .link(RichText::new(t("reports.note.see_entries")).size(12.0))
                            .on_hover_text(t("reports.drill.hint"));
                        if link.clicked() {
                            follow = Some(d.clone());
                        }
                    }
                    None => {
                        ui.label(text);
                    }
                }
            }
        });
        if let Some(d) = follow {
            super::drill::open(self, &d);
        }
        ui.add_space(10.0);
    }

    fn grid(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let rows = self.shown_rows();
        let Some(table) = self.table.as_ref() else {
            return;
        };
        if table.rows.is_empty() {
            form::hint(ui, tokens, &t("reports.empty"));
            return;
        }
        let mut widths = self.measure(ui, &rows);
        let Some(table) = self.table.as_ref() else {
            return;
        };
        // Each cell is followed by the layout's spacing.
        #[allow(clippy::cast_precision_loss, reason = "a column count")]
        let gaps = ui.spacing().item_spacing.x * (table.columns.len() + 1) as f32;
        fit(&mut widths, table, ui.available_width() - gaps - 16.0);
        let style = crate::grid_env::shell_frame(ui.ctx()).money_style;
        let mut action: Option<GridAction> = None;
        let width_of = |ci: usize| widths.cells.get(ci).copied().unwrap_or(NUM_MIN);
        // The heading row follows the rows' horizontal scroll.
        egui::ScrollArea::horizontal()
            .id_salt(("report_heading", self.view))
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
            .scroll_source(egui::scroll_area::ScrollSource::NONE)
            .horizontal_scroll_offset(self.h_offset)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.allocate_exact_size(Vec2::new(widths.caption, ROW_H), egui::Sense::hover());
                    for (ci, c) in table.columns.iter().enumerate() {
                        cell_box(ui, width_of(ci), c.kind.is_numeric(), |ui| {
                            let text = RichText::new(label(&c.caption)).strong().size(12.0);
                            let sortable = self.spec.sort == SortMode::WithinGroups;
                            if sortable {
                                if ui
                                    .add(egui::Button::new(text).frame(false))
                                    .on_hover_text(t("reports.sort.hint"))
                                    .clicked()
                                {
                                    action = Some(GridAction::Sort(ci));
                                }
                            } else {
                                ui.add(egui::Label::new(text).truncate());
                            }
                        });
                    }
                });
            });
        ui.separator();
        let collapsed = &self.options.collapsed;
        let out = egui::ScrollArea::both()
            .id_salt(("report_rows", self.view))
            .auto_shrink([false, true])
            .show_rows(ui, ROW_H, rows.len(), |ui, range| {
                for &i in &rows[range] {
                    let r = &table.rows[i];
                    ui.horizontal(|ui| {
                        ui.set_height(ROW_H);
                        let strong = matches!(
                            r.kind,
                            RowKind::Header
                                | RowKind::Total
                                | RowKind::Grand
                                | RowKind::Subtotal
                                | RowKind::Formula
                        );
                        fixed_box(
                            ui,
                            Vec2::new(widths.caption, ROW_H),
                            Layout::left_to_right(Align::Center),
                            |ui| {
                                ui.add_space(f32::from(r.level) * 16.0);
                                let caption = self.caption_of(table, i);
                                if r.expandable {
                                    let open = match &self.builder {
                                        Some(b) => b.state.is_open(&r.path),
                                        None => !collapsed.contains(&r.path),
                                    };
                                    let arrow = if open { "\u{2212}" } else { "+" };
                                    let key = if open {
                                        "reports.collapse_row"
                                    } else {
                                        "reports.expand_row"
                                    };
                                    let name = t_with(key, &[("row", &caption)]);
                                    let resp = ui.add(egui::Button::new(arrow).frame(false));
                                    resp.widget_info(|| {
                                        egui::WidgetInfo::labeled(
                                            egui::Role::Button,
                                            true,
                                            name.clone(),
                                        )
                                    });
                                    if resp.clicked() {
                                        action = Some(GridAction::Collapse(r.path.clone()));
                                    }
                                }
                                // A capped path's `more` row pages the rest.
                                if r.kind == RowKind::More
                                    && let Some(d) = &r.drill
                                {
                                    if ui.button(label(&r.caption)).clicked() {
                                        action = Some(GridAction::Drill(d.clone()));
                                    }
                                    return;
                                }
                                let text = RichText::new(caption);
                                ui.add(
                                    egui::Label::new(if strong { text.strong() } else { text })
                                        .truncate(),
                                );
                            },
                        );
                        for (ci, cell) in r.cells.iter().enumerate() {
                            let Some(col) = table.columns.get(ci) else {
                                continue;
                            };
                            // A ledger line's empty side is blank, not a
                            // 0.00 link (QA-2).
                            let blank_side = r.kind == RowKind::Detail
                                && matches!(col.key.as_str(), "debit" | "credit")
                                && matches!(cell, Cell::Amount(d) if d.is_zero());
                            let text = if blank_side {
                                String::new()
                            } else {
                                cell_text(cell, style)
                            };
                            cell_box(ui, width_of(ci), col.kind.is_numeric(), |ui| {
                                // Figures and dates in the figure font; names,
                                // types and memos in the text font (A07 QA-6).
                                let rich = RichText::new(&text);
                                let rich = if matches!(cell, Cell::Text(_)) {
                                    rich
                                } else {
                                    rich.family(egui::FontFamily::Monospace)
                                };
                                let rich = if strong { rich.strong() } else { rich };
                                let drill = r
                                    .drill
                                    .as_ref()
                                    .filter(|_| matches!(cell, Cell::Amount(_)) && !text.is_empty())
                                    .and_then(|d| d.for_cell(col))
                                    .filter(|d| {
                                        super::drill::live(&self.link, self.spec.source, d)
                                    });
                                match drill {
                                    Some(d) => {
                                        let resp =
                                            ui.link(rich).on_hover_text(t("reports.drill.hint"));
                                        if resp.clicked() {
                                            action = Some(GridAction::Drill(d));
                                        }
                                    }
                                    // Amounts fit their column; only text
                                    // ends in an ellipsis (QA-5).
                                    None if col.kind.is_numeric() => {
                                        ui.label(rich);
                                    }
                                    None => {
                                        ui.add(egui::Label::new(rich).truncate());
                                    }
                                }
                            });
                        }
                    });
                }
            });
        if (out.state.clamped_offset().x - self.h_offset).abs() > 0.5 {
            ui.ctx().request_repaint();
        }
        let new_offset = out.state.clamped_offset().x;
        if let Some(n) = table.meta.truncated {
            form::hint(
                ui,
                tokens,
                &t_with(
                    "reports.truncated",
                    &[
                        ("shown", &crate::i18n::count_text(n.shown)),
                        ("total", &crate::i18n::count_text(n.total)),
                    ],
                ),
            );
        }
        // What the Duplicate Finder compares (U-209 note, QA-9).
        if self.spec.source == cl_core::report::Source::Duplicates {
            ui.label(
                RichText::new(t("reports.duplicates.how"))
                    .size(12.0)
                    .color(tokens.text_secondary),
            );
        }
        if self.spec.drill != cl_model::reports::DrillRule::None {
            ui.label(
                RichText::new(t("reports.drill.footer"))
                    .size(12.0)
                    .color(tokens.text_secondary),
            );
        }
        self.h_offset = new_offset;
        match action {
            Some(GridAction::Collapse(p)) if self.builder.is_some() => {
                super::builder_run::toggle(self, &p, ui);
            }
            Some(GridAction::Collapse(p)) => self.toggle_collapse(&p),
            Some(GridAction::Drill(d)) => super::drill::open(self, &d),
            Some(GridAction::Sort(ci)) => self.sort_by(ci),
            None => {}
        }
    }

    /// Sorts by column `ci`, then flips the order on a second click.
    fn sort_by(&mut self, ci: usize) {
        let Some(key) = self
            .table
            .as_ref()
            .and_then(|t| t.columns.get(ci))
            .map(|c| c.key.clone())
        else {
            return;
        };
        let mut next = self.options.clone();
        next.sort = Some(match &next.sort {
            Some(s) if s.column == key => cl_model::reports::Sort {
                column: key,
                descending: !s.descending,
            },
            _ => cl_model::reports::Sort {
                column: key,
                descending: false,
            },
        });
        self.set_options(next);
    }

    /// Esc: closes the innermost form open in the window (the Memorize
    /// dialog, Email, a question of the budget editor, the editor when
    /// nothing in it is unsaved). False when nothing was open (QA-13).
    pub(crate) fn escape(&mut self) -> bool {
        if self.memo.dialog_open() {
            self.memo.dialog = None;
        } else if self.mailer.open {
            self.mailer.open = false;
            self.mailer.message = None;
        } else if self.budgets.confirm_discard.is_some() || self.budgets.confirm_copy.is_some() {
            self.budgets.confirm_discard = None;
            self.budgets.confirm_copy = None;
        } else if self.budgets.open {
            // Unsaved rows stay on screen; the owner leaves the row (it
            // saves) or picks Discard.
            if !self.budgets.has_unsaved() {
                self.budgets.open = false;
            }
        } else {
            return false;
        }
        true
    }

    #[cfg(test)]
    pub(crate) fn state_json_for_test(&self) -> Value {
        self.state_json()
    }

    fn state_json(&self) -> Value {
        let rows = self.shown_rows();
        json!({
            "screen": "reports",
            "report": self.spec.key,
            "state": self.state.name(),
            "available": available(&self.link, self.spec),
            "rows": rows.len(),
            "columns": self.table.as_ref().map_or(0, |t| t.columns.len()),
            "column_keys": self.table.as_ref().map(|t| t.columns.iter().map(|c| c.key.clone()).collect::<Vec<_>>()),
            "collapsed": self.options.collapsed.len(),
            // The Collapse button's label (A07 QA-9).
            "collapse_label": if self.collapse_expands() { "expand_all" } else { "collapse" },
            "message": self.export.message().is_some() || self.notice.is_some(),
            "basis": self.options.basis.map(cl_core::report::Basis::as_str),
            "columns_by": self.options.columns,
            "extras": self.options.extras.len(),
            "customize": self.customize,
            // A08: why "Edit in Report Builder" is off (`null`: it works).
            "builder_customize_off": super::builder_run::customize_off(self),
            "memorize_available": super::memorized::available(&self.link),
            "memorize_dialog": self.memo.dialog_open(),
            "memorized_changed": super::memorized::changed(self),
            "problem": self.problem.map(|p| p.rsplit('.').next().unwrap_or_default()),
            "error": self.error.is_some(),
            "export": self.export.state(),
            "search": !self.search.is_empty(),
            "budget": self.budgets.state_json(),
            "ask": self.ask.state_json(),
            "email": self.mailer.state_json(),
            "email_available": super::email::available(self),
            "builder": self.builder.as_deref().map(|b| super::builder_run::state_json(b, self.table.as_ref())),
        })
    }
}

/// What a grid click asks for.
enum GridAction {
    Collapse(RowPath),
    Drill(cl_core::report::Drill),
    Sort(usize),
}

/// A fixed-width cell, numbers right-aligned.
fn cell_box(ui: &mut Ui, width: f32, right: bool, add: impl FnOnce(&mut Ui)) {
    let layout = if right {
        Layout::right_to_left(Align::Center)
    } else {
        Layout::left_to_right(Align::Center)
    };
    fixed_box(ui, Vec2::new(width, ROW_H), layout, add);
}

/// Exactly `size`, whatever the content (columns line up row to row).
fn fixed_box(ui: &mut Ui, size: Vec2, layout: Layout, add: impl FnOnce(&mut Ui)) {
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(layout));
    // Long names and memos stop at the cell's edge instead of running into
    // the next column.
    child.set_clip_rect(rect.intersect(ui.clip_rect()));
    add(&mut child);
}

/// Narrows the caption and text columns (names, memos) toward their
/// minimum until the table fits `room`; amounts, dates and short codes or
/// statuses keep their width, so a figure is never cut (QA-2, PR6-QA-4).
/// What still doesn't fit scrolls.
fn fit(w: &mut Widths, table: &ReportTable, room: f32) {
    let total = w.caption + w.cells.iter().sum::<f32>();
    let excess = total - room;
    if excess <= 0.0 {
        return;
    }
    let text: Vec<usize> = table
        .columns
        .iter()
        .enumerate()
        .filter(|(_, c)| c.kind == cl_core::report::ColumnKind::Text)
        .map(|(i, _)| i)
        .filter(|i| *i < w.cells.len() && !w.short.get(*i).copied().unwrap_or(false))
        .collect();
    let slack: f32 = (w.caption - CAPTION_FIT_MIN).max(0.0)
        + text
            .iter()
            .map(|&i| (w.cells[i] - TEXT_MIN).max(0.0))
            .sum::<f32>();
    if slack <= 0.0 {
        return;
    }
    let share = (excess / slack).min(1.0);
    let cut = (w.caption - CAPTION_FIT_MIN).max(0.0) * share;
    w.caption -= cut;
    for &i in &text {
        let c = (w.cells[i] - TEXT_MIN).max(0.0) * share;
        w.cells[i] -= c;
    }
}

/// The caption column's smallest width when the table must fit.
const CAPTION_FIT_MIN: f32 = 120.0;

/// The open company's fiscal calendar (4-4-5 and the like), kept on this
/// computer (Q-P08c); the report window and the Report Builder editor read
/// it the same way, so their runs match (A08 AC8).
pub(super) fn company_calendar(
    link: &CompanyLink,
    open: Option<&(cl_core::ids::TenantId, String)>,
) -> cl_settings::FiscalCalendar {
    open.and_then(|(id, _)| {
        link.store()
            .load()
            .ok()
            .and_then(|list| list.get(*id).map(|r| r.flags.fiscal_calendar))
    })
    .unwrap_or_default()
}

/// The company's fiscal start month (the window and the A08 editor; the
/// answer becomes a spec through `home::charts::spec(start, calendar)`).
pub(super) fn fiscal_read(link: &CompanyLink, group: Option<&RequestGroup>) -> Job<Option<u8>> {
    engine::read(
        link,
        group,
        |_| Ok(Some(1)),
        |s| async move { Ok(s.company_settings().await?.fiscal_year_start_month) },
    )
}

/// A name row whose every amount is zero (pinned rows never are).
pub(crate) fn all_zero(r: &cl_core::report::ReportRow) -> bool {
    !r.kind.is_pinned()
        && r.cells.iter().any(|c| matches!(c, Cell::Amount(_)))
        && r.cells.iter().all(|c| match c {
            Cell::Amount(d) => d.is_zero(),
            _ => true,
        })
}

/// A cell as shown (display formatting only, plan §6).
pub(crate) fn cell_text(c: &Cell, style: MoneyStyle) -> String {
    match c {
        Cell::Empty => String::new(),
        Cell::Text(s) => label(s),
        Cell::Amount(d) => display_amount(*d, style),
        Cell::Percent(p) => display_percent(*p),
        Cell::Quantity(d) => cl_print::format_quantity(*d),
        Cell::Date(d) => crate::shell::us_date(*d),
        Cell::Count(n) => crate::i18n::count_text(*n),
    }
}

/// Sorts each run of unpinned rows by column `col`; headers and totals
/// stay where the engine put them (Q-P28k). Comparing is not arithmetic.
pub(crate) fn sort_within_groups(table: &ReportTable, rows: &mut [usize], col: usize, desc: bool) {
    let key = |i: usize| table.rows.get(i).and_then(|r| r.cells.get(col));
    let mut start = 0;
    while start < rows.len() {
        let pinned = |i: usize| table.rows.get(i).is_none_or(|r| r.kind.is_pinned());
        if pinned(rows[start]) {
            start += 1;
            continue;
        }
        let mut end = start;
        while end < rows.len() && !pinned(rows[end]) {
            end += 1;
        }
        rows[start..end].sort_by(|&a, &b| {
            let o = compare(key(a), key(b));
            if desc { o.reverse() } else { o }
        });
        start = end;
    }
}

fn compare(a: Option<&Cell>, b: Option<&Cell>) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (a, b) {
        (
            Some(Cell::Amount(x) | Cell::Quantity(x) | Cell::Percent(Some(x))),
            Some(Cell::Amount(y) | Cell::Quantity(y) | Cell::Percent(Some(y))),
        ) => x.cmp(y),
        (Some(Cell::Date(x)), Some(Cell::Date(y))) => x.cmp(y),
        (Some(Cell::Count(x)), Some(Cell::Count(y))) => x.cmp(y),
        (Some(Cell::Text(x)), Some(Cell::Text(y))) => x.to_lowercase().cmp(&y.to_lowercase()),
        (Some(Cell::Empty | Cell::Percent(None)) | None, Some(_)) => Ordering::Less,
        (Some(_), Some(Cell::Empty | Cell::Percent(None)) | None) => Ordering::Greater,
        _ => Ordering::Equal,
    }
}

fn problem_key(e: &cl_model::reports::ResolveError) -> &'static str {
    use cl_model::reports::ResolveError as R;
    match e {
        R::Fiscal(_) => "reports.problem.fiscal",
        R::Backwards => "reports.problem.backwards",
        R::TooManyColumns => "reports.problem.too_many_columns",
        R::NoTagGroup => "reports.problem.no_tag_group",
        R::ProjectsUnavailable => "reports.problem.projects",
        R::TagSplitWithCompare => "reports.problem.tag_split_compare",
        R::TagSplitWithTagFilter => "reports.problem.tag_split_tag_filter",
        R::BadAmount => "reports.problem.bad_amount",
    }
}

impl ReportWindow {
    /// The editor's Export or Print, once the full run is on screen; a run
    /// that failed drops it (the tab says why).
    pub(super) fn do_after_run(&mut self, ctx: &egui::Context) {
        use super::builder_ref::AfterRun;
        match self.state {
            RunState::Shown if self.table.is_some() => match self.after_run.take() {
                Some(AfterRun::Print) => super::export::print(self),
                Some(AfterRun::Export(kind)) => super::export::start(self, ctx, kind),
                None => {}
            },
            RunState::Failed | RunState::Unavailable => self.after_run = None,
            _ => {}
        }
    }
}

impl Screen for ReportWindow {
    fn title(&self) -> String {
        self.title_text()
    }

    fn ui(&mut self, ui: &mut Ui) {
        self.poll();
        super::builder_run::tick(self, ui);
        if self.job.is_none() {
            let mut changed = self.link.bus().take(&mut self.bus);
            // A report memorized elsewhere (an editor, another window): the
            // Memorized list reads again; the figures stand (QA-16).
            if std::mem::take(&mut changed.reports) {
                super::memorized::load(self);
            }
            if !changed.is_empty() {
                self.run();
            }
        }
        super::export::poll(self);
        self.do_after_run(ui.ctx());
        egui::ScrollArea::vertical()
            .id_salt(("report_window", self.view))
            .show(ui, |ui| self.draw(ui));
        if self.job.is_some()
            || self.fiscal_job.is_some()
            || self.export.busy()
            || self.memo.busy()
            || self.budgets.busy()
            || self.ask.busy()
            || self.mailer.busy()
            || self.filter_lists.busy()
            || self.drill_job.is_some()
            || self.builder.as_ref().is_some_and(|b| b.busy())
            || self.starters.busy()
            || self.edit.busy()
        {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    fn scope(&self) -> Scope {
        // With a form open in the window (Memorize, Email, the budget
        // editor) Esc reaches it (QA-13).
        if self.memo.dialog_open() || self.mailer.open || self.budgets.open {
            Scope::Form
        } else {
            Scope::Window
        }
    }

    /// Typed budget rows not saved yet, or a memorized report whose options
    /// changed (plan §4): the tab shows the dot and closing asks (B1).
    fn dirty(&self) -> bool {
        self.budgets.has_unsaved() || super::memorized::changed(self)
    }

    /// Budget rows save as the owner leaves them and a memorized report
    /// through its dialog, so the close prompt offers Close anyway or Keep
    /// editing only.
    fn can_save(&self) -> bool {
        false
    }

    fn command(&mut self, id: CommandId) -> bool {
        match id.as_str() {
            "reports.refresh" => {
                self.run();
                true
            }
            "reports.memorize" => {
                if super::memorized::available(&self.link) {
                    super::memorized::open_dialog(self);
                }
                true
            }
            "reports.collapse" => {
                self.toggle_collapse_all();
                true
            }
            "file.print" => {
                super::export::print(self);
                true
            }
            "form.revert" => self.escape(),
            _ => false,
        }
    }

    fn open_with(&mut self, payload: OpenPayload) -> bool {
        let open = match payload {
            OpenPayload::Report(open) => open,
            OpenPayload::BuilderStarter(key) => {
                super::builder_run::open_starter(self, &key);
                return true;
            }
            OpenPayload::BuilderDetail(open) => {
                super::builder_run::open_detail(self, *open);
                return true;
            }
            OpenPayload::BuilderDefinition(open) => {
                super::builder_run::open_definition(self, *open);
                return true;
            }
            OpenPayload::Memorized(open) => {
                super::memorized::open_row(self, &open.0);
                return true;
            }
            _ => return false,
        };
        let ReportOpen { key, options } = *open;
        let Some(spec) = cl_model::reports::find(&key) else {
            return false;
        };
        let options = options.unwrap_or_else(|| ReportOptions::for_spec(spec));
        self.show_report(spec, options);
        true
    }

    fn state(&self) -> Value {
        self.state_json()
    }

    /// An editor's next Run report refreshes its result tab (Q-A08x).
    fn takes_in_place(&self, payload: &OpenPayload) -> bool {
        super::builder_run::takes(self, payload)
    }
}

impl std::fmt::Debug for ReportWindow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReportWindow")
            .field("kind", &self.kind)
            .field("report", &self.spec.key)
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}
