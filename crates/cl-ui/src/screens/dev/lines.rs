//! The grid demo's split lines: a check with Account, Amount, Memo,
//! Service date, Client:Job, Billable and Tags lines (all synthetic), and
//! the lists its pickers use.

use std::borrow::Cow;

use cl_core::{CalendarDate, Money};
use cl_grid::{
    Align, CellInput, CellKind, CellPos, CellProblem, CellValue, Column, ColumnId, EditableModel,
    GridProblem, GridSpec, Line, ListId, LookupResult, Lookups, RecordOutcome, RowKey, RowKeyGen,
    RowModel, RowState, Source, Suggestion, TagChip, TextKey, WidthSpec,
};
use cl_model::amount_entry::AmountRules;
use cl_model::forms::FormState;
use cl_model::lookup::{LookupEntry, LookupIndex};

use crate::grid_form::LinesAdapter;

/// Account list.
pub const ACCOUNTS: ListId = ListId(1);
/// Client:Job list.
pub const JOBS: ListId = ListId(2);
/// Tag list.
pub const TAG_LIST: ListId = ListId(3);

/// Account column.
pub const ACCOUNT: ColumnId = ColumnId(1);
/// Amount column.
pub const AMOUNT: ColumnId = ColumnId(2);
/// Memo column.
pub const MEMO: ColumnId = ColumnId(3);
/// Service date column.
pub const SERVICE_DATE: ColumnId = ColumnId(4);
/// Client:Job column.
pub const JOB: ColumnId = ColumnId(5);
/// Billable column.
pub const BILLABLE: ColumnId = ColumnId(6);
/// Tags column.
pub const TAGS: ColumnId = ColumnId(7);

/// The Crew tag group (one crew per line).
pub const CREW_GROUP: u64 = 70;

const ACCOUNT_LIST: &[(u64, &str)] = &[
    (5100, "Materials:Plants"),
    (5110, "Materials:Mulch & stone"),
    (5120, "Materials:Hardscape"),
    (6200, "Fuel"),
    (6210, "Equipment rental"),
    (6300, "Repairs & maintenance"),
    (6400, "Subcontractors"),
];

const JOB_LIST: &[(u64, &str)] = &[
    (1001, "Hollister Family"),
    (1002, "Hollister Family:Fall cleanup"),
    (1003, "Ridgeview HOA"),
    (1004, "Ridgeview HOA:Spring planting"),
    (1005, "Bentonville Dental"),
];

const TAG_ENTRIES: &[(u64, &str, Option<u64>)] = &[
    (1, "Install", None),
    (2, "Crew A", Some(CREW_GROUP)),
    (3, "Crew B", Some(CREW_GROUP)),
    (4, "Warranty", None),
];

/// A picker cell's value.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Pick {
    /// Empty.
    #[default]
    None,
    /// A list entry.
    Id(u64),
    /// Text that matched nothing (flagged; blocks the save).
    Unresolved(String),
}

/// One split line.
#[derive(Debug, Clone, PartialEq)]
pub struct DemoLine {
    /// Stable row key (never an engine id).
    pub key: RowKey,
    /// Account.
    pub account: Pick,
    /// Amount (exact).
    pub amount: Option<Money>,
    /// Memo.
    pub memo: String,
    /// Service date.
    pub date: Option<CalendarDate>,
    /// Client:Job.
    pub job: Pick,
    /// Billable to the job.
    pub billable: bool,
    /// Tags.
    pub tags: Vec<TagChip>,
}

impl DemoLine {
    /// A blank line.
    #[must_use]
    pub fn blank(key: RowKey) -> DemoLine {
        DemoLine {
            key,
            account: Pick::None,
            amount: None,
            memo: String::new(),
            date: None,
            job: Pick::None,
            billable: false,
            tags: Vec::new(),
        }
    }

    /// Nothing entered.
    #[must_use]
    pub fn is_blank(&self) -> bool {
        *self == DemoLine::blank(self.key)
    }
}

