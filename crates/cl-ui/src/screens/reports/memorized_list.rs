//! Memorized Reports (A09 S1, plan §3.1, Q-310, Q-A09e/f/k): every
//! memorized report, standard and Report Builder together, under the P28
//! groups, with its author. A tab window shaped like the Templates list
//! (no mockup draws it, Q-A09e): a toolbar, a search box, then the groups
//! and their reports.
//!
//! - **Open** (Enter, double-click) shows the report in a report window,
//!   as memorized; **Edit in Report Builder** opens a builder row in the
//!   editor.
//! - **Rename** (F2), **Move to group** and **Delete** (Ctrl+Delete, with a
//!   confirm) only when the row's `canEdit` (its author, an owner or an
//!   administrator, Q-310). Anyone else gets **Memorize a copy**, which
//!   makes `"<name> (copy)"` in the same group, theirs. The engine is the
//!   enforcer: 403 `NOT_AUTHOR` shows the same words and reloads.
//! - Groups: New group (Ctrl+N), Rename group, Delete group (its reports
//!   move to no group after a confirm). Anyone with report access manages
//!   them (Q-A09f).
//! - Every change carries the row's `version`; 409 `STALE_VERSION` reloads
//!   and asks the owner to try again. Every change publishes
//!   `Change::Reports` so open report windows list it at once.
//!
//! Nothing here reads or shows an amount.

use std::sync::Arc;

use cl_api::resources::reports::definitions::{
    DefinitionGroup, DefinitionPatch, DefinitionRefusal, DefinitionRow, DefinitionWrite,
    definition_refusal,
};
use cl_api::{ApiError, IdempotencyKey, OrphanLabel, RequestGroup};
use cl_core::CalendarDate;
use cl_core::ids::TenantId;
use cl_model::{CommandId, Scope};
use eframe::egui::{self, RichText, Ui};
use serde_json::{Value, json};

use crate::company::{CompanyEvent, CompanyLink};
use crate::i18n::{t, t_with};
use crate::layout::{Body, FixedFrame, FrameOpts};
use crate::screens::company::form;
use crate::screens::invoice::engine::{self, Job};
use crate::theme::Tokens;
use crate::windows::{OpenPayload, Screen};

/// The command and window kind.
pub const KIND: &str = "reports.memorized_list";

/// The engine's longest report or group name (`maxLength` 200).
const MAX_NAME: usize = 200;

/// A memorized report to show in a report window (the list's Open). Its
/// `Debug` names no report (owner data).
#[derive(Clone, PartialEq)]
pub struct MemorizedOpen(pub DefinitionRow);

// The definition JSON compares as JSON values do (exactly).
impl Eq for MemorizedOpen {}

impl std::fmt::Debug for MemorizedOpen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemorizedOpen")
            .field("kind", &self.0.kind)
            .finish_non_exhaustive()
    }
}

/// What is selected: a report or a group heading.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Selected {
    /// A memorized report (its id).
    Report(String),
    /// A group (its id).
    Group(String),
}

/// The open question or edit, shown as a card under the toolbar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Edit {
    /// Rename report `id`.
    Rename { id: String, name: String },
    /// Move report `id` to `group` (`None`: no group).
    Move { id: String, group: Option<String> },
    /// Delete report `id`?
    Delete { id: String },
    /// A new group's name.
    NewGroup { name: String },
    /// Rename group `id`.
    RenameGroup { id: String, name: String },
    /// Delete group `id`? Its reports move to no group.
    DeleteGroup { id: String },
}

impl Edit {
    fn kind(&self) -> &'static str {
        match self {
            Edit::Rename { .. } => "rename",
            Edit::Move { .. } => "move",
            Edit::Delete { .. } => "delete",
            Edit::NewGroup { .. } => "new_group",
            Edit::RenameGroup { .. } => "rename_group",
            Edit::DeleteGroup { .. } => "delete_group",
        }
    }
}

/// What the list reads.
type Loaded = (Vec<DefinitionRow>, Vec<DefinitionGroup>);

/// The name of a copy of `name`: `"<name> (copy)"`, then `"<name> (copy
/// 2)"` and on, the first no other report has (names compare as the
/// engine does, ignoring case). Kept within the engine's 200 characters.
#[must_use]
pub(crate) fn copy_name(name: &str, taken: &[&str]) -> String {
    let is_taken = |c: &str| {
        taken
            .iter()
            .any(|x| x.trim().to_lowercase() == c.to_lowercase())
    };
    let base = name.trim();
    for n in 1u32.. {
        let suffix = if n == 1 {
            t("reports.memorized.copy_suffix")
        } else {
            t_with("reports.memorized.copy_suffix_n", &[("n", &n.to_string())])
        };
        let room = MAX_NAME.saturating_sub(suffix.chars().count() + 1);
        let head: String = base.chars().take(room).collect();
        let candidate = format!("{} {suffix}", head.trim_end());
        if !is_taken(&candidate) {
            return candidate;
        }
    }
    // `1u32..` ends only past u32::MAX names (invariant).
    base.to_string()
}

