//! The demo twin of `/batch/*`: one test per case of the contract suite
//! (`crates/cl-api/tests/batch.rs`, P15A §6) plus the twin's own rules.
//! Synthetic data only.

use super::*;
use crate::company::demo::account_id;
use crate::company::demo::register::DEMO_CLOSING_PASSWORD;
use cl_api::BatchAmount;

fn d(y: i32, m: u32, day: u32) -> CalendarDate {
    CalendarDate::from_ymd(y, m, day).expect("date")
}

fn money(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}

fn amt(s: &str) -> BatchAmount {
    BatchAmount::new(money(s)).expect("whole cents")
}

fn bank() -> AccountId {
    account_id(1010)
}

fn card() -> AccountId {
    account_id(2050)
}

/// An open-period row on `account` (typed text) for `amount`.
fn row(n: u32, account: &str, amount: &str) -> BatchRowIn {
    let mut r = BatchRowIn::new(n, d(2026, 10, 15));
    r.account_name = Some(account.into());
    r.amount = Some(amt(amount));
    r
}

fn je_row(
    n: u32,
    entry: &str,
    account: &str,
    debit: Option<&str>,
    credit: Option<&str>,
) -> BatchRowIn {
    let mut r = BatchRowIn::new(n, d(2026, 10, 31));
    r.number = Some(entry.into());
    r.account_name = Some(account.into());
    r.debit = debit.map(amt);
    r.credit = credit.map(amt);
    r
}

fn group(kind: BatchKind, context: Option<AccountId>, rows: Vec<BatchRowIn>) -> BatchGroup {
    BatchGroup {
        kind,
        context,
        rows,
    }
}

fn balance(s: &mut DemoState, a: AccountId) -> Money {
    super::super::checks::balance(s, a).expect("a balance")
}

fn txn_count(s: &DemoState) -> usize {
    s.ledger.transactions().len()
}

fn rows_of(e: &ApiError) -> Vec<(u32, u32, BatchField, ProblemKind)> {
    match e {
        ApiError::BatchRows(v) => v
            .iter()
            .map(|p| (p.group, p.row_number, p.problem.field, p.problem.kind))
            .collect(),
        other => panic!("expected BatchRows, got {other:?}"),
    }
}

// ------------------------------------------------------------ contract

/// `fifty_rows_exact`: 25 checks and 25 deposits on 1010, amounts from
/// 0.01 to 999.99 with x.x5 values; the register moves by the batch sum
/// exactly.
#[test]
fn fifty_rows_exact() {
    let mut s = DemoState::new();
    let before = balance(&mut s, bank());
    let count = txn_count(&s);
    let cents = |i: i64| 1 + (i * 20_407) % 99_999;
    let mut checks = Vec::new();
    let mut deposits = Vec::new();
    for i in 0..25_i64 {
        let n = u32::try_from(i).expect("small");
        let c = Money::from_cents(cents(i)).expect("money");
        let mut r = row(n + 1, "Fuel", &c.to_wire());
        r.contact_name = Some("hollister fuel stop".into());
        checks.push(r);
        let dep = Money::from_cents(cents(i + 25)).expect("money");
        let mut r = row(n + 26, "Service Income", &dep.to_wire());
        r.contact_name = Some("Riverbend HOA".into());
        deposits.push(r);
    }
    // The edges and half-cent-looking values are in the batch.
    checks[0].amount = Some(amt("0.01"));
    checks[1].amount = Some(amt("999.99"));
    checks[2].amount = Some(amt("12.35"));
    deposits[0].amount = Some(amt("100.05"));
    let sum = |rows: &[BatchRowIn]| {
        Money::sum(rows.iter().filter_map(|r| r.amount.map(BatchAmount::money))).expect("sum")
    };
    let paid = sum(&checks);
    let received = sum(&deposits);
    let groups = vec![
        group(BatchKind::Check, Some(bank()), checks),
        group(BatchKind::Deposit, Some(bank()), deposits),
    ];
    let v = validate(&s, &groups).expect("validate");
    assert_eq!(v.rows.len(), 50);
    assert!(v.rows.iter().all(|r| r.ok && r.contact.is_some()));
    let saved = save(&mut s, &groups, &IdempotencyKey::new(), false).expect("saved");
    assert_eq!(saved.saved.len(), 50);
    assert!(saved.batch_id.is_some());
    assert_eq!(txn_count(&s), count + 50);
    let after = balance(&mut s, bank());
    assert_eq!(
        after.checked_sub(before).expect("delta"),
        received.checked_sub(paid).expect("net"),
        "the register moves by the batch sum exactly"
    );
    // Auto-numbered checks get unique, rising numbers.
    let chart = s.chart.clone().expect("chart");
    let mut numbers: Vec<u32> = s
        .ledger
        .check_numbers(&chart, bank())
        .into_iter()
        .filter(|(id, _, _)| saved.saved.iter().any(|t| t.id == *id))
        .map(|(_, n, _)| n)
        .collect();
    numbers.sort_unstable();
    numbers.dedup();
    assert_eq!(numbers.len(), 25);
    assert_eq!(numbers[24] - numbers[0], 24);
}

