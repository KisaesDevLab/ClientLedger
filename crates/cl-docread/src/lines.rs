//! Line items (WP-A, step 10; plan D3 `lines.rs`, S7): the header row sets
//! the column bands; rows run to Subtotal/Total; wrapped descriptions
//! merge. Quantity × unit price is checked with `Money::mul_round(qty, 2)`
//! (the engine's `LINE_AMOUNT_MISMATCH`, `bill.service.ts:229-258`): a
//! failing line keeps its printed amount, drops quantity and unit price,
//! and gains a `LineQuantityTimesPrice` check. A unit price is never kept
//! without a quantity (engine `LINE_QTY_RATE_INCOMPLETE`), nor a quantity
//! of zero or less or a negative price (engine `parseQuantity`,
//! `parseUnitPrice`). Not confident → no lines and a `LinesNotRead` check
//! (one total line to the vendor's default account, Q-285, Q-A03m).
//!
//! Every amount is one page word read by [`amount::parse`].
//!
//! Internal to the rules.

use crate::amount;
use crate::fields::{Label, find_labels, label_tokens};
use crate::layout::{Layout, center2_x, join, union};
use crate::model::{
    Bbox, Check, CheckCode, CheckValue, ColumnBand, ColumnKind, Confidence, Found, Line, LineTable,
    PageWords, RuleId, Spot, Word,
};
use cl_core::{Money, Quantity};

/// The confidence ceiling of a value read from a table column.
const COLUMN: u16 = 9_000;

/// One line-item table on one page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableSpan {
    pub page_index: usize,
    /// The header row's line, when the table was found by its header.
    pub header_line: Option<usize>,
    pub header: Bbox,
    pub columns: Vec<ColumnBand>,
    /// The rows' lines, top to bottom.
    pub rows: Vec<usize>,
}

const COLUMN_WORDS: &[(&[&str], ColumnKind)] = &[
    (&["description"], ColumnKind::Description),
    (&["desc"], ColumnKind::Description),
    (&["item"], ColumnKind::Description),
    (&["items"], ColumnKind::Description),
    (&["item", "description"], ColumnKind::Description),
    (&["product"], ColumnKind::Description),
    (&["products"], ColumnKind::Description),
    (&["product/service"], ColumnKind::Description),
    (&["service"], ColumnKind::Description),
    (&["services"], ColumnKind::Description),
    (&["details"], ColumnKind::Description),
    (&["particulars"], ColumnKind::Description),
    (&["memo"], ColumnKind::Description),
    (&["qty"], ColumnKind::Quantity),
    (&["quantity"], ColumnKind::Quantity),
    (&["units"], ColumnKind::Quantity),
    (&["hrs"], ColumnKind::Quantity),
    (&["hours"], ColumnKind::Quantity),
    (&["count"], ColumnKind::Quantity),
    (&["rate"], ColumnKind::UnitPrice),
    (&["price"], ColumnKind::UnitPrice),
    (&["unit", "price"], ColumnKind::UnitPrice),
    (&["unit", "cost"], ColumnKind::UnitPrice),
    (&["cost"], ColumnKind::UnitPrice),
    (&["each"], ColumnKind::UnitPrice),
    (&["price", "each"], ColumnKind::UnitPrice),
    (&["price/unit"], ColumnKind::UnitPrice),
    (&["amount"], ColumnKind::Amount),
    (&["total"], ColumnKind::Amount),
    (&["line", "total"], ColumnKind::Amount),
    (&["ext"], ColumnKind::Amount),
    (&["extended"], ColumnKind::Amount),
    (&["ext", "price"], ColumnKind::Amount),
    (&["extended", "price"], ColumnKind::Amount),
    (&["sku"], ColumnKind::Other),
    (&["item", "#"], ColumnKind::Other),
    (&["part", "#"], ColumnKind::Other),
    (&["code"], ColumnKind::Other),
    (&["unit"], ColumnKind::Other),
    (&["uom"], ColumnKind::Other),
    (&["date"], ColumnKind::Other),
    (&["tax"], ColumnKind::Other),
];

