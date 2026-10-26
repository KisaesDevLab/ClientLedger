//! Header fields (WP-A, step 9; plan D3 `fields.rs`, M7): vendor (top
//! block, not "Bill to"/"Ship to"), date, reference, terms, due date,
//! subtotal, tax, fees, total, amount paid, balance, payment (card last 4,
//! PAID, check number, cash). A value is taken right of its label, then
//! below it, each with a spot, rule id and confidence.
//!
//! M7: arithmetic may only break a tie among *unlabelled* candidates (and
//! then adds a `PickedUsingTotals` check); a labelled value is never
//! replaced for failing to reconcile. With a template, each field comes
//! from the template's region when valid (parses, in the region) and falls
//! back to the rules per field on validity only; template and rules both
//! valid but different → the template's value plus a `ReadersDisagree`
//! check.
//!
//! Every amount is one page word read by [`amount::parse`], with that
//! word's box as its spot.
//!
//! Internal to the rules.

mod labels;

pub use labels::{Label, LabelHit, Token, canon, find as find_labels, tokens as label_tokens};

use crate::amount;
use crate::dates;
use crate::layout::{Layout, join, union};
use crate::model::{
    Bbox, Check, CheckCode, CheckValue, Confidence, FieldName, Found, Header, Last4, PageWords,
    Payment, RuleId, Spot, Template, Terms, Word,
};
use crate::templates;
use cl_core::{CalendarDate, Money};
use std::fmt;

/// What the fields step found.
#[derive(Debug, Clone, Default)]
pub struct Extracted {
    pub header: Header,
    pub checks: Vec<Check>,
    /// How the document says it was paid, for [`crate::kind`].
    pub signals: Signals,
    /// The template supplied at least one field.
    pub template_used: bool,
}

/// Payment and receipt signs on the page (plan D3 `kind.rs`).
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Signals {
    /// Masked card digits ("****4821", "ending in 4821").
    pub card: Option<Found<Last4>>,
    /// A card brand or "credit card" without digits.
    pub card_brand: bool,
    /// "Check # 1234".
    pub check: Option<Found<String>>,
    /// "Cash" as a payment.
    pub cash: Option<Spot>,
    /// A "PAID" stamp.
    pub paid_stamp: Option<Spot>,
    /// "Change" with an amount.
    pub change: bool,
    /// "Invoice", "Bill" or "Statement" as a title, or terms/due date.
    pub looks_like_bill: bool,
    /// "Statement" (of account) as a title: it lists several invoices, so
    /// it has no one reference.
    pub statement: bool,
}

impl fmt::Debug for Signals {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Signals")
            .field("card", &self.card.is_some())
            .field("card_brand", &self.card_brand)
            .field("check", &self.check.is_some())
            .field("cash", &self.cash.is_some())
            .field("paid_stamp", &self.paid_stamp.is_some())
            .field("change", &self.change)
            .field("looks_like_bill", &self.looks_like_bill)
            .field("statement", &self.statement)
            .finish()
    }
}

/// Confidence of a value right of its label, in the same segment.
const SAME_SEGMENT: u16 = 9_500;
/// Right of its label, across a gap.
const NEXT_SEGMENT: u16 = 9_300;
/// Below its label.
const BELOW: u16 = 8_800;
/// Several labelled values disagree; the position rule picked one.
const CONFLICT_CAP: u16 = 7_000;
/// The vendor's name from the letterhead.
const VENDOR: u16 = 8_000;
/// Address and phone from the letterhead.
const VENDOR_DETAIL: u16 = 7_000;
/// Terms recognised without a label ("Net 30").
const TERMS_PATTERN: u16 = 7_000;
/// The only unlabelled amount below the table.
const LONE_UNLABELLED: u16 = 5_000;
/// A date printed without a label.
const UNLABELLED_DATE: u16 = 6_500;
/// An unlabelled amount picked by the totals (M7).
const PICKED: u16 = 6_000;
/// A value from a template region.
const TEMPLATE: u16 = 9_800;

/// Where a value was relative to its label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Place {
    Same,
    Next,
    Below,
}

impl Place {
    fn bp(self) -> u16 {
        match self {
            Place::Same => SAME_SEGMENT,
            Place::Next => NEXT_SEGMENT,
            Place::Below => BELOW,
        }
    }
}

fn rule(field: FieldName, place: Place) -> RuleId {
    macro_rules! ids {
        ($($f:ident => $name:literal),* $(,)?) => {
            match (field, place) {
                $(
                    (FieldName::$f, Place::Below) => RuleId::new(concat!("fields.", $name, ".label-below")),
                    (FieldName::$f, _) => RuleId::new(concat!("fields.", $name, ".label-right")),
                )*
            }
        };
    }
    ids!(
        Vendor => "vendor",
        VendorAddress => "vendorAddress",
        VendorPhone => "vendorPhone",
        AccountNumber => "accountNumber",
        Date => "date",
        Reference => "reference",
        Terms => "terms",
        DueDate => "dueDate",
        Subtotal => "subtotal",
        Tax => "tax",
        Fees => "fees",
        Total => "total",
        AmountPaid => "amountPaid",
        BalanceDue => "balanceDue",
        Payment => "payment",
    )
}

/// A labelled candidate.
#[derive(Clone)]
struct Cand<T> {
    found: Found<T>,
    prio: u8,
    /// (page index, top, left) of the value.
    at: (usize, u16, u16),
}

