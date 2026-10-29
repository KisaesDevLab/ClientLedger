//! Pay Bills against the live engine (P18 WP-A). The partial payment with a
//! vendor credit runs on the pinned engine; discounts (U-101), the keyed
//! pay (U-102) and check numbers now (U-103) are PENDING by capability until
//! the pin carries them.
//!
//! Money is checked exactly; each test uses its own vendors and bank account
//! so the engine's figures can be compared to the penny.

use cl_api::{
    AccountType, BillDraft, BillLineDraft, BillTerms, BillToPay, Capabilities, CheckNumbering,
    ContactFields, ContactType, CreditToApply, NewContact, PayBillsRequest, PayMethod, PayProblem,
    PayableFilter, Percent, PostingOptions, Session, TxnStatus, VendorCreditDraft, pay_problem,
};
use cl_core::ids::{AccountId, ContactId, TransactionId};
use cl_core::{CalendarDate, Money};

use crate::common::{account, date, key, m};
use crate::contract_bank_postings::detail_account;
use crate::support::{Harness, unique};

fn vendor(h: &Harness, s: &Session, name: &str) -> ContactId {
    h.run(s.contacts().create(
        &NewContact {
            fields: ContactFields::named(ContactType::Vendor, unique(name)),
            opening_balance: Money::ZERO,
            opening_balance_date: None,
        },
        key(),
    ))
    .unwrap_or_else(|e| panic!("vendor: {e}"))
    .record
    .id
}

fn bill(
    h: &Harness,
    s: &Session,
    v: ContactId,
    expense: AccountId,
    amount: &str,
    terms: Option<BillTerms>,
    due: CalendarDate,
) -> TransactionId {
    let draft = BillDraft {
        vendor: v,
        date: date(2026, 11, 1),
        vendor_invoice_number: Some(unique("INV").replace(' ', "-")),
        terms,
        due_date: Some(due),
        memo: None,
        amount_due: m(amount),
        lines: vec![BillLineDraft::expense(expense, m(amount))],
        received: true,
        draft_attachment: None,
    };
    h.run(s.bills().create(&draft, &PostingOptions::default(), key()))
        .unwrap_or_else(|e| panic!("bill: {e:?}"))
        .record
        .txn
        .id
}

fn credit(
    h: &Harness,
    s: &Session,
    v: ContactId,
    expense: AccountId,
    amount: &str,
) -> TransactionId {
    let draft = VendorCreditDraft {
        vendor: v,
        date: date(2026, 11, 2),
        vendor_invoice_number: Some(unique("CM").replace(' ', "-")),
        memo: None,
        amount: m(amount),
        lines: vec![BillLineDraft::expense(expense, m(amount))],
        draft_attachment: None,
    };
    h.run(
        s.vendor_credits()
            .create(&draft, &PostingOptions::default(), key()),
    )
    .unwrap_or_else(|e| panic!("credit: {e:?}"))
    .record
    .txn
    .id
}

fn balance(h: &Harness, s: &Session, a: AccountId) -> Money {
    h.run(s.accounts().get(a))
        .expect("account")
        .record
        .balance
        .unwrap_or(Money::ZERO)
}

fn bill_balance(h: &Harness, s: &Session, id: TransactionId) -> Money {
    h.run(s.bills().get(id))
        .expect("bill")
        .record
        .balance_due
        .unwrap_or(Money::ZERO)
}

fn request(bank: AccountId, bills: Vec<BillToPay>, credits: Vec<CreditToApply>) -> PayBillsRequest {
    PayBillsRequest {
        bank_account: bank,
        date: date(2026, 11, 4),
        bills,
        credits,
        discount_account: None,
        method: PayMethod::Check(CheckNumbering::PrintLater),
        memo: None,
    }
}