/// A builder body with its title set to `name` (Q-A08r: a builder
/// report's name is its title).
fn titled(definition: &Value, name: &str) -> Value {
    let mut body = definition.clone();
    if body["kind"] == "builder"
        && let Some(obj) = body.as_object_mut()
    {
        obj.insert("title".into(), Value::from(name));
    }
    body
}

/// The words for a refusal of a list change, as an i18n key.
fn refusal_key(e: &ApiError) -> &'static str {
    match definition_refusal(e) {
        Some(DefinitionRefusal::Stale) => "reports.memorized.stale",
        Some(DefinitionRefusal::NotAuthor) => "reports.memorized.not_author",
        Some(DefinitionRefusal::NameTaken) => "reports.memorize.name_taken",
        Some(DefinitionRefusal::TooLarge | DefinitionRefusal::Invalid { .. }) => {
            "reports.memorize.invalid"
        }
        None => match e {
            ApiError::NotFound { .. } => "reports.memorized.gone",
            _ => "reports.memorized.failed",
        },
    }
}

/// The Memorized Reports window.
pub struct MemorizedList {
    link: Arc<CompanyLink>,
    tenant: Option<(TenantId, String)>,
    group: Option<RequestGroup>,
    pub(crate) rows: Vec<DefinitionRow>,
    pub(crate) groups: Vec<DefinitionGroup>,
    pub(crate) loaded: bool,
    load: Option<Job<Loaded>>,
    /// The change in flight; its answer is what to select after.
    change: Option<(&'static str, Job<Option<Selected>>)>,
    pub(crate) selected: Option<Selected>,
    pub(crate) search: String,
    pub(crate) edit: Option<Edit>,
    /// A refusal shown under the edit's name field (`NAME_TAKEN`).
    pub(crate) problem: Option<&'static str>,
    pub(crate) notice: Option<String>,
    /// The last change that worked (scenarios).
    pub(crate) last_action: Option<&'static str>,
    /// The edit card's field takes the focus once.
    focus_edit: bool,
    bus: crate::data_bus::Subscription,
    layout: crate::layout::FrameReport,
}

impl MemorizedList {
    /// The list of the open company.
    #[must_use]
    pub fn new(link: Arc<CompanyLink>, tenant: Option<(TenantId, String)>) -> MemorizedList {
        let group = link.group().and_then(Result::ok);
        let mut s = MemorizedList {
            bus: link.bus().subscribe(),
            link,
            tenant,
            group,
            rows: Vec::new(),
            groups: Vec::new(),
            loaded: false,
            load: None,
            change: None,
            selected: None,
            search: String::new(),
            edit: None,
            problem: None,
            notice: None,
            last_action: None,
            focus_edit: false,
            layout: crate::layout::FrameReport::default(),
        };
        s.reload();
        s.poll();
        s
    }

    fn available(&self) -> bool {
        super::memorized::available(&self.link)
    }

    pub(crate) fn reload(&mut self) {
        if !self.available() {
            self.loaded = true;
            return;
        }
        self.load = Some(engine::read(
            &self.link,
            self.group.as_ref(),
            |d| Ok(crate::company::demo::report_store::definitions(d)),
            |s| async move {
                let d = s.report_definitions();
                Ok((d.list().await?, d.groups().await?))
            },
        ));
    }

    pub(crate) fn poll(&mut self) {
        if let Some(r) = engine::poll(&mut self.load) {
            self.loaded = true;
            match r {
                Ok((rows, mut groups)) => {
                    groups.sort_by(|a, b| a.sort.cmp(&b.sort).then(a.name.cmp(&b.name)));
                    self.rows = rows;
                    self.groups = groups;
                    // A selection that went away goes too.
                    if !self.selection_exists() {
                        self.selected = None;
                    }
                }
                Err(_) => self.notice = Some(t("reports.memorized.load_failed")),
            }
        }
        let done = match self.change.as_mut() {
            Some((what, job)) => job.take().map(|r| (*what, r)),
            None => None,
        };
        if let Some((what, r)) = done {
            self.change = None;
            self.changed(what, r);
        }
    }

