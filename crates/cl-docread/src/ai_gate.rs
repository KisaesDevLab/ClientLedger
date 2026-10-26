//! The AI amount gate (WP-A, step 11; plan D5 "AI amount gate", S18,
//! Q-A03j, non-negotiable 4). Runs in the desktop **before any
//! `DocReading` exists**: an AI amount, quantity or unit price, header or
//! line, is kept only when a page word parses to an equal value, and then
//! takes that word's spot; otherwise it is reported as an `AiDisagrees`
//! check and never used. Other AI values stay suggestions the owner must
//! accept. An Ollama answer that agrees raises confidence; one that
//! disagrees adds a check.
//!
//! The rules' own values always win: an optional reader fills only what
//! the rules left empty, and only from the page (an amount from a word
//! equal to it, a date from words that read as that date, a reference or
//! vendor name from words that normalize to it). A value it gives that
//! differs from the rules' is a check naming the AI's value; agreement
//! raises the rules' confidence. A gap the AI fills is capped below the
//! medium bucket's top, so the owner sees it tinted. Lines come from the
//! AI only when the rules read none; each needs its amount on the page,
//! keeps a quantity or price only when that is on the page too, and then
//! passes the engine's quantity × price rule like any read line.
//!
//! "Equal to a page word" means one word: two words are never joined, so
//! a quantity next to a price can't pass as a thousands amount.
//!
//! **Consumer:** the desktop optional readers (WP-G, step 21); the
//! signatures here are fixed.

use crate::amount;
use crate::dates;
use crate::dupes::normalize_reference;
use crate::fields::{self, clean_id};
use crate::layout::{self, Layout, join, union};
use crate::lines::qty_times_price;
use crate::model::{
    Check, CheckCode, CheckValue, Confidence, FieldName, Found, Line, MAX_CHECKS, MAX_LINES,
    MAX_TEXT_CHARS, PageWords, Reading, RuleId, Spot,
};
use crate::vendor_match::normalize_loose;
use cl_core::{CalendarDate, Money, Quantity};
use std::fmt;

/// Which optional reader answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiSource {
    /// A local Ollama model.
    Ollama,
    /// The company's cloud provider, with the owner's consent (Q-A03k).
    Cloud,
}

impl AiSource {
    fn rule(self) -> RuleId {
        match self {
            AiSource::Ollama => RuleId::new("ai.ollama"),
            AiSource::Cloud => RuleId::new("ai.cloud"),
        }
    }
}

/// One AI line item; amounts already parsed from the engine's strings.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct AiLine {
    pub description: Option<String>,
    pub quantity: Option<Quantity>,
    pub unit_price: Option<Money>,
    pub amount: Option<Money>,
}

impl fmt::Debug for AiLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AiLine")
            .field("description", &self.description.is_some())
            .field("quantity", &self.quantity.is_some())
            .field("unit_price", &self.unit_price.is_some())
            .field("amount", &self.amount.is_some())
            .finish()
    }
}

/// What an optional reader returned (the engine's `BillOcrResult` shape,
/// `ai-bill-ocr.service.ts:243-272`).
#[derive(Clone, PartialEq, Eq, Default)]
pub struct AiReading {
    pub vendor: Option<String>,
    pub reference: Option<String>,
    pub date: Option<CalendarDate>,
    pub due_date: Option<CalendarDate>,
    pub subtotal: Option<Money>,
    pub tax: Option<Money>,
    pub total: Option<Money>,
    pub amount_paid: Option<Money>,
    pub lines: Vec<AiLine>,
    /// The reader's overall confidence.
    pub confidence: Confidence,
}

impl fmt::Debug for AiReading {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AiReading")
            .field("lines", &self.lines.len())
            .field("confidence", &self.confidence)
            .finish_non_exhaustive()
    }
}

/// Added to a rules value the AI agrees with.
const AGREE_BONUS: u16 = 1_000;
/// The most confidence a value the AI filled can have.
const AI_FILL: u16 = 7_000;