/// The check: its amount and its lines (the form's value).
#[derive(Debug, Clone, PartialEq)]
pub struct DemoCheck {
    /// The check amount (header field).
    pub amount: Option<Money>,
    /// The lines, the blank entry line last.
    pub lines: Vec<DemoLine>,
}

/// The lists, ranked by `cl_model::lookup`.
pub struct DemoLists {
    accounts: LookupIndex,
    jobs: LookupIndex,
}

impl Default for DemoLists {
    fn default() -> Self {
        let entries = |list: &[(u64, &str)], numbered: bool| {
            LookupIndex::new(
                list.iter()
                    .map(|(id, name)| LookupEntry {
                        id: *id,
                        number: numbered.then(|| id.to_string()),
                        full_name: (*name).to_string(),
                        active: true,
                    })
                    .collect(),
            )
        };
        DemoLists {
            accounts: entries(ACCOUNT_LIST, true),
            jobs: entries(JOB_LIST, false),
        }
    }
}

fn account_label(id: u64) -> Option<String> {
    ACCOUNT_LIST
        .iter()
        .find(|(i, _)| *i == id)
        .map(|(i, n)| format!("{i} \u{b7} {n}"))
}

fn job_label(id: u64) -> Option<String> {
    JOB_LIST
        .iter()
        .find(|(i, _)| *i == id)
        .map(|(_, n)| (*n).to_string())
}

fn tag(id: u64) -> Option<Suggestion> {
    TAG_ENTRIES
        .iter()
        .find(|(i, _, _)| *i == id)
        .map(|(id, label, group)| Suggestion {
            id: *id,
            label: (*label).to_string(),
            group: *group,
        })
}

impl DemoLists {
    fn index(&self, list: ListId) -> Option<&LookupIndex> {
        match list {
            ACCOUNTS => Some(&self.accounts),
            JOBS => Some(&self.jobs),
            _ => None,
        }
    }

    fn label(list: ListId, id: u64) -> Option<String> {
        match list {
            ACCOUNTS => account_label(id),
            JOBS => job_label(id),
            TAG_LIST => tag(id).map(|t| t.label),
            _ => None,
        }
    }
}

impl Lookups for DemoLists {
    fn suggest(&self, list: ListId, query: &str, limit: usize) -> LookupResult {
        if list == TAG_LIST {
            let q = query.trim().to_lowercase();
            return LookupResult::Ready(
                TAG_ENTRIES
                    .iter()
                    .filter(|(_, l, _)| l.to_lowercase().contains(&q))
                    .take(limit)
                    .filter_map(|(id, _, _)| tag(*id))
                    .collect(),
            );
        }
        let Some(index) = self.index(list) else {
            return LookupResult::Ready(Vec::new());
        };
        LookupResult::Ready(
            index
                .rank(query, limit)
                .into_iter()
                .filter_map(|r| {
                    DemoLists::label(list, r.id).map(|label| Suggestion {
                        id: r.id,
                        label,
                        group: None,
                    })
                })
                .collect(),
        )
    }

    fn exact(&self, list: ListId, query: &str) -> Option<Suggestion> {
        if list == TAG_LIST {
            return TAG_ENTRIES
                .iter()
                .find(|(_, l, _)| l.eq_ignore_ascii_case(query.trim()))
                .and_then(|(id, _, _)| tag(*id));
        }
        let e = self.index(list)?.exact(query)?;
        Some(Suggestion {
            id: e.id,
            label: DemoLists::label(list, e.id)?,
            group: None,
        })
    }

    fn display(&self, list: ListId, id: u64) -> Option<Cow<'_, str>> {
        DemoLists::label(list, id).map(Cow::Owned)
    }

    fn all(&self, list: ListId) -> LookupResult {
        self.suggest(list, "", 100)
    }
}

fn column(id: ColumnId, header: TextKey, kind: CellKind, width: (u16, u16, u8)) -> Column {
    Column {
        id,
        header,
        placeholder: None,
        kind,
        line: Line::First,
        align: Align::Left,
        width: WidthSpec {
            min: width.0,
            preferred: width.1,
            flex: width.2,
        },
        editable: true,
        mono: false,
    }
}

