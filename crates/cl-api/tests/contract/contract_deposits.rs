//! Make Deposit against the real engine (P14 WP-A1.3, AC1).
//!
//! - `grouping_clears_payments_clearing`: two payments waiting in Payments
//!   Clearing (1,250.00 + 842.50, posted as journal entries Dr Payments
//!   Clearing / Cr income, as the sample loader does) are deposited into a
//!   bank account as one group: the bank is debited 2,092.50 exactly, each
//!   payment's line credits Payments Clearing at the engine's amount with
//!   its `sourceTransactionId`, Payments Clearing drops by exactly
//!   2,092.50, neither payment is pending any more and the bank register
//!   shows one 2,092.50 deposit. Depositing them again is a 409
//!   `ALREADY_DEPOSITED` with nothing posted; voiding the deposit returns
//!   them to pending (E2). Without `deposit-grouping-v1` the pending list
//!   is read (display-only amounts are never grouped) and the grouping is
//!   PENDING(U-52).
//! - `fees_and_cash_back`: a payment plus a 300.00 line, a 35.20 fee and
//!   100.00 cash back post exactly (PENDING(U-53) without
//!   `deposit-cashback-v1`; a fee or cash back is then refused locally).
//! - `closed_period`: a deposit dated on or before the closing date asks
//!   for the closing-date password (second company).
//! - `vendor_refund` (U-54): refunds against a vendor credit lower its open
//!   balance exactly; more than is open is a 409.
//! - `feed_deposit_groupings` (U-55): a bank-file deposit gets the exact
//!   set of waiting payments as a suggestion.
//! - `clearing_warning_on_create_and_edit`: `clearingWouldGoNegative` on a
//!   create and on an edit.

use cl_api::{
    AccountFilter, AccountType, ApiError, BankPosting, Capabilities, CashBack,
    ClosedPeriodAuthorization, ContactFields, ContactType, DepositCaps, DepositDraft,
    LocalValidation, Method, NewContact, PostingOptions, RegisterQuery, Session, SplitLine,
    TxnStatus, TxnType, VendorRefundDraft, is_already_deposited, is_vendor_credit_exceeded,
};
use cl_core::ids::{AccountId, TransactionId};
use cl_core::{CalendarDate, Money, Secret};
use serde_json::json;

use crate::common::{account, date, je2, key, m, post};
use crate::contract_bank_postings::detail_account;
use crate::support::{Harness, unique};

fn clearing(h: &Harness, s: &Session) -> AccountId {
    h.run(s.accounts().list_all(&AccountFilter::default()))
        .expect("accounts")
        .into_iter()
        .find(|a| a.system_tag.as_deref() == Some("payments_clearing"))
        .expect("the tenant's Payments Clearing account")
        .id
}

fn balance(h: &Harness, s: &Session, a: AccountId) -> Money {
    h.run(s.accounts().get(a))
        .expect("account")
        .record
        .balance
        .unwrap_or(Money::ZERO)
}

fn caps(h: &Harness) -> DepositCaps {
    DepositCaps {
        grouping: h.caps.has(Capabilities::DEPOSIT_GROUPING),
        cashback: h.caps.has(Capabilities::DEPOSIT_CASHBACK),
        vendor_refund: h.caps.has(Capabilities::VENDOR_REFUND),
    }
}

/// A payment waiting in Payments Clearing (Dr clearing / Cr income).
fn waiting(
    h: &Harness,
    s: &Session,
    clearing: AccountId,
    income: AccountId,
    amount: &str,
) -> TransactionId {
    post(
        h,
        s,
        &je2(
            date(2026, 11, 2),
            &unique("payment"),
            clearing,
            income,
            amount,
        ),
    )
    .record
    .id
}

fn pending_ids(h: &Harness, s: &Session) -> Vec<TransactionId> {
    h.run(s.payments().pending_deposits())
        .expect("pending")
        .items
        .iter()
        .map(|p| p.source)
        .collect()
}

