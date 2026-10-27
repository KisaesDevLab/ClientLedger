//! Statement lines read from a document into the paste path (A06, plan
//! decision b, Q-296).
//!
//! The reader's lines become the text the owner could have pasted
//! ([`rows_from_read`]): one tab-separated row per line, date, ref. no.,
//! amount and a kind word when the column said it. [`super::parse_paste`]
//! reads that text exactly as it reads typed lines, so Match runs
//! unchanged and the owner can still edit any line by typing. The text is
//! built so that reading it back gives the read lines (the round trip is
//! tested); a line the paste format can't hold exactly (a zero amount, or
//! more than two decimals) is written as printed and comes back as a bad
//! row for the owner to fix, never rounded or dropped.
//!
//! Each read line keeps a mark ([`ReadMark`]): read, not accepted, and
//! whether the reader was unsure. Match waits until every line is
//! accepted ([`ReadStatement::all_accepted`]), one Accept for all or line
//! by line (ADD-02 step 3). Typing in the box takes the lines over: they
//! are the owner's then, and the marks go ([`ReadStatement::still_read`]).

use std::fmt;

use cl_core::{CalendarDate, Money};

use super::{StatementKind, parse_paste};
use crate::date_entry::DateOrder;

/// Which column a read line's amount came from (the reader's `SignSource`;
/// this crate can't depend on `cl-docread`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LineColumn {
    /// One amount column: the printed sign decides (positive a charge,
    /// negative a credit or payment).
    Single,
    /// A charges column.
    Charges,
    /// A credits column.
    Credits,
    /// A payments column.
    Payments,
}

/// One line as read from the statement. `amount` is signed (a charge
/// positive). `Debug` is redacted.
#[derive(Clone, PartialEq, Eq)]
pub struct ReadLine {
    /// Date printed on the line.
    pub date: CalendarDate,
    /// Ref. no. printed on the line (empty when none).
    pub reference: String,
    /// The amount, signed as the column says.
    pub amount: Money,
    /// The column it came from.
    pub column: LineColumn,
    /// The reader was not sure of a value on this line.
    pub unsure: bool,
}

impl fmt::Debug for ReadLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReadLine")
            .field("column", &self.column)
            .field("unsure", &self.unsure)
            .finish_non_exhaustive()
    }
}

/// A read line's state in the box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadMark {
    /// Its line number in the box text (from 1), as `PastedRow::line`.
    pub line: usize,
    /// Index into the read lines (its spot on the page).
    pub source: usize,
    /// The reader was not sure: marked more strongly.
    pub unsure: bool,
    /// The owner accepted it.
    pub accepted: bool,
}

/// The read lines as box text, with their marks.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct ReadStatement {
    /// The box text (one row per read line).
    pub text: String,
    /// One mark per read line, in text order.
    pub marks: Vec<ReadMark>,
}

impl fmt::Debug for ReadStatement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReadStatement")
            .field("lines", &self.marks.len())
            .field("accepted", &self.marks.iter().filter(|m| m.accepted).count())
            .finish_non_exhaustive()
    }
}

impl ReadStatement {
    /// Accept every line.
    pub fn accept_all(&mut self) {
        for m in &mut self.marks {
            m.accepted = true;
        }
    }

    /// Accept the line on box line `line`; false when there is none.
    pub fn accept(&mut self, line: usize) -> bool {
        match self.marks.iter_mut().find(|m| m.line == line) {
            Some(m) => {
                m.accepted = true;
                true
            }
            None => false,
        }
    }

    /// Every read line is accepted (Match may run).
    #[must_use]
    pub fn all_accepted(&self) -> bool {
        self.marks.iter().all(|m| m.accepted)
    }

    /// Read lines not accepted yet.
    #[must_use]
    pub fn pending(&self) -> usize {
        self.marks.iter().filter(|m| !m.accepted).count()
    }

    /// The box still holds the read text (the owner hasn't typed in it).
    #[must_use]
    pub fn still_read(&self, box_text: &str) -> bool {
        box_text == self.text
    }

    /// The mark of box line `line`.
    #[must_use]
    pub fn mark_of(&self, line: usize) -> Option<&ReadMark> {
        self.marks.iter().find(|m| m.line == line)
    }
}

/// The amount as the box shows it: its size, with two decimals when it
/// has at most two, else every decimal it has (the paste reader then
/// refuses it, and the owner fixes it).
fn amount_text(m: Money) -> String {
    let units = m.abs().to_ten_thousandths();
    let (whole, frac) = (units / 10_000, units % 10_000);
    if frac % 100 == 0 {
        format!("{whole}.{:02}", frac / 100)
    } else {
        let digits = format!("{frac:04}");
        format!("{whole}.{}", digits.trim_end_matches('0'))
    }
}

/// A cell can't hold a tab or a line break (they split the box text).
fn cell_text(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .trim()
        .to_string()
}

fn kind_word(column: LineColumn) -> Option<&'static str> {
    match column {
        LineColumn::Single => None,
        LineColumn::Charges => Some("charge"),
        LineColumn::Credits => Some("credit"),
        LineColumn::Payments => Some("payment"),
    }
}

/// The kind a read line becomes in the paste reader.
#[must_use]
pub fn kind_of(line: &ReadLine) -> StatementKind {
    match line.column {
        LineColumn::Charges => StatementKind::Charge,
        LineColumn::Credits => StatementKind::Credit,
        LineColumn::Payments => StatementKind::Payment,
        LineColumn::Single if line.amount.is_negative() => StatementKind::CreditOrPayment,
        LineColumn::Single => StatementKind::Charge,
    }
}

