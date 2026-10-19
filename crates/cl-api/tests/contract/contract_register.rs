//! The account register (P12 WP1): dates always honoured, paging reaches
//! every row once, the entry kinds per account, and exact money.
//!
//! At the pinned engine the register answers money as JSON numbers, so the
//! typed client refuses every page (`InexactMoney`); the shape checks below
//! read the raw JSON for the non-money fields only. With
//! `register-exact-v1` (engine PR #153, R1–R4) the typed pages are checked
//! too: exact strings, payment = credit and deposit = debit against
//! `GET /transactions/{id}`, the same balance per line whatever the sort or
//! filter, and the summary's ending balance. PENDING(R1–R4) until then.

use std::collections::{BTreeMap, BTreeSet};

use cl_api::{
    AccountFields, AccountType, ApiError, Capabilities, Method, RegisterQuery, RegisterSort,
    SortDir,
};
use cl_core::Money;
use cl_core::ids::AccountId;
use serde_json::Value;

use crate::common::{account, date, je2, key, post};
use crate::support::{Harness, unique};

fn bank(h: &Harness, s: &cl_api::Session) -> AccountId {
    let fields = AccountFields {
        name: unique("Register bank"),
        account_number: None,
        account_type: AccountType::Asset,
        detail_type: Some("bank".into()),
        description: None,
        parent_id: None,
    };
    h.run(s.accounts().create(&fields, key()))
        .unwrap_or_else(|e| panic!("create bank: {e}"))
        .record
        .id
}

fn raw_page(h: &Harness, acct: AccountId, q: &[(&str, &str)]) -> Value {
    let r = h
        .run(h.s1.raw(
            Method::Get,
            &format!("/api/v1/accounts/{acct}/register"),
            q,
            None,
        ))
        .expect("register");
    assert_eq!(r.status, 200, "register {q:?}: {}", r.body);
    r.body
}

fn line_ids(v: &Value) -> Vec<String> {
    v["lines"]
        .as_array()
        .expect("lines")
        .iter()
        .map(|l| l["lineId"].as_str().expect("lineId").to_string())
        .collect()
}

pub fn dates_paging_and_exact_money(h: &Harness) {
    let s = &h.s1;
    let acct = bank(h, s);
    let supplies = account(h, s, AccountType::Expense, "Register supplies").id;
    let memo = unique("register");
    // Far from today, so the engine's default range could never include them.
    let days = [date(2019, 5, 1), date(2019, 5, 2), date(2019, 5, 3)];
    let made: Vec<_> = days
        .iter()
        .enumerate()
        .map(|(i, on)| {
            let amount = ["12.3400", "100.0000", "7.0500"][i];
            // Money in on the first day, out on the others.
            let e = if i == 0 {
                je2(*on, &memo, acct, supplies, amount)
            } else {
                je2(*on, &memo, supplies, acct, amount)
            };
            post(h, s, &e).record
        })
        .collect();

    // Dates are honoured: only the first two days.
    let two = raw_page(
        h,
        acct,
        &[
            ("start_date", "2019-05-01"),
            ("end_date", "2019-05-02"),
            ("exact", "1"),
        ],
    );
    let got_dates: BTreeSet<&str> = two["lines"]
        .as_array()
        .expect("lines")
        .iter()
        .map(|l| l["txnDate"].as_str().expect("date"))
        .collect();
    assert_eq!(
        got_dates,
        ["2019-05-01", "2019-05-02"].into_iter().collect(),
        "dates not honoured"
    );
    assert_eq!(two["pagination"]["totalRows"], 2);
    assert_eq!(
        two["allowedEntryTypes"],
        serde_json::json!(["expense", "deposit", "transfer", "journal_entry"]),
        "bank register entry kinds"
    );

    // The "All dates" bounds (Q-P12l) are accepted; one row a page reaches
    // every row exactly once.
    let all = [
        ("start_date", "1900-01-01"),
        ("end_date", "2199-12-31"),
        ("exact", "1"),
    ];
    let first = raw_page(h, acct, &all);
    let total = first["pagination"]["totalRows"].as_u64().expect("rows");
    assert_eq!(total, 3);
    let mut seen = Vec::new();
    for page in 1..=total {
        let p = page.to_string();
        let mut q = all.to_vec();
        q.extend([("page", p.as_str()), ("per_page", "1")]);
        seen.extend(line_ids(&raw_page(h, acct, &q)));
    }
    let unique_ids: BTreeSet<_> = seen.iter().collect();
    assert_eq!(seen.len(), 3, "every row once");
    assert_eq!(unique_ids.len(), 3, "no duplicate line ids");

    // The typed client.
    let q = RegisterQuery::new(date(1900, 1, 1), date(2199, 12, 31));
    let typed = h.run(s.register().page(acct, &q, 1, 200));
    let summary = h.run(s.register().summary(acct, None));
    if !h.require_cap(Capabilities::REGISTER_EXACT, "R1-R4") {
        // Float money is refused, never parsed.
        assert!(
            matches!(typed, Err(ApiError::InexactMoney { .. })),
            "{typed:?}"
        );
        assert!(
            matches!(summary, Err(ApiError::InexactMoney { .. })),
            "{summary:?}"
        );
        return;
    }
    let page = typed.expect("exact page");
    assert_eq!(page.total_rows, 3);
    assert_eq!(page.ending_balance.to_wire(), "-94.7100");
    let summary = summary.expect("exact summary");
    assert_eq!(summary.ending_balance, Some(page.ending_balance));
    // Payment = credit and deposit = debit of this account's line.
    for l in &page.lines {
        let t = h.run(s.transactions().get(l.txn_id)).expect("txn").record;
        let mine = t
            .lines
            .iter()
            .find(|x| x.account_id == acct)
            .expect("bank line");
        let side = |m: Money| (!m.is_zero()).then_some(m);
        assert_eq!(l.deposit, side(mine.debit), "deposit = debit");
        assert_eq!(l.payment, side(mine.credit), "payment = credit");
        assert!(made.iter().any(|m| m.id == l.txn_id));
    }
    // The same balance per line whatever the sort (R1).
    let by_line = |p: &cl_api::RegisterPage| -> BTreeMap<String, String> {
        p.lines
            .iter()
            .map(|l| (l.line_id.to_string(), l.balance.to_wire()))
            .collect()
    };
    let base = by_line(&page);
    for sort in [
        RegisterSort::Amount,
        RegisterSort::Type,
        RegisterSort::Number,
    ] {
        let mut sq = q.clone();
        sq.sort = sort;
        sq.dir = SortDir::Desc;
        let p = h.run(s.register().page(acct, &sq, 1, 200)).expect("sorted");
        assert_eq!(by_line(&p), base, "{sort:?}");
    }
    // A filter hides rows but never changes a balance (R1).
    let mut fq = q;
    fq.min = Some(Money::parse_wire("50").expect("m"));
    let p = h
        .run(s.register().page(acct, &fq, 1, 200))
        .expect("filtered");
    assert_eq!(p.lines.len(), 1);
    for (id, bal) in by_line(&p) {
        assert_eq!(base.get(&id), Some(&bal), "filtered balance");
    }
    assert_eq!(p.ending_balance, page.ending_balance, "unfiltered ending");
}
