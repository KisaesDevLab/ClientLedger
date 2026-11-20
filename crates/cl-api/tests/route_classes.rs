//! LAN route classes (P33 D3 step 8, engine U-288 `route-classes-v1`).
//!
//! - Every mutating operation of the pinned engine spec carries `x-lan`
//!   (`workstation` | `host`) and `x-stale` (one of the classes below). An
//!   unclassified route fails here (default deny is the engine's; this is
//!   the client's side of the same rule).
//! - The cl-api operations that edit a stale-checked record are recorded in
//!   `tests/fixtures/stale_routes.txt` (with their stamp dialect), and those a
//!   workstation may not call in `tests/fixtures/host_only_routes.txt`. A
//!   change in the engine's classification of a covered route fails until
//!   the fixtures (and the forms that send stamps, and the host-only list the
//!   UI disables) are updated: `CL_UPDATE_FIXTURES=1` rewrites them.
//!
//! A spec with no `x-lan` at all is an engine before U-288: PENDING, unless
//! `CL_API_REQUIRE_CAPS` lists `route-classes-v1`.

#[path = "support/spec.rs"]
mod spec;

use std::path::{Path, PathBuf};

use serde_json::Value;

const OPERATIONS_TXT: &str = include_str!("../operations.txt");
const MUTATING: [&str; 4] = ["post", "put", "patch", "delete"];
const STALE_CLASSES: [&str; 7] = [
    "stale(updatedAt)",
    "stale(version)",
    "stale(schedule)",
    "create",
    "state-guarded",
    "bulk",
    "n-a",
];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// The spec, or `None` (skipped or pending, with a note).
fn spec_with_classes() -> Option<Value> {
    let path = std::env::var_os("CL_ENGINE_OPENAPI")
        .map_or_else(|| repo_root().join("engine/openapi.json"), PathBuf::from);
    if !path.is_file() {
        assert!(
            std::env::var("CL_API_REQUIRE").as_deref() != Ok("1"),
            "{} is missing and CL_API_REQUIRE=1",
            path.display()
        );
        eprintln!("SKIPPED: {} is absent", path.display());
        return None;
    }
    let raw = std::fs::read(&path).expect("read spec");
    let spec: Value = serde_json::from_slice(&raw).expect("openapi json");
    let any = spec["paths"].as_object().is_some_and(|p| {
        p.values().any(|ops| {
            ops.as_object()
                .is_some_and(|o| o.values().any(|op| op.get("x-lan").is_some()))
        })
    });
    if !any {
        let caps = std::env::var("CL_API_REQUIRE_CAPS").unwrap_or_default();
        assert!(
            !caps.split(',').any(|c| c.trim() == "route-classes-v1"),
            "CL_API_REQUIRE_CAPS lists route-classes-v1 but the spec has no x-lan"
        );
        eprintln!("PENDING(U-288): the spec has no x-lan marks");
        return None;
    }
    Some(spec)
}

fn op<'a>(spec: &'a Value, method: &str, route: &str) -> Option<&'a Value> {
    spec["paths"].get(route)?.get(method.to_ascii_lowercase())
}

#[test]
fn every_mutating_operation_is_classified() {
    let Some(spec) = spec_with_classes() else {
        return;
    };
    let mut problems = Vec::new();
    for (route, ops) in spec["paths"].as_object().expect("paths") {
        for m in MUTATING {
            let Some(o) = ops.get(m) else { continue };
            match o.get("x-lan").and_then(Value::as_str) {
                Some("workstation" | "host") => {}
                other => problems.push(format!("{m} {route}: x-lan {other:?}")),
            }
            match o.get("x-stale").and_then(Value::as_str) {
                Some(s) if STALE_CLASSES.contains(&s) => {}
                other => problems.push(format!("{m} {route}: x-stale {other:?}")),
            }
        }
    }
    assert!(
        problems.is_empty(),
        "unclassified routes:\n{}",
        problems.join("\n")
    );
}

fn computed(spec: &Value) -> (String, String, Vec<String>) {
    let mut stale = Vec::new();
    let mut host = Vec::new();
    let mut missing = Vec::new();
    for (method, route) in spec::operations(OPERATIONS_TXT) {
        let Some(o) = op(spec, &method, &route) else {
            // Desktop-only routes (`/api/v1/desktop/*`) are not in the
            // server spec; they follow the U-288 rules: host-only except
            // `GET /desktop/info` and `GET /desktop/session-secrets`.
            if route == "/openapi.json" {
                // The spec itself (a probe), not an engine operation.
            } else if route.starts_with("/api/v1/tenants/{id}/") {
                // U-288: every route below a tenant id is host-only (some,
                // like the delete job, are desktop-only and not in the spec).
                host.push(format!("{method} {route}"));
            } else if route.starts_with("/api/v1/desktop/") {
                let read_ok = method == "GET"
                    && (route == "/api/v1/desktop/info"
                        || route == "/api/v1/desktop/session-secrets");
                if !read_ok {
                    host.push(format!("{method} {route}"));
                }
            } else {
                missing.push(format!("{method} {route}"));
            }
            continue;
        };
        let lan = o.get("x-lan").and_then(Value::as_str).unwrap_or("host");
        if lan == "host" {
            host.push(format!("{method} {route}"));
        }
        if let Some(s) = o.get("x-stale").and_then(Value::as_str)
            && s.starts_with("stale(")
        {
            stale.push(format!("{method} {route} {s}"));
        }
    }
    stale.sort();
    stale.dedup();
    host.sort();
    host.dedup();
    let head = |what: &str| {
        format!(
            "# {what}\n# Generated by crates/cl-api/tests/route_classes.rs from the pinned engine spec\n# (CL_UPDATE_FIXTURES=1). One `METHOD route` per line.\n"
        )
    };
    (
        head(
            "cl-api operations that edit a stale-checked record (x-stale), with the stamp dialect.",
        ) + &stale.join("\n")
            + "\n",
        head(
            "cl-api operations a LAN workstation may not call (x-lan: host); the UI disables them there.",
        ) + &host.join("\n")
            + "\n",
        missing,
    )
}

#[test]
fn covered_routes_match_the_recorded_classes() {
    let Some(spec) = spec_with_classes() else {
        return;
    };
    let (stale, host, missing) = computed(&spec);
    assert!(
        missing.is_empty(),
        "covered operations missing from the spec:\n{}",
        missing.join("\n")
    );
    if std::env::var("CL_UPDATE_FIXTURES").as_deref() == Ok("1") {
        std::fs::write(fixture_path("stale_routes.txt"), &stale).expect("write");
        std::fs::write(fixture_path("host_only_routes.txt"), &host).expect("write");
        return;
    }
    let hint = "the engine's route classes changed: rerun with CL_UPDATE_FIXTURES=1, then update the forms' stamps and the host-only list";
    let read = |n: &str| {
        std::fs::read_to_string(fixture_path(n))
            .unwrap_or_default()
            .replace("\r\n", "\n")
    };
    assert_eq!(read("stale_routes.txt"), stale, "stale_routes.txt: {hint}");
    assert_eq!(
        read("host_only_routes.txt"),
        host,
        "host_only_routes.txt: {hint}"
    );
}
