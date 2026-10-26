//! Final values (A04 D5, Q-A03q): the reading as the owner accepted it,
//! for template learning (`templates::learn`) after a successful convert.
//!
//! Learning stores **where** values are printed, never the values. So
//! [`to_final`] keeps a field's spot only when it is sure the owner's final
//! value is printed there:
//!
//! * accepted as read ([`Entry::Read`]) and equal to the read value: the
//!   read spot (the vendor: accepting the read vendor is enough, as the
//!   owner's contact name is often spelled differently from the print);
//! * filled by clicking a word ([`Entry::Clicked`]) and still equal to the
//!   clicked value: the clicked spot;
//! * typed ([`Entry::Typed`]), or changed after a read or a click: the
//!   **single** page word that parses equal (amounts through
//!   [`amount::parse`], dates through [`dates::parse`], references through
//!   [`normalize_reference`]); when no word or more than one does, no
//!   region (never a joined pair of words, M4).
//!
//! Fields the forms don't show follow the field they belong with: the
//! vendor's address, phone and account number only when the vendor's spot
//! is kept from the read; subtotal, tax, fees, amount paid and balance due
//! only when the total was accepted as read; terms only when the due date
//! was. Line items are kept only when the final line amounts equal the read
//! ones, in order. Checks, the payment (card digits) and the template index
//! are dropped. Words, pages and the fingerprint are kept.
//!
//! Pure: no I/O, no clock (the caller passes "today" for two-digit years).

use crate::amount;
use crate::dates;
use crate::dupes::normalize_reference;
use crate::model::{Confidence, Found, PageWords, Reading, RuleId, Spot};
use crate::vendor_match::normalize_loose;
use cl_core::{CalendarDate, Money};
use std::fmt;

/// How the owner's final value got into its field.
#[derive(Clone, PartialEq, Eq)]
pub enum Entry<T> {
    /// The read value, accepted.
    Read,
    /// Filled by clicking the word at `spot`, which gave `filled`.
    Clicked { spot: Spot, filled: T },
    /// Typed (or picked from a list).
    Typed,
}

impl<T> fmt::Debug for Entry<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Entry::Read => f.write_str("Read"),
            Entry::Clicked { spot, .. } => f.debug_struct("Clicked").field("spot", spot).finish(),
            Entry::Typed => f.write_str("Typed"),
        }
    }
}

/// One field's final value and how it was entered.
#[derive(Clone, PartialEq, Eq)]
pub struct Final<T> {
    pub value: T,
    pub entry: Entry<T>,
}

impl<T> fmt::Debug for Final<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Final")
            .field("entry", &self.entry)
            .finish_non_exhaustive()
    }
}

/// The values the owner accepted (the convert body's), by field.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct FinalValues {
    /// The vendor or payee's name as entered.
    pub vendor: Option<Final<String>>,
    pub date: Option<Final<CalendarDate>>,
    pub due_date: Option<Final<CalendarDate>>,
    /// Bill number, reference or check number.
    pub reference: Option<Final<String>>,
    pub total: Option<Final<Money>>,
    /// The final line amounts in order, when the form has lines.
    pub line_amounts: Option<Vec<Money>>,
}

impl fmt::Debug for FinalValues {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FinalValues")
            .field("vendor", &self.vendor)
            .field("date", &self.date)
            .field("due_date", &self.due_date)
            .field("reference", &self.reference)
            .field("total", &self.total)
            .field("lines", &self.line_amounts.as_ref().map(Vec::len))
            .finish()
    }
}

/// Rule ids of the final values.
pub mod rule {
    use crate::model::RuleId;
    /// Accepted as read.
    pub const READ: RuleId = RuleId::new("owner.read");
    /// Filled by a click.
    pub const CLICKED: RuleId = RuleId::new("owner.clicked");
    /// Typed; the one page word that parses equal.
    pub const TYPED: RuleId = RuleId::new("owner.typed");
}

