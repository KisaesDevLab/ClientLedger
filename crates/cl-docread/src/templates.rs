//! Vendor layout templates (WP-A, step 11; plan D3 `templates.rs`, M7).
//! Fingerprint = masked anchor tokens from the top 35 % of page 1 (digit
//! runs of 4+ masked) + aspect ratio; a template matches at Jaccard ≥ 0.6
//! (integer: `intersection × 10 ≥ union × 6`). `learn` builds a template
//! from an accepted (corrected) reading, `merge` folds a new sample into a
//! saved one with integer arithmetic, `apply` reads the template's regions.
//!
//! A region value is valid when it parses and its words sit in the region
//! (centres inside, with a small margin for scan drift). Sum checks never
//! make a template value invalid (M7).
//!
//! **Consumers:** the pipeline (`fingerprint`, `best_match`, `apply`) and
//! the desktop / A04 (`fingerprint`, `learn`, `merge`); these signatures
//! are fixed.

use crate::amount;
use crate::dates;
use crate::fields::{self, clean_id, phone_in, terms_days};
use crate::layout::{Layout, center2_x, join, union};
use crate::model::{
    Bbox, Check, ColumnBand, ColumnKind, FieldName, Fingerprint, Found, Header, LineTable,
    MAX_FINGERPRINT_TOKENS, MAX_TEMPLATE_FIELDS, MAX_TEXT_CHARS, MAX_WORD_CHARS, PageWords,
    Reading, RuleId, Spot, Template, TemplateField, Terms, Word,
};
use cl_core::{CalendarDate, Money};

/// Jaccard similarity at which a template applies, in basis points.
pub const MATCH_BP: u16 = 6_000;
/// Fingerprint words come from above this line of page 1 (35 %).
const TOP_BAND: u16 = 3_500;
/// Page shapes further apart than this (per cent of the larger) never match.
const ASPECT_PCT: u64 = 5;
/// Margin around a region, x and y units.
const MARGIN_X: u16 = 200;
const MARGIN_Y: u16 = 80;

/// One fingerprint token: lower case, outer punctuation dropped, digit runs
/// of four or more masked as `#`.
fn token(text: &str) -> Option<String> {
    let t = text
        .trim_matches(|c: char| !c.is_alphanumeric())
        .to_lowercase();
    if t.is_empty() {
        return None;
    }
    let mut out = String::new();
    let mut run = String::new();
    for c in t.chars() {
        if c.is_ascii_digit() {
            run.push(c);
        } else {
            flush(&mut out, &mut run);
            out.push(c);
        }
    }
    flush(&mut out, &mut run);
    Some(out.chars().take(MAX_WORD_CHARS).collect())
}

fn flush(out: &mut String, run: &mut String) {
    if run.len() >= 4 {
        out.push('#');
    } else {
        out.push_str(run);
    }
    run.clear();
}

/// The layout fingerprint of a document, or `None` with no words on page 1.
#[must_use]
pub fn fingerprint(pages: &[PageWords]) -> Option<Fingerprint> {
    let page = pages.first()?;
    let mut tokens: Vec<String> = page
        .words
        .iter()
        .filter(|w| w.bbox.top() < TOP_BAND)
        .filter_map(|w| token(&w.text))
        .collect();
    if tokens.is_empty() {
        return None;
    }
    tokens.sort();
    tokens.dedup();
    tokens.truncate(MAX_FINGERPRINT_TOKENS);
    Some(Fingerprint {
        tokens,
        aspect: page.aspect,
    })
}

/// Jaccard similarity of two fingerprints' tokens, in basis points (0 when
/// the aspect ratios differ too much).
#[must_use]
pub fn similarity(a: &Fingerprint, b: &Fingerprint) -> u16 {
    let (hi, lo) = (a.aspect.max(b.aspect), a.aspect.min(b.aspect));
    if u64::from(hi - lo) * 100 > u64::from(hi) * ASPECT_PCT {
        return 0;
    }
    let (mut at_a, mut at_b, mut inter, mut uni) = (0, 0, 0_u64, 0_u64);
    while at_a < a.tokens.len() || at_b < b.tokens.len() {
        match (a.tokens.get(at_a), b.tokens.get(at_b)) {
            (Some(ta), Some(tb)) if ta == tb => {
                inter += 1;
                at_a += 1;
                at_b += 1;
            }
            (Some(ta), Some(tb)) if ta < tb => at_a += 1,
            (Some(_), None) => at_a += 1,
            _ => at_b += 1,
        }
        uni += 1;
    }
    if uni == 0 {
        return 0;
    }
    u16::try_from(inter * 10_000 / uni).unwrap_or(0)
}

