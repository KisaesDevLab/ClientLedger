//! Paste from a spreadsheet and Import CSV file (plan WP-D2, Q-P15Af).
//!
//! Both open the mapping panel: a preview of the first 5 rows, a field per
//! column, "First row is headers" and "Remember for this company". The
//! rows themselves are read by the engine's `parse-csv` (paste is sent as
//! a file too); the app reads only the header and the preview locally.
//! A headerless paste gets a synthetic `Column 1…n` header and maps left
//! to right from the cursor column. The engine's text cells come back as
//! rows through `rows_from_parsed`: amounts re-parsed with the money rules
//! (rounded to the cent, Q-232), dates as typed dates.
//!
//! The rows are appended to the section, or replace rows from the cursor
//! row when pasted onto rows; Ctrl+Z undoes the whole paste. A batch holds
//! at most 2,000 rows; a file at most 10 MB. Tags are never imported.

use std::sync::Arc;

use cl_api::{ApiError, PARSE_CSV_MAX_BYTES};
use cl_grid::{Lookups, RowKey};
use cl_model::forms::batch::{
    Field, ImportedRow, MAX_ROWS, Mapping, SectionKind, default_mapping, looks_like_header,
    mappable_fields, positional_mapping, rows_from_parsed,
};
use eframe::egui::{self, RichText, Ui};

use super::grid::{ensure_entry, header_key};
use super::{BulkEntryScreen, wire};
use crate::i18n::{t, t_with};
use crate::prefs::BulkEntryMappings;
use crate::screens::register::lists::{ACCOUNTS, NAMES};
use crate::theme::Tokens;

/// Rows shown in the preview.
pub(crate) const PREVIEW_ROWS: usize = 5;

/// Where the rows came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Origin {
    /// A paste onto the grid at this row and grid column.
    Paste {
        /// The cursor's row.
        at: Option<RowKey>,
        /// The cursor's column (left to right among the section's).
        column: usize,
    },
    /// A CSV file.
    File,
}

/// The mapping panel.
pub(crate) struct MappingPanel {
    pub origin: Origin,
    /// The section the rows go to.
    pub section: usize,
    pub kind: SectionKind,
    /// The text as pasted or read (decoded).
    pub text: String,
    /// The separator.
    pub separator: char,
    /// The first record's cells.
    pub first: Vec<String>,
    /// The next records' cells (preview).
    pub preview: Vec<Vec<String>>,
    /// Columns in the widest record.
    pub width: usize,
    pub first_row_headers: bool,
    pub mapping: Mapping,
    pub remember: bool,
    /// The mapping came from memory.
    pub recalled: bool,
    pub error: Option<String>,
}

/// The panel's words for "not imported".
const SKIP: &str = "batch_entry.mapping.skip";

impl MappingPanel {
    /// The headers the mapping is keyed by: the first row, or `Column n`.
    #[must_use]
    pub(crate) fn headers(&self) -> Vec<String> {
        if self.first_row_headers {
            self.first.clone()
        } else {
            (1..=self.width)
                .map(|n| t_with("batch_entry.mapping.column", &[("n", &n.to_string())]))
                .collect()
        }
    }

    /// The preview rows (the first row too when it is data).
    #[must_use]
    pub(crate) fn data_preview(&self) -> Vec<Vec<String>> {
        let mut rows = Vec::new();
        if !self.first_row_headers {
            rows.push(self.first.clone());
        }
        rows.extend(self.preview.iter().cloned());
        rows.truncate(PREVIEW_ROWS);
        rows
    }

    /// The mapping for the current header choice: remembered, else from
    /// the header names, else left to right from the cursor column.
    pub(crate) fn remap(&mut self, memory: &cl_model::forms::batch::MappingMemory) {
        let headers = self.headers();
        let refs: Vec<&str> = headers.iter().map(String::as_str).collect();
        self.recalled = false;
        if self.first_row_headers {
            if let Some(m) = memory.recall(self.kind, &refs).filter(|m| !m.is_empty()) {
                self.mapping = m;
                self.recalled = true;
            } else {
                self.mapping = default_mapping(self.kind, &refs);
            }
        } else {
            let start = match self.origin {
                Origin::Paste { column, .. } => column,
                Origin::File => 0,
            };
            self.mapping = positional_mapping(self.kind, start, self.width);
        }
    }

