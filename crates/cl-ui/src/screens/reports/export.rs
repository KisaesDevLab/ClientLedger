//! Print, Export Excel, Export CSV and Save PDF (plan WP1 step 6). Every
//! output reads the rows on screen (after collapse and sort) through
//! `cl_print::report_export`; files go through a temp file and a rename, so
//! a failed export never leaves a partial file. Print opens the PDF in the
//! document tab, which prints through `cl-host::os_print`.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};

use cl_model::CommandId;
use eframe::egui::Context;

use super::window::ReportWindow;
use crate::company::CompanyEvent;
use crate::i18n::{t, t_with};

/// Which file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Csv,
    Xlsx,
    Pdf,
}

impl Kind {
    const fn ext(self) -> &'static str {
        match self {
            Kind::Csv => "csv",
            Kind::Xlsx => "xlsx",
            Kind::Pdf => "pdf",
        }
    }

    const fn filter(self) -> &'static str {
        match self {
            Kind::Csv => "picker.filter.csv",
            Kind::Xlsx => "picker.filter.xlsx",
            Kind::Pdf => "picker.filter.pdf",
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Kind::Csv => "csv",
            Kind::Xlsx => "xlsx",
            Kind::Pdf => "pdf",
        }
    }
}

/// An export: its file being built (off the UI thread, S5) and its Save
/// dialog; written when both are in.
struct Pending {
    kind: Kind,
    build: Option<Receiver<Option<Built>>>,
    built: Slot<Built>,
    dialog: Receiver<Option<PathBuf>>,
    path: Slot<PathBuf>,
    /// The report it was made from ([`ReportWindow::view`]): once another
    /// report is shown, "Saved ..." isn't said over it (A07 QA-7).
    view: u64,
}

/// A finished file and how many values it wrote as text (xlsx).
pub(crate) type Built = (Vec<u8>, usize);

/// An export in progress and its last outcome.
#[derive(Default)]
pub(crate) struct Exporter {
    pending: Option<Pending>,
    /// Print: the PDF being built.
    printing: Option<(String, Receiver<Option<Built>>)>,
    message: Option<String>,
    last: Option<(Kind, bool)>,
}

impl std::fmt::Debug for Exporter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Exporter")
            .field("busy", &self.busy())
            .finish_non_exhaustive()
    }
}

impl Exporter {
    pub(crate) fn busy(&self) -> bool {
        self.pending.is_some() || self.printing.is_some()
    }

    pub(crate) fn message(&self) -> Option<String> {
        self.message.clone()
    }

    pub(crate) fn clear_message(&mut self) {
        self.message = None;
    }

    pub(crate) fn state(&self) -> serde_json::Value {
        serde_json::json!({
            "busy": self.busy(),
            "last": self.last.map(|(k, _)| k.name()),
            "ok": self.last.map(|(_, ok)| ok),
        })
    }
}

/// An `@` caption's text: `key` or `key name` (a total of a named row).
fn tr(key: &str) -> String {
    super::label(&format!("@{key}"))
}

/// What a file is made from: the rows on screen and the header lines,
/// copied so the file is built on a worker thread (S5).
pub(crate) struct Snapshot {
    table: cl_core::report::ReportTable,
    title: String,
    /// Company, title, dates and basis, then any note.
    head: Vec<String>,
}

#[cfg(test)]
impl Snapshot {
    /// The rows a file is made from (tests).
    pub(crate) fn table(&self) -> &cl_core::report::ReportTable {
        &self.table
    }
}

/// The rows on screen, or `None` when the table on screen isn't the
/// answer to the options on screen (B2).
pub(crate) fn snapshot(w: &ReportWindow) -> Option<Snapshot> {
    if !w.exportable() {
        return None;
    }
    // What the window shows: its rows in order, tag rows in the owner's
    // label style, hidden account totals left out (Q-271, AC5).
    let source = w.table.as_ref()?;
    let mut table = cl_model::reports::present(source, &w.shown_rows(), &w.options);
    // A builder `more` row is a button on screen; on paper it says rows
    // are missing (#10, plan §4 N8).
    if w.builder.is_some() {
        for r in &mut table.rows {
            if r.kind == cl_core::report::RowKind::More {
                r.caption = "@reports.builder.not_loaded".into();
                r.drill = None;
            }
        }
    }
    let title = w.title_text();
    let mut head = Vec::new();
    if !w.company.is_empty() {
        head.push(w.company.clone());
    }
    head.push(title.clone());
    head.push(w.subtitle());
    // A file says what the screen says about its figures (S1): the
    // report's note and a capped ledger (no general cash-basis sentence,
    // Q-P28ai).
    if let Some(note) = &source.meta.note {
        head.push(super::label(note));
    }
    if let Some(n) = source.meta.truncated {
        head.push(t_with(
            "reports.truncated",
            &[
                ("shown", &crate::i18n::count_text(n.shown)),
                ("total", &crate::i18n::count_text(n.total)),
            ],
        ));
    }
    Some(Snapshot { table, title, head })
}

