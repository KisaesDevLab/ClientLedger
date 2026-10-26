//! Click to fill (A04 D4, M4): the words the owner picked on the page, or
//! the words a dragged box holds, turned into one field value. Words come
//! in by text only (`overlay.rs` did the hit-testing and handed over word
//! indices); nothing here knows where a word sat.
//!
//! - **Money** (M4): each word is parsed alone through the one document
//!   amount parser, [`cl_docread::amount::parse`]; two words are never
//!   joined into one number. A pick fills only when exactly one word holds
//!   a digit and that word parses; the other words may be a currency sign
//!   or a label ("$", "Total:"), never a sign, a bracket or a separator
//!   that could change the number ("-", "(", ","). Anything else is "That
//!   isn't one amount" and nothing changes.
//! - **Dates** may join words ("Mar 14, 2026"), only through
//!   [`cl_docread::dates::parse`].
//! - **Text** joins the words with single spaces.
//!
//! No floats, no geometry (`cargo xtask nofloat` scans this file).

use cl_core::{CalendarDate, Money};
use cl_docread::model::{MAX_TEXT_CHARS, Word};

use super::{FillKind, PageBox};

/// A value a pick fills. `Debug` never shows the value (owner data).
#[derive(Clone, PartialEq, Eq)]
pub enum FillValue {
    /// One amount, exactly as printed.
    Money(Money),
    /// A date.
    Date(CalendarDate),
    /// Text, words joined with single spaces.
    Text(String),
}

impl std::fmt::Debug for FillValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            FillValue::Money(_) => "Money(..)",
            FillValue::Date(_) => "Date(..)",
            FillValue::Text(_) => "Text(..)",
        })
    }
}

/// Why a pick filled nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FillRefusal {
    /// No words were picked (an empty box, a page with no words).
    NoWords,
    /// Not exactly one amount ("That isn't one amount").
    NotOneAmount,
    /// The words don't read as a date.
    NotADate,
    /// The words are longer than a field takes.
    TooLong,
}

impl FillRefusal {
    /// Stable name (control channel, logs).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            FillRefusal::NoWords => "no_words",
            FillRefusal::NotOneAmount => "not_one_amount",
            FillRefusal::NotADate => "not_a_date",
            FillRefusal::TooLong => "too_long",
        }
    }

    /// The i18n key of the words shown.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            FillRefusal::NoWords => "compare.refused.no_words",
            FillRefusal::NotOneAmount => "compare.refused.not_one_amount",
            FillRefusal::NotADate => "compare.refused.not_a_date",
            FillRefusal::TooLong => "compare.refused.too_long",
        }
    }
}

/// A currency sign printed apart from its number.
fn is_currency_sign(word: &str) -> bool {
    matches!(word.to_ascii_uppercase().as_str(), "$" | "US$" | "USD")
}

/// A word that could change a number printed next to it: a sign, a
/// bracket or a separator (`-`, `(`, `1,`).
fn could_change_a_number(word: &str) -> bool {
    word.chars()
        .any(|c| c.is_ascii_digit() || matches!(c, '-' | '\u{2212}' | '(' | ')' | ',' | '.'))
}

/// The one amount in `words` (see the module comment), parsed alone.
///
/// # Errors
/// [`FillRefusal::NoWords`] for no words, else
/// [`FillRefusal::NotOneAmount`].
pub fn money(words: &[&str]) -> Result<Money, FillRefusal> {
    if words.is_empty() {
        return Err(FillRefusal::NoWords);
    }
    let mut number = None;
    for w in words {
        let w = w.trim();
        if w.chars().any(|c| c.is_ascii_digit()) {
            if number.is_some() {
                // Two words with digits: never joined (M4).
                return Err(FillRefusal::NotOneAmount);
            }
            number = Some(w);
        } else if !is_currency_sign(w) && could_change_a_number(w) {
            return Err(FillRefusal::NotOneAmount);
        }
    }
    number
        .and_then(cl_docread::amount::parse)
        .ok_or(FillRefusal::NotOneAmount)
}

