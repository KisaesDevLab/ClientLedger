//! The line model every transaction form shares (P13 WP3.1): the register's
//! Splits panel and Write Check's Expenses tab hold the same [`FormLines`]
//! value, so the exact lines-versus-amount arithmetic exists once.
//!
//! "Remaining" is the header amount the owner typed minus the amounts typed
//! on the lines: exact [`Money`] arithmetic on the owner's own input, never a
//! ledger figure (non-negotiable 7).

use std::fmt;

use cl_core::Money;
use cl_core::ids::{AccountId, ContactId, TagId};

use crate::rowkey::{RowKey, RowKeyGen};
use crate::text::TextKey;

/// One line of a form. `Debug` is redacted (amounts and memos are owner
/// data).
#[derive(Clone, PartialEq, Eq)]
pub struct FormLine {
    /// Stable row key (never an engine id).
    pub key: RowKey,
    /// Account.
    pub account: Option<AccountId>,
    /// Amount (the owner's typed value).
    pub amount: Option<Money>,
    /// Memo.
    pub memo: String,
    /// Customer (or customer project) the line is for.
    pub customer: Option<ContactId>,
    /// Billable to the customer.
    pub billable: bool,
    /// Tag (one per line, Q-233).
    pub tag: Option<TagId>,
}

impl FormLine {
    /// A blank line.
    #[must_use]
    pub fn blank(key: RowKey) -> FormLine {
        FormLine {
            key,
            account: None,
            amount: None,
            memo: String::new(),
            customer: None,
            billable: false,
            tag: None,
        }
    }

    /// Nothing entered.
    #[must_use]
    pub fn is_blank(&self) -> bool {
        *self == FormLine::blank(self.key)
    }
}

impl fmt::Debug for FormLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FormLine")
            .field("key", &self.key)
            .field("account", &self.account.is_some())
            .field("amount", &self.amount.is_some())
            .field("memo", &!self.memo.is_empty())
            .field("customer", &self.customer.is_some())
            .field("billable", &self.billable)
            .field("tag", &self.tag.is_some())
            .finish()
    }
}

/// A form's lines, normally with one blank entry line last.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FormLines {
    /// Lines.
    pub lines: Vec<FormLine>,
}

impl FormLines {
    /// Lines from `seeds`, each with a fresh key, plus the blank entry line.
    #[must_use]
    pub fn from_lines(seeds: Vec<FormLine>, keys: &mut RowKeyGen) -> FormLines {
        let mut lines: Vec<FormLine> = seeds
            .into_iter()
            .map(|l| FormLine {
                key: keys.next_key(),
                ..l
            })
            .collect();
        ensure_entry(&mut lines, keys);
        FormLines { lines }
    }

    /// The lines with something on them.
    pub fn used(&self) -> impl Iterator<Item = &FormLine> {
        self.lines.iter().filter(|l| !l.is_blank())
    }

    /// Mutable access to the lines with something on them.
    pub fn used_mut(&mut self) -> impl Iterator<Item = &mut FormLine> {
        self.lines.iter_mut().filter(|l| !l.is_blank())
    }

    /// Exactly one blank entry line at the end (see [`ensure_entry`]).
    pub fn ensure_entry(&mut self, keys: &mut RowKeyGen) {
        ensure_entry(&mut self.lines, keys);
    }
}

/// Exactly one blank entry line at the end: trailing blank lines collapse to
/// one, and a blank line is added when the last one is used.
pub fn ensure_entry(lines: &mut Vec<FormLine>, keys: &mut RowKeyGen) {
    while lines.len() >= 2 && lines[lines.len() - 2..].iter().all(FormLine::is_blank) {
        lines.pop();
    }
    if lines.last().is_none_or(|l| !l.is_blank()) {
        lines.push(FormLine::blank(keys.next_key()));
    }
}

/// The exact sum of the amounts typed on the lines (blank amounts count as
/// nothing; no lines is zero). `None` only when the sum leaves the money
/// range.
#[must_use]
pub fn lines_total(v: &FormLines) -> Option<Money> {
    Money::sum(v.lines.iter().filter_map(|l| l.amount)).ok()
}

/// Remaining to assign: the header amount minus the typed line amounts
/// (`None` without a header amount or when the sum does not fit). Form
/// arithmetic on the owner's input only.
#[must_use]
pub fn remaining(header: Option<Money>, v: &FormLines) -> Option<Money> {
    header?.checked_sub(lines_total(v)?).ok()
}

