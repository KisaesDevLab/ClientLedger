//! The grid demo's register (200,000 two-line rows computed from the row
//! index) and its fixed-row "Pay bills" example. All data is synthetic.
//!
//! The running Balance column (Q-187, mockup `Register.html`) is computed
//! once when the register is built: exact `Money` sums in date order from
//! the fixture's opening balance, never per frame. P12 feeds the engine's
//! true running balance into the same read-only column instead.

use std::borrow::Cow;

use cl_core::{CalendarDate, Money};
use cl_grid::{
    Align, CellInput, CellKind, CellPos, CellProblem, CellValue, Column, ColumnId, EditableModel,
    GridSpec, Line, MarkKind, RecordOutcome, RowHeight, RowKey, RowModel, RowState, Source,
    TextKey, WidthSpec,
};
use cl_model::amount_entry::AmountRules;

/// Rows in the demo register.
pub const REGISTER_ROWS: usize = 200_000;

const DATE: ColumnId = ColumnId(1);
const NUMBER: ColumnId = ColumnId(2);
const PAYEE: ColumnId = ColumnId(3);
const PAYMENT: ColumnId = ColumnId(4);
const MARK: ColumnId = ColumnId(5);
const DEPOSIT: ColumnId = ColumnId(6);
const KIND: ColumnId = ColumnId(7);
const ACCOUNT: ColumnId = ColumnId(8);
const BALANCE: ColumnId = ColumnId(9);

/// The fixture's opening balance (synthetic).
const OPENING_CENTS: i64 = 2_500_000;

const PAYEES: &[&str] = &[
    "Ozark Feed & Seed",
    "Hollister Family",
    "Ridgeview HOA",
    "Bentonville Dental",
    "Casey's Fuel",
    "Northwest Equipment Rental",
    "Springdale Stone Supply",
];
const ACCOUNTS: &[&str] = &[
    "5100 \u{b7} Materials:Plants",
    "5110 \u{b7} Materials:Mulch & stone",
    "6200 \u{b7} Fuel",
    "6210 \u{b7} Equipment rental",
    "4000 \u{b7} Landscaping income",
];
/// Synthetic memos; an empty one leaves the account alone on line two.
const MEMOS: &[&str] = &[
    "",
    "September fuel",
    "Skid steer, 3 days",
    "Fall install",
    "",
    "Hardwood mulch",
    "Invoice 1084",
];

/// SplitMix64: any row's values from its index alone (S9).
fn mix(i: u64) -> u64 {
    let mut z = i.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn pick<'a>(list: &[&'a str], h: u64) -> &'a str {
    let n = u64::try_from(list.len()).unwrap_or(1).max(1);
    list[usize::try_from(h % n).unwrap_or(0)]
}

fn column(id: ColumnId, header: TextKey, kind: CellKind, line: Line, w: u16) -> Column {
    Column {
        id,
        header,
        placeholder: None,
        help: None,
        kind,
        line,
        align: Align::Left,
        width: WidthSpec {
            min: w.min(80),
            preferred: w,
            flex: u8::from(w > 150),
        },
        editable: false,
        mono: false,
    }
}

