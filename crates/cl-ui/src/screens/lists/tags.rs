//! Tags (P11 step 5): every tag by group on the ledger grid, the tag
//! groups under it, add/edit panels for tags and groups, make inactive /
//! active again, delete an unused tag, rename, merge, and where the
//! selected tag is used (the engine's usage counts).
//!
//! The rules the transaction forms apply (Q-178 to Q-181) live in
//! `cl_model::lists::tags`. A group's "one per line" flag is the engine's
//! (`isSingleSelect`); "required for these account types" (Q-180) is kept
//! on the company record and checked by the forms until the engine stores
//! it (U-42). The engine's merge moves header tags only (U-41): merge is
//! offered only for a tag the engine reports as used on transactions alone,
//! never on lines or as a default.
//!
//! Keyboard: Ctrl+N new tag, Enter or Ctrl+E edit, Ctrl+Shift+I make
//! inactive or active again, Ctrl+D delete an unused tag, Ctrl+Shift+H
//! include inactive, Ctrl+F find; New group and Merge are toolbar buttons
//! (Tab, then Space). In a panel Alt+S saves and closes, Alt+N saves and
//! starts another, Esc reverts, then closes.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use cl_api::{
    ApiError, Capabilities, ConflictReason, EditableFields, IdempotencyKey, OrphanLabel,
    RecordVersion, RequestGroup, Tag, TagFields, TagFilter as ApiTagFilter, TagGroup,
    TagGroupFields, TagUpdate, TagUsage, Versioned,
};
use cl_core::ids::{TagGroupId, TagId, TenantId};
use cl_grid::{CellPos, Grid, GridEvent, GridHeight, GridSnapshot, ModelRef, NoLookups, RowKey};
use cl_model::forms::{AfterSave, FormState, SaveIntent, SaveStart};
use cl_model::lists::accounts::AccountKind;
use cl_model::lists::tags::{
    GroupDraft, GroupEntry, TagDraft, TagEntry, TagField, TagFilter, TagList, TagProblem,
    check_group, check_tag,
};
use cl_model::{CommandId, Scope, commands};
use eframe::egui::{self, RichText, Ui};
use serde_json::{Value, json};

use crate::company::{CompanyLink, Task};
use crate::grid_env::EnvParts;
use crate::i18n::{t, t_with};
use crate::screens::company::form;
use crate::screens::lists::detail_pane as pane;
use crate::screens::lists::rows::{TextRow, TextRows, column, list_spec};
use crate::theme::Tokens;
use crate::windows::Screen;

/// The command and window kind.
pub const KIND: &str = "lists.tags";

const COL_NAME: cl_grid::ColumnId = cl_grid::ColumnId(1);
const COL_GROUP: cl_grid::ColumnId = cl_grid::ColumnId(2);
const COL_DESCRIPTION: cl_grid::ColumnId = cl_grid::ColumnId(3);
const COL_STATUS: cl_grid::ColumnId = cl_grid::ColumnId(4);

// Field ids for undo merging.
const F_NAME: u64 = 1;
const F_GROUP: u64 = 2;
const F_COLOR: u64 = 3;
const F_DESCRIPTION: u64 = 4;
const F_SINGLE: u64 = 5;
const F_REQUIRED: u64 = 6;

const PANEL_WIDTH: f32 = 420.0;
const TYPE_AHEAD_SECS: f64 = 1.0;

fn field_id(kind: &str, f: TagField) -> egui::Id {
    egui::Id::unique(("tags-form", kind, f as u8))
}

fn search_id() -> egui::Id {
    egui::Id::unique("tags-search")
}

fn shortcut_hint(id: &str) -> String {
    commands::find(id)
        .and_then(|c| c.shortcut)
        .map(crate::input::shortcut_text)
        .unwrap_or_default()
}

fn new_grid(compact: bool) -> (Grid, TextRows<TagId>) {
    let cols = if compact {
        vec![
            column(COL_NAME, "tags.col.name", 200, 2),
            column(COL_GROUP, "tags.col.group", 140, 1),
            column(COL_STATUS, "tags.col.status", 90, 0),
        ]
    } else {
        vec![
            column(COL_NAME, "tags.col.name", 220, 2),
            column(COL_GROUP, "tags.col.group", 180, 1),
            column(COL_DESCRIPTION, "tags.col.description", 260, 2),
            column(COL_STATUS, "tags.col.status", 90, 0),
        ]
    };
    let ids = cols.iter().map(|c| c.id).collect();
    let salt = if compact {
        "tags/list-compact"
    } else {
        "tags/list"
    };
    // The specs are fixed and covered by tests; a failure is a programming
    // error in this file.
    let grid = Grid::new(salt, list_spec(cols)).expect("the tag list grid spec is valid");
    (grid, TextRows::new(ids))
}

/// The add/edit tag panel.
struct TagForm {
    editing: Option<TagId>,
    form: FormState<TagDraft>,
    problems: Vec<TagProblem>,
    error: Option<ApiError>,
    focus: Option<TagField>,
    task: Option<Task<Versioned<Tag>>>,
}

impl TagForm {
    /// Takes the engine's refusal of a save: a used name (another window
    /// got there first) reads like the local check, on the name field.
    fn save_failed(&mut self, e: ApiError) {
        self.form.save_failed(cl_model::forms::SaveFailure {
            message: cl_core::OwnerFacing::owner_message(&e),
            detail: cl_core::OwnerFacing::scrubbed_detail(&e),
        });
        if matches!(
            &e,
            ApiError::Conflict {
                reason: ConflictReason::AlreadyExists | ConflictReason::Unspecified,
                ..
            }
        ) {
            self.problems = vec![TagProblem::NameTaken];
            self.focus = Some(TagField::Name);
        } else {
            self.error = Some(e);
        }
    }
}

/// The add/edit group panel.
struct GroupForm {
    editing: Option<TagGroupId>,
    form: FormState<GroupDraft>,
    problems: Vec<TagProblem>,
    error: Option<ApiError>,
    focus: Option<TagField>,
    task: Option<Task<TagGroup>>,
    deleting: Option<Task<()>>,
}

/// Merge the selected tag into another.
struct MergePanel {
    source: TagId,
    targets: Vec<TagId>,
    pick: usize,
    task: Option<Task<()>>,
    error: Option<ApiError>,
    focus: bool,
}

enum Panel {
    None,
    Tag(Box<TagForm>),
    Group(Box<GroupForm>),
    Merge(Box<MergePanel>),
}

impl Panel {
    fn name(&self) -> &'static str {
        match self {
            Panel::None => "none",
            Panel::Tag(f) if f.editing.is_some() => "edit_tag",
            Panel::Tag(_) => "new_tag",
            Panel::Group(f) if f.editing.is_some() => "edit_group",
            Panel::Group(_) => "new_group",
            Panel::Merge(_) => "merge",
        }
    }

    fn dirty(&self) -> bool {
        match self {
            Panel::Tag(f) => f.form.is_dirty(),
            Panel::Group(f) => f.form.is_dirty(),
            _ => false,
        }
    }

    fn busy(&self) -> bool {
        match self {
            Panel::Tag(f) => f.task.is_some(),
            Panel::Group(f) => f.task.is_some() || f.deleting.is_some(),
            Panel::Merge(m) => m.task.is_some(),
            Panel::None => false,
        }
    }
}

