//! Document numbers (Q-94, Q-P19f): an optional prefix and a number that
//! counts up, keeping its leading zeros (`INV-0099` → `INV-0100`). A number
//! already used is a warning, not a block: the owner may keep it.
//!
//! Duplicates compare the trimmed text without regard to letter case, so
//! `inv-0100` duplicates `INV-0100`, but `INV-100` does not (it is a
//! different string, as the engine stores it).

/// A number split into its prefix and trailing digits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocNumber {
    /// Everything before the trailing digits (may be empty).
    pub prefix: String,
    /// The trailing digits (may be empty), leading zeros kept.
    pub digits: String,
}

impl DocNumber {
    /// Split `text` (trimmed) at its trailing digits.
    #[must_use]
    pub fn parse(text: &str) -> DocNumber {
        let t = text.trim();
        let start = t
            .char_indices()
            .rev()
            .take_while(|(_, c)| c.is_ascii_digit())
            .last()
            .map_or(t.len(), |(i, _)| i);
        DocNumber {
            prefix: t[..start].to_string(),
            digits: t[start..].to_string(),
        }
    }
}

/// `prefix` followed by `number` padded with zeros to `width` digits.
#[must_use]
pub fn format_number(prefix: &str, number: u64, width: usize) -> String {
    format!("{prefix}{number:0width$}")
}

/// The number after `last`: its trailing digits plus one, same prefix,
/// same width unless the count needs another digit (`A-99` → `A-100`).
/// `None` when `last` has no trailing digits or they would overflow.
#[must_use]
pub fn next_number(last: &str) -> Option<String> {
    let d = DocNumber::parse(last);
    if d.digits.is_empty() {
        return None;
    }
    let n: u64 = d.digits.parse().ok()?;
    Some(format_number(&d.prefix, n.checked_add(1)?, d.digits.len()))
}

/// The first number for a type with a prefix and nothing used yet.
#[must_use]
pub fn first_number(prefix: &str, start: u64, width: usize) -> String {
    format_number(prefix, start, width)
}

/// What is wrong with a number the owner is about to save.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NumberWarning {
    /// Another document of this type already has this number.
    Duplicate {
        /// The number as the other document has it.
        existing: String,
    },
}

/// Warn when `candidate` matches one of `existing` (trimmed, ignoring
/// letter case). Blank numbers are never duplicates.
#[must_use]
pub fn check_duplicate<'a, I>(candidate: &str, existing: I) -> Option<NumberWarning>
where
    I: IntoIterator<Item = &'a str>,
{
    let c = candidate.trim().to_lowercase();
    if c.is_empty() {
        return None;
    }
    existing
        .into_iter()
        .find(|e| e.trim().to_lowercase() == c)
        .map(|e| NumberWarning::Duplicate {
            existing: e.trim().to_string(),
        })
}

/// The next number after `last` that none of `existing` uses (skipping
/// numbers the owner typed by hand), trying at most `limit` numbers.
#[must_use]
pub fn next_free(last: &str, existing: &[&str], limit: usize) -> Option<String> {
    let mut cur = next_number(last)?;
    for _ in 0..limit {
        if check_duplicate(&cur, existing.iter().copied()).is_none() {
            return Some(cur);
        }
        cur = next_number(&cur)?;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_split_at_their_trailing_digits() {
        assert_eq!(
            DocNumber::parse(" INV-0042 "),
            DocNumber {
                prefix: "INV-".into(),
                digits: "0042".into()
            }
        );
        assert_eq!(DocNumber::parse("1001").prefix, "");
        assert_eq!(DocNumber::parse("2026-A").digits, "");
        assert_eq!(DocNumber::parse("").digits, "");
    }

    #[test]
    fn next_keeps_prefix_and_leading_zeros() {
        assert_eq!(next_number("INV-0099").as_deref(), Some("INV-0100"));
        assert_eq!(next_number("INV-9999").as_deref(), Some("INV-10000"));
        assert_eq!(next_number("1001").as_deref(), Some("1002"));
        assert_eq!(next_number("SR 007").as_deref(), Some("SR 008"));
        assert_eq!(next_number("A-2026-0009").as_deref(), Some("A-2026-0010"));
        assert_eq!(next_number("ABC"), None);
        assert_eq!(next_number("18446744073709551615"), None);
        assert_eq!(format_number("CM-", 7, 4), "CM-0007");
        assert_eq!(format_number("", 12345, 3), "12345");
        assert_eq!(first_number("INV-", 1, 4), "INV-0001");
    }

    #[test]
    fn duplicates_are_warned_ignoring_case_and_spaces() {
        let used = ["INV-0100", "INV-0101", " 1002 "];
        assert_eq!(
            check_duplicate("inv-0100", used),
            Some(NumberWarning::Duplicate {
                existing: "INV-0100".into()
            })
        );
        assert!(check_duplicate(" 1002", used).is_some());
        assert_eq!(check_duplicate("INV-100", used), None);
        assert_eq!(check_duplicate("INV-0102", used), None);
        assert_eq!(check_duplicate("  ", used), None);
    }

    #[test]
    fn next_free_skips_numbers_typed_by_hand() {
        let used = ["INV-0100", "INV-0101", "INV-0102"];
        assert_eq!(
            next_free("INV-0099", &used, 10).as_deref(),
            Some("INV-0103")
        );
        assert_eq!(next_free("INV-0099", &used, 2), None);
        assert_eq!(next_free("X", &used, 10), None);
    }
}