    /// The bytes `parse-csv` reads: the text, under a synthetic header
    /// when the first row is data.
    #[must_use]
    pub(crate) fn bytes(&self) -> Vec<u8> {
        if self.first_row_headers {
            return self.text.clone().into_bytes();
        }
        let sep = self.separator.to_string();
        let header = self.headers().join(&sep);
        format!("{header}\n{}", self.text).into_bytes()
    }

    /// The field mapped to column `col`.
    #[must_use]
    pub(crate) fn field_at(&self, col: usize) -> Option<Field> {
        self.mapping
            .iter()
            .find(|(_, c)| usize::from(**c) == col)
            .map(|(f, _)| *f)
    }

    /// Maps column `col` to `field` (or to nothing); a field takes one
    /// column.
    pub(crate) fn set(&mut self, col: usize, field: Option<Field>) {
        self.mapping.retain(|_, c| usize::from(*c) != col);
        if let (Some(f), Ok(c)) = (field, u16::try_from(col)) {
            self.mapping.insert(f, c);
        }
        self.recalled = false;
    }
}

/// Splits pasted or read text into records for the preview.
fn records(text: &str) -> Result<(char, Vec<Vec<String>>), String> {
    let sep = if text.lines().next().is_some_and(|l| l.contains('\t')) {
        '\t'
    } else {
        cl_import::csv::sniff_separator(text)
    };
    let rows = cl_import::csv::parse_with(text, sep).map_err(|e| match e {
        cl_import::csv::CsvError::Empty => t("csv.read.empty"),
        cl_import::csv::CsvError::UnclosedQuote { line } => {
            t_with("csv.read.quote", &[("line", &line.to_string())])
        }
        _ => t("csv.read.too_big"),
    })?;
    Ok((sep, rows.into_iter().map(|r| r.fields).collect()))
}

impl BulkEntryScreen {
    /// The company the remembered mappings belong to.
    fn company_key(&self) -> String {
        self.tenant
            .as_ref()
            .map_or_else(|| "demo".to_string(), |(t, _)| t.to_string())
    }

    fn settings_dir(&self) -> Option<std::path::PathBuf> {
        self.link
            .store()
            .path()
            .parent()
            .map(std::path::Path::to_path_buf)
    }

    /// This company's remembered mappings.
    pub(crate) fn memory(&self) -> cl_model::forms::batch::MappingMemory {
        self.settings_dir()
            .map(|d| BulkEntryMappings::load(&d).get(&self.company_key()))
            .unwrap_or_default()
    }

    /// Opens the mapping panel for `text` (a paste at `origin`, or a file).
    pub(crate) fn open_mapping(&mut self, text: &str, origin: Origin) {
        if self.busy() || !self.caps().exact {
            self.notice = Some("batch_entry.paste.unavailable");
            return;
        }
        let text = text.strip_prefix('\u{FEFF}').unwrap_or(text).to_string();
        let (separator, recs) = match records(&text) {
            Ok(r) => r,
            Err(e) => {
                self.import_error = Some(e);
                return;
            }
        };
        let Some((first, rest)) = recs.split_first() else {
            self.import_error = Some(t("csv.read.empty"));
            return;
        };
        let width = recs.iter().map(Vec::len).max().unwrap_or(0);
        let cells: Vec<&str> = first.iter().map(String::as_str).collect();
        let first_row_headers = match origin {
            Origin::File => true,
            Origin::Paste { .. } => looks_like_header(&cells),
        };
        let section = self.selected();
        let kind = self.batch().sections[section].kind;
        let mut panel = MappingPanel {
            origin,
            section,
            kind,
            text,
            separator,
            first: first.clone(),
            preview: rest.iter().take(PREVIEW_ROWS).cloned().collect(),
            width,
            first_row_headers,
            mapping: Mapping::new(),
            remember: false,
            recalled: false,
            error: None,
        };
        panel.remap(&self.memory());
        self.import_error = None;
        self.mapping = Some(panel);
        self.last_action = Some("mapping");
    }

