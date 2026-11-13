//! The Report Builder editor (ADD-03 A08, plan §3.8; mockup
//! `docs/design/mockups/ReportBuilder.html`): one window per report, with
//! the outline, the row/columns/report properties and a live preview the
//! engine runs. Guided mode asks five questions instead (Q-A08a).
//!
//! The UI adds up nothing: every figure in the preview is the engine's.
//! Every change goes through `cl_model::report_def::editor` (one undo step
//! each); text fields commit when they lose focus.
//!
//! Files: `chrome.rs` the toolbar, hint and settings band; `outline.rs`
//! the tree; `properties.rs` the Row/Columns/Report tabs with
//! `sources.rs`, `formula.rs` and `columns.rs`; `preview.rs`; `guided.rs`;
//! `save.rs` Memorize and Save changes; `lists.rs` the names filters pick
//! from; `picker.rs` the small controls.

mod chrome;
mod columns;
mod formula;
mod guided;
mod lists;
mod outline;
mod picker;
mod preview;
mod properties;
mod save;
mod sources;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use cl_api::RequestGroup;
use cl_core::CalendarDate;
use cl_core::fiscal::FiscalYearSpec;
use cl_core::report::{Adjusting, Basis};
use cl_model::report_def::editor::{
    Applied, BuilderEditor, Op, Origin, Saved, choices::amount_filter_value,
};
use cl_model::report_def::guided::GuidedState;
use cl_model::report_def::{BuilderDefinition, DateMode, Node, Periods, outline as tree};
use cl_model::reports::{ColumnBy, DateChoice, DatePreset, ReportOptions, ReportSpec};
use cl_model::{CommandId, Scope};
use eframe::egui::{self, Ui};
use serde_json::{Value, json};

use crate::company::{CompanyEvent, CompanyLink};
use crate::i18n::{t, t_with};
use crate::screens::invoice::engine::{self, Job};
use crate::theme::Tokens;
use crate::windows::{OpenPayload, Screen};

/// The editor's window kind.
pub const KIND: &str = "reports.builder_editor";

/// One editor window's identity: its result tab and a Customize from that
/// tab find it by this (Q-A08x).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EditorId(u64);

/// Hands out [`EditorId`]s.
static EDITORS: AtomicU64 = AtomicU64::new(1);

/// The open editors' report titles, by company and editor (QA-2): two
/// editors on the same title read "New report" and "New report 2" in their
/// tabs, the sidebar and their result tabs.
static OPEN_TITLES: std::sync::Mutex<Vec<(usize, u64, String)>> = std::sync::Mutex::new(Vec::new());

/// The open-titles list (a poisoned lock still holds the list).
fn open_titles() -> std::sync::MutexGuard<'static, Vec<(usize, u64, String)>> {
    OPEN_TITLES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl EditorId {
    /// A new editor's id.
    #[must_use]
    pub(crate) fn next() -> EditorId {
        EditorId(EDITORS.fetch_add(1, Ordering::Relaxed))
    }
}

/// What the editor opens on. `Debug` shows no captions or names (owner
/// data).
#[derive(Clone, PartialEq, Eq)]
pub enum BuilderSeed {
    /// Guided mode, blank (Q-A08a; Reports list "New report...").
    Guided,
    /// A blank report in Advanced.
    Blank,
    /// A ready-made report (`/starters`), in Advanced.
    Starter {
        /// Its key.
        key: String,
        /// Its definition.
        def: Box<BuilderDefinition>,
        /// Its title (the Report name when the definition has none).
        title: String,
    },
    /// Customize on a report window (Q-A08g, Q-A08k).
    Customize(Box<CustomizeSeed>),
    /// A09 WP-A: a report read from a `.clreport` file, not memorized yet:
    /// its Memorize is sent with `origin: 'import'`.
    Imported {
        /// The definition (this company's ids).
        def: Box<BuilderDefinition>,
        /// The name from the file (or the review's Rename).
        name: String,
        /// The matched group, for the Memorize dialog.
        group_id: Option<String>,
    },
}

/// Customize on a standard report (its mapped starter), a builder run or a
/// memorized report.
#[derive(Clone, PartialEq, Eq)]
pub struct CustomizeSeed {
    /// The definition to edit.
    pub def: BuilderDefinition,
    /// The Report name to start with (owner data): the memorized name, else
    /// the window's report title. The editor makes it the definition's
    /// title too (Q-A08r).
    pub title: String,
    /// Where it started.
    pub origin: Origin,
    /// The memorized row, as re-read for this action (Q-A07u(1)).
    pub saved: Option<Saved>,
    /// The window's dates (a preset, or session-only fixed dates).
    pub dates: Option<DateChoice>,
    /// The window's basis (`None`: the definition's).
    pub basis: Option<Basis>,
    /// The window's adjusting lens (`None`: the definition's).
    pub adjusting: Option<Adjusting>,
    /// The window had filters, which Customize doesn't carry: the editor
    /// says so (Q-A08k).
    pub filters_dropped: bool,
    /// The editor whose result tab this is: it takes the seed back in
    /// place (Q-A08x). `None`: a new editor.
    pub editor: Option<EditorId>,
}

impl std::fmt::Debug for BuilderSeed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BuilderSeed::Guided => f.write_str("Guided"),
            BuilderSeed::Blank => f.write_str("Blank"),
            BuilderSeed::Starter { key, .. } => f
                .debug_struct("Starter")
                .field("key", key)
                .finish_non_exhaustive(),
            BuilderSeed::Imported { group_id, .. } => f
                .debug_struct("Imported")
                .field("grouped", &group_id.is_some())
                .finish_non_exhaustive(),
            BuilderSeed::Customize(c) => f
                .debug_struct("Customize")
                .field("origin", &c.origin)
                .field("saved", &c.saved)
                .field("editor", &c.editor)
                .finish_non_exhaustive(),
        }
    }
}

