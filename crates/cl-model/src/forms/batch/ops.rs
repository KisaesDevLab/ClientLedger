//! Bulk Entry row operations (P16 D6-D7, Q-272, Q-274): Copy down, Copy to
//! rows below, bulk Set and Delete rows.
//!
//! Each function changes one [`Section`] in place and returns how many rows
//! it changed; the screen wraps one call in one `FormState::edit_step`, so
//! every action is one undo step. Changed rows lose their engine check on
//! their own: `EngineChecks` keys answers by `BulkBatch::row_hash`.
//!
//! Money is never copied or bulk-set (non-negotiable 6, plan §3): Amount,
//! Debit and Credit are refused, and so is the check number / Entry No.
//! ([`copyable`], [`settable`]). Blank rows ([`RowDraft::is_blank`]) are
//! never filled in, so an action never turns an empty line into a half-made
//! transaction, and no rows are ever added.

use std::collections::BTreeSet;
use std::fmt;

use cl_core::CalendarDate;
use cl_core::ids::{AccountId, ContactId, TagId};

use super::{Field, RowDraft, RowKey, Section, SectionKind};

/// Which rows Copy down fills.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CopyScope {
    /// Every displayed row of the section ("Copy down").
    AllDisplayed,
    /// Only the rows after the source row ("Copy to rows below").
    Below,
}

/// True for the fields that never take a copied or bulk-set value: the
/// owner's amounts and the check number / Ref no. / Entry No.
const fn protected(f: Field) -> bool {
    matches!(
        f,
        Field::Amount | Field::Debit | Field::Credit | Field::Number
    )
}

/// True when Copy down is offered for `f` in a `kind` section: a column the
/// section has, never Amount, Debit, Credit or Number.
#[must_use]
pub fn copyable(kind: SectionKind, f: Field) -> bool {
    !protected(f) && kind.columns().contains(&f)
}

/// True when the selection bar can set `f` on a `kind` section: the same
/// fields as [`copyable`] (date, name, account, memo, tag, and Print later
/// on checks).
#[must_use]
pub fn settable(kind: SectionKind, f: Field) -> bool {
    copyable(kind, f)
}

/// A value for one cell. `Debug` prints the variant only: names and memos
/// never reach a log (Q-118).
#[derive(Clone, PartialEq, Eq)]
pub enum FieldValue {
    /// Date.
    Date(CalendarDate),
    /// The name: the picked contact (if any) and the text shown.
    Name {
        /// The picked contact.
        id: Option<ContactId>,
        /// The name as shown or typed.
        text: String,
    },
    /// The line account: the picked account (if any) and the text shown.
    Account {
        /// The picked account.
        id: Option<AccountId>,
        /// The account as shown or typed.
        text: String,
    },
    /// Memo.
    Memo(String),
    /// Tag (`None` clears it).
    Tag(Option<TagId>),
    /// Print later (checks).
    PrintLater(bool),
}

impl fmt::Debug for FieldValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            FieldValue::Date(_) => "Date(..)",
            FieldValue::Name { .. } => "Name(..)",
            FieldValue::Account { .. } => "Account(..)",
            FieldValue::Memo(_) => "Memo(..)",
            FieldValue::Tag(_) => "Tag(..)",
            FieldValue::PrintLater(_) => "PrintLater(..)",
        })
    }
}

impl FieldValue {
    /// The field this value is for.
    #[must_use]
    pub const fn field(&self) -> Field {
        match self {
            FieldValue::Date(_) => Field::Date,
            FieldValue::Name { .. } => Field::Contact,
            FieldValue::Account { .. } => Field::Account,
            FieldValue::Memo(_) => Field::Memo,
            FieldValue::Tag(_) => Field::Tag,
            FieldValue::PrintLater(_) => Field::PrintLater,
        }
    }

    /// `row`'s value of `f`. `None` for the protected fields (amounts,
    /// numbers) and for a row without a date.
    #[must_use]
    pub fn of(row: &RowDraft, f: Field) -> Option<FieldValue> {
        Some(match f {
            Field::Date => FieldValue::Date(row.date?),
            Field::Contact => FieldValue::Name {
                id: row.contact,
                text: row.contact_text.clone(),
            },
            Field::Account => FieldValue::Account {
                id: row.account,
                text: row.account_text.clone(),
            },
            Field::Memo => FieldValue::Memo(row.memo.clone()),
            Field::Tag => FieldValue::Tag(row.tag),
            Field::PrintLater => FieldValue::PrintLater(row.print_later),
            Field::Number | Field::Amount | Field::Debit | Field::Credit => return None,
        })
    }