/// Candidates by field.
#[derive(Default)]
struct Cands {
    total: Vec<Cand<Money>>,
    amount_due: Vec<Cand<Money>>,
    balance: Vec<Cand<Money>>,
    paid: Vec<Cand<Money>>,
    subtotal: Vec<Cand<Money>>,
    tax: Vec<Cand<Money>>,
    fees: Vec<Cand<Money>>,
    date: Vec<(Cand<CalendarDate>, bool)>,
    due: Vec<(Cand<CalendarDate>, bool)>,
    reference: Vec<Cand<String>>,
    terms: Vec<Cand<Terms>>,
    account: Vec<Cand<String>>,
    phone: Vec<Cand<String>>,
}

impl Cands {
    fn count(&self) -> usize {
        self.total.len()
            + self.amount_due.len()
            + self.balance.len()
            + self.paid.len()
            + self.subtotal.len()
            + self.tax.len()
            + self.fees.len()
            + self.date.len()
            + self.due.len()
            + self.reference.len()
            + self.terms.len()
            + self.account.len()
            + self.phone.len()
    }
}

/// Context shared by the scan.
struct Ctx<'a> {
    layout: &'a Layout,
    pages: &'a [PageWords],
    /// Lines inside a line-item table (header row included).
    in_table: Vec<bool>,
    today: CalendarDate,
}

impl Ctx<'_> {
    fn word(&self, pi: usize, w: usize) -> &Word {
        &self.pages[pi].words[w]
    }

    fn spot(&self, pi: usize, words: &[usize]) -> Spot {
        Spot {
            page: self.pages[pi].page,
            bbox: union(words.iter().map(|&w| self.word(pi, w).bbox)).unwrap_or(Bbox::PAGE),
        }
    }

    fn min_conf(&self, pi: usize, words: &[usize]) -> u16 {
        words
            .iter()
            .map(|&w| self.word(pi, w).confidence.bp())
            .min()
            .unwrap_or(0)
    }

    fn seg_tokens(&self, seg: usize) -> Vec<Token> {
        label_tokens(self.layout.segment_words(self.pages, seg))
    }
}

/// Extract the header.
#[must_use]
pub fn extract(
    layout: &Layout,
    pages: &[PageWords],
    template: Option<&Template>,
    today: CalendarDate,
) -> Extracted {
    let spans =
        crate::lines::table_spans(layout, pages, template.and_then(|t| t.line_table.as_ref()));
    let mut in_table = vec![false; layout.lines.len()];
    for s in &spans {
        for &l in s.header_line.iter().chain(&s.rows) {
            in_table[l] = true;
        }
    }
    let ctx = Ctx {
        layout,
        pages,
        in_table,
        today,
    };
    let mut cands = Cands::default();
    let mut signals = Signals::default();
    for li in 0..layout.lines.len() {
        if !ctx.in_table[li] {
            scan_line(&ctx, li, &mut cands, &mut signals);
        }
    }
    let mut checks = Vec::new();
    let mut header = choose(&ctx, cands, &mut checks);
    vendor(&ctx, &mut header);
    header.payment = payment(&signals);
    if signals.statement {
        header.reference = None;
    }
    signals.looks_like_bill |= header.terms.is_some() || header.due_date.is_some();
    let mut out = Extracted {
        header,
        checks,
        signals,
        template_used: false,
    };
    if let Some(t) = template {
        let (from_template, template_checks) = templates::apply_with_checks(t, pages, today);
        merge_template(&mut out, from_template, template_checks);
    }
    out
}

// ---------------------------------------------------------------------------
// Scanning
// ---------------------------------------------------------------------------

fn scan_line(ctx: &Ctx<'_>, li: usize, cands: &mut Cands, signals: &mut Signals) {
    let line = &ctx.layout.lines[li];
    let pi = line.page_index;
    for (k, &seg) in line.segments.iter().enumerate() {
        let seg_words = &ctx.layout.segments[seg].words;
        let tokens = ctx.seg_tokens(seg);
        scan_signals(ctx, pi, seg, &tokens, signals);
        let hits = find_labels(&tokens);
        for (h, hit) in hits.iter().enumerate() {
            let end = hits.get(h + 1).map_or(tokens.len(), |n| n.start);
            let mut value: Vec<usize> = Vec::new();
            for t in &tokens[hit.end..end] {
                let w = seg_words[t.word];
                if value.last() != Some(&w) {
                    value.push(w);
                }
            }
            // Right of the label in its segment; failing that (nothing
            // there, or nothing valid), the next segment, then below.
            if !value.is_empty()
                && candidate(ctx, pi, hit.label, &value, Place::Same, cands, signals)
            {
                continue;
            }
            let last = h + 1 == hits.len();
            // A bare "Invoice" or "#" takes a value beside or under it only
            // when that is a lone number, not an address line or a title's
            // neighbour ("PARTS INVOICE" → "915 W Walnut St").
            let lone = |seg: usize| {
                hit.label != Label::Reference(1) || ctx.layout.segments[seg].words.len() <= 2
            };
            if last
                && hit.label.value_may_be_next_segment()
                && let Some(&next) = line.segments.get(k + 1)
                && lone(next)
                && !labels::starts_with_label(&ctx.seg_tokens(next))
                && candidate(
                    ctx,
                    pi,
                    hit.label,
                    &ctx.layout.segments[next].words,
                    Place::Next,
                    cands,
                    signals,
                )
            {
                continue;
            }
            if value.is_empty()
                && last
                && hit.label.value_may_be_below()
                && let Some(below) = below_segment(ctx, seg, hit.label.is_amount())
                && lone(below)
            {
                candidate(
                    ctx,
                    pi,
                    hit.label,
                    &ctx.layout.segments[below].words,
                    Place::Below,
                    cands,
                    signals,
                );
            }
        }
    }
}