/// The index of the most similar template at or above [`MATCH_BP`]
/// (the first on a tie).
#[must_use]
pub fn best_match(fingerprint: &Fingerprint, templates: &[Template]) -> Option<usize> {
    templates
        .iter()
        .enumerate()
        .map(|(i, t)| (similarity(fingerprint, &t.fingerprint), i))
        .filter(|(s, _)| *s >= MATCH_BP)
        .max_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)))
        .map(|(_, i)| i)
}

/// Read the template's regions. Fields that don't parse or fall outside
/// their region are left `None` for the rules to fill (per field, validity
/// only).
#[must_use]
pub fn apply(template: &Template, pages: &[PageWords], today: CalendarDate) -> Header {
    apply_with_checks(template, pages, today).0
}

/// [`apply`] plus the template's own "could be day-first" checks.
///
/// A field learned with a label ("Total", "Invoice #") is anchored to it:
/// the region moves up or down with the label's line, so totals boxes
/// that shift with the number of line items are still found. The label's
/// occurrences are tried nearest first; the first that yields a valid
/// value wins. A labelled field whose label is not on the page is invalid.
pub(crate) fn apply_with_checks(
    template: &Template,
    pages: &[PageWords],
    today: CalendarDate,
) -> (Header, Vec<Check>) {
    let layout = crate::layout::analyze(pages);
    let mut h = Header::default();
    let mut checks = Vec::new();
    for tf in template.fields.iter().take(MAX_TEMPLATE_FIELDS) {
        let Some(pi) = pages.iter().position(|p| p.page == tf.page) else {
            continue;
        };
        let page = &pages[pi];
        let value = anchored_regions(&layout, page, pi, tf)
            .into_iter()
            .find_map(|region| {
                read_region(tf.field, &region_words(page, region), page.page, today)
            });
        match value {
            Some(Value::Text(f)) => match tf.field {
                FieldName::Vendor => h.vendor = Some(f),
                FieldName::VendorAddress => h.vendor_address = Some(f),
                FieldName::VendorPhone => h.vendor_phone = Some(f),
                FieldName::Reference => h.reference = Some(f),
                _ => h.account_number = Some(f),
            },
            Some(Value::Date(f, flag)) => {
                if flag {
                    checks.push(fields::day_first_check(tf.field, &f));
                }
                if tf.field == FieldName::Date {
                    h.date = Some(f);
                } else {
                    h.due_date = Some(f);
                }
            }
            Some(Value::Terms(f)) => h.terms = Some(f),
            Some(Value::Money(f)) => match tf.field {
                FieldName::Subtotal => h.subtotal = Some(f),
                FieldName::Tax => h.tax = Some(f),
                FieldName::Total => h.total = Some(f),
                FieldName::AmountPaid => h.amount_paid = Some(f),
                FieldName::BalanceDue => h.balance_due = Some(f),
                _ => h.fees.push(f),
            },
            None => {}
        }
    }
    (h, checks)
}

/// A value read from a region.
enum Value {
    Text(Found<String>),
    Date(Found<CalendarDate>, bool),
    Terms(Found<Terms>),
    Money(Found<Money>),
}