/// `mixed_one_call_all_in_register`: every kind in one call; each row's
/// transaction is in its register.
#[test]
fn mixed_one_call_all_in_register() {
    let mut s = DemoState::new();
    let card_before = balance(&mut s, card());
    let mut check = row(1, "6100", "84.15");
    check.contact_name = Some("Hollister Fuel Stop".into());
    check.number = Some("7001".into());
    let mut queued = row(2, "Fuel", "10.00");
    queued.contact_name = Some("Hollister Fuel Stop".into());
    queued.print_later = true;
    let mut deposit = row(3, "Service Income", "2300.00");
    deposit.contact_name = Some("riverbend hoa".into());
    let groups = vec![
        group(BatchKind::Check, Some(bank()), vec![check, queued]),
        group(BatchKind::Deposit, Some(bank()), vec![deposit]),
        group(
            BatchKind::Expense,
            Some(bank()),
            vec![row(4, "Bank and Merchant Fees", "3.50")],
        ),
        group(
            BatchKind::CardCharge,
            Some(card()),
            vec![row(5, "Fuel", "45.05")],
        ),
        group(
            BatchKind::CardCredit,
            Some(card()),
            vec![row(6, "Fuel", "5.00")],
        ),
        group(
            BatchKind::JournalEntry,
            None,
            vec![
                je_row(7, "ADJ-1", "Insurance", Some("600.00"), None),
                je_row(8, "ADJ-1", "1010", None, Some("600.00")),
            ],
        ),
    ];
    let saved = save(&mut s, &groups, &IdempotencyKey::new(), false).expect("saved");
    assert_eq!(saved.saved.len(), 8);
    // The two journal rows are one entry.
    assert_eq!(saved.saved[6].id, saved.saved[7].id);
    let ids: BTreeSet<TransactionId> = saved.saved.iter().map(|t| t.id).collect();
    assert_eq!(ids.len(), 7);
    let all = s.ledger.transactions();
    for t in &saved.saved {
        assert!(
            all.iter()
                .any(|x| super::super::register::DemoLedger::id_of(x) == t.id),
            "row {} is in the books",
            t.row_number
        );
    }
    // The hand-written check keeps its number; the queued one has none.
    let chart = s.chart.clone().expect("chart");
    let nums = s.ledger.check_numbers(&chart, bank());
    assert!(
        nums.iter()
            .any(|(id, n, _)| *id == saved.saved[0].id && *n == 7001)
    );
    assert!(!nums.iter().any(|(id, _, _)| *id == saved.saved[1].id));
    // The card: a charge of 45.05 and a credit of 5.00 (debits minus
    // credits on a liability).
    let card_after = balance(&mut s, card());
    assert_eq!(
        card_after,
        card_before.checked_sub(money("40.05")).expect("card")
    );
    assert_eq!(s.batch.posted.len(), 1);
    assert_eq!(s.batch.posted[0].1.len(), 7);
}

/// `one_bad_row_saves_nothing`: row 37 uses an inactive account; the
/// refusal names it and the books are unchanged.
#[test]
fn one_bad_row_saves_nothing() {
    let mut s = DemoState::new();
    let before = balance(&mut s, bank());
    let count = txn_count(&s);
    let mut rows: Vec<BatchRowIn> = (1..=40).map(|n| row(n, "Fuel", "1.00")).collect();
    rows[36].account_name = Some("Answering Service".into());
    let groups = vec![
        group(BatchKind::Expense, Some(bank()), rows[..20].to_vec()),
        group(BatchKind::Expense, Some(bank()), rows[20..].to_vec()),
    ];
    let v = validate(&s, &groups).expect("validate");
    let bad: Vec<&RowCheck> = v.rows.iter().filter(|r| !r.ok).collect();
    assert_eq!(bad.len(), 1);
    assert_eq!((bad[0].group, bad[0].row_number), (1, 37));
    assert_eq!(bad[0].problems[0].kind, ProblemKind::Inactive);
    let e = save(&mut s, &groups, &IdempotencyKey::new(), false).expect_err("refused");
    assert_eq!(
        rows_of(&e),
        vec![(1, 37, BatchField::Account, ProblemKind::Inactive)]
    );
    assert_eq!(balance(&mut s, bank()), before);
    assert_eq!(txn_count(&s), count);
    assert!(s.batch.replays.is_empty() && s.batch.posted.is_empty());
    assert_eq!(s.batch.saves, 1, "the request reached the demo engine");
}

/// A posting refused after earlier rows posted (an auto number taken by
/// a later hand-written row) rolls the whole batch back.
#[test]
fn a_late_posting_failure_rolls_everything_back() {
    let mut s = DemoState::new();
    let count = txn_count(&s);
    let next = super::super::checks::numbers(&mut s, bank())
        .1
        .map_or(1, |n| n + 1);
    let first = row(1, "Fuel", "10.00");
    let mut second = row(2, "Fuel", "20.00");
    second.number = Some(next.to_string());
    let groups = vec![group(BatchKind::Check, Some(bank()), vec![first, second])];
    assert!(validate(&s, &groups).expect("v").rows.iter().all(|r| r.ok));
    let e = save(&mut s, &groups, &IdempotencyKey::new(), false).expect_err("409");
    assert_eq!(
        rows_of(&e),
        vec![(0, 2, BatchField::Number, ProblemKind::Other)]
    );
    assert_eq!(txn_count(&s), count, "row 1 was rolled back");
}