/// The segment right under `seg` (overlapping it horizontally, within two
/// line heights) unless it starts with a label. Amount values below a
/// label must be a lone number.
fn below_segment(ctx: &Ctx<'_>, seg: usize, amount_only: bool) -> Option<usize> {
    let s = &ctx.layout.segments[seg];
    let sb = s.bbox;
    let reach = sb
        .bottom()
        .saturating_add(sb.height().saturating_mul(2).max(1));
    let best = ctx
        .layout
        .segments
        .iter()
        .enumerate()
        .filter(|(_, o)| {
            o.page_index == s.page_index
                && !ctx.in_table[o.line]
                && o.bbox.top() > sb.bottom().saturating_sub(sb.height() / 4)
                && o.bbox.top() > sb.top()
                && o.bbox.top() <= reach
                && o.bbox.left() <= sb.right()
                && sb.left() <= o.bbox.right()
        })
        .min_by_key(|(_, o)| (o.bbox.top(), o.bbox.left()))?
        .0;
    if labels::starts_with_label(&ctx.seg_tokens(best)) {
        return None;
    }
    if amount_only && ctx.layout.segments[best].words.len() > 2 {
        return None;
    }
    Some(best)
}

/// Add the candidate a label's value gives; `true` when it gave one.
fn candidate(
    ctx: &Ctx<'_>,
    pi: usize,
    label: Label,
    value: &[usize],
    place: Place,
    cands: &mut Cands,
    signals: &mut Signals,
) -> bool {
    let before = (cands.count(), signals.change, signals.check.is_some());
    // After a card brand or "paid", four bare digits are the card ("Paid
    // with Visa ending in 1172"), not an amount.
    let need_point = matches!(label, Label::Card | Label::Paid);
    let money_field = |field: FieldName| -> Option<Cand<Money>> {
        let (w, m) = value.iter().rev().find_map(|&w| {
            let text = &ctx.word(pi, w).text;
            if need_point && !text.contains('.') {
                return None;
            }
            amount::parse(text).map(|m| (w, m))
        })?;
        let spot = ctx.spot(pi, &[w]);
        Some(Cand {
            found: Found {
                value: m,
                spot,
                rule: rule(field, place),
                confidence: conf(place.bp(), ctx.min_conf(pi, &[w])),
            },
            prio: 0,
            at: (pi, spot.bbox.top(), spot.bbox.left()),
        })
    };
    match label {
        Label::Total(p) => cands
            .total
            .extend(money_field(FieldName::Total).map(|c| prio(c, p))),
        Label::AmountDue => cands.amount_due.extend(money_field(FieldName::Total)),
        Label::Balance => cands.balance.extend(money_field(FieldName::BalanceDue)),
        Label::Paid => cands.paid.extend(money_field(FieldName::AmountPaid)),
        Label::Card => {
            signals.card_brand = true;
            cands
                .paid
                .extend(money_field(FieldName::AmountPaid).map(|c| prio(c, 1)));
        }
        Label::Subtotal => cands.subtotal.extend(money_field(FieldName::Subtotal)),
        Label::Tax => cands.tax.extend(money_field(FieldName::Tax)),
        Label::Fee => cands.fees.extend(money_field(FieldName::Fees)),
        Label::Change => signals.change |= money_field(FieldName::AmountPaid).is_some(),
        Label::Date(p) => cands.date.extend(
            date_value(ctx, pi, value, FieldName::Date, place).map(|(c, f)| (prio(c, p), f)),
        ),
        Label::Due => match date_value(ctx, pi, value, FieldName::DueDate, place) {
            Some(d) => cands.due.push(d),
            // "Payment due $84.10": an amount due, not a date.
            None => cands.amount_due.extend(money_field(FieldName::Total)),
        },
        Label::Reference(p) => cands
            .reference
            .extend(id_value(ctx, pi, value, FieldName::Reference, place).map(|c| prio(c, p))),
        Label::Account => {
            cands
                .account
                .extend(id_value(ctx, pi, value, FieldName::AccountNumber, place));
        }
        Label::Terms => cands.terms.extend(terms_value(ctx, pi, value, place)),
        Label::Phone => {
            let text = join(value.iter().map(|&w| ctx.word(pi, w)));
            if let Some(phone) = phone_in(&text) {
                let spot = ctx.spot(pi, value);
                cands.phone.push(Cand {
                    found: Found {
                        value: phone,
                        spot,
                        rule: rule(FieldName::VendorPhone, place),
                        confidence: conf(place.bp(), ctx.min_conf(pi, value)),
                    },
                    prio: 0,
                    at: (pi, spot.bbox.top(), spot.bbox.left()),
                });
            }
        }
        Label::CheckNumber => {
            // A restaurant's "Check #" is the bill's number: a reference
            // candidate too (the kind decides whether it was a payment).
            cands
                .reference
                .extend(id_value(ctx, pi, value, FieldName::Reference, place).map(|c| prio(c, 1)));
            if place == Place::Same
                && let Some(&w) = value.first()
            {
                let t = ctx.word(pi, w).text.trim_start_matches('#');
                if (1..=10).contains(&t.len()) && t.bytes().all(|b| b.is_ascii_digit()) {
                    signals.check.get_or_insert(Found {
                        value: t.to_string(),
                        spot: ctx.spot(pi, &[w]),
                        rule: RuleId::new("fields.payment.check-number"),
                        confidence: conf(SAME_SEGMENT, ctx.min_conf(pi, &[w])),
                    });
                }
            }
        }
        Label::Party => {}
    }
    before != (cands.count(), signals.change, signals.check.is_some())
}

