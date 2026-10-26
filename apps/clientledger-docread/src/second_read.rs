//! The best model's second read of a page (Q-A03b; A03 review S1).
//!
//! A page whose numbers read with low confidence is read again with the
//! more accurate model. The second read is used only when it is at least
//! as good as the first ([`crate::batch::read_score`]) and has at least as
//! many words with digits, and only when every amount of the first read
//! lines up with exactly one word of the second (same pixels, so the boxes
//! overlap by at least half). An amount the two reads give differently
//! keeps the **first** read's text; the other value is recorded and, when
//! the reading uses that amount, becomes a `ReadersDisagree` check naming
//! it. Nothing is guessed: every amount is still one printed word.
//!
//! Never logged: the words are the owner's document.

use cl_core::Money;
use cl_docread::amount;
use cl_docread::model::{
    Bbox, Check, CheckCode, CheckValue, FieldName, Found, MAX_CHECKS, Reading, RuleId,
};
use cl_docread::tsv::{OcrWord, PixelBox};

/// An amount the two reads give differently, at `px` on the cleaned page:
/// `first` was kept.
#[derive(Clone, Copy)]
pub(crate) struct Disagreement<B> {
    pub at: B,
    pub first: Money,
    pub other: Money,
}

/// Words with digits (at least two letters or digits, one a digit).
pub(crate) fn digit_words(words: &[OcrWord]) -> usize {
    words
        .iter()
        .filter(|w| {
            w.text.chars().filter(|c| c.is_alphanumeric()).count() >= 2
                && w.text.chars().any(|c| c.is_ascii_digit())
        })
        .count()
}

fn area(b: PixelBox) -> u64 {
    u64::from(b.width) * u64::from(b.height)
}

fn overlap(a: PixelBox, b: PixelBox) -> u64 {
    let left = a.left.max(b.left);
    let top = a.top.max(b.top);
    let right = a
        .left
        .saturating_add(a.width)
        .min(b.left.saturating_add(b.width));
    let bottom = a
        .top
        .saturating_add(a.height)
        .min(b.top.saturating_add(b.height));
    u64::from(right.saturating_sub(left)) * u64::from(bottom.saturating_sub(top))
}

/// The words to keep: `second` under the rule above, else `first`.
pub(crate) fn merge(
    first: Vec<OcrWord>,
    mut second: Vec<OcrWord>,
) -> (Vec<OcrWord>, Vec<Disagreement<PixelBox>>) {
    if second.is_empty()
        || crate::batch::read_score(&second) < crate::batch::read_score(&first)
        || digit_words(&second) < digit_words(&first)
    {
        return (first, Vec::new());
    }
    let mut used = vec![false; second.len()];
    let mut found = Vec::new();
    for f in &first {
        let Some(fv) = amount::parse(&f.text) else {
            continue;
        };
        let hits: Vec<usize> = (0..second.len())
            .filter(|&i| overlap(f.px, second[i].px) > 0)
            .collect();
        let [i] = hits.as_slice() else {
            return (first, Vec::new());
        };
        let i = *i;
        let shared = overlap(f.px, second[i].px);
        if used[i] || shared * 2 < area(f.px).max(area(second[i].px)) {
            return (first, Vec::new());
        }
        used[i] = true;
        let s = &mut second[i];
        let sv = amount::parse(&s.text);
        if sv == Some(fv) {
            continue;
        }
        if let Some(other) = sv {
            found.push(Disagreement {
                at: f.px,
                first: fv,
                other,
            });
        }
        s.text.clone_from(&f.text);
        s.confidence = f.confidence;
    }
    (second, found)
}

