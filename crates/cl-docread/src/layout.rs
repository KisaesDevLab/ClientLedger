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