fn prio<T>(mut c: Cand<T>, p: u8) -> Cand<T> {
    c.prio = p;
    c
}

/// `min(rule, words)` in basis points.
fn conf(rule_bp: u16, word_bp: u16) -> Confidence {
    Confidence::saturating(u32::from(rule_bp.min(word_bp)))
}

fn date_value(
    ctx: &Ctx<'_>,
    pi: usize,
    value: &[usize],
    field: FieldName,
    place: Place,
) -> Option<(Cand<CalendarDate>, bool)> {
    for start in 0..value.len() {
        for len in (1..=4.min(value.len() - start)).rev() {
            let ws = &value[start..start + len];
            let text = join(ws.iter().map(|&w| ctx.word(pi, w)));
            if let Some(p) = dates::parse(&text, ctx.today) {
                let spot = ctx.spot(pi, ws);
                let bp = place.bp().min(p.confidence.bp());
                return Some((
                    Cand {
                        found: Found {
                            value: p.date,
                            spot,
                            rule: rule(field, place),
                            confidence: conf(bp, ctx.min_conf(pi, ws)),
                        },
                        prio: 0,
                        at: (pi, spot.bbox.top(), spot.bbox.left()),
                    },
                    p.could_be_day_first,
                ));
            }
        }
    }
    None
}

/// An invoice or account number: the first word with a digit.
fn id_value(
    ctx: &Ctx<'_>,
    pi: usize,
    value: &[usize],
    field: FieldName,
    place: Place,
) -> Option<Cand<String>> {
    let (w, text) = value.iter().find_map(|&w| {
        let t = clean_id(&ctx.word(pi, w).text);
        is_id(&t, ctx.today).then_some((w, t))
    })?;
    let spot = ctx.spot(pi, &[w]);
    Some(Cand {
        found: Found {
            value: text,
            spot,
            rule: rule(field, place),
            confidence: conf(place.bp(), ctx.min_conf(pi, &[w])),
        },
        prio: 0,
        at: (pi, spot.bbox.top(), spot.bbox.left()),
    })
}

/// Whether 	 can be an invoice or account number: it has a digit and is
/// not a date or a printed amount (`12.50`, `$` anything).
fn is_id(t: &str, today: CalendarDate) -> bool {
    let money_like = t.split_once('.').is_some_and(|(i, f)| {
        !i.is_empty()
            && f.len() == 2
            && (i.bytes().chain(f.bytes())).all(|b| b.is_ascii_digit() || b == b',')
    });
    t.chars().any(|c| c.is_ascii_digit())
        && !t.contains('$')
        && !money_like
        && dates::parse(t, today).is_none()
}

/// An id as printed, without a leading `#`/`:` or trailing punctuation.
pub(crate) fn clean_id(text: &str) -> String {
    text.trim_start_matches(['#', ':'])
        .trim_end_matches([',', '.', ';', ':'])
        .chars()
        .take(crate::model::MAX_TEXT_CHARS)
        .collect()
}

fn terms_value(ctx: &Ctx<'_>, pi: usize, value: &[usize], place: Place) -> Option<Cand<Terms>> {
    let ws: Vec<usize> = value.iter().copied().take(6).collect();
    let text: String = join(ws.iter().map(|&w| ctx.word(pi, w)))
        .chars()
        .take(crate::model::MAX_TEXT_CHARS)
        .collect();
    if !text.chars().any(char::is_alphanumeric) {
        return None;
    }
    let spot = ctx.spot(pi, &ws);
    Some(Cand {
        found: Found {
            value: Terms {
                days: terms_days(&text),
                text,
            },
            spot,
            rule: rule(FieldName::Terms, place),
            confidence: conf(place.bp(), ctx.min_conf(pi, &ws)),
        },
        prio: 0,
        at: (pi, spot.bbox.top(), spot.bbox.left()),
    })
}

/// The days payment terms mean: "Net 30" → 30, "2/10 Net 45" → 45, "Due on
/// receipt" → 0; `None` when unknown.
#[must_use]
pub fn terms_days(text: &str) -> Option<u16> {
    let lower = text.to_lowercase();
    let tokens: Vec<&str> = lower
        .split(|c: char| c.is_whitespace() || matches!(c, ',' | ';' | ':' | '(' | ')'))
        .filter(|t| !t.is_empty())
        .collect();
    let number = |t: &str| -> Option<u16> {
        (!t.is_empty() && t.len() <= 3 && t.bytes().all(|b| b.is_ascii_digit()))
            .then(|| t.parse().ok())
            .flatten()
    };
    for (i, t) in tokens.iter().enumerate() {
        for prefix in ["net", "n/", "n"] {
            if let Some(rest) = t.strip_prefix(prefix)
                && let Some(d) = number(rest)
            {
                return Some(d);
            }
        }
        if *t == "net"
            && let Some(d) = tokens.get(i + 1).and_then(|n| number(n))
        {
            return Some(d);
        }
        if *t == "in"
            && let Some(d) = tokens.get(i + 1).and_then(|n| number(n))
            && tokens.get(i + 2).is_some_and(|n| n.starts_with("day"))
        {
            return Some(d);
        }
    }
    let joined = tokens.join(" ");
    let now = [
        "due on receipt",
        "due upon receipt",
        "upon receipt",
        "on receipt",
        "cod",
        "c.o.d.",
        "due now",
    ];
    now.iter().any(|p| joined.contains(p)).then_some(0)
}

