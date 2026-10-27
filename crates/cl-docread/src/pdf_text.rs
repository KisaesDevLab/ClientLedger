//! The PDF text layer (WP-B, step 4; feature `worker`): a page's words with
//! positions through `PDFium`, mapped to [`Bbox`] ten-thousandths of the page
//! **as displayed** (crop box and `/Rotate` applied, y down), in reading
//! order; plus a quality score that sends pages with a poor text layer to
//! OCR (R6).
//!
//! Positions go through `PDFium`'s own page-to-device mapping
//! (`FPDF_PageToDevice`, the matrix the viewer renders with), sampled once
//! per page, so a word lands where the viewer draws it whatever the page's
//! crop box origin or rotation.
//!
//! One of the three files exempt from `cargo xtask nofloat` (`PDFium` reports
//! points as floats); it may not mention money types. The crate denies
//! float lints, so this module allows them.
#![allow(
    clippy::float_arithmetic,
    clippy::float_cmp,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use crate::model::{Bbox, Confidence, MAX_WORD_CHARS, MAX_WORDS_PER_PAGE, Word};
use pdfium_render::prelude::{PdfPage, PdfPoints, PdfRect, PdfRenderConfig};

/// Below this many basis points the page is read with OCR instead.
pub const MIN_TEXT_QUALITY_BP: u16 = 5_000;

/// A text layer with fewer words than this counts only in part: a scanned
/// page with a stamped "PAID" or a page number has a few real words over an
/// image that holds the content.
pub const FULL_QUALITY_WORDS: usize = 20;

/// Characters read from one page's text layer at most (beyond that the
/// page is not an invoice; the rest is ignored).
pub const MAX_PAGE_CHARS: usize = 200_000;

/// The device size used to sample the page-to-display mapping: large, so
/// the integer device coordinates `PDFium` returns lose nothing that
/// matters at ten-thousandths of the page.
const SAMPLE: i32 = 1_000_000;

/// One character and its box in ten-thousandths of the displayed page.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CharBox {
    pub ch: char,
    pub left: f64,
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
}

impl CharBox {
    fn height(&self) -> f64 {
        self.bottom - self.top
    }

    fn middle(&self) -> f64 {
        f64::midpoint(self.top, self.bottom)
    }
}

/// The affine map from page points to displayed ten-thousandths, sampled
/// from `PDFium` at three points.
#[derive(Debug, Clone, Copy)]
struct DisplayMap {
    origin: (f64, f64),
    per_x: (f64, f64),
    per_y: (f64, f64),
}

impl DisplayMap {
    fn of(page: &PdfPage<'_>) -> Option<DisplayMap> {
        let config = PdfRenderConfig::new().set_target_size(SAMPLE, SAMPLE);
        // 1000 points apart: far apart enough for precision, any page size.
        let at = |x: f32, y: f32| -> Option<(f64, f64)> {
            let (dx, dy) = page
                .points_to_pixels(PdfPoints::new(x), PdfPoints::new(y), &config)
                .ok()?;
            let scale = f64::from(Bbox::SCALE) / f64::from(SAMPLE);
            Some((f64::from(dx) * scale, f64::from(dy) * scale))
        };
        let o = at(0.0, 0.0)?;
        let x = at(1000.0, 0.0)?;
        let y = at(0.0, 1000.0)?;
        let per = |p: (f64, f64)| ((p.0 - o.0) / 1000.0, (p.1 - o.1) / 1000.0);
        let map = DisplayMap {
            origin: o,
            per_x: per(x),
            per_y: per(y),
        };
        let det = map.per_x.0 * map.per_y.1 - map.per_x.1 * map.per_y.0;
        (det.is_finite() && det != 0.0).then_some(map)
    }

    fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        (
            self.origin.0 + x * self.per_x.0 + y * self.per_y.0,
            self.origin.1 + x * self.per_x.1 + y * self.per_y.1,
        )
    }

    fn char_box(&self, ch: char, r: &PdfRect) -> CharBox {
        let (l, b, rt, t) = (
            f64::from(r.left().value),
            f64::from(r.bottom().value),
            f64::from(r.right().value),
            f64::from(r.top().value),
        );
        let corners = [
            self.apply(l, b),
            self.apply(l, t),
            self.apply(rt, b),
            self.apply(rt, t),
        ];
        let xs = corners.map(|c| c.0);
        let ys = corners.map(|c| c.1);
        CharBox {
            ch,
            left: xs.iter().copied().fold(f64::INFINITY, f64::min),
            right: xs.iter().copied().fold(f64::NEG_INFINITY, f64::max),
            top: ys.iter().copied().fold(f64::INFINITY, f64::min),
            bottom: ys.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        }
    }
}

