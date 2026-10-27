//! A statement's line table (A06 step 3): the header row sets the column
//! bands (date, reference, description, one amount column or charges /
//! credits / payments, running balance); a page without a header row
//! carries the bands on from the page before. A row is a line when it has
//! a date in the date band and an amount in an amount band.
//!
//! The sign comes from the column, never from the words on the line: a
//! single amount column keeps the printed sign ("CR", a trailing minus and
//! parentheses mean negative); a charges column keeps what is printed; a
//! credits or payments column is always negative.
//!
//! Balance forward, subtotals, totals and aging rows are left out of the
//! lines (Q-296) and listed in `left_out`.

use crate::amount;
use crate::dates;
use crate::fields::label_tokens;
use crate::layout::{Layout, join, union};
use crate::model::{
    Bbox, Confidence, Found, LeftOut, LeftOutKind, MAX_TEXT_CHARS, PageWords, RuleId, SignSource,
    Spot, StatementLineRead, Word,
};
use cl_core::{CalendarDate, Money};

/// The confidence ceiling of a value read from a statement column.
const COLUMN: u16 = 9_000;
/// A text word starts in a column when it starts at most this far left of
/// the column's header (x units).
const SLACK: u16 = 150;
/// An amount belongs to a money column when its right edge is this close
/// to the header's right edge (x units)…
const RIGHT_TOL: u16 = 450;
/// …or its centre lies within the header widened by this much.
const CENTRE_TOL: u16 = 150;

/// What a statement column holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Col {
    Date,
    Reference,
    Description,
    /// One signed amount column.
    Amount,
    Charges,
    Credits,
    Payments,
    /// The running balance.
    Balance,
    Other,
}

impl Col {
    fn is_money(self) -> bool {
        matches!(
            self,
            Col::Amount | Col::Charges | Col::Credits | Col::Payments | Col::Balance
        )
    }

    fn sign(self) -> Option<SignSource> {
        match self {
            Col::Amount => Some(SignSource::SingleColumn),
            Col::Charges => Some(SignSource::ChargesColumn),
            Col::Credits => Some(SignSource::CreditsColumn),
            Col::Payments => Some(SignSource::PaymentsColumn),
            _ => None,
        }
    }
}

/// One column: its kind and its header's extent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Band {
    pub col: Col,
    pub left: u16,
    pub right: u16,
}

const COLUMN_WORDS: &[(&[&str], Col)] = &[
    (&["date"], Col::Date),
    (&["txn", "date"], Col::Date),
    (&["trans", "date"], Col::Date),
    (&["transaction", "date"], Col::Date),
    (&["invoice", "date"], Col::Date),
    (&["inv", "date"], Col::Date),
    (&["doc", "date"], Col::Date),
    (&["posted"], Col::Date),
    (&["post", "date"], Col::Date),
    (&["posting", "date"], Col::Date),
    (&["due", "date"], Col::Other),
    (&["due"], Col::Other),
    (&["ref"], Col::Reference),
    (&["ref", "#"], Col::Reference),
    (&["reference"], Col::Reference),
    (&["reference", "#"], Col::Reference),
    (&["invoice"], Col::Reference),
    (&["invoice", "#"], Col::Reference),
    (&["inv", "#"], Col::Reference),
    (&["document"], Col::Reference),
    (&["document", "#"], Col::Reference),
    (&["doc", "#"], Col::Reference),
    (&["transaction", "#"], Col::Reference),
    (&["#"], Col::Reference),
    (&["description"], Col::Description),
    (&["desc"], Col::Description),
    (&["details"], Col::Description),
    (&["transaction"], Col::Description),
    (&["activity"], Col::Description),
    (&["memo"], Col::Description),
    (&["particulars"], Col::Description),
    (&["type"], Col::Other),
    (&["po", "#"], Col::Other),
    (&["amount"], Col::Amount),
    (&["amt"], Col::Amount),
    (&["charges"], Col::Charges),
    (&["charge"], Col::Charges),
    (&["debits"], Col::Charges),
    (&["debit"], Col::Charges),
    (&["invoices"], Col::Charges),
    (&["invoice", "amount"], Col::Charges),
    (&["billed"], Col::Charges),
    (&["new", "charges"], Col::Charges),
    (&["credits"], Col::Credits),
    (&["credit"], Col::Credits),
    (&["credit", "amount"], Col::Credits),
    (&["payments"], Col::Payments),
    (&["payment"], Col::Payments),
    (&["paid"], Col::Payments),
    (&["payments", "received"], Col::Payments),
    (&["balance"], Col::Balance),
    (&["running", "balance"], Col::Balance),
    (&["bal"], Col::Balance),
];

