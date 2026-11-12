//! Report Builder runs in the report window (A07 WP2, plan §4). The window
//! hosts a builder run under a stand-in spec ([`cl_model::reports::
//! BUILDER_RANGE`] / [`cl_model::reports::BUILDER_AS_OF`]) so its toolbar
//! dates and basis, header, widths, export and print work unchanged; this
//! module owns the definition, the pure run state
//! ([`cl_model::report_def::run_state`]), the one `RunRef` every page of a
//! run reuses (its fingerprint), and the jobs.
//!
//! Two modes: a run (expand, collapse, Load more, drills) and a detail tab
//! (a subtotal's lines, pages of 500, the engine's subtotal as the footer).
//!
//! A run's answer carries its generation; a late answer to an older one is
//! dropped (the POST may still finish on the engine, which cancels its own
//! work when the client goes away).

use std::cell::RefCell;

use cl_api::ApiError;
use cl_api::resources::report_builder::{
    BuilderRefusal, BuilderRun, ColumnLayout, DetailPage, DetailRequest, RunDates, RunPurpose,
    RunRef, RunRequest, Starter, builder_refusal,
};
use cl_core::report::{
    Cell, ColumnHead, DateRange, Drill, ReportDates, ReportMeta, ReportRow, ReportTable, RowKind,
    RowPath,
};
use cl_model::CommandId;
use cl_model::report_def::run_state::{
    Banner, BuilderRunState, DetailPaging, Effect, Failure, Phase,
};
use cl_model::report_def::{
    BuilderDefinition, apply_units, apply_units_to_cells, apply_units_to_rows,
};
use cl_model::reports::{
    BUILDER_AS_OF, BUILDER_RANGE, ColumnBy, DateChoice, DatePreset, ReportOptions,
};
use eframe::egui::{RichText, Ui};
use serde_json::{Value, json};

use super::KIND;
use super::window::{ReportWindow, RunState};
use crate::company::CompanyEvent;
use crate::i18n::t;
use crate::screens::company::form;
use crate::screens::invoice::engine::{self, Job};
use crate::theme::Tokens;
use crate::windows::OpenPayload;

/// A subtotal's detail, opened in a new report tab.
#[derive(Clone, Debug, PartialEq)]
pub struct BuilderDetailOpen {
    title: String,
    definition: Value,
    reference: RunRef,
    columns: Vec<ColumnHead>,
    layout: ColumnLayout,
    node_id: String,
    path: RowPath,
    range: Option<DateRange>,
    fingerprint: String,
    stamp: u64,
}

// The definition JSON compares as JSON values do (exactly).
impl Eq for BuilderDetailOpen {}

/// The run behind the table on screen.
struct Shown {
    reference: RunRef,
    layout: ColumnLayout,
    fingerprint: String,
    stamp: u64,
}

struct DetailTab {
    open: BuilderDetailOpen,
    paging: DetailPaging,
    rows: Vec<ReportRow>,
    /// Bumped by every Refresh: a page or rerun of an older one is dropped
    /// (#15).
    generation: u64,
    job: Option<(u64, Job<DetailPage>)>,
    /// Refresh runs the parent report again first, for a fresh fingerprint
    /// and stamp (#4): resending the old ones would be refused forever.
    rerun: Option<(u64, Job<BuilderRun>)>,
    banner: Option<Banner>,
}

impl DetailTab {
    fn busy(&self) -> bool {
        self.job.is_some() || self.rerun.is_some()
    }
}

/// The starters list beside the catalog.
#[derive(Default)]
pub(crate) struct Starters {
    pub(crate) list: Option<Vec<Starter>>,
    job: Option<Job<Vec<Starter>>>,
}

impl Starters {
    /// The list is being read.
    pub(crate) fn busy(&self) -> bool {
        self.job.is_some()
    }
}

/// A builder report in the window.
pub(crate) struct Builder {
    key: String,
    title: String,
    def: Option<BuilderDefinition>,
    json: Value,
    pub(crate) state: BuilderRunState,
    reference: Option<RunRef>,
    shown: Option<Shown>,
    job: Option<(u64, Job<BuilderRun>)>,
    page: Option<(u64, RowPath, Job<DetailPage>)>,
    detail: Option<DetailTab>,
    /// A starter asked for by key, waiting for the list.
    wanted: Option<String>,
    problem: Option<&'static str>,
    /// Pages put in place by Load more (the control channel's proof that a
    /// click did something, #17).
    loaded: usize,
    /// The rows shown, for (table serial, run-state revision): rebuilt only
    /// when either moves, not every frame (S-L6).
    visible: RefCell<Option<(u64, u64, Vec<usize>)>>,
}

impl Builder {
    fn new(key: &str) -> Builder {
        Builder {
            key: key.to_string(),
            title: String::new(),
            def: None,
            json: Value::Null,
            state: BuilderRunState::new(),
            reference: None,
            shown: None,
            job: None,
            page: None,
            detail: None,
            wanted: None,
            problem: None,
            loaded: 0,
            visible: RefCell::new(None),
        }
    }

    /// A subtotal's detail tab (its dates and basis are its run's).
    pub(crate) fn is_detail(&self) -> bool {
        self.detail.is_some()
    }

