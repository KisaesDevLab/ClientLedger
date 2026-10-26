//! Page layout (WP-A, step 8; plan D3 `layout.rs`): words grouped into
//! text lines, the lines split into segments at wide gaps (a label and
//! its value across a totals box are two segments of one line), segments
//! stacked into blocks (an address, the totals box), and right-aligned
//! numeric columns.
//!
//! Internal to the rules. Integer geometry only: boxes are ten-thousandths
//! of the page's width (x) or height (y); a page's `aspect` (height ÷ width
//! × 10000) converts a height into x units.

use crate::amount;
use crate::model::{Bbox, PageWords, Word};

/// One text line: words on the same baseline, left to right.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextLine {
    /// Index into the input pages slice (not the page number).
    pub page_index: usize,
    /// Indices into that page's `words`, left to right.
    pub words: Vec<usize>,
    pub bbox: Bbox,
    /// Indices into [`Layout::segments`], left to right.
    pub segments: Vec<usize>,
}

/// A run of words on one line with no wide gap inside.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub page_index: usize,
    /// Index into [`Layout::lines`].
    pub line: usize,
    /// Indices into the page's `words`, left to right.
    pub words: Vec<usize>,
    pub bbox: Bbox,
}

/// A block: segments stacked closely above one another that overlap
/// horizontally (an address, a totals box).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub page_index: usize,
    /// Indices into [`Layout::segments`], top to bottom.
    pub segments: Vec<usize>,
    pub bbox: Bbox,
}

/// A right-aligned column of numbers (amounts, quantities).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NumericColumn {
    pub page_index: usize,
    /// The shared right edge.
    pub right: u16,
    /// Indices into [`Layout::segments`] ending in a number on that edge.
    pub segments: Vec<usize>,
}

/// The layout of a document.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Layout {
    pub lines: Vec<TextLine>,
    pub segments: Vec<Segment>,
    pub blocks: Vec<Block>,
    pub numeric_columns: Vec<NumericColumn>,
}

/// Right edges this close (x units) share a numeric column.
const COLUMN_TOLERANCE: u16 = 60;
/// A gap wider than this many tenths of the line height splits a segment.
const GAP_TENTHS: u32 = 15;
/// The smallest gap that can split a segment (x units).
const MIN_GAP: u32 = 60;

impl Layout {
    /// The words of a segment.
    pub fn segment_words<'a>(
        &self,
        pages: &'a [PageWords],
        segment: usize,
    ) -> impl Iterator<Item = &'a Word> + 'a {
        let s = &self.segments[segment];
        let page = &pages[s.page_index];
        s.words.clone().into_iter().map(move |w| &page.words[w])
    }

    /// A segment's words joined with single spaces.
    #[must_use]
    pub fn segment_text(&self, pages: &[PageWords], segment: usize) -> String {
        join(self.segment_words(pages, segment))
    }

    /// The segments of a line, left to right.
    #[must_use]
    pub fn line_segments(&self, line: usize) -> &[usize] {
        &self.lines[line].segments
    }
}

/// Words joined with single spaces.
pub fn join<'a>(words: impl Iterator<Item = &'a Word>) -> String {
    let mut out = String::new();
    for w in words {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&w.text);
    }
    out
}

/// The union of the boxes, or `None` for no boxes.
pub fn union<I: IntoIterator<Item = Bbox>>(boxes: I) -> Option<Bbox> {
    boxes.into_iter().reduce(Bbox::union)
}

/// Twice the vertical centre (avoids halves).
fn center2_y(b: Bbox) -> u32 {
    u32::from(b.top()) + u32::from(b.bottom())
}

/// Twice the horizontal centre.
#[must_use]
pub fn center2_x(b: Bbox) -> u32 {
    u32::from(b.left()) + u32::from(b.right())
}

/// A height in y units as x units on a page of `aspect`.
#[must_use]
pub fn y_to_x(height: u16, aspect: u32) -> u32 {
    let v = u64::from(height) * u64::from(aspect) / 10_000;
    u32::try_from(v).unwrap_or(u32::MAX)
}

