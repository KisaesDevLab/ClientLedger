//! The P14 lane C helpers against the real engine (plan WP-C1.2, §6.2).
//!
//! - `transfers_in_both_registers` (AC2): 1010→1020 500.00 and 1010→card
//!   300.00 each show once in **both** registers with the same id and the
//!   exact amount; the balances move by exactly ±amount and the card owed
//!   drops by 300.00 (Q-241).
//! - `card_charge_and_credit`: a charge with a line customer and billable
//!   (`line-billable-v1`; without it a plain line) and a credit posted as a
//!   deposit into the card (Q-P14i) move the amount owed exactly.
//! - `personal_funds_postings` (E4): an expense paid from an equity account
//!   and one paid from a liability account; each shows in its register.
//! - `owner_money_postings`: a draw by check and by online payment, a
//!   contribution by deposit; the equity roles (`equity-roles-v1`, U-56)
//!   are PENDING without the capability.
//! - `loan_payments_match_the_lender` (AC3): three payments from
//!   `fixtures/loans/lender_statement_3mo.json` split as the Loan Payment
//!   helper splits them (`principal = payment − interest`, exact) leave the
//!   engine's loan balance at the statement's 21,906.64 to the cent.

use cl_api::{
    AccountType, BankPosting, Capabilities, CardChargeDraft, CheckDraft, ContactFields,
    ContactType, DepositDraft, EquityRole, NewContact, PostingOptions, RegisterAmount,
    RegisterQuery, Session, SplitLine, TransferDraft,
};
use cl_core::ids::{AccountId, ContactId, TransactionId};
use cl_core::{CalendarDate, Money};

use crate::common::{account, date, je2, key, m, post};
use crate::contract_bank_postings::detail_account;
use crate::support::{Harness, unique};

fn split(account: AccountId, amount: &str) -> SplitLine {
    SplitLine {
        account,
        amount: m(amount),
        memo: None,
        contact: None,
        billable: false,
        tag: None,
    }
}

/// The engine's stored balance (debits minus credits for every type).
fn balance(h: &Harness, s: &Session, a: AccountId) -> Money {
    h.run(s.accounts().get(a))
        .expect("account")
        .record
        .balance
        .unwrap_or(Money::ZERO)
}

fn moved(h: &Harness, s: &Session, a: AccountId, before: Money) -> Money {
    balance(h, s, a).checked_sub(before).expect("delta")
}

fn create(h: &Harness, s: &Session, p: &BankPosting) -> TransactionId {
    h.run(
        s.transactions()
            .create_bank_posting(p, &PostingOptions::default(), key()),
    )
    .unwrap_or_else(|e| panic!("{:?}: {e}", p.kind()))
    .record
    .id
}

fn contact(h: &Harness, s: &Session, t: ContactType, name: &str) -> ContactId {
    h.run(s.contacts().create(
        &NewContact {
            fields: ContactFields::named(t, unique(name)),
            opening_balance: Money::ZERO,
            opening_balance_date: None,
        },
        key(),
    ))
    .unwrap_or_else(|e| panic!("create contact: {e}"))
    .record
    .id
}

/// The rows of `id` in `account`'s register on `on`, with the amount as
/// the register shows it (payment or deposit column, 2 dp).
fn register_rows(
    h: &Harness,
    s: &Session,
    account: AccountId,
    on: CalendarDate,
    id: TransactionId,
) -> Vec<String> {
    let q = RegisterQuery::new(on, on);
    let page = h
        .run(s.register().page(account, &q, 1, 200))
        .expect("register");
    let shown = |a: Option<&RegisterAmount>| a.and_then(|a| a.display(false));
    page.lines
        .iter()
        .filter(|l| l.txn_id == id)
        .map(|l| {
            shown(l.payment.as_ref())
                .or_else(|| shown(l.deposit.as_ref()))
                .unwrap_or_default()
        })
        .collect()
}

/// AC2: transfers match in both registers.
pub fn transfers_in_both_registers(h: &Harness) {
    let s = &h.s1;
    let bank = detail_account(h, s, AccountType::Asset, "bank");
    let savings = detail_account(h, s, AccountType::Asset, "bank");
    let card = detail_account(h, s, AccountType::Liability, "credit_card");
    let on = date(2026, 11, 4);
    // The card owes something first, so the payment lowers it.
    let expense = account(h, s, AccountType::Expense, "Helper supplies").id;
    create(
        h,
        s,
        &BankPosting::card_charge(CardChargeDraft {
            pay_from: card,
            date: date(2026, 11, 1),
            number: None,
            payee: None,
            memo: None,
            lines: vec![split(expense, "640.00")],
        })
        .expect("charge"),
    );
    let before = [bank, savings, card].map(|a| balance(h, s, a));
    let memo = unique("AC2 transfer");
    let mut ids = Vec::new();
    for (to, amount) in [(savings, "500.00"), (card, "300.00")] {
        let id = create(
            h,
            s,
            &BankPosting::transfer(TransferDraft {
                from: bank,
                to,
                date: on,
                amount: m(amount),
                memo: Some(memo.clone()),
            })
            .expect("transfer"),
        );
        for side in [bank, to] {
            assert_eq!(
                register_rows(h, s, side, on, id),
                vec![amount.to_string()],
                "the transfer shows once, with its exact amount"
            );
        }
        ids.push(id);
    }
    assert_ne!(ids[0], ids[1]);
    assert_eq!(moved(h, s, bank, before[0]), m("-800.00"));
    assert_eq!(moved(h, s, savings, before[1]), m("500.00"));
    // Stored debit-positive: the card's credit balance (owed) drops by
    // exactly 300.00 (Q-241).
    assert_eq!(moved(h, s, card, before[2]), m("300.00"));
    assert_eq!(balance(h, s, card), m("-340.00"), "owed 340.00");
}

