//! Document numbers (Q-94, Q-P19f), following the engine's rules
//! (U-111, `document-numbers-v1`): a number is the type's prefix followed by
//! a plain integer (`INV-` + 100 = `INV-100`, no zero padding), and a
//! duplicate is the same text (case matters; surrounding spaces don't). When
//! the engine offers `GET /document-numbers/next`, its `next` is the number
//! to use ([`suggest_next`]); these helpers only cover engines without it.
//! A duplicate is a warning, not a block: the owner may keep the number.

/// A number split into its prefix and trailing digits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocNumber {
    /// Everything before the trailing digits (may be empty).
    pub prefix: String,
    /// The trailing digits (may be empty), as typed.
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

/// `prefix` followed by `number` (`INV-` + 7 = `INV-7`), as the engine
/// formats it.
#[must_use]
pub fn format_number(prefix: &str, number: u64) -> String {
    format!("{prefix}{number}")
}

/// The number after `last`: the same prefix and its trailing integer plus
/// one (`INV-0099` → `INV-100`). `None` when `last` has no trailing digits
/// or they would overflow.
#[must_use]
pub fn next_number(last: &str) -> Option<String> {
    let d = DocNumber::parse(last);
    if d.digits.is_empty() {
        return None;
    }
    let n: u64 = d.digits.parse().ok()?;
    Some(format_number(&d.prefix, n.checked_add(1)?))
}

/// The number to offer: the engine's `next` when the engine has
/// `document-numbers-v1`, else the number after the last one used, else
/// none (the owner types it, Q-P19f).
#[must_use]
pub fn suggest_next(engine_next: Option<&str>, last: Option<&str>) -> Option<String> {
    match engine_next.map(str::trim).filter(|s| !s.is_empty()) {
        Some(n) => Some(n.to_string()),
        None => last.and_then(next_number),
    }
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

/// Warn when `candidate` is the same text as one of `existing` (surrounding
/// spaces ignored, case kept, as the engine compares). Blank numbers are
/// never duplicates.
#[must_use]
pub fn check_duplicate<'a, I>(candidate: &str, existing: I) -> Option<NumberWarning>
where
    I: IntoIterator<Item = &'a str>,
{
    let c = candidate.trim();
    if c.is_empty() {
        return None;
    }
    existing
        .into_iter()
        .find(|e| e.trim() == c)
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
    fn next_is_prefix_plus_integer_like_the_engine() {
        assert_eq!(next_number("INV-0099").as_deref(), Some("INV-100"));
        assert_eq!(next_number("INV-9999").as_deref(), Some("INV-10000"));
        assert_eq!(next_number("1001").as_deref(), Some("1002"));
        assert_eq!(next_number("SR 007").as_deref(), Some("SR 8"));
        assert_eq!(next_number("A-2026-9").as_deref(), Some("A-2026-10"));
        assert_eq!(next_number("ABC"), None);
        assert_eq!(next_number("18446744073709551615"), None);
        assert_eq!(format_number("CM-", 7), "CM-7");
        assert_eq!(format_number("", 12345), "12345");
    }

    #[test]
    fn the_engines_next_wins_when_offered() {
        assert_eq!(
            suggest_next(Some("INV-0100"), Some("INV-41")).as_deref(),
            Some("INV-0100")
        );
        assert_eq!(
            suggest_next(Some(" "), Some("INV-41")).as_deref(),
            Some("INV-42")
        );
        assert_eq!(
            suggest_next(None, Some("INV-41")).as_deref(),
            Some("INV-42")
        );
        assert_eq!(suggest_next(None, None), None);
    }

    #[test]
    fn duplicates_are_the_same_text() {
        let used = ["INV-100", "INV-101", " 1002 "];
        assert_eq!(
            check_duplicate(" INV-100", used),
            Some(NumberWarning::Duplicate {
                existing: "INV-100".into()
            })
        );
        assert!(check_duplicate("1002", used).is_some());
        assert_eq!(check_duplicate("inv-100", used), None);
        assert_eq!(check_duplicate("INV-0100", used), None);
        assert_eq!(check_duplicate("  ", used), None);
    }

    #[test]
    fn next_free_skips_numbers_typed_by_hand() {
        let used = ["INV-100", "INV-101", "INV-102"];
        assert_eq!(next_free("INV-99", &used, 10).as_deref(), Some("INV-103"));
        assert_eq!(next_free("INV-99", &used, 2), None);
        assert_eq!(next_free("X", &used, 10), None);
    }
}
