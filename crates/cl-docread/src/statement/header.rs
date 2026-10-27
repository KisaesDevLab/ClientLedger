//! A statement's header (A06 step 2): statement date, period from/to,
//! opening balance ("Previous balance", "Balance forward", …) and closing
//! balance ("Closing balance", "Amount due", "Total due", …). A value is
//! taken right of its label on the same line, else from the line below
//! under the label (a boxed summary row). Lines inside the line table are
//! skipped; vendor and account number come from the bill header rules.

use crate::dates;
use crate::fields::label_tokens;
use crate::layout::{Layout, join, union};
use crate::model::{Bbox, Confidence, Found, PageWords, RuleId, Spot};
use crate::statement::table::money_word;
use cl_core::{CalendarDate, Money};

/// A value right of its label.
const RIGHT: u16 = 9_000;
/// A value under its label.
const BELOW: u16 = 8_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum What {
    Opening,
    Closing(u8),
    StatementDate(u8),
    Period,
    /// A label that looks like one of ours but is not ("Due date").
    Ignore,
}

const LABELS: &[(&[&str], What)] = &[
    (&["previous", "balance"], What::Opening),
    (&["previous", "statement", "balance"], What::Opening),
    (&["balance", "forward"], What::Opening),
    (&["balance", "brought", "forward"], What::Opening),
    (&["prior", "balance"], What::Opening),
    (&["beginning", "balance"], What::Opening),
    (&["opening", "balance"], What::Opening),
    (&["starting", "balance"], What::Opening),
    (&["last", "statement", "balance"], What::Opening),
    (&["closing", "balance"], What::Closing(0)),
    (&["ending", "balance"], What::Closing(0)),
    (&["new", "balance"], What::Closing(0)),
    (&["statement", "balance"], What::Closing(0)),
    (&["total", "due"], What::Closing(1)),
    (&["total", "amount", "due"], What::Closing(1)),
    (&["total", "balance", "due"], What::Closing(1)),
    (&["amount", "due"], What::Closing(1)),
    (&["balance", "due"], What::Closing(1)),
    (&["please", "pay"], What::Closing(2)),
    (&["pay", "this", "amount"], What::Closing(2)),
    (&["statement", "date"], What::StatementDate(0)),
    (&["closing", "date"], What::StatementDate(1)),
    (&["date"], What::StatementDate(2)),
    (&["due", "date"], What::Ignore),
    (&["payment", "due", "date"], What::Ignore),
    (&["invoice", "date"], What::Ignore),
    (&["statement", "period"], What::Period),
    (&["billing", "period"], What::Period),
    (&["period"], What::Period),
    (&["for", "the", "period"], What::Period),
    (&["activity", "from"], What::Period),
    (&["period", "from"], What::Period),
    (&["from"], What::Period),
    (&["activity", "for"], What::Period),
];

/// What the header step found.
#[derive(Debug, Clone, Default)]
pub struct Head {
    pub statement_date: Option<Found<CalendarDate>>,
    pub period_from: Option<Found<CalendarDate>>,
    pub period_to: Option<Found<CalendarDate>>,
    pub opening: Option<Found<Money>>,
    pub closing: Option<Found<Money>>,
}

/// One label on one line: its kind, the words of the label and the
/// line's words after it.
struct Hit {
    what: What,
    li: usize,
    /// Word indices (into the page) of the label.
    label: Vec<usize>,
    /// Word indices after the label on the line.
    after: Vec<usize>,
}

