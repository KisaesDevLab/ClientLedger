//! The Audit Trail viewer (P29 WP6, Q-91, Q-156): every change to the
//! company's records, newest first, with filters (dates, action, record
//! type, user, text, one record), pages of 100, and each change's fields
//! before and after **as they were saved** (`cl_model::audit_view::diff`,
//! never computed). The source column is derived in the app (Q-P29s).
//!
//! Export CSV and Save PDF are for Admins only (Q-156, Q-P29t: the gate is
//! in the app; anyone with `audit_log:read` can read the rows). An export
//! reads every matching row in pages of 500, skips rows already written or
//! made after it started, and writes a temp file in the chosen folder that
//! is renamed on success, so a failed export leaves no file.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::Receiver;

use cl_api::{ApiError, AuditFilter, AuditPage, AuditRow, RequestGroup};
use cl_core::ids::UserId;
use cl_core::report::{
    Cell, ColumnHead, ColumnKind, ReportRow, ReportTable, RowKind, RowPath, literal_caption,
};
use cl_core::{CalendarDate, Timestamp};
use cl_model::CommandId;
use cl_model::audit_view::{self, AuditSource, EXPORT_PAGE, ExportCursor, ExportRow, SCREEN_PAGE};
use eframe::egui::{self, RichText, Ui};
use serde_json::{Value, json};

use crate::company::CompanyLink;
use crate::i18n::{t, t_with};
use crate::screens::company::form::{self, DateInput};
use crate::screens::invoice::engine::{self, Job};
use crate::theme::Tokens;
use crate::windows::Screen;

/// The window kind.
pub const KIND: &str = "company.audit_trail";

/// The window for `kind`, if it is this one.
#[must_use]
pub fn make(
    kind: CommandId,
    link: &Arc<CompanyLink>,
    _open: Option<(cl_core::ids::TenantId, String)>,
    today: CalendarDate,
) -> Option<Box<dyn Screen>> {
    (kind.as_str() == KIND).then(|| Box::new(AuditTrailScreen::new(Arc::clone(link), today)) as _)
}

/// The engine's action words the Action filter offers.
const ACTIONS: [&str; 5] = ["create", "update", "void", "delete", "override"];

/// Which file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Csv,
    Pdf,
}

impl Kind {
    const fn ext(self) -> &'static str {
        match self {
            Kind::Csv => "csv",
            Kind::Pdf => "pdf",
        }
    }

    const fn filter(self) -> &'static str {
        match self {
            Kind::Csv => "picker.filter.csv",
            Kind::Pdf => "picker.filter.pdf",
        }
    }
}

/// An export: its rows being read and turned into a file, and its Save
/// dialog; written when both are in.
struct Export {
    kind: Kind,
    build: Job<(Vec<u8>, usize)>,
    built: Option<Result<(Vec<u8>, usize), ApiError>>,
    dialog: Receiver<Option<PathBuf>>,
    /// The Save dialog's answer, once in (Done(None): cancelled).
    path: Answer,
}

/// The Save dialog: not answered yet, or answered (None: cancelled).
enum Answer {
    Waiting,
    Done(Option<PathBuf>),
}

/// The screen.
pub struct AuditTrailScreen {
    link: Arc<CompanyLink>,
    group: Option<RequestGroup>,
    today: CalendarDate,
    from: DateInput,
    to: DateInput,
    action: Option<&'static str>,
    entity: Option<&'static str>,
    user: Option<UserId>,
    search: String,
    /// "Only this record" (opened from a record), and whether it applies.
    record: Option<uuid::Uuid>,
    only_record: bool,
    applied: AuditFilter,
    offset: u64,
    page: Option<AuditPage>,
    read: Option<Job<AuditPage>>,
    selected: Option<u64>,
    /// People seen in the rows read so far (no user list read here).
    users: BTreeMap<UserId, String>,
    export: Option<Export>,
    /// Rows in the last file saved.
    exported: Option<usize>,
    /// i18n key or text of the last message, and whether it is good news.
    message: Option<(String, bool)>,
    error: Option<ApiError>,
}

