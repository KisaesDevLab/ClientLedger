//! The register balance property (P12 WP3, plan §6.2, AC1): after a long
//! run of random creates, edits, voids and deletes of bank postings, the
//! register's balance on every line, its ending balance and the engine's
//! account balance all agree with an exact `Money` oracle kept here, in
//! test code only; and a filtered or re-sorted register shows the same
//! balance on every line as the unfiltered one.
//!
//! # What runs where
//!
//! - The random operations and the account-balance checks (oracle ==
//!   `GET /accounts/{id}` balance, every 100 operations and at the end) run
//!   on any engine.
//! - The register checks need exact register money and filter-independent
//!   balances (`register-exact-v1`, R1/R2) and are PENDING without it.
//! - Deletes (2 % of operations) need `txn-delete-v1` (R7); without it the
//!   roll is drawn again.
//!
//! # Knobs
//!
//! - `CL_PROP_OPS` — number of operations. Default: 1,000 (AC1) when the
//!   engine advertises `register-exact-v1`, so the full register checks
//!   always run at AC1 size; 150 otherwise, so the normal contract job on
//!   an engine without it stays short.
//! - `CL_PROP_SEED` — the RNG seed (default fixed). The seed is printed at
//!   the start and on every failure, so a failing run can be replayed.
//!
//! # Running it against the engine branch with the capabilities
//!
//! The pinned engine does not advertise `register-exact-v1` or
//! `txn-delete-v1` yet (engine PR #153). To run the full property against
//! an engine built from branch `clientledger/register-exact`, point the
//! harness at that executable with `CL_ENGINE_EXE` (the same override
//! `cl-host` uses for the app) and require the capabilities so that a
//! missing one fails instead of going PENDING:
//!
//! ```text
//! cargo xtask pg-fetch
//! # build the engine SEA from the branch checkout, then:
//! CL_ENGINE_EXE=/path/to/clientledger-engine.exe \
//! CL_API_REQUIRE_CAPS=register-exact-v1,txn-delete-v1 \
//! CL_PROP_OPS=1000 CL_API_TEST_BUDGET_SECS=60 \
//!   cargo test -p cl-api --test contract -- register_property
//! ```
//!
//! The RNG is a `SplitMix64` written here (no new dependency); every
//! amount is built with `Money::from_cents` and every comparison is
//! `Money ==`. Nothing here is float.

use std::collections::{BTreeMap, BTreeSet};

use cl_api::{
    AccountType, BankPosting, Capabilities, CardChargeDraft, CheckDraft, ClearedFilter,
    DeleteOptions, DepositDraft, PostingOptions, RecordVersion, RegisterAmount, RegisterLine,
    RegisterQuery, RegisterSort, SortDir, SplitLine, TransferDraft, TxnType,
};
use cl_core::ids::{AccountId, TransactionId};
use cl_core::{CalendarDate, Money};

use crate::common::{account, date, key};
use crate::contract_bank_postings::detail_account;
use crate::support::{Harness, unique};

/// Operations when the register checks run (AC1).
const DEFAULT_OPS_EXACT: usize = 1_000;
/// Operations when only the account balances can be checked.
const DEFAULT_OPS: usize = 150;
const DEFAULT_SEED: u64 = 0x00C1_1ED6_E512_0012;
const PAYEES: [&str; 5] = [
    "Payee Alder",
    "Payee Birch",
    "Payee Cedar",
    "Payee Dogwood",
    "Payee Elm",
];

/// `SplitMix64`: tiny, seedable, the same sequence on every platform.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `0..n` (n > 0; the modulo bias is irrelevant here).
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        let i = usize::try_from(self.below(xs.len() as u64)).expect("index");
        &xs[i]
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }
}

fn env_or<T: std::str::FromStr>(name: &str, default: T) -> T {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(default)
}

/// The accounts of the run. A is the register under test; B and C are the
/// other sides of transfers, and C is the card (credit-normal).
struct Books {
    a: AccountId,
    b: AccountId,
    c: AccountId,
    expenses: [AccountId; 2],
    income: AccountId,
}

