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
use cl_api::resources::report_builder::fill::ExportFill;
pub(crate) use cl_api::resources::report_builder::fill::{Filled, MAX_EXPORT_ROWS};
use cl_api::resources::report_builder::{
    BuilderRun, DetailPage, DetailRequest, RunPurpose, RunRequest,
};
use cl_core::report::{ReportTable, RowKind};
use cl_model::report_def::BuilderDefinition;
use cl_model::reports::ReportOptions;

use super::ReportWindow;
use crate::screens::invoice::engine::{self, Job};

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

/// Fills `run`'s `more` rows with pages from `fetch` up to `max` detail
/// rows, through the one fill ([`ExportFill`]) the engine path awaits too
/// (the demo answers at once; tests pass a stub).
///
/// # Errors
/// The first page's refusal: the whole file fails; `Cancelled`.
pub(crate) fn fill_with(
    req: &RunRequest,
    run: BuilderRun,
    max: usize,
    cancel: &AtomicBool,
    mut fetch: impl FnMut(&DetailRequest, &BuilderRun) -> Result<DetailPage, ApiError>,
) -> Result<Filled, ApiError> {
    let mut fill = ExportFill::new(req, run, max);
    while let Some(dr) = fill.next_request() {
        if cancel.load(Ordering::Relaxed) {
            return Err(ApiError::Cancelled);
        }
        let page = fetch(&dr, fill.run())?;
        fill.take(page);
    }
    Ok(fill.finish())
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
            let run = rb.run(&live_req).await?;
            // The same fill as `fill_with`, with the engine's awaits.
            let mut fill = ExportFill::new(&live_req, run, MAX_EXPORT_ROWS);
            while let Some(dr) = fill.next_request() {
                if live_cancel.load(Ordering::Relaxed) {
                    return Err(ApiError::Cancelled);
                }
                let r = fill.run();
                let page = rb.detail(&dr, &r.table.columns, &r.layout).await?;
                fill.take(page);
            }
            Ok(fill.finish())
        },
    );
    Some(FillJob { job, cancel })
}

/// What turns a filled run into the file's table, copied from the window
/// so it runs on the file's worker thread (A09 #3).
#[derive(Clone)]
pub(crate) struct FileCtx {
    def: Option<BuilderDefinition>,
    options: ReportOptions,
}

impl FileCtx {
    /// The window's definition and options.
    pub(crate) fn of(w: &ReportWindow) -> FileCtx {
        FileCtx {
            def: w.builder.as_ref().and_then(|b| b.def.clone()),
            options: w.options.clone(),
        }
    }
}
/// The filled run as the file's table: dressed as the screen dresses its
/// rows, every row in engine order (the engine already applied the open
/// and collapsed lists), and a `more` row left over saying rows are
/// missing.
pub(crate) fn file_table(ctx: &FileCtx, f: Filled) -> ReportTable {
    let denied = !f.run.denied.is_empty();
    let table = super::dress(ctx.def.as_ref(), f.run.table, denied);
    let rows: Vec<usize> = (0..table.rows.len()).collect();
    let mut out = cl_model::reports::present(&table, &rows, &ctx.options);
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