/// Partial payment with a vendor credit (pinned engine): 1,000.00 cash and
/// the 250.00 credit on a 3,915.00 bill leave 2,665.00 owed; the bank moves
/// by the cash only; the payment reads back with both applications; a void
/// restores the bill.
pub fn partial_payment_with_credit_live(h: &Harness) {
    let s = &h.s1;
    let expense = account(h, s, AccountType::Expense, "Nursery stock");
    let bank = detail_account(h, s, AccountType::Asset, "bank");
    let v = vendor(h, s, "Table Rock Nursery");
    let b = bill(h, s, v, expense.id, "3915.00", None, date(2026, 11, 11));
    let c = credit(h, s, v, expense.id, "250.00");
    let list = h
        .run(s.bills().payable_list(&PayableFilter {
            vendor: Some(v),
            ..PayableFilter::default()
        }))
        .expect("payable");
    let row = list.bills.iter().find(|r| r.id == b).expect("listed");
    assert_eq!(row.balance_due, m("3915.00"));
    let cr = list
        .credits
        .iter()
        .find(|r| r.id == c)
        .expect("credit listed");
    assert_eq!(cr.balance_due, m("250.00"));

    let before = balance(h, s, bank);
    let paid = h
        .run(s.bill_payments().pay(
            &request(
                bank,
                vec![BillToPay {
                    bill: b,
                    cash: m("1000.00"),
                    discount: Money::ZERO,
                }],
                vec![CreditToApply {
                    credit: c,
                    bill: b,
                    amount: m("250.00"),
                }],
            ),
            &PostingOptions::default(),
            key(),
        ))
        .unwrap_or_else(|e| panic!("pay: {e:?}"));
    assert_eq!(paid.len(), 1, "one check for one vendor");
    assert_eq!(paid[0].net_payment, m("1000.00"));
    assert_eq!(paid[0].print_status.as_deref(), Some("queue"));
    assert_eq!(bill_balance(h, s, b), m("2665.00"), "partial payment");
    assert_eq!(
        before.checked_sub(balance(h, s, bank)).expect("delta"),
        m("1000.00"),
        "the bank moves by the cash"
    );
    let rec = h.run(s.bill_payments().get(paid[0].id)).expect("get");
    assert_eq!(rec.status, TxnStatus::Posted);
    assert_eq!(rec.bill_applications.len(), 1);
    assert_eq!(rec.bill_applications[0].amount, m("1000.00"), "cash");
    assert_eq!(rec.credit_applications.len(), 1);
    assert_eq!(rec.credit_applications[0].amount, m("250.00"));

    // Paying more than is left is the uncoded refusal Q-P18g reads as
    // "the bills changed".
    let e = h
        .run(s.bill_payments().pay(
            &request(
                bank,
                vec![BillToPay {
                    bill: b,
                    cash: m("2665.01"),
                    discount: Money::ZERO,
                }],
                Vec::new(),
            ),
            &PostingOptions::default(),
            key(),
        ))
        .expect_err("over the balance");
    assert_eq!(pay_problem(&e), Some(PayProblem::BillsChanged), "{e:?}");

    h.run(
        s.ap_voids()
            .bill_payment(paid[0].id, "contract clean-up", key()),
    )
    .expect("void");
    assert_eq!(
        bill_balance(h, s, b),
        m("3915.00"),
        "void restores the bill"
    );
}

/// A card payment (pinned engine; owner answer 2026-10-28): the bill is
/// paid from a credit card account, the card's balance owed goes up by the
/// amount, no check number and nothing to print.
pub fn card_payment_live(h: &Harness) {
    let s = &h.s1;
    let expense = account(h, s, AccountType::Expense, "Card paid bill");
    let card = detail_account(h, s, AccountType::Liability, "credit_card");
    let v = vendor(h, s, "Monett Tire & Lube");
    let b = bill(h, s, v, expense.id, "640.00", None, date(2026, 11, 8));
    let before = balance(h, s, card);
    let mut req = request(
        card,
        vec![BillToPay {
            bill: b,
            cash: m("640.00"),
            discount: Money::ZERO,
        }],
        Vec::new(),
    );
    req.method = PayMethod::CreditCard {
        reference: Some("AUTH-77".into()),
    };
    let paid = h
        .run(
            s.bill_payments()
                .pay(&req, &PostingOptions::default(), key()),
        )
        .unwrap_or_else(|e| panic!("pay: {e:?}"));
    assert_eq!(paid.len(), 1);
    assert_eq!(paid[0].check_number, None);
    assert_ne!(paid[0].print_status.as_deref(), Some("queue"));
    assert_eq!(bill_balance(h, s, b), Money::ZERO);
    // Credit-normal: the stored balance (debits - credits) falls by 640.00,
    // i.e. the amount owed on the card rises by it.
    assert_eq!(
        before.checked_sub(balance(h, s, card)).expect("delta"),
        m("640.00")
    );
}

