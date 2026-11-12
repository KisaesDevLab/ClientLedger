//! The live preview (plan §3.4, AC4, AC8, Q-A08j).
//!
//! - One run path: the same `RunRef` as Run report
//!   (`builder_ref::run_ref`), sent as a `screen` run through
//!   `builder_run::run_job`; the table is dressed by `builder_run::dress`
//!   as the result tab's is. Every figure is the engine's.
//! - With no client-side problem, an edit schedules a run 400 ms after the
//!   last edit; Ctrl+Enter runs now; a newer edit cancels the run in
//!   flight and a late answer is dropped by generation (A07's
//!   `BuilderRunState`).
//! - The 50-row cut: with `report-builder-preview-v1` (U-217) the run is
//!   sent with purpose `preview` and the engine cuts each line at 50
//!   transactions (the line's "more" row marks the cut; not shown here).
//!   An older engine gets the `screen` run and the cut is display only:
//!   after the 50th transaction row in display order, later transaction
//!   and "more" rows are not shown. Either way one info row says so.
//!   Rows are counted, never amounts.
//! - A failure shows over the last good table, dimmed; while problems
//!   exist nothing is sent and the last good table stays under them.

use cl_api::resources::report_builder::{BuilderRun, RunPurpose, RunRef, RunRequest};
use cl_core::report::{Cell, Drill, ReportTable, RowKind, RowPath};
use cl_model::report_def::BuilderDefinition;
use cl_model::report_def::editor::BuilderEditor;
use cl_model::report_def::problems::{Anchor, Problem};
use cl_model::report_def::run_state::{Banner, BuilderRunState, Effect, Phase};
use eframe::egui::{self, RichText, Ui};
use serde_json::{Value, json};

use super::{BuilderScreen, Pane, PropTab};
use crate::i18n::{t, t_with};
use crate::screens::company::form;
use crate::screens::invoice::engine::Job;
use crate::theme::Tokens;

/// Debounce after the last edit (plan §3.4).
pub(crate) const PREVIEW_DEBOUNCE_MS: u64 = 400;
/// Transaction rows the preview shows (Q-A08j).
pub(crate) const PREVIEW_DETAIL_ROWS: usize = 50;

/// The preview's run.
#[derive(Default)]
pub(crate) struct Preview {
    state: BuilderRunState,
    /// The reference and definition of the run asked for.
    asked: Option<(RunRef, BuilderDefinition)>,
    job: Option<(u64, Job<BuilderRun>)>,
    /// The last good table, dressed, and the definition it ran.
    pub(crate) table: Option<ReportTable>,
    /// The reference behind [`Self::table`] (AC8: equal to Run report's).
    pub(crate) shown_ref: Option<RunRef>,
    /// An edit waits to run: when (ms) it is due.
    due: Option<u64>,
    /// Edits since the last run was asked.
    stale: bool,
    /// Ctrl+Enter: run at the next frame.
    now: bool,
    /// Why the last run failed (an i18n key), shown over the table.
    pub(crate) failed: Option<&'static str>,
    /// When the table was made ("Updated 9:41 AM").
    pub(crate) ran_at: Option<String>,
    /// Runs asked (tests: debounce and cancel).
    pub(crate) asked_count: u64,
    /// The engine cut [`Self::table`] per line (a preview run, U-217).
    pub(crate) engine_cut: bool,
    /// What a drill on [`Self::table`] opens from (plan §3.4).
    pub(crate) drill_base: Option<super::super::builder_run::DrillBase>,
    /// A drill's read of the transaction it opens (an expense or deposit).
    drill_job: Option<(cl_core::ids::TransactionId, Job<super::super::drill::Opens>)>,
    /// A drill that opened nothing says why (an i18n key).
    pub(crate) drill_note: Option<&'static str>,
}

impl Preview {
    /// True while a run is awaited or due.
    pub(crate) fn busy(&self) -> bool {
        self.job.is_some()
            || self.stale
            || self.state.due_at().is_some()
            || self.drill_job.is_some()
    }

    /// The report changed: a run is due after the debounce, and the run in
    /// flight no longer matters: it is dropped now, so its answer (made for
    /// the report before the edit) never lands on the edited one (R1).
    pub(crate) fn edited(&mut self, _ed: &BuilderEditor) {
        self.stale = true;
        self.due = None;
        self.job = None;
        self.state.cancel();
    }

