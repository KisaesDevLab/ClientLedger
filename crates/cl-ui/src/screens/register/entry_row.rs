//! The entry row pinned under the register (P12 S3): Date · Number · Payee ·
//! Account · Memo · Payment · Deposit, the kind it becomes, Splits and
//! Record (Enter).
//!
//! The row is a one-line editable [`cl_grid`] grid kept in a [`GridForm`],
//! so the P10 editors (date keys, autofill, Ctrl+L, the amount calculator)
//! work as everywhere else and every save path commits the open cell
//! first. The amounts are the owner's typed values as exact `Money`; the
//! row never reads or adds up a register figure. Recording builds a
//! `cl_api` posting (`create_bank_posting`, or a two-line journal entry)
//! with an idempotency key kept for as long as the same draft is retried.

use std::borrow::Cow;
use std::sync::Arc;

use cl_api::{
    BankPosting, Basis, CardChargeDraft, CheckDraft, DepositDraft, EntryType, JeLine,
    LocalValidation, NewJournalEntry, SplitLine, TransferDraft,
};
use cl_core::ids::{AccountId, ContactId, TagId, TransactionId};
use cl_core::{CalendarDate, Money};
use cl_grid::{
    Align, CellInput, CellKind, CellPos, CellProblem, CellValue, Column, ColumnId, EditableModel,
    Grid, GridHeight, GridSpec, Line, RecordOutcome, RowHeight, RowKey, RowKeyGen, RowModel,
    RowState, SelectionMode, Source, TextKey, WidthSpec,
};
use cl_model::amount_entry::AmountRules;
use cl_model::forms::FormState;
use cl_model::register::{
    AccountClass, CheckedEntry, ColumnLabels, EntryDraft, EntryKind, EntryKindTag, Side,
};

use super::lists::{ACCOUNTS, NAMES, RegisterLists};
use crate::company::demo::register::DemoPosting;
use crate::grid_form::{GridForm, LinesAdapter};
use crate::i18n::t;

/// Date.
pub const E_DATE: ColumnId = ColumnId(1);
/// Number.
pub const E_NUMBER: ColumnId = ColumnId(2);
/// Payee.
pub const E_PAYEE: ColumnId = ColumnId(3);
/// Account.
pub const E_ACCOUNT: ColumnId = ColumnId(4);
/// Memo.
pub const E_MEMO: ColumnId = ColumnId(5);
/// First amount column (credits to the register's account).
pub const E_PAYMENT: ColumnId = ColumnId(6);
/// Second amount column (debits).
pub const E_DEPOSIT: ColumnId = ColumnId(7);

/// Height of the entry row (header, the row, the kind and buttons line)
/// before any problem text; the screen measures the real height each frame.
pub const HEIGHT: f32 = 112.0;

/// Shown when a picker cell holds text that matches nothing.
const PICK_FROM_LIST: TextKey = TextKey("register.entry.pick_from_list");
/// Shown when a cell is committed while the row is being saved.
const SAVING: TextKey = TextKey("grid.form.saving");

