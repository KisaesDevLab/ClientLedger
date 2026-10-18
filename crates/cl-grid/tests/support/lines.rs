//! A split-line model over `FormState` (like the demo screen and P13): an
//! Account picker, Amount, Memo, Service date, Billable, Tags and a
//! read-only Balance column, with a blank entry line at the end.

use std::borrow::Cow;

use cl_core::{CalendarDate, Money, MoneyStyle, NegativeStyle, Weekday};
use cl_grid::core::keys::Formats;
use cl_grid::{
    Align, CellInput, CellKind, CellPos, CellProblem, CellValue, Column, ColumnId, EditableModel,
    GridSpec, Line, ListId, LookupResult, Lookups, OptionSpec, RecordOutcome, RowHeight, RowKey,
    RowKeyGen, RowModel, RowState, Source, Suggestion, TagChip, TextKey, WidthSpec,
};
use cl_model::amount_entry::AmountRules;
use cl_model::date_entry::DateOrder;
use cl_model::forms::FormState;
use cl_model::lookup::{LookupEntry, LookupIndex};

pub const ACCOUNT: ColumnId = ColumnId(1);
pub const AMOUNT: ColumnId = ColumnId(2);
pub const MEMO: ColumnId = ColumnId(3);
pub const DATE: ColumnId = ColumnId(4);
pub const BILLABLE: ColumnId = ColumnId(5);
pub const TAGS: ColumnId = ColumnId(6);
pub const BALANCE: ColumnId = ColumnId(7);
pub const STATUS: ColumnId = ColumnId(8);
pub const STATUS_OPTIONS: &[OptionSpec] = &[
    OptionSpec {
        id: 1,
        label: TextKey("Open"),
    },
    OptionSpec {
        id: 2,
        label: TextKey("Billed"),
    },
    OptionSpec {
        id: 3,
        label: TextKey("Paid"),
    },
];
pub const ACCOUNTS: ListId = ListId(1);
pub const TAG_LIST: ListId = ListId(2);
pub const CREW: u64 = 70;

