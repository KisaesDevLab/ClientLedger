//! The demo engine's units of measure (P24 WP-3, U-161; Q-P24i, Q-P24v,
//! Q-P24w), as `/unit-sets` and the item-line rules answer:
//!
//! - Unit sets with a base unit and other units, each a fixed
//!   [`UnitFactor`]; a name used twice is 409 `DUPLICATE_NAME`.
//! - A unit named on a saved line keeps its factor and stays in its set
//!   (409 `UNIT_IN_USE`); new units can be added.
//! - A line's base quantity is `round4(quantity × factor)` half away from
//!   zero; zero for a non-zero line is 400 `UNIT_QUANTITY_TOO_SMALL`; a unit
//!   outside the item's set is 400 `UNIT_NOT_IN_SET`.
//! - The price in a unit is `round4(base price × factor)` (`unitPrices[]`).

use std::collections::BTreeSet;

use cl_api::{ApiError, NewUnitSet, Unit, UnitPrice, UnitSet};
use cl_core::ids::{ItemId, UnitId, UnitSetId};
use cl_core::{Money, MoneyError, Quantity, UnitFactor};
use rust_decimal::RoundingStrategy;
use serde_json::json;

use super::DemoState;

/// The demo's unit sets and the units saved lines name.
#[derive(Clone, Debug, Default)]
pub struct DemoUnits {
    sets: Vec<UnitSet>,
    used: BTreeSet<String>,
    next: u64,
}

/// The engine's refusal of a unit write or a line's unit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnitError {
    /// 400 `UNIT_NOT_IN_SET`.
    NotInSet,
    /// 400 `UNIT_QUANTITY_TOO_SMALL`.
    QuantityTooSmall,
    /// 409 `UNIT_IN_USE`.
    InUse,
    /// 409 `DUPLICATE_NAME`.
    DuplicateName,
    /// 400 `VALIDATION_ERROR` (a quantity out of range, bad names).
    Invalid,
    /// 404.
    NotFound,
}

impl UnitError {
    /// The engine code.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            UnitError::NotInSet => "UNIT_NOT_IN_SET",
            UnitError::QuantityTooSmall => "UNIT_QUANTITY_TOO_SMALL",
            UnitError::InUse => "UNIT_IN_USE",
            UnitError::DuplicateName => "DUPLICATE_NAME",
            UnitError::Invalid => "VALIDATION_ERROR",
            UnitError::NotFound => "NOT_FOUND",
        }
    }

    /// The engine's reply.
    #[must_use]
    pub fn api(self) -> ApiError {
        let status = match self {
            UnitError::InUse | UnitError::DuplicateName => 409,
            UnitError::NotFound => return ApiError::NotFound { code: None },
            _ => 400,
        };
        cl_api::engine_refusal(
            status,
            &json!({"error": {"message": "That unit can't be used here.", "code": self.code()}}),
        )
    }
}

/// `round4(quantity × factor)`; zero for a non-zero quantity is refused
/// (Q-P24v).
///
/// # Errors
/// [`UnitError::QuantityTooSmall`], [`UnitError::Invalid`] (out of range).
pub fn base_quantity(factor: UnitFactor, quantity: Quantity) -> Result<Quantity, UnitError> {
    let b = factor
        .base_quantity(quantity)
        .map_err(|_: MoneyError| UnitError::Invalid)?;
    if b.is_zero() && !quantity.is_zero() {
        return Err(UnitError::QuantityTooSmall);
    }
    Ok(b)
}

/// The price in a unit: `round4(base price × factor)`.
///
/// # Errors
/// [`UnitError::Invalid`] out of the money range.
pub fn unit_price(base: Money, factor: UnitFactor) -> Result<Money, UnitError> {
    let v = base
        .as_decimal()
        .checked_mul(factor.as_decimal())
        .ok_or(UnitError::Invalid)?
        .round_dp_with_strategy(4, RoundingStrategy::MidpointAwayFromZero);
    Money::try_from(v).map_err(|_| UnitError::Invalid)
}

