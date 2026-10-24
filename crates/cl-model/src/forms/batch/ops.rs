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
/// menu item is disabled otherwise, e.g. in a section with no other rows).
#[must_use]
pub fn can_copy_down(s: &Section, from: RowKey, f: Field, scope: CopyScope) -> bool {
    copyable(s.kind, f)
        && s.rows
            .iter()
            .find(|r| r.key == from)
            .and_then(|r| FieldValue::of(r, f))
            .is_some()
        && !copy_targets(s, from, scope).is_empty()
}

/// Copy `from`'s value of `f` onto the other rows of the section (every
/// row, or only the later ones). Blank rows are skipped and no rows are
/// added. Amounts and numbers are refused ([`copyable`]). Returns how many
/// rows changed.
pub fn copy_down(s: &mut Section, from: RowKey, f: Field, scope: CopyScope) -> usize {
    if !copyable(s.kind, f) {
        return 0;
    }
    let Some(value) = s
        .rows
        .iter()
        .find(|r| r.key == from)
        .and_then(|r| FieldValue::of(r, f))
    else {
        return 0;
    };
    let mut changed = 0;
    for i in copy_targets(s, from, scope) {
        if value.apply(&mut s.rows[i]) {
            changed += 1;
        }
    }
    changed
}

/// Set `v` on the selected rows of the section. Blank rows are skipped;
/// fields the section does not have, amounts and numbers are refused
/// ([`settable`]). Returns how many rows changed.
pub fn set_field(s: &mut Section, rows: &BTreeSet<RowKey>, v: &FieldValue) -> usize {
    if !settable(s.kind, v.field()) {
        return 0;
    }
    let mut changed = 0;
    for r in &mut s.rows {
        if rows.contains(&r.key) && !r.is_blank() && v.apply(r) {
            changed += 1;
        }
    }
    changed
}

/// Remove the selected rows from the section (unsaved rows only: nothing
/// is sent to the engine). Returns how many were removed.
pub fn delete_rows(s: &mut Section, rows: &BTreeSet<RowKey>) -> usize {
    let before = s.rows.len();
    s.rows.retain(|r| !rows.contains(&r.key));
    before - s.rows.len()
}