/// The header row's bands, or `None` when the line is not a statement
/// table header: a date column, an amount column (single, charges,
/// credits or payments), three columns or more, and most of the line made
/// of column words.
#[must_use]
pub fn header_bands(words: &[&Word]) -> Option<Vec<Band>> {
    let tokens = label_tokens(words.iter().copied());
    let mut bands: Vec<Band> = Vec::new();
    let mut covered = 0;
    let mut i = 0;
    // An unknown word just before a column word belongs to its header
    // ("Pro #", "Trans #"): the column starts there.
    let mut unknown_left: Option<u16> = None;
    while i < tokens.len() {
        let best = COLUMN_WORDS
            .iter()
            .filter(|(ws, _)| {
                i + ws.len() <= tokens.len()
                    && ws.iter().zip(&tokens[i..]).all(|(w, t)| *w == t.text)
            })
            .max_by_key(|(ws, _)| ws.len());
        if let Some((ws, col)) = best {
            let bbox = union(tokens[i..i + ws.len()].iter().map(|t| words[t.word].bbox))?;
            let left = match unknown_left {
                Some(l) if bbox.left().saturating_sub(l) <= 1_000 => l,
                _ => bbox.left(),
            };
            bands.push(Band {
                col: *col,
                left,
                right: bbox.right(),
            });
            covered += ws.len();
            i += ws.len();
            unknown_left = None;
        } else {
            let left = words[tokens[i].word].bbox.left();
            unknown_left = Some(unknown_left.map_or(left, |l| l.min(left)));
            i += 1;
        }
    }
    let has = |c: Col| bands.iter().any(|b| b.col == c);
    let priced = has(Col::Amount) || has(Col::Charges) || has(Col::Credits) || has(Col::Payments);
    if bands.len() < 3 || !has(Col::Date) || !priced || covered * 10 < tokens.len() * 6 {
        return None;
    }
    // Only the first date column is the line's date.
    let mut seen_date = false;
    for b in &mut bands {
        if b.col == Col::Date {
            if seen_date {
                b.col = Col::Other;
            }
            seen_date = true;
        }
    }
    bands.sort_by_key(|b| b.left);
    Some(bands)
}

/// A money word as a statement prints it: what [`amount::parse`] reads,
/// or that followed by `CR` or a minus (a credit). Returns the value and
/// whether it was marked as a credit.
#[must_use]
pub fn money_word(text: &str) -> Option<(Money, bool)> {
    if let Some(m) = amount::parse(text) {
        return Some((m, false));
    }
    let t = text.trim();
    let stem = t
        .strip_suffix("CR")
        .or_else(|| t.strip_suffix("cr"))
        .or_else(|| t.strip_suffix("Cr"))
        .or_else(|| t.strip_suffix('-'))?;
    amount::parse(stem.trim_end()).map(|m| (m, true))
}

fn is_cr(text: &str) -> bool {
    matches!(text.trim().trim_end_matches('.'), "CR" | "cr" | "Cr")
}

/// What a table row is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    Line(Box<StatementLineRead>),
    Left(LeftOut, Option<Found<Money>>),
    /// More description for the line above.
    Wrap(Found<String>),
    Nothing,
}

/// A row's words sorted into the bands.
#[derive(Default)]
struct Parts {
    date: Vec<usize>,
    reference: Vec<usize>,
    description: Vec<usize>,
    other: Vec<usize>,
    /// (band index, value, credit marked, word)
    money: Vec<(usize, Money, bool, usize)>,
}

/// The band a text word starting at `left` belongs to: the last band that
/// starts at or before it (with [`SLACK`]), else the first.
fn text_band(bands: &[Band], left: u16) -> usize {
    bands
        .iter()
        .rposition(|b| b.left <= left.saturating_add(SLACK))
        .unwrap_or(0)
}

