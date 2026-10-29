//! Layout specs in millimetres on US Letter, and their resolution into the
//! exact text runs and shapes the PDF draws.
//!
//! Two stages: a [`Document`] holds boxes (where a field goes, how it
//! aligns, wraps, clips or fills); [`resolve`] turns each text box into
//! [`Run`]s — the final string and its baseline origin — using the
//! embedded fonts' advance widths. The PDF renderer ([`crate::pdf`]) draws
//! exactly those runs, one text matrix per run, so a test on the resolved
//! layout is a test on the printed page.
//!
//! Coordinates: millimetres from the page's top-left corner, `y` growing
//! down. Floating point is used for geometry only.

use crate::PrintError;
use crate::fonts::Font;
use crate::image::Logo;

/// US Letter width (8.5 in).
pub const LETTER_W_MM: f32 = 215.9;
/// US Letter height (11 in).
pub const LETTER_H_MM: f32 = 279.4;
/// Line height as a multiple of the font size.
pub const LINE_HEIGHT: f32 = 1.2;

/// Horizontal alignment inside a text box.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Align {
    /// Flush left.
    #[default]
    Left,
    /// Centred.
    Center,
    /// Flush right (amounts).
    Right,
}

/// A rectangle in millimetres.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    /// Left edge.
    pub x: f32,
    /// Top edge.
    pub y: f32,
    /// Width.
    pub w: f32,
    /// Height.
    pub h: f32,
}

impl Rect {
    /// A rectangle from its top-left corner and size.
    #[must_use]
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }

    /// The same rectangle moved by (`dx`, `dy`).
    #[must_use]
    pub fn moved(self, dx: f32, dy: f32) -> Rect {
        Rect {
            x: self.x + dx,
            y: self.y + dy,
            ..self
        }
    }

    /// Right edge.
    #[must_use]
    pub fn right(self) -> f32 {
        self.x + self.w
    }

    /// Bottom edge.
    #[must_use]
    pub fn bottom(self) -> f32 {
        self.y + self.h
    }

    /// True when `other` lies entirely inside `self`.
    #[must_use]
    pub fn contains(self, other: Rect) -> bool {
        other.x >= self.x
            && other.y >= self.y
            && other.right() <= self.right()
            && other.bottom() <= self.bottom()
    }
}

/// A text field.
#[derive(Debug, Clone, PartialEq)]
pub struct TextBox {
    /// What the box holds (`"check1.payee"`); tests and the alignment page
    /// find fields by it. Never shown.
    pub tag: String,
    /// Where the text may go. The first baseline sits one ascent below the
    /// top.
    pub rect: Rect,
    /// The face.
    pub font: Font,
    /// Size in points.
    pub size_pt: f32,
    /// Horizontal alignment.
    pub align: Align,
    /// Break at spaces to fit the width (explicit `\n` always breaks).
    pub wrap: bool,
    /// Drop what does not fit: characters past the width, lines past the
    /// height.
    pub clip: bool,
    /// Protective fill (checks): repeat this character from the text to
    /// the far edge of the box — after the text when left-aligned, before
    /// it when right-aligned. Single-line boxes only.
    pub fill: Option<char>,
    /// The text.
    pub text: String,
}

impl TextBox {
    /// A left-aligned, single-line, clipped box.
    #[must_use]
    pub fn new(
        tag: impl Into<String>,
        rect: Rect,
        font: Font,
        size_pt: f32,
        text: impl Into<String>,
    ) -> TextBox {
        TextBox {
            tag: tag.into(),
            rect,
            font,
            size_pt,
            align: Align::Left,
            wrap: false,
            clip: true,
            fill: None,
            text: text.into(),
        }
    }

    /// With this alignment.
    #[must_use]
    pub fn align(mut self, align: Align) -> TextBox {
        self.align = align;
        self
    }

    /// Wrapping at spaces.
    #[must_use]
    pub fn wrapped(mut self) -> TextBox {
        self.wrap = true;
        self
    }

    /// Not clipped (may overflow its box).
    #[must_use]
    pub fn unclipped(mut self) -> TextBox {
        self.clip = false;
        self
    }

    /// With a protective fill character.
    #[must_use]
    pub fn filled(mut self, c: char) -> TextBox {
        self.fill = Some(c);
        self
    }

