//! Bulk Entry against the real engine (P15A lane A, WP-A2; engine PR #163,
//! U-67–U-74). Each test is PENDING by the `batch-*-v1` capabilities it
//! needs until the engine pin carries them; the parts the pinned engine
//! already serves (one-section validate, account suggestions, the refusal
//! of float amounts from `parse-csv`) run live.
//!
//! Money is built from exact strings and compared as exact engine values:
//! a register moves by exactly the sum of the batch, never approximately.

use cl_api::{
    AccountType, ApiError, AuditAction, AuditEntity, BatchAmount, BatchField, BatchGroup,
    BatchKind, BatchRowIn, BatchSaved, Capabilities, ColumnMapping, ContactFields, ContactType,
    CsvField, IdempotencyKey, Method, NewContact, ProblemKind, RegisterQuery, Session,
    SuggestSource, is_key_reused,
};
use cl_core::ids::{AccountId, ContactId, TransactionId};
use cl_core::{CalendarDate, Money, Secret};
use serde_json::json;

use crate::common::{account, date, key, m};
use crate::support::{Harness, unique};

/// The capabilities a Bulk Entry save needs (P15A §2).
const SAVE_CAPS: [(&str, &str); 4] = [
    (Capabilities::BATCH_ATOMIC, "engine #163 batch-atomic-v1"),
    (Capabilities::BATCH_EXACT, "engine #163 batch-exact-v1"),
    (
        Capabilities::BATCH_IDEMPOTENT,
        "engine #163 batch-idempotent-v1",
    ),
    (Capabilities::BATCH_MIXED, "engine #163 batch-mixed-v1"),
];

const CHECK_CAP: (&str, &str) = (Capabilities::BATCH_CHECK, "engine #163 batch-check-v1");
const CLOSING_CAP: (&str, &str) = (Capabilities::BATCH_CLOSING, "engine #163 batch-closing-v1");

/// True when the engine has every capability; each missing one is listed
/// as PENDING (no short-circuit, so the list names them all).
fn ready(h: &Harness, caps: &[(&str, &str)]) -> bool {
    let missing = caps
        .iter()
        .filter(|(cap, item)| !h.require_cap(cap, item))
        .count();
    missing == 0
}

fn save_ready(h: &Harness, more: &[(&str, &str)]) -> bool {
    let all: Vec<(&str, &str)> = SAVE_CAPS.iter().chain(more).copied().collect();
    ready(h, &all)
}

fn amount(s: &str) -> BatchAmount {
    BatchAmount::new(m(s)).unwrap_or_else(|e| panic!("amount {s}: {e:?}"))
}

/// A fresh bank account with a vendor, a customer and the line accounts.
struct Books {
    bank: AccountId,
    expense: AccountId,
    income: AccountId,
    vendor: ContactId,
    customer: ContactId,
}

fn contact(h: &Harness, s: &Session, t: ContactType, prefix: &str) -> ContactId {
    h.run(s.contacts().create(
        &NewContact {
            fields: ContactFields::named(t, unique(prefix)),
            opening_balance: Money::ZERO,
            opening_balance_date: None,
        },
        key(),
    ))
    .unwrap_or_else(|e| panic!("contact: {e}"))
    .record
    .id
}

fn books(h: &Harness, s: &Session) -> Books {
    Books {
        bank: account(h, s, AccountType::Asset, "Batch bank").id,
        expense: account(h, s, AccountType::Expense, "Batch supplies").id,
        income: account(h, s, AccountType::Revenue, "Batch sales").id,
        vendor: contact(h, s, ContactType::Vendor, "Batch vendor"),
        customer: contact(h, s, ContactType::Customer, "Batch customer"),
    }
}

fn on() -> CalendarDate {
    date(2026, 6, 15)
}

fn check_row(b: &Books, n: u32, amt: &str, number: Option<&str>) -> BatchRowIn {
    let mut r = BatchRowIn::new(n, on());
    r.number = number.map(str::to_string);
    r.contact = Some(b.vendor);
    r.account = Some(b.expense);
    r.amount = Some(amount(amt));
    r.memo = Some(unique("batch check"));
    r
}

fn deposit_row(b: &Books, n: u32, amt: &str) -> BatchRowIn {
    let mut r = BatchRowIn::new(n, on());
    r.contact = Some(b.customer);
    r.account = Some(b.income);
    r.amount = Some(amount(amt));
    r
}

fn group(kind: BatchKind, context: Option<AccountId>, rows: Vec<BatchRowIn>) -> BatchGroup {
    BatchGroup {
        kind,
        context,
        rows,
    }
}

