//! Parity with the engine's zod schema (A07 plan §3, R-T14): every shared
//! fixture in `engine/packages/shared/src/report-builder/fixtures/` parses
//! here exactly as it does there. A file whose name starts with `invalid`
//! must be refused; every other one must parse and pass the rules.
//!
//! Until PR-A (`report-defs-builder-v1`) is pinned the folder is absent and
//! the test prints PENDING (Q-220); the hand fixtures under
//! `tests/fixtures/report_def/` cover the rules meanwhile.

use std::path::PathBuf;

fn engine_fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../engine/packages/shared/src/report-builder/fixtures")
}

#[test]
fn engine_fixtures_parse_the_same_way() {
    let dir = engine_fixtures();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        println!("PENDING (PR-A report-defs-builder-v1): no shared fixtures yet");
        return;
    };
    let mut n = 0;
    for e in entries.flatten() {
        let path = e.path();
        if path.extension().and_then(|x| x.to_str()) != Some("json") {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|x| x.to_str())
            .unwrap_or_default()
            .to_string();
        let text = std::fs::read_to_string(&path).expect("read");
        let r = cl_model::report_def::parse(&text);
        if name.starts_with("invalid") {
            assert!(r.is_err(), "{name} should be refused");
        } else {
            r.unwrap_or_else(|e| panic!("{name}: {e}"));
        }
        n += 1;
    }
    assert!(n > 0, "the fixture folder is empty");
}

#[test]
fn hand_fixtures_parse() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/report_def");
    for e in std::fs::read_dir(dir).expect("dir").flatten() {
        let text = std::fs::read_to_string(e.path()).expect("read");
        cl_model::report_def::parse(&text)
            .unwrap_or_else(|err| panic!("{}: {err}", e.path().display()));
    }
}