    /// The grid's block paste.
    pub(crate) fn block_paste(&mut self, at: cl_grid::CellPos, text: &str) {
        let kind = self.shown_kind;
        let column = super::grid::field_of(at.col)
            .and_then(|f| kind.columns().iter().position(|c| *c == f))
            .unwrap_or(0);
        self.open_mapping(
            text,
            Origin::Paste {
                at: Some(at.row),
                column,
            },
        );
    }

    /// Import CSV file: reads the named file (10 MB at most).
    pub(crate) fn read_file(&mut self) {
        let path = std::path::PathBuf::from(self.import_path.trim().trim_matches('"'));
        let meta = match std::fs::metadata(&path) {
            Ok(m) if m.is_file() => m,
            _ => {
                self.import_error = Some(t("csv.read.missing"));
                return;
            }
        };
        if meta.len() > PARSE_CSV_MAX_BYTES as u64 {
            self.import_error = Some(t("batch_entry.import.too_big"));
            return;
        }
        match std::fs::read(&path) {
            Ok(bytes) => {
                let text = cl_import::csv::decode(&bytes);
                self.open_mapping(&text, Origin::File);
                if self.mapping.is_some() {
                    self.import_open = false;
                }
            }
            Err(_) => self.import_error = Some(t("csv.read.failed")),
        }
    }

    /// Add rows: the engine reads the file with the mapping, and the rows
    /// go into the section as one undo step.
    pub(crate) fn apply_mapping(&mut self) {
        let Some(panel) = self.mapping.as_ref() else {
            return;
        };
        if panel.mapping.is_empty() {
            if let Some(p) = self.mapping.as_mut() {
                p.error = Some(t("batch_entry.mapping.none"));
            }
            return;
        }
        let bytes = panel.bytes();
        let kind = panel.kind;
        let mapping = wire::column_mapping(&panel.mapping);
        let link = Arc::clone(&self.link);
        let parsed = if let Some(local) = self.local.as_mut() {
            local.parse_csv(&link, wire::batch_kind(kind), &bytes, &mapping)
        } else {
            // The live engine answers on the queue.
            if let Some(task) = super::engine::live::parse_csv(
                &self.link,
                self.group.as_ref(),
                wire::batch_kind(kind),
                bytes,
                mapping,
            ) {
                self.parse_task = Some(task);
            }
            return;
        };
        self.parsed(parsed);
    }

    pub(crate) fn poll_parse(&mut self) {
        if let Some(r) = self
            .parse_task
            .as_mut()
            .and_then(crate::company::Task::try_take)
        {
            self.parse_task = None;
            self.parsed(r);
        }
    }