    /// The box's lines after wrapping and clipping (no fill).
    #[must_use]
    pub fn lines(&self) -> Vec<String> {
        let width = self.rect.w;
        let mut out = Vec::new();
        for para in self.text.split('\n') {
            if self.wrap {
                out.extend(wrap_line(para, self.font, self.size_pt, width));
            } else {
                out.push(para.to_string());
            }
        }
        if self.clip {
            let max = max_lines(self.rect.h, self.size_pt);
            out.truncate(max.max(1));
            for l in &mut out {
                *l = truncate_to(l, self.font, self.size_pt, width);
            }
        }
        out
    }
}

/// How many lines of `size_pt` text fit in `height` mm.
#[must_use]
pub fn max_lines(height: f32, size_pt: f32) -> usize {
    let lh = line_height_mm(size_pt);
    let mut n = 0usize;
    // The first line needs one line height; count while the next fits.
    let mut used = lh;
    while used <= height + 0.01 {
        n += 1;
        used += lh;
    }
    n
}

/// Line pitch for `size_pt` text, in millimetres.
#[must_use]
pub fn line_height_mm(size_pt: f32) -> f32 {
    size_pt * LINE_HEIGHT * crate::fonts::MM_PER_PT
}

fn wrap_line(text: &str, font: Font, size: f32, width: f32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        let candidate = if cur.is_empty() {
            word.to_string()
        } else {
            format!("{cur} {word}")
        };
        if cur.is_empty() || font.width_mm(&candidate, size) <= width {
            cur = candidate;
        } else {
            lines.push(std::mem::take(&mut cur));
            cur = word.to_string();
        }
        // A single word longer than the line is broken by characters.
        while font.width_mm(&cur, size) > width && cur.chars().count() > 1 {
            let head = truncate_to(&cur, font, size, width);
            let head = if head.is_empty() {
                cur.chars().take(1).collect()
            } else {
                head
            };
            let rest: String = cur.chars().skip(head.chars().count()).collect();
            lines.push(head);
            cur = rest;
        }
    }
    if !cur.is_empty() || lines.is_empty() {
        lines.push(cur);
    }
    lines
}

/// The longest prefix of `text` that fits in `width` mm.
fn truncate_to(text: &str, font: Font, size: f32, width: f32) -> String {
    if font.width_mm(text, size) <= width {
        return text.to_string();
    }
    let mut out = String::new();
    for c in text.chars() {
        out.push(c);
        if font.width_mm(&out, size) > width {
            out.pop();
            break;
        }
    }
    out
}

/// A drawn shape.
#[derive(Debug, Clone, PartialEq)]
pub enum Shape {
    /// A rule from (`x1`, `y1`) to (`x2`, `y2`).
    Line {
        /// Start x.
        x1: f32,
        /// Start y.
        y1: f32,
        /// End x.
        x2: f32,
        /// End y.
        y2: f32,
        /// Stroke width in points.
        width_pt: f32,
    },
    /// A rectangle, stroked (`stroke_pt`) and/or filled (`fill_gray`, 0 =
    /// black, 1 = white).
    Box {
        /// Where.
        rect: Rect,
        /// Stroke width in points, if outlined.
        stroke_pt: Option<f32>,
        /// Fill grey, if filled.
        fill_gray: Option<f32>,
    },
}

/// One thing on a page.
#[derive(Debug, Clone, PartialEq)]
pub enum Element {
    /// A text field.
    Text(TextBox),
    /// A rule or box.
    Shape(Shape),
    /// The document's logo, scaled to fit the box keeping its proportions,
    /// top-left aligned.
    Logo(Rect),
}

/// One page of elements.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Page {
    /// In drawing order.
    pub elements: Vec<Element>,
}

impl Page {
    /// Add a text box.
    pub fn text(&mut self, b: TextBox) {
        self.elements.push(Element::Text(b));
    }

    /// Add a shape.
    pub fn shape(&mut self, s: Shape) {
        self.elements.push(Element::Shape(s));
    }

    /// A text box by tag.
    #[must_use]
    pub fn find(&self, tag: &str) -> Option<&TextBox> {
        self.elements.iter().find_map(|e| match e {
            Element::Text(b) if b.tag == tag => Some(b),
            _ => None,
        })
    }
}

/// A document: Letter pages and an optional logo.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Document {
    /// The PDF title (document information).
    pub title: String,
    /// The pages.
    pub pages: Vec<Page>,
    /// The logo `Element::Logo` boxes draw.
    pub logo: Option<Logo>,
}