/// The page's words, indexed for the gate.
struct Page<'a> {
    words: &'a [PageWords],
    layout: Layout,
}

impl Page<'_> {
    /// The first word (reading order) whose amount equals `m`.
    fn money_word(&self, m: Money) -> Option<(Spot, u16)> {
        self.find_word(|t| amount::parse(t) == Some(m))
    }

    fn quantity_word(&self, q: Quantity) -> Option<(Spot, u16)> {
        self.find_word(|t| amount::parse_quantity(t) == Some(q))
    }

    fn find_word(&self, pred: impl Fn(&str) -> bool) -> Option<(Spot, u16)> {
        self.words.iter().find_map(|p| {
            p.words.iter().find(|w| pred(&w.text)).map(|w| {
                (
                    Spot {
                        page: p.page,
                        bbox: w.bbox,
                    },
                    w.confidence.bp(),
                )
            })
        })
    }

    /// Up to four neighbouring words on a line that read as `date`.
    fn date_words(&self, date: CalendarDate, today: CalendarDate) -> Option<(Spot, u16, bool)> {
        for line in &self.layout.lines {
            let page = &self.words[line.page_index];
            for start in 0..line.words.len() {
                for len in 1..=4.min(line.words.len() - start) {
                    let ws = &line.words[start..start + len];
                    let text = join(ws.iter().map(|&w| &page.words[w]));
                    if let Some(p) = dates::parse(&text, today)
                        && p.date == date
                    {
                        let bbox = union(ws.iter().map(|&w| page.words[w].bbox))?;
                        let bp = ws.iter().map(|&w| page.words[w].confidence.bp()).min()?;
                        return Some((
                            Spot {
                                page: page.page,
                                bbox,
                            },
                            bp,
                            p.could_be_day_first,
                        ));
                    }
                }
            }
        }
        None
    }

    /// A segment whose loose text equals `name`'s.
    fn segment_named(&self, name: &str) -> Option<(String, Spot, u16)> {
        let want = normalize_loose(name);
        if want.is_empty() {
            return None;
        }
        (0..self.layout.segments.len()).find_map(|s| {
            let text = self.layout.segment_text(self.words, s);
            (normalize_loose(&text) == want).then(|| {
                let seg = &self.layout.segments[s];
                let page = &self.words[seg.page_index];
                let bbox = union(seg.words.iter().map(|&w| page.words[w].bbox))
                    .unwrap_or(crate::model::Bbox::PAGE);
                let bp = seg
                    .words
                    .iter()
                    .map(|&w| page.words[w].confidence.bp())
                    .min()
                    .unwrap_or(0);
                (
                    text.chars().take(MAX_TEXT_CHARS).collect(),
                    Spot {
                        page: page.page,
                        bbox,
                    },
                    bp,
                )
            })
        })
    }
}

/// Merge an optional reader's answer into the local reading under the
/// gate.
#[must_use]
pub fn merge(mut reading: Reading, ai: &AiReading, source: AiSource) -> Reading {
    let words = std::mem::take(&mut reading.words);
    {
        let page = Page {
            words: &words,
            layout: layout::analyze(&words),
        };
        let mut checks: Vec<Check> = Vec::new();
        let rule = source.rule();
        let fill_cap = AI_FILL.min(ai.confidence.bp().max(1));
        let h = &mut reading.header;
        for (target, value, field) in [
            (&mut h.subtotal, ai.subtotal, FieldName::Subtotal),
            (&mut h.tax, ai.tax, FieldName::Tax),
            (&mut h.total, ai.total, FieldName::Total),
            (&mut h.amount_paid, ai.amount_paid, FieldName::AmountPaid),
        ] {
            gate_money(target, value, field, &page, &rule, fill_cap, &mut checks);
        }
        for (target, value, field) in [
            (&mut h.date, ai.date, FieldName::Date),
            (&mut h.due_date, ai.due_date, FieldName::DueDate),
        ] {
            if let Some(d) = value {
                gate_date(target, d, field, &page, &rule, fill_cap, &mut checks);
            }
        }
        if let Some(r) = ai.reference.as_deref() {
            gate_reference(&mut h.reference, r, &page, &rule, fill_cap, &mut checks);
        }
        if let Some(v) = ai.vendor.as_deref() {
            gate_vendor(&mut h.vendor, v, &page, &rule, fill_cap, &mut checks);
        }
        gate_lines(&mut reading, &ai.lines, &page, &rule, fill_cap, &mut checks);
        reading.checks.extend(checks);
        reading.checks.truncate(MAX_CHECKS);
    }
    reading.words = words;
    reading
}