/// The engine's balance of an account.
fn balance(h: &Harness, s: &Session, a: AccountId) -> Money {
    h.run(s.accounts().get(a))
        .expect("account")
        .record
        .balance
        .unwrap_or_default()
}

fn sum(amounts: &[String]) -> Money {
    amounts
        .iter()
        .map(|a| m(a))
        .try_fold(Money::ZERO, Money::checked_add)
        .expect("sum")
}

/// 50 amounts from 0.01 to 999.99, every third one ending in 5.
fn fifty_amounts() -> Vec<String> {
    (0..50u32)
        .map(|i| {
            let cents = match i {
                0 => 1,
                49 => 99_999,
                _ => {
                    let c = 1 + (i * 20_407) % 99_998;
                    if i % 3 == 0 { c / 10 * 10 + 5 } else { c }
                }
            };
            format!("{}.{:02}", cents / 100, cents % 100)
        })
        .collect()
}

/// Every transaction id in the account's register, with its exact balance.
fn register_ids(h: &Harness, s: &Session, a: AccountId) -> Vec<(TransactionId, Option<Money>)> {
    let q = RegisterQuery::new(date(1900, 1, 1), date(2199, 12, 31));
    let page = h.run(s.register().page(a, &q, 1, 200)).expect("register");
    assert!(page.exact, "the register is exact");
    page.lines
        .iter()
        .map(|l| (l.txn_id, l.balance.exact()))
        .collect()
}

fn saved_ids(saved: &BatchSaved) -> Vec<TransactionId> {
    let mut ids: Vec<_> = saved.saved.iter().map(|t| t.id).collect();
    ids.sort();
    ids.dedup();
    ids
}

/// One section validates on any engine (the pinned engine's one-type
/// shape, or `groups[]`): a known account passes, an unknown name is
/// "Not in your lists".
pub fn validate_one_section_live(h: &Harness) {
    let s = &h.s1;
    let b = books(h, s);
    let ok = check_row(&b, 1, "12.34", None);
    let mut unknown = check_row(&b, 2, "5.00", None);
    unknown.account = None;
    unknown.account_name = Some(unique("No such account"));
    let v = h
        .run(
            s.batch()
                .validate(&[group(BatchKind::Expense, Some(b.bank), vec![ok, unknown])]),
        )
        .expect("validate");
    assert_eq!(v.rows.len(), 2);
    let first = v.rows.iter().find(|r| r.row_number == 1).expect("row 1");
    assert!(first.ok, "{first:?}");
    assert_eq!(first.account, Some(b.expense));
    let second = v.rows.iter().find(|r| r.row_number == 2).expect("row 2");
    assert!(!second.ok);
    assert!(
        second
            .problems
            .iter()
            .any(|p| p.field == BatchField::Account && p.kind == ProblemKind::NotInLists),
        "{second:?}"
    );
}

/// AC1: 25 checks and 25 deposits in one call; the register moves by
/// exactly the batch sum.
pub fn fifty_rows_exact(h: &Harness) {
    if !save_ready(h, &[CHECK_CAP]) {
        return;
    }
    let s = &h.s1;
    let b = books(h, s);
    let amounts = fifty_amounts();
    let (outs, ins) = amounts.split_at(25);
    let checks: Vec<_> = outs
        .iter()
        .zip(1u32..)
        .map(|(a, n)| check_row(&b, n, a, None))
        .collect();
    let deposits: Vec<_> = ins
        .iter()
        .zip(26u32..)
        .map(|(a, n)| deposit_row(&b, n, a))
        .collect();
    let before = balance(h, s, b.bank);
    let saved = h
        .run(s.batch().save(
            &[
                group(BatchKind::Check, Some(b.bank), checks),
                group(BatchKind::Deposit, Some(b.bank), deposits),
            ],
            key(),
            false,
        ))
        .expect("save");
    assert_eq!(saved.saved.len(), 50);
    assert!(saved.batch_id.is_some());
    let moved = balance(h, s, b.bank).checked_sub(before).expect("delta");
    let expected = sum(ins).checked_sub(sum(outs)).expect("expected");
    assert_eq!(moved.to_wire(), expected.to_wire(), "exact to the cent");
    let lines = register_ids(h, s, b.bank);
    assert_eq!(lines.len(), 50);
    assert!(lines.iter().all(|(_, bal)| bal.is_some()));
}