/// `replay_same_key`: the same key and body give the first result and
/// post nothing again; another body is refused; a failed batch stores
/// nothing.
#[test]
fn replay_same_key() {
    let mut s = DemoState::new();
    let key = IdempotencyKey::new();
    let groups = vec![group(
        BatchKind::Expense,
        Some(bank()),
        vec![row(1, "Fuel", "9.99")],
    )];
    let first = save(&mut s, &groups, &key, false).expect("saved");
    let count = txn_count(&s);
    let again = save(&mut s, &groups, &key, false).expect("replayed");
    assert_eq!(again, first);
    assert_eq!(txn_count(&s), count, "no duplicate");
    let other = vec![group(
        BatchKind::Expense,
        Some(bank()),
        vec![row(1, "Fuel", "9.98")],
    )];
    let e = save(&mut s, &other, &key, false).expect_err("reused");
    assert!(cl_api::has_code(&e, IDEMPOTENCY_KEY_REUSED), "{e:?}");
    assert_eq!(txn_count(&s), count);
    // A refused batch keeps nothing under its key: fixed, it saves.
    let key2 = IdempotencyKey::new();
    let bad = vec![group(
        BatchKind::Expense,
        Some(bank()),
        vec![row(1, "Nope", "1.00")],
    )];
    assert!(save(&mut s, &bad, &key2, false).is_err());
    let fixed = vec![group(
        BatchKind::Expense,
        Some(bank()),
        vec![row(1, "Fuel", "1.00")],
    )];
    assert!(save(&mut s, &fixed, &key2, false).is_ok());
    assert_eq!(s.batch.posted.len(), 2);
}

/// The closing password is not part of the stored body: a replay with it
/// added still replays.
#[test]
fn replay_ignores_the_password() {
    let mut s = DemoState::new();
    let key = IdempotencyKey::new();
    let groups = vec![group(
        BatchKind::Expense,
        Some(bank()),
        vec![row(1, "Fuel", "2.00")],
    )];
    let first = save(&mut s, &groups, &key, false).expect("saved");
    let mut with = groups.clone();
    with[0].rows[0].closing_password = Some(Secret::new("anything"));
    assert_eq!(save(&mut s, &with, &key, false).expect("replay"), first);
}

/// `closed_period_row`: refused without the password, refused with a
/// wrong one, saved with the right one.
#[test]
fn closed_period_row() {
    let mut s = DemoState::new();
    let count = txn_count(&s);
    let mut locked = row(1, "Fuel", "15.00");
    locked.date = d(2026, 9, 15);
    let open = row(2, "Fuel", "16.00");
    let groups = |pw: Option<&str>| {
        let mut l = locked.clone();
        l.closing_password = pw.map(Secret::new);
        vec![group(
            BatchKind::Expense,
            Some(bank()),
            vec![l, open.clone()],
        )]
    };
    // Validate does not refuse a locked row: the password comes on save.
    assert!(
        validate(&s, &groups(None))
            .expect("v")
            .rows
            .iter()
            .all(|r| r.ok)
    );
    let e = save(&mut s, &groups(None), &IdempotencyKey::new(), false).expect_err("locked");
    assert!(matches!(e, ApiError::ClosedPeriod(ref c) if c.password_required == Some(true)));
    let e = save(
        &mut s,
        &groups(Some("wrong")),
        &IdempotencyKey::new(),
        false,
    )
    .expect_err("wrong");
    assert_eq!(e, ApiError::ClosingPasswordRejected);
    assert_eq!(txn_count(&s), count);
    save(
        &mut s,
        &groups(Some(DEMO_CLOSING_PASSWORD)),
        &IdempotencyKey::new(),
        false,
    )
    .expect("saved");
    assert_eq!(txn_count(&s), count + 2);
}

/// A locked check, deposit and journal entry post with the password too.
#[test]
fn every_kind_takes_the_closing_password() {
    let mut s = DemoState::new();
    let pw = || Some(Secret::new(DEMO_CLOSING_PASSWORD));
    let date = d(2026, 9, 1);
    let mut c = row(1, "Fuel", "1.00");
    let mut dep = row(2, "Service Income", "2.00");
    let mut j1 = je_row(3, "OLD", "Fuel", Some("3.00"), None);
    let mut j2 = je_row(4, "OLD", "1010", None, Some("3.00"));
    for r in [&mut c, &mut dep, &mut j1, &mut j2] {
        r.date = date;
        r.closing_password = pw();
    }
    let groups = vec![
        group(BatchKind::Check, Some(bank()), vec![c]),
        group(BatchKind::Deposit, Some(bank()), vec![dep]),
        group(BatchKind::JournalEntry, None, vec![j1, j2]),
    ];
    let saved = save(&mut s, &groups, &IdempotencyKey::new(), false).expect("saved");
    assert_eq!(saved.saved.len(), 4);
}