    /// A change answered: select what it made, reload, tell other windows;
    /// or show why not.
    fn changed(&mut self, what: &'static str, r: Result<Option<Selected>, ApiError>) {
        match r {
            Ok(sel) => {
                self.edit = None;
                self.problem = None;
                self.notice = None;
                self.last_action = Some(what);
                if sel.is_some() {
                    self.selected = sel;
                }
                self.reload();
                self.link
                    .bus()
                    .publish_from(&self.bus, crate::data_bus::Change::Reports);
            }
            Err(e) => {
                let key = refusal_key(&e);
                // A taken name stays on the card so the owner types
                // another; anything else closes it and reloads.
                if key == "reports.memorize.name_taken"
                    && matches!(
                        self.edit,
                        Some(
                            Edit::Rename { .. } | Edit::NewGroup { .. } | Edit::RenameGroup { .. }
                        )
                    )
                {
                    self.problem = Some(if matches!(self.edit, Some(Edit::Rename { .. })) {
                        key
                    } else {
                        "reports.memorized.group_taken"
                    });
                    return;
                }
                self.edit = None;
                self.problem = None;
                self.notice = Some(match key {
                    "reports.memorized.not_author" => self.gate_text(),
                    k => t(k),
                });
                self.reload();
            }
        }
    }

    fn selection_exists(&self) -> bool {
        match &self.selected {
            Some(Selected::Report(id)) => self.rows.iter().any(|r| r.id == *id),
            Some(Selected::Group(id)) => self.groups.iter().any(|g| g.id == *id),
            None => true,
        }
    }

    pub(crate) fn selected_row(&self) -> Option<&DefinitionRow> {
        match &self.selected {
            Some(Selected::Report(id)) => self.rows.iter().find(|r| r.id == *id),
            _ => None,
        }
    }

    fn selected_group(&self) -> Option<&DefinitionGroup> {
        match &self.selected {
            Some(Selected::Group(id)) => self.groups.iter().find(|g| g.id == *id),
            _ => None,
        }
    }

    fn busy(&self) -> bool {
        self.load.is_some() || self.change.is_some()
    }

    /// Why the selected report can't be changed here (Q-310), or `None`.
    pub(crate) fn gate(&self) -> Option<String> {
        self.selected_row()
            .filter(|r| !r.can_edit)
            .map(|_| self.gate_text())
    }

    fn gate_text(&self) -> String {
        let author = self
            .selected_row()
            .and_then(|r| r.created_by_name.clone())
            .unwrap_or_else(|| t("reports.memorized.its_author"));
        t_with("reports.memorized.gated", &[("author", &author)])
    }

    /// The search's words (every word in the name, as the catalog).
    fn matches(&self, r: &DefinitionRow) -> bool {
        let name = r.name.to_lowercase();
        self.search
            .split_whitespace()
            .all(|w| name.contains(&w.to_lowercase()))
    }

    /// The groups in order, then "no group", each with its shown reports.
    /// A group with no report that matches shows only while not searching.
    fn sections(&self) -> Vec<(Option<&DefinitionGroup>, Vec<&DefinitionRow>)> {
        let searching = !self.search.trim().is_empty();
        let mut out = Vec::new();
        let mut keys: Vec<Option<&DefinitionGroup>> = self.groups.iter().map(Some).collect();
        keys.push(None);
        for g in keys {
            let mut rows: Vec<&DefinitionRow> = self
                .rows
                .iter()
                .filter(|r| {
                    let gid = r.group_id.as_deref();
                    let in_group = match g {
                        Some(g) => gid == Some(g.id.as_str()),
                        // A row naming a group that is gone shows here.
                        None => gid.is_none_or(|id| !self.groups.iter().any(|x| x.id == id)),
                    };
                    in_group && self.matches(r)
                })
                .collect();
            rows.sort_by_key(|r| r.name.to_lowercase());
            if rows.is_empty() && (searching || g.is_none()) {
                continue;
            }
            out.push((g, rows));
        }
        out
    }

    /// Everything the arrows step through, top to bottom.
    fn order(&self) -> Vec<Selected> {
        let mut out = Vec::new();
        for (g, rows) in self.sections() {
            if let Some(g) = g {
                out.push(Selected::Group(g.id.clone()));
            }
            out.extend(rows.iter().map(|r| Selected::Report(r.id.clone())));
        }
        out
    }

    fn step(&mut self, down: bool) {
        let order = self.order();
        if order.is_empty() {
            return;
        }
        let at = self
            .selected
            .as_ref()
            .and_then(|s| order.iter().position(|x| x == s));
        let next = match (at, down) {
            (None, _) => 0,
            (Some(i), true) => (i + 1).min(order.len() - 1),
            (Some(i), false) => i.saturating_sub(1),
        };
        self.selected = Some(order[next].clone());
    }

    // ------------------------------------------------------------ actions

    /// Opens the selected report in a report window (Enter).
    pub(crate) fn open_selected(&mut self) {
        let Some(row) = self.selected_row().cloned() else {
            return;
        };
        self.link.push(CompanyEvent::OpenWith(
            CommandId::new(super::KIND),
            Box::new(OpenPayload::Memorized(Box::new(MemorizedOpen(row)))),
        ));
    }

    /// Opens the selected builder report in the Report Builder.
    pub(crate) fn edit_selected(&mut self) {
        let Some(row) = self.selected_row().cloned() else {
            return;
        };
        if row.kind != "builder" {
            return;
        }
        if let Err(key) = super::builder_run::edit_row(&self.link, &row) {
            self.notice = Some(t(key));
        }
    }