/// AC1: grouped payments equal the bank deposit and clear Payments
/// Clearing.
pub fn grouping_clears_payments_clearing(h: &Harness) {
    let s = &h.s1;
    let clearing = clearing(h, s);
    let income = account(h, s, AccountType::Revenue, "Plant sales").id;
    let bank = detail_account(h, s, AccountType::Asset, "bank");
    let a = waiting(h, s, clearing, income, "1250.00");
    let b = waiting(h, s, clearing, income, "842.50");
    let pending = h.run(s.payments().pending_deposits()).expect("pending");
    let ours: Vec<_> = pending
        .items
        .iter()
        .filter(|p| p.source == a || p.source == b)
        .collect();
    assert_eq!(ours.len(), 2, "both payments wait in Payments Clearing");
    if !h.require_cap(Capabilities::DEPOSIT_GROUPING, "U-52") {
        // An older engine: numbers are display only and never grouped; any
        // exact item is refused locally without the capability.
        let mut d = DepositDraft::plain(bank, date(2026, 11, 4), None, Vec::new());
        d.payments = ours.iter().filter_map(|p| p.grouped(clearing)).collect();
        if !d.payments.is_empty() {
            assert_eq!(
                BankPosting::deposit_with(d, caps(h)).err(),
                Some(LocalValidation::DepositGroupingUnavailable),
                "nothing is sent without deposit-grouping-v1"
            );
        }
        return;
    }
    assert!(pending.exact);
    let before = balance(h, s, clearing);
    let mut d = DepositDraft::plain(
        bank,
        date(2026, 11, 4),
        Some(unique("Bank run")),
        Vec::new(),
    );
    d.payments = ours
        .iter()
        .map(|p| p.grouped(clearing).expect("exact"))
        .collect();
    assert_eq!(d.total(), Some(m("2092.50")));
    let posting = BankPosting::deposit_with(d, caps(h)).expect("deposit");
    let made = h
        .run(
            s.transactions()
                .create_bank_posting(&posting, &PostingOptions::default(), key()),
        )
        .unwrap_or_else(|e| panic!("grouped deposit: {e}"));

    // The journal, exactly: Dr bank 2,092.50 / Cr clearing 1,250.00 + 842.50.
    let got = h
        .run(s.transactions().get(made.record.id))
        .expect("get")
        .record;
    assert_eq!(got.txn_type, TxnType::Deposit);
    let bank_lines: Vec<_> = got.lines.iter().filter(|l| l.account_id == bank).collect();
    assert_eq!(bank_lines.len(), 1);
    assert_eq!(bank_lines[0].debit.to_wire(), "2092.5000");
    let mut credits: Vec<(Option<TransactionId>, String)> = got
        .lines
        .iter()
        .filter(|l| l.account_id == clearing)
        .map(|l| (l.source_transaction_id, l.credit.to_wire()))
        .collect();
    credits.sort_by(|x, y| x.1.cmp(&y.1));
    assert_eq!(
        credits,
        [
            (Some(a), "1250.0000".to_string()),
            (Some(b), "842.5000".to_string())
        ]
    );
    // Payments Clearing drops by exactly the deposit.
    assert_eq!(
        balance(h, s, clearing),
        before.checked_sub(m("2092.50")).expect("sub")
    );
    let left = pending_ids(h, s);
    assert!(
        !left.contains(&a) && !left.contains(&b),
        "nothing left to deposit"
    );
    // The bank register shows one 2,092.50 deposit.
    let page = h
        .run(s.register().page(
            bank,
            &RegisterQuery::new(CalendarDate::min(), CalendarDate::max()),
            1,
            50,
        ))
        .expect("register");
    assert_eq!(page.lines.len(), 1);
    assert_eq!(
        page.lines[0]
            .deposit
            .as_ref()
            .and_then(cl_api::RegisterAmount::exact),
        Some(m("2092.50"))
    );

    // Depositing them again: 409, nothing posted.
    let bank_before = balance(h, s, bank);
    let e = h
        .run(
            s.transactions()
                .create_bank_posting(&posting, &PostingOptions::default(), key()),
        )
        .expect_err("already deposited");
    assert!(is_already_deposited(&e), "{e:?}");
    assert_eq!(balance(h, s, bank), bank_before, "nothing posted");

    // Voiding the deposit returns the payments to pending (E2).
    h.run(s.transactions().void(
        made.record.id,
        "contract: release",
        &PostingOptions::default(),
        key(),
    ))
    .expect("void");
    let returned = pending_ids(h, s);
    assert!(
        returned.contains(&a) && returned.contains(&b),
        "the payments wait again"
    );
    assert_eq!(balance(h, s, clearing), before);
    let void = h
        .run(s.transactions().get(made.record.id))
        .expect("get")
        .record;
    assert_eq!(void.status, TxnStatus::Void);
}

