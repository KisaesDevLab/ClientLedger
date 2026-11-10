//! A minimal report renderer (P27, Q-P27b): a title block, label/amount
//! sections and simple tables with page breaks, on US Letter. The
//! reconciliation summary, detail and discrepancy reports use it; P28
//! extends it to the general reports.
//!
//! Every amount arrives as display text (the engine's figure, formatted by
//! the caller); nothing here parses, adds or compares amounts.

use crate::PrintError;
use crate::fonts::Font;
use crate::layout::{
    Align, Document, LETTER_H_MM, LETTER_W_MM, Page, PageSize, Rect, Shape, TextBox, line_height_mm,
};

const MARGIN: f32 = 18.0;
const BODY_PT: f32 = 9.0;
const HEAD_PT: f32 = 10.5;
const TITLE_PT: f32 = 16.0;

/// One column of a table.
#[derive(Debug, Clone, PartialEq)]
pub struct Column {
    /// Heading.
    pub heading: String,
    /// Share of the table's width (the shares are normalized).
    pub share: f32,
    /// Right-aligned (amounts).
    pub right: bool,
}

/// A report section.
#[derive(Debug, Clone, PartialEq)]
pub enum Section {
    /// Label and amount rows (a summary). A row whose label starts a total
    /// gets a rule above it when `total` is set.
    Summary {
        /// Heading (may be empty).
        heading: String,
        /// `(label, amount, total)`: a total row gets a rule above.
        rows: Vec<(String, String, bool)>,
    },
    /// A table with a header row.
    Table {
        /// Heading.
        heading: String,
        /// The columns.
        columns: Vec<Column>,
        /// The rows (one string per column).
        rows: Vec<Vec<String>>,
        /// The words shown when there are no rows.
        empty: String,
    },
}

/// A report to print.
#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    /// The PDF title and the first heading.
    pub title: String,
    /// Lines under the title (company, account, dates).
    pub subtitle: Vec<String>,
    /// The sections, in order.
    pub sections: Vec<Section>,
    /// A footer on every page (for example "Figures as of today").
    pub footer: String,
    /// The page number text with `{n}` and `{m}` ("Page {n} of {m}").
    pub page_label: String,
}

struct Cursor {
    pages: Vec<Page>,
    y: f32,
    n: usize,
}

impl Cursor {
    fn bottom() -> f32 {
        LETTER_H_MM - MARGIN - 8.0
    }

    fn page(&mut self) -> &mut Page {
        if self.pages.is_empty() {
            self.pages.push(Page::default());
        }
        let last = self.pages.len() - 1;
        &mut self.pages[last]
    }

    /// Room for `h` mm, else a new page.
    fn need(&mut self, h: f32) {
        if self.y + h > Self::bottom() {
            self.pages.push(Page::default());
            self.y = MARGIN;
        }
    }

    fn text(&mut self, x: f32, w: f32, size: f32, font: Font, align: Align, text: &str) {
        self.n += 1;
        let tag = format!("t{}", self.n);
        let h = line_height_mm(size);
        let y = self.y;
        self.page()
            .text(TextBox::new(tag, Rect { x, y, w, h }, font, size, text).align(align));
    }

    fn rule(&mut self, x1: f32, x2: f32) {
        let y = self.y;
        self.page().shape(Shape::Line {
            x1,
            y1: y,
            x2,
            y2: y,
            width_pt: 0.5,
        });
    }
}

fn width() -> f32 {
    LETTER_W_MM - 2.0 * MARGIN
}

fn table_header(c: &mut Cursor, columns: &[Column], xs: &[(f32, f32)]) {
    let h = line_height_mm(BODY_PT);
    c.need(h * 2.0);
    for (col, (x, w)) in columns.iter().zip(xs) {
        let align = if col.right { Align::Right } else { Align::Left };
        c.text(*x, *w, BODY_PT, Font::SansBold, align, &col.heading);
    }
    c.y += h + 0.8;
    c.rule(MARGIN, MARGIN + width());
    c.y += 1.0;
}

