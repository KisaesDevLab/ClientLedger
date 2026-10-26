//! The "read from the document" state of a bill's fields (P17, plan §3
//! WP3; non-negotiable 4).
//!
//! When the owner clicks **Read with AI**, the screen (WP7) maps the
//! engine's reading into a [`DocReading`] and calls [`apply`]; **Read
//! document** (A03) maps a reading made on this computer the same way. Every field
//! it fills is marked [`ReadField::Read`]: *read, not accepted*. The bill
//! cannot be saved while such a field remains ([`crate::forms::bill`]
//! `BillProblem::ReadNotAccepted`); the owner accepts them all
//! ([`ReadState::accept_all`]) or edits a field, which accepts that field
//! ([`ReadState::edited`]). Nothing here ever marks a field accepted on its
//! own.
//!
//! [`reconcile`] compares the document's own figures exactly (no
//! tolerance): the lines read against the document total, and subtotal +
//! tax against the total. The lines-total rule of the form still governs
//! Save.
//!
//! The UI side (the banner, Read with AI) is `cl-ui` `screens/bill/doc_read.rs`.

use std::collections::BTreeMap;
use std::fmt;

use cl_core::ids::{AccountId, ContactId};
use cl_core::{CalendarDate, Money};

use crate::forms::bill::BillForm;
use crate::forms::lines::{FormLine, FormLines};
use crate::rowkey::{RowKey, RowKeyGen};

/// How sure the reader was about one field (`bill-ocr-v2`). Without
/// per-field confidence every read field is [`Confidence::Unrated`] ("check
/// it").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Confidence {
    /// The reader was sure.
    High,
    /// Fairly sure.
    Medium,
    /// Unsure: the screen marks it more strongly.
    Low,
    /// No per-field confidence was given.
    Unrated,
}

/// The state of one read field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReadField {
    /// Filled from the document and not accepted yet.
    Read {
        /// How sure the reader was.
        level: Confidence,
    },
    /// The owner accepted the read value.
    Accepted,
    /// The owner changed the value (which accepts it).
    Edited,
}

/// A field that a reading can fill.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ReadTarget {
    /// The vendor.
    Vendor,
    /// The bill date.
    BillDate,
    /// The due date.
    DueDate,
    /// The vendor's invoice number (Ref. no.).
    Reference,
    /// Amount due (the document total).
    AmountDue,
    /// One expense line filled from the document (its amount, memo and
    /// account). Accepted by Accept or by changing its amount only.
    Line(RowKey),
    /// A04 draft mode: the payee of a card charge or check ("Purchased
    /// from", "Pay to the order of").
    Payee,
    /// A04 draft mode: the date of a card charge or check.
    Date,
    /// A04 draft mode: the amount of a card charge or check (the
    /// document's printed total).
    Total,
    /// A04 draft mode: the check number printed on a check.
    CheckNumber,
    /// A04 draft mode: the account it was paid from (the card of a card
    /// charge, the bank or cash account of a check or cash expense).
    PaidFrom,
}

/// Which fields came from a reading and whether the owner accepted them.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReadState {
    fields: BTreeMap<ReadTarget, ReadField>,
}

impl ReadState {
    /// Nothing was read.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    /// The state of `target`, when it was read.
    #[must_use]
    pub fn state(&self, target: ReadTarget) -> Option<ReadField> {
        self.fields.get(&target).copied()
    }

    /// Marks `target` as read from the document (not accepted).
    pub fn mark_read(&mut self, target: ReadTarget, level: Confidence) {
        self.fields.insert(target, ReadField::Read { level });
    }

    /// The owner clicked **Accept these values** (Alt+A).
    pub fn accept_all(&mut self) {
        for f in self.fields.values_mut() {
            if matches!(f, ReadField::Read { .. }) {
                *f = ReadField::Accepted;
            }
        }
    }

    /// The owner edited `target`: that field counts as accepted. A field
    /// that was not read is not tracked. For a [`ReadTarget::Line`] the
    /// caller reports only a change of the line's *amount*: editing its
    /// description or account must not accept the AI-read amount
    /// (non-negotiable 4).
    pub fn edited(&mut self, target: ReadTarget) {
        if let Some(f) = self.fields.get_mut(&target) {
            *f = ReadField::Edited;
        }
    }

    /// The line `key` was deleted.
    pub fn forget_line(&mut self, key: RowKey) {
        self.fields.remove(&ReadTarget::Line(key));
    }

    /// "Clear and type it myself": forget the reading's marks (the screen
    /// clears the values).
    pub fn clear(&mut self) {
        self.fields.clear();
    }