/// Parse a region's words as `field`; `None` when invalid.
fn read_region(field: FieldName, words: &[&Word], page: u16, today: CalendarDate) -> Option<Value> {
    if words.is_empty() {
        return None;
    }
    let spot_of = |ws: &[&Word]| Spot {
        page,
        bbox: union(ws.iter().map(|w| w.bbox)).unwrap_or(Bbox::PAGE),
    };
    let text_value = |ws: &[&Word]| -> Option<Found<String>> {
        let text: String = join(ws.iter().copied())
            .chars()
            .take(MAX_TEXT_CHARS)
            .collect();
        (!text.trim().is_empty()).then(|| found(text, spot_of(ws), ws))
    };
    match field {
        FieldName::Vendor => text_value(words).map(Value::Text),
        FieldName::VendorAddress => address_value(words, page).map(Value::Text),
        FieldName::VendorPhone => phone_in(&join(words.iter().copied()))
            .map(|p| Value::Text(found(p, spot_of(words), words))),
        FieldName::AccountNumber | FieldName::Reference => words.iter().find_map(|w| {
            let t = clean_id(&w.text);
            t.chars()
                .any(|c| c.is_ascii_digit())
                .then(|| Value::Text(found(t, spot_of(&[*w]), &[*w])))
        }),
        FieldName::Date | FieldName::DueDate => dates::parse(&join(words.iter().copied()), today)
            .map(|p| Value::Date(found(p.date, spot_of(words), words), p.could_be_day_first)),
        FieldName::Terms => text_value(words).map(|f| {
            Value::Terms(Found {
                value: Terms {
                    days: terms_days(&f.value),
                    text: f.value,
                },
                spot: f.spot,
                rule: f.rule,
                confidence: f.confidence,
            })
        }),
        FieldName::Subtotal
        | FieldName::Tax
        | FieldName::Total
        | FieldName::AmountPaid
        | FieldName::BalanceDue
        | FieldName::Fees => words.iter().rev().find_map(|w| {
            amount::parse(&w.text).map(|m| Value::Money(found(m, spot_of(&[*w]), &[*w])))
        }),
        // Payment signs are read by the rules every time.
        FieldName::Payment => None,
    }
}

/// The regions to try for a field: the saved one for an unlabelled field;
/// for a labelled one, the region moved to each line carrying the label,
/// nearest first (none when the label is missing).
fn anchored_regions(layout: &Layout, page: &PageWords, pi: usize, tf: &TemplateField) -> Vec<Bbox> {
    let Some(label) = tf.label.as_deref() else {
        return vec![tf.region];
    };
    let want: Vec<&str> = label.split(' ').filter(|t| !t.is_empty()).collect();
    if want.is_empty() {
        return vec![tf.region];
    }
    let mut occurrences: Vec<Bbox> = Vec::new();
    for line in layout.lines.iter().filter(|l| l.page_index == pi) {
        let words: Vec<&Word> = line.words.iter().map(|&w| &page.words[w]).collect();
        let toks = fields::label_tokens(words.iter().copied());
        let hits = fields::find_labels(&toks);
        for start in 0..toks.len() {
            let end = start + want.len();
            if end > toks.len()
                || !want
                    .iter()
                    .zip(&toks[start..end])
                    .all(|(w, t)| *w == t.text)
            {
                continue;
            }
            let exact = hits.iter().any(|h| h.start == start && h.end == end);
            let free = !hits.iter().any(|h| h.start < end && start < h.end);
            if (exact || free)
                && let Some(b) = union(toks[start..end].iter().map(|t| words[t.word].bbox))
            {
                occurrences.push(b);
            }
        }
    }
    let rcy = i32::from(tf.region.top()) + i32::from(tf.region.bottom());
    let cy = |b: &Bbox| i32::from(b.top()) + i32::from(b.bottom());
    occurrences.sort_by_key(|o| ((cy(o) - rcy).abs(), o.left()));
    occurrences
        .iter()
        .filter_map(|o| {
            let dy = (cy(o) - rcy) / 2;
            let top = i32::from(tf.region.top()) + dy;
            let bottom = i32::from(tf.region.bottom()) + dy;
            Bbox::new(
                tf.region.left(),
                u16::try_from(top).ok()?,
                tf.region.right(),
                u16::try_from(bottom).ok()?,
            )
        })
        .collect()
}
fn found<T>(value: T, spot: Spot, words: &[&Word]) -> Found<T> {
    Found {
        value,
        spot,
        rule: RuleId::new("template"),
        confidence: fields::template_conf(words),
    }
}

/// Address lines: words grouped by line (top), joined with `\n`.
fn address_value(words: &[&Word], page: u16) -> Option<Found<String>> {
    let mut lines: Vec<(u16, Vec<&Word>)> = Vec::new();
    for w in words {
        match lines
            .iter_mut()
            .find(|(top, _)| w.bbox.top().abs_diff(*top) <= w.bbox.height() / 2)
        {
            Some((_, l)) => l.push(w),
            None => lines.push((w.bbox.top(), vec![w])),
        }
    }
    let text: String = lines
        .iter()
        .map(|(_, l)| join(l.iter().copied()))
        .collect::<Vec<_>>()
        .join("\n")
        .chars()
        .take(MAX_TEXT_CHARS)
        .collect();
    let bbox = union(words.iter().map(|w| w.bbox))?;
    Some(found(text, Spot { page, bbox }, words))
}