impl Books {
    /// `debit_minus_credit` in the register's sign for `acct`: the engine's
    /// register reports a credit-normal account (the card C) as
    /// credit − debit, the amount owed positive (Q-P12h, Q-241), and a
    /// debit-normal one as debit − credit. `accounts.balance` is
    /// debit − credit for both.
    fn register_sign(&self, acct: AccountId, debit_minus_credit: Money) -> Money {
        if acct == self.c {
            debit_minus_credit.checked_neg().expect("neg")
        } else {
            debit_minus_credit
        }
    }
}

/// What a posting is, enough to rebuild it and to know its effect.
#[derive(Clone)]
enum Shape {
    Check {
        payee: usize,
        lines: Vec<(AccountId, Money)>,
    },
    Deposit {
        to: AccountId,
        lines: Vec<(AccountId, Money)>,
    },
    Transfer {
        from: AccountId,
        to: AccountId,
        amount: Money,
    },
    Card {
        lines: Vec<(AccountId, Money)>,
    },
}

#[derive(Clone)]
struct Spec {
    date: CalendarDate,
    memo: String,
    shape: Shape,
}

fn total(lines: &[(AccountId, Money)]) -> Money {
    Money::sum(lines.iter().map(|(_, m)| *m)).expect("line total")
}

fn split(account: AccountId, amount: Money) -> SplitLine {
    SplitLine {
        account,
        amount,
        memo: None,
        contact: None,
        tag: None,
    }
}

impl Spec {
    fn posting(&self, b: &Books) -> BankPosting {
        let memo = Some(self.memo.clone());
        let splits = |lines: &[(AccountId, Money)]| -> Vec<SplitLine> {
            lines.iter().map(|(a, m)| split(*a, *m)).collect()
        };
        match &self.shape {
            Shape::Check { payee, lines } => BankPosting::check(CheckDraft {
                bank: b.a,
                date: self.date,
                payee: None,
                payee_name: PAYEES[*payee].into(),
                check_number: None,
                memo,
                amount: total(lines),
                lines: splits(lines),
            })
            .expect("check"),
            Shape::Deposit { to, lines } => BankPosting::deposit(DepositDraft {
                account: *to,
                date: self.date,
                memo,
                lines: splits(lines),
            })
            .expect("deposit"),
            Shape::Transfer { from, to, amount } => BankPosting::transfer(TransferDraft {
                from: *from,
                to: *to,
                date: self.date,
                amount: *amount,
                memo,
            })
            .expect("transfer"),
            Shape::Card { lines } => BankPosting::card_charge(CardChargeDraft {
                pay_from: b.c,
                date: self.date,
                number: None,
                payee: None,
                memo,
                lines: splits(lines),
            })
            .expect("card charge"),
        }
    }

    /// The effect on A, B and C as debit − credit (the register's sign).
    fn effects(&self, b: &Books) -> Vec<(AccountId, Money)> {
        let neg = |m: Money| m.checked_neg().expect("neg");
        match &self.shape {
            Shape::Check { lines, .. } => vec![(b.a, neg(total(lines)))],
            Shape::Deposit { to, lines } => vec![(*to, total(lines))],
            Shape::Transfer { from, to, amount } => vec![(*from, neg(*amount)), (*to, *amount)],
            Shape::Card { lines } => vec![(b.c, neg(total(lines)))],
        }
    }

    fn type_(&self) -> TxnType {
        match self.shape {
            Shape::Deposit { .. } => TxnType::Deposit,
            Shape::Transfer { .. } => TxnType::Transfer,
            Shape::Check { .. } | Shape::Card { .. } => TxnType::Expense,
        }
    }
}

struct Entry {
    spec: Spec,
    version: RecordVersion,
    void: bool,
}

/// The oracle: every posting the run made that still exists, by id.
#[derive(Default)]
struct Oracle {
    entries: BTreeMap<TransactionId, Entry>,
    /// Ids in creation order: random picks must not depend on the
    /// engine's (random) ids, or a seed would not replay.
    order: Vec<TransactionId>,
}