/// AC1: checks, deposits and expenses in one call; every row is in the
/// register with an engine running balance, and the last equals the
/// account balance.
pub fn mixed_one_call_all_in_register(h: &Harness) {
    if !save_ready(h, &[CHECK_CAP]) {
        return;
    }
    let s = &h.s1;
    let b = books(h, s);
    let mut expense = check_row(&b, 6, "7.25", Some("R-1"));
    expense.contact = None;
    let saved = h
        .run(s.batch().save(
            &[
                group(
                    BatchKind::Check,
                    Some(b.bank),
                    vec![
                        check_row(&b, 1, "100.10", None),
                        check_row(&b, 2, "0.05", None),
                        check_row(&b, 3, "45.00", None),
                    ],
                ),
                group(
                    BatchKind::Deposit,
                    Some(b.bank),
                    vec![deposit_row(&b, 4, "250.00"), deposit_row(&b, 5, "19.95")],
                ),
                group(BatchKind::Expense, Some(b.bank), vec![expense]),
            ],
            key(),
            false,
        ))
        .expect("save");
    let groups: Vec<_> = saved
        .saved
        .iter()
        .map(|t| (t.row_number, t.group))
        .collect();
    assert_eq!(groups, [(1, 0), (2, 0), (3, 0), (4, 1), (5, 1), (6, 2)]);
    let lines = register_ids(h, s, b.bank);
    for id in saved_ids(&saved) {
        let (_, bal) = lines
            .iter()
            .find(|(t, _)| *t == id)
            .unwrap_or_else(|| panic!("{id:?} is in the register"));
        assert!(bal.is_some(), "engine balance");
    }
    assert_eq!(balance(h, s, b.bank).to_wire(), m("117.55").to_wire());
}

/// Check numbers: typed ones are kept, empty ones are the next numbers,
/// none repeats, the account's counter moves past them, and a number
/// already used is refused.
pub fn check_numbers_unique_and_counter(h: &Harness) {
    if !save_ready(h, &[CHECK_CAP]) {
        return;
    }
    let s = &h.s1;
    let b = books(h, s);
    let rows = vec![
        check_row(&b, 1, "1.00", Some("5001")),
        check_row(&b, 2, "2.00", None),
        check_row(&b, 3, "3.00", None),
        check_row(&b, 4, "4.00", Some("5010")),
        check_row(&b, 5, "5.00", None),
    ];
    let saved = h
        .run(
            s.batch()
                .save(&[group(BatchKind::Check, Some(b.bank), rows)], key(), false),
        )
        .expect("save");
    let mut numbers: Vec<u32> = saved
        .saved
        .iter()
        .map(|t| {
            h.run(s.checks().get(t.id))
                .expect("check")
                .record
                .check_number
                .expect("numbered")
        })
        .collect();
    assert_eq!(&numbers[..1], [5001]);
    assert_eq!(numbers[3], 5010);
    let highest = *numbers.iter().max().expect("max");
    numbers.sort_unstable();
    numbers.dedup();
    assert_eq!(numbers.len(), 5, "no number repeats");
    let next = h
        .run(s.checks().settings())
        .expect("settings")
        .next_for(b.bank)
        .expect("counter");
    assert!(next > highest, "counter {next} after {highest}");

    let again = [group(
        BatchKind::Check,
        Some(b.bank),
        vec![
            check_row(&b, 1, "1.00", Some("5001")),
            check_row(&b, 2, "1.00", Some("6001")),
            check_row(&b, 3, "1.00", Some("6001")),
        ],
    )];
    let v = h.run(s.batch().validate(&again)).expect("validate");
    for n in [1, 3] {
        let r = v.rows.iter().find(|r| r.row_number == n).expect("row");
        assert!(
            !r.ok && r.problems.iter().any(|p| p.field == BatchField::Number),
            "row {n}: {r:?}"
        );
    }
    let before = balance(h, s, b.bank);
    let e = h
        .run(s.batch().save(&again, key(), false))
        .expect_err("duplicates");
    assert!(matches!(e, ApiError::BatchRows(_)), "{e:?}");
    assert_eq!(balance(h, s, b.bank), before, "nothing posted");
}