/// The selected tag's usage, as the engine reports it.
struct Usage {
    id: TagId,
    value: Option<TagUsage>,
    task: Option<Task<TagUsage>>,
}

/// What the engine can do beyond today's contract.
#[derive(Clone, Copy, Default)]
struct Caps {
    merge_lines: bool,
}

/// The Tags window.
pub struct TagsScreen {
    link: Arc<CompanyLink>,
    tenant: Option<(TenantId, String)>,
    group: Option<RequestGroup>,
    tags: Vec<Tag>,
    groups: Vec<TagGroup>,
    required: BTreeMap<TagGroupId, BTreeSet<String>>,
    list: TagList,
    filter: TagFilter,
    model: TextRows<TagId>,
    grid: Grid,
    load: Option<Task<(Vec<Tag>, Vec<TagGroup>)>>,
    loaded: bool,
    load_error: Option<ApiError>,
    toggle: Option<(TagId, Task<Versioned<Tag>>)>,
    delete: Option<(TagId, Task<()>)>,
    action_error: Option<ApiError>,
    usage: Option<Usage>,
    panel: Panel,
    note: Option<&'static str>,
    warn: Option<&'static str>,
    parts: EnvParts,
    snap: GridSnapshot,
    focus_search: bool,
    want_cursor: Option<TagId>,
    typed: String,
    typed_at: f64,
    compact: bool,
    just_hidden: Option<TagId>,
    caps: Caps,
}

impl TagsScreen {
    /// The window over the open company.
    #[must_use]
    pub fn new(link: Arc<CompanyLink>, tenant: Option<(TenantId, String)>) -> TagsScreen {
        let group = link.group().and_then(Result::ok);
        let caps =
            link.client()
                .map(cl_api::Client::capabilities)
                .map_or_else(Caps::default, |c| Caps {
                    merge_lines: c.has(Capabilities::TAG_MERGE),
                });
        let (grid, model) = new_grid(false);
        let mut s = TagsScreen {
            link,
            tenant,
            group,
            tags: Vec::new(),
            groups: Vec::new(),
            required: BTreeMap::new(),
            list: TagList::default(),
            filter: TagFilter::default(),
            model,
            grid,
            load: None,
            loaded: false,
            load_error: None,
            toggle: None,
            delete: None,
            action_error: None,
            usage: None,
            panel: Panel::None,
            note: None,
            warn: None,
            parts: EnvParts::default(),
            snap: GridSnapshot::default(),
            focus_search: false,
            want_cursor: None,
            typed: String::new(),
            typed_at: f64::NEG_INFINITY,
            compact: false,
            just_hidden: None,
            caps,
        };
        s.read_prefs();
        s.start_load();
        s
    }

    /// Required groups (Q-180) from the company record.
    fn read_prefs(&mut self) {
        if let Some(d) = self.link.demo_state() {
            self.required.clone_from(&d.required_tag_groups);
            return;
        }
        let Some((id, _)) = &self.tenant else { return };
        if let Some(rec) = self
            .link
            .store()
            .load()
            .ok()
            .and_then(|l| l.get(*id).cloned())
        {
            self.required = rec.prefs.required_tag_groups;
        }
    }

    fn store_required(&mut self, id: TagGroupId, kinds: Option<BTreeSet<String>>) {
        match kinds.filter(|k| !k.is_empty()) {
            Some(k) => {
                self.required.insert(id, k);
            }
            None => {
                self.required.remove(&id);
            }
        }
        if let Some(mut d) = self.link.demo_state() {
            d.required_tag_groups.clone_from(&self.required);
            return;
        }
        if let Some((tenant, _)) = &self.tenant {
            let map = self.required.clone();
            let tenant = *tenant;
            if self
                .link
                .store()
                .update(|l| {
                    if let Some(rec) = l.get_mut(tenant) {
                        rec.prefs.required_tag_groups = map;
                    }
                })
                .is_err()
            {
                self.warn = Some("tags.required_not_saved");
            }
        }
    }

    fn start_load(&mut self) {
        self.load_error = None;
        if let Some(mut d) = self.link.demo_state() {
            let tags = d.tags_mut().clone();
            let groups = d.tag_groups_mut().clone();
            self.load = Some(Task::ready((tags, groups)));
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            self.loaded = true;
            return;
        };
        match group.read(&session, |s| async move {
            let tags = s.tags().list_all(&ApiTagFilter::default()).await?;
            let groups = s.tags().groups().await?;
            Ok((tags, groups))
        }) {
            Ok(p) => self.load = Some(Task::Pending(p)),
            Err(_) => self.loaded = true,
        }
    }

    fn apply(&mut self) {
        let keep = self.want_cursor.take().or_else(|| self.selected());
        let groups = self
            .groups
            .iter()
            .map(|g| GroupEntry {
                id: g.id,
                name: g.fields.name.clone(),
                description: g.fields.description.clone(),
                single: g.fields.is_single_select,
                required_for: self
                    .required
                    .get(&g.id)
                    .map(|k| k.iter().filter_map(|w| AccountKind::from_wire(w)).collect())
                    .unwrap_or_default(),
                order: g.sort_order.unwrap_or(0),
            })
            .collect();
        let tags = self
            .tags
            .iter()
            .map(|t| TagEntry {
                id: t.id,
                name: t.fields.name.clone(),
                group: t.fields.group_id,
                color: t.fields.color.clone(),
                description: t.fields.description.clone(),
                active: t.is_active,
                usage: t.usage_count,
            })
            .collect();
        self.list = TagList::new(groups, tags);
        self.refilter();
        self.loaded = true;
        self.place_cursor(keep);
    }

    fn refilter(&mut self) {
        let inactive = t("tags.status.inactive");
        let rows = self
            .list
            .visible(&self.filter)
            .into_iter()
            .map(|i| {
                let e = &self.list.tags()[i];
                let group = e
                    .group
                    .and_then(|g| self.list.group(g))
                    .map(|g| g.name.clone())
                    .unwrap_or_default();
                let status = if e.active {
                    String::new()
                } else {
                    inactive.clone()
                };
                let mut label = e.name.clone();
                if !group.is_empty() {
                    label = format!("{label}, {group}");
                }
                if !e.active {
                    label = format!("{label}, {inactive}");
                }
                let cells = if self.compact {
                    vec![e.name.clone(), group, status]
                } else {
                    vec![
                        e.name.clone(),
                        group,
                        e.description.clone().unwrap_or_default(),
                        status,
                    ]
                };
                TextRow {
                    id: e.id,
                    cells,
                    label,
                }
            })
            .collect();
        self.model.set(rows);
    }

    fn place_cursor(&mut self, id: Option<TagId>) {
        let key = id
            .and_then(|i| self.model.key_of(i))
            .or_else(|| cl_grid::RowModel::row_key(&self.model, 0));
        if let Some(key) = key {
            self.focus_row(key, !self.compact);
        }
    }

    fn focus_row(&mut self, key: RowKey, take_focus: bool) {
        let label = String::new();
        let env = self
            .parts
            .env(&NoLookups, &label, GridHeight::Rows { min: 1, max: 1 });
        let pos = CellPos {
            row: key,
            col: COL_NAME,
        };
        if take_focus {
            self.grid
                .focus(pos, ModelRef::Browse(&mut self.model), &env);
        } else {
            self.grid
                .set_cursor(pos, ModelRef::Browse(&mut self.model), &env);
        }
        self.grid.scroll_to(
            key,
            cl_grid::core::viewport::RowAlign::Nearest,
            &ModelRef::Browse(&mut self.model),
        );
    }