/// Opens the editor on `seed` (a new window, or the editor named in a
/// Customize seed).
#[allow(dead_code, reason = "A08: WP-B3's Customize and New report call it")]
pub(crate) fn open(link: &CompanyLink, seed: BuilderSeed) {
    link.push(CompanyEvent::OpenWith(
        CommandId::new(KIND),
        Box::new(OpenPayload::BuilderEditor(Box::new(seed))),
    ));
}

/// The engine's date preset names and the app's presets (A07's mapping).
pub(crate) const PRESETS: [(DatePreset, &str); 9] = [
    (DatePreset::Today, "today"),
    (DatePreset::ThisMonth, "this_month"),
    (DatePreset::LastMonth, "last_month"),
    (DatePreset::ThisQuarter, "this_quarter"),
    (DatePreset::LastQuarter, "last_quarter"),
    (DatePreset::ThisYear, "this_year"),
    (DatePreset::LastYear, "last_year"),
    (DatePreset::YearToDate, "this_year_to_date"),
    (DatePreset::EndOfLastMonth, "end_of_last_month"),
];

/// The app's preset for an engine preset name.
pub(crate) fn preset_of(name: &str) -> Option<DatePreset> {
    match name {
        "year_to_date" => Some(DatePreset::YearToDate),
        _ => PRESETS.iter().find(|(_, n)| *n == name).map(|(p, _)| *p),
    }
}

/// The engine's name for an app preset.
pub(crate) fn preset_name(p: DatePreset) -> &'static str {
    PRESETS
        .iter()
        .find(|(x, _)| *x == p)
        .map_or("this_month", |(_, n)| n)
}

/// A text field the editor commits when it loses focus.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum FieldKey {
    /// The Report name.
    Name,
    /// A row's caption (a text row's text).
    Caption(String),
    /// A section's or group's total caption.
    TotalCaption(String),
    /// A renamed column header (its engine column key).
    Header(String),
    /// A section's amount filter value (section id, filter index).
    Amount(String, usize),
    /// Guided mode's Report name.
    GuidedName,
}

/// The outline's width (mockup 240–250).
const OUTLINE_W: f32 = 248.0;
/// The properties' width (mockup 300–320).
const PROPS_W: f32 = 312.0;
/// The preview's least width beside the other panes.
const PREVIEW_MIN: f32 = 360.0;

/// How the panes are laid out (H1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Layout {
    /// Side by side.
    Wide,
    /// Outline and properties side by side, the preview below.
    PreviewBelow,
    /// One above the other, scrolling.
    Stacked,
}

impl Layout {
    /// The layout for `width` points.
    pub(crate) fn for_width(width: f32) -> Layout {
        if width >= OUTLINE_W + PROPS_W + PREVIEW_MIN + 24.0 {
            Layout::Wide
        } else if width >= OUTLINE_W + 280.0 + 12.0 {
            Layout::PreviewBelow
        } else {
            Layout::Stacked
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Layout::Wide => "wide",
            Layout::PreviewBelow => "preview_below",
            Layout::Stacked => "stacked",
        }
    }
}

/// The three panes F6 moves between (Q-A08d).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Pane {
    /// The outline tree.
    Outline,
    /// The Row / Columns / Report tabs.
    Properties,
    /// The preview.
    Preview,
}

impl Pane {
    const ALL: [Pane; 3] = [Pane::Outline, Pane::Properties, Pane::Preview];

    const fn name(self) -> &'static str {
        match self {
            Pane::Outline => "outline",
            Pane::Properties => "properties",
            Pane::Preview => "preview",
        }
    }
}

/// The properties tabs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PropTab {
    /// The selected row.
    Row,
    /// The report's columns.
    Columns,
    /// Name, group, who may change it.
    Report,
}

/// Session dates: From/To typed over the preset (never saved, Q-A08q).
#[derive(Clone, Debug)]
pub(crate) struct Band {
    /// Custom dates, when typed.
    pub(crate) custom: Option<DateChoice>,
    pub(crate) from: crate::screens::company::form::DateInput,
    pub(crate) to: crate::screens::company::form::DateInput,
    /// From or To had the keyboard last frame: its text is left alone.
    pub(crate) typing: bool,
    /// The From field's widget (tests focus it).
    pub(crate) from_id: Option<egui::Id>,
    /// When the Dates list last changed the report's date choice (a
    /// type-ahead passing through it is taken back for Custom dates).
    pub(crate) picked_at: Option<f64>,
}