fn blank_date() -> DateInput {
    DateInput {
        text: String::new(),
        value: None,
    }
}

/// The time of a change as the owner reads it (local time).
fn when_text(ts: Timestamp) -> String {
    ts.as_utc()
        .with_timezone(&chrono::Local)
        .format("%m/%d/%Y %I:%M %p")
        .to_string()
}

fn action_text(a: &str) -> String {
    let key = format!("reports.audit.action.{a}");
    if crate::i18n::has(&key) {
        t(&key)
    } else {
        a.replace('_', " ")
    }
}

fn entity_text(e: &str) -> String {
    let key = format!("reports.audit.entity.{e}");
    if crate::i18n::has(&key) {
        t(&key)
    } else {
        e.replace('_', " ")
    }
}

/// The source column in words (Q-P29s).
fn source_text(r: &AuditRow) -> String {
    match audit_view::source(
        r.user.is_some(),
        r.user_name.as_deref(),
        r.before.as_ref().map(cl_api::AuditImage::raw),
        r.after.as_ref().map(cl_api::AuditImage::raw),
    ) {
        AuditSource::Automatic => t("audit.source.automatic"),
        AuditSource::Channel(c) => {
            AuditSource::channel_key(&c).map_or_else(|| c.replace('_', " "), t)
        }
        AuditSource::EnteredBy(name) if name.trim().is_empty() => t("audit.source.someone"),
        AuditSource::EnteredBy(name) => t_with("audit.source.entered_by", &[("name", &name)]),
    }
}

/// A field's path in words: known fields by name, the rest as saved.
fn field_text(path: &str) -> String {
    path.split('.')
        .map(|part| {
            let (name, index) = part.split_once('[').map_or((part, ""), |(n, i)| (n, i));
            let key = format!("reports.audit.field.{name}");
            let shown = if crate::i18n::has(&key) {
                t(&key)
            } else {
                name.to_string()
            };
            if index.is_empty() {
                shown
            } else {
                format!("{shown} [{index}")
            }
        })
        .collect::<Vec<_>>()
        .join(" > ")
}

fn changes(r: &AuditRow) -> Vec<(String, String, String)> {
    let (yes, no) = (t("reports.audit.yes"), t("reports.audit.no"));
    audit_view::diff(
        r.before.as_ref().map(cl_api::AuditImage::raw),
        r.after.as_ref().map(cl_api::AuditImage::raw),
    )
    .into_iter()
    .map(|c| {
        let v =
            |x: Option<audit_view::FieldValue>| x.map(|x| x.text(&yes, &no)).unwrap_or_default();
        (field_text(&c.path), v(c.before), v(c.after))
    })
    .collect()
}

fn export_row(r: &AuditRow) -> ExportRow {
    ExportRow {
        when: when_text(r.at),
        user: r.user_name.clone().unwrap_or_default(),
        action: action_text(&r.action),
        record: entity_text(&r.entity_type),
        record_id: r.entity_id.clone().unwrap_or_default(),
        source: source_text(r),
        fields: changes(r),
    }
}

fn csv_captions() -> Vec<String> {
    [
        "audit.col.when",
        "audit.col.user",
        "audit.col.action",
        "audit.col.record",
        "audit.col.record_id",
        "audit.col.source",
        "audit.col.field",
        "audit.col.before",
        "audit.col.after",
    ]
    .iter()
    .map(|k| t(k))
    .collect()
}

/// Collects the export's file from its rows.
struct FileBuilder {
    kind: Kind,
    cursor: ExportCursor,
    csv: String,
    pdf: Vec<ReportRow>,
}

impl FileBuilder {
    fn new(kind: Kind, started: Timestamp) -> FileBuilder {
        FileBuilder {
            kind,
            cursor: ExportCursor::new(started),
            csv: audit_view::csv_heading(&csv_captions()),
            pdf: Vec::new(),
        }
    }