/// The money band an amount word lines up with, if any.
fn money_band(bands: &[Band], b: Bbox) -> Option<usize> {
    let centre = u32::midpoint(u32::from(b.left()), u32::from(b.right()));
    bands
        .iter()
        .enumerate()
        .filter(|(_, band)| band.col.is_money())
        .filter(|(_, band)| {
            b.right().abs_diff(band.right) <= RIGHT_TOL
                || (u32::from(band.left.saturating_sub(CENTRE_TOL))
                    ..=u32::from(band.right.saturating_add(CENTRE_TOL)))
                    .contains(&centre)
        })
        .min_by_key(|(_, band)| b.right().abs_diff(band.right))
        .map(|(i, _)| i)
}

fn split(page: &PageWords, words: &[usize], bands: &[Band]) -> Parts {
    let mut p = Parts::default();
    for (k, &w) in words.iter().enumerate() {
        let word = &page.words[w];
        let text = word.text.trim();
        if text.is_empty() || text == "$" {
            continue;
        }
        if is_cr(text) {
            // "125.00 CR": marks the amount just before it.
            if let Some(last) = p.money.last_mut()
                && k > 0
                && words[k - 1] == last.3
            {
                last.2 = true;
            }
            continue;
        }
        if let Some((m, cr)) = money_word(text)
            && let Some(bi) = money_band(bands, word.bbox)
        {
            p.money.push((bi, m, cr, w));
            continue;
        }
        let bi = text_band(bands, word.bbox.left());
        match bands[bi].col {
            Col::Date => p.date.push(w),
            Col::Reference => p.reference.push(w),
            Col::Description => p.description.push(w),
            // Text under a money header (a "$" on its own, a note).
            _ => p.other.push(w),
        }
    }
    p
}

fn word_found<T>(page: &PageWords, w: usize, value: T, rule: &'static str) -> Found<T> {
    let word = &page.words[w];
    Found {
        value,
        spot: Spot {
            page: page.page,
            bbox: word.bbox,
        },
        rule: RuleId::new(rule),
        confidence: Confidence::saturating(u32::from(word.confidence.bp().min(COLUMN))),
    }
}

fn words_found(page: &PageWords, words: &[usize], rule: &'static str) -> Option<Found<String>> {
    let bbox = union(words.iter().map(|&w| page.words[w].bbox))?;
    let text: String = join(words.iter().map(|&w| &page.words[w]))
        .chars()
        .take(MAX_TEXT_CHARS)
        .collect();
    let bp = words
        .iter()
        .map(|&w| page.words[w].confidence.bp())
        .min()
        .unwrap_or(0)
        .min(COLUMN);
    Some(Found {
        value: text,
        spot: Spot {
            page: page.page,
            bbox,
        },
        rule: RuleId::new(rule),
        confidence: Confidence::saturating(u32::from(bp)),
    })
}

/// The date at the start of the date band's words (up to four words).
fn row_date(page: &PageWords, words: &[usize], today: CalendarDate) -> Option<Found<CalendarDate>> {
    for n in (1..=words.len().min(4)).rev() {
        let ws = &words[..n];
        let text = join(ws.iter().map(|&w| &page.words[w]));
        if let Some(d) = dates::parse(&text, today) {
            let bbox = union(ws.iter().map(|&w| page.words[w].bbox))?;
            let bp = ws
                .iter()
                .map(|&w| page.words[w].confidence.bp())
                .min()
                .unwrap_or(0)
                .min(d.confidence.bp())
                .min(COLUMN);
            return Some(Found {
                value: d.date,
                spot: Spot {
                    page: page.page,
                    bbox,
                },
                rule: RuleId::new("statement.line.date"),
                confidence: Confidence::saturating(u32::from(bp)),
            });
        }
    }
    None
}