/// The report laid out on Letter pages (page breaks repeat a table's
/// header row; every page carries the footer and "Page n of m").
#[must_use]
#[allow(clippy::too_many_lines)] // one pass over the sections, kept together
pub fn layout_report(r: &Report) -> Document {
    let mut c = Cursor {
        pages: vec![Page::default()],
        y: MARGIN,
        n: 0,
    };
    let full = width();
    c.text(
        MARGIN,
        full,
        TITLE_PT,
        Font::SansBold,
        Align::Left,
        &r.title,
    );
    c.y += line_height_mm(TITLE_PT) + 1.0;
    for line in &r.subtitle {
        c.text(MARGIN, full, BODY_PT, Font::Sans, Align::Left, line);
        c.y += line_height_mm(BODY_PT);
    }
    c.y += 4.0;
    let body = line_height_mm(BODY_PT) + 0.8;
    for s in &r.sections {
        match s {
            Section::Summary { heading, rows } => {
                if !heading.is_empty() {
                    c.need(line_height_mm(HEAD_PT) + body);
                    c.text(MARGIN, full, HEAD_PT, Font::SansBold, Align::Left, heading);
                    c.y += line_height_mm(HEAD_PT) + 1.5;
                }
                let label_w = full * 0.6;
                let amount_x = MARGIN + label_w;
                let amount_w = full * 0.25;
                for (label, amount, total) in rows {
                    c.need(body + 1.0);
                    if *total {
                        c.rule(amount_x, amount_x + amount_w);
                        c.y += 0.8;
                    }
                    let font = if *total { Font::SansBold } else { Font::Sans };
                    c.text(MARGIN, label_w, BODY_PT, font, Align::Left, label);
                    c.text(
                        amount_x,
                        amount_w,
                        BODY_PT,
                        Font::Mono,
                        Align::Right,
                        amount,
                    );
                    c.y += body;
                }
                c.y += 4.0;
            }
            Section::Table {
                heading,
                columns,
                rows,
                empty,
            } => {
                c.need(line_height_mm(HEAD_PT) + body * 3.0);
                c.text(MARGIN, full, HEAD_PT, Font::SansBold, Align::Left, heading);
                c.y += line_height_mm(HEAD_PT) + 1.5;
                let total: f32 = columns.iter().map(|k| k.share).sum::<f32>().max(0.01);
                let mut x = MARGIN;
                let xs: Vec<(f32, f32)> = columns
                    .iter()
                    .map(|k| {
                        let w = full * k.share / total;
                        let at = (x, w - 2.0);
                        x += w;
                        at
                    })
                    .collect();
                table_header(&mut c, columns, &xs);
                if rows.is_empty() {
                    c.text(MARGIN, full, BODY_PT, Font::Sans, Align::Left, empty);
                    c.y += body;
                }
                for row in rows {
                    if c.y + body > Cursor::bottom() {
                        c.need(Cursor::bottom());
                        table_header(&mut c, columns, &xs);
                    }
                    for ((col, (x, w)), cell) in columns.iter().zip(&xs).zip(row) {
                        let (font, align) = if col.right {
                            (Font::Mono, Align::Right)
                        } else {
                            (Font::Sans, Align::Left)
                        };
                        c.text(*x, *w, BODY_PT, font, align, cell);
                    }
                    c.y += body;
                }
                c.y += 4.0;
            }
        }
    }
    // Footers.
    let count = c.pages.len();
    for (i, page) in c.pages.iter_mut().enumerate() {
        let y = LETTER_H_MM - MARGIN;
        let h = line_height_mm(8.0);
        page.text(
            TextBox::new(
                "footer",
                Rect {
                    x: MARGIN,
                    y,
                    w: full * 0.7,
                    h,
                },
                Font::Sans,
                8.0,
                r.footer.clone(),
            )
            .gray(0.4),
        );
        page.text(
            TextBox::new(
                "page",
                Rect {
                    x: MARGIN + full * 0.7,
                    y,
                    w: full * 0.3,
                    h,
                },
                Font::Sans,
                8.0,
                r.page_label
                    .replace("{n}", &(i + 1).to_string())
                    .replace("{m}", &count.to_string()),
            )
            .align(Align::Right)
            .gray(0.4),
        );
    }
    Document {
        title: r.title.clone(),
        size: PageSize::LETTER,
        pages: c.pages,
        logo: None,
    }
}

