//! Tesseract TSV parsing (WP-B, step 5; plan D3 "Float exemption", T15).
//! Scanned by `cargo xtask nofloat`: confidence arrives as a decimal string
//! (`"91.734"`) and becomes basis points with integer arithmetic. Garbage,
//! huge or negative input must stay in range or be refused, never panic.
//!
//! **Consumer:** the worker; the signatures here are fixed for WP-B's own
//! use (no other work package calls them).
//!
//! The format (`-c tessedit_create_tsv=1`): a header line, then one row per
//! page, block, paragraph, line and word with 12 tab-separated columns:
//! `level page_num block_num par_num line_num word_num left top width
//! height conf text`. Only level 5 (word) rows with text are kept.

use crate::model::{Confidence, MAX_WORD_CHARS, MAX_WORDS_PER_PAGE};
use std::fmt;

/// The header tesseract writes (the first 11 columns are enough).
const HEADER: &str =
    "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf";

/// A word's box in the **OCR image's** pixels (before mapping back to the
/// original page with `ocr_geom`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelBox {
    pub left: u32,
    pub top: u32,
    pub width: u32,
    pub height: u32,
}

/// One level-5 (word) row of the TSV.
#[derive(Clone, PartialEq, Eq)]
pub struct OcrWord {
    pub text: String,
    pub px: PixelBox,
    pub confidence: Confidence,
    /// Tesseract's block, paragraph and line numbers (layout hints).
    pub block: u32,
    pub paragraph: u32,
    pub line: u32,
}

impl fmt::Debug for OcrWord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OcrWord")
            .field("chars", &self.text.chars().count())
            .field("px", &self.px)
            .field("confidence", &self.confidence)
            .finish_non_exhaustive()
    }
}

/// The TSV could not be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TsvError {
    #[error("not tesseract TSV output")]
    NotTsv,
    #[error("too many words")]
    TooManyWords,
}

/// The words of a page image `width` × `height` pixels. Boxes are clamped
/// to the image; malformed rows are skipped; text is cut at
/// [`MAX_WORD_CHARS`] characters with control characters removed.
///
/// # Errors
/// No TSV header ([`TsvError::NotTsv`]), or more than
/// [`MAX_WORDS_PER_PAGE`] words.
pub fn parse(tsv: &str, width: u32, height: u32) -> Result<Vec<OcrWord>, TsvError> {
    let mut lines = tsv.lines();
    let header = lines.next().ok_or(TsvError::NotTsv)?;
    if !header.starts_with(HEADER) {
        return Err(TsvError::NotTsv);
    }
    let mut out = Vec::new();
    for line in lines {
        let cols: Vec<&str> = line.splitn(12, '\t').collect();
        let [level, _, block, par, ln, _, left, top, w, h, conf, text] = cols[..] else {
            continue;
        };
        if level.trim() != "5" {
            continue;
        }
        let text: String = text
            .trim()
            .chars()
            .filter(|c| !c.is_control())
            .take(MAX_WORD_CHARS)
            .collect();
        if text.is_empty() {
            continue;
        }
        let Some(px) = pixel_box(left, top, w, h, width, height) else {
            continue;
        };
        if out.len() == MAX_WORDS_PER_PAGE {
            return Err(TsvError::TooManyWords);
        }
        out.push(OcrWord {
            text,
            px,
            confidence: confidence(conf),
            block: number(block),
            paragraph: number(par),
            line: number(ln),
        });
    }
    Ok(out)
}

/// A non-negative decimal integer, or 0.
fn number(s: &str) -> u32 {
    s.trim().parse().unwrap_or(0)
}

/// A signed decimal integer (a box can start left of the image), or `None`.
fn signed(s: &str) -> Option<i64> {
    let t = s.trim();
    if t.len() > 12 {
        return None;
    }
    t.parse().ok()
}