/// Joins `words` with single spaces (each word's own spaces folded).
fn joined(words: &[&str]) -> String {
    words
        .iter()
        .flat_map(|w| w.split_whitespace())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The value `words` fill a field of `kind` with (`today` reads two-digit
/// years and dates without a year).
///
/// # Errors
/// A [`FillRefusal`]; nothing is filled then.
pub fn fill(kind: FillKind, words: &[&str], today: CalendarDate) -> Result<FillValue, FillRefusal> {
    if words.iter().all(|w| w.trim().is_empty()) {
        return Err(FillRefusal::NoWords);
    }
    match kind {
        FillKind::Money => money(words).map(FillValue::Money),
        FillKind::Date => {
            let text = joined(words);
            cl_docread::dates::parse(&text, today)
                .map(|d| FillValue::Date(d.date))
                .ok_or(FillRefusal::NotADate)
        }
        FillKind::Text => {
            let text = joined(words);
            if text.chars().count() > MAX_TEXT_CHARS {
                Err(FillRefusal::TooLong)
            } else {
                Ok(FillValue::Text(text))
            }
        }
    }
}

/// The smallest box holding `words` (all on `page`), for the field's new
/// spot; `None` for no words.
#[must_use]
pub fn spot_of(page: u16, words: &[&Word]) -> Option<PageBox> {
    let first = words.first()?;
    let b = words
        .iter()
        .skip(1)
        .fold(first.bbox, |acc, w| acc.union(w.bbox));
    PageBox::new(page, b.left(), b.top(), b.right(), b.bottom())
}

/// The words of `page_words` whose centre lies inside `area`, in reading
/// order: what a box dragged over a text page picks (integer arithmetic).
#[must_use]
pub fn words_in(area: PageBox, page_words: &[Word]) -> Vec<usize> {
    let inside = |lo: u16, hi: u16, a: u16, b: u16| {
        let c = u32::from(a) + u32::from(b);
        c >= 2 * u32::from(lo) && c <= 2 * u32::from(hi)
    };
    page_words
        .iter()
        .enumerate()
        .filter(|(_, w)| {
            inside(area.l, area.r, w.bbox.left(), w.bbox.right())
                && inside(area.t, area.b, w.bbox.top(), w.bbox.bottom())
        })
        .map(|(i, _)| i)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use cl_docread::model::{Bbox, Confidence};

    fn today() -> CalendarDate {
        CalendarDate::from_ymd(2026, 10, 26).unwrap()
    }

    fn m(s: &str) -> Money {
        Money::parse_wire(s).unwrap()
    }

    /// T10 (M4): the cases the review named.
    #[test]
    fn two_numbers_are_never_joined() {
        assert_eq!(money(&["2", "12.50"]), Err(FillRefusal::NotOneAmount));
        assert_eq!(money(&["1", "250.00"]), Err(FillRefusal::NotOneAmount));
        assert_eq!(money(&["1,", "250.00"]), Err(FillRefusal::NotOneAmount));
        assert_eq!(money(&["$", "12.50"]), Ok(m("12.50")));
        assert_eq!(money(&["USD", "12.50"]), Ok(m("12.50")));
        assert_eq!(money(&["(12.50)"]), Ok(m("-12.50")));
        assert_eq!(money(&["Total:", "12.50"]), Ok(m("12.50")));
        assert_eq!(money(&["12.50"]), Ok(m("12.50")));
        assert_eq!(money(&["$1,234.56"]), Ok(m("1234.56")));
    }

    #[test]
    fn signs_and_brackets_beside_a_number_refuse() {
        assert_eq!(money(&["-", "12.50"]), Err(FillRefusal::NotOneAmount));
        assert_eq!(money(&["(", "12.50", ")"]), Err(FillRefusal::NotOneAmount));
        assert_eq!(money(&["12,50"]), Err(FillRefusal::NotOneAmount));
        assert_eq!(
            money(&["1.23456"]),
            Err(FillRefusal::NotOneAmount),
            "never rounded"
        );
        assert_eq!(money(&["Total"]), Err(FillRefusal::NotOneAmount));
        assert_eq!(money(&["$"]), Err(FillRefusal::NotOneAmount));
        assert_eq!(money(&[]), Err(FillRefusal::NoWords));
    }

    /// Property (seeded loop, as A02's clamp test): whatever words are
    /// picked, a money fill is one picked word's own parse, or nothing.
    #[test]
    fn a_money_fill_is_one_words_parse_or_nothing() {
        const POOL: [&str; 16] = [
            "$", "12.50", "2", "1,", "250.00", "(12.50)", "-", "Total:", "USD", "0.99", "1,234.56",
            "Due", "12,50", ".", "3", "x9",
        ];
        let mut seed: u64 = 0x5eed_c0de;
        for _ in 0..20_000 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let n = usize::try_from(seed % 4).unwrap_or(0) + 1;
            let picked: Vec<&str> = (0..n)
                .map(|i| {
                    let k = (seed >> (8 * i)) % 16;
                    POOL[usize::try_from(k).unwrap_or(0)]
                })
                .collect();
            if let Ok(v) = money(&picked) {
                assert!(
                    picked
                        .iter()
                        .any(|w| cl_docread::amount::parse(w) == Some(v)),
                    "{picked:?}"
                );
                let digit_words = picked
                    .iter()
                    .filter(|w| w.chars().any(|c| c.is_ascii_digit()))
                    .count();
                assert_eq!(digit_words, 1, "{picked:?}");
            }
        }
    }

    #[test]
    fn dates_join_words_and_text_joins_with_single_spaces() {
        assert_eq!(
            fill(FillKind::Date, &["Mar", "14,", "2026"], today()),
            Ok(FillValue::Date(
                CalendarDate::from_ymd(2026, 3, 14).unwrap()
            ))
        );
        assert_eq!(
            fill(FillKind::Date, &["3/14/2026"], today()),
            Ok(FillValue::Date(
                CalendarDate::from_ymd(2026, 3, 14).unwrap()
            ))
        );
        assert_eq!(
            fill(FillKind::Date, &["Total"], today()),
            Err(FillRefusal::NotADate)
        );
        assert_eq!(
            fill(FillKind::Text, &["Ozark ", " Ridge", "Supply"], today()),
            Ok(FillValue::Text("Ozark Ridge Supply".into()))
        );
        assert_eq!(
            fill(FillKind::Text, &[" "], today()),
            Err(FillRefusal::NoWords)
        );
        let long = "x".repeat(MAX_TEXT_CHARS + 1);
        assert_eq!(
            fill(FillKind::Text, &[long.as_str()], today()),
            Err(FillRefusal::TooLong)
        );
        assert_eq!(
            fill(FillKind::Money, &["$", "12.50"], today()),
            Ok(FillValue::Money(m("12.50")))
        );
        assert_eq!(
            format!("{:?}", FillValue::Text("secret".into())),
            "Text(..)"
        );
        assert_eq!(FillRefusal::NotOneAmount.name(), "not_one_amount");
    }

    fn word(l: u16, t: u16, r: u16, b: u16) -> Word {
        Word {
            text: "w".into(),
            bbox: Bbox::new(l, t, r, b).unwrap(),
            confidence: Confidence::CERTAIN,
        }
    }

    #[test]
    fn a_box_picks_the_words_whose_centre_is_inside() {
        let words = vec![
            word(100, 100, 300, 200),
            word(400, 100, 600, 200),
            word(100, 900, 300, 1000),
        ];
        let area = PageBox::new(0, 50, 50, 500, 300).unwrap();
        assert_eq!(
            words_in(area, &words),
            vec![0, 1],
            "centre 500 is on the edge"
        );
        let area = PageBox::new(0, 50, 50, 499, 300).unwrap();
        assert_eq!(words_in(area, &words), vec![0]);
        let picked: Vec<&Word> = words.iter().take(2).collect();
        assert_eq!(spot_of(2, &picked), PageBox::new(2, 100, 100, 600, 200));
        assert_eq!(spot_of(0, &[]), None);
    }
}
