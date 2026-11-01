//! The receivables golden file (P22 plan §6.1): the desktop app's
//! `fixtures/receivables/golden.json` must be byte-identical to the engine's
//! `packages/shared/fixtures/receivables-golden.json` (the P21
//! precedent). The engine has shipped it since pin bf2daf38 (P22 engine
//! PRs #209-#213).

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
    assert!(
        engine.is_file(),
        "{} is missing: the pinned engine (bf2daf38 or later) ships it",
        engine.display()
    );
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