/// One line of text exactly as the PDF draws it.
#[derive(Debug, Clone, PartialEq)]
pub struct Run {
    /// The box's tag.
    pub tag: String,
    /// Left end of the baseline (mm from the left edge).
    pub x: f32,
    /// Baseline (mm from the top edge).
    pub baseline: f32,
    /// The face.
    pub font: Font,
    /// Size in points.
    pub size_pt: f32,
    /// The text, fill included.
    pub text: String,
}

/// A logo placement after scaling.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LogoPlacement {
    /// Where the picture is drawn.
    pub rect: Rect,
}

/// A page ready to draw.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ResolvedPage {
    /// Shapes, drawn first.
    pub shapes: Vec<Shape>,
    /// Logo placements, drawn after the shapes.
    pub logos: Vec<LogoPlacement>,
    /// Text runs, drawn last, in order.
    pub runs: Vec<Run>,
}

impl ResolvedPage {
    /// The runs of the box tagged `tag`.
    pub fn runs_of<'a>(&'a self, tag: &'a str) -> impl Iterator<Item = &'a Run> + 'a {
        self.runs.iter().filter(move |r| r.tag == tag)
    }
}

/// Resolve every page.
///
/// # Errors
/// [`PrintError::NoPages`] for a document without pages.
pub fn resolve(doc: &Document) -> Result<Vec<ResolvedPage>, PrintError> {
    if doc.pages.is_empty() {
        return Err(PrintError::NoPages);
    }
    Ok(doc
        .pages
        .iter()
        .map(|p| resolve_page(p, doc.logo.as_ref()))
        .collect())
}

fn resolve_page(page: &Page, logo: Option<&Logo>) -> ResolvedPage {
    let mut out = ResolvedPage::default();
    for e in &page.elements {
        match e {
            Element::Shape(s) => out.shapes.push(s.clone()),
            Element::Logo(r) => {
                if let Some(l) = logo {
                    out.logos.push(LogoPlacement {
                        rect: fit(*r, l.width, l.height),
                    });
                }
            }
            Element::Text(b) => resolve_text(b, &mut out.runs),
        }
    }
    out
}

/// `w`×`h` pixels scaled to fit `r`, top-left aligned.
fn fit(r: Rect, w: u32, h: u32) -> Rect {
    if w == 0 || h == 0 {
        return Rect {
            w: 0.0,
            h: 0.0,
            ..r
        };
    }
    // Pixel counts are capped at 10 000 when a logo is read, so the
    // conversion is exact.
    let (pw, ph) = (px_f32(w), px_f32(h));
    let s = (r.w / pw).min(r.h / ph);
    Rect::new(r.x, r.y, pw * s, ph * s)
}

fn px_f32(v: u32) -> f32 {
    f32::from(u16::try_from(v).unwrap_or(u16::MAX))
}

fn resolve_text(b: &TextBox, runs: &mut Vec<Run>) {
    let lh = line_height_mm(b.size_pt);
    let ascent = b.font.ascent_mm(b.size_pt);
    let lines = b.lines();
    let single = lines.len() == 1;
    let mut baseline = b.rect.y + ascent;
    for line in lines {
        let text = match b.fill {
            Some(c) if single => fill_text(&line, c, b),
            _ => line,
        };
        if !text.is_empty() {
            let w = b.font.width_mm(&text, b.size_pt);
            let x = match b.align {
                Align::Left => b.rect.x,
                Align::Center => b.rect.x + (b.rect.w - w) / 2.0,
                Align::Right => b.rect.right() - w,
            };
            runs.push(Run {
                tag: b.tag.clone(),
                x,
                baseline,
                font: b.font,
                size_pt: b.size_pt,
                text,
            });
        }
        baseline += lh;
    }
}

fn fill_text(line: &str, c: char, b: &TextBox) -> String {
    let fill_w = b.font.width_mm(&c.to_string(), b.size_pt);
    if fill_w <= 0.0 {
        return line.to_string();
    }
    let used = b.font.width_mm(line, b.size_pt);
    let gap = if line.is_empty() {
        0.0
    } else {
        b.font.width_mm(" ", b.size_pt)
    };
    let mut n = 0usize;
    while used + gap + fill_w * as_f32(n + 1) <= b.rect.w + 0.001 {
        n += 1;
    }
    let fill: String = std::iter::repeat_n(c, n).collect();
    if n == 0 {
        return line.to_string();
    }
    match (b.align, line.is_empty()) {
        (_, true) => fill,
        (Align::Right, false) => format!("{fill} {line}"),
        _ => format!("{line} {fill}"),
    }
}