/// U-101: the mockup's run to the penny. Four bills (1,286.40, 1,412.06,
/// 218.44 at 2% to 11/06, 3,915.00) and the 250.00 credit: discount 4.37,
/// credits 250.00, cash 6,577.53; the bank moves by the cash and the
/// discount account by the discount.
pub fn mockup_run_with_discount_u101(h: &Harness) {
    if !h.require_cap(Capabilities::BILL_DISCOUNTS, "U-101") {
        return;
    }
    let s = &h.s1;
    let expense = account(h, s, AccountType::Expense, "Pay bills supplies");
    let disc = account(h, s, AccountType::OtherRevenue, "Discounts taken");
    let bank = detail_account(h, s, AccountType::Asset, "bank");
    let feed = vendor(h, s, "Ozark Feed & Seed");
    let fuel = vendor(h, s, "Riverbend Fuel");
    let office = vendor(h, s, "Ozark Office Supply");
    let nursery = vendor(h, s, "Table Rock Nursery");
    let b1 = bill(h, s, feed, expense.id, "1286.40", None, date(2026, 11, 5));
    let b2 = bill(h, s, fuel, expense.id, "1412.06", None, date(2026, 11, 6));
    let terms = BillTerms {
        code: "custom".into(),
        days: Some(15),
        discount_percent: Percent::parse("2").ok(),
        discount_days: Some(5),
    };
    let b3 = bill(
        h,
        s,
        office,
        expense.id,
        "218.44",
        Some(terms),
        date(2026, 11, 16),
    );
    let b4 = bill(
        h,
        s,
        nursery,
        expense.id,
        "3915.00",
        None,
        date(2026, 11, 11),
    );
    let c = credit(h, s, nursery, expense.id, "250.00");
    let list = h
        .run(s.bills().payable_list(&PayableFilter {
            payment_date: Some(date(2026, 11, 4)),
            ..PayableFilter::default()
        }))
        .expect("payable");
    let r3 = list.bills.iter().find(|r| r.id == b3).expect("7781");
    assert_eq!(
        r3.discount_available,
        Some(m("4.37")),
        "the engine's discount"
    );
    let r1 = list.bills.iter().find(|r| r.id == b1).expect("20931");
    assert_eq!(r1.discount_available, Some(Money::ZERO));
    let before = balance(h, s, bank);
    let mut req = request(
        bank,
        vec![
            BillToPay {
                bill: b1,
                cash: m("1286.40"),
                discount: Money::ZERO,
            },
            BillToPay {
                bill: b2,
                cash: m("1412.06"),
                discount: Money::ZERO,
            },
            BillToPay {
                bill: b3,
                cash: m("214.07"),
                discount: m("4.37"),
            },
            BillToPay {
                bill: b4,
                cash: m("3665.00"),
                discount: Money::ZERO,
            },
        ],
        vec![CreditToApply {
            credit: c,
            bill: b4,
            amount: m("250.00"),
        }],
    );
    req.discount_account = Some(disc.id);
    let paid = h
        .run(
            s.bill_payments()
                .pay(&req, &PostingOptions::default(), key()),
        )
        .unwrap_or_else(|e| panic!("pay: {e:?}"));
    assert_eq!(paid.len(), 4, "one check per vendor");
    let cash = Money::sum(paid.iter().map(|p| p.net_payment)).expect("sum");
    assert_eq!(cash, m("6577.53"));
    assert_eq!(
        before.checked_sub(balance(h, s, bank)).expect("delta"),
        m("6577.53")
    );
    assert_eq!(
        balance(h, s, disc.id).abs(),
        m("4.37"),
        "the discount posts"
    );
    for b in [b1, b2, b3, b4] {
        assert_eq!(bill_balance(h, s, b), Money::ZERO);
    }
}