    /// The window's title.
    pub(crate) fn title(&self) -> String {
        if self.title.is_empty() {
            t("reports.builder.title")
        } else {
            self.title.clone()
        }
    }

    /// True while an answer is awaited or a debounced run is due: the
    /// window keeps asking for frames, so the run goes without input (#7).
    pub(crate) fn busy(&self) -> bool {
        self.job.is_some()
            || self.page.is_some()
            || self.detail.as_ref().is_some_and(DetailTab::busy)
            || self.state.phase() == Phase::Running
            || self.state.due_at().is_some()
    }
}

/// True when the open company runs builder reports.
pub(crate) fn available(w: &ReportWindow) -> bool {
    w.link.is_demo()
        || (w
            .link
            .capabilities()
            .has(cl_api::Capabilities::REPORT_BUILDER)
            && w.link
                .capabilities()
                .has(cl_api::Capabilities::REPORT_BUILDER_STARTERS))
}

/// Starts reading the starters (once).
pub(crate) fn load_starters(w: &mut ReportWindow) {
    if w.starters.list.is_some() || w.starters.job.is_some() || !available(w) {
        return;
    }
    w.starters.job = Some(engine::read(
        &w.link,
        w.group.as_ref(),
        |_| crate::company::demo::report_builder::starters(),
        |s| async move { s.report_builder().starters().await },
    ));
}

/// Opens the starter `key` (`OpenPayload::BuilderStarter`).
pub(crate) fn open_starter(w: &mut ReportWindow, key: &str) {
    let mut b = Builder::new(key);
    b.wanted = Some(key.to_string());
    w.builder = Some(Box::new(b));
    w.table = None;
    w.error = None;
    w.notice = None;
    load_starters(w);
    poll_starters(w);
    if !available(w)
        && let Some(b) = w.builder.as_mut()
    {
        b.state
            .run_failed(b.state.generation(), Failure::NeedsEngineUpdate);
        w.state = RunState::Unavailable;
    }
}

/// Shows `starter` (a catalog click or a loaded payload).
pub(crate) fn show_starter(w: &mut ReportWindow, starter: &Starter) {
    let text = serde_json::to_string(&starter.definition).unwrap_or_default();
    let mut b = Builder::new(&starter.key);
    match cl_model::report_def::parse(&text) {
        Ok(def) => {
            b.title = def.title.clone().unwrap_or_else(|| starter.title.clone());
            let as_of = starter.definition["dates"]["mode"] == "asOf";
            let spec = if as_of {
                &BUILDER_AS_OF
            } else {
                &BUILDER_RANGE
            };
            let mut o = ReportOptions::for_spec(spec);
            if let Some(p) = preset(starter.definition["dates"]["preset"].as_str()) {
                o.dates = DateChoice::Preset(p);
            }
            o.columns = match starter.definition["columns"]["periods"].as_str() {
                Some("month") => ColumnBy::Month,
                Some("quarter") => ColumnBy::Quarter,
                Some("year") => ColumnBy::Year,
                _ => ColumnBy::Total,
            };
            o.basis = match starter.definition["basis"].as_str() {
                Some("cash") => Some(cl_core::report::Basis::Cash),
                Some("accrual") => Some(cl_core::report::Basis::Accrual),
                _ => o.basis,
            };
            b.def = Some(def);
            b.json = starter.definition.clone();
            w.spec = spec;
            w.options = o;
        }
        // Saved by a newer build (a LAN host, a restore): say to update,
        // never a blank window (#8).
        Err(cl_model::report_def::DefinitionError::Newer) => {
            b.problem = Some("reports.builder.newer");
        }
        Err(_) => b.problem = Some("reports.builder.unreadable"),
    }
    w.builder = Some(Box::new(b));
    w.table = None;
    w.error = None;
    w.notice = None;
    w.memo.current = None;
    w.run();
}

/// The engine's date preset names, as the window's presets.
fn preset(name: Option<&str>) -> Option<DatePreset> {
    Some(match name? {
        "today" => DatePreset::Today,
        "this_month" => DatePreset::ThisMonth,
        "last_month" => DatePreset::LastMonth,
        "this_quarter" => DatePreset::ThisQuarter,
        "last_quarter" => DatePreset::LastQuarter,
        "this_year" => DatePreset::ThisYear,
        "last_year" => DatePreset::LastYear,
        "this_year_to_date" | "year_to_date" => DatePreset::YearToDate,
        "end_of_last_month" => DatePreset::EndOfLastMonth,
        _ => return None,
    })
}

/// Opens a subtotal's detail tab (`OpenPayload::BuilderDetail`).
pub(crate) fn open_detail(w: &mut ReportWindow, open: BuilderDetailOpen) {
    let mut b = Builder::new("detail");
    b.title.clone_from(&open.title);
    b.json = open.definition.clone();
    b.def = serde_json::to_string(&open.definition)
        .ok()
        .and_then(|s| cl_model::report_def::parse(&s).ok());
    // The window's options follow the run's basis; the filter bar hides
    // the dates and basis of a detail tab (#14).
    let mut o = ReportOptions::for_spec(&BUILDER_RANGE);
    o.basis = open.reference.basis.or(o.basis);
    b.detail = Some(DetailTab {
        open,
        paging: DetailPaging::default(),
        rows: Vec::new(),
        generation: 0,
        job: None,
        rerun: None,
        banner: None,
    });
    w.spec = &BUILDER_RANGE;
    w.options = o;
    w.builder = Some(Box::new(b));
    w.table = None;
    next_detail_page(w);
}