    /// Put the value into `row`; true when the row changed.
    fn apply(&self, row: &mut RowDraft) -> bool {
        let before = row.clone();
        match self {
            FieldValue::Date(d) => row.date = Some(*d),
            FieldValue::Name { id, text } => {
                row.contact = *id;
                row.contact_text.clone_from(text);
            }
            FieldValue::Account { id, text } => {
                row.account = *id;
                row.account_text.clone_from(text);
            }
            FieldValue::Memo(m) => row.memo.clone_from(m),
            FieldValue::Tag(t) => row.tag = *t,
            FieldValue::PrintLater(p) => row.print_later = *p,
        }
        *row != before
    }
}

/// Why Copy down is off for a cell (the menu greys the item and the status
/// line says why in plain words).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CopyRefusal {
    /// Amount, Debit, Credit or the number: never copied.
    Protected,
    /// The row is not in the section.
    UnknownRow,
    /// The row is blank: the empty line for a new entry (its carried-over
    /// date alone is not something to copy).
    BlankRow,
    /// The row has no amount yet (no Amount, Debit or Credit).
    NoAmount,
    /// The cell is empty: copying it would empty the column.
    EmptyCell,
    /// No other filled-in row is in reach.
    NoTargets,
}

/// What Copy down needs to know about one row: its key and which cells are
/// filled, never a name, memo or amount (the menu keeps these per frame).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RowFacts {
    /// The row.
    pub key: RowKey,
    /// [`RowDraft::is_blank`].
    pub blank: bool,
    /// Amount, Debit or Credit is filled in.
    pub has_amount: bool,
    /// One bit per copyable field that has a value.
    filled: u16,
}

/// The bit of `f` in [`RowFacts::filled`].
const fn field_bit(f: Field) -> u16 {
    1 << (f as u16)
}

impl RowFacts {
    /// The facts of `row`.
    #[must_use]
    pub fn of(row: &RowDraft) -> RowFacts {
        let mut filled = 0;
        for (f, on) in [
            (Field::Date, row.date.is_some()),
            (Field::Contact, row.has_contact()),
            (Field::Account, row.has_account()),
            (Field::Memo, !row.memo.trim().is_empty()),
            (Field::Tag, row.tag.is_some()),
            // Ticked or not, Print later always has a value.
            (Field::PrintLater, true),
        ] {
            if on {
                filled |= field_bit(f);
            }
        }
        RowFacts {
            key: row.key,
            blank: row.is_blank(),
            has_amount: row.amount.is_some() || row.debit.is_some() || row.credit.is_some(),
            filled,
        }
    }

    /// True when the row's `f` cell has a value to copy.
    #[must_use]
    pub const fn filled(&self, f: Field) -> bool {
        self.filled & field_bit(f) != 0
    }
}

/// The facts of every row of `s`, in order.
#[must_use]
pub fn row_facts(s: &Section) -> Vec<RowFacts> {
    s.rows.iter().map(RowFacts::of).collect()
}

/// Why Copy down of `f` from `from` is off, over the rows' facts; `None`
/// when it would fill at least one row. The source must be a filled-in row
/// with an amount and a value in the cell (review P16-part1 #2: Ctrl+D on
/// the blank entry line must not empty a whole column).
#[must_use]
pub fn copy_refusal(
    kind: SectionKind,
    rows: &[RowFacts],
    from: RowKey,
    f: Field,
    scope: CopyScope,
) -> Option<CopyRefusal> {
    if !copyable(kind, f) {
        return Some(CopyRefusal::Protected);
    }
    let Some(at) = rows.iter().position(|r| r.key == from) else {
        return Some(CopyRefusal::UnknownRow);
    };
    let src = rows[at];
    if src.blank {
        return Some(CopyRefusal::BlankRow);
    }
    if !src.has_amount {
        return Some(CopyRefusal::NoAmount);
    }
    if !src.filled(f) {
        return Some(CopyRefusal::EmptyCell);
    }
    let any = rows
        .iter()
        .enumerate()
        .any(|(i, r)| i != at && !r.blank && (scope == CopyScope::AllDisplayed || i > at));
    (!any).then_some(CopyRefusal::NoTargets)
}

/// The rows Copy down from `from` fills: not blank, not `from` itself, and
/// for [`CopyScope::Below`] after it. Empty when `from` is not in `s`.
fn copy_targets(s: &Section, from: RowKey, scope: CopyScope) -> Vec<usize> {
    let Some(at) = s.rows.iter().position(|r| r.key == from) else {
        return Vec::new();
    };
    s.rows
        .iter()
        .enumerate()
        .filter(|(i, r)| *i != at && !r.is_blank() && (scope == CopyScope::AllDisplayed || *i > at))
        .map(|(i, _)| i)
        .collect()
}

/// True when Copy down of `f` from `from` would reach at least one row (the
/// menu item is disabled otherwise): see [`copy_refusal`].
#[must_use]
pub fn can_copy_down(s: &Section, from: RowKey, f: Field, scope: CopyScope) -> bool {
    copy_refusal(s.kind, &row_facts(s), from, f, scope).is_none()
}