/// `je_group_unbalanced_refused`: an entry off by a cent is refused (every
/// row of it named); a balanced one saves as one transaction.
#[test]
fn je_group_unbalanced_refused() {
    let mut s = DemoState::new();
    let count = txn_count(&s);
    let groups = vec![group(
        BatchKind::JournalEntry,
        None,
        vec![
            je_row(1, "ADJ-1", "Insurance", Some("600.00"), None),
            je_row(2, "ADJ-1", "1010", None, Some("600.00")),
            je_row(3, "ADJ-2", "Fuel", Some("25.10"), None),
            je_row(4, "ADJ-2", "Repairs and Maintenance", None, Some("25.01")),
        ],
    )];
    let v = validate(&s, &groups).expect("v");
    let ok: Vec<bool> = v.rows.iter().map(|r| r.ok).collect();
    assert_eq!(ok, vec![true, true, false, false]);
    assert_eq!(v.rows[2].problems[0].kind, ProblemKind::Unbalanced);
    let e = save(&mut s, &groups, &IdempotencyKey::new(), false).expect_err("unbalanced");
    assert_eq!(
        rows_of(&e),
        vec![
            (0, 3, BatchField::Row, ProblemKind::Unbalanced),
            (0, 4, BatchField::Row, ProblemKind::Unbalanced),
        ]
    );
    assert_eq!(txn_count(&s), count);
    let mut fixed = groups;
    fixed[0].rows[3].credit = Some(amt("25.10"));
    let saved = save(&mut s, &fixed, &IdempotencyKey::new(), false).expect("saved");
    assert_eq!(saved.saved[0].id, saved.saved[1].id);
    assert_eq!(saved.saved[2].id, saved.saved[3].id);
    assert_ne!(saved.saved[0].id, saved.saved[2].id);
    assert_eq!(txn_count(&s), count + 2);
}

/// The journal entry rules beyond the balance: two lines at least, one
/// side per line, a name on every A/R-A/P line (any number of them,
/// Q-206).
#[test]
fn journal_entry_rules() {
    let s = DemoState::new();
    let lone = je_row(1, "", "Fuel", Some("1.00"), None);
    let mut both = je_row(2, "E", "Fuel", Some("1.00"), None);
    both.credit = Some(amt("1.00"));
    let other = je_row(3, "E", "1010", None, Some("1.00"));
    let ar = je_row(4, "AR", "Accounts Receivable", Some("5.00"), None);
    let mut ap = je_row(5, "AR", "Accounts Payable", None, Some("5.00"));
    ap.contact_name = Some("Galena Gravel".into());
    // Two named A/R lines in one entry pass.
    let mut ar1 = je_row(6, "AR2", "1200", Some("3.00"), None);
    ar1.contact_name = Some("Riverbend HOA".into());
    let mut ar2 = je_row(7, "AR2", "1200", Some("2.00"), None);
    ar2.contact_name = Some("Riverbend HOA".into());
    let income = je_row(8, "AR2", "Service Income", None, Some("5.00"));
    let groups = vec![group(
        BatchKind::JournalEntry,
        None,
        vec![lone, both, other, ar, ap, ar1, ar2, income],
    )];
    let v = validate(&s, &groups).expect("v");
    let kinds = |i: usize| -> Vec<(BatchField, ProblemKind)> {
        v.rows[i]
            .problems
            .iter()
            .map(|p| (p.field, p.kind))
            .collect()
    };
    assert_eq!(kinds(0), vec![(BatchField::Number, ProblemKind::Invalid)]);
    assert!(kinds(1).contains(&(BatchField::Debit, ProblemKind::Required)));
    assert_eq!(kinds(3), vec![(BatchField::Contact, ProblemKind::Required)]);
    assert_eq!(kinds(4), vec![], "a named A/P line beside an A/R line");
    assert!(v.rows[5..8].iter().all(|r| r.ok), "{:?}", &v.rows[5..8]);
}

/// `suggest_account_offline`: the vendor's default, then its most recent
/// account; an unknown name has none.
#[test]
fn suggest_account_offline() {
    let mut s = DemoState::new();
    let fuel_stop = s
        .contacts_mut()
        .iter()
        .find(|c| c.fields.display_name == "Hollister Fuel Stop")
        .map(|c| c.id)
        .expect("vendor");
    let got = suggest_account(&s, fuel_stop).expect("ok").expect("one");
    assert_eq!(got.account, account_id(6100));
    assert_eq!(got.source, cl_api::SuggestSource::Default);
    let nobody = super::super::contacts::contact_id(99_999);
    assert_eq!(suggest_account(&s, nobody), Ok(None));
    // The three story vendors carry a default account.
    let defaults = s
        .contacts_mut()
        .iter()
        .filter(|c| {
            [
                "Hollister Fuel Stop",
                "Crane Creek Tree Service",
                "Galena Gravel",
            ]
            .contains(&c.fields.display_name.as_str())
                && c.fields.default_expense_account_id.is_some()
        })
        .count();
    assert_eq!(defaults, 3);
}

// ------------------------------------------------------------ the twin