/// Words that mark a row left out of the lines, matched at the start of
/// the row's text (after the date).
const LEFT_OUT: &[(&[&str], LeftOutKind)] = &[
    (&["balance", "forward"], LeftOutKind::BalanceForward),
    (
        &["balance", "brought", "forward"],
        LeftOutKind::BalanceForward,
    ),
    (&["brought", "forward"], LeftOutKind::BalanceForward),
    (&["previous", "balance"], LeftOutKind::BalanceForward),
    (&["prior", "balance"], LeftOutKind::BalanceForward),
    (&["beginning", "balance"], LeftOutKind::BalanceForward),
    (&["opening", "balance"], LeftOutKind::BalanceForward),
    (&["starting", "balance"], LeftOutKind::BalanceForward),
    (
        &["last", "statement", "balance"],
        LeftOutKind::BalanceForward,
    ),
    (&["subtotal"], LeftOutKind::Subtotal),
    (&["page", "total"], LeftOutKind::Subtotal),
    (&["page", "subtotal"], LeftOutKind::Subtotal),
    (&["carried", "forward"], LeftOutKind::Subtotal),
    (&["balance", "carried", "forward"], LeftOutKind::Subtotal),
    (&["total"], LeftOutKind::Total),
    (&["totals"], LeftOutKind::Total),
    (&["statement", "total"], LeftOutKind::Total),
    (&["closing", "balance"], LeftOutKind::Total),
    (&["ending", "balance"], LeftOutKind::Total),
    (&["new", "balance"], LeftOutKind::Total),
    (&["amount", "due"], LeftOutKind::Total),
    (&["total", "due"], LeftOutKind::Total),
    (&["balance", "due"], LeftOutKind::Total),
    (&["total", "amount", "due"], LeftOutKind::Total),
    (&["please", "pay"], LeftOutKind::Total),
    (&["statement", "balance"], LeftOutKind::Total),
    (&["current"], LeftOutKind::Aging),
];

/// Aging header words: "Current  1-30  31-60  61-90  Over 90".
fn is_aging_header(texts: &[String]) -> bool {
    let has = |w: &str| texts.iter().any(|t| t == w);
    let buckets = texts
        .iter()
        .filter(|t| {
            matches!(
                t.as_str(),
                "1-30" | "31-60" | "61-90" | "30" | "60" | "90" | "120" | "91+" | "90+" | "over"
            ) || t.ends_with("days")
        })
        .count();
    (has("current") && buckets >= 1) || buckets >= 3
}

fn left_out_kind(texts: &[String]) -> Option<LeftOutKind> {
    if is_aging_header(texts) {
        return Some(LeftOutKind::Aging);
    }
    // At the start, or after one word ("Page 2 total", "09/01 Previous
    // balance" with the date already taken out).
    for start in 0..texts.len().min(3) {
        let best = LEFT_OUT
            .iter()
            .filter(|(ws, _)| {
                start + ws.len() <= texts.len()
                    && ws.iter().zip(&texts[start..]).all(|(w, t)| *w == t)
            })
            .max_by_key(|(ws, _)| ws.len());
        if let Some((_, kind)) = best {
            // "Current" alone is an aging word only on an aging row.
            if *kind == LeftOutKind::Aging {
                continue;
            }
            return Some(*kind);
        }
    }
    None
}

