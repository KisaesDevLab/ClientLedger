//! Duplicate keys (WP-A, step 11; plan D3 `dupes.rs`): the normalized
//! reference, the total and the date, sent to the engine, which decides
//! (U-92: same file hash; same vendor + reference; same vendor + amount +
//! date within 60 days).
//!
//! Reference normalization: lower case; only letters and digits kept
//! (spaces, `#`, `-`, `/`, `.` and other punctuation dropped); leading
//! zeros of every digit run dropped (`INV-00123` → `inv123`, `0000` → `0`).
//! The table is in `fixtures/docs/rule-cases.json` (`reference`); the
//! engine's U-92b duplicate check must normalize the same way (review N4).
//!
//! **Consumer:** the desktop (WP-F/G); the signatures here are fixed.

use crate::model::Header;
use cl_core::{CalendarDate, Money};
use std::fmt;

/// What the engine's duplicate check needs from a reading.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct DupeKey {
    /// [`normalize_reference`] of the reference.
    pub reference: Option<String>,
    pub total: Option<Money>,
    pub date: Option<CalendarDate>,
}

impl fmt::Debug for DupeKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DupeKey")
            .field("reference", &self.reference.is_some())
            .field("total", &self.total.is_some())
            .field("date", &self.date.is_some())
            .finish()
    }
}

/// The key of a reading's header. An empty normalized reference counts as
/// none.
#[must_use]
pub fn key(header: &Header) -> DupeKey {
    DupeKey {
        reference: header
            .reference
            .as_ref()
            .map(|r| normalize_reference(&r.value))
            .filter(|r| !r.is_empty()),
        total: header.total.as_ref().map(|t| t.value),
        date: header.date.as_ref().map(|d| d.value),
    }
}

/// A reference normalized for comparison (case, spaces, punctuation and
/// leading zeros, as the engine compares).
#[must_use]
pub fn normalize_reference(reference: &str) -> String {
    let mut out = String::new();
    let mut run = String::new();
    let flush = |out: &mut String, run: &mut String| {
        if !run.is_empty() {
            let trimmed = run.trim_start_matches('0');
            out.push_str(if trimmed.is_empty() { "0" } else { trimmed });
            run.clear();
        }
    };
    for c in reference.chars() {
        if c.is_ascii_digit() {
            run.push(c);
        } else {
            flush(&mut out, &mut run);
            if c.is_alphanumeric() {
                out.extend(c.to_lowercase());
            }
        }
    }
    flush(&mut out, &mut run);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Bbox, Confidence, Found, RuleId, Spot};
    use proptest::prelude::*;

    #[test]
    fn matches_the_shared_reference_table() {
        let json: serde_json::Value =
            serde_json::from_str(include_str!("../../../fixtures/docs/rule-cases.json")).unwrap();
        let cases = json["reference"].as_array().unwrap();
        assert!(cases.len() >= 8);
        for c in cases {
            let input = c["in"].as_str().unwrap();
            assert_eq!(
                normalize_reference(input),
                c["out"].as_str().unwrap(),
                "{input:?}"
            );
        }
    }

    #[test]
    fn key_of_a_header() {
        let found = |v: &str| Found {
            value: v.to_string(),
            spot: Spot {
                page: 0,
                bbox: Bbox::PAGE,
            },
            rule: RuleId::new("t"),
            confidence: Confidence::HIGH,
        };
        let mut h = Header::default();
        assert_eq!(key(&h), DupeKey::default());
        h.reference = Some(found("--"));
        assert_eq!(key(&h).reference, None, "nothing left after normalizing");
        h.reference = Some(found("INV-0042"));
        assert_eq!(key(&h).reference.as_deref(), Some("inv42"));
        assert_eq!(
            format!("{:?}", key(&h)),
            "DupeKey { reference: true, total: false, date: false }"
        );
    }

    proptest! {
        #[test]
        fn normalizing_ignores_case_punctuation_and_leading_zeros(s in "[A-Za-z0-9 #./-]{0,20}") {
            let n = normalize_reference(&s);
            prop_assert!(n.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()));
            prop_assert_eq!(normalize_reference(&format!("{s}-0042")), format!("{n}42"));
            prop_assert_eq!(normalize_reference(&s.to_uppercase()), n.clone());
            prop_assert_eq!(normalize_reference(&format!(" #{s}. ")), n);
        }
    }
}