/// The reading as accepted (see the module comment).
#[must_use]
pub fn to_final(read: &Reading, fin: &FinalValues, today: CalendarDate) -> Reading {
    let h = &read.header;
    let words = &read.words;
    let mut out = read.clone();
    out.checks.clear();
    out.template = None;
    let o = &mut out.header;
    o.payment = None;

    // Vendor: the read spot when accepted as read.
    let vendor_from_read = fin
        .vendor
        .as_ref()
        .is_some_and(|v| matches!(v.entry, Entry::Read) && h.vendor.is_some());
    o.vendor = fin.vendor.as_ref().and_then(|v| {
        if vendor_from_read {
            return h.vendor.as_ref().map(|r| kept(v.value.clone(), r.spot));
        }
        settle(
            v,
            None,
            |a, b| a == b,
            |w| {
                let want = normalize_loose(&v.value);
                !want.is_empty() && normalize_loose(w) == want
            },
            words,
        )
    });
    if !vendor_from_read {
        o.vendor_address = None;
        o.vendor_phone = None;
        o.account_number = None;
    }

    o.date = fin.date.as_ref().and_then(|d| {
        settle(
            d,
            h.date.as_ref(),
            |a, b| a == b,
            |w| dates::parse(w, today).is_some_and(|p| p.date == d.value),
            words,
        )
    });

    let due_from_read = fin
        .due_date
        .as_ref()
        .is_some_and(|d| read_equal(d, h.due_date.as_ref()));
    o.due_date = fin.due_date.as_ref().and_then(|d| {
        settle(
            d,
            h.due_date.as_ref(),
            |a, b| a == b,
            |w| dates::parse(w, today).is_some_and(|p| p.date == d.value),
            words,
        )
    });
    if !due_from_read {
        o.terms = None;
    }

    o.reference = fin.reference.as_ref().and_then(|r| {
        let want = normalize_reference(&r.value);
        settle(
            r,
            h.reference.as_ref(),
            |a, b| normalize_reference(a) == normalize_reference(b),
            |w| !want.is_empty() && normalize_reference(w) == want,
            words,
        )
    });

    let total_from_read = fin
        .total
        .as_ref()
        .is_some_and(|t| read_equal(t, h.total.as_ref()));
    o.total = fin.total.as_ref().and_then(|t| {
        settle(
            t,
            h.total.as_ref(),
            |a, b| a == b,
            |w| amount::parse(w) == Some(t.value),
            words,
        )
    });
    if !total_from_read {
        o.subtotal = None;
        o.tax = None;
        o.fees.clear();
        o.amount_paid = None;
        o.balance_due = None;
    }

    let lines_equal = fin.line_amounts.as_ref().is_some_and(|amounts| {
        amounts.len() == read.lines.len()
            && amounts
                .iter()
                .zip(&read.lines)
                .all(|(a, l)| *a == l.amount.value)
    });
    if !lines_equal {
        out.lines.clear();
    }
    out
}

/// Whether `f` was accepted as read and equals the read value.
fn read_equal<T: PartialEq>(f: &Final<T>, read: Option<&Found<T>>) -> bool {
    matches!(f.entry, Entry::Read) && read.is_some_and(|r| r.value == f.value)
}

fn kept<T>(value: T, spot: Spot) -> Found<T> {
    Found {
        value,
        spot,
        rule: rule::READ,
        confidence: Confidence::CERTAIN,
    }
}

/// One field's final spot: the read one when accepted and equal, the
/// clicked one when still equal, else the single word `is_word` accepts.
fn settle<T: Clone>(
    f: &Final<T>,
    read: Option<&Found<T>>,
    same: impl Fn(&T, &T) -> bool,
    is_word: impl Fn(&str) -> bool,
    words: &[PageWords],
) -> Option<Found<T>> {
    let at = |spot: Spot, rule: RuleId| Found {
        value: f.value.clone(),
        spot,
        rule,
        confidence: Confidence::CERTAIN,
    };
    match &f.entry {
        Entry::Read => {
            if let Some(r) = read
                && same(&r.value, &f.value)
            {
                return Some(at(r.spot, rule::READ));
            }
        }
        Entry::Clicked { spot, filled } if same(filled, &f.value) => {
            return Some(at(*spot, rule::CLICKED));
        }
        Entry::Clicked { .. } | Entry::Typed => {}
    }
    single_word(words, is_word).map(|spot| at(spot, rule::TYPED))
}

