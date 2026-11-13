//! The Report Builder editor's state (ADD-03 A08, plan §3.1): the
//! definition being edited, the memorized row it came from, where it
//! started, the selection, undo and redo, and the problems to show.
//!
//! No egui, no I/O. Every change is one operation that pushes an undo
//! snapshot and re-validates; the engine computes every figure.
//!
//! Rules (plan §3.1, Q-A08o, Q-A08p, Q-A08q):
//! - every change is one [`Op`] through [`BuilderEditor::apply`]: it
//!   pushes an undo snapshot (100 deep), clears redo and the engine's
//!   problem, and re-validates;
//! - invalid states are accepted (a formula left below itself is a
//!   problem, not a refusal); only structure is refused ([`Refusal`]);
//! - dirty compares structure (definition, name, memorized group), never
//!   undo depth; custom From/To, open paths and the selected tab are
//!   session state and never make it dirty;
//! - owner text is trimmed when committed; lengths count UTF-16 units (the
//!   rules report them).

use std::collections::BTreeSet;

use cl_core::Timestamp;
use cl_core::report::{Adjusting, Basis};

use super::outline::{self, OutlineError};
use super::problems::{self, Problem, ProblemCode};
use super::schema::{
    BuilderKind, Columns, DateMode, Dates, Display, Formula, Group, GroupBy, Measure, Node,
    Section, SourceKey, Spacer, TextRow,
};
use super::validate::limits;
use super::{BuilderDefinition, MAX_BYTES, SCHEMA_VERSION, to_text};

/// Where an editor's report started (the hint's "Started from ..." link).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Origin {
    /// A blank report in Advanced.
    Blank,
    /// Guided mode's questions (Q-A08a).
    Guided,
    /// A ready-made report (`/starters` key).
    Starter(String),
    /// Customize on a standard report: its catalog key (P28), opened on
    /// its mapped starter (Q-A08g).
    Standard(String),
    /// A memorized Report Builder report.
    Memorized,
    /// A report read from a `.clreport` file, not memorized yet (A09
    /// WP-A): its Memorize is sent with `origin: 'import'`.
    Imported,
}

/// The kind of the memorized row an editor came from. The engine never
/// changes a row's kind (PUT refuses), so Save changes works only on
/// `Builder` rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SavedKind {
    /// A Report Builder row (`kind: 'builder'`).
    Builder,
    /// A memorized standard report (`kind: 'standard'`): Customize opens
    /// its starter, and only Memorize saves (a new row).
    Standard,
}

/// The memorized row behind an editor, as last read or saved.
#[derive(Clone, PartialEq, Eq)]
pub struct Saved {
    /// The row's id.
    pub id: String,
    /// Its version, sent with the next change (409 when stale).
    pub version: u64,
    /// Its kind.
    pub kind: SavedKind,
    /// True when this user may change it (the engine's `canEdit`: the
    /// author, an owner or a super admin, Q-310).
    pub can_edit: bool,
    /// The definition as saved (dirty compares with it). For a
    /// [`SavedKind::Standard`] row, whose body is not a builder definition,
    /// it is the mapped starter's definition the editor opened on (with the
    /// row's dates and basis applied), so an untouched Customize is not
    /// dirty; Save changes stays off for that kind.
    pub def: BuilderDefinition,
    /// The memorized name (owner data).
    pub name: String,
    /// Its memorized-report group.
    pub group_id: Option<String>,
    /// Who memorized it (owner data).
    pub author: Option<String>,
    /// Last change (the hint's "Saved hh:mm").
    pub updated_at: Option<Timestamp>,
}

// Names and captions are owner data: Debug shows only the row's identity.
impl std::fmt::Debug for Saved {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Saved")
            .field("id", &self.id)
            .field("version", &self.version)
            .field("kind", &self.kind)
            .field("can_edit", &self.can_edit)
            .finish_non_exhaustive()
    }
}

pub mod choices;

/// How many undo steps an editor keeps (Q-A08p).
pub const UNDO_DEPTH: usize = 100;

/// The six kinds of outline row.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RowKind {
    /// A section over a source.
    Section,
    /// A heading over other rows, with a total.
    Group,
    /// Adds and subtracts earlier rows.
    Formula,
    /// A line of text.
    Text,
    /// An empty line.
    Blank,
    /// A page break.
    PageBreak,
}

