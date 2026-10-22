//! Bulk Entry's grid: one P10 line grid showing the selected section's rows
//! (plan §1). Each section type has its own columns (WP-D1):
//!
//! - Checks: Date · No. · Print later · Pay to · Account · Amount · Memo ·
//!   Tag;
//! - Expenses, card charges and card credits: Date · Ref no. · Payee ·
//!   Account · Amount · Memo · Tag;
//! - Deposits: Date · Ref no. · Received from · From account · Amount ·
//!   Memo · Tag;
//! - Journal entries: Date · Entry No. · Account · Debit · Credit · Name ·
//!   Memo · Tag.
//!
//! The grid stores what the owner typed into the batch's
//! `cl_model::forms::batch::RowDraft`s. A name or account that is not in
//! the lists is kept as typed text for the engine's check ("Not in your
//! lists", Q-P15Ad). Amounts are exact `Money` rounded to the cent (Q-232);
//! nothing is added up here.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::Arc;

use cl_core::ids::AccountId;
use cl_core::{CalendarDate, Money};
use cl_grid::{
    Align, CellInput, CellKind, CellPos, CellProblem, CellValue, Column, ColumnId, EditableModel,
    GridSpec, Line, Lookups, RecordOutcome, RowHeight, RowKey, RowKeyGen, RowModel, RowNote,
    RowState, Source, TextKey, WidthSpec,
};
use cl_model::amount_entry::AmountRules;
use cl_model::forms::FormState;
use cl_model::forms::batch::{BulkBatch, Field, RowDraft, Section, SectionKind, parse_amount};

use crate::grid_form::LinesAdapter;
use crate::screens::register::lists::{ACCOUNTS, NAMES, RegisterLists, TAGS};

const BAD_VALUE: TextKey = TextKey("register.entry.bad_value");
const SAVING: TextKey = TextKey("grid.form.saving");

/// The column of a field (ids are stable across section types).
#[must_use]
pub(crate) const fn column_of(f: Field) -> ColumnId {
    ColumnId(match f {
        Field::Date => 1,
        Field::Number => 2,
        Field::PrintLater => 3,
        Field::Contact => 4,
        Field::Account => 5,
        Field::Amount => 6,
        Field::Debit => 7,
        Field::Credit => 8,
        Field::Memo => 9,
        Field::Tag => 10,
    })
}

/// The field of a column.
#[must_use]
pub(crate) fn field_of(c: ColumnId) -> Option<Field> {
    [
        Field::Date,
        Field::Number,
        Field::PrintLater,
        Field::Contact,
        Field::Account,
        Field::Amount,
        Field::Debit,
        Field::Credit,
        Field::Memo,
        Field::Tag,
    ]
    .into_iter()
    .find(|f| column_of(*f) == c)
}

/// The i18n key of a column header for `kind`.
#[must_use]
pub(crate) const fn header_key(kind: SectionKind, f: Field) -> &'static str {
    match (kind, f) {
        (_, Field::Date) => "batch_entry.col.date",
        (SectionKind::Checks, Field::Number) => "batch_entry.col.check_no",
        (SectionKind::JournalEntries, Field::Number) => "batch_entry.col.entry_no",
        (_, Field::Number) => "batch_entry.col.ref_no",
        (_, Field::PrintLater) => "batch_entry.col.print_later",
        (SectionKind::Checks, Field::Contact) => "batch_entry.col.pay_to",
        (SectionKind::Deposits, Field::Contact) => "batch_entry.col.received_from",
        (SectionKind::JournalEntries, Field::Contact) => "batch_entry.col.name",
        (_, Field::Contact) => "batch_entry.col.payee",
        (SectionKind::Deposits, Field::Account) => "batch_entry.col.from_account",
        (_, Field::Account) => "batch_entry.col.account",
        (_, Field::Amount) => "batch_entry.col.amount",
        (_, Field::Debit) => "batch_entry.col.debit",
        (_, Field::Credit) => "batch_entry.col.credit",
        (_, Field::Memo) => "batch_entry.col.memo",
        (_, Field::Tag) => "batch_entry.col.tag",
    }
}

