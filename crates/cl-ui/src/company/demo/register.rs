//! The demo "engine" for the account register (P12): pages and the summary
//! of a register, built from the synthetic Ozark Ridge sample.
//!
//! This stands in for the engine in `--demo` only, so it plays the engine's
//! part, including the running balance in date order (filter-independent,
//! as with the engine's `register-exact-v1`). The register screen never
//! adds anything up: it shows what this module (or the real engine) sends.
//! All figures are synthetic.

use std::cmp::Ordering;

use cl_api::{
    Account, Category, ClearState, EntryType, RegisterAccount, RegisterAmount, RegisterLine,
    RegisterPage, RegisterQuery, RegisterSort, RegisterSummary, SortDir, TxnStatus, TxnType,
};
use cl_company::Sample;
use cl_company::sample::SampleTransaction;
use cl_core::ids::{AccountId, JournalLineId, TransactionId};
use cl_core::{CalendarDate, Money};

/// The statement date of the demo's last finished reconciliation (the
/// mockup's "Last reconciled 09/30/2026"); cleared lines on or before it
/// show as reconciled.
#[must_use]
pub fn last_reconciled() -> CalendarDate {
    CalendarDate::from_ymd(2026, 9, 30).unwrap_or_else(|_| CalendarDate::min())
}

fn txn_id(t: &SampleTransaction) -> TransactionId {
    TransactionId::parse(&format!("d0000000-0000-4000-8000-b{:011}", t.id))
        .unwrap_or_else(|_| unreachable!("a well-formed demo id"))
}

fn line_id(t: &SampleTransaction, index: usize) -> JournalLineId {
    JournalLineId::parse(&format!("d0000000-0000-4000-8000-c{:07}{index:04}", t.id))
        .unwrap_or_else(|_| unreachable!("a well-formed demo id"))
}

fn txn_type(kind: &str) -> TxnType {
    match kind {
        "invoice" => TxnType::Invoice,
        "payment" => TxnType::CustomerPayment,
        "bill" => TxnType::Bill,
        "bill_payment" => TxnType::BillPayment,
        "deposit" => TxnType::Deposit,
        "journal_entry" => TxnType::JournalEntry,
        "transfer" => TxnType::Transfer,
        // `check` and `card_charge` are expenses paid from the account.
        _ => TxnType::Expense,
    }
}

/// One line of the account, before filtering, with the engine-style
/// running balance after it.
struct Built {
    line: RegisterLine,
    amount: Money,
}

fn number_of(a: &Account) -> Option<&str> {
    a.fields.account_number.as_deref()
}

/// Every line of `account` in date order with its running balance, and the
/// account's balance before each date (for the balance forward).
fn build(chart: &[Account], account: &Account) -> Vec<Built> {
    let Some(number) = number_of(account) else {
        return Vec::new();
    };
    let sample = Sample::load();
    let reconciled_through = last_reconciled();
    let mut txns: Vec<&SampleTransaction> = sample
        .transactions
        .iter()
        .filter(|t| t.lines.iter().any(|l| l.account == number))
        .collect();
    txns.sort_by(|a, b| a.date.cmp(&b.date).then(a.id.cmp(&b.id)));
    let mut running = Money::ZERO;
    let mut out = Vec::new();
    for t in txns {
        let others: Vec<&str> = t
            .lines
            .iter()
            .filter(|l| l.account != number)
            .map(|l| l.account.as_str())
            .collect();
        let category = match others.as_slice() {
            [one] => chart
                .iter()
                .find(|a| number_of(a) == Some(one))
                .map_or(Category::None, |a| Category::Account {
                    id: a.id,
                    name: Some(a.fields.name.clone()),
                }),
            [] => Category::None,
            many if many.iter().all(|n| *n == many[0]) => chart
                .iter()
                .find(|a| number_of(a) == Some(many[0]))
                .map_or(Category::None, |a| Category::Account {
                    id: a.id,
                    name: Some(a.fields.name.clone()),
                }),
            _ => Category::Split,
        };
        for (i, l) in t
            .lines
            .iter()
            .enumerate()
            .filter(|(_, l)| l.account == number)
        {
            let net = l.debit.checked_sub(l.credit).unwrap_or(Money::ZERO);
            running = running.checked_add(net).unwrap_or(running);
            let cleared = match (t.cleared, t.date <= reconciled_through) {
                (true, true) => ClearState::Reconciled,
                (true, false) => ClearState::Cleared,
                _ => ClearState::None,
            };
            let kind = txn_type(&t.kind);
            let number = (!t.number.is_empty()).then(|| t.number.clone());
            out.push(Built {
                amount: net.abs(),
                line: RegisterLine {
                    line_id: line_id(t, i),
                    txn_id: txn_id(t),
                    txn_type: kind,
                    check_number: (t.kind == "check").then(|| t.number.parse().ok()).flatten(),
                    number,
                    date: t.date,
                    payee: t.contact.clone(),
                    contact: None,
                    category: category.clone(),
                    memo: (!t.memo.is_empty()).then(|| t.memo.clone()),
                    payment: l
                        .credit
                        .is_positive()
                        .then_some(RegisterAmount::Exact(l.credit)),
                    deposit: l
                        .debit
                        .is_positive()
                        .then_some(RegisterAmount::Exact(l.debit)),
                    balance: RegisterAmount::Exact(running),
                    cleared,
                    has_splits: matches!(category, Category::Split),
                    engine_editable: cleared != ClearState::Reconciled,
                    status: TxnStatus::Posted,
                },
            });
        }
    }
    out
}