    /// The read fields not accepted yet, in field order.
    #[must_use]
    pub fn unaccepted(&self) -> Vec<ReadTarget> {
        self.fields
            .iter()
            .filter(|(_, f)| matches!(f, ReadField::Read { .. }))
            .map(|(t, _)| *t)
            .collect()
    }
}

/// One line read from the document. `Debug` is redacted.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct ReadLine {
    /// Description as printed.
    pub description: String,
    /// The line amount as printed (never computed).
    pub amount: Option<Money>,
    /// A suggested account (the vendor default or the item's account).
    pub account: Option<AccountId>,
    /// How sure the reader was about this line.
    pub confidence: Option<Confidence>,
}

impl fmt::Debug for ReadLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReadLine")
            .field("amount", &self.amount.is_some())
            .field("confidence", &self.confidence)
            .finish_non_exhaustive()
    }
}

/// Per-field confidence (`bill-ocr-v2`); `None` = not given.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FieldConfidence {
    /// Vendor.
    pub vendor: Option<Confidence>,
    /// Bill date.
    pub bill_date: Option<Confidence>,
    /// Due date.
    pub due_date: Option<Confidence>,
    /// Vendor invoice number.
    pub reference: Option<Confidence>,
    /// Total.
    pub total: Option<Confidence>,
    /// Payment terms (A03 local reading; the expense forms use it in A04).
    pub terms: Option<Confidence>,
    /// Amount paid (receipts; A04).
    pub amount_paid: Option<Confidence>,
    /// How it was paid (receipts; A04): also the confidence of
    /// [`DocReading::paid_from`].
    pub payment: Option<Confidence>,
    /// The check number (A04).
    pub check_number: Option<Confidence>,
}

/// What was read from a document, mapped from the engine's reading by the
/// screen. Amounts are the document's own printed figures. `Debug` is
/// redacted.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct DocReading {
    /// The vendor, when the read name resolved to one.
    pub vendor: Option<ContactId>,
    /// Bill date.
    pub bill_date: Option<CalendarDate>,
    /// Due date printed on the document.
    pub due_date: Option<CalendarDate>,
    /// Vendor invoice number.
    pub reference: Option<String>,
    /// Subtotal as printed.
    pub subtotal: Option<Money>,
    /// Tax as printed.
    pub tax: Option<Money>,
    /// Total as printed.
    pub total: Option<Money>,
    /// Lines as printed.
    pub lines: Vec<ReadLine>,
    /// A04: the account it was paid from, when the desktop matched one (the
    /// card whose printed digits matched, Q-A03o). Card and check forms only.
    pub paid_from: Option<AccountId>,
    /// A04: the check number as printed (checks only).
    pub check_number: Option<String>,
    /// Per-field confidence.
    pub confidence: FieldConfidence,
}

impl fmt::Debug for DocReading {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DocReading")
            .field("vendor", &self.vendor.is_some())
            .field("bill_date", &self.bill_date.is_some())
            .field("reference", &self.reference.is_some())
            .field("total", &self.total.is_some())
            .field("lines", &self.lines.len())
            .field("paid_from", &self.paid_from.is_some())
            .field("check_number", &self.check_number.is_some())
            .finish_non_exhaustive()
    }
}

/// How the document's own figures agree, exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadCheck {
    /// The exact sum of the line amounts read; `None` without lines, when a
    /// line has no amount, or when the sum leaves the money range.
    pub lines_sum: Option<Money>,
    /// Subtotal + tax == total; `None` unless all three were read.
    pub subtotal_tax_ok: Option<bool>,
    /// Lines sum == total; `None` unless both are known.
    pub total_ok: Option<bool>,
    /// Total − lines sum (exact); `None` unless both are known.
    pub difference: Option<Money>,
}

/// Exact checks on the document's own figures (plan §4: no tolerance).
#[must_use]
pub fn reconcile(reading: &DocReading) -> ReadCheck {
    let lines_sum = if reading.lines.is_empty() {
        None
    } else {
        reading
            .lines
            .iter()
            .map(|l| l.amount)
            .collect::<Option<Vec<Money>>>()
            .and_then(|v| Money::sum(v).ok())
    };
    let subtotal_tax_ok = match (reading.subtotal, reading.tax, reading.total) {
        (Some(s), Some(t), Some(total)) => Some(s.checked_add(t).ok() == Some(total)),
        _ => None,
    };
    let difference = match (reading.total, lines_sum) {
        (Some(total), Some(sum)) => total.checked_sub(sum).ok(),
        _ => None,
    };
    ReadCheck {
        lines_sum,
        subtotal_tax_ok,
        total_ok: difference.map(Money::is_zero),
        difference,
    }
}