/// A fee (negative line) and cash back post exactly.
pub fn fees_and_cash_back(h: &Harness) {
    let s = &h.s1;
    let clearing = clearing(h, s);
    let income = account(h, s, AccountType::Revenue, "Counter sales").id;
    let fees = account(h, s, AccountType::Expense, "Card fees").id;
    let draw = account(h, s, AccountType::Equity, "Owner draw").id;
    let bank = detail_account(h, s, AccountType::Asset, "bank");
    let line = |account: AccountId, amount: &str, memo: &str| SplitLine {
        account,
        amount: m(amount),
        memo: Some(memo.to_string()),
        contact: None,
        billable: false,
        tag: None,
    };
    let mut d = DepositDraft::plain(
        bank,
        date(2026, 11, 4),
        None,
        vec![line(income, "300.00", "Plant sale")],
    );
    d.deductions = vec![line(fees, "35.20", "Card fee")];
    d.cash_back = Some(CashBack {
        account: draw,
        amount: m("100.00"),
        memo: Some("Petty cash".into()),
    });
    if !h.require_cap(Capabilities::DEPOSIT_CASHBACK, "U-53") {
        assert_eq!(
            BankPosting::deposit_with(d, caps(h)).err(),
            Some(LocalValidation::DepositCashBackUnavailable),
            "nothing is sent without deposit-cashback-v1"
        );
        return;
    }
    if h.caps.has(Capabilities::DEPOSIT_GROUPING) {
        let p = waiting(h, s, clearing, income, "100.00");
        let item = h
            .run(s.payments().pending_deposits())
            .expect("pending")
            .items
            .into_iter()
            .find(|i| i.source == p)
            .expect("pending");
        d.payments.push(item.grouped(clearing).expect("exact"));
    }
    let expected = d.total().expect("total");
    let before = [bank, fees, draw].map(|a| balance(h, s, a));
    let posting = BankPosting::deposit_with(d, caps(h)).expect("deposit");
    let made = h
        .run(
            s.transactions()
                .create_bank_posting(&posting, &PostingOptions::default(), key()),
        )
        .unwrap_or_else(|e| panic!("deposit with fee and cash back: {e}"));
    let rec = h
        .run(s.payments().deposit_record(made.record.id, bank))
        .expect("record")
        .0;
    assert!(rec.exact);
    assert_eq!(
        rec.total, expected,
        "the bank gets exactly the form's total"
    );
    assert_eq!(
        rec.cash_back
            .as_ref()
            .map(|c| c.amount.to_wire())
            .as_deref(),
        Some("100.0000")
    );
    assert!(rec.lines.iter().any(|l| l.amount == m("-35.20")));
    let after = [bank, fees, draw].map(|a| balance(h, s, a));
    assert_eq!(after[0], before[0].checked_add(expected).expect("add"));
    assert_eq!(after[1], before[1].checked_add(m("35.20")).expect("add"));
    assert_eq!(
        after[2].checked_sub(before[2]).expect("sub").abs(),
        m("100.00")
    );
}