/// The header row's columns, or `None` when the line is not a header row:
/// at least two column words, one of them an amount or price column and
/// one a description or quantity column, and most of the line made of
/// column words.
fn header_columns(words: &[&Word]) -> Option<Vec<ColumnBand>> {
    let tokens = label_tokens(words.iter().copied());
    let mut cols: Vec<ColumnBand> = Vec::new();
    let mut covered = 0;
    let mut i = 0;
    while i < tokens.len() {
        let best = COLUMN_WORDS
            .iter()
            .filter(|(ws, _)| {
                i + ws.len() <= tokens.len()
                    && ws.iter().zip(&tokens[i..]).all(|(w, t)| *w == t.text)
            })
            .max_by_key(|(ws, _)| ws.len());
        if let Some((ws, kind)) = best {
            let bbox = union(tokens[i..i + ws.len()].iter().map(|t| words[t.word].bbox))?;
            cols.push(ColumnBand {
                kind: *kind,
                left: bbox.left(),
                right: bbox.right(),
            });
            covered += ws.len();
            i += ws.len();
        } else {
            i += 1;
        }
    }
    let has = |k: ColumnKind| cols.iter().any(|c| c.kind == k);
    let priced = has(ColumnKind::Amount) || has(ColumnKind::UnitPrice);
    let described = has(ColumnKind::Description) || has(ColumnKind::Quantity);
    if cols.len() < 2 || !priced || !described || covered * 10 < tokens.len() * 6 {
        return None;
    }
    // One amount column: the rightmost; any other becomes the unit price
    // (if there is none) or is ignored.
    let amount_cols: Vec<usize> = (0..cols.len())
        .filter(|&i| cols[i].kind == ColumnKind::Amount)
        .collect();
    if let Some((&last, rest)) = amount_cols.split_last() {
        let has_price = has(ColumnKind::UnitPrice);
        for (n, &i) in rest.iter().enumerate() {
            cols[i].kind = if !has_price && n + 1 == rest.len() {
                ColumnKind::UnitPrice
            } else {
                ColumnKind::Other
            };
        }
        let _ = last;
    }
    cols.sort_by_key(|c| c.left);
    Some(cols)
}

/// Whether a row line ends the table (a totals label at its start).
fn ends_table(words: &[&Word]) -> bool {
    let tokens = label_tokens(words.iter().copied());
    find_labels(&tokens).first().is_some_and(|h| {
        h.start == 0
            && matches!(
                h.label,
                Label::Total(_)
                    | Label::AmountDue
                    | Label::Balance
                    | Label::Subtotal
                    | Label::Tax
                    | Label::Paid
            )
    })
}

fn line_words<'a>(layout: &Layout, pages: &'a [PageWords], li: usize) -> Vec<&'a Word> {
    let l = &layout.lines[li];
    l.words
        .iter()
        .map(|&w| &pages[l.page_index].words[w])
        .collect()
}

/// The line-item tables: a template's, else every header row found.
#[must_use]
pub fn table_spans(
    layout: &Layout,
    pages: &[PageWords],
    table: Option<&LineTable>,
) -> Vec<TableSpan> {
    if let Some(t) = table
        && let Some(pi) = pages.iter().position(|p| p.page == t.page)
    {
        let mut lines: Vec<usize> = (0..layout.lines.len())
            .filter(|&li| {
                let l = &layout.lines[li];
                l.page_index == pi && l.bbox.top().saturating_add(20) >= t.header.bottom()
            })
            .collect();
        lines.sort_by_key(|&li| layout.lines[li].bbox.top());
        let rows = rows_until_end(layout, pages, &lines);
        if !rows.is_empty() {
            return vec![TableSpan {
                page_index: pi,
                header_line: None,
                header: t.header,
                columns: t.columns.clone(),
                rows,
            }];
        }
    }
    let mut spans = Vec::new();
    for pi in 0..pages.len() {
        let mut lines: Vec<usize> = (0..layout.lines.len())
            .filter(|&li| layout.lines[li].page_index == pi)
            .collect();
        lines.sort_by_key(|&li| layout.lines[li].bbox.top());
        let Some((k, columns)) = lines
            .iter()
            .enumerate()
            .find_map(|(k, &li)| header_columns(&line_words(layout, pages, li)).map(|c| (k, c)))
        else {
            continue;
        };
        let header_line = lines[k];
        let rows = rows_until_end(layout, pages, &lines[k + 1..]);
        spans.push(TableSpan {
            page_index: pi,
            header_line: Some(header_line),
            header: layout.lines[header_line].bbox,
            columns,
            rows,
        });
    }
    spans
}