/// The run's reference from the window's dates and basis, and the
/// definition's adjusting lens (sent, so an earnings drill keeps it, #13).
/// Compare ranges are left to the engine's calendar shift until A08 offers
/// compare columns (Q-A07u).
fn reference(
    w: &ReportWindow,
    json: &Value,
    adjusting: Option<cl_core::report::Adjusting>,
) -> Result<RunRef, &'static str> {
    let req = cl_model::reports::resolve(w.spec, &w.options, &w.fiscal, w.today)
        .map_err(|_| "reports.problem.fiscal")?;
    let as_of = w.spec.dates == cl_model::reports::DateMode::AsOf;
    let (dates, period_ranges) = match req.dates {
        ReportDates::Range(r) => (RunDates::range(r), Vec::new()),
        ReportDates::AsOf(d) => (RunDates::as_of(d), Vec::new()),
        ReportDates::Ranges(v) => {
            let (Some(first), Some(last)) = (v.first(), v.last()) else {
                return Err("reports.problem.fiscal");
            };
            let dates = if as_of {
                RunDates::as_of(last.to)
            } else {
                RunDates::range(DateRange {
                    from: first.from,
                    to: last.to,
                })
            };
            (dates, if v.len() > 1 { v } else { Vec::new() })
        }
        ReportDates::None => return Err("reports.problem.fiscal"),
    };
    let to = dates.to.or(dates.as_of).ok_or("reports.problem.fiscal")?;
    let from = dates.from.unwrap_or(to);
    // Every fiscal year the run touches, from the year before (compare and
    // prior-years' earnings) through the last.
    let mut starts = Vec::new();
    let first = w
        .fiscal
        .year_containing(from)
        .map_err(|_| "reports.problem.fiscal")?;
    let mut year = first
        .start
        .add_days(-1)
        .ok()
        .and_then(|d| w.fiscal.year_containing(d).ok())
        .unwrap_or(first);
    loop {
        starts.push(year.start);
        if year.end >= to
            || starts.len() >= cl_api::resources::report_builder::MAX_FISCAL_YEAR_STARTS
        {
            break;
        }
        match year
            .end
            .add_days(1)
            .ok()
            .and_then(|d| w.fiscal.year_containing(d).ok())
        {
            Some(next) => year = next,
            None => break,
        }
    }
    Ok(RunRef {
        definition: Some(json.clone()),
        definition_id: None,
        version: None,
        dates,
        period_ranges,
        compare_ranges: Vec::new(),
        fiscal_year_starts: starts,
        basis: w.options.basis,
        adjusting,
    })
}

/// Runs (Refresh, a date or basis change).
pub(crate) fn run(w: &mut ReportWindow) {
    let Some(b) = w.builder.as_mut() else {
        return;
    };
    if b.detail.is_some() {
        refresh_detail(w);
        return;
    }
    if b.def.is_none() {
        // The window cleared its problem before this run: say it again
        // (#8), with no table and no Retry.
        w.problem = b.problem;
        w.state = if b.problem.is_some() {
            RunState::Failed
        } else {
            RunState::Empty
        };
        return;
    }
    let json = b.json.clone();
    let adjusting = b.def.as_ref().and_then(|d| d.adjusting);
    match reference(w, &json, adjusting) {
        Ok(r) => {
            if let Some(b) = w.builder.as_mut() {
                b.reference = Some(r);
                b.problem = None;
                let fx = b.state.refresh();
                w.state = RunState::Running;
                apply(w, fx);
            }
        }
        Err(key) => {
            w.problem = Some(key);
            w.state = RunState::Failed;
        }
    }
}

/// Performs the run state's effects.
fn apply(w: &mut ReportWindow, fx: Vec<Effect>) {
    for e in fx {
        match e {
            Effect::Run {
                generation,
                open,
                collapsed,
                expand_all,
            } => start_run(w, generation, open, collapsed, expand_all),
            Effect::Cancel { generation } => {
                if let Some(b) = w.builder.as_mut()
                    && b.job.as_ref().is_some_and(|(g, _)| *g == generation)
                {
                    // Dropped: its answer, if it still comes, is ignored.
                    b.job = None;
                }
            }
            Effect::Page {
                generation,
                node_id,
                path,
                cursor,
            } => start_page(w, generation, node_id, path, cursor),
        }
    }
}

