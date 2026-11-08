//! Bank Feed receipt match (A05, Q-282, Q-A05a): which document drafts a
//! bank line could be, so the owner can file the receipt on the entry the
//! line posts instead of entering it twice. Pure rules: no I/O, no clock,
//! no floats.
//!
//! A draft is **offered** for a bank line when all of these hold:
//!
//! * it is live and not a vendor statement (the caller lists `new` and
//!   `ready` drafts only; [`DraftFacts::statement`] drops the rest);
//! * its total **equals** the bank amount exactly (`Money == Money`, so a
//!   4-decimal `12.5000` equals the bank's `12.50` and `12.5001` never
//!   does; nothing is rounded, summed or compared with a tolerance);
//! * the money goes the same way: every draft kind is a purchase (a bill,
//!   card charge, check or cash expense, or not known yet), so a positive
//!   total is money **out**; a negative total (a refund read as a credit)
//!   is money **in**. Q-A05c: a receipt whose total differs (tip, cash
//!   back) is never offered;
//! * its date is within [`DATE_WINDOW_DAYS`] days of the bank line's,
//!   either way, both edges included (the A04 attach window).
//!
//! Matches are ranked: the draft's vendor is the line's payee first, then
//! the nearer date, then the draft id (a stable order); at most
//! [`MAX_OFFERS`] are returned. Nothing here applies a match: the owner
//! always picks (Q-A05a).
//!
//! **Privacy:** `Debug` prints no amount, date or name.

use std::cmp::Reverse;
use std::fmt;

use cl_core::ids::ContactId;
use cl_core::{CalendarDate, Money};

pub use crate::register_match::DATE_WINDOW_DAYS;

/// Drafts offered for one bank line at most (Q-A05a).
pub const MAX_OFFERS: usize = 3;

/// A bank line as the rules take it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct BankLine {
    /// The amount, either sign (only its size is compared).
    pub amount: Money,
    /// Money left the account (the feed's own direction; the caller asks
    /// the row, never the sign: the engine and the demo differ).
    pub money_out: bool,
    /// The bank's date.
    pub date: CalendarDate,
    /// The payee staged or suggested on the line.
    pub contact: Option<ContactId>,
}

impl fmt::Debug for BankLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BankLine")
            .field("money_out", &self.money_out)
            .field("contact", &self.contact.is_some())
            .finish_non_exhaustive()
    }
}

/// What a draft's summary says (`view=summary`).
#[derive(Clone, PartialEq, Eq)]
pub struct DraftFacts<Id> {
    /// The draft.
    pub id: Id,
    /// The total as read (`docTotal`, up to 4 decimals).
    pub total: Option<Money>,
    /// The date as read (`docDate`).
    pub date: Option<CalendarDate>,
    /// The vendor matched when it was read.
    pub contact: Option<ContactId>,
    /// A vendor statement: never offered.
    pub statement: bool,
}

impl<Id: fmt::Debug> fmt::Debug for DraftFacts<Id> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DraftFacts")
            .field("id", &self.id)
            .field("total", &self.total.is_some())
            .field("date", &self.date.is_some())
            .field("contact", &self.contact.is_some())
            .field("statement", &self.statement)
            .finish()
    }
}

/// Why a draft is not offered (tests).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Excluded {
    /// A vendor statement.
    Statement,
    /// No total or no date read.
    Unread,
    /// The amounts differ.
    Amount,
    /// Money the other way.
    Direction,
    /// Dated more than [`DATE_WINDOW_DAYS`] away.
    Date,
}

/// A draft offered for the line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Offer<Id> {
    /// The draft.
    pub id: Id,
    /// The draft's vendor is the line's payee.
    pub same_contact: bool,
    /// Days between the bank date and the draft's.
    pub days_apart: u32,
}

/// Whether `d` may be offered for `line`, or why not.
///
/// # Errors
/// The first rule the draft fails.
pub fn check<Id>(line: &BankLine, d: &DraftFacts<Id>) -> Result<u32, Excluded> {
    if d.statement {
        return Err(Excluded::Statement);
    }
    let (Some(total), Some(date)) = (d.total, d.date) else {
        return Err(Excluded::Unread);
    };
    if total.abs() != line.amount.abs() {
        return Err(Excluded::Amount);
    }
    // A purchase: a positive total is money out; a credit, money in.
    let draft_out = !total.is_negative();
    if draft_out != line.money_out {
        return Err(Excluded::Direction);
    }
    let days = line.date.days_until(date).abs();
    if days > DATE_WINDOW_DAYS {
        return Err(Excluded::Date);
    }
    Ok(u32::try_from(days).unwrap_or(u32::MAX))
}

