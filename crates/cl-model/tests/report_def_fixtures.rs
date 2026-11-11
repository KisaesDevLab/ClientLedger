//! Parity with the engine's zod schema (A07 plan §3, R-T14): every case in
//! `engine/packages/shared/src/report-builder/fixtures/cases.json` is read
//! here as the engine reads it, and every starter parses.
//!
//! - engine `ok`: the desktop accepts, unless the case says the desktop
//!   refuses (`duplicate_key.json`: serde sees the repeated key that
//!   `JSON.parse` drops);
//! - engine `SCHEMA`: the desktop refuses (serde's message has no zod path);
//!   a newer `schemaVersion` is `DefinitionError::Newer`;
//! - any other code: the same code at the same path.
//!
//! Before the engine pin with `report-defs-builder-v1` the folder is absent
//! and the test prints PENDING (Q-220); under `CL_API_REQUIRE_CAPS` listing
//! that capability it fails instead.

use std::path::PathBuf;

use cl_model::report_def::{Code, DefinitionError, parse};

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../engine/packages/shared/src/report-builder/fixtures")
}

fn required() -> bool {
    std::env::var("CL_API_REQUIRE_CAPS")
        .unwrap_or_default()
        .split(',')
        .any(|c| c.trim() == "report-defs-builder-v1")
}

#[test]
fn engine_cases_read_the_same_way() {
    let dir = fixtures();
    let Ok(text) = std::fs::read_to_string(dir.join("cases.json")) else {
        assert!(
            !required(),
            "report-defs-builder-v1 required but no shared fixtures"
        );
        println!("PENDING (PR-A report-defs-builder-v1): no shared fixtures yet");
        return;
    };
    let cases: Vec<serde_json::Value> = serde_json::from_str(&text).expect("cases.json");
    assert_ne!(cases.len(), 0, "cases.json is empty");
    for case in &cases {
        let file = case["file"].as_str().expect("file");
        let body =
            std::fs::read_to_string(dir.join(file)).unwrap_or_else(|e| panic!("{file}: {e}"));
        let got = parse(&body);
        let engine = &case["engine"];
        let desktop_refuses = case["desktop"]
            .as_str()
            .is_some_and(|d| d.starts_with("refused"));
        if engine["ok"] == true && !desktop_refuses {
            got.unwrap_or_else(|e| panic!("{file}: {e}"));
            continue;
        }
        if desktop_refuses {
            assert!(
                matches!(got, Err(DefinitionError::Malformed(ref m)) if m.contains("duplicate")),
                "{file}: {got:?}"
            );
            continue;
        }
        let code = engine["code"].as_str().expect("code");
        let path = engine["path"].as_str().expect("path");
        match code {
            "SCHEMA" if path == "definition.schemaVersion" => {
                assert_eq!(got, Err(DefinitionError::Newer), "{file}");
            }
            "SCHEMA" => assert!(
                matches!(
                    got,
                    Err(DefinitionError::Malformed(_)
                        | DefinitionError::Invalid {
                            code: Code::Schema,
                            ..
                        })
                ),
                "{file}: {got:?}"
            ),
            _ => assert_eq!(
                got,
                Err(DefinitionError::Invalid {
                    path: path.to_string(),
                    code: Code::parse(code).unwrap_or_else(|| panic!("{file}: code {code}")),
                }),
                "{file}"
            ),
        }
    }
}

#[test]
fn every_engine_starter_parses() {
    let Ok(entries) = std::fs::read_dir(fixtures().join("starters")) else {
        assert!(
            !required(),
            "report-defs-builder-v1 required but no starters"
        );
        println!("PENDING (PR-A report-defs-builder-v1): no starter fixtures yet");
        return;
    };
    let mut n = 0;
    for e in entries.flatten() {
        let text = std::fs::read_to_string(e.path()).expect("read");
        parse(&text).unwrap_or_else(|err| panic!("{}: {err}", e.path().display()));
        n += 1;
    }
    assert!(n > 0, "no starters");
}

#[test]
fn hand_fixtures_parse() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/report_def");
    for e in std::fs::read_dir(dir).expect("dir").flatten() {
        let text = std::fs::read_to_string(e.path()).expect("read");
        parse(&text).unwrap_or_else(|err| panic!("{}: {err}", e.path().display()));
    }
}
