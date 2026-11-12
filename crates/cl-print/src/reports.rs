//! A minimal report renderer (P27, Q-P27b): a title block, label/amount
//! sections and simple tables with page breaks, on US Letter. The
//! reconciliation summary, detail and discrepancy reports use it; P28
//! extends it to the general reports.
//!
//! Every amount arrives as display text (the engine's figure, formatted by
//! the caller); nothing here parses, adds or compares amounts.
//!
//! Wide tables (A07 QA-8, Q-A07v): a table whose numbers or dates don't fit
//! its share of the page is sized from its content instead. The steps, in
//! order, until every number and date is whole: portrait Letter; landscape
//! Letter (the whole document); a smaller body size, down to
//! [`MIN_BODY_PT`]; then the columns are split across pages, each part
//! repeating the row captions. Names and memos may be cut; numbers and
//! dates never are (Q-P19ah).

use crate::PrintError;
use crate::fonts::Font;
use crate::layout::{
    Align, Document, LETTER_H_MM, LETTER_W_MM, Page, PageSize, Rect, Shape, TextBox, line_height_mm,
};

const MARGIN: f32 = 18.0;
const BODY_PT: f32 = 9.0;
const HEAD_PT: f32 = 10.5;
const TITLE_PT: f32 = 16.0;
/// The smallest body size a wide table shrinks to before its columns are
/// split across pages.
pub const MIN_BODY_PT: f32 = 6.5;
/// Space after each column.
const GAP: f32 = 2.0;
/// The narrowest a cut text column goes (names, memos).
const TEXT_FLOOR: f32 = 22.0;
/// The narrowest the row captions' column goes.
const CAPTION_FLOOR: f32 = 45.0;

const PORTRAIT: PageSize = PageSize::LETTER;
const LANDSCAPE: PageSize = PageSize {
    w: LETTER_H_MM,
    h: LETTER_W_MM,
};

/// One column of a table.
#[derive(Debug, Clone, PartialEq)]
pub struct Column {
    /// Heading.
    pub heading: String,
    /// Share of the table's width (the shares are normalized).
    pub share: f32,
    /// Right-aligned (amounts). Never cut.
    pub right: bool,
    /// Never cut, though left-aligned (dates, numbers, short codes).
    pub whole: bool,
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
    size: PageSize,
}

impl Cursor {
    fn bottom(&self) -> f32 {
        self.size.h - MARGIN - 8.0
    }

    fn page(&mut self) -> &mut Page {
        if self.pages.is_empty() {
            self.pages.push(Page::default());
        }
        let last = self.pages.len() - 1;
        &mut self.pages[last]
    }

    /// A new page.
    fn break_page(&mut self) {
        self.pages.push(Page::default());
        self.y = MARGIN;
    }

