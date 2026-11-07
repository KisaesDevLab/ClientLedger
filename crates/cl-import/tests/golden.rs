//! The demo key copy against the engine's golden file (U-06, Q-P25r).
//!
//! `fixtures/bank/dedupe_v1.json` is a byte-identical copy of the engine's
//! `packages/api/src/services/bank-import/__fixtures__/dedupe_v1.json`. CI
//! checks out without submodules, so the copy is what the tests read; when
//! the submodule is present, a second test proves the copy has not drifted.

use cl_core::CalendarDate;
use cl_import::dedupe::{
    self, canonical_amount, fitid_key, key_rows, norm, ofx_description, row_key,
};
use rust_decimal::Decimal;
use serde_json::Value;
use std::path::PathBuf;
use std::str::FromStr as _;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn golden() -> Value {
    let bytes = std::fs::read(repo().join("fixtures/bank/dedupe_v1.json")).unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap()
}

fn date(v: &Value) -> CalendarDate {
    CalendarDate::parse_wire(s(v)).unwrap()
}

fn dec(v: &Value) -> Decimal {
    Decimal::from_str(s(v)).unwrap()
}

#[test]
fn version_matches() {
    assert_eq!(
        golden()["version"].as_u64(),
        Some(u64::from(dedupe::VERSION))
    );
}

#[test]
fn norm_cases() {
    for case in golden()["norm"].as_array().unwrap() {
        assert_eq!(norm(s(&case["input"])), s(&case["output"]), "{case}");
    }
}

#[test]
fn canonical_amount_cases() {
    for case in golden()["canonicalAmount"].as_array().unwrap() {
        assert_eq!(
            canonical_amount(dec(&case["input"])),
            s(&case["output"]),
            "{case}"
        );
    }
}

#[test]
fn row_key_cases() {
    for case in golden()["rowKey"].as_array().unwrap() {
        let ordinal = u32::try_from(case["ordinal"].as_u64().unwrap()).unwrap();
        assert_eq!(
            canonical_amount(dec(&case["amount"])),
            s(&case["canonicalAmount"])
        );
        assert_eq!(norm(s(&case["description"])), s(&case["normDescription"]));
        assert_eq!(
            row_key(
                date(&case["date"]),
                dec(&case["amount"]),
                s(&case["description"]),
                ordinal
            ),
            s(&case["rowKey"]),
            "{case}"
        );
    }
}

#[test]
fn ordinals() {
    let g = golden();
    let rows: Vec<(CalendarDate, Decimal, &str)> = g["ordinals"]["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| (date(&r["date"]), dec(&r["amount"]), s(&r["description"])))
        .collect();
    let keyed = key_rows(rows);
    let expected = g["ordinals"]["expected"].as_array().unwrap();
    assert_eq!(keyed.len(), expected.len());
    for (k, e) in keyed.iter().zip(expected) {
        assert_eq!(u64::from(k.ordinal), e["ordinal"].as_u64().unwrap());
        assert_eq!(k.row_key, s(&e["rowKey"]));
    }
}

#[test]
fn fitid_key_cases() {
    for case in golden()["fitidKey"].as_array().unwrap() {
        assert_eq!(fitid_key(s(&case["fitid"])), s(&case["key"]), "{case}");
    }
}

#[test]
fn ofx_description_cases() {
    for case in golden()["ofxDescription"].as_array().unwrap() {
        assert_eq!(
            ofx_description(case["name"].as_str(), case["memo"].as_str()),
            s(&case["description"]),
            "{case}"
        );
    }
}

#[test]
fn copy_matches_the_engine_when_present() {
    let engine =
        repo().join("engine/packages/api/src/services/bank-import/__fixtures__/dedupe_v1.json");
    let Ok(theirs) = std::fs::read(&engine) else {
        // The submodule is not checked out (CI) or not yet bumped past U-06.
        return;
    };
    let ours = std::fs::read(repo().join("fixtures/bank/dedupe_v1.json")).unwrap();
    assert!(
        ours == theirs,
        "fixtures/bank/dedupe_v1.json drifted from the engine's golden file"
    );
}
