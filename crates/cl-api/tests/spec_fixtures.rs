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

/// The engine's keyed route sets as recorded: `idempotency-v1` (U-18),
/// `idempotency-ap-v1` (U-83 to U-87), `idempotency-docs-v1` (U-92a) and
/// `document-drafts-attach-v1` (U-95), sorted and disjoint.
type Routes = Vec<(String, String)>;

fn recorded_idempotency_sets() -> (Routes, Routes, Routes, Routes, Routes, Routes, Routes) {
    let v1 = spec::route_lines(&fixture("idempotency_v1_routes.txt"));
    let ap = spec::route_lines(&fixture("idempotency_ap_v1_routes.txt"));
    let docs = spec::route_lines(&fixture("idempotency_docs_v1_routes.txt"));
    let attach = spec::route_lines(&fixture("idempotency_docs_attach_v1_routes.txt"));
    let pay = spec::route_lines(&fixture("idempotency_pay_bills_v1_routes.txt"));
    let print = spec::route_lines(&fixture("idempotency_check_print_v1_routes.txt"));
    let invoice = spec::route_lines(&fixture("idempotency_invoice_v1_routes.txt"));
    assert_eq!(v1.len(), 20, "the idempotency-v1 set has 20 routes");
    assert_eq!(ap.len(), 6, "the idempotency-ap-v1 set has 6 routes");
    assert_eq!(docs.len(), 2, "the idempotency-docs-v1 set has 2 routes");
    assert_eq!(
        attach.len(),
        1,
        "the document-drafts-attach-v1 set has 1 route"
    );
    assert!(
        ap.iter().all(|r| !v1.contains(r)),
        "idempotency-ap-v1 only adds routes"
    );
    assert!(
        docs.iter().all(|r| !v1.contains(r) && !ap.contains(r)),
        "idempotency-docs-v1 only adds routes"
    );
    assert!(
        attach
            .iter()
            .all(|r| !v1.contains(r) && !ap.contains(r) && !docs.contains(r)),
        "document-drafts-attach-v1 only adds routes"
    );
    assert_eq!(pay.len(), 1, "the idempotency-pay-bills-v1 set has 1 route");
    assert!(
        pay.iter().all(|r| !v1.contains(r)
            && !ap.contains(r)
            && !docs.contains(r)
            && !attach.contains(r)),
        "idempotency-pay-bills-v1 only adds routes"
    );
    assert_eq!(
        print.len(),
        1,
        "the idempotency-check-print-v1 set has 1 route"
    );
    assert!(
        print.iter().all(|r| !v1.contains(r)
            && !ap.contains(r)
            && !docs.contains(r)
            && !attach.contains(r)
            && !pay.contains(r)),
        "idempotency-check-print-v1 only adds routes"
    );
    assert_eq!(
        invoice.len(),
        3,
        "the idempotency-invoice-v1 set has 3 routes"
    );
    assert!(
        invoice.iter().all(|r| !v1.contains(r)
            && !ap.contains(r)
            && !docs.contains(r)
            && !attach.contains(r)
            && !pay.contains(r)
            && !print.contains(r)),
        "idempotency-invoice-v1 only adds routes"
    );
    (v1, ap, docs, attach, pay, print, invoice)
}

/// The rows `operations.txt` must class `keyable` (every `idempotency-v1`
/// route), `keyable-ap` (every `idempotency-ap-v1` route; P17 covers all
/// six) and `keyable-docs` (every `idempotency-docs-v1` route; A03 covers
/// both) and `keyable-docs-attach` (the `document-drafts-attach-v1` route;
/// A04 covers it). A route is keyed only under its own set's capability.
fn expected_keyable() -> (Routes, Routes, Routes, Routes, Routes, Routes, Routes) {
    recorded_idempotency_sets()
}

/// The typed table's routes in `set`, sorted.
fn typed_keyable(set: cl_api::KeySet) -> Routes {
    let mut typed: Routes = cl_api::OPERATIONS
        .iter()
        .filter(|o| o.key_set() == Some(set))
        .map(|o| (o.method().as_str().to_string(), o.route().to_string()))
        .collect();
    typed.sort();
    typed
}