/// What a Copy down or Set did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Applied {
    /// Rows changed.
    pub changed: usize,
    /// Rows left as they were because the change would have joined two
    /// journal entries ([`keep_entries_apart`]).
    pub kept_apart: usize,
}

/// Puts back the rows of a journal entry section whose change would join
/// two different journal entries (Q-P16 default, review P16-part1 #5):
/// entries are grouped by date and Entry No. (`JeGroupKey`), so moving a
/// line of entry "1" on 11/02 to 11/01 would silently make it part of entry
/// "1" on 11/01. A row that had no date or no Entry No. was not part of an
/// entry and may join one. Repeats until no change joins two entries (a put
/// back row may expose another). Returns how many rows were put back.
fn keep_entries_apart(before: &Section, after: &mut Section) -> usize {
    use std::collections::{BTreeMap, HashMap};

    use super::JeGroupKey;

    if !after.kind.is_journal() {
        return 0;
    }
    let old: HashMap<RowKey, &RowDraft> = before.rows.iter().map(|r| (r.key, r)).collect();
    let origin = |r: &RowDraft| {
        old.get(&r.key)
            .filter(|b| !b.is_blank())
            .and_then(|b| JeGroupKey::of(b))
    };
    let mut put_back = BTreeSet::new();
    loop {
        // Each entry after the change, and the entries its rows came from.
        let mut from: BTreeMap<JeGroupKey, BTreeSet<JeGroupKey>> = BTreeMap::new();
        for r in after.rows.iter().filter(|r| !r.is_blank()) {
            if let (Some(now), Some(was)) = (JeGroupKey::of(r), origin(r)) {
                from.entry(now).or_default().insert(was);
            }
        }
        let joins: Vec<usize> = after
            .rows
            .iter()
            .enumerate()
            .filter(|(_, r)| !r.is_blank() && old.get(&r.key).is_some_and(|b| *b != *r))
            .filter(|(_, r)| match (JeGroupKey::of(r), origin(r)) {
                (Some(now), Some(was)) => {
                    now != was && from.get(&now).is_some_and(|set| set.len() > 1)
                }
                _ => false,
            })
            .map(|(i, _)| i)
            .collect();
        if joins.is_empty() {
            return put_back.len();
        }
        for i in joins {
            let key = after.rows[i].key;
            if let Some(b) = old.get(&key) {
                after.rows[i] = (*b).clone();
                put_back.insert(key);
            }
        }
    }
}

/// Rows of `after` that differ from `before` (same keys, same order).
fn count_changed(before: &Section, after: &Section) -> usize {
    after
        .rows
        .iter()
        .zip(&before.rows)
        .filter(|(a, b)| a != b)
        .count()
}

/// Copy `from`'s value of `f` onto the other rows of the section (every
/// row, or only the later ones). Blank rows are skipped and no rows are
/// added. Amounts and numbers are refused ([`copyable`]), and so is every
/// source [`copy_refusal`] names. Rows that would join two journal entries
/// keep their value.
pub fn copy_down(s: &mut Section, from: RowKey, f: Field, scope: CopyScope) -> Applied {
    if copy_refusal(s.kind, &row_facts(s), from, f, scope).is_some() {
        return Applied::default();
    }
    let Some(value) = s
        .rows
        .iter()
        .find(|r| r.key == from)
        .and_then(|r| FieldValue::of(r, f))
    else {
        return Applied::default();
    };
    let before = s.clone();
    for i in copy_targets(s, from, scope) {
        value.apply(&mut s.rows[i]);
    }
    let kept_apart = keep_entries_apart(&before, s);
    Applied {
        changed: count_changed(&before, s),
        kept_apart,
    }
}

/// Set `v` on the selected rows of the section. Blank rows are skipped;
/// fields the section does not have, amounts and numbers are refused
/// ([`settable`]). Rows that would join two journal entries keep their
/// value.
pub fn set_field(s: &mut Section, rows: &BTreeSet<RowKey>, v: &FieldValue) -> Applied {
    if !settable(s.kind, v.field()) {
        return Applied::default();
    }
    let before = s.clone();
    for r in &mut s.rows {
        if rows.contains(&r.key) && !r.is_blank() {
            v.apply(r);
        }
    }
    let kept_apart = keep_entries_apart(&before, s);
    Applied {
        changed: count_changed(&before, s),
        kept_apart,
    }
}

/// Remove the selected rows from the section (unsaved rows only: nothing
/// is sent to the engine). Returns how many were removed.
pub fn delete_rows(s: &mut Section, rows: &BTreeSet<RowKey>) -> usize {
    let before = s.rows.len();
    s.rows.retain(|r| !rows.contains(&r.key));
    before - s.rows.len()
}