/// Rows from the top of `lines` until a totals line or a gap of more than
/// four row heights.
fn rows_until_end(layout: &Layout, pages: &[PageWords], lines: &[usize]) -> Vec<usize> {
    let mut rows = Vec::new();
    let mut prev: Option<Bbox> = None;
    for &li in lines {
        let b = layout.lines[li].bbox;
        if ends_table(&line_words(layout, pages, li)) {
            break;
        }
        if let Some(p) = prev
            && b.top()
                > p.bottom()
                    .saturating_add(p.height().max(b.height()).saturating_mul(4))
        {
            break;
        }
        rows.push(li);
        prev = Some(b);
    }
    rows
}

/// Extract the line items.
#[must_use]
pub fn extract(
    layout: &Layout,
    pages: &[PageWords],
    table: Option<&LineTable>,
) -> (Vec<Line>, Vec<Check>) {
    let spans = table_spans(layout, pages, table);
    let mut lines: Vec<Line> = Vec::new();
    for span in &spans {
        read_span(layout, pages, span, &mut lines);
    }
    lines.truncate(crate::model::MAX_LINES);
    if lines.is_empty() {
        return (
            Vec::new(),
            vec![Check {
                code: CheckCode::LinesNotRead,
                rule: RuleId::new("lines.not-read"),
                field: None,
                line: None,
                expected: None,
                found: None,
                difference: None,
                spot: None,
            }],
        );
    }
    let mut checks = Vec::new();
    for (i, line) in lines.iter_mut().enumerate() {
        checks.extend(qty_times_price(line, u16::try_from(i).unwrap_or(u16::MAX)));
    }
    (lines, checks)
}

/// A row's words sorted into columns.
struct RowParts {
    description: Vec<usize>,
    quantity: Option<usize>,
    unit_price: Option<usize>,
    amount: Option<usize>,
}

fn split_row(page: &PageWords, words: &[usize], columns: &[ColumnBand]) -> RowParts {
    let desc_edge = columns
        .iter()
        .filter(|c| c.kind != ColumnKind::Description)
        .map(|c| c.left)
        .min()
        .unwrap_or(u16::MAX);
    let has_amount_col = columns.iter().any(|c| c.kind == ColumnKind::Amount);
    let mut parts = RowParts {
        description: Vec::new(),
        quantity: None,
        unit_price: None,
        amount: None,
    };
    let mut rightmost_number: Option<usize> = None;
    for &w in words {
        let word = &page.words[w];
        let b = word.bbox;
        let numeric = amount::parse(&word.text).is_some();
        if b.right() <= desc_edge.saturating_add(40)
            && (!numeric || b.left() < desc_edge.saturating_sub(200))
        {
            parts.description.push(w);
            continue;
        }
        if !numeric {
            continue;
        }
        rightmost_number = Some(w);
        let nearest = columns
            .iter()
            .filter(|c| c.kind != ColumnKind::Description)
            .min_by_key(|c| {
                let right = u32::from(b.right().abs_diff(c.right));
                let center = center2_x(b).abs_diff(u32::from(c.left) + u32::from(c.right)) / 2;
                right.min(center)
            });
        match nearest.map(|c| c.kind) {
            Some(ColumnKind::Quantity) => parts.quantity = Some(w),
            Some(ColumnKind::UnitPrice) => parts.unit_price = Some(w),
            Some(ColumnKind::Amount) => parts.amount = Some(w),
            _ => {}
        }
    }
    if !has_amount_col && parts.amount.is_none() {
        parts.amount = rightmost_number;
        if parts.unit_price == rightmost_number {
            parts.unit_price = None;
        }
    }
    parts
}

