//! Engine spec fixtures (P05 plan §2.8, B6, AC8): the `OpenAPI` hash, the
//! covered-operations table and the hashes of the engine's Zod sources must
//! match the pinned engine checkout. A mismatch means the engine changed:
//! run `cargo xtask api-spec --update`, review the diff against the cl-api
//! models, and commit the fixtures with the model changes.
//!
//! The checkout-dependent test is skipped (with a note) when `engine/` is
//! not checked out, and fails under `CL_API_REQUIRE=1` (CI).

#[path = "support/spec.rs"]
mod spec;

use std::path::{Path, PathBuf};

const OPERATIONS_TXT: &str = include_str!("../operations.txt");

fn crate_dir() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn repo_root() -> PathBuf {
    crate_dir().join("../..")
}

fn fixture(name: &str) -> String {
    let path = crate_dir().join("tests/fixtures").join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn require() -> bool {
    std::env::var("CL_API_REQUIRE").as_deref() == Ok("1")
}

#[test]
fn fixtures_match_the_engine_checkout() {
    let root = repo_root();
    if !root.join("engine/openapi.json").is_file() {
        assert!(
            !require(),
            "engine/openapi.json is missing and CL_API_REQUIRE=1"
        );
        eprintln!("SKIPPED: engine/openapi.json is absent (submodule not checked out)");
        return;
    }
    let now = spec::compute(&root, OPERATIONS_TXT).unwrap_or_else(|e| panic!("{e}"));
    let hint = "the engine changed: run `cargo xtask api-spec --update`, review the diff against the cl-api models, and commit";
    assert_eq!(
        now.openapi,
        fixture("openapi.sha256"),
        "openapi.sha256: {hint}"
    );
    assert_eq!(
        now.covered,
        fixture("covered_operations.json"),
        "covered_operations.json: {hint}"
    );
    assert_eq!(
        now.schemas,
        fixture("engine_schemas.sha256"),
        "engine_schemas.sha256 (engine Zod sources): {hint}"
    );
}

/// Every operation in the cl-api table is in the recorded spec table, and
/// is documented by the engine unless it is one of the two it serves
/// outside the spec. No engine checkout needed.
#[test]
fn every_operation_is_recorded_and_in_the_spec() {
    let recorded: serde_json::Value =
        serde_json::from_str(&fixture("covered_operations.json")).expect("fixture json");
    let rows = recorded.as_array().expect("array");
    assert_eq!(rows.len(), cl_api::OPERATIONS.len());
    for op in &cl_api::OPERATIONS {
        let method = op.method().as_str();
        let row = rows
            .iter()
            .find(|r| r["method"] == method && r["path"] == op.route())
            .unwrap_or_else(|| panic!("{method} {} is not in covered_operations.json", op.route()));
        let outside = spec::OUTSIDE_SPEC
            .iter()
            .any(|(m, p)| *m == method && *p == op.route());
        assert_eq!(
            row["inSpec"],
            !outside,
            "{method} {}: expected inSpec={}",
            op.route(),
            !outside
        );
    }
}

/// C2 (U-18 plan §10.1): the `keyable` rows of `operations.txt` are exactly
/// the engine's `idempotency-v1` route set, as its `openapi.json` marks it
/// (`x-idempotency: true`, recorded in `idempotency_v1_routes.txt`), and
/// the typed table agrees. No engine checkout needed.
#[test]
fn keyable_rows_are_the_engines_idempotency_routes() {
    let recorded = spec::route_lines(&fixture("idempotency_v1_routes.txt"));
    assert_eq!(recorded.len(), 20, "the idempotency-v1 set has 20 routes");
    assert_eq!(spec::keyable_operations(OPERATIONS_TXT), recorded);
    let mut typed: Vec<(String, String)> = cl_api::OPERATIONS
        .iter()
        .filter(|o| o.is_keyable())
        .map(|o| (o.method().as_str().to_string(), o.route().to_string()))
        .collect();
    typed.sort();
    assert_eq!(typed, recorded);
}

/// The engine spec's `x-idempotency` marks equal the `keyable` rows. Reads
/// `CL_ENGINE_OPENAPI` (a path) when set, else the pinned
/// `engine/openapi.json`. A spec without any mark is an engine without
/// `idempotency-v1` (pin 7b267e48): PENDING, unless `CL_API_REQUIRE_CAPS`
/// lists the capability.
#[test]
fn engine_idempotency_marks_equal_the_keyable_rows() {
    let path = std::env::var_os("CL_ENGINE_OPENAPI")
        .map_or_else(|| repo_root().join("engine/openapi.json"), PathBuf::from);
    if !path.is_file() {
        assert!(
            !require(),
            "{} is missing and CL_API_REQUIRE=1",
            path.display()
        );
        eprintln!("SKIPPED: {} is absent", path.display());
        return;
    }
    let raw = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let spec: serde_json::Value = serde_json::from_slice(&raw).expect("openapi json");
    let marked = spec::idempotent_operations(&spec);
    if marked.is_empty() {
        let caps = std::env::var("CL_API_REQUIRE_CAPS").unwrap_or_default();
        assert!(
            !caps.split(',').any(|c| c.trim() == "idempotency-v1"),
            "CL_API_REQUIRE_CAPS lists idempotency-v1 but {} marks no route",
            path.display()
        );
        eprintln!(
            "PENDING(U-18): {} marks no x-idempotency route",
            path.display()
        );
        return;
    }
    assert_eq!(marked, spec::keyable_operations(OPERATIONS_TXT));
}

#[test]
fn idempotency_marks_are_read_per_operation() {
    let spec: serde_json::Value = serde_json::from_str(
        r#"{"paths":{
            "/a":{"post":{"x-idempotency":true},"get":{}},
            "/a/{id}":{"put":{"x-idempotency":true},"delete":{"x-idempotency":false}},
            "/b":{"post":{}}
        }}"#,
    )
    .expect("json");
    assert_eq!(
        spec::idempotent_operations(&spec),
        vec![
            ("POST".to_string(), "/a".to_string()),
            ("PUT".to_string(), "/a/{id}".to_string()),
        ]
    );
}

#[test]
fn schema_sources_are_listed_and_recorded() {
    let sources = spec::schema_sources(OPERATIONS_TXT);
    assert!(sources.len() >= 8, "operations.txt lists the Zod sources");
    let recorded = fixture("engine_schemas.sha256");
    for s in &sources {
        assert!(recorded.contains(s.as_str()), "{s} is not hashed");
    }
}

#[test]
fn canonical_form_sorts_keys_and_drops_whitespace() {
    let a: serde_json::Value =
        serde_json::from_str(r#"{ "b": [1, {"y":2,"x":1}], "a": "s" }"#).expect("json");
    assert_eq!(spec::canonical(&a), r#"{"a":"s","b":[1,{"x":1,"y":2}]}"#);
}