impl Oracle {
    /// The expected balance of `acct` (debit − credit), voids excluded.
    fn balance(&self, b: &Books, acct: AccountId) -> Money {
        Money::sum(
            self.entries
                .values()
                .filter(|e| !e.void)
                .flat_map(|e| e.spec.effects(b))
                .filter(|(a, _)| *a == acct)
                .map(|(_, m)| m),
        )
        .expect("oracle sum")
    }

    /// Postings that touch `acct`, voids included (the register lists them).
    fn rows(&self, b: &Books, acct: AccountId) -> usize {
        self.entries
            .values()
            .filter(|e| e.spec.effects(b).iter().any(|(a, _)| *a == acct))
            .count()
    }

    /// Ids of postings that can still be edited.
    fn live(&self) -> Vec<TransactionId> {
        self.order
            .iter()
            .filter(|id| self.entries.get(id).is_some_and(|e| !e.void))
            .copied()
            .collect()
    }
}

/// Cents uniform in 0.01–99,999.99, with the edge values 0.01, 0.10 and
/// 1,000,000.00 mixed in (3 draws in 20).
fn amount(rng: &mut Rng) -> Money {
    let cents = match rng.below(20) {
        0 => 1,
        1 => 10,
        2 => 100_000_000,
        _ => i64::try_from(rng.below(9_999_999)).expect("cents") + 1,
    };
    Money::from_cents(cents).expect("amount")
}

/// Dates over 2025-01-01…2026-12-31, half of them on a few busy days so
/// there are many same-day ties.
fn day(rng: &mut Rng) -> CalendarDate {
    let start = date(2025, 1, 1);
    let offset = if rng.chance(50) {
        *rng.pick(&[0_u64, 31, 59, 180, 364, 365, 400, 545, 700, 729])
    } else {
        rng.below(730)
    };
    start
        .add_days(i64::try_from(offset).expect("days"))
        .expect("date")
}

/// One or two lines over the given accounts.
fn lines(rng: &mut Rng, accounts: &[AccountId]) -> Vec<(AccountId, Money)> {
    let n = if rng.chance(25) { 2 } else { 1 };
    (0..n).map(|_| (*rng.pick(accounts), amount(rng))).collect()
}

fn new_spec(rng: &mut Rng, b: &Books, roll: u64, memo: &str) -> Spec {
    let shape = match roll {
        0..22 => Shape::Check {
            payee: usize::try_from(rng.below(PAYEES.len() as u64)).expect("payee"),
            lines: lines(rng, &b.expenses),
        },
        22..40 => Shape::Deposit {
            to: if rng.chance(80) { b.a } else { b.b },
            lines: lines(rng, &[b.income]),
        },
        40..55 => {
            let (from, to) = *rng.pick(&[(b.a, b.b), (b.b, b.a), (b.a, b.c)]);
            Shape::Transfer {
                from,
                to,
                amount: amount(rng),
            }
        }
        _ => Shape::Card {
            lines: lines(rng, &b.expenses),
        },
    };
    Spec {
        date: day(rng),
        memo: memo.to_string(),
        shape,
    }
}

/// A new amount on every line (and the header).
fn new_amounts(rng: &mut Rng, s: &mut Spec) {
    match &mut s.shape {
        Shape::Check { lines, .. } | Shape::Deposit { lines, .. } | Shape::Card { lines } => {
            for l in lines.iter_mut() {
                l.1 = amount(rng);
            }
        }
        Shape::Transfer { amount: a, .. } => *a = amount(rng),
    }
}