    fn sync_compact(&mut self) {
        let want = !matches!(self.panel, Panel::None);
        if want == self.compact {
            return;
        }
        let keep = self.selected();
        self.compact = want;
        (self.grid, self.model) = new_grid(want);
        self.refilter();
        if let Some(key) = keep.and_then(|id| self.model.key_of(id)) {
            self.focus_row(key, !want);
        } else if !want {
            self.place_cursor(None);
        }
    }

    /// The tag under the cursor.
    #[must_use]
    pub fn selected(&self) -> Option<TagId> {
        self.grid
            .core()
            .cursor()
            .and_then(|c| self.model.id_at(c.row))
    }

    fn tag(&self, id: TagId) -> Option<&Tag> {
        self.tags.iter().find(|t| t.id == id)
    }

    /// Loads the selected tag's usage when the selection moved.
    fn follow_selection(&mut self) {
        let sel = self.selected();
        match (sel, self.usage.as_ref().map(|u| u.id)) {
            (Some(id), Some(cur)) if id == cur => {}
            (Some(id), _) => self.load_usage(id),
            (None, _) => self.usage = None,
        }
    }

    fn load_usage(&mut self, id: TagId) {
        let task = if self.link.is_demo() {
            Some(Task::ready(crate::company::demo::lists::tag_usage(id)))
        } else {
            match (self.link.session(), self.group.as_ref()) {
                (Some(session), Some(group)) => group
                    .read(&session, move |s| async move { s.tags().usage(id).await })
                    .ok()
                    .map(Task::Pending),
                _ => None,
            }
        };
        self.usage = Some(Usage {
            id,
            value: None,
            task,
        });
    }

    fn selected_usage(&self) -> Option<TagUsage> {
        let sel = self.selected()?;
        self.usage
            .as_ref()
            .filter(|u| u.id == sel)
            .and_then(|u| u.value)
    }

    /// Merge is offered for a tag whose every use the engine can move.
    fn merge_available(&self) -> bool {
        self.selected_usage()
            .is_some_and(|u| self.caps.merge_lines || u.merge_safe())
    }

    fn panel_blocks(&mut self) -> bool {
        if self.panel.dirty() || self.panel.busy() {
            self.warn = Some("tags.finish_first");
            return true;
        }
        false
    }

    /// Ctrl+N.
    pub fn open_new_tag(&mut self) {
        if self.panel_blocks() {
            return;
        }
        let group = self
            .selected()
            .and_then(|id| self.list.tag(id))
            .and_then(|e| e.group);
        self.warn = None;
        let draft = TagDraft {
            group,
            ..TagDraft::default()
        };
        self.panel = Panel::Tag(Box::new(TagForm {
            editing: None,
            form: FormState::new(draft),
            problems: Vec::new(),
            error: None,
            focus: Some(TagField::Name),
            task: None,
        }));
    }

    /// Enter or Ctrl+E.
    pub fn open_edit_tag(&mut self, id: Option<TagId>) {
        let Some(id) = id else {
            self.warn = Some("tags.select_first");
            return;
        };
        if self.panel_blocks() {
            return;
        }
        let Some(e) = self.list.tag(id) else { return };
        self.warn = None;
        self.panel = Panel::Tag(Box::new(TagForm {
            editing: Some(id),
            form: FormState::new(TagDraft::from_entry(e)),
            problems: Vec::new(),
            error: None,
            focus: Some(TagField::Name),
            task: None,
        }));
    }

    /// New group (toolbar) or edit a group (its button under the list).
    pub fn open_group(&mut self, id: Option<TagGroupId>) {
        if self.panel_blocks() {
            return;
        }
        let draft = id
            .and_then(|g| self.list.group(g))
            .map(GroupDraft::from_entry)
            .unwrap_or_default();
        self.warn = None;
        self.panel = Panel::Group(Box::new(GroupForm {
            editing: id,
            form: FormState::new(draft),
            problems: Vec::new(),
            error: None,
            focus: Some(TagField::Name),
            task: None,
            deleting: None,
        }));
    }

    /// Merge (toolbar): only for a tag the engine can merge completely.
    pub fn open_merge(&mut self) {
        let Some(source) = self.selected() else {
            self.warn = Some("tags.select_first");
            return;
        };
        if self.panel_blocks() {
            return;
        }
        if self.selected_usage().is_none() {
            self.warn = Some("tags.usage_loading");
            return;
        }
        if !self.merge_available() {
            self.warn = Some("tags.merge_lines_later");
            return;
        }
        let targets: Vec<TagId> = self
            .list
            .merge_targets(source)
            .iter()
            .map(|e| e.id)
            .collect();
        if targets.is_empty() {
            self.warn = Some("tags.merge_no_targets");
            return;
        }
        self.warn = None;
        self.panel = Panel::Merge(Box::new(MergePanel {
            source,
            targets,
            pick: 0,
            task: None,
            error: None,
            focus: true,
        }));
    }

