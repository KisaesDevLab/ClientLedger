//! Chart of Accounts (P11 step 1): unique numbers (Q-209), deactivate and
//! reactivate, the engine-kept balance of each account on its own (no
//! parent rollup), and the quick report (`/reports/account-report`). The
//! engine gaps U-31..U-34 are capability probes: each asserts what the
//! engine does today and prints `PENDING(U-xx)` until the capability is
//! advertised.

use cl_api::{
    AccountType, AccountUpdate, ApiError, Capabilities, ConflictReason, Method, ReportDate,
};
use serde_json::json;

use crate::common::{account, account_fields, date, je2, key, m, pair, post};
use crate::support::{Harness, unique};

/// A number nobody else uses in this run (at most 20 characters).
fn fresh_number(prefix: char) -> String {
    let u = unique("n");
    let digits: String = u.chars().filter(char::is_ascii_alphanumeric).collect();
    let tail = &digits[digits.len().saturating_sub(10)..];
    format!("{prefix}{tail}")
}

/// Q-209: a used number is refused with 409 `ACCOUNT_NUMBER_EXISTS`, on
/// create and on update, and nothing is created.
pub fn unique_numbers(h: &Harness) {
    let s = &h.s1;
    let number = fresh_number('7');
    let mut first = account_fields(AccountType::Expense, "Dues");
    first.account_number = Some(number.clone());
    h.run(s.accounts().create(&first, key())).expect("first");

    let mut second = account_fields(AccountType::Expense, "Dues again");
    second.account_number = Some(number.clone());
    let e = h
        .run(s.accounts().create(&second, key()))
        .expect_err("duplicate number");
    assert!(
        matches!(
            e,
            ApiError::Conflict {
                reason: ConflictReason::AlreadyExists,
                ..
            }
        ),
        "{e:?}"
    );

    let other = account(h, s, AccountType::Expense, "Other dues");
    let got = h.run(s.accounts().get(other.id)).expect("get");
    let mut fields = got.record.fields.clone();
    fields.account_number = Some(number);
    let upd = AccountUpdate {
        fields,
        is_active: true,
    };
    let e = h
        .run(s.accounts().update(other.id, &got.version, &upd, key()))
        .expect_err("duplicate number on update");
    assert!(
        matches!(
            e,
            ApiError::Conflict {
                reason: ConflictReason::AlreadyExists,
                ..
            }
        ),
        "{e:?}"
    );
}

/// Deactivate (zero balance) and reactivate through an update.
pub fn deactivate_and_reactivate(h: &Harness) {
    let s = &h.s1;
    let a = account(h, s, AccountType::Expense, "Seasonal");
    let off = h
        .run(s.accounts().deactivate(a.id, key()))
        .expect("deactivate");
    assert!(!off.record.is_active);
    let got = h.run(s.accounts().get(a.id)).expect("get");
    assert!(!got.record.is_active);
    let upd = AccountUpdate {
        fields: got.record.fields.clone(),
        is_active: true,
    };
    let on = h
        .run(s.accounts().update(a.id, &got.version, &upd, key()))
        .expect("reactivate");
    assert!(on.record.is_active);
}

/// Each account reports its own balance; a parent's balance is not the
/// sum of its children (the UI never adds them up either, NN7).
pub fn own_balance_only(h: &Harness) {
    let s = &h.s1;
    let parent = account(h, s, AccountType::Expense, "Vehicles");
    let mut child = account_fields(AccountType::Expense, "Fuel");
    child.parent_id = Some(parent.id);
    let child = h
        .run(s.accounts().create(&child, key()))
        .expect("child")
        .record;
    assert_eq!(child.fields.parent_id, Some(parent.id));
    let cash = account(h, s, AccountType::Asset, "Cash");
    post(
        h,
        s,
        &je2(
            date(2026, 7, 6),
            &unique("fuel"),
            child.id,
            cash.id,
            "41.2500",
        ),
    );
    let child_now = h.run(s.accounts().get(child.id)).expect("child").record;
    let parent_now = h.run(s.accounts().get(parent.id)).expect("parent").record;
    assert_eq!(child_now.balance, Some(m("41.2500")));
    assert_eq!(
        parent_now.balance.map(cl_core::Money::is_zero),
        Some(true),
        "the parent keeps its own (zero) balance"
    );
}