#[derive(Debug, Clone, PartialEq, Default)]
pub enum Account {
    #[default]
    None,
    Id(u64),
    Unresolved(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct LineValue {
    pub key: RowKey,
    pub account: Account,
    pub suggested: Option<u64>,
    pub amount: Option<Money>,
    pub memo: String,
    pub date: Option<CalendarDate>,
    pub billable: bool,
    pub tags: Vec<TagChip>,
    pub status: Option<u64>,
    pub locked: bool,
}

impl LineValue {
    pub fn blank(key: RowKey) -> LineValue {
        LineValue {
            key,
            account: Account::None,
            suggested: None,
            amount: None,
            memo: String::new(),
            date: None,
            billable: false,
            tags: Vec::new(),
            status: None,
            locked: false,
        }
    }

    pub fn is_blank(&self) -> bool {
        let b = LineValue::blank(self.key);
        LineValue {
            suggested: None,
            locked: false,
            ..self.clone()
        } == b
    }
}

pub struct Lines {
    pub form: FormState<Vec<LineValue>>,
    pub keygen: RowKeyGen,
    pub entry_line: bool,
    pub snapshot: Option<(RowKey, LineValue)>,
    pub set_calls: Vec<(CellPos, Source)>,
    pub accounts: Vec<(u64, &'static str)>,
    /// `record_row` answers `Pending` (an async save, P12).
    pub async_records: bool,
    /// A broken model that offers an AI amount (NN4: never shown).
    pub suggest_amount: bool,
}

pub fn accounts() -> Vec<(u64, &'static str)> {
    vec![
        (5100, "5100 · Materials:Plants"),
        (5110, "5110 · Materials:Mulch & stone"),
        (6200, "6200 · Fuel"),
        (6210, "6210 · Equipment rental"),
    ]
}

impl Lines {
    pub fn new(entry_line: bool, rows: usize) -> Lines {
        let mut keygen = RowKeyGen::new();
        let mut v: Vec<LineValue> = (0..rows)
            .map(|_| LineValue::blank(keygen.next_key()))
            .collect();
        if entry_line {
            v.push(LineValue::blank(keygen.next_key()));
        }
        Lines {
            form: FormState::new(v),
            keygen,
            entry_line,
            snapshot: None,
            set_calls: Vec::new(),
            accounts: accounts(),
            async_records: false,
            suggest_amount: false,
        }
    }

    pub fn lines(&self) -> &[LineValue] {
        self.form.current()
    }

    pub fn line(&self, key: RowKey) -> &LineValue {
        self.lines().iter().find(|l| l.key == key).expect("line")
    }

    pub fn key(&self, i: usize) -> RowKey {
        self.lines()[i].key
    }

    fn ensure_entry(lines: &mut Vec<LineValue>, keygen: &mut RowKeyGen, entry: bool) {
        if entry && lines.last().is_none_or(|l| !l.is_blank()) {
            lines.push(LineValue::blank(keygen.next_key()));
        }
    }

    fn name(&self, id: u64) -> String {
        self.accounts
            .iter()
            .find(|(i, _)| *i == id)
            .map_or_else(String::new, |(_, n)| (*n).to_string())
    }
}

impl RowModel for Lines {
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
        if l.locked {
            s = s | RowState::LOCKED;
        }
        if self.entry_line && index + 1 == self.lines().len() && l.is_blank() {
            s = s | RowState::ENTRY;
        }
        if matches!(l.account, Account::Unresolved(_)) {
            s = s | RowState::ERROR;
        }
        s
    }
    fn cell(&self, index: usize, col: ColumnId) -> CellValue<'_> {
        let Some(l) = self.lines().get(index) else {
            return CellValue::Empty;
        };
        match col {
            ACCOUNT => match &l.account {
                Account::Id(id) => CellValue::Ref {
                    id: *id,
                    display: Cow::Owned(self.name(*id)),
                },
                Account::Unresolved(t) => CellValue::Text(Cow::Borrowed(t)),
                Account::None => match l.suggested {
                    Some(id) => CellValue::Suggested {
                        id,
                        display: Cow::Owned(self.name(id)),
                    },
                    None => CellValue::Empty,
                },
            },
            AMOUNT => match l.amount {
                Some(m) => CellValue::Money(m),
                None if self.suggest_amount => CellValue::Suggested {
                    id: 1,
                    display: Cow::Borrowed("999.00"),
                },
                None => CellValue::Empty,
            },
            MEMO => CellValue::Text(Cow::Borrowed(&l.memo)),
            DATE => l.date.map_or(CellValue::Empty, CellValue::Date),
            BILLABLE => CellValue::Bool(l.billable),
            TAGS => CellValue::Tags(&l.tags),
            STATUS => l.status.map_or(CellValue::Empty, CellValue::Choice),
            _ => CellValue::Empty,
        }
    }
    fn row_label(&self, _index: usize) -> String {
        String::new()
    }
}

impl EditableModel for Lines {
    fn cell_editable(&self, index: usize, col: ColumnId) -> bool {
        col != BALANCE && self.lines().get(index).is_some_and(|l| !l.locked)
    }
    fn row_entered(&mut self, key: RowKey) {
        self.snapshot = self
            .lines()
            .iter()
            .find(|l| l.key == key)
            .map(|l| (key, l.clone()));
    }
    fn set_cell(
        &mut self,
        pos: CellPos,
        value: CellInput,
        source: Source,
    ) -> Result<(), CellProblem> {
        self.set_calls.push((pos, source));
        let entry = self.entry_line;
        let keygen = &mut self.keygen;
        let mut bad = None;
        self.form.edit_step(|lines| {
            let Some(l) = lines.iter_mut().find(|l| l.key == pos.row) else {
                return;
            };
            match (pos.col, value) {
                (ACCOUNT, CellInput::Ref(id)) => l.account = Account::Id(id),
                (ACCOUNT, CellInput::Unresolved(t)) => l.account = Account::Unresolved(t),
                (ACCOUNT, CellInput::Clear) => l.account = Account::None,
                (AMOUNT, CellInput::Money(m)) => l.amount = Some(m),
                (AMOUNT, CellInput::Clear) => l.amount = None,
                (MEMO, CellInput::Text(t)) => l.memo = t,
                (MEMO, CellInput::Clear) => l.memo.clear(),
                (DATE, CellInput::Date(d)) => l.date = Some(d),
                (DATE, CellInput::Clear) => l.date = None,
                (BILLABLE, CellInput::Bool(b)) => l.billable = b,
                (TAGS, CellInput::Tags(t)) => l.tags = t,
                (STATUS, CellInput::Choice(id)) => l.status = Some(id),
                _ => bad = Some(CellProblem::new(TextKey("test.bad_input"))),
            }
            Lines::ensure_entry(lines, keygen, entry);
        });
        bad.map_or(Ok(()), Err)
    }
    fn revert_row(&mut self, key: RowKey) -> bool {
        let Some((k, snap)) = self.snapshot.clone() else {
            return false;
        };
        if k != key || self.line(key) == &snap {
            return false;
        }
        self.form.edit_step(|lines| {
            if let Some(l) = lines.iter_mut().find(|l| l.key == key) {
                *l = snap;
            }
        });
        true
    }
    fn insert_line(&mut self, before: Option<RowKey>) -> Option<RowKey> {
        let key = self.keygen.next_key();
        self.form.edit_step(|lines| {
            let at = before
                .and_then(|b| lines.iter().position(|l| l.key == b))
                .unwrap_or(lines.len());
            lines.insert(at, LineValue::blank(key));
        });
        Some(key)
    }
    fn delete_line(&mut self, key: RowKey) -> bool {
        let entry = self.entry_line;
        let keygen = &mut self.keygen;
        let before = self.form.current().len();
        self.form.edit_step(|lines| {
            lines.retain(|l| l.key != key);
            Lines::ensure_entry(lines, keygen, entry);
        });
        self.form.current().len() != before || self.form.current().iter().all(|l| l.key != key)
    }
    fn record_row(&mut self, key: RowKey) -> RecordOutcome {
        let l = self.line(key).clone();
        if matches!(l.account, Account::None | Account::Unresolved(_)) {
            return RecordOutcome::Invalid(vec![
                CellProblem::new(TextKey("test.account_needed")).at(ACCOUNT),
            ]);
        }
        if self.async_records {
            return RecordOutcome::Pending;
        }
        let i = self.index_of(key).unwrap_or(0);
        RecordOutcome::Recorded {
            next: self.row_key(i + 1),
        }
    }
}

pub struct TestLookups {
    accounts: LookupIndex,
    tags: Vec<Suggestion>,
    pub loading: bool,
}

impl Default for TestLookups {
    fn default() -> Self {
        TestLookups {
            accounts: LookupIndex::new(
                accounts()
                    .into_iter()
                    .map(|(id, name)| LookupEntry {
                        id,
                        number: Some(id.to_string()),
                        full_name: name.split(" · ").nth(1).unwrap_or(name).to_string(),
                        active: true,
                    })
                    .collect(),
            ),
            tags: vec![
                Suggestion {
                    id: 1,
                    label: "Install".into(),
                    group: None,
                },
                Suggestion {
                    id: 2,
                    label: "Crew A".into(),
                    group: Some(CREW),
                },
                Suggestion {
                    id: 3,
                    label: "Crew B".into(),
                    group: Some(CREW),
                },
            ],
            loading: false,
        }
    }
}

impl TestLookups {
    fn label(id: u64) -> String {
        accounts()
            .into_iter()
            .find(|(i, _)| *i == id)
            .map_or_else(String::new, |(_, n)| n.to_string())
    }
}

impl Lookups for TestLookups {
    fn suggest(&self, list: ListId, query: &str, limit: usize) -> LookupResult {
        if self.loading {
            return LookupResult::Loading;
        }
        let items = if list == ACCOUNTS {
            self.accounts
                .rank(query, limit)
                .into_iter()
                .map(|r| Suggestion {
                    id: r.id,
                    label: Self::label(r.id),
                    group: None,
                })
                .collect()
        } else {
            let q = query.to_lowercase();
            self.tags
                .iter()
                .filter(|t| t.label.to_lowercase().starts_with(&q))
                .take(limit)
                .cloned()
                .collect()
        };
        LookupResult::Ready(items)
    }
    fn exact(&self, list: ListId, query: &str) -> Option<Suggestion> {
        if list == ACCOUNTS {
            self.accounts.exact(query).map(|e| Suggestion {
                id: e.id,
                label: Self::label(e.id),
                group: None,
            })
        } else {
            self.tags
                .iter()
                .find(|t| t.label.eq_ignore_ascii_case(query.trim()))
                .cloned()
        }
    }
    fn display(&self, _list: ListId, id: u64) -> Option<Cow<'_, str>> {
        Some(Cow::Owned(Self::label(id)))
    }
    fn all(&self, list: ListId) -> LookupResult {
        self.suggest(list, "", 100)
    }
}