/// Read one row with the page's bands.
#[must_use]
pub fn read_row(
    page: &PageWords,
    words: &[usize],
    row_box: Bbox,
    bands: &[Band],
    today: CalendarDate,
) -> Row {
    let p = split(page, words, bands);
    let date = row_date(page, &p.date, today);
    // The row's text after the date, for the left-out words.
    let date_words = date.as_ref().map_or(0, |d| {
        p.date
            .iter()
            .take_while(|&&w| d.spot.bbox.overlaps(page.words[w].bbox))
            .count()
    });
    let mut text_words: Vec<usize> = p.date[date_words..]
        .iter()
        .chain(&p.reference)
        .chain(&p.description)
        .chain(&p.other)
        .copied()
        .collect();
    text_words.sort_by_key(|&w| page.words[w].bbox.left());
    let texts: Vec<String> = label_tokens(text_words.iter().map(|&w| &page.words[w]))
        .into_iter()
        .map(|t| t.text)
        .collect();
    let spot = Spot {
        page: page.page,
        bbox: row_box,
    };
    if let Some(what) = left_out_kind(&texts) {
        // The row's printed amount: its balance, else its last amount.
        let value = p
            .money
            .iter()
            .rev()
            .find(|m| bands[m.0].col == Col::Balance)
            .or_else(|| p.money.last())
            .map(|&(_, m, cr, w)| {
                let v = if cr {
                    m.abs().checked_neg().unwrap_or(m)
                } else {
                    m
                };
                word_found(page, w, v, "statement.left-out")
            });
        return Row::Left(LeftOut { what, spot }, value);
    }
    let amount = p
        .money
        .iter()
        .filter(|m| bands[m.0].col != Col::Balance)
        .find(|m| !m.1.is_zero())
        .or_else(|| p.money.iter().find(|m| bands[m.0].col != Col::Balance));
    let running = p
        .money
        .iter()
        .find(|m| bands[m.0].col == Col::Balance)
        .map(|&(_, m, cr, w)| {
            let v = if cr {
                m.abs().checked_neg().unwrap_or(m)
            } else {
                m
            };
            word_found(page, w, v, "statement.line.running")
        });
    match (date, amount) {
        (Some(date), Some(&(bi, m, cr, w))) => {
            let Some(sign_from) = bands[bi].col.sign() else {
                return Row::Nothing;
            };
            let value = match sign_from {
                SignSource::SingleColumn if cr => m.abs().checked_neg().unwrap_or(m),
                SignSource::SingleColumn | SignSource::ChargesColumn => m,
                SignSource::CreditsColumn | SignSource::PaymentsColumn => {
                    m.abs().checked_neg().unwrap_or(m)
                }
            };
            let reference =
                words_found(page, &p.reference, "statement.line.reference").map(|mut f| {
                    f.value = crate::fields::clean_id(&f.value);
                    f
                });
            Row::Line(Box::new(StatementLineRead {
                date,
                reference: reference.filter(|r| !r.value.is_empty()),
                description: words_found(page, &p.description, "statement.line.description"),
                amount: word_found(page, w, value, "statement.line.amount"),
                sign_from,
                running,
            }))
        }
        (None, None) if !p.description.is_empty() && p.reference.is_empty() => {
            match words_found(page, &p.description, "statement.line.description") {
                Some(f) => Row::Wrap(f),
                None => Row::Nothing,
            }
        }
        _ => Row::Nothing,
    }
}

/// What the table step found.
#[derive(Debug, Clone, Default)]
pub struct Table {
    pub lines: Vec<StatementLineRead>,
    pub left_out: Vec<LeftOut>,
    /// The balance-forward row's amount, the first one.
    pub forward: Option<Found<Money>>,
    /// The last total row's amount.
    pub total: Option<Found<Money>>,
    /// Layout lines inside the table (header, rows, left-out rows).
    pub in_table: Vec<bool>,
    /// Whether a header had a balance, charges, credits or payments column.
    pub statement_columns: bool,
    /// Lines found past the cap and dropped.
    pub capped: bool,
}

fn line_words<'a>(layout: &Layout, pages: &'a [PageWords], li: usize) -> Vec<&'a Word> {
    let l = &layout.lines[li];
    l.words
        .iter()
        .map(|&w| &pages[l.page_index].words[w])
        .collect()
}

/// Read every page's table.
#[must_use]
pub fn read(layout: &Layout, pages: &[PageWords], today: CalendarDate) -> Table {
    let (t, found_header) = read_with(layout, pages, today, None);
    if found_header {
        return t;
    }
    // No header row read (OCR lost a shaded band): columns from the
    // dated rows themselves.
    match crate::statement::infer::bands(layout, pages, today) {
        Some(b) => {
            let (mut t, _) = read_with(layout, pages, today, Some(b));
            t.statement_columns = false;
            t
        }
        None => t,
    }
}