    /// Ctrl+Shift+I.
    pub fn toggle_active(&mut self) {
        let Some(id) = self.selected() else {
            self.warn = Some("tags.select_first");
            return;
        };
        if self.toggle.is_some() {
            return;
        }
        let Some(tag) = self.tag(id).cloned() else {
            return;
        };
        self.warn = None;
        self.note = None;
        self.action_error = None;
        let update = TagUpdate {
            fields: tag.fields.clone(),
            is_active: !tag.is_active,
        };
        let task = if let Some(mut d) = self.link.demo_state() {
            let mut changed = tag.clone();
            changed.is_active = update.is_active;
            if let Some(x) = d.tags_mut().iter_mut().find(|x| x.id == id) {
                *x = changed.clone();
            }
            Task::ready(Versioned {
                version: RecordVersion::Fingerprint(changed.fingerprint()),
                record: changed,
            })
        } else {
            let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
                return;
            };
            let key = IdempotencyKey::new();
            let label = OrphanLabel {
                what: "orphan.tag_save",
                key: Some(key.clone()),
            };
            let version = RecordVersion::Fingerprint(tag.fingerprint());
            match group.write(&session, label, move |s| async move {
                s.tags().update(id, &version, &update, key).await
            }) {
                Ok(p) => Task::Pending(p),
                Err(_) => return,
            }
        };
        self.toggle = Some((id, task));
    }

    /// Ctrl+D: delete the selected tag when nothing uses it.
    pub fn delete_tag(&mut self) {
        let Some(id) = self.selected() else {
            self.warn = Some("tags.select_first");
            return;
        };
        if self.delete.is_some() || self.panel_blocks() {
            return;
        }
        if self.selected_usage().is_some_and(|u| u.total > 0) {
            self.warn = Some("tags.in_use");
            return;
        }
        self.warn = None;
        self.note = None;
        self.action_error = None;
        let task = if let Some(mut d) = self.link.demo_state() {
            d.tags_mut().retain(|x| x.id != id);
            d.tags_gone.insert(id);
            Task::ready(())
        } else {
            let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
                return;
            };
            let key = IdempotencyKey::new();
            let label = OrphanLabel {
                what: "orphan.tag_delete",
                key: Some(key.clone()),
            };
            match group.write(&session, label, move |s| async move {
                s.tags().delete(id, key).await
            }) {
                Ok(p) => Task::Pending(p),
                Err(_) => return,
            }
        };
        self.delete = Some((id, task));
    }

    /// Ctrl+Shift+H.
    pub fn set_show_inactive(&mut self, on: bool) {
        self.filter.show_inactive = on;
        let hidden = self.just_hidden.take().filter(|_| on);
        let keep = hidden.or_else(|| self.selected());
        self.refilter();
        self.place_cursor(keep);
    }

    /// Alt+N / Alt+S in a panel. Returns true when it finished at once.
    pub fn save_panel(&mut self, intent: SaveIntent) -> bool {
        match &self.panel {
            Panel::Tag(_) => self.save_tag(intent),
            Panel::Group(_) => self.save_group(intent),
            Panel::Merge(_) => {
                self.confirm_merge();
                false
            }
            Panel::None => false,
        }
    }

    fn save_tag(&mut self, intent: SaveIntent) -> bool {
        let checked = match &self.panel {
            Panel::Tag(f) if f.task.is_none() => check_tag(f.form.current(), &self.list, f.editing),
            _ => return false,
        };
        let Panel::Tag(f) = &mut self.panel else {
            return false;
        };
        let checked = match checked {
            Ok(c) => c,
            Err(problems) => {
                f.focus = problems.first().map(|p| p.field());
                f.problems = problems;
                return false;
            }
        };
        f.problems.clear();
        f.error = None;
        match f.form.begin_save(intent) {
            SaveStart::Send(_) => {}
            SaveStart::Skip(after) => {
                let id = f.editing;
                self.after_tag_save(after, id);
                return true;
            }
            SaveStart::Busy => return false,
        }
        let fields = TagFields {
            name: checked.name,
            color: checked.color,
            group_id: checked.group,
            description: checked.description,
        };
        let existing = f
            .editing
            .and_then(|id| self.tags.iter().find(|x| x.id == id).cloned());
        if self.link.is_demo() {
            let saved = demo_save_tag(&self.link, existing, fields);
            if let Panel::Tag(f) = &mut self.panel {
                f.task = Some(Task::ready(saved));
            }
            return self.poll_tag_save();
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return false;
        };
        let key = IdempotencyKey::new();
        let label = OrphanLabel {
            what: "orphan.tag_save",
            key: Some(key.clone()),
        };
        let pending = match existing {
            Some(x) => {
                let id = x.id;
                let version = RecordVersion::Fingerprint(x.fingerprint());
                let update = TagUpdate {
                    fields,
                    is_active: x.is_active,
                };
                group.write(&session, label, move |s| async move {
                    s.tags().update(id, &version, &update, key).await
                })
            }
            None => group.write(&session, label, move |s| async move {
                s.tags().create(&fields, key).await
            }),
        };
        if let (Ok(p), Panel::Tag(f)) = (pending, &mut self.panel) {
            f.task = Some(Task::Pending(p));
        }
        false
    }

    fn poll_tag_save(&mut self) -> bool {
        let Panel::Tag(f) = &mut self.panel else {
            return false;
        };
        let Some(result) = f.task.as_mut().and_then(Task::try_take) else {
            return false;
        };
        f.task = None;
        match result {
            Ok(saved) => {
                let tag = saved.record;
                let after = f.form.save_ok(f.form.current().clone());
                match self.tags.iter_mut().find(|x| x.id == tag.id) {
                    Some(x) => *x = tag.clone(),
                    None => self.tags.push(tag.clone()),
                }
                self.want_cursor = Some(tag.id);
                self.apply();
                self.note = Some("tags.saved");
                self.after_tag_save(after, Some(tag.id));
                true
            }
            Err(e) => {
                f.save_failed(e);
                false
            }
        }
    }

    fn after_tag_save(&mut self, after: AfterSave, saved: Option<TagId>) {
        match after {
            AfterSave::Stay => {
                if let Some(id) = saved {
                    self.open_edit_tag(Some(id));
                    if let Panel::Tag(f) = &mut self.panel {
                        f.focus = None;
                    }
                }
            }
            AfterSave::NewForm => {
                let group = match &self.panel {
                    Panel::Tag(f) => f.form.current().group,
                    _ => None,
                };
                self.panel = Panel::None;
                self.open_new_tag();
                if let Panel::Tag(f) = &mut self.panel {
                    f.form = FormState::new(TagDraft {
                        group,
                        ..TagDraft::default()
                    });
                }
            }
            AfterSave::CloseWindow => self.close_panel(saved),
        }
    }

    fn save_group(&mut self, intent: SaveIntent) -> bool {
        let checked = match &self.panel {
            Panel::Group(f) if f.task.is_none() && f.deleting.is_none() => {
                check_group(f.form.current(), &self.list, f.editing)
            }
            _ => return false,
        };
        let Panel::Group(f) = &mut self.panel else {
            return false;
        };
        let checked = match checked {
            Ok(c) => c,
            Err(problems) => {
                f.focus = problems.first().map(|p| p.field());
                f.problems = problems;
                return false;
            }
        };
        f.problems.clear();
        f.error = None;
        match f.form.begin_save(intent) {
            SaveStart::Send(_) => {}
            SaveStart::Skip(_) => {
                self.close_panel(None);
                return true;
            }
            SaveStart::Busy => return false,
        }
        let fields = TagGroupFields {
            name: checked.name,
            description: checked.description,
            is_single_select: checked.single,
        };
        let editing = f.editing;
        let demo_fields = fields.clone();
        let demo = self.link.demo_state().map(|mut d| {
            let fields = demo_fields;
            let groups = d.tag_groups_mut();
            if let Some(g) = editing.and_then(|id| groups.iter_mut().find(|g| g.id == id)) {
                g.fields = fields;
                g.clone()
            } else {
                let n = u32::try_from(groups.len()).unwrap_or(0);
                let g = TagGroup {
                    id: crate::company::demo::lists::group_id(900_000 + n),
                    fields,
                    sort_order: Some(i64::from(n) + 1),
                    tags: Vec::new(),
                };
                groups.push(g.clone());
                g
            }
        });
        if let Some(saved) = demo {
            if let Panel::Group(f) = &mut self.panel {
                f.task = Some(Task::ready(saved));
            }
            return self.poll_group_save();
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return false;
        };
        let key = IdempotencyKey::new();
        let label = OrphanLabel {
            what: "orphan.tag_group_save",
            key: Some(key.clone()),
        };
        let pending = match editing {
            Some(id) => group.write(&session, label, move |s| async move {
                s.tags().update_group(id, &fields, key).await
            }),
            None => group.write(&session, label, move |s| async move {
                s.tags().create_group(&fields, key).await
            }),
        };
        if let (Ok(p), Panel::Group(f)) = (pending, &mut self.panel) {
            f.task = Some(Task::Pending(p));
        }
        false
    }

    fn poll_group_save(&mut self) -> bool {
        let Panel::Group(f) = &mut self.panel else {
            return false;
        };
        let Some(result) = f.task.as_mut().and_then(Task::try_take) else {
            return false;
        };
        f.task = None;
        match result {
            Ok(saved) => {
                let required: BTreeSet<String> = f
                    .form
                    .current()
                    .required_for
                    .iter()
                    .map(|k| k.wire().to_string())
                    .collect();
                let after = f.form.save_ok(f.form.current().clone());
                match self.groups.iter_mut().find(|g| g.id == saved.id) {
                    Some(g) => {
                        let tags = std::mem::take(&mut g.tags);
                        *g = saved.clone();
                        g.tags = tags;
                    }
                    None => self.groups.push(saved.clone()),
                }
                self.store_required(saved.id, Some(required));
                self.apply();
                self.note = Some("tags.group_saved");
                match after {
                    AfterSave::NewForm => {
                        self.panel = Panel::None;
                        self.open_group(None);
                    }
                    AfterSave::Stay => {
                        self.panel = Panel::None;
                        self.open_group(Some(saved.id));
                    }
                    AfterSave::CloseWindow => self.close_panel(None),
                }
                true
            }
            Err(e) => {
                f.form.save_failed(cl_model::forms::SaveFailure {
                    message: cl_core::OwnerFacing::owner_message(&e),
                    detail: cl_core::OwnerFacing::scrubbed_detail(&e),
                });
                f.error = Some(e);
                false
            }
        }
    }

    /// Deletes the group open in the panel; its tags stay, ungrouped.
    fn delete_group(&mut self) {
        let Panel::Group(f) = &mut self.panel else {
            return;
        };
        let Some(id) = f.editing else { return };
        if f.task.is_some() || f.deleting.is_some() {
            return;
        }
        f.error = None;
        if let Some(mut d) = self.link.demo_state() {
            d.tag_groups_mut().retain(|g| g.id != id);
            for t in d.tags_mut() {
                if t.fields.group_id == Some(id) {
                    t.fields.group_id = None;
                }
            }
            f.deleting = Some(Task::ready(()));
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        let key = IdempotencyKey::new();
        let label = OrphanLabel {
            what: "orphan.tag_group_save",
            key: Some(key.clone()),
        };
        if let Ok(p) = group.write(&session, label, move |s| async move {
            s.tags().delete_group(id, key).await
        }) {
            f.deleting = Some(Task::Pending(p));
        }
    }

    fn confirm_merge(&mut self) {
        let Panel::Merge(m) = &mut self.panel else {
            return;
        };
        if m.task.is_some() {
            return;
        }
        let (source, Some(target)) = (m.source, m.targets.get(m.pick).copied()) else {
            return;
        };
        m.error = None;
        if let Some(mut d) = self.link.demo_state() {
            d.tags_mut().retain(|x| x.id != source);
            d.tags_gone.insert(source);
            m.task = Some(Task::ready(()));
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        let key = IdempotencyKey::new();
        let label = OrphanLabel {
            what: "orphan.tag_merge",
            key: Some(key.clone()),
        };
        if let Ok(p) = group.write(&session, label, move |s| async move {
            s.tags().merge(source, target, key).await
        }) {
            m.task = Some(Task::Pending(p));
        }
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one poll per running task, each short"
    )]
    fn poll(&mut self) {
        if let Some(result) = self.load.as_mut().and_then(Task::try_take) {
            self.load = None;
            match result {
                Ok((tags, groups)) => {
                    self.tags = tags;
                    self.groups = groups;
                    self.apply();
                }
                Err(e) => {
                    self.loaded = true;
                    self.load_error = Some(e);
                }
            }
        }
        if let Some((id, task)) = self.toggle.as_mut()
            && let Some(result) = task.try_take()
        {
            let id = *id;
            self.toggle = None;
            match result {
                Ok(saved) => {
                    let active = saved.record.is_active;
                    self.just_hidden = (!active && !self.filter.show_inactive).then_some(id);
                    if let Some(x) = self.tags.iter_mut().find(|x| x.id == id) {
                        *x = saved.record;
                    }
                    self.want_cursor = Some(id);
                    self.apply();
                    self.note = Some(if active {
                        "tags.made_active"
                    } else {
                        "tags.made_inactive"
                    });
                }
                Err(e) => self.action_error = Some(e),
            }
        }
        if let Some((id, task)) = self.delete.as_mut()
            && let Some(result) = task.try_take()
        {
            let id = *id;
            self.delete = None;
            match result {
                Ok(()) => {
                    self.tags.retain(|x| x.id != id);
                    self.usage = None;
                    self.apply();
                    self.note = Some("tags.deleted");
                }
                Err(ApiError::Conflict {
                    reason: ConflictReason::InUse,
                    ..
                }) => self.warn = Some("tags.in_use"),
                Err(e) => self.action_error = Some(e),
            }
        }
        if let Some(u) = self.usage.as_mut()
            && let Some(result) = u.task.as_mut().and_then(Task::try_take)
        {
            u.task = None;
            // Without the counts merge and delete stay cautious.
            u.value = result.ok();
        }
        if matches!(&self.panel, Panel::Tag(f) if f.task.is_some()) {
            self.poll_tag_save();
        }
        if matches!(&self.panel, Panel::Group(f) if f.task.is_some()) {
            self.poll_group_save();
        }
        if let Panel::Group(f) = &mut self.panel
            && let Some(result) = f.deleting.as_mut().and_then(Task::try_take)
        {
            f.deleting = None;
            match result {
                Ok(()) => {
                    if let Some(id) = f.editing {
                        self.groups.retain(|g| g.id != id);
                        for x in &mut self.tags {
                            if x.fields.group_id == Some(id) {
                                x.fields.group_id = None;
                            }
                        }
                        self.store_required(id, None);
                    }
                    self.panel = Panel::None;
                    self.apply();
                    self.note = Some("tags.group_deleted");
                }
                Err(e) => f.error = Some(e),
            }
        }
        if let Panel::Merge(m) = &mut self.panel
            && let Some(result) = m.task.as_mut().and_then(Task::try_take)
        {
            m.task = None;
            match result {
                Ok(()) => {
                    let target = m.targets.get(m.pick).copied();
                    self.panel = Panel::None;
                    self.usage = None;
                    self.want_cursor = target;
                    self.note = Some("tags.merged");
                    // The engine moved the uses and deleted the source:
                    // read both back.
                    self.start_load();
                }
                Err(e) => m.error = Some(e),
            }
        }
    }

    fn toolbar(&mut self, ui: &mut Ui) {
        let selected_active = self
            .selected()
            .and_then(|id| self.list.tag(id))
            .map(|e| e.active);
        let merge = self.merge_available();
        ui.horizontal_wrapped(|ui| {
            let b = |ui: &mut Ui, key: &str, cmd: &str| {
                let hint = shortcut_hint(cmd);
                let r = ui.button(t(key));
                if hint.is_empty() {
                    r
                } else {
                    r.on_hover_text(format!("{} ({hint})", t(key)))
                }
            };
            if b(ui, "tags.new", "list.new").clicked() {
                self.open_new_tag();
            }
            if ui.button(t("tags.new_group")).clicked() {
                self.open_group(None);
            }
            if b(ui, "tags.edit", "edit.edit_transaction").clicked() {
                self.open_edit_tag(self.selected());
            }
            let toggle_key = if selected_active == Some(false) {
                "tags.make_active"
            } else {
                "tags.make_inactive"
            };
            if b(ui, toggle_key, "list.toggle_active").clicked() {
                self.toggle_active();
            }
            if b(ui, "tags.delete", "form.delete_item").clicked() {
                self.delete_tag();
            }
            // Hidden for a tag used on lines or as a default (U-41).
            if merge && ui.button(t("tags.merge")).clicked() {
                self.open_merge();
            }
            ui.separator();
            let l = ui.add(egui::Label::new(t("tags.find")).selectable(false));
            let search = ui
                .add_sized(
                    [200.0, 24.0],
                    egui::TextEdit::singleline(&mut self.filter.search).id(search_id()),
                )
                .labelled_by(l.id)
                .on_hover_text(shortcut_hint("edit.find"));
            if self.focus_search {
                search.request_focus();
                self.focus_search = false;
            }
            if search.changed() {
                self.refilter();
            }
            let mut inactive = self.filter.show_inactive;
            if ui
                .checkbox(&mut inactive, t("tags.include_inactive"))
                .on_hover_text(shortcut_hint("list.show_inactive"))
                .changed()
            {
                self.set_show_inactive(inactive);
            }
        });
    }

    fn messages(&self, ui: &mut Ui, tokens: &Tokens) {
        for e in [&self.load_error, &self.action_error].into_iter().flatten() {
            form::inline_error(ui, tokens, e);
        }
        if let Some(w) = self.warn {
            ui.label(RichText::new(t(w)).color(tokens.warn_text));
        } else if let Some(n) = self.note {
            ui.label(RichText::new(t(n)).color(tokens.text_secondary));
        }
    }

    fn list_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let label = t("tags.grid_label");
        // Room for the usage lines and up to six group rows under the list.
        let rows = u16::try_from(self.groups.len().min(6)).unwrap_or(6);
        let reserve = 120.0 + 26.0 * f32::from(rows);
        let env = self.parts.env(
            &NoLookups,
            &label,
            GridHeight::Fill {
                reserve_below: reserve,
                min_rows: 5,
            },
        );
        let out = self.grid.show(ui, ModelRef::Browse(&mut self.model), &env);
        self.snap = self.grid.snapshot(&ModelRef::Browse(&mut self.model));
        for e in out.events {
            match e {
                GridEvent::Activate { key } => {
                    let id = self.model.id_at(key);
                    self.open_edit_tag(id);
                }
                GridEvent::TypedInBrowse(text) => {
                    let now = ui.input(|i| i.time);
                    if now - self.typed_at > TYPE_AHEAD_SECS {
                        self.typed.clear();
                    }
                    self.typed_at = now;
                    self.typed.push_str(&text);
                    let mut key = self.model.find_prefix(&self.typed);
                    if key.is_none() {
                        self.typed.clone_from(&text);
                        key = self.model.find_prefix(&text);
                    }
                    if let Some(key) = key {
                        self.focus_row(key, true);
                    }
                }
                _ => {}
            }
        }
        ui.label(
            RichText::new(t_with(
                "tags.count",
                &[
                    ("shown", &self.model.len().to_string()),
                    ("total", &self.list.len().to_string()),
                ],
            ))
            .color(tokens.text_secondary),
        );
        self.usage_ui(ui, tokens);
        self.groups_ui(ui, tokens);
    }

    /// Where the selected tag is used.
    fn usage_ui(&self, ui: &mut Ui, tokens: &Tokens) {
        let Some(sel) = self.selected() else { return };
        let name = self
            .list
            .tag(sel)
            .map(|e| e.name.clone())
            .unwrap_or_default();
        let text = match self.selected_usage() {
            None => t("tags.usage.loading"),
            Some(u) if u.total == 0 => t_with("tags.usage.none", &[("tag", &name)]),
            Some(u) => t_with(
                "tags.usage.some",
                &[
                    ("tag", &name),
                    ("transactions", &u.transactions.to_string()),
                    ("lines", &u.transaction_lines.to_string()),
                    (
                        "defaults",
                        &(u.items + u.vendor_contacts + u.customer_contacts + u.bank_rules)
                            .to_string(),
                    ),
                    ("budgets", &u.budgets.to_string()),
                ],
            ),
        };
        ui.label(RichText::new(text).color(tokens.text_secondary));
        if self.selected_usage().is_some() && !self.merge_available() {
            pane::pending(ui, tokens, &t("tags.merge_lines_later"));
        }
    }

    fn groups_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        pane::section(ui, tokens, &t("tags.groups.title"));
        if self.list.groups().is_empty() {
            ui.label(RichText::new(t("tags.groups.none")).color(tokens.text_secondary));
            return;
        }
        let mut open = None;
        let rows: Vec<(TagGroupId, String, String)> = self
            .list
            .groups()
            .iter()
            .map(|g| {
                let mut parts = Vec::new();
                parts.push(if g.single {
                    t("tags.groups.one_per_line")
                } else {
                    t("tags.groups.any_number")
                });
                if !g.required_for.is_empty() {
                    let kinds: Vec<String> = g
                        .required_for
                        .iter()
                        .map(|k| t(k.label().as_str()))
                        .collect();
                    parts.push(t_with(
                        "tags.groups.required_on",
                        &[("kinds", &kinds.join(", "))],
                    ));
                }
                (g.id, g.name.clone(), parts.join(" \u{b7} "))
            })
            .collect();
        egui::ScrollArea::vertical()
            .id_salt("tags-groups")
            .max_height(26.0 * 6.0)
            .show(ui, |ui| {
                for (id, name, info) in rows {
                    ui.horizontal(|ui| {
                        let r = ui
                            .button(&name)
                            .on_hover_text(t_with("tags.groups.edit_hint", &[("group", &name)]));
                        r.widget_info(|| {
                            egui::WidgetInfo::labeled(
                                egui::Role::Button,
                                true,
                                t_with("tags.groups.edit_label", &[("group", &name)]),
                            )
                        });
                        if r.clicked() {
                            open = Some(id);
                        }
                        ui.label(RichText::new(info).color(tokens.text_secondary));
                    });
                }
            });
        if let Some(id) = open {
            self.open_group(Some(id));
        }
    }

    fn panel_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        match &self.panel {
            Panel::Tag(_) => self.tag_form_ui(ui, tokens),
            Panel::Group(_) => self.group_form_ui(ui, tokens),
            Panel::Merge(_) => self.merge_ui(ui, tokens),
            Panel::None => {}
        }
    }

    fn tag_form_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let group_ids: Vec<Option<TagGroupId>> = std::iter::once(None)
            .chain(self.list.groups().iter().map(|g| Some(g.id)))
            .collect();
        let group_labels: Vec<String> = std::iter::once(t("tags.form.no_group"))
            .chain(self.list.groups().iter().map(|g| g.name.clone()))
            .collect();
        let Panel::Tag(f) = &mut self.panel else {
            return;
        };
        let mut save = None;
        let mut cancel = false;
        let heading = if f.editing.is_some() {
            t("tags.form.edit_title")
        } else {
            t("tags.form.new_title")
        };
        form::heading(ui, tokens, &heading);
        ui.add_space(6.0);
        let saving = f.task.is_some();
        ui.add_enabled_ui(!saving, |ui| {
            let d = f.form.current().clone();
            let focus = f.focus.take();
            let want = |x: TagField| focus == Some(x);
            let mut name = d.name.clone();
            let r = pane::text_row(
                ui,
                field_id("tag", TagField::Name),
                &t("tags.form.name"),
                &mut name,
                want(TagField::Name),
            );
            if r.changed() {
                f.form.edit(F_NAME, |x| x.name = name);
            }
            if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                save = Some(SaveIntent::SaveAndNew);
            }
            let cur = group_ids.iter().position(|g| *g == d.group).unwrap_or(0);
            if let Some(i) = pane::picker(
                ui,
                field_id("tag", TagField::Group),
                &t("tags.form.group"),
                &group_labels,
                cur,
                want(TagField::Group),
            ) {
                let g = group_ids[i];
                f.form.edit(F_GROUP, |x| x.group = g);
            }
            let mut color = d.color.clone();
            let r = pane::text_row(
                ui,
                field_id("tag", TagField::Color),
                &t("tags.form.color"),
                &mut color,
                want(TagField::Color),
            );
            if r.changed() {
                f.form.edit(F_COLOR, |x| x.color = color);
            }
            let mut description = d.description.clone();
            let r = pane::text_row(
                ui,
                field_id("tag", TagField::Description),
                &t("tags.form.description"),
                &mut description,
                want(TagField::Description),
            );
            if r.changed() {
                f.form.edit(F_DESCRIPTION, |x| x.description = description);
            }
        });
        problems_ui(ui, tokens, &f.problems, f.error.as_ref());
        if saving {
            form::progress_line(ui, tokens, &t("tags.form.saving"));
        }
        save_buttons(ui, &mut save, &mut cancel, true);
        if let Some(intent) = save {
            self.save_tag(intent);
        }
        if cancel {
            self.close_panel(None);
        }
    }

    fn group_form_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let Panel::Group(f) = &mut self.panel else {
            return;
        };
        let mut save = None;
        let mut cancel = false;
        let mut delete = false;
        let heading = if f.editing.is_some() {
            t("tags.group_form.edit_title")
        } else {
            t("tags.group_form.new_title")
        };
        form::heading(ui, tokens, &heading);
        ui.add_space(6.0);
        let busy = f.task.is_some() || f.deleting.is_some();
        ui.add_enabled_ui(!busy, |ui| {
            let d = f.form.current().clone();
            let focus = f.focus.take();
            let mut name = d.name.clone();
            let r = pane::text_row(
                ui,
                field_id("group", TagField::Name),
                &t("tags.group_form.name"),
                &mut name,
                focus == Some(TagField::Name),
            );
            if r.changed() {
                f.form.edit(F_NAME, |x| x.name = name);
            }
            let mut description = d.description.clone();
            let r = pane::text_row(
                ui,
                field_id("group", TagField::Description),
                &t("tags.group_form.description"),
                &mut description,
                focus == Some(TagField::Description),
            );
            if r.changed() {
                f.form.edit(F_DESCRIPTION, |x| x.description = description);
            }
            let mut single = d.single;
            if ui
                .checkbox(&mut single, t("tags.group_form.single"))
                .on_hover_text(t("tags.group_form.single_hint"))
                .changed()
            {
                f.form.edit(F_SINGLE, |x| x.single = single);
            }
            pane::section(ui, tokens, &t("tags.group_form.required"));
            ui.horizontal_wrapped(|ui| {
                for k in AccountKind::ALL {
                    let mut on = d.required_for.contains(&k);
                    if ui.checkbox(&mut on, t(k.label().as_str())).changed() {
                        f.form.edit(F_REQUIRED, |x| {
                            if on {
                                x.required_for.insert(k);
                            } else {
                                x.required_for.remove(&k);
                            }
                        });
                    }
                }
            });
            form::hint(ui, tokens, &t("tags.group_form.required_hint"));
        });
        problems_ui(ui, tokens, &f.problems, f.error.as_ref());
        if busy {
            form::progress_line(ui, tokens, &t("tags.form.saving"));
        }
        let editing = f.editing.is_some();
        save_buttons(ui, &mut save, &mut cancel, true);
        if editing {
            ui.add_space(4.0);
            if ui
                .button(t("tags.group_form.delete"))
                .on_hover_text(t("tags.group_form.delete_hint"))
                .clicked()
            {
                delete = true;
            }
        }
        if let Some(intent) = save {
            self.save_group(intent);
        }
        if delete {
            self.delete_group();
        }
        if cancel {
            self.close_panel(None);
        }
    }

    fn merge_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let names: Vec<String> = match &self.panel {
            Panel::Merge(m) => m
                .targets
                .iter()
                .map(|id| {
                    self.list
                        .tag(*id)
                        .map(|e| e.name.clone())
                        .unwrap_or_default()
                })
                .collect(),
            _ => return,
        };
        let source = match &self.panel {
            Panel::Merge(m) => self.list.tag(m.source).map(|e| e.name.clone()),
            _ => None,
        }
        .unwrap_or_default();
        let Panel::Merge(m) = &mut self.panel else {
            return;
        };
        let mut go = false;
        let mut cancel = false;
        form::heading(ui, tokens, &t_with("tags.merge.title", &[("tag", &source)]));
        ui.add_space(6.0);
        ui.label(t("tags.merge.explain"));
        let focus = std::mem::take(&mut m.focus);
        if let Some(i) = pane::picker(
            ui,
            egui::Id::unique("tags-merge-target"),
            &t("tags.merge.into"),
            &names,
            m.pick,
            focus,
        ) {
            m.pick = i;
        }
        form::hint(ui, tokens, &t("tags.merge.cant_undo"));
        if let Some(e) = &m.error {
            form::inline_error(ui, tokens, e);
        }
        if m.task.is_some() {
            form::progress_line(ui, tokens, &t("tags.merge.working"));
        }
        ui.horizontal_wrapped(|ui| {
            if crate::widgets::primary_button(ui, &t("tags.merge.confirm"))
                .on_hover_text(shortcut_hint("form.save_close"))
                .clicked()
            {
                go = true;
            }
            if ui.button(t("tags.form.cancel")).clicked() {
                cancel = true;
            }
        });
        if go {
            self.confirm_merge();
        }
        if cancel {
            self.close_panel(None);
        }
    }

    fn close_panel(&mut self, saved: Option<TagId>) {
        let keep = saved.or_else(|| self.selected());
        self.panel = Panel::None;
        self.sync_compact();
        self.place_cursor(keep);
    }
}