/// A deposit in the closed period asks for the closing-date password.
pub fn closed_period(h: &Harness) {
    let s = h.s3();
    let income = account(h, &s, AccountType::Revenue, "Sales").id;
    let bank = detail_account(h, &s, AccountType::Asset, "bank");
    let settings = |lock: serde_json::Value| {
        h.run(s.raw_mapped(
            Method::Put,
            "/api/v1/company/settings",
            Some(&json!({ "lockDate": lock })),
        ))
        .expect("closing date");
    };
    settings(json!("2026-09-30"));
    let deposit = || {
        BankPosting::deposit(DepositDraft::plain(
            bank,
            date(2026, 9, 15),
            None,
            vec![SplitLine {
                account: income,
                amount: m("25.00"),
                memo: None,
                contact: None,
                billable: false,
                tag: None,
            }],
        ))
        .expect("deposit")
    };
    let e = h
        .run(
            s.transactions()
                .create_bank_posting(&deposit(), &PostingOptions::default(), key()),
        )
        .expect_err("closed");
    assert!(matches!(&e, ApiError::ClosedPeriod(_)), "{e:?}");
    if h.require_cap(Capabilities::CLOSING_PASSWORD, "U-15") {
        let pw = Secret::new(unique("closing-deposit"));
        assert!(
            h.run(s.set_closing_password(None, Some(&pw), key()))
                .expect("set password")
        );
        let with = PostingOptions {
            closed_period: Some(ClosedPeriodAuthorization::with_password(pw.clone())),
        };
        h.run(
            s.transactions()
                .create_bank_posting(&deposit(), &with, key()),
        )
        .expect("right password");
        assert!(
            !h.run(s.set_closing_password(Some(&pw), None, key()))
                .expect("clear password")
        );
    }
    settings(serde_json::Value::Null);
}

/// The tenant's Accounts Payable account.
fn payables(h: &Harness, s: &Session) -> AccountId {
    h.run(s.accounts().list_all(&AccountFilter::default()))
        .expect("accounts")
        .into_iter()
        .find(|a| a.system_tag.as_deref() == Some("accounts_payable"))
        .expect("the tenant's Accounts Payable account")
        .id
}

/// The engine's open balance and status of a vendor credit
/// (`GET /vendor-credits/{id}`).
fn credit_balance(h: &Harness, s: &Session, id: TransactionId) -> (Money, String) {
    let v = h
        .run(s.raw_mapped(Method::Get, &format!("/api/v1/vendor-credits/{id}"), None))
        .expect("vendor credit");
    let c = &v["credit"];
    let due = match &c["balanceDue"] {
        serde_json::Value::String(t) => m(t),
        other => panic!("balanceDue {other}"),
    };
    (
        due,
        c["billStatus"].as_str().unwrap_or_default().to_string(),
    )
}

