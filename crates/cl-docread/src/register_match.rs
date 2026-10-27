//! Register match (A04 D3, Q-282, Q-A04d, review M5): which transactions
//! already in the books a document could be, so the owner can attach it
//! instead of entering it twice. Pure rules: no I/O, no clock, no floats.
//! A05 reuses them for bank lines.
//!
//! A row is a **candidate** when all of these hold:
//!
//! * its amount **equals** the document's exactly (`Money == Money`, so
//!   `10.1` equals `10.10` and `10.10` never equals `10.01`; nothing is
//!   rounded, summed or compared with a tolerance);
//! * its date is within [`DATE_WINDOW_DAYS`] days of the document's, either
//!   way;
//! * it is posted (not void, deleted or a draft);
//! * its type is one the document could have been entered as (an
//!   **allowlist**, security review M2: an attach can't be undone, Q-A04e):
//!   an expense, a check or a card charge for a bill or a receipt
//!   ([`DocSide::Purchase`]); a card credit only for a credit or a refund
//!   ([`DocSide::Credit`]). Deposits, journal entries, transfers, invoices,
//!   bills, bill payments and any type not known here are never offered
//!   (bills are matched by reference through the engine's duplicate check,
//!   Q-A04d; journal entries are left out entirely rather than offered as
//!   Possible: a receipt is never entered as one);
//! * the **payee gate** passes: when both the document and the row name a
//!   known contact and they differ, the row is never offered (M5). A row
//!   with no contact (a bank-feed expense) stays a candidate.
//!
//! Each candidate is labelled [`MatchLabel::Strong`] (same contact or same
//! account, within [`STRONG_DAYS`]) or [`MatchLabel::Possible`], for
//! display only: the owner always picks and confirms; nothing here
//! attaches anything. They are ranked same account, then same contact, then
//! a folded payee-name match, then date distance, and at most
//! [`MAX_CANDIDATES`] are returned.
//!
//! **Privacy:** `Debug` prints no amount, date or name.

use crate::vendor_match::normalize_loose;
use cl_core::ids::{ContactId, TransactionId};
use cl_core::{CalendarDate, Money};
use std::cmp::Reverse;
use std::collections::BTreeSet;
use std::fmt;

/// Days either side of the document's date a candidate may be dated.
pub const DATE_WINDOW_DAYS: i64 = 7;
/// Days either side within which a same-contact or same-account row is
/// labelled [`MatchLabel::Strong`].
pub const STRONG_DAYS: i64 = 3;
/// Candidates shown at most.
pub const MAX_CANDIDATES: usize = 5;

/// What the document says (the form's values at Accept, or the reading's).
#[derive(Clone, PartialEq, Eq)]
pub struct Probe {
    /// The document's amount (the form's total).
    pub amount: Money,
    /// The document's date.
    pub date: CalendarDate,
    /// The payee as printed or typed, if any (a folded-name tie-break).
    pub payee: Option<String>,
    /// The contact the owner chose, if any (the payee gate).
    pub contact: Option<ContactId>,
    /// Whether the document is a purchase or a credit/refund (which row
    /// types it may be attached to).
    pub side: DocSide,
}

/// Which way the document's money went (security review M2).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum DocSide {
    /// A bill or a receipt: money paid out.
    #[default]
    Purchase,
    /// A credit memo or a refund: money coming back.
    Credit,
}

impl fmt::Debug for Probe {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Probe")
            .field("payee", &self.payee.is_some())
            .field("contact", &self.contact.is_some())
            .field("side", &self.side)
            .finish_non_exhaustive()
    }
}

/// A row's transaction type, from the engine's `txnType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RowKind {
    Expense,
    Check,
    CardCharge,
    CardCredit,
    /// Never a candidate (security review M2).
    Deposit,
    /// Never a candidate (security review M2).
    JournalEntry,
    /// Matched by reference through the engine's duplicate check, never
    /// here (Q-A04d).
    Bill,
    /// Never a candidate (Q-A04d, review N6).
    BillPayment,
    /// Any other type (invoices, transfers, ...): never a candidate.
    Other,
}