    /// Adds one page; true when the trail has more.
    fn add(&mut self, page: &AuditPage) -> bool {
        let got = page.rows.len() as u64;
        for r in &page.rows {
            if !self.cursor.take(r.id, r.at) {
                continue;
            }
            let e = export_row(r);
            match self.kind {
                Kind::Csv => self.csv.push_str(&audit_view::csv_lines(&e)),
                Kind::Pdf => self.pdf_rows(&e),
            }
        }
        self.cursor.offset += got;
        got > 0 && self.cursor.offset < page.total
    }

    fn pdf_rows(&mut self, e: &ExportRow) {
        let k = self.pdf.len().to_string();
        let text = |s: &str| {
            if s.is_empty() {
                Cell::Empty
            } else {
                Cell::Text(literal_caption(s))
            }
        };
        self.pdf.push(ReportRow {
            kind: RowKind::Header,
            level: 0,
            path: RowPath::of([k.as_str()]),
            caption: literal_caption(&e.when),
            cells: vec![
                text(&e.user),
                text(&e.action),
                text(&e.record),
                text(&e.source),
                Cell::Empty,
                Cell::Empty,
            ],
            drill: None,
            expandable: false,
        });
        for (i, (f, b, a)) in e.fields.iter().enumerate() {
            let p = format!("{k}.{i}");
            self.pdf.push(ReportRow {
                kind: RowKind::Detail,
                level: 1,
                path: RowPath::of([p.as_str()]),
                caption: literal_caption(f),
                cells: vec![
                    Cell::Empty,
                    Cell::Empty,
                    Cell::Empty,
                    Cell::Empty,
                    text(b),
                    text(a),
                ],
                drill: None,
                expandable: false,
            });
        }
    }

    fn finish(self) -> Result<(Vec<u8>, usize), ApiError> {
        let n = self.cursor.written();
        match self.kind {
            Kind::Csv => Ok((self.csv.into_bytes(), n)),
            Kind::Pdf => {
                let col = |key: &str, caption: &str| ColumnHead {
                    key: key.into(),
                    caption: literal_caption(&t(caption)),
                    kind: ColumnKind::Text,
                    range: None,
                    drill: None,
                };
                let table = ReportTable {
                    columns: vec![
                        col("user", "audit.col.user"),
                        col("action", "audit.col.action"),
                        col("record", "audit.col.record"),
                        col("source", "audit.col.source"),
                        col("before", "audit.col.before"),
                        col("after", "audit.col.after"),
                    ],
                    rows: self.pdf,
                    ..ReportTable::default()
                };
                let rows: Vec<usize> = (0..table.rows.len()).collect();
                let tr = |k: &str| t(k);
                let doc = cl_print::report_export::to_report(
                    &table,
                    &rows,
                    &t("audit.export.title"),
                    Vec::new(),
                    &tr,
                );
                cl_print::reports::render_report(&doc)
                    .map(|b| (b, n))
                    .map_err(|_| ApiError::Local(cl_api::LocalValidation::InvalidValue))
            }
        }
    }
}

impl AuditTrailScreen {
    /// Reads the first page of the whole trail.
    #[must_use]
    pub fn new(link: Arc<CompanyLink>, today: CalendarDate) -> AuditTrailScreen {
        let group = link.group().and_then(Result::ok);
        let mut s = AuditTrailScreen {
            link,
            group,
            today,
            from: blank_date(),
            to: blank_date(),
            action: None,
            entity: None,
            user: None,
            search: String::new(),
            record: None,
            only_record: false,
            applied: AuditFilter::default(),
            offset: 0,
            page: None,
            read: None,
            selected: None,
            users: BTreeMap::new(),
            export: None,
            exported: None,
            message: None,
            error: None,
        };
        s.load();
        s
    }

    /// Only the changes to one record ("this record").
    pub fn for_record(&mut self, id: uuid::Uuid) {
        self.record = Some(id);
        self.only_record = true;
        self.apply();
    }

    fn filters_supported(&self) -> bool {
        self.link.is_demo()
            || self
                .link
                .capabilities()
                .has(cl_api::resources::reports::run::AUDIT)
    }

    fn admin(&self) -> bool {
        self.link.rights().is_admin()
    }