/// The spot of the only word on any page `is_word` accepts.
fn single_word(words: &[PageWords], is_word: impl Fn(&str) -> bool) -> Option<Spot> {
    let mut found = None;
    for p in words {
        for w in &p.words {
            if is_word(&w.text) {
                if found.is_some() {
                    return None;
                }
                found = Some(Spot {
                    page: p.page,
                    bbox: w.bbox,
                });
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Bbox, Line, Reader, Word, WordSource};
    use crate::templates;

    fn money(s: &str) -> Money {
        Money::parse_wire(s).unwrap()
    }

    fn today() -> CalendarDate {
        CalendarDate::from_ymd(2026, 10, 26).unwrap()
    }

    fn bbox(l: u16, t: u16) -> Bbox {
        Bbox::new(l, t, l + 800, t + 120).unwrap()
    }

    fn word(text: &str, l: u16, t: u16) -> Word {
        Word {
            text: text.into(),
            bbox: bbox(l, t),
            confidence: Confidence::CERTAIN,
        }
    }

    fn found<T>(value: T, l: u16, t: u16) -> Found<T> {
        Found {
            value,
            spot: Spot {
                page: 0,
                bbox: bbox(l, t),
            },
            rule: RuleId::new("t"),
            confidence: Confidence::HIGH,
        }
    }

    /// A read with a wrong total (the rules took the subtotal) and words
    /// for the real one.
    fn reading() -> Reading {
        let mut r = Reading::empty(Reader::TextPdf, 1);
        r.words.push(PageWords {
            page: 0,
            aspect: 12_941,
            source: WordSource::TextLayer,
            words: vec![
                word("Riverbend", 500, 500),
                word("INV-0042", 7000, 1000),
                word("03/14/2026", 7000, 1200),
                word("Subtotal", 6000, 8000),
                word("500.00", 8000, 8000),
                word("Total", 6000, 8400),
                word("544.82", 8000, 8400),
                word("12.50", 8000, 6000),
                word("12.50", 8000, 6200),
            ],
        });
        r.fingerprint = Some(crate::model::Fingerprint {
            tokens: vec!["invoice".into(), "total".into()],
            aspect: 12_941,
        });
        let h = &mut r.header;
        h.vendor = Some(found("RIVERBEND".to_string(), 500, 500));
        h.vendor_phone = Some(found("555-0100".to_string(), 500, 700));
        h.reference = Some(found("INV-0042".to_string(), 7000, 1000));
        h.date = Some(found(
            CalendarDate::from_ymd(2026, 3, 14).unwrap(),
            7000,
            1200,
        ));
        h.subtotal = Some(found(money("500.00"), 8000, 8000));
        h.total = Some(found(money("500.00"), 8000, 8000));
        r.lines.push(Line {
            description: None,
            quantity: None,
            unit_price: None,
            amount: found(money("500.00"), 8000, 5000),
        });
        r
    }

    // Reads as the optional field it fills.
    #[allow(clippy::unnecessary_wraps)]
    fn fin<T>(value: T, entry: Entry<T>) -> Option<Final<T>> {
        Some(Final { value, entry })
    }

    #[test]
    fn read_values_keep_their_spots_and_a_typed_total_finds_its_word() {
        let r = reading();
        let f = FinalValues {
            vendor: fin("Riverbend Supply Co.".to_string(), Entry::Read),
            date: fin(CalendarDate::from_ymd(2026, 3, 14).unwrap(), Entry::Read),
            reference: fin("inv 42".to_string(), Entry::Read),
            total: fin(money("544.82"), Entry::Typed),
            ..FinalValues::default()
        };
        let out = to_final(&r, &f, today());
        let h = &out.header;
        assert_eq!(h.vendor.as_ref().unwrap().spot.bbox, bbox(500, 500));
        assert_eq!(h.vendor.as_ref().unwrap().value, "Riverbend Supply Co.");
        assert!(h.vendor_phone.is_some(), "kept with the read vendor");
        assert_eq!(h.reference.as_ref().unwrap().spot.bbox, bbox(7000, 1000));
        assert_eq!(h.date.as_ref().unwrap().rule, rule::READ);
        let total = h.total.as_ref().unwrap();
        assert_eq!(total.value, money("544.82"));
        assert_eq!(total.spot.bbox, bbox(8000, 8400));
        assert_eq!(total.rule, rule::TYPED);
        assert!(h.subtotal.is_none(), "the total was corrected");
        assert!(out.lines.is_empty(), "no final lines given");
        assert!(out.checks.is_empty() && out.template.is_none());
        // Learns a template with the corrected total's region.
        let t = templates::learn(&out).unwrap();
        assert!(
            t.fields
                .iter()
                .any(|tf| tf.field == crate::model::FieldName::Total
                    && tf.region == bbox(8000, 8400))
        );
    }

    #[test]
    fn a_typed_value_on_two_words_or_none_gets_no_region() {
        let r = reading();
        let f = FinalValues {
            total: fin(money("12.50"), Entry::Typed),
            date: fin(CalendarDate::from_ymd(2026, 3, 15).unwrap(), Entry::Typed),
            ..FinalValues::default()
        };
        let out = to_final(&r, &f, today());
        assert!(out.header.total.is_none(), "two words read 12.50");
        assert!(out.header.date.is_none(), "no word reads that date");
        assert!(out.header.vendor.is_none() && out.header.vendor_phone.is_none());
        // A read value the owner changed is treated as typed.
        let f = FinalValues {
            total: fin(money("544.82"), Entry::Read),
            ..FinalValues::default()
        };
        let out = to_final(&r, &f, today());
        assert_eq!(out.header.total.as_ref().unwrap().rule, rule::TYPED);
    }

    #[test]
    fn a_clicked_value_keeps_the_clicked_spot_only_while_unchanged() {
        let r = reading();
        let spot = Spot {
            page: 0,
            bbox: bbox(8000, 6000),
        };
        let f = FinalValues {
            total: fin(
                money("12.5"),
                Entry::Clicked {
                    spot,
                    filled: money("12.50"),
                },
            ),
            ..FinalValues::default()
        };
        let out = to_final(&r, &f, today());
        let t = out.header.total.as_ref().unwrap();
        assert_eq!((t.spot, t.rule.clone()), (spot, rule::CLICKED));
        let f = FinalValues {
            total: fin(
                money("13.00"),
                Entry::Clicked {
                    spot,
                    filled: money("12.50"),
                },
            ),
            ..FinalValues::default()
        };
        assert!(to_final(&r, &f, today()).header.total.is_none());
    }

    #[test]
    fn lines_and_totals_block_stay_when_accepted() {
        let r = reading();
        let f = FinalValues {
            total: fin(money("500.00"), Entry::Read),
            line_amounts: Some(vec![money("500")]),
            ..FinalValues::default()
        };
        let out = to_final(&r, &f, today());
        assert_eq!(out.lines.len(), 1);
        assert!(out.header.subtotal.is_some());
        let f = FinalValues {
            line_amounts: Some(vec![money("499.99")]),
            ..f
        };
        assert_eq!(to_final(&r, &f, today()).lines.len(), 0);
    }

    #[test]
    fn debug_prints_no_values() {
        let f = FinalValues {
            vendor: fin("Riverbend".to_string(), Entry::Typed),
            total: fin(
                money("544.82"),
                Entry::Clicked {
                    spot: Spot {
                        page: 0,
                        bbox: Bbox::PAGE,
                    },
                    filled: money("544.82"),
                },
            ),
            ..FinalValues::default()
        };
        let text = format!("{f:?}");
        assert!(
            !text.contains("Riverbend") && !text.contains("544"),
            "{text}"
        );
    }
}