    fn parsed(&mut self, r: Result<cl_api::ParsedRows, ApiError>) {
        let Some(panel) = self.mapping.take() else {
            return;
        };
        // The rows were read for the section's type then: if the section
        // is gone or has another type now, they would land in the wrong
        // columns.
        let same = self
            .batch()
            .sections
            .get(panel.section)
            .is_some_and(|s| s.kind == panel.kind);
        if !same {
            self.notice = Some("batch_entry.import.kind_changed");
            self.last_action = Some("import_discarded");
            return;
        }
        let rows = match r {
            Ok(p) => p,
            Err(e) => {
                let mut panel = panel;
                panel.error = Some(t(cl_core::OwnerFacing::owner_message(&e).key));
                self.mapping = Some(panel);
                return;
            }
        };
        let cells: Vec<_> = rows.rows.iter().map(wire::cells).collect();
        let order = self.date_order;
        let today = self.today;
        let mut imported = rows_from_parsed(
            panel.kind,
            &cells,
            order,
            today,
            &mut self.grid.adapter_mut().keys,
        );
        let lists = Arc::clone(&self.books.lists);
        for r in &mut imported {
            resolve_names(r, &lists);
        }
        let existing = self.batch().filled_count();
        if existing + imported.len() > MAX_ROWS {
            let mut panel = panel;
            panel.error = Some(t_with(
                "batch_entry.blocker.too_many",
                &[("n", &(existing + imported.len()).to_string())],
            ));
            self.mapping = Some(panel);
            return;
        }
        if panel.remember
            && let Some(dir) = self.settings_dir()
        {
            let headers = panel.headers();
            let refs: Vec<&str> = headers.iter().map(String::as_str).collect();
            let mut book = BulkEntryMappings::load(&dir);
            let company = self.company_key();
            let mut memory = book.get(&company);
            memory.remember(panel.kind, &refs, &panel.mapping);
            book.set(&company, memory);
            if book.save(&dir).is_err() {
                tracing::debug!("the bulk entry mapping was not written");
            }
        }
        self.insert_rows(&panel, imported);
    }

    /// Puts the rows in the section (one undo step): over the rows from
    /// the pasted-on row, else before the blank entry row.
    fn insert_rows(&mut self, panel: &MappingPanel, imported: Vec<ImportedRow>) {
        let at = match panel.origin {
            Origin::Paste { at, .. } => at,
            Origin::File => None,
        };
        let section = panel.section;
        let count = imported.len();
        let keys: Vec<RowKey> = imported.iter().map(|r| r.row.key).collect();
        let today = self.today;
        let highest = self.grid.adapter().highest.clone();
        let mut keygen = std::mem::take(&mut self.grid.adapter_mut().keys);
        let applied = self.grid.edit_step(|b| {
            let Some(s) = b.sections.get_mut(section) else {
                return;
            };
            let start = at
                .and_then(|k| s.rows.iter().position(|r| r.key == k))
                .filter(|i| !s.rows[*i].is_blank())
                .unwrap_or_else(|| {
                    s.rows
                        .iter()
                        .rposition(|r| !r.is_blank())
                        .map_or(0, |i| i + 1)
                });
            for (i, r) in (start..).zip(imported) {
                if i < s.rows.len() && !s.rows[i].is_blank() {
                    s.rows[i] = r.row;
                } else {
                    s.rows.insert(i, r.row);
                }
            }
            ensure_entry(b, section, &mut keygen, today, &highest);
        });
        self.grid.adapter_mut().keys = keygen;
        if applied {
            self.touched.extend(keys);
            self.pasted_rows = count;
            self.last_action = Some("pasted");
            if section != self.selected() {
                self.select_section(section);
            }
        }
    }