/// The read lines as box text with their marks (see the module docs).
/// Nothing is accepted yet.
#[must_use]
pub fn rows_from_read(lines: &[ReadLine]) -> ReadStatement {
    let mut text = String::new();
    let mut marks = Vec::with_capacity(lines.len());
    for (i, l) in lines.iter().enumerate() {
        let date = format!(
            "{:02}/{:02}/{:04}",
            l.date.month(),
            l.date.day(),
            l.date.year()
        );
        let amount = match l.column {
            // A single column keeps its sign: the paste reader's rule.
            LineColumn::Single if l.amount.is_negative() => format!("-{}", amount_text(l.amount)),
            _ => amount_text(l.amount),
        };
        text.push_str(&date);
        text.push('\t');
        text.push_str(&cell_text(&l.reference));
        text.push('\t');
        text.push_str(&amount);
        if let Some(k) = kind_word(l.column) {
            text.push('\t');
            text.push_str(k);
        }
        text.push('\n');
        marks.push(ReadMark {
            line: i + 1,
            source: i,
            unsure: l.unsure,
            accepted: false,
        });
    }
    ReadStatement { text, marks }
}

/// Reads the box text back as the paste reader does (US dates).
#[must_use]
pub fn read_back(statement: &ReadStatement, today: CalendarDate) -> super::ParsedPaste {
    parse_paste(&statement.text, DateOrder::Mdy, today)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::statement_match::{PasteProblem, StatementLine};

    fn d(m: u32, day: u32) -> CalendarDate {
        CalendarDate::from_ymd(2026, m, day).expect("date")
    }

    fn money(s: &str) -> Money {
        Money::parse_wire(s).expect("money")
    }

    fn line(r: &str, amount: &str, column: LineColumn) -> ReadLine {
        ReadLine {
            date: d(10, 3),
            reference: r.into(),
            amount: money(amount),
            column,
            unsure: false,
        }
    }

    /// Read lines go through the paste path exactly: reading the box text
    /// back gives the same lines (date, ref., kind, size), one row each,
    /// numbered as the marks.
    #[test]
    fn the_box_text_reads_back_as_the_read_lines() {
        let lines = vec![
            line("INV-1", "1330.00", LineColumn::Charges),
            line("CM-7", "-20.00", LineColumn::Credits),
            line("1001", "-1500.00", LineColumn::Payments),
            line("INV-2", "265.5", LineColumn::Single),
            line("", "-75.00", LineColumn::Single),
            line("A\tB\nC", "0.01", LineColumn::Single),
            line("INV-9", "999999999.99", LineColumn::Charges),
        ];
        let st = rows_from_read(&lines);
        let back = read_back(&st, d(11, 4));
        assert!(back.bad.is_empty(), "{:?}", back.bad);
        assert!(!back.header_skipped);
        assert_eq!(back.rows.len(), lines.len());
        for ((row, mark), l) in back.rows.iter().zip(&st.marks).zip(&lines) {
            assert_eq!(row.line, mark.line);
            assert_eq!(
                row.entry,
                StatementLine {
                    date: l.date,
                    reference: cell_text(&l.reference),
                    kind: kind_of(l),
                    amount: l.amount.abs(),
                }
            );
        }
        assert_eq!(back.rows[3].entry.kind, StatementKind::Charge);
        assert_eq!(back.rows[4].entry.kind, StatementKind::CreditOrPayment);
        assert_eq!(back.rows[5].entry.reference, "A B C");
    }

    /// What the paste format can't hold exactly comes back as a bad row:
    /// never rounded, never dropped.
    #[test]
    fn inexact_lines_are_flagged_not_rounded() {
        let lines = vec![
            line("INV-1", "12.345", LineColumn::Charges),
            line("INV-2", "0", LineColumn::Single),
            line("INV-3", "5.00", LineColumn::Charges),
        ];
        let st = rows_from_read(&lines);
        assert!(st.text.contains("12.345"), "written as printed");
        let back = read_back(&st, d(11, 4));
        assert_eq!(back.rows.len(), 1);
        assert_eq!(
            back.bad.iter().map(|b| (b.line, b.problem)).collect::<Vec<_>>(),
            vec![(1, PasteProblem::Amount), (2, PasteProblem::ZeroAmount)]
        );
    }

    #[test]
    fn accept_all_or_line_by_line() {
        let mut lines = vec![
            line("INV-1", "10.00", LineColumn::Charges),
            line("INV-2", "20.00", LineColumn::Charges),
        ];
        lines[1].unsure = true;
        let mut st = rows_from_read(&lines);
        assert_eq!(st.pending(), 2);
        assert!(!st.all_accepted());
        assert!(st.mark_of(2).is_some_and(|m| m.unsure));
        assert!(st.accept(1));
        assert!(!st.accept(9), "no such line");
        assert_eq!(st.pending(), 1);
        st.accept_all();
        assert!(st.all_accepted());
        assert!(st.still_read(&st.text.clone()));
        assert!(!st.still_read("typed over"));
        assert!(rows_from_read(&[]).all_accepted(), "nothing read");
        let shown = format!("{st:?} {:?}", lines[0]);
        assert!(!shown.contains("INV") && !shown.contains("10.00"), "{shown}");
    }
}