/// A different account or payee, never changing the posting's kind.
fn new_account(rng: &mut Rng, b: &Books, s: &mut Spec) {
    let other = |a: AccountId| {
        if a == b.expenses[0] {
            b.expenses[1]
        } else {
            b.expenses[0]
        }
    };
    match &mut s.shape {
        Shape::Check { payee, lines } => {
            *payee = (*payee + 1) % PAYEES.len();
            lines[0].0 = other(lines[0].0);
        }
        Shape::Card { lines } => lines[0].0 = other(lines[0].0),
        Shape::Deposit { to, .. } => *to = if *to == b.a { b.b } else { b.a },
        Shape::Transfer { from, to, .. } => {
            let next = *rng.pick(&[(b.a, b.b), (b.b, b.a), (b.a, b.c)]);
            if next == (*from, *to) {
                std::mem::swap(from, to);
                if *from == b.c {
                    // C → A is not a transfer this run makes; go A → B.
                    (*from, *to) = (b.a, b.b);
                }
            } else {
                (*from, *to) = next;
            }
        }
    }
}

fn exact(a: &RegisterAmount, what: &str) -> Money {
    a.exact()
        .unwrap_or_else(|| panic!("{what} is not exact money"))
}

/// Every row of `acct` for `q`, page by page (200 a page).
fn all_rows(h: &Harness, acct: AccountId, q: &RegisterQuery) -> (Vec<RegisterLine>, Option<Money>) {
    let mut out = Vec::new();
    let mut ending;
    let mut page = 1;
    loop {
        let p = h
            .run(h.s1.register().page(acct, q, page, 200))
            .unwrap_or_else(|e| panic!("register page {page}: {e}"));
        assert!(p.exact, "page {page} is not exact");
        ending = p.ending_balance.as_ref().map(|e| exact(e, "endingBalance"));
        if page == 1 {
            assert!(
                exact(&p.balance_forward, "balanceForward").is_zero() || q.start > date(1900, 1, 1),
                "nothing is dated before 1900"
            );
        }
        let n = p.lines.len();
        out.extend(p.lines);
        if u64::from(page) >= p.total_pages || n == 0 {
            assert_eq!(out.len() as u64, p.total_rows, "every row once");
            break;
        }
        page += 1;
    }
    (out, ending)
}

fn account_balance(h: &Harness, acct: AccountId) -> Money {
    h.run(h.s1.accounts().get(acct))
        .expect("account")
        .record
        .balance
        .expect("the engine reports a balance")
}