#[test]
fn names_and_accounts_resolve_like_the_engine() {
    let s = DemoState::new();
    let mut by_number = row(1, "6100", "1.00");
    by_number.contact_name = Some("  GALENA gravel ".into());
    let by_case = row(2, "fuel", "1.00");
    let missing = row(3, "No Such Account", "1.00");
    let mut unknown_name = row(4, "Fuel", "1.00");
    unknown_name.contact_name = Some("Brand New Vendor".into());
    let mut no_account = row(5, "x", "1.00");
    no_account.account_name = None;
    let same = row(6, "1010", "1.00");
    let groups = vec![group(
        BatchKind::Expense,
        Some(bank()),
        vec![by_number, by_case, missing, unknown_name, no_account, same],
    )];
    let v = validate(&s, &groups).expect("v");
    assert_eq!(v.rows[0].account, Some(account_id(6100)));
    assert!(v.rows[0].contact.is_some() && v.rows[0].ok);
    assert_eq!(v.rows[1].account, Some(account_id(6100)));
    assert_eq!(v.rows[2].problems[0].kind, ProblemKind::NotInLists);
    assert!(v.rows[3].ok && v.rows[3].new_contact && v.rows[3].contact.is_none());
    assert_eq!(v.rows[4].problems[0].kind, ProblemKind::Required);
    assert_eq!(v.rows[5].problems[0].kind, ProblemKind::Invalid);
}

#[test]
fn a_name_prefers_the_rows_contact_type() {
    let mut s = DemoState::new();
    // "Blue Eye Feed Co" is a customer and a vendor; make a vendor-only
    // twin of a customer's name and check which one each kind picks.
    let customer = s
        .contacts_mut()
        .iter()
        .find(|c| c.fields.display_name == "Riverbend HOA")
        .map(|c| c.id)
        .expect("customer");
    let twin = super::super::contacts::contact_id(88_888);
    s.contacts_mut().push(Contact {
        id: twin,
        fields: ContactFields::named(ContactType::Vendor, "Riverbend HOA"),
        is_active: true,
        opening_balance: None,
        opening_balance_date: None,
        created_at: None,
        updated_at: None,
    });
    let mut r = row(1, "Fuel", "1.00");
    r.contact_name = Some("Riverbend HOA".into());
    let mut dep = row(2, "Service Income", "1.00");
    dep.contact_name = Some("Riverbend HOA".into());
    let groups = vec![
        group(BatchKind::Expense, Some(bank()), vec![r]),
        group(BatchKind::Deposit, Some(bank()), vec![dep]),
    ];
    let v = validate(&s, &groups).expect("v");
    assert_eq!(v.rows[0].contact, Some(twin));
    assert_eq!(v.rows[1].contact, Some(customer));
}

#[test]
fn unknown_names_need_add_new_names() {
    let mut s = DemoState::new();
    let names = s.contacts_mut().len();
    let mut a = row(1, "Fuel", "1.00");
    a.contact_name = Some("Brand New Vendor".into());
    let mut b = row(2, "Fuel", "2.00");
    b.contact_name = Some("brand new vendor".into());
    let groups = vec![group(BatchKind::Check, Some(bank()), vec![a, b])];
    let e = save(&mut s, &groups, &IdempotencyKey::new(), false).expect_err("unknown");
    assert_eq!(
        rows_of(&e),
        vec![
            (0, 1, BatchField::Contact, ProblemKind::NotInLists),
            (0, 2, BatchField::Contact, ProblemKind::NotInLists),
        ]
    );
    assert_eq!(s.contacts_mut().len(), names);
    let saved = save(&mut s, &groups, &IdempotencyKey::new(), true).expect("added");
    assert_eq!(saved.created_contacts, 1, "one name, any case");
    let added = s
        .contacts_mut()
        .iter()
        .find(|c| c.fields.display_name == "Brand New Vendor")
        .cloned()
        .expect("added");
    assert_eq!(added.fields.contact_type, ContactType::Vendor);
    assert_eq!(s.contacts_mut().len(), names + 1);
}

#[test]
fn check_numbers_and_print_later() {
    let mut s = DemoState::new();
    let used = super::super::checks::numbers(&mut s, bank())
        .1
        .expect("the sample has checks");
    let mut taken = row(1, "Fuel", "1.00");
    taken.number = Some(used.to_string());
    let mut a = row(2, "Fuel", "1.00");
    a.number = Some("8001".into());
    let mut b = row(3, "Fuel", "1.00");
    b.number = Some("8001".into());
    let mut bad = row(4, "Fuel", "1.00");
    bad.number = Some("12a".into());
    let mut queued_no_payee = row(5, "Fuel", "1.00");
    queued_no_payee.print_later = true;
    let mut queued_numbered = row(6, "Fuel", "1.00");
    queued_numbered.print_later = true;
    queued_numbered.number = Some("8100".into());
    queued_numbered.contact_name = Some("Galena Gravel".into());
    let mut expense_queued = row(7, "Fuel", "1.00");
    expense_queued.print_later = true;
    expense_queued.contact_name = Some("Galena Gravel".into());
    let groups = vec![
        group(
            BatchKind::Check,
            Some(bank()),
            vec![taken, a, b, bad, queued_no_payee, queued_numbered],
        ),
        group(BatchKind::Expense, Some(bank()), vec![expense_queued]),
    ];
    let v = validate(&s, &groups).expect("v");
    let first = |i: usize| (v.rows[i].problems[0].field, v.rows[i].problems[0].kind);
    assert_eq!(first(0), (BatchField::Number, ProblemKind::Other));
    assert_eq!(first(1), (BatchField::Number, ProblemKind::Other));
    assert_eq!(first(2), (BatchField::Number, ProblemKind::Other));
    assert_eq!(first(3), (BatchField::Number, ProblemKind::Invalid));
    assert_eq!(first(4), (BatchField::Contact, ProblemKind::Required));
    assert_eq!(first(5), (BatchField::Number, ProblemKind::Invalid));
    assert_eq!(first(6), (BatchField::Row, ProblemKind::Invalid));
}