    /// Starts an edit of the selected report, unless the row is gated.
    fn start(&mut self, make: impl FnOnce(&DefinitionRow) -> Edit) {
        if self.busy() {
            return;
        }
        let Some(row) = self.selected_row() else {
            return;
        };
        if !row.can_edit {
            self.notice = Some(self.gate_text());
            return;
        }
        self.edit = Some(make(row));
        self.problem = None;
        self.focus_edit = true;
    }

    pub(crate) fn ask_rename(&mut self) {
        if let Some(g) = self.selected_group().cloned() {
            if !self.busy() {
                self.edit = Some(Edit::RenameGroup {
                    id: g.id,
                    name: g.name,
                });
                self.problem = None;
                self.focus_edit = true;
            }
            return;
        }
        self.start(|r| Edit::Rename {
            id: r.id.clone(),
            name: r.name.clone(),
        });
    }

    pub(crate) fn ask_move(&mut self) {
        self.start(|r| Edit::Move {
            id: r.id.clone(),
            group: r.group_id.clone(),
        });
    }

    pub(crate) fn ask_delete(&mut self) {
        if let Some(g) = self.selected_group().cloned() {
            if !self.busy() {
                self.edit = Some(Edit::DeleteGroup { id: g.id });
                self.problem = None;
            }
            return;
        }
        self.start(|r| Edit::Delete { id: r.id.clone() });
    }

    pub(crate) fn ask_new_group(&mut self) {
        if !self.busy() {
            self.edit = Some(Edit::NewGroup {
                name: String::new(),
            });
            self.problem = None;
            self.focus_edit = true;
        }
    }

    /// Sends the edit on the card (its OK button, or Enter in its field).
    pub(crate) fn confirm(&mut self) {
        if self.busy() {
            return;
        }
        let Some(edit) = self.edit.clone() else {
            return;
        };
        match edit {
            Edit::Rename { id, name } => self.rename(&id, &name),
            Edit::Move { id, group } => self.move_to(&id, group),
            Edit::Delete { id } => self.delete(&id),
            Edit::NewGroup { name } => self.create_group(&name),
            Edit::RenameGroup { id, name } => self.rename_group(&id, &name),
            Edit::DeleteGroup { id } => self.delete_group(&id),
        }
    }

    fn patch(&mut self, what: &'static str, row: &DefinitionRow, p: DefinitionPatch) {
        let (id, version, kind) = (row.id.clone(), row.version, row.kind.clone());
        let sel = Some(Selected::Report(id.clone()));
        let job = engine::write(
            &self.link,
            self.group.as_ref(),
            OrphanLabel {
                what: "orphan.report_memorize",
                key: None,
            },
            {
                let (id, p, sel) = (id.clone(), p.clone(), sel.clone());
                move |d| {
                    crate::company::demo::report_store::patch_definition(d, &id, version, &p)
                        .map(|_| sel)
                }
            },
            move |s| async move {
                s.report_definitions()
                    .patch(&id, version, &kind, &p)
                    .await
                    .map(|_| sel)
            },
        );
        self.change = Some((what, job));
        self.poll();
    }

    fn rename(&mut self, id: &str, name: &str) {
        let Some(row) = self.rows.iter().find(|r| r.id == id).cloned() else {
            return;
        };
        let name = name.trim().to_string();
        if name.is_empty() {
            self.problem = Some("reports.memorize.name_needed");
            return;
        }
        // A builder row's title follows its name (Q-A08r); a standard row
        // sends its name only.
        let definition = (row.kind == "builder").then(|| titled(&row.definition, &name));
        self.patch(
            "renamed",
            &row,
            DefinitionPatch {
                name: Some(name),
                group_id: None,
                definition,
            },
        );
    }

    fn move_to(&mut self, id: &str, group: Option<String>) {
        let Some(row) = self.rows.iter().find(|r| r.id == id).cloned() else {
            return;
        };
        if row.group_id == group {
            self.edit = None;
            return;
        }
        self.patch(
            "moved",
            &row,
            DefinitionPatch {
                group_id: Some(group),
                ..DefinitionPatch::default()
            },
        );
    }

    fn delete(&mut self, id: &str) {
        let Some(version) = self.rows.iter().find(|r| r.id == id).map(|r| r.version) else {
            return;
        };
        let id = id.to_string();
        let job = engine::write(
            &self.link,
            self.group.as_ref(),
            OrphanLabel {
                what: "orphan.report_memorize",
                key: None,
            },
            {
                let id = id.clone();
                move |d| {
                    crate::company::demo::report_store::delete_definition(d, &id, version)
                        .map(|()| None)
                }
            },
            move |s| async move {
                s.report_definitions()
                    .delete(&id, version)
                    .await
                    .map(|()| None)
            },
        );
        self.selected = None;
        self.change = Some(("deleted", job));
        self.poll();
    }