/// The editor window.
pub struct BuilderScreen {
    pub(crate) link: Arc<CompanyLink>,
    pub(crate) group: Option<RequestGroup>,
    pub(crate) today: CalendarDate,
    /// The open company (header name; the key of its fiscal calendar).
    pub(crate) open: Option<(cl_core::ids::TenantId, String)>,
    pub(crate) fiscal: FiscalYearSpec,
    calendar: cl_settings::FiscalCalendar,
    fiscal_job: Option<Job<Option<u8>>>,
    pub(crate) id: EditorId,
    /// The report being edited (Advanced; Guided builds into it).
    pub(crate) ed: BuilderEditor,
    /// Guided mode's answers, while Guided is shown.
    pub(crate) guided: Option<GuidedState>,
    pub(crate) tab: PropTab,
    pub(crate) pane: Pane,
    /// Focus to move this frame (F6, keys in the tree).
    pub(crate) focus_to: Option<Pane>,
    /// Focus to move next frame: a choice made with the keyboard in a
    /// widget that goes away this frame (the Add menu) can't hand the
    /// focus on in the same frame.
    pub(crate) focus_next: Option<Pane>,
    /// The outline tree had the keyboard last frame (its scope is on).
    pub(crate) outline_focused: bool,
    /// The row being renamed in place (F2): its id and the text so far.
    pub(crate) rename: Option<(String, String)>,
    /// The Add menu is open (Insert).
    pub(crate) add_menu: bool,
    /// The Add menu just opened: its first choice takes the keyboard.
    pub(crate) add_menu_new: bool,
    /// The Export menu is open (Excel, CSV, PDF).
    pub(crate) export_menu: bool,
    /// The Export menu just opened: its first choice takes the keyboard.
    pub(crate) export_menu_new: bool,
    /// Text being typed into a field, by field.
    pub(crate) drafts: HashMap<FieldKey, String>,
    /// The field with the keyboard, when it is one of ours.
    pub(crate) pending: Option<FieldKey>,
    /// One plain line about the last change (a refusal, a removal).
    pub(crate) status: Option<String>,
    pub(crate) band: Band,
    pub(crate) lists: lists::Lists,
    pub(crate) save: save::SaveState,
    pub(crate) preview: preview::Preview,
    /// The panes' layout last frame (the control channel).
    pub(crate) layout: Layout,
    /// The ready-made reports (`/starters`), once read.
    pub(crate) starters: Option<Vec<cl_api::resources::report_builder::Starter>>,
    starters_job: Option<Job<Vec<cl_api::resources::report_builder::Starter>>>,
    /// Guided's "Start from a ready-made report" list is open.
    pub(crate) starter_menu: bool,
    /// The list just opened: its first choice takes the keyboard.
    pub(crate) starter_menu_new: bool,
    /// Where Guided's Next button was drawn (tests click it).
    pub(crate) next_at: Option<egui::Rect>,
    /// Where each outline row was drawn last frame (tests click them).
    pub(crate) row_at: HashMap<String, egui::Rect>,
    /// An outline key (Tab, Shift+Tab, an arrow) acted on the tree this
    /// frame: egui must not also move the focus with it (QA-11).
    pub(crate) keep_focus: bool,
}

impl BuilderScreen {
    /// A new editor (Guided until a payload says otherwise).
    #[must_use]
    pub fn new(
        link: Arc<CompanyLink>,
        open: Option<(cl_core::ids::TenantId, String)>,
        today: CalendarDate,
    ) -> BuilderScreen {
        let group = link.group().and_then(Result::ok);
        let calendar = super::window::company_calendar(&link, open.as_ref());
        let fiscal_job = Some(super::window::fiscal_read(&link, group.as_ref()));
        let date = crate::screens::company::form::DateInput::new(today);
        BuilderScreen {
            link,
            group,
            today,
            open,
            fiscal: FiscalYearSpec::CALENDAR_YEAR,
            calendar,
            fiscal_job,
            id: EditorId::next(),
            ed: BuilderEditor::new(
                cl_model::report_def::editor::blank_definition(),
                "",
                Origin::Guided,
                None,
            ),
            guided: Some(GuidedState::new()),
            tab: PropTab::Row,
            pane: Pane::Outline,
            // A new editor takes the keyboard (AC6).
            focus_to: Some(Pane::Outline),
            focus_next: None,
            outline_focused: false,
            rename: None,
            add_menu: false,
            add_menu_new: false,
            export_menu: false,
            export_menu_new: false,
            drafts: HashMap::new(),
            pending: None,
            status: None,
            band: Band {
                custom: None,
                from: date.clone(),
                to: date,
                typing: false,
                from_id: None,
                picked_at: None,
            },
            lists: lists::Lists::default(),
            save: save::SaveState::default(),
            preview: preview::Preview::default(),
            layout: Layout::Wide,
            starters: None,
            starters_job: None,
            starter_menu: false,
            starter_menu_new: false,
            next_at: None,
            row_at: HashMap::new(),
            keep_focus: false,
        }
    }

    /// Customize from this editor's own result tab (Q-A08x): the editor
    /// comes forward as it is, its undo, its later edits and its saved row
    /// kept (R2). Only what the owner changed in the tab comes along: its
    /// date preset or basis, each one undo step; fixed From/To dates for
    /// the session.
    fn come_back(&mut self, c: &CustomizeSeed) {
        self.commit_pending();
        match c.dates {
            Some(DateChoice::Preset(p)) => {
                let mut d = self.ed.definition().dates.clone();
                d.preset = Some(preset_name(p).into());
                self.apply(Op::Dates(d));
                self.band.custom = None;
            }
            Some(fixed) => self.band.custom = Some(fixed),
            None => {}
        }
        if c.basis.is_some() && c.basis != self.ed.definition().basis {
            self.apply(Op::Basis(c.basis));
        }
        self.preview.edited(&self.ed);
    }

    /// The seed is a Customize from this editor's own result tab, on the
    /// report this editor holds (a tab memorized as a new report opens a
    /// new editor on that report).
    fn is_mine(&self, c: &CustomizeSeed) -> bool {
        c.editor == Some(self.id)
            && c.saved
                .as_ref()
                .is_none_or(|x| self.ed.saved().is_some_and(|m| m.id == x.id))
    }

