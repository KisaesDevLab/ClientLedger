//! The Pay Bills grid (fixed rows, mockup `PayBills.html`): Select, Due,
//! Vendor (with the bill's note), Ref. no., Disc. date, Amt. due, Discount,
//! Credits and Amt. to pay. Space ticks the bill (and applies its discount
//! and the vendor's credits, through the form); Amt. to pay is the only
//! amount the owner types. Every figure is the form's, which keeps the
//! engine's balance, discount and credits apart (no percent worked out
//! here).

use std::borrow::Cow;

use cl_core::ids::TransactionId;
use cl_grid::{
    Align, CellInput, CellKind, CellPos, CellProblem, CellValue, Column, ColumnId, EditableModel,
    GridSpec, Line, RecordOutcome, RowKey, RowModel, RowState, Source, TextKey, WidthSpec,
};
use cl_model::amount_entry::AmountRules;
use cl_model::forms::bill_payment::PayBillsForm;

pub(crate) const SELECT: ColumnId = ColumnId(1);
pub(crate) const DUE: ColumnId = ColumnId(2);
pub(crate) const VENDOR: ColumnId = ColumnId(3);
pub(crate) const REF: ColumnId = ColumnId(4);
pub(crate) const DISC_DATE: ColumnId = ColumnId(5);
pub(crate) const AMOUNT_DUE: ColumnId = ColumnId(6);
pub(crate) const DISCOUNT: ColumnId = ColumnId(7);
pub(crate) const CREDITS: ColumnId = ColumnId(8);
pub(crate) const TO_PAY: ColumnId = ColumnId(9);

/// A column `w` wide; `flex` > 0 gives width first when the window is
/// narrow, down to `min` (Vendor, then Ref. no. and Disc. date), so the
/// amounts and Amt. to pay keep theirs (UI review 1). Narrower still (200%
/// zoom), the columns scroll sideways to follow the cursor.
fn column(id: ColumnId, header: &'static str, kind: CellKind, w: (u16, u16, u8)) -> Column {
    let (min, preferred, flex) = w;
    Column {
        id,
        header: TextKey(header),
        placeholder: None,
        empty: None,
        help: None,
        kind,
        line: Line::First,
        align: Align::Left,
        width: WidthSpec {
            min,
            preferred,
            flex,
        },
        editable: false,
        mono: false,
    }
}

/// The grid's columns.
#[must_use]
pub(crate) fn spec() -> GridSpec {
    let money = CellKind::Amount(AmountRules::MONEY);
    let mono = |c: Column| Column { mono: true, ..c };
    let right = |c: Column| Column {
        align: Align::Right,
        mono: true,
        ..c
    };
    GridSpec::fixed_edit(vec![
        Column {
            align: Align::Center,
            editable: true,
            help: Some(TextKey("paybills.col.select_help")),
            ..column(
                SELECT,
                "paybills.col.select",
                CellKind::Checkbox,
                (40, 40, 0),
            )
        },
        mono(column(
            DUE,
            "paybills.col.due",
            CellKind::Display,
            (56, 56, 0),
        )),
        column(
            VENDOR,
            "paybills.col.vendor",
            CellKind::Display,
            (140, 240, 3),
        ),
        // Not mono: a mono column grows to its widest value and stops
        // giving width (Q-P10o), and Ref. no. gives width at 960 x 640.
        column(REF, "paybills.col.ref", CellKind::Display, (60, 100, 1)),
        mono(column(
            DISC_DATE,
            "paybills.col.disc_date",
            CellKind::Display,
            (56, 90, 1),
        )),
        right(column(
            AMOUNT_DUE,
            "paybills.col.amount_due",
            money,
            (92, 92, 0),
        )),
        Column {
            help: Some(TextKey("paybills.col.discount_help")),
            ..right(column(
                DISCOUNT,
                "paybills.col.discount",
                money,
                (72, 72, 0),
            ))
        },
        right(column(CREDITS, "paybills.col.credits", money, (72, 72, 0))),
        Column {
            editable: true,
            ..right(column(TO_PAY, "paybills.col.to_pay", money, (100, 100, 0)))
        },
    ])
}

/// MM/DD, as the mockup shows due and discount dates.
pub(crate) fn month_day(d: cl_core::CalendarDate) -> String {
    format!("{:02}/{:02}", d.month(), d.day())
}

/// The grid's model over the form: the listed bills in list order.
pub(crate) struct Rows<'a> {
    pub form: &'a mut PayBillsForm,
    /// Bumped by every change the grid makes (the screen keeps it).
    pub revision: &'a mut u64,
    ids: Vec<TransactionId>,
}

