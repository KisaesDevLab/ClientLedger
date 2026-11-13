//! Report exports (P28 plan §6): the owner's CSV, the Excel workbook and
//! the PDF/print layout of any [`ReportTable`].
//!
//! - **CSV**: the engine's exact figures (`-1234.56`, at least 2 places,
//!   no thousands separator), percents as `63.7`.
//! - **xlsx** (Q-P28f): the only place money becomes `f64`, at the file
//!   format's boundary. Each value is checked by printing the `f64` back at
//!   the value's own scale; one that doesn't come back exactly is written as
//!   text and counted in a note. Amounts use `#,##0.00;-#,##0.00`, percents
//!   are numbers with a literal `%` (`0.0"%"`), totals are values (no
//!   formulas).
//! - **PDF**: the same display formatting as the screen (2 places, half
//!   away from zero), captions indented by level.
//!
//! Captions starting with `@` are translated by the caller's `tr`.

use std::io::Write as _;
use std::path::Path;

use cl_core::MoneyStyle;
use cl_core::report::{Cell, ColumnKind, ReportTable, RowKind, canonical_decimal, push_csv_line};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use rust_xlsxwriter::{ExcelDateTime, Format, Workbook, XlsxError};

use crate::reports::{Column, Report, Section};

/// Why an export failed.
#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    /// The workbook writer refused.
    #[error("the workbook could not be written")]
    Xlsx(#[from] XlsxError),
    /// The file could not be written.
    #[error("the file could not be written")]
    Io(#[from] std::io::Error),
}

/// The amount format of the workbook.
pub const AMOUNT_FORMAT: &str = "#,##0.00;-#,##0.00";
/// The percent format of the workbook.
pub const PERCENT_FORMAT: &str = "0.0\"%\"";

/// `@key` is translated; `@@text` is text that starts with `@`
/// (`cl_core::report::literal_caption`); anything else is shown as is.
fn label(tr: &dyn Fn(&str) -> String, s: &str) -> String {
    match s.strip_prefix('@') {
        Some(lit) if lit.starts_with('@') => lit.to_string(),
        Some(key) => tr(key),
        None => s.to_string(),
    }
}

/// An exact amount with at least 2 places and no separators.
fn exact(d: Decimal) -> String {
    let mut n = d.normalize();
    if n.is_zero() {
        n = Decimal::ZERO;
    }
    if n.scale() < 2 {
        n.rescale(2);
    }
    n.to_string()
}

/// A date as the app shows it (MM/DD/YYYY), in every export (QA-9).
fn us_date(d: cl_core::CalendarDate) -> String {
    format!("{:02}/{:02}/{}", d.month(), d.day(), d.year())
}

/// Text a spreadsheet would read as a formula (`=`, `+`, `-`, `@`, a tab
/// or a carriage return first) gets a leading apostrophe, so an owner's
/// payee or memo never runs as one when the CSV is opened. Only text
/// cells: amounts keep their exact strings.
fn neutral(s: String) -> String {
    match s.trim_start_matches(' ').chars().next() {
        Some('=' | '+' | '-' | '@' | '\t' | '\r') => format!("'{s}"),
        _ => s,
    }
}

fn csv_cell(c: &Cell, tr: &dyn Fn(&str) -> String) -> String {
    match c {
        Cell::Empty | Cell::Percent(None) => String::new(),
        Cell::Text(s) => neutral(label(tr, s)),
        Cell::Amount(d) => exact(*d),
        Cell::Percent(Some(d)) | Cell::Quantity(d) => canonical_decimal(*d),
        Cell::Date(d) => us_date(*d),
        Cell::Count(n) => n.to_string(),
    }
}

/// The owner's CSV: a heading line, then one line per row with the caption
/// indented two spaces per level and the exact figures.
#[must_use]
pub fn to_csv(
    t: &ReportTable,
    rows: &[usize],
    head: &[String],
    tr: &dyn Fn(&str) -> String,
) -> String {
    let mut out = String::new();
    // The company, title, dates and basis, and any note, as in the
    // workbook and the PDF (QA-6, S1), then a blank line.
    for h in head {
        push_csv_line(&mut out, &[neutral(h.clone())]);
    }
    if !head.is_empty() {
        out.push('\n');
    }
    // A list without row captions has no empty first column (QA-9).
    let captions = rows
        .iter()
        .filter_map(|&i| t.rows.get(i))
        .any(|r| !label(tr, &r.caption).is_empty());
    let mut heading = Vec::new();
    if captions {
        heading.push(String::new());
    }
    heading.extend(t.columns.iter().map(|c| neutral(label(tr, &c.caption))));
    push_csv_line(&mut out, &heading);
    for &i in rows {
        let Some(r) = t.rows.get(i) else { continue };
        let mut line = Vec::new();
        if captions {
            line.push(neutral(format!(
                "{}{}",
                "  ".repeat(usize::from(r.level)),
                label(tr, &r.caption)
            )));
        }
        line.extend(r.cells.iter().map(|c| csv_cell(c, tr)));
        push_csv_line(&mut out, &line);
    }
    out
}

/// The single `Decimal → f64` conversion (Q-P28f): `Some` only when the
/// `f64` prints back as exactly the value at its own scale.
#[must_use]
pub fn xlsx_number(d: Decimal) -> Option<f64> {
    let n = d.normalize();
    let f = n.to_f64()?;
    let scale = usize::try_from(n.scale()).ok()?;
    let back = format!("{f:.scale$}");
    let want = if n.is_zero() {
        format!("{:.scale$}", 0)
    } else {
        n.to_string()
    };
    (back == want || (n.is_zero() && back.trim_start_matches('-') == want)).then_some(f)
}

/// The workbook: `title` lines on top, the heading row, then the rows.
/// Returns the bytes and how many values had to be written as text.
///
/// # Errors
/// [`ExportError::Xlsx`] when the writer refuses.
pub fn to_xlsx(
    t: &ReportTable,
    rows: &[usize],
    title: &[String],
    tr: &dyn Fn(&str) -> String,
) -> Result<(Vec<u8>, u32), ExportError> {
    let mut wb = Workbook::new();
    let ws = wb.add_worksheet();
    let bold = Format::new().set_bold();
    let money = Format::new().set_num_format(AMOUNT_FORMAT);
    let money_bold = Format::new().set_num_format(AMOUNT_FORMAT).set_bold();
    let pct = Format::new().set_num_format(PERCENT_FORMAT);
    let day = Format::new().set_num_format("mm/dd/yyyy");
    let mut r: u32 = 0;
    for line in title {
        ws.write_string_with_format(r, 0, line, &bold)?;
        r += 1;
    }
    if !title.is_empty() {
        r += 1;
    }
    for (ci, c) in t.columns.iter().enumerate() {
        let col = u16::try_from(ci + 1).unwrap_or(u16::MAX);
        ws.write_string_with_format(r, col, label(tr, &c.caption), &bold)?;
    }
    r += 1;
    let mut text_fallbacks = 0u32;
    for &i in rows {
        let Some(row) = t.rows.get(i) else { continue };
        let strong = matches!(
            row.kind,
            RowKind::Total
                | RowKind::Grand
                | RowKind::Header
                | RowKind::Subtotal
                | RowKind::Formula
        );
        let caption = format!(
            "{}{}",
            "  ".repeat(usize::from(row.level)),
            label(tr, &row.caption)
        );
        if strong {
            ws.write_string_with_format(r, 0, caption, &bold)?;
        } else {
            ws.write_string(r, 0, caption)?;
        }
        for (ci, cell) in row.cells.iter().enumerate() {
            let col = u16::try_from(ci + 1).unwrap_or(u16::MAX);
            let kind = t.columns.get(ci).map_or(ColumnKind::Text, |c| c.kind);
            match cell {
                Cell::Empty | Cell::Percent(None) => {}
                Cell::Text(s) => {
                    ws.write_string(r, col, label(tr, s))?;
                }
                Cell::Date(d) => {
                    // A real date, so owners can sort and filter by it (N3);
                    // text only outside Excel's calendar.
                    let x = u16::try_from(d.year()).ok().and_then(|y| {
                        ExcelDateTime::from_ymd(
                            y,
                            u8::try_from(d.month()).ok()?,
                            u8::try_from(d.day()).ok()?,
                        )
                        .ok()
                    });
                    match x {
                        Some(x) => ws.write_datetime_with_format(r, col, &x, &day)?,
                        None => ws.write_string(r, col, us_date(*d))?,
                    };
                }
                Cell::Count(n) => {
                    // A count up to u32::MAX is exact in an f64; larger as text.
                    match u32::try_from(*n) {
                        Ok(v) => ws.write_number(r, col, f64::from(v))?,
                        Err(_) => ws.write_string(r, col, n.to_string())?,
                    };
                }
                Cell::Amount(d) | Cell::Percent(Some(d)) | Cell::Quantity(d) => {
                    let fmt = match (cell, kind) {
                        (Cell::Percent(_), _) | (_, ColumnKind::Percent) => &pct,
                        (Cell::Amount(_), _) if strong => &money_bold,
                        (Cell::Amount(_), _) => &money,
                        _ => &Format::new(),
                    };
                    if let Some(f) = xlsx_number(*d) {
                        ws.write_number_with_format(r, col, f, fmt)?;
                    } else {
                        text_fallbacks += 1;
                        ws.write_string(r, col, canonical_decimal(*d))?;
                    }
                }
            }
        }
        r += 1;
    }
    if text_fallbacks > 0 {
        r += 1;
        let note = tr("reports.export.text_fallback").replace("{n}", &text_fallbacks.to_string());
        ws.write_string(r, 0, note)?;
    }
    ws.set_column_width(0, 40)?;
    Ok((wb.save_to_buffer()?, text_fallbacks))
}

fn display_cell(c: &Cell, tr: &dyn Fn(&str) -> String) -> String {
    match c {
        Cell::Empty => String::new(),
        Cell::Text(s) => label(tr, s),
        Cell::Amount(d) => cl_core::report::display_amount(*d, MoneyStyle::default()),
        Cell::Percent(p) => cl_core::report::display_percent(*p),
        Cell::Quantity(d) => crate::format_quantity(*d),
        Cell::Date(d) => format!("{:02}/{:02}/{}", d.month(), d.day(), d.year()),
        Cell::Count(n) => n.to_string(),
    }
}

/// A text column whose every cell is at most this long prints whole.
const SHORT_TEXT: usize = 14;

/// The PDF/print layout: one table, captions indented by level.
#[must_use]
pub fn to_report(
    t: &ReportTable,
    rows: &[usize],
    title: &str,
    subtitle: Vec<String>,
    tr: &dyn Fn(&str) -> String,
) -> Report {
    let mut columns = vec![Column {
        heading: String::new(),
        share: 3.0,
        right: false,
        whole: false,
    }];
    // A Report Builder page break starts a new page (A08 QA-13); it is not
    // a line of its own.
    let mut body: Vec<Vec<String>> = Vec::new();
    let mut breaks = Vec::new();
    for r in rows.iter().filter_map(|&i| t.rows.get(i)) {
        if r.kind == cl_core::report::RowKind::PageBreak {
            breaks.push(body.len());
            continue;
        }
        let mut line = vec![format!(
            "{}{}",
            "   ".repeat(usize::from(r.level)),
            label(tr, &r.caption)
        )];
        line.extend(r.cells.iter().map(|c| display_cell(c, tr)));
        body.push(line);
    }
    // Dates, and text columns of short codes (a type, a number), print
    // whole; names and memos may be cut on a wide report (A07 QA-8).
    columns.extend(t.columns.iter().enumerate().map(|(ci, c)| {
        let short = body
            .iter()
            .filter_map(|r| r.get(ci + 1))
            .all(|s| s.chars().count() <= SHORT_TEXT);
        Column {
            heading: label(tr, &c.caption),
            share: 1.2,
            right: c.kind.is_numeric(),
            whole: c.kind == ColumnKind::Date || short,
        }
    }));
    Report {
        title: title.to_string(),
        subtitle,
        sections: vec![Section::Table {
            heading: String::new(),
            columns,
            rows: body,
            empty: tr("reports.empty"),
            breaks,
        }],
        footer: String::new(),
        page_label: tr("reports.page_label"),
    }
}

/// Writes `bytes` to `path` through a temporary file in the same folder and
/// a rename, so a failed export never leaves a partial file.
///
/// # Errors
/// Any I/O error; the temporary file is removed.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), ExportError> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .map_or_else(|| "export".into(), |n| n.to_string_lossy().into_owned());
    let tmp = dir.join(format!(".{name}.partial"));
    let result = (|| -> std::io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    Ok(result?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cl_core::report::{ColumnHead, ReportMeta, ReportRow, RowPath};
    use std::io::Read as _;
    use std::str::FromStr;

    fn d(s: &str) -> Decimal {
        Decimal::from_str(s).expect("decimal")
    }

    fn tr(k: &str) -> String {
        match k {
            "reports.col.total" => "Total".into(),
            "reports.export.text_fallback" => "{n} values were too long for Excel numbers".into(),
            other => other.to_string(),
        }
    }

    fn table() -> ReportTable {
        let head = |key: &str, kind| ColumnHead {
            key: key.into(),
            caption: format!("@reports.col.{key}"),
            kind,
            range: None,
            drill: None,
        };
        let r = |kind, level, caption: &str, cells| ReportRow {
            kind,
            level,
            path: RowPath::of([caption]),
            caption: caption.into(),
            cells,
            drill: None,
            expandable: false,
        };
        ReportTable {
            columns: vec![
                head("total", ColumnKind::Amount),
                head("pct", ColumnKind::Percent),
            ],
            rows: vec![
                r(
                    RowKind::Account,
                    1,
                    "4000 · Installation",
                    vec![Cell::Amount(d("38420.5")), Cell::Percent(Some(d("63.7")))],
                ),
                r(
                    RowKind::Account,
                    1,
                    "Huge",
                    vec![Cell::Amount(d("123456789012345.6789")), Cell::Percent(None)],
                ),
                r(
                    RowKind::Grand,
                    0,
                    "Net income",
                    vec![Cell::Amount(d("-0.1")), Cell::Percent(Some(d("0.0")))],
                ),
            ],
            meta: ReportMeta::default(),
        }
    }

    #[test]
    fn csv_has_exact_figures_and_no_separators() {
        let t = table();
        let csv = to_csv(&t, &[0, 2], &[], &tr);
        assert_eq!(
            csv,
            ",Total,reports.col.pct\n  4000 · Installation,38420.50,63.7\nNet income,-0.10,0\n"
        );
    }

    /// QA-6, QA-9 and the formula nit: header lines first, dates as the app
    /// shows them, no empty first column on a list, and text that a
    /// spreadsheet would run as a formula is neutralised (amounts aren't).
    #[test]
    fn csv_has_a_header_us_dates_and_no_formulas() {
        let t = ReportTable {
            columns: vec![
                ColumnHead {
                    key: "date".into(),
                    caption: "Date".into(),
                    kind: ColumnKind::Date,
                    range: None,
                    drill: None,
                },
                ColumnHead {
                    key: "name".into(),
                    caption: "Name".into(),
                    kind: ColumnKind::Text,
                    range: None,
                    drill: None,
                },
                ColumnHead {
                    key: "amount".into(),
                    caption: "Amount".into(),
                    kind: ColumnKind::Amount,
                    range: None,
                    drill: None,
                },
            ],
            rows: vec![ReportRow {
                kind: RowKind::Detail,
                level: 0,
                path: RowPath::of(["r"]),
                caption: String::new(),
                cells: vec![
                    Cell::Date(cl_core::CalendarDate::from_ymd(2026, 12, 5).expect("date")),
                    Cell::Text("=HYPERLINK(1)".into()),
                    Cell::Amount(d("-64.18")),
                ],
                drill: None,
                expandable: false,
            }],
            meta: ReportMeta::default(),
        };
        // N3: xlsx writes the date as a date (Excel serial, date format).
        let (bytes, _) = to_xlsx(&t, &[0], &[], &tr).expect("xlsx");
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("zip");
        let mut sheet = String::new();
        zip.by_name("xl/worksheets/sheet1.xml")
            .expect("sheet")
            .read_to_string(&mut sheet)
            .expect("read");
        assert!(sheet.contains("<v>46361</v>"), "{sheet}");
        let csv = to_csv(&t, &[0], &["Acme".into(), "Duplicates".into()], &tr);
        assert_eq!(
            csv,
            "Acme\nDuplicates\n\nDate,Name,Amount\n12/05/2026,'=HYPERLINK(1),-64.18\n"
        );
    }

    #[test]
    fn the_float_boundary_round_trips_or_falls_back() {
        assert_eq!(xlsx_number(d("38420.50")), Some(38420.5));
        assert_eq!(xlsx_number(d("-0.10")), Some(-0.1));
        assert_eq!(xlsx_number(d("0.00")), Some(0.0));
        assert_eq!(xlsx_number(d("63.7")), Some(63.7));
        // More than 15 significant digits don't survive an f64.
        assert_eq!(xlsx_number(d("123456789012345.6789")), None);
    }

    /// AC3: the workbook keeps numbers numeric with the amount and percent
    /// formats, and the text fallback is counted.
    #[test]
    fn xlsx_numbers_stay_numeric() {
        let t = table();
        let (bytes, fallbacks) =
            to_xlsx(&t, &[0, 1, 2], &["Profit & Loss".into()], &tr).expect("xlsx");
        assert_eq!(fallbacks, 1);
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("zip");
        let mut sheet = String::new();
        zip.by_name("xl/worksheets/sheet1.xml")
            .expect("sheet")
            .read_to_string(&mut sheet)
            .expect("read");
        let mut styles = String::new();
        zip.by_name("xl/styles.xml")
            .expect("styles")
            .read_to_string(&mut styles)
            .expect("read");
        // Numeric cells carry the value (no t="s"/t="str").
        assert!(sheet.contains("<v>38420.5</v>"), "{sheet}");
        assert!(sheet.contains("<v>63.7</v>"));
        assert!(sheet.contains("<v>-0.1</v>"));
        assert!(styles.contains("#,##0.00;-#,##0.00"), "{styles}");
        assert!(styles.contains("0.0&quot;%&quot;"), "{styles}");
        // The 19-digit value went in as text.
        let mut strings = String::new();
        zip.by_name("xl/sharedStrings.xml")
            .expect("strings")
            .read_to_string(&mut strings)
            .expect("read");
        assert!(strings.contains("123456789012345.6789"));
        assert!(strings.contains("1 values were too long"));
    }

    /// The `<c ...>` tag of the cell holding `value` in `sheet`.
    fn cell_tag<'a>(sheet: &'a str, value: &str) -> &'a str {
        let at = sheet
            .find(&format!("<v>{value}</v>"))
            .unwrap_or_else(|| panic!("{value} in {sheet}"));
        let start = sheet[..at].rfind("<c ").expect("cell");
        let end = start + sheet[start..].find('>').expect("tag end");
        &sheet[start..=end]
    }

    /// The `s="N"` style index of a cell tag.
    fn style_of(tag: &str) -> &str {
        let at = tag.find(" s=\"").map_or(tag.len(), |i| i + 4);
        let rest = &tag[at..];
        &rest[..rest.find('"').unwrap_or(0)]
    }

    /// A07-5 (A09 AC2): bold formula and total rows keep their numbers
    /// numeric, in the bold amount format; an ordinary row is not bold.
    #[test]
    fn bold_formula_and_total_rows_stay_numeric() {
        let mut t = table();
        let r = |kind, caption: &str, v: &str| ReportRow {
            kind,
            level: 0,
            path: RowPath::of([caption]),
            caption: caption.into(),
            cells: vec![Cell::Amount(d(v)), Cell::Percent(None)],
            drill: None,
            expandable: false,
        };
        t.rows = vec![
            r(RowKind::Account, "Sales", "1200.75"),
            r(RowKind::Formula, "Gross profit", "250.25"),
            r(RowKind::Total, "Total expenses", "-40.1"),
            r(RowKind::Subtotal, "Subtotal", "77.5"),
        ];
        let (bytes, fallbacks) = to_xlsx(&t, &[0, 1, 2, 3], &[], &tr).expect("xlsx");
        assert_eq!(fallbacks, 0);
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("zip");
        let mut sheet = String::new();
        zip.by_name("xl/worksheets/sheet1.xml")
            .expect("sheet")
            .read_to_string(&mut sheet)
            .expect("read");
        let plain = cell_tag(&sheet, "1200.75");
        let formula = cell_tag(&sheet, "250.25");
        let total = cell_tag(&sheet, "-40.1");
        let subtotal = cell_tag(&sheet, "77.5");
        for tag in [plain, formula, total, subtotal] {
            // A number: no shared-string or inline-string type.
            assert!(!tag.contains(" t=\""), "{tag}");
        }
        assert_eq!(style_of(formula), style_of(total), "{formula} {total}");
        assert_eq!(style_of(formula), style_of(subtotal));
        assert_ne!(style_of(formula), style_of(plain), "{formula} {plain}");
        // That style is the amount format, bold.
        let mut styles = String::new();
        zip.by_name("xl/styles.xml")
            .expect("styles")
            .read_to_string(&mut styles)
            .expect("read");
        let xfs = &styles[styles.find("<cellXfs").expect("xfs")..];
        let n: usize = style_of(formula).parse().expect("index");
        let xf = xfs.split("<xf ").nth(n + 1).expect("xf");
        let font: usize = xf
            .split("fontId=\"")
            .nth(1)
            .and_then(|x| x.split('"').next())
            .and_then(|x| x.parse().ok())
            .expect("font");
        let fonts = &styles[styles.find("<fonts").expect("fonts")..];
        let f = fonts.split("<font>").nth(font + 1).expect("font entry");
        assert!(
            f.split("</font>")
                .next()
                .is_some_and(|f| f.contains("<b/>")),
            "{f}"
        );
        assert!(xf.contains("numFmtId=\"16"), "{xf}");
    }

    #[test]
    fn the_pdf_layout_uses_display_formatting() {
        let r = to_report(&table(), &[0, 2], "Profit & Loss", vec![], &tr);
        let Section::Table { rows, columns, .. } = &r.sections[0] else {
            panic!("table")
        };
        assert_eq!(columns[1].heading, "Total");
        assert!(columns[1].right);
        assert_eq!(
            rows[0],
            vec!["   4000 · Installation", "38,420.50", "63.7%"]
        );
        assert_eq!(rows[1], vec!["Net income", "-0.10", "0.0%"]);
    }

    fn heading(key: &str, kind: ColumnKind) -> ColumnHead {
        ColumnHead {
            key: key.into(),
            caption: key.into(),
            kind,
            range: None,
            drill: None,
        }
    }

    /// The printed text of `r` (every page).
    fn printed(r: &Report) -> Vec<String> {
        crate::layout::resolve(&crate::reports::layout_report(r))
            .expect("every figure fits")
            .into_iter()
            .flat_map(|p| p.runs.into_iter().map(|run| run.text))
            .collect()
    }

    /// A07 QA-8: a 13-column monthly P&L (12 months and a Total) with
    /// 6-figure negatives builds a PDF (it failed with `DoesNotFit`, shown
    /// as "the file couldn't be saved"), every figure whole.
    #[test]
    fn a_thirteen_column_pl_builds_a_pdf() {
        let mut columns: Vec<ColumnHead> = (1..=12)
            .map(|m| heading(&format!("m{m}"), ColumnKind::Amount))
            .collect();
        columns.push(heading("total", ColumnKind::Amount));
        let row = |caption: &str, figure: &str| ReportRow {
            kind: RowKind::Account,
            level: 1,
            path: RowPath::of([caption]),
            caption: caption.into(),
            cells: (0..13).map(|_| Cell::Amount(d(figure))).collect(),
            drill: None,
            expandable: false,
        };
        let t = ReportTable {
            columns,
            rows: vec![
                row("4000 · Service Income", "1140832.39"),
                row("30170 · Owner's Draw", "-353267.44"),
            ],
            meta: ReportMeta::default(),
        };
        let r = to_report(&t, &[0, 1], "Profit and Loss by Month", vec![], &tr);
        let text = printed(&r);
        assert_eq!(text.iter().filter(|s| *s == "1,140,832.39").count(), 13);
        assert_eq!(text.iter().filter(|s| *s == "-353,267.44").count(), 13);
        assert!(crate::reports::render_report(&r).is_ok());
    }

    /// A07 QA-8: a long transaction detail builds a PDF; dates print as
    /// MM/DD/YYYY (they were cut to "11/05/20") and short codes whole.
    #[test]
    fn a_long_transaction_detail_prints_whole_dates() {
        let columns = vec![
            heading("Date", ColumnKind::Date),
            heading("Type", ColumnKind::Text),
            heading("Num", ColumnKind::Text),
            heading("Name", ColumnKind::Text),
            heading("Memo", ColumnKind::Text),
            heading("Account", ColumnKind::Text),
            heading("Debit", ColumnKind::Amount),
            heading("Credit", ColumnKind::Amount),
            heading("Amount", ColumnKind::Amount),
            heading("Balance", ColumnKind::Amount),
        ];
        let day = cl_core::CalendarDate::from_ymd(2026, 11, 5).expect("date");
        let rows: Vec<ReportRow> = (0..400)
            .map(|i| ReportRow {
                kind: RowKind::Detail,
                level: 2,
                path: RowPath::of([format!("r{i}")]),
                caption: String::new(),
                cells: vec![
                    Cell::Date(day),
                    Cell::Text("Journal entry".into()),
                    Cell::Text(format!("{}", 10_000 + i)),
                    Cell::Text("Riverbend Table Rock Landscaping Partners".into()),
                    Cell::Text("Plants and soil for the north lot, second delivery".into()),
                    Cell::Text("5100 · Plants and Nursery Stock".into()),
                    Cell::Amount(d("-26531.68")),
                    Cell::Empty,
                    Cell::Amount(d("-26531.68")),
                    Cell::Amount(d("1186310.55")),
                ],
                drill: None,
                expandable: false,
            })
            .collect();
        let t = ReportTable {
            columns,
            rows,
            meta: ReportMeta::default(),
        };
        let all: Vec<usize> = (0..400).collect();
        let r = to_report(&t, &all, "Transaction Detail by Account", vec![], &tr);
        let text = printed(&r);
        assert_eq!(text.iter().filter(|s| *s == "11/05/2026").count(), 400);
        assert_eq!(text.iter().filter(|s| *s == "Journal entry").count(), 400);
        assert_eq!(text.iter().filter(|s| *s == "1,186,310.55").count(), 400);
        assert!(!text.iter().any(|s| s == "11/05/20"));
        let pdf = crate::reports::render_report(&r).expect("pdf");
        assert!(pdf.starts_with(b"%PDF"));
    }

    #[test]
    fn an_atomic_write_leaves_no_partial_file() {
        let dir = tempfile::tempdir().expect("tmp");
        let path = dir.path().join("report.csv");
        write_atomic(&path, b"a,b\n").expect("write");
        assert_eq!(std::fs::read(&path).expect("read"), b"a,b\n");
        assert!(!dir.path().join(".report.csv.partial").exists());
        // Into a folder that doesn't exist: an error, nothing left behind.
        let bad = dir.path().join("missing").join("x.csv");
        assert!(write_atomic(&bad, b"x").is_err());
    }

    /// A08 QA-13: a Report Builder page break starts a new PDF page and is
    /// not a line of its own.
    #[test]
    fn a_page_break_row_starts_a_new_page() {
        let mut t = table();
        let at = t.rows.len();
        let mut br = t.rows[0].clone();
        br.kind = cl_core::report::RowKind::PageBreak;
        br.caption = String::new();
        br.cells = vec![Cell::Empty, Cell::Empty];
        t.rows.push(br);
        let after = t.rows[0].clone();
        t.rows.push(after);
        let all: Vec<usize> = (0..t.rows.len()).collect();
        let r = to_report(&t, &all, "Title", Vec::new(), &tr);
        let Section::Table { rows, breaks, .. } = &r.sections[0] else {
            panic!("table");
        };
        assert_eq!(rows.len(), t.rows.len() - 1, "the break is no line");
        assert_eq!(breaks, &[at]);
        let doc = crate::reports::layout_report(&r);
        assert_eq!(doc.pages.len(), 2, "the row after it is on page 2");
        let mut none = r.clone();
        if let Section::Table { breaks, .. } = &mut none.sections[0] {
            breaks.clear();
        }
        assert_eq!(crate::reports::layout_report(&none).pages.len(), 1);
    }
}