    /// Memorize a copy (Q-310): the selected report as a new one of mine,
    /// named `"<name> (copy)"` with the next free suffix, in its group.
    pub(crate) fn memorize_copy(&mut self) {
        if self.busy() {
            return;
        }
        let Some(row) = self.selected_row().cloned() else {
            return;
        };
        let taken: Vec<&str> = self.rows.iter().map(|r| r.name.as_str()).collect();
        let name = copy_name(&row.name, &taken);
        let write = DefinitionWrite {
            definition: titled(&row.definition, &name),
            name,
            group_id: row.group_id.clone(),
            kind: row.kind.clone(),
        };
        // One key per press: a retried send stores one copy.
        let key = IdempotencyKey::new();
        let job = engine::write(
            &self.link,
            self.group.as_ref(),
            OrphanLabel {
                what: "orphan.report_memorize",
                key: None,
            },
            {
                let write = write.clone();
                move |d| {
                    crate::company::demo::report_store::save_definition(d, None, &write)
                        .map(|r| Some(Selected::Report(r.id)))
                }
            },
            move |s| async move {
                s.report_definitions()
                    .create(&write, key)
                    .await
                    .map(|r| Some(Selected::Report(r.id)))
            },
        );
        self.edit = None;
        self.change = Some(("copied", job));
        self.poll();
    }

    fn create_group(&mut self, name: &str) {
        let name = name.trim().to_string();
        if name.is_empty() {
            self.problem = Some("reports.memorized.group_name_needed");
            return;
        }
        let job = engine::write(
            &self.link,
            self.group.as_ref(),
            OrphanLabel {
                what: "orphan.report_memorize",
                key: None,
            },
            {
                let name = name.clone();
                move |d| {
                    crate::company::demo::report_store::create_group(d, &name)
                        .map(|g| Some(Selected::Group(g.id)))
                }
            },
            move |s| async move {
                s.report_definitions()
                    .create_group(&name)
                    .await
                    .map(|g| Some(Selected::Group(g.id)))
            },
        );
        self.change = Some(("group_added", job));
        self.poll();
    }

    fn rename_group(&mut self, id: &str, name: &str) {
        let Some(g) = self.groups.iter().find(|g| g.id == id).cloned() else {
            return;
        };
        let name = name.trim().to_string();
        if name.is_empty() {
            self.problem = Some("reports.memorized.group_name_needed");
            return;
        }
        let job = engine::write(
            &self.link,
            self.group.as_ref(),
            OrphanLabel {
                what: "orphan.report_memorize",
                key: None,
            },
            {
                let (id, name) = (g.id.clone(), name.clone());
                move |d| {
                    crate::company::demo::report_store::rename_group(d, &id, g.version, &name)
                        .map(|g| Some(Selected::Group(g.id)))
                }
            },
            move |s| async move {
                s.report_definitions()
                    .update_group(&g.id, g.version, &name, g.sort)
                    .await
                    .map(|g| Some(Selected::Group(g.id)))
            },
        );
        self.change = Some(("group_renamed", job));
        self.poll();
    }

    fn delete_group(&mut self, id: &str) {
        let Some(g) = self.groups.iter().find(|g| g.id == id).cloned() else {
            return;
        };
        let job = engine::write(
            &self.link,
            self.group.as_ref(),
            OrphanLabel {
                what: "orphan.report_memorize",
                key: None,
            },
            {
                let id = g.id.clone();
                move |d| {
                    crate::company::demo::report_store::delete_group(d, &id, g.version)
                        .map(|()| None)
                }
            },
            move |s| async move {
                s.report_definitions()
                    .delete_group(&g.id, g.version)
                    .await
                    .map(|()| None)
            },
        );
        self.selected = None;
        self.change = Some(("group_deleted", job));
        self.poll();
    }

    // ------------------------------------------------------------ drawing

    fn draw(&mut self, ui: &mut Ui) {
        let tokens = Tokens::of(ui.ctx());
        if !self.available() {
            form::hint(ui, &tokens, &t("reports.memorized.unavailable"));
            return;
        }
        if !self.loaded {
            form::progress_line(ui, &tokens, &t("reports.memorized.loading"));
            return;
        }
        let mut frame = FixedFrame::begin(ui, KIND, FrameOpts::default());
        frame.top(|ui| {
            self.toolbar(ui, &tokens);
            self.messages(ui, &tokens);
        });
        frame.body(Body::Scroll, |ui| {
            if self.rows.is_empty() && self.groups.is_empty() {
                ui.add_space(12.0);
                form::hint(ui, &tokens, &t("reports.memorized.empty"));
            } else {
                self.table(ui, &tokens);
            }
        });
        self.layout = frame.end();
    }

