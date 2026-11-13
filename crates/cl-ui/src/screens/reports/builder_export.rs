//! Export and Print of a Report Builder report (A09 S2, Q-A09h, plan
//! §3.2): a fresh `purpose: 'export'` run with the window's definition,
//! dates, basis and the open/collapsed lists of the table shown. Every
//! `more` row that has a cursor is then filled through `run/detail` pages,
//! spliced in place exactly as Load more does, until the file holds
//! [`MAX_EXPORT_ROWS`] detail rows. A `more` row left over prints as "rows
//! not loaded" and the file's head says it is cut.
//!
//! The run and the pages happen on the request queue, never on the egui
//! thread; Cancel stops between pages. A page that fails fails the whole
//! file: nothing is written (no silent fallback to the screen's rows).
//! Nothing here adds up money: rows are moved, never summed.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use cl_api::ApiError;
use cl_api::resources::report_builder::{
    BuilderRun, DetailPage, DetailRequest, RunPurpose, RunRequest,
};
use cl_core::report::{Drill, ReportTable, RowKind, RowPath};

use super::{ReportWindow, splice};
use crate::screens::invoice::engine::{self, Job};

/// The most detail rows an export carries (Q-311, the engine's
/// `MAX_DETAIL_ROWS_EXPORT`).
pub(crate) const MAX_EXPORT_ROWS: usize = 100_000;

/// An export run with its `more` rows filled.
#[derive(Clone, Debug)]
pub(crate) struct Filled {
    /// The run, its table spliced (not dressed yet).
    pub(crate) run: BuilderRun,
    /// A `more` row is left: the file is cut.
    pub(crate) cut: bool,
}

/// An export fill on its way, and its Cancel.
pub(crate) struct FillJob {
    pub(crate) job: Job<Filled>,
    pub(crate) cancel: Arc<AtomicBool>,
}

