//! The sales tax golden file (P23 plan §2.0, §6.1): the desktop app's
//! `fixtures/sales_tax/golden.json` must be byte-identical to the engine's
//! `packages/shared/fixtures/sales-tax-golden.json` (published on the E1
//! branch `clientledger/sales-tax-setup`). PENDING until the WP-4 pin bump
//! checks the engine's copy out.

use std::path::Path;

#[test]
fn the_sales_tax_golden_file_is_byte_identical_to_the_engines() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let ours = std::fs::read(root.join("fixtures/sales_tax/golden.json")).expect("our golden file");
    assert!(!ours.contains(&b'\r'), "LF line ends, as the engine's");
    let parsed: serde_json::Value = serde_json::from_slice(&ours).expect("valid JSON");
    assert!(parsed["cases"].as_array().is_some_and(|c| !c.is_empty()));
    let engine = root.join("engine/packages/shared/fixtures/sales-tax-golden.json");
    if !engine.is_file() {
        // Without the engine's copy (the pin before P23) the comparison is
        // pending, unless the run requires the P23 capabilities.
        let caps = std::env::var("CL_API_REQUIRE_CAPS").unwrap_or_default();
        assert!(
            !caps.split(',').any(|c| c.trim() == "sales-tax-setup-v1"),
            "CL_API_REQUIRE_CAPS lists sales-tax-setup-v1 but {} is missing",
            engine.display()
        );
        eprintln!("PENDING: {} is not checked out", engine.display());
        return;
    }
    let theirs = std::fs::read(&engine).expect("engine golden file");
    // A Windows submodule checkout may have CRLF; compare committed content.
    let theirs: Vec<u8> = theirs.into_iter().filter(|&b| b != b'\r').collect();
    assert!(
        ours == theirs,
        "fixtures/sales_tax/golden.json differs from the engine's"
    );
}