/// AC2: row 37 names an inactive account; the save names it and posts
/// nothing at all.
pub fn one_bad_row_saves_nothing(h: &Harness) {
    if !save_ready(h, &[CHECK_CAP]) {
        return;
    }
    let s = &h.s1;
    let b = books(h, s);
    let gone = account(h, s, AccountType::Revenue, "Batch closed line").id;
    h.run(s.accounts().deactivate(gone, key()))
        .expect("deactivate");
    let checks: Vec<_> = (1..=20).map(|n| check_row(&b, n, "10.00", None)).collect();
    let deposits: Vec<_> = (21..=40)
        .map(|n| {
            let mut r = deposit_row(&b, n, "20.00");
            if n == 37 {
                r.account = Some(gone);
            }
            r
        })
        .collect();
    let before = (balance(h, s, b.bank), balance(h, s, b.expense));
    let e = h
        .run(s.batch().save(
            &[
                group(BatchKind::Check, Some(b.bank), checks),
                group(BatchKind::Deposit, Some(b.bank), deposits),
            ],
            key(),
            false,
        ))
        .expect_err("refused");
    let ApiError::BatchRows(rows) = &e else {
        panic!("{e:?}")
    };
    assert!(
        rows.iter().any(|r| r.group == 1 && r.row_number == 37),
        "{rows:?}"
    );
    assert_eq!(
        (balance(h, s, b.bank), balance(h, s, b.expense)),
        before,
        "nothing posted"
    );
    assert_eq!(register_ids(h, s, b.bank).len(), 0, "no register rows");
}

/// Idempotency: the same key and body replay the first result and post
/// nothing again; the same key with another body is refused.
pub fn replay_same_key(h: &Harness) {
    if !save_ready(h, &[]) {
        return;
    }
    let s = &h.s1;
    let b = books(h, s);
    let batch = |amt: &str| {
        [group(
            BatchKind::Deposit,
            Some(b.bank),
            vec![deposit_row(&b, 1, amt), deposit_row(&b, 2, "0.10")],
        )]
    };
    let k = IdempotencyKey::new();
    let first = h
        .run(s.batch().save(&batch("33.33"), k.clone(), false))
        .expect("first");
    let again = h
        .run(s.batch().save(&batch("33.33"), k.clone(), false))
        .expect("replay");
    assert_eq!(first, again, "the first result");
    assert_eq!(balance(h, s, b.bank).to_wire(), m("33.43").to_wire());
    assert_eq!(register_ids(h, s, b.bank).len(), 2, "no duplicates");
    let e = h
        .run(s.batch().save(&batch("33.34"), k, false))
        .expect_err("another body");
    assert!(is_key_reused(&e), "{e:?}");
    assert_eq!(balance(h, s, b.bank).to_wire(), m("33.43").to_wire());
}

/// A row in a closed period: refused without the password and with a
/// wrong one, saved with the right one, and audited. Tenant B, so no
/// other test sees the closing date.
pub fn closed_period_row(h: &Harness) {
    if !save_ready(h, &[CLOSING_CAP])
        || !h.require_cap(Capabilities::CLOSING_PASSWORD, "U-15 closing-password-v1")
    {
        return;
    }
    let s = h.s3();
    let b = books(h, &s);
    let lock = |v: serde_json::Value| {
        h.run(s.raw_mapped(
            Method::Put,
            "/api/v1/company/settings",
            Some(&json!({ "lockDate": v })),
        ))
        .expect("closing date");
    };
    let pw = Secret::new(unique("closing-batch"));
    h.run(s.set_closing_password(None, Some(&pw), IdempotencyKey::new()))
        .expect("password");
    lock(json!("2026-01-31"));
    let batch = |password: Option<&Secret>| {
        let mut closed = deposit_row(&b, 1, "12.00");
        closed.date = date(2026, 1, 15);
        closed.closing_password = password.cloned();
        [group(
            BatchKind::Deposit,
            Some(b.bank),
            vec![closed, deposit_row(&b, 2, "1.00")],
        )]
    };
    let e = h
        .run(s.batch().save(&batch(None), key(), false))
        .expect_err("closed");
    assert!(
        matches!(&e, ApiError::ClosedPeriod(c) if c.password_required == Some(true)),
        "{e:?}"
    );
    let e = h
        .run(
            s.batch()
                .save(&batch(Some(&Secret::new("not-the-password"))), key(), false),
        )
        .expect_err("wrong");
    assert_eq!(e, ApiError::ClosingPasswordRejected);
    assert_eq!(balance(h, &s, b.bank), Money::ZERO, "nothing posted");
    let saved = h
        .run(s.batch().save(&batch(Some(&pw)), key(), false))
        .expect("right password");
    assert_eq!(balance(h, &s, b.bank).to_wire(), m("13.00").to_wire());
    let closed_txn = saved
        .saved
        .iter()
        .find(|t| t.row_number == 1)
        .expect("row 1")
        .id;
    let audit = h
        .run(
            s.audit()
                .for_record(AuditEntity::ClosingDateOverride, *closed_txn.as_uuid()),
        )
        .expect("audit");
    assert!(
        audit.iter().any(|r| r.action == AuditAction::Override),
        "the closed-period row is audited"
    );
    lock(serde_json::Value::Null);
    h.run(s.set_closing_password(Some(&pw), None, IdempotencyKey::new()))
        .expect("clear password");
}