/// The plain-language tooltip of a column header, if it has one.
const fn help_key(kind: SectionKind, f: Field) -> Option<&'static str> {
    match (kind, f) {
        (_, Field::PrintLater) => Some("batch_entry.col.print_later.tip"),
        (SectionKind::JournalEntries, Field::Number) => Some("batch_entry.col.entry_no.tip"),
        (_, Field::Debit) => Some("jargon.debit"),
        (_, Field::Credit) => Some("jargon.credit"),
        (_, Field::Tag) => Some("batch_entry.col.tag.tip"),
        _ => None,
    }
}

/// The grid id of a section type (the control channel finds the grid by
/// its label; the id keeps each type's widths and scroll apart).
#[must_use]
pub(crate) const fn grid_id(kind: SectionKind) -> &'static str {
    match kind {
        SectionKind::Checks => "batch_entry/grid/checks",
        SectionKind::Expenses => "batch_entry/grid/expenses",
        SectionKind::CardCharges => "batch_entry/grid/card_charges",
        SectionKind::CardCredits => "batch_entry/grid/card_credits",
        SectionKind::Deposits => "batch_entry/grid/deposits",
        SectionKind::JournalEntries => "batch_entry/grid/journal_entries",
    }
}

/// The grid spec of a section type: a line grid (entry line, Ctrl+Insert
/// and Ctrl+Delete) with block paste on and rows measured for their note.
#[must_use]
pub(crate) fn spec(kind: SectionKind) -> GridSpec {
    let columns = kind
        .columns()
        .iter()
        .map(|f| {
            let (cell, width, align, mono) = match f {
                Field::Date => (CellKind::Date, (96, 104, 0), Align::Left, true),
                Field::Number => (
                    CellKind::Text {
                        max_chars: u32::try_from(kind.max_number_chars()).unwrap_or(50),
                    },
                    (64, 80, 0),
                    Align::Left,
                    true,
                ),
                Field::PrintLater => (CellKind::Checkbox, (70, 80, 0), Align::Center, false),
                Field::Contact => (
                    CellKind::Picker {
                        list: NAMES,
                        add_new: false,
                    },
                    (120, 180, 2),
                    Align::Left,
                    false,
                ),
                Field::Account => (
                    CellKind::Picker {
                        list: ACCOUNTS,
                        add_new: false,
                    },
                    (130, 200, 2),
                    Align::Left,
                    false,
                ),
                Field::Amount | Field::Debit | Field::Credit => (
                    CellKind::Amount(AmountRules::MONEY),
                    (90, 110, 0),
                    Align::Right,
                    true,
                ),
                Field::Memo => (
                    CellKind::Text {
                        max_chars: u32::try_from(cl_model::forms::batch::MAX_MEMO_CHARS)
                            .unwrap_or(2000),
                    },
                    (100, 180, 2),
                    Align::Left,
                    false,
                ),
                Field::Tag => (
                    CellKind::Picker {
                        list: TAGS,
                        add_new: false,
                    },
                    (90, 120, 1),
                    Align::Left,
                    false,
                ),
            };
            Column {
                id: column_of(*f),
                header: TextKey(header_key(kind, *f)),
                placeholder: (*f == kind.columns()[0]).then_some(TextKey("batch_entry.col.date")),
                empty: None,
                help: help_key(kind, *f).map(TextKey),
                kind: cell,
                line: Line::First,
                align,
                width: WidthSpec {
                    min: width.0,
                    preferred: width.1,
                    flex: width.2,
                },
                editable: true,
                mono,
            }
        })
        .collect();
    let mut s = GridSpec::line_entry(columns);
    s.block_paste = true;
    s.height = RowHeight::Measured {
        max_rows: u32::try_from(cl_model::forms::batch::MAX_ROWS + 1).unwrap_or(2_001),
    };
    s
}

/// What a row shows besides its cells (worked out by the screen each frame
/// from the local and engine checks).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub(crate) struct RowLook {
    /// The row needs attention (`warn` bar and surface).
    pub error: bool,
    /// The caption under the row.
    pub note: Option<Note>,
}