    /// Room for `h` mm, else a new page.
    fn need(&mut self, h: f32) {
        if self.y + h > self.bottom() {
            self.break_page();
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

fn width_of(size: PageSize) -> f32 {
    size.w - 2.0 * MARGIN
}

/// The font a cell of `col` prints in.
fn cell_font(col: &Column) -> Font {
    if col.right { Font::Mono } else { Font::Sans }
}

/// How a table is laid out: its body size, each column's width (gap
/// included) and the column parts (each runs through every row; the first
/// column is in every part).
#[derive(Debug, Clone, PartialEq)]
struct TablePlan {
    pt: f32,
    widths: Vec<f32>,
    parts: Vec<Vec<usize>>,
}

impl TablePlan {
    /// The (x, text width) of each column of `part`, left to right.
    fn xs(&self, part: &[usize]) -> Vec<(f32, f32)> {
        let mut x = MARGIN;
        part.iter()
            .map(|&ci| {
                let w = self.widths.get(ci).copied().unwrap_or(TEXT_FLOOR);
                let at = (x, (w - GAP).max(1.0));
                x += w;
                at
            })
            .collect()
    }
}

fn table_header(c: &mut Cursor, plan: &TablePlan, columns: &[Column], part: &[usize]) {
    let h = line_height_mm(plan.pt);
    c.need(h * 2.0);
    let mut right_edge = MARGIN;
    for (&ci, (x, w)) in part.iter().zip(plan.xs(part)) {
        let Some(col) = columns.get(ci) else { continue };
        let align = if col.right { Align::Right } else { Align::Left };
        c.text(x, w, plan.pt, Font::SansBold, align, &col.heading);
        right_edge = x + w;
    }
    c.y += h + 0.8;
    c.rule(MARGIN, right_edge.max(MARGIN + 1.0));
    c.y += 1.0;
}

/// The share layout (P27): the page width split by the columns' shares.
fn share_widths(columns: &[Column], full: f32) -> Vec<f32> {
    let total: f32 = columns.iter().map(|k| k.share).sum::<f32>().max(0.01);
    columns.iter().map(|k| full * k.share / total).collect()
}

/// Each column's widest text at `pt` (heading included), plus the gap.
fn needs(columns: &[Column], rows: &[Vec<String>], pt: f32) -> Vec<f32> {
    columns
        .iter()
        .enumerate()
        .map(|(ci, col)| {
            let font = cell_font(col);
            let widest = rows
                .iter()
                .filter_map(|r| r.get(ci))
                .map(|t| font.width_mm(t, pt))
                .fold(Font::SansBold.width_mm(&col.heading, pt), f32::max);
            widest + GAP
        })
        .collect()
}

/// True when `widths` at `pt` cut nothing that must stay whole: numbers
/// fit after their shrink-to-fit, dates and codes fit as they are.
fn whole_fits(columns: &[Column], rows: &[Vec<String>], widths: &[f32], pt: f32) -> bool {
    let need = needs(columns, rows, pt);
    columns.iter().zip(widths).zip(&need).all(|((col, w), n)| {
        let room = w - GAP + 0.001;
        if col.right {
            // A number shrinks to MIN_SHRINK of its size (layout::fitted).
            (n - GAP) * crate::layout::MIN_SHRINK <= room
        } else if col.whole {
            n - GAP <= room
        } else {
            true
        }
    })
}

/// The least each column may have: the whole text of numbers, dates and
/// codes; cut text down to a floor.
fn minimums(columns: &[Column], need: &[f32]) -> Vec<f32> {
    columns
        .iter()
        .zip(need)
        .enumerate()
        .map(|(ci, (col, n))| {
            if col.right || col.whole {
                *n
            } else if ci == 0 {
                n.min(CAPTION_FLOOR)
            } else {
                n.min(TEXT_FLOOR)
            }
        })
        .collect()
}

/// Content widths filling `full`: the minimums, then cut text columns
/// grow back toward their whole text, then what is left goes to the first
/// column (the captions), so amounts sit at the right edge.
fn content_widths(need: &[f32], min: &[f32], full: f32) -> Vec<f32> {
    let mut w = min.to_vec();
    let mut spare = full - w.iter().sum::<f32>();
    let short: f32 = need.iter().zip(&w).map(|(n, m)| (n - m).max(0.0)).sum();
    if spare > 0.0 && short > 0.0 {
        let k = (spare / short).min(1.0);
        for (wi, n) in w.iter_mut().zip(need) {
            let add = (n - *wi).max(0.0) * k;
            *wi += add;
            spare -= add;
        }
    }
    if spare > 0.0
        && let Some(first) = w.first_mut()
    {
        *first += spare;
    }
    w
}

/// The table's plan on a page `full` wide without splitting its columns:
/// the share layout when it cuts nothing, else content widths at the
/// largest body size that fits; `None` when even [`MIN_BODY_PT`] doesn't.
fn plan_on(columns: &[Column], rows: &[Vec<String>], full: f32) -> Option<TablePlan> {
    let all: Vec<usize> = (0..columns.len()).collect();
    let shares = share_widths(columns, full);
    if whole_fits(columns, rows, &shares, BODY_PT) {
        return Some(TablePlan {
            pt: BODY_PT,
            widths: shares,
            parts: vec![all],
        });
    }
    let mut pt = BODY_PT;
    while pt >= MIN_BODY_PT - 0.001 {
        let need = needs(columns, rows, pt);
        let min = minimums(columns, &need);
        if min.iter().sum::<f32>() <= full + 0.001 {
            return Some(TablePlan {
                pt,
                widths: content_widths(&need, &min, full),
                parts: vec![all],
            });
        }
        pt -= 0.5;
    }
    None
}

/// The plan at [`MIN_BODY_PT`] with the columns split into parts that
/// each fit the page, the first column repeated in each.
fn split_plan(columns: &[Column], rows: &[Vec<String>], full: f32) -> TablePlan {
    let pt = MIN_BODY_PT;
    let need = needs(columns, rows, pt);
    let min = minimums(columns, &need);
    let lead = min.first().copied().unwrap_or(0.0);
    let mut parts: Vec<Vec<usize>> = Vec::new();
    let mut part = vec![0];
    let mut used = lead;
    for (ci, m) in min.iter().enumerate().skip(1) {
        if part.len() > 1 && used + m > full + 0.001 {
            parts.push(std::mem::replace(&mut part, vec![0]));
            used = lead;
        }
        part.push(ci);
        used += m;
    }
    parts.push(part);
    // The captions grow into the room every part has left.
    let spare = parts
        .iter()
        .map(|p| full - p.iter().map(|&ci| min[ci]).sum::<f32>())
        .fold(f32::INFINITY, f32::min)
        .max(0.0);
    let mut widths = min;
    if let (Some(w), Some(n)) = (widths.first_mut(), need.first()) {
        *w += spare.min((n - *w).max(0.0));
    }
    TablePlan { pt, widths, parts }
}

fn tables(r: &Report) -> impl Iterator<Item = (&[Column], &[Vec<String>])> {
    r.sections.iter().filter_map(|s| match s {
        Section::Table { columns, rows, .. } => Some((columns.as_slice(), rows.as_slice())),
        Section::Summary { .. } => None,
    })
}

/// Portrait unless a table's numbers or dates fit a portrait page only at
/// a smaller size or split (then the whole document is landscape).
fn page_size(r: &Report) -> PageSize {
    let portrait = width_of(PORTRAIT);
    let fits = |(columns, rows): (&[Column], &[Vec<String>])| {
        whole_fits(columns, rows, &share_widths(columns, portrait), BODY_PT) || {
            let need = needs(columns, rows, BODY_PT);
            minimums(columns, &need).iter().sum::<f32>() <= portrait + 0.001
        }
    };
    if tables(r).all(fits) {
        PORTRAIT
    } else {
        LANDSCAPE
    }
}

/// The report laid out on Letter pages (page breaks repeat a table's
/// header row; every page carries the footer and "Page n of m"). Portrait,
/// or landscape for a table too wide for it.
#[must_use]
#[allow(clippy::too_many_lines)] // one pass over the sections, kept together
pub fn layout_report(r: &Report) -> Document {
    let size = page_size(r);
    let mut c = Cursor {
        pages: vec![Page::default()],
        y: MARGIN,
        n: 0,
        size,
    };
    let full = width_of(size);
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
                let plan =
                    plan_on(columns, rows, full).unwrap_or_else(|| split_plan(columns, rows, full));
                let line = line_height_mm(plan.pt) + 0.8;
                for (pi, part) in plan.parts.iter().enumerate() {
                    // A table's next column part starts a page.
                    if pi > 0 {
                        c.break_page();
                    }
                    c.need(line_height_mm(HEAD_PT) + line * 3.0);
                    c.text(MARGIN, full, HEAD_PT, Font::SansBold, Align::Left, heading);
                    c.y += line_height_mm(HEAD_PT) + 1.5;
                    let xs = plan.xs(part);
                    table_header(&mut c, &plan, columns, part);
                    if rows.is_empty() {
                        c.text(MARGIN, full, plan.pt, Font::Sans, Align::Left, empty);
                        c.y += line;
                    }
                    for row in rows {
                        if c.y + line > c.bottom() {
                            c.break_page();
                            table_header(&mut c, &plan, columns, part);
                        }
                        for (&ci, (x, w)) in part.iter().zip(&xs) {
                            let (Some(col), Some(cell)) = (columns.get(ci), row.get(ci)) else {
                                continue;
                            };
                            let align = if col.right { Align::Right } else { Align::Left };
                            c.text(*x, *w, plan.pt, cell_font(col), align, cell);
                        }
                        c.y += line;
                    }
                    c.y += 4.0;
                }
            }
        }
    }
    // Footers.
    let count = c.pages.len();
    for (i, page) in c.pages.iter_mut().enumerate() {
        let y = size.h - MARGIN;
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
        size,
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

    fn col(heading: &str, share: f32, right: bool) -> Column {
        Column {
            heading: heading.into(),
            share,
            right,
            whole: false,
        }
    }

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
                        col("Date", 1.0, false),
                        col("Payee", 3.0, false),
                        col("Amount", 1.2, true),
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
        // A narrow report keeps the share layout, portrait.
        assert_eq!(layout_report(&summary()).size, PageSize::LETTER);
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

    /// A table of `n` amount columns, every figure `figure`.
    fn wide(n: usize, figure: &str) -> Report {
        let mut columns = vec![col("", 3.0, false)];
        columns.extend((0..n).map(|i| col(&format!("Month {i}"), 1.2, true)));
        let row = |caption: &str| {
            let mut r = vec![caption.to_string()];
            r.extend((0..n).map(|_| figure.to_string()));
            r
        };
        Report {
            title: "Profit and Loss by Month".into(),
            subtitle: vec![],
            sections: vec![Section::Table {
                heading: String::new(),
                columns,
                rows: vec![row("   30170 · Owner's Draw"), row("Net income")],
                empty: "None".into(),
            }],
            footer: String::new(),
            page_label: "Page {n} of {m}".into(),
        }
    }

    /// A07 QA-8: a 13-column monthly report with 6-figure negatives built
    /// no PDF (`DoesNotFit`); now it goes landscape with every figure whole.
    #[test]
    fn a_thirteen_column_report_goes_landscape_with_whole_figures() {
        let r = wide(13, "-353,267.44");
        let doc = layout_report(&r);
        assert_eq!(doc.size, LANDSCAPE);
        let pages = resolve(&doc).expect("every figure fits");
        let figures: Vec<_> = pages
            .iter()
            .flat_map(|p| &p.runs)
            .filter(|run| run.font == Font::Mono)
            .collect();
        assert_eq!(figures.len(), 26);
        assert!(figures.iter().all(|f| f.text == "-353,267.44"));
        // Each figure ends inside the page's right margin.
        for p in &pages {
            for run in &p.runs {
                let right = run.x + run.font.width_mm(&run.text, run.size_pt);
                assert!(right <= LANDSCAPE.w - MARGIN + 0.01, "{}", run.text);
            }
        }
        assert!(render_report(&r).expect("pdf").starts_with(b"%PDF"));
    }

    /// Too many columns for a landscape page even at the smallest size: the
    /// columns are split across pages, each part with the row captions.
    #[test]
    fn a_table_too_wide_for_any_page_splits_its_columns() {
        let r = wide(30, "-1,353,267.44");
        let pages = resolve(&layout_report(&r)).expect("every figure fits");
        assert!(pages.len() >= 2);
        for p in &pages {
            assert!(p.runs.iter().any(|run| run.text.contains("Owner's Draw")));
        }
        let figures = pages
            .iter()
            .flat_map(|p| &p.runs)
            .filter(|run| run.text == "-1,353,267.44")
            .count();
        assert_eq!(figures, 60, "every figure printed once");
        assert!(render_report(&r).is_ok());
    }

    /// Dates and other whole columns are never cut; a long memo is.
    #[test]
    fn dates_print_whole_and_memos_are_cut() {
        let mut columns = vec![
            Column {
                whole: true,
                ..col("Date", 1.2, false)
            },
            Column {
                whole: true,
                ..col("Type", 1.2, false)
            },
            col("Memo", 1.2, false),
        ];
        columns.extend((0..7).map(|i| col(&format!("C{i}"), 1.2, true)));
        let mut row = vec![
            "11/05/2026".to_string(),
            "Journal entry".to_string(),
            "A very long memo that goes on and on about the plants bought".to_string(),
        ];
        row.extend((0..7).map(|_| "186,310.55".to_string()));
        let r = Report {
            title: "Transaction Detail".into(),
            subtitle: vec![],
            sections: vec![Section::Table {
                heading: String::new(),
                columns,
                rows: vec![row; 80],
                empty: String::new(),
            }],
            footer: String::new(),
            page_label: "Page {n} of {m}".into(),
        };
        let pages = resolve(&layout_report(&r)).expect("fits");
        let all: Vec<&str> = pages
            .iter()
            .flat_map(|p| &p.runs)
            .map(|run| run.text.as_str())
            .collect();
        assert_eq!(all.iter().filter(|t| **t == "11/05/2026").count(), 80);
        assert_eq!(all.iter().filter(|t| **t == "Journal entry").count(), 80);
        assert_eq!(all.iter().filter(|t| **t == "186,310.55").count(), 560);
        assert!(
            !all.iter()
                .any(|t| t.starts_with("11/05/20") && t.len() < 10)
        );
    }
}