fn col(id: ColumnId, kind: CellKind, editable: bool) -> Column {
    let header = match id {
        ACCOUNT => "Account",
        AMOUNT => "Amount",
        MEMO => "Memo",
        DATE => "Service date",
        BILLABLE => "Billable",
        TAGS => "Tags",
        STATUS => "Status",
        _ => "Balance",
    };
    Column {
        id,
        header: TextKey(header),
        placeholder: None,
        help: None,
        kind,
        line: Line::First,
        align: Align::Left,
        width: WidthSpec {
            min: 40,
            preferred: 100,
            flex: 1,
        },
        editable,
        mono: false,
    }
}

pub fn columns() -> Vec<Column> {
    vec![
        col(
            ACCOUNT,
            CellKind::Picker {
                list: ACCOUNTS,
                add_new: true,
            },
            true,
        ),
        Column {
            align: Align::Right,
            mono: true,
            ..col(AMOUNT, CellKind::Amount(AmountRules::MONEY), true)
        },
        col(MEMO, CellKind::Text { max_chars: 100 }, true),
        col(DATE, CellKind::Date, true),
        col(BILLABLE, CellKind::Checkbox, true),
        col(TAGS, CellKind::Tags { list: TAG_LIST }, true),
        col(BALANCE, CellKind::Display, false),
    ]
}