/// A row's caption.
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum Note {
    /// A problem in the app's words.
    Problem(TextKey),
    /// The engine's own words for a problem the app has no words for.
    Engine(String),
    /// A plain caption ("In a closed period").
    Caption(TextKey),
    /// The account suggestion's basis.
    Suggestion(String),
}

/// The variant only (an engine message may name a vendor).
impl std::fmt::Debug for Note {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Note::Problem(_) => "Problem",
            Note::Engine(_) => "Engine",
            Note::Caption(_) => "Caption",
            Note::Suggestion(_) => "Suggestion",
        })
    }
}

/// Row keys, the lists, the shown section and what each row looks like.
pub(crate) struct BulkAdapter {
    /// Key generator (every row of every section).
    pub keys: RowKeyGen,
    /// Picker lists.
    pub lists: Arc<RegisterLists>,
    /// The section the grid shows.
    pub selected: usize,
    /// A new row's date when there is no row above it.
    pub today: CalendarDate,
    /// The highest check number in the books per bank account (Q-242).
    pub highest: BTreeMap<AccountId, u64>,
    /// Problems and notes per row (set through [`BulkAdapter::set_looks`]).
    pub looks: BTreeMap<RowKey, RowLook>,
    /// Goes up whenever `looks` change: part of the grid's
    /// revision, so a caption that arrives with no edit (the engine's
    /// answer) re-measures its row.
    pub looks_gen: u64,
    /// A suggested account per row (a ghost value in the empty Account
    /// cell); Tab or Accept sets it. Never an amount (non-negotiable 4).
    pub suggested: BTreeMap<RowKey, AccountId>,
    /// Suggestions accepted with Tab since the screen last asked.
    pub accepted: Vec<(RowKey, AccountId)>,
    snapshot: Option<RowDraft>,
}

impl BulkAdapter {
    pub(crate) fn new(lists: Arc<RegisterLists>, keys: RowKeyGen, today: CalendarDate) -> Self {
        BulkAdapter {
            keys,
            lists,
            selected: 0,
            today,
            highest: BTreeMap::new(),
            looks: BTreeMap::new(),
            looks_gen: 0,
            suggested: BTreeMap::new(),
            accepted: Vec::new(),
            snapshot: None,
        }
    }
}

impl BulkAdapter {
    /// What each row looks like now; a change bumps the grid's revision.
    pub(crate) fn set_looks(&mut self, looks: BTreeMap<RowKey, RowLook>) {
        if looks != self.looks {
            self.looks = looks;
            self.looks_gen = self.looks_gen.wrapping_add(1);
        }
    }
}

/// Exactly one blank entry row at the end of a section, dated like the
/// row above it (or today), and on a Checks section numbered after the
/// highest check number in the books and the batch (Q-242).
pub(crate) fn ensure_entry(
    batch: &mut BulkBatch,
    index: usize,
    keys: &mut RowKeyGen,
    today: CalendarDate,
    highest: &BTreeMap<AccountId, u64>,
) {
    let next_number = |batch: &BulkBatch, s: &Section| -> String {
        match (s.kind, s.context) {
            (SectionKind::Checks, Some(bank)) => batch
                .next_check_number(bank, highest.get(&bank).copied())
                .unwrap_or_default(),
            _ => String::new(),
        }
    };
    let Some(s) = batch.sections.get_mut(index) else {
        return;
    };
    while s.rows.len() >= 2 && s.rows[s.rows.len() - 2..].iter().all(RowDraft::is_blank) {
        s.rows.pop();
    }
    if s.rows.last().is_some_and(RowDraft::is_blank) {
        return;
    }
    let date = s.rows.iter().rev().find_map(|r| r.date).unwrap_or(today);
    let mut row = RowDraft::new(keys.next_key());
    row.date = Some(date);
    let number = next_number(batch, &batch.sections[index]);
    row.number = number;
    batch.sections[index].rows.push(row);
}

/// A money cell's value as stored: rounded to the cent half away from
/// zero (Q-232; the editor already does, this keeps any other path exact).
fn cents(m: Money) -> Option<Money> {
    if m.to_ten_thousandths() % 100 == 0 {
        Some(m)
    } else {
        parse_amount(&m.to_wire()).ok().flatten()
    }
}

