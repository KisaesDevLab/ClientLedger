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
    BuilderRefusal, BuilderRun, ColumnLayout, DetailPage, DetailRequest, RunPurpose, RunRef,
    RunRequest, Starter, builder_refusal,
};
use cl_api::resources::reports::definitions::DefinitionRow;
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
use cl_model::reports::{BUILDER_RANGE, ColumnBy, DateChoice, DatePreset, ReportOptions};
use eframe::egui::{RichText, Ui};
use serde_json::{Value, json};

use super::KIND;
use super::builder::{BuilderSeed, CustomizeSeed, EditorId};
use super::builder_ref::BuilderRunOpen;
use super::window::{ReportWindow, RunState};
use crate::company::CompanyEvent;
use crate::i18n::t;
use crate::screens::company::form;
use crate::screens::invoice::engine::{self, Job};
use crate::theme::Tokens;
use crate::windows::OpenPayload;
use cl_model::report_def::editor::{Origin, Saved, SavedKind};

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
    /// Every column, empty field columns too, for a drill tab (U-119f).
    detail_columns: Vec<ColumnHead>,
    detail_layout: ColumnLayout,
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
    /// The window's options as the report was shown: Memorize writes only
    /// what changed since into the definition (A08).
    baseline: Option<ReportOptions>,
    /// The editor whose Run report this is (Q-A08x).
    pub(crate) editor: Option<EditorId>,
    /// Where the editor's report started (Customize returns to it).
    origin: Option<Origin>,
    /// The memorized row behind the editor's report.
    saved: Option<Saved>,
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
            baseline: None,
            editor: None,
            origin: None,
            saved: None,
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
                .has(cl_api::Capabilities::REPORT_BUILDER_STARTERS)
            // Every drill path a run gives and rows that keep their paths
            // (U-119e): the adapter refuses an older engine's rows.
            && w
                .link
                .capabilities()
                .has(cl_api::Capabilities::REPORT_BUILDER_DRILL)
            // Starters v2 (U-119f): an older engine's starters show bare
            // drills and debit-signed ledgers; it needs an update.
            && w
                .link
                .capabilities()
                .has(cl_api::Capabilities::REPORT_BUILDER_STARTERS_V2))
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
    w.new_view();
    let mut b = Builder::new(key);
    b.wanted = Some(key.to_string());
    w.builder = Some(Box::new(b));
    w.table = None;
    w.error = None;
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
    w.new_view();
    let text = serde_json::to_string(&starter.definition).unwrap_or_default();
    let mut b = Builder::new(&starter.key);
    match cl_model::report_def::parse(&text) {
        Ok(def) => {
            b.title = def.title.clone().unwrap_or_else(|| starter.title.clone());
            // Aging buckets refuse period and compare ranges (U-119e) and a
            // running balance takes no period columns: no Columns choice
            // for them (#2).
            let spec = super::builder_ref::spec_for(&def);
            let fixed = spec.columns.len() == 1;
            let mut o = ReportOptions::for_spec(spec);
            if let Some(p) = preset(starter.definition["dates"]["preset"].as_str()) {
                o.dates = DateChoice::Preset(p);
            }
            o.columns = match starter.definition["columns"]["periods"].as_str() {
                Some("month") if !fixed => ColumnBy::Month,
                Some("quarter") if !fixed => ColumnBy::Quarter,
                Some("year") if !fixed => ColumnBy::Year,
                _ => ColumnBy::Total,
            };
            o.basis = match starter.definition["basis"].as_str() {
                Some("cash") => Some(cl_core::report::Basis::Cash),
                Some("accrual") => Some(cl_core::report::Basis::Accrual),
                _ => o.basis,
            };
            b.def = Some(def);
            b.json = starter.definition.clone();
            b.baseline = Some(o.clone());
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
    w.new_view();
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

/// The run's reference ([`super::builder_ref::run_ref`], the one the
/// editor's preview sends too, AC8).
fn reference(
    w: &ReportWindow,
    def: &BuilderDefinition,
    json: &Value,
) -> Result<RunRef, &'static str> {
    super::builder_ref::run_ref(&super::builder_ref::RunInput {
        def,
        json,
        spec: w.spec,
        options: &w.options,
        fiscal: &w.fiscal,
        today: w.today,
    })
}

/// Runs (Refresh, a date or basis change).
pub(crate) fn run(w: &mut ReportWindow) {
    // A memorized builder report on an engine without the drill contract
    // says the engine needs an update, as a starter does (Q-A07u(6)).
    if w.builder.is_some() && !available(w) {
        w.table = None;
        w.state = RunState::Unavailable;
        return;
    }
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
    let Some(def) = b.def.clone() else {
        return;
    };
    match reference(w, &def, &json) {
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
    let job = run_job(&w.link, w.group.as_ref(), req, slow);
    b.job = Some((generation, job));
}

/// A builder run on the engine (the demo plays it in demo mode); `slow`
/// makes the demo take its time (the control channel's cancel checks).
/// The editor's preview runs through it too (AC8).
pub(crate) fn run_job(
    link: &crate::company::CompanyLink,
    group: Option<&cl_api::RequestGroup>,
    req: RunRequest,
    slow: bool,
) -> Job<BuilderRun> {
    let live = req.clone();
    engine::read(
        link,
        group,
        move |d| crate::company::demo::report_builder::run(d, &req, slow),
        move |s| async move { s.report_builder().run(&live).await },
    )
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

/// What the window (and the editor's preview) says for an engine refusal.
pub(crate) fn failure(e: &ApiError) -> Failure {
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
        // Aging-bucket groups, as the aging columns head them (A07 QA-6).
        "1 - 30" => "@reports.col.days_1_30",
        "31 - 60" => "@reports.col.days_31_60",
        "61 - 90" => "@reports.col.days_61_90",
        "Over 90" => "@reports.col.over_90",
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

/// A period column's heading as the standard reports write it ("Jan
/// 2026", "2026"); other ranges keep the engine's caption (A07 QA-6).
fn period_caption(c: &ColumnHead) -> Option<String> {
    let base = c.key.split(':').next().unwrap_or_default();
    let period = base.len() > 1
        && base.starts_with('p')
        && base[1..].bytes().all(|b| b.is_ascii_digit())
        && base == c.key;
    let r = c.range.filter(|_| period)?;
    let caption = crate::company::demo::reports::caption_of(r);
    (!caption.contains(" - ")).then_some(caption)
}

/// A detail row's type cell in the app's words ("Journal entry", "Check"),
/// from the row's own transaction type (A07 QA-6).
fn type_cell(r: &cl_core::report::ReportRow) -> Option<Cell> {
    let Some(cl_core::report::Drill::Transaction { txn_type, .. }) = &r.drill else {
        return None;
    };
    let t = if txn_type == "check" {
        "expense"
    } else {
        txn_type
    };
    let key = cl_api::resources::reports::adapters::ledger::type_label(t);
    key.starts_with('@').then_some(Cell::Text(key))
}

/// Every detail row's type cell in the app's words.
fn dress_types(columns: &[ColumnHead], rows: &mut [cl_core::report::ReportRow]) {
    let Some(ci) = columns.iter().position(|c| c.key == "f:txn_type") else {
        return;
    };
    for r in rows.iter_mut().filter(|r| r.kind == RowKind::Detail) {
        if let Some(cell) = type_cell(r)
            && let Some(slot) = r.cells.get_mut(ci)
        {
            *slot = cell;
        }
    }
}

/// Column headings in the app's words (the definition's own headers kept).
fn dress_columns(def: Option<&BuilderDefinition>, columns: &mut [ColumnHead]) {
    let Some(def) = def else { return };
    for c in columns {
        if !def.columns.headers.contains_key(&c.key)
            && let Some(k) = column_key(c).or_else(|| period_caption(c))
        {
            c.caption = k;
        }
    }
}

/// The table as the window shows it: units, i18n captions, the dash. The
/// editor's preview dresses its rows here too, so they equal the result
/// tab's (AC8).
pub(crate) fn dress(
    def: Option<&BuilderDefinition>,
    mut table: ReportTable,
    denied: bool,
) -> ReportTable {
    if let Some(def) = def {
        apply_units(def, &mut table);
    }
    dress_columns(def, &mut table.columns);
    dress_types(&table.columns, &mut table.rows);
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
    poll_edit(w);
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
            let table = dress(b.def.as_ref(), run.table, denied);
            let mut detail_columns = run.detail_columns;
            dress_columns(b.def.as_ref(), &mut detail_columns);
            b.shown = b.reference.clone().map(|reference| Shown {
                reference,
                layout: run.layout,
                detail_columns,
                detail_layout: run.detail_layout,
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
            dress_types(&table.columns, &mut rows);
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

/// True when Collapse closes everything (anything is open); else the
/// button expands all (A07 QA-9).
pub(crate) fn collapses(b: &Builder) -> bool {
    b.state.is_open_all() || !b.state.open_paths().is_empty()
}

/// Expand all, or Collapse all when anything is open.
pub(crate) fn toggle_all(w: &mut ReportWindow) {
    if let Some(b) = w.builder.as_mut() {
        let fx = if collapses(b) {
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
    let shown = b.shown.as_ref()?;
    w.table.as_ref()?;
    Some(BuilderDetailOpen {
        title: format!("{} \u{2014} {}", b.title(), t("reports.builder.detail")),
        definition: b.json.clone(),
        reference: shown.reference.clone(),
        columns: shown.detail_columns.clone(),
        layout: shown.detail_layout.clone(),
        node_id: node_id.to_string(),
        path: path.clone(),
        range,
        fingerprint: shown.fingerprint.clone(),
        stamp: shown.stamp,
    })
}

/// What a drill on the editor's preview needs from its run: the same as a
/// run window's (plan §3.4, "underlined amounts drill as in A07").
#[derive(Clone, Debug)]
pub(crate) struct DrillBase {
    title: String,
    definition: Value,
    reference: RunRef,
    detail_columns: Vec<ColumnHead>,
    detail_layout: ColumnLayout,
    fingerprint: String,
    stamp: u64,
}

impl DrillBase {
    /// The drill base of a preview `run` of `def` (titled `title`) sent as
    /// `reference`.
    pub(crate) fn of(
        title: String,
        def: &BuilderDefinition,
        reference: RunRef,
        run: &BuilderRun,
    ) -> DrillBase {
        let mut detail_columns = run.detail_columns.clone();
        dress_columns(Some(def), &mut detail_columns);
        DrillBase {
            title,
            definition: serde_json::to_value(def).unwrap_or(Value::Null),
            reference,
            detail_columns,
            detail_layout: run.detail_layout.clone(),
            fingerprint: run.fingerprint.clone(),
            stamp: run.stamp,
        }
    }

    /// The detail tab of `node_id` at `path` (a subtotal's transactions).
    pub(crate) fn detail(
        &self,
        node_id: &str,
        path: &RowPath,
        range: Option<DateRange>,
    ) -> BuilderDetailOpen {
        BuilderDetailOpen {
            title: format!("{} \u{2014} {}", self.title, t("reports.builder.detail")),
            definition: self.definition.clone(),
            reference: self.reference.clone(),
            columns: self.detail_columns.clone(),
            layout: self.detail_layout.clone(),
            node_id: node_id.to_string(),
            path: path.clone(),
            range,
            fingerprint: self.fingerprint.clone(),
            stamp: self.stamp,
        }
    }
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
            d.open.layout = run.detail_layout;
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
    dress_types(&d.open.columns, &mut rows);
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
    // A column's drill covers that column: the header says so (#5); an
    // as-of column's from the beginning is its date.
    let column = d.open.range.map(|r| {
        if r.from == cl_core::CalendarDate::min() {
            ReportDates::AsOf(r.to)
        } else {
            ReportDates::Range(r)
        }
    });
    let dates = column.or_else(|| {
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
    // A08: a new report starts in Guided mode (Q-A08a).
    if ui
        .selectable_label(false, t("reports.builder.new_report"))
        .on_hover_text(t("reports.builder.new_report.hint"))
        .clicked()
    {
        super::builder::open(&w.link, BuilderSeed::Guided);
    }
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

// ------------------------------------------------- A08: editor entry points

/// A Customize waiting for something (WP-B3, plan §3.7).
#[derive(Default)]
pub(crate) struct Edit {
    /// Customize on a standard report waits for the starters list.
    wanted: bool,
    /// Customize on a memorized builder report re-reads its row first, once
    /// per press (Q-A07u(1)): `saved` and `can_edit` are the engine's now.
    job: Option<Job<DefinitionRow>>,
}

impl Edit {
    /// Waiting for the starters or the row.
    pub(crate) fn busy(&self) -> bool {
        self.wanted || self.job.is_some()
    }
}

/// Opens an editor's Run report in the window (`OpenPayload::
/// BuilderDefinition`, Q-A08x): a full run of its definition with the
/// editor's dates, basis and period columns (the same `RunRef` as the
/// editor's preview, AC8).
pub(crate) fn open_definition(w: &mut ReportWindow, open: BuilderRunOpen) {
    w.new_view();
    w.after_run = open.then;
    let mut b = Builder::new("editor");
    b.title.clone_from(&open.title);
    b.json = serde_json::to_value(&open.def).unwrap_or(Value::Null);
    b.editor = Some(open.editor);
    b.origin = Some(open.origin);
    b.saved = open.saved;
    b.baseline = Some(open.options.clone());
    w.spec = super::builder_ref::spec_for(&open.def);
    b.def = Some(open.def);
    w.options = open.options;
    w.builder = Some(Box::new(b));
    w.table = None;
    w.error = None;
    // Memorize in the result tab makes a new report; the editor's own
    // Save changes updates its row.
    w.memo.current = None;
    w.memo.saved = None;
    w.run();
}

/// Shows an editor's Run report in its result tab (a new report window the
/// first time, the same tab after, Q-A08x).
pub(crate) fn show_result(link: &crate::company::CompanyLink, open: BuilderRunOpen) {
    link.push(CompanyEvent::OpenWith(
        CommandId::new(KIND),
        Box::new(OpenPayload::BuilderDefinition(Box::new(open))),
    ));
}

/// The window is `payload`'s editor's result tab (`Screen::takes_in_place`).
pub(crate) fn takes(w: &ReportWindow, payload: &OpenPayload) -> bool {
    matches!(
        payload,
        OpenPayload::BuilderDefinition(o)
            if w.builder.as_ref().is_some_and(|b| b.editor == Some(o.editor))
    )
}

/// Why Customize (open in the Report Builder) is off for what the window
/// shows, as an i18n key; `None` when it works (Q-A08k).
pub(crate) fn customize_off(w: &ReportWindow) -> Option<&'static str> {
    if let Some(b) = &w.builder {
        if b.detail.is_some() {
            return Some("reports.customize.detail");
        }
        if b.def.is_none() {
            return Some(b.problem.unwrap_or("reports.builder.unreadable"));
        }
        return (!available(w)).then_some("reports.customize.engine");
    }
    let Some(key) = cl_model::reports::starter_for(w.spec.key) else {
        return Some("reports.customize.no_starter");
    };
    if !available(w) {
        return Some("reports.customize.engine");
    }
    w.starters
        .list
        .as_ref()
        .is_some_and(|l| !l.iter().any(|s| s.key == key))
        .then_some("reports.customize.no_starter")
}

/// Customize: opens the Report Builder editor on what the window shows (a
/// standard report's starter, a starter, a memorized builder report, or the
/// editor this result tab belongs to).
pub(crate) fn customize(w: &mut ReportWindow) {
    if customize_off(w).is_some() || w.edit.busy() {
        return;
    }
    if w.builder.is_some() {
        if let Some(id) = w
            .memo
            .current
            .as_ref()
            .filter(|c| c.kind == "builder")
            .map(|c| c.id.clone())
        {
            let demo_id = id.clone();
            w.edit.job = Some(engine::read(
                &w.link,
                w.group.as_ref(),
                move |d| crate::company::demo::report_store::definition(d, &demo_id),
                move |s| async move { s.report_definitions().get(&id).await },
            ));
            return;
        }
        seed_from_builder(w, None);
        return;
    }
    load_starters(w);
    w.edit.wanted = true;
    poll_edit(w);
}

/// Polls a Customize in waiting.
fn poll_edit(w: &mut ReportWindow) {
    if let Some(r) = engine::poll(&mut w.edit.job) {
        match r {
            Ok(row) => seed_from_builder(w, Some(row)),
            Err(e) => {
                w.notice = Some(t(match failure(&e) {
                    Failure::Deleted => "reports.builder.deleted",
                    _ => "reports.customize.failed",
                }));
            }
        }
    }
    if !w.edit.wanted {
        return;
    }
    let Some(list) = w.starters.list.as_ref() else {
        return;
    };
    w.edit.wanted = false;
    let key = cl_model::reports::starter_for(w.spec.key);
    match list.iter().find(|s| Some(s.key.as_str()) == key).cloned() {
        Some(st) => seed_from_standard(w, &st),
        None => w.notice = Some(t("reports.customize.no_starter")),
    }
}

/// The editor's view of a memorized row.
fn saved_of(row: &DefinitionRow, def: BuilderDefinition, kind: SavedKind) -> Saved {
    Saved {
        id: row.id.clone(),
        version: row.version,
        kind,
        can_edit: row.can_edit,
        def,
        name: row.name.clone(),
        group_id: row.group_id.clone(),
        author: row.created_by_name.clone(),
        updated_at: row.updated_at,
    }
}

/// A definition's text read, or the notice that says why it can't be.
fn read_def(w: &mut ReportWindow, v: &Value) -> Option<BuilderDefinition> {
    let text = serde_json::to_string(v).unwrap_or_default();
    match cl_model::report_def::parse(&text) {
        Ok(def) => Some(def),
        Err(e) => {
            w.notice = Some(t(
                if matches!(e, cl_model::report_def::DefinitionError::Newer) {
                    "reports.builder.newer"
                } else {
                    "reports.builder.unreadable"
                },
            ));
            None
        }
    }
}

/// Customize on a builder report (`row`: its memorized row, as re-read).
fn seed_from_builder(w: &mut ReportWindow, row: Option<DefinitionRow>) {
    let (def, saved, origin) = if let Some(row) = row {
        let Some(def) = read_def(w, &row.definition) else {
            return;
        };
        let saved = saved_of(&row, def.clone(), SavedKind::Builder);
        (def, Some(saved), Origin::Memorized)
    } else {
        let Some(b) = w.builder.as_ref() else {
            return;
        };
        let Some(def) = b.def.clone() else {
            return;
        };
        let origin = b
            .origin
            .clone()
            .unwrap_or_else(|| Origin::Starter(b.key.clone()));
        (def, b.saved.clone(), origin)
    };
    let Some(b) = w.builder.as_ref() else {
        return;
    };
    let base = b.baseline.as_ref();
    // Only dates changed in this tab come along: back to its editor
    // (Q-A08x) the editor may have moved on since its Run report (R2); a
    // memorized report runs on the window's defaults where it has none
    // (no preset), and those are not a change to it (QA-15).
    let dates = if base.is_some_and(|x| x.dates == w.options.dates) {
        None
    } else {
        Some(w.options.dates)
    };
    let seed = CustomizeSeed {
        title: saved.as_ref().map_or_else(|| b.title(), |s| s.name.clone()),
        def,
        origin,
        saved,
        dates,
        // The definition's own basis unless the owner changed it here.
        basis: if base.is_some_and(|x| x.basis == w.options.basis) {
            None
        } else {
            w.options.basis
        },
        adjusting: None,
        filters_dropped: false,
        editor: b.editor,
    };
    super::builder::open(&w.link, BuilderSeed::Customize(Box::new(seed)));
}

/// Customize on a standard report: the editor on its starter, with the
/// window's dates, basis and adjusting lens, not its filters (Q-A08k).
fn seed_from_standard(w: &mut ReportWindow, st: &Starter) {
    let Some(def) = read_def(w, &st.definition) else {
        return;
    };
    // A memorized standard report: Memorize makes a new builder report
    // (the kind can't change); Save changes stays off.
    let saved = w
        .memo
        .current
        .as_ref()
        .filter(|c| c.kind == "standard")
        .map(|row| saved_of(row, def.clone(), SavedKind::Standard));
    let o = &w.options;
    let seed = CustomizeSeed {
        title: saved
            .as_ref()
            .map_or_else(|| w.title_text(), |s| s.name.clone()),
        def,
        origin: Origin::Standard(w.spec.key.to_string()),
        saved,
        dates: Some(o.dates),
        basis: o.basis,
        adjusting: o.filters.adjusting,
        filters_dropped: o.filters != cl_model::reports::options::Filters::default()
            || o.tag_split.is_some(),
        editor: None,
    };
    super::builder::open(&w.link, BuilderSeed::Customize(Box::new(seed)));
}

/// The engine's name for a date preset (the reverse of [`preset`]).
fn preset_name(p: DatePreset) -> &'static str {
    match p {
        DatePreset::Today => "today",
        DatePreset::ThisMonth => "this_month",
        DatePreset::LastMonth => "last_month",
        DatePreset::ThisQuarter => "this_quarter",
        DatePreset::LastQuarter => "last_quarter",
        DatePreset::ThisYear => "this_year",
        DatePreset::LastYear => "last_year",
        DatePreset::YearToDate => "this_year_to_date",
        DatePreset::EndOfLastMonth => "end_of_last_month",
    }
}

/// The definition Memorize saves for the builder report shown: its own,
/// with the window's date preset, basis and period columns where the owner
/// changed them here (fixed From/To dates stay session only, Q-A08q).
/// `None` for a detail tab or an unreadable definition.
pub(crate) fn memorize_body(w: &ReportWindow) -> Option<Value> {
    let b = w.builder.as_ref().filter(|b| b.detail.is_none())?;
    b.def.as_ref()?;
    let mut v = b.json.clone();
    let base = b.baseline.as_ref();
    let o = &w.options;
    let obj = v.as_object_mut()?;
    if base.is_none_or(|x| x.dates != o.dates)
        && let DateChoice::Preset(p) = o.dates
        && let Some(dates) = obj.get_mut("dates").and_then(Value::as_object_mut)
    {
        dates.insert("preset".into(), Value::from(preset_name(p)));
    }
    if base.is_none_or(|x| x.basis != o.basis) {
        match o.basis {
            Some(basis) => {
                obj.insert("basis".into(), Value::from(basis.as_str()));
            }
            None => {
                obj.remove("basis");
            }
        }
    }
    if base.is_none_or(|x| x.columns != o.columns)
        && let Some(columns) = obj.get_mut("columns").and_then(Value::as_object_mut)
    {
        match o.columns {
            ColumnBy::Month => {
                columns.insert("periods".into(), Value::from("month"));
            }
            ColumnBy::Quarter => {
                columns.insert("periods".into(), Value::from("quarter"));
            }
            ColumnBy::Year => {
                columns.insert("periods".into(), Value::from("year"));
            }
            _ => {
                columns.remove("periods");
            }
        }
    }
    let text = serde_json::to_string(&v).ok()?;
    cl_model::report_def::parse(&text).ok()?;
    Some(v)
}

/// A builder report was memorized as `row` (Memorize in the window): the
/// window shows that definition from now on.
pub(crate) fn memorized(w: &mut ReportWindow, row: &DefinitionRow) {
    let options = w.options.clone();
    let Some(b) = w.builder.as_mut() else {
        return;
    };
    let text = serde_json::to_string(&row.definition).unwrap_or_default();
    if let Ok(def) = cl_model::report_def::parse(&text) {
        b.def = Some(def);
        b.json = row.definition.clone();
        b.baseline = Some(options);
    }
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

    /// A07 QA-6: period columns head like the standard reports' ("Jan
    /// 2026", "2026"), a part-month keeps the engine's range; aging groups
    /// read as the aging columns; types in the app's words.
    #[test]
    fn builder_text_follows_the_app() {
        let day = |m, d| cl_core::CalendarDate::from_ymd(2026, m, d).expect("date");
        let col = |key: &str, from, to| ColumnHead {
            key: key.into(),
            caption: "Jan 1, 2026 - Jan 31, 2026".into(),
            kind: cl_core::report::ColumnKind::Amount,
            range: Some(cl_core::report::DateRange { from, to }),
            drill: None,
        };
        assert_eq!(
            period_caption(&col("p0", day(1, 1), day(1, 31))).as_deref(),
            Some("Jan 2026")
        );
        assert_eq!(
            period_caption(&col("p11", day(1, 1), day(12, 31))).as_deref(),
            Some("2026")
        );
        assert_eq!(period_caption(&col("p0", day(1, 1), day(1, 15))), None);
        // The Total and compare columns keep theirs.
        assert_eq!(period_caption(&col("total", day(1, 1), day(1, 31))), None);
        assert_eq!(period_caption(&col("p0:c0", day(1, 1), day(1, 31))), None);
        assert_eq!(caption_key("1 - 30"), Some("@reports.col.days_1_30"));
        assert_eq!(caption_key("Over 90"), Some("@reports.col.over_90"));
        let row = |txn_type: &str| cl_core::report::ReportRow {
            kind: RowKind::Detail,
            level: 0,
            path: cl_core::report::RowPath::of(["n:a"]),
            caption: String::new(),
            cells: vec![Cell::Text("Journal Entry".into())],
            drill: Some(cl_core::report::Drill::Transaction {
                id: "d0000000-0000-4000-8000-b00000000001".parse().expect("id"),
                txn_type: txn_type.into(),
            }),
            expandable: false,
        };
        let mut rows = vec![row("journal_entry"), row("check"), row("something_new")];
        let mut type_col = col("f:txn_type", day(1, 1), day(1, 1));
        type_col.kind = cl_core::report::ColumnKind::Text;
        dress_types(&[type_col], &mut rows);
        assert_eq!(
            rows[0].cells[0],
            Cell::Text("@reports.txn.journal_entry".into())
        );
        assert_eq!(rows[1].cells[0], Cell::Text("@reports.txn.expense".into()));
        // A type the app doesn't know keeps the engine's words.
        assert_eq!(rows[2].cells[0], Cell::Text("Journal Entry".into()));
        assert_eq!(crate::i18n::t("reports.txn.expense"), "Check");
    }
}
