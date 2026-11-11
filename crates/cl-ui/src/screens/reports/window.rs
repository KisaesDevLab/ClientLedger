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
const CAPTION_W: f32 = 230.0;
const CELL_W: f32 = 96.0;

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
    bus: crate::data_bus::Subscription,
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
        let calendar = open
            .as_ref()
            .and_then(|(id, _)| {
                link.store()
                    .load()
                    .ok()
                    .and_then(|list| list.get(*id).map(|r| r.flags.fiscal_calendar))
            })
            .unwrap_or_default();
        let fiscal_job = Some(engine::read(
            &link,
            group.as_ref(),
            |_| Ok(Some(1)),
            |s| async move { Ok(s.company_settings().await?.fiscal_year_start_month) },
        ));
        let tag_groups_job = Some(engine::read(
            &link,
            group.as_ref(),
            |_| Ok(Vec::new()),
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
            error: None,
            problem: None,
            ran_at: None,
            dates: super::filters::DateInputs::new(today),
            search: String::new(),
            customize: false,
            export: super::export::Exporter::default(),
            memo: super::memorized::Memo::default(),
            notice: None,
            bus: crate::data_bus::Subscription::detached(),
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
        self.spec = spec;
        self.options = options;
        self.table = None;
        self.error = None;
        self.notice = None;
        self.memo.current = None;
        self.run();
    }

    /// New options: run again when the engine's answer would change;
    /// collapse, sort and header changes are local.
    pub(crate) fn set_options(&mut self, next: ReportOptions) {
        let rerun = self.options.needs_run(&next);
        self.options = next;
        if rerun {
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
        if self.fiscal_job.is_some() {
            self.state = RunState::Empty;
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
            let start = r.ok().flatten();
            self.fiscal = crate::screens::home::charts::spec(start, self.calendar);
            self.run();
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
                    self.table = Some(table);
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
        super::memorized::poll(self);
        super::budgets::poll(self);
        super::ask::poll(self);
        super::email::poll(self);
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
        let mut rows = table.visible(&self.options.collapsed);
        // The tag split's account totals show only when asked for (Q-271).
        rows.retain(|&i| !cl_model::reports::hidden(&table.rows[i], &self.options));
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

    /// Collapse every expandable row, or expand all when any is collapsed.
    pub(crate) fn toggle_collapse_all(&mut self) {
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

    /// The report's title as shown (the owner's rename first).
    pub(crate) fn title_text(&self) -> String {
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
        if let Some(b) = self.options.basis {
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
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(230.0);
                super::catalog::show(self, ui, &tokens);
            });
            ui.separator();
            ui.vertical(|ui| {
                super::filters::toolbar(self, ui, &tokens);
                super::ask::ui(self, ui, &tokens);
                super::filters::filter_bar(self, ui, &tokens);
                super::budgets::ui(self, ui, &tokens);
                if self.customize {
                    super::filters::options_panel(self, ui, &tokens);
                }
                super::memorized::ui(self, ui, &tokens);
                super::email::ui(self, ui, &tokens);
                self.body(ui, &tokens);
            });
        });
    }

    fn body(&mut self, ui: &mut Ui, tokens: &Tokens) {
        if let Some(n) = &self.notice {
            form::hint(ui, tokens, n);
        }
        if let Some(n) = self.export.message() {
            form::hint(ui, tokens, &n);
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
        if let Some(e) = &self.error {
            form::inline_error(ui, tokens, e);
            if ui.button(t("reports.retry")).clicked() {
                self.run();
            }
        }
        if self.table.is_none() {
            return;
        }
        self.header(ui, tokens);
        let dim = matches!(self.state, RunState::Stale | RunState::Running);
        if dim {
            ui.multiply_opacity(0.55);
        }
        self.grid(ui, tokens);
    }

    fn header(&self, ui: &mut Ui, tokens: &Tokens) {
        ui.add_space(8.0);
        ui.vertical_centered(|ui| {
            if !self.company.is_empty() {
                ui.label(RichText::new(&self.company).strong());
            }
            ui.label(RichText::new(self.title_text()).size(18.0).strong());
            ui.label(RichText::new(self.subtitle()).color(tokens.text_secondary));
            if let Some(note) = self
                .table
                .as_ref()
                .and_then(|t| t.meta.cash_basis_rules.as_ref())
            {
                ui.label(RichText::new(note).size(12.0).color(tokens.text_secondary));
            }
        });
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
        let style = crate::grid_env::shell_frame(ui.ctx()).money_style;
        let mut action: Option<GridAction> = None;
        // The heading row.
        ui.horizontal(|ui| {
            ui.allocate_exact_size(Vec2::new(CAPTION_W, ROW_H), egui::Sense::hover());
            for (ci, c) in table.columns.iter().enumerate() {
                cell_box(ui, c.kind.is_numeric(), |ui| {
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
                        ui.label(text);
                    }
                });
            }
        });
        ui.separator();
        let collapsed = &self.options.collapsed;
        egui::ScrollArea::both()
            .id_salt("report_rows")
            .auto_shrink([false, true])
            .show_rows(ui, ROW_H, rows.len(), |ui, range| {
                for &i in &rows[range] {
                    let r = &table.rows[i];
                    ui.horizontal(|ui| {
                        ui.set_height(ROW_H);
                        let strong = matches!(
                            r.kind,
                            RowKind::Header | RowKind::Total | RowKind::Grand | RowKind::Subtotal
                        );
                        fixed_box(
                            ui,
                            Vec2::new(CAPTION_W, ROW_H),
                            Layout::left_to_right(Align::Center),
                            |ui| {
                                ui.add_space(f32::from(r.level) * 16.0);
                                let caption = self.caption_of(table, i);
                                if r.expandable {
                                    let open = !collapsed.contains(&r.path);
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
                                let text = RichText::new(caption);
                                ui.label(if strong { text.strong() } else { text });
                            },
                        );
                        for (ci, cell) in r.cells.iter().enumerate() {
                            let Some(col) = table.columns.get(ci) else {
                                continue;
                            };
                            let text = cell_text(cell, style);
                            cell_box(ui, col.kind.is_numeric(), |ui| {
                                let rich = RichText::new(&text).family(egui::FontFamily::Monospace);
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
                                    None => {
                                        ui.label(rich);
                                    }
                                }
                            });
                        }
                    });
                }
            });
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
        if self.spec.drill != cl_model::reports::DrillRule::None {
            ui.label(
                RichText::new(t("reports.drill.footer"))
                    .size(12.0)
                    .color(tokens.text_secondary),
            );
        }
        match action {
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
            "basis": self.options.basis.map(cl_core::report::Basis::as_str),
            "columns_by": self.options.columns,
            "extras": self.options.extras.len(),
            "customize": self.customize,
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
fn cell_box(ui: &mut Ui, right: bool, add: impl FnOnce(&mut Ui)) {
    let layout = if right {
        Layout::right_to_left(Align::Center)
    } else {
        Layout::left_to_right(Align::Center)
    };
    fixed_box(ui, Vec2::new(CELL_W, ROW_H), layout, add);
}

/// Exactly `size`, whatever the content (columns line up row to row).
fn fixed_box(ui: &mut Ui, size: Vec2, layout: Layout, add: impl FnOnce(&mut Ui)) {
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(layout));
    add(&mut child);
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

impl Screen for ReportWindow {
    fn title(&self) -> String {
        self.title_text()
    }

    fn ui(&mut self, ui: &mut Ui) {
        self.poll();
        if self.job.is_none() && !self.link.bus().take(&mut self.bus).is_empty() {
            self.run();
        }
        super::export::poll(self);
        egui::ScrollArea::vertical()
            .id_salt("report_window")
            .show(ui, |ui| self.draw(ui));
        if self.job.is_some()
            || self.fiscal_job.is_some()
            || self.export.busy()
            || self.memo.busy()
            || self.budgets.busy()
            || self.ask.busy()
            || self.mailer.busy()
        {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    fn scope(&self) -> Scope {
        Scope::Window
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
            "form.revert" if self.memo.dialog_open() => {
                self.memo.dialog = None;
                true
            }
            _ => false,
        }
    }

    fn open_with(&mut self, payload: OpenPayload) -> bool {
        let OpenPayload::Report(open) = payload else {
            return false;
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
