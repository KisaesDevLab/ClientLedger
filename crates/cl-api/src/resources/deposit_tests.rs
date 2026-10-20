//! P14 deposits (grouped payments, fees, cash back, vendor refunds) and
//! card-charge line customers: local refusals, exact string bodies and the
//! fingerprint of the record the engine stores (goldens
//! `fixtures/engine/deposit-grouped*.json`, shaped by the engine's
//! `deposit.service` planDeposit at PR #159).

use super::*;
use crate::resources::transactions::LINE_ROLE_CASH_BACK;
use crate::resources::versioning::EditableFields;
use proptest::prelude::*;

const GROUPED: &str = include_str!("../../../../fixtures/engine/deposit-grouped.json");
const FEES: &str = include_str!("../../../../fixtures/engine/deposit-grouped-fees.json");
const REFUND: &str = include_str!("../../../../fixtures/engine/deposit-grouped-refund.json");

const BANK: &str = "2a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const CLEARING: &str = "2d0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const INCOME: &str = "2e0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const FEE: &str = "2f0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const DRAW: &str = "2a1c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const AP: &str = "2b1c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const PAY1: &str = "7d0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const PAY2: &str = "7d0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a02";
const CREDIT: &str = "7d0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a03";
const CUST1: &str = "4a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const CUST2: &str = "4c0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const VENDOR: &str = "4d0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";

fn m(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}

fn a(s: &str) -> AccountId {
    AccountId::parse(s).expect("id")
}

fn t(s: &str) -> TransactionId {
    TransactionId::parse(s).expect("id")
}

fn c(s: &str) -> ContactId {
    ContactId::parse(s).expect("id")
}

fn day() -> CalendarDate {
    CalendarDate::from_ymd(2026, 11, 4).expect("date")
}

fn record(text: &str) -> Transaction {
    let v: Value = serde_json::from_str(text).expect("golden json");
    serde_json::from_value(v["transaction"].clone()).expect("transaction")
}

fn all() -> DepositCaps {
    DepositCaps {
        grouping: true,
        cashback: true,
        vendor_refund: true,
    }
}

fn payment(source: &str, amount: &str, contact: &str) -> GroupedPayment {
    GroupedPayment {
        source: t(source),
        kind: DepositSource::Payment,
        account: a(CLEARING),
        amount: m(amount),
        contact: Some(c(contact)),
        memo: None,
    }
}

fn line(account: &str, amount: &str, memo: Option<&str>) -> SplitLine {
    SplitLine {
        account: a(account),
        amount: m(amount),
        memo: memo.map(str::to_string),
        contact: None,
        billable: false,
        tag: None,
    }
}

/// AC1: Hollister 1,250.00 and Maple Street HOA 842.50 into 1010.
fn grouped() -> DepositDraft {
    DepositDraft {
        payments: vec![
            payment(PAY1, "1250.00", CUST1),
            payment(PAY2, "842.5", CUST2),
        ],
        ..DepositDraft::plain(a(BANK), day(), None, Vec::new())
    }
}

/// One payment, another 300.00, a 12.50 card fee and 50.00 cash back.
fn with_fees() -> DepositDraft {
    DepositDraft {
        account: a(BANK),
        date: day(),
        memo: Some("Saturday counter".into()),
        payments: vec![payment(PAY1, "1250", CUST1)],
        lines: vec![line(INCOME, "300", Some("Plant sale"))],
        deductions: vec![line(FEE, "12.50", Some("Card fee"))],
        cash_back: Some(CashBack {
            account: a(DRAW),
            amount: m("50"),
            memo: Some("Petty cash".into()),
        }),
    }
}

fn refund() -> DepositDraft {
    DepositDraft {
        payments: vec![GroupedPayment {
            source: t(CREDIT),
            kind: DepositSource::VendorCredit,
            account: a(AP),
            amount: m("40"),
            contact: Some(c(VENDOR)),
            memo: Some("Refund of credit".into()),
        }],
        ..DepositDraft::plain(a(BANK), day(), None, Vec::new())
    }
}