/// U-102: the same key twice makes one set of payments.
pub fn keyed_pay_u102(h: &Harness) {
    if !h.require_cap(Capabilities::IDEMPOTENCY_PAY_BILLS, "U-102") {
        return;
    }
    let s = &h.s1;
    let expense = account(h, s, AccountType::Expense, "Keyed pay");
    let bank = detail_account(h, s, AccountType::Asset, "bank");
    let v = vendor(h, s, "Monett Tire & Lube");
    let b = bill(h, s, v, expense.id, "640.00", None, date(2026, 11, 8));
    let req = request(
        bank,
        vec![BillToPay {
            bill: b,
            cash: m("640.00"),
            discount: Money::ZERO,
        }],
        Vec::new(),
    );
    let k = key();
    let first = h
        .run(
            s.bill_payments()
                .pay(&req, &PostingOptions::default(), k.clone()),
        )
        .expect("pay");
    let again = h
        .run(s.bill_payments().pay(&req, &PostingOptions::default(), k))
        .expect("replay");
    assert_eq!(first[0].id, again[0].id, "replayed, not paid twice");
    assert_eq!(bill_balance(h, s, b), Money::ZERO);
}

/// U-103: numbers now, one per vendor in order; a used number is refused.
pub fn check_numbers_now_u103(h: &Harness) {
    if !h.require_cap(Capabilities::BILL_PAYMENT_CHECK_NUMBER, "U-103") {
        return;
    }
    let s = &h.s1;
    let expense = account(h, s, AccountType::Expense, "Numbered pay");
    let bank = detail_account(h, s, AccountType::Asset, "bank");
    let v1 = vendor(h, s, "Numbered one");
    let v2 = vendor(h, s, "Numbered two");
    let b1 = bill(h, s, v1, expense.id, "100.00", None, date(2026, 11, 8));
    let b2 = bill(h, s, v2, expense.id, "200.00", None, date(2026, 11, 8));
    let mut req = request(
        bank,
        vec![
            BillToPay {
                bill: b1,
                cash: m("100.00"),
                discount: Money::ZERO,
            },
            BillToPay {
                bill: b2,
                cash: m("200.00"),
                discount: Money::ZERO,
            },
        ],
        Vec::new(),
    );
    req.method = PayMethod::Check(CheckNumbering::Assign { first: 7001 });
    let paid = h
        .run(
            s.bill_payments()
                .pay(&req, &PostingOptions::default(), key()),
        )
        .unwrap_or_else(|e| panic!("pay: {e:?}"));
    let mut numbers: Vec<u64> = paid.iter().filter_map(|p| p.check_number).collect();
    numbers.sort_unstable();
    assert_eq!(numbers, vec![7001, 7002]);
    let b3 = bill(h, s, v1, expense.id, "50.00", None, date(2026, 11, 8));
    let mut again = request(
        bank,
        vec![BillToPay {
            bill: b3,
            cash: m("50.00"),
            discount: Money::ZERO,
        }],
        Vec::new(),
    );
    again.method = PayMethod::Check(CheckNumbering::Assign { first: 7002 });
    let e = h
        .run(
            s.bill_payments()
                .pay(&again, &PostingOptions::default(), key()),
        )
        .expect_err("used");
    assert_eq!(pay_problem(&e), Some(PayProblem::CheckNumberInUse), "{e:?}");
}