/// Group the pages' words.
#[must_use]
pub fn analyze(pages: &[PageWords]) -> Layout {
    let mut layout = Layout::default();
    for (pi, page) in pages.iter().enumerate() {
        let first_line = layout.lines.len();
        group_lines(&mut layout, pi, page);
        let line_range = first_line..layout.lines.len();
        for li in line_range.clone() {
            split_segments(&mut layout, li, page);
        }
        let segs: Vec<usize> = line_range
            .flat_map(|li| layout.lines[li].segments.clone())
            .collect();
        group_blocks(&mut layout, pi, &segs, page.aspect);
        numeric_columns(&mut layout, pi, &segs, page);
    }
    layout
}

fn group_lines(layout: &mut Layout, pi: usize, page: &PageWords) {
    let mut order: Vec<usize> = (0..page.words.len())
        .filter(|&i| !page.words[i].text.trim().is_empty())
        .collect();
    order.sort_by_key(|&i| {
        let b = page.words[i].bbox;
        (center2_y(b), b.left(), i)
    });
    let mut current: Option<(Vec<usize>, Bbox)> = None;
    let mut done: Vec<(Vec<usize>, Bbox)> = Vec::new();
    for i in order {
        let b = page.words[i].bbox;
        let c = center2_y(b);
        let joins = current.as_ref().is_some_and(|(_, lb)| {
            let core_top = u32::from(lb.top()) * 2;
            let core_bottom = u32::from(lb.bottom()) * 2;
            c >= core_top && c <= core_bottom
        });
        if joins {
            if let Some((words, lb)) = current.as_mut() {
                words.push(i);
                *lb = lb.union(b);
            }
        } else if let Some(prev) = current.replace((vec![i], b)) {
            done.push(prev);
        }
    }
    done.extend(current);
    for (mut words, bbox) in done {
        words.sort_by_key(|&i| (page.words[i].bbox.left(), i));
        layout.lines.push(TextLine {
            page_index: pi,
            words,
            bbox,
            segments: Vec::new(),
        });
    }
}

fn split_segments(layout: &mut Layout, li: usize, page: &PageWords) {
    let line = &layout.lines[li];
    let min_h = line
        .words
        .iter()
        .map(|&i| page.words[i].bbox.height())
        .filter(|&h| h > 0)
        .min()
        .unwrap_or(0);
    let gap = (y_to_x(min_h, page.aspect) * GAP_TENTHS / 10).max(MIN_GAP);
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut prev_right: Option<u16> = None;
    for &i in &line.words {
        let b = page.words[i].bbox;
        let split = prev_right.is_some_and(|r| u32::from(b.left()) > u32::from(r) + gap);
        match groups.last_mut() {
            Some(g) if !split => g.push(i),
            _ => groups.push(vec![i]),
        }
        prev_right = Some(prev_right.map_or(b.right(), |r| r.max(b.right())));
    }
    let pi = line.page_index;
    let mut ids = Vec::new();
    for words in groups {
        let bbox = union(words.iter().map(|&i| page.words[i].bbox)).unwrap_or(Bbox::PAGE);
        ids.push(layout.segments.len());
        layout.segments.push(Segment {
            page_index: pi,
            line: li,
            words,
            bbox,
        });
    }
    layout.lines[li].segments = ids;
}

fn group_blocks(layout: &mut Layout, pi: usize, segs: &[usize], _aspect: u32) {
    let mut order = segs.to_vec();
    order.sort_by_key(|&s| {
        let b = layout.segments[s].bbox;
        (b.top(), b.left())
    });
    let first_block = layout.blocks.len();
    for s in order {
        let sb = layout.segments[s].bbox;
        let target = (first_block..layout.blocks.len()).rev().find(|&bi| {
            let block = &layout.blocks[bi];
            let Some(&last) = block.segments.last() else {
                return false;
            };
            let lb = layout.segments[last].bbox;
            let x_overlap = sb.left() <= block.bbox.right() && block.bbox.left() <= sb.right();
            let limit = lb.height().max(sb.height());
            x_overlap && sb.top() > lb.top() && sb.top() <= lb.bottom().saturating_add(limit)
        });
        match target {
            Some(bi) => {
                let block = &mut layout.blocks[bi];
                block.segments.push(s);
                block.bbox = block.bbox.union(sb);
            }
            None => layout.blocks.push(Block {
                page_index: pi,
                segments: vec![s],
                bbox: sb,
            }),
        }
    }
}