/// The line grid's spec.
#[must_use]
pub fn spec() -> GridSpec {
    GridSpec::line_entry(vec![
        Column {
            placeholder: Some(TextKey("dev.grid_demo.col.account")),
            ..column(
                ACCOUNT,
                TextKey("dev.grid_demo.col.account"),
                CellKind::Picker {
                    list: ACCOUNTS,
                    add_new: true,
                },
                (120, 200, 2),
            )
        },
        Column {
            align: Align::Right,
            mono: true,
            ..column(
                AMOUNT,
                TextKey("dev.grid_demo.col.amount"),
                CellKind::Amount(AmountRules::MONEY),
                (90, 110, 0),
            )
        },
        column(
            MEMO,
            TextKey("dev.grid_demo.col.memo"),
            CellKind::Text { max_chars: 200 },
            (100, 180, 2),
        ),
        Column {
            mono: true,
            ..column(
                SERVICE_DATE,
                TextKey("dev.grid_demo.col.service_date"),
                CellKind::Date,
                (96, 104, 0),
            )
        },
        column(
            JOB,
            TextKey("dev.grid_demo.col.job"),
            CellKind::Picker {
                list: JOBS,
                add_new: true,
            },
            (120, 190, 2),
        ),
        Column {
            align: Align::Center,
            ..column(
                BILLABLE,
                TextKey("dev.grid_demo.col.billable"),
                CellKind::Checkbox,
                (60, 70, 0),
            )
        },
        column(
            TAGS,
            TextKey("dev.grid_demo.col.tags"),
            CellKind::Tags { list: TAG_LIST },
            (100, 160, 1),
        ),
    ])
}

/// Row keys and the Esc snapshot of the row being edited.
pub struct LinesState {
    /// Key generator.
    pub keys: RowKeyGen,
    snapshot: Option<DemoLine>,
}

impl LinesState {
    /// A new check: no amount and one blank entry line.
    #[must_use]
    pub fn new_check(&mut self) -> DemoCheck {
        DemoCheck {
            amount: None,
            lines: vec![DemoLine::blank(self.keys.next_key())],
        }
    }

    /// Fresh state.
    #[must_use]
    pub fn new() -> LinesState {
        LinesState {
            keys: RowKeyGen::new(),
            snapshot: None,
        }
    }
}

impl Default for LinesState {
    fn default() -> Self {
        LinesState::new()
    }
}

struct View<'a> {
    form: &'a mut FormState<DemoCheck>,
    state: &'a mut LinesState,
}

impl LinesAdapter<DemoCheck> for LinesState {
    fn model<'a>(&'a mut self, form: &'a mut FormState<DemoCheck>) -> Box<dyn EditableModel + 'a> {
        Box::new(View { form, state: self })
    }
}

impl View<'_> {
    fn lines(&self) -> &[DemoLine] {
        &self.form.current().lines
    }

    /// Exactly one blank entry line at the end: add one after a line is
    /// used, and drop the extra one left when a line is put back to blank
    /// (Esc on a row the owner had started).
    fn ensure_entry(lines: &mut Vec<DemoLine>, keys: &mut RowKeyGen) {
        while lines.len() >= 2 && lines[lines.len() - 2..].iter().all(DemoLine::is_blank) {
            lines.pop();
        }
        if lines.last().is_none_or(|l| !l.is_blank()) {
            lines.push(DemoLine::blank(keys.next_key()));
        }
    }
}

fn pick_value(p: &Pick, list: ListId) -> CellValue<'_> {
    match p {
        Pick::None => CellValue::Empty,
        Pick::Id(id) => CellValue::Ref {
            id: *id,
            display: Cow::Owned(DemoLists::label(list, *id).unwrap_or_default()),
        },
        Pick::Unresolved(t) => CellValue::Text(Cow::Borrowed(t)),
    }
}