    fn toolbar(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let idle = !self.busy() && self.edit.is_none();
        let row = self.selected_row().cloned();
        let group = self.selected_group().is_some();
        let editable = row.as_ref().is_some_and(|r| r.can_edit);
        let gate = self.gate().unwrap_or_default();
        let mut click: Option<&str> = None;
        crate::forms::header::toolbar(ui, tokens, false, "", |ui| {
            let mut b = |ui: &mut Ui, key: &'static str, on: bool, tip: &str, off_tip: &str| {
                let r = ui
                    .add_enabled(on && idle, egui::Button::new(t(key)))
                    .on_hover_text(tip);
                let r = if off_tip.is_empty() {
                    r
                } else {
                    r.on_disabled_hover_text(off_tip)
                };
                if r.clicked() {
                    click = Some(key);
                }
            };
            let some = row.is_some();
            b(
                ui,
                "reports.memorized.open",
                some,
                &t("reports.memorized.open.tip"),
                "",
            );
            b(
                ui,
                "reports.memorized.edit",
                row.as_ref().is_some_and(|r| r.kind == "builder"),
                &t("reports.memorized.edit.tip"),
                "",
            );
            let gated = if some && !editable { gate.as_str() } else { "" };
            b(
                ui,
                "reports.memorized.rename",
                editable || group,
                &t("reports.memorized.rename.tip"),
                gated,
            );
            b(
                ui,
                "reports.memorized.move",
                editable,
                &t("reports.memorized.move.tip"),
                gated,
            );
            b(
                ui,
                "reports.memorized.delete",
                editable || group,
                &t("reports.memorized.delete.tip"),
                gated,
            );
            b(
                ui,
                "reports.memorized.copy",
                some,
                &t("reports.memorized.copy.tip"),
                "",
            );
            ui.separator();
            b(
                ui,
                "reports.memorized.new_group",
                true,
                &t("reports.memorized.new_group.tip"),
                "",
            );
            None::<()>
        });
        match click {
            Some("reports.memorized.open") => self.open_selected(),
            Some("reports.memorized.edit") => self.edit_selected(),
            Some("reports.memorized.rename") => self.ask_rename(),
            Some("reports.memorized.move") => self.ask_move(),
            Some("reports.memorized.delete") => self.ask_delete(),
            Some("reports.memorized.copy") => self.memorize_copy(),
            Some("reports.memorized.new_group") => self.ask_new_group(),
            _ => {}
        }
        let find = t("reports.find");
        ui.horizontal(|ui| {
            let l = ui.label(RichText::new(&find).size(12.0).color(tokens.text_secondary));
            ui.add(
                egui::TextEdit::singleline(&mut self.search)
                    .hint_text(&find)
                    .desired_width(240.0),
            )
            .labelled_by(l.id);
        });
    }