/// The page's words, in reading order (at most
/// [`MAX_WORDS_PER_PAGE`]). Empty when the page has no text layer.
#[must_use]
pub fn page_words(page: &PdfPage<'_>) -> Vec<Word> {
    let Some(map) = DisplayMap::of(page) else {
        return Vec::new();
    };
    let Ok(text) = page.text() else {
        return Vec::new();
    };
    let chars = text.chars();
    let mut boxes = Vec::new();
    for c in chars.iter().take(MAX_PAGE_CHARS) {
        let Some(ch) = c.unicode_char() else {
            continue;
        };
        if breaks_word(ch) {
            boxes.push(CharBox {
                ch: ' ',
                left: 0.0,
                top: 0.0,
                right: 0.0,
                bottom: 0.0,
            });
            continue;
        }
        if let Ok(r) = c.loose_bounds().or_else(|_| c.tight_bounds()) {
            boxes.push(map.char_box(ch, &r));
        }
    }
    words_from_chars(&boxes)
}

/// A character that ends a word and is never part of one: any space
/// (JavaScript's `\s`, so U+FEFF too), a control character, or an invisible
/// format character (soft hyphen, zero-width and joiner characters,
/// direction marks and controls, the BOM). A format character inside
/// "1<U+FEFF>250.00" must not join two numbers into one (security review
/// L4).
fn breaks_word(c: char) -> bool {
    c.is_whitespace()
        || c.is_control()
        || matches!(
            c,
            '\u{00AD}'
                | '\u{061C}'
                | '\u{180E}'
                | '\u{200B}'..='\u{200F}'
                | '\u{202A}'..='\u{202E}'
                | '\u{2060}'..='\u{2064}'
                | '\u{2066}'..='\u{206F}'
                | '\u{FEFF}'
                | '\u{FFF9}'..='\u{FFFB}'
        )
}

/// Group characters (in content order; `' '` separates) into words and put
/// the words in reading order: lines top to bottom, words left to right.
#[must_use]
pub fn words_from_chars(chars: &[CharBox]) -> Vec<Word> {
    let mut words: Vec<CharBox> = Vec::new();
    let mut texts: Vec<String> = Vec::new();
    let mut open = false;
    let mut last: Option<CharBox> = None;
    for c in chars {
        if breaks_word(c.ch) {
            open = false;
            continue;
        }
        if !(c.left.is_finite() && c.right.is_finite() && c.top.is_finite() && c.bottom.is_finite())
        {
            open = false;
            continue;
        }
        let joins = open && last.is_some_and(|p| continues(&p, c));
        if joins && let (Some(w), Some(t)) = (words.last_mut(), texts.last_mut()) {
            w.left = w.left.min(c.left);
            w.right = w.right.max(c.right);
            w.top = w.top.min(c.top);
            w.bottom = w.bottom.max(c.bottom);
            if t.chars().count() < MAX_WORD_CHARS {
                t.push(c.ch);
            }
        } else {
            if words.len() >= MAX_WORDS_PER_PAGE {
                break;
            }
            words.push(*c);
            texts.push(c.ch.to_string());
        }
        open = true;
        last = Some(*c);
    }
    let mut out: Vec<(CharBox, String)> = words.into_iter().zip(texts).collect();
    reading_order(&mut out);
    out.into_iter()
        .map(|(b, text)| Word {
            text,
            bbox: to_bbox(&b),
            confidence: Confidence::CERTAIN,
        })
        .collect()
}

/// Whether `c` continues the word that ended with `prev`: on the same line
/// (middles within half a character height) and close after it (a gap of
/// at most a quarter of the height, no step backwards).
fn continues(prev: &CharBox, c: &CharBox) -> bool {
    let h = prev.height().max(c.height()).max(1.0);
    let same_line = (prev.middle() - c.middle()).abs() <= h / 2.0;
    let gap = c.left - prev.right;
    same_line && gap <= h * 0.25 && gap >= -h * 0.5
}

