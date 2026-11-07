//! The Unit Sets editor (P24 WP-3, U-161; Q-P24i, Q-P24w): a set's name,
//! its base unit (stock is counted in it) and its other units, each with a
//! factor = base units per unit.
//!
//! The editor checks what the engine would refuse before anything is sent:
//! names present and unique (the base unit's included), factors above zero
//! with at most six decimals and at most 1,000,000, and (Q-P24w) a unit
//! already on saved transactions keeps its factor: the owner adds a new
//! unit instead. Factors are exact [`UnitFactor`]s; nothing is a float.

use cl_core::UnitFactor;
use cl_core::ids::UnitId;

use crate::text::TextKey;

/// The longest set name.
pub const SET_NAME_MAX: usize = 100;
/// The longest unit name.
pub const UNIT_NAME_MAX: usize = 30;
/// The longest abbreviation.
pub const ABBREVIATION_MAX: usize = 10;
/// The most units besides the base unit.
pub const UNITS_MAX: usize = 50;

/// One unit row as typed.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct UnitRow {
    /// The saved unit (`None` for a new row).
    pub id: Option<UnitId>,
    /// Name.
    pub name: String,
    /// Abbreviation.
    pub abbreviation: String,
    /// The factor as typed.
    pub factor: String,
    /// The saved factor of a unit saved lines name (locked, Q-P24w).
    pub locked: Option<UnitFactor>,
}

/// The editor's value.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct UnitSetDraft {
    /// Set name.
    pub name: String,
    /// Base unit name.
    pub base_name: String,
    /// Base unit abbreviation.
    pub base_abbreviation: String,
    /// The other units.
    pub units: Vec<UnitRow>,
}

/// Why a set can't be saved.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum UnitProblem {
    /// No set name (or too long).
    NameMissing,
    /// No base unit name (or too long).
    BaseMissing,
    /// A row without a name (or too long).
    UnitNameMissing(usize),
    /// A name used twice (the base unit's included).
    DuplicateName(usize),
    /// A factor that is not above 0 with at most six decimals, at most
    /// 1,000,000.
    FactorInvalid(usize),
    /// A used unit's factor changed (Q-P24w).
    FactorLocked(usize),
    /// An abbreviation over ten characters.
    AbbreviationTooLong,
    /// More than [`UNITS_MAX`] units.
    TooManyUnits,
}

impl UnitProblem {
    /// Plain-language message (strings live in `cl-ui::i18n`).
    #[must_use]
    pub const fn key(self) -> TextKey {
        match self {
            UnitProblem::NameMissing => TextKey("units.problem.name"),
            UnitProblem::BaseMissing => TextKey("units.problem.base"),
            UnitProblem::UnitNameMissing(_) => TextKey("units.problem.unit_name"),
            UnitProblem::DuplicateName(_) => TextKey("units.problem.duplicate"),
            UnitProblem::FactorInvalid(_) => TextKey("units.problem.factor"),
            UnitProblem::FactorLocked(_) => TextKey("units.problem.locked"),
            UnitProblem::AbbreviationTooLong => TextKey("units.problem.abbreviation"),
            UnitProblem::TooManyUnits => TextKey("units.problem.too_many"),
        }
    }

    /// The control channel's name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            UnitProblem::NameMissing => "name_missing",
            UnitProblem::BaseMissing => "base_missing",
            UnitProblem::UnitNameMissing(_) => "unit_name_missing",
            UnitProblem::DuplicateName(_) => "duplicate_name",
            UnitProblem::FactorInvalid(_) => "factor_invalid",
            UnitProblem::FactorLocked(_) => "factor_locked",
            UnitProblem::AbbreviationTooLong => "abbreviation_too_long",
            UnitProblem::TooManyUnits => "too_many_units",
        }
    }
}

/// One checked unit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckedUnit {
    /// The saved unit (`None`: new).
    pub id: Option<UnitId>,
    /// Name, trimmed.
    pub name: String,
    /// Abbreviation, trimmed (`None` when blank).
    pub abbreviation: Option<String>,
    /// Base units per unit.
    pub factor: UnitFactor,
}

/// A set that passed [`UnitSetDraft::check`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckedUnitSet {
    /// Name, trimmed.
    pub name: String,
    /// Base unit name, trimmed.
    pub base_name: String,
    /// Base unit abbreviation (`None` when blank).
    pub base_abbreviation: Option<String>,
    /// The other units, in row order.
    pub units: Vec<CheckedUnit>,
}

fn blank_none(s: &str) -> Option<String> {
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_string())
}

impl UnitSetDraft {
    /// A new row with nothing typed.
    pub fn add_row(&mut self) {
        self.units.push(UnitRow::default());
    }