    /// The mapping panel.
    pub(crate) fn mapping_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let memory_hint = self.mapping.as_ref().is_some_and(|p| p.recalled);
        let Some(panel) = self.mapping.as_mut() else {
            return;
        };
        let mut apply = false;
        let mut cancel = false;
        let mut flip = None;
        crate::screens::company::form::card(ui, tokens, |ui| {
            crate::screens::company::form::heading(ui, tokens, &t("batch_entry.mapping.title"));
            crate::screens::company::form::hint(ui, tokens, &t("batch_entry.mapping.explain"));
            if memory_hint {
                crate::screens::company::form::hint(ui, tokens, &t("batch_entry.mapping.recalled"));
            }
            let mut headers_on = panel.first_row_headers;
            if ui
                .checkbox(&mut headers_on, t("batch_entry.mapping.first_row_headers"))
                .changed()
            {
                flip = Some(headers_on);
            }
            let headers = panel.headers();
            let rows = panel.data_preview();
            let fields = mappable_fields(panel.kind);
            egui::ScrollArea::horizontal()
                .id_salt("batch_entry/mapping/scroll")
                .show(ui, |ui| {
                    egui::Grid::new("batch_entry/mapping/grid")
                        .striped(true)
                        .spacing([12.0, 4.0])
                        .show(ui, |ui| {
                            for (c, h) in headers.iter().enumerate() {
                                ui.vertical(|ui| {
                                    let l = ui.label(RichText::new(h).strong());
                                    let current = panel.field_at(c);
                                    let shown = current
                                        .map_or_else(|| t(SKIP), |f| t(header_key(panel.kind, f)));
                                    let mut pick: Option<Option<Field>> = None;
                                    egui::ComboBox::from_id_salt(("batch_entry/mapping/field", c))
                                        .width(130.0)
                                        .selected_text(shown)
                                        .show_ui(ui, |ui| {
                                            if ui
                                                .selectable_label(current.is_none(), t(SKIP))
                                                .clicked()
                                            {
                                                pick = Some(None);
                                            }
                                            for f in &fields {
                                                if ui
                                                    .selectable_label(
                                                        current == Some(*f),
                                                        t(header_key(panel.kind, *f)),
                                                    )
                                                    .clicked()
                                                {
                                                    pick = Some(Some(*f));
                                                }
                                            }
                                        })
                                        .response
                                        .labelled_by(l.id);
                                    if let Some(p) = pick {
                                        panel.set(c, p);
                                    }
                                });
                            }
                            ui.end_row();
                            for r in &rows {
                                for c in 0..headers.len() {
                                    ui.label(r.get(c).map_or("", String::as_str));
                                }
                                ui.end_row();
                            }
                        });
                });
            ui.checkbox(&mut panel.remember, t("batch_entry.mapping.remember"))
                .on_hover_text(t("batch_entry.mapping.remember.tip"));
            if let Some(e) = &panel.error {
                ui.label(RichText::new(e).color(tokens.warn_text));
            }
            ui.horizontal(|ui| {
                apply =
                    crate::widgets::primary_button(ui, &t("batch_entry.mapping.apply")).clicked();
                cancel = ui.button(t("batch_entry.mapping.cancel")).clicked();
            });
        });
        if let Some(on) = flip {
            let memory = self.memory();
            if let Some(p) = self.mapping.as_mut() {
                p.first_row_headers = on;
                p.remap(&memory);
            }
        }
        if cancel {
            self.mapping = None;
        } else if apply {
            self.apply_mapping();
        }
    }

    /// Import CSV file: the path field (as in the list import).
    pub(crate) fn import_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        if !self.import_open {
            if let Some(e) = &self.import_error {
                ui.label(RichText::new(e).color(tokens.warn_text));
            }
            return;
        }
        let mut read = false;
        let mut cancel = false;
        crate::screens::company::form::card(ui, tokens, |ui| {
            crate::screens::company::form::heading(ui, tokens, &t("batch_entry.import.title"));
            crate::screens::company::form::hint(ui, tokens, &t("batch_entry.import.explain"));
            let r = crate::screens::company::form::field_wide(
                ui,
                &t("batch_entry.import.path"),
                &mut self.import_path,
            );
            read |= crate::screens::company::form::activated(&r);
            if let Some(e) = &self.import_error {
                ui.label(RichText::new(e).color(tokens.warn_text));
            }
            ui.horizontal(|ui| {
                read |= crate::widgets::primary_button(ui, &t("batch_entry.import.read")).clicked();
                cancel = ui.button(t("batch_entry.mapping.cancel")).clicked();
            });
        });
        if cancel {
            self.import_open = false;
            self.import_error = None;
        } else if read {
            self.read_file();
        }
    }

    /// The mapping panel's control-channel state (no cell text).
    pub(crate) fn mapping_json(&self) -> serde_json::Value {
        match &self.mapping {
            None => serde_json::Value::Null,
            Some(p) => serde_json::json!({
                "columns": p.width,
                "first_row_headers": p.first_row_headers,
                "mapped": p.mapping.keys().collect::<Vec<_>>(),
                "preview_rows": p.data_preview().len(),
                "remember": p.remember,
                "recalled": p.recalled,
                "error": p.error.is_some(),
            }),
        }
    }
}