fn found_word<T>(page: &PageWords, w: usize, value: T) -> Found<T> {
    let word = &page.words[w];
    Found {
        value,
        spot: Spot {
            page: page.page,
            bbox: word.bbox,
        },
        rule: RuleId::new("lines.column"),
        confidence: Confidence::saturating(u32::from(word.confidence.bp().min(COLUMN))),
    }
}

fn found_text(page: &PageWords, words: &[usize]) -> Option<Found<String>> {
    let first = *words.first()?;
    let _ = first;
    let text: String = join(words.iter().map(|&w| &page.words[w]))
        .chars()
        .take(crate::model::MAX_TEXT_CHARS)
        .collect();
    let bbox = union(words.iter().map(|&w| page.words[w].bbox))?;
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
        rule: RuleId::new("lines.description"),
        confidence: Confidence::saturating(u32::from(bp)),
    })
}

fn read_span(layout: &Layout, pages: &[PageWords], span: &TableSpan, out: &mut Vec<Line>) {
    let page = &pages[span.page_index];
    let mut pending: Vec<usize> = Vec::new();
    let mut last_row: Option<Bbox> = None;
    for &li in &span.rows {
        let row = &layout.lines[li];
        let parts = split_row(page, &row.words, &span.columns);
        let amount = parts
            .amount
            .and_then(|w| amount::parse(&page.words[w].text).map(|m| (w, m)));
        if let Some((aw, m)) = amount {
            let mut desc = std::mem::take(&mut pending);
            desc.extend(&parts.description);
            out.push(Line {
                description: found_text(page, &desc),
                quantity: parts.quantity.and_then(|w| {
                    amount::parse_quantity(&page.words[w].text).map(|q| found_word(page, w, q))
                }),
                unit_price: parts.unit_price.and_then(|w| {
                    amount::parse(&page.words[w].text).map(|p| found_word(page, w, p))
                }),
                amount: found_word(page, aw, m),
            });
            last_row = Some(row.bbox);
        } else if !parts.description.is_empty() {
            // A wrapped description: joins the line above when close,
            // else starts the next line's description.
            let close = last_row.is_some_and(|p| {
                row.bbox.top()
                    <= p.bottom()
                        .saturating_add(p.height().max(row.bbox.height()) * 3 / 2)
            });
            match out.last_mut() {
                Some(prev) if close && pending.is_empty() => {
                    let mut text = prev
                        .description
                        .as_ref()
                        .map(|d| d.value.clone())
                        .unwrap_or_default();
                    let extra = join(parts.description.iter().map(|&w| &page.words[w]));
                    if !text.is_empty() {
                        text.push(' ');
                    }
                    text.push_str(&extra);
                    let extra_box = union(parts.description.iter().map(|&w| page.words[w].bbox));
                    match prev.description.as_mut() {
                        Some(d) => {
                            d.value = text.chars().take(crate::model::MAX_TEXT_CHARS).collect();
                            if let Some(eb) = extra_box {
                                d.spot.bbox = d.spot.bbox.union(eb);
                            }
                        }
                        None => prev.description = found_text(page, &parts.description),
                    }
                    last_row = Some(row.bbox);
                }
                _ => pending.extend(&parts.description),
            }
        }
    }
}

/// The engine's line rule (S7) on a read line: a price needs a quantity;
/// a quantity must be above zero and a price not below; quantity × price
/// rounded half away from zero to cents must be the printed amount. A line
/// failing the product keeps its amount, loses quantity and price, and
/// gets a check. The amount is never changed.
#[must_use]
pub fn qty_times_price(line: &mut Line, index: u16) -> Option<Check> {
    if line
        .quantity
        .as_ref()
        .is_some_and(|q| q.value <= Quantity::ZERO)
        || line
            .unit_price
            .as_ref()
            .is_some_and(|p| p.value.is_negative())
    {
        line.quantity = None;
        line.unit_price = None;
        return None;
    }
    let (q, p) = match (&line.quantity, &line.unit_price) {
        (Some(q), Some(p)) => (q.value, p.value),
        (None, Some(_)) => {
            line.unit_price = None;
            return None;
        }
        _ => return None,
    };
    let expected = p.mul_round(q, 2).ok();
    if expected == Some(line.amount.value) {
        return None;
    }
    line.quantity = None;
    line.unit_price = None;
    Some(Check {
        code: CheckCode::LineQuantityTimesPrice,
        rule: RuleId::new("lines.qty-times-rate"),
        field: None,
        line: Some(index),
        expected: expected.map(CheckValue::Money),
        found: Some(CheckValue::Money(line.amount.value)),
        difference: expected.and_then(|e| line.amount.value.checked_sub(e).ok()),
        spot: Some(line.amount.spot),
    })
}