/// The register spec; one line puts every column on the first line.
#[must_use]
pub fn spec(one_line: bool) -> GridSpec {
    let second = if one_line { Line::First } else { Line::Second };
    let money = CellKind::Amount(AmountRules::MONEY);
    let right = |c: Column| Column {
        align: Align::Right,
        mono: true,
        ..c
    };
    let mut s = GridSpec::browse(vec![
        Column {
            mono: true,
            ..column(
                DATE,
                TextKey("dev.grid_demo.col.date"),
                CellKind::Date,
                Line::First,
                100,
            )
        },
        column(
            NUMBER,
            TextKey("dev.grid_demo.col.number"),
            CellKind::Display,
            Line::First,
            80,
        ),
        column(
            KIND,
            TextKey("dev.grid_demo.col.type"),
            CellKind::Display,
            second,
            80,
        ),
        column(
            PAYEE,
            TextKey("dev.grid_demo.col.payee"),
            CellKind::Display,
            Line::First,
            220,
        ),
        column(
            ACCOUNT,
            TextKey("dev.grid_demo.col.account_memo"),
            CellKind::Display,
            second,
            220,
        ),
        right(column(
            PAYMENT,
            TextKey("dev.grid_demo.col.payment"),
            money,
            Line::First,
            110,
        )),
        Column {
            align: Align::Center,
            help: Some(TextKey("dev.grid_demo.col.mark_help")),
            ..column(
                MARK,
                TextKey("dev.grid_demo.col.mark"),
                CellKind::Mark,
                Line::First,
                40,
            )
        },
        right(column(
            DEPOSIT,
            TextKey("dev.grid_demo.col.deposit"),
            money,
            Line::First,
            110,
        )),
        // Read-only (Browse grids never edit; `editable: false` also keeps
        // it locked and out of the Tab order in editable grids).
        right(column(
            BALANCE,
            TextKey("dev.grid_demo.col.balance"),
            money,
            Line::First,
            120,
        )),
    ]);
    s.height = RowHeight::Uniform {
        two_lines: !one_line,
    };
    s
}

/// The demo register: row `i` is computed from `i`, except the running
/// balance, which is summed once here.
#[derive(Debug, Clone)]
pub struct Register {
    rows: usize,
    start: CalendarDate,
    /// The balance after each row (`None` past an overflow, which this
    /// fixture never reaches).
    balances: Vec<Option<Money>>,
}

impl Register {
    /// A register of `rows` rows ending near `today`.
    #[must_use]
    pub fn new(rows: usize, today: CalendarDate) -> Register {
        let back = i64::try_from(rows / 40).unwrap_or(0);
        let mut running = Money::from_cents(OPENING_CENTS).ok();
        let balances = (0..rows)
            .map(|i| {
                let (payment, amount) = Register::amount(i);
                running = running.and_then(|b| {
                    if payment {
                        b.checked_sub(amount).ok()
                    } else {
                        b.checked_add(amount).ok()
                    }
                });
                running
            })
            .collect();
        Register {
            rows,
            start: today.add_days(-back).unwrap_or(today),
            balances,
        }
    }

    fn date(&self, index: usize) -> Option<CalendarDate> {
        self.start
            .add_days(i64::try_from(index / 40).unwrap_or(0))
            .ok()
    }

    fn amount(index: usize) -> (bool, Money) {
        let h = mix(index as u64);
        let cents = i64::try_from(h % 250_000).unwrap_or(0) + 100;
        (h & 1 == 0, Money::from_cents(cents).unwrap_or(Money::ZERO))
    }
}

