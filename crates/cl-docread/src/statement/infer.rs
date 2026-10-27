//! Columns of a statement table whose header row could not be read (a
//! shaded header band OCR lost on a photo): taken from the dated rows.
//!
//! Rows that start with a date and end in amounts give the date column
//! (their left edge), the text columns (where their text runs start) and
//! the amount columns (shared right edges). The rightmost amount column
//! printed on nearly every row is the running balance. With one other
//! amount column it is a single signed amount column (printed sign kept).
//! With more, a column is a charges column when the balance goes up by its
//! amounts and a payments column when it goes down, decided per column by
//! a clear majority of rows; otherwise nothing is inferred. Amounts are
//! never changed: the result only places the columns.

use crate::dates;
use crate::layout::{Layout, join};
use crate::model::PageWords;
use crate::statement::table::{Band, Col, money_word};
use cl_core::{CalendarDate, Money};

/// Edges this close (x units) are one column.
const GAP: u16 = 350;
/// Dated rows needed to infer anything.
const MIN_ROWS: usize = 3;

struct RowInfo {
    /// Left of the date, the row's anchor.
    date_left: u16,
    /// Starts of the text runs after the date (anchored).
    text_starts: Vec<u16>,
    /// Text runs' first words contain a digit (reference-like).
    text_digits: Vec<bool>,
    /// (anchored right edge, value)
    money: Vec<(u16, Money)>,
}

fn anchored(x: u16, shift: i32) -> u16 {
    u16::try_from((i32::from(x) - shift).clamp(0, 10_000)).unwrap_or(x)
}

fn rows(layout: &Layout, pages: &[PageWords], today: CalendarDate) -> Vec<RowInfo> {
    let mut out = Vec::new();
    let mut base: Option<u16> = None;
    for line in &layout.lines {
        let page = &pages[line.page_index];
        let ws = &line.words;
        let Some(n) = (1..=ws.len().min(4)).rev().find(|&n| {
            dates::parse(&join(ws[..n].iter().map(|&w| &page.words[w])), today).is_some()
        }) else {
            continue;
        };
        let date_left = page.words[ws[0]].bbox.left();
        let b = *base.get_or_insert(date_left);
        let shift = i32::from(date_left) - i32::from(b);
        if shift.abs() > 1_000 {
            continue;
        }
        let mut info = RowInfo {
            date_left,
            text_starts: Vec::new(),
            text_digits: Vec::new(),
            money: Vec::new(),
        };
        let mut prev_right: Option<u16> = Some(page.words[ws[n - 1]].bbox.right());
        for &w in &ws[n..] {
            let word = &page.words[w];
            if let Some((m, cr)) = money_word(&word.text)
                && word.text.chars().any(|c| c == '.')
            {
                let v = if cr {
                    m.abs().checked_neg().unwrap_or(m)
                } else {
                    m
                };
                info.money.push((anchored(word.bbox.right(), shift), v));
                prev_right = None;
                continue;
            }
            let gap_before = prev_right.is_none_or(|r| word.bbox.left().saturating_sub(r) > 250);
            if gap_before {
                info.text_starts.push(anchored(word.bbox.left(), shift));
                info.text_digits
                    .push(word.text.chars().any(|c| c.is_ascii_digit()));
            }
            prev_right = Some(word.bbox.right());
        }
        if !info.money.is_empty() {
            out.push(info);
        }
    }
    out
}

/// Cluster sorted positions; each cluster's (median, count).
fn clusters(mut xs: Vec<u16>) -> Vec<(u16, usize)> {
    xs.sort_unstable();
    let mut out: Vec<Vec<u16>> = Vec::new();
    for x in xs {
        match out.last_mut() {
            Some(c) if c.last().is_some_and(|&l| x - l <= GAP) => c.push(x),
            _ => out.push(vec![x]),
        }
    }
    out.into_iter().map(|c| (c[c.len() / 2], c.len())).collect()
}

fn nearest(cs: &[(u16, usize)], x: u16) -> Option<usize> {
    cs.iter()
        .enumerate()
        .filter(|(_, (c, _))| c.abs_diff(x) <= GAP)
        .min_by_key(|(_, (c, _))| c.abs_diff(x))
        .map(|(i, _)| i)
}