/// Sort words into lines (top to bottom; a word joins a line when its
/// middle is within half the line's height), then left to right.
fn reading_order(words: &mut [(CharBox, String)]) {
    words.sort_by(|a, b| a.0.middle().total_cmp(&b.0.middle()));
    let mut line_start = 0;
    let mut line_mid = None::<f64>;
    let mut line_h = 0.0_f64;
    let mut lines: Vec<(usize, usize)> = Vec::new();
    for (i, (b, _)) in words.iter().enumerate() {
        match line_mid {
            Some(m) if (b.middle() - m).abs() <= line_h.max(b.height()) / 2.0 => {}
            _ => {
                if i > line_start {
                    lines.push((line_start, i));
                }
                line_start = i;
                line_mid = Some(b.middle());
                line_h = b.height();
            }
        }
    }
    if words.len() > line_start {
        lines.push((line_start, words.len()));
    }
    for (s, e) in lines {
        words[s..e].sort_by(|a, b| a.0.left.total_cmp(&b.0.left));
    }
}

fn to_bbox(b: &CharBox) -> Bbox {
    let clamp = |v: f64| v.round().clamp(0.0, f64::from(Bbox::SCALE)) as u16;
    let (l, r) = (clamp(b.left), clamp(b.right));
    let (t, bt) = (clamp(b.top), clamp(b.bottom));
    Bbox::new(l.min(r), t.min(bt), l.max(r), t.max(bt)).unwrap_or(Bbox::PAGE)
}

/// Whether a character is one a business document prints: letters and
/// digits of any script, and common punctuation and symbols. Private-use
/// and replacement characters (a font without a usable text map) are not.
fn plausible(c: char) -> bool {
    if c.is_alphanumeric() {
        return !matches!(c, '\u{E000}'..='\u{F8FF}');
    }
    matches!(
        c,
        '.' | ','
            | ':'
            | ';'
            | '-'
            | '/'
            | '#'
            | '$'
            | '%'
            | '&'
            | '('
            | ')'
            | '\''
            | '"'
            | '@'
            | '*'
            | '+'
            | '='
            | '!'
            | '?'
            | '_'
            | '€'
            | '£'
            | '¢'
            | '–'
            | '—'
            | '’'
            | '‘'
            | '“'
            | '”'
            | '·'
            | '•'
    )
}