fn start_run(
    w: &mut ReportWindow,
    generation: u64,
    open: Vec<RowPath>,
    collapsed: Vec<RowPath>,
    expand_all: bool,
) {
    let Some(b) = w.builder.as_mut() else {
        return;
    };
    let Some(reference) = b.reference.clone() else {
        return;
    };
    let req = RunRequest {
        reference,
        open,
        collapsed,
        expand_all,
        purpose: RunPurpose::Screen,
    };
    let slow = b.key == crate::company::demo::report_builder::SLOW;
    let live = req.clone();
    let job = engine::read(
        &w.link,
        w.group.as_ref(),
        move |d| crate::company::demo::report_builder::run(d, &req, slow),
        move |s| async move { s.report_builder().run(&live).await },
    );
    b.job = Some((generation, job));
}

fn start_page(
    w: &mut ReportWindow,
    generation: u64,
    node_id: String,
    path: RowPath,
    cursor: Option<String>,
) {
    let Some(b) = w.builder.as_ref() else {
        return;
    };
    let (Some(shown), Some(table)) = (b.shown.as_ref(), w.table.as_ref()) else {
        return;
    };
    let req = DetailRequest {
        reference: shown.reference.clone(),
        node_id,
        path: path.clone(),
        range: None,
        cursor,
        fingerprint: shown.fingerprint.clone(),
        stamp: shown.stamp,
    };
    let job = detail_job(w, req, table.columns.clone(), shown.layout.clone());
    if let Some(b) = w.builder.as_mut() {
        b.page = Some((generation, path, job));
    }
}

fn detail_job(
    w: &ReportWindow,
    req: DetailRequest,
    columns: Vec<ColumnHead>,
    layout: ColumnLayout,
) -> Job<DetailPage> {
    let live = req.clone();
    let (c2, l2) = (columns.clone(), layout.clone());
    engine::read(
        &w.link,
        w.group.as_ref(),
        move |d| crate::company::demo::report_builder::detail(d, &req, &columns, &layout),
        move |s| async move { s.report_builder().detail(&live, &c2, &l2).await },
    )
}

/// What the window says for an engine refusal.
fn failure(e: &ApiError) -> Failure {
    match builder_refusal(e) {
        Some(BuilderRefusal::BooksChanged) => Failure::BooksChanged,
        Some(BuilderRefusal::ParamsChanged) => Failure::ParamsChanged,
        Some(BuilderRefusal::Timeout) => Failure::Timeout,
        Some(BuilderRefusal::TooLarge) => Failure::TooLarge,
        Some(BuilderRefusal::StaleDefinition) => Failure::StaleDefinition,
        Some(BuilderRefusal::Deleted) => Failure::Deleted,
        Some(BuilderRefusal::Denied) => Failure::Denied,
        _ => match e {
            ApiError::Timeout => Failure::Timeout,
            ApiError::Local(cl_api::LocalValidation::Unsupported) => Failure::NeedsEngineUpdate,
            _ => Failure::Other,
        },
    }
}

/// Engine captions that are fixed English, as i18n keys.
fn caption_key(s: &str) -> Option<&'static str> {
    Some(match s {
        "Total" => "@reports.row.total",
        "No name" => "@reports.builder.none.contact",
        "No item" => "@reports.builder.none.item",
        "No tag" => "@reports.builder.none.tag",
        "No account" => "@reports.builder.none.account",
        "(deleted)" => "@reports.builder.deleted_name",
        "Prior years' net income" => "@reports.builder.earnings.prior",
        "Net income this year" => "@reports.builder.earnings.current",
        "Retained Earnings (Prior Years)" => "@reports.builder.earnings.retained",
        _ => return None,
    })
}

/// A column caption the definition didn't set, as an i18n key.
fn column_key(c: &ColumnHead) -> Option<String> {
    if let Some(f) = c.key.strip_prefix("f:") {
        return Some(format!("@reports.builder.field.{f}"));
    }
    if let Some(b) = c.key.strip_prefix("bucket:") {
        return Some(
            match b {
                "current" => "@reports.col.current",
                "d1_30" => "@reports.col.days_1_30",
                "d31_60" => "@reports.col.days_31_60",
                "d61_90" => "@reports.col.days_61_90",
                _ => "@reports.col.over_90",
            }
            .to_string(),
        );
    }
    Some(
        match c.caption.as_str() {
            "Total" => "@reports.col.total",
            "Prior Year" => "@reports.builder.col.prior_year",
            "Prior Period" => "@reports.builder.col.prior_period",
            "$ Change" => "@reports.builder.col.change",
            "% Change" => "@reports.builder.col.pct_change",
            "%" => "@reports.builder.col.percent",
            _ => return None,
        }
        .to_string(),
    )
}

/// The table as the window shows it: units, i18n captions, the dash.
fn dress(b: &Builder, mut table: ReportTable, denied: bool) -> ReportTable {
    if let Some(def) = &b.def {
        apply_units(def, &mut table);
        for c in &mut table.columns {
            if !def.columns.headers.contains_key(&c.key)
                && let Some(k) = column_key(c)
            {
                c.caption = k;
            }
        }
    }
    for r in &mut table.rows {
        if let Some(k) = caption_key(&r.caption) {
            r.caption = k.to_string();
        }
        // A figure that needs a section the role can't see (Q-A07i).
        if denied
            && matches!(r.kind, RowKind::Formula | RowKind::Subtotal)
            && r.cells
                .iter()
                .all(|c| matches!(c, Cell::Empty | Cell::Percent(None)))
        {
            for (cell, col) in r.cells.iter_mut().zip(&table.columns) {
                if !col.key.starts_with("f:") {
                    *cell = Cell::Text("\u{2014}".into());
                }
            }
        }
    }
    table
}