fn hits(layout: &Layout, pages: &[PageWords], li: usize) -> Vec<Hit> {
    let line = &layout.lines[li];
    let page = &pages[line.page_index];
    let words: Vec<&crate::model::Word> = line.words.iter().map(|&w| &page.words[w]).collect();
    let tokens = label_tokens(words.iter().copied());
    let mut out: Vec<Hit> = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        let best = LABELS
            .iter()
            .filter(|(ws, _)| {
                i + ws.len() <= tokens.len()
                    && ws.iter().zip(&tokens[i..]).all(|(w, t)| *w == t.text)
            })
            .max_by_key(|(ws, _)| ws.len());
        if let Some((ws, what)) = best {
            let first = tokens[i].word;
            let last = tokens[i + ws.len() - 1].word;
            out.push(Hit {
                what: *what,
                li,
                label: line.words[first..=last].to_vec(),
                after: line.words[last + 1..].to_vec(),
            });
            i += ws.len();
        } else {
            i += 1;
        }
    }
    // Each label's value stops at the next label on the line.
    for k in 0..out.len() {
        if let Some(next_first) = out.get(k + 1).and_then(|n| n.label.first().copied()) {
            let cut = out[k]
                .after
                .iter()
                .position(|&w| w == next_first)
                .unwrap_or(out[k].after.len());
            out[k].after.truncate(cut);
        }
    }
    out
}

fn conf(page: &PageWords, words: &[usize], place: u16) -> Confidence {
    let bp = words
        .iter()
        .map(|&w| page.words[w].confidence.bp())
        .min()
        .unwrap_or(0)
        .min(place);
    Confidence::saturating(u32::from(bp))
}

fn spot(page: &PageWords, words: &[usize]) -> Option<Spot> {
    Some(Spot {
        page: page.page,
        bbox: union(words.iter().map(|&w| page.words[w].bbox))?,
    })
}

/// The first amount among `words`.
fn amount_in(
    page: &PageWords,
    words: &[usize],
    place: u16,
    rule: &'static str,
) -> Option<Found<Money>> {
    words.iter().find_map(|&w| {
        let (m, cr) = money_word(&page.words[w].text)?;
        // "1,204.66 CR" on a statement is a credit balance.
        let next_cr = words
            .iter()
            .skip_while(|&&x| x != w)
            .nth(1)
            .is_some_and(|&n| page.words[n].text.trim() == "CR");
        let value = if cr || next_cr {
            m.abs().checked_neg().unwrap_or(m)
        } else {
            m
        };
        Some(Found {
            value,
            spot: spot(page, &[w])?,
            rule: RuleId::new(rule),
            confidence: conf(page, &[w], place),
        })
    })
}

/// The first date at the start of `words` (up to four words).
fn date_in(
    page: &PageWords,
    words: &[usize],
    today: CalendarDate,
    place: u16,
    rule: &'static str,
) -> Option<(Found<CalendarDate>, usize)> {
    for start in 0..words.len().min(2) {
        for n in (1..=(words.len() - start).min(4)).rev() {
            let ws = &words[start..start + n];
            let text = join(ws.iter().map(|&w| &page.words[w]));
            if let Some(d) = dates::parse(&text, today) {
                let bp = conf(page, ws, place).bp().min(d.confidence.bp());
                return Some((
                    Found {
                        value: d.date,
                        spot: spot(page, ws)?,
                        rule: RuleId::new(rule),
                        confidence: Confidence::saturating(u32::from(bp)),
                    },
                    start + n,
                ));
            }
        }
    }
    None
}

fn is_range_word(t: &str) -> bool {
    matches!(
        t.trim().trim_end_matches(':').to_lowercase().as_str(),
        "-" | "–" | "—" | "to" | "through" | "thru" | "until"
    )
}