    /// Opens on `seed`.
    fn seed(&mut self, seed: BuilderSeed) {
        if let BuilderSeed::Customize(c) = &seed
            && self.is_mine(c)
        {
            self.come_back(c);
            return;
        }
        self.guided = None;
        self.status = None;
        self.band.custom = None;
        match seed {
            BuilderSeed::Guided => {
                self.guided = Some(GuidedState::new());
                self.ed = BuilderEditor::new(
                    cl_model::report_def::editor::blank_definition(),
                    "",
                    Origin::Guided,
                    None,
                );
            }
            BuilderSeed::Blank => self.ed = BuilderEditor::blank(),
            BuilderSeed::Starter { key, def, title } => {
                let name = def.title.clone().unwrap_or(title);
                self.ed = BuilderEditor::new(*def, &name, Origin::Starter(key), None);
            }
            BuilderSeed::Imported {
                def,
                name,
                group_id,
            } => {
                self.ed = BuilderEditor::new(*def, &name, Origin::Imported, None);
                if group_id.is_some() {
                    let _ = self.ed.apply(Op::MemorizedGroup(group_id));
                }
                self.status = Some(t("report_file.builder.imported"));
            }
            BuilderSeed::Customize(c) => {
                let c = *c;
                let def = customized(c.def.clone(), &c);
                if let Some(fixed) = c.dates.filter(|d| !matches!(d, DateChoice::Preset(_))) {
                    self.band.custom = Some(fixed);
                }
                // A memorized standard report: its baseline is the starter
                // as the window showed it, so an untouched Customize has no
                // changes to save (R7).
                let saved = c.saved.clone().map(|mut s| {
                    if s.kind == cl_model::report_def::editor::SavedKind::Standard {
                        s.def = customized(s.def.clone(), &c);
                    }
                    s
                });
                self.ed = BuilderEditor::new(def, &c.title, c.origin, saved);
                if c.filters_dropped {
                    self.status = Some(t("builder.status.filters_dropped"));
                }
            }
        }
        self.tab = PropTab::Row;
        self.pane = Pane::Outline;
        self.focus_to = Some(Pane::Outline);
        self.preview.edited(&self.ed);
    }

    /// Makes one change; a refusal or a removal says what happened.
    pub(crate) fn apply(&mut self, op: Op) -> Option<Applied> {
        match self.ed.apply(op) {
            Ok(a) => {
                if let Some(r) = a.removed {
                    self.status = Some(removed_text(r));
                } else if a.changed {
                    self.status = None;
                }
                if a.changed {
                    self.preview.edited(&self.ed);
                }
                Some(a)
            }
            Err(r) => {
                self.status = Some(t(r.key()));
                None
            }
        }
    }

    /// Commits a text field.
    pub(crate) fn commit(&mut self, key: &FieldKey, text: String) {
        match key {
            FieldKey::Name => {
                self.apply(Op::Name(text));
            }
            FieldKey::Caption(id) => {
                self.apply(Op::Rename {
                    id: id.clone(),
                    text,
                });
            }
            FieldKey::TotalCaption(id) => {
                let node = tree::find(&self.ed.definition().nodes, id).cloned();
                let text = (!text.trim().is_empty()).then_some(text);
                match node {
                    Some(Node::Section(mut s)) => {
                        s.total_caption = text;
                        self.apply(Op::Edit(Node::Section(s)));
                    }
                    Some(Node::Group(mut g)) => {
                        g.total_caption = text;
                        self.apply(Op::Edit(Node::Group(g)));
                    }
                    _ => {}
                }
            }
            FieldKey::Header(k) => {
                let mut c = self.ed.definition().columns.clone();
                c.headers.insert(k.clone(), text);
                self.apply(Op::Columns(c));
            }
            FieldKey::Amount(id, i) => {
                let Some(Node::Section(mut s)) =
                    tree::find(&self.ed.definition().nodes, id).cloned()
                else {
                    return;
                };
                match amount_filter_value(&text) {
                    Some(v) => {
                        if let Some(f) = s.filters.get_mut(*i) {
                            f.value = Some(v);
                        }
                        self.apply(Op::Edit(Node::Section(s)));
                    }
                    None => self.status = Some(t("builder.status.not_an_amount")),
                }
            }
            FieldKey::GuidedName => {
                if let Some(g) = self.guided.as_mut() {
                    g.name = text.trim().to_string();
                    self.guided_changed();
                }
            }
        }
    }

    /// Commits the field being typed in (before an undo, a save or a run).
    pub(crate) fn commit_pending(&mut self) {
        if let Some(key) = self.pending.take()
            && let Some(text) = self.drafts.remove(&key)
        {
            let current = self.current_text(&key);
            if text != current {
                self.commit(&key, text);
            }
        }
    }

    /// What a field shows when it is not being typed in.
    fn current_text(&self, key: &FieldKey) -> String {
        let def = self.ed.definition();
        match key {
            FieldKey::Name => self.ed.name().to_string(),
            FieldKey::Caption(id) => match tree::find(&def.nodes, id) {
                Some(Node::Section(s)) => s.caption.clone(),
                Some(Node::Group(g)) => g.caption.clone(),
                Some(Node::Formula(f)) => f.caption.clone(),
                Some(Node::Text(x)) => x.text.clone(),
                _ => String::new(),
            },
            FieldKey::TotalCaption(id) => match tree::find(&def.nodes, id) {
                Some(Node::Section(s)) => s.total_caption.clone().unwrap_or_default(),
                Some(Node::Group(g)) => g.total_caption.clone().unwrap_or_default(),
                _ => String::new(),
            },
            FieldKey::Header(k) => def.columns.headers.get(k).cloned().unwrap_or_default(),
            FieldKey::Amount(id, i) => match tree::find(&def.nodes, id) {
                Some(Node::Section(s)) => s
                    .filters
                    .get(*i)
                    .and_then(|f| f.value.clone())
                    .unwrap_or_default(),
                _ => String::new(),
            },
            FieldKey::GuidedName => self
                .guided
                .as_ref()
                .map(|g| g.name.clone())
                .unwrap_or_default(),
        }
    }