/// Polls the jobs (each frame).
pub(crate) fn poll(w: &mut ReportWindow) {
    poll_starters(w);
    poll_run(w);
    poll_page(w);
    poll_detail(w);
}

fn poll_starters(w: &mut ReportWindow) {
    if let Some(r) = engine::poll(&mut w.starters.job) {
        match r {
            Ok(list) => w.starters.list = Some(list),
            Err(e) => {
                w.starters.list = Some(Vec::new());
                if w.builder.as_ref().is_some_and(|b| b.wanted.is_some()) {
                    w.error = Some(e);
                    w.state = RunState::Failed;
                }
            }
        }
    }
    let wanted = w.builder.as_ref().and_then(|b| b.wanted.clone());
    if let (Some(key), Some(list)) = (wanted, w.starters.list.as_ref()) {
        if let Some(s) = list.iter().find(|s| s.key == key).cloned() {
            show_starter(w, &s);
        } else {
            {
                if let Some(b) = w.builder.as_mut() {
                    b.wanted = None;
                    b.problem = Some("reports.builder.no_starter");
                }
                w.problem = Some("reports.builder.no_starter");
                w.state = RunState::Failed;
            }
        }
    }
}

fn poll_run(w: &mut ReportWindow) {
    let Some(b) = w.builder.as_mut() else {
        return;
    };
    let Some((generation, job)) = b.job.as_mut() else {
        return;
    };
    let generation = *generation;
    let Some(r) = job.take() else {
        return;
    };
    b.job = None;
    match r {
        Ok(run) => {
            if !b.state.run_ok(generation, run.stamp, &run.expanded) {
                return;
            }
            let denied = !run.denied.is_empty();
            let table = dress(b, run.table, denied);
            b.shown = b.reference.clone().map(|reference| Shown {
                reference,
                layout: run.layout,
                fingerprint: run.fingerprint,
                stamp: run.stamp,
            });
            w.set_table(table);
            w.state = RunState::Shown;
            w.error = None;
            w.ran_at = Some(
                chrono::Local::now()
                    .format("%m/%d/%Y %-I:%M %p")
                    .to_string(),
            );
        }
        Err(e) => {
            let why = failure(&e);
            if b.state.generation() != generation {
                return;
            }
            b.state.run_failed(generation, why);
            match b.state.phase() {
                Phase::Shown => w.state = RunState::Shown,
                Phase::Unavailable(cl_model::report_def::run_state::Cap::NeedsEngineUpdate) => {
                    w.state = RunState::Unavailable;
                }
                _ => {
                    w.state = RunState::Failed;
                    w.problem = match (b.state.phase(), why) {
                        (Phase::TimedOut, _) => Some("reports.builder.took_too_long"),
                        (Phase::Unavailable(_), _) => Some("reports.builder.too_large"),
                        (Phase::Deleted, _) => Some("reports.builder.deleted"),
                        // A message, not "Try again": it would be refused
                        // again (S-L7).
                        (_, Failure::Denied) => Some("reports.builder.denied"),
                        _ => None,
                    };
                    if w.problem.is_none() {
                        w.error = Some(e);
                    }
                }
            }
        }
    }
}

fn poll_page(w: &mut ReportWindow) {
    let Some(b) = w.builder.as_mut() else {
        return;
    };
    let Some((generation, path, job)) = b.page.as_mut() else {
        return;
    };
    let (generation, path) = (*generation, path.clone());
    let Some(r) = job.take() else {
        return;
    };
    b.page = None;
    match r {
        Ok(page) => {
            if !b.state.page_ok(generation, &path) {
                return;
            }
            let Some(mut table) = w.table.take() else {
                return;
            };
            let mut rows = page.rows;
            if let Some(def) = &b.def {
                apply_units_to_rows(def, &table.columns, &mut rows);
            }
            splice(&mut table, &path, rows, page.cursor);
            b.loaded += 1;
            w.set_table(table);
        }
        Err(e) => {
            let fx = b.state.page_failed(generation, &path, failure(&e));
            apply(w, fx);
        }
    }
}

/// Puts a page's rows where the `more` row at `path` was, one level under
/// its line, and a new `more` row after them while pages remain.
fn splice(table: &mut ReportTable, path: &RowPath, rows: Vec<ReportRow>, cursor: Option<String>) {
    let Some(at) = table
        .rows
        .iter()
        .position(|r| r.kind == RowKind::More && &r.path == path)
    else {
        return;
    };
    let more = table.rows.remove(at);
    let level = more.level;
    let mut new: Vec<ReportRow> = rows
        .into_iter()
        .map(|mut r| {
            r.level = level;
            r
        })
        .collect();
    if let Some(c) = cursor
        && let Some(Drill::BuilderPage { node_id, path, .. }) = more.drill.clone()
    {
        let mut m = more;
        m.drill = Some(Drill::BuilderPage {
            node_id,
            path,
            cursor: Some(c),
        });
        new.push(m);
    }
    table.rows.splice(at..at, new);
}