fn agree<T>(f: &mut Found<T>) {
    f.confidence = Confidence::saturating(u32::from(f.confidence.bp()) + u32::from(AGREE_BONUS));
}

fn disagrees(
    field: Option<FieldName>,
    line: Option<u16>,
    ai: CheckValue,
    local: Option<CheckValue>,
    spot: Option<Spot>,
) -> Check {
    let difference = match (&local, &ai) {
        (Some(CheckValue::Money(l)), CheckValue::Money(a)) => l.checked_sub(*a).ok(),
        _ => None,
    };
    Check {
        code: CheckCode::AiDisagrees,
        rule: RuleId::new("ai.gate"),
        field,
        line,
        expected: Some(ai),
        found: local,
        difference,
        spot,
    }
}

fn filled<T>(value: T, spot: Spot, word_bp: u16, rule: &RuleId, cap: u16) -> Found<T> {
    Found {
        value,
        spot,
        rule: rule.clone(),
        confidence: Confidence::saturating(u32::from(word_bp.min(cap))),
    }
}

fn gate_money(
    target: &mut Option<Found<Money>>,
    value: Option<Money>,
    field: FieldName,
    page: &Page<'_>,
    rule: &RuleId,
    cap: u16,
    checks: &mut Vec<Check>,
) {
    let Some(ai) = value else { return };
    match target {
        Some(local) if local.value == ai => agree(local),
        Some(local) => checks.push(disagrees(
            Some(field),
            None,
            CheckValue::Money(ai),
            Some(CheckValue::Money(local.value)),
            Some(local.spot),
        )),
        None => match page.money_word(ai) {
            Some((spot, bp)) => *target = Some(filled(ai, spot, bp, rule, cap)),
            None => checks.push(disagrees(
                Some(field),
                None,
                CheckValue::Money(ai),
                None,
                None,
            )),
        },
    }
}

/// The page is read with the AI's date as "today": only equality matters
/// here, not plausibility.
fn gate_date(
    target: &mut Option<Found<CalendarDate>>,
    ai: CalendarDate,
    field: FieldName,
    page: &Page<'_>,
    rule: &RuleId,
    cap: u16,
    checks: &mut Vec<Check>,
) {
    match target {
        Some(local) if local.value == ai => agree(local),
        Some(local) => checks.push(disagrees(
            Some(field),
            None,
            CheckValue::Date(ai),
            Some(CheckValue::Date(local.value)),
            Some(local.spot),
        )),
        None => match page.date_words(ai, ai) {
            Some((spot, bp, day_first)) => {
                let f = filled(ai, spot, bp, rule, cap);
                if day_first {
                    checks.push(fields::day_first_check(field, &f));
                }
                *target = Some(f);
            }
            None => checks.push(disagrees(
                Some(field),
                None,
                CheckValue::Date(ai),
                None,
                None,
            )),
        },
    }
}