/// U-54 (`vendor-refund-v1`): a refund from a vendor applied to its vendor
/// credit lowers the credit's open balance exactly; more than the open
/// balance is a 409 `VENDOR_CREDIT_EXCEEDED` with nothing posted; the rest
/// pays the credit off.
pub fn vendor_refund(h: &Harness) {
    if !h.require_cap(Capabilities::VENDOR_REFUND, "U-54") {
        return;
    }
    let s = &h.s1;
    let bank = detail_account(h, s, AccountType::Asset, "bank");
    let expense = account(h, s, AccountType::Expense, "Returned supplies").id;
    let ap = payables(h, s);
    let vendor = h
        .run(s.contacts().create(
            &NewContact {
                fields: ContactFields::named(ContactType::Vendor, unique("Refund vendor")),
                opening_balance: Money::ZERO,
                opening_balance_date: None,
            },
            key(),
        ))
        .expect("vendor")
        .record
        .id;
    let made = h
        .run(s.raw_mapped(
            Method::Post,
            "/api/v1/vendor-credits",
            Some(&json!({
                "contactId": vendor.to_string(),
                "txnDate": "2026-11-05",
                "lines": [{ "accountId": expense.to_string(), "amount": "100.00" }],
            })),
        ))
        .expect("vendor credit");
    let credit: TransactionId = made["credit"]["id"]
        .as_str()
        .expect("credit id")
        .parse()
        .expect("uuid");
    assert_eq!(credit_balance(h, s, credit).0, m("100.00"));

    let refund = |amount: &str| VendorRefundDraft {
        vendor,
        deposit_to: bank,
        payables: ap,
        date: date(2026, 11, 6),
        amount: m(amount),
        applies_to: credit,
        memo: None,
    };
    let bank_before = balance(h, s, bank);
    let first = refund("30.00").posting(caps(h)).expect("refund");
    let saved = h
        .run(
            s.payments()
                .create_deposit(&first, &PostingOptions::default(), key()),
        )
        .unwrap_or_else(|e| panic!("refund: {e}"));
    assert!(!saved.clearing_would_go_negative);
    assert_eq!(credit_balance(h, s, credit).0, m("70.00"));
    assert_eq!(
        balance(h, s, bank),
        bank_before.checked_add(m("30.00")).expect("add")
    );
    // The A/P line points at the credit.
    let got = h
        .run(s.transactions().get(saved.saved.record.id))
        .expect("get")
        .record;
    let ap_line = got
        .lines
        .iter()
        .find(|l| l.account_id == ap)
        .expect("A/P line");
    assert_eq!(ap_line.source_transaction_id, Some(credit));
    assert_eq!(ap_line.credit.to_wire(), "30.0000");

    // More than is open: refused, nothing posted.
    let too_much = refund("80.00").posting(caps(h)).expect("refund");
    let e = h
        .run(
            s.payments()
                .create_deposit(&too_much, &PostingOptions::default(), key()),
        )
        .expect_err("exceeds the credit");
    assert!(is_vendor_credit_exceeded(&e), "{e:?}");
    assert_eq!(
        balance(h, s, bank),
        bank_before.checked_add(m("30.00")).expect("add")
    );

    // The rest pays it off.
    let rest = refund("70.00").posting(caps(h)).expect("refund");
    h.run(
        s.payments()
            .create_deposit(&rest, &PostingOptions::default(), key()),
    )
    .unwrap_or_else(|e| panic!("refund the rest: {e}"));
    let (due, status) = credit_balance(h, s, credit);
    assert_eq!(due, Money::ZERO);
    assert_eq!(status, "paid");
}

/// A bank-file upload (`POST /banking/feed/import`, CSV date, description,
/// amount; a negative amount is money in).
fn import_feed(h: &Harness, s: &Session, bank: AccountId, csv: &str) -> serde_json::Value {
    // A boundary token: no spaces (`unique` puts one after the prefix).
    let boundary = unique("cl-boundary").replace(' ', "-");
    let mut body = Vec::new();
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"accountId\"\r\n\r\n{bank}\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"feed.csv\"\r\nContent-Type: text/csv\r\n\r\n{csv}\r\n--{boundary}--\r\n"
        )
        .as_bytes(),
    );
    h.run(s.raw_multipart_mapped("/api/v1/banking/feed/import", &boundary, body))
        .unwrap_or_else(|e| panic!("feed import: {e}"))
}