impl RowKind {
    /// Every kind, in the Add menu's order.
    pub const ALL: [RowKind; 6] = [
        RowKind::Section,
        RowKind::Group,
        RowKind::Formula,
        RowKind::Text,
        RowKind::Blank,
        RowKind::PageBreak,
    ];

    /// The kind of `node`.
    #[must_use]
    pub const fn of(node: &Node) -> RowKind {
        match node {
            Node::Section(_) => RowKind::Section,
            Node::Group(_) => RowKind::Group,
            Node::Formula(_) => RowKind::Formula,
            Node::Text(_) => RowKind::Text,
            Node::Blank(_) => RowKind::Blank,
            Node::PageBreak(_) => RowKind::PageBreak,
        }
    }

    /// The first letter of its new ids (`s7`, `f3`, ...).
    const fn prefix(self) -> &'static str {
        match self {
            RowKind::Section => "s",
            RowKind::Group => "g",
            RowKind::Formula => "f",
            RowKind::Text => "t",
            RowKind::Blank => "b",
            RowKind::PageBreak => "p",
        }
    }

    /// True for rows whose caption may not be empty.
    #[must_use]
    pub const fn has_caption(self) -> bool {
        matches!(self, RowKind::Section | RowKind::Group | RowKind::Formula)
    }
}

/// One change to the report. Each is one undo step.
#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    /// A new row after the selection (into the selected group when it is
    /// shown open). `caption` is already in the owner's language (the
    /// UI's "New section", ...).
    Add {
        /// Its kind.
        kind: RowKind,
        /// Its caption (sections, groups, formulas) or text (text rows).
        caption: String,
    },
    /// Removes a row and its rows; formulas drop terms naming them and a
    /// "% of" naming one is cleared (Q-A08o).
    Remove(String),
    /// Swaps a row with the one before it (Alt+Up).
    MoveUp(String),
    /// Swaps a row with the one after it (Alt+Down).
    MoveDown(String),
    /// Puts a row into the group directly above (Tab).
    Indent(String),
    /// Moves a row out of its group, right after it (Shift+Tab).
    Outdent(String),
    /// Renames a row (F2): its caption, or a text row's text.
    Rename {
        /// The row.
        id: String,
        /// The text typed.
        text: String,
    },
    /// Replaces a row's settings (the Row tab); a group keeps its rows.
    Edit(Node),
    /// Replaces the columns (the Columns tab).
    Columns(Columns),
    /// Date mode and preset (saved, Q-A08q).
    Dates(Dates),
    /// Basis (`None`: the company default, left out).
    Basis(Option<Basis>),
    /// The adjusting-entries lens.
    Adjusting(Option<Adjusting>),
    /// The Report name: the memorized name and the title (Q-A08r).
    Name(String),
    /// The memorized-report group.
    MemorizedGroup(Option<String>),
    /// Another definition wholesale: "Load their version" (Q-A08n),
    /// Guided's "Open in Advanced".
    Load {
        /// The definition.
        def: Box<BuilderDefinition>,
        /// Its name.
        name: String,
        /// Its memorized group.
        group_id: Option<String>,
    },
}

/// Why an operation was not made (structure only; everything else is a
/// problem to fix).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// 200 rows already.
    TooManyRows,
    /// It would sit deeper than 8 levels.
    TooDeep,
    /// Indent needs a group directly above.
    NoGroupAbove,
    /// 24 transaction columns already.
    TooManyFields,
    /// A caption can't be empty: the field goes back to what it was.
    EmptyCaption,
    /// No such row.
    NotFound,
}

impl Refusal {
    /// The i18n key of its message.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Refusal::TooManyRows => "builder.refused.too_many_rows",
            Refusal::TooDeep => "builder.refused.too_deep",
            Refusal::NoGroupAbove => "builder.refused.no_group_above",
            Refusal::TooManyFields => "builder.refused.too_many_fields",
            Refusal::EmptyCaption => "builder.refused.empty_caption",
            Refusal::NotFound => "builder.refused.not_found",
        }
    }
}