fn gate_reference(
    target: &mut Option<Found<String>>,
    ai: &str,
    page: &Page<'_>,
    rule: &RuleId,
    cap: u16,
    checks: &mut Vec<Check>,
) {
    let want = normalize_reference(ai);
    if want.is_empty() {
        return;
    }
    let text = |s: &str| CheckValue::Text(s.chars().take(MAX_TEXT_CHARS).collect());
    match target {
        Some(local) if normalize_reference(&local.value) == want => agree(local),
        Some(local) => checks.push(disagrees(
            Some(FieldName::Reference),
            None,
            text(ai),
            Some(text(&local.value)),
            Some(local.spot),
        )),
        None => {
            let hit = page.words.iter().find_map(|p| {
                p.words
                    .iter()
                    .find(|w| normalize_reference(&w.text) == want)
                    .map(|w| {
                        (
                            clean_id(&w.text),
                            Spot {
                                page: p.page,
                                bbox: w.bbox,
                            },
                            w.confidence.bp(),
                        )
                    })
            });
            match hit {
                Some((value, spot, bp)) => *target = Some(filled(value, spot, bp, rule, cap)),
                None => checks.push(disagrees(
                    Some(FieldName::Reference),
                    None,
                    text(ai),
                    None,
                    None,
                )),
            }
        }
    }
}

fn gate_vendor(
    target: &mut Option<Found<String>>,
    ai: &str,
    page: &Page<'_>,
    rule: &RuleId,
    cap: u16,
    checks: &mut Vec<Check>,
) {
    let text = |s: &str| CheckValue::Text(s.chars().take(MAX_TEXT_CHARS).collect());
    match target {
        Some(local) if normalize_loose(&local.value) == normalize_loose(ai) => agree(local),
        Some(local) => checks.push(disagrees(
            Some(FieldName::Vendor),
            None,
            text(ai),
            Some(text(&local.value)),
            Some(local.spot),
        )),
        None => match page.segment_named(ai) {
            Some((value, spot, bp)) => *target = Some(filled(value, spot, bp, rule, cap)),
            None => checks.push(disagrees(
                Some(FieldName::Vendor),
                None,
                text(ai),
                None,
                None,
            )),
        },
    }
}

fn gate_lines(
    reading: &mut Reading,
    ai_lines: &[AiLine],
    page: &Page<'_>,
    rule: &RuleId,
    cap: u16,
    checks: &mut Vec<Check>,
) {
    if !reading.lines.is_empty() {
        // The rules read the table: the AI only confirms or disagrees.
        for (i, l) in ai_lines.iter().enumerate() {
            let Some(a) = l.amount else { continue };
            match reading.lines.iter_mut().find(|x| x.amount.value == a) {
                Some(local) => agree(&mut local.amount),
                None => checks.push(disagrees(
                    None,
                    u16::try_from(i).ok(),
                    CheckValue::Money(a),
                    None,
                    None,
                )),
            }
        }
        return;
    }
    let mut added: Vec<Line> = Vec::new();
    for (i, l) in ai_lines.iter().enumerate().take(MAX_LINES) {
        let Some(a) = l.amount else { continue };
        let Some((spot, bp)) = page.money_word(a) else {
            checks.push(disagrees(
                None,
                u16::try_from(i).ok(),
                CheckValue::Money(a),
                None,
                None,
            ));
            continue;
        };
        let amount = filled(a, spot, bp, rule, cap);
        let quantity = l.quantity.and_then(|q| {
            page.quantity_word(q)
                .map(|(s, b)| filled(q, s, b, rule, cap))
        });
        let unit_price = l
            .unit_price
            .and_then(|p| page.money_word(p).map(|(s, b)| filled(p, s, b, rule, cap)));
        let description = l.description.as_deref().map(|d| {
            let (value, dspot, dbp) = page
                .segment_named(d)
                .unwrap_or_else(|| (d.chars().take(MAX_TEXT_CHARS).collect(), spot, 0));
            filled(value, dspot, dbp, rule, cap)
        });
        let mut line = Line {
            description,
            quantity,
            unit_price,
            amount,
        };
        let index = u16::try_from(added.len()).unwrap_or(u16::MAX);
        checks.extend(qty_times_price(&mut line, index));
        added.push(line);
    }
    if !added.is_empty() {
        reading.lines = added;
        reading.checks.retain(|c| c.code != CheckCode::LinesNotRead);
    }
}

#[cfg(test)]
mod tests;