#[test]
fn a_grouped_deposit_is_exactly_what_the_engine_stores() {
    let d = grouped();
    assert_eq!(d.total(), Some(m("2092.50")));
    let p = BankPosting::deposit_with(d, all()).expect("deposit");
    let rec = record(GROUPED);
    assert_eq!(
        rec.total,
        Some(m("2092.50")),
        "header total is the bank amount"
    );
    assert_eq!(rec.fingerprint(), p.created_fingerprint());
    assert_eq!(rec.lines[1].source_transaction_id, Some(t(PAY1)));

    let v = p.create_value();
    assert_eq!(v["txnType"], "deposit");
    assert_eq!(v["depositToAccountId"], BANK);
    assert_eq!(v["lines"][0]["accountId"], CLEARING);
    assert_eq!(v["lines"][0]["amount"], "1250.0000");
    assert_eq!(v["lines"][0]["sourceTransactionId"], PAY1);
    assert_eq!(v["lines"][0]["contactId"], CUST1);
    assert_eq!(v["lines"][1]["amount"], "842.5000");
    assert_eq!(v["lines"][1]["sourceTransactionId"], PAY2);
    assert!(v.get("cashBack").is_none());
}

#[test]
fn a_swapped_source_changes_the_fingerprint() {
    let base = BankPosting::deposit_with(grouped(), all()).expect("deposit");
    let mut d = grouped();
    d.payments[0].source = t(CREDIT);
    let other = BankPosting::deposit_with(d, all()).expect("deposit");
    assert_ne!(base.created_fingerprint(), other.created_fingerprint());
}

#[test]
fn fees_and_cash_back_post_exactly() {
    let d = with_fees();
    // 1,250.00 + 300.00 − 12.50 − 50.00
    assert_eq!(d.total(), Some(m("1487.50")));
    let p = BankPosting::deposit_with(d, all()).expect("deposit");
    let rec = record(FEES);
    assert_eq!(rec.total, Some(m("1487.50")));
    assert_eq!(rec.fingerprint(), p.created_fingerprint());
    assert_eq!(rec.lines[4].line_role.as_deref(), Some(LINE_ROLE_CASH_BACK));

    let v = p.create_value();
    let lines = v["lines"].as_array().expect("lines");
    assert_eq!(lines.len(), 3, "cash back is not a line");
    assert_eq!(lines[1]["amount"], "300.0000");
    assert!(lines[1].get("sourceTransactionId").is_none());
    assert_eq!(lines[2]["accountId"], FEE);
    assert_eq!(lines[2]["amount"], "-12.5000", "a fee goes out negative");
    assert_eq!(lines[2]["description"], "Card fee");
    assert_eq!(v["cashBack"]["accountId"], DRAW);
    assert_eq!(v["cashBack"]["amount"], "50.0000");
    assert_eq!(v["cashBack"]["memo"], "Petty cash");
    assert_eq!(v["memo"], "Saturday counter");
}

#[test]
fn a_vendor_refund_names_the_vendor_on_the_payables_line() {
    let p = BankPosting::deposit_with(refund(), all()).expect("refund");
    assert_eq!(record(REFUND).fingerprint(), p.created_fingerprint());
    let v = p.create_value();
    assert_eq!(v["lines"][0]["accountId"], AP);
    assert_eq!(v["lines"][0]["sourceTransactionId"], CREDIT);
    assert_eq!(v["lines"][0]["contactId"], VENDOR);
    assert_eq!(v["lines"][0]["amount"], "40.0000");
}

