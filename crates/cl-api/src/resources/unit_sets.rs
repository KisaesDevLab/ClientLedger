//! Units of measure (P24 plan §2.4 U-161, `units-of-measure-v1`; Q-P24i,
//! Q-P24v, Q-P24w).
//!
//! - A unit set has a base unit (stock is kept in it) and other units, each
//!   a fixed [`UnitFactor`] = base units per unit. `GET/POST /unit-sets`,
//!   `GET/PUT/DELETE /unit-sets/{id}`; an edit sends the whole list of
//!   units (kept ones by id, new ones without) with `expectedUpdatedAt`.
//! - A unit named on any posted line keeps its factor and set (409
//!   `UNIT_IN_USE`); new units can be added. A name used twice is 409
//!   `DUPLICATE_NAME`.
//! - Item lines take `unitId` (`None` = the base unit); quantity and price
//!   stay in the unit entered, the engine stores the base quantity
//!   `round4(quantity × factor)` (400 `UNIT_QUANTITY_TOO_SMALL` when that is
//!   zero; 400 `UNIT_NOT_IN_SET` for a unit outside the item's set).
//! - Creating a set carries an idempotency key (the engine keys
//!   `POST /unit-sets` under `units-of-measure-v1`), so a lost reply is
//!   replayed rather than refused as `DUPLICATE_NAME`. Edits and deletes
//!   carry `expectedUpdatedAt` instead.

use std::fmt;

use cl_core::ids::{UnitId, UnitSetId};
use cl_core::{Timestamp, UnitFactor};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::client::{Call, Method};
use crate::errors::{ApiError, LocalValidation};
use crate::idempotency::IdempotencyKey;
use crate::resources::customer_payments::{error_code, require};
use crate::resources::operations::{
    UNIT_SET_CREATE, UNIT_SET_DELETE, UNIT_SET_GET, UNIT_SET_PUT, UNIT_SETS_LIST,
};
use crate::resources::sales_tax::{decode_list, decode_one};
use crate::session::Session;
use crate::wire::Capabilities;

/// The longest unit name the engine takes.
pub const UNIT_NAME_MAX: usize = 30;
/// The longest unit abbreviation.
pub const UNIT_ABBREVIATION_MAX: usize = 10;
/// The longest unit-set name.
pub const UNIT_SET_NAME_MAX: usize = 100;
/// The most units one set holds besides its base unit.
pub const UNITS_MAX: usize = 50;

/// One unit of a set (not the base unit).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Unit {
    /// Id (`None` for a unit not saved yet).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<UnitId>,
    /// Name (`case`).
    pub name: String,
    /// Abbreviation (`cs`).
    #[serde(default)]
    pub abbreviation: Option<String>,
    /// Base units per unit.
    pub factor: UnitFactor,
}

impl fmt::Debug for Unit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Unit")
            .field("id", &self.id)
            .field("factor", &self.factor)
            .finish_non_exhaustive()
    }
}

/// A saved unit set.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnitSet {
    /// Id.
    pub id: UnitSetId,
    /// Name (`Bottle`).
    pub name: String,
    /// The base unit's name (stock is counted in it).
    pub base_unit_name: String,
    /// The base unit's abbreviation.
    #[serde(default)]
    pub base_unit_abbreviation: Option<String>,
    /// Active.
    #[serde(default = "yes")]
    pub is_active: bool,
    /// The other units.
    #[serde(default)]
    pub units: Vec<Unit>,
    /// A line or an item names the set (its factors are locked).
    #[serde(default)]
    pub in_use: bool,
    /// Last change (the next edit's `expectedUpdatedAt`).
    #[serde(default)]
    pub updated_at: Option<Timestamp>,
}

fn yes() -> bool {
    true
}

impl fmt::Debug for UnitSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UnitSet")
            .field("id", &self.id)
            .field("units", &self.units.len())
            .field("in_use", &self.in_use)
            .finish_non_exhaustive()
    }
}

impl UnitSet {
    /// A unit's factor (`None` is the base unit: one).
    #[must_use]
    pub fn factor_of(&self, unit: Option<UnitId>) -> Option<UnitFactor> {
        match unit {
            None => Some(UnitFactor::ONE),
            Some(u) => self
                .units
                .iter()
                .find(|x| x.id == Some(u))
                .map(|x| x.factor),
        }
    }