impl RowModel for Register {
    fn revision(&self) -> u64 {
        0
    }
    fn row_count(&self) -> usize {
        self.rows
    }
    fn row_key(&self, index: usize) -> Option<RowKey> {
        (index < self.rows).then(|| RowKey(index as u64 + 1))
    }
    fn index_of(&self, key: RowKey) -> Option<usize> {
        let i = usize::try_from(key.0.checked_sub(1)?).ok()?;
        (i < self.rows).then_some(i)
    }
    fn row_state(&self, index: usize) -> RowState {
        match mix(index as u64 ^ 0xA5) % 10 {
            0 => RowState::RECONCILED,
            1 | 2 => RowState::CLEARED,
            _ => RowState::empty(),
        }
    }
    fn cell(&self, index: usize, col: ColumnId) -> CellValue<'_> {
        if index >= self.rows {
            return CellValue::Empty;
        }
        let h = mix(index as u64);
        let (payment, amount) = Register::amount(index);
        match col {
            DATE => self.date(index).map_or(CellValue::Empty, CellValue::Date),
            NUMBER if payment => CellValue::Text(Cow::Owned((1000 + index).to_string())),
            KIND => CellValue::Text(Cow::Borrowed(if payment { "CHK" } else { "DEP" })),
            PAYEE => CellValue::Text(Cow::Borrowed(pick(PAYEES, h >> 8))),
            // Line two reads "Account · Memo" (mockup `Register.html`).
            ACCOUNT => {
                let account = pick(ACCOUNTS, h >> 16);
                match pick(MEMOS, h >> 24) {
                    "" => CellValue::Text(Cow::Borrowed(account)),
                    memo => CellValue::Text(Cow::Owned(format!("{account} \u{b7} {memo}"))),
                }
            }
            PAYMENT if payment => CellValue::Money(amount),
            DEPOSIT if !payment => CellValue::Money(amount),
            BALANCE => self
                .balances
                .get(index)
                .copied()
                .flatten()
                .map_or(CellValue::Empty, CellValue::Money),
            MARK => CellValue::Mark(match self.row_state(index) {
                s if s.contains(RowState::RECONCILED) => MarkKind::Reconciled,
                s if s.contains(RowState::CLEARED) => MarkKind::Cleared,
                _ => MarkKind::None,
            }),
            _ => CellValue::Empty,
        }
    }
    fn row_label(&self, index: usize) -> String {
        let h = mix(index as u64);
        let date = self
            .date(index)
            .map(crate::shell::us_date)
            .unwrap_or_default();
        format!("{date}, {}", pick(PAYEES, h >> 8))
    }
}

const SELECT: ColumnId = ColumnId(1);
const VENDOR: ColumnId = ColumnId(2);
const DUE: ColumnId = ColumnId(3);
const AMOUNT_DUE: ColumnId = ColumnId(4);
const TO_PAY: ColumnId = ColumnId(5);

/// The Pay bills example spec (fixed rows).
#[must_use]
pub fn bills_spec() -> GridSpec {
    let money = CellKind::Amount(AmountRules::MONEY);
    GridSpec::fixed_edit(vec![
        Column {
            align: Align::Center,
            editable: true,
            ..column(
                SELECT,
                TextKey("dev.grid_demo.col.select"),
                CellKind::Checkbox,
                Line::First,
                60,
            )
        },
        column(
            VENDOR,
            TextKey("dev.grid_demo.col.vendor"),
            CellKind::Display,
            Line::First,
            220,
        ),
        Column {
            mono: true,
            ..column(
                DUE,
                TextKey("dev.grid_demo.col.due"),
                CellKind::Date,
                Line::First,
                100,
            )
        },
        Column {
            align: Align::Right,
            mono: true,
            ..column(
                AMOUNT_DUE,
                TextKey("dev.grid_demo.col.amount_due"),
                money,
                Line::First,
                110,
            )
        },
        Column {
            align: Align::Right,
            mono: true,
            editable: true,
            ..column(
                TO_PAY,
                TextKey("dev.grid_demo.col.to_pay"),
                money,
                Line::First,
                110,
            )
        },
    ])
}

#[derive(Debug, Clone)]
struct Bill {
    vendor: &'static str,
    due: CalendarDate,
    amount: Money,
    select: bool,
    pay: Option<Money>,
}

/// Six open bills (fixed rows: no entry line, no insert or delete).
#[derive(Debug, Clone)]
pub struct Bills {
    bills: Vec<Bill>,
    revision: u64,
}

impl Bills {
    /// The example bills, due around `today`.
    #[must_use]
    pub fn new(today: CalendarDate) -> Bills {
        let bills = (0..6u32)
            .map(|i| {
                let idx = i as usize;
                Bill {
                    vendor: PAYEES[(idx + 2) % PAYEES.len()],
                    due: today.add_days(i64::from(i) * 4 - 6).unwrap_or(today),
                    amount: Money::from_cents(i64::from(i + 1) * 12_345).unwrap_or(Money::ZERO),
                    select: false,
                    pay: None,
                }
            })
            .collect();
        Bills { bills, revision: 0 }
    }

    /// Bills selected for payment.
    #[must_use]
    pub fn selected(&self) -> usize {
        self.bills.iter().filter(|b| b.select).count()
    }
}