    /// Guided's answers changed: the editor follows (the preview updates).
    pub(crate) fn guided_changed(&mut self) {
        let Some(g) = self.guided.as_ref() else {
            return;
        };
        let caption = g
            .source
            .map(|s| t(&format!("builder.source.{}", s.as_str())))
            .unwrap_or_default();
        if let Some(def) = g.to_definition(&caption) {
            let name = g.name.clone();
            // A guided report memorized from Done keeps its row: a later
            // answer is a change to it, never a new report (no "name taken"
            // on its own name).
            let saved = self.ed.saved().cloned();
            let origin = self.ed.origin().clone();
            let group = self.ed.group_id().map(str::to_string);
            self.ed = BuilderEditor::new(def, &name, origin, saved);
            if group.as_deref() != self.ed.group_id() {
                let _ = self.ed.apply(Op::MemorizedGroup(group));
            }
            self.preview.edited(&self.ed);
        }
    }

    /// Reads the ready-made reports once.
    pub(crate) fn load_starters(&mut self) {
        if self.starters.is_some() || self.starters_job.is_some() {
            return;
        }
        self.starters_job = Some(engine::read(
            &self.link,
            self.group.as_ref(),
            |_| crate::company::demo::report_builder::starters(),
            |s| async move { s.report_builder().starters().await },
        ));
    }

    /// A ready-made report opens in Advanced (Guided's list).
    pub(crate) fn open_starter(&mut self, key: String, def: BuilderDefinition, title: String) {
        self.seed(BuilderSeed::Starter {
            key,
            def: Box::new(def),
            title,
        });
    }

    /// Guided's "Open in Advanced": the answers become the report, one
    /// undo step (plan §3.2).
    pub(crate) fn open_in_advanced(&mut self) {
        self.commit_pending();
        let Some(g) = self.guided.take() else {
            return;
        };
        let caption = g
            .source
            .map(|s| t(&format!("builder.source.{}", s.as_str())))
            .unwrap_or_default();
        let def = g
            .to_definition(&caption)
            .unwrap_or_else(cl_model::report_def::editor::blank_definition);
        let mut ed = BuilderEditor::new(
            cl_model::report_def::editor::blank_definition(),
            "",
            Origin::Guided,
            None,
        );
        let _ = ed.apply(Op::Load {
            def: Box::new(def),
            name: g.name,
            group_id: None,
        });
        if let Some(first) = ed.definition().nodes.first().map(|n| n.id().to_string()) {
            ed.select(Some(&first));
        }
        self.ed = ed;
        self.tab = PropTab::Row;
        self.focus_to = Some(Pane::Outline);
        self.preview.edited(&self.ed);
    }

    /// "Guided view": offered only for the Guided shape (Q-A08l).
    pub(crate) fn show_guided(&mut self) {
        self.commit_pending();
        if let Some(g) = GuidedState::from_definition(self.ed.definition()) {
            self.guided = Some(g);
        }
    }

    /// The stand-in spec and window options a run of this report uses (the
    /// same as its Run report tab, AC8).
    pub(crate) fn run_options(&self) -> (&'static ReportSpec, ReportOptions) {
        let def = self.ed.definition();
        let spec = super::builder_ref::spec_for(def);
        let mut o = ReportOptions::for_spec(spec);
        o.dates = match self.band.custom {
            Some(c) => c,
            None => def
                .dates
                .preset
                .as_deref()
                .and_then(preset_of)
                .map_or(o.dates, DateChoice::Preset),
        };
        o.basis = def.basis;
        let fixed = std::ptr::eq(spec, &raw const cl_model::reports::BUILDER_AS_OF_TOTAL)
            || std::ptr::eq(spec, &raw const cl_model::reports::BUILDER_RANGE_TOTAL);
        o.columns = match def.columns.periods {
            Some(Periods::Month) if !fixed => ColumnBy::Month,
            Some(Periods::Quarter) if !fixed => ColumnBy::Quarter,
            Some(Periods::Year) if !fixed => ColumnBy::Year,
            _ => ColumnBy::Total,
        };
        (spec, o)
    }

    /// The report's dates as the band shows them.
    pub(crate) fn shown_dates(&self) -> Option<cl_core::report::DateRange> {
        let (_, o) = self.run_options();
        match o.dates {
            DateChoice::Preset(p) => p.range(&self.fiscal, self.today).ok(),
            DateChoice::Fixed { from, to } => Some(cl_core::report::DateRange { from, to }),
            DateChoice::AsOf(d) => Some(cl_core::report::DateRange { from: d, to: d }),
        }
    }

    /// "Custom dates": the dates shown now become this session's own
    /// (Q-A08q); From/To then take typing.
    pub(crate) fn choose_custom_dates(&mut self) {
        if let Some(r) = self.shown_dates() {
            self.band.custom = Some(if self.as_of() {
                DateChoice::AsOf(r.to)
            } else {
                DateChoice::Fixed {
                    from: r.from,
                    to: r.to,
                }
            });
            self.preview.edited(&self.ed);
        }
    }

    /// True when the report runs as of a date.
    pub(crate) fn as_of(&self) -> bool {
        self.ed.definition().dates.mode == DateMode::AsOf
    }

    /// Run report: the full report in its result tab (one per editor,
    /// Q-A08x), never the capped preview (Q-A08m).
    pub(crate) fn run_report(&mut self) {
        self.run_then(None);
    }