impl FillJob {
    /// Stops it between pages; its answer is then dropped.
    pub(crate) fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// The export run for the builder report on screen: its reference and the
/// lists of the table shown (a detail tab and a report with no run have
/// none).
pub(crate) fn export_request(w: &ReportWindow) -> Option<RunRequest> {
    let b = w.builder.as_ref()?;
    if b.detail.is_some() {
        return None;
    }
    let reference = b.shown.as_ref().map(|s| s.reference.clone())?;
    let (open, collapsed, expand_all) = b.state.shown_lists();
    Some(RunRequest {
        reference,
        open,
        collapsed,
        expand_all,
        purpose: RunPurpose::Export,
    })
}

/// The first `more` row that can be paged: its node, path and cursor.
fn next_more(t: &ReportTable) -> Option<(String, RowPath, String)> {
    t.rows.iter().find_map(|r| match (&r.kind, &r.drill) {
        (
            RowKind::More,
            Some(Drill::BuilderPage {
                node_id,
                path,
                cursor: Some(c),
            }),
        ) => Some((node_id.clone(), path.clone(), c.clone())),
        _ => None,
    })
}

fn details(t: &ReportTable) -> usize {
    t.rows.iter().filter(|r| r.kind == RowKind::Detail).count()
}

/// The detail request for a `more` row of `run`.
fn page_request(
    req: &RunRequest,
    run: &BuilderRun,
    node_id: String,
    path: RowPath,
    cursor: String,
) -> DetailRequest {
    DetailRequest {
        reference: req.reference.clone(),
        node_id,
        path,
        range: None,
        cursor: Some(cursor),
        fingerprint: run.fingerprint.clone(),
        stamp: run.stamp,
    }
}

/// A `more` row's cursor is spent once: a page that brought nothing new
/// for it (an engine that answered the same cursor) ends the fill there
/// rather than looping.
fn stuck(before: &ReportTable, after: &ReportTable) -> bool {
    next_more(before) == next_more(after) && details(before) == details(after)
}

/// Fills `run`'s `more` rows with pages from `fetch` up to `max` detail
/// rows (the demo answers at once; tests pass a stub).
///
/// # Errors
/// The first page's refusal: the whole file fails.
pub(crate) fn fill_with(
    req: &RunRequest,
    mut run: BuilderRun,
    max: usize,
    cancel: &AtomicBool,
    mut fetch: impl FnMut(&DetailRequest, &BuilderRun) -> Result<DetailPage, ApiError>,
) -> Result<Filled, ApiError> {
    while details(&run.table) < max {
        if cancel.load(Ordering::Relaxed) {
            return Err(ApiError::Cancelled);
        }
        let Some((node_id, path, cursor)) = next_more(&run.table) else {
            break;
        };
        let dr = page_request(req, &run, node_id, path.clone(), cursor);
        let page = fetch(&dr, &run)?;
        let before = run.table.clone();
        splice(&mut run.table, &path, page.rows, page.cursor);
        if stuck(&before, &run.table) {
            break;
        }
    }
    let cut = run.table.rows.iter().any(|r| r.kind == RowKind::More);
    Ok(Filled { run, cut })
}

/// Starts the export run and its fill.
pub(crate) fn start(w: &ReportWindow) -> Option<FillJob> {
    let req = export_request(w)?;
    let cancel = Arc::new(AtomicBool::new(false));
    let live_req = req.clone();
    let live_cancel = Arc::clone(&cancel);
    let demo_cancel = Arc::clone(&cancel);
    let job = engine::read(
        &w.link,
        w.group.as_ref(),
        move |d| {
            let run = crate::company::demo::report_builder::run(d, &req, false)?;
            fill_with(&req, run, MAX_EXPORT_ROWS, &demo_cancel, |dr, run| {
                crate::company::demo::report_builder::detail(d, dr, &run.table.columns, &run.layout)
            })
        },
        move |s| async move {
            let rb = s.report_builder();
            let mut run = rb.run(&live_req).await?;
            // The same loop as `fill_with`, with the engine's awaits.
            while details(&run.table) < MAX_EXPORT_ROWS {
                if live_cancel.load(Ordering::Relaxed) {
                    return Err(ApiError::Cancelled);
                }
                let Some((node_id, path, cursor)) = next_more(&run.table) else {
                    break;
                };
                let dr = page_request(&live_req, &run, node_id, path.clone(), cursor);
                let page = rb.detail(&dr, &run.table.columns, &run.layout).await?;
                let before = run.table.clone();
                splice(&mut run.table, &path, page.rows, page.cursor);
                if stuck(&before, &run.table) {
                    break;
                }
            }
            let cut = run.table.rows.iter().any(|r| r.kind == RowKind::More);
            Ok(Filled { run, cut })
        },
    );
    Some(FillJob { job, cancel })
}

/// The filled run as the file's table: dressed as the screen dresses its
/// rows, every row in engine order (the engine already applied the open
/// and collapsed lists), and a `more` row left over saying rows are
/// missing.
pub(crate) fn file_table(w: &ReportWindow, f: Filled) -> ReportTable {
    let def = w.builder.as_ref().and_then(|b| b.def.as_ref());
    let denied = !f.run.denied.is_empty();
    let table = super::dress(def, f.run.table, denied);
    let rows: Vec<usize> = (0..table.rows.len()).collect();
    let mut out = cl_model::reports::present(&table, &rows, &w.options);
    for r in &mut out.rows {
        if r.kind == RowKind::More {
            r.caption = "@reports.builder.not_loaded".into();
            r.drill = None;
        }
    }
    out
}

/// Polls a fill: `Some` once answered (a cancelled one answers
/// `Cancelled`, whatever came).
pub(crate) fn poll(f: &mut FillJob) -> Option<Result<Filled, ApiError>> {
    let r = f.job.take()?;
    if f.cancel.load(Ordering::Relaxed) {
        return Some(Err(ApiError::Cancelled));
    }
    Some(r)
}

/// The engine's "took too long" (30 s run limit), said in its own words.
pub(crate) fn is_timeout(e: &ApiError) -> bool {
    matches!(
        super::failure(e),
        cl_model::report_def::run_state::Failure::Timeout
    )
}

#[cfg(test)]
#[path = "builder_export_tests.rs"]
mod tests;