fn numeric_columns(layout: &mut Layout, pi: usize, segs: &[usize], page: &PageWords) {
    let mut ends: Vec<(u16, usize)> = segs
        .iter()
        .filter_map(|&s| {
            let seg = &layout.segments[s];
            let &last = seg.words.last()?;
            let w = &page.words[last];
            amount::parse(&w.text).map(|_| (w.bbox.right(), s))
        })
        .collect();
    ends.sort_unstable();
    let mut cols: Vec<NumericColumn> = Vec::new();
    for (right, s) in ends {
        match cols.last_mut() {
            Some(c) if right <= c.right.saturating_add(COLUMN_TOLERANCE) => c.segments.push(s),
            _ => cols.push(NumericColumn {
                page_index: pi,
                right,
                segments: vec![s],
            }),
        }
    }
    layout
        .numeric_columns
        .extend(cols.into_iter().filter(|c| c.segments.len() >= 2));
}

// ---------------------------------------------------------------------------
// Straightening (WP-A2): words of a turned page, levelled for the rules
// ---------------------------------------------------------------------------

/// Slopes searched, y units per 10,000 x units, either way (about 9° on a
/// letter page).
const MAX_SLOPE: i32 = 1_200;
const SLOPE_STEP: i32 = 20;
/// Words used to measure a page's slope.
const SLOPE_WORDS: usize = 3_000;

/// How far a box at `cx2` (twice its centre x) moves up when levelled.
fn shear_dy(cx2: u32, slope: i32) -> i32 {
    let from_middle = i64::from(cx2) - i64::from(Bbox::SCALE);
    i32::try_from(from_middle * i64::from(slope) / 20_000).unwrap_or(0)
}

/// `b` moved vertically by `-dy` (levelled) or `+dy` (back), clamped to
/// the page.
fn shift(b: Bbox, dy: i32) -> Bbox {
    let move_y =
        |v: u16| u16::try_from((i32::from(v) - dy).clamp(0, i32::from(Bbox::SCALE))).unwrap_or(0);
    Bbox::new(b.left(), move_y(b.top()), b.right(), move_y(b.bottom())).unwrap_or(b)
}

/// A box on a page of `slope` levelled (the rules' view).
#[must_use]
pub fn level(b: Bbox, slope: i32) -> Bbox {
    if slope == 0 {
        return b;
    }
    shift(b, shear_dy(center2_x(b), slope))
}

/// A levelled box back where the page shows it.
#[must_use]
pub fn unlevel(b: Bbox, slope: i32) -> Bbox {
    if slope == 0 {
        return b;
    }
    shift(b, -shear_dy(center2_x(b), slope))
}

/// The slope of a page's text lines (y units per 10,000 x units; positive
/// when lines fall to the right): the vertical shear that brings the most
/// word centres onto shared lines. 0 for a level page, or one with too
/// few words to tell. Integer only: a picture turned a few degrees reads
/// with its lines levelled, while every box still maps to the page as
/// shown.
#[must_use]
pub fn page_slope(page: &PageWords) -> i32 {
    let words: Vec<(u32, u32)> = page
        .words
        .iter()
        .filter(|w| w.text.chars().count() >= 2 && w.bbox.height() > 0)
        .take(SLOPE_WORDS)
        .map(|w| (center2_x(w.bbox), center2_y(w.bbox)))
        .collect();
    if words.len() < 8 {
        return 0;
    }
    let mut heights: Vec<u16> = page
        .words
        .iter()
        .take(SLOPE_WORDS)
        .map(|w| w.bbox.height())
        .filter(|&h| h > 0)
        .collect();
    heights.sort_unstable();
    // Centres within a quarter of a line height (doubled units) share a
    // line.
    let reach = i32::from(heights[heights.len() / 2].max(8)) / 2;
    let mut ys: Vec<i32> = Vec::with_capacity(words.len());
    let mut score = |slope: i32| -> u64 {
        ys.clear();
        ys.extend(
            words
                .iter()
                .map(|&(cx2, cy2)| i32::try_from(cy2).unwrap_or(0) - 2 * shear_dy(cx2, slope)),
        );
        ys.sort_unstable();
        // For each centre, how many others sit within `reach` of it.
        let (mut lo, mut total) = (0usize, 0u64);
        let mut hi = 0usize;
        for i in 0..ys.len() {
            while ys[i] - ys[lo] > reach {
                lo += 1;
            }
            while hi + 1 < ys.len() && ys[hi + 1] - ys[i] <= reach {
                hi += 1;
            }
            total += u64::try_from(hi.max(i) - lo).unwrap_or(0);
        }
        total
    };
    let level_score = score(0);
    let (mut best, mut best_score) = (0_i32, level_score);
    let mut s = -MAX_SLOPE;
    while s <= MAX_SLOPE {
        let v = score(s);
        if v > best_score || (v == best_score && s.abs() < best.abs()) {
            best = s;
            best_score = v;
        }
        s += SLOPE_STEP;
    }
    // A clear gain only: a level page stays exactly as read.
    if best_score * 100 < level_score * 115 + 100 {
        return 0;
    }
    best
}