impl From<OutlineError> for Refusal {
    fn from(e: OutlineError) -> Refusal {
        match e {
            OutlineError::NotFound => Refusal::NotFound,
            OutlineError::TooDeep => Refusal::TooDeep,
            OutlineError::NoGroupAbove => Refusal::NoGroupAbove,
        }
    }
}

/// What else a removal changed (the status line, Q-A08o).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Removed {
    /// Rows removed, the row itself included.
    pub rows: usize,
    /// Formula terms dropped.
    pub terms: usize,
    /// "% of" named a removed row and was cleared.
    pub percent_cleared: bool,
}

/// What an operation did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Applied {
    /// False when nothing changed (no undo step was taken).
    pub changed: bool,
    /// For a removal: what else changed.
    pub removed: Option<Removed>,
}

/// One undo step: the whole definition (Q-A08p).
#[derive(Clone, Debug, PartialEq)]
struct Snapshot {
    def: BuilderDefinition,
    name: String,
    group_id: Option<String>,
    selection: Option<String>,
}

/// One row as the outline shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutlineRow {
    /// Its id.
    pub id: String,
    /// Its kind.
    pub kind: RowKind,
    /// 1 = top level.
    pub depth: usize,
    /// A group with rows.
    pub has_children: bool,
    /// A group shown folded.
    pub folded: bool,
}

/// A definition with no rows (Blank in Advanced, Guided's start).
#[must_use]
pub fn blank_definition() -> BuilderDefinition {
    BuilderDefinition {
        schema_version: SCHEMA_VERSION,
        kind: BuilderKind::Builder,
        title: None,
        dates: Dates {
            mode: DateMode::Range,
            preset: Some("this_month".into()),
        },
        basis: None,
        adjusting: None,
        columns: Columns::default(),
        nodes: Vec::new(),
    }
}

/// A new row's settings.
fn new_node(kind: RowKind, id: String, caption: String) -> Node {
    match kind {
        RowKind::Section => Node::Section(Section {
            id,
            caption,
            source: SourceKey::Gl,
            measure: Measure::Amount,
            polarity: None,
            filters: Vec::new(),
            group_by: vec![GroupBy {
                key: "account".into(),
                tag_group_id: None,
            }],
            display: Display::Summary,
            default_expanded: false,
            show_total: true,
            total_caption: None,
            hide_zero: true,
            earnings: None,
        }),
        RowKind::Group => Node::Group(Group {
            id,
            caption,
            children: Vec::new(),
            show_total: true,
            total_caption: None,
        }),
        RowKind::Formula => Node::Formula(Formula {
            id,
            caption,
            terms: Vec::new(),
        }),
        RowKind::Text => Node::Text(TextRow { id, text: caption }),
        RowKind::Blank => Node::Blank(Spacer { id }),
        RowKind::PageBreak => Node::PageBreak(Spacer { id }),
    }
}

/// A fresh id for a row of `kind`: unique in the tree, matching
/// `^[a-z0-9_-]{1,32}$`, never `section`.
fn new_id(nodes: &[Node], kind: RowKind) -> String {
    let used: BTreeSet<String> = outline::walk(nodes)
        .iter()
        .map(|v| v.node.id().to_string())
        .collect();
    (1..=limits::NODES + 1)
        .map(|n| format!("{}{n}", kind.prefix()))
        .find(|id| !used.contains(id))
        // At most 200 rows exist, so one of 201 numbers is free.
        .unwrap_or_default()
}

/// Owner text as committed: trimmed at both ends.
fn tidy_text(s: &str) -> String {
    s.trim().to_string()
}

/// `def` with its title set to the Report name (Q-A08r): the name is the
/// memorized name and `definition.title`, so the two never differ in the
/// editor or in what it saves. An empty name leaves no title.
#[must_use]
pub fn titled(mut def: BuilderDefinition, name: &str) -> BuilderDefinition {
    let name = tidy_text(name);
    def.title = (!name.is_empty()).then_some(name);
    def
}

