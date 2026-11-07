//! Entry points for the fuzz targets in `crates/cl-import/fuzz/` (cargo-fuzz,
//! outside the workspace) and the CI smoke test (`tests/fuzz_smoke.rs`).
//! Each must return for any input without panicking.

use crate::{AmountColumns, CsvMapping, DateOrder, ParseOptions};
use cl_core::CalendarDate;

fn today() -> CalendarDate {
    // A fixed date: fuzzing must be repeatable.
    CalendarDate::from_ymd(2026, 11, 7).unwrap_or_else(|_| CalendarDate::min())
}

/// OFX/QFX bytes through the OFX reader.
pub fn fuzz_ofx(data: &[u8]) {
    let _ = crate::ofx::parse(data);
}

/// QIF bytes through the QIF reader.
pub fn fuzz_qif(data: &[u8]) {
    let _ = crate::qif::parse(data, today(), None);
}

/// CSV bytes through table reading, the mapping guess, and two mappings.
pub fn fuzz_csv(data: &[u8]) {
    if let Ok(table) = crate::csv::mapping::read_table(data) {
        if let Some(mapping) = crate::csv::mapping::guess(&table) {
            let _ = crate::csv::mapping::apply(&table, &mapping, today());
        }
        let fixed = CsvMapping {
            version: 1,
            has_header: false,
            date: 0,
            date_order: DateOrder::Dmy,
            amount: AmountColumns::DebitCredit {
                debit: 1,
                credit: 2,
            },
            description: vec![3],
            memo: None,
            check_number: None,
            balance: Some(4),
            headers: Vec::new(),
        };
        let _ = crate::csv::mapping::apply(&table, &fixed, today());
    }
}

/// Any bytes through detection and the matching reader.
pub fn fuzz_any(data: &[u8]) {
    let _ = crate::parse(data, &ParseOptions::new(today()));
}