/// True when a shown amount has more than two decimal places (imports,
/// 4-place journal entries): rows print rounded to cents while totals are
/// the engine's exact sums, so a printed column can miss its total by a
/// cent (review N3).
fn has_sub_cents(t: &cl_core::report::ReportTable) -> bool {
    t.rows
        .iter()
        .flat_map(|r| &r.cells)
        .any(|c| matches!(c, cl_core::report::Cell::Amount(d) if d.normalize().scale() > 2))
}

impl Snapshot {
    /// The file (pure; runs on any thread), or why it couldn't be made
    /// (a short name for the log: no figures, names or text).
    pub(crate) fn render(&self, kind: Kind) -> Result<Built, &'static str> {
        let table = &self.table;
        let rows: Vec<usize> = (0..table.rows.len()).collect();
        match kind {
            Kind::Csv => Ok((
                cl_print::report_export::to_csv(table, &rows, &self.head, &tr).into_bytes(),
                0,
            )),
            Kind::Xlsx => cl_print::report_export::to_xlsx(table, &rows, &self.head, &tr)
                .map(|(b, n)| (b, usize::try_from(n).unwrap_or(usize::MAX)))
                .map_err(|_| "xlsx"),
            Kind::Pdf => {
                let mut subtitle: Vec<String> = self
                    .head
                    .iter()
                    .filter(|l| **l != self.title)
                    .cloned()
                    .collect();
                if has_sub_cents(table) {
                    subtitle.push(tr("reports.export.sub_cents"));
                }
                let doc =
                    cl_print::report_export::to_report(table, &rows, &self.title, subtitle, &tr);
                cl_print::reports::render_report(&doc)
                    .map(|b| (b, 0))
                    .map_err(|e| match e {
                        cl_print::PrintError::DoesNotFit(_) => "pdf: a figure does not fit",
                        cl_print::PrintError::Font(_) => "pdf: font",
                        cl_print::PrintError::Compress(_) => "pdf: compress",
                        cl_print::PrintError::NoPages => "pdf: no pages",
                        _ => "pdf: other",
                    })
            }
        }
    }

    /// Builds the file on a worker thread.
    pub(crate) fn spawn(self, kind: Kind) -> Receiver<Option<Built>> {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let built = self.render(kind).map_err(|why| {
                // The real cause, in the log (A07 QA-8: a build failure was
                // reported as a folder problem with nothing logged).
                tracing::warn!(file = kind.name(), why, "a report file could not be built");
            });
            // The window may have closed; nobody is waiting then.
            let _ = tx.send(built.ok());
        });
        rx
    }
}

/// The file's bytes for the rows on screen, built here (tests).
#[cfg(test)]
pub(crate) fn bytes(w: &ReportWindow, kind: Kind) -> Option<Vec<u8>> {
    snapshot(w)?.render(kind).ok().map(|(b, _)| b)
}

/// A message as if a file were just saved (tests).
#[cfg(test)]
pub(crate) fn set_message_for_test(w: &mut ReportWindow, m: &str) {
    w.export.message = Some(m.to_string());
}

/// An export of `bytes` to `path` whose build and Save dialog are both in,
/// made from the report on screen now (tests).
#[cfg(test)]
pub(crate) fn pending_for_test(w: &mut ReportWindow, bytes: Vec<u8>, path: PathBuf) {
    let (btx, build) = std::sync::mpsc::channel();
    let (dtx, dialog) = std::sync::mpsc::channel();
    let _ = btx.send(Some((bytes, 0)));
    let _ = dtx.send(Some(path));
    w.export.pending = Some(Pending {
        kind: Kind::Csv,
        build: Some(build),
        built: Slot::Waiting,
        dialog,
        path: Slot::Waiting,
        view: w.view,
    });
}

/// Builds the file and asks where to save it, then writes.
pub(super) fn start(w: &mut ReportWindow, ctx: &Context, kind: Kind) {
    w.export.message = None;
    if w.export.pending.is_some() {
        return;
    }
    // A run is on its way (a dates or basis change a moment ago): the
    // export waits for it and starts with its rows, never failing or
    // saving nothing on the first click (A08 QA export).
    if waits_for_run(w) {
        w.after_run = Some(super::builder_ref::AfterRun::Export(kind));
        return;
    }
    let Some(snap) = snapshot(w) else {
        w.export.message = Some(t("reports.export.failed"));
        w.export.last = Some((kind, false));
        return;
    };
    let name = format!(
        "{}.{}",
        cl_print::files::safe_component(&w.title_text()),
        kind.ext()
    );
    let dialog = crate::widgets::file_picker::save_file_as(
        ctx,
        crate::widgets::file_picker::PickUse::SaveReport,
        None,
        &name,
        (kind.filter(), kind.ext()),
    );
    w.export.pending = Some(Pending {
        kind,
        build: Some(snap.spawn(kind)),
        built: Slot::Waiting,
        dialog,
        path: Slot::Waiting,
        view: w.view,
    });
}