fn problems_ui(ui: &mut Ui, tokens: &Tokens, problems: &[TagProblem], error: Option<&ApiError>) {
    ui.add_space(6.0);
    for p in problems {
        ui.label(RichText::new(t(p.key().as_str())).color(tokens.warn_text));
    }
    if let Some(e) = error {
        form::inline_error(ui, tokens, e);
    }
}

fn save_buttons(ui: &mut Ui, save: &mut Option<SaveIntent>, cancel: &mut bool, new: bool) {
    ui.add_space(6.0);
    ui.horizontal_wrapped(|ui| {
        if crate::widgets::primary_button(ui, &t("tags.form.save_close"))
            .on_hover_text(shortcut_hint("form.save_close"))
            .clicked()
        {
            *save = Some(SaveIntent::SaveAndClose);
        }
        if new
            && ui
                .button(t("tags.form.save_new"))
                .on_hover_text(shortcut_hint("form.save_new"))
                .clicked()
        {
            *save = Some(SaveIntent::SaveAndNew);
        }
        if ui.button(t("tags.form.cancel")).clicked() {
            *cancel = true;
        }
    });
}

/// Saves into the demo tags at once (no engine).
fn demo_save_tag(link: &CompanyLink, existing: Option<Tag>, fields: TagFields) -> Versioned<Tag> {
    let mut d = link.demo_state();
    let tag = if let Some(mut x) = existing {
        x.fields = fields;
        x
    } else {
        let n = d.as_mut().map_or(0, |d| d.tags_mut().len());
        Tag {
            id: crate::company::demo::lists::tag_id(
                900_000
                    + u32::try_from(n).unwrap_or(0)
                    + d.as_ref()
                        .map_or(0, |d| u32::try_from(d.tags_gone.len()).unwrap_or(0)),
            ),
            fields,
            is_active: true,
            usage_count: Some(0),
            sort_order: Some(0),
            created_at: Some(crate::company::demo::DemoState::now()),
            updated_at: Some(crate::company::demo::DemoState::now()),
        }
    };
    if let Some(d) = d.as_mut() {
        let tags = d.tags_mut();
        match tags.iter_mut().find(|x| x.id == tag.id) {
            Some(x) => *x = tag.clone(),
            None => tags.push(tag.clone()),
        }
    }
    Versioned {
        version: RecordVersion::Fingerprint(tag.fingerprint()),
        record: tag,
    }
}