/// The quick report: the account's posted lines with exact amounts, inside the
/// date range only.
pub fn quick_report(h: &Harness) {
    let s = &h.s1;
    let (cash, supplies) = pair(h, s);
    let memo = unique("qr");
    post(
        h,
        s,
        &je2(date(2026, 7, 3), &memo, supplies.id, cash.id, "12.3400"),
    );
    post(
        h,
        s,
        &je2(date(2026, 8, 3), &memo, supplies.id, cash.id, "1.0000"),
    );
    let july = h
        .run(s.reports().account_report(
            supplies.id,
            Some(date(2026, 7, 1)),
            Some(date(2026, 7, 31)),
        ))
        .expect("report");
    assert_eq!(july.account_id, supplies.id);
    assert_eq!(july.data.len(), 1, "only July");
    let line = &july.data[0];
    assert_eq!(line.debit.to_wire(), "12.3400");
    assert!(line.credit.is_zero());
    assert_eq!(line.txn_type, "journal_entry");
    assert_eq!(line.memo.as_deref(), Some(memo.as_str()));
    match line.txn_date {
        ReportDate::Day(d) => assert_eq!(d, date(2026, 7, 3)),
        // Local midnight as an instant: within a day of 07/03 UTC.
        ReportDate::Midnight(t) => {
            let at = t.as_utc().date_naive();
            let want = date(2026, 7, 3).as_naive();
            assert!((at - want).num_days().abs() <= 1, "{at}");
        }
    }
    let all = h
        .run(s.reports().account_report(supplies.id, None, None))
        .expect("all");
    assert_eq!(all.data.len(), 2);
    let credit_side = h
        .run(s.reports().account_report(cash.id, None, None))
        .expect("cash");
    assert_eq!(
        credit_side.data[0].credit.to_wire(),
        "1.0000",
        "newest first"
    );
}

/// U-31: today `/accounts/merge` only deactivates the source and leaves
/// its lines behind, so the Chart of Accounts hides Merge.
pub fn merge_u31(h: &Harness) {
    let s = &h.s1;
    let (cash, source) = pair(h, s);
    let target = account(h, s, AccountType::Expense, "Merge target");
    post(
        h,
        s,
        &je2(
            date(2026, 7, 7),
            &unique("merge"),
            source.id,
            cash.id,
            "2.0000",
        ),
    );
    let raw = h
        .run(s.raw(
            Method::Post,
            "/api/v1/accounts/merge",
            &[],
            Some(&json!({"sourceId": source.id, "targetId": target.id})),
        ))
        .expect("raw");
    let moved = h
        .run(s.reports().account_report(target.id, None, None))
        .expect("target lines")
        .data
        .len();
    if h.require_cap(Capabilities::ACCOUNT_MERGE, "U-31") {
        assert_eq!(raw.status, 200, "{}", raw.body);
        assert_eq!(moved, 1, "the line moved to the target");
    } else {
        // Whether or not the stub answers 200, nothing moves.
        assert_eq!(moved, 0, "the merge stub moves no lines");
    }
}

/// U-32: today `parentId` is not checked (a parent of another type is
/// accepted), so the UI checks type and cycles itself.
pub fn parent_rules_u32(h: &Harness) {
    let s = &h.s1;
    let parent = account(h, s, AccountType::Asset, "Wrong parent");
    let raw = h
        .run(s.raw(
            Method::Post,
            "/api/v1/accounts",
            &[],
            Some(&json!({
                "name": unique("Mismatched child"),
                "accountType": "expense",
                "parentId": parent.id,
            })),
        ))
        .expect("raw");
    if h.require_cap(Capabilities::ACCOUNT_TREE, "U-32") {
        assert_eq!(raw.status, 400, "a parent of another type is refused");
    } else {
        assert_eq!(raw.status, 201, "accepted today: {}", raw.body);
    }
}

/// U-33: today an `isSensitive` field is dropped by the engine's schema.
pub fn sensitive_flag_u33(h: &Harness) {
    let s = &h.s1;
    let raw = h
        .run(s.raw(
            Method::Post,
            "/api/v1/accounts",
            &[],
            Some(&json!({
                "name": unique("Owner draws"),
                "accountType": "equity",
                "isSensitive": true,
            })),
        ))
        .expect("raw");
    assert_eq!(raw.status, 201, "{}", raw.body);
    let flag = &raw.body["account"]["isSensitive"];
    if h.require_cap(Capabilities::ACCOUNT_SENSITIVE, "U-33") {
        assert_eq!(flag, &json!(true));
    } else {
        assert!(flag.is_null(), "not stored today: {flag}");
    }
}

/// U-34: today a full bank number is dropped and nothing masked comes back.
pub fn masked_number_u34(h: &Harness) {
    let s = &h.s1;
    let raw = h
        .run(s.raw(
            Method::Post,
            "/api/v1/accounts",
            &[],
            Some(&json!({
                "name": unique("Operating checking"),
                "accountType": "asset",
                "detailType": "bank",
                "bankAccountNumber": "000123456789",
            })),
        ))
        .expect("raw");
    assert_eq!(raw.status, 201, "{}", raw.body);
    let acct = &raw.body["account"];
    assert!(
        !acct.to_string().contains("123456789"),
        "a full number never comes back"
    );
    if h.require_cap(Capabilities::ACCOUNT_MASKED_NUMBER, "U-34") {
        assert_eq!(
            acct["bankAccountNumberMasked"],
            json!("\u{2022}\u{2022}\u{2022}\u{2022}6789")
        );
    } else {
        assert!(acct["bankAccountNumberMasked"].is_null());
    }
}