#[test]
fn every_part_needs_its_capability() {
    use LocalValidation as L;
    let none = DepositCaps::default();
    assert_eq!(
        BankPosting::deposit(grouped()),
        Err(L::DepositGroupingUnavailable)
    );
    assert_eq!(
        BankPosting::deposit_with(
            grouped(),
            DepositCaps {
                grouping: false,
                ..all()
            }
        ),
        Err(L::DepositGroupingUnavailable)
    );
    assert_eq!(
        BankPosting::deposit_with(
            refund(),
            DepositCaps {
                vendor_refund: false,
                ..all()
            }
        ),
        Err(L::VendorRefundUnavailable)
    );
    // Grouping alone does not open fees or cash back.
    let only_grouping = DepositCaps {
        grouping: true,
        ..none
    };
    assert_eq!(
        BankPosting::deposit_with(with_fees(), only_grouping),
        Err(L::DepositCashBackUnavailable)
    );
    let mut d = with_fees();
    d.cash_back = None;
    assert_eq!(
        BankPosting::deposit_with(d, only_grouping),
        Err(L::DepositCashBackUnavailable),
        "a fee alone"
    );
    let mut d = with_fees();
    d.deductions.clear();
    assert_eq!(
        BankPosting::deposit_with(d, only_grouping),
        Err(L::DepositCashBackUnavailable),
        "cash back alone"
    );
    // Plain lines need nothing.
    let plain = DepositDraft::plain(a(BANK), day(), None, vec![line(INCOME, "300", None)]);
    assert!(BankPosting::deposit_with(plain, none).is_ok());
}

#[test]
fn bad_deposits_are_refused_locally() {
    use LocalValidation as L;
    let refuse = |d: DepositDraft| BankPosting::deposit_with(d, all()).err();
    // Nothing to deposit.
    assert_eq!(
        refuse(DepositDraft::plain(a(BANK), day(), None, Vec::new())),
        Some(L::TooFewLines)
    );
    let mut d = with_fees();
    d.lines.clear();
    d.payments.clear();
    d.cash_back = None;
    // A fee alone: a part, but the total is −12.50.
    assert_eq!(refuse(d), Some(L::NonPositiveAmount));
    // Too many parts (cash back counts).
    let mut d = with_fees();
    d.lines = vec![line(INCOME, "1", None); MAX_LINES - 2];
    assert_eq!(refuse(d.clone()), Some(L::TooManyLines));
    d.cash_back = None;
    assert!(BankPosting::deposit_with(d, all()).is_ok(), "exactly 500");
    // A part that is not greater than zero.
    let mut d = grouped();
    d.payments[1].amount = Money::ZERO;
    assert_eq!(refuse(d), Some(L::NonPositiveAmount));
    let mut d = with_fees();
    d.deductions[0].amount = m("-12.50");
    assert_eq!(refuse(d), Some(L::NonPositiveAmount), "fees are positive");
    let mut d = with_fees();
    if let Some(cb) = &mut d.cash_back {
        cb.amount = Money::ZERO;
    }
    assert_eq!(refuse(d), Some(L::NonPositiveAmount));
    // Cash back that eats the whole deposit, and one cent more.
    let mut d = with_fees();
    if let Some(cb) = &mut d.cash_back {
        cb.amount = m("1537.50");
    }
    assert_eq!(refuse(d.clone()), Some(L::NonPositiveAmount), "total 0.00");
    if let Some(cb) = &mut d.cash_back {
        cb.amount = m("1537.49");
    }
    let ok = BankPosting::deposit_with(d.clone(), all()).expect("0.01 left");
    assert_eq!(d.total(), Some(m("0.01")));
    assert_eq!(ok.create_value()["cashBack"]["amount"], "1537.4900");
    // The deposit account on any side.
    let mut d = grouped();
    d.payments[0].account = a(BANK);
    assert_eq!(refuse(d), Some(L::SameAccount));
    let mut d = with_fees();
    d.deductions[0].account = a(BANK);
    assert_eq!(refuse(d), Some(L::SameAccount));
    let mut d = with_fees();
    if let Some(cb) = &mut d.cash_back {
        cb.account = a(BANK);
    }
    assert_eq!(refuse(d), Some(L::SameAccount), "cash back to the bank");
    // The same source twice.
    let mut d = grouped();
    d.payments[1].source = d.payments[0].source;
    assert_eq!(refuse(d), Some(L::InvalidValue));
    // A vendor credit without its vendor.
    let mut d = refund();
    d.payments[0].contact = None;
    assert_eq!(refuse(d), Some(L::InvalidValue));
    // Billable has no meaning on a deposit.
    let mut d = with_fees();
    d.lines[0].billable = true;
    d.lines[0].contact = Some(c(CUST1));
    assert_eq!(refuse(d), Some(L::InvalidValue));
    let mut d = with_fees();
    d.deductions[0].billable = true;
    assert_eq!(refuse(d), Some(L::InvalidValue));
    // A cash-back memo over 1000 characters; blank ones are dropped.
    let mut d = with_fees();
    if let Some(cb) = &mut d.cash_back {
        cb.memo = Some("m".repeat(CASH_BACK_MEMO_MAX + 1));
    }
    assert_eq!(refuse(d.clone()), Some(L::InvalidValue));
    if let Some(cb) = &mut d.cash_back {
        cb.memo = Some("   ".into());
    }
    let p = BankPosting::deposit_with(d, all()).expect("blank memo");
    assert!(p.create_value()["cashBack"].get("memo").is_none());
    // An overflowing sum.
    let mut d = grouped();
    d.payments[0].amount = m("999999999999999.9999");
    d.payments[1].amount = m("999999999999999.9999");
    assert_eq!(refuse(d), Some(L::InvalidValue));
}

