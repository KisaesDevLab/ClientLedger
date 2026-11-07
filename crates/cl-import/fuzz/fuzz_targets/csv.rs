//! cargo-fuzz target: csv bytes through cl-import (P25 step 8).
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    cl_import::fuzz::fuzz_csv(data);
    cl_import::fuzz::fuzz_any(data);
});