/// The inferred bands, or `None` when the rows don't show them clearly.
#[must_use]
pub fn bands(layout: &Layout, pages: &[PageWords], today: CalendarDate) -> Option<Vec<Band>> {
    let rows = rows(layout, pages, today);
    if rows.len() < MIN_ROWS {
        return None;
    }
    let n = rows.len();
    let enough = |count: usize| count * 4 >= n;
    let date_left = {
        let mut l: Vec<u16> = rows.iter().map(|r| r.date_left).collect();
        l.sort_unstable();
        l[l.len() / 2]
    };
    let money: Vec<(u16, usize)> = clusters(
        rows.iter()
            .flat_map(|r| r.money.iter().map(|m| m.0))
            .collect(),
    )
    .into_iter()
    .filter(|c| enough(c.1))
    .collect();
    if money.is_empty() {
        return None;
    }
    let mut bands = vec![Band {
        col: Col::Date,
        left: date_left,
        right: date_left.saturating_add(300),
    }];
    let first_money_left = money[0].0.saturating_sub(600);
    let texts: Vec<(u16, usize)> = clusters(
        rows.iter()
            .flat_map(|r| r.text_starts.iter().copied())
            .filter(|&x| x < first_money_left)
            .collect(),
    )
    .into_iter()
    .filter(|c| enough(c.1))
    .collect();
    let digits_at = |x: u16| {
        let (mut d, mut all) = (0, 0);
        for r in &rows {
            for (s, has) in r.text_starts.iter().zip(&r.text_digits) {
                if s.abs_diff(x) <= GAP {
                    all += 1;
                    d += usize::from(*has);
                }
            }
        }
        d * 2 > all
    };
    for (k, (x, _)) in texts.iter().take(2).enumerate() {
        let col = if k == 0 && digits_at(*x) {
            Col::Reference
        } else {
            Col::Description
        };
        if bands.iter().any(|b| b.col == col) {
            continue;
        }
        bands.push(Band {
            col,
            left: *x,
            right: x.saturating_add(300),
        });
    }
    // The running balance: rightmost, on nearly every row.
    let last = money.len() - 1;
    let running = money.len() >= 2 && money[last].1 * 10 >= n * 8;
    let amount_cols = if running { &money[..last] } else { &money[..] };
    let kinds: Vec<Col> = match amount_cols.len() {
        0 => return None,
        1 => vec![Col::Amount],
        _ if running => amount_cols
            .iter()
            .enumerate()
            .map(|(ci, _)| vote(&rows, &money, ci, last))
            .collect::<Option<Vec<Col>>>()?,
        _ => return None,
    };
    for ((right, _), col) in amount_cols.iter().zip(kinds) {
        bands.push(Band {
            col,
            left: right.saturating_sub(300),
            right: *right,
        });
    }
    if running {
        bands.push(Band {
            col: Col::Balance,
            left: money[last].0.saturating_sub(300),
            right: money[last].0,
        });
    }
    bands.sort_by_key(|b| b.left);
    Some(bands)
}

/// Whether amount column `ci` moves the balance (column `bi`) up
/// (charges) or down (payments), by a clear majority of rows.
fn vote(rows: &[RowInfo], money: &[(u16, usize)], ci: usize, bi: usize) -> Option<Col> {
    let (mut up, mut down) = (0usize, 0usize);
    let mut prev: Option<Money> = None;
    for r in rows {
        let at = |i: usize| {
            r.money
                .iter()
                .find(|m| nearest(money, m.0) == Some(i))
                .map(|m| m.1)
        };
        let bal = at(bi);
        if let (Some(p), Some(b), Some(v)) = (prev, bal, at(ci)) {
            let d = b.checked_sub(p).ok();
            if d == Some(v.abs()) {
                up += 1;
            } else if d == v.abs().checked_neg().ok() {
                down += 1;
            }
        }
        if bal.is_some() {
            prev = bal;
        }
    }
    if up >= 2 && up >= down * 4 {
        Some(Col::Charges)
    } else if down >= 2 && down >= up * 4 {
        Some(Col::Payments)
    } else {
        None
    }
}
