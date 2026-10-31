//! The invoice money golden file (U-122, P20 plan §2): the desktop app's
//! `fixtures/invoices/golden.json` must be byte-identical to the engine's
//! `packages/shared/fixtures/invoice-golden.json` (the U-85 precedent). A
//! pinned engine from before engine PR #197 has no such file: PENDING,
//! unless `CL_API_REQUIRE_CAPS` lists `sales-cents-v1`.

use std::path::Path;

#[test]
fn the_golden_file_is_byte_identical_to_the_engines() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let ours = std::fs::read(root.join("fixtures/invoices/golden.json")).expect("our golden file");
    assert!(!ours.contains(&b'\r'), "LF line ends, as the engine's");
    let engine = root.join("engine/packages/shared/fixtures/invoice-golden.json");
    if !engine.is_file() {
        let caps = std::env::var("CL_API_REQUIRE_CAPS").unwrap_or_default();
        assert!(
            !caps.split(',').any(|c| c.trim() == "sales-cents-v1"),
            "CL_API_REQUIRE_CAPS lists sales-cents-v1 but {} is missing",
            engine.display()
        );
        eprintln!(
            "PENDING(U-122): {} is not in the pinned engine",
            engine.display()
        );
        return;
    }
    let theirs = std::fs::read(&engine).expect("engine golden file");
    assert!(
        ours == theirs,
        "fixtures/invoices/golden.json differs from the engine's"
    );
}
