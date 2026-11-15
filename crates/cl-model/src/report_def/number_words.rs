//! English number words (A10, Q-A10l): the AI never states a number, in
//! digits or in words. A draft's heading or text row with a number word
//! ("Bills over five hundred dollars", "Top ten customers") is refused,
//! unless the text is exactly a name from the books (a vendor called "Two
//! Rivers Supply"). The one list of words lives here.
//!
//! The rule and the list are the engine's (`draft-numbers.ts`
//! `NUMBER_WORDS`, `hasNumberWord`; A10 QA, Q-A10j): the text is split into
//! words at anything that is not a letter, and any word on the list counts,
//! hyphenated or not ("one" counts: "One-time customers" is refused; being
//! over-careful is the safe side). "a grand" and "half a grand" count too,
//! but not "Grand total". Ordinals ("First", "Second") and "quarter" are
//! period words and allowed.
//!
//! One difference remains: the engine first folds the text with Unicode
//! NFKC and drops format characters (zero-width spaces), so fullwidth
//! letters ("ｏｎｅ") or a word split by a zero-width space count there and
//! not here. The engine's check runs first on every draft, so such a
//! heading never reaches the desktop.

/// The words that state a quantity: the cardinals, the tens and the scale
/// words, with the plurals the engine lists.
pub const NUMBER_WORDS: [&str; 41] = [
    "zero",
    "one",
    "two",
    "three",
    "four",
    "five",
    "six",
    "seven",
    "eight",
    "nine",
    "ten",
    "eleven",
    "twelve",
    "thirteen",
    "fourteen",
    "fifteen",
    "sixteen",
    "seventeen",
    "eighteen",
    "nineteen",
    "twenty",
    "thirty",
    "forty",
    "fourty",
    "fifty",
    "sixty",
    "seventy",
    "eighty",
    "ninety",
    "hundred",
    "hundreds",
    "thousand",
    "thousands",
    "million",
    "millions",
    "billion",
    "billions",
    "trillion",
    "trillions",
    "dozen",
    "dozens",
];

/// True when `w` (any case) is one of [`NUMBER_WORDS`].
#[must_use]
pub fn is_number_word(w: &str) -> bool {
    let w = w.to_lowercase();
    NUMBER_WORDS.contains(&w.as_str())
}

/// The words of `s` (lowercased) with their byte spans: runs of letters,
/// split at anything else (the engine's `split(/[^\p{L}]+/u)`).
fn words(s: &str) -> Vec<(String, usize, usize)> {
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    for (i, c) in s.char_indices() {
        match (c.is_alphabetic(), start) {
            (true, None) => start = Some(i),
            (false, Some(b)) => {
                out.push((s[b..i].to_lowercase(), b, i));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(b) = start {
        out.push((s[b..].to_lowercase(), b, s.len()));
    }
    out
}

/// Only white space between two words.
fn spaced(s: &str, from: usize, to: usize) -> bool {
    from < to && s[from..to].chars().all(char::is_whitespace)
}

/// "a grand" or "half a grand" (slang for a thousand dollars), not "a
/// grand total".
fn a_grand(s: &str, w: &[(String, usize, usize)]) -> bool {
    w.windows(2).enumerate().any(|(i, p)| {
        let (a, g) = (&p[0], &p[1]);
        a.0 == "a"
            && g.0 == "grand"
            && spaced(s, a.2, g.1)
            && !w
                .get(i + 2)
                .is_some_and(|t| t.0.starts_with("total") && spaced(s, g.2, t.1))
    })
}

/// True when `s` holds a number word (see the module docs).
#[must_use]
pub fn states_a_number(s: &str) -> bool {
    let w = words(s);
    w.iter().any(|(word, _, _)| NUMBER_WORDS.contains(&word.as_str())) || a_grand(s, &w)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantities_in_words_are_found() {
        for s in [
            "Bills over five hundred dollars",
            "Top ten customers",
            "Twenty-five biggest vendors",
            "FIVE HUNDRED",
            "Sales above one thousand",
            "Spend (a dozen vendors)",
            "two",
            "Hundreds of bills",
            "Fourty days",
            "Dozens of vendors",
            "Spend over a grand",
            "Half a grand or more",
        ] {
            assert!(states_a_number(s), "{s}");
        }
    }

    /// As the engine: "one" counts even in a hyphenated word (Q-A10j).
    #[test]
    fn hyphenated_number_words_count_as_the_engine_s() {
        for s in ["One-time customers", "Three-way match", "No-one assigned"] {
            assert!(states_a_number(s), "{s}");
        }
    }

    #[test]
    fn ordinary_words_pass() {
        for s in [
            "Grand total",
            "A grand total",
            "First quarter",
            "Someone's often tonight",
            "Fuel by month",
            "Network costs",
            "Alone at last",
            "",
        ] {
            assert!(!states_a_number(s), "{s}");
        }
    }

    #[test]
    fn the_list_is_lowercase_and_unique() {
        let mut seen = std::collections::BTreeSet::new();
        for w in NUMBER_WORDS {
            assert_eq!(w, w.to_lowercase());
            assert!(seen.insert(w), "{w}");
        }
        assert!(is_number_word("Hundred"));
        assert!(!is_number_word("grand"));
    }
}