    fn messages(&mut self, ui: &mut Ui, tokens: &Tokens) {
        if let Some(n) = &self.notice {
            form::inline_warning(ui, tokens, n);
        }
        // Keyboard users can't hover a disabled button: the reason shows
        // for the selected row too (Q-310).
        if let Some(g) = self.gate() {
            form::hint(ui, tokens, &g);
        }
        if self.busy() && self.change.is_some() {
            form::progress_line(ui, tokens, &t("reports.memorized.saving"));
        }
        let Some(edit) = self.edit.clone() else {
            return;
        };
        let groups = self.groups.clone();
        let problem = self.problem;
        let focus = std::mem::take(&mut self.focus_edit);
        let mut answer = None;
        form::card(ui, tokens, |ui| {
            let mut field = |ui: &mut Ui, label: &str, text: &mut String| {
                ui.horizontal(|ui| {
                    let l = ui.label(label);
                    let r = ui
                        .add(egui::TextEdit::singleline(text).desired_width(280.0))
                        .labelled_by(l.id);
                    if focus {
                        r.request_focus();
                    }
                    if form::activated(&r) {
                        answer = Some(true);
                    }
                });
            };
            let mut next = edit.clone();
            let (ok, ok_destructive) = match &mut next {
                Edit::Rename { name, .. } => {
                    field(ui, &t("reports.memorized.name"), name);
                    ("reports.memorized.rename_ok", false)
                }
                Edit::NewGroup { name } => {
                    field(ui, &t("reports.memorized.group_name"), name);
                    ("reports.memorized.new_group_ok", false)
                }
                Edit::RenameGroup { name, .. } => {
                    field(ui, &t("reports.memorized.group_name"), name);
                    ("reports.memorized.rename_ok", false)
                }
                Edit::Move { group, .. } => {
                    let shown = group
                        .as_ref()
                        .and_then(|id| groups.iter().find(|g| &g.id == id))
                        .map_or_else(|| t("reports.memorize.no_group"), |g| g.name.clone());
                    ui.horizontal(|ui| {
                        let l = ui.label(t("reports.memorize.group"));
                        egui::ComboBox::from_id_salt("memorized_list_move")
                            .selected_text(shown)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(group, None, t("reports.memorize.no_group"));
                                for g in &groups {
                                    ui.selectable_value(group, Some(g.id.clone()), &g.name);
                                }
                            })
                            .response
                            .labelled_by(l.id);
                    });
                    ("reports.memorized.move_ok", false)
                }
                Edit::Delete { id } => {
                    let name = self
                        .rows
                        .iter()
                        .find(|r| r.id == *id)
                        .map(|r| r.name.clone())
                        .unwrap_or_default();
                    ui.label(t_with(
                        "reports.memorized.delete_question",
                        &[("name", &name)],
                    ));
                    ("reports.memorized.delete_ok", true)
                }
                Edit::DeleteGroup { id } => {
                    let name = groups
                        .iter()
                        .find(|g| g.id == *id)
                        .map(|g| g.name.clone())
                        .unwrap_or_default();
                    let n = self
                        .rows
                        .iter()
                        .filter(|r| r.group_id.as_deref() == Some(id.as_str()))
                        .count();
                    ui.label(crate::i18n::t_count(
                        "reports.memorized.delete_group_question",
                        n,
                        &[("name", &name), ("n", &n.to_string())],
                    ));
                    ("reports.memorized.delete_group_ok", true)
                }
            };
            if let Some(p) = problem {
                form::inline_warning(ui, tokens, &t(p));
            }
            ui.horizontal(|ui| {
                let busy = self.change.is_some();
                let r = if ok_destructive {
                    ui.add_enabled_ui(!busy, |ui| crate::widgets::destructive_button(ui, &t(ok)))
                        .inner
                } else {
                    ui.add_enabled(!busy, egui::Button::new(t(ok)))
                };
                if r.clicked() {
                    answer = Some(true);
                }
                if ui.button(t("reports.memorized.cancel")).clicked() {
                    answer = Some(false);
                }
            });
            self.edit = Some(next);
        });
        match answer {
            Some(true) => self.confirm(),
            Some(false) => {
                self.edit = None;
                self.problem = None;
            }
            None => {}
        }
    }

    fn table(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let sections: Vec<(Option<DefinitionGroup>, Vec<DefinitionRow>)> = self
            .sections()
            .into_iter()
            .map(|(g, rows)| (g.cloned(), rows.into_iter().cloned().collect()))
            .collect();
        if sections.is_empty() {
            form::hint(ui, tokens, &t("reports.find.none"));
            return;
        }
        let mut picked = None;
        let mut opened = None;
        egui::Grid::new("memorized_list/table")
            .num_columns(4)
            .striped(true)
            .spacing([16.0, 6.0])
            .show(ui, |ui| {
                for key in [
                    "reports.memorized.col.name",
                    "reports.memorized.col.report",
                    "reports.memorized.col.author",
                    "reports.memorized.col.changed",
                ] {
                    ui.label(RichText::new(t(key)).strong().color(tokens.text_secondary));
                }
                ui.end_row();
                for (g, rows) in &sections {
                    match g {
                        Some(g) => {
                            let sel = Selected::Group(g.id.clone());
                            let on = self.selected.as_ref() == Some(&sel);
                            if ui
                                .selectable_label(on, RichText::new(&g.name).strong())
                                .clicked()
                            {
                                picked = Some(sel);
                            }
                        }
                        None => {
                            ui.label(
                                RichText::new(t("reports.memorized.no_group"))
                                    .strong()
                                    .color(tokens.text_secondary),
                            );
                        }
                    }
                    ui.end_row();
                    for r in rows {
                        let sel = Selected::Report(r.id.clone());
                        let on = self.selected.as_ref() == Some(&sel);
                        let resp = ui.selectable_label(on, format!("    {}", r.name));
                        if resp.clicked() {
                            picked = Some(sel.clone());
                        }
                        if resp.double_clicked() {
                            opened = Some(sel);
                        }
                        ui.label(report_title(r));
                        ui.label(
                            r.created_by_name
                                .clone()
                                .unwrap_or_else(|| "\u{2014}".into()),
                        );
                        ui.label(r.updated_at.map(changed_text).unwrap_or_default());
                        ui.end_row();
                    }
                }
            });
        if let Some(s) = picked {
            self.selected = Some(s);
        }
        if let Some(s) = opened {
            self.selected = Some(s);
            self.open_selected();
        }
    }

    /// The arrows move and Esc closes the card, then the window (when no
    /// text field has the keyboard). Enter, F2 and Ctrl+Delete are the
    /// keymap's (`Scope::ReportList`).
    fn keys(&mut self, ui: &Ui) {
        if ui.ctx().text_edit_focused() {
            return;
        }
        let (up, down, esc) = ui.input(|i| {
            (
                i.key_pressed(egui::Key::ArrowUp),
                i.key_pressed(egui::Key::ArrowDown),
                i.key_pressed(egui::Key::Escape),
            )
        });
        if self.edit.is_none() && (up || down) {
            self.step(down);
        }
        if esc && !self.command(CommandId::new("form.revert")) {
            self.link
                .push(CompanyEvent::CloseWindow(CommandId::new(KIND)));
        }
    }

    fn state_json(&self) -> Value {
        let rows: Vec<Value> = self
            .sections()
            .iter()
            .flat_map(|(_, rows)| rows.iter())
            .map(|r| {
                json!({
                    "id": r.id,
                    "name": r.name,
                    "kind": r.kind,
                    "group": r.group_id,
                    "author": r.created_by_name,
                    "can_edit": r.can_edit,
                    "version": r.version,
                })
            })
            .collect();
        let groups: Vec<Value> = self
            .groups
            .iter()
            .map(|g| json!({"id": g.id, "name": g.name}))
            .collect();
        json!({
            "screen": "memorized_reports",
            "layout": self.layout.json(),
            "company_open": self.tenant.is_some(),
            "available": self.available(),
            "loaded": self.loaded,
            "count": self.rows.len(),
            "shown": rows.len(),
            "rows": rows,
            "groups": groups,
            "selected": match &self.selected {
                Some(Selected::Report(id)) => json!({"report": id}),
                Some(Selected::Group(id)) => json!({"group": id}),
                None => Value::Null,
            },
            "selected_name": self.selected_row().map(|r| r.name.clone())
                .or_else(|| self.selected_group().map(|g| g.name.clone())),
            "gated": self.gate().is_some(),
            "gate": self.gate(),
            "edit": self.edit.as_ref().map(Edit::kind),
            "problem": self.problem,
            "notice": self.notice,
            "last_action": self.last_action,
            "empty": self.loaded && self.rows.is_empty() && self.groups.is_empty(),
            "busy": self.busy(),
        })
    }
}

