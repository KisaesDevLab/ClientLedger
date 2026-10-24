//! Bulk Entry column choice (P16 D8, Q-273): which columns a section type
//! shows, in which order, with the columns its save rules need always on.
//!
//! [`required_fields`] is the one list of columns a kind cannot hide. A
//! property test (`ops_tests.rs`) keeps it in step with the save rules: every
//! [`super::Problem::Missing`] that [`super::local_problems`] can report is
//! in it, so an owner can never hide the cell a problem points at. Hidden
//! columns keep their values; hiding only changes what the grid draws.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::{Field, SectionKind};

/// The columns a `kind` row needs to be saved: they cannot be hidden.
///
/// - Checks: Date, Pay to (a check marked Print later needs a payee),
///   Account, Amount.
/// - Expenses, card charges, card credits, deposits: Date, Account, Amount.
/// - Journal entries: Date, Entry No., Account, Debit, Credit.
#[must_use]
pub const fn required_fields(kind: SectionKind) -> &'static [Field] {
    match kind {
        SectionKind::Checks => &[Field::Date, Field::Contact, Field::Account, Field::Amount],
        SectionKind::Expenses
        | SectionKind::CardCharges
        | SectionKind::CardCredits
        | SectionKind::Deposits => &[Field::Date, Field::Account, Field::Amount],
        SectionKind::JournalEntries => &[
            Field::Date,
            Field::Number,
            Field::Account,
            Field::Debit,
            Field::Credit,
        ],
    }
}

/// True when `f` cannot be hidden in a `kind` section.
#[must_use]
pub fn is_required(kind: SectionKind, f: Field) -> bool {
    required_fields(kind).contains(&f)
}

/// Why a column change is refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ColumnError {
    /// The column is needed to save this kind of transaction.
    #[error("this column is needed to save this kind of transaction")]
    Required(Field),
    /// The section type has no such column.
    #[error("this section type has no such column")]
    NotInSection(Field),
}

/// One section type's column order and hidden columns (remembered per
/// company and type by the screen).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColumnChoice {
    order: Vec<Field>,
    #[serde(default)]
    hidden: BTreeSet<Field>,
}

impl ColumnChoice {
    /// Every column of `kind`, in its default order, none hidden.
    #[must_use]
    pub fn default_for(kind: SectionKind) -> ColumnChoice {
        ColumnChoice {
            order: kind.columns().to_vec(),
            hidden: BTreeSet::new(),
        }
    }

    /// The shown columns, in order.
    #[must_use]
    pub fn visible(&self) -> Vec<Field> {
        self.order
            .iter()
            .copied()
            .filter(|f| !self.hidden.contains(f))
            .collect()
    }

    /// Every column, shown or hidden, in order (the "Columns…" list).
    #[must_use]
    pub fn all(&self) -> &[Field] {
        &self.order
    }

    /// True when `f` is hidden.
    #[must_use]
    pub fn is_hidden(&self, f: Field) -> bool {
        self.hidden.contains(&f)
    }

    /// Hide or show `f`.
    ///
    /// # Errors
    /// [`ColumnError::Required`] when hiding a column `kind` needs to save;
    /// [`ColumnError::NotInSection`] for a column `kind` does not have.
    pub fn set_hidden(
        &mut self,
        kind: SectionKind,
        f: Field,
        hide: bool,
    ) -> Result<(), ColumnError> {
        if !kind.columns().contains(&f) {
            return Err(ColumnError::NotInSection(f));
        }
        if hide {
            if is_required(kind, f) {
                return Err(ColumnError::Required(f));
            }
            self.hidden.insert(f);
        } else {
            self.hidden.remove(&f);
        }
        Ok(())
    }

    /// Move `f` by `delta` places (negative: left/up), stopping at the
    /// ends. True when it moved.
    pub fn move_by(&mut self, f: Field, delta: i8) -> bool {
        let Some(at) = self.order.iter().position(|c| *c == f) else {
            return false;
        };
        let last = self.order.len() - 1;
        let to = if delta < 0 {
            at.saturating_sub(usize::from(delta.unsigned_abs()))
        } else {
            at.saturating_add(usize::from(delta.unsigned_abs()))
                .min(last)
        };
        if to == at {
            return false;
        }
        let moved = self.order.remove(at);
        self.order.insert(to, moved);
        true
    }

    /// Make a remembered choice fit `kind` (old or edited prefs): drop
    /// columns the kind does not have and repeats, append missing ones in
    /// their default order, and show required columns again.
    pub fn normalize(&mut self, kind: SectionKind) {
        let columns = kind.columns();
        let mut seen = BTreeSet::new();
        self.order
            .retain(|f| columns.contains(f) && seen.insert(*f));
        for f in columns {
            if !seen.contains(f) {
                self.order.push(*f);
            }
        }
        self.hidden
            .retain(|f| columns.contains(f) && !is_required(kind, *f));
    }

    /// [`ColumnChoice::normalize`] by value (loading prefs).
    #[must_use]
    pub fn normalized(mut self, kind: SectionKind) -> ColumnChoice {
        self.normalize(kind);
        self
    }
}