fn column(id: ColumnId, header: TextKey, kind: CellKind, width: (u16, u16, u8)) -> Column {
    Column {
        id,
        header,
        placeholder: None,
        help: None,
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

/// The entry row's grid for a register of `class` (amount headings per
/// Q-P12g).
#[must_use]
pub fn spec(class: AccountClass) -> GridSpec {
    let labels = ColumnLabels::for_class(class);
    let money = |id, header| Column {
        align: Align::Right,
        mono: true,
        ..column(
            id,
            header,
            CellKind::Amount(AmountRules::MONEY),
            (90, 110, 0),
        )
    };
    GridSpec {
        columns: vec![
            Column {
                mono: true,
                ..column(
                    E_DATE,
                    TextKey("register.col.date"),
                    CellKind::Date,
                    (96, 104, 0),
                )
            },
            Column {
                mono: true,
                ..column(
                    E_NUMBER,
                    TextKey("register.col.number"),
                    CellKind::Text { max_chars: 50 },
                    (70, 84, 0),
                )
            },
            Column {
                placeholder: Some(TextKey("register.col.payee")),
                ..column(
                    E_PAYEE,
                    TextKey("register.col.payee"),
                    CellKind::Picker {
                        list: NAMES,
                        add_new: false,
                    },
                    (120, 180, 2),
                )
            },
            Column {
                placeholder: Some(TextKey("register.entry.col.account")),
                ..column(
                    E_ACCOUNT,
                    TextKey("register.entry.col.account"),
                    CellKind::Picker {
                        list: ACCOUNTS,
                        add_new: false,
                    },
                    (120, 190, 2),
                )
            },
            column(
                E_MEMO,
                TextKey("register.entry.col.memo"),
                CellKind::Text { max_chars: 500 },
                (100, 170, 2),
            ),
            money(E_PAYMENT, labels.payment),
            money(E_DEPOSIT, labels.deposit),
        ],
        height: RowHeight::Uniform { two_lines: false },
        editable: true,
        entry_line: false,
        insert_delete: false,
        selection: SelectionMode::None,
        async_record: false,
    }
}

/// What the row's model needs besides the draft.
pub struct EntryAdapter {
    /// The row's key.
    pub key: RowKey,
    /// Picker lists (ids are turned into engine ids here).
    pub lists: Arc<RegisterLists>,
    /// The Splits panel is open: the Account cell shows "Split".
    pub splits_open: bool,
    /// The Number cell can't be changed (editing a check, deposit, transfer
    /// or journal entry: the engine keeps its number).
    pub lock_number: bool,
    /// The row's accessible name.
    pub label: String,
    snapshot: Option<EntryDraft>,
}

impl EntryAdapter {
    /// A new adapter.
    #[must_use]
    pub fn new(lists: Arc<RegisterLists>) -> EntryAdapter {
        EntryAdapter {
            key: RowKeyGen::new().next_key(),
            lists,
            splits_open: false,
            lock_number: false,
            label: t("register.entry.row_new"),
            snapshot: None,
        }
    }
}

struct EntryView<'a> {
    form: &'a mut FormState<EntryDraft>,
    a: &'a mut EntryAdapter,
}

impl LinesAdapter<EntryDraft> for EntryAdapter {
    fn model<'a>(&'a mut self, form: &'a mut FormState<EntryDraft>) -> Box<dyn EditableModel + 'a> {
        Box::new(EntryView { form, a: self })
    }
}

impl RowModel for EntryView<'_> {
    fn revision(&self) -> u64 {
        self.form
            .revision()
            .wrapping_mul(4)
            .wrapping_add(u64::from(self.a.splits_open) * 2 + u64::from(self.a.lock_number))
    }
    fn row_count(&self) -> usize {
        1
    }
    fn row_key(&self, index: usize) -> Option<RowKey> {
        (index == 0).then_some(self.a.key)
    }
    fn index_of(&self, key: RowKey) -> Option<usize> {
        (key == self.a.key).then_some(0)
    }
    fn row_state(&self, _index: usize) -> RowState {
        if self.form.current().is_empty() {
            RowState::ENTRY
        } else {
            RowState::empty()
        }
    }
    fn cell(&self, _index: usize, col: ColumnId) -> CellValue<'_> {
        let d = self.form.current();
        let lists = &self.a.lists;
        match col {
            E_DATE => d.date.map_or(CellValue::Empty, CellValue::Date),
            E_NUMBER => CellValue::Text(Cow::Borrowed(&d.number)),
            E_PAYEE => d
                .payee
                .and_then(|c| Some((lists.name_key(c)?, lists.name_text(c)?)))
                .map_or(CellValue::Empty, |(id, name)| CellValue::Ref {
                    id,
                    display: Cow::Owned(name.to_string()),
                }),
            E_ACCOUNT if self.a.splits_open => CellValue::Text(Cow::Owned(t("register.split"))),
            E_ACCOUNT => {
                d.account
                    .and_then(|a| lists.account_key(a))
                    .map_or(CellValue::Empty, |id| CellValue::Ref {
                        id,
                        display: Cow::Owned(
                            cl_grid::Lookups::display(&**lists, ACCOUNTS, id)
                                .map(Cow::into_owned)
                                .unwrap_or_default(),
                        ),
                    })
            }
            E_MEMO => CellValue::Text(Cow::Borrowed(&d.memo)),
            E_PAYMENT => d.payment.map_or(CellValue::Empty, CellValue::Money),
            E_DEPOSIT => d.deposit.map_or(CellValue::Empty, CellValue::Money),
            _ => CellValue::Empty,
        }
    }
    fn row_label(&self, _index: usize) -> String {
        self.a.label.clone()
    }
}