/// The page's words whose centre lies in `region` grown by the margin, in
/// reading order (top, then left).
fn region_words(page: &PageWords, region: Bbox) -> Vec<&Word> {
    let l = u32::from(region.left().saturating_sub(MARGIN_X)) * 2;
    let r = u32::from(region.right().saturating_add(MARGIN_X)) * 2;
    let t = u32::from(region.top().saturating_sub(MARGIN_Y)) * 2;
    let b = u32::from(region.bottom().saturating_add(MARGIN_Y)) * 2;
    let mut ws: Vec<&Word> = page
        .words
        .iter()
        .filter(|w| {
            let cx = center2_x(w.bbox);
            let cy = u32::from(w.bbox.top()) + u32::from(w.bbox.bottom());
            (l..=r).contains(&cx) && (t..=b).contains(&cy)
        })
        .collect();
    ws.sort_by_key(|w| (w.bbox.top() / 50, w.bbox.left()));
    ws
}

/// A template learned from an accepted reading (the owner's corrected
/// values with the spots of the matching words), of any kind (a vendor's
/// recurring receipts learn one too). `None` without a fingerprint or
/// without any field.
#[must_use]
pub fn learn(reading: &Reading) -> Option<Template> {
    // Regions are learned on the levelled page, as the pipeline applies
    // them (a turned picture's words are levelled before the rules run).
    let slopes: Vec<i32> = reading
        .words
        .iter()
        .map(crate::layout::page_slope)
        .collect();
    let levelled_copy;
    let reading = if slopes.iter().any(|&s| s != 0) {
        let mut r = reading.clone();
        r.words = crate::layout::levelled(&reading.words, &slopes);
        let slope_of = |page: u16| {
            reading
                .words
                .iter()
                .position(|p| p.page == page)
                .map_or(0, |i| slopes[i])
        };
        crate::layout::map_spots(&mut r, |page, b| crate::layout::level(b, slope_of(page)));
        levelled_copy = r;
        &levelled_copy
    } else {
        reading
    };
    let fingerprint = reading.fingerprint.clone()?;
    let h = &reading.header;
    let mut fields: Vec<TemplateField> = Vec::new();
    let mut add = |field: FieldName, spot: Option<Spot>| {
        if let Some(s) = spot {
            fields.push(TemplateField {
                field,
                page: s.page,
                region: s.bbox,
                label: label_left_of(reading, s),
            });
        }
    };
    add(FieldName::Vendor, h.vendor.as_ref().map(|f| f.spot));
    add(
        FieldName::VendorAddress,
        h.vendor_address.as_ref().map(|f| f.spot),
    );
    add(
        FieldName::VendorPhone,
        h.vendor_phone.as_ref().map(|f| f.spot),
    );
    add(
        FieldName::AccountNumber,
        h.account_number.as_ref().map(|f| f.spot),
    );
    add(FieldName::Date, h.date.as_ref().map(|f| f.spot));
    add(FieldName::Reference, h.reference.as_ref().map(|f| f.spot));
    add(FieldName::Terms, h.terms.as_ref().map(|f| f.spot));
    add(FieldName::DueDate, h.due_date.as_ref().map(|f| f.spot));
    add(FieldName::Subtotal, h.subtotal.as_ref().map(|f| f.spot));
    add(FieldName::Tax, h.tax.as_ref().map(|f| f.spot));
    for f in &h.fees {
        add(FieldName::Fees, Some(f.spot));
    }
    add(FieldName::Total, h.total.as_ref().map(|f| f.spot));
    add(
        FieldName::AmountPaid,
        h.amount_paid.as_ref().map(|f| f.spot),
    );
    add(
        FieldName::BalanceDue,
        h.balance_due.as_ref().map(|f| f.spot),
    );
    if fields.is_empty() {
        return None;
    }
    fields.truncate(MAX_TEMPLATE_FIELDS);
    Some(Template {
        fingerprint,
        fields,
        line_table: learn_table(reading),
        samples: 1,
    })
}