/// The box clamped to the `width` × `height` image; `None` when unparsable.
fn pixel_box(left: &str, top: &str, w: &str, h: &str, width: u32, height: u32) -> Option<PixelBox> {
    let (l, t, bw, bh) = (signed(left)?, signed(top)?, signed(w)?, signed(h)?);
    let clamp = |v: i64, max: u32| u32::try_from(v.clamp(0, i64::from(max))).unwrap_or(0);
    let (x0, y0) = (clamp(l, width), clamp(t, height));
    let (x1, y1) = (
        clamp(l.saturating_add(bw.max(0)), width),
        clamp(t.saturating_add(bh.max(0)), height),
    );
    Some(PixelBox {
        left: x0,
        top: y0,
        width: x1.saturating_sub(x0),
        height: y1.saturating_sub(y0),
    })
}

/// Tesseract's confidence (`"91.734"`, `"96"`, `"-1"`) in basis points:
/// integer part × 100 plus the first two decimals, clamped to 0..=10000.
/// Anything else is [`Confidence::NONE`].
#[must_use]
pub fn confidence(s: &str) -> Confidence {
    let t = s.trim();
    let (int, frac) = t.split_once('.').unwrap_or((t, ""));
    if int.is_empty() || !int.bytes().all(|b| b.is_ascii_digit()) {
        return Confidence::NONE;
    }
    if !frac.bytes().all(|b| b.is_ascii_digit()) {
        return Confidence::NONE;
    }
    // More than three digits is over 100 % anyway.
    let whole: u32 = if int.len() > 3 {
        1_000
    } else {
        int.parse().unwrap_or(0)
    };
    let mut hundredths = 0u32;
    for (i, b) in frac.bytes().take(2).enumerate() {
        let d = u32::from(b - b'0');
        hundredths += if i == 0 { d * 10 } else { d };
    }
    Confidence::saturating(whole * 100 + hundredths)
}

/// Tesseract's orientation report (`--psm 0`): the clockwise turn the page
/// needs (0, 90, 180 or 270) and the orientation confidence in
/// hundredths. `None` when the report lacks either.
#[must_use]
pub fn parse_orientation(report: &str) -> Option<(u16, u32)> {
    let mut rotate = None;
    let mut confidence = None;
    for line in report.lines().take(32) {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "Rotate" => {
                rotate = value
                    .parse::<u16>()
                    .ok()
                    .filter(|r| matches!(r, 0 | 90 | 180 | 270));
            }
            "Orientation confidence" => confidence = centi(value),
            _ => {}
        }
    }
    Some((rotate?, confidence?))
}