impl EditableModel for EntryView<'_> {
    fn cell_editable(&self, _index: usize, col: ColumnId) -> bool {
        !(col == E_ACCOUNT && self.a.splits_open || col == E_NUMBER && self.a.lock_number)
    }
    fn row_entered(&mut self, _key: RowKey) {
        self.a.snapshot = Some(self.form.current().clone());
    }
    fn set_cell(
        &mut self,
        pos: CellPos,
        value: CellInput,
        _source: Source,
    ) -> Result<(), CellProblem> {
        let lists = Arc::clone(&self.a.lists);
        let mut problem = None;
        let applied = self.form.edit_step(|d| {
            problem = apply(d, pos.col, value, &lists);
        });
        if !applied {
            return Err(CellProblem::new(SAVING).at(pos.col));
        }
        match problem {
            None => Ok(()),
            Some(key) => Err(CellProblem::new(key).at(pos.col)),
        }
    }
    fn revert_row(&mut self, _key: RowKey) -> bool {
        let Some(snap) = self.a.snapshot.clone() else {
            return false;
        };
        if *self.form.current() == snap {
            return false;
        }
        self.form.edit_step(|d| *d = snap)
    }
    fn insert_line(&mut self, _before: Option<RowKey>) -> Option<RowKey> {
        None
    }
    fn delete_line(&mut self, _key: RowKey) -> bool {
        false
    }
    fn record_row(&mut self, _key: RowKey) -> RecordOutcome {
        // The screen checks and sends the row on `RowRecorded`; the cursor
        // stays on the row until the save comes back.
        RecordOutcome::Recorded {
            next: Some(self.a.key),
        }
    }
}

/// Puts one committed cell into the draft. Typing an amount in one column
/// clears the other, as a paper register would.
fn apply(
    d: &mut EntryDraft,
    col: ColumnId,
    value: CellInput,
    lists: &RegisterLists,
) -> Option<TextKey> {
    match (col, value) {
        (E_DATE, CellInput::Date(v)) => d.date = Some(v),
        (E_DATE, CellInput::Clear) => d.date = None,
        (E_NUMBER | E_MEMO, CellInput::Text(v)) => {
            if col == E_NUMBER {
                d.number = v;
            } else {
                d.memo = v;
            }
        }
        (E_NUMBER, CellInput::Clear) => d.number.clear(),
        (E_MEMO, CellInput::Clear) => d.memo.clear(),
        (E_PAYEE, CellInput::Ref(id)) => match lists.name(id) {
            Some(c) => d.payee = Some(c),
            None => return Some(PICK_FROM_LIST),
        },
        (E_PAYEE, CellInput::Clear) => d.payee = None,
        (E_ACCOUNT, CellInput::Ref(id)) => match lists.account(id) {
            Some(a) => d.account = Some(a),
            None => return Some(PICK_FROM_LIST),
        },
        (E_ACCOUNT, CellInput::Clear) => d.account = None,
        (E_PAYEE | E_ACCOUNT, CellInput::Unresolved(_)) => return Some(PICK_FROM_LIST),
        (E_PAYMENT, CellInput::Money(m)) => {
            d.payment = Some(m);
            d.deposit = None;
        }
        (E_DEPOSIT, CellInput::Money(m)) => {
            d.deposit = Some(m);
            d.payment = None;
        }
        (E_PAYMENT, CellInput::Clear) => d.payment = None,
        (E_DEPOSIT, CellInput::Clear) => d.deposit = None,
        _ => return Some(TextKey("register.entry.bad_value")),
    }
    None
}

/// The engine's entry types as the model's tags (unknown ones dropped).
#[must_use]
pub fn allowed_tags(allowed: &[EntryType]) -> Vec<EntryKindTag> {
    allowed
        .iter()
        .filter_map(|e| match e {
            EntryType::Expense => Some(EntryKindTag::Expense),
            EntryType::Deposit => Some(EntryKindTag::Deposit),
            EntryType::Transfer => Some(EntryKindTag::Transfer),
            EntryType::JournalEntry => Some(EntryKindTag::JournalEntry),
            EntryType::Other(_) => None,
        })
        .collect()
}