    /// Run report, then `then` in the tab: its export or print (Q-A08m).
    pub(crate) fn run_then(&mut self, then: Option<super::builder_ref::AfterRun>) {
        self.commit_pending();
        if !self.ed.can_run() {
            self.status = Some(t("builder.status.fix_first"));
            return;
        }
        let (_, options) = self.run_options();
        let open = super::builder_ref::BuilderRunOpen {
            editor: self.id,
            title: self.shown_title(),
            def: self.ed.definition().clone(),
            options,
            saved: self.ed.saved().cloned(),
            origin: self.ed.origin().clone(),
            then,
        };
        super::builder_run::show_result(&self.link, open);
    }

    /// The report's title as its tab shows it: the title, numbered when an
    /// earlier editor of this company has the same one ("New report 2",
    /// QA-2).
    pub(crate) fn shown_title(&self) -> String {
        let base = self.report_title();
        let company = Arc::as_ptr(&self.link) as usize;
        let mut open = open_titles();
        match open.iter_mut().find(|(_, id, _)| *id == self.id.0) {
            Some(e) => e.2.clone_from(&base),
            None => open.push((company, self.id.0, base.clone())),
        }
        let before = open
            .iter()
            .filter(|(c, id, t)| *c == company && *id < self.id.0 && *t == base)
            .count();
        if before == 0 {
            base
        } else {
            format!("{base} {}", before + 1)
        }
    }

    /// The report's title: its name, else "Report Builder".
    pub(crate) fn report_title(&self) -> String {
        if self.ed.name().is_empty() {
            t("builder.untitled")
        } else {
            self.ed.name().to_string()
        }
    }

    /// The three panes, laid out for the room there is (UI review H1,
    /// docs/design README rule 6): side by side when the preview keeps
    /// `PREVIEW_MIN`; the outline and properties above the preview when
    /// only they fit side by side; else all three stacked, scrolling (the
    /// minimum window at 200% zoom).
    fn panes(&mut self, ui: &mut Ui, tokens: &Tokens, room: f32) {
        // The window's width, taken before the toolbar and band drew (a
        // wide row there must not widen the panes).
        let w = ui.available_width().min(room);
        let h = ui.available_height();
        let layout = Layout::for_width(w);
        self.layout = layout;
        let pane = |ui: &mut Ui, size: egui::Vec2, f: &mut dyn FnMut(&mut Ui)| {
            ui.allocate_ui_with_layout(size, egui::Layout::top_down(egui::Align::Min), |ui| {
                ui.set_min_size(size);
                ui.set_max_size(size);
                f(ui);
            });
        };
        match layout {
            Layout::Wide => {
                ui.horizontal_top(|ui| {
                    ui.set_min_height(h);
                    pane(ui, egui::vec2(OUTLINE_W, h), &mut |ui| {
                        outline::draw(self, ui, tokens);
                    });
                    ui.separator();
                    pane(ui, egui::vec2(PROPS_W, h), &mut |ui| {
                        properties::draw(self, ui, tokens);
                    });
                    ui.separator();
                    let rest = ui.available_width();
                    pane(ui, egui::vec2(rest, h), &mut |ui| {
                        preview::draw(self, ui, tokens);
                    });
                });
            }
            // The whole screen scrolls (see `ui`): fixed heights here.
            Layout::PreviewBelow => {
                let _ = h;
                let top = 380.0;
                ui.horizontal_top(|ui| {
                    pane(ui, egui::vec2(OUTLINE_W, top), &mut |ui| {
                        outline::draw(self, ui, tokens);
                    });
                    ui.separator();
                    let rest = ui.available_width();
                    pane(ui, egui::vec2(rest, top), &mut |ui| {
                        properties::draw(self, ui, tokens);
                    });
                });
                ui.separator();
                let below = 420.0;
                pane(ui, egui::vec2(w, below), &mut |ui| {
                    preview::draw(self, ui, tokens);
                });
            }
            // The whole screen scrolls (see `ui`): fixed heights here.
            Layout::Stacked => {
                pane(ui, egui::vec2(w, 300.0), &mut |ui| {
                    outline::draw(self, ui, tokens);
                });
                ui.separator();
                pane(ui, egui::vec2(w, 420.0), &mut |ui| {
                    properties::draw(self, ui, tokens);
                });
                ui.separator();
                pane(ui, egui::vec2(w, 420.0), &mut |ui| {
                    preview::draw(self, ui, tokens);
                });
            }
        }
    }

    /// Moves focus to the next or previous pane (F6 / Shift+F6).
    fn cycle_pane(&mut self, forward: bool) {
        let at = Pane::ALL.iter().position(|p| *p == self.pane).unwrap_or(0);
        let n = Pane::ALL.len();
        let next = if forward {
            (at + 1) % n
        } else {
            (at + n - 1) % n
        };
        self.pane = Pane::ALL[next];
        self.focus_to = Some(self.pane);
    }

    /// The selected row, as an id.
    fn selected(&self) -> Option<String> {
        self.ed.selection().map(str::to_string)
    }

    fn row_op(&mut self, make: impl FnOnce(String) -> Op) {
        if let Some(id) = self.selected() {
            let changed = self.apply(make(id)).is_some_and(|a| a.changed);
            self.focus_to = Some(Pane::Outline);
            // A key that changed the tree is the tree's; one that did
            // nothing (Shift+Tab on a top-level row) still moves on, so the
            // keyboard can leave the outline.
            self.keep_focus |= changed;
        }
    }

    /// Starts renaming the selected row in place (F2).
    pub(crate) fn start_rename(&mut self) {
        let Some(id) = self.selected() else {
            return;
        };
        let text = self.current_text(&FieldKey::Caption(id.clone()));
        let named = matches!(
            tree::find(&self.ed.definition().nodes, &id),
            Some(Node::Section(_) | Node::Group(_) | Node::Formula(_) | Node::Text(_))
        );
        if named {
            self.rename = Some((id, text));
        }
    }