/// A row's settings with its owner text trimmed.
///
/// # Errors
/// [`Refusal::EmptyCaption`] for a caption left empty.
fn tidy_node(node: Node) -> Result<Node, Refusal> {
    let caption = |c: &str| {
        let c = tidy_text(c);
        if c.is_empty() {
            Err(Refusal::EmptyCaption)
        } else {
            Ok(c)
        }
    };
    let total = |c: Option<String>| c.map(|c| tidy_text(&c)).filter(|c| !c.is_empty());
    Ok(match node {
        Node::Section(mut s) => {
            s.caption = caption(&s.caption)?;
            s.total_caption = total(s.total_caption);
            Node::Section(s)
        }
        Node::Group(mut g) => {
            g.caption = caption(&g.caption)?;
            g.total_caption = total(g.total_caption);
            Node::Group(g)
        }
        Node::Formula(mut f) => {
            f.caption = caption(&f.caption)?;
            Node::Formula(f)
        }
        Node::Text(mut t) => {
            t.text = tidy_text(&t.text);
            Node::Text(t)
        }
        other => other,
    })
}

/// The node at an index path.
fn node_at<'a>(nodes: &'a [Node], path: &[usize]) -> Option<&'a Node> {
    let (first, rest) = path.split_first()?;
    let mut node = nodes.get(*first)?;
    for &i in rest {
        node = node.children().get(i)?;
    }
    Some(node)
}

/// The row to select after removing the one at `path`: the row now in its
/// place, else the one before, else its group.
fn after_removal(nodes: &[Node], path: &[usize]) -> Option<String> {
    let (&idx, parent) = path.split_last()?;
    let list: &[Node] = if parent.is_empty() {
        nodes
    } else {
        node_at(nodes, parent)?.children()
    };
    list.get(idx)
        .or_else(|| idx.checked_sub(1).and_then(|i| list.get(i)))
        .map(|n| n.id().to_string())
        .or_else(|| node_at(nodes, parent).map(|n| n.id().to_string()))
}

/// The Report Builder editor: one per window.
#[derive(Clone, Debug)]
pub struct BuilderEditor {
    def: BuilderDefinition,
    name: String,
    group_id: Option<String>,
    saved: Option<Saved>,
    origin: Origin,
    selection: Option<String>,
    folded: BTreeSet<String>,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    problems: Vec<Problem>,
    engine_problem: Option<Problem>,
}

impl BuilderEditor {
    /// An editor on `def`, named `name`, from `origin`; `saved` is the
    /// memorized row behind it (its group is the editor's).
    #[must_use]
    pub fn new(
        def: BuilderDefinition,
        name: &str,
        origin: Origin,
        saved: Option<Saved>,
    ) -> BuilderEditor {
        let group_id = saved.as_ref().and_then(|s| s.group_id.clone());
        let selection = def.nodes.first().map(|n| n.id().to_string());
        // The title is the name (Q-A08r), in the baseline too: a row saved
        // with another title does not open with changes.
        let def = titled(def, name);
        let saved = saved.map(|mut s| {
            s.def = titled(s.def, &s.name);
            s
        });
        let mut e = BuilderEditor {
            def,
            name: tidy_text(name),
            group_id,
            saved,
            origin,
            selection,
            folded: BTreeSet::new(),
            undo: Vec::new(),
            redo: Vec::new(),
            problems: Vec::new(),
            engine_problem: None,
        };
        e.revalidate();
        e
    }

    /// A blank report in Advanced.
    #[must_use]
    pub fn blank() -> BuilderEditor {
        BuilderEditor::new(blank_definition(), "", Origin::Blank, None)
    }

    /// The definition being edited.
    #[must_use]
    pub fn definition(&self) -> &BuilderDefinition {
        &self.def
    }

    /// The Report name (owner data).
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The memorized-report group.
    #[must_use]
    pub fn group_id(&self) -> Option<&str> {
        self.group_id.as_deref()
    }

    /// The memorized row behind it.
    #[must_use]
    pub fn saved(&self) -> Option<&Saved> {
        self.saved.as_ref()
    }

    /// Where it started.
    #[must_use]
    pub fn origin(&self) -> &Origin {
        &self.origin
    }

    /// The selected row.
    #[must_use]
    pub fn selection(&self) -> Option<&str> {
        self.selection.as_deref()
    }

    /// The selected row's node.
    #[must_use]
    pub fn selected_node(&self) -> Option<&Node> {
        outline::find(&self.def.nodes, self.selection.as_deref()?)
    }