/// The pages with every word levelled by its page's slope.
#[must_use]
pub fn levelled(pages: &[PageWords], slopes: &[i32]) -> Vec<PageWords> {
    pages
        .iter()
        .zip(slopes)
        .map(|(p, &s)| PageWords {
            page: p.page,
            aspect: p.aspect,
            source: p.source,
            words: p
                .words
                .iter()
                .map(|w| crate::model::Word {
                    text: w.text.clone(),
                    bbox: level(w.bbox, s),
                    confidence: w.confidence,
                })
                .collect(),
        })
        .collect()
}

/// Every spot of a reading's fields, lines and checks passed through `f`
/// (page number, box) — words are left alone.
pub fn map_spots(r: &mut crate::model::Reading, f: impl Fn(u16, Bbox) -> Bbox) {
    use crate::model::{Found, Spot};
    fn opt<T>(x: &mut Option<Found<T>>, fix: &impl Fn(&mut Spot)) {
        if let Some(found) = x {
            fix(&mut found.spot);
        }
    }
    let fix = |s: &mut Spot| s.bbox = f(s.page, s.bbox);
    let h = &mut r.header;
    opt(&mut h.vendor, &fix);
    opt(&mut h.vendor_address, &fix);
    opt(&mut h.vendor_phone, &fix);
    opt(&mut h.account_number, &fix);
    opt(&mut h.date, &fix);
    opt(&mut h.reference, &fix);
    opt(&mut h.terms, &fix);
    opt(&mut h.due_date, &fix);
    opt(&mut h.subtotal, &fix);
    opt(&mut h.tax, &fix);
    for fee in &mut h.fees {
        fix(&mut fee.spot);
    }
    opt(&mut h.total, &fix);
    opt(&mut h.amount_paid, &fix);
    opt(&mut h.balance_due, &fix);
    opt(&mut h.payment, &fix);
    for l in &mut r.lines {
        opt(&mut l.description, &fix);
        opt(&mut l.quantity, &fix);
        opt(&mut l.unit_price, &fix);
        fix(&mut l.amount.spot);
    }
    for c in &mut r.checks {
        if let Some(s) = &mut c.spot {
            fix(s);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::testdoc::Doc;
    use proptest::prelude::*;

    #[test]
    fn lines_segments_and_blocks() {
        let mut d = Doc::new();
        d.text(500, 400, "Riverbend Supply Co.");
        d.text(7000, 400, "INVOICE");
        d.text(500, 600, "1200 Mill Road");
        d.text(500, 800, "Springfield, MO 65801");
        d.text(6000, 800, "Invoice #: RB-1042");
        d.text(6000, 6000, "Subtotal");
        d.amount(9500, 6000, "100.00");
        d.text(6000, 6200, "Tax");
        d.amount(9500, 6200, "8.25");
        d.text(6000, 6400, "Total");
        d.amount(9500, 6400, "108.25");
        let pages = d.pages();
        let l = analyze(&pages);
        assert_eq!(l.lines.len(), 6);
        let texts: Vec<String> = (0..l.segments.len())
            .map(|s| l.segment_text(&pages, s))
            .collect();
        assert!(
            texts.contains(&"Riverbend Supply Co.".to_string()),
            "{texts:?}"
        );
        assert!(texts.contains(&"INVOICE".to_string()));
        assert!(
            texts.contains(&"108.25".to_string()),
            "value is its own segment"
        );
        // The letterhead is one block of three segments.
        let first = l
            .blocks
            .iter()
            .find(|b| {
                b.segments
                    .iter()
                    .any(|&s| l.segment_text(&pages, s) == "Riverbend Supply Co.")
            })
            .unwrap();
        assert_eq!(first.segments.len(), 3);
        // The totals column.
        assert_eq!(l.numeric_columns.len(), 1);
        assert_eq!(l.numeric_columns[0].segments.len(), 3);
    }

    /// The test invoice with every word moved down by its distance from
    /// the page's middle times `slope` per 10,000 (a turned picture).
    fn turned(slope: i32) -> Vec<PageWords> {
        let d = crate::pipeline::testdoc::invoice(
            "INV-99476",
            "12/11/2025",
            &[
                ("LED fixture install", "5.25", "70.34", "369.29"),
                ("GFCI receptacles", "4", "72.17", "288.68"),
            ],
            "657.97",
            "42.77",
            "700.74",
        );
        let mut pages = d.pages();
        for w in &mut pages[0].words {
            w.bbox = unlevel(w.bbox, slope);
        }
        pages
    }

    #[test]
    fn a_turned_page_is_levelled_for_the_rules_and_spots_map_back() {
        let level_pages = turned(0);
        assert_eq!(page_slope(&level_pages[0]), 0);
        let input = |pages: &[PageWords]| {
            crate::pipeline::read(&crate::pipeline::ReadInput {
                pages,
                page_count: 1,
                unread_pages: &[],
                templates: &[],
                hint: crate::protocol::JobHint::default(),
                today: cl_core::CalendarDate::from_ymd(2026, 10, 26).unwrap(),
            })
        };
        let flat = input(&level_pages);
        for slope in [-700, 450, 900] {
            let pages = turned(slope);
            let got = page_slope(&pages[0]);
            assert!((got - slope).abs() <= 40, "{slope}: {got}");
            let r = input(&pages);
            let (h, f) = (&r.header, &flat.header);
            assert_eq!(
                h.total.as_ref().map(|x| x.value),
                f.total.as_ref().map(|x| x.value)
            );
            assert_eq!(
                h.reference.as_ref().map(|x| x.value.clone()),
                f.reference.as_ref().map(|x| x.value.clone())
            );
            assert_eq!(
                h.date.as_ref().map(|x| x.value),
                f.date.as_ref().map(|x| x.value)
            );
            assert_eq!(r.lines.len(), flat.lines.len());
            // The total's spot is the turned word's box.
            let word = pages[0].words.iter().find(|w| w.text == "700.74").unwrap();
            let spot = h.total.as_ref().unwrap().spot.bbox;
            assert!(
                spot.top().abs_diff(word.bbox.top()) <= 2,
                "{slope}: {spot:?} {:?}",
                word.bbox
            );
        }
    }

    #[test]
    fn level_and_unlevel_are_inverse() {
        let b = Bbox::new(8_000, 4_000, 9_000, 4_120).unwrap();
        for s in [-1_200, -35, 0, 600] {
            assert_eq!(unlevel(level(b, s), s), b);
        }
        assert_eq!(level(b, 1_000).top(), 4_000 - 350);
    }
    proptest! {
        /// Every word lands in exactly one line and one segment.
        #[test]
        fn every_word_is_placed_once(
            words in prop::collection::vec((0_u16..9_000, 0_u16..9_800, 1_u16..900, 1_u16..150), 0..80)
        ) {
            let mut d = Doc::new();
            for (x, y, w, h) in &words {
                d.word_box(*x, *y, *x + *w, *y + *h, "w");
            }
            let pages = d.pages();
            let l = analyze(&pages);
            let mut seen = vec![0_u8; words.len()];
            for s in &l.segments {
                for &w in &s.words {
                    seen[w] += 1;
                }
            }
            prop_assert!(seen.iter().all(|&n| n == 1));
            let in_lines: usize = l.lines.iter().map(|x| x.words.len()).sum();
            prop_assert_eq!(in_lines, words.len());
        }
    }
}