/// The Report column: the catalog title of a standard report, "Report
/// Builder" for a builder one.
fn report_title(r: &DefinitionRow) -> String {
    if r.kind == "builder" {
        return t("reports.memorized.kind.builder");
    }
    r.definition["reportKey"]
        .as_str()
        .and_then(cl_model::reports::find)
        .map_or_else(|| t("reports.memorized.kind.standard"), |s| t(s.title))
}

/// The Last changed column: the local date.
fn changed_text(ts: cl_core::Timestamp) -> String {
    let d = ts.as_utc().with_timezone(&chrono::Local).date_naive();
    CalendarDate::from_naive(d).map_or_else(|_| String::new(), crate::shell::us_date)
}

impl Screen for MemorizedList {
    fn title(&self) -> String {
        t("reports.memorized.title")
    }

    fn ui(&mut self, ui: &mut Ui) {
        self.poll();
        let changes = self.link.bus().take(&mut self.bus);
        if changes.reports && !self.busy() {
            self.reload();
            self.poll();
        }
        self.draw(ui);
        self.keys(ui);
        if self.busy() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    /// The list's keys while no card is open (Q-A09k).
    fn scope(&self) -> Scope {
        if self.edit.is_some() {
            Scope::Window
        } else {
            Scope::ReportList
        }
    }

    fn command(&mut self, id: CommandId) -> bool {
        match id.as_str() {
            KIND => true,
            "reports.memorized_open" => {
                if self.edit.is_none() {
                    self.open_selected();
                }
                true
            }
            "reports.memorized_rename" => {
                if self.edit.is_none() {
                    self.ask_rename();
                }
                true
            }
            "reports.memorized_delete" | "list.delete" => {
                if self.edit.is_none() {
                    self.ask_delete();
                }
                true
            }
            "list.new" => {
                if self.edit.is_none() {
                    self.ask_new_group();
                }
                true
            }
            "form.revert" => {
                if self.edit.take().is_some() {
                    self.problem = None;
                    return true;
                }
                false
            }
            _ => false,
        }
    }

    /// A focused row is not a text field: Ctrl+Delete there deletes; in the
    /// search box it deletes a word.
    fn keeps_text_key(&self, ctx: &egui::Context, _s: cl_model::Shortcut) -> bool {
        ctx.text_edit_focused()
    }

    fn state(&self) -> Value {
        self.state_json()
    }

    fn fixed_layout(&self) -> bool {
        true
    }
}

/// The Memorized Reports window for `kind`, if it is one.
#[must_use]
pub fn make(
    kind: CommandId,
    link: &Arc<CompanyLink>,
    open: Option<(TenantId, String)>,
) -> Option<Box<dyn Screen>> {
    (kind.as_str() == KIND).then(|| Box::new(MemorizedList::new(Arc::clone(link), open)) as Box<_>)
}

#[cfg(test)]
#[path = "memorized_list_tests.rs"]
mod tests;