impl Screen for TagsScreen {
    fn title(&self) -> String {
        t(&format!("cmd.{KIND}"))
    }

    fn ui(&mut self, ui: &mut Ui) {
        self.parts = EnvParts::capture(ui.ctx());
        self.poll();
        self.sync_compact();
        self.follow_selection();
        let tokens = Tokens::of(ui.ctx());
        form::heading(ui, &tokens, &t("cmd.lists.tags"));
        ui.add_space(4.0);
        self.toolbar(ui);
        self.messages(ui, &tokens);
        ui.add_space(4.0);
        if !self.loaded {
            form::progress_line(ui, &tokens, &t("tags.loading"));
            return;
        }
        if matches!(self.panel, Panel::None) {
            self.list_ui(ui, &tokens);
        } else {
            ui.horizontal_top(|ui| {
                let left = (ui.available_width() - PANEL_WIDTH - 16.0).max(300.0);
                ui.vertical(|ui| {
                    ui.set_width(left);
                    self.list_ui(ui, &tokens);
                });
                ui.separator();
                ui.vertical(|ui| {
                    ui.set_width(PANEL_WIDTH);
                    egui::ScrollArea::vertical()
                        .id_salt("tags-panel-scroll")
                        .show(ui, |ui| self.panel_ui(ui, &tokens));
                });
            });
        }
    }