/// The rows shown: the engine's, less those under a closed line. Worked
/// out once per table (`serial`) and run-state revision, not every frame
/// (S-L6: up to 100,000 rows).
pub(crate) fn shown_rows(b: &Builder, table: &ReportTable, serial: u64) -> Vec<usize> {
    let revision = b.state.revision();
    if let Some((s, r, rows)) = b.visible.borrow().as_ref()
        && *s == serial
        && *r == revision
    {
        return rows.clone();
    }
    let rows: Vec<(RowPath, bool, bool)> = table
        .rows
        .iter()
        .map(|r| {
            let leaf = matches!(r.kind, RowKind::Detail | RowKind::Earnings | RowKind::More);
            (r.path.clone(), r.expandable, leaf)
        })
        .collect();
    let shown: Vec<usize> = b
        .state
        .hidden(&rows)
        .into_iter()
        .enumerate()
        .filter(|(_, h)| !h)
        .map(|(i, _)| i)
        .collect();
    *b.visible.borrow_mut() = Some((serial, revision, shown.clone()));
    shown
}

fn now_ms(ui: &Ui) -> u64 {
    let s = ui.input(|i| i.time);
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a frame clock in milliseconds, never money"
    )]
    let ms = (s * 1000.0).max(0.0) as u64;
    ms
}

/// Opens or closes `path`.
pub(crate) fn toggle(w: &mut ReportWindow, path: &RowPath, ui: &Ui) {
    let now = now_ms(ui);
    if let Some(b) = w.builder.as_mut() {
        let fx = if b.state.is_open(path) {
            b.state.collapse(path, now)
        } else {
            b.state.expand(path, now)
        };
        apply(w, fx);
        // No input may follow (a keyboard Enter): a frame must come when
        // the debounce ends, or the run is never sent (#7).
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(
                cl_model::report_def::run_state::DEBOUNCE_MS,
            ));
    }
}

/// Expand all, or Collapse all when anything is open.
pub(crate) fn toggle_all(w: &mut ReportWindow) {
    if let Some(b) = w.builder.as_mut() {
        let fx = if b.state.is_open_all() || !b.state.open_paths().is_empty() {
            b.state.collapse_all()
        } else {
            b.state.expand_all()
        };
        w.state = RunState::Running;
        apply(w, fx);
    }
}

/// The debounce clock (each frame).
pub(crate) fn tick(w: &mut ReportWindow, ui: &Ui) {
    let now = now_ms(ui);
    if let Some(b) = w.builder.as_mut() {
        let fx = b.state.tick(now);
        if !fx.is_empty() {
            w.state = RunState::Running;
        }
        apply(w, fx);
    }
}

/// A builder drill: a subtotal's detail in a new tab, or the next page.
pub(crate) fn drill(w: &mut ReportWindow, d: &Drill) {
    let Some(b) = w.builder.as_mut() else {
        return;
    };
    match d {
        Drill::BuilderPage {
            node_id,
            path,
            cursor,
        } => {
            if b.detail.is_some() {
                next_detail_page(w);
                return;
            }
            let fx = b.state.more(node_id, path, cursor.as_deref());
            apply(w, fx);
        }
        Drill::BuilderDetail {
            node_id,
            path,
            range,
        } => {
            let Some(open) = detail_open(w, node_id, path, *range) else {
                return;
            };
            w.link.push(CompanyEvent::OpenWith(
                CommandId::new(KIND),
                Box::new(OpenPayload::BuilderDetail(Box::new(open))),
            ));
        }
        _ => {}
    }
}

/// The detail tab of `path` under the run on screen.
fn detail_open(
    w: &ReportWindow,
    node_id: &str,
    path: &RowPath,
    range: Option<DateRange>,
) -> Option<BuilderDetailOpen> {
    let b = w.builder.as_ref()?;
    let (shown, table) = (b.shown.as_ref()?, w.table.as_ref()?);
    Some(BuilderDetailOpen {
        title: format!("{} \u{2014} {}", b.title(), t("reports.builder.detail")),
        definition: b.json.clone(),
        reference: shown.reference.clone(),
        columns: table.columns.clone(),
        layout: shown.layout.clone(),
        node_id: node_id.to_string(),
        path: path.clone(),
        range,
        fingerprint: shown.fingerprint.clone(),
        stamp: shown.stamp,
    })
}

/// [`detail_open`] for tests.
#[cfg(test)]
pub(crate) fn detail_open_for_test(
    w: &ReportWindow,
    node_id: &str,
    path: &RowPath,
) -> BuilderDetailOpen {
    detail_open(w, node_id, path, None).expect("a run on screen")
}

