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

/// The rows on screen, or `None` when the table on screen isn't the
/// answer to the options on screen (B2).
pub(crate) fn snapshot(w: &ReportWindow) -> Option<Snapshot> {
    if !w.exportable() {
        return None;
    }
    // What the window shows: its rows in order, tag rows in the owner's
    // label style, hidden account totals left out (Q-271, AC5).
    let source = w.table.as_ref()?;
    let table = cl_model::reports::present(source, &w.shown_rows(), &w.options);
    let title = w.title_text();
    let mut head = Vec::new();
    if !w.company.is_empty() {
        head.push(w.company.clone());
    }
    head.push(title.clone());
    head.push(w.subtitle());
    // A file says what the screen says about its figures (S1): the
    // cash-basis rules and a capped ledger.
    if let Some(note) = &source.meta.cash_basis_rules {
        head.push(note.clone());
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

impl Snapshot {
    /// The file (pure; runs on any thread).
    pub(crate) fn render(&self, kind: Kind) -> Option<Built> {
        let table = &self.table;
        let rows: Vec<usize> = (0..table.rows.len()).collect();
        match kind {
            Kind::Csv => Some((
                cl_print::report_export::to_csv(table, &rows, &self.head, &tr).into_bytes(),
                0,
            )),
            Kind::Xlsx => cl_print::report_export::to_xlsx(table, &rows, &self.head, &tr)
                .ok()
                .map(|(b, n)| (b, usize::try_from(n).unwrap_or(usize::MAX))),
            Kind::Pdf => {
                let subtitle = self
                    .head
                    .iter()
                    .filter(|l| **l != self.title)
                    .cloned()
                    .collect();
                let doc =
                    cl_print::report_export::to_report(table, &rows, &self.title, subtitle, &tr);
                cl_print::reports::render_report(&doc).ok().map(|b| (b, 0))
            }
        }
    }

    /// Builds the file on a worker thread.
    pub(crate) fn spawn(self, kind: Kind) -> Receiver<Option<Built>> {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            // The window may have closed; nobody is waiting then.
            let _ = tx.send(self.render(kind));
        });
        rx
    }
}

/// The file's bytes for the rows on screen, built here (tests).
#[cfg(test)]
pub(crate) fn bytes(w: &ReportWindow, kind: Kind) -> Option<Vec<u8>> {
    snapshot(w)?.render(kind).map(|(b, _)| b)
}

/// Builds the file and asks where to save it, then writes.
pub(super) fn start(w: &mut ReportWindow, ctx: &Context, kind: Kind) {
    w.export.message = None;
    if w.export.pending.is_some() {
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
    });
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
        kind, built, path, ..
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
        w.export.message = Some(t("reports.export.failed"));
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