/// One `ReadersDisagree` check per disagreement the reading uses (an
/// amount whose spot holds the word, on its page, with the first read's
/// value), within [`MAX_CHECKS`].
pub(crate) fn add_checks(reading: &mut Reading, found: &[(u16, Disagreement<Bbox>)]) {
    for (page, d) in found {
        let holds = |f: &Found<Money>| {
            f.spot.page == *page
                && f.value == d.first
                && f.spot.bbox.left() <= d.at.left()
                && f.spot.bbox.top() <= d.at.top()
                && f.spot.bbox.right() >= d.at.right()
                && f.spot.bbox.bottom() >= d.at.bottom()
        };
        let h = &reading.header;
        let header = [
            (FieldName::Subtotal, h.subtotal.as_ref()),
            (FieldName::Tax, h.tax.as_ref()),
            (FieldName::Total, h.total.as_ref()),
            (FieldName::AmountPaid, h.amount_paid.as_ref()),
            (FieldName::BalanceDue, h.balance_due.as_ref()),
        ]
        .into_iter()
        .filter_map(|(n, f)| Some((Some(n), None, f?)))
        .chain(h.fees.iter().map(|f| (Some(FieldName::Fees), None, f)));
        let lines = reading.lines.iter().enumerate().flat_map(|(i, l)| {
            let i = u16::try_from(i).ok();
            std::iter::once((None, i, &l.amount))
                .chain(l.unit_price.iter().map(move |p| (None, i, p)))
        });
        let uses: Vec<(Option<FieldName>, Option<u16>, cl_docread::model::Spot)> = header
            .chain(lines)
            .filter(|(_, _, f)| holds(f))
            .map(|(n, i, f)| (n, i, f.spot))
            .collect();
        for (field, line, spot) in uses {
            if reading.checks.len() >= MAX_CHECKS {
                return;
            }
            reading.checks.push(Check {
                code: CheckCode::ReadersDisagree,
                rule: RuleId::new("ocr.second-read"),
                field,
                line,
                expected: Some(CheckValue::Money(d.other)),
                found: Some(CheckValue::Money(d.first)),
                difference: d.first.checked_sub(d.other).ok(),
                spot: Some(spot),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cl_docread::model::{Confidence, Spot};

    fn w(text: &str, left: u32, conf: u16) -> OcrWord {
        OcrWord {
            text: text.into(),
            px: PixelBox {
                left,
                top: 100,
                width: 80,
                height: 20,
            },
            confidence: Confidence::saturating(u32::from(conf)),
            block: 1,
            paragraph: 1,
            line: 1,
        }
    }

    fn m(s: &str) -> Money {
        Money::parse_wire(s).unwrap()
    }

    fn texts(words: &[OcrWord]) -> Vec<&str> {
        words.iter().map(|w| w.text.as_str()).collect()
    }

    #[test]
    fn a_better_read_is_kept_but_a_first_amount_stands() {
        let first = vec![
            w("Invoice", 0, 9_600),
            w("1O42", 100, 6_000),
            w("12.40", 200, 7_000),
            w("Total", 300, 9_600),
        ];
        let second = vec![
            w("Invoice", 0, 9_600),
            w("1042", 100, 9_500),
            w("12.48", 200, 9_400),
            w("Total", 300, 9_600),
        ];
        let (kept, found) = merge(first, second);
        // The non-amount word improves; the amount keeps the first read.
        assert_eq!(texts(&kept), ["Invoice", "1042", "12.40", "Total"]);
        assert_eq!(kept[2].confidence.bp(), 7_000);
        assert_eq!(found.len(), 1);
        assert_eq!((found[0].first, found[0].other), (m("12.40"), m("12.48")));
    }

    #[test]
    fn a_worse_read_or_fewer_numbers_is_not_used() {
        let first = vec![w("Total", 0, 9_000), w("12.40", 100, 7_000)];
        // Fewer words with digits.
        let (kept, _) = merge(
            first.clone(),
            vec![w("Total", 0, 9_900), w("Tota", 100, 9_900)],
        );
        assert_eq!(texts(&kept), ["Total", "12.40"]);
        // Fewer confident words.
        let (kept, _) = merge(
            first.clone(),
            vec![w("Total", 0, 1_000), w("12.40", 100, 9_900)],
        );
        assert_eq!(kept[0].confidence.bp(), 9_000);
        // An amount that doesn't line up with one word: the first read.
        let mut split = w("12", 100, 9_900);
        split.px.width = 30;
        let mut rest = w(".40", 140, 9_900);
        rest.px.width = 40;
        let (kept, found) = merge(
            first,
            vec![w("Total", 0, 9_900), split, rest, w("x9", 500, 9_900)],
        );
        assert_eq!(texts(&kept), ["Total", "12.40"]);
        assert!(found.is_empty());
    }

    #[test]
    fn a_used_amount_gets_a_check_naming_the_other_read() {
        let bbox = Bbox::new(100, 100, 200, 140).unwrap();
        let spot = Spot { page: 0, bbox };
        let mut r = Reading::empty(cl_docread::model::Reader::Ocr, 1);
        r.header.total = Some(Found {
            value: m("12.40"),
            spot,
            rule: RuleId::new("t"),
            confidence: Confidence::HIGH,
        });
        let d = Disagreement {
            at: bbox,
            first: m("12.40"),
            other: m("12.48"),
        };
        // Elsewhere on the page: not used, no check.
        let away = Disagreement {
            at: Bbox::new(500, 500, 600, 540).unwrap(),
            ..d
        };
        add_checks(&mut r, &[(0, d), (0, away), (1, d)]);
        assert_eq!(r.checks.len(), 1);
        let c = &r.checks[0];
        assert_eq!(c.code, CheckCode::ReadersDisagree);
        assert_eq!(c.field, Some(FieldName::Total));
        assert!(matches!(c.expected, Some(CheckValue::Money(v)) if v == m("12.48")));
        assert_eq!(c.difference, Some(m("-0.08")));
    }
}