/// A non-negative decimal string in hundredths, truncated ("3.26" → 326).
fn centi(s: &str) -> Option<u32> {
    let (int, frac) = s.split_once('.').unwrap_or((s, ""));
    if int.is_empty() || int.len() > 6 || !int.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if !frac.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let mut f: String = frac.chars().take(2).collect();
    while f.len() < 2 {
        f.push('0');
    }
    Some(int.parse::<u32>().ok()? * 100 + f.parse::<u32>().ok()?)
}
#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const HEAD: &str = "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n";

    #[test]
    fn word_rows_become_words() {
        let tsv = format!(
            "{HEAD}1\t1\t0\t0\t0\t0\t0\t0\t2550\t3300\t-1\t\n\
             4\t1\t1\t1\t1\t0\t100\t200\t500\t40\t-1\t\n\
             5\t1\t1\t1\t1\t1\t100\t200\t180\t40\t96.734\tInvoice\n\
             5\t1\t1\t1\t1\t2\t300\t200\t200\t40\t91\tINV-1001\r\n\
             5\t1\t2\t1\t3\t1\t10\t10\t10\t10\t95\t   \n"
        );
        let words = parse(&tsv, 2550, 3300).unwrap();
        assert_eq!(words.len(), 2);
        assert_eq!(words[0].text, "Invoice");
        assert_eq!(
            words[0].px,
            PixelBox {
                left: 100,
                top: 200,
                width: 180,
                height: 40
            }
        );
        assert_eq!(words[0].confidence.bp(), 9_673);
        assert_eq!(
            (words[0].block, words[0].paragraph, words[0].line),
            (1, 1, 1)
        );
        assert_eq!(words[1].text, "INV-1001");
        assert_eq!(words[1].confidence.bp(), 9_100);
    }

    #[test]
    fn confidence_strings_become_basis_points() {
        for (s, bp) in [
            ("96.734", 9_673),
            ("96", 9_600),
            ("0.5", 50),
            ("100", 10_000),
            ("100.00", 10_000),
            ("250", 10_000),
            ("99999999999999999999", 10_000),
            ("-1", 0),
            ("-0.5", 0),
            ("", 0),
            ("abc", 0),
            ("9.x", 0),
            (".5", 0),
            ("NaN", 0),
        ] {
            assert_eq!(confidence(s).bp(), bp, "{s}");
        }
    }

    #[test]
    fn boxes_are_clamped_to_the_image() {
        let tsv = format!(
            "{HEAD}5\t1\t1\t1\t1\t1\t-20\t-5\t100\t30\t90\ta\n5\t1\t1\t1\t1\t2\t90\t90\t50\t50\t90\tb\n"
        );
        let words = parse(&tsv, 100, 100).unwrap();
        assert_eq!(
            words[0].px,
            PixelBox {
                left: 0,
                top: 0,
                width: 80,
                height: 25
            }
        );
        assert_eq!(
            words[1].px,
            PixelBox {
                left: 90,
                top: 90,
                width: 10,
                height: 10
            }
        );
    }

    #[test]
    fn garbage_is_refused_or_skipped() {
        assert_eq!(parse("", 10, 10), Err(TsvError::NotTsv));
        assert_eq!(parse("hello\nworld", 10, 10), Err(TsvError::NotTsv));
        let tsv = format!("{HEAD}5\tx\n5\t1\t1\t1\t1\t1\tx\t0\t1\t1\t90\ta\n");
        assert_eq!(parse(&tsv, 10, 10), Ok(Vec::new()));
        let long = "w".repeat(MAX_WORD_CHARS + 9);
        let tsv = format!("{HEAD}5\t1\t1\t1\t1\t1\t0\t0\t1\t1\t90\t{long}\u{7}\n");
        assert_eq!(parse(&tsv, 10, 10).unwrap()[0].text.len(), MAX_WORD_CHARS);
    }

    #[test]
    fn too_many_words_is_an_error() {
        let mut tsv = HEAD.to_string();
        for _ in 0..=MAX_WORDS_PER_PAGE {
            tsv.push_str("5\t1\t1\t1\t1\t1\t0\t0\t1\t1\t90\tw\n");
        }
        assert_eq!(parse(&tsv, 10, 10), Err(TsvError::TooManyWords));
    }

    proptest! {
        // T15: whatever the row holds, every word is inside the image and
        // its confidence in range; nothing panics.
        #[test]
        fn any_row_stays_in_range(
            cols in proptest::collection::vec("[-0-9a-z.]{0,25}", 12),
            w in 1u32..5000,
            h in 1u32..5000,
        ) {
            let mut row = cols.join("\t");
            row.replace_range(0..0, "5\t");
            let tsv = format!("{HEAD}{row}\n");
            if let Ok(words) = parse(&tsv, w, h) {
                for word in words {
                    prop_assert!(word.px.left + word.px.width <= w);
                    prop_assert!(word.px.top + word.px.height <= h);
                    prop_assert!(word.confidence.bp() <= 10_000);
                    prop_assert!(word.text.chars().count() <= MAX_WORD_CHARS);
                }
            }
        }

        #[test]
        fn any_confidence_string_is_in_range(s in "\\PC{0,30}") {
            prop_assert!(confidence(&s).bp() <= 10_000);
        }
    }

    #[test]
    fn orientation_reports() {
        let report = "Page number: 0\nOrientation in degrees: 90\nRotate: 270\nOrientation confidence: 3.26\nScript: Latin\nScript confidence: 0.71\n";
        assert_eq!(parse_orientation(report), Some((270, 326)));
        assert_eq!(
            parse_orientation("Rotate: 0\nOrientation confidence: 12\n"),
            Some((0, 1200))
        );
        assert_eq!(
            parse_orientation("Rotate: 45\nOrientation confidence: 9.1\n"),
            None
        );
        assert_eq!(parse_orientation("Rotate: 90\n"), None);
        assert_eq!(parse_orientation("garbage"), None);
        assert_eq!(centi("-1"), None);
    }
}