    /// A run was sent and its answer not yet taken (tests).
    #[cfg(test)]
    pub(crate) fn in_flight(&self) -> bool {
        self.job.is_some()
    }

    /// Ctrl+Enter.
    pub(crate) fn run_now(&mut self) {
        self.stale = true;
        self.now = true;
    }

    /// For the control channel (no amounts).
    pub(crate) fn state_json(&self) -> Value {
        let rows = self
            .table
            .as_ref()
            .map(|t| shown(t, self.engine_cut).0.len());
        json!({
            "phase": self.state.phase().name(),
            "rows": rows,
            "cut": self.table.as_ref().is_some_and(|t| shown(t, self.engine_cut).1),
            "engine_cut": self.engine_cut,
            "failed": self.failed,
            "pending": self.stale,
            "asked": self.asked_count,
        })
    }
}

/// The node a preview row belongs to: the innermost `n:<id>` of its path.
pub(crate) fn node_of(path: &RowPath) -> Option<&str> {
    path.0.iter().rev().find_map(|p| p.strip_prefix("n:"))
}

/// The rows shown, in engine order, and whether the 50-row cut hid some.
///
/// Both paths cut the same way (R4): after the 50th transaction row in
/// display order, later transaction and "more" rows are not shown, so the
/// preview shows at most 50 transactions in all and its caption is true.
/// `engine_cut`: the engine already cut each line at 50 (a preview run,
/// U-217); a line's "more" row then also says rows were cut.
pub(crate) fn shown(table: &ReportTable, engine_cut: bool) -> (Vec<usize>, bool) {
    let mut out = Vec::new();
    let mut details = 0usize;
    let mut cut = engine_cut && table.rows.iter().any(|r| r.kind == RowKind::More);
    for (i, r) in table.rows.iter().enumerate() {
        let detail = matches!(r.kind, RowKind::Detail | RowKind::More);
        if detail && details >= PREVIEW_DETAIL_ROWS {
            cut = true;
            continue;
        }
        if r.kind == RowKind::Detail {
            details += 1;
        }
        out.push(i);
    }
    (out, cut)
}

fn now_ms(ui: &Ui) -> u64 {
    let s = ui.input(|i| i.time).max(0.0) * 1000.0;
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "frame time in ms, non-negative and far below u64::MAX"
    )]
    let ms = s as u64;
    ms
}

/// The reference a run of the editor's report sends now (the same as its
/// Run report tab's, AC8).
pub(crate) fn reference(s: &BuilderScreen) -> Result<RunRef, &'static str> {
    let def = s.ed.definition();
    let json = serde_json::to_value(def).map_err(|_| "reports.problem.fiscal")?;
    let (spec, options) = s.run_options();
    super::super::builder_ref::run_ref(&super::super::builder_ref::RunInput {
        def,
        json: &json,
        spec,
        options: &options,
        fiscal: &s.fiscal,
        today: s.today,
    })
}

/// Each frame: take answers, send a due run.
pub(crate) fn tick(s: &mut BuilderScreen, ui: &Ui) {
    poll(s);
    let now = now_ms(ui);
    let fx = s.preview.state.tick(now);
    apply(s, fx);
    if !s.preview.stale {
        return;
    }
    if !s.ed.can_run() || s.ed.definition().nodes.is_empty() {
        // Nothing is sent while problems exist; the last good table stays.
        cancel_in_flight(s);
        s.preview.stale = false;
        s.preview.now = false;
        return;
    }
    let due = *s
        .preview
        .due
        .get_or_insert(now.saturating_add(PREVIEW_DEBOUNCE_MS));
    if !s.preview.now && now < due {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(due - now));
        return;
    }
    s.preview.now = false;
    s.preview.stale = false;
    s.preview.due = None;
    start(s);
}

fn cancel_in_flight(s: &mut BuilderScreen) {
    s.preview.job = None;
}

/// Asks for a run of the report as it is now.
fn start(s: &mut BuilderScreen) {
    match reference(s) {
        Ok(r) => {
            s.preview.asked = Some((r, s.ed.definition().clone()));
            s.preview.failed = None;
            let fx = s.preview.state.refresh();
            apply(s, fx);
        }
        Err(key) => {
            s.preview.failed = Some(key);
        }
    }
}