impl RowKind {
    /// The kind of an engine `txnType` string.
    #[must_use]
    pub fn from_txn_type(txn_type: &str) -> RowKind {
        match txn_type {
            "expense" => RowKind::Expense,
            "check" => RowKind::Check,
            "credit_card_charge" => RowKind::CardCharge,
            "credit_card_credit" => RowKind::CardCredit,
            "deposit" => RowKind::Deposit,
            "journal_entry" => RowKind::JournalEntry,
            "bill" => RowKind::Bill,
            "bill_payment" => RowKind::BillPayment,
            _ => RowKind::Other,
        }
    }

    /// Whether rows of this kind may be offered for a document of `side`
    /// (an allowlist: anything not named here is never offered).
    #[must_use]
    pub const fn matchable(self, side: DocSide) -> bool {
        match side {
            DocSide::Purchase => {
                matches!(
                    self,
                    RowKind::Expense | RowKind::Check | RowKind::CardCharge
                )
            }
            DocSide::Credit => matches!(self, RowKind::CardCredit),
        }
    }
}

/// A row's status, from the engine's `status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RowStatus {
    Posted,
    Void,
    Deleted,
    Draft,
    Other,
}

impl RowStatus {
    /// The status of an engine `status` string.
    #[must_use]
    pub fn from_wire(status: &str) -> RowStatus {
        match status {
            "posted" => RowStatus::Posted,
            "void" => RowStatus::Void,
            "deleted" => RowStatus::Deleted,
            "draft" => RowStatus::Draft,
            _ => RowStatus::Other,
        }
    }
}

/// One transaction from the register search.
#[derive(Clone, PartialEq, Eq)]
pub struct RegisterRow {
    pub id: TransactionId,
    pub date: CalendarDate,
    /// The engine's `total`, parsed exactly (`Money::parse_wire`).
    pub amount: Money,
    /// The payee's display name, if any.
    pub payee: Option<String>,
    pub contact: Option<ContactId>,
    pub kind: RowKind,
    pub status: RowStatus,
    /// Found again by the second query filtered to the document's account.
    pub same_account: bool,
}

impl fmt::Debug for RegisterRow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RegisterRow")
            .field("id", &self.id)
            .field("kind", &self.kind)
            .field("status", &self.status)
            .field("contact", &self.contact.is_some())
            .field("same_account", &self.same_account)
            .finish_non_exhaustive()
    }
}

/// How close a candidate is (display only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MatchLabel {
    /// Same contact or same account, dated within [`STRONG_DAYS`].
    Strong,
    /// Any other candidate.
    Possible,
}

/// A row the owner may attach the document to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Candidate {
    pub id: TransactionId,
    /// Index of the row in the slice given to [`candidates`].
    pub row: usize,
    pub label: MatchLabel,
    /// Days between the document's date and the row's.
    pub days_apart: u32,
    pub same_contact: bool,
    pub same_account: bool,
    /// The folded payee names are equal.
    pub same_name: bool,
}

/// Why a row is not a candidate (tests and the "why not" note).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Excluded {
    /// The amounts differ.
    Amount,
    /// Dated more than [`DATE_WINDOW_DAYS`] away.
    Date,
    /// Not posted.
    Status,
    /// A type the document can't be attached to (see [`RowKind::matchable`]).
    Kind,
    /// Both name a known contact and they differ (M5).
    OtherContact,
}

/// Whether `row` may be offered for `probe`, or why not.
///
/// # Errors
/// The first rule the row fails.
pub fn check(probe: &Probe, row: &RegisterRow) -> Result<(), Excluded> {
    if row.amount != probe.amount {
        return Err(Excluded::Amount);
    }
    if probe.date.days_until(row.date).abs() > DATE_WINDOW_DAYS {
        return Err(Excluded::Date);
    }
    if row.status != RowStatus::Posted {
        return Err(Excluded::Status);
    }
    if !row.kind.matchable(probe.side) {
        return Err(Excluded::Kind);
    }
    if let (Some(a), Some(b)) = (probe.contact, row.contact)
        && a != b
    {
        return Err(Excluded::OtherContact);
    }
    Ok(())
}

