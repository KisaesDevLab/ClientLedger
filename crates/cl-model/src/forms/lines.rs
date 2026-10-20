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
    /// Customer (or customer job) the line is for.
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