fn apply(s: &mut BuilderScreen, fx: Vec<Effect>) {
    for e in fx {
        match e {
            Effect::Run {
                generation,
                open,
                collapsed,
                expand_all,
            } => {
                let Some((reference, _)) = s.preview.asked.clone() else {
                    continue;
                };
                // U-217: the engine cuts a preview run (the same figures
                // as the full run; purpose is outside the fingerprint).
                let purpose = if s.link.is_demo()
                    || s.link
                        .capabilities()
                        .has(cl_api::Capabilities::REPORT_BUILDER_PREVIEW)
                {
                    RunPurpose::Preview
                } else {
                    RunPurpose::Screen
                };
                let req = RunRequest {
                    reference,
                    open,
                    collapsed,
                    expand_all,
                    purpose,
                };
                s.preview.asked_count += 1;
                let job = super::super::builder_run::run_job(&s.link, s.group.as_ref(), req, false);
                s.preview.job = Some((generation, job));
            }
            Effect::Cancel { generation } => {
                if s.preview
                    .job
                    .as_ref()
                    .is_some_and(|(g, _)| *g == generation)
                {
                    s.preview.job = None;
                }
            }
            // The preview pages nothing: its cut is a display one.
            Effect::Page { .. } => {}
        }
    }
}

fn poll(s: &mut BuilderScreen) {
    if let Some((id, job)) = s.preview.drill_job.as_mut()
        && let Some(r) = job.take()
    {
        let id = *id;
        s.preview.drill_job = None;
        s.preview.drill_note = match r {
            Ok(found) => super::super::drill::opened(&s.link, id, found),
            Err(_) => Some("builder.preview.drill_failed"),
        };
    }
    let Some((generation, job)) = s.preview.job.as_mut() else {
        return;
    };
    let generation = *generation;
    let Some(r) = job.take() else {
        return;
    };
    s.preview.job = None;
    // Only an answer for the report as it is now counts (R1): the session
    // dates or the fiscal year can move the run without an edit.
    let now = reference(s).ok();
    if s.preview.asked.as_ref().map(|(r, _)| r) != now.as_ref() {
        s.preview.state.cancel();
        s.preview.stale = true;
        return;
    }
    let title = s.report_title();
    let p = &mut s.preview;
    match r {
        Ok(run) => {
            if !p.state.run_ok(generation, run.stamp, &run.expanded) {
                return;
            }
            let denied = !run.denied.is_empty();
            p.engine_cut = run.preview_rows.is_some();
            let def = p.asked.as_ref().map(|(_, d)| d.clone());
            p.drill_base = p
                .asked
                .as_ref()
                .map(|(r, d)| super::super::builder_run::DrillBase::of(title, d, r.clone(), &run));
            p.table = Some(super::super::builder_run::dress(
                def.as_ref(),
                run.table,
                denied,
            ));
            p.shown_ref = p.asked.as_ref().map(|(r, _)| r.clone());
            p.failed = None;
            p.ran_at = Some(chrono::Local::now().format("%-I:%M %p").to_string());
        }
        Err(e) => {
            if p.state.generation() != generation {
                return;
            }
            // The engine's DEFINITION_INVALID: a problem on its row until
            // the next edit (plan §3.1).
            if let Some(cl_api::resources::report_builder::BuilderRefusal::Invalid {
                path: Some(path),
                code: Some(code),
            }) = cl_api::resources::report_builder::builder_refusal(&e)
            {
                p.state
                    .run_failed(generation, super::super::builder_run::failure(&e));
                s.ed.set_engine_problem(&path, &code);
                return;
            }
            let why = super::super::builder_run::failure(&e);
            p.state.run_failed(generation, why);
            p.failed = failed_key(p.state.phase(), p.state.banner());
        }
    }
}

/// The message for a failed run with no table to keep (R6). With a table
/// the run state's banner says it ([`banner_key`]), over the dimmed table.
pub(crate) fn failed_key(phase: Phase, banner: Option<Banner>) -> Option<&'static str> {
    use cl_model::report_def::run_state::Cap;
    if banner.is_some() {
        return None;
    }
    Some(match phase {
        Phase::TimedOut => "reports.builder.took_too_long",
        Phase::Unavailable(Cap::TooLarge) => "reports.builder.too_large",
        Phase::Unavailable(Cap::NeedsEngineUpdate) => "reports.needs_update",
        Phase::Deleted => "reports.builder.deleted",
        Phase::Shown | Phase::Running | Phase::Empty => return None,
        Phase::Failed => "builder.preview.cant_reach",
    })
}