/// No amount anywhere in a deposit body is a JSON number, and the P12
/// plain body carries none of the P14 keys.
#[test]
fn bodies_send_money_only_as_strings() {
    fn numbers(v: &Value, path: &str, out: &mut Vec<String>) {
        match v {
            Value::Number(_) => out.push(path.to_string()),
            Value::Object(o) => o
                .iter()
                .for_each(|(k, v)| numbers(v, &format!("{path}.{k}"), out)),
            Value::Array(a) => a
                .iter()
                .enumerate()
                .for_each(|(i, v)| numbers(v, &format!("{path}[{i}]"), out)),
            _ => {}
        }
    }
    for d in [grouped(), with_fees(), refund()] {
        let p = BankPosting::deposit_with(d, all()).expect("deposit");
        for body in [p.create_value(), p.update_value()] {
            let mut found = Vec::new();
            numbers(&body, "", &mut found);
            assert!(found.is_empty(), "JSON numbers at {found:?}");
            for bad in ["overrideConfirmed", "lockDate", "closingPassword"] {
                assert!(!body.to_string().contains(bad), "{bad}");
            }
        }
    }
    let plain = BankPosting::deposit(DepositDraft::plain(
        a(BANK),
        day(),
        None,
        vec![line(INCOME, "300", None)],
    ))
    .expect("plain")
    .create_value()
    .to_string();
    for key in ["sourceTransactionId", "cashBack", "-"] {
        assert!(!plain.contains(&format!("\"{key}")), "{key} in {plain}");
    }
}

#[test]
fn debug_shows_no_amounts_or_memos() {
    let text = format!("{:?}", with_fees());
    for secret in [
        "1250",
        "300",
        "12.5",
        "50.00",
        "Plant sale",
        "Card fee",
        "Petty",
    ] {
        assert!(!text.contains(secret), "{secret} in {text}");
    }
    assert!(text.contains("DepositDraft"));
}

#[test]
fn caps_follow_the_engine() {
    let caps: Capabilities = [Capabilities::DEPOSIT_GROUPING, Capabilities::VENDOR_REFUND]
        .into_iter()
        .collect();
    assert_eq!(
        DepositCaps::from_caps(&caps),
        DepositCaps {
            grouping: true,
            cashback: false,
            vendor_refund: true,
        }
    );
    assert_eq!(
        DepositCaps::from_caps(&Capabilities::default()),
        DepositCaps::default()
    );
}

