//! `cl-import`: readers for the files owners bring in.
//!
//! - [`csv`]: the RFC 4180 reader and writer the list import/export uses
//!   (P11), plus the bank CSV mapping in [`csv::mapping`] (P25).
//! - Bank files (P25, Q-11): [`detect`] finds the format, [`parse`] turns
//!   the bytes into a [`ParsedFile`] of [`Section`]s holding normalized
//!   [`BankRow`]s, skipped rows and line-numbered problems. The engine
//!   (U-06) computes keys, statuses and the balance check; this crate only
//!   reads files. [`dedupe`] is the demo-only copy of the engine's keys.
//!
//! No floating point anywhere (CLAUDE.md non-negotiable 6): amounts are
//! exact [`rust_decimal::Decimal`] values parsed through `cl-core` money,
//! and `clippy.toml` bans `f32`/`f64` in this crate.
#![forbid(unsafe_code)]
#![deny(
    clippy::float_arithmetic,
    clippy::float_cmp,
    clippy::cast_precision_loss
)]

pub mod csv;
pub mod dedupe;
pub mod detect;
pub mod error;
pub mod fuzz;
pub mod link;
pub mod normalize;
pub mod ofx;
pub mod qfx;
pub mod qif;
pub mod sign;

pub use csv::dates::{DateColumn, DateOrder};
pub use csv::mapping::{AmountColumns, CsvMapping, CsvTable};
pub use detect::{Detected, FileFormat, OfxDialect, detect};
pub use error::ImportError;
pub use normalize::{
    AccountId, AccountKind, Balance, BankRow, EncodingNote, Institution, MAX_DESCRIPTION_CHARS,
    MAX_FILE_BYTES, MAX_INTEGER_DIGITS, MAX_MEMO_CHARS, MAX_ROWS, ParsedAmount, ParsedFile,
    Problem, ProblemKind, Section, SkipReason, SkippedRow, TextEncoding,
};
pub use sign::{Continuity, RowOrder, SignCheck, SignReason, check_as, to_display, to_feed_amount};

use cl_core::CalendarDate;

/// What [`parse`] needs besides the bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseOptions {
    /// Today's date, for two-digit years (Q-P25g). Injected: the crate has
    /// no clock.
    pub today: CalendarDate,
    /// Date order for QIF and CSV files. `None` uses the CSV mapping's
    /// order, otherwise US month/day (Q-195).
    pub date_order: Option<DateOrder>,
    /// The CSV mapping to apply. `None` guesses one from the header and the
    /// values ([`csv::mapping::guess`]); a CSV with no usable guess fails
    /// with [`ImportError::CsvNeedsMapping`].
    pub csv_mapping: Option<CsvMapping>,
}

impl ParseOptions {
    /// Options with only today's date set.
    #[must_use]
    pub fn new(today: CalendarDate) -> Self {
        Self {
            today,
            date_order: None,
            csv_mapping: None,
        }
    }
}

/// Reads a bank file of any supported format.
///
/// The format is found from the content ([`detect`]), never from the file
/// name. Files over [`MAX_FILE_BYTES`] are refused before anything is read.
///
/// # Errors
/// [`ImportError`] when the whole file cannot be read: an unknown format or
/// character set, broken or refused markup (DOCTYPE, ENTITY, too deep), a
/// bank error inside the file, an investment statement, or a CSV with no
/// usable mapping. Problems with single rows are not errors; they are listed
/// on the section.
pub fn parse(bytes: &[u8], options: &ParseOptions) -> Result<ParsedFile, ImportError> {
    let detected = detect(bytes)?;
    match detected.format {
        FileFormat::Ofx => ofx::parse(bytes),
        FileFormat::Qfx => qfx::parse(bytes),
        FileFormat::Qif => qif::parse(bytes, options.today, options.date_order),
        FileFormat::Csv => csv::mapping::parse(bytes, options),
    }
}

/// Returns the crate name; kept so the skeleton test stays meaningful.
#[must_use]
pub fn crate_name() -> &'static str {
    "cl-import"
}

#[cfg(test)]
mod tests {
    #[test]
    fn crate_name_matches() {
        assert_eq!(super::crate_name(), "cl-import");
    }
}