/// A charge with a line customer + billable and a credit into the card.
pub fn card_charge_and_credit(h: &Harness) {
    let s = &h.s1;
    let card = detail_account(h, s, AccountType::Liability, "credit_card");
    let supplies = account(h, s, AccountType::Expense, "Card supplies").id;
    let fuel = account(h, s, AccountType::Expense, "Card fuel").id;
    let job = contact(h, s, ContactType::Customer, "Card job");
    let vendor = contact(h, s, ContactType::Vendor, "Card vendor");
    let billable = h.require_cap(Capabilities::LINE_BILLABLE, "line-billable-v1");
    let on = date(2026, 11, 4);
    let before = [card, supplies, fuel].map(|a| balance(h, s, a));
    let charge = BankPosting::card_charge_with(
        CardChargeDraft {
            pay_from: card,
            date: on,
            number: Some("R-1104".into()),
            payee: Some(vendor),
            memo: Some(unique("card charge")),
            lines: vec![
                SplitLine {
                    contact: billable.then_some(job),
                    billable,
                    ..split(supplies, "40.10")
                },
                split(fuel, "24.10"),
            ],
        },
        billable,
    )
    .expect("charge");
    let id = create(h, s, &charge);
    assert_eq!(register_rows(h, s, card, on, id), vec!["64.20".to_string()]);
    if billable {
        let t = h.run(s.transactions().get(id)).expect("get").record;
        assert!(
            t.lines
                .iter()
                .any(|l| l.contact_id == Some(job) && l.is_billable),
            "the line keeps its customer and billable mark"
        );
    }
    assert_eq!(moved(h, s, card, before[0]), m("-64.20"), "owed +64.20");
    assert_eq!(moved(h, s, supplies, before[1]), m("40.10"));
    assert_eq!(moved(h, s, fuel, before[2]), m("24.10"));

    // A credit (refund) is a deposit into the card (Q-P14i).
    let credit = BankPosting::deposit(DepositDraft::plain(
        card,
        on,
        Some(unique("card credit")),
        vec![SplitLine {
            contact: Some(vendor),
            ..split(supplies, "15.00")
        }],
    ))
    .expect("credit");
    let cid = create(h, s, &credit);
    assert_eq!(
        register_rows(h, s, card, on, cid),
        vec!["15.00".to_string()]
    );
    assert_eq!(moved(h, s, card, before[0]), m("-49.20"), "owed 49.20");
    assert_eq!(moved(h, s, supplies, before[1]), m("25.10"));
}

/// E4: an expense paid from an equity or a liability account.
pub fn personal_funds_postings(h: &Harness) {
    let s = &h.s1;
    let supplies = account(h, s, AccountType::Expense, "Personal supplies").id;
    let equity = account(h, s, AccountType::Equity, "Owner contributions").id;
    let due = account(h, s, AccountType::Liability, "Due to owner").id;
    let vendor = contact(h, s, ContactType::Vendor, "Office supply store");
    let on = date(2026, 11, 4);
    for (offset, amount) in [(equity, "42.18"), (due, "19.99")] {
        let before = [offset, supplies].map(|a| balance(h, s, a));
        let id = create(
            h,
            s,
            &BankPosting::card_charge(CardChargeDraft {
                pay_from: offset,
                date: on,
                number: None,
                payee: Some(vendor),
                memo: Some(unique("paid with personal funds")),
                lines: vec![split(supplies, amount)],
            })
            .expect("personal funds"),
        );
        assert_eq!(
            register_rows(h, s, offset, on, id),
            vec![amount.to_string()],
            "shows in the equity / liability register"
        );
        let neg = Money::ZERO.checked_sub(m(amount)).expect("neg");
        assert_eq!(moved(h, s, offset, before[0]), neg, "credited exactly");
        assert_eq!(moved(h, s, supplies, before[1]), m(amount));
    }
}

