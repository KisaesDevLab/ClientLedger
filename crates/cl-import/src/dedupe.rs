//! The demo-only copy of the engine's dedupe keys v1 (U-06, Q-P25r).
//!
//! The engine computes the real keys; this copy lets the demo backend
//! classify rows the same way. It must match the golden file
//! `fixtures/bank/dedupe_v1.json` (a byte-identical copy of the engine's
//! `packages/api/src/services/bank-import/__fixtures__/dedupe_v1.json`),
//! which also pins the OFX description rule, so a later desktop version
//! cannot change the keys the engine sees.

use crate::normalize::{is_ws, trim_ws};
use cl_core::CalendarDate;
use rust_decimal::Decimal;
use sha2::{Digest as _, Sha256};
use std::collections::HashMap;
use unicode_normalization::UnicodeNormalization as _;

/// The key version this copy computes (the rows' `dedupe_version`).
pub const VERSION: u32 = 1;
/// `norm` keeps at most this many code points.
pub const NORM_MAX_CODE_POINTS: usize = 500;

/// NFC; ASCII a–z upper-cased only; every run of the [`is_ws`] set becomes
/// one space; trimmed; cut to 500 code points (in that order, as the
/// engine does).
#[must_use]
pub fn norm(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_ws = false;
    for c in s.nfc() {
        if is_ws(c) {
            if !in_ws {
                out.push(' ');
            }
            in_ws = true;
        } else {
            out.push(c.to_ascii_uppercase());
            in_ws = false;
        }
    }
    let trimmed = trim_ws(&out);
    trimmed.chars().take(NORM_MAX_CODE_POINTS).collect()
}

/// The amount with exactly 2 places, `-` only when negative. Callers pass
/// amounts of at most 2 decimals (plan §4); more would round half up, as
/// the engine's decimal.js does.
#[must_use]
pub fn canonical_amount(amount: Decimal) -> String {
    let mut d =
        amount.round_dp_with_strategy(2, rust_decimal::RoundingStrategy::MidpointAwayFromZero);
    if d.is_zero() {
        d.set_sign_positive(true);
    }
    d.rescale(2);
    d.to_string()
}

/// `sha256("v1|row|" + date + "|" + canonical + "|" + norm + "|" + ordinal)`.
#[must_use]
pub fn row_key(
    date: CalendarDate,
    wire_amount: Decimal,
    description: &str,
    ordinal: u32,
) -> String {
    row_key_raw(
        &date.to_wire(),
        &canonical_amount(wire_amount),
        &norm(description),
        ordinal,
    )
}

fn row_key_raw(date: &str, canonical: &str, norm_description: &str, ordinal: u32) -> String {
    sha256_hex(&format!(
        "v1|row|{date}|{canonical}|{norm_description}|{ordinal}"
    ))
}

/// `sha256("v1|fitid|" + trimmed FITID)`.
#[must_use]
pub fn fitid_key(fitid: &str) -> String {
    sha256_hex(&format!("v1|fitid|{}", trim_ws(fitid)))
}

/// The OFX description: NAME only, trimmed (owner decision Q-P25r,
/// 2026-11-07). MEMO is never merged in; it goes to the row's memo.
#[must_use]
pub fn ofx_description(name: Option<&str>) -> String {
    trim_ws(name.unwrap_or_default()).to_owned()
}

/// The OFX row memo: MEMO trimmed, `None` when empty (golden `memoOut`).
#[must_use]
pub fn ofx_memo(memo: Option<&str>) -> Option<String> {
    memo.map(trim_ws)
        .filter(|m| !m.is_empty())
        .map(str::to_owned)
}

/// One row's key, with the ordinal it got.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyedRow {
    /// 1-based position among rows with the same date, canonical amount and
    /// normalized description.
    pub ordinal: u32,
    /// The row key (lowercase hex).
    pub row_key: String,
}

/// Row keys for a request's rows in order (date, wire amount, description).
/// Pass only the rows that are sent: skipped zero rows take no ordinal.
#[must_use]
pub fn key_rows<'a, I>(rows: I) -> Vec<KeyedRow>
where
    I: IntoIterator<Item = (CalendarDate, Decimal, &'a str)>,
{
    let mut seen: HashMap<String, u32> = HashMap::new();
    rows.into_iter()
        .map(|(date, amount, description)| {
            let date = date.to_wire();
            let canonical = canonical_amount(amount);
            let d = norm(description);
            let ordinal = seen.entry(format!("{date}|{canonical}|{d}")).or_insert(0);
            *ordinal += 1;
            KeyedRow {
                ordinal: *ordinal,
                row_key: row_key_raw(&date, &canonical, &d, *ordinal),
            }
        })
        .collect()
}

fn sha256_hex(s: &str) -> String {
    hex(&Sha256::digest(s.as_bytes()))
}

/// Lowercase hex of some bytes.
pub(crate) fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        // Writing to a String cannot fail.
        let _ = write!(out, "{b:02x}");
    }
    out
}