/// The control-channel name of a kind.
#[must_use]
pub fn kind_name(k: EntryKind) -> &'static str {
    match k {
        EntryKind::Check => "check",
        EntryKind::Deposit => "deposit",
        EntryKind::Transfer => "transfer",
        EntryKind::CardCharge => "card_charge",
        EntryKind::JournalEntry => "journal_entry",
    }
}

/// One line on the other side of the register's account.
#[derive(Clone, PartialEq, Eq)]
pub struct PLine {
    /// Account.
    pub account: AccountId,
    /// Amount (the owner's typed value).
    pub amount: Money,
    /// Line memo.
    pub memo: Option<String>,
    /// Line customer.
    pub contact: Option<ContactId>,
    /// Line tag (one per line, Q-233).
    pub tag: Option<TagId>,
}

/// A checked entry ready to become an engine posting. `Debug` shows no
/// amounts, names or memo text.
#[derive(Clone)]
pub struct Prepared {
    /// What it becomes.
    pub kind: EntryKind,
    /// The register's account.
    pub own: AccountId,
    /// Which column the amount is in.
    pub side: Side,
    /// Date.
    pub date: CalendarDate,
    /// Number (check number, or a card charge's reference).
    pub number: Option<String>,
    /// Payee.
    pub payee: Option<ContactId>,
    /// The payee's name (printed on a check).
    pub payee_name: Option<String>,
    /// Memo.
    pub memo: Option<String>,
    /// The header amount.
    pub amount: Money,
    /// The other side.
    pub lines: Vec<PLine>,
    /// A journal entry's report basis.
    pub basis: Basis,
    /// A journal entry's own-line memo and tag (kept as they were).
    pub own_line: (Option<String>, Option<TagId>),
}

impl std::fmt::Debug for Prepared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Prepared")
            .field("kind", &self.kind)
            .field("side", &self.side)
            .field("lines", &self.lines.len())
            .finish_non_exhaustive()
    }
}

/// Puts an amount on a journal side.
type SideOf = fn(Money) -> cl_api::Side;

/// An engine posting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Built {
    /// A check, deposit, transfer or card charge.
    Bank(BankPosting),
    /// A journal entry.
    Journal(NewJournalEntry),
}

impl Prepared {
    /// From a checked entry row with its other-side lines (one line from
    /// the row itself, or the Splits lines).
    #[must_use]
    pub fn new(own: AccountId, c: &CheckedEntry, lines: Vec<PLine>) -> Prepared {
        Prepared {
            kind: c.kind,
            own,
            side: c.side,
            date: c.date,
            number: c.number.clone(),
            payee: c.payee,
            payee_name: None,
            memo: c.memo.clone(),
            amount: c.amount,
            lines,
            basis: Basis::Both,
            own_line: (None, None),
        }
    }

    fn split_lines(&self) -> Vec<SplitLine> {
        self.lines
            .iter()
            .map(|l| SplitLine {
                account: l.account,
                amount: l.amount,
                memo: l.memo.clone(),
                contact: l.contact,
                tag: l.tag,
            })
            .collect()
    }

    /// The engine posting.
    ///
    /// # Errors
    /// The posting builders' local refusals (nothing is sent).
    pub fn build(&self, update: bool) -> Result<Built, LocalValidation> {
        let lines = self.split_lines();
        let posting = match self.kind {
            EntryKind::Check if update => BankPosting::card_charge(CardChargeDraft {
                // A check changes as an expense; its number and printed
                // name stay as they were (cl-api bank_postings).
                pay_from: self.own,
                date: self.date,
                number: None,
                payee: self.payee,
                memo: self.memo.clone(),
                lines,
            })?,
            EntryKind::Check => BankPosting::check(CheckDraft {
                bank: self.own,
                date: self.date,
                payee: self.payee,
                payee_name: self.payee_name.clone().unwrap_or_default(),
                check_number: match &self.number {
                    Some(n) => Some(n.parse().map_err(|_| LocalValidation::InvalidValue)?),
                    None => None,
                },
                memo: self.memo.clone(),
                amount: self.amount,
                lines,
            })?,
            EntryKind::CardCharge => BankPosting::card_charge(CardChargeDraft {
                pay_from: self.own,
                date: self.date,
                number: self.number.clone(),
                payee: self.payee,
                memo: self.memo.clone(),
                lines,
            })?,
            EntryKind::Deposit => BankPosting::deposit(DepositDraft {
                account: self.own,
                date: self.date,
                memo: self.memo.clone(),
                lines,
            })?,
            EntryKind::Transfer => {
                let [other] = lines.as_slice() else {
                    return Err(LocalValidation::InvalidValue);
                };
                let (from, to) = match self.side {
                    Side::Payment => (self.own, other.account),
                    Side::Deposit => (other.account, self.own),
                };
                BankPosting::transfer(TransferDraft {
                    from,
                    to,
                    date: self.date,
                    amount: self.amount,
                    memo: self.memo.clone(),
                })?
            }
            EntryKind::JournalEntry => return self.journal().map(Built::Journal),
        };
        Ok(Built::Bank(posting))
    }