/// "09/01/2026 - 09/30/2026", "Sep 1 to Sep 30, 2026", "From 9/1/26 To
/// 9/30/26": two dates with a range word between (the first may borrow
/// the second's year).
fn period_in(
    page: &PageWords,
    words: &[usize],
    today: CalendarDate,
    place: u16,
) -> Option<(Found<CalendarDate>, Found<CalendarDate>)> {
    // Split at a range word between two dates.
    let at = words
        .iter()
        .position(|&w| is_range_word(&page.words[w].text))?;
    let left = &words[..at];
    let right: Vec<usize> = words[at + 1..].to_vec();
    let (to, _) = date_in(page, &right, today, place, "statement.period.to")?;
    let from = date_in(page, left, today, place, "statement.period.from")
        .map(|(f, _)| f)
        .or_else(|| {
            // "Sep 1 – Sep 30, 2026": the year is printed once.
            let start = left.len().saturating_sub(3);
            let text = join(left[start..].iter().map(|&w| &page.words[w]));
            let with_year = format!("{text}, {}", to.value.year());
            let d = dates::parse(&with_year, today)?;
            Some(Found {
                value: d.date,
                spot: spot(page, &left[start..])?,
                rule: RuleId::new("statement.period.from"),
                confidence: conf(page, &left[start..], place.min(BELOW)),
            })
        })?;
    (from.value <= to.value).then_some((from, to))
}

/// The words of the line below `li` under the label's extent (a boxed
/// summary row: labels on one line, values on the next).
fn below(layout: &Layout, li: usize, label: Bbox, pages: &[PageWords]) -> Vec<usize> {
    let line = &layout.lines[li];
    let page = &pages[line.page_index];
    let next = (0..layout.lines.len())
        .filter(|&k| {
            let l = &layout.lines[k];
            l.page_index == line.page_index && l.bbox.top() > line.bbox.top()
        })
        .min_by_key(|&k| layout.lines[k].bbox.top());
    let Some(k) = next else { return Vec::new() };
    let n = &layout.lines[k];
    if n.bbox.top()
        > line
            .bbox
            .bottom()
            .saturating_add(line.bbox.height().saturating_mul(3))
    {
        return Vec::new();
    }
    let lo = label.left().saturating_sub(400);
    let hi = label.right().saturating_add(400);
    n.words
        .iter()
        .copied()
        .filter(|&w| {
            let b = page.words[w].bbox;
            b.right() >= lo && b.left() <= hi
        })
        .collect()
}

/// Read the header from the lines outside the table.
#[must_use]
pub fn read(layout: &Layout, pages: &[PageWords], in_table: &[bool], today: CalendarDate) -> Head {
    let mut out = Head::default();
    let mut closing_prio = u8::MAX;
    let mut date_prio = u8::MAX;
    for li in 0..layout.lines.len() {
        if in_table.get(li).copied().unwrap_or(false) {
            continue;
        }
        let page = &pages[layout.lines[li].page_index];
        for h in hits(layout, pages, li) {
            let Some(label_box) = union(h.label.iter().map(|&w| page.words[w].bbox)) else {
                continue;
            };
            let under = || below(layout, h.li, label_box, pages);
            match h.what {
                What::Opening if out.opening.is_none() => {
                    out.opening = amount_in(page, &h.after, RIGHT, "statement.opening.right")
                        .or_else(|| amount_in(page, &under(), BELOW, "statement.opening.below"));
                }
                What::Closing(p) if p < closing_prio => {
                    let v = amount_in(page, &h.after, RIGHT, "statement.closing.right")
                        .or_else(|| amount_in(page, &under(), BELOW, "statement.closing.below"));
                    if v.is_some() {
                        out.closing = v;
                        closing_prio = p;
                    }
                }
                What::StatementDate(p) if p < date_prio => {
                    // "Date" alone only takes a value right of it.
                    let v = date_in(page, &h.after, today, RIGHT, "statement.date.right")
                        .map(|(d, _)| d)
                        .or_else(|| {
                            (p < 2)
                                .then(|| {
                                    date_in(page, &under(), today, BELOW, "statement.date.below")
                                        .map(|(d, _)| d)
                                })
                                .flatten()
                        });
                    if v.is_some() {
                        out.statement_date = v;
                        date_prio = p;
                    }
                }
                What::Period if out.period_from.is_none() => {
                    let v = period_in(page, &h.after, today, RIGHT)
                        .or_else(|| period_in(page, &under(), today, BELOW));
                    if let Some((from, to)) = v {
                        out.period_from = Some(from);
                        out.period_to = Some(to);
                    }
                }
                _ => {}
            }
        }
    }
    out
}