    fn load(&mut self) {
        let (f, offset) = (self.applied.clone(), self.offset);
        let f2 = f.clone();
        self.error = None;
        self.read = Some(engine::read(
            &self.link,
            self.group.as_ref(),
            move |_d| Ok(crate::company::demo::audit::page(&f2, SCREEN_PAGE, offset)),
            move |s| async move { s.audit().list(&f, SCREEN_PAGE, offset).await },
        ));
        self.poll();
    }

    /// Reads the filters typed; `None` (with a message) when one is wrong.
    fn typed_filter(&mut self) -> Option<AuditFilter> {
        for d in [&mut self.from, &mut self.to] {
            if d.text.trim().is_empty() {
                d.value = None;
            } else {
                d.reparse(self.today);
                if d.value.is_none() {
                    self.message = Some(("audit.bad_date".into(), false));
                    return None;
                }
            }
        }
        if let (Some(a), Some(b)) = (self.from.value, self.to.value)
            && a > b
        {
            self.message = Some(("audit.dates_reversed".into(), false));
            return None;
        }
        let f = AuditFilter {
            entity_type: self.entity.map(str::to_string),
            action: self.action.map(str::to_string),
            user: self.user,
            from: self.from.value,
            to: self.to.value,
            search: Some(self.search.trim().to_string()).filter(|s| !s.is_empty()),
            entity_id: self.record.filter(|_| self.only_record),
        };
        if (f.user.is_some() || f.entity_id.is_some()) && !self.filters_supported() {
            self.message = Some(("audit.needs_update".into(), false));
            return None;
        }
        Some(f)
    }

    fn apply(&mut self) {
        self.message = None;
        if let Some(f) = self.typed_filter() {
            self.applied = f;
            self.offset = 0;
            self.selected = None;
            self.load();
        }
    }

    fn clear(&mut self) {
        self.from = blank_date();
        self.to = blank_date();
        self.action = None;
        self.entity = None;
        self.user = None;
        self.search.clear();
        self.only_record = false;
        self.apply();
    }

    fn total(&self) -> u64 {
        self.page.as_ref().map_or(0, |p| p.total)
    }

    fn next(&mut self) {
        let step = u64::from(SCREEN_PAGE);
        if self.offset + step < self.total() {
            self.offset += step;
            self.selected = None;
            self.load();
        }
    }

    fn previous(&mut self) {
        if self.offset > 0 {
            self.offset = self.offset.saturating_sub(u64::from(SCREEN_PAGE));
            self.selected = None;
            self.load();
        }
    }

    fn poll(&mut self) {
        if let Some(r) = engine::poll(&mut self.read) {
            match r {
                Ok(p) => {
                    for r in &p.rows {
                        if let (Some(u), Some(n)) = (r.user, &r.user_name) {
                            self.users.entry(u).or_insert_with(|| n.clone());
                        }
                    }
                    self.page = Some(p);
                }
                Err(ApiError::Local(cl_api::LocalValidation::Unsupported)) => {
                    self.message = Some(("audit.needs_update".into(), false));
                }
                Err(e) => self.error = Some(e),
            }
        }
        self.poll_export();
    }

    /// Export CSV or Save PDF (Admins only): reads every matching row.
    fn start_export(&mut self, ctx: &egui::Context, kind: Kind) {
        if !self.admin() || self.export.is_some() {
            return;
        }
        self.message = None;
        let started = Timestamp::from_unix_millis(chrono::Utc::now().timestamp_millis())
            .unwrap_or_else(|_| {
                // Invariant: the clock is inside chrono's range.
                Timestamp::from_unix_millis(0).unwrap_or_else(|_| unreachable!())
            });
        let (f, f2) = (self.applied.clone(), self.applied.clone());
        let build = engine::read(
            &self.link,
            self.group.as_ref(),
            move |_d| {
                let mut b = FileBuilder::new(kind, started);
                while b.add(&crate::company::demo::audit::page(
                    &f2,
                    EXPORT_PAGE,
                    b.cursor.offset,
                )) {}
                b.finish()
            },
            move |s| async move {
                let mut b = FileBuilder::new(kind, started);
                loop {
                    let page = s.audit().list(&f, EXPORT_PAGE, b.cursor.offset).await?;
                    if !b.add(&page) {
                        break;
                    }
                }
                b.finish()
            },
        );
        let name = format!(
            "{}.{}",
            cl_print::files::safe_component(&t("audit.export.file")),
            kind.ext()
        );
        let dialog = crate::widgets::file_picker::save_file_as(
            ctx,
            crate::widgets::file_picker::PickUse::SaveReport,
            None,
            &name,
            (kind.filter(), kind.ext()),
        );
        self.export = Some(Export {
            kind,
            build,
            built: None,
            dialog,
            path: Answer::Waiting,
        });
        self.poll_export();
    }