/// Refresh in a detail tab (the button, F5, any posting): drops whatever
/// is in flight, then runs the parent report again with the same reference
/// for a fresh fingerprint and stamp, and pages from the start (#4, #15).
fn refresh_detail(w: &mut ReportWindow) {
    let Some(d) = w.builder.as_mut().and_then(|b| b.detail.as_mut()) else {
        return;
    };
    d.generation += 1;
    d.job = None;
    d.rows.clear();
    d.paging = DetailPaging::default();
    d.banner = None;
    let generation = d.generation;
    let req = RunRequest {
        reference: d.open.reference.clone(),
        open: Vec::new(),
        collapsed: Vec::new(),
        expand_all: false,
        purpose: RunPurpose::Screen,
    };
    let live = req.clone();
    let job = engine::read(
        &w.link,
        w.group.as_ref(),
        move |demo| crate::company::demo::report_builder::run(demo, &req, false),
        move |s| async move { s.report_builder().run(&live).await },
    );
    if let Some(d) = w.builder.as_mut().and_then(|b| b.detail.as_mut()) {
        d.rerun = Some((generation, job));
    }
    w.state = RunState::Running;
}

fn next_detail_page(w: &mut ReportWindow) {
    let Some(d) = w.builder.as_ref().and_then(|b| b.detail.as_ref()) else {
        return;
    };
    if d.busy() {
        return;
    }
    let generation = d.generation;
    let o = &d.open;
    let req = DetailRequest {
        reference: o.reference.clone(),
        node_id: o.node_id.clone(),
        path: o.path.clone(),
        range: o.range,
        cursor: d.paging.cursor.clone(),
        fingerprint: o.fingerprint.clone(),
        stamp: o.stamp,
    };
    let job = detail_job(w, req, o.columns.clone(), o.layout.clone());
    if let Some(d) = w.builder.as_mut().and_then(|b| b.detail.as_mut()) {
        d.job = Some((generation, job));
    }
    if w.table.is_none() {
        w.state = RunState::Running;
    }
}

/// The parent run for a detail tab's Refresh answered: page with its
/// fingerprint and stamp. A refusal shows a banner only; nothing retries
/// by itself.
fn poll_detail_rerun(w: &mut ReportWindow) {
    let Some(d) = w.builder.as_mut().and_then(|b| b.detail.as_mut()) else {
        return;
    };
    let Some((generation, job)) = d.rerun.as_mut() else {
        return;
    };
    let generation = *generation;
    let Some(r) = job.take() else {
        return;
    };
    d.rerun = None;
    if generation != d.generation {
        return;
    }
    match r {
        Ok(run) => {
            d.open.fingerprint = run.fingerprint;
            d.open.stamp = run.stamp;
            d.open.layout = run.layout;
            next_detail_page(w);
        }
        Err(e) => detail_failed(w, e),
    }
}

/// A detail tab's page or rerun failed.
fn detail_failed(w: &mut ReportWindow, e: ApiError) {
    let Some(d) = w.builder.as_mut().and_then(|b| b.detail.as_mut()) else {
        return;
    };
    match failure(&e) {
        Failure::BooksChanged | Failure::ParamsChanged => {
            d.banner = Some(Banner::BooksChanged);
        }
        // The role can't see it: a message, never "Try again" (S-L7).
        Failure::Denied => {
            d.banner = None;
            w.problem = Some("reports.builder.denied");
            if w.table.is_none() {
                w.state = RunState::Failed;
            }
            return;
        }
        _ => d.banner = Some(Banner::TryAgain),
    }
    if w.table.is_none() {
        w.error = Some(e);
        w.state = RunState::Failed;
    } else {
        w.state = RunState::Shown;
    }
}

#[allow(
    clippy::many_single_char_names,
    reason = "window, builder, detail tab, page, rows"
)]
fn poll_detail(w: &mut ReportWindow) {
    poll_detail_rerun(w);
    let Some(b) = w.builder.as_mut() else {
        return;
    };
    let def = b.def.clone();
    let Some(d) = b.detail.as_mut() else {
        return;
    };
    let Some((generation, job)) = d.job.as_mut() else {
        return;
    };
    let generation = *generation;
    let Some(r) = job.take() else {
        return;
    };
    d.job = None;
    // A page asked before a Refresh is dropped (#15).
    if generation != d.generation {
        return;
    }
    let page = match r {
        Ok(page) => page,
        Err(e) => {
            detail_failed(w, e);
            return;
        }
    };
    let mut rows = page.rows;
    let mut subtotal = page.subtotal;
    if let Some(def) = &def {
        apply_units_to_rows(def, &d.open.columns, &mut rows);
        apply_units_to_cells(def, &d.open.columns, &d.open.node_id, &mut subtotal);
    }
    d.paging.page(page.cursor, subtotal);
    if d.paging.books_changed {
        d.banner = Some(Banner::BooksChanged);
    }
    d.rows.extend(rows);
    let mut table_rows = d.rows.clone();
    if let Some(c) = d.paging.cursor.clone() {
        table_rows.push(ReportRow {
            kind: RowKind::More,
            level: 0,
            path: d.open.path.clone(),
            caption: "@reports.builder.more".into(),
            cells: vec![Cell::Empty; d.open.columns.len()],
            drill: Some(Drill::BuilderPage {
                node_id: d.open.node_id.clone(),
                path: d.open.path.clone(),
                cursor: Some(c),
            }),
            expandable: false,
        });
    }
    table_rows.push(ReportRow {
        kind: RowKind::Total,
        level: 0,
        path: d.open.path.clone(),
        caption: "@reports.builder.subtotal".into(),
        cells: d.paging.subtotal.clone().unwrap_or_default(),
        drill: None,
        expandable: false,
    });
    let reference = &d.open.reference;
    // A period column's drill covers that period: the header says so (#5).
    let dates = d.open.range.map(ReportDates::Range).or_else(|| {
        reference.dates.as_of.map(ReportDates::AsOf).or_else(|| {
            reference
                .dates
                .from
                .zip(reference.dates.to)
                .map(|(from, to)| ReportDates::Range(DateRange { from, to }))
        })
    });
    let table = ReportTable {
        columns: d.open.columns.clone(),
        rows: table_rows,
        meta: ReportMeta {
            dates,
            basis: reference.basis,
            ..ReportMeta::default()
        },
    };
    w.set_table(table);
    w.state = RunState::Shown;
}

