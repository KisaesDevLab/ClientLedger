//! Shared builders for the contract tests. Money is always built from exact
//! strings (`Money::parse_wire`); no float anywhere.

use cl_api::{
    Account, AccountFields, AccountType, Basis, IdempotencyKey, JeLine, NewJournalEntry,
    PostingOptions, Session, Side, Transaction, Versioned,
};
use cl_core::ids::AccountId;
use cl_core::{CalendarDate, Money};

use crate::support::{Harness, unique};

pub fn m(s: &str) -> Money {
    Money::parse_wire(s).unwrap_or_else(|e| panic!("money {s}: {e:?}"))
}

pub fn date(y: i32, mo: u32, d: u32) -> CalendarDate {
    CalendarDate::from_ymd(y, mo, d).expect("date")
}

pub fn key() -> IdempotencyKey {
    IdempotencyKey::new()
}

pub fn account_fields(t: AccountType, prefix: &str) -> AccountFields {
    AccountFields {
        name: unique(prefix),
        account_number: None,
        account_type: t,
        detail_type: None,
        description: None,
        parent_id: None,
    }
}

pub fn account(h: &Harness, s: &Session, t: AccountType, prefix: &str) -> Account {
    h.run(s.accounts().create(&account_fields(t, prefix), key()))
        .unwrap_or_else(|e| panic!("create account: {e}"))
        .record
}

pub fn line(account: AccountId, side: Side) -> JeLine {
    JeLine {
        account,
        side,
        description: None,
        tag: None,
    }
}

/// A two-line entry: `amount` from `credit` to `debit`.
pub fn je2(
    on: CalendarDate,
    memo: &str,
    debit: AccountId,
    credit: AccountId,
    amount: &str,
) -> NewJournalEntry {
    NewJournalEntry::new(
        on,
        Some(memo.to_string()),
        Basis::Both,
        vec![
            line(debit, Side::Debit(m(amount))),
            line(credit, Side::Credit(m(amount))),
        ],
    )
    .expect("balanced entry")
}

pub fn post(h: &Harness, s: &Session, entry: &NewJournalEntry) -> Versioned<Transaction> {
    h.run(
        s.transactions()
            .create_journal_entry(entry, &PostingOptions::default(), key()),
    )
    .unwrap_or_else(|e| panic!("post entry: {e}"))
}

/// Two fresh accounts for postings: (asset, expense).
pub fn pair(h: &Harness, s: &Session) -> (Account, Account) {
    (
        account(h, s, AccountType::Asset, "Cash"),
        account(h, s, AccountType::Expense, "Supplies"),
    )
}

/// Debit and credit wire strings of a transaction's lines, in line order.
pub fn wire_lines(t: &Transaction) -> Vec<(String, String)> {
    let mut lines: Vec<_> = t.lines.iter().collect();
    lines.sort_by_key(|l| l.line_order.unwrap_or(0));
    lines
        .into_iter()
        .map(|l| (l.debit.to_wire(), l.credit.to_wire()))
        .collect()
}
