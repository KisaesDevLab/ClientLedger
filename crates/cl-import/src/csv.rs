//! A small RFC 4180 CSV reader and writer for list import and export
//! (P11 step 6).
//!
//! Reading: UTF-8 with or without a byte-order mark, falling back to
//! Windows-1252 for files saved by older spreadsheet programs; comma,
//! semicolon or tab separated (sniffed from the first line); quoted fields
//! with doubled quotes and line breaks inside; CRLF, LF or CR line ends;
//! blank lines skipped. Every value is kept as text: amounts are parsed
//! later, exactly, by the list's mapping (never as floating point).
//!
//! Writing: fields are quoted when they hold the separator, a quote or a
//! line break; [`cell_text`] neutralizes text that a spreadsheet would run
//! as a formula.

use thiserror::Error;

/// Most data rows read from one file.
pub const MAX_ROWS: usize = 100_000;
/// Most columns in one row.
pub const MAX_COLUMNS: usize = 256;
/// Longest single field, in bytes.
pub const MAX_FIELD_BYTES: usize = 64 * 1024;

/// Why a file could not be read.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CsvError {
    /// The file has no rows.
    #[error("the file is empty")]
    Empty,
    /// A quoted field never ends.
    #[error("a quoted value starting on line {line} never ends")]
    UnclosedQuote {
        /// Line (1-based) the quote opened on.
        line: usize,
    },
    /// More rows than [`MAX_ROWS`].
    #[error("the file has more than {MAX_ROWS} rows")]
    TooManyRows,
    /// A row with more than [`MAX_COLUMNS`] columns.
    #[error("line {line} has more than {MAX_COLUMNS} columns")]
    TooManyColumns {
        /// Line (1-based).
        line: usize,
    },
    /// A field longer than [`MAX_FIELD_BYTES`].
    #[error("line {line} has a value that is too long")]
    FieldTooLong {
        /// Line (1-based).
        line: usize,
    },
}

/// One row as read, with the line it started on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// Line (1-based) the row starts on, for messages.
    pub line: usize,
    /// The values, as text.
    pub fields: Vec<String>,
}

/// Bytes from disk as text: UTF-8 (a byte-order mark dropped), otherwise
/// Windows-1252.
#[must_use]
pub fn decode(bytes: &[u8]) -> String {
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_owned(),
        Err(_) => bytes.iter().map(|&b| cp1252(b)).collect(),
    }
}

/// Windows-1252 to Unicode (the 0x80–0x9F block differs from Latin-1).
fn cp1252(b: u8) -> char {
    const HIGH: [char; 32] = [
        '\u{20AC}', '\u{FFFD}', '\u{201A}', '\u{0192}', '\u{201E}', '\u{2026}', '\u{2020}',
        '\u{2021}', '\u{02C6}', '\u{2030}', '\u{0160}', '\u{2039}', '\u{0152}', '\u{FFFD}',
        '\u{017D}', '\u{FFFD}', '\u{FFFD}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}',
        '\u{2022}', '\u{2013}', '\u{2014}', '\u{02DC}', '\u{2122}', '\u{0161}', '\u{203A}',
        '\u{0153}', '\u{FFFD}', '\u{017E}', '\u{0178}',
    ];
    if (0x80..0xA0).contains(&b) {
        HIGH[usize::from(b - 0x80)]
    } else {
        char::from(b)
    }
}

/// The separator of `text`: whichever of comma, semicolon and tab appears
/// most often outside quotes on the first line (comma on a tie or none).
#[must_use]
pub fn sniff_separator(text: &str) -> char {
    let mut counts = [0usize; 3];
    let mut quoted = false;
    for c in text.chars() {
        match c {
            '"' => quoted = !quoted,
            '\n' | '\r' if !quoted => break,
            ',' if !quoted => counts[0] += 1,
            ';' if !quoted => counts[1] += 1,
            '\t' if !quoted => counts[2] += 1,
            _ => {}
        }
    }
    let seps = [',', ';', '\t'];
    let mut best = 0;
    for i in 1..3 {
        if counts[i] > counts[best] {
            best = i;
        }
    }
    seps[best]
}

/// Reads every row of `text` (the header too, if any), separator sniffed.
///
/// # Errors
/// [`CsvError`] for an empty file, an unclosed quote or a limit passed.
pub fn parse(text: &str) -> Result<Vec<Row>, CsvError> {
    parse_with(text, sniff_separator(text))
}

