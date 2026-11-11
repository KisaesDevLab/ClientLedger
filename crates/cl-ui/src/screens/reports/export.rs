//! Print, Export Excel, Export CSV and Save PDF (plan WP1 step 6). Every
//! output reads the rows on screen (after collapse and sort) through
//! `cl_print::report_export`; files go through a temp file and a rename, so
//! a failed export never leaves a partial file. Print opens the PDF in the
//! document tab, which prints through `cl-host::os_print`.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::Receiver;

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

/// An export waiting for its Save dialog.
#[derive(Default)]
pub(crate) struct Exporter {
    pending: Option<(Kind, Vec<u8>, Receiver<Option<PathBuf>>)>,
    message: Option<String>,
    last: Option<(Kind, bool)>,
}

impl std::fmt::Debug for Exporter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Exporter")
            .field("busy", &self.pending.is_some())
            .finish_non_exhaustive()
    }
}

impl Exporter {
    pub(crate) fn busy(&self) -> bool {
        self.pending.is_some()
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

/// The file's bytes for the rows on screen.
pub(crate) fn bytes(w: &ReportWindow, kind: Kind) -> Option<Vec<u8>> {
    // Never an old table under new labels (B2).
    if !w.exportable() {
        return None;
    }
    // What the window shows: its rows in order, tag rows in the owner's
    // label style, hidden account totals left out (Q-271, AC5).
    let table = &cl_model::reports::present(w.table.as_ref()?, &w.shown_rows(), &w.options);
    let rows: Vec<usize> = (0..table.rows.len()).collect();
    let title = w.title_text();
    let mut head = Vec::new();
    if !w.company.is_empty() {
        head.push(w.company.clone());
    }
    head.push(title.clone());
    head.push(w.subtitle());
    match kind {
        Kind::Csv => Some(cl_print::report_export::to_csv(table, &rows, &tr).into_bytes()),
        Kind::Xlsx => cl_print::report_export::to_xlsx(table, &rows, &head, &tr)
            .ok()
            .map(|(b, _)| b),
        Kind::Pdf => {
            let subtitle = head.into_iter().filter(|l| *l != title).collect();
            let doc = cl_print::report_export::to_report(table, &rows, &title, subtitle, &tr);
            cl_print::reports::render_report(&doc).ok()
        }
    }
}

/// Asks where to save, then writes.
pub(super) fn start(w: &mut ReportWindow, ctx: &Context, kind: Kind) {
    w.export.message = None;
    let Some(b) = bytes(w, kind) else {
        w.export.message = Some(t("reports.export.failed"));
        w.export.last = Some((kind, false));
        return;
    };
    let name = format!(
        "{}.{}",
        cl_print::files::safe_component(&w.title_text()),
        kind.ext()
    );
    let rx = crate::widgets::file_picker::save_file_as(
        ctx,
        crate::widgets::file_picker::PickUse::SaveReport,
        None,
        &name,
        (kind.filter(), kind.ext()),
    );
    w.export.pending = Some((kind, b, rx));
}

/// Writes the file once the dialog answered.
pub(super) fn poll(w: &mut ReportWindow) {
    let Some((kind, _, rx)) = w.export.pending.as_ref() else {
        return;
    };
    let kind = *kind;
    let Ok(chosen) = rx.try_recv() else {
        return;
    };
    let Some((_, b, _)) = w.export.pending.take() else {
        return;
    };
    let Some(path) = chosen else {
        return;
    };
    let ok = cl_print::report_export::write_atomic(&path, &b).is_ok();
    let file = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    w.export.message = Some(if ok {
        t_with("reports.export.saved", &[("file", &file)])
    } else {
        t("reports.export.failed")
    });
    w.export.last = Some((kind, ok));
}

/// Print (Ctrl+P): the PDF in the document tab, which prints it.
pub(super) fn print(w: &mut ReportWindow) {
    let Some(b) = bytes(w, Kind::Pdf) else {
        return;
    };
    w.link.push(CompanyEvent::OpenWith(
        CommandId::new(crate::screens::document_view::KIND),
        Box::new(crate::windows::OpenPayload::Document(
            crate::widgets::pdf_viewer::DocSource {
                name: format!("{}.pdf", w.title_text()),
                bytes: Arc::from(b),
            },
        )),
    ));
}