/// The drafts offered for `line` among `drafts`, best first, at most
/// [`MAX_OFFERS`] (each id once).
#[must_use]
pub fn offers<Id: Copy + Ord>(line: &BankLine, drafts: &[DraftFacts<Id>]) -> Vec<Offer<Id>> {
    let mut out: Vec<Offer<Id>> = Vec::new();
    for d in drafts {
        let Ok(days_apart) = check(line, d) else {
            continue;
        };
        if out.iter().any(|o| o.id == d.id) {
            continue;
        }
        out.push(Offer {
            id: d.id,
            same_contact: line.contact.is_some() && line.contact == d.contact,
            days_apart,
        });
    }
    out.sort_by_key(|o| (Reverse(o.same_contact), o.days_apart, o.id));
    out.truncate(MAX_OFFERS);
    out
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

    fn contact(n: u8) -> ContactId {
        format!("c0000000-0000-4000-8000-0000000000{n:02}")
            .parse()
            .expect("contact")
    }

    fn line(amount: &str) -> BankLine {
        BankLine {
            amount: money(amount),
            money_out: true,
            date: day(15),
            contact: None,
        }
    }

    fn draft(id: u32, total: &str, date: CalendarDate) -> DraftFacts<u32> {
        DraftFacts {
            id,
            total: Some(money(total)),
            date: Some(date),
            contact: None,
            statement: false,
        }
    }

    #[test]
    fn four_decimal_totals_equal_two_decimal_bank_amounts_exactly() {
        let l = line("12.50");
        assert_eq!(check(&l, &draft(1, "12.5000", day(15))), Ok(0));
        assert_eq!(check(&l, &draft(1, "12.5", day(15))), Ok(0));
        assert_eq!(
            check(&l, &draft(1, "12.5001", day(15))),
            Err(Excluded::Amount)
        );
        assert_eq!(
            check(&l, &draft(1, "12.49", day(15))),
            Err(Excluded::Amount)
        );
        // The engine's feed sign doesn't matter, only its direction.
        let neg = BankLine {
            amount: money("-12.50"),
            ..l
        };
        assert_eq!(check(&neg, &draft(1, "12.50", day(15))), Ok(0));
    }

    #[test]
    fn a_receipt_never_matches_money_coming_in() {
        let incoming = BankLine {
            money_out: false,
            ..line("40.00")
        };
        assert_eq!(
            check(&incoming, &draft(1, "40.00", day(15))),
            Err(Excluded::Direction)
        );
        // A credit read as a negative total is money in.
        assert_eq!(check(&incoming, &draft(1, "-40.00", day(15))), Ok(0));
        assert_eq!(
            check(&line("40.00"), &draft(1, "-40.00", day(15))),
            Err(Excluded::Direction)
        );
    }

    #[test]
    fn the_window_is_seven_days_both_edges_included() {
        let l = line("9.99");
        assert_eq!(check(&l, &draft(1, "9.99", day(8))), Ok(7));
        assert_eq!(check(&l, &draft(1, "9.99", day(22))), Ok(7));
        assert_eq!(check(&l, &draft(1, "9.99", day(7))), Err(Excluded::Date));
        assert_eq!(check(&l, &draft(1, "9.99", day(23))), Err(Excluded::Date));
    }

    #[test]
    fn statements_and_unread_drafts_are_never_offered() {
        let l = line("9.99");
        let mut s = draft(1, "9.99", day(15));
        s.statement = true;
        assert_eq!(check(&l, &s), Err(Excluded::Statement));
        let mut no_total = draft(2, "9.99", day(15));
        no_total.total = None;
        assert_eq!(check(&l, &no_total), Err(Excluded::Unread));
        let mut no_date = draft(3, "9.99", day(15));
        no_date.date = None;
        assert_eq!(check(&l, &no_date), Err(Excluded::Unread));
    }

    #[test]
    fn same_payee_first_then_nearer_date_then_id_best_three() {
        let mut l = line("25.00");
        l.contact = Some(contact(1));
        let mut payee = draft(9, "25.00", day(20));
        payee.contact = Some(contact(1));
        let mut other = draft(2, "25.00", day(15));
        other.contact = Some(contact(2));
        let drafts = vec![
            draft(5, "25.00", day(14)),
            other,
            payee,
            draft(4, "25.00", day(16)),
            draft(7, "25.00", day(18)),
            draft(8, "26.00", day(15)),
            draft(5, "25.00", day(14)),
        ];
        let got: Vec<u32> = offers(&l, &drafts).iter().map(|o| o.id).collect();
        assert_eq!(got, vec![9, 2, 4]);
        assert!(offers(&l, &drafts)[0].same_contact);
        // Without a payee on the line: date, then id.
        l.contact = None;
        let got: Vec<u32> = offers(&l, &drafts).iter().map(|o| o.id).collect();
        assert_eq!(got, vec![2, 4, 5]);
    }

    #[test]
    fn debug_prints_no_amount_date_or_name() {
        let shown = format!("{:?} {:?}", line("1234.56"), draft(1, "1234.56", day(15)));
        assert!(!shown.contains("1234"));
        assert!(!shown.contains("2026"));
    }
}