    /// Rows at every depth (the "N of 200 rows" count).
    #[must_use]
    pub fn row_count(&self) -> usize {
        outline::count(&self.def.nodes)
    }

    /// True when Add can make a row.
    #[must_use]
    pub fn can_add(&self) -> bool {
        self.row_count() < limits::NODES
    }

    /// Problems to fix: the client rules', then the engine's last one.
    #[must_use]
    pub fn problems(&self) -> Vec<Problem> {
        let mut all = self.problems.clone();
        all.extend(self.engine_problem.clone());
        all
    }

    /// The client rules' problems only.
    #[must_use]
    pub fn client_problems(&self) -> &[Problem] {
        &self.problems
    }

    /// The engine's problem since the last edit.
    #[must_use]
    pub fn engine_problem(&self) -> Option<&Problem> {
        self.engine_problem.as_ref()
    }

    /// True when a row has a problem (its warning tint).
    #[must_use]
    pub fn row_has_problem(&self, id: &str) -> bool {
        self.problems
            .iter()
            .chain(self.engine_problem.iter())
            .any(|p| p.node_id.as_deref() == Some(id))
    }

    /// True when the definition is over the engine's 256 KB.
    #[must_use]
    pub fn too_large(&self) -> bool {
        to_text(&self.def).map_or(true, |t| t.len() > MAX_BYTES)
    }

    /// True when it may be run (Run report, the preview): no problem the
    /// desktop sees, none the engine reported since the last edit, and
    /// within the size cap.
    #[must_use]
    pub fn can_run(&self) -> bool {
        self.problems.is_empty() && self.engine_problem.is_none() && !self.too_large()
    }

    /// The engine refused the definition (`DEFINITION_INVALID {path,
    /// code}` from a run or a save): shown until the next edit.
    pub fn set_engine_problem(&mut self, path: &str, code: &str) {
        self.engine_problem = Some(problems::map(
            &self.def,
            path,
            ProblemCode::from_engine(code),
        ));
    }

    /// True when there are unsaved changes.
    #[must_use]
    pub fn dirty(&self) -> bool {
        match &self.saved {
            None => !self.def.nodes.is_empty(),
            Some(s) => s.def != self.def || s.name != self.name || s.group_id != self.group_id,
        }
    }

    /// True when Save changes is offered: a builder row this user may
    /// change, with changes (Q-A08c).
    #[must_use]
    pub fn can_save_changes(&self) -> bool {
        self.saved
            .as_ref()
            .is_some_and(|s| s.kind == SavedKind::Builder && s.can_edit)
            && self.dirty()
    }

    /// The row was saved (Memorize, Save changes): it is the new baseline.
    /// Undo is kept (Q-A08p).
    pub fn mark_saved(&mut self, saved: Saved) {
        self.group_id.clone_from(&saved.group_id);
        self.saved = Some(saved);
        self.origin = Origin::Memorized;
    }

    /// The engine said only the author or an admin may change the row
    /// (403 `NOT_AUTHOR`): Save changes turns off, the edits stay.
    pub fn not_author(&mut self) {
        if let Some(s) = self.saved.as_mut() {
            s.can_edit = false;
        }
    }