#[test]
fn amounts_and_sides_follow_the_kind() {
    let s = DemoState::new();
    let mut no_amount = row(1, "Fuel", "1.00");
    no_amount.amount = None;
    let mut debit = row(2, "Fuel", "1.00");
    debit.debit = Some(amt("1.00"));
    let mut long_memo = row(3, "Fuel", "1.00");
    long_memo.memo = Some("m".repeat(2001));
    let mut je_amount = je_row(4, "E", "Fuel", Some("1.00"), None);
    je_amount.amount = Some(amt("1.00"));
    let je_other = je_row(5, "E", "1010", None, Some("1.00"));
    let groups = vec![
        group(
            BatchKind::Expense,
            Some(bank()),
            vec![no_amount, debit, long_memo],
        ),
        group(BatchKind::JournalEntry, None, vec![je_amount, je_other]),
    ];
    let v = validate(&s, &groups).expect("v");
    let first = |i: usize| (v.rows[i].problems[0].field, v.rows[i].problems[0].kind);
    assert_eq!(first(0), (BatchField::Amount, ProblemKind::Required));
    assert_eq!(first(1), (BatchField::Debit, ProblemKind::Invalid));
    assert_eq!(first(2), (BatchField::Memo, ProblemKind::Invalid));
    assert_eq!(first(3), (BatchField::Amount, ProblemKind::Invalid));
    assert!(v.rows[4].ok);
}

#[test]
fn tags_must_be_live() {
    let mut s = DemoState::new();
    let tag = s.tags_mut().first().map(|t| t.id).expect("a tag");
    let mut live = row(1, "Fuel", "1.00");
    live.tag = Some(tag);
    let mut missing = row(2, "Fuel", "1.00");
    missing.tag = Some(super::super::lists::tag_id(99_999));
    let groups = vec![group(
        BatchKind::Expense,
        Some(bank()),
        vec![live.clone(), missing],
    )];
    let v = validate(&s, &groups).expect("v");
    assert!(v.rows[0].ok);
    assert_eq!(v.rows[1].problems[0].kind, ProblemKind::NotInLists);
    s.tags_gone.insert(tag);
    let v = validate(&s, &[group(BatchKind::Expense, Some(bank()), vec![live])]).expect("v");
    assert_eq!(v.rows[0].problems[0].kind, ProblemKind::Inactive);
}

#[test]
fn only_journal_entry_rows_may_use_receivable_or_payable() {
    let mut s = DemoState::new();
    let count = txn_count(&s);
    let groups = vec![
        group(
            BatchKind::Check,
            Some(bank()),
            vec![row(1, "1200", "5.00"), row(2, "Fuel", "5.00")],
        ),
        group(
            BatchKind::CardCharge,
            Some(card()),
            vec![row(3, "Accounts Payable", "5.00")],
        ),
    ];
    let v = validate(&s, &groups).expect("v");
    assert_eq!(
        (v.rows[0].problems[0].field, v.rows[0].problems[0].kind),
        (BatchField::Account, ProblemKind::Invalid)
    );
    assert!(v.rows[1].ok);
    assert!(!v.rows[2].ok);
    let e = save(&mut s, &groups, &IdempotencyKey::new(), false).expect_err("refused");
    let rows = rows_of(&e);
    assert!(rows.contains(&(0, 1, BatchField::Account, ProblemKind::Invalid)));
    assert!(rows.contains(&(1, 3, BatchField::Account, ProblemKind::Invalid)));
    assert_eq!(txn_count(&s), count, "nothing posted");
    // A journal entry line may (with a name).
    let mut ar = je_row(4, "J9", "1200", Some("5.00"), None);
    ar.contact_name = Some("Riverbend HOA".into());
    let je = vec![group(
        BatchKind::JournalEntry,
        None,
        vec![ar, je_row(5, "J9", "Service Income", None, Some("5.00"))],
    )];
    let v = validate(&s, &je).expect("v");
    assert!(v.rows.iter().all(|r| r.ok), "{:?}", v.rows);
}

#[test]
fn a_journal_entry_amount_is_never_negative() {
    // `BatchAmount` refuses zero and negative amounts before anything is
    // sent, so the twin's own rule can't be reached from this side.
    assert_eq!(
        BatchAmount::new(money("-5.00")),
        Err(LocalValidation::NonPositiveAmount)
    );
    assert_eq!(
        BatchAmount::new(Money::ZERO),
        Err(LocalValidation::NonPositiveAmount)
    );
}