/// A changed set against the saved one: a unit saved lines name may not
/// change its factor or leave the set (Q-P24w).
///
/// # Errors
/// [`UnitError::InUse`].
pub fn check_change(
    saved: &UnitSet,
    next: &NewUnitSet,
    used: impl Fn(UnitId) -> bool,
) -> Result<(), UnitError> {
    for u in &saved.units {
        let Some(id) = u.id else { continue };
        if !used(id) {
            continue;
        }
        match next.units.iter().find(|n| n.id == Some(id)) {
            Some(n) if n.factor == u.factor => {}
            _ => return Err(UnitError::InUse),
        }
    }
    Ok(())
}

impl DemoUnits {
    fn id(&mut self, kind: u64) -> String {
        self.next += 1;
        format!("d0000000-0000-4000-8{kind:03}-{:012}", self.next)
    }

    fn duplicate(&self, name: &str, except: Option<UnitSetId>) -> bool {
        self.sets
            .iter()
            .any(|s| Some(s.id) != except && s.name.trim().eq_ignore_ascii_case(name.trim()))
    }

    fn stored(&mut self, n: &NewUnitSet, id: UnitSetId) -> Result<UnitSet, UnitError> {
        n.check().map_err(|_| UnitError::Invalid)?;
        let mut units = Vec::with_capacity(n.units.len());
        for u in &n.units {
            let id = match u.id {
                Some(id) => id,
                None => UnitId::parse(&self.id(2)).map_err(|_| UnitError::Invalid)?,
            };
            units.push(Unit {
                id: Some(id),
                ..u.clone()
            });
        }
        Ok(UnitSet {
            id,
            name: n.name.trim().to_string(),
            base_unit_name: n.base_unit_name.trim().to_string(),
            base_unit_abbreviation: n.base_unit_abbreviation.clone(),
            is_active: n.is_active,
            units,
            in_use: false,
            updated_at: Some(DemoState::now()),
        })
    }

    /// Every set, with `in_use` as the engine reports it.
    #[must_use]
    pub fn list(&self) -> Vec<UnitSet> {
        self.sets
            .iter()
            .map(|s| UnitSet {
                in_use: s
                    .units
                    .iter()
                    .any(|u| u.id.is_some_and(|i| self.used.contains(&i.to_string()))),
                ..s.clone()
            })
            .collect()
    }

    /// One set.
    #[must_use]
    pub fn get(&self, id: UnitSetId) -> Option<UnitSet> {
        self.list().into_iter().find(|s| s.id == id)
    }

    /// `POST /unit-sets`.
    ///
    /// # Errors
    /// [`UnitError::DuplicateName`], [`UnitError::Invalid`].
    pub fn create(&mut self, n: &NewUnitSet) -> Result<UnitSet, UnitError> {
        if self.duplicate(&n.name, None) {
            return Err(UnitError::DuplicateName);
        }
        let id = UnitSetId::parse(&self.id(1)).map_err(|_| UnitError::Invalid)?;
        let s = self.stored(n, id)?;
        self.sets.push(s.clone());
        Ok(s)
    }

    /// `PUT /unit-sets/{id}`.
    ///
    /// # Errors
    /// [`UnitError::NotFound`], [`UnitError::InUse`],
    /// [`UnitError::DuplicateName`], [`UnitError::Invalid`].
    pub fn update(&mut self, id: UnitSetId, n: &NewUnitSet) -> Result<UnitSet, UnitError> {
        let saved = self.get(id).ok_or(UnitError::NotFound)?;
        check_change(&saved, n, |u| self.used.contains(&u.to_string()))?;
        if self.duplicate(&n.name, Some(id)) {
            return Err(UnitError::DuplicateName);
        }
        let s = self.stored(n, id)?;
        if let Some(x) = self.sets.iter_mut().find(|x| x.id == id) {
            *x = s.clone();
        }
        Ok(self.get(id).unwrap_or(s))
    }

    /// A saved line names `unit` (its factor is locked from now on).
    pub fn mark_used(&mut self, unit: UnitId) {
        self.used.insert(unit.to_string());
    }

    /// A unit's set and factor.
    #[must_use]
    pub fn factor(&self, set: UnitSetId, unit: UnitId) -> Option<UnitFactor> {
        self.sets
            .iter()
            .find(|s| s.id == set)?
            .units
            .iter()
            .find(|u| u.id == Some(unit))
            .map(|u| u.factor)
    }