fn read_with(
    layout: &Layout,
    pages: &[PageWords],
    today: CalendarDate,
    initial: Option<Vec<Band>>,
) -> (Table, bool) {
    let mut t = Table {
        in_table: vec![false; layout.lines.len()],
        ..Table::default()
    };
    let mut found_header = false;
    let mut bands: Option<Vec<Band>> = initial;
    for pi in 0..pages.len() {
        let page = &pages[pi];
        let mut lines: Vec<usize> = (0..layout.lines.len())
            .filter(|&li| layout.lines[li].page_index == pi)
            .collect();
        lines.sort_by_key(|&li| layout.lines[li].bbox.top());
        let header = lines
            .iter()
            .position(|&li| header_bands(&line_words(layout, pages, li)).is_some());
        let start = match header {
            Some(k) => {
                let li = lines[k];
                bands = header_bands(&line_words(layout, pages, li));
                found_header = true;
                t.in_table[li] = true;
                k + 1
            }
            // A page without a header row carries the bands on.
            None => 0,
        };
        let Some(b) = bands.as_ref() else { continue };
        t.statement_columns |= b.iter().any(|band| {
            matches!(
                band.col,
                Col::Balance | Col::Charges | Col::Credits | Col::Payments
            )
        });
        let mut ended = false;
        let mut last_line_box: Option<Bbox> = None;
        let mut dx: i32 = 0;
        for &li in &lines[start..] {
            let row = &layout.lines[li];
            // A turned or skewed page drifts sideways down the page: each
            // dated row re-anchors the bands on its date.
            if let Some(shift) = date_shift(page, &row.words, b, today) {
                dx = shift;
            }
            let shifted = shift_bands(b, dx);
            let r = read_row(page, &row.words, row.bbox, &shifted, today);
            match r {
                Row::Left(lo, value) => {
                    t.in_table[li] = true;
                    match lo.what {
                        LeftOutKind::BalanceForward if t.forward.is_none() => {
                            t.forward = value;
                        }
                        LeftOutKind::Total if !ended => {
                            t.total = value;
                            ended = true;
                        }
                        _ => {}
                    }
                    if t.left_out.len() < crate::model::MAX_STATEMENT_LEFT_OUT {
                        t.left_out.push(lo);
                    }
                    last_line_box = None;
                }
                Row::Line(line) if !ended => {
                    t.in_table[li] = true;
                    if t.lines.len() < crate::model::MAX_STATEMENT_LINES {
                        t.lines.push(*line);
                    } else {
                        t.capped = true;
                    }
                    last_line_box = Some(row.bbox);
                }
                Row::Wrap(more) if !ended => {
                    let close = last_line_box.is_some_and(|p| {
                        row.bbox.top()
                            <= p.bottom()
                                .saturating_add(p.height().max(row.bbox.height()) * 3 / 2)
                    });
                    if close && let Some(prev) = t.lines.last_mut() {
                        t.in_table[li] = true;
                        match prev.description.as_mut() {
                            Some(d) => {
                                let mut text = d.value.clone();
                                text.push(' ');
                                text.push_str(&more.value);
                                d.value = text.chars().take(MAX_TEXT_CHARS).collect();
                                d.spot.bbox = d.spot.bbox.union(more.spot.bbox);
                            }
                            None => prev.description = Some(more),
                        }
                        last_line_box = Some(row.bbox);
                    } else {
                        last_line_box = None;
                    }
                }
                _ => last_line_box = None,
            }
        }
    }
    (t, found_header)
}

/// How far a dated row's date sits from the date column's header (x
/// units), when the row starts with a date near it.
fn date_shift(
    page: &PageWords,
    words: &[usize],
    bands: &[Band],
    today: CalendarDate,
) -> Option<i32> {
    let date = bands.iter().find(|b| b.col == Col::Date)?;
    let first = &page.words[*words.first()?];
    let shift = i32::from(first.bbox.left()) - i32::from(date.left);
    if shift.abs() > 1_000 {
        return None;
    }
    (1..=words.len().min(4))
        .any(|n| dates::parse(&join(words[..n].iter().map(|&w| &page.words[w])), today).is_some())
        .then_some(shift)
}

/// The bands moved `dx` sideways (kept on the page).
fn shift_bands(bands: &[Band], dx: i32) -> Vec<Band> {
    let mv =
        |x: u16| u16::try_from((i32::from(x) + dx).clamp(0, i32::from(Bbox::SCALE))).unwrap_or(x);
    bands
        .iter()
        .map(|b| Band {
            col: b.col,
            left: mv(b.left),
            right: mv(b.right),
        })
        .collect()
}
