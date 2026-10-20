//! A two-line register of any size, computed from the row index (no stored
//! rows, S9), that counts `cell` calls so tests can prove the grid only
//! reads visible rows.

use std::borrow::Cow;
use std::cell::Cell;

use cl_core::{CalendarDate, Money};
use cl_grid::{
    Align, CellKind, CellValue, Column, ColumnId, GridSpec, Line, MarkKind, RowKey, RowModel,
    RowState, TextKey, WidthSpec,
};

pub const DATE: ColumnId = ColumnId(1);
pub const NUMBER: ColumnId = ColumnId(2);
pub const TYPE: ColumnId = ColumnId(3);
pub const PAYEE: ColumnId = ColumnId(4);
pub const MEMO: ColumnId = ColumnId(5);
pub const PAYMENT: ColumnId = ColumnId(6);
pub const MARK: ColumnId = ColumnId(7);
pub const DEPOSIT: ColumnId = ColumnId(8);
pub const BALANCE: ColumnId = ColumnId(9);

/// `SplitMix64` of the index: any row in O(1).
fn mix(i: u64) -> u64 {
    let mut z = i.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

pub struct Register {
    pub n: usize,
    pub calls: Cell<usize>,
    pub states: Vec<(usize, RowState)>,
    /// This row's payment is the most negative amount the engine holds.
    pub big: Option<usize>,
    /// Rows with a caption note (measured grids).
    pub notes: Vec<usize>,
}

impl Register {
    pub fn new(n: usize) -> Register {
        Register {
            n,
            calls: Cell::new(0),
            states: Vec::new(),
            big: None,
            notes: Vec::new(),
        }
    }

    fn payment(i: usize) -> Money {
        Money::from_cents(i64::try_from(mix(i as u64) % 900_000).unwrap_or(0)).expect("cents")
    }
}

impl RowModel for Register {
    fn revision(&self) -> u64 {
        0
    }
    fn row_count(&self) -> usize {
        self.n
    }
    fn row_key(&self, index: usize) -> Option<RowKey> {
        (index < self.n).then(|| RowKey(index as u64 + 1))
    }
    fn index_of(&self, key: RowKey) -> Option<usize> {
        usize::try_from(key.0)
            .ok()
            .and_then(|k| k.checked_sub(1))
            .filter(|i| *i < self.n)
    }
    fn row_state(&self, index: usize) -> RowState {
        self.states
            .iter()
            .find(|(i, _)| *i == index)
            .map_or(RowState::empty(), |(_, s)| *s)
    }
    fn cell(&self, index: usize, col: ColumnId) -> CellValue<'_> {
        self.calls.set(self.calls.get() + 1);
        let r = mix(index as u64);
        match col {
            DATE => CellValue::Date(
                CalendarDate::from_ymd(
                    2026,
                    1 + u32::try_from(r % 12).unwrap_or(0),
                    1 + u32::try_from(r % 28).unwrap_or(0),
                )
                .expect("date"),
            ),
            NUMBER => CellValue::Text(Cow::Owned(format!("{}", 1000 + r % 9000))),
            TYPE => CellValue::Text(Cow::Borrowed("CHK")),
            PAYEE => CellValue::Text(Cow::Owned(format!("Ozark Feed & Seed {}", r % 97))),
            MEMO => CellValue::Text(Cow::Borrowed("5100 · Materials:Plants · line memo")),
            PAYMENT if self.big == Some(index) => CellValue::Money(Money::MIN),
            PAYMENT => CellValue::Money(Register::payment(index)),
            MARK => CellValue::Mark(if r.is_multiple_of(3) {
                MarkKind::Cleared
            } else {
                MarkKind::None
            }),
            BALANCE => CellValue::Money(
                Money::from_cents(i64::try_from(r % 90_000_000).unwrap_or(0)).expect("cents"),
            ),
            _ => CellValue::Empty,
        }
    }
    fn row_note(&self, index: usize) -> Option<cl_grid::RowNote<'_>> {
        self.notes
            .contains(&index)
            .then_some(cl_grid::RowNote::Caption(Cow::Borrowed(
                "Suggested from 4 earlier checks",
            )))
    }
    fn row_label(&self, index: usize) -> String {
        format!("Check {}", 1000 + mix(index as u64) % 9000)
    }
}

#[allow(clippy::too_many_arguments)] // a compact column table
fn col(
    id: ColumnId,
    header: &'static str,
    line: Line,
    width: u16,
    flex: u8,
    mono: bool,
    align: Align,
    kind: CellKind,
) -> Column {
    Column {
        id,
        header: TextKey(header),
        placeholder: None,
        empty: None,
        help: None,
        kind,
        line,
        align,
        width: WidthSpec {
            min: width.min(60),
            preferred: width,
            flex,
        },
        editable: false,
        mono,
    }
}

pub fn spec() -> GridSpec {
    use Align::{Center, Left, Right};
    use CellKind::{Display, Mark};
    GridSpec::browse(vec![
        col(DATE, "Date", Line::First, 100, 0, true, Left, Display),
        col(NUMBER, "Number", Line::First, 80, 0, true, Left, Display),
        col(TYPE, "Type", Line::Second, 80, 0, false, Left, Display),
        col(PAYEE, "Payee", Line::First, 260, 3, false, Left, Display),
        col(
            MEMO,
            "Account · Memo",
            Line::Second,
            260,
            1,
            false,
            Left,
            Display,
        ),
        col(
            PAYMENT,
            "Payment",
            Line::First,
            120,
            0,
            true,
            Right,
            Display,
        ),
        col(MARK, "Cleared", Line::First, 30, 0, false, Center, Mark),
        col(
            DEPOSIT,
            "Deposit",
            Line::First,
            120,
            0,
            true,
            Right,
            Display,
        ),
        col(
            BALANCE,
            "Balance",
            Line::First,
            130,
            0,
            true,
            Right,
            Display,
        ),
    ])
}
