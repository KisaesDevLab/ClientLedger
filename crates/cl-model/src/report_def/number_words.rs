//! English number words (A10, Q-A10l): the AI never states a number, in
//! digits or in words. A draft's heading or text row that uses a number
//! word as a quantity ("Bills over five hundred dollars", "Top ten
//! customers") is refused, unless the text is exactly a name from the books
//! (a vendor called "Two Rivers Supply"). The one list of words lives here.
//!
//! A number word joined by a hyphen to a word that is not a number word is
//! part of an ordinary word, not a quantity: "One-time customers",
//! "Three-way match", "No-one". Ordinals ("First", "Second") and "grand"
//! ("Grand total") are not in the list.

/// The words that state a quantity: the cardinals, the tens and the scale
/// words.
pub const NUMBER_WORDS: [&str; 34] = [
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
    "fifty",
    "sixty",
    "seventy",
    "eighty",
    "ninety",
    "hundred",
    "thousand",
    "million",
    "billion",
    "trillion",
    "dozen",
];

/// True when `w` (any case) is one of [`NUMBER_WORDS`].
#[must_use]
pub fn is_number_word(w: &str) -> bool {
    let w = w.to_lowercase();
    NUMBER_WORDS.contains(&w.as_str())
}

/// The words of `s` with what joins each to the next: `(word, hyphen
/// after)`. A word is a run of letters (and apostrophes inside it).
fn words(s: &str) -> Vec<(&str, bool)> {
    let mut out: Vec<(&str, bool)> = Vec::new();
    let mut start: Option<usize> = None;
    let mut last_end = 0;
    for (i, c) in s.char_indices() {
        let part = c.is_alphabetic() || (c == '\'' && start.is_some());
        match (part, start) {
            (true, None) => {
                // A hyphen right before this word joins it to the last one.
                start = Some(i);
            }
            (false, Some(b)) => {
                out.push((&s[b..i], false));
                start = None;
                last_end = i;
            }
            _ => {}
        }
        if c == '-'
            && start.is_none()
            && last_end == i
            && let Some(prev) = out.last_mut()
        {
            prev.1 = true;
        }
    }
    if let Some(b) = start {
        out.push((&s[b..], false));
    }
    out
}

/// True when `s` uses a number word as a quantity (see the module docs).
#[must_use]
pub fn states_a_number(s: &str) -> bool {
    let w = words(s);
    w.iter().enumerate().any(|(i, (word, joined_next))| {
        if !is_number_word(word) {
            return false;
        }
        // Joined to a word that isn't a number word: part of an ordinary
        // word ("one-time", "no-one").
        let next_plain = *joined_next && w.get(i + 1).is_some_and(|(n, _)| !is_number_word(n));
        let prev_plain = i > 0 && w[i - 1].1 && !is_number_word(w[i - 1].0);
        !(next_plain || prev_plain)
    })
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
        ] {
            assert!(states_a_number(s), "{s}");
        }
    }

    #[test]
    fn ordinary_words_pass() {
        for s in [
            "One-time customers",
            "Three-way match",
            "No-one assigned",
            "Grand total",
            "First quarter",
            "Someone's often tonight",
            "Fuel by month",
            "Network costs",
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