/// How usable a text layer is, in basis points: the share of letters and
/// digits (outside the private-use area) among the characters, scaled down when the page has fewer than
/// [`FULL_QUALITY_WORDS`] words. 0 for no words.
#[must_use]
pub fn text_quality(words: &[Word]) -> u16 {
    let (mut good, mut all) = (0u64, 0u64);
    for w in words {
        for c in w.text.chars() {
            all += 1;
            // Letters and digits carry the text; a font without a text map
            // often comes out as runs of punctuation (glyph numbers read as
            // `!` `"` `#`…), which plausible alone would accept.
            if plausible(c) && c.is_alphanumeric() {
                good += 1;
            }
        }
    }
    if all == 0 {
        return 0;
    }
    let share = good * u64::from(Bbox::SCALE) / all;
    let n = words.len().min(FULL_QUALITY_WORDS) as u64;
    let bp = share * n / FULL_QUALITY_WORDS as u64;
    u16::try_from(bp).unwrap_or(Bbox::SCALE)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ch(c: char, left: f64, top: f64, w: f64, h: f64) -> CharBox {
        CharBox {
            ch: c,
            left,
            top,
            right: left + w,
            bottom: top + h,
        }
    }

    /// A line of text starting at `left`, one box per character, 100 wide.
    fn line(text: &str, left: f64, top: f64) -> Vec<CharBox> {
        text.chars()
            .enumerate()
            .map(|(i, c)| ch(c, left + i as f64 * 100.0, top, 95.0, 200.0))
            .collect()
    }

    fn texts(words: &[Word]) -> Vec<&str> {
        words.iter().map(|w| w.text.as_str()).collect()
    }

    #[test]
    fn characters_group_into_words_in_reading_order() {
        // Content order: the second line first, then the first.
        let mut chars = line("Total 12.50", 1000.0, 3000.0);
        chars.push(ch(' ', 0.0, 0.0, 0.0, 0.0));
        chars.extend(line("Invoice", 1000.0, 1000.0));
        chars.extend(line("INV-7", 5000.0, 1010.0));
        let words = words_from_chars(&chars);
        assert_eq!(texts(&words), ["Invoice", "INV-7", "Total", "12.50"]);
        let inv = &words[0];
        assert_eq!(inv.bbox, Bbox::new(1000, 1000, 1695, 1200).unwrap());
        assert_eq!(inv.confidence, Confidence::CERTAIN);
    }

    /// Security review L4: an invisible format character between two
    /// numbers splits them; it is never kept in a word.
    #[test]
    fn format_characters_split_words_and_are_dropped() {
        for sep in ['\u{FEFF}', '\u{200B}', '\u{2060}', '\u{00AD}', '\u{202E}'] {
            let text = format!("1{sep}250.00");
            let words = words_from_chars(&line(&text, 0.0, 0.0));
            assert_eq!(texts(&words), ["1", "250.00"], "{sep:?}");
        }
        assert!(!breaks_word('a') && !breaks_word('$') && !breaks_word('-'));
    }

    #[test]
    fn a_wide_gap_or_a_new_line_starts_a_new_word() {
        let mut chars = line("ab", 0.0, 0.0);
        chars.extend(line("cd", 400.0, 0.0)); // gap of 205 > 50
        chars.extend(line("ef", 600.0, 900.0)); // next line
        assert_eq!(texts(&words_from_chars(&chars)), ["ab", "cd", "ef"]);
    }

    #[test]
    fn boxes_are_clamped_to_the_page_and_odd_input_is_skipped() {
        let chars = [
            ch('x', -50.0, -10.0, 100.0, 100.0),
            ch(' ', 0.0, 0.0, 0.0, 0.0),
            ch('y', 9_990.0, 9_990.0, 100.0, 100.0),
            ch(' ', 0.0, 0.0, 0.0, 0.0),
            ch('z', f64::NAN, 0.0, 1.0, 1.0),
            ch('\u{7}', 0.0, 0.0, 1.0, 1.0),
        ];
        let words = words_from_chars(&chars);
        assert_eq!(texts(&words), ["x", "y"]);
        assert_eq!(words[0].bbox, Bbox::new(0, 0, 50, 90).unwrap());
        assert_eq!(
            words[1].bbox,
            Bbox::new(9_990, 9_990, 10_000, 10_000).unwrap()
        );
    }

    #[test]
    fn words_and_their_length_are_capped() {
        let long: String = "a".repeat(MAX_WORD_CHARS + 50);
        let words = words_from_chars(&line(&long, 0.0, 0.0));
        assert_eq!(words.len(), 1);
        assert_eq!(words[0].text.chars().count(), MAX_WORD_CHARS);

        let many: Vec<CharBox> = (0..MAX_WORDS_PER_PAGE + 10)
            .flat_map(|i| {
                [
                    ch(
                        'w',
                        (i % 50) as f64 * 150.0,
                        (i / 50) as f64 * 3.0,
                        100.0,
                        2.0,
                    ),
                    ch(' ', 0.0, 0.0, 0.0, 0.0),
                ]
            })
            .collect();
        assert_eq!(words_from_chars(&many).len(), MAX_WORDS_PER_PAGE);
    }

    fn word(text: &str) -> Word {
        Word {
            text: text.into(),
            bbox: Bbox::PAGE,
            confidence: Confidence::CERTAIN,
        }
    }

    #[test]
    fn quality_rewards_plausible_text_on_a_full_page() {
        assert_eq!(text_quality(&[]), 0);
        let page: Vec<Word> = (0..30).map(|i| word(&format!("Item{i}"))).collect();
        assert_eq!(text_quality(&page), 10_000);
        // Two words over a scan: OCR instead.
        let stamp = [word("PAID"), word("1/2")];
        assert!(text_quality(&stamp) < MIN_TEXT_QUALITY_BP);
        // A font without a text map: private-use glyph codes.
        let garbage: Vec<Word> = (0..30).map(|_| word("\u{E001}\u{E002}\u{FFFD}")).collect();
        assert_eq!(text_quality(&garbage), 0);
        // Glyph numbers read as punctuation (no ToUnicode map).
        let glyphs: Vec<Word> = (0..30).map(|_| word("!\"#$%&'()*")).collect();
        assert!(text_quality(&glyphs) < MIN_TEXT_QUALITY_BP);
        // A real line still scores high.
        let real: Vec<Word> = "Invoice # INV-99476 Date 12/11/2025 Total $734.63 Riverbend Electric Co. Phone (479) 555-0142 Net 30 Description Hours Rate Amount Subtotal 689.79 Tax"
            .split(' ')
            .map(word)
            .collect();
        assert!(text_quality(&real) >= 7_500, "{}", text_quality(&real));
        // Half plausible.
        let half: Vec<Word> = (0..30).map(|_| word("ab\u{E001}\u{E002}")).collect();
        assert_eq!(text_quality(&half), 5_000);
    }
}