    fn journal(&self) -> Result<NewJournalEntry, LocalValidation> {
        let (own_side, other_side): (SideOf, SideOf) = match self.side {
            Side::Payment => (cl_api::Side::Credit, cl_api::Side::Debit),
            Side::Deposit => (cl_api::Side::Debit, cl_api::Side::Credit),
        };
        let mut lines = vec![JeLine {
            account: self.own,
            side: own_side(self.amount),
            description: self.own_line.0.clone(),
            tag: self.own_line.1,
        }];
        lines.extend(self.lines.iter().map(|l| JeLine {
            account: l.account,
            side: other_side(l.amount),
            description: l.memo.clone(),
            tag: l.tag,
        }));
        NewJournalEntry::new(self.date, self.memo.clone(), self.basis, lines)
    }

    /// The same posting for the demo engine.
    #[must_use]
    pub fn demo(&self) -> DemoPosting {
        DemoPosting {
            kind: kind_name(self.kind),
            date: self.date,
            number: self.number.clone().unwrap_or_default(),
            contact: self.payee_name.clone(),
            memo: self.memo.clone().unwrap_or_default(),
            own: self.own,
            own_debit: self.side == Side::Deposit,
            lines: self.lines.iter().map(|l| (l.account, l.amount)).collect(),
        }
    }
}

/// The plain-language text for a posting builder's refusal.
#[must_use]
pub fn refusal_key(v: LocalValidation) -> TextKey {
    match v {
        LocalValidation::TotalMismatch | LocalValidation::Unbalanced => {
            TextKey("register.splits.not_assigned")
        }
        LocalValidation::SameAccount => TextKey("register.entry.problem.same_account"),
        LocalValidation::NonPositiveAmount => TextKey("register.entry.problem.amount_not_positive"),
        LocalValidation::TooFewLines => TextKey("register.entry.problem.account_missing"),
        _ => TextKey("register.entry.problem.invalid"),
    }
}

/// Whether a kind takes a number from the entry row.
#[must_use]
pub fn takes_number(k: EntryKind) -> bool {
    matches!(k, EntryKind::Check | EntryKind::CardCharge)
}

/// A new entry row's grid form, its draft dated `date`.
///
/// # Panics
/// Never: the entry spec is fixed and covered by tests.
#[must_use]
pub fn new_form(
    class: AccountClass,
    lists: Arc<RegisterLists>,
    date: CalendarDate,
) -> GridForm<EntryDraft, EntryAdapter> {
    // A fixed spec, covered by `entry_spec_is_valid`.
    let grid = Grid::new("register/entry", spec(class)).expect("the entry row spec is valid");
    GridForm::new(
        FormState::new(EntryDraft {
            date: Some(date),
            ..EntryDraft::default()
        }),
        EntryAdapter::new(lists),
        grid,
        t("register.entry.label"),
        GridHeight::Rows { min: 1, max: 1 },
    )
}

/// The edit target: the transaction as read, at its version.
#[derive(Clone, Debug)]
pub struct Editing {
    /// The transaction.
    pub id: TransactionId,
    /// Its date as read (a change of a closed-period transaction needs the
    /// closing-date password, `needs_password(old, new)`).
    pub old_date: cl_core::CalendarDate,
    /// Its version (the fingerprint guard).
    pub version: cl_api::RecordVersion,
    /// What it is.
    pub kind: EntryKind,
    /// Its side in this register.
    pub side: Side,
    /// A journal entry's report basis.
    pub basis: Basis,
    /// A journal entry's own-line memo and tag.
    pub own_line: (Option<String>, Option<TagId>),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn money(s: &str) -> Money {
        Money::parse_wire(s).expect("money")
    }