    fn poll_export(&mut self) {
        let Some(x) = self.export.as_mut() else {
            return;
        };
        if x.built.is_none()
            && let Some(r) = x.build.take()
        {
            x.built = Some(r);
        }
        if matches!(x.path, Answer::Waiting)
            && let Ok(p) = x.dialog.try_recv()
        {
            x.path = Answer::Done(p);
        }
        if matches!(x.path, Answer::Done(None)) {
            self.export = None;
            return;
        }
        if x.built.is_none() || matches!(x.path, Answer::Waiting) {
            return;
        }
        let Some(Export {
            kind,
            built: Some(built),
            path: Answer::Done(Some(path)),
            ..
        }) = self.export.take()
        else {
            return;
        };
        let file = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        match built {
            Ok((bytes, n)) if cl_print::report_export::write_atomic(&path, &bytes).is_ok() => {
                tracing::info!(file = kind.ext(), rows = n, "audit trail exported");
                self.exported = Some(n);
                self.message = Some((
                    crate::i18n::t_count("audit.export.saved", n, &[("file", &file)]),
                    true,
                ));
            }
            Ok(_) => {
                tracing::warn!(
                    file = kind.ext(),
                    "an audit trail export could not be written"
                );
                self.message = Some((t("audit.export.failed"), false));
            }
            Err(e) => {
                tracing::warn!(file = kind.ext(), "an audit trail export could not be read");
                self.error = Some(e);
            }
        }
    }

    fn selected_row(&self) -> Option<&AuditRow> {
        let id = self.selected?;
        self.page.as_ref()?.rows.iter().find(|r| r.id == id)
    }

    fn message_text(&self) -> Option<(String, bool)> {
        self.message
            .as_ref()
            .map(|(m, good)| (if crate::i18n::has(m) { t(m) } else { m.clone() }, *good))
    }