    /// A unit's name (`None` is the base unit).
    #[must_use]
    pub fn name_of(&self, unit: Option<UnitId>) -> Option<&str> {
        match unit {
            None => Some(self.base_unit_name.as_str()),
            Some(u) => self
                .units
                .iter()
                .find(|x| x.id == Some(u))
                .map(|x| x.name.as_str()),
        }
    }
}

/// A unit set to create or the whole set an edit stores.
#[derive(Clone, PartialEq, Eq)]
pub struct NewUnitSet {
    /// Name.
    pub name: String,
    /// The base unit's name.
    pub base_unit_name: String,
    /// The base unit's abbreviation.
    pub base_unit_abbreviation: Option<String>,
    /// Active (an edit only).
    pub is_active: bool,
    /// The other units: kept ones with their id, new ones without; a unit
    /// left out of an edit is removed (refused while in use).
    pub units: Vec<Unit>,
}

impl fmt::Debug for NewUnitSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NewUnitSet")
            .field("units", &self.units.len())
            .finish_non_exhaustive()
    }
}

impl NewUnitSet {
    /// The engine's shape checks, before anything is sent: names present
    /// and short enough, no name twice (the base unit included), at most
    /// [`UNITS_MAX`] units.
    ///
    /// # Errors
    /// `LocalValidation::InvalidValue`; `TooManyLines` for too many units.
    pub fn check(&self) -> Result<(), LocalValidation> {
        let name_ok = |s: &str, max: usize| !s.trim().is_empty() && s.trim().chars().count() <= max;
        let abbr_ok = |a: &Option<String>| {
            a.as_ref()
                .is_none_or(|a| a.trim().chars().count() <= UNIT_ABBREVIATION_MAX)
        };
        if !name_ok(&self.name, UNIT_SET_NAME_MAX)
            || !name_ok(&self.base_unit_name, UNIT_NAME_MAX)
            || !abbr_ok(&self.base_unit_abbreviation)
        {
            return Err(LocalValidation::InvalidValue);
        }
        if self.units.len() > UNITS_MAX {
            return Err(LocalValidation::TooManyLines);
        }
        let mut seen = vec![self.base_unit_name.trim().to_lowercase()];
        for u in &self.units {
            if !name_ok(&u.name, UNIT_NAME_MAX) || !abbr_ok(&u.abbreviation) {
                return Err(LocalValidation::InvalidValue);
            }
            let n = u.name.trim().to_lowercase();
            if seen.contains(&n) {
                return Err(LocalValidation::InvalidValue);
            }
            seen.push(n);
        }
        Ok(())
    }

    fn body(&self, edit: bool, expected: Option<Timestamp>) -> Value {
        let clean = |s: &Option<String>| {
            s.as_ref()
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        let units: Vec<Value> = self
            .units
            .iter()
            .map(|u| {
                let mut v = json!({
                    "name": u.name.trim(),
                    "abbreviation": clean(&u.abbreviation),
                    "factor": u.factor.to_wire(),
                });
                if let Some(id) = u.id {
                    v["id"] = json!(id);
                }
                v
            })
            .collect();
        let mut v = json!({
            "name": self.name.trim(),
            "baseUnitName": self.base_unit_name.trim(),
            "baseUnitAbbreviation": clean(&self.base_unit_abbreviation),
            "units": units,
        });
        if edit {
            v["isActive"] = Value::Bool(self.is_active);
            if let Some(t) = expected {
                v["expectedUpdatedAt"] = Value::String(t.to_wire());
            }
        }
        v
    }
}

/// Why the engine refused a unit write or a line's unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnitRefusal {
    /// A line's unit is not one of its item's set (400 `UNIT_NOT_IN_SET`).
    NotInSet,
    /// A line's base quantity rounds to zero (400
    /// `UNIT_QUANTITY_TOO_SMALL`).
    QuantityTooSmall,
    /// Changing a unit or set a posted line names (409 `UNIT_IN_USE`,
    /// Q-P24w): add a new unit instead.
    InUse,
    /// A unit set or unit name used twice (409 `DUPLICATE_NAME`).
    DuplicateName,
}

