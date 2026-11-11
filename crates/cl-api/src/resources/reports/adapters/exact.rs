//! Readers for the engine's exact report answers (U-14, `reports-exact-v1`):
//! every money value is an `*Exact` decimal string, parsed at full scale
//! (a cash-basis share may carry more places than the journal's four) and
//! never rounded. A missing or unreadable exact field is
//! [`ApiError::ReportShape`], never a fallback to the number field.

use cl_core::CalendarDate;
use cl_core::report::{Basis, Cell};
use rust_decimal::Decimal;
use serde_json::Value;

use crate::errors::ApiError;

/// The shape error for `what`.
pub(crate) const fn shape(what: &'static str) -> ApiError {
    ApiError::ReportShape { what }
}

/// An exact decimal string at full scale. `from_str_exact` refuses a value
/// it would have to round.
pub(crate) fn decimal(s: &str, what: &'static str) -> Result<Decimal, ApiError> {
    Decimal::from_str_exact(s).map_err(|_| shape(what))
}

/// The required exact amount `key` of `v`.
pub(crate) fn amount(v: &Value, key: &'static str) -> Result<Decimal, ApiError> {
    match v.get(key) {
        Some(Value::String(s)) => decimal(s, key),
        _ => Err(shape(key)),
    }
}

/// The exact amount `key` of `v` that the engine may send as `null` (gross
/// profit without cost of goods sold, a row with no amount). A missing key
/// is still a shape error.
pub(crate) fn amount_or_null(v: &Value, key: &'static str) -> Result<Option<Decimal>, ApiError> {
    match v.get(key) {
        Some(Value::String(s)) => decimal(s, key).map(Some),
        Some(Value::Null) => Ok(None),
        _ => Err(shape(key)),
    }
}

/// An optional exact amount: absent or `null` is `None` (`rollupExact` is
/// sent on parent rows only).
pub(crate) fn amount_opt(v: &Value, key: &'static str) -> Result<Option<Decimal>, ApiError> {
    match v.get(key) {
        Some(Value::String(s)) => decimal(s, key).map(Some),
        None | Some(Value::Null) => Ok(None),
        _ => Err(shape(key)),
    }
}

/// An amount cell from [`amount`].
pub(crate) fn cell(v: &Value, key: &'static str) -> Result<Cell, ApiError> {
    amount(v, key).map(Cell::Amount)
}

/// Text at `key` (absent, `null` or blank is `None`).
pub(crate) fn text<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// An id at `key`, parsed with `parse` (absent or `null` is `None`; an id
/// that doesn't parse is a shape error).
pub(crate) fn id<T, E>(
    v: &Value,
    key: &'static str,
    parse: impl Fn(&str) -> Result<T, E>,
) -> Result<Option<T>, ApiError> {
    match v.get(key) {
        Some(Value::String(s)) => parse(s).map(Some).map_err(|_| shape(key)),
        None | Some(Value::Null) => Ok(None),
        _ => Err(shape(key)),
    }
}

/// A `YYYY-MM-DD` date at `key`.
pub(crate) fn date(v: &Value, key: &'static str) -> Result<CalendarDate, ApiError> {
    v.get(key)
        .and_then(Value::as_str)
        .and_then(|s| CalendarDate::parse_wire(s).ok())
        .ok_or(shape(key))
}

/// The array at `key` (absent or `null` is empty).
pub(crate) fn list<'a>(v: &'a Value, key: &'static str) -> Result<&'a [Value], ApiError> {
    match v.get(key) {
        Some(Value::Array(a)) => Ok(a.as_slice()),
        None | Some(Value::Null) => Ok(&[]),
        _ => Err(shape(key)),
    }
}

/// The answer is an exact report: `meta.exact` is `true`.
pub(crate) fn require_exact(v: &Value) -> Result<(), ApiError> {
    if v.pointer("/meta/exact") == Some(&Value::Bool(true)) {
        Ok(())
    } else {
        Err(shape("meta.exact"))
    }
}

/// The basis the engine says it ran on.
pub(crate) fn meta_basis(v: &Value) -> Option<Basis> {
    match v.pointer("/meta/basis").and_then(Value::as_str) {
        Some("cash") => Some(Basis::Cash),
        Some("accrual") => Some(Basis::Accrual),
        _ => None,
    }
}

/// `number · name`, or the name alone (the mockup's account caption).
pub(crate) fn account_caption(number: Option<&str>, name: Option<&str>) -> String {
    let name = name.unwrap_or("");
    match number {
        Some(n) => format!("{n} \u{b7} {name}"),
        None => name.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn exact_strings_parse_at_full_scale() {
        let v = json!({"a": "0.3000", "b": "12.345678901234", "n": null, "x": 15});
        assert_eq!(amount(&v, "a").expect("a").to_string(), "0.3000");
        assert_eq!(amount(&v, "b").expect("b").to_string(), "12.345678901234");
        assert_eq!(amount_or_null(&v, "n").expect("n"), None);
        assert_eq!(amount_opt(&v, "missing").expect("absent"), None);
    }

    #[test]
    fn a_missing_or_number_field_is_a_shape_error_never_a_fallback() {
        let v = json!({"x": 15, "n": null});
        for (k, r) in [
            ("x", amount(&v, "x")),
            ("missing", amount(&v, "missing")),
            ("n", amount(&v, "n")),
        ] {
            assert!(matches!(r, Err(ApiError::ReportShape { .. })), "{k}");
        }
        assert!(amount_or_null(&v, "missing").is_err());
        assert!(require_exact(&json!({"meta": {"exact": false}})).is_err());
        assert!(require_exact(&json!({})).is_err());
    }

    #[test]
    fn a_value_that_would_round_is_refused() {
        // 30 places: more than a Decimal holds exactly.
        let v = json!({"a": "0.123456789012345678901234567891"});
        assert!(amount(&v, "a").is_err());
    }
}