/// A phone number in `text`: 10 digits (or 11 starting with 1) written
/// with at least one separator. Returns the printed run.
#[must_use]
pub fn phone_in(text: &str) -> Option<String> {
    let allowed = |c: char| c.is_ascii_digit() || matches!(c, '(' | ')' | '-' | '.' | ' ' | '+');
    let mut runs: Vec<&str> = Vec::new();
    let mut start: Option<usize> = None;
    for (i, c) in text.char_indices() {
        match (allowed(c), start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                runs.push(&text[s..i]);
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        runs.push(&text[s..]);
    }
    runs.into_iter().find_map(|r| {
        let r = r.trim_matches(|c: char| c == ' ' || c == '.' || c == '-');
        let digits: String = r.chars().filter(char::is_ascii_digit).collect();
        let ok_len = digits.len() == 10 || (digits.len() == 11 && digits.starts_with('1'));
        let has_sep = r.chars().any(|c| !c.is_ascii_digit());
        (ok_len && has_sep).then(|| r.to_string())
    })
}

// ---------------------------------------------------------------------------
// Payment signs
// ---------------------------------------------------------------------------

fn scan_signals(ctx: &Ctx<'_>, pi: usize, seg: usize, tokens: &[Token], signals: &mut Signals) {
    let seg_words = &ctx.layout.segments[seg].words;
    let texts: Vec<&str> = tokens.iter().map(|t| t.text.as_str()).collect();
    // Card digits.
    if signals.card.is_none() {
        for (i, t) in tokens.iter().enumerate() {
            let w = seg_words[t.word];
            let raw = ctx.word(pi, w).text.trim_end_matches(['.', ',', ')', ';']);
            let after_cue = i > 0
                && (matches!(texts[i - 1], "ending" | "in" | "x" | "card" | "#")
                    || is_mask(texts[i - 1]));
            let last4 = masked_last4(raw).or_else(|| {
                (after_cue && raw.len() == 4)
                    .then(|| Last4::parse(raw))
                    .flatten()
            });
            if let Some(l4) = last4 {
                signals.card = Some(Found {
                    value: l4,
                    spot: ctx.spot(pi, &[w]),
                    rule: RuleId::new("fields.payment.card-last4"),
                    confidence: conf(9_000, ctx.min_conf(pi, &[w])),
                });
                break;
            }
        }
    }
    // Cash as a payment.
    if signals.cash.is_none()
        && let Some(i) = texts.iter().position(|t| *t == "cash")
        && !texts.get(i + 1).is_some_and(|n| {
            matches!(
                *n,
                "back" | "discount" | "advance" | "flow" | "basis" | "account"
            )
        })
    {
        signals.cash = Some(ctx.spot(pi, &[seg_words[tokens[i].word]]));
    }
    // A PAID stamp.
    if signals.paid_stamp.is_none() && texts.first() == Some(&"paid") {
        let rest = &texts[1..];
        let mut rest_words: Vec<usize> = tokens[1..].iter().map(|t| seg_words[t.word]).collect();
        rest_words.dedup();
        let rest_text = join(rest_words.iter().map(|&w| ctx.word(pi, w)));
        if rest.is_empty()
            || rest == ["in", "full"]
            || dates::parse(&rest_text, ctx.today).is_some()
        {
            signals.paid_stamp = Some(ctx.spot(pi, seg_words));
        }
    }
    if texts
        .first()
        .is_some_and(|t| matches!(*t, "invoice" | "bill" | "statement"))
        && texts.len() == 1
    {
        signals.looks_like_bill = true;
    }
    if texts == ["statement"] || texts == ["statement", "of", "account"] {
        signals.statement = true;
    }
}

fn is_mask(t: &str) -> bool {
    t.len() >= 2
        && t.chars()
            .all(|c| matches!(c, '*' | 'x' | 'X' | '•' | '#' | '.'))
}

/// "************4821", "XXXX-4821", "x4821" (one `x` is too weak alone).
fn masked_last4(raw: &str) -> Option<Last4> {
    let digits_at = raw.len().checked_sub(4)?;
    if !raw.is_char_boundary(digits_at) {
        return None;
    }
    let (mask, digits) = raw.split_at(digits_at);
    let mask = mask.trim_end_matches(['-', ' ']);
    if is_mask(mask) {
        Last4::parse(digits)
    } else {
        None
    }
}

fn payment(signals: &Signals) -> Option<Found<Payment>> {
    if let Some(c) = &signals.card {
        return Some(Found {
            value: Payment::Card { last4: c.value },
            spot: c.spot,
            rule: c.rule.clone(),
            confidence: c.confidence,
        });
    }
    if let Some(c) = &signals.check {
        return Some(Found {
            value: Payment::Check {
                number: c.value.clone(),
            },
            spot: c.spot,
            rule: c.rule.clone(),
            confidence: c.confidence,
        });
    }
    let simple = |spot: Spot, value: Payment, id: &'static str| Found {
        value,
        spot,
        rule: RuleId::new(id),
        confidence: Confidence::saturating(8_000),
    };
    if let Some(spot) = signals.cash {
        return Some(simple(spot, Payment::Cash, "fields.payment.cash"));
    }
    signals
        .paid_stamp
        .map(|spot| simple(spot, Payment::Paid, "fields.payment.paid-stamp"))
}

// ---------------------------------------------------------------------------
// Choosing among candidates
// ---------------------------------------------------------------------------

/// Amounts: the strongest label, then the last on the page (totals boxes
/// sit at the bottom). Labelled values that disagree are capped at medium.
fn pick_last<T: Clone + PartialEq>(cands: &[Cand<T>]) -> Option<Found<T>> {
    let best = cands.iter().map(|c| c.prio).min()?;
    let group: Vec<&Cand<T>> = cands.iter().filter(|c| c.prio == best).collect();
    let chosen = group.iter().max_by_key(|c| c.at)?;
    Some(capped(chosen, &group))
}