/// The label printed left of a value on its line, normalized, if any: the
/// nearest word to the left (at any distance, as across a totals box), then
/// up to two more words directly next to it.
fn label_left_of(reading: &Reading, spot: Spot) -> Option<String> {
    const ADJACENT: u16 = 250;
    let page = reading.words.iter().find(|p| p.page == spot.page)?;
    let cy = u32::from(spot.bbox.top()) + u32::from(spot.bbox.bottom());
    let mut left: Vec<&Word> = page
        .words
        .iter()
        .filter(|w| {
            w.bbox.right() <= spot.bbox.left()
                && u32::from(w.bbox.top()) * 2 <= cy
                && cy <= u32::from(w.bbox.bottom()) * 2
        })
        .collect();
    left.sort_by_key(|w| std::cmp::Reverse(w.bbox.right()));
    let mut picked: Vec<&Word> = Vec::new();
    for w in left {
        if let Some(prev) = picked.last()
            && w.bbox.right().saturating_add(ADJACENT) < prev.bbox.left()
        {
            break;
        }
        if amount::parse(&w.text).is_some() || picked.len() == 3 {
            break;
        }
        picked.push(w);
    }
    picked.reverse();
    let text: String = picked
        .iter()
        .flat_map(|w| fields::canon(&w.text))
        .collect::<Vec<_>>()
        .join(" ");
    (!text.is_empty()).then(|| text.chars().take(MAX_WORD_CHARS).collect())
}
/// Column bands from the lines' value spots; the header is the band one
/// row height above the first line.
fn learn_table(reading: &Reading) -> Option<LineTable> {
    let first = reading.lines.first()?;
    let page = first.amount.spot.page;
    let on_page = reading.lines.iter().filter(|l| l.amount.spot.page == page);
    let mut bands: Vec<ColumnBand> = Vec::new();
    let mut band = |kind: ColumnKind, b: Option<Bbox>| {
        if let Some(b) = b {
            match bands.iter_mut().find(|c| c.kind == kind) {
                Some(c) => {
                    c.left = c.left.min(b.left());
                    c.right = c.right.max(b.right());
                }
                None => bands.push(ColumnBand {
                    kind,
                    left: b.left(),
                    right: b.right(),
                }),
            }
        }
    };
    let mut top = u16::MAX;
    let mut row_h = 0;
    for l in on_page {
        band(
            ColumnKind::Description,
            l.description.as_ref().map(|d| d.spot.bbox),
        );
        band(
            ColumnKind::Quantity,
            l.quantity.as_ref().map(|d| d.spot.bbox),
        );
        band(
            ColumnKind::UnitPrice,
            l.unit_price.as_ref().map(|d| d.spot.bbox),
        );
        band(ColumnKind::Amount, Some(l.amount.spot.bbox));
        top = top.min(l.amount.spot.bbox.top());
        row_h = row_h.max(l.amount.spot.bbox.height());
    }
    bands.sort_by_key(|c| c.left);
    let left = bands.iter().map(|c| c.left).min()?;
    let right = bands.iter().map(|c| c.right).max()?;
    let header = Bbox::new(
        left,
        top.saturating_sub(row_h.saturating_mul(2)),
        right,
        top.saturating_sub(row_h / 2),
    )?;
    Some(LineTable {
        page,
        header,
        columns: bands,
    })
}