    fn poll(&mut self) {
        if let Some(r) = engine::poll(&mut self.fiscal_job) {
            // Without the company's year the calendar year stands in; the
            // engine still checks every run.
            if let Ok(start) = r {
                self.fiscal = crate::screens::home::charts::spec(start, self.calendar);
            }
            self.preview.edited(&self.ed);
        }
        if let Some(r) = engine::poll(&mut self.starters_job) {
            // An engine without starters: the list is empty and the button
            // says why.
            self.starters = Some(r.unwrap_or_default());
        }
        self.lists.poll();
        save::poll(self);
    }

    /// Control-channel state: no names, captions or amounts.
    fn state_json(&self) -> Value {
        let problems: Vec<String> = self.ed.problems().iter().map(|p| p.code.key()).collect();
        json!({
            "screen": "report_builder",
            "editor": self.id.0,
            "mode": if self.guided.is_some() { "guided" } else { "advanced" },
            "guided_step": self.guided.as_ref().map(|g| format!("{:?}", g.step).to_lowercase()),
            "rows": self.ed.row_count(),
            "selected": self.ed.selection(),
            "selected_depth": self.ed.selection().and_then(|id| {
                self.ed.visible_rows().into_iter().find(|r| r.id == id).map(|r| r.depth)
            }),
            "tab": format!("{:?}", self.tab).to_lowercase(),
            "pane": self.pane.name(),
            "layout": self.layout.name(),
            "outline_focused": self.outline_focused,
            "dirty": self.ed.dirty(),
            "can_run": self.ed.can_run(),
            "can_save_changes": self.ed.can_save_changes(),
            "can_undo": self.ed.can_undo(),
            "can_redo": self.ed.can_redo(),
            "problems": problems,
            "saved": self.ed.saved().map(|s| json!({"version": s.version, "can_edit": s.can_edit})),
            "save": self.save.state_name(),
            "preview": self.preview.state_json(),
        })
    }
}

/// A Customize seed's definition with the window's date preset, basis and
/// adjusting lens (Q-A08k). One rule for the report and, for a memorized
/// standard report, its baseline (R7).
fn customized(mut def: BuilderDefinition, c: &CustomizeSeed) -> BuilderDefinition {
    if let Some(DateChoice::Preset(p)) = c.dates {
        def.dates.preset = Some(preset_name(p).into());
    }
    if c.basis.is_some() {
        def.basis = c.basis;
    }
    if c.adjusting.is_some() {
        def.adjusting = c.adjusting;
    }
    def
}

/// The status line after a removal (Q-A08o).
fn removed_text(r: cl_model::report_def::editor::Removed) -> String {
    let mut s = crate::i18n::t_count("builder.status.removed_rows", r.rows, &[]);
    if r.terms > 0 {
        s.push(' ');
        s.push_str(&crate::i18n::t_count(
            "builder.status.removed_terms",
            r.terms,
            &[],
        ));
    }
    if r.percent_cleared {
        s.push(' ');
        s.push_str(&t("builder.status.percent_cleared"));
    }
    s
}

impl Screen for BuilderScreen {
    fn title(&self) -> String {
        t_with("builder.tab", &[("name", &self.shown_title())])
    }