    fn filters_ui(&mut self, ui: &mut Ui) -> bool {
        let mut apply = false;
        ui.horizontal_wrapped(|ui| {
            let r = form::date_field(ui, &t("audit.from"), &mut self.from, self.today);
            apply |= form::activated(&r);
            let r = form::date_field(ui, &t("audit.to"), &mut self.to, self.today);
            apply |= form::activated(&r);
            let any = t("audit.any");
            let l = ui.add(egui::Label::new(t("audit.action")).selectable(false));
            egui::ComboBox::from_id_salt("audit/action")
                .selected_text(self.action.map_or_else(|| any.clone(), action_text))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.action, None, any.clone());
                    for a in ACTIONS {
                        ui.selectable_value(&mut self.action, Some(a), action_text(a));
                    }
                })
                .response
                .labelled_by(l.id);
            let l = ui.add(egui::Label::new(t("audit.record_type")).selectable(false));
            egui::ComboBox::from_id_salt("audit/entity")
                .selected_text(self.entity.map_or_else(|| any.clone(), entity_text))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.entity, None, any.clone());
                    for e in cl_api::resources::reports::adapters::controls::AUDIT_ENTITIES {
                        ui.selectable_value(&mut self.entity, Some(e), entity_text(e));
                    }
                })
                .response
                .labelled_by(l.id);
            if self.filters_supported() {
                let l = ui.add(egui::Label::new(t("audit.user")).selectable(false));
                let shown = self
                    .user
                    .and_then(|u| self.users.get(&u).cloned())
                    .unwrap_or_else(|| any.clone());
                let users: Vec<(UserId, String)> =
                    self.users.iter().map(|(k, v)| (*k, v.clone())).collect();
                egui::ComboBox::from_id_salt("audit/user")
                    .selected_text(shown)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.user, None, any.clone());
                        for (id, name) in users {
                            ui.selectable_value(&mut self.user, Some(id), name);
                        }
                    })
                    .response
                    .labelled_by(l.id);
            }
            let l = ui.add(egui::Label::new(t("audit.search")).selectable(false));
            let r = ui
                .add_sized([180.0, 24.0], egui::TextEdit::singleline(&mut self.search))
                .labelled_by(l.id)
                .on_hover_text(t("audit.search.tip"));
            apply |= form::activated(&r);
            if self.record.is_some() {
                ui.checkbox(&mut self.only_record, t("audit.this_record"));
            }
            apply |= ui.button(t("audit.apply")).clicked();
            if ui.button(t("audit.clear")).clicked() {
                self.clear();
            }
        });
        apply
    }

    fn table_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let Some(page) = &self.page else {
            return;
        };
        if page.rows.is_empty() {
            form::hint(ui, tokens, &t("audit.empty"));
            return;
        }
        let mut pick = None;
        egui::ScrollArea::vertical()
            .id_salt("audit/rows")
            .max_height(ui.available_height() * 0.55)
            .show(ui, |ui| {
                egui::Grid::new("audit/grid")
                    .striped(true)
                    .num_columns(5)
                    .spacing([16.0, 4.0])
                    .show(ui, |ui| {
                        for k in [
                            "audit.col.when",
                            "audit.col.user",
                            "audit.col.action",
                            "audit.col.record",
                            "audit.col.source",
                        ] {
                            let r = ui.label(RichText::new(t(k)).strong());
                            if k == "audit.col.source" {
                                r.on_hover_text(t("audit.source.tip"));
                            }
                        }
                        ui.end_row();
                        for r in &page.rows {
                            let on = self.selected == Some(r.id);
                            if ui.selectable_label(on, when_text(r.at)).clicked() {
                                pick = Some(r.id);
                            }
                            ui.label(r.user_name.clone().unwrap_or_default());
                            ui.label(action_text(&r.action));
                            ui.label(entity_text(&r.entity_type));
                            ui.label(source_text(r));
                            ui.end_row();
                        }
                    });
            });
        if let Some(id) = pick {
            self.selected = Some(id);
        }
    }

    fn pager_ui(&mut self, ui: &mut Ui) {
        let total = self.total();
        let shown = self.page.as_ref().map_or(0, |p| p.rows.len() as u64);
        let (mut prev, mut next) = (false, false);
        ui.horizontal(|ui| {
            if total > 0 {
                ui.label(t_with(
                    "audit.rows",
                    &[
                        ("from", &crate::i18n::count_text(self.offset + 1)),
                        ("to", &crate::i18n::count_text(self.offset + shown)),
                        ("total", &crate::i18n::count_text(total)),
                    ],
                ));
            }
            prev = ui
                .add_enabled(self.offset > 0, egui::Button::new(t("audit.previous")))
                .clicked();
            next = ui
                .add_enabled(
                    self.offset + u64::from(SCREEN_PAGE) < total,
                    egui::Button::new(t("audit.next")),
                )
                .clicked();
        });
        if prev {
            self.previous();
        }
        if next {
            self.next();
        }
    }

    fn details_ui(&self, ui: &mut Ui, tokens: &Tokens) {
        ui.label(RichText::new(t("audit.details")).strong())
            .on_hover_text(t("audit.details.tip"));
        let Some(r) = self.selected_row() else {
            form::hint(ui, tokens, &t("audit.details.pick"));
            return;
        };
        let rows = changes(r);
        if rows.is_empty() {
            form::hint(ui, tokens, &t("audit.details.none"));
            return;
        }
        egui::ScrollArea::vertical()
            .id_salt("audit/details")
            .show(ui, |ui| {
                egui::Grid::new("audit/fields")
                    .striped(true)
                    .num_columns(3)
                    .spacing([16.0, 4.0])
                    .show(ui, |ui| {
                        for k in ["audit.col.field", "audit.col.before", "audit.col.after"] {
                            ui.label(RichText::new(t(k)).strong());
                        }
                        ui.end_row();
                        for (f, b, a) in rows {
                            ui.label(f);
                            ui.label(RichText::new(b).monospace());
                            ui.label(RichText::new(a).monospace());
                            ui.end_row();
                        }
                    });
            });
    }
}