/// The candidates for `probe` among `rows` (any order, possibly repeated
/// across search pages: each id counts once), best first, at most
/// [`MAX_CANDIDATES`].
#[must_use]
pub fn candidates(probe: &Probe, rows: &[RegisterRow]) -> Vec<Candidate> {
    let probe_name = probe
        .payee
        .as_deref()
        .map(normalize_loose)
        .filter(|n| !n.is_empty());
    let mut seen: BTreeSet<TransactionId> = BTreeSet::new();
    let mut out: Vec<Candidate> = Vec::new();
    for (i, row) in rows.iter().enumerate() {
        if check(probe, row).is_err() || !seen.insert(row.id) {
            continue;
        }
        let days = probe.date.days_until(row.date).unsigned_abs();
        let days_apart = u32::try_from(days).unwrap_or(u32::MAX);
        let same_contact = probe.contact.is_some() && probe.contact == row.contact;
        let same_name = probe_name.as_deref().is_some_and(|p| {
            row.payee
                .as_deref()
                .map(normalize_loose)
                .is_some_and(|r| r == p)
        });
        let close = i64::from(days_apart) <= STRONG_DAYS;
        let label = if close && (same_contact || row.same_account) {
            MatchLabel::Strong
        } else {
            MatchLabel::Possible
        };
        out.push(Candidate {
            id: row.id,
            row: i,
            label,
            days_apart,
            same_contact,
            same_account: row.same_account,
            same_name,
        });
    }
    out.sort_by_key(|c| {
        (
            Reverse(c.same_account),
            Reverse(c.same_contact),
            Reverse(c.same_name),
            c.days_apart,
            c.id,
        )
    });
    out.truncate(MAX_CANDIDATES);
    out
}

/// The register search window: the document's date ± [`DATE_WINDOW_DAYS`]
/// (clamped at the calendar's ends).
#[must_use]
pub fn search_window(date: CalendarDate) -> (CalendarDate, CalendarDate) {
    let start = date
        .add_days(-DATE_WINDOW_DAYS)
        .unwrap_or_else(|_| CalendarDate::min());
    let end = date
        .add_days(DATE_WINDOW_DAYS)
        .unwrap_or_else(|_| CalendarDate::max());
    (start, end)
}