fn matches(b: &Built, q: &RegisterQuery) -> bool {
    let l = &b.line;
    if l.date < q.start || l.date > q.end {
        return false;
    }
    if q.txn_type.is_some_and(|k| k != l.txn_type) {
        return false;
    }
    if let Some(p) = q.payee.as_deref().filter(|p| !p.is_empty()) {
        let want = p.to_lowercase();
        if !l
            .payee
            .as_deref()
            .is_some_and(|n| n.to_lowercase().contains(&want))
        {
            return false;
        }
    }
    if q.min.is_some_and(|m| b.amount < m) || q.max.is_some_and(|m| b.amount > m) {
        return false;
    }
    match q.cleared {
        Some(cl_api::ClearedFilter::Uncleared) => l.cleared == ClearState::None,
        Some(cl_api::ClearedFilter::Cleared) => l.cleared == ClearState::Cleared,
        Some(cl_api::ClearedFilter::Reconciled) => l.cleared == ClearState::Reconciled,
        None => true,
    }
}

fn type_name(t: TxnType) -> String {
    serde_json::to_value(t)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// One page of the demo register, shaped like the engine's answer with
/// `register-exact-v1` (exact amounts, balances that ignore filters and
/// sort). `page` counts from 1.
#[must_use]
pub fn page(
    chart: &[Account],
    account: AccountId,
    q: &RegisterQuery,
    page: u32,
    per_page: u16,
) -> Option<RegisterPage> {
    let acct = chart.iter().find(|a| a.id == account)?;
    let all = build(chart, acct);
    let balance_forward = all
        .iter()
        .take_while(|b| b.line.date < q.start)
        .last()
        .and_then(|b| b.line.balance.exact())
        .unwrap_or(Money::ZERO);
    let ending = all
        .iter()
        .take_while(|b| b.line.date <= q.end)
        .last()
        .and_then(|b| b.line.balance.exact())
        .unwrap_or(Money::ZERO);
    let mut rows: Vec<Built> = all.into_iter().filter(|b| matches(b, q)).collect();
    let by_date = |a: &Built, b: &Built| a.line.date.cmp(&b.line.date);
    rows.sort_by(|a, b| {
        let o = match q.sort {
            RegisterSort::Date => Ordering::Equal,
            RegisterSort::Amount => a.amount.cmp(&b.amount),
            RegisterSort::Type => type_name(a.line.txn_type).cmp(&type_name(b.line.txn_type)),
            RegisterSort::Number => a.line.number.cmp(&b.line.number),
        };
        let o = o.then_with(|| by_date(a, b));
        if q.dir == SortDir::Desc {
            o.reverse()
        } else {
            o
        }
    });
    let total_rows = rows.len() as u64;
    let per = u64::from(per_page.max(1));
    let skip = usize::try_from(u64::from(page.saturating_sub(1)) * per).unwrap_or(usize::MAX);
    let lines: Vec<RegisterLine> = rows
        .into_iter()
        .skip(skip)
        .take(usize::from(per_page.max(1)))
        .map(|b| b.line)
        .collect();
    let allowed = match acct.fields.detail_type.as_deref() {
        Some("bank") => vec![
            EntryType::Expense,
            EntryType::Deposit,
            EntryType::Transfer,
            EntryType::JournalEntry,
        ],
        Some("credit_card") => vec![
            EntryType::Expense,
            EntryType::Transfer,
            EntryType::JournalEntry,
        ],
        _ => vec![EntryType::JournalEntry],
    };
    Some(RegisterPage {
        account: RegisterAccount {
            id: acct.id,
            account_type: acct.fields.account_type,
            detail_type: acct.fields.detail_type.clone(),
        },
        exact: true,
        balance_forward: RegisterAmount::Exact(balance_forward),
        ending_balance: Some(RegisterAmount::Exact(ending)),
        page,
        per_page,
        total_rows,
        total_pages: total_rows.div_ceil(per),
        allowed,
        lines,
    })
}

/// The demo register footer through `end` (all dates when `None`).
#[must_use]
pub fn summary(
    chart: &[Account],
    account: AccountId,
    end: Option<CalendarDate>,
) -> Option<RegisterSummary> {
    let acct = chart.iter().find(|a| a.id == account)?;
    let all = build(chart, acct);
    let through = |b: &&Built| end.is_none_or(|e| b.line.date <= e);
    let last_balance = |it: &mut dyn Iterator<Item = &Built>| {
        it.last()
            .and_then(|b| b.line.balance.exact())
            .unwrap_or(Money::ZERO)
    };
    let current = last_balance(&mut all.iter());
    let ending = last_balance(&mut all.iter().filter(through));
    // The demo engine's own cleared total (not the screen's arithmetic).
    let cleared = all
        .iter()
        .filter(through)
        .filter(|b| b.line.cleared != ClearState::None)
        .try_fold(Money::ZERO, |acc, b| {
            let net = match (&b.line.deposit, &b.line.payment) {
                (Some(d), _) => d.exact(),
                (None, Some(p)) => p.exact().and_then(|m| m.checked_neg().ok()),
                _ => Some(Money::ZERO),
            }?;
            acc.checked_add(net).ok()
        })
        .unwrap_or(Money::ZERO);
    let uncleared = all
        .iter()
        .filter(through)
        .filter(|b| b.line.cleared == ClearState::None)
        .count() as u64;
    let reconciled = all
        .iter()
        .any(|b| b.line.cleared == ClearState::Reconciled)
        .then(last_reconciled);
    Some(RegisterSummary {
        exact: true,
        current_balance: RegisterAmount::Exact(current),
        ending_balance: Some(RegisterAmount::Exact(ending)),
        cleared_balance: Some(RegisterAmount::Exact(cleared)),
        as_of: end,
        uncleared_count: uncleared,
        last_reconciled: reconciled,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn operating(chart: &[Account]) -> AccountId {
        chart
            .iter()
            .find(|a| a.fields.account_number.as_deref() == Some("1010"))
            .map(|a| a.id)
            .expect("1010")
    }

    #[test]
    fn pages_cover_the_range_once_and_end_on_the_account_balance() {
        let chart = super::super::chart();
        let id = operating(&chart);
        let all = RegisterQuery::new(CalendarDate::min(), CalendarDate::max());
        let first = page(&chart, id, &all, 1, 200).expect("page");
        assert!(first.exact);
        assert!(first.total_rows > 200, "{}", first.total_rows);
        let mut seen = std::collections::BTreeSet::new();
        let mut last = None;
        for p in 1..=u32::try_from(first.total_pages).expect("pages") {
            let pg = page(&chart, id, &all, p, 200).expect("page");
            for l in &pg.lines {
                assert!(seen.insert(l.line_id), "a line once");
                last = l.balance.exact();
            }
        }
        assert_eq!(seen.len() as u64, first.total_rows);
        let acct = chart.iter().find(|a| a.id == id).expect("acct");
        assert_eq!(last, acct.balance, "the last balance is the account's");
        let s = summary(&chart, id, None).expect("summary");
        assert_eq!(s.ending_balance.and_then(|a| a.exact()), acct.balance);
        assert_eq!(s.last_reconciled, Some(last_reconciled()));
        assert!(s.uncleared_count > 0);
    }

    #[test]
    fn balances_ignore_sort_and_filters() {
        let chart = super::super::chart();
        let id = operating(&chart);
        let mut q = RegisterQuery::new(
            CalendarDate::from_ymd(2026, 9, 1).expect("d"),
            CalendarDate::from_ymd(2026, 10, 31).expect("d"),
        );
        let base = page(&chart, id, &q, 1, 200).expect("page");
        let by_line = |p: &RegisterPage| -> Vec<(JournalLineId, Option<Money>)> {
            let mut v: Vec<_> = p
                .lines
                .iter()
                .map(|l| (l.line_id, l.balance.exact()))
                .collect();
            v.sort_by_key(|(k, _)| *k);
            v
        };
        q.sort = RegisterSort::Amount;
        q.dir = SortDir::Desc;
        let sorted = page(&chart, id, &q, 1, 200).expect("page");
        assert_eq!(by_line(&sorted), by_line(&base));
        q.min = Some(Money::parse_wire("1000").expect("m"));
        let filtered = page(&chart, id, &q, 1, 200).expect("page");
        assert!(filtered.lines.len() < base.lines.len());
        let all = by_line(&base);
        for (k, b) in by_line(&filtered) {
            assert!(all.contains(&(k, b)));
        }
        assert!(
            page(
                &chart,
                AccountId::parse("d0000000-0000-4000-8000-a99999999999").expect("id"),
                &q,
                1,
                1
            )
            .is_none()
        );
    }
}