/// Owner draws (check, online) and a contribution (deposit); equity roles.
pub fn owner_money_postings(h: &Harness) {
    let s = &h.s1;
    let bank = detail_account(h, s, AccountType::Asset, "bank");
    let draws = account(h, s, AccountType::Equity, "Owner draws").id;
    let contributions = account(h, s, AccountType::Equity, "Owner contributions").id;
    let owner = contact(h, s, ContactType::Vendor, "Owner");
    let on = date(2026, 11, 4);
    let before = [bank, draws, contributions].map(|a| balance(h, s, a));
    let check = BankPosting::check(CheckDraft {
        bank,
        date: on,
        payee: Some(owner),
        payee_name: "Owner".into(),
        check_number: None,
        print_later: false,
        payee_address: None,
        memo: Some(unique("owner draw")),
        printed_memo: None,
        tag_ids: Vec::new(),
        draft_attachment: None,
        amount: m("250.00"),
        lines: vec![split(draws, "250.00")],
    })
    .expect("draw by check");
    create(h, s, &check);
    let online = BankPosting::card_charge(CardChargeDraft {
        pay_from: bank,
        date: on,
        number: None,
        payee: Some(owner),
        memo: Some(unique("owner draw online")),
        lines: vec![split(draws, "500.00")],
    })
    .expect("draw online");
    create(h, s, &online);
    let deposit = BankPosting::deposit(DepositDraft::plain(
        bank,
        on,
        Some(unique("owner contribution")),
        vec![SplitLine {
            contact: Some(owner),
            ..split(contributions, "1000.00")
        }],
    ))
    .expect("contribution");
    create(h, s, &deposit);
    assert_eq!(moved(h, s, bank, before[0]), m("250.00"));
    assert_eq!(moved(h, s, draws, before[1]), m("750.00"));
    assert_eq!(moved(h, s, contributions, before[2]), m("-1000.00"));

    if h.require_cap(Capabilities::EQUITY_ROLES, "U-56") {
        let roles = h.run(s.equity_roles()).expect("equity roles");
        if let Some(entity) = roles.entity_type {
            let (out, inn) = match entity {
                cl_api::EntityType::SoleProp | cl_api::EntityType::SingleMemberLlc => {
                    (EquityRole::OwnerDraw, EquityRole::OwnerContribution)
                }
                cl_api::EntityType::Partnership => {
                    (EquityRole::PartnerDraws, EquityRole::PartnerContributions)
                }
                cl_api::EntityType::SCorp | cl_api::EntityType::CCorp => (
                    EquityRole::ShareholderDistributions,
                    EquityRole::PaidInCapital,
                ),
            };
            for role in [out, inn] {
                let a = roles.account(role).expect("seeded role account");
                let acct = h.run(s.accounts().get(a)).expect("role account").record;
                assert_eq!(acct.fields.account_type, AccountType::Equity, "{role:?}");
            }
        }
    }
}

/// AC3: the loan balance ends at the lender's statement, exactly.
pub fn loan_payments_match_the_lender(h: &Harness) {
    let s = &h.s1;
    let raw = include_str!("../../../../fixtures/loans/lender_statement_3mo.json");
    let v: serde_json::Value = serde_json::from_str(raw).expect("fixture");
    let text = |v: &serde_json::Value| v.as_str().expect("string amount").to_string();
    let bank = detail_account(h, s, AccountType::Asset, "bank");
    let loan = detail_account(h, s, AccountType::Liability, "long_term_liability");
    let interest = account(h, s, AccountType::Expense, "Loan interest").id;
    let equipment = account(h, s, AccountType::Asset, "Loan equipment").id;
    let lender = contact(h, s, ContactType::Vendor, "Sample Lender");
    // Opening balance: the equipment bought with the loan.
    post(
        h,
        s,
        &je2(
            date(2026, 7, 31),
            &unique("loan opening"),
            equipment,
            loan,
            &text(&v["opening"]),
        ),
    );
    for (i, p) in v["payments"]
        .as_array()
        .expect("payments")
        .iter()
        .enumerate()
    {
        let payment = m(&text(&p["payment"]));
        let int = m(&text(&p["interest"]));
        // The helper's split: principal = payment − interest, exactly.
        let principal = payment.checked_sub(int).expect("principal");
        assert_eq!(principal, m(&text(&p["principal"])));
        let on = CalendarDate::parse_wire(p["date"].as_str().expect("date")).expect("date");
        let lines = vec![
            split(loan, &principal.to_wire()),
            split(interest, &int.to_wire()),
        ];
        let posting = if i == 0 {
            BankPosting::check(CheckDraft {
                bank,
                date: on,
                payee: Some(lender),
                payee_name: "Sample Lender".into(),
                check_number: None,
                print_later: false,
                payee_address: None,
                memo: Some(unique("loan payment")),
                printed_memo: None,
                tag_ids: Vec::new(),
                draft_attachment: None,
                amount: payment,
                lines,
            })
        } else {
            BankPosting::card_charge(CardChargeDraft {
                pay_from: bank,
                date: on,
                number: None,
                payee: Some(lender),
                memo: Some(unique("loan payment")),
                lines,
            })
        }
        .expect("payment");
        create(h, s, &posting);
        let owed = Money::ZERO.checked_sub(balance(h, s, loan)).expect("owed");
        assert_eq!(owed, m(&text(&p["balanceAfter"])), "payment {}", i + 1);
    }
    let owed = Money::ZERO.checked_sub(balance(h, s, loan)).expect("owed");
    assert_eq!(owed, m(&text(&v["closing"])));
    assert_eq!(owed, m("21906.64"), "the engine's balance is the lender's");
}