impl<'a> Rows<'a> {
    pub(crate) fn new(form: &'a mut PayBillsForm, revision: &'a mut u64) -> Rows<'a> {
        let ids = form.visible().iter().map(|r| r.bill).collect();
        Rows {
            form,
            revision,
            ids,
        }
    }

    /// A bill's key: its place in the engine's whole list (stable while the
    /// list is not read again).
    fn key_of(&self, bill: TransactionId) -> Option<RowKey> {
        self.form
            .all_rows()
            .iter()
            .position(|r| r.bill == bill)
            .map(|i| RowKey(i as u64 + 1))
    }

    /// The note beside the vendor, as the mockup shows it under the name:
    /// the bill's memo, else its discount terms, else the vendor's open
    /// credit (the engine's open amounts of its credits added up for show
    /// only; the engine has no vendor credit total, Q-P18i).
    fn note(&self, r: &cl_model::forms::bill_payment::PayableRow) -> Option<String> {
        if let Some(n) = r.note.as_ref().filter(|n| !n.is_empty()) {
            return Some(n.clone());
        }
        if r.discount_date.is_some() {
            return r.terms.clone();
        }
        let open = cl_core::Money::sum(self.form.credits_for(r.vendor).iter().map(|c| c.open))
            .ok()
            .filter(|m| m.is_positive())?;
        Some(crate::i18n::t_with(
            "paybills.vendor.credit_available",
            &[("amount", &open.format(cl_core::MoneyStyle::default()))],
        ))
    }

    fn bill_at(&self, index: usize) -> Option<TransactionId> {
        self.ids.get(index).copied()
    }
}

/// The bill a grid row key stands for.
pub(crate) fn bill_of(form: &PayBillsForm, key: RowKey) -> Option<TransactionId> {
    let i = usize::try_from(key.0.checked_sub(1)?).ok()?;
    form.all_rows().get(i).map(|r| r.bill)
}

impl RowModel for Rows<'_> {
    fn revision(&self) -> u64 {
        *self.revision
    }
    fn row_count(&self) -> usize {
        self.ids.len()
    }
    fn row_key(&self, index: usize) -> Option<RowKey> {
        self.key_of(self.bill_at(index)?)
    }
    fn index_of(&self, key: RowKey) -> Option<usize> {
        let bill = bill_of(self.form, key)?;
        self.ids.iter().position(|b| *b == bill)
    }
    fn row_state(&self, _index: usize) -> RowState {
        RowState::empty()
    }
    fn cell(&self, index: usize, col: ColumnId) -> CellValue<'_> {
        let Some(bill) = self.bill_at(index) else {
            return CellValue::Empty;
        };
        let Some(r) = self.form.all_rows().iter().find(|r| r.bill == bill) else {
            return CellValue::Empty;
        };
        match col {
            SELECT => CellValue::Bool(self.form.is_selected(bill)),
            DUE => CellValue::Text(Cow::Owned(r.due.map(month_day).unwrap_or_default())),
            VENDOR => CellValue::Text(Cow::Owned(match self.note(r) {
                Some(n) => format!("{} \u{b7} {n}", r.vendor_name),
                None => r.vendor_name.clone(),
            })),
            REF => CellValue::Text(Cow::Owned(r.reference.clone().unwrap_or_default())),
            DISC_DATE => CellValue::Text(Cow::Owned(
                r.discount_date
                    .map_or_else(|| "\u{2014}".to_string(), month_day),
            )),
            AMOUNT_DUE => CellValue::Money(r.balance_due),
            DISCOUNT => CellValue::Money(self.form.discount(bill)),
            CREDITS => CellValue::Money(self.form.credit_total(bill)),
            TO_PAY => CellValue::Money(self.form.cash(bill)),
            _ => CellValue::Empty,
        }
    }
    fn row_label(&self, index: usize) -> String {
        let Some(bill) = self.bill_at(index) else {
            return String::new();
        };
        self.form
            .all_rows()
            .iter()
            .find(|r| r.bill == bill)
            .map(|r| match &r.reference {
                Some(n) => format!("{} {n}", r.vendor_name),
                None => r.vendor_name.clone(),
            })
            .unwrap_or_default()
    }
}

impl EditableModel for Rows<'_> {
    fn cell_editable(&self, index: usize, col: ColumnId) -> bool {
        index < self.ids.len() && matches!(col, SELECT | TO_PAY)
    }
    fn row_entered(&mut self, _key: RowKey) {}
    fn set_cell(
        &mut self,
        pos: CellPos,
        value: CellInput,
        _source: Source,
    ) -> Result<(), CellProblem> {
        let bill = bill_of(self.form, pos.row)
            .ok_or_else(|| CellProblem::new(TextKey("grid.row.gone")))?;
        match (pos.col, value) {
            (SELECT, CellInput::Bool(v)) => self.form.set_selected(bill, v),
            (TO_PAY, CellInput::Money(m)) if m.is_negative() => {
                return Err(CellProblem::new(TextKey("paybills.problem.negative")).at(pos.col));
            }
            (TO_PAY, CellInput::Money(m)) => self.form.set_cash(bill, m),
            (TO_PAY, CellInput::Clear) => self.form.clear_cash(bill),
            _ => return Err(CellProblem::new(TextKey("paybills.problem.bad_value")).at(pos.col)),
        }
        *self.revision += 1;
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
        let next = self.index_of(key).and_then(|i| self.row_key(i + 1));
        RecordOutcome::Recorded { next }
    }
}