impl RowModel for Bills {
    fn revision(&self) -> u64 {
        self.revision
    }
    fn row_count(&self) -> usize {
        self.bills.len()
    }
    fn row_key(&self, index: usize) -> Option<RowKey> {
        (index < self.bills.len()).then(|| RowKey(index as u64 + 1))
    }
    fn index_of(&self, key: RowKey) -> Option<usize> {
        let i = usize::try_from(key.0.checked_sub(1)?).ok()?;
        (i < self.bills.len()).then_some(i)
    }
    fn row_state(&self, _index: usize) -> RowState {
        RowState::empty()
    }
    fn cell(&self, index: usize, col: ColumnId) -> CellValue<'_> {
        let Some(b) = self.bills.get(index) else {
            return CellValue::Empty;
        };
        match col {
            SELECT => CellValue::Bool(b.select),
            VENDOR => CellValue::Text(Cow::Borrowed(b.vendor)),
            DUE => CellValue::Date(b.due),
            AMOUNT_DUE => CellValue::Money(b.amount),
            TO_PAY => b.pay.map_or(CellValue::Empty, CellValue::Money),
            _ => CellValue::Empty,
        }
    }
    fn row_label(&self, index: usize) -> String {
        self.bills
            .get(index)
            .map(|b| b.vendor.to_string())
            .unwrap_or_default()
    }
}