/// The editors suite's columns: a multiline Memo and a Status dropdown
/// after Billable, in a measured grid so memo rows grow.
pub fn extras_spec() -> GridSpec {
    let mut cols = columns();
    for c in &mut cols {
        if c.id == MEMO {
            c.kind = CellKind::Memo {
                multiline: true,
                max_chars: 400,
            };
        }
    }
    let at = cols
        .iter()
        .position(|c| c.id == BILLABLE)
        .map_or(0, |i| i + 1);
    cols.insert(
        at,
        col(
            STATUS,
            CellKind::Dropdown {
                options: STATUS_OPTIONS,
            },
            true,
        ),
    );
    GridSpec {
        height: RowHeight::Measured { max_rows: 5_000 },
        ..GridSpec::line_entry(cols)
    }
}

pub fn line_spec() -> GridSpec {
    GridSpec::line_entry(columns())
}

pub fn fixed_spec() -> GridSpec {
    GridSpec::fixed_edit(columns())
}

pub fn formats() -> Formats {
    Formats {
        today: CalendarDate::from_ymd(2026, 11, 4).expect("date"),
        first_weekday: Weekday::Sun,
        date_order: DateOrder::Mdy,
        money: MoneyStyle {
            negative: NegativeStyle::Minus,
            symbol: false,
        },
    }
}

pub fn money(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}