impl RowModel for View<'_> {
    fn revision(&self) -> u64 {
        self.form.revision()
    }
    fn row_count(&self) -> usize {
        self.lines().len()
    }
    fn row_key(&self, index: usize) -> Option<RowKey> {
        self.lines().get(index).map(|l| l.key)
    }
    fn index_of(&self, key: RowKey) -> Option<usize> {
        self.lines().iter().position(|l| l.key == key)
    }
    fn row_state(&self, index: usize) -> RowState {
        let Some(l) = self.lines().get(index) else {
            return RowState::empty();
        };
        let mut s = RowState::empty();
        if index + 1 == self.lines().len() && l.is_blank() {
            s = s | RowState::ENTRY;
        }
        if matches!(l.account, Pick::Unresolved(_)) || matches!(l.job, Pick::Unresolved(_)) {
            s = s | RowState::ERROR;
        }
        s
    }
    fn cell(&self, index: usize, col: ColumnId) -> CellValue<'_> {
        let Some(l) = self.lines().get(index) else {
            return CellValue::Empty;
        };
        match col {
            ACCOUNT => pick_value(&l.account, ACCOUNTS),
            AMOUNT => l.amount.map_or(CellValue::Empty, CellValue::Money),
            MEMO => CellValue::Text(Cow::Borrowed(&l.memo)),
            SERVICE_DATE => l.date.map_or(CellValue::Empty, CellValue::Date),
            JOB => pick_value(&l.job, JOBS),
            BILLABLE => CellValue::Bool(l.billable),
            TAGS => CellValue::Tags(&l.tags),
            _ => CellValue::Empty,
        }
    }
    fn row_label(&self, _index: usize) -> String {
        String::new()
    }
}

fn set_pick(p: &mut Pick, value: CellInput) -> bool {
    match value {
        CellInput::Ref(id) => *p = Pick::Id(id),
        CellInput::Unresolved(t) => *p = Pick::Unresolved(t),
        CellInput::Clear => *p = Pick::None,
        _ => return false,
    }
    true
}