impl EditableModel for Bills {
    fn cell_editable(&self, index: usize, col: ColumnId) -> bool {
        index < self.bills.len() && matches!(col, SELECT | TO_PAY)
    }
    fn row_entered(&mut self, _key: RowKey) {}
    fn set_cell(
        &mut self,
        pos: CellPos,
        value: CellInput,
        _source: Source,
    ) -> Result<(), CellProblem> {
        let i = self
            .index_of(pos.row)
            .ok_or(CellProblem::new(TextKey("grid.row.gone")))?;
        let b = &mut self.bills[i];
        match (pos.col, value) {
            (SELECT, CellInput::Bool(v)) => {
                b.select = v;
                // Selecting fills the amount due; the owner can change it.
                if v && b.pay.is_none() {
                    b.pay = Some(b.amount);
                }
            }
            (TO_PAY, CellInput::Money(m)) => {
                b.pay = Some(m);
                b.select = !m.is_zero();
            }
            (TO_PAY, CellInput::Clear) => {
                b.pay = None;
                b.select = false;
            }
            _ => return Err(CellProblem::new(TextKey("dev.grid_demo.bad_value")).at(pos.col)),
        }
        self.revision += 1;
        Ok(())
    }
    fn revert_row(&mut self, _key: RowKey) -> bool {
        false
    }
    fn insert_line(&mut self, _before: Option<RowKey>) -> Option<RowKey> {
        None
    }
    fn delete_line(&mut self, _key: RowKey) -> bool {
        false
    }
    fn record_row(&mut self, key: RowKey) -> RecordOutcome {
        let i = self.index_of(key).unwrap_or(0);
        RecordOutcome::Recorded {
            next: self.row_key(i + 1),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_come_from_their_index() {
        let today = CalendarDate::from_ymd(2026, 11, 4).unwrap();
        let r = Register::new(1_000_000, today);
        assert_eq!(r.row_count(), 1_000_000);
        let last = r.row_count() - 1;
        assert_eq!(r.index_of(r.row_key(last).unwrap()), Some(last));
        assert_eq!(r.cell(last, PAYEE), r.cell(last, PAYEE));
        assert!(r.row_key(r.row_count()).is_none());
        assert!(r.row_label(5).contains(", "));
        let small = Register::new(REGISTER_ROWS, today);
        assert!(small.date(REGISTER_ROWS - 1).is_some());
        for i in 0..50 {
            let pay = matches!(small.cell(i, PAYMENT), CellValue::Money(_));
            let dep = matches!(small.cell(i, DEPOSIT), CellValue::Money(_));
            assert!(pay != dep, "row {i} is a payment or a deposit");
        }
        assert!(spec(false).validate().is_ok() && spec(true).validate().is_ok());
    }

    /// Line two is "Account · Memo", or the account alone without a memo;
    /// the mark column explains its marks.
    #[test]
    fn second_line_reads_account_then_memo() {
        let today = CalendarDate::from_ymd(2026, 11, 4).unwrap();
        let r = Register::new(200, today);
        let (mut with_memo, mut without) = (0, 0);
        for i in 0..200 {
            let CellValue::Text(t) = r.cell(i, ACCOUNT) else {
                panic!("row {i} has an account");
            };
            assert!(ACCOUNTS.iter().any(|a| t.starts_with(a)), "{t}");
            if ACCOUNTS.contains(&t.as_ref()) {
                without += 1;
            } else {
                with_memo += 1;
                let memo = t.rsplit(" \u{b7} ").next().unwrap_or_default();
                assert!(MEMOS.contains(&memo), "{t}");
            }
        }
        assert!(with_memo > 0 && without > 0);
        let s = spec(false);
        let col = s.columns.iter().find(|c| c.id == ACCOUNT).unwrap();
        assert_eq!(col.header, TextKey("dev.grid_demo.col.account_memo"));
        let mark = s.columns.iter().find(|c| c.id == MARK).unwrap();
        assert_eq!(mark.help, Some(TextKey("dev.grid_demo.col.mark_help")));
    }

    fn money_at(r: &Register, i: usize, col: ColumnId) -> Money {
        match r.cell(i, col) {
            CellValue::Money(m) => m,
            _ => Money::ZERO,
        }
    }

    /// The Balance column: last, right-aligned, mono, on the first line in
    /// both layouts, never editable; each row's balance is the previous
    /// one minus the payment plus the deposit, exactly.
    #[test]
    fn running_balance_column() {
        for one_line in [false, true] {
            let s = spec(one_line);
            let last = s.columns.last().expect("columns");
            assert_eq!(last.id, BALANCE);
            assert!(!last.editable, "Balance is locked");
            assert_eq!(last.line, Line::First);
            assert_eq!(last.align, Align::Right);
            assert!(last.mono);
            assert!(!s.editable, "the register is read-only");
        }
        let today = CalendarDate::from_ymd(2026, 11, 4).unwrap();
        let r = Register::new(REGISTER_ROWS, today);
        let opening = Money::from_cents(OPENING_CENTS).unwrap();
        let first = opening
            .checked_sub(money_at(&r, 0, PAYMENT))
            .and_then(|b| b.checked_add(money_at(&r, 0, DEPOSIT)))
            .unwrap();
        assert_eq!(money_at(&r, 0, BALANCE), first);
        let mut rows: Vec<usize> = (1..200).collect();
        rows.extend((1..REGISTER_ROWS).step_by(997));
        rows.push(REGISTER_ROWS - 1);
        for n in rows {
            let want = money_at(&r, n - 1, BALANCE)
                .checked_sub(money_at(&r, n, PAYMENT))
                .and_then(|b| b.checked_add(money_at(&r, n, DEPOSIT)))
                .unwrap();
            assert_eq!(money_at(&r, n, BALANCE), want, "row {n}");
        }
        assert!(matches!(r.cell(REGISTER_ROWS, BALANCE), CellValue::Empty));
    }

    #[test]
    fn selecting_a_bill_fills_the_amount() {
        let today = CalendarDate::from_ymd(2026, 11, 4).unwrap();
        let mut b = Bills::new(today);
        let pos = CellPos {
            row: RowKey(1),
            col: SELECT,
        };
        b.set_cell(pos, CellInput::Bool(true), Source::Typed)
            .unwrap();
        assert_eq!(b.selected(), 1);
        assert!(matches!(b.cell(0, TO_PAY), CellValue::Money(_)));
        assert!(!b.cell_editable(0, VENDOR));
        assert!(bills_spec().validate().is_ok());
    }
}