/// C2 (U-18 plan §10.1): the `keyable` rows of `operations.txt` are exactly
/// the engine's `idempotency-v1` route set and the `keyable-ap` rows exactly
/// its `idempotency-ap-v1` set, as its `openapi.json` marks them
/// (`x-idempotency: true`, recorded in `idempotency_v1_routes.txt` and
/// `idempotency_ap_v1_routes.txt`), and the typed table agrees set by set.
/// No engine checkout needed.
#[test]
fn keyable_invoice_email_rows_are_the_recorded_set() {
    let email = spec::route_lines(&fixture("idempotency_invoice_email_v1_routes.txt"));
    assert_eq!(email.len(), 2, "the invoice-email-v1 set has 2 routes");
    assert_eq!(
        spec::keyable_invoice_email_operations(OPERATIONS_TXT),
        email
    );
    assert_eq!(typed_keyable(cl_api::KeySet::InvoiceEmailV1), email);
}

/// The `keyable-receive-payment` rows are the recorded
/// `idempotency-receive-payment-v1` set (U-132, P21; client-declared until
/// the pin has it) and the typed table agrees.
#[test]
fn keyable_receive_payment_rows_are_the_recorded_set() {
    let pay = spec::route_lines(&fixture("idempotency_receive_payment_v1_routes.txt"));
    assert_eq!(
        pay.len(),
        7,
        "the idempotency-receive-payment-v1 set has 7 routes"
    );
    assert_eq!(
        spec::keyable_receive_payment_operations(OPERATIONS_TXT),
        pay
    );
    assert_eq!(typed_keyable(cl_api::KeySet::ReceivePaymentV1), pay);
}

#[test]
fn keyable_rows_are_the_engines_idempotency_routes() {
    let (v1, ap, docs, attach, pay, print, invoice) = expected_keyable();
    assert_eq!(spec::keyable_invoice_operations(OPERATIONS_TXT), invoice);
    assert_eq!(typed_keyable(cl_api::KeySet::InvoiceV1), invoice);
    assert_eq!(spec::keyable_operations(OPERATIONS_TXT), v1);
    assert_eq!(spec::keyable_pay_bills_operations(OPERATIONS_TXT), pay);
    assert_eq!(typed_keyable(cl_api::KeySet::PayBillsV1), pay);
    assert_eq!(spec::keyable_check_print_operations(OPERATIONS_TXT), print);
    assert_eq!(typed_keyable(cl_api::KeySet::CheckPrintV1), print);
    assert_eq!(spec::keyable_ap_operations(OPERATIONS_TXT), ap);
    assert_eq!(spec::keyable_docs_operations(OPERATIONS_TXT), docs);
    assert_eq!(spec::keyable_docs_attach_operations(OPERATIONS_TXT), attach);
    assert_eq!(typed_keyable(cl_api::KeySet::V1), v1);
    assert_eq!(typed_keyable(cl_api::KeySet::ApV1), ap);
    assert_eq!(typed_keyable(cl_api::KeySet::DocsV1), docs);
    assert_eq!(typed_keyable(cl_api::KeySet::DocsAttachV1), attach);
    assert_eq!(
        cl_api::OPERATIONS.iter().filter(|o| o.is_keyable()).count(),
        v1.len()
            + ap.len()
            + docs.len()
            + attach.len()
            + pay.len()
            + print.len()
            + invoice.len()
            + spec::route_lines(&fixture("idempotency_invoice_email_v1_routes.txt")).len()
            + spec::route_lines(&fixture("idempotency_receive_payment_v1_routes.txt")).len()
    );
}

/// The engine spec's `x-idempotency` marks are exactly the recorded
/// `idempotency-v1` and `idempotency-ap-v1` sets, and the `keyable` rows are
/// those marks the registry covers. Reads
/// `CL_ENGINE_OPENAPI` (a path) when set, else the pinned
/// `engine/openapi.json`. A spec without any mark is an engine without
/// `idempotency-v1` (pins before f3ce33fd): PENDING, unless `CL_API_REQUIRE_CAPS`
/// lists the capability.
/// Whether the spec marks any route of a keyed set that arrived later; when
/// it marks none, PENDING unless `CL_API_REQUIRE_CAPS` lists `cap`.
fn later_set(marked: &Routes, set: &Routes, cap: &str, item: &str, path: &Path) -> bool {
    if set.iter().any(|r| marked.contains(r)) {
        return true;
    }
    let caps = std::env::var("CL_API_REQUIRE_CAPS").unwrap_or_default();
    assert!(
        !caps.split(',').any(|c| c.trim() == cap),
        "CL_API_REQUIRE_CAPS lists {cap} but {} marks none of its routes",
        path.display()
    );
    eprintln!("PENDING({item}): {} marks no {cap} route", path.display());
    false
}