/// Puts one committed cell into a row. `None` when it took.
pub(crate) fn apply(
    row: &mut RowDraft,
    kind: SectionKind,
    col: ColumnId,
    value: CellInput,
    lists: &RegisterLists,
) -> Option<TextKey> {
    let Some(f) = field_of(col).filter(|f| kind.columns().contains(f)) else {
        return Some(BAD_VALUE);
    };
    match (f, value) {
        (Field::Date, CellInput::Date(d)) => row.date = Some(d),
        (Field::Date, CellInput::Clear) => row.date = None,
        (Field::Number | Field::Memo, CellInput::Text(t) | CellInput::Unresolved(t)) => {
            if f == Field::Number {
                row.number = t;
            } else {
                row.memo = t;
            }
        }
        (Field::Number, CellInput::Clear) => row.number.clear(),
        (Field::Memo, CellInput::Clear) => row.memo.clear(),
        (Field::PrintLater, CellInput::Bool(b)) => row.print_later = b,
        (Field::PrintLater, CellInput::Clear) => row.print_later = false,
        (Field::Contact, CellInput::Ref(id)) => match lists.name(id) {
            Some(c) => {
                row.contact = Some(c);
                row.contact_text.clear();
            }
            None => return Some(BAD_VALUE),
        },
        (Field::Contact, CellInput::Unresolved(t) | CellInput::Text(t)) => {
            row.contact = None;
            row.contact_text = t;
        }
        (Field::Contact, CellInput::Clear) => {
            row.contact = None;
            row.contact_text.clear();
        }
        (Field::Account, CellInput::Ref(id)) => match lists.account(id) {
            Some(a) => {
                row.account = Some(a);
                row.account_text.clear();
            }
            None => return Some(BAD_VALUE),
        },
        (Field::Account, CellInput::Unresolved(t) | CellInput::Text(t)) => {
            row.account = None;
            row.account_text = t;
        }
        (Field::Account, CellInput::Clear) => {
            row.account = None;
            row.account_text.clear();
        }
        (Field::Amount, CellInput::Money(m)) => row.amount = cents(m),
        (Field::Amount, CellInput::Clear) => row.amount = None,
        // A journal entry row has one side: typing in one clears the other.
        (Field::Debit, CellInput::Money(m)) => {
            row.debit = cents(m);
            row.credit = None;
        }
        (Field::Debit, CellInput::Clear) => row.debit = None,
        (Field::Credit, CellInput::Money(m)) => {
            row.credit = cents(m);
            row.debit = None;
        }
        (Field::Credit, CellInput::Clear) => row.credit = None,
        (Field::Tag, CellInput::Ref(id)) => match lists.tag(id) {
            Some(t) => row.tag = Some(t),
            None => return Some(BAD_VALUE),
        },
        (Field::Tag, CellInput::Clear) => row.tag = None,
        _ => return Some(BAD_VALUE),
    }
    None
}

struct View<'a> {
    form: &'a mut FormState<BulkBatch>,
    a: &'a mut BulkAdapter,
}

impl LinesAdapter<BulkBatch> for BulkAdapter {
    fn model<'a>(&'a mut self, form: &'a mut FormState<BulkBatch>) -> Box<dyn EditableModel + 'a> {
        Box::new(View { form, a: self })
    }
}

static NO_ROWS: Vec<RowDraft> = Vec::new();

impl View<'_> {
    fn section(&self) -> Option<&Section> {
        self.form.current().sections.get(self.a.selected)
    }

    fn rows(&self) -> &[RowDraft] {
        self.section().map_or(&NO_ROWS, |s| &s.rows)
    }

    fn kind(&self) -> SectionKind {
        self.section().map_or(SectionKind::Checks, |s| s.kind)
    }
}

fn picked(lists: &RegisterLists, list: cl_grid::ListId, key: Option<u64>) -> CellValue<'_> {
    key.map_or(CellValue::Empty, |id| CellValue::Ref {
        id,
        display: lists.display(list, id).unwrap_or(Cow::Borrowed("")),
    })
}