/// True when the report's next table is on its way (asked, or options
/// changed since the one on screen): an export or print then waits for it.
fn waits_for_run(w: &ReportWindow) -> bool {
    use super::window::RunState;
    matches!(
        w.state,
        RunState::Running | RunState::Stale | RunState::Empty
    )
}

/// An answer from another thread.
enum Slot<T> {
    /// Not in yet.
    Waiting,
    /// In: a value, or nothing (cancelled, failed, the sender went away).
    Done(Option<T>),
}

impl<T> Slot<T> {
    const fn waiting(&self) -> bool {
        matches!(self, Slot::Waiting)
    }
}

/// A finished receive, once; `Done(None)` when the sender went away.
fn take<T>(rx: &Receiver<Option<T>>) -> Slot<T> {
    match rx.try_recv() {
        Ok(v) => Slot::Done(v),
        Err(TryRecvError::Empty) => Slot::Waiting,
        Err(TryRecvError::Disconnected) => Slot::Done(None),
    }
}

/// Writes the file once it is built and the dialog answered; opens a
/// printed PDF once built.
pub(super) fn poll(w: &mut ReportWindow) {
    poll_print(w);
    let Some(p) = w.export.pending.as_mut() else {
        return;
    };
    if p.built.waiting()
        && let Some(rx) = &p.build
    {
        p.built = take(rx);
        if !p.built.waiting() {
            p.build = None;
        }
    }
    if p.path.waiting()
        && let Ok(chosen) = p.dialog.try_recv()
    {
        p.path = Slot::Done(chosen);
    }
    // Cancelled: nothing to write, whatever the build.
    if matches!(p.path, Slot::Done(None)) {
        w.export.pending = None;
        return;
    }
    if p.built.waiting() || p.path.waiting() {
        return;
    }
    let Some(Pending {
        kind,
        built,
        path,
        view,
        ..
    }) = w.export.pending.take()
    else {
        return;
    };
    let (Slot::Done(built), Slot::Done(Some(path))) = (built, path) else {
        return;
    };
    let file = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ok = built
        .as_ref()
        .is_some_and(|(b, _)| cl_print::report_export::write_atomic(&path, b).is_ok());
    // A file that couldn't be made says so; only a write is about the
    // folder (A07 QA-8).
    if built.is_none() {
        w.export.message = Some(t("reports.export.build_failed"));
        w.export.last = Some((kind, false));
        return;
    }
    // Another report is on screen now: the file is still written, and
    // only a failure is said (A07 QA-7).
    if view != w.view && ok {
        w.export.last = Some((kind, ok));
        return;
    }
    w.export.message = Some(match built {
        Some((_, text)) if ok && text > 0 => t_with(
            "reports.export.saved_with_text",
            &[("file", &file), ("n", &text.to_string())],
        ),
        _ if ok => t_with("reports.export.saved", &[("file", &file)]),
        _ => t("reports.export.failed"),
    });
    w.export.last = Some((kind, ok));
}

/// Print (Ctrl+P): the PDF in the document tab, which prints it.
pub(super) fn print(w: &mut ReportWindow) {
    if w.export.printing.is_some() {
        return;
    }
    if waits_for_run(w) {
        w.after_run = Some(super::builder_ref::AfterRun::Print);
        return;
    }
    let Some(snap) = snapshot(w) else {
        return;
    };
    w.export.printing = Some((w.title_text(), snap.spawn(Kind::Pdf)));
    poll_print(w);
}

fn poll_print(w: &mut ReportWindow) {
    let Some((_, rx)) = w.export.printing.as_ref() else {
        return;
    };
    let Slot::Done(built) = take(rx) else {
        return;
    };
    let Some((title, _)) = w.export.printing.take() else {
        return;
    };
    let Some((b, _)) = built else {
        w.export.message = Some(t("reports.export.build_failed"));
        return;
    };
    w.link.push(CompanyEvent::OpenWith(
        CommandId::new(crate::screens::document_view::KIND),
        Box::new(crate::windows::OpenPayload::Document(
            crate::widgets::pdf_viewer::DocSource {
                name: format!("{title}.pdf"),
                bytes: Arc::from(b),
            },
        )),
    ));
}

#[cfg(test)]
mod sub_cent_tests {
    use cl_core::report::{Cell, ReportRow, ReportTable, RowKind, RowPath};

    fn table(amount: &str) -> ReportTable {
        ReportTable {
            rows: vec![ReportRow {
                kind: RowKind::Account,
                level: 1,
                path: RowPath::of(["a"]),
                caption: "a".into(),
                cells: vec![Cell::Amount(amount.parse().expect("d"))],
                drill: None,
                expandable: false,
            }],
            ..ReportTable::default()
        }
    }

    /// N3: a PDF footnote when a shown amount has sub-cents.
    #[test]
    fn sub_cent_amounts_are_found() {
        assert!(super::has_sub_cents(&table("10.0050")));
        assert!(!super::has_sub_cents(&table("10.5000")));
        assert!(crate::i18n::has("reports.export.sub_cents"));
    }
}