/// Dates and ids: the strongest label, then the first.
fn pick_first<T: Clone + PartialEq>(cands: &[Cand<T>]) -> Option<Found<T>> {
    let best = cands.iter().map(|c| c.prio).min()?;
    let group: Vec<&Cand<T>> = cands.iter().filter(|c| c.prio == best).collect();
    let chosen = group.iter().min_by_key(|c| c.at)?;
    Some(capped(chosen, &group))
}

fn capped<T: Clone + PartialEq>(chosen: &Cand<T>, group: &[&Cand<T>]) -> Found<T> {
    let mut f = chosen.found.clone();
    if group.iter().any(|c| c.found.value != f.value) {
        f.confidence = f
            .confidence
            .min(Confidence::saturating(u32::from(CONFLICT_CAP)));
    }
    f
}

fn choose(ctx: &Ctx<'_>, mut c: Cands, checks: &mut Vec<Check>) -> Header {
    let mut h = Header::default();
    if c.total.is_empty() {
        c.total = std::mem::take(&mut c.amount_due);
    } else {
        c.balance.extend(c.amount_due.drain(..).map(|x| prio(x, 1)));
    }
    h.total = pick_last(&c.total);
    h.subtotal = pick_last(&c.subtotal);
    h.tax = pick_last(&c.tax);
    h.amount_paid = pick_last(&c.paid);
    h.balance_due = pick_last(&c.balance);
    let mut fees: Vec<Cand<Money>> = c.fees;
    fees.sort_by_key(|f| f.at);
    fees.dedup_by(|a, b| a.found.spot == b.found.spot);
    h.fees = fees.into_iter().map(|f| f.found).collect();
    for (target, list, field) in [
        (&mut h.date, &c.date, FieldName::Date),
        (&mut h.due_date, &c.due, FieldName::DueDate),
    ] {
        let plain: Vec<Cand<CalendarDate>> = list.iter().map(|(x, _)| x.clone()).collect();
        *target = pick_first(&plain);
        if let Some(found) = target.as_ref() {
            let flagged = list.iter().any(|(x, f)| *f && x.found.spot == found.spot);
            if flagged {
                checks.push(day_first_check(field, found));
            }
        }
    }
    h.reference = pick_first(&c.reference);
    h.account_number = pick_first(&c.account);
    h.terms = pick_first(&c.terms).or_else(|| terms_pattern(ctx));
    h.vendor_phone = pick_first(&c.phone);
    if h.total.is_none() {
        // No total printed: a statement's balance due, else what a
        // receipt says was paid ("Amount paid").
        h.total = h
            .balance_due
            .clone()
            .map(|f| relabel(f, "fields.total.balance"))
            .or_else(|| {
                h.amount_paid
                    .clone()
                    .map(|f| relabel(f, "fields.total.amount-paid"))
            });
    }
    if h.total.is_none() {
        unlabelled_total(ctx, &mut h, checks);
    }
    if h.date.is_none() {
        h.date = unlabelled_date(ctx);
    }
    h
}

fn relabel<T>(mut f: Found<T>, id: &'static str) -> Found<T> {
    f.rule = RuleId::new(id);
    f
}

/// No labelled date (a register receipt prints it bare at the top): the
/// first date in reading order on the first page, outside line tables,
/// at medium confidence at most.
fn unlabelled_date(ctx: &Ctx<'_>) -> Option<Found<CalendarDate>> {
    let first_pi = ctx.layout.lines.first()?.page_index;
    for line in ctx
        .layout
        .lines
        .iter()
        .enumerate()
        .filter_map(|(li, l)| (l.page_index == first_pi && !ctx.in_table[li]).then_some(l))
    {
        // A due date is not the document's date.
        let due = line.segments.iter().any(|&s| {
            find_labels(&ctx.seg_tokens(s))
                .iter()
                .any(|h| h.label == Label::Due)
        });
        if due {
            continue;
        }
        for &seg in &line.segments {
            let words = &ctx.layout.segments[seg].words;
            if let Some((c, _)) = date_value(ctx, first_pi, words, FieldName::Date, Place::Below) {
                let mut f = c.found;
                f.rule = RuleId::new("fields.date.unlabelled");
                f.confidence = f
                    .confidence
                    .min(Confidence::saturating(u32::from(UNLABELLED_DATE)));
                return Some(f);
            }
        }
    }
    None
}

pub(crate) fn day_first_check(field: FieldName, found: &Found<CalendarDate>) -> Check {
    Check {
        code: CheckCode::DateCouldBeDayFirst,
        rule: RuleId::new("dates.could-be-day-first"),
        field: Some(field),
        line: None,
        expected: None,
        found: Some(CheckValue::Date(found.value)),
        difference: None,
        spot: Some(found.spot),
    }
}

/// "Net 30" or "Due on receipt" printed without a "Terms" label.
fn terms_pattern(ctx: &Ctx<'_>) -> Option<Found<Terms>> {
    ctx.layout.segments.iter().enumerate().find_map(|(si, s)| {
        if ctx.in_table[s.line] {
            return None;
        }
        let text: String = ctx
            .layout
            .segment_text(ctx.pages, si)
            .chars()
            .take(crate::model::MAX_TEXT_CHARS)
            .collect();
        let lower = text.to_lowercase();
        let looks = lower.contains("net") || lower.contains("receipt");
        let days = terms_days(&text).filter(|_| looks)?;
        Some(Found {
            value: Terms {
                text,
                days: Some(days),
            },
            spot: ctx.spot(s.page_index, &s.words),
            rule: RuleId::new("fields.terms.pattern"),
            confidence: conf(TERMS_PATTERN, ctx.min_conf(s.page_index, &s.words)),
        })
    })
}