impl RowModel for View<'_> {
    fn revision(&self) -> u64 {
        // The shown section and the rows' captions are part of what the
        // grid draws (a caption adds a line to its row).
        self.form
            .revision()
            .wrapping_mul(64)
            .wrapping_add(self.a.selected as u64)
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            .wrapping_add(self.a.looks_gen)
    }
    fn row_count(&self) -> usize {
        self.rows().len()
    }
    fn row_key(&self, index: usize) -> Option<RowKey> {
        self.rows().get(index).map(|r| r.key)
    }
    fn index_of(&self, key: RowKey) -> Option<usize> {
        self.rows().iter().position(|r| r.key == key)
    }
    fn row_state(&self, index: usize) -> RowState {
        let rows = self.rows();
        let Some(r) = rows.get(index) else {
            return RowState::empty();
        };
        let look = self.a.looks.get(&r.key);
        let mut s = RowState::empty();
        if index + 1 == rows.len() && r.is_blank() {
            s = s | RowState::ENTRY;
        }
        if look.is_some_and(|l| l.error) {
            s = s | RowState::ERROR;
        } else if !r.has_account() && self.a.suggested.contains_key(&r.key) {
            s = s | RowState::SUGGESTED;
        }
        s
    }
    fn cell(&self, index: usize, col: ColumnId) -> CellValue<'_> {
        let Some(r) = self.rows().get(index) else {
            return CellValue::Empty;
        };
        let lists = &*self.a.lists;
        match field_of(col) {
            Some(Field::Date) => r.date.map_or(CellValue::Empty, CellValue::Date),
            Some(Field::Number) => CellValue::Text(Cow::Borrowed(&r.number)),
            Some(Field::PrintLater) => CellValue::Bool(r.print_later),
            Some(Field::Contact) => match r.contact {
                Some(c) => picked(lists, NAMES, lists.name_key(c)),
                None if !r.contact_text.is_empty() => {
                    CellValue::Text(Cow::Borrowed(&r.contact_text))
                }
                None => CellValue::Empty,
            },
            Some(Field::Account) => match r.account {
                Some(a) => picked(lists, ACCOUNTS, lists.account_key(a)),
                None if !r.account_text.is_empty() => {
                    CellValue::Text(Cow::Borrowed(&r.account_text))
                }
                None => self
                    .a
                    .suggested
                    .get(&r.key)
                    .and_then(|a| lists.account_key(*a))
                    .map_or(CellValue::Empty, |id| CellValue::Suggested {
                        id,
                        display: lists.display(ACCOUNTS, id).unwrap_or(Cow::Borrowed("")),
                    }),
            },
            Some(Field::Amount) => r.amount.map_or(CellValue::Empty, CellValue::Money),
            Some(Field::Debit) => r.debit.map_or(CellValue::Empty, CellValue::Money),
            Some(Field::Credit) => r.credit.map_or(CellValue::Empty, CellValue::Money),
            Some(Field::Memo) => CellValue::Text(Cow::Borrowed(&r.memo)),
            Some(Field::Tag) => picked(lists, TAGS, r.tag.and_then(|t| lists.tag_key(t))),
            None => CellValue::Empty,
        }
    }
    fn row_note(&self, index: usize) -> Option<RowNote<'_>> {
        let r = self.rows().get(index)?;
        match &self.a.looks.get(&r.key)?.note {
            Some(Note::Problem(k)) => Some(RowNote::Error(*k)),
            Some(Note::Engine(m) | Note::Suggestion(m)) => {
                Some(RowNote::Suggestion(Cow::Borrowed(m)))
            }
            Some(Note::Caption(k)) => Some(RowNote::Caption(Cow::Owned(crate::i18n::t(k.0)))),
            None => None,
        }
    }
    fn row_label(&self, _index: usize) -> String {
        String::new()
    }
}