    fn dirty(&self) -> bool {
        self.panel.dirty()
    }

    fn scope(&self) -> Scope {
        // A grid screen: Esc arrives as `form.revert` (Q-P10d).
        Scope::Form
    }

    fn command(&mut self, id: CommandId) -> bool {
        match id.as_str() {
            "list.new" => self.open_new_tag(),
            "edit.edit_transaction" => self.open_edit_tag(self.selected()),
            "list.toggle_active" => self.toggle_active(),
            "list.show_inactive" => self.set_show_inactive(!self.filter.show_inactive),
            "list.merge" => self.open_merge(),
            "form.delete_item" => self.delete_tag(),
            "edit.find" => self.focus_search = true,
            "form.save_new" | "form.save_close" => {
                if matches!(self.panel, Panel::None) {
                    return false;
                }
                let intent = if id.as_str() == "form.save_new" {
                    SaveIntent::SaveAndNew
                } else {
                    SaveIntent::SaveAndClose
                };
                self.save_panel(intent);
            }
            "edit.undo" | "edit.redo" => {
                let undo = id.as_str() == "edit.undo";
                match &mut self.panel {
                    Panel::Tag(f) if undo => {
                        f.form.undo();
                    }
                    Panel::Tag(f) => {
                        f.form.redo();
                    }
                    Panel::Group(f) if undo => {
                        f.form.undo();
                    }
                    Panel::Group(f) => {
                        f.form.redo();
                    }
                    _ => return false,
                }
            }
            "form.revert" => match &mut self.panel {
                Panel::Tag(f) if f.form.is_dirty() && f.task.is_none() => {
                    f.form.revert_form();
                    f.problems.clear();
                    f.error = None;
                    f.focus = Some(TagField::Name);
                }
                Panel::Group(f) if f.form.is_dirty() && f.task.is_none() => {
                    f.form.revert_form();
                    f.problems.clear();
                    f.error = None;
                    f.focus = Some(TagField::Name);
                }
                Panel::Tag(_) | Panel::Group(_) | Panel::Merge(_) => self.close_panel(None),
                Panel::None if !self.filter.search.is_empty() => {
                    self.filter.search.clear();
                    let keep = self.selected();
                    self.refilter();
                    self.place_cursor(keep);
                }
                Panel::None => return false,
            },
            other => {
                let cmd = match other {
                    "form.prev_record" => cl_grid::GridCommand::PageUp,
                    "form.next_record" => cl_grid::GridCommand::PageDown,
                    _ => return false,
                };
                let label = String::new();
                let env = self
                    .parts
                    .env(&NoLookups, &label, GridHeight::Rows { min: 1, max: 1 });
                return self
                    .grid
                    .command(cmd, ModelRef::Browse(&mut self.model), &env)
                    .0
                    .yes();
            }
        }
        true
    }

