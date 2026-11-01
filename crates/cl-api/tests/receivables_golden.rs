//! The receivables golden file (P22 plan §6.1): the desktop app's
//! `fixtures/receivables/golden.json` must be byte-identical to the engine's
//! `packages/shared/fixtures/receivables-golden.json` (the P21
//! precedent). A pinned engine without the P22 engine PRs has no such
//! file: PENDING, unless `CL_API_REQUIRE_CAPS` lists `credit-memo-apply-v1`.

use std::path::Path;

#[test]
fn the_receivables_golden_file_is_byte_identical_to_the_engines() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let ours =
        std::fs::read(root.join("fixtures/receivables/golden.json")).expect("our golden file");
    assert!(!ours.contains(&b'\r'), "LF line ends, as the engine's");
    let parsed: serde_json::Value = serde_json::from_slice(&ours).expect("valid JSON");
    assert!(parsed["cases"].as_array().is_some_and(|c| !c.is_empty()));
    let engine = root.join("engine/packages/shared/fixtures/receivables-golden.json");
    if !engine.is_file() {
        let caps = std::env::var("CL_API_REQUIRE_CAPS").unwrap_or_default();
        assert!(
            !caps.split(',').any(|c| c.trim() == "credit-memo-apply-v1"),
            "CL_API_REQUIRE_CAPS lists credit-memo-apply-v1 but {} is missing",
            engine.display()
        );
        eprintln!(
            "PENDING(U-137): {} is not in the pinned engine",
            engine.display()
        );
        return;
    }
    let theirs = std::fs::read(&engine).expect("engine golden file");
    // The engine is a submodule with its own line-ending settings: on a
    // Windows checkout with core.autocrlf its working copy has CRLF while
    // the committed file (and ours) is LF. Compare the committed content.
    let theirs: Vec<u8> = theirs.into_iter().filter(|&b| b != b'\r').collect();
    assert!(
        ours == theirs,
        "fixtures/receivables/golden.json differs from the engine's"
    );
}