    /// The `unitPrices[]` of an item in `set` with `base` price: the base
    /// unit first.
    #[must_use]
    pub fn unit_prices(&self, set: UnitSetId, base: Option<Money>) -> Vec<UnitPrice> {
        let Some(s) = self.sets.iter().find(|s| s.id == set) else {
            return Vec::new();
        };
        let mut out = vec![UnitPrice {
            unit_id: None,
            name: s.base_unit_name.clone(),
            abbreviation: s.base_unit_abbreviation.clone(),
            factor: UnitFactor::ONE,
            unit_price: base,
        }];
        out.extend(s.units.iter().map(|u| UnitPrice {
            unit_id: u.id,
            name: u.name.clone(),
            abbreviation: u.abbreviation.clone(),
            factor: u.factor,
            unit_price: base.and_then(|b| unit_price(b, u.factor).ok()),
        }));
        out
    }
}

/// A line's base quantity for `item` in `unit` (`None`: the base unit):
/// the demo engine's `baseQuantity(line)`.
///
/// # Errors
/// 400 `UNIT_NOT_IN_SET`, 400 `UNIT_QUANTITY_TOO_SMALL`.
pub fn line_base(
    state: &mut DemoState,
    item: ItemId,
    unit: Option<UnitId>,
    quantity: Quantity,
) -> Result<Quantity, ApiError> {
    let Some(unit) = unit else {
        return Ok(quantity);
    };
    let set = state
        .items_mut()
        .iter()
        .find(|i| i.id == item)
        .and_then(|i| i.fields.purchase.unit_set_id);
    let factor = set
        .and_then(|s| state.units.factor(s, unit))
        .ok_or_else(|| UnitError::NotInSet.api())?;
    base_quantity(factor, quantity).map_err(UnitError::api)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(s: &str) -> UnitFactor {
        UnitFactor::parse_wire(s).expect("factor")
    }

    fn bottle() -> NewUnitSet {
        let u = |name: &str, factor: &str| Unit {
            id: None,
            name: name.into(),
            abbreviation: None,
            factor: f(factor),
        };
        NewUnitSet {
            name: "Bottle".into(),
            base_unit_name: "bottle".into(),
            base_unit_abbreviation: None,
            is_active: true,
            units: vec![u("case", "12"), u("half-case", "6")],
        }
    }

    #[test]
    fn a_used_unit_keeps_its_factor_and_new_units_can_be_added() {
        let mut d = DemoUnits::default();
        let s = d.create(&bottle()).expect("create");
        assert_eq!(d.create(&bottle()), Err(UnitError::DuplicateName));
        let case = s.units[0].id.expect("id");
        d.mark_used(case);
        assert!(d.get(s.id).expect("set").in_use);
        let mut next = NewUnitSet {
            units: s.units.clone(),
            ..bottle()
        };
        next.units[0].factor = f("24");
        assert_eq!(d.update(s.id, &next), Err(UnitError::InUse));
        next.units[0].factor = f("12");
        next.units.push(Unit {
            id: None,
            name: "pallet".into(),
            abbreviation: None,
            factor: f("600"),
        });
        assert_eq!(d.update(s.id, &next).map(|s| s.units.len()), Ok(3));
        // The unused half-case may still change.
        next = NewUnitSet {
            units: d.get(s.id).expect("set").units,
            ..bottle()
        };
        next.units[1].factor = f("5");
        assert!(d.update(s.id, &next).is_ok());
    }

    #[test]
    fn prices_and_quantities_round_half_away_from_zero() {
        let m = |s: &str| Money::parse_wire(s).expect("money");
        assert_eq!(unit_price(m("4.50"), f("12")), Ok(m("54.0000")));
        assert_eq!(unit_price(m("0.0001"), f("0.5")), Ok(m("0.0001")));
        let q = |s: &str| Quantity::parse_wire(s).expect("qty");
        assert_eq!(
            base_quantity(f("0.001333"), q("0.0001")),
            Err(UnitError::QuantityTooSmall)
        );
        assert_eq!(base_quantity(f("1.333333"), q("0.75")), Ok(q("1")));
    }
}