impl Screen for AuditTrailScreen {
    fn title(&self) -> String {
        t("cmd.company.audit_trail")
    }

    fn dirty(&self) -> bool {
        false
    }

    fn can_save(&self) -> bool {
        false
    }

    fn ui(&mut self, ui: &mut Ui) {
        self.poll();
        let tokens = Tokens::of(ui.ctx());
        if self.read.is_some() || self.export.is_some() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
        }
        form::heading(ui, &tokens, &t("cmd.company.audit_trail"));
        form::hint(ui, &tokens, &t("audit.intro"));
        if self.filters_ui(ui) {
            self.apply();
        }
        // Export: Admins only (Q-156); others don't see the buttons.
        if self.admin() {
            let busy = self.export.is_some();
            let mut kind = None;
            ui.horizontal(|ui| {
                ui.add_enabled_ui(!busy, |ui| {
                    if ui
                        .button(t("audit.export_csv"))
                        .on_hover_text(t("audit.export.tip"))
                        .clicked()
                    {
                        kind = Some(Kind::Csv);
                    }
                    if ui
                        .button(t("audit.save_pdf"))
                        .on_hover_text(t("audit.export.tip"))
                        .clicked()
                    {
                        kind = Some(Kind::Pdf);
                    }
                });
                if busy {
                    ui.label(t("audit.export.busy"));
                }
            });
            if let Some(k) = kind {
                self.start_export(ui.ctx(), k);
            }
        }
        if let Some((m, good)) = self.message_text() {
            if good {
                form::inline_note(ui, &tokens, &m);
            } else {
                form::inline_warning(ui, &tokens, &m);
            }
        }
        if let Some(e) = &self.error {
            form::inline_error_details(ui, &tokens, e, "audit");
        }
        if self.page.is_none() && self.read.is_some() {
            form::progress_line(ui, &tokens, &t("audit.loading"));
            return;
        }
        self.pager_ui(ui);
        self.table_ui(ui, &tokens);
        ui.separator();
        self.details_ui(ui, &tokens);
    }

    fn state(&self) -> Value {
        let rows = self.page.as_ref().map(|p| &p.rows);
        json!({
            "screen": "audit_trail",
            "loaded": self.page.is_some(),
            "rows": rows.map_or(0, Vec::len),
            "total": self.total(),
            "offset": self.offset,
            "first_id": rows.and_then(|r| r.first()).map(|r| r.id),
            "selected": self.selected,
            "fields": self.selected_row().map_or(0, |r| changes(r).len()),
            "can_export": self.admin(),
            "exporting": self.export.is_some(),
            "exported": self.exported,
            "message": self.message.as_ref().map(|(m, _)| m.clone()),
            "filter": {
                "action": self.applied.action,
                "entity_type": self.applied.entity_type,
                "from": self.applied.from.map(CalendarDate::to_wire),
                "to": self.applied.to.map(CalendarDate::to_wire),
                "user": self.applied.user.is_some(),
                "search": self.applied.search.is_some(),
                "record": self.applied.entity_id.is_some(),
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen() -> (tempfile::TempDir, AuditTrailScreen) {
        let dir = tempfile::tempdir().expect("tmp");
        let link = Arc::new(CompanyLink::demo(dir.path()));
        let today = CalendarDate::from_ymd(2026, 11, 16).expect("d");
        let s = AuditTrailScreen::new(link, today);
        (dir, s)
    }

    #[test]
    fn pages_through_the_demo_trail() {
        let (_d, mut s) = screen();
        assert_eq!(s.state()["rows"], 100);
        assert_eq!(s.state()["total"], 130);
        s.next();
        assert_eq!(s.state()["rows"], 30);
        assert_eq!(s.state()["offset"], 100);
        s.next();
        assert_eq!(s.state()["offset"], 100, "no page after the last");
        s.previous();
        assert_eq!(s.state()["offset"], 0);
    }

    #[test]
    fn filters_apply_and_bad_dates_are_refused() {
        let (_d, mut s) = screen();
        s.action = Some("void");
        s.apply();
        let total = s.total();
        assert!(total > 0 && total < 130);
        assert_eq!(s.state()["filter"]["action"], "void");
        s.from.text = "13/45/2026".into();
        s.apply();
        assert_eq!(s.state()["message"], "audit.bad_date");
        assert_eq!(s.total(), total, "the last answer stays");
        s.from.text = "11/30/2026".into();
        s.to.text = "11/01/2026".into();
        s.apply();
        assert_eq!(s.state()["message"], "audit.dates_reversed");
        s.clear();
        assert_eq!(s.total(), 130);
    }

    #[test]
    fn the_details_are_verbatim_and_hide_secrets() {
        let (_d, mut s) = screen();
        // The newest: a nested change.
        s.selected = Some(130);
        let rows = changes(s.selected_row().expect("row"));
        assert!(
            rows.iter()
                .any(|(_, b, a)| b == "Office chairs" && a == "=Office chairs")
        );
        assert!(
            rows.iter()
                .any(|(f, b, a)| f.contains("[1") && b == "Chairs" && a == "Chairs (2)")
        );
        // The contact: its taxpayer id never shows.
        s.selected = Some(128);
        let shown = format!("{:?}", changes(s.selected_row().expect("row")));
        assert!(shown.contains("65802"), "{shown}");
        assert!(!shown.contains("12-3456789") && !shown.contains("98-7654321"));
    }

    #[test]
    fn the_source_column_reads_in_plain_words() {
        let (_d, s) = screen();
        let rows = &s.page.as_ref().expect("page").rows;
        let feed = rows.iter().find(|r| r.id == 129).expect("feed row");
        assert_eq!(source_text(feed), "Bank feed");
        let auto = rows.iter().find(|r| r.user.is_none()).expect("automatic");
        assert_eq!(source_text(auto), "Automatic");
        let by = rows.iter().find(|r| r.id == 130).expect("row");
        assert!(
            source_text(by).starts_with("Entered by "),
            "{}",
            source_text(by)
        );
    }

    #[test]
    fn the_export_is_for_admins_and_writes_every_row_formula_safe() {
        let (_d, s) = screen();
        assert_eq!(s.state()["can_export"], true);
        let mut b = FileBuilder::new(
            Kind::Csv,
            Timestamp::parse_wire("2030-01-01T00:00:00Z").expect("t"),
        );
        while b.add(&crate::company::demo::audit::page(
            &AuditFilter::default(),
            50,
            b.cursor.offset,
        )) {}
        let (bytes, n) = b.finish().expect("file");
        assert_eq!(n, 130);
        let csv = String::from_utf8(bytes).expect("utf8");
        assert!(csv.contains(",'=Office chairs"), "formula neutralised");
        assert!(!csv.contains("12-3456789"));
        let mut p = FileBuilder::new(
            Kind::Pdf,
            Timestamp::parse_wire("2030-01-01T00:00:00Z").expect("t"),
        );
        while p.add(&crate::company::demo::audit::page(
            &AuditFilter::default(),
            500,
            p.cursor.offset,
        )) {}
        let (pdf, _) = p.finish().expect("pdf");
        assert!(pdf.starts_with(b"%PDF"));
        // A non-admin sees no export.
        s.link
            .set_rights(cl_model::permissions::Rights::view_only());
        assert_eq!(s.state()["can_export"], false);
    }

    #[test]
    fn every_text_is_in_the_catalog() {
        for k in csv_captions() {
            assert!(!k.starts_with("audit."), "{k}");
        }
        for k in [
            "audit.source.bank_feed",
            "audit.export.saved",
            "audit.empty",
        ] {
            assert!(crate::i18n::has(k), "{k}");
        }
    }
}