impl EditableModel for View<'_> {
    fn cell_editable(&self, index: usize, col: ColumnId) -> bool {
        index < self.rows().len()
            && field_of(col).is_some_and(|f| self.kind().columns().contains(&f))
    }
    fn row_entered(&mut self, key: RowKey) {
        self.a.snapshot = self.rows().iter().find(|r| r.key == key).cloned();
    }
    fn set_cell(
        &mut self,
        pos: CellPos,
        value: CellInput,
        source: Source,
    ) -> Result<(), CellProblem> {
        let lists = Arc::clone(&self.a.lists);
        if source == Source::SuggestionAccepted
            && field_of(pos.col) == Some(Field::Account)
            && let CellInput::Ref(id) = &value
            && let Some(account) = lists.account(*id)
        {
            self.a.accepted.push((pos.row, account));
        }
        let selected = self.a.selected;
        let today = self.a.today;
        let highest = self.a.highest.clone();
        let keys = &mut self.a.keys;
        let mut problem = None;
        let applied = self.form.edit_step(|b| {
            if let Some(s) = b.sections.get_mut(selected) {
                let kind = s.kind;
                if let Some(r) = s.rows.iter_mut().find(|r| r.key == pos.row) {
                    problem = apply(r, kind, pos.col, value, &lists);
                }
            }
            ensure_entry(b, selected, keys, today, &highest);
        });
        if !applied {
            return Err(CellProblem::new(SAVING).at(pos.col));
        }
        problem.map_or(Ok(()), |k| Err(CellProblem::new(k).at(pos.col)))
    }
    fn revert_row(&mut self, key: RowKey) -> bool {
        let Some(snap) = self.a.snapshot.clone().filter(|s| s.key == key) else {
            return false;
        };
        if self.rows().contains(&snap) {
            return false;
        }
        let (selected, today, highest) = (self.a.selected, self.a.today, self.a.highest.clone());
        let keys = &mut self.a.keys;
        self.form.edit_step(|b| {
            if let Some(r) = b
                .sections
                .get_mut(selected)
                .and_then(|s| s.rows.iter_mut().find(|r| r.key == key))
            {
                *r = snap;
            }
            ensure_entry(b, selected, keys, today, &highest);
        })
    }
    fn insert_line(&mut self, before: Option<RowKey>) -> Option<RowKey> {
        let key = self.a.keys.next_key();
        let selected = self.a.selected;
        let today = self.a.today;
        self.form
            .edit_step(|b| {
                let Some(s) = b.sections.get_mut(selected) else {
                    return;
                };
                let at = before
                    .and_then(|k| s.rows.iter().position(|r| r.key == k))
                    .unwrap_or(s.rows.len());
                let mut row = RowDraft::new(key);
                row.date = s.rows[..at]
                    .iter()
                    .rev()
                    .find_map(|r| r.date)
                    .or(Some(today));
                s.rows.insert(at, row);
            })
            .then_some(key)
    }
    fn delete_line(&mut self, key: RowKey) -> bool {
        if self.index_of(key).is_none() {
            return false;
        }
        let (selected, today, highest) = (self.a.selected, self.a.today, self.a.highest.clone());
        let keys = &mut self.a.keys;
        self.form.edit_step(|b| {
            if let Some(s) = b.sections.get_mut(selected) {
                s.rows.retain(|r| r.key != key);
            }
            ensure_entry(b, selected, keys, today, &highest);
        })
    }
    fn record_row(&mut self, key: RowKey) -> RecordOutcome {
        let Some(i) = self.index_of(key) else {
            return RecordOutcome::Invalid(Vec::new());
        };
        // Enter moves on; the row's problems show once the cursor leaves it
        // (local checks at once, the engine's a moment later).
        RecordOutcome::Recorded {
            next: self.row_key(i + 1),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn money(s: &str) -> Money {
        Money::parse_wire(s).expect("money")
    }

    #[test]
    fn every_section_spec_is_valid_with_its_own_headers() {
        for kind in SectionKind::ALL {
            let s = spec(kind);
            assert_eq!(s.validate(), Ok(()), "{kind:?}");
            assert!(s.block_paste && s.entry_line && s.insert_delete);
            let ids: Vec<ColumnId> = s.columns.iter().map(|c| c.id).collect();
            let want: Vec<ColumnId> = kind.columns().iter().map(|f| column_of(*f)).collect();
            assert_eq!(ids, want);
            for c in &s.columns {
                assert!(crate::i18n::has(c.header.0), "{}", c.header.0);
                if let Some(h) = c.help {
                    assert!(crate::i18n::has(h.0), "{}", h.0);
                }
            }
        }
        let t = |k, f| crate::i18n::t(header_key(k, f));
        assert_eq!(t(SectionKind::Checks, Field::Contact), "Pay to");
        assert_eq!(t(SectionKind::Deposits, Field::Contact), "Received from");
        assert_eq!(t(SectionKind::Deposits, Field::Account), "From account");
        assert_eq!(t(SectionKind::JournalEntries, Field::Number), "Entry No.");
        assert_eq!(t(SectionKind::Expenses, Field::Number), "Ref no.");
        for f in SectionKind::JournalEntries.columns() {
            assert_eq!(field_of(column_of(*f)), Some(*f));
        }
    }

    #[test]
    fn cells_keep_typed_names_and_round_amounts_to_the_cent() {
        let chart = crate::company::demo::chart();
        let mut lists = RegisterLists::default();
        lists.set_accounts(&chart);
        let mut r = RowDraft::new(RowKeyGen::new().next_key());
        let k = SectionKind::Checks;
        assert_eq!(
            apply(
                &mut r,
                k,
                column_of(Field::Account),
                CellInput::Unresolved("9999".into()),
                &lists
            ),
            None
        );
        assert_eq!((r.account, r.account_text.as_str()), (None, "9999"));
        let key = lists
            .account_key(crate::company::demo::account_id(6100))
            .expect("6100");
        assert_eq!(
            apply(
                &mut r,
                k,
                column_of(Field::Account),
                CellInput::Ref(key),
                &lists
            ),
            None
        );
        assert_eq!(r.account, Some(crate::company::demo::account_id(6100)));
        assert_eq!(r.account_text, "");
        assert_eq!(
            apply(
                &mut r,
                k,
                column_of(Field::Amount),
                CellInput::Money(money("12.345")),
                &lists
            ),
            None
        );
        assert_eq!(r.amount, Some(money("12.35")), "Q-232");
        // No Debit column on a check.
        assert_eq!(
            apply(
                &mut r,
                k,
                column_of(Field::Debit),
                CellInput::Money(money("1")),
                &lists
            ),
            Some(BAD_VALUE)
        );
        let j = SectionKind::JournalEntries;
        apply(
            &mut r,
            j,
            column_of(Field::Debit),
            CellInput::Money(money("5")),
            &lists,
        );
        apply(
            &mut r,
            j,
            column_of(Field::Credit),
            CellInput::Money(money("7")),
            &lists,
        );
        assert_eq!((r.debit, r.credit), (None, Some(money("7"))), "one side");
    }

    #[test]
    fn the_entry_row_is_dated_and_numbered_after_the_books_and_the_batch() {
        let bank = crate::company::demo::account_id(1010);
        let today = CalendarDate::from_ymd(2026, 11, 4).expect("date");
        let mut keys = RowKeyGen::new();
        let mut b = BulkBatch::new(Section::new(SectionKind::Checks, Some(bank)));
        let highest: BTreeMap<AccountId, u64> = [(bank, 1040)].into();
        ensure_entry(&mut b, 0, &mut keys, today, &highest);
        assert_eq!(b.sections[0].rows.len(), 1);
        assert_eq!(b.sections[0].rows[0].number, "1041");
        assert_eq!(b.sections[0].rows[0].date, Some(today));
        b.sections[0].rows[0].amount = Some(money("5"));
        b.sections[0].rows[0].date = CalendarDate::from_ymd(2026, 11, 2).ok();
        ensure_entry(&mut b, 0, &mut keys, today, &highest);
        let rows = &b.sections[0].rows;
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].number, "1042");
        assert_eq!(rows[1].date, rows[0].date, "dated like the row above");
        assert!(rows[1].is_blank());
        // Repeats change nothing.
        ensure_entry(&mut b, 0, &mut keys, today, &highest);
        assert_eq!(b.sections[0].rows.len(), 2);
    }
}