/// The lines that just got their account (Q-304, as Q-P15n): an account and
/// no amount now, and no account (or no such line) in `seen`. In line order.
#[must_use]
pub fn fresh_account_lines(seen: &FormLines, now: &FormLines) -> Vec<RowKey> {
    now.lines
        .iter()
        .filter(|l| l.account.is_some() && l.amount.is_none())
        .filter(|l| {
            seen.lines
                .iter()
                .find(|o| o.key == l.key)
                .is_none_or(|o| o.account.is_none())
        })
        .map(|l| l.key)
        .collect()
}

/// What line `key` takes when it gets its account with no amount (Q-304):
/// the typed `header` total, less `elsewhere` (amounts assigned outside
/// these lines, e.g. a bill's item lines), less the other lines' amounts.
/// `None` without a header total, when nothing is left (fully or over
/// assigned), when the line already has an amount, or for an unknown key.
/// Form arithmetic on the owner's own input only.
#[must_use]
pub fn remaining_for(
    header: Option<Money>,
    elsewhere: Money,
    lines: &FormLines,
    key: RowKey,
) -> Option<Money> {
    let header = header?;
    let line = lines.lines.iter().find(|l| l.key == key)?;
    if line.amount.is_some() {
        return None;
    }
    // The line's own amount is blank, so the lines total is the others'.
    let left = header
        .checked_sub(elsewhere)
        .ok()?
        .checked_sub(lines_total(lines)?)
        .ok()?;
    left.is_positive().then_some(left)
}

/// Puts [`remaining_for`] on line `key`. False (and nothing changed) when
/// there is nothing to put; an amount the owner typed is never changed.
pub fn fill_remaining(
    header: Option<Money>,
    elsewhere: Money,
    lines: &mut FormLines,
    key: RowKey,
) -> bool {
    let Some(left) = remaining_for(header, elsewhere, lines, key) else {
        return false;
    };
    let Some(line) = lines.lines.iter_mut().find(|l| l.key == key) else {
        return false;
    };
    line.amount = Some(left);
    true
}

/// Fills every line that just got its account (see [`fresh_account_lines`])
/// with what is left, in line order, so the first takes the whole total
/// and later ones what the earlier left (often nothing). True when a line
/// changed.
pub fn fill_fresh_lines(
    header: Option<Money>,
    elsewhere: Money,
    seen: &FormLines,
    lines: &mut FormLines,
) -> bool {
    let mut changed = false;
    for key in fresh_account_lines(seen, lines) {
        changed |= fill_remaining(header, elsewhere, lines, key);
    }
    changed
}

/// The footer line under a form's lines (tri-state, P13 plan WP6): the
/// header amount against the typed lines total.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TotalsStatus {
    /// The lines add up to the header amount.
    Balanced,
    /// This much of the header amount is not on a line yet.
    Remaining(Money),
    /// The lines are this much more than the header amount.
    Over(Money),
}

/// Header amount − sum of the typed line amounts, in exact units (blank
/// amounts count as nothing). Saturates at the money range (only reachable
/// with absurd line sums; a save is refused either way).
#[must_use]
pub fn difference(header: Money, lines: &FormLines) -> Money {
    let sum: i128 = lines
        .lines
        .iter()
        .filter_map(|l| l.amount)
        .map(Money::to_ten_thousandths)
        .fold(0_i128, i128::saturating_add);
    let diff = header.to_ten_thousandths().saturating_sub(sum);
    Money::from_ten_thousandths(diff).unwrap_or(if diff > 0 { Money::MAX } else { Money::MIN })
}

/// The exact lines-versus-header footer every lines form shares (P14 C2.1):
/// `None` without a header amount or when the lines total leaves the money
/// range.
#[must_use]
pub fn exact_totals(header: Option<Money>, lines: &FormLines) -> Option<TotalsStatus> {
    let left = header?.checked_sub(lines_total(lines)?).ok()?;
    Some(if left.is_zero() {
        TotalsStatus::Balanced
    } else if left.is_positive() {
        TotalsStatus::Remaining(left)
    } else {
        TotalsStatus::Over(left.abs())
    })
}

/// One checked line, ready for a posting builder.
#[derive(Clone, PartialEq, Eq)]
pub struct PostingLine {
    /// Account.
    pub account: AccountId,
    /// Amount (the owner's typed value, more than zero).
    pub amount: Money,
    /// Memo, when typed (trimmed).
    pub memo: Option<String>,
    /// Customer.
    pub contact: Option<ContactId>,
    /// Billable to the customer.
    pub billable: bool,
    /// Tag (one per line, Q-233).
    pub tag: Option<TagId>,
}