/// Sum of the lines' amounts (`None` on overflow).
#[must_use]
pub fn sum(lines: &[Line]) -> Option<Money> {
    Money::sum(lines.iter().map(|l| l.amount.value)).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::analyze;
    use crate::pipeline::testdoc::Doc;
    use proptest::prelude::*;

    fn money(s: &str) -> Money {
        Money::parse_wire(s).unwrap()
    }

    fn table_doc() -> Doc {
        let mut d = Doc::new();
        d.text(500, 3000, "Description");
        d.text(5500, 3000, "Qty");
        d.text(7000, 3000, "Rate");
        d.text(8900, 3000, "Amount");
        d
    }

    #[test]
    fn reads_rows_wraps_and_stops_at_subtotal() {
        let mut d = table_doc();
        d.text(500, 3200, "Canvas tarp 12x16");
        d.amount(5800, 3200, "12");
        d.amount(7600, 3200, "24.50");
        d.amount(9500, 3200, "294.00");
        d.text(500, 3350, "heavy duty, brown");
        d.text(500, 3500, "Rope");
        d.amount(5800, 3500, "2");
        d.amount(7600, 3500, "5.00");
        d.amount(9500, 3500, "10.00");
        d.text(6000, 3800, "Subtotal");
        d.amount(9500, 3800, "304.00");
        let pages = d.pages();
        let (lines, checks) = extract(&analyze(&pages), &pages, None);
        assert!(checks.is_empty(), "{checks:?}");
        assert_eq!(lines.len(), 2);
        assert_eq!(
            lines[0].description.as_ref().unwrap().value,
            "Canvas tarp 12x16 heavy duty, brown"
        );
        assert_eq!(
            lines[0].quantity.as_ref().unwrap().value,
            Quantity::parse_wire("12").unwrap()
        );
        assert_eq!(lines[0].unit_price.as_ref().unwrap().value, money("24.50"));
        assert_eq!(lines[0].amount.value, money("294.00"));
        assert_eq!(lines[1].amount.value, money("10.00"));
        assert_eq!(sum(&lines), Some(money("304")));
    }

    /// T12: 10.512 × 3.459 = 36.361…, printed 36.36, passes; printed 36.37
    /// keeps 36.37 and drops quantity and rate with a check.
    #[test]
    fn qty_times_rate_matches_the_engine_rule() {
        let mut d = table_doc();
        d.text(500, 3200, "Gravel");
        d.amount(5800, 3200, "3.459");
        d.amount(7600, 3200, "10.512");
        d.amount(9500, 3200, "36.36");
        d.text(500, 3400, "Sand");
        d.amount(5800, 3400, "3.459");
        d.amount(7600, 3400, "10.512");
        d.amount(9500, 3400, "36.37");
        let pages = d.pages();
        let (lines, checks) = extract(&analyze(&pages), &pages, None);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].quantity.is_some() && lines[0].unit_price.is_some());
        assert_eq!(lines[1].amount.value, money("36.37"), "printed amount kept");
        assert!(lines[1].quantity.is_none() && lines[1].unit_price.is_none());
        assert_eq!(checks.len(), 1);
        let c = &checks[0];
        assert_eq!(c.code, CheckCode::LineQuantityTimesPrice);
        assert_eq!(c.line, Some(1));
        assert!(matches!(c.expected, Some(CheckValue::Money(m)) if m == money("36.36")));
        assert_eq!(c.difference, Some(money("0.01")));
    }

    fn found<T>(value: T) -> Found<T> {
        Found {
            value,
            spot: Spot {
                page: 0,
                bbox: Bbox::PAGE,
            },
            rule: RuleId::new("t"),
            confidence: Confidence::HIGH,
        }
    }

    fn mk_line(qty: Quantity, price: Money, amt: Money) -> Line {
        Line {
            description: None,
            quantity: Some(found(qty)),
            unit_price: Some(found(price)),
            amount: found(amt),
        }
    }
    #[test]
    fn half_cent_rounds_away_from_zero() {
        // 2.5 × 0.01 = 0.025 → 0.03.
        let q = Quantity::parse_wire("2.5").unwrap();
        assert!(qty_times_price(&mut mk_line(q, money("0.01"), money("0.03")), 0).is_none());
        assert!(qty_times_price(&mut mk_line(q, money("0.01"), money("0.02")), 0).is_some());
    }
    #[test]
    fn price_without_quantity_is_dropped() {
        let mut d = Doc::new();
        d.text(500, 3000, "Item");
        d.text(7000, 3000, "Price");
        d.text(8900, 3000, "Total");
        d.text(500, 3200, "Widget");
        d.amount(7600, 3200, "4.00");
        d.amount(9500, 3200, "8.00");
        let pages = d.pages();
        let (lines, checks) = extract(&analyze(&pages), &pages, None);
        assert_eq!(checks.len(), 0);
        assert_eq!(lines.len(), 1);
        assert!(
            lines[0].unit_price.is_none(),
            "engine LINE_QTY_RATE_INCOMPLETE"
        );
        assert_eq!(lines[0].amount.value, money("8.00"));
    }

    #[test]
    fn no_table_means_lines_not_read() {
        let mut d = Doc::new();
        d.text(500, 500, "Corner Cafe");
        d.text(500, 3000, "Total");
        d.amount(9500, 3000, "12.40");
        let pages = d.pages();
        let (lines, checks) = extract(&analyze(&pages), &pages, None);
        assert_eq!(lines.len(), 0);
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].code, CheckCode::LinesNotRead);
    }

    #[test]
    fn a_template_table_reads_without_a_header_row() {
        let mut d = Doc::new();
        d.text(500, 3200, "Widget");
        d.amount(5800, 3200, "2");
        d.amount(7600, 3200, "4.00");
        d.amount(9500, 3200, "8.00");
        let pages = d.pages();
        let table = LineTable {
            page: 0,
            header: Bbox::new(400, 2950, 9600, 3150).unwrap(),
            columns: vec![
                ColumnBand {
                    kind: ColumnKind::Description,
                    left: 400,
                    right: 5000,
                },
                ColumnBand {
                    kind: ColumnKind::Quantity,
                    left: 5600,
                    right: 5800,
                },
                ColumnBand {
                    kind: ColumnKind::UnitPrice,
                    left: 7200,
                    right: 7600,
                },
                ColumnBand {
                    kind: ColumnKind::Amount,
                    left: 9100,
                    right: 9500,
                },
            ],
        };
        let (lines, checks) = extract(&analyze(&pages), &pages, Some(&table));
        assert!(checks.is_empty(), "{checks:?}");
        assert_eq!(lines.len(), 1);
        assert!(lines[0].quantity.is_some());
    }

    proptest! {
        /// S7 parity: the check passes exactly when the printed amount is
        /// quantity × price rounded half away from zero to cents, and the
        /// amount is never changed.
        #[test]
        fn qty_rate_parity(q in 1_i128..100_000_000, p in 0_i128..10_000_000, delta in -2_i128..=2) {
            let qty = Quantity::from_ten_thousandths(q).unwrap();
            let price = Money::from_ten_thousandths(p).unwrap();
            let exact = q * p; // scale 8
            let cents = (exact + 500_000) / 1_000_000; // half up, non-negative
            let printed = Money::from_ten_thousandths((cents + delta) * 100).unwrap();
            let mut line = mk_line(qty, price, printed);
            let check = qty_times_price(&mut line, 0);
            prop_assert_eq!(line.amount.value, printed);
            prop_assert_eq!(check.is_none(), delta == 0);
            prop_assert_eq!(line.quantity.is_some(), delta == 0);
        }
    }
}