/// No labelled total (M7): lone numbers outside the line table on the
/// last page are the candidates. One → taken at low confidence. Several →
/// only arithmetic can pick, and only when exactly one equals subtotal +
/// tax + fees; a `PickedUsingTotals` check says so. Otherwise none.
fn unlabelled_total(ctx: &Ctx<'_>, h: &mut Header, checks: &mut Vec<Check>) {
    let Some(last_pi) = ctx.layout.lines.iter().map(|l| l.page_index).max() else {
        return;
    };
    let used: Vec<Spot> = [&h.subtotal, &h.tax, &h.amount_paid, &h.balance_due]
        .into_iter()
        .flatten()
        .map(|f| f.spot)
        .chain(h.fees.iter().map(|f| f.spot))
        .collect();
    let mut found: Vec<Found<Money>> = Vec::new();
    for s in &ctx.layout.segments {
        if s.page_index != last_pi || ctx.in_table[s.line] || s.words.len() > 2 {
            continue;
        }
        let tokens = label_tokens(s.words.iter().map(|&w| ctx.word(last_pi, w)));
        if !find_labels(&tokens).is_empty() {
            continue;
        }
        let Some((w, m)) = s
            .words
            .iter()
            .rev()
            .find_map(|&w| amount::parse(&ctx.word(last_pi, w).text).map(|m| (w, m)))
        else {
            continue;
        };
        let spot = ctx.spot(last_pi, &[w]);
        if used.contains(&spot) || dates::parse(&ctx.word(last_pi, w).text, ctx.today).is_some() {
            continue;
        }
        found.push(Found {
            value: m,
            spot,
            rule: RuleId::new("fields.total.unlabelled"),
            confidence: conf(LONE_UNLABELLED, ctx.min_conf(last_pi, &[w])),
        });
    }
    let mut values: Vec<Money> = found.iter().map(|f| f.value).collect();
    values.sort_unstable();
    values.dedup();
    match values.len() {
        0 => {}
        1 => h.total = found.into_iter().next_back(),
        _ => {
            let Some(sub) = &h.subtotal else { return };
            let parts = std::iter::once(sub.value)
                .chain(h.tax.iter().map(|t| t.value))
                .chain(h.fees.iter().map(|f| f.value));
            let Ok(sum) = Money::sum(parts) else { return };
            let matching: Vec<&Found<Money>> = found.iter().filter(|f| f.value == sum).collect();
            if let Some(pick) = matching.last() {
                let mut pick = (*pick).clone();
                pick.rule = RuleId::new("fields.total.picked-using-totals");
                pick.confidence = pick
                    .confidence
                    .min(Confidence::saturating(u32::from(PICKED)));
                checks.push(Check {
                    code: CheckCode::PickedUsingTotals,
                    rule: RuleId::new("fields.total.picked-using-totals"),
                    field: Some(FieldName::Total),
                    line: None,
                    expected: Some(CheckValue::Money(sum)),
                    found: Some(CheckValue::Money(pick.value)),
                    difference: None,
                    spot: Some(pick.spot),
                });
                h.total = Some(pick);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Vendor
// ---------------------------------------------------------------------------

const TITLES: [&str; 12] = [
    "invoice",
    "receipt",
    "bill",
    "statement",
    "sales receipt",
    "tax invoice",
    "credit memo",
    "purchase order",
    "remittance",
    "original",
    "copy",
    "paid",
];

/// The vendor's name, address and phone from the first block in the top
/// quarter of the first page that is not the customer's ("Bill to").
fn vendor(ctx: &Ctx<'_>, h: &mut Header) {
    let Some(first_pi) = ctx.layout.lines.first().map(|l| l.page_index) else {
        return;
    };
    let mut blocks: Vec<usize> = (0..ctx.layout.blocks.len())
        .filter(|&b| {
            let block = &ctx.layout.blocks[b];
            block.page_index == first_pi && block.bbox.top() < 2_500
        })
        .collect();
    blocks.sort_by_key(|&b| {
        let bb = ctx.layout.blocks[b].bbox;
        (bb.top(), bb.left())
    });
    for b in blocks {
        let block = &ctx.layout.blocks[b];
        let first = block.segments[0];
        if find_labels(&ctx.seg_tokens(first))
            .first()
            .is_some_and(|hit| hit.label == Label::Party && hit.start == 0)
        {
            continue;
        }
        let Some(pos) = block.segments.iter().position(|&s| is_name(ctx, s)) else {
            continue;
        };
        let name_seg = block.segments[pos];
        let s = &ctx.layout.segments[name_seg];
        let text: String = ctx
            .layout
            .segment_text(ctx.pages, name_seg)
            .chars()
            .take(crate::model::MAX_TEXT_CHARS)
            .collect();
        h.vendor = Some(Found {
            value: text,
            spot: ctx.spot(s.page_index, &s.words),
            rule: RuleId::new("fields.vendor.top-block"),
            confidence: conf(VENDOR, ctx.min_conf(s.page_index, &s.words)),
        });
        let mut address: Vec<usize> = Vec::new();
        for &seg in &block.segments[pos + 1..] {
            let text = ctx.layout.segment_text(ctx.pages, seg);
            if let Some(phone) = phone_in(&text) {
                if h.vendor_phone.is_none() {
                    let s = &ctx.layout.segments[seg];
                    h.vendor_phone = Some(Found {
                        value: phone,
                        spot: ctx.spot(s.page_index, &s.words),
                        rule: RuleId::new("fields.vendorPhone.top-block"),
                        confidence: conf(VENDOR_DETAIL, ctx.min_conf(s.page_index, &s.words)),
                    });
                }
                break;
            }
            if text.contains('@')
                || text.to_lowercase().contains("www")
                || !find_labels(&ctx.seg_tokens(seg)).is_empty()
                || address.len() == 3
            {
                break;
            }
            address.push(seg);
        }
        if !address.is_empty() {
            let text: String = address
                .iter()
                .map(|&s| ctx.layout.segment_text(ctx.pages, s))
                .collect::<Vec<_>>()
                .join("\n")
                .chars()
                .take(crate::model::MAX_TEXT_CHARS)
                .collect();
            let words: Vec<usize> = address
                .iter()
                .flat_map(|&s| ctx.layout.segments[s].words.clone())
                .collect();
            h.vendor_address = Some(Found {
                value: text,
                spot: ctx.spot(first_pi, &words),
                rule: RuleId::new("fields.vendorAddress.top-block"),
                confidence: conf(VENDOR_DETAIL, ctx.min_conf(first_pi, &words)),
            });
        }
        return;
    }
}

/// A segment that can be a business name: letters, not a title, not a
/// label, not a date or a number.
fn is_name(ctx: &Ctx<'_>, seg: usize) -> bool {
    let text = ctx.layout.segment_text(ctx.pages, seg);
    let lower = text.to_lowercase();
    let letters = text.chars().filter(|c| c.is_alphabetic()).count();
    letters >= 2
        && !TITLES.contains(&lower.trim_end_matches([':', '.']))
        && find_labels(&ctx.seg_tokens(seg)).is_empty()
        && dates::parse(&text, ctx.today).is_none()
        && phone_in(&text).is_none()
        && !text.contains('@')
}

// ---------------------------------------------------------------------------
// Template merge (M7)
// ---------------------------------------------------------------------------

fn merge_template(out: &mut Extracted, t: Header, t_checks: Vec<Check>) {
    let mut used = false;
    let checks = &mut out.checks;
    let h = &mut out.header;
    macro_rules! field {
        ($name:ident, $field:expr, $val:expr) => {
            if let Some(tv) = t.$name {
                used = true;
                if let Some(rv) = h.$name.take() {
                    if !same(&rv, &tv) {
                        checks.push(disagree($field, $val(&rv.value), $val(&tv.value), rv.spot));
                    }
                }
                // The rules' day-first check for this field no longer
                // applies; the template's own comes with `t_checks`.
                checks.retain(|c| {
                    !(c.code == CheckCode::DateCouldBeDayFirst && c.field == Some($field))
                });
                h.$name = Some(tv);
            }
        };
    }
    let text = |s: &String| CheckValue::Text(s.clone());
    let money = |m: &Money| CheckValue::Money(*m);
    let date = |d: &CalendarDate| CheckValue::Date(*d);
    let terms = |t: &Terms| CheckValue::Text(t.text.clone());
    field!(vendor, FieldName::Vendor, text);
    field!(vendor_address, FieldName::VendorAddress, text);
    field!(vendor_phone, FieldName::VendorPhone, text);
    field!(account_number, FieldName::AccountNumber, text);
    field!(date, FieldName::Date, date);
    field!(reference, FieldName::Reference, text);
    field!(terms, FieldName::Terms, terms);
    field!(due_date, FieldName::DueDate, date);
    field!(subtotal, FieldName::Subtotal, money);
    field!(tax, FieldName::Tax, money);
    field!(total, FieldName::Total, money);
    field!(amount_paid, FieldName::AmountPaid, money);
    field!(balance_due, FieldName::BalanceDue, money);
    if !t.fees.is_empty() {
        used = true;
        let rule_sum = Money::sum(h.fees.iter().map(|f| f.value));
        let t_sum = Money::sum(t.fees.iter().map(|f| f.value));
        if !h.fees.is_empty()
            && let (Ok(r), Ok(tv)) = (rule_sum, t_sum)
            && r != tv
        {
            checks.push(disagree(
                FieldName::Fees,
                CheckValue::Money(r),
                CheckValue::Money(tv),
                h.fees[0].spot,
            ));
        }
        h.fees = t.fees;
    }
    // A total picked by arithmetic is replaced by the template's.
    if h.total
        .as_ref()
        .is_some_and(|f| f.rule.as_str() == "template")
    {
        checks.retain(|c| c.code != CheckCode::PickedUsingTotals);
    }
    checks.extend(t_checks);
    out.template_used = used;
}

fn same<T: PartialEq>(a: &Found<T>, b: &Found<T>) -> bool {
    a.value == b.value
}

fn disagree(field: FieldName, rules: CheckValue, template: CheckValue, rules_spot: Spot) -> Check {
    let difference = match (&template, &rules) {
        (CheckValue::Money(t), CheckValue::Money(r)) => t.checked_sub(*r).ok(),
        _ => None,
    };
    Check {
        code: CheckCode::ReadersDisagree,
        rule: RuleId::new("fields.template-vs-rules"),
        field: Some(field),
        line: None,
        expected: Some(rules),
        found: Some(template),
        difference,
        spot: Some(rules_spot),
    }
}

/// The template confidence for a value made of `words`.
pub(crate) fn template_conf(words: &[&Word]) -> Confidence {
    let w = words.iter().map(|w| w.confidence.bp()).min().unwrap_or(0);
    conf(TEMPLATE, w)
}

#[cfg(test)]
mod tests;