#[test]
fn the_request_as_a_whole() {
    let s = DemoState::new();
    let gate = Err(invalid());
    let one = || vec![row(1, "Fuel", "1.00")];
    assert_eq!(validate(&s, &[]), gate);
    // A journal section has no context account; others need the right one.
    assert_eq!(
        validate(&s, &[group(BatchKind::JournalEntry, Some(bank()), one())]),
        gate
    );
    // A missing or wrong section account names its group
    // (`BATCH_CONTEXT_ACCOUNT_*`, `groupIndex`).
    let refused = |g: u32| {
        Err(ApiError::BatchGroup {
            group: g,
            problem: cl_api::BatchGroupProblem::ContextAccount,
        })
    };
    assert_eq!(
        validate(&s, &[group(BatchKind::Expense, None, one())]),
        refused(0)
    );
    assert_eq!(
        validate(&s, &[group(BatchKind::Check, Some(card()), one())]),
        refused(0)
    );
    assert_eq!(
        validate(&s, &[group(BatchKind::CardCharge, Some(bank()), one())]),
        refused(0)
    );
    assert_eq!(
        validate(
            &s,
            &[group(BatchKind::Deposit, Some(account_id(6100)), one())]
        ),
        refused(0)
    );
    // An asset that is not a bank account (A/R, the clearing account).
    for asset in [1200, 10150] {
        assert_eq!(
            validate(
                &s,
                &[group(BatchKind::Check, Some(account_id(asset)), one())]
            ),
            refused(0),
            "{asset}"
        );
    }
    // A/P is a liability but not a card.
    assert_eq!(
        validate(
            &s,
            &[group(BatchKind::CardCredit, Some(account_id(2000)), one())]
        ),
        refused(0)
    );
    // The first refused group is named.
    let mut second = one();
    second[0].row_number = 2;
    assert_eq!(
        validate(
            &s,
            &[
                group(BatchKind::Check, Some(bank()), one()),
                group(BatchKind::Deposit, Some(card()), second.clone()),
            ]
        ),
        refused(1)
    );
    // Expenses take a bank account or a card.
    for ctx in [bank(), card()] {
        assert!(validate(&s, &[group(BatchKind::Expense, Some(ctx), one())]).is_ok());
    }
    // Row numbers are unique across groups.
    assert_eq!(
        validate(
            &s,
            &[
                group(BatchKind::Expense, Some(bank()), one()),
                group(BatchKind::Expense, Some(card()), one()),
            ]
        ),
        gate
    );
    assert_eq!(
        validate(&s, &[group(BatchKind::Expense, Some(bank()), vec![])]),
        gate
    );
    let many: Vec<BatchGroup> = (1..=51)
        .map(|n| {
            group(
                BatchKind::Expense,
                Some(bank()),
                vec![row(n, "Fuel", "1.00")],
            )
        })
        .collect();
    assert_eq!(validate(&s, &many), gate);
    // An expense may come from a card.
    assert!(validate(&s, &[group(BatchKind::Expense, Some(card()), one())]).is_ok());
}

// ------------------------------------------------------------------- CSV

fn fixture(name: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/batch")
        .join(name);
    std::fs::read(path).expect("fixture")
}

fn mapping(pairs: &[(CsvField, u16)]) -> ColumnMapping {
    ColumnMapping(pairs.iter().copied().collect())
}

/// `parse_csv_exact`: a byte-order mark, a quoted line break, doubled
/// quotes, `(1,234.50)`, `.5`, leading zeros and a blank line.
#[test]
fn parse_csv_exact() {
    let bytes = fixture("bom-quoted.csv");
    assert_eq!(
        &bytes[..3],
        &[0xEF, 0xBB, 0xBF],
        "the fixture keeps its BOM"
    );
    let m = mapping(&[
        (CsvField::Date, 0),
        (CsvField::Payee, 1),
        (CsvField::Account, 2),
        (CsvField::Memo, 3),
        (CsvField::Amount, 4),
    ]);
    let p = parse_csv(BatchKind::Expense, &bytes, &m).expect("parsed");
    assert_eq!(p.rows.len(), 3, "the blank record is skipped");
    let r = &p.rows[0];
    assert_eq!(r.row_number, 1);
    assert_eq!(r.date.as_deref(), Some("2026-10-14"));
    assert_eq!(
        r.memo.as_deref(),
        Some("Gravel, two loads\nsecond line of the memo")
    );
    assert_eq!(r.amount.as_deref(), Some("-1234.50"));
    assert_eq!(
        p.rows[1].contact_name.as_deref(),
        Some("Crane Creek \"Tree\" Service")
    );
    assert_eq!(p.rows[1].amount.as_deref(), Some("0.5"));
    assert_eq!(p.rows[2].memo, None);
    assert_eq!(p.rows[2].amount.as_deref(), Some("7.10"));
    assert_eq!(p.rows[2].row_number, 3);
}

#[test]
fn three_decimals_come_back_unrounded() {
    let p = parse_csv(
        BatchKind::Expense,
        &fixture("three-decimals.csv"),
        &ColumnMapping::default(),
    )
    .expect("parsed");
    let amounts: Vec<&str> = p.rows.iter().filter_map(|r| r.amount.as_deref()).collect();
    assert_eq!(amounts, vec!["12.345", "2.675", "0.005", "1.0049"]);
    assert_eq!(
        p.rows[0].contact_name.as_deref(),
        Some("Hollister Fuel Stop")
    );
}