/// Opens or closes a preview line (open paths are session state).
fn toggle(s: &mut BuilderScreen, path: &RowPath, open: bool, ui: &Ui) {
    let now = now_ms(ui);
    let fx = if open {
        s.preview.state.expand(path, now)
    } else {
        s.preview.state.collapse(path, now)
    };
    apply(s, fx);
}

/// The "Problems to fix" callouts, each with "Go to row".
pub(crate) fn problems(s: &mut BuilderScreen, ui: &mut Ui, tokens: &Tokens) {
    let list = s.ed.problems();
    if list.is_empty() {
        return;
    }
    ui.add(egui::Label::new(RichText::new(t("builder.problems.title")).strong()).selectable(false));
    for (i, p) in list.iter().enumerate() {
        ui.horizontal_wrapped(|ui| {
            form::inline_note(ui, tokens, &problem_text(s, p));
            if p.node_id.is_some() || matches!(p.anchor, Anchor::Columns(_)) {
                let r = ui.button(t("builder.problems.go"));
                let r = r.on_hover_text(t("builder.problems.go_hint"));
                if r.clicked() {
                    go_to(s, p);
                }
                let _ = i;
            }
        });
    }
}

/// A problem's sentence, the row's caption filled in.
pub(crate) fn problem_text(s: &BuilderScreen, p: &Problem) -> String {
    let caption = p.node_id.as_deref().map_or_else(
        || t("builder.problem.the_report"),
        |id| super::outline::caption_of(s, id),
    );
    t_with(&p.code.key(), &[("row", &caption)])
}

fn go_to(s: &mut BuilderScreen, p: &Problem) {
    match &p.anchor {
        Anchor::Columns(_) => s.tab = PropTab::Columns,
        Anchor::Report(_) => s.tab = PropTab::Report,
        Anchor::Row(_) => s.tab = PropTab::Row,
    }
    if let Some(id) = &p.node_id {
        s.ed.select(Some(id));
    }
    s.pane = Pane::Properties;
    s.focus_to = Some(Pane::Properties);
}

/// The preview pane.
pub(crate) fn draw(s: &mut BuilderScreen, ui: &mut Ui, tokens: &Tokens) {
    ui.horizontal(|ui| {
        let head = ui.add(
            egui::Label::new(
                RichText::new(t("builder.preview.title"))
                    .size(12.0)
                    .strong()
                    .color(tokens.text_secondary),
            )
            .sense(egui::Sense::focusable_noninteractive()),
        );
        if s.focus_to == Some(Pane::Preview) {
            head.request_focus();
        }
        if head.has_focus() {
            s.pane = Pane::Preview;
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                RichText::new(t("builder.preview.for_all"))
                    .size(12.0)
                    .color(tokens.text_secondary),
            );
            if ui.link(t("builder.toolbar.run_report")).clicked() {
                s.run_report();
            }
            let mut line = match &s.preview.ran_at {
                Some(at) => t_with("builder.preview.updated", &[("time", at)]),
                None => String::new(),
            };
            if s.preview
                .table
                .as_ref()
                .is_some_and(|t| shown(t, s.preview.engine_cut).1)
            {
                line.push_str(&t("builder.preview.first_50"));
            }
            ui.label(RichText::new(line).size(12.0).color(tokens.text_secondary));
        });
    });
    ui.separator();
    problems(s, ui, tokens);
    if let Some(key) = s.preview.failed {
        form::inline_note(ui, tokens, &t(key));
    } else if let Some(b) = s.preview.state.banner() {
        form::inline_note(ui, tokens, &t(banner_key(b)));
    }
    if s.ed.definition().nodes.is_empty() {
        ui.add_space(24.0);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new(t("builder.preview.empty")).color(tokens.text_secondary));
        });
        return;
    }
    let dim = !s.ed.client_problems().is_empty()
        || s.preview.failed.is_some()
        || s.preview
            .state
            .banner()
            .is_some_and(|b| b != Banner::TooManyOpen);
    let Some(table) = s.preview.table.clone() else {
        if s.preview.busy() {
            form::progress_line(ui, tokens, &t("builder.preview.running"));
        }
        return;
    };
    header(s, ui, tokens);
    rows(s, ui, tokens, &table, dim);
    footnote(ui, tokens);
}