/// Fills the empty fields of `form` from `reading` and marks each one
/// *read, not accepted*. Fields the owner already filled are left alone;
/// lines are filled only when the form has no used line (expenses or
/// items). Line amounts are the document's printed amounts; an amount that
/// was not printed stays blank (never computed). Returns the fields filled.
pub fn apply(reading: &DocReading, form: &mut BillForm, keys: &mut RowKeyGen) -> Vec<ReadTarget> {
    let mut filled = Vec::new();
    let level = |c: Option<Confidence>| c.unwrap_or(Confidence::Unrated);
    let conf = reading.confidence;
    if form.vendor.is_none()
        && let Some(v) = reading.vendor
    {
        form.vendor = Some(v);
        form.read.mark_read(ReadTarget::Vendor, level(conf.vendor));
        filled.push(ReadTarget::Vendor);
    }
    if form.date.is_none()
        && let Some(d) = reading.bill_date
    {
        form.date = Some(d);
        form.read
            .mark_read(ReadTarget::BillDate, level(conf.bill_date));
        filled.push(ReadTarget::BillDate);
    }
    if !form.due_overridden
        && let Some(d) = reading.due_date
    {
        // A printed due date is the document's, so it replaces the terms
        // default like an owner override, once accepted.
        form.due_date = Some(d);
        form.due_overridden = true;
        form.read
            .mark_read(ReadTarget::DueDate, level(conf.due_date));
        filled.push(ReadTarget::DueDate);
    }
    if form.reference.trim().is_empty()
        && let Some(r) = reading.reference.as_deref().map(str::trim)
        && !r.is_empty()
    {
        form.reference = r.to_string();
        form.read
            .mark_read(ReadTarget::Reference, level(conf.reference));
        filled.push(ReadTarget::Reference);
    }
    if form.amount_due.is_none()
        && let Some(t) = reading.total
    {
        form.amount_due = Some(t);
        form.amount_from_lines = false;
        form.read
            .mark_read(ReadTarget::AmountDue, level(conf.total));
        filled.push(ReadTarget::AmountDue);
    }
    let no_lines = form.expenses.used().next().is_none() && form.items.used().next().is_none();
    if no_lines && let Some(v) = read_lines(reading, keys, &mut form.read, &mut filled) {
        form.expenses = v;
    }
    filled
}

/// The reading's lines as form lines, each marked *read, not accepted*
/// and pushed to `filled`; `None` when the reading has no lines. Line
/// amounts are the printed ones (blank when not printed, never computed).
/// Shared by the bill, card ([`crate::forms::card::apply`]) and check
/// ([`crate::forms::check::apply`]) forms.
pub(crate) fn read_lines(
    reading: &DocReading,
    keys: &mut RowKeyGen,
    read: &mut ReadState,
    filled: &mut Vec<ReadTarget>,
) -> Option<FormLines> {
    if reading.lines.is_empty() {
        return None;
    }
    let mut lines = Vec::with_capacity(reading.lines.len() + 1);
    for l in &reading.lines {
        let key = keys.next_key();
        lines.push(FormLine {
            account: l.account,
            amount: l.amount,
            memo: l.description.trim().to_string(),
            ..FormLine::blank(key)
        });
        read.mark_read(
            ReadTarget::Line(key),
            l.confidence.unwrap_or(Confidence::Unrated),
        );
        filled.push(ReadTarget::Line(key));
    }
    let mut v = FormLines { lines };
    v.ensure_entry(keys);
    Some(v)
}