    fn save(&mut self) -> bool {
        self.save_panel(SaveIntent::Save)
    }

    fn state(&self) -> Value {
        let selected = self.selected().and_then(|id| self.list.tag(id));
        let (dirty, problems, error, saving) = match &self.panel {
            Panel::Tag(f) => (
                f.form.is_dirty(),
                f.problems.clone(),
                f.error.is_some(),
                f.task.is_some(),
            ),
            Panel::Group(f) => (
                f.form.is_dirty(),
                f.problems.clone(),
                f.error.is_some(),
                f.task.is_some() || f.deleting.is_some(),
            ),
            Panel::Merge(m) => (false, Vec::new(), m.error.is_some(), m.task.is_some()),
            Panel::None => (false, Vec::new(), false, false),
        };
        let problems: Vec<&str> = problems
            .iter()
            .map(|p| p.key().as_str().rsplit('.').next().unwrap_or_default())
            .collect();
        let usage = self.selected_usage();
        json!({
            "screen": "tags",
            "loaded": self.loaded,
            "load_error": self.load_error.is_some(),
            "rows": self.model.len(),
            "total": self.list.len(),
            "groups": self.list.groups().len(),
            "show_inactive": self.filter.show_inactive,
            "searching": !self.filter.search.is_empty(),
            "cursor_row": self.snap.cursor_row,
            "focused": self.snap.focused,
            "selected_active": selected.map(|e| e.active),
            "selected_group": selected
                .and_then(|e| e.group)
                .and_then(|g| self.list.group(g))
                .map(|g| g.name.clone()),
            "usage_loaded": usage.is_some(),
            "usage_total": usage.map(|u| u.total),
            "usage_lines": usage.map(|u| u.transaction_lines),
            "merge_available": self.merge_available(),
            "panel": self.panel.name(),
            "dirty": dirty,
            "problem": problems.first(),
            "problems": problems,
            "error": error,
            "saving": saving,
            "warning": self.warn.map(|w| w.rsplit('.').next().unwrap_or_default()),
            "note": self.note.map(|n| n.rsplit('.').next().unwrap_or_default()),
            "required_groups": self.required.len(),
        })
    }
}

#[cfg(test)]
mod tests;