    /// The row is gone (404): the next save is a Memorize.
    pub fn forget_saved(&mut self) {
        self.saved = None;
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            def: self.def.clone(),
            name: self.name.clone(),
            group_id: self.group_id.clone(),
            selection: self.selection.clone(),
        }
    }

    fn restore(&mut self, s: Snapshot) {
        self.def = s.def;
        self.name = s.name;
        self.group_id = s.group_id;
        self.selection = s
            .selection
            .filter(|id| outline::find(&self.def.nodes, id).is_some());
        self.engine_problem = None;
        self.revalidate();
    }

    fn revalidate(&mut self) {
        self.problems = problems::of(&self.def);
    }

    /// True when there is a step to undo.
    #[must_use]
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    /// True when there is a step to redo.
    #[must_use]
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Undoes the last change. Returns false when there is none.
    pub fn undo(&mut self) -> bool {
        let Some(s) = self.undo.pop() else {
            return false;
        };
        self.redo.push(self.snapshot());
        self.restore(s);
        true
    }

    /// Redoes the last undone change. Returns false when there is none.
    pub fn redo(&mut self) -> bool {
        let Some(s) = self.redo.pop() else {
            return false;
        };
        self.undo.push(self.snapshot());
        self.restore(s);
        true
    }

    /// Makes one change (one undo step when it changes anything).
    ///
    /// # Errors
    /// [`Refusal`] for a change the structure can't take; nothing changes.
    pub fn apply(&mut self, op: Op) -> Result<Applied, Refusal> {
        let before = self.snapshot();
        let mut next = before.clone();
        let mut applied = Applied::default();
        self.change(&mut next, op, &mut applied)?;
        if next == before {
            return Ok(applied);
        }
        self.undo.push(before);
        if self.undo.len() > UNDO_DEPTH {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.restore(next);
        applied.changed = true;
        Ok(applied)
    }

    fn change(&mut self, s: &mut Snapshot, op: Op, applied: &mut Applied) -> Result<(), Refusal> {
        let nodes = &mut s.def.nodes;
        match op {
            Op::Add { kind, caption } => {
                if outline::count(nodes) >= limits::NODES {
                    return Err(Refusal::TooManyRows);
                }
                let caption = tidy_text(&caption);
                if kind.has_caption() && caption.is_empty() {
                    return Err(Refusal::EmptyCaption);
                }
                let id = new_id(nodes, kind);
                let sel = s.selection.as_deref();
                let open = sel.is_some_and(|x| !self.folded.contains(x));
                outline::insert_after(nodes, sel, open, new_node(kind, id.clone(), caption))?;
                s.selection = Some(id);
            }
            Op::Remove(id) => self.remove(s, &id, applied)?,
            Op::MoveUp(id) => {
                outline::shift(nodes, &id, true);
            }
            Op::MoveDown(id) => {
                outline::shift(nodes, &id, false);
            }
            Op::Indent(id) => {
                outline::indent(nodes, &id)?;
                // The row stays in sight: its new group opens.
                if let Some(p) = outline::locate(nodes, &id)
                    && let Some((_, parent)) = p.split_last()
                    && let Some(group) = node_at(nodes, parent)
                {
                    self.folded.remove(group.id());
                }
            }
            Op::Outdent(id) => {
                outline::outdent(nodes, &id);
            }
            Op::Rename { id, text } => {
                let kind = outline::find(nodes, &id)
                    .map(RowKind::of)
                    .ok_or(Refusal::NotFound)?;
                let text = tidy_text(&text);
                if kind.has_caption() && text.is_empty() {
                    return Err(Refusal::EmptyCaption);
                }
                if !outline::rename(nodes, &id, &text) {
                    return Err(Refusal::NotFound);
                }
            }
            Op::Edit(node) => {
                let node = tidy_node(node)?;
                // A section that starts showing its transactions while the
                // report has no transaction columns gets the usual ones in
                // the same step, so each line says what it is (QA-7).
                let shows_lines = |n: &Node| matches!(n, Node::Section(x) if x.display != crate::report_def::Display::Summary);
                let first_lines = shows_lines(&node)
                    && !outline::find(nodes, node.id()).is_some_and(shows_lines)
                    && s.def.columns.detail_fields.is_empty();
                let source = match &node {
                    Node::Section(x) => Some(x.source),
                    _ => None,
                };
                if !outline::replace(nodes, node) {
                    return Err(Refusal::NotFound);
                }
                if first_lines && let Some(source) = source {
                    s.def.columns.detail_fields = super::guided::default_detail_fields(source);
                }
            }
            Op::Columns(mut c) => {
                if c.detail_fields.len() > limits::DETAIL_FIELDS {
                    return Err(Refusal::TooManyFields);
                }
                c.headers = c
                    .headers
                    .into_iter()
                    .map(|(k, v)| (k, tidy_text(&v)))
                    .filter(|(_, v)| !v.is_empty())
                    .collect();
                s.def.columns = c;
            }
            Op::Dates(d) => s.def.dates = d,
            Op::Basis(b) => s.def.basis = b,
            Op::Adjusting(a) => s.def.adjusting = a,
            Op::Name(n) => {
                let n = tidy_text(&n);
                s.def.title = (!n.is_empty()).then(|| n.clone());
                s.name = n;
            }
            Op::MemorizedGroup(g) => s.group_id = g,
            Op::Load {
                def,
                name,
                group_id,
            } => {
                s.def = titled(*def, &name);
                s.name = tidy_text(&name);
                s.group_id = group_id;
            }
        }
        Ok(())
    }

    fn remove(&mut self, s: &mut Snapshot, id: &str, applied: &mut Applied) -> Result<(), Refusal> {
        let nodes = &mut s.def.nodes;
        let path = outline::locate(nodes, id).ok_or(Refusal::NotFound)?;
        let gone = outline::remove(nodes, id).ok_or(Refusal::NotFound)?;
        let terms = outline::drop_terms(nodes, &gone);
        let percent_cleared = s
            .def
            .columns
            .percent_of
            .as_ref()
            .is_some_and(|p| gone.contains(p));
        if percent_cleared {
            s.def.columns.percent_of = None;
        }
        for g in &gone {
            self.folded.remove(g);
        }
        applied.removed = Some(Removed {
            rows: gone.len(),
            terms,
            percent_cleared,
        });
        s.selection = after_removal(&s.def.nodes, &path);
        Ok(())
    }

    // ---- Outline navigation: session state, never an undo step ----

    /// Selects a row (`None`, or an unknown id: nothing).
    pub fn select(&mut self, id: Option<&str>) {
        self.selection = id
            .filter(|id| outline::find(&self.def.nodes, id).is_some())
            .map(str::to_string);
    }

    /// True when the group `id` is shown folded.
    #[must_use]
    pub fn is_folded(&self, id: &str) -> bool {
        self.folded.contains(id)
    }

    /// Folds or unfolds the group `id`.
    pub fn set_folded(&mut self, id: &str, folded: bool) {
        if folded {
            self.folded.insert(id.to_string());
        } else {
            self.folded.remove(id);
        }
    }

    /// The rows the outline shows: rows under a folded group are hidden.
    #[must_use]
    pub fn visible_rows(&self) -> Vec<OutlineRow> {
        let mut out = Vec::new();
        let mut hidden_below: Option<usize> = None;
        for v in outline::walk(&self.def.nodes) {
            if let Some(d) = hidden_below {
                if v.depth > d {
                    continue;
                }
                hidden_below = None;
            }
            let folded = self.folded.contains(v.node.id());
            if folded {
                hidden_below = Some(v.depth);
            }
            out.push(OutlineRow {
                id: v.node.id().to_string(),
                kind: RowKind::of(v.node),
                depth: v.depth,
                has_children: !v.node.children().is_empty(),
                folded,
            });
        }
        out
    }

    /// Moves the selection to the next (`down`) or previous shown row.
    pub fn select_step(&mut self, down: bool) {
        let rows = self.visible_rows();
        let at = self
            .selection
            .as_deref()
            .and_then(|s| rows.iter().position(|r| r.id == s));
        let next = match (at, down) {
            (None, _) => rows.first(),
            (Some(i), true) => rows.get(i + 1).or(rows.get(i)),
            (Some(i), false) => rows.get(i.saturating_sub(1)),
        };
        self.selection = next.map(|r| r.id.clone());
    }

    /// Left in the outline: folds an open group with rows, else selects
    /// the group the row is in.
    pub fn fold_or_parent(&mut self) {
        let Some(sel) = self.selection.clone() else {
            return;
        };
        let has_rows = self
            .selected_node()
            .is_some_and(|n| !n.children().is_empty());
        if has_rows && !self.folded.contains(&sel) {
            self.folded.insert(sel);
            return;
        }
        if let Some(p) = outline::locate(&self.def.nodes, &sel)
            && let Some((_, parent)) = p.split_last()
            && let Some(group) = node_at(&self.def.nodes, parent)
        {
            self.selection = Some(group.id().to_string());
        }
    }

    /// Right in the outline: unfolds a folded group, else selects an open
    /// group's first row.
    pub fn unfold_or_child(&mut self) {
        let Some(sel) = self.selection.clone() else {
            return;
        };
        if self.folded.remove(&sel) {
            return;
        }
        if let Some(first) = self
            .selected_node()
            .and_then(|n| n.children().first())
            .map(|n| n.id().to_string())
        {
            self.selection = Some(first);
        }
    }
}

#[cfg(test)]
mod tests;