/// Reads every row of `text` with the separator `sep`.
///
/// # Errors
/// [`CsvError`] for an empty file, an unclosed quote or a limit passed.
pub fn parse_with(text: &str, sep: char) -> Result<Vec<Row>, CsvError> {
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    let mut rows = Vec::new();
    let mut fields: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut line = 1usize;
    let mut row_line = 1usize;
    let mut quoted = false;
    let mut quote_line = 0usize;
    // True once the current field started with a quote (text after the
    // closing quote is kept, as spreadsheets do).
    let mut chars = text.chars().peekable();
    let mut any_in_row = false;
    while let Some(c) = chars.next() {
        if quoted {
            match c {
                '"' if chars.peek() == Some(&'"') => {
                    chars.next();
                    field.push('"');
                }
                '"' => quoted = false,
                '\r' => {
                    if chars.peek() == Some(&'\n') {
                        chars.next();
                    }
                    line += 1;
                    field.push('\n');
                }
                '\n' => {
                    line += 1;
                    field.push('\n');
                }
                other => field.push(other),
            }
            if field.len() > MAX_FIELD_BYTES {
                return Err(CsvError::FieldTooLong { line: quote_line });
            }
            continue;
        }
        match c {
            '"' if field.is_empty() => {
                quoted = true;
                quote_line = line;
                any_in_row = true;
            }
            '\r' | '\n' => {
                if c == '\r' && chars.peek() == Some(&'\n') {
                    chars.next();
                }
                end_row(&mut rows, &mut fields, &mut field, any_in_row, row_line)?;
                any_in_row = false;
                line += 1;
                row_line = line;
            }
            c if c == sep => {
                fields.push(std::mem::take(&mut field));
                any_in_row = true;
                if fields.len() > MAX_COLUMNS {
                    return Err(CsvError::TooManyColumns { line: row_line });
                }
            }
            other => {
                field.push(other);
                any_in_row = true;
                if field.len() > MAX_FIELD_BYTES {
                    return Err(CsvError::FieldTooLong { line: row_line });
                }
            }
        }
    }
    if quoted {
        return Err(CsvError::UnclosedQuote { line: quote_line });
    }
    end_row(&mut rows, &mut fields, &mut field, any_in_row, row_line)?;
    if rows.is_empty() {
        return Err(CsvError::Empty);
    }
    Ok(rows)
}

fn end_row(
    rows: &mut Vec<Row>,
    fields: &mut Vec<String>,
    field: &mut String,
    any: bool,
    line: usize,
) -> Result<(), CsvError> {
    if !any {
        return Ok(());
    }
    fields.push(std::mem::take(field));
    if fields.len() > MAX_COLUMNS {
        return Err(CsvError::TooManyColumns { line });
    }
    if fields.iter().all(|f| f.trim().is_empty()) {
        fields.clear();
        return Ok(());
    }
    if rows.len() > MAX_ROWS {
        return Err(CsvError::TooManyRows);
    }
    rows.push(Row {
        line,
        fields: std::mem::take(fields),
    });
    Ok(())
}

/// One field as written: quoted when it holds a comma, a quote or a line
/// break, or starts or ends with a space.
#[must_use]
pub fn field(value: &str) -> String {
    let needs = value.contains([',', '"', '\n', '\r', ';', '\t'])
        || value.starts_with(' ')
        || value.ends_with(' ');
    if needs {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_owned()
    }
}

/// Text a spreadsheet would run as a formula (a leading `=`, `+`, `-`,
/// `@`, `|`, `%`, tab or carriage return) gets a leading apostrophe. Only a
/// whole plain number (`-12.50`, `^-?\d+(\.\d+)?$`) is left as it is, so
/// `-2+3+cmd|...` is still neutralized. Use it for names and notes, never
/// for amounts.
#[must_use]
pub fn cell_text(value: &str) -> String {
    let risky = matches!(
        value.chars().next(),
        Some('=' | '+' | '-' | '@' | '|' | '%' | '\t' | '\r')
    ) && !plain_number(value);
    if risky {
        format!("'{value}")
    } else {
        value.to_owned()
    }
}

/// `^-?\d+(\.\d+)?$`.
fn plain_number(s: &str) -> bool {
    let s = s.strip_prefix('-').unwrap_or(s);
    let (whole, frac) = match s.split_once('.') {
        Some((w, f)) => (w, Some(f)),
        None => (s, None),
    };
    let digits = |t: &str| !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit());
    digits(whole) && frac.is_none_or(digits)
}

/// A whole file: comma separated, CRLF line ends (what spreadsheet
/// programs expect), a final line end.
#[must_use]
pub fn write<R, F>(rows: R) -> String
where
    R: IntoIterator<Item = F>,
    F: IntoIterator,
    F::Item: AsRef<str>,
{
    let mut out = String::new();
    for row in rows {
        let mut first = true;
        for f in row {
            if !first {
                out.push(',');
            }
            first = false;
            out.push_str(&field(f.as_ref()));
        }
        out.push_str("\r\n");
    }
    out
}

#[cfg(test)]
mod tests;
