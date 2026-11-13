//! Filling an export run's `more` rows (A09 S2, Q-A09h, plan §3.2): the
//! one fill every caller drives (the desktop's engine and demo backends,
//! and the golden and contract tests), so what the tests check is what the
//! desktop writes.
//!
//! [`ExportFill`] is the state: it says which page to ask for next
//! ([`ExportFill::next_request`]) and takes each answer
//! ([`ExportFill::take`]); the caller does the asking (async against the
//! engine, at once against the demo). It keeps a running count of detail
//! rows and the position of the `more` row being filled, so a 100,000-row
//! file never copies or rescans the whole table per page. A page that would
//! pass the cap is trimmed: its `more` row stays and the file is cut.
//!
//! Nothing here adds up money: rows are moved, never summed.

use cl_core::report::{Drill, ReportRow, ReportTable, RowKind, RowPath};

use super::{BuilderRun, DetailPage, DetailRequest, RunRef, RunRequest};

/// The most detail rows an export carries (Q-311, the engine's
/// `MAX_DETAIL_ROWS_EXPORT`).
pub const MAX_EXPORT_ROWS: usize = 100_000;

/// Puts a page's rows where the `more` row at `path` was (searched from
/// row `from`), one level under its line, and a new `more` row after them
/// while pages remain. The `more` row's index, when it was found.
pub fn splice_from(
    table: &mut ReportTable,
    from: usize,
    path: &RowPath,
    rows: Vec<ReportRow>,
    cursor: Option<String>,
) -> Option<usize> {
    let at = table
        .rows
        .get(from..)?
        .iter()
        .position(|r| r.kind == RowKind::More && &r.path == path)?
        + from;
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
    Some(at)
}

/// [`splice_from`] the first row (Load more on screen).
pub fn splice(
    table: &mut ReportTable,
    path: &RowPath,
    rows: Vec<ReportRow>,
    cursor: Option<String>,
) {
    let _ = splice_from(table, 0, path, rows, cursor);
}

/// An export run with its `more` rows filled.
#[derive(Clone, Debug)]
pub struct Filled {
    /// The run, its table spliced.
    pub run: BuilderRun,
    /// A `more` row is left (the cap, or a page that brought nothing): the
    /// file is cut.
    pub cut: bool,
}

/// The page being asked for.
#[derive(Clone, Debug)]
struct Asked {
    path: RowPath,
    /// Where its `more` row is.
    at: usize,
    cursor: String,
}

/// Filling one export run.
#[derive(Debug)]
pub struct ExportFill {
    reference: RunRef,
    run: BuilderRun,
    max: usize,
    /// Detail rows in the table (kept as pages come, never recounted).
    details: usize,
    /// No `more` row before this index can be paged.
    from: usize,
    asked: Option<Asked>,
    /// Stopped: the cap, or a page that brought nothing for its cursor.
    stopped: bool,
}

impl ExportFill {
    /// Starts filling `run` (the answer to `req`) up to `max` detail rows.
    #[must_use]
    pub fn new(req: &RunRequest, run: BuilderRun, max: usize) -> ExportFill {
        let details = run
            .table
            .rows
            .iter()
            .filter(|r| r.kind == RowKind::Detail)
            .count();
        ExportFill {
            reference: req.reference.clone(),
            run,
            max,
            details,
            from: 0,
            asked: None,
            stopped: false,
        }
    }

    /// The run so far (its columns and layout read each page).
    #[must_use]
    pub fn run(&self) -> &BuilderRun {
        &self.run
    }

    /// Detail rows so far.
    #[must_use]
    pub fn details(&self) -> usize {
        self.details
    }

    /// The next page to ask for, or `None` when the fill is done.
    pub fn next_request(&mut self) -> Option<DetailRequest> {
        if self.stopped || self.details >= self.max {
            return None;
        }
        let (at, node_id, path, cursor) = self
            .run
            .table
            .rows
            .iter()
            .enumerate()
            .skip(self.from)
            .find_map(|(i, r)| match (&r.kind, &r.drill) {
                (
                    RowKind::More,
                    Some(Drill::BuilderPage {
                        node_id,
                        path,
                        cursor: Some(c),
                    }),
                ) => Some((i, node_id.clone(), path.clone(), c.clone())),
                _ => None,
            })?;
        self.from = at;
        self.asked = Some(Asked {
            path: path.clone(),
            at,
            cursor: cursor.clone(),
        });
        Some(DetailRequest {
            reference: self.reference.clone(),
            node_id,
            path,
            range: None,
            cursor: Some(cursor),
            fingerprint: self.run.fingerprint.clone(),
            stamp: self.run.stamp,
        })
    }

    /// Takes the answer to the last [`ExportFill::next_request`]. A page
    /// that would pass the cap is trimmed to it (its `more` row stays); a
    /// page that brought nothing new for its cursor ends the fill rather
    /// than loop.
    pub fn take(&mut self, page: DetailPage) {
        let Some(asked) = self.asked.take() else {
            return;
        };
        let DetailPage {
            mut rows, cursor, ..
        } = page;
        let left = self.max.saturating_sub(self.details);
        let mut added = rows.iter().filter(|r| r.kind == RowKind::Detail).count();
        let mut cursor = cursor;
        if added > left {
            // Keep rows up to the `left`-th detail row; the rest stays behind
            // the `more` row (its cursor unchanged), which prints as "rows
            // not loaded".
            let mut seen = 0;
            let keep = rows
                .iter()
                .position(|r| {
                    if r.kind == RowKind::Detail {
                        seen += 1;
                    }
                    seen > left
                })
                .unwrap_or(rows.len());
            rows.truncate(keep);
            added = left;
            cursor = Some(asked.cursor.clone());
            self.stopped = true;
        } else if added == 0 && cursor.as_deref() == Some(asked.cursor.as_str()) {
            self.stopped = true;
        }
        if splice_from(&mut self.run.table, asked.at, &asked.path, rows, cursor).is_some() {
            self.details += added;
        } else {
            self.stopped = true;
        }
    }

    /// The filled run.
    #[must_use]
    pub fn finish(self) -> Filled {
        let cut = self.run.table.rows.iter().any(|r| r.kind == RowKind::More);
        Filled { run: self.run, cut }
    }
}

#[cfg(test)]
#[path = "report_builder_fill_tests.rs"]
mod tests;