/// The engine's stored account balances. `accounts.balance` is
/// debit-positive for every account type (the engine's own convention), so
/// the card's is negative while money is owed, the opposite of its register
/// (Q-P12h); the screen flips the stored figure for display only.
fn engine_balances(h: &Harness, b: &Books) -> [(&'static str, AccountId, Money); 3] {
    [
        ("A", b.a, account_balance(h, b.a)),
        ("B", b.b, account_balance(h, b.b)),
        ("C", b.c, account_balance(h, b.c)),
    ]
}

fn check_balances(h: &Harness, b: &Books, o: &Oracle, ctx: &str) {
    for (name, acct, got) in engine_balances(h, b) {
        assert_eq!(
            got.to_wire(),
            o.balance(b, acct).to_wire(),
            "{ctx}: account {name} balance vs oracle"
        );
    }
}

/// The full register check of one account (R1/R2).
fn check_register(h: &Harness, b: &Books, o: &Oracle, acct: AccountId, rng: &mut Rng, ctx: &str) {
    let all = RegisterQuery::new(date(1900, 1, 1), date(2999, 12, 31));
    let (rows, ending) = all_rows(h, acct, &all);
    assert_eq!(rows.len(), o.rows(b, acct), "{ctx}: rows vs oracle");
    let ids: BTreeSet<_> = rows.iter().map(|l| l.line_id.to_string()).collect();
    assert_eq!(ids.len(), rows.len(), "{ctx}: a line id twice");

    // Each balance = previous + deposit − payment, in date order: the
    // engine's deposit is the increase on the account's normal side.
    let mut running = Money::ZERO;
    let mut base = BTreeMap::new();
    for l in &rows {
        assert!(
            o.entries.contains_key(&l.txn_id),
            "{ctx}: a row the run never made"
        );
        let dep = l
            .deposit
            .as_ref()
            .map_or(Money::ZERO, |m| exact(m, "deposit"));
        let pay = l
            .payment
            .as_ref()
            .map_or(Money::ZERO, |m| exact(m, "payment"));
        running = running
            .checked_add(dep)
            .and_then(|r| r.checked_sub(pay))
            .expect("running");
        let bal = exact(&l.balance, "runningBalance");
        assert_eq!(bal.to_wire(), running.to_wire(), "{ctx}: row balance");
        base.insert(l.line_id.to_string(), bal.to_wire());
    }
    let expected = b.register_sign(acct, o.balance(b, acct));
    let ending = ending.unwrap_or_else(|| panic!("{ctx}: no endingBalance"));
    assert_eq!(
        running.to_wire(),
        expected.to_wire(),
        "{ctx}: last row vs oracle"
    );
    assert_eq!(
        ending.to_wire(),
        expected.to_wire(),
        "{ctx}: ending vs oracle"
    );

    // The footer: the stored balance (debit − credit, so the card's is the
    // register's negated) and no cleared amount.
    let summary = h.run(h.s1.register().summary(acct, None)).expect("summary");
    assert!(summary.exact, "{ctx}: summary not exact");
    let stored = account_balance(h, acct);
    assert_eq!(
        exact(&summary.current_balance, "currentBalance").to_wire(),
        stored.to_wire(),
        "{ctx}: summary current balance"
    );
    assert_eq!(
        b.register_sign(acct, stored).to_wire(),
        ending.to_wire(),
        "{ctx}: stored balance vs register ending (per-account sign)"
    );
    let summary_ending = summary
        .ending_balance
        .as_ref()
        .map(|e| exact(e, "summary endingBalance"));
    assert_eq!(summary_ending, Some(ending), "{ctx}: summary ending");
    if let Some(c) = summary.cleared_balance.as_ref() {
        assert!(
            exact(c, "clearedBalance").is_zero(),
            "{ctx}: nothing is cleared"
        );
    }

    // R1: filters and sorts hide rows or move them; never a balance.
    let mut queries = Vec::new();
    for _ in 0..20 {
        let mut q = all.clone();
        match rng.below(4) {
            0 => q.payee = Some((*rng.pick(&PAYEES)).to_string()),
            1 => {
                let lo = amount(rng);
                let hi = amount(rng);
                let (lo, hi) = if lo.to_ten_thousandths() <= hi.to_ten_thousandths() {
                    (lo, hi)
                } else {
                    (hi, lo)
                };
                q.min = Some(lo);
                q.max = Some(hi);
            }
            2 => {
                q.txn_type =
                    Some(*rng.pick(&[TxnType::Expense, TxnType::Deposit, TxnType::Transfer]));
            }
            _ => q.cleared = Some(ClearedFilter::Uncleared),
        }
        queries.push(q);
    }
    for sort in [
        RegisterSort::Amount,
        RegisterSort::Type,
        RegisterSort::Number,
    ] {
        let mut q = all.clone();
        q.sort = sort;
        q.dir = if rng.chance(50) {
            SortDir::Asc
        } else {
            SortDir::Desc
        };
        queries.push(q);
    }
    for q in &queries {
        let (got, got_ending) = all_rows(h, acct, q);
        if !q.filters_active() {
            assert_eq!(got.len(), rows.len(), "{ctx}: a sort lost rows ({q:?})");
        }
        for l in &got {
            let bal = exact(&l.balance, "runningBalance").to_wire();
            assert_eq!(
                base.get(&l.line_id.to_string()),
                Some(&bal),
                "{ctx}: balance differs from the unfiltered one ({q:?})"
            );
        }
        assert_eq!(got_ending, Some(ending), "{ctx}: ending changed ({q:?})");
    }
}

pub fn balance_holds_after_random_edits(h: &Harness) {
    let exact_register = h.require_cap(Capabilities::REGISTER_EXACT, "R1/R2");
    let default_ops = if exact_register {
        DEFAULT_OPS_EXACT
    } else {
        DEFAULT_OPS
    };
    let ops: usize = env_or("CL_PROP_OPS", default_ops);
    let seed: u64 = env_or("CL_PROP_SEED", DEFAULT_SEED);
    eprintln!("    register property: CL_PROP_SEED={seed} CL_PROP_OPS={ops}");
    // The seed in every failure message, for a replay.
    let ctx = |i: usize| format!("seed {seed}, op {i}");

    let can_delete = h.require_cap(Capabilities::TXN_DELETE, "R7");
    let s = &h.s1;
    let b = Books {
        a: detail_account(h, s, AccountType::Asset, "bank"),
        b: detail_account(h, s, AccountType::Asset, "bank"),
        c: detail_account(h, s, AccountType::Liability, "credit_card"),
        expenses: [
            account(h, s, AccountType::Expense, "Property supplies").id,
            account(h, s, AccountType::Expense, "Property fuel").id,
        ],
        income: account(h, s, AccountType::Revenue, "Property sales").id,
    };
    let memo = unique("register property");
    let mut rng = Rng(seed);
    let mut o = Oracle::default();
    let posting_opts = PostingOptions::default();

    for i in 0..ops {
        let live = o.live();
        let roll = loop {
            let r = rng.below(100);
            let edit = r >= 65;
            let delete = r >= 98;
            if (edit && live.is_empty()) || (delete && !can_delete) {
                continue;
            }
            break r;
        };
        match roll {
            // Create: check 22 · deposit 18 · transfer 15 · card charge 10.
            0..65 => {
                let spec = new_spec(&mut rng, &b, roll, &memo);
                let made = h
                    .run(s.transactions().create_bank_posting(
                        &spec.posting(&b),
                        &posting_opts,
                        key(),
                    ))
                    .unwrap_or_else(|e| panic!("{}: create: {e}", ctx(i)));
                assert_eq!(made.record.txn_type, spec.type_(), "{}", ctx(i));
                o.order.push(made.record.id);
                o.entries.insert(
                    made.record.id,
                    Entry {
                        spec,
                        version: made.version,
                        void: false,
                    },
                );
            }
            // Edit: amount 12 · date 10 · account or payee 5.
            65..92 => {
                let id = *rng.pick(&live);
                let e = o.entries.get_mut(&id).expect("live entry");
                let mut spec = e.spec.clone();
                match roll {
                    65..77 => new_amounts(&mut rng, &mut spec),
                    77..87 => spec.date = day(&mut rng),
                    _ => new_account(&mut rng, &b, &mut spec),
                }
                let upd = h
                    .run(s.transactions().update_bank_posting(
                        id,
                        &e.version,
                        &spec.posting(&b),
                        &posting_opts,
                        key(),
                    ))
                    .unwrap_or_else(|err| panic!("{}: update: {err}", ctx(i)));
                e.spec = spec;
                e.version = upd.version;
            }
            // Void 6.
            92..98 => {
                let id = *rng.pick(&live);
                h.run(
                    s.transactions()
                        .void(id, "property test", &posting_opts, key()),
                )
                .unwrap_or_else(|err| panic!("{}: void: {err}", ctx(i)));
                o.entries.get_mut(&id).expect("live entry").void = true;
            }
            // Delete 2 (R7).
            _ => {
                let id = *rng.pick(&live);
                h.run(
                    s.transactions()
                        .delete(id, &DeleteOptions::default(), key()),
                )
                .unwrap_or_else(|err| panic!("{}: delete: {err}", ctx(i)));
                o.entries.remove(&id);
                o.order.retain(|x| *x != id);
            }
        }
        if (i + 1) % 100 == 0 {
            check_balances(h, &b, &o, &ctx(i));
        }
    }

    let end = format!("seed {seed}, after {ops} operations");
    check_balances(h, &b, &o, &end);
    if exact_register {
        for acct in [b.a, b.b, b.c] {
            check_register(h, &b, &o, acct, &mut rng, &end);
        }
    }
}
