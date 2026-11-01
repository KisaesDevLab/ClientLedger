//! The customer payment golden file (P21 plan §6.1): the desktop app's
//! `fixtures/payments/golden.json` must be byte-identical to the engine's
//! `packages/shared/fixtures/payment-golden.json` (the U-85 / P20
//! precedent). Until the D9 pin bump the engine file is absent or still
//! being written: PENDING, unless `CL_API_REQUIRE_CAPS` lists
//! `receive-payment-v1` (then it must exist and match).

use std::path::Path;

#[test]
fn the_payment_golden_file_is_byte_identical_to_the_engines() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let ours = std::fs::read(root.join("fixtures/payments/golden.json")).expect("our golden file");
    assert!(!ours.contains(&b'\r'), "LF line ends, as the engine's");
    let parsed: serde_json::Value = serde_json::from_slice(&ours).expect("valid JSON");
    assert!(parsed["cases"].as_array().is_some_and(|c| !c.is_empty()));
    let required = std::env::var("CL_API_REQUIRE_CAPS")
        .unwrap_or_default()
        .split(',')
        .any(|c| c.trim() == "receive-payment-v1");
    let engine = root.join("engine/packages/shared/fixtures/payment-golden.json");
    let theirs = std::fs::read(&engine).ok().map(|t| {
        // The engine submodule may be checked out with CRLF (core.autocrlf).
        t.into_iter().filter(|&b| b != b'\r').collect::<Vec<u8>>()
    });
    match theirs {
        Some(t) if t == ours => {}
        other => {
            assert!(
                !required,
                "CL_API_REQUIRE_CAPS lists receive-payment-v1 but {} is {}",
                engine.display(),
                if other.is_some() {
                    "different"
                } else {
                    "missing"
                }
            );
            eprintln!(
                "PENDING(U-132): {} is not the pinned engine's copy yet (D9)",
                engine.display()
            );
        }
    }
}