/// U-55 (`feed-deposit-groupings-v1`): for a bank-file deposit the engine
/// suggests the set of waiting payments whose exact amounts add up to it.
pub fn feed_deposit_groupings(h: &Harness) {
    if !h.require_cap(Capabilities::FEED_DEPOSIT_GROUPINGS, "U-55") {
        return;
    }
    let s = &h.s1;
    let clearing = clearing(h, s);
    let income = account(h, s, AccountType::Revenue, "Spring sales").id;
    let bank = detail_account(h, s, AccountType::Asset, "bank");
    // Dated apart from the other tests' waiting payments (feed date ±10 d).
    let pay = |amount: &str, day: u32| {
        post(
            h,
            s,
            &je2(
                date(2027, 3, day),
                &unique("payment"),
                clearing,
                income,
                amount,
            ),
        )
        .record
        .id
    };
    let x = pay("413.27", 12);
    let y = pay("286.19", 14);
    let imported = import_feed(
        h,
        s,
        bank,
        "Date,Description,Amount\n2027-03-16,DEPOSIT,-699.46\n",
    );
    assert_eq!(imported["imported"], 1, "{imported}");
    let item = h
        .run(s.feed().deposits_waiting())
        .expect("feed")
        .into_iter()
        .find(|d| d.amount == m("699.46") && d.date == date(2027, 3, 16))
        .expect("the bank-file deposit waits in the feed");
    let g = h
        .run(s.feed().deposit_groupings(item.id))
        .unwrap_or_else(|e| panic!("groupings: {e}"));
    assert_eq!(g.target, m("699.46"));
    let sets: Vec<Vec<TransactionId>> = g
        .candidates
        .iter()
        .map(|c| {
            let mut ids: Vec<TransactionId> = c.items.iter().map(|i| i.source).collect();
            ids.sort();
            ids
        })
        .collect();
    let mut want = vec![x, y];
    want.sort();
    assert!(sets.contains(&want), "the two payments are suggested");
    for c in &g.candidates {
        let sum =
            Money::sum(c.items.iter().filter_map(cl_api::PendingPayment::exact)).expect("sum");
        assert_eq!(sum, g.target, "every set adds up exactly");
    }
    // Deposit them so later tests don't see them waiting.
    let pending = h.run(s.payments().pending_deposits()).expect("pending");
    let mut d = DepositDraft::plain(bank, date(2027, 3, 16), None, Vec::new());
    d.payments = pending
        .items
        .iter()
        .filter(|p| p.source == x || p.source == y)
        .map(|p| p.grouped(clearing).expect("exact"))
        .collect();
    let posting = BankPosting::deposit_with(d, caps(h)).expect("deposit");
    h.run(
        s.payments()
            .create_deposit(&posting, &PostingOptions::default(), key()),
    )
    .unwrap_or_else(|e| panic!("deposit the suggestion: {e}"));
}

/// PR #159: `clearingWouldGoNegative` comes back on a create **and** on an
/// edit (`PUT`), so Make Deposit warns either way.
pub fn clearing_warning_on_create_and_edit(h: &Harness) {
    if !h.require_cap(Capabilities::DEPOSIT_GROUPING, "U-52") {
        return;
    }
    let s = &h.s1;
    let clearing = clearing(h, s);
    let bank = detail_account(h, s, AccountType::Asset, "bank");
    let held = balance(h, s, clearing);
    let over = held.checked_add(m("1.00")).expect("add");
    let deposit = |amount: Money| {
        BankPosting::deposit_with(
            DepositDraft::plain(
                bank,
                date(2027, 4, 1),
                None,
                vec![SplitLine {
                    account: clearing,
                    amount,
                    memo: None,
                    contact: None,
                    billable: false,
                    tag: None,
                }],
            ),
            caps(h),
        )
        .expect("deposit")
    };
    // An unlinked line taking more than Payments Clearing holds.
    let made = h
        .run(
            s.payments()
                .create_deposit(&deposit(over), &PostingOptions::default(), key()),
        )
        .unwrap_or_else(|e| panic!("create: {e}"));
    assert!(made.clearing_would_go_negative, "create warns");
    // Edited down to a cent: no warning.
    let fine = h
        .run(s.payments().update_deposit(
            made.saved.record.id,
            &made.saved.version,
            &deposit(m("0.01")),
            &PostingOptions::default(),
            key(),
        ))
        .unwrap_or_else(|e| panic!("edit down: {e}"));
    assert!(!fine.clearing_would_go_negative);
    // Edited back over: the edit warns too.
    let again = h
        .run(s.payments().update_deposit(
            made.saved.record.id,
            &fine.saved.version,
            &deposit(over),
            &PostingOptions::default(),
            key(),
        ))
        .unwrap_or_else(|e| panic!("edit over: {e}"));
    assert!(again.clearing_would_go_negative, "an edit warns");
    h.run(s.transactions().void(
        made.saved.record.id,
        "contract: clean up",
        &PostingOptions::default(),
        key(),
    ))
    .expect("void");
    assert_eq!(balance(h, s, clearing), held);
}