/// The note under the table (the mockup's footnote).
fn footnote(ui: &mut Ui, tokens: &Tokens) {
    ui.add(
        egui::Label::new(
            RichText::new(t("builder.preview.footnote"))
                .size(12.0)
                .color(tokens.text_tertiary),
        )
        .wrap(),
    );
}

fn banner_key(b: Banner) -> &'static str {
    match b {
        Banner::BooksChanged => "reports.builder.books_changed",
        Banner::TookTooLong => "reports.builder.took_too_long",
        Banner::TooLarge => "reports.builder.too_large",
        Banner::TooManyOpen => "reports.builder.too_many_open",
        Banner::TryAgain => "builder.preview.cant_reach",
    }
}

fn header(s: &BuilderScreen, ui: &mut Ui, tokens: &Tokens) {
    ui.vertical_centered(|ui| {
        if let Some((_, company)) = &s.open {
            ui.label(RichText::new(company).strong());
        }
        ui.label(RichText::new(s.report_title()).size(17.0).strong());
        let dates = s
            .shown_dates()
            .map(|r| {
                if s.as_of() || r.from == r.to {
                    crate::shell::us_date(r.to)
                } else if r.from.day() == 1 && r.to == r.from.end_of_month() {
                    // A whole month reads as the mockup's "October 2026".
                    format!(
                        "{} {}",
                        t(&format!("home.month_long.{}", r.from.month())),
                        r.from.year()
                    )
                } else {
                    format!(
                        "{} \u{2013} {}",
                        crate::shell::us_date(r.from),
                        crate::shell::us_date(r.to)
                    )
                }
            })
            .unwrap_or_default();
        let basis = match s.ed.definition().basis {
            Some(cl_core::report::Basis::Cash) => t("builder.band.cash_basis"),
            Some(cl_core::report::Basis::Accrual) => t("builder.band.accrual_basis"),
            None => t("builder.band.default_basis"),
        };
        ui.label(
            RichText::new(t_with(
                "builder.preview.subtitle",
                &[("dates", &dates), ("basis", &basis)],
            ))
            .size(12.0)
            .color(tokens.text_secondary),
        );
    });
    ui.add_space(6.0);
}

const CAPTION_W: f32 = 260.0;
const CELL_W: f32 = 96.0;
/// Every preview row is this tall: the rows are virtualized (R4).
const ROW_H: f32 = 20.0;
/// Opened-line detail rows: date · type · number · name · memo in the
/// label column, those columns hidden (Q-A08f).
const COMPOSED: [&str; 5] = ["f:date", "f:txn_type", "f:number", "f:contact", "f:memo"];

/// What a click in the preview asked for.
enum Act {
    Toggle(RowPath, bool),
    Pick(String),
    Drill(Drill),
}