impl EditableModel for View<'_> {
    fn cell_editable(&self, index: usize, _col: ColumnId) -> bool {
        index < self.lines().len()
    }
    fn row_entered(&mut self, key: RowKey) {
        self.state.snapshot = self.lines().iter().find(|l| l.key == key).cloned();
    }
    fn set_cell(
        &mut self,
        pos: CellPos,
        value: CellInput,
        _source: Source,
    ) -> Result<(), CellProblem> {
        let keys = &mut self.state.keys;
        let mut ok = true;
        let applied = self.form.edit_step(|check| {
            let Some(l) = check.lines.iter_mut().find(|l| l.key == pos.row) else {
                return;
            };
            ok = match (pos.col, value) {
                (ACCOUNT, v) => set_pick(&mut l.account, v),
                (JOB, v) => set_pick(&mut l.job, v),
                (AMOUNT, CellInput::Money(m)) => {
                    l.amount = Some(m);
                    true
                }
                (AMOUNT | SERVICE_DATE, CellInput::Clear) => {
                    if pos.col == AMOUNT {
                        l.amount = None;
                    } else {
                        l.date = None;
                    }
                    true
                }
                (MEMO, CellInput::Text(t)) => {
                    l.memo = t;
                    true
                }
                (MEMO, CellInput::Clear) => {
                    l.memo.clear();
                    true
                }
                (SERVICE_DATE, CellInput::Date(d)) => {
                    l.date = Some(d);
                    true
                }
                (BILLABLE, CellInput::Bool(b)) => {
                    l.billable = b;
                    true
                }
                (TAGS, CellInput::Tags(t)) => {
                    l.tags = t;
                    true
                }
                (TAGS, CellInput::Clear) => {
                    l.tags.clear();
                    true
                }
                _ => false,
            };
            View::ensure_entry(&mut check.lines, keys);
        });
        if !applied {
            // A save is in flight: refuse, so the grid keeps the text.
            return Err(CellProblem::new(SAVING).at(pos.col));
        }
        if ok {
            Ok(())
        } else {
            Err(CellProblem::new(TextKey("dev.grid_demo.bad_value")).at(pos.col))
        }
    }
    fn revert_row(&mut self, key: RowKey) -> bool {
        let Some(snap) = self.state.snapshot.clone().filter(|s| s.key == key) else {
            return false;
        };
        if self.lines().contains(&snap) {
            return false;
        }
        let keys = &mut self.state.keys;
        self.form.edit_step(|check| {
            if let Some(l) = check.lines.iter_mut().find(|l| l.key == key) {
                *l = snap;
            }
            View::ensure_entry(&mut check.lines, keys);
        })
    }
    fn insert_line(&mut self, before: Option<RowKey>) -> Option<RowKey> {
        let key = self.state.keys.next_key();
        self.form
            .edit_step(|check| {
                let at = before
                    .and_then(|b| check.lines.iter().position(|l| l.key == b))
                    .unwrap_or(check.lines.len());
                check.lines.insert(at, DemoLine::blank(key));
            })
            .then_some(key)
    }
    fn delete_line(&mut self, key: RowKey) -> bool {
        if self.index_of(key).is_none() {
            return false;
        }
        let keys = &mut self.state.keys;
        self.form.edit_step(|check| {
            check.lines.retain(|l| l.key != key);
            View::ensure_entry(&mut check.lines, keys);
        })
    }
    fn record_row(&mut self, key: RowKey) -> RecordOutcome {
        let Some(i) = self.index_of(key) else {
            return RecordOutcome::Invalid(Vec::new());
        };
        let l = &self.lines()[i];
        let mut problems = Vec::new();
        if !matches!(l.account, Pick::Id(_)) {
            problems.push(CellProblem::new(TextKey("dev.grid_demo.need_account")).at(ACCOUNT));
        }
        if l.billable && !matches!(l.job, Pick::Id(_)) {
            problems.push(CellProblem::new(TextKey("dev.grid_demo.billable_needs_job")).at(JOB));
        }
        if !problems.is_empty() {
            return RecordOutcome::Invalid(problems);
        }
        RecordOutcome::Recorded {
            next: self.row_key(i + 1),
        }
    }
    fn validate_grid(&self) -> Vec<GridProblem> {
        validate(self.form.current())
    }
}

/// Shown when a cell is committed while the record is being saved.
pub const SAVING: TextKey = TextKey("grid.form.saving");

/// What a save sends: the check without its blank lines, so the entry
/// line is never part of a saved record (P10 plan §4, T23).
#[must_use]
pub fn for_save(check: &DemoCheck) -> DemoCheck {
    DemoCheck {
        amount: check.amount,
        lines: check
            .lines
            .iter()
            .filter(|l| !l.is_blank())
            .cloned()
            .collect(),
    }
}

/// The lines total; `None` when it does not fit (never rounded).
#[must_use]
pub fn lines_total(check: &DemoCheck) -> Option<Money> {
    Money::sum(check.lines.iter().filter_map(|l| l.amount)).ok()
}

/// The check's rules: every used line has an account, Billable needs a
/// Client:Job, and the lines total equals the check amount.
#[must_use]
pub fn validate(check: &DemoCheck) -> Vec<GridProblem> {
    let mut out = Vec::new();
    for l in check.lines.iter().filter(|l| !l.is_blank()) {
        if !matches!(l.account, Pick::Id(_)) {
            out.push(GridProblem {
                key: TextKey("dev.grid_demo.need_account"),
                fix: None,
                row: Some(l.key),
                col: Some(ACCOUNT),
            });
        }
        if l.billable && !matches!(l.job, Pick::Id(_)) {
            out.push(GridProblem {
                key: TextKey("dev.grid_demo.billable_needs_job"),
                fix: None,
                row: Some(l.key),
                col: Some(JOB),
            });
        }
    }
    if !balanced(check) {
        out.push(GridProblem {
            key: TextKey("dev.grid_demo.not_balanced"),
            fix: Some(TextKey("dev.grid_demo.not_balanced.fix")),
            row: None,
            col: None,
        });
    }
    out
}