/// The report's PDF bytes.
///
/// # Errors
/// As [`crate::render`].
pub fn render_report(r: &Report) -> Result<Vec<u8>, PrintError> {
    crate::pdf::render(&layout_report(r))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::resolve;

    fn summary() -> Report {
        Report {
            title: "Reconciliation Summary".into(),
            subtitle: vec![
                "1010 Operating Checking".into(),
                "Statement ending 10/31/2026".into(),
            ],
            sections: vec![
                Section::Summary {
                    heading: "Cleared".into(),
                    rows: vec![
                        ("Beginning balance".into(), "71,436.20".into(), false),
                        ("6 deposits and credits".into(), "40,107.84".into(), false),
                        ("9 checks and payments".into(), "-24,641.91".into(), false),
                        ("Cleared balance".into(), "86,902.13".into(), true),
                        ("Statement ending balance".into(), "86,902.13".into(), false),
                        ("Difference".into(), "0.00".into(), true),
                    ],
                },
                Section::Table {
                    heading: "Uncleared".into(),
                    columns: vec![
                        Column {
                            heading: "Date".into(),
                            share: 1.0,
                            right: false,
                        },
                        Column {
                            heading: "Payee".into(),
                            share: 3.0,
                            right: false,
                        },
                        Column {
                            heading: "Amount".into(),
                            share: 1.2,
                            right: true,
                        },
                    ],
                    rows: vec![vec![
                        "11/02/2026".into(),
                        "Kimberling Marina".into(),
                        "2,744.67".into(),
                    ]],
                    empty: "None".into(),
                },
            ],
            footer: "Figures as of today".into(),
            page_label: "Page {n} of {m}".into(),
        }
    }

    fn texts(r: &Report) -> Vec<Vec<String>> {
        resolve(&layout_report(r))
            .expect("resolved")
            .iter()
            .map(|p| p.runs.iter().map(|r| r.text.clone()).collect())
            .collect()
    }

    #[test]
    fn summary_golden_text() {
        let pages = texts(&summary());
        assert_eq!(pages.len(), 1);
        assert_eq!(
            pages[0],
            [
                "Reconciliation Summary",
                "1010 Operating Checking",
                "Statement ending 10/31/2026",
                "Cleared",
                "Beginning balance",
                "71,436.20",
                "6 deposits and credits",
                "40,107.84",
                "9 checks and payments",
                "-24,641.91",
                "Cleared balance",
                "86,902.13",
                "Statement ending balance",
                "86,902.13",
                "Difference",
                "0.00",
                "Uncleared",
                "Date",
                "Payee",
                "Amount",
                "11/02/2026",
                "Kimberling Marina",
                "2,744.67",
                "Figures as of today",
                "Page 1 of 1",
            ]
        );
    }

    #[test]
    fn long_tables_break_pages_and_repeat_the_header() {
        let mut r = summary();
        if let Section::Table { rows, .. } = &mut r.sections[1] {
            for i in 0..120 {
                rows.push(vec!["11/02/2026".into(), format!("Row {i}"), "1.00".into()]);
            }
        }
        let pages = texts(&r);
        assert!(pages.len() >= 2);
        assert!(pages[1].iter().any(|t| t == "Payee"), "header repeated");
        assert_eq!(
            pages[1].last().map(String::as_str),
            Some(format!("Page 2 of {}", pages.len()).as_str())
        );
        let pdf = render_report(&r).expect("pdf");
        assert!(pdf.starts_with(b"%PDF"));
    }
}