/// The table: headings, then only the rows in view (`show_rows`). While
/// problems or a failure stand, the last good table is drawn in the
/// secondary text color with a line saying so (never by opacity, which
/// would put the text under 4.5:1).
#[allow(
    clippy::many_single_char_names,
    clippy::too_many_lines,
    reason = "one row loop: s, i, r, l as in the run window's"
)]
fn rows(s: &mut BuilderScreen, ui: &mut Ui, tokens: &Tokens, table: &ReportTable, dim: bool) {
    let style = crate::grid_env::shell_frame(ui.ctx()).money_style;
    let composed: Vec<usize> = table
        .columns
        .iter()
        .enumerate()
        .filter(|(_, c)| COMPOSED.contains(&c.key.as_str()))
        .map(|(i, _)| i)
        .collect();
    let shown_cols: Vec<usize> = (0..table.columns.len())
        .filter(|i| !composed.contains(i))
        .collect();
    let ink = if dim {
        tokens.text_secondary
    } else {
        tokens.text_primary
    };
    if dim {
        ui.label(
            RichText::new(t("builder.preview.last_good"))
                .size(12.0)
                .italics()
                .color(tokens.text_secondary),
        );
    }
    if let Some(key) = s.preview.drill_note {
        form::inline_note(ui, tokens, &t(key));
    }
    #[allow(clippy::cast_precision_loss, reason = "a few dozen columns at most")]
    let width = CAPTION_W + CELL_W * shown_cols.len() as f32;
    let rows_of: Vec<(RowPath, bool, bool)> = table
        .rows
        .iter()
        .map(|r| {
            let leaf = matches!(r.kind, RowKind::Detail | RowKind::Earnings | RowKind::More);
            (r.path.clone(), r.expandable, leaf)
        })
        .collect();
    let hidden = s.preview.state.hidden(&rows_of);
    let (order, cut) = shown(table, s.preview.engine_cut);
    let visible: Vec<usize> = order
        .into_iter()
        .filter(|&i| !hidden.get(i).copied().unwrap_or(false))
        .filter(|&i| table.rows[i].kind != RowKind::More)
        .collect();
    let selected = s.ed.selection().map(str::to_string);
    let source = super::super::builder_ref::spec_for(s.ed.definition()).source;
    let mut acts = Vec::new();
    let total = visible.len() + usize::from(cut);
    egui::ScrollArea::both()
        .id_salt(("builder-preview", s.id.0))
        .auto_shrink([false, false])
        // Room for the footnote under the table.
        .max_height((ui.available_height() - 40.0).max(80.0))
        .show_rows(ui, ROW_H, total + 1, |ui, range| {
            ui.set_min_width(width);
            for k in range {
                // Row 0: the headings (they scroll across with the figures).
                if k == 0 {
                    headings(s, ui, table, &shown_cols);
                    continue;
                }
                let Some(&i) = visible.get(k - 1) else {
                    // The info row after the cut.
                    ui.add_sized(
                        [width, ROW_H],
                        egui::Label::new(
                            RichText::new(t("builder.preview.cut"))
                                .italics()
                                .color(tokens.text_secondary),
                        )
                        .truncate(),
                    );
                    continue;
                };
                let r = &table.rows[i];
                let node = node_of(&r.path).map(str::to_string);
                let tinted = node.is_some() && node == selected;
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(width, ROW_H), egui::Sense::hover());
                if tinted {
                    ui.painter().rect_filled(rect, 0.0, tokens.accent_tint);
                }
                let mut row = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(rect)
                        .layout(egui::Layout::left_to_right(egui::Align::Center)),
                );
                let row = &mut row;
                let indent = f32::from(r.level) * 14.0;
                row.add_space(indent);
                if r.expandable {
                    let open = s.preview.state.is_open(&r.path);
                    let mark = if open { "\u{2212}" } else { "+" };
                    let name = t(if open {
                        "builder.preview.close_line"
                    } else {
                        "builder.preview.open_line"
                    });
                    let b = row.small_button(mark);
                    // The +/− is named for screen readers and the control
                    // channel, not only on hover (AC7).
                    b.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &name));
                    if b.on_hover_text(&name).clicked() {
                        acts.push(Act::Toggle(r.path.clone(), !open));
                    }
                } else {
                    row.add_space(18.0);
                }
                let mut caption = super::super::label(&r.caption);
                if r.kind == RowKind::Detail && !composed.is_empty() {
                    let parts: Vec<String> = composed
                        .iter()
                        .filter_map(|&ci| r.cells.get(ci))
                        .map(|c| super::super::window::cell_text(c, style))
                        .filter(|x| !x.is_empty())
                        .collect();
                    caption = parts.join(" \u{b7} ");
                }
                let strong = matches!(
                    r.kind,
                    RowKind::Subtotal
                        | RowKind::Total
                        | RowKind::Grand
                        | RowKind::Formula
                        | RowKind::Header
                );
                let text = RichText::new(caption).color(ink);
                let text = if strong { text.strong() } else { text };
                // Captions start at the left, amounts end at the right
                // (the mockup's statement layout).
                let cw = (CAPTION_W - indent - 18.0).max(40.0);
                let l = row
                    .allocate_ui_with_layout(
                        egui::vec2(cw, ROW_H),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.set_min_width(cw);
                            ui.add(
                                egui::Label::new(text)
                                    .truncate()
                                    .sense(egui::Sense::click()),
                            )
                        },
                    )
                    .inner;
                if l.clicked()
                    && let Some(n) = node.clone()
                {
                    acts.push(Act::Pick(n));
                }
                for &ci in &shown_cols {
                    let cell = r.cells.get(ci);
                    let text = cell
                        .map(|c| match c {
                            Cell::Empty => String::new(),
                            other => super::super::window::cell_text(other, style),
                        })
                        .unwrap_or_default();
                    // Underlined amounts drill as in A07 (plan §3.4).
                    let drill = r
                        .drill
                        .as_ref()
                        .filter(|_| matches!(cell, Some(Cell::Amount(_))) && !text.is_empty())
                        .and_then(|d| d.for_cell(&table.columns[ci]))
                        .filter(|d| super::super::drill::live(&s.link, source, d));
                    let rich = RichText::new(text).monospace().color(ink);
                    let rich = if strong { rich.strong() } else { rich };
                    row.allocate_ui_with_layout(
                        egui::vec2(CELL_W, ROW_H),
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui| {
                            ui.set_min_width(CELL_W);
                            match drill {
                                Some(d) => {
                                    let resp = ui.link(rich).on_hover_text(t("reports.drill.hint"));
                                    if resp.clicked() {
                                        acts.push(Act::Drill(d));
                                    }
                                }
                                None => {
                                    ui.label(rich);
                                }
                            }
                        },
                    );
                }
            }
        });
    for a in acts {
        match a {
            Act::Toggle(path, open) => toggle(s, &path, open, ui),
            Act::Pick(n) => {
                s.ed.select(Some(&n));
                s.tab = PropTab::Row;
            }
            Act::Drill(d) => drill(s, &d),
        }
    }
}