/// Fold a new sample into a saved template: tokens both share (the stable
/// anchors), aspect and regions averaged by sample count, fields only one
/// has kept. Integer arithmetic only.
#[must_use]
pub fn merge(saved: &Template, sample: &Template) -> Template {
    let ws = u64::from(saved.samples.max(1));
    let wn = u64::from(sample.samples.max(1));
    let avg = |a: u16, b: u16| -> u16 {
        let v = (u64::from(a) * ws + u64::from(b) * wn + u64::midpoint(ws, wn)) / (ws + wn);
        u16::try_from(v).unwrap_or(Bbox::SCALE).min(Bbox::SCALE)
    };
    let shared: Vec<String> = saved
        .fingerprint
        .tokens
        .iter()
        .filter(|t| sample.fingerprint.tokens.binary_search(t).is_ok())
        .cloned()
        .collect();
    let aspect = u32::try_from(
        (u64::from(saved.fingerprint.aspect) * ws + u64::from(sample.fingerprint.aspect) * wn)
            / (ws + wn),
    )
    .unwrap_or(saved.fingerprint.aspect);
    let fingerprint = Fingerprint {
        tokens: if shared.is_empty() {
            saved.fingerprint.tokens.clone()
        } else {
            shared
        },
        aspect,
    };
    let mut fields: Vec<TemplateField> = Vec::new();
    let mut used = vec![false; sample.fields.len()];
    for f in &saved.fields {
        let partner = sample
            .fields
            .iter()
            .enumerate()
            .find(|(i, s)| !used[*i] && s.field == f.field && s.page == f.page);
        match partner {
            Some((idx, other)) => {
                used[idx] = true;
                let old = f.region;
                let new = other.region;
                let (left, right) = (avg(old.left(), new.left()), avg(old.right(), new.right()));
                let (top, bottom) = (avg(old.top(), new.top()), avg(old.bottom(), new.bottom()));
                let region = Bbox::new(
                    left.min(right),
                    top.min(bottom),
                    left.max(right),
                    top.max(bottom),
                )
                .unwrap_or(old);
                fields.push(TemplateField {
                    field: f.field,
                    page: f.page,
                    region,
                    label: other.label.clone().or_else(|| f.label.clone()),
                });
            }
            None => fields.push(f.clone()),
        }
    }
    fields.extend(
        sample
            .fields
            .iter()
            .zip(&used)
            .filter(|(_, u)| !**u)
            .map(|(f, _)| f.clone()),
    );
    fields.truncate(MAX_TEMPLATE_FIELDS);
    Template {
        fingerprint,
        fields,
        line_table: sample
            .line_table
            .clone()
            .or_else(|| saved.line_table.clone()),
        samples: saved.samples.saturating_add(sample.samples.max(1)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::testdoc::{Doc, invoice};
    use crate::pipeline::{ReadInput, read};
    use crate::protocol::JobHint;
    use proptest::prelude::*;

    fn today() -> CalendarDate {
        CalendarDate::from_ymd(2026, 10, 26).unwrap()
    }

    fn fp(tokens: &[&str], aspect: u32) -> Fingerprint {
        let mut t: Vec<String> = tokens.iter().map(|s| (*s).to_string()).collect();
        t.sort();
        Fingerprint { tokens: t, aspect }
    }

    #[test]
    fn tokens_mask_long_digit_runs() {
        assert_eq!(token("INV-20417").as_deref(), Some("inv-#"));
        assert_eq!(token("(555)").as_deref(), Some("555"));
        assert_eq!(token("Riverbend,").as_deref(), Some("riverbend"));
        assert_eq!(token("--"), None);
    }

    #[test]
    fn similarity_is_jaccard_in_basis_points() {
        let a = fp(&["a", "b", "c", "d"], 12_941);
        let b = fp(&["a", "b", "c", "e"], 12_941);
        assert_eq!(similarity(&a, &b), 6_000);
        assert_eq!(similarity(&a, &a), 10_000);
        let wide = fp(&["a", "b", "c", "d"], 7_727);
        assert_eq!(similarity(&a, &wide), 0, "different page shape");
        let t = |f: Fingerprint| Template {
            fingerprint: f,
            fields: vec![],
            line_table: None,
            samples: 1,
        };
        assert_eq!(
            best_match(&a, &[t(fp(&["x"], 12_941)), t(b.clone()), t(a.clone())]),
            Some(2)
        );
        assert_eq!(
            best_match(&a, &[t(fp(&["a", "x", "y", "z"], 12_941))]),
            None
        );
    }

    #[test]
    fn learn_then_apply_reads_the_next_document() {
        let first = invoice(
            "RB-1042",
            "07/14/2026",
            &[("Tarp", "2", "10.00", "20.00")],
            "20.00",
            "1.65",
            "21.65",
        );
        let pages = first.pages();
        let r = read(&ReadInput {
            pages: &pages,
            page_count: 1,
            unread_pages: &[],
            templates: &[],
            hint: JobHint::default(),
            today: today(),
        });
        let t = learn(&r).unwrap();
        assert!(t.fields.iter().any(|f| f.field == FieldName::Total));
        assert!(t.line_table.is_some());
        let second = invoice(
            "RB-1077",
            "08/02/2026",
            &[("Rope", "1", "5.00", "5.00")],
            "5.00",
            "0.41",
            "5.41",
        );
        let pages2 = second.pages();
        let fp2 = fingerprint(&pages2).unwrap();
        assert!(similarity(&fp2, &t.fingerprint) >= MATCH_BP);
        let h = apply(&t, &pages2, today());
        assert_eq!(
            h.total.unwrap().value,
            cl_core::Money::parse_wire("5.41").unwrap()
        );
        assert_eq!(h.reference.unwrap().value, "RB-1077");
        assert_eq!(
            h.date.unwrap().value,
            CalendarDate::from_ymd(2026, 8, 2).unwrap()
        );

        // Receipts use templates too: one is learned from a card receipt,
        // and the vendor's template reads its next receipt, which stays a
        // card charge (the rules decide the kind).
        let receipt = |reference: &str, total: &str, extra_line: bool| {
            let mut lines = vec![("Tarp", "2", "10.00", "20.00")];
            if extra_line {
                lines.push(("Rope", "1", "5.00", "5.00"));
            }
            let mut d = invoice(reference, "08/02/2026", &lines, "", "", total);
            d.text(500, 9000, "VISA ************4821");
            d.pages()
        };
        let input = |pages: &[PageWords], templates: &[Template]| {
            read(&ReadInput {
                pages,
                page_count: 1,
                unread_pages: &[],
                templates,
                hint: JobHint::default(),
                today: today(),
            })
        };
        let first_receipt = input(&receipt("RB-2001", "20.00", false), &[]);
        assert_eq!(first_receipt.kind, crate::model::DocKind::CardCharge);
        let rt = learn(&first_receipt).expect("a receipt's template");
        // The next receipt is a line longer: its total moved down a row and
        // is still read from the template, next to its label.
        let next = input(
            &receipt("RB-2002", "25.00", true),
            std::slice::from_ref(&rt),
        );
        assert_eq!(next.kind, crate::model::DocKind::CardCharge);
        assert_eq!(next.reader, crate::model::Reader::Template);
        assert_eq!(next.template, Some(0));
        let total = next.header.total.unwrap();
        assert_eq!(total.value, cl_core::Money::parse_wire("25.00").unwrap());
        assert_eq!(total.rule.as_str(), "template");
        assert_eq!(next.header.reference.unwrap().value, "RB-2002");
        // A bill's template applies to the same vendor's card receipt.
        let with = input(
            &receipt("RB-2003", "20.00", false),
            std::slice::from_ref(&t),
        );
        assert_eq!(with.template, Some(0));
        assert_eq!(with.kind, crate::model::DocKind::CardCharge);
    }

    #[test]
    fn invalid_region_values_are_left_for_the_rules() {
        let mut d = Doc::new();
        d.text(6000, 6400, "Total");
        d.text(9000, 6400, "n/a");
        let pages = d.pages();
        let t = Template {
            fingerprint: fp(&["total"], 12_941),
            fields: vec![TemplateField {
                field: FieldName::Total,
                page: 0,
                region: Bbox::new(8800, 6400, 9500, 6520).unwrap(),
                label: Some("total".into()),
            }],
            line_table: None,
            samples: 1,
        };
        assert!(apply(&t, &pages, today()).total.is_none());
    }

    #[test]
    fn merge_averages_by_samples() {
        let f = |l, r| TemplateField {
            field: FieldName::Total,
            page: 0,
            region: Bbox::new(l, 100, r, 200).unwrap(),
            label: None,
        };
        let saved = Template {
            fingerprint: fp(&["a", "b"], 12_941),
            fields: vec![f(1000, 2000)],
            line_table: None,
            samples: 3,
        };
        let sample = Template {
            fingerprint: fp(&["a", "c"], 12_941),
            fields: vec![f(2000, 3000)],
            line_table: None,
            samples: 1,
        };
        let m = merge(&saved, &sample);
        assert_eq!(m.samples, 4);
        assert_eq!(m.fingerprint.tokens, ["a"]);
        assert_eq!(m.fields[0].region, Bbox::new(1250, 100, 2250, 200).unwrap());
    }

    proptest! {
        #[test]
        fn similarity_is_symmetric_and_bounded(
            a in prop::collection::btree_set("[a-e]{1,2}", 0..12),
            b in prop::collection::btree_set("[a-e]{1,2}", 0..12),
        ) {
            let fa = Fingerprint { tokens: a.into_iter().collect(), aspect: 12_941 };
            let fb = Fingerprint { tokens: b.into_iter().collect(), aspect: 12_941 };
            let s = similarity(&fa, &fb);
            prop_assert!(s <= 10_000);
            prop_assert_eq!(s, similarity(&fb, &fa));
        }
    }
}