    fn acct(n: u32) -> AccountId {
        crate::company::demo::account_id(n)
    }

    fn prepared(kind: EntryKind, side: Side) -> Prepared {
        Prepared {
            kind,
            own: acct(1010),
            side,
            date: CalendarDate::from_ymd(2026, 11, 4).expect("date"),
            number: None,
            payee: None,
            payee_name: Some("Hollister Fuel Stop".into()),
            memo: Some("Fuel".into()),
            amount: money("125.50"),
            lines: vec![PLine {
                account: acct(6100),
                amount: money("125.50"),
                memo: None,
                contact: None,
                tag: None,
            }],
            basis: Basis::Both,
            own_line: (None, None),
        }
    }

    #[test]
    fn entry_spec_is_valid_for_every_class() {
        for c in [
            AccountClass::Bank,
            AccountClass::CreditCard,
            AccountClass::OtherAsset,
            AccountClass::Equity,
        ] {
            assert_eq!(spec(c).validate(), Ok(()));
        }
    }

    #[test]
    fn each_kind_builds_its_posting() {
        let p = prepared(EntryKind::Check, Side::Payment);
        assert!(
            matches!(p.build(false), Ok(Built::Bank(b)) if b.kind() == cl_api::BankPostingKind::Check)
        );
        assert!(
            matches!(p.build(true), Ok(Built::Bank(b)) if b.kind() == cl_api::BankPostingKind::CardCharge)
        );
        let mut bad = p.clone();
        bad.number = Some("12a".into());
        assert_eq!(bad.build(false).err(), Some(LocalValidation::InvalidValue));
        let d = prepared(EntryKind::Deposit, Side::Deposit);
        assert!(
            matches!(d.build(false), Ok(Built::Bank(b)) if b.kind() == cl_api::BankPostingKind::Deposit)
        );
        let mut tr = prepared(EntryKind::Transfer, Side::Deposit);
        tr.lines[0].account = acct(1020);
        assert!(
            matches!(tr.build(false), Ok(Built::Bank(b)) if b.kind() == cl_api::BankPostingKind::Transfer)
        );
        let je = prepared(EntryKind::JournalEntry, Side::Payment);
        let Ok(Built::Journal(j)) = je.build(false) else {
            panic!("journal");
        };
        assert_eq!(j.lines().len(), 2);
        assert_eq!(j.lines()[0].side, cl_api::Side::Credit(money("125.50")));
        // Splits that don't add up to a check are refused before sending.
        let mut split = prepared(EntryKind::Check, Side::Payment);
        split.lines[0].amount = money("100.00");
        assert_eq!(
            split.build(false).err(),
            Some(LocalValidation::TotalMismatch)
        );
        assert_eq!(
            refusal_key(LocalValidation::TotalMismatch),
            TextKey("register.splits.not_assigned")
        );
    }

    #[test]
    fn typing_one_amount_clears_the_other_and_pickers_need_a_match() {
        let lists = RegisterLists::default();
        let mut d = EntryDraft::default();
        assert_eq!(
            apply(&mut d, E_PAYMENT, CellInput::Money(money("5")), &lists),
            None
        );
        assert_eq!(
            apply(&mut d, E_DEPOSIT, CellInput::Money(money("7")), &lists),
            None
        );
        assert_eq!((d.payment, d.deposit), (None, Some(money("7"))));
        assert_eq!(
            apply(
                &mut d,
                E_ACCOUNT,
                CellInput::Unresolved("zz".into()),
                &lists
            ),
            Some(PICK_FROM_LIST)
        );
        assert_eq!(
            apply(&mut d, E_ACCOUNT, CellInput::Ref(1), &lists),
            Some(PICK_FROM_LIST)
        );
        assert!(format!("{:?}", prepared(EntryKind::Check, Side::Payment)).contains("Check"));
        assert!(!format!("{:?}", prepared(EntryKind::Check, Side::Payment)).contains("125"));
        let demo = prepared(EntryKind::Deposit, Side::Deposit).demo();
        assert!(demo.own_debit);
        assert_eq!(demo.kind, "deposit");
    }
}