/// The column headings (a renamed header shows its new name).
fn headings(s: &BuilderScreen, ui: &mut Ui, table: &ReportTable, shown_cols: &[usize]) {
    ui.horizontal(|ui| {
        ui.set_height(ROW_H);
        ui.add_sized([CAPTION_W, ROW_H], egui::Label::new(""));
        for &ci in shown_cols {
            let c = &table.columns[ci];
            let text =
                s.ed.definition()
                    .columns
                    .headers
                    .get(&c.key)
                    .cloned()
                    .unwrap_or_else(|| super::super::label(&c.caption));
            right(
                ui,
                CELL_W,
                egui::Label::new(RichText::new(text).size(12.0).strong()).truncate(),
            );
        }
    });
}

/// A drill on the preview: a subtotal's transactions in a detail tab, a
/// transaction in its form (plan §3.4, as A07's run window does).
pub(crate) fn drill(s: &mut BuilderScreen, d: &Drill) {
    s.preview.drill_note = None;
    match d {
        Drill::BuilderDetail {
            node_id,
            path,
            range,
        } => {
            let Some(base) = s.preview.drill_base.as_ref() else {
                return;
            };
            let open = base.detail(node_id, path, *range);
            s.link.push(crate::company::CompanyEvent::OpenWith(
                cl_model::CommandId::new(super::super::KIND),
                Box::new(crate::windows::OpenPayload::BuilderDetail(Box::new(open))),
            ));
        }
        // The preview pages nothing: Run report shows every transaction.
        Drill::BuilderPage { .. } => s.preview.drill_note = Some("builder.preview.cut"),
        other => {
            let source = super::super::builder_ref::spec_for(s.ed.definition()).source;
            match super::super::drill::target(source, other) {
                super::super::drill::Target::Form(form, id) => {
                    s.link.push(crate::company::CompanyEvent::OpenRecord(
                        cl_model::CommandId::new(form),
                        id,
                    ));
                }
                super::super::drill::Target::Probe(id) => {
                    let job = super::super::drill::probe_job(&s.link, s.group.as_ref(), id);
                    s.preview.drill_job = Some((id, job));
                }
                super::super::drill::Target::Report(open) => {
                    s.link.push(crate::company::CompanyEvent::OpenWith(
                        cl_model::CommandId::new(super::super::KIND),
                        Box::new(crate::windows::OpenPayload::Report(open)),
                    ));
                }
                super::super::drill::Target::Nothing(key) => s.preview.drill_note = Some(key),
            }
        }
    }
}

/// A label in a cell `width` wide, against its right edge.
fn right(ui: &mut Ui, width: f32, label: egui::Label) {
    ui.allocate_ui_with_layout(
        egui::vec2(width, ROW_H),
        egui::Layout::right_to_left(egui::Align::Center),
        |ui| {
            ui.set_min_width(width);
            ui.add(label);
        },
    );
}