/// Names and accounts typed exactly as in the lists become picks (the
/// grid then shows them as picked and suggestions can run); anything else
/// stays as typed for the engine's check.
fn resolve_names(r: &mut ImportedRow, lists: &crate::screens::register::lists::RegisterLists) {
    let row = &mut r.row;
    if row.contact.is_none()
        && !row.contact_text.trim().is_empty()
        && let Some(id) = lists
            .exact(NAMES, row.contact_text.trim())
            .and_then(|s| lists.name(s.id))
    {
        row.contact = Some(id);
        row.contact_text.clear();
    }
    if row.account.is_none()
        && !row.account_text.trim().is_empty()
        && let Some(id) = lists
            .exact(ACCOUNTS, row.account_text.trim())
            .and_then(|s| lists.account(s.id))
    {
        row.account = Some(id);
        row.account_text.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn panel(text: &str, origin: Origin, kind: SectionKind) -> MappingPanel {
        let (separator, recs) = records(text).expect("records");
        let (first, rest) = recs.split_first().expect("rows");
        let cells: Vec<&str> = first.iter().map(String::as_str).collect();
        let mut p = MappingPanel {
            first_row_headers: origin == Origin::File || looks_like_header(&cells),
            origin,
            section: 0,
            kind,
            text: text.to_string(),
            separator,
            first: first.clone(),
            preview: rest.to_vec(),
            width: recs.iter().map(Vec::len).max().unwrap_or(0),
            mapping: Mapping::new(),
            remember: false,
            recalled: false,
            error: None,
        };
        p.remap(&cl_model::forms::batch::MappingMemory::default());
        p
    }

    #[test]
    fn a_header_paste_maps_by_name_and_a_headerless_one_by_position() {
        let p = panel(
            "Date\tPayee\tAmount\n11/02/2026\tHollister Fuel Stop\t12.345\n",
            Origin::Paste {
                at: None,
                column: 0,
            },
            SectionKind::Checks,
        );
        assert!(p.first_row_headers);
        assert_eq!(p.mapping.get(&Field::Date), Some(&0));
        assert_eq!(p.mapping.get(&Field::Contact), Some(&1));
        assert_eq!(p.mapping.get(&Field::Amount), Some(&2));
        assert_eq!(p.data_preview().len(), 1);
        assert_eq!(p.bytes(), p.text.as_bytes());
        // No header: Column 1..n, left to right from the cursor's column
        // (Pay to on a check, then Account, then Amount).
        let q = panel(
            "Hollister Fuel Stop\t6100\t20.00\n",
            Origin::Paste {
                at: None,
                column: 3,
            },
            SectionKind::Checks,
        );
        assert!(!q.first_row_headers);
        assert_eq!(q.mapping.get(&Field::Contact), Some(&0));
        assert_eq!(q.mapping.get(&Field::Account), Some(&1));
        assert_eq!(q.mapping.get(&Field::Amount), Some(&2));
        let bytes = String::from_utf8(q.bytes()).expect("utf8");
        assert!(
            bytes.starts_with("Column 1\tColumn 2\tColumn 3\n"),
            "{bytes}"
        );
        assert_eq!(q.data_preview().len(), 1);
    }

    #[test]
    fn one_field_per_column() {
        let mut p = panel(
            "Date,Memo,Amount\n2026-11-01,x,5\n",
            Origin::File,
            SectionKind::Expenses,
        );
        p.set(1, Some(Field::Amount));
        assert_eq!(p.field_at(1), Some(Field::Amount));
        assert_eq!(p.field_at(2), None, "Amount moved");
        p.set(1, None);
        assert_eq!(p.field_at(1), None);
    }
}