impl fmt::Debug for PostingLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PostingLine")
            .field("memo", &self.memo.is_some())
            .field("contact", &self.contact.is_some())
            .field("billable", &self.billable)
            .field("tag", &self.tag.is_some())
            .finish_non_exhaustive()
    }
}

/// Why the lines cannot become posting lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LinesProblem {
    /// A used line has no account, or no line is used.
    NeedAccount,
    /// A used line's amount is missing, zero or negative.
    AmountNotPositive,
    /// The lines don't add up to the header amount.
    NotAssigned,
}

impl LinesProblem {
    /// Plain-language message (the register's P12 wording).
    #[must_use]
    pub const fn key(self) -> TextKey {
        match self {
            LinesProblem::NeedAccount => TextKey("register.splits.need_account"),
            LinesProblem::AmountNotPositive => {
                TextKey("register.entry.problem.amount_not_positive")
            }
            LinesProblem::NotAssigned => TextKey("register.splits.not_assigned"),
        }
    }
}

/// Trims a memo; `None` when nothing is left.
pub(crate) fn trimmed(s: &str) -> Option<String> {
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_string())
}

/// The used lines as posting lines, or the first problem: a used line
/// without an account or a positive amount (in line order), no used line,
/// or lines that don't add up to `header` exactly.
///
/// # Errors
/// See [`LinesProblem`].
pub fn to_posting_lines(header: Money, v: &FormLines) -> Result<Vec<PostingLine>, LinesProblem> {
    let mut out = Vec::new();
    for l in v.used() {
        let account = l.account.ok_or(LinesProblem::NeedAccount)?;
        let amount = l
            .amount
            .filter(|m| m.is_positive())
            .ok_or(LinesProblem::AmountNotPositive)?;
        out.push(PostingLine {
            account,
            amount,
            memo: trimmed(&l.memo),
            contact: l.customer,
            billable: l.billable,
            tag: l.tag,
        });
    }
    if out.is_empty() {
        return Err(LinesProblem::NeedAccount);
    }
    if remaining(Some(header), v) != Some(Money::ZERO) {
        return Err(LinesProblem::NotAssigned);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn money(s: &str) -> Money {
        Money::parse_wire(s).expect("money")
    }

    fn account(n: u32) -> AccountId {
        AccountId::parse(&format!("00000000-0000-0000-0000-{n:012}")).expect("id")
    }

    fn line(keys: &mut RowKeyGen, n: u32, amount: &str) -> FormLine {
        FormLine {
            account: Some(account(n)),
            amount: Some(money(amount)),
            ..FormLine::blank(keys.next_key())
        }
    }

    #[test]
    fn entry_line_is_kept_single_and_last() {
        let mut keys = RowKeyGen::new();
        let mut v = FormLines::default();
        v.ensure_entry(&mut keys);
        assert_eq!(v.lines.len(), 1);
        v.lines.push(FormLine::blank(keys.next_key()));
        v.lines.push(FormLine::blank(keys.next_key()));
        v.ensure_entry(&mut keys);
        assert_eq!(v.lines.len(), 1, "trailing blanks collapse");
        v.lines[0].memo = "x".into();
        v.ensure_entry(&mut keys);
        assert_eq!(v.lines.len(), 2);
        assert!(v.lines[1].is_blank());
        assert_eq!(v.used().count(), 1);
    }

    #[test]
    fn from_lines_gives_fresh_keys_and_an_entry_line() {
        let mut keys = RowKeyGen::after([RowKey(40)]);
        let seed = FormLine {
            memo: "m".into(),
            ..FormLine::blank(RowKey(1))
        };
        let v = FormLines::from_lines(vec![seed.clone(), seed], &mut keys);
        let ks: Vec<u64> = v.lines.iter().map(|l| l.key.0).collect();
        assert_eq!(ks, vec![41, 42, 43]);
        assert!(v.lines[2].is_blank());
    }

    /// Sets line `i`'s account (no amount), as picking it in the grid does.
    fn pick(v: &mut FormLines, keys: &mut RowKeyGen, i: usize, n: u32) -> RowKey {
        v.lines[i].account = Some(account(n));
        v.ensure_entry(keys);
        v.lines[i].key
    }

    #[test]
    fn the_first_line_takes_the_whole_total_and_the_next_what_is_left() {
        let mut keys = RowKeyGen::new();
        let mut v = FormLines::from_lines(Vec::new(), &mut keys);
        let total = Some(money("1500.01"));
        let seen = v.clone();
        let first = pick(&mut v, &mut keys, 0, 1500);
        assert_eq!(fresh_account_lines(&seen, &v), vec![first]);
        assert!(fill_fresh_lines(total, Money::ZERO, &seen, &mut v));
        assert_eq!(v.lines[0].amount, Some(money("1500.01")));
        // The owner types 1,000.00 over it; the next line takes 500.01.
        v.lines[0].amount = Some(money("1000.00"));
        let seen = v.clone();
        let next = pick(&mut v, &mut keys, 1, 6100);
        assert_eq!(
            remaining_for(total, Money::ZERO, &v, next),
            Some(money("500.01"))
        );
        assert!(fill_fresh_lines(total, Money::ZERO, &seen, &mut v));
        assert_eq!(v.lines[1].amount, Some(money("500.01")));
        assert_eq!(v.lines[0].amount, Some(money("1000.00")), "typed, kept");
        assert_eq!(remaining(total, &v), Some(Money::ZERO));
        // Fully assigned: a third line gets nothing.
        let seen = v.clone();
        let third = pick(&mut v, &mut keys, 2, 6200);
        assert_eq!(remaining_for(total, Money::ZERO, &v, third), None);
        assert!(!fill_fresh_lines(total, Money::ZERO, &seen, &mut v));
        assert_eq!(v.lines[2].amount, None);
    }

    #[test]
    fn nothing_is_filled_without_a_total_or_when_over_assigned() {
        let mut keys = RowKeyGen::new();
        let mut v = FormLines {
            lines: vec![line(&mut keys, 6100, "80.00")],
        };
        v.ensure_entry(&mut keys);
        let seen = v.clone();
        let key = pick(&mut v, &mut keys, 1, 6200);
        assert_eq!(remaining_for(None, Money::ZERO, &v, key), None);
        assert!(!fill_fresh_lines(None, Money::ZERO, &seen, &mut v));
        assert_eq!(
            remaining_for(Some(money("75.00")), Money::ZERO, &v, key),
            None,
            "over-assigned"
        );
        assert_eq!(
            remaining_for(Some(money("80.00")), Money::ZERO, &v, key),
            None,
            "fully assigned"
        );
        // Amounts outside these lines (a bill's items) count as assigned.
        assert_eq!(
            remaining_for(Some(money("100.00")), money("15.00"), &v, key),
            Some(money("5.00"))
        );
        assert_eq!(
            remaining_for(Some(money("100.00")), money("20.00"), &v, key),
            None
        );
        assert_eq!(
            remaining_for(Some(money("100.00")), Money::ZERO, &v, RowKey(999)),
            None,
            "unknown key"
        );
    }

    #[test]
    fn a_typed_amount_is_never_changed_and_only_new_accounts_count() {
        let mut keys = RowKeyGen::new();
        let mut v = FormLines {
            lines: vec![line(&mut keys, 6100, "10.00")],
        };
        v.ensure_entry(&mut keys);
        let total = Some(money("50.00"));
        let typed = v.lines[0].key;
        assert_eq!(remaining_for(total, Money::ZERO, &v, typed), None);
        assert!(!fill_remaining(total, Money::ZERO, &mut v, typed));
        assert_eq!(v.lines[0].amount, Some(money("10.00")));
        // A line whose amount the owner cleared keeps its account: not new.
        let seen = v.clone();
        v.lines[0].amount = None;
        assert!(fresh_account_lines(&seen, &v).is_empty());
        assert!(!fill_fresh_lines(total, Money::ZERO, &seen, &mut v));
        assert_eq!(v.lines[0].amount, None);
        // A changed account on a line with an account is not new either.
        let seen = v.clone();
        v.lines[0].account = Some(account(6200));
        assert!(fresh_account_lines(&seen, &v).is_empty());
        // A line that arrives with an account and no amount is.
        let seen = FormLines::default();
        assert_eq!(fresh_account_lines(&seen, &v), vec![v.lines[0].key]);
        // Two new lines at once: the first takes it all, the second nothing.
        let mut w = FormLines::from_lines(Vec::new(), &mut keys);
        let seen = w.clone();
        w.lines[0].account = Some(account(1));
        w.lines.push(FormLine {
            account: Some(account(2)),
            ..FormLine::blank(keys.next_key())
        });
        assert!(fill_fresh_lines(total, Money::ZERO, &seen, &mut w));
        assert_eq!(w.lines[0].amount, Some(money("50.00")));
        assert_eq!(w.lines[1].amount, None);
    }

    #[test]
    fn remaining_is_the_typed_amount_less_the_typed_lines() {
        let mut keys = RowKeyGen::new();
        let mut v = FormLines {
            lines: vec![
                line(&mut keys, 6100, "100.10"),
                line(&mut keys, 6350, "25.40"),
            ],
        };
        v.ensure_entry(&mut keys);
        assert_eq!(v.lines.len(), 3, "one blank entry line");
        assert_eq!(lines_total(&v), Some(money("125.50")));
        assert_eq!(remaining(Some(money("125.50")), &v), Some(Money::ZERO));
        assert_eq!(remaining(Some(money("130.00")), &v), Some(money("4.50")));
        assert_eq!(remaining(Some(money("120.00")), &v), Some(money("-5.50")));
        assert_eq!(remaining(None, &v), None);
        let lines = to_posting_lines(money("125.50"), &v).expect("lines");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1].amount, money("25.40"));
        assert_eq!(
            to_posting_lines(money("130.00"), &v).err(),
            Some(LinesProblem::NotAssigned)
        );
        assert_eq!(
            to_posting_lines(money("125.49"), &v).err(),
            Some(LinesProblem::NotAssigned),
            "one cent off is refused"
        );
        v.lines[1].amount = Some(Money::ZERO);
        assert_eq!(
            to_posting_lines(money("100.10"), &v).err(),
            Some(LinesProblem::AmountNotPositive)
        );
        v.lines[0].account = None;
        assert_eq!(
            to_posting_lines(money("125.50"), &v).err(),
            Some(LinesProblem::NeedAccount)
        );
        assert_eq!(
            to_posting_lines(money("1"), &FormLines::default()).err(),
            Some(LinesProblem::NeedAccount)
        );
    }

    #[test]
    fn exact_totals_is_tri_state_to_the_cent() {
        let mut keys = RowKeyGen::new();
        let v = FormLines {
            lines: vec![line(&mut keys, 1, "40.00"), line(&mut keys, 2, "24.10")],
        };
        assert_eq!(
            exact_totals(Some(money("64.20")), &v),
            Some(TotalsStatus::Remaining(money("0.10")))
        );
        assert_eq!(
            exact_totals(Some(money("64.10")), &v),
            Some(TotalsStatus::Balanced)
        );
        assert_eq!(
            exact_totals(Some(money("64.00")), &v),
            Some(TotalsStatus::Over(money("0.10")))
        );
        assert_eq!(exact_totals(None, &v), None);
        assert_eq!(difference(money("64.20"), &v), money("0.10"));
        assert_eq!(difference(money("64.00"), &v), money("-0.10"));
        assert_eq!(difference(Money::ZERO, &FormLines::default()), Money::ZERO);
        let mut a = line(&mut keys, 1, "1");
        a.amount = Some(Money::MIN);
        let huge = FormLines {
            lines: vec![a.clone(), a],
        };
        assert_eq!(difference(Money::MAX, &huge), Money::MAX, "saturates");
    }

    #[test]
    fn posting_lines_carry_customer_billable_and_trimmed_memo() {
        let mut keys = RowKeyGen::new();
        let contact = ContactId::parse("00000000-0000-0000-0000-000000000009").expect("id");
        let mut l = line(&mut keys, 1, "5");
        l.memo = "  mulch  ".into();
        l.customer = Some(contact);
        l.billable = true;
        let v = FormLines { lines: vec![l] };
        let out = to_posting_lines(money("5"), &v).expect("ok");
        assert_eq!(out[0].memo.as_deref(), Some("mulch"));
        assert_eq!(out[0].contact, Some(contact));
        assert!(out[0].billable);
    }

    #[test]
    fn debug_hides_owner_data() {
        let mut keys = RowKeyGen::new();
        let mut l = line(&mut keys, 1, "1286.40");
        l.memo = "secret memo".into();
        let s = format!("{l:?}");
        assert!(!s.contains("1286") && !s.contains("secret"), "{s}");
        let p = to_posting_lines(money("1286.40"), &FormLines { lines: vec![l] }).expect("ok");
        let s = format!("{p:?}");
        assert!(!s.contains("1286") && !s.contains("secret"), "{s}");
    }

    #[test]
    fn problem_keys_keep_the_register_wording() {
        assert_eq!(
            LinesProblem::NeedAccount.key(),
            TextKey("register.splits.need_account")
        );
        assert_eq!(
            LinesProblem::NotAssigned.key(),
            TextKey("register.splits.not_assigned")
        );
        assert_eq!(
            LinesProblem::AmountNotPositive.key(),
            TextKey("register.entry.problem.amount_not_positive")
        );
    }
}
