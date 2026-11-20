//! P32 review B2 / plan T9: the exact transaction list the IIF export
//! reads (`reports().transaction_list_exact`) ties to the engine's accrual
//! trial balance at the end date, account by account.
//!
//! The trial balance is not a plain sum: balance-sheet accounts are
//! cumulative through the end date, income and expense accounts cover the
//! fiscal year to date, and earlier income is one "Retained Earnings
//! (Prior Years)" row with no account id. The test posts its own 2-dp
//! entries across two fiscal years (one future-dated), so a filtered run
//! on a near-empty company still has prior-year income and several months.

use std::collections::BTreeMap;

use cl_api::{AccountType, ApiError, Capabilities, Method, Session, TxnList};
use cl_core::CalendarDate;
use rust_decimal::Decimal;
use serde_json::Value;

use crate::common::{account, date, je2, post};
use crate::support::Harness;

/// The list, waiting out the shared report rate limit.
fn list(h: &Harness, s: &Session, end: CalendarDate) -> TxnList {
    for _ in 0..12 {
        match h.run(s.reports().transaction_list_exact(None, Some(end))) {
            Err(ApiError::RateLimited { .. }) => {
                std::thread::sleep(std::time::Duration::from_secs(10));
            }
            other => return other.unwrap_or_else(|e| panic!("transaction list: {e}")),
        }
    }
    panic!("transaction list: still rate limited");
}

fn trial_balance(h: &Harness, s: &Session, start: &str, end: &str) -> Value {
    let q = [
        ("exact", "1"),
        ("basis", "accrual"),
        ("start_date", start),
        ("end_date", end),
    ];
    for _ in 0..12 {
        let r = h
            .run(s.raw(Method::Get, "/api/v1/reports/trial-balance", &q, None))
            .unwrap_or_else(|e| panic!("trial balance: {e}"));
        if r.status != 429 {
            assert_eq!(r.status, 200, "trial balance: {}", r.body);
            return r.body;
        }
        std::thread::sleep(std::time::Duration::from_secs(10));
    }
    panic!("trial balance: still rate limited");
}

fn dec(v: &Value) -> Decimal {
    v.as_str()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("an exact string: {v}"))
}

fn balance_sheet(kind: &str) -> bool {
    matches!(kind, "asset" | "liability" | "equity")
}

pub fn transaction_list_ties_to_the_trial_balance_t9(h: &Harness) {
    if !h.require_cap(Capabilities::REPORTS_EXACT, "U-14") {
        return;
    }
    let s = &h.s1;
    let bank = account(h, s, AccountType::Asset, "IifBank").id;
    let owed = account(h, s, AccountType::Liability, "IifOwed").id;
    let sales = account(h, s, AccountType::Revenue, "IifSales").id;
    let costs = account(h, s, AccountType::Expense, "IifCosts").id;
    // Two fiscal years of 2-dp entries; the last is future-dated.
    let posted = [
        post(
            h,
            s,
            &je2(date(2044, 3, 15), "iif t9", bank, sales, "120.00"),
        ),
        post(
            h,
            s,
            &je2(date(2044, 11, 2), "iif t9", costs, bank, "40.25"),
        ),
        post(h, s, &je2(date(2045, 2, 1), "iif t9", bank, sales, "10.10")),
        post(h, s, &je2(date(2045, 6, 30), "iif t9", costs, owed, "3.33")),
    ];
    let end = date(2045, 12, 31);
    let got = list(h, s, end);
    assert!(!got.hidden_notice, "the owner sees every account");
    assert_ne!(got.rows.len(), 0);
    for p in &posted {
        assert_eq!(
            got.rows.iter().filter(|r| r.id == p.record.id).count(),
            2,
            "each test entry's two lines are listed"
        );
    }
    // Every transaction balances.
    let mut per_txn: BTreeMap<String, Decimal> = BTreeMap::new();
    for r in &got.rows {
        *per_txn.entry(r.id.to_string()).or_default() += r.amount.as_decimal();
        assert!(r.txn_date <= end, "nothing after the end date");
    }
    assert!(
        per_txn.values().all(Decimal::is_zero),
        "every entry balances"
    );

    let tb = trial_balance(h, s, "2045-01-01", "2045-12-31");
    let fy = CalendarDate::parse_wire(tb["fiscalYearStart"].as_str().expect("fiscalYearStart"))
        .expect("date");
    // The year containing the end date, whatever its first month.
    assert_eq!(fy.year(), 2045, "{fy:?}");
    let rows = tb["data"].as_array().expect("rows");
    let mut tb_net: BTreeMap<String, (String, Decimal)> = BTreeMap::new();
    let mut prior_years = Decimal::ZERO;
    for r in rows {
        match r["accountId"].as_str() {
            Some(id) => {
                let kind = r["account_type"].as_str().unwrap_or_default().to_string();
                tb_net.insert(id.to_string(), (kind, dec(&r["netExact"])));
            }
            // The computed "Retained Earnings (Prior Years)" row.
            None => prior_years += dec(&r["netExact"]),
        }
    }

    // The list, summed the trial balance's way.
    let mut sums: BTreeMap<String, Decimal> = BTreeMap::new();
    let mut prior = Decimal::ZERO;
    for r in &got.rows {
        let id = r.account_id.to_string();
        let kind = if let Some((k, _)) = tb_net.get(&id) {
            k.clone()
        } else {
            {
                let a = h
                    .run(s.accounts().get(r.account_id))
                    .unwrap_or_else(|e| panic!("account: {e}"));
                serde_json::to_value(a.record.fields.account_type)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_string))
                    .unwrap_or_default()
            }
        };
        let amount = r.amount.as_decimal();
        if balance_sheet(&kind) || r.txn_date >= fy {
            *sums.entry(id).or_default() += amount;
        } else {
            prior += amount;
        }
    }
    for (id, (_, net)) in &tb_net {
        assert_eq!(
            sums.get(id).copied().unwrap_or_default(),
            *net,
            "account {id}: the list's lines equal its trial-balance row"
        );
    }
    for (id, sum) in &sums {
        if !tb_net.contains_key(id) {
            assert!(
                sum.is_zero(),
                "account {id} is off the trial balance yet sums to {sum}"
            );
        }
    }
    assert_eq!(
        prior, prior_years,
        "prior years' income equals the computed row"
    );
    assert!(!prior.is_zero(), "the test posted prior-year income");
    // The test's balance-sheet accounts, exactly (cumulative, so they
    // don't depend on the company's fiscal year).
    let net = |id: cl_core::ids::AccountId| tb_net.get(&id.to_string()).map(|(_, n)| *n);
    assert_eq!(net(bank), Some("89.85".parse().expect("d")));
    assert_eq!(net(owed), Some("-3.33".parse().expect("d")));
    let _ = (sales, costs);
}