/// A journal entry group whose debits and credits differ by a cent is
/// refused, and nothing posts.
pub fn je_group_unbalanced_refused(h: &Harness) {
    if !save_ready(h, &[]) {
        return;
    }
    let s = &h.s1;
    let b = books(h, s);
    let line = |n: u32, debit: Option<&str>, credit: Option<&str>, a: AccountId| {
        let mut r = BatchRowIn::new(n, on());
        r.number = Some("JE-U1".into());
        r.account = Some(a);
        r.debit = debit.map(amount);
        r.credit = credit.map(amount);
        r
    };
    let batch = [group(
        BatchKind::JournalEntry,
        None,
        vec![
            line(1, Some("10.00"), None, b.expense),
            line(2, None, Some("9.99"), b.bank),
        ],
    )];
    let v = h.run(s.batch().validate(&batch)).expect("validate");
    assert!(
        v.rows
            .iter()
            .any(|r| r.problems.iter().any(|p| p.kind == ProblemKind::Unbalanced)),
        "{v:?}"
    );
    let e = h
        .run(s.batch().save(&batch, key(), false))
        .expect_err("unbalanced");
    assert!(matches!(e, ApiError::BatchRows(_)), "{e:?}");
    assert_eq!(balance(h, s, b.bank), Money::ZERO);
    assert_eq!(balance(h, s, b.expense), Money::ZERO);
}

/// `parse-csv` reads a BOM, a quoted line break, `(1,234.50)` and a
/// three-decimal amount exactly, as strings. On an engine without
/// `batch-exact-v1` its float amounts are refused.
pub fn parse_csv_exact(h: &Harness) {
    let s = &h.s1;
    let file = "\u{feff}Date,No,Payee,Account,Memo,Amount\n\
                10/01/2026,101,Feed Co,Supplies,\"line one\nline two\",\"(1,234.50)\"\n\
                2026-10-02,102,Seed Co,Supplies,plain,12.345\n";
    let mut mapping = ColumnMapping::default();
    for (i, f) in [
        CsvField::Date,
        CsvField::RefNo,
        CsvField::Payee,
        CsvField::Account,
        CsvField::Memo,
        CsvField::Amount,
    ]
    .into_iter()
    .enumerate()
    {
        mapping.0.insert(f, u16::try_from(i).expect("column"));
    }
    let got = h.run(
        s.batch()
            .parse_csv(BatchKind::Expense, file.as_bytes(), &mapping),
    );
    if !h.require_cap(Capabilities::BATCH_EXACT, "engine #163 batch-exact-v1") {
        // The pinned engine answers numbers: refused, never rounded.
        assert!(matches!(got, Err(ApiError::InexactMoney { .. })), "{got:?}");
        return;
    }
    let rows = got.expect("parsed").rows;
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].date.as_deref(), Some("2026-10-01"));
    assert_eq!(rows[0].ref_no.as_deref(), Some("101"));
    assert_eq!(rows[0].memo.as_deref(), Some("line one\nline two"));
    assert_eq!(rows[0].amount.as_deref(), Some("-1234.50"));
    assert_eq!(rows[1].amount.as_deref(), Some("12.345"), "not rounded");
    assert_eq!(rows[1].contact_name.as_deref(), Some("Seed Co"));
}

/// Account suggestions come from the engine alone (no AI, offline): the
/// name's default account, or nothing.
pub fn suggest_account_offline(h: &Harness) {
    let s = &h.s1;
    let fuel = account(h, s, AccountType::Expense, "Batch fuel").id;
    let mut fields = ContactFields::named(ContactType::Vendor, unique("Batch fuel stop"));
    fields.default_expense_account_id = Some(fuel);
    let with_default = h
        .run(s.contacts().create(
            &NewContact {
                fields,
                opening_balance: Money::ZERO,
                opening_balance_date: None,
            },
            key(),
        ))
        .expect("vendor")
        .record
        .id;
    let got = h
        .run(s.batch().suggest_account(with_default))
        .expect("suggest")
        .expect("a suggestion");
    assert_eq!((got.account, got.source), (fuel, SuggestSource::Default));
    let bare = contact(h, s, ContactType::Vendor, "Batch bare vendor");
    assert_eq!(h.run(s.batch().suggest_account(bare)).expect("none"), None);
}