/// The message over the table.
pub(crate) fn banner(w: &mut ReportWindow, ui: &mut Ui, tokens: &Tokens) {
    let Some(b) = w.builder.as_ref() else {
        return;
    };
    let banner = b.detail.as_ref().map_or(b.state.banner(), |d| d.banner);
    let key = match banner {
        Some(Banner::BooksChanged) => "reports.builder.books_changed",
        Some(Banner::TryAgain) => "reports.builder.try_again",
        Some(Banner::TookTooLong) => "reports.builder.took_too_long",
        Some(Banner::TooManyOpen) => "reports.builder.too_many_open",
        Some(Banner::TooLarge) => "reports.builder.too_large",
        None => return,
    };
    form::inline_warning(ui, tokens, &t(key));
    if matches!(banner, Some(Banner::BooksChanged | Banner::TryAgain))
        && ui.button(t("reports.tool.refresh")).clicked()
    {
        w.run();
    }
}

/// The starters in the catalog column.
pub(crate) fn catalog(w: &mut ReportWindow, ui: &mut Ui, tokens: &Tokens) {
    load_starters(w);
    let Some(list) = w.starters.list.as_ref() else {
        return;
    };
    if list.is_empty() {
        return;
    }
    let words: Vec<String> = w.search.split_whitespace().map(str::to_lowercase).collect();
    let shown: Vec<&Starter> = list
        .iter()
        .filter(|s| {
            let title = s.title.to_lowercase();
            words.iter().all(|x| title.contains(x.as_str()))
        })
        .collect();
    if shown.is_empty() {
        return;
    }
    ui.add_space(6.0);
    ui.label(
        RichText::new(t("reports.group.builder").to_uppercase())
            .size(11.0)
            .color(tokens.text_secondary),
    );
    let current = w.builder.as_ref().map(|b| b.key.clone());
    let mut pick = None;
    for s in shown {
        let on = current.as_deref() == Some(s.key.as_str());
        if ui
            .selectable_label(on, &s.title)
            .on_hover_text(t("reports.builder.starter_hint"))
            .clicked()
        {
            pick = Some(s.clone());
        }
    }
    if let Some(s) = pick {
        show_starter(w, &s);
    }
}

/// The control channel's view.
pub(crate) fn state_json(b: &Builder, table: Option<&ReportTable>) -> Value {
    let count = |k: RowKind| table.map_or(0, |t| t.rows.iter().filter(|r| r.kind == k).count());
    json!({
        "key": b.key,
        "denied": count(RowKind::Denied),
        "detail_rows": count(RowKind::Detail),
        "more": count(RowKind::More),
        "phase": b.state.phase().name(),
        "generation": b.state.generation(),
        "open": b.state.open_paths().len(),
        "expand_all": b.state.is_open_all(),
        "banner": b.detail.as_ref().map_or(b.state.banner(), |d| d.banner).map(|x| format!("{x:?}")),
        "detail": b.detail.as_ref().map(|d| json!({"pages": d.paging.pages, "more": d.paging.has_more()})),
        "paging": b.page.is_some(),
        "loaded": b.loaded,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_presets_map_to_the_windows() {
        assert_eq!(
            preset(Some("this_year_to_date")),
            Some(DatePreset::YearToDate)
        );
        assert_eq!(preset(Some("today")), Some(DatePreset::Today));
        assert_eq!(preset(Some("fortnight")), None);
    }

    #[test]
    fn fixed_engine_captions_become_keys() {
        assert_eq!(
            caption_key("No name"),
            Some("@reports.builder.none.contact")
        );
        assert_eq!(caption_key("Rivera Remodel"), None);
        let col = |key: &str, caption: &str| ColumnHead {
            key: key.into(),
            caption: caption.into(),
            kind: cl_core::report::ColumnKind::Amount,
            range: None,
            drill: None,
        };
        assert_eq!(
            column_key(&col("total:c0", "Prior Year")).as_deref(),
            Some("@reports.builder.col.prior_year")
        );
        assert_eq!(
            column_key(&col("f:memo", "Memo")).as_deref(),
            Some("@reports.builder.field.memo")
        );
        assert_eq!(column_key(&col("p0", "Jan 2026")), None);
    }
}