/// Classifies `e` (`None` for anything else).
#[must_use]
pub fn unit_refusal(e: &ApiError) -> Option<UnitRefusal> {
    match error_code(e)? {
        "UNIT_NOT_IN_SET" => Some(UnitRefusal::NotInSet),
        "UNIT_QUANTITY_TOO_SMALL" => Some(UnitRefusal::QuantityTooSmall),
        "UNIT_IN_USE" => Some(UnitRefusal::InUse),
        "DUPLICATE_NAME" => Some(UnitRefusal::DuplicateName),
        _ => None,
    }
}

/// Unit-set operations of a session (`session.unit_sets()`).
#[derive(Clone, Copy, Debug)]
pub struct UnitSets<'a>(pub(crate) &'a Session);

impl Session {
    /// Unit sets of the open company (P24, U-161).
    #[must_use]
    pub fn unit_sets(&self) -> UnitSets<'_> {
        UnitSets(self)
    }
}

fn id_path(route: &str, id: UnitSetId) -> String {
    route.replace("{id}", &id.to_string())
}

impl UnitSets<'_> {
    /// Every unit set.
    ///
    /// # Errors
    /// `Local(Unsupported)` without `units-of-measure-v1`; any [`ApiError`].
    pub async fn list(&self) -> Result<Vec<UnitSet>, ApiError> {
        require(self.0, Capabilities::UNITS_OF_MEASURE)?;
        let call = Call::at(Method::Get, UNIT_SETS_LIST.route());
        let reply = self.0.send(&UNIT_SETS_LIST, call, None).await?;
        decode_list(&reply.body, "unitSets", "unit sets")
    }

    /// One unit set.
    ///
    /// # Errors
    /// As [`UnitSets::list`]; `NotFound`.
    pub async fn get(&self, id: UnitSetId) -> Result<UnitSet, ApiError> {
        require(self.0, Capabilities::UNITS_OF_MEASURE)?;
        let call = Call::new(
            Method::Get,
            UNIT_SET_GET.route(),
            id_path(UNIT_SET_GET.route(), id),
        );
        let reply = self.0.send(&UNIT_SET_GET, call, None).await?;
        decode_one(&reply.body, "unitSet", "unit set")
    }

    /// Creates a unit set under `key` (one key per Save click, reused on a
    /// retry).
    ///
    /// # Errors
    /// `Local(…)` from [`NewUnitSet::check`] (nothing sent); `DUPLICATE_NAME`;
    /// any [`ApiError`].
    pub async fn create(&self, n: &NewUnitSet, key: IdempotencyKey) -> Result<UnitSet, ApiError> {
        require(self.0, Capabilities::UNITS_OF_MEASURE)?;
        n.check().map_err(ApiError::Local)?;
        let mut call = Call::at(Method::Post, UNIT_SET_CREATE.route()).json_body(
            serde_json::to_vec(&n.body(false, None))
                .map_err(|_| ApiError::Local(LocalValidation::InvalidValue))?,
        );
        call.idempotency_key = Some(key);
        let reply = self.0.send(&UNIT_SET_CREATE, call, None).await?;
        decode_one(&reply.body, "unitSet", "unit set")
    }

    /// Stores the whole set read at `expected` (its `updatedAt`, sent as
    /// `expectedUpdatedAt`; required so an edit never overwrites a change
    /// saved since, P24 review D5).
    ///
    /// # Errors
    /// As [`UnitSets::create`]; `UNIT_IN_USE` (a used unit's factor or a
    /// used unit removed); `STALE`.
    pub async fn update(
        &self,
        id: UnitSetId,
        n: &NewUnitSet,
        expected: Timestamp,
    ) -> Result<UnitSet, ApiError> {
        require(self.0, Capabilities::UNITS_OF_MEASURE)?;
        n.check().map_err(ApiError::Local)?;
        let call = Call::new(
            Method::Put,
            UNIT_SET_PUT.route(),
            id_path(UNIT_SET_PUT.route(), id),
        )
        .json_body(
            serde_json::to_vec(&n.body(true, Some(expected)))
                .map_err(|_| ApiError::Local(LocalValidation::InvalidValue))?,
        );
        let reply = self.0.send(&UNIT_SET_PUT, call, None).await?;
        decode_one(&reply.body, "unitSet", "unit set")
    }

    /// Deletes a set no item or line names.
    ///
    /// # Errors
    /// `UNIT_IN_USE`; `NotFound`; any [`ApiError`].
    pub async fn delete(&self, id: UnitSetId) -> Result<(), ApiError> {
        require(self.0, Capabilities::UNITS_OF_MEASURE)?;
        let call = Call::new(
            Method::Delete,
            UNIT_SET_DELETE.route(),
            id_path(UNIT_SET_DELETE.route(), id),
        );
        self.0.send(&UNIT_SET_DELETE, call, None).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(name: &str, factor: &str) -> Unit {
        Unit {
            id: None,
            name: name.into(),
            abbreviation: None,
            factor: UnitFactor::parse_wire(factor).expect("factor"),
        }
    }

    fn bottle() -> NewUnitSet {
        NewUnitSet {
            name: "Bottle".into(),
            base_unit_name: "bottle".into(),
            base_unit_abbreviation: Some(" btl ".into()),
            is_active: true,
            units: vec![unit("case", "12"), unit("litre", "1.333333")],
        }
    }

    #[test]
    fn a_set_goes_out_with_factors_as_six_place_strings() {
        let b = bottle().body(false, None);
        assert_eq!(b["baseUnitName"], "bottle");
        assert_eq!(b["baseUnitAbbreviation"], "btl");
        assert_eq!(b["units"][0]["factor"], "12.000000");
        assert_eq!(b["units"][1]["factor"], "1.333333");
        assert!(b.get("isActive").is_none(), "a create sends no isActive");
        let e = bottle().body(true, None);
        assert_eq!(e["isActive"], true);
    }

    #[test]
    fn names_are_checked_before_sending() {
        assert_eq!(bottle().check(), Ok(()));
        let mut twice = bottle();
        twice.units.push(unit("Case", "24"));
        assert_eq!(twice.check(), Err(LocalValidation::InvalidValue));
        let mut base = bottle();
        base.units.push(unit("Bottle", "1"));
        assert_eq!(
            base.check(),
            Err(LocalValidation::InvalidValue),
            "the base name too"
        );
        let mut blank = bottle();
        blank.base_unit_name = "  ".into();
        assert_eq!(blank.check(), Err(LocalValidation::InvalidValue));
    }

    #[test]
    fn a_saved_set_reads_and_answers_factors() {
        let s: UnitSet = serde_json::from_value(json!({
            "id": "6a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01",
            "name": "Bottle", "baseUnitName": "bottle", "baseUnitAbbreviation": null,
            "isActive": true, "inUse": true, "updatedAt": "2026-11-06T10:00:00.000Z",
            "units": [{"id": "6b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01", "name": "case",
                       "abbreviation": "cs", "factor": "12.000000"}]
        }))
        .expect("set");
        let case = s.units[0].id;
        assert_eq!(
            s.factor_of(case).map(UnitFactor::format).as_deref(),
            Some("12")
        );
        assert_eq!(s.factor_of(None), Some(UnitFactor::ONE));
        assert_eq!(s.name_of(None), Some("bottle"));
        assert!(s.in_use);
    }
    /// Review D5: an edit always sends the `updatedAt` it read.
    #[tokio::test]
    async fn an_edit_sends_expected_updated_at() {
        use crate::resources::testkit::{json as reply, requests, session_with_caps};
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer};
        let server = MockServer::start().await;
        let set = r#"{"unitSet":{"id":"6a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01","name":"Bottle",
            "baseUnitName":"bottle","isActive":true,"updatedAt":"2026-11-06T10:00:00.000Z",
            "units":[]}}"#;
        Mock::given(method("PUT"))
            .respond_with(reply(200, set))
            .mount(&server)
            .await;
        let s = session_with_caps(&server, &[Capabilities::UNITS_OF_MEASURE]).await;
        let at = Timestamp::parse_wire("2026-11-06T10:00:00.000Z").expect("time");
        let id = UnitSetId::parse("6a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01").expect("id");
        s.unit_sets()
            .update(id, &bottle(), at)
            .await
            .expect("saved");
        let put: Value =
            serde_json::from_slice(&requests(&server, "PUT").await[0].body).expect("json");
        assert_eq!(put["expectedUpdatedAt"], "2026-11-06T10:00:00.000Z");
    }
}