#[test]
#[allow(clippy::too_many_lines, reason = "one check per keyed route set")]
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
    let (v1, ap, docs, attach, pay, print, invoice) = recorded_idempotency_sets();
    let mut sets = v1.clone();
    sets.extend(ap.iter().cloned());
    // A keyed set that arrived after `idempotency-v1` (U-92a with engine
    // PR #178, U-95 with PR #180, U-102 with the P18 engine PR): a spec that
    // marks none of its routes is a pin from before it (PENDING, unless
    // `CL_API_REQUIRE_CAPS` lists the capability); one that marks any must
    // mark exactly the recorded set.
    let has_docs = later_set(&marked, &docs, "idempotency-docs-v1", "U-92a", &path);
    let has_attach = later_set(&marked, &attach, "document-drafts-attach-v1", "U-95", &path);
    let has_pay = later_set(&marked, &pay, "idempotency-pay-bills-v1", "U-102", &path);
    let has_print = later_set(
        &marked,
        &print,
        "idempotency-check-print-v1",
        "U-116",
        &path,
    );
    let has_invoice = later_set(&marked, &invoice, "idempotency-invoice-v1", "U-124", &path);
    let email = spec::route_lines(&fixture("idempotency_invoice_email_v1_routes.txt"));
    let has_email = later_set(&marked, &email, "invoice-email-v1", "U-127", &path);
    let receive = spec::route_lines(&fixture("idempotency_receive_payment_v1_routes.txt"));
    let has_receive = later_set(
        &marked,
        &receive,
        "idempotency-receive-payment-v1",
        "U-132",
        &path,
    );
    for (has, set) in [
        (has_email, &email),
        (has_receive, &receive),
        (has_docs, &docs),
        (has_attach, &attach),
        (has_pay, &pay),
        (has_print, &print),
        (has_invoice, &invoice),
    ] {
        if has {
            sets.extend(set.iter().cloned());
        }
    }
    sets.sort();
    assert_eq!(marked, sets, "the engine's keyed routes changed");
    let keyable = spec::keyable_operations(OPERATIONS_TXT);
    let keyable_ap = spec::keyable_ap_operations(OPERATIONS_TXT);
    let keyable_docs = spec::keyable_docs_operations(OPERATIONS_TXT);
    let keyable_attach = spec::keyable_docs_attach_operations(OPERATIONS_TXT);
    let keyable_pay = spec::keyable_pay_bills_operations(OPERATIONS_TXT);
    let keyable_print = spec::keyable_check_print_operations(OPERATIONS_TXT);
    let keyable_invoice = spec::keyable_invoice_operations(OPERATIONS_TXT);
    assert!(
        keyable
            .iter()
            .chain(&keyable_ap)
            .chain(if has_docs {
                keyable_docs.iter()
            } else {
                [].iter()
            })
            .chain(if has_attach {
                keyable_attach.iter()
            } else {
                [].iter()
            })
            .chain(if has_pay {
                keyable_pay.iter()
            } else {
                [].iter()
            })
            .chain(if has_print {
                keyable_print.iter()
            } else {
                [].iter()
            })
            .chain(if has_invoice {
                keyable_invoice.iter()
            } else {
                [].iter()
            })
            .all(|r| marked.contains(r)),
        "every keyable row is marked by the engine"
    );
    assert_eq!(keyable, v1, "keyable = the idempotency-v1 set");
    assert_eq!(keyable_ap, ap, "keyable-ap = the idempotency-ap-v1 set");
    assert_eq!(
        keyable_docs, docs,
        "keyable-docs = the idempotency-docs-v1 set"
    );
    assert_eq!(
        keyable_attach, attach,
        "keyable-docs-attach = the document-drafts-attach-v1 set"
    );
    assert_eq!(
        keyable_pay, pay,
        "keyable-pay-bills = the idempotency-pay-bills-v1 set"
    );
    assert_eq!(
        keyable_print, print,
        "keyable-check-print = the idempotency-check-print-v1 set"
    );
    assert_eq!(
        keyable_invoice, invoice,
        "keyable-invoice = the idempotency-invoice-v1 set"
    );
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