#[test]
fn a_card_line_carries_its_customer_and_billable_mark() {
    let draft = CardChargeDraft {
        pay_from: a(BANK),
        date: day(),
        number: None,
        payee: None,
        memo: None,
        lines: vec![
            SplitLine {
                contact: Some(c(CUST1)),
                billable: true,
                ..line(INCOME, "40.00", Some("Mulch"))
            },
            line(FEE, "24.20", Some("Gloves")),
        ],
    };
    let p = BankPosting::card_charge_with(draft, true).expect("card");
    let v = p.create_value();
    assert_eq!(v["lines"][0]["contactId"], CUST1);
    assert_eq!(v["lines"][0]["isBillable"], true);
    assert!(v["lines"][1].get("contactId").is_none());
    assert!(v["lines"][1].get("isBillable").is_none());
    let stored: Transaction = serde_json::from_value(serde_json::json!({
        "id": PAY1, "txnType": "expense", "txnDate": "2026-11-04", "status": "posted",
        "basis": "both", "contactId": null, "memo": null,
        "lines": [
            {"id": "8e0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a00", "accountId": INCOME, "debit": "40.0000",
             "credit": "0.0000", "description": "Mulch", "lineOrder": 0, "tagId": null,
             "contactId": CUST1, "isBillable": true, "billedStatus": "unbilled"},
            {"id": "8e0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01", "accountId": FEE, "debit": "24.2000",
             "credit": "0.0000", "description": "Gloves", "lineOrder": 1, "tagId": null,
             "contactId": null, "isBillable": false},
            {"id": "8e0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a02", "accountId": BANK, "debit": "0.0000",
             "credit": "64.2000", "description": null, "lineOrder": 2, "tagId": null,
             "contactId": null, "isBillable": false}
        ]
    }))
    .expect("stored");
    assert_eq!(stored.fingerprint(), p.created_fingerprint());
}

fn cents(n: i64) -> Money {
    Money::from_cents(n).expect("cents")
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// The bank total is `Σpayments + Σlines − Σdeductions − cash back`
    /// to the cent; the builder takes a deposit iff that is above zero; the
    /// fees go out as exactly their negatives; and the record the engine
    /// would store has the total on its bank line.
    #[test]
    fn the_bank_total_is_exact(
        pays in proptest::collection::vec(1i64..=500_000_000, 0..4),
        others in proptest::collection::vec(1i64..=500_000_000, 0..4),
        fees in proptest::collection::vec(1i64..=50_000_000, 0..3),
        cash in proptest::option::of(1i64..=500_000_000),
    ) {
        let d = DepositDraft {
            account: a(BANK),
            date: day(),
            memo: None,
            payments: pays
                .iter()
                .enumerate()
                .map(|(i, p)| GroupedPayment {
                    source: TransactionId::parse(&format!("7f0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a{i:02}"))
                        .expect("id"),
                    amount: cents(*p),
                    ..payment(PAY1, "1", CUST1)
                })
                .collect(),
            lines: others.iter().map(|o| SplitLine { amount: cents(*o), ..line(INCOME, "1", None) }).collect(),
            deductions: fees.iter().map(|f| SplitLine { amount: cents(*f), ..line(FEE, "1", None) }).collect(),
            cash_back: cash.map(|c| CashBack { account: a(DRAW), amount: cents(c), memo: None }),
        };
        let expected = pays.iter().sum::<i64>() + others.iter().sum::<i64>()
            - fees.iter().sum::<i64>() - cash.unwrap_or(0);
        prop_assert_eq!(d.total(), Some(cents(expected)));
        let built = BankPosting::deposit_with(d.clone(), all());
        let parts = pays.len() + others.len() + fees.len();
        prop_assert_eq!(built.is_ok(), parts > 0 && expected > 0);
        if let Ok(p) = built {
            let v = p.create_value();
            let sent = v["lines"].as_array().expect("lines");
            for (i, f) in fees.iter().enumerate() {
                let wire = &sent[pays.len() + others.len() + i]["amount"];
                let negative = cents(-*f).to_wire();
                prop_assert_eq!(wire.as_str(), Some(negative.as_str()));
            }
            let journal = p.editable_value(false);
            let bank = cents(expected).to_wire();
            prop_assert_eq!(journal["lines"][0]["debit"].as_str(), Some(bank.as_str()));
        }
    }
}