/// The amount as the engine's free-text `search` matches it before the
/// exact `amount` filter (U-95 c) is pinned: two decimals (`numeric(19,4)`
/// text starts with them, `12.50` in `12.5000`). The exact filter is still
/// applied in [`candidates`]. `None` for an amount with more than two
/// decimals that are not zero (it can't be a register total typed in
/// cents, and a shorter search would match too much).
#[must_use]
pub fn search_text(amount: Money) -> Option<String> {
    let wire = amount.to_wire();
    let (whole, frac) = wire.split_once('.').unwrap_or((wire.as_str(), ""));
    let frac = frac.trim_end_matches('0');
    if frac.len() > 2 {
        return None;
    }
    let whole = whole.trim_start_matches('-');
    Some(format!("{whole}.{frac:0<2}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn money(s: &str) -> Money {
        Money::parse_wire(s).unwrap()
    }

    fn day(d: u32) -> CalendarDate {
        CalendarDate::from_ymd(2026, 3, d).unwrap()
    }

    fn tid(n: u8) -> TransactionId {
        TransactionId::parse(&format!("00000000-0000-4000-8000-0000000000{n:02x}")).unwrap()
    }

    fn cid(n: u8) -> ContactId {
        ContactId::parse(&format!("00000000-0000-4000-9000-0000000000{n:02x}")).unwrap()
    }

    fn probe() -> Probe {
        Probe {
            amount: money("25.00"),
            date: day(14),
            payee: Some("Riverbend Supply".into()),
            contact: Some(cid(1)),
            side: DocSide::Purchase,
        }
    }

    fn row(n: u8, amount: &str, d: u32) -> RegisterRow {
        RegisterRow {
            id: tid(n),
            date: day(d),
            amount: money(amount),
            payee: None,
            contact: None,
            kind: RowKind::Expense,
            status: RowStatus::Posted,
            same_account: false,
        }
    }

    #[test]
    fn amounts_must_be_exactly_equal() {
        let mut p = probe();
        p.amount = money("10.10");
        assert_eq!(check(&p, &row(1, "10.01", 14)), Err(Excluded::Amount));
        assert_eq!(check(&p, &row(1, "10.1", 14)), Ok(()));
        assert_eq!(check(&p, &row(1, "10.1000", 14)), Ok(()));
        assert_eq!(check(&p, &row(1, "10.11", 14)), Err(Excluded::Amount));
        assert_eq!(check(&p, &row(1, "-10.10", 14)), Err(Excluded::Amount));
    }

    #[test]
    fn the_date_window_is_seven_days_either_way() {
        let p = probe();
        assert_eq!(check(&p, &row(1, "25", 7)), Ok(()));
        assert_eq!(check(&p, &row(1, "25", 21)), Ok(()));
        assert_eq!(check(&p, &row(1, "25", 6)), Err(Excluded::Date));
        assert_eq!(check(&p, &row(1, "25", 22)), Err(Excluded::Date));
        let (start, end) = search_window(day(14));
        assert_eq!((start, end), (day(7), day(21)));
    }

    #[test]
    fn only_posted_expenses_checks_and_card_charges_are_offered_for_a_purchase() {
        let p = probe();
        for (status, ok) in [
            (RowStatus::Posted, true),
            (RowStatus::Void, false),
            (RowStatus::Deleted, false),
            (RowStatus::Draft, false),
            (RowStatus::Other, false),
        ] {
            let mut r = row(1, "25", 14);
            r.status = status;
            assert_eq!(check(&p, &r).is_ok(), ok, "{status:?}");
        }
        for (kind, ok) in [
            ("expense", true),
            ("check", true),
            ("credit_card_charge", true),
            ("credit_card_credit", false),
            ("deposit", false),
            ("journal_entry", false),
            ("transfer", false),
            ("invoice", false),
            ("bill", false),
            ("bill_payment", false),
            ("something_new", false),
        ] {
            let mut r = row(1, "25", 14);
            r.kind = RowKind::from_txn_type(kind);
            let want = if ok { Ok(()) } else { Err(Excluded::Kind) };
            assert_eq!(check(&p, &r), want, "{kind}");
        }
        assert_eq!(RowStatus::from_wire("void"), RowStatus::Void);
        assert_eq!(RowKind::from_txn_type("invoice"), RowKind::Other);
        assert_eq!(RowKind::from_txn_type("transfer"), RowKind::Other);
    }

    #[test]
    fn a_credit_or_refund_is_offered_card_credits_only() {
        let mut p = probe();
        p.side = DocSide::Credit;
        for (kind, ok) in [
            ("credit_card_credit", true),
            ("credit_card_charge", false),
            ("expense", false),
            ("check", false),
            ("deposit", false),
            ("journal_entry", false),
            ("transfer", false),
            ("invoice", false),
            ("bill", false),
            ("bill_payment", false),
        ] {
            let mut r = row(1, "25", 14);
            r.kind = RowKind::from_txn_type(kind);
            assert_eq!(check(&p, &r).is_ok(), ok, "{kind}");
        }
    }

    #[test]
    fn transfers_and_invoices_of_the_same_amount_are_never_candidates() {
        let p = probe();
        let mut transfer = row(1, "25", 14);
        transfer.kind = RowKind::from_txn_type("transfer");
        transfer.same_account = true;
        let mut invoice = row(2, "25", 14);
        invoice.kind = RowKind::from_txn_type("invoice");
        invoice.contact = Some(cid(1));
        let expense = row(3, "25", 14);
        let got = candidates(&p, &[transfer, invoice, expense]);
        let ids: Vec<TransactionId> = got.iter().map(|c| c.id).collect();
        assert_eq!(ids, [tid(3)]);
    }

    #[test]
    fn a_different_known_contact_is_never_offered() {
        let p = probe();
        let mut other = row(1, "25", 14);
        other.contact = Some(cid(2));
        other.same_account = true;
        assert_eq!(check(&p, &other), Err(Excluded::OtherContact));
        // No contact on the row (a bank-feed expense): offered.
        let bare = row(2, "25", 14);
        assert_eq!(check(&p, &bare), Ok(()));
        // No contact on the document: any row's contact is fine.
        let mut q = probe();
        q.contact = None;
        assert_eq!(check(&q, &other), Ok(()));
        let got = candidates(&p, &[other, bare]);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id, tid(2));
    }

    #[test]
    fn ranking_and_labels() {
        let p = probe();
        let mut same_contact_far = row(1, "25", 19);
        same_contact_far.contact = Some(cid(1));
        let mut same_account_near = row(2, "25", 13);
        same_account_near.same_account = true;
        let mut by_name = row(3, "25", 15);
        by_name.payee = Some("RIVERBEND  supply".into());
        let plain_near = row(4, "25", 14);
        let plain_far = row(5, "25", 20);
        let mut same_contact_near = row(6, "25", 16);
        same_contact_near.contact = Some(cid(1));
        let rows = [
            plain_far,
            by_name,
            same_contact_far,
            plain_near,
            same_account_near,
            same_contact_near,
        ];
        let got = candidates(&p, &rows);
        let ids: Vec<TransactionId> = got.iter().map(|c| c.id).collect();
        assert_eq!(ids, [tid(2), tid(6), tid(1), tid(3), tid(4)]);
        let labels: Vec<MatchLabel> = got.iter().map(|c| c.label).collect();
        assert_eq!(
            labels,
            [
                MatchLabel::Strong,
                MatchLabel::Strong,
                MatchLabel::Possible,
                MatchLabel::Possible,
                MatchLabel::Possible
            ]
        );
        assert_eq!(rows[got[0].row].id, tid(2));
        assert_eq!(got[2].days_apart, 5);
        assert!(got[3].same_name);
    }

    #[test]
    fn repeated_rows_count_once_and_at_most_five_are_kept() {
        let p = probe();
        let rows: Vec<RegisterRow> = (0..12).map(|i| row(i % 8, "25", 14)).collect();
        let got = candidates(&p, &rows);
        assert_eq!(got.len(), MAX_CANDIDATES);
        let ids: BTreeSet<TransactionId> = got.iter().map(|c| c.id).collect();
        assert_eq!(ids.len(), MAX_CANDIDATES);
    }

    #[test]
    fn search_text_is_two_decimals() {
        assert_eq!(search_text(money("12.5")).as_deref(), Some("12.50"));
        assert_eq!(search_text(money("12.5000")).as_deref(), Some("12.50"));
        assert_eq!(search_text(money("1200")).as_deref(), Some("1200.00"));
        assert_eq!(search_text(money("-3.10")).as_deref(), Some("3.10"));
        assert_eq!(search_text(money("1.2345")), None);
    }

    #[test]
    fn debug_prints_no_amounts_or_names() {
        let mut r = row(1, "544.82", 14);
        r.payee = Some("Riverbend".into());
        let text = format!("{:?} {:?}", probe(), r);
        for secret in ["544", "25", "Riverbend", "2026"] {
            assert!(!text.contains(secret), "{secret}: {text}");
        }
    }

    proptest! {
        /// A candidate's amount always equals the probe's: no amount that
        /// differs, by any number of ten-thousandths, is ever offered.
        #[test]
        fn no_differing_amount_is_offered(a in -1_000_000i64..1_000_000, b in -1_000_000i64..1_000_000, d in 0u32..28) {
            let mut p = probe();
            p.amount = Money::from_ten_thousandths(i128::from(a)).unwrap();
            let r = RegisterRow {
                amount: Money::from_ten_thousandths(i128::from(b)).unwrap(),
                ..row(1, "0", d + 1)
            };
            let got = candidates(&p, std::slice::from_ref(&r));
            prop_assert_eq!(!got.is_empty(), a == b && p.date.days_until(r.date).abs() <= 7);
            for c in got {
                prop_assert_eq!(c.id, r.id);
                prop_assert_eq!(r.amount, p.amount);
            }
        }
    }
}