fn as_f32(n: usize) -> f32 {
    f32::from(u16::try_from(n).unwrap_or(u16::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tb(text: &str, w: f32, h: f32) -> TextBox {
        TextBox::new("t", Rect::new(10.0, 20.0, w, h), Font::Sans, 10.0, text)
    }

    #[test]
    fn left_right_center_alignment_use_font_widths() {
        for (align, expect) in [
            (Align::Left, 10.0),
            (Align::Right, 60.0),
            (Align::Center, 35.0),
        ] {
            let mut p = Page::default();
            p.text(tb("Total", 50.0, 6.0).align(align));
            let r = &resolve_page(&p, None).runs[0];
            let w = Font::Sans.width_mm("Total", 10.0);
            let x = match align {
                Align::Left => expect,
                Align::Right => expect - w,
                Align::Center => expect - w / 2.0,
            };
            assert!((r.x - x).abs() < 0.001, "{align:?}");
            assert!((r.baseline - (20.0 + Font::Sans.ascent_mm(10.0))).abs() < 0.001);
        }
    }

    #[test]
    fn wrap_breaks_at_spaces_and_clip_drops_extra_lines() {
        let b = tb(
            "one two three four five six seven eight nine ten",
            25.0,
            9.0,
        )
        .wrapped();
        let lines = b.lines();
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines.iter().all(|l| Font::Sans.width_mm(l, 10.0) <= 25.0));
        let all = b.clone().unclipped().lines();
        assert!(all.len() > 2);
        assert_eq!(all.join(" "), b.text);
    }

    #[test]
    fn clip_truncates_a_long_single_line_and_long_words_break() {
        let b = tb("Supercalifragilisticexpialidocious", 15.0, 6.0);
        let l = &b.lines()[0];
        assert!(b.text.starts_with(l.as_str()) && l.len() < b.text.len());
        let w = tb("Supercalifragilisticexpialidocious", 15.0, 50.0)
            .wrapped()
            .lines();
        assert!(w.len() > 1 && w.concat() == b.text);
    }

    #[test]
    fn fill_runs_to_the_edge_on_the_correct_side() {
        let left = TextBox {
            fill: Some('*'),
            ..tb("One and 00/100", 80.0, 6.0)
        };
        let mut p = Page::default();
        p.text(left);
        p.text(tb("1.00", 30.0, 6.0).align(Align::Right).filled('*'));
        let rp = resolve_page(&p, None);
        let a = &rp.runs[0];
        assert!(a.text.starts_with("One and 00/100 *") && a.text.ends_with('*'));
        let w = Font::Sans.width_mm(&a.text, 10.0);
        let star = Font::Sans.width_mm("*", 10.0);
        assert!(w <= 80.0 + 0.001 && w > 80.0 - star - 0.001, "{w}");
        let b = &rp.runs[1];
        assert!(b.text.starts_with('*') && b.text.ends_with(" 1.00"));
        assert!((b.x + Font::Sans.width_mm(&b.text, 10.0) - 40.0).abs() < 0.001);
    }

    #[test]
    fn max_lines_counts_whole_lines() {
        let lh = line_height_mm(10.0);
        assert_eq!(max_lines(lh * 3.0, 10.0), 3);
        assert_eq!(max_lines(lh * 3.0 - 0.1, 10.0), 2);
        assert_eq!(max_lines(0.5, 10.0), 0);
    }

    #[test]
    fn logo_fits_keeping_proportions() {
        let r = fit(Rect::new(0.0, 0.0, 50.0, 20.0), 400, 100);
        assert!((r.w - 50.0).abs() < 0.001 && (r.h - 12.5).abs() < 0.001);
        let r = fit(Rect::new(5.0, 6.0, 50.0, 20.0), 100, 100);
        assert!(
            (r.w - 20.0).abs() < 0.001 && (r.x - 5.0).abs() < 0.001 && (r.y - 6.0).abs() < 0.001
        );
    }

    #[test]
    fn empty_documents_are_refused() {
        assert!(matches!(
            resolve(&Document::default()),
            Err(PrintError::NoPages)
        ));
    }

    #[test]
    fn rect_contains_and_moves() {
        let a = Rect::new(0.0, 0.0, 10.0, 10.0);
        assert!(a.contains(Rect::new(1.0, 1.0, 8.0, 8.0)));
        assert!(!a.contains(Rect::new(1.0, 1.0, 10.0, 8.0)));
        assert_eq!(a.moved(2.0, -1.0), Rect::new(2.0, -1.0, 10.0, 10.0));
    }
}