/// True while a read field is not accepted. A read line counts only while
/// it is still among `lines` (a deleted line no longer blocks Save). The
/// card and check forms' `ReadNotAccepted` gate (A04 D2).
#[must_use]
pub fn blocks_save(read: &ReadState, lines: &FormLines) -> bool {
    read.unaccepted().into_iter().any(|t| match t {
        ReadTarget::Line(k) => lines.lines.iter().any(|l| l.key == k),
        _ => true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn money(s: &str) -> Money {
        Money::parse_wire(s).expect("money")
    }

    fn day(d: u32) -> CalendarDate {
        CalendarDate::from_ymd(2026, 11, d).expect("date")
    }

    fn contact() -> ContactId {
        ContactId::parse("00000000-0000-0000-0000-000000000005").expect("id")
    }

    fn reading() -> DocReading {
        DocReading {
            vendor: Some(contact()),
            bill_date: Some(day(3)),
            due_date: Some(day(18)),
            reference: Some(" INV-55871 ".into()),
            subtotal: Some(money("1480")),
            tax: Some(Money::ZERO),
            total: Some(money("1480")),
            lines: vec![
                ReadLine {
                    description: "Excavator, 2 days".into(),
                    amount: Some(money("1330")),
                    account: None,
                    confidence: Some(Confidence::High),
                },
                ReadLine {
                    description: "Delivery".into(),
                    amount: Some(money("150")),
                    account: None,
                    confidence: Some(Confidence::Low),
                },
            ],
            paid_from: None,
            check_number: None,
            confidence: FieldConfidence {
                total: Some(Confidence::High),
                ..FieldConfidence::default()
            },
        }
    }

    #[test]
    fn reconcile_is_exact_on_the_documents_figures() {
        let r = reading();
        let c = reconcile(&r);
        assert_eq!(c.lines_sum, Some(money("1480")));
        assert_eq!(c.subtotal_tax_ok, Some(true));
        assert_eq!(c.total_ok, Some(true));
        assert_eq!(c.difference, Some(Money::ZERO));
        let mut off = r.clone();
        off.total = Some(money("1480.01"));
        let c = reconcile(&off);
        assert_eq!(c.total_ok, Some(false), "one cent off is a mismatch");
        assert_eq!(c.difference, Some(money("0.01")));
        assert_eq!(c.subtotal_tax_ok, Some(false));
        let mut missing = r.clone();
        missing.lines[1].amount = None;
        missing.tax = None;
        let c = reconcile(&missing);
        assert_eq!(c.lines_sum, None);
        assert_eq!(c.total_ok, None);
        assert_eq!(c.subtotal_tax_ok, None);
        assert_eq!(reconcile(&DocReading::default()).lines_sum, None);
    }

    #[test]
    fn apply_fills_empty_fields_as_read_never_accepted() {
        let mut keys = RowKeyGen::new();
        let mut form = BillForm::new(None, &mut keys);
        let filled = apply(&reading(), &mut form, &mut keys);
        assert_eq!(filled.len(), 7);
        assert_eq!(form.reference, "INV-55871");
        assert_eq!(form.amount_due, Some(money("1480")));
        assert!(form.due_overridden);
        assert_eq!(form.expenses.used().count(), 2);
        assert_eq!(form.expenses.lines.len(), 3, "plus the entry line");
        let un = form.read.unaccepted();
        assert_eq!(un.len(), 7, "nothing is accepted by applying");
        assert_eq!(
            form.read.state(ReadTarget::AmountDue),
            Some(ReadField::Read {
                level: Confidence::High
            })
        );
        assert_eq!(
            form.read.state(ReadTarget::Vendor),
            Some(ReadField::Read {
                level: Confidence::Unrated
            })
        );
        let second = form.expenses.lines[1].key;
        assert_eq!(
            form.read.state(ReadTarget::Line(second)),
            Some(ReadField::Read {
                level: Confidence::Low
            })
        );
        form.read.edited(ReadTarget::AmountDue);
        assert_eq!(
            form.read.state(ReadTarget::AmountDue),
            Some(ReadField::Edited)
        );
        form.read.edited(ReadTarget::Line(RowKey(999)));
        assert_eq!(form.read.state(ReadTarget::Line(RowKey(999))), None);
        form.read.forget_line(second);
        assert_eq!(form.read.unaccepted().len(), 5);
        form.read.accept_all();
        assert_eq!(form.read.unaccepted().len(), 0);
        assert_eq!(
            form.read.state(ReadTarget::AmountDue),
            Some(ReadField::Edited)
        );
        assert!(!form.read.is_empty());
        form.read.clear();
        assert!(form.read.is_empty());
    }

    #[test]
    fn apply_leaves_what_the_owner_typed() {
        let mut keys = RowKeyGen::new();
        let mut form = BillForm::new(Some(day(1)), &mut keys);
        form.reference = "MINE".into();
        form.amount_due = Some(money("5"));
        form.due_date = Some(day(9));
        form.due_overridden = true;
        form.expenses.lines[0].memo = "typed".into();
        form.expenses.ensure_entry(&mut keys);
        let filled = apply(&reading(), &mut form, &mut keys);
        assert_eq!(filled, vec![ReadTarget::Vendor]);
        assert_eq!(form.date, Some(day(1)));
        assert_eq!(form.due_date, Some(day(9)));
        assert_eq!(form.reference, "MINE");
        assert_eq!(form.amount_due, Some(money("5")));
        assert_eq!(form.expenses.used().count(), 1);
    }

    #[test]
    fn debug_hides_document_data() {
        let s = format!("{:?}", reading());
        assert!(
            !s.contains("1480") && !s.contains("INV") && !s.contains("Excavator"),
            "{s}"
        );
        let s = format!("{:?}", reading().lines[0]);
        assert!(!s.contains("1330") && !s.contains("Excavator"), "{s}");
    }
}