    fn ui(&mut self, ui: &mut Ui) {
        if let Some(p) = self.focus_next.take() {
            self.focus_to = Some(p);
        }
        // egui read Tab / the arrows as a focus move before the shell ran
        // the outline's command for them: that move is cancelled (QA-11).
        if std::mem::take(&mut self.keep_focus) {
            ui.ctx()
                .memory_mut(|m| m.move_focus(egui::FocusDirection::None));
        }
        self.poll();
        let tokens = Tokens::of(ui.ctx());
        if self.preview.busy()
            || self.save.busy()
            || self.fiscal_job.is_some()
            || self.lists.busy()
            || self.starters_job.is_some()
        {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
        }
        preview::tick(self, ui);
        // A little room is kept at the right edge, so a wrapping row never
        // reaches under the window's edge (H1).
        // The shell can hand a rectangle wider than the window on a narrow
        // one (its least content width): only what is on screen counts.
        let on_screen =
            ui.clip_rect().intersect(ui.ctx().content_rect()).right() - ui.max_rect().left();
        let room = (ui.available_width().min(on_screen) - 16.0).max(200.0);
        let was_guided = self.guided.is_some();
        let area =
            egui::Rect::from_min_size(ui.max_rect().min, egui::vec2(room, ui.available_height()));
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(area)
                .layout(egui::Layout::top_down(egui::Align::Min)),
            |ui| {
                ui.set_clip_rect(ui.clip_rect().intersect(area.expand(8.0)));
                if self.guided.is_some() {
                    guided::draw(self, ui, &tokens);
                } else if Layout::for_width(room) != Layout::Wide {
                    // A narrow window (and the minimum one at 200% zoom):
                    // toolbar, band and panes scroll together, so nothing is
                    // squeezed or cut out of sight (H1).
                    egui::ScrollArea::vertical()
                        .id_salt(("builder-stacked", self.id.0))
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            chrome::toolbar(self, ui, &tokens);
                            chrome::band(self, ui, &tokens);
                            ui.separator();
                            self.panes(ui, &tokens, room);
                            // Tab and F6 bring the field into view (the
                            // shell does this for screens it scrolls).
                            crate::layout::keep_focus_in_view(ui);
                        });
                } else {
                    chrome::toolbar(self, ui, &tokens);
                    chrome::band(self, ui, &tokens);
                    ui.separator();
                    self.panes(ui, &tokens, room);
                }
            },
        );
        save::dialogs(self, ui, &tokens);
        // A switch between Guided and Advanced moves the keyboard on the
        // next frame, when the other mode is drawn.
        if self.guided.is_some() == was_guided {
            self.focus_to = None;
        } else {
            ui.ctx().request_repaint();
        }
    }

    fn dirty(&self) -> bool {
        // Guided builds into the editor: the same rule (a memorized guided
        // report is clean until the next answer).
        self.ed.dirty()
    }

    fn can_save(&self) -> bool {
        true
    }

    /// The close prompt's Save: Save changes when allowed, else Memorize
    /// (the window stays open until it is saved).
    fn save(&mut self) -> bool {
        self.commit_pending();
        if self.ed.can_save_changes() {
            save::save_changes(self, true);
        } else {
            save::open_memorize(self, true);
        }
        false
    }

    fn scope(&self) -> Scope {
        if self.save.dialog_open() {
            Scope::Window
        } else if self.guided.is_none() && self.outline_focused && self.rename.is_none() {
            Scope::Outline
        } else {
            Scope::Builder
        }
    }

    fn command(&mut self, id: CommandId) -> bool {
        let advanced = self.guided.is_none();
        match id.as_str() {
            // At 200 rows Insert says why instead of opening the menu (R15).
            "builder.add" if advanced && !self.ed.can_add() => {
                self.status = Some(t("builder.refused.too_many_rows"));
            }
            "builder.add" if advanced => {
                self.add_menu = true;
                self.add_menu_new = true;
            }
            "builder.remove" if advanced => self.row_op(Op::Remove),
            "builder.move_up" if advanced => self.row_op(Op::MoveUp),
            "builder.move_down" if advanced => self.row_op(Op::MoveDown),
            "builder.indent" if advanced => self.row_op(Op::Indent),
            "builder.outdent" if advanced => self.row_op(Op::Outdent),
            "builder.rename" if advanced => self.start_rename(),
            "builder.next_row" if advanced => {
                self.ed.select_step(true);
                self.focus_to = Some(Pane::Outline);
                self.keep_focus = true;
            }
            "builder.prev_row" if advanced => {
                self.ed.select_step(false);
                self.focus_to = Some(Pane::Outline);
                self.keep_focus = true;
            }
            "builder.fold" if advanced => {
                self.ed.fold_or_parent();
                self.focus_to = Some(Pane::Outline);
            }
            "builder.unfold" if advanced => {
                self.ed.unfold_or_child();
                self.focus_to = Some(Pane::Outline);
            }
            "builder.refresh_preview" => {
                self.commit_pending();
                self.preview.run_now();
            }
            "builder.undo" => {
                // Ctrl+Z in a text field: the field's text is a step first,
                // so undo reverts the field (Q-A08p).
                self.commit_pending();
                if self.ed.undo() {
                    self.preview.edited(&self.ed);
                }
            }
            "builder.redo" => {
                self.commit_pending();
                if self.ed.redo() {
                    self.preview.edited(&self.ed);
                }
            }
            "builder.next_pane" if advanced => self.cycle_pane(true),
            "builder.prev_pane" if advanced => self.cycle_pane(false),
            // Guided: the open step and the preview (AC6).
            "builder.next_pane" | "builder.prev_pane" => {
                self.pane = if self.pane == Pane::Preview {
                    Pane::Outline
                } else {
                    Pane::Preview
                };
                self.focus_to = Some(self.pane);
            }
            "builder.guided_back" => {
                if let Some(g) = self.guided.as_mut() {
                    g.go_back();
                }
                self.pane = Pane::Outline;
                self.focus_to = Some(Pane::Outline);
            }
            "builder.guided_next" => {
                self.commit_pending();
                if let Some(g) = self.guided.as_mut() {
                    g.go_next();
                }
                self.pane = Pane::Outline;
                self.focus_to = Some(Pane::Outline);
            }
            "builder.run_report" => self.run_report(),
            "file.print" => self.run_then(Some(super::builder_ref::AfterRun::Print)),
            "builder.save_changes" => {
                self.commit_pending();
                if self.ed.can_save_changes() {
                    save::save_changes(self, false);
                }
            }
            "reports.memorize" => {
                self.commit_pending();
                save::open_memorize(self, false);
            }
            _ => return false,
        }
        true
    }

    fn open_with(&mut self, payload: OpenPayload) -> bool {
        match payload {
            OpenPayload::BuilderEditor(seed) => {
                self.seed(*seed);
                true
            }
            _ => false,
        }
    }

    fn state(&self) -> Value {
        self.state_json()
    }

    fn take_close_request(&mut self) -> bool {
        std::mem::take(&mut self.save.close_now)
    }

    /// The editor lays out and scrolls its own panes (H1): the shell draws
    /// it straight into the window, so nothing scrolls the whole screen
    /// sideways.
    fn fixed_layout(&self) -> bool {
        true
    }

    fn save_in_progress(&self) -> bool {
        self.save.closing()
    }

    /// Only a text field keeps Delete (and Ctrl+A, C, V, X): a focused
    /// outline row is not one, so Delete removes it (QA-11).
    fn keeps_text_key(&self, ctx: &egui::Context, s: cl_model::Shortcut) -> bool {
        let _ = s;
        ctx.text_edit_focused()
    }

    fn takes_in_place(&self, payload: &OpenPayload) -> bool {
        matches!(
            payload,
            OpenPayload::BuilderEditor(seed)
                if matches!(&**seed, BuilderSeed::Customize(c) if self.is_mine(c))
        )
    }
}

impl Drop for BuilderScreen {
    fn drop(&mut self) {
        open_titles().retain(|(_, id, _)| *id != self.id.0);
    }
}

impl std::fmt::Debug for BuilderScreen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BuilderScreen")
            .field("id", &self.id)
            .field("guided", &self.guided.is_some())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests;