/// The lines total equals the check amount (both present).
#[must_use]
pub fn balanced(check: &DemoCheck) -> bool {
    match (check.amount, lines_total(check)) {
        (Some(a), Some(t)) => a == t,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn money(s: &str) -> Money {
        Money::parse_wire(s).unwrap()
    }

    #[test]
    fn lists_rank_by_number_and_name() {
        let l = DemoLists::default();
        let LookupResult::Ready(a) = l.suggest(ACCOUNTS, "51", 5) else {
            panic!("ready");
        };
        assert_eq!(a[0].id, 5100);
        assert_eq!(a[0].label, "5100 \u{b7} Materials:Plants");
        assert_eq!(l.exact(ACCOUNTS, "6200").map(|s| s.id), Some(6200));
        let LookupResult::Ready(j) = l.suggest(JOBS, "Holl", 5) else {
            panic!("ready");
        };
        assert_eq!(j[0].id, 1001);
        let LookupResult::Ready(f) = l.suggest(JOBS, "Fall", 5) else {
            panic!("ready");
        };
        assert_eq!(f[0].id, 1002);
        assert_eq!(
            l.exact(TAG_LIST, "crew a").and_then(|t| t.group),
            Some(CREW_GROUP)
        );
        assert_eq!(l.display(TAG_LIST, 1).as_deref(), Some("Install"));
    }

    /// S2: while a save is in flight, commits and line edits are refused
    /// (never silently dropped).
    #[test]
    fn edits_while_saving_are_refused() {
        let mut st = LinesState::new();
        let mut form = FormState::new(st.new_check());
        let key = form.current().lines[0].key;
        form.edit_step(|c| c.amount = Some(money("1.00")));
        let _ = form.begin_save(cl_model::forms::SaveIntent::Save);
        assert!(form.is_saving());
        let mut model = st.model(&mut form);
        let pos = CellPos {
            row: key,
            col: MEMO,
        };
        let got = model.set_cell(pos, CellInput::Text("Mulch".into()), Source::Typed);
        assert_eq!(got.map_err(|p| p.key), Err(SAVING));
        assert_eq!(model.insert_line(None), None);
        assert!(!model.delete_line(key));
        drop(model);
        assert_eq!(form.current().lines[0].memo, "");
        assert_eq!(form.current().lines.len(), 1);
    }

    #[test]
    fn one_entry_line_at_the_end() {
        let mut keys = RowKeyGen::new();
        let mut lines: Vec<DemoLine> = (0..3).map(|_| DemoLine::blank(keys.next_key())).collect();
        lines[0].account = Pick::Id(5100);
        let kept = lines[1].key;
        View::ensure_entry(&mut lines, &mut keys);
        assert_eq!(lines.len(), 2, "the extra blank line goes");
        assert_eq!(lines[1].key, kept, "the earlier blank line stays");
        lines[1].account = Pick::Id(6200);
        View::ensure_entry(&mut lines, &mut keys);
        assert_eq!(lines.len(), 3);
        assert!(lines[2].is_blank());
    }

    #[test]
    fn rules_balance_and_billable_job() {
        let mut st = LinesState::new();
        let mut check = st.new_check();
        assert!(!balanced(&check));
        check.amount = Some(money("10.00"));
        check.lines[0].account = Pick::Id(5100);
        check.lines[0].amount = Some(money("10.00"));
        check.lines[0].billable = true;
        let p = validate(&check);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].key, TextKey("dev.grid_demo.billable_needs_job"));
        check.lines[0].job = Pick::Id(1002);
        assert_eq!(validate(&check), Vec::new());
        check.amount = Some(money("10.01"));
        assert_eq!(
            validate(&check)[0].key,
            TextKey("dev.grid_demo.not_balanced")
        );
        assert_eq!(lines_total(&check), Some(money("10.00")));
    }
}
