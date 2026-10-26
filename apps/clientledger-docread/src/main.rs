//! `clientledger-docread`: the document reading worker (A03, plan D2).
//!
//! `cl-host` starts it per batch with a scrubbed environment and an empty
//! working directory, writes one `Batch` frame to stdin and reads
//! `cl_docread::protocol::FromWorker` frames from stdout. It reads untrusted
//! files (PDF through `PDFium`, images in Rust), asks the host to OCR pages
//! (`NeedOcr`), runs the rules and answers `JobDone`/`JobFailed`.
//!
//! It has no network code and spawns nothing; its limits (`limits.rs`) are
//! set first in `main`, before `PDFium`, threads or stdin (WP-B, step 6:
//! `limits.rs`, `batch.rs`, `hooks.rs`, `pdfium.rs`).
//!
//! Skeleton (A03 step 2): it refuses to run until step 6 lands.
#![forbid(unsafe_code)]

// Test hooks crash, hang and allocate on purpose: never in a release build.
// Tests build the worker in the dev profile (or `docread-bench`, which keeps
// debug assertions) with this feature on demand.
#[cfg(all(feature = "test-support", not(debug_assertions)))]
compile_error!(
    "the `test-support` feature is for tests only and must not be enabled in a release build"
);

use std::process::ExitCode;

fn main() -> ExitCode {
    // Not a user-facing program: cl-host reports a worker that exits before
    // `Ready` as "Couldn't read, enter by hand".
    let _ = cl_docread::protocol::PROTOCOL_VERSION;
    eprintln!(
        "clientledger-docread: started only by ClientLedger (worker loop lands in A03 step 6)"
    );
    ExitCode::from(2)
}