#[test]
fn tabs_and_aliases() {
    let p = parse_csv(
        BatchKind::CardCharge,
        &fixture("card.tsv"),
        &ColumnMapping::default(),
    )
    .expect("parsed");
    assert_eq!(p.rows.len(), 3);
    let r = &p.rows[0];
    assert_eq!(r.date.as_deref(), Some("2026-10-08"));
    assert_eq!(r.contact_name.as_deref(), Some("Hollister Fuel Stop"));
    assert_eq!(r.account_name.as_deref(), Some("Fuel"));
    assert_eq!(r.memo.as_deref(), Some("Fuel for the mower trailer"));
    assert_eq!(r.amount.as_deref(), Some("45.05"));
    assert_eq!(p.rows[2].amount.as_deref(), Some("-18.25"));
}

#[test]
fn bank_and_journal_samples() {
    let p = parse_csv(
        BatchKind::Check,
        &fixture("mixed-bank.csv"),
        &ColumnMapping::default(),
    )
    .expect("parsed");
    let amounts: Vec<&str> = p.rows.iter().filter_map(|r| r.amount.as_deref()).collect();
    assert_eq!(amounts, vec!["84.15", "1234.50", "450.00", "2300.00"]);
    assert_eq!(p.rows[0].ref_no, None);
    let j = parse_csv(
        BatchKind::JournalEntry,
        &fixture("je.csv"),
        &ColumnMapping::default(),
    )
    .expect("parsed");
    assert_eq!(j.rows.len(), 4);
    assert_eq!(j.rows[0].ref_no.as_deref(), Some("ADJ-1"));
    assert_eq!(j.rows[0].debit.as_deref(), Some("600.00"));
    assert_eq!(j.rows[0].credit, None);
    assert_eq!(j.rows[3].credit.as_deref(), Some("25.01"));
}

#[test]
fn an_explicit_mapping_wins_and_out_of_range_columns_are_empty() {
    let csv = b"a,b,c\n1/2/2026,Fuel,3.00\n";
    let m = mapping(&[
        (CsvField::Date, 0),
        (CsvField::Account, 1),
        (CsvField::Amount, 2),
        (CsvField::Memo, 9),
    ]);
    let p = parse_csv(BatchKind::Expense, csv, &m).expect("parsed");
    assert_eq!(p.rows[0].date.as_deref(), Some("2026-01-02"));
    assert_eq!(p.rows[0].account_name.as_deref(), Some("Fuel"));
    assert_eq!(p.rows[0].amount.as_deref(), Some("3.00"));
    assert_eq!(p.rows[0].memo, None);
}

#[test]
fn parse_csv_refusals() {
    let none = ColumnMapping::default();
    let too_big = ApiError::Local(LocalValidation::AttachmentTooLarge);
    assert_eq!(
        parse_csv(BatchKind::Expense, b"", &none),
        Err(too_big.clone())
    );
    let huge = vec![b'a'; PARSE_CSV_MAX_BYTES + 1];
    assert_eq!(parse_csv(BatchKind::Expense, &huge, &none), Err(too_big));
    assert_eq!(
        parse_csv(BatchKind::Expense, b"Date,Amount\n", &none),
        Err(invalid()),
        "CSV_EMPTY"
    );
    assert_eq!(
        parse_csv(BatchKind::Expense, b"Date,Memo\n1/1/2026,\"open", &none),
        Err(invalid()),
        "CSV_MALFORMED"
    );
    let mut many = String::from("Date,Amount\n");
    for _ in 0..=ENGINE_MAX_ROWS {
        many.push_str("1/1/2026,1\n");
    }
    assert_eq!(
        parse_csv(BatchKind::Expense, many.as_bytes(), &none),
        Err(ApiError::Local(LocalValidation::TooManyLines)),
        "CSV_TOO_MANY_ROWS"
    );
}

#[test]
fn amounts_normalize_exactly() {
    for (raw, want) in [
        ("$1,234.50", "1234.50"),
        ("(12.50)", "-12.50"),
        ("12.50-", "-12.50"),
        ("-3", "-3"),
        (".5", "0.5"),
        ("0007.10", "7.10"),
        ("000", "0"),
        ("12.345", "12.345"),
        ("€ 1 000,00", "100000"),
        ("£5", "5"),
        ("12.", "12"),
        ("abc", "abc"),
        ("1.2.3", "1.2.3"),
        ("(-5)", "(-5)"),
        ("-", "-"),
    ] {
        assert_eq!(normalize_amount(raw), want, "{raw}");
    }
}

#[test]
fn dates_normalize_like_the_engine() {
    assert_eq!(normalize_date("2026-10-05"), "2026-10-05");
    assert_eq!(normalize_date("10/5/2026"), "2026-10-05");
    assert_eq!(normalize_date("1-2-2026"), "2026-01-02");
    assert_eq!(normalize_date("2026/1/2"), "2026-01-02");
    assert_eq!(normalize_date("Oct 5"), "Oct 5");
    assert_eq!(normalize_date("10/5/26"), "10/5/26");
}

#[test]
fn auto_mapping_uses_the_aliases() {
    let headers: Vec<String> = [
        "Posted Date",
        "Check #",
        "Paid To",
        "GL Account",
        "Notes",
        "Total",
        "DR",
        "CR",
    ]
    .iter()
    .map(|s| (*s).to_string())
    .collect();
    let m = auto_mapping(&headers);
    assert_eq!(m.0.len(), 8);
    assert_eq!(m.0.get(&CsvField::Credit), Some(&7));
    assert!(auto_mapping(&["x".to_string()]).0.is_empty());
}