    /// Takes out row `i` unless saved lines name it (a used unit stays,
    /// Q-P24w). True when it went.
    pub fn remove_row(&mut self, i: usize) -> bool {
        if self.units.get(i).is_some_and(|r| r.locked.is_none()) {
            self.units.remove(i);
            return true;
        }
        false
    }

    /// Checks the draft; rows that are wholly blank are left out.
    ///
    /// # Errors
    /// Every [`UnitProblem`] found, in field order.
    pub fn check(&self) -> Result<CheckedUnitSet, Vec<UnitProblem>> {
        let mut problems = Vec::new();
        let len_ok = |s: &str, max: usize| !s.trim().is_empty() && s.trim().chars().count() <= max;
        if !len_ok(&self.name, SET_NAME_MAX) {
            problems.push(UnitProblem::NameMissing);
        }
        if !len_ok(&self.base_name, UNIT_NAME_MAX) {
            problems.push(UnitProblem::BaseMissing);
        }
        let abbr_long = |s: &str| s.trim().chars().count() > ABBREVIATION_MAX;
        if abbr_long(&self.base_abbreviation) {
            problems.push(UnitProblem::AbbreviationTooLong);
        }
        let mut seen = vec![self.base_name.trim().to_lowercase()];
        let mut units = Vec::new();
        for (i, r) in self.units.iter().enumerate() {
            let blank = r.id.is_none()
                && r.name.trim().is_empty()
                && r.abbreviation.trim().is_empty()
                && r.factor.trim().is_empty();
            if blank {
                continue;
            }
            if len_ok(&r.name, UNIT_NAME_MAX) {
                let n = r.name.trim().to_lowercase();
                if seen.contains(&n) {
                    problems.push(UnitProblem::DuplicateName(i));
                }
                seen.push(n);
            } else {
                problems.push(UnitProblem::UnitNameMissing(i));
            }
            if abbr_long(&r.abbreviation) {
                problems.push(UnitProblem::AbbreviationTooLong);
            }
            match UnitFactor::parse_user(&r.factor) {
                Ok(f) if r.locked.is_some_and(|l| l != f) => {
                    problems.push(UnitProblem::FactorLocked(i));
                }
                Ok(f) => units.push(CheckedUnit {
                    id: r.id,
                    name: r.name.trim().to_string(),
                    abbreviation: blank_none(&r.abbreviation),
                    factor: f,
                }),
                Err(_) => problems.push(UnitProblem::FactorInvalid(i)),
            }
        }
        if units.len() > UNITS_MAX {
            problems.push(UnitProblem::TooManyUnits);
        }
        if !problems.is_empty() {
            return Err(problems);
        }
        Ok(CheckedUnitSet {
            name: self.name.trim().to_string(),
            base_name: self.base_name.trim().to_string(),
            base_abbreviation: blank_none(&self.base_abbreviation),
            units,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(name: &str, factor: &str) -> UnitRow {
        UnitRow {
            name: name.into(),
            factor: factor.into(),
            ..UnitRow::default()
        }
    }

    fn bottle() -> UnitSetDraft {
        UnitSetDraft {
            name: " Bottle ".into(),
            base_name: "bottle".into(),
            base_abbreviation: String::new(),
            units: vec![
                row("case", "12"),
                row("litre", "1.333333"),
                UnitRow::default(),
            ],
        }
    }

    #[test]
    fn a_valid_set_checks_exactly() {
        let c = bottle().check().expect("valid");
        assert_eq!(c.name, "Bottle");
        assert_eq!(c.units.len(), 2, "the blank row is left out");
        assert_eq!(c.units[1].factor.to_wire(), "1.333333");
        assert_eq!(c.base_abbreviation, None);
    }

    #[test]
    fn names_factors_and_locked_units_are_refused() {
        let mut d = bottle();
        d.units.push(row("Bottle", "1"));
        d.units.push(row("ml", "0"));
        d.units.push(row("drop", "0.0000001"));
        d.units.push(row("", "2"));
        let p = d.check().expect_err("refused");
        assert!(p.contains(&UnitProblem::DuplicateName(3)));
        assert!(p.contains(&UnitProblem::FactorInvalid(4)));
        assert!(p.contains(&UnitProblem::FactorInvalid(5)));
        assert!(p.contains(&UnitProblem::UnitNameMissing(6)));
        let mut used = bottle();
        used.units[0].locked = UnitFactor::parse_wire("12").ok();
        used.units[0].factor = "24".into();
        assert_eq!(used.check(), Err(vec![UnitProblem::FactorLocked(0)]));
        used.units[0].factor = "12.000".into();
        assert!(used.check().is_ok(), "the same factor typed differently");
        assert!(!used.remove_row(0), "a used unit stays");
        assert!(used.remove_row(1));
        assert_eq!(UnitProblem::FactorLocked(0).key().0, "units.problem.locked");
    }
}
