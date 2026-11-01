//! The demo receivables engine: every golden case of
//! `fixtures/payments/golden.json` (1-9, 11), the story seed, and
//! properties of the allocation and of void and bounce.

use std::str::FromStr;

use cl_api::{
    ApplicationRequest, ApplyInvoice, ApplyRequest, BounceRequest, DepositDraft, DepositSource,
    GroupedPayment, NewCustomerPayment, PaymentPreviewRequest, SaveApplication,
};
use proptest::prelude::*;
use serde_json::Value;

use super::*;

fn golden() -> Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/payments/golden.json");
    serde_json::from_slice(&std::fs::read(path).expect("golden file")).expect("json")
}

fn m(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}

fn d(s: &str) -> CalendarDate {
    CalendarDate::parse_wire(s).expect("date")
}

fn s(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_string()
}

/// One golden case run through the demo engine.
struct Run {
    state: DemoState,
    client: ContactId,
    invoices: BTreeMap<String, TransactionId>,
    payment: Option<TransactionId>,
    other_payment: Option<TransactionId>,
    deposit: Option<TransactionId>,
    operating_before: Decimal,
    next: u32,
}

fn role(r: &mut Run, name: &str) -> AccountId {
    match name {
        "undeposited_funds" => clearing(&mut r.state).expect("clearing"),
        "accounts_receivable" => super::super::account_id(AR),
        "operating" => super::super::account_id(OPERATING),
        "write_off" => super::super::account_id(BAD_DEBTS),
        "sales_discount" => super::super::account_id(SALES_DISCOUNTS),
        "customer_deposits" => customer_deposits(&mut r.state),
        "bank_fee" => super::super::account_id(6050),
        other => panic!("role {other}"),
    }
}

fn number_of(r: &mut Run, a: AccountId) -> String {
    chart(&mut r.state)
        .iter()
        .find(|x| x.id == a)
        .and_then(|x| x.fields.account_number.clone())
        .expect("in chart")
}

fn account_total(r: &mut Run, a: AccountId) -> Decimal {
    let n = number_of(r, a);
    let ledger = &r.state.ledger;
    ledger
        .transactions()
        .iter()
        .filter(|t| !ledger.is_void(DemoLedger::id_of(t)))
        .flat_map(|t| t.lines.iter())
        .filter(|l| l.account == n)
        .map(|l| l.debit.as_decimal() - l.credit.as_decimal())
        .sum()
}

fn new_run(case: &Value) -> Run {
    let mut state = DemoState::new();
    seed(&mut state);
    let contacts = state.contacts_mut();
    let base = u32::try_from(contacts.len()).expect("n") + 100;
    for (i, name) in ["Golden Client", "Golden Other"].iter().enumerate() {
        contacts.push(cl_api::Contact {
            id: super::super::contacts::contact_id(base + u32::try_from(i).expect("i")),
            fields: cl_api::ContactFields::named(cl_api::ContactType::Customer, *name),
            is_active: true,
            opening_balance: Some(Money::ZERO),
            opening_balance_date: None,
            created_at: None,
            updated_at: None,
        });
    }
    let client = super::super::contacts::contact_id(base);
    let other = super::super::contacts::contact_id(base + 1);
    state.receivables.settings = DemoReceivablesSettings {
        discount_account: Some(super::super::account_id(SALES_DISCOUNTS)),
        write_off_account: Some(super::super::account_id(BAD_DEBTS)),
        write_off_threshold: case["company"]["writeOffThreshold"].as_str().map(m),
        nsf_bank_fee_account: Some(super::super::account_id(6050)),
        nsf_fee_income_account: None,
        nsf_customer_fee: None,
    };
    let mut r = Run {
        state,
        client,
        invoices: BTreeMap::new(),
        payment: None,
        other_payment: None,
        deposit: None,
        operating_before: Decimal::ZERO,
        next: 0,
    };
    for inv in case["invoices"].as_array().into_iter().flatten() {
        add_invoice(&mut r, inv, client);
    }
    if let Some(amount) = case["otherPending"].as_str() {
        let id = add_invoice(
            &mut r,
            &serde_json::json!({"number": "9999", "date": "2026-10-01", "dueDate": "2026-10-31", "total": amount}),
            other,
        );
        let p = save(
            &mut r,
            other,
            "2026-11-03",
            amount,
            &[(id, amount.to_string(), None, None)],
            false,
        );
        r.other_payment = Some(receive(&mut r.state, &p, None, None).expect("other").id);
    }
    let operating = super::super::account_id(OPERATING);
    r.operating_before = account_total(&mut r, operating);
    r
}

fn add_invoice(r: &mut Run, inv: &Value, client: ContactId) -> TransactionId {
    r.next += 1;
    let id = TransactionId::parse(&format!("d5000000-0000-4000-8000-{:012}", r.next)).expect("id");
    let terms = inv["discountPercent"].as_str().map(|p| {
        (
            Decimal::from_str(p).expect("pct"),
            inv["discountDays"].as_i64().expect("days"),
        )
    });
    seed_invoice_with_terms(
        &mut r.state,
        id,
        client,
        &s(&inv["number"]),
        (d(&s(&inv["date"])), d(&s(&inv["dueDate"]))),
        m(&s(&inv["total"])),
        terms,
    )
    .expect("seed");
    r.invoices.insert(s(&inv["number"]), id);
    id
}

type Line = (TransactionId, String, Option<String>, Option<String>);

/// A save as the screen builds it: the preview of the same applications
/// gives each invoice's token.
fn save(
    r: &mut Run,
    client: ContactId,
    date: &str,
    amount: &str,
    apps: &[Line],
    hold: bool,
) -> NewCustomerPayment {
    let clearing = clearing(&mut r.state).expect("clearing");
    let req = PaymentPreviewRequest {
        customer_id: client,
        date: d(date),
        amount: Some(m(amount)),
        deposit_to: Some(clearing),
        applications: Some(
            apps.iter()
                .map(|(i, a, disc, wo)| ApplicationRequest {
                    invoice_id: *i,
                    amount: Some(m(a)),
                    discount_amount: disc.as_deref().map(m),
                    write_off_amount: wo.as_deref().map(m),
                })
                .collect(),
        ),
        credit_applications: Vec::new(),
        hold_as_deposit: hold,
        editing_payment_id: None,
    };
    let p = preview(&mut r.state, &req).expect("preview");
    NewCustomerPayment {
        customer_id: client,
        date: d(date),
        amount: p.amount,
        deposit_to: clearing,
        payment_method: Some("check".into()),
        ref_no: None,
        memo: None,
        applications: apps
            .iter()
            .map(|(i, a, disc, wo)| SaveApplication {
                invoice_id: *i,
                amount: m(a),
                expected_balance_due: p.invoice(*i).expect("row").balance_due_exact.clone(),
                discount_amount: disc.as_deref().map(m),
                write_off_amount: wo.as_deref().map(m),
            })
            .collect(),
        credit_applications: Vec::new(),
        write_off_account_id: None,
        discount_account_id: None,
        hold_as_deposit: hold,
        bank_feed_item_id: None,
    }
}

fn apps_of(r: &Run, req: &Value) -> Vec<Line> {
    req["applications"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|a| {
            (
                r.invoices[&s(&a["invoice"])],
                s(&a["amount"]),
                a["discountAmount"].as_str().map(str::to_string),
                a["writeOffAmount"].as_str().map(str::to_string),
            )
        })
        .collect()
}

fn code_of(e: &ApiError) -> String {
    match e {
        ApiError::Local(_) => "VALIDATION_ERROR".into(),
        ApiError::Conflict { code: Some(c), .. } => c.as_str().into(),
        ApiError::Validation(v) => v.code().map(|c| c.as_str().to_string()).unwrap_or_default(),
        other => format!("{other:?}"),
    }
}

fn check_lines(r: &mut Run, ledger: TransactionId, want: &Value, case: &str) {
    let mut expect: Vec<(String, String, String)> = Vec::new();
    for l in want.as_array().into_iter().flatten() {
        let a = role(r, &s(&l["account"]));
        expect.push((
            number_of(r, a),
            l["debit"]
                .as_str()
                .map_or("0.0000".into(), |x| m(x).to_wire()),
            l["credit"]
                .as_str()
                .map_or("0.0000".into(), |x| m(x).to_wire()),
        ));
    }
    let t = r
        .state
        .ledger
        .transactions()
        .into_iter()
        .find(|t| DemoLedger::id_of(t) == ledger)
        .expect("posted");
    let mut got: Vec<(String, String, String)> = t
        .lines
        .iter()
        .map(|l| (l.account.clone(), l.debit.to_wire(), l.credit.to_wire()))
        .collect();
    expect.sort();
    got.sort();
    assert_eq!(got, expect, "{case}: lines");
}

fn check_common(r: &mut Run, e: &Value, case: &str) {
    for (number, want) in e["invoices"].as_object().into_iter().flatten() {
        let inv = receivable(&r.state, r.invoices[number]).expect("invoice");
        assert_eq!(
            money(inv.balance).expect("m").to_wire(),
            m(&s(&want["balanceDue"])).to_wire(),
            "{case}: {number} balance"
        );
        let status = super::super::invoices::get(&r.state, inv.id).expect("record");
        assert_eq!(
            status
                .record
                .invoice_status
                .map(|x| format!("{x:?}").to_lowercase()),
            Some(s(&want["status"])),
            "{case}: {number} status"
        );
    }
    if let Some(b) = e["contactBalance"].as_str() {
        assert_eq!(
            money(contact_balance(&r.state, r.client)).expect("m"),
            m(b),
            "{case}: balance"
        );
    }
    if let Some(b) = e["customerDeposits"].as_str() {
        assert_eq!(
            money(deposits_held(&r.state)).expect("m"),
            m(b),
            "{case}: deposits held"
        );
    }
    if let Some(b) = e["operatingChange"].as_str() {
        let operating = super::super::account_id(OPERATING);
        let now = account_total(r, operating);
        assert_eq!(
            money(now - r.operating_before).expect("m"),
            m(b),
            "{case}: operating"
        );
    }
    if let Some(p) = r.payment {
        let rec = record_of(&r.state, p).expect("record");
        if let Some(u) = e["unappliedRemaining"].as_str() {
            assert_eq!(rec.unapplied_remaining, m(u), "{case}: remaining");
        }
        if let Some(u) = e["depositRemaining"].as_str() {
            assert_eq!(rec.deposit_remaining, m(u), "{case}: deposit remaining");
        }
    }
}

fn check_preview(r: &Run, p: &PaymentPreview, e: &Value, case: &str) {
    let row = |n: &str| p.invoice(r.invoices[n]).expect("row");
    let pairs = |k: &str| e[k].as_object().into_iter().flatten();
    if let Some(a) = e["amount"].as_str() {
        assert_eq!(p.amount, m(a), "{case}: amount");
    }
    if let Some(src) = e["amountSource"].as_str() {
        let want = if src == "typed" {
            AmountSource::Typed
        } else {
            AmountSource::FromApplications
        };
        assert_eq!(p.amount_source, want, "{case}: source");
    }
    for (n, v) in pairs("payments") {
        assert_eq!(row(n).payment, m(&s(v)), "{case}: payment {n}");
    }
    for (n, v) in pairs("remaining") {
        assert_eq!(row(n).remaining, m(&s(v)), "{case}: remaining {n}");
    }
    for (n, v) in pairs("discountAvailable") {
        assert_eq!(row(n).discount_available, m(&s(v)), "{case}: discount {n}");
    }
    for (n, v) in pairs("suggestDiscount") {
        assert_eq!(
            row(n).suggest_discount,
            v.as_bool().unwrap_or(false),
            "{case}: suggest {n}"
        );
    }
    if let Some(t) = e["totals"].as_object() {
        assert_eq!(p.totals.amount_due, m(&s(&t["amountDue"])), "{case}: due");
        assert_eq!(p.totals.applied, m(&s(&t["applied"])), "{case}: applied");
        assert_eq!(
            p.totals.discounts_and_credits,
            m(&s(&t["discountsAndCredits"])),
            "{case}: d&c"
        );
        assert_eq!(
            p.totals.unapplied,
            m(&s(&t["unapplied"])),
            "{case}: unapplied"
        );
    }
    if let Some(sf) = e["shortfalls"].as_array() {
        assert_eq!(p.shortfalls.len(), sf.len(), "{case}: shortfalls");
        for (got, want) in p.shortfalls.iter().zip(sf) {
            assert_eq!(got.invoice_id, r.invoices[&s(&want["invoice"])], "{case}");
            assert_eq!(got.amount, m(&s(&want["amount"])), "{case}: shortfall");
            assert_eq!(
                got.within_write_off_limit,
                want["withinWriteOffLimit"].as_bool().unwrap_or(false),
                "{case}: limit"
            );
        }
    }
    for (k, v) in [
        ("contactBalanceBefore", p.contact_balance_before),
        ("contactBalanceAfter", p.contact_balance_after),
    ] {
        if let Some(want) = e[k].as_str() {
            assert_eq!(v, m(want), "{case}: {k}");
        }
    }
    if let Some(o) = e["overpayment"].as_str() {
        assert_eq!(p.overpayment, Some(m(o)), "{case}: overpayment");
    }
    if let Some(o) = e["holdAsDepositOffered"].as_bool() {
        assert_eq!(p.hold_as_deposit_offered, o, "{case}: offered");
    }
}

#[allow(clippy::too_many_lines)]
fn run_case(case: &Value) {
    let name = format!("case {}", case["id"]);
    let mut r = new_run(case);
    for step in case["steps"].as_array().into_iter().flatten() {
        let req = &step["request"];
        let e = &step["expect"];
        match s(&step["op"]).as_str() {
            "preview" => {
                let apps = req["applications"].as_array().map(|a| {
                    a.iter()
                        .map(|x| ApplicationRequest {
                            invoice_id: r.invoices[&s(&x["invoice"])],
                            amount: x["amount"].as_str().map(m),
                            discount_amount: None,
                            write_off_amount: None,
                        })
                        .collect()
                });
                let amount = req["amount"]
                    .as_str()
                    .map(|a| Money::parse_wire(a).expect("parse"));
                let clearing = clearing(&mut r.state);
                let pr = PaymentPreviewRequest {
                    customer_id: r.client,
                    date: d(&s(&req["date"])),
                    amount,
                    deposit_to: clearing,
                    applications: apps,
                    credit_applications: Vec::new(),
                    hold_as_deposit: req["holdAsDeposit"].as_bool().unwrap_or(false),
                    editing_payment_id: None,
                };
                match preview(&mut r.state, &pr) {
                    Ok(p) => {
                        assert!(e["error"].is_null(), "{name}: expected an error");
                        check_preview(&r, &p, e, &name);
                    }
                    Err(err) => assert_eq!(code_of(&err), s(&e["error"]), "{name}"),
                }
            }
            "receive" => {
                let apps = apps_of(&r, req);
                let hold = req["holdAsDeposit"].as_bool().unwrap_or(false);
                let client = r.client;
                let date = s(&req["date"]);
                let amount = s(&req["amount"]);
                let res = if e["error"].is_string() {
                    // A save the preview would already flag: tokens from the
                    // invoice as stored.
                    let clearing = clearing(&mut r.state).expect("c");
                    let p = NewCustomerPayment {
                        customer_id: client,
                        date: d(&date),
                        amount: m(&amount),
                        deposit_to: clearing,
                        payment_method: None,
                        ref_no: None,
                        memo: None,
                        applications: apps
                            .iter()
                            .map(|(i, a, _, _)| SaveApplication {
                                invoice_id: *i,
                                amount: m(a),
                                expected_balance_due: token(
                                    receivable(&r.state, *i).expect("i").balance,
                                )
                                .expect("t"),
                                discount_amount: None,
                                write_off_amount: None,
                            })
                            .collect(),
                        credit_applications: Vec::new(),
                        write_off_account_id: None,
                        discount_account_id: None,
                        hold_as_deposit: hold,
                        bank_feed_item_id: None,
                    };
                    receive(&mut r.state, &p, None, None)
                } else {
                    let p = save(&mut r, client, &date, &amount, &apps, hold);
                    receive(&mut r.state, &p, None, Some("key-1"))
                };
                match res {
                    Ok(rec) => {
                        r.payment = Some(rec.id);
                        if !e["lines"].is_null() {
                            let l = ledger_of(&r.state, rec.id).expect("ledger");
                            check_lines(&mut r, l, &e["lines"], &name);
                        }
                        check_common(&mut r, e, &name);
                    }
                    Err(err) => assert_eq!(code_of(&err), s(&e["error"]), "{name}"),
                }
            }
            "void" => {
                let p = r.payment.expect("payment");
                void(&mut r.state, p, None, None).expect("void");
                check_common(&mut r, e, &name);
            }
            "invoice" => {
                let client = r.client;
                add_invoice(&mut r, &step["invoice"], client);
            }
            "apply" => {
                let p = r.payment.expect("payment");
                let before: Vec<TransactionId> = r
                    .state
                    .ledger
                    .transactions()
                    .iter()
                    .map(DemoLedger::id_of)
                    .collect();
                let ar = ApplyRequest {
                    date: d(&s(&req["date"])),
                    applications: req["applications"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|x| ApplyInvoice {
                            invoice_id: r.invoices[&s(&x["invoice"])],
                            amount: x["amount"].as_str().map(m),
                            expected_balance_due: None,
                        })
                        .collect(),
                    expected_updated_at: None,
                };
                let out = apply(&mut r.state, p, &ar, None).expect("apply");
                for (n, v) in e["applied"].as_object().into_iter().flatten() {
                    let got = out
                        .applied
                        .iter()
                        .find(|a| a.invoice_id == r.invoices[n])
                        .expect("applied");
                    assert_eq!(got.amount, m(&s(v)), "{name}: applied");
                }
                assert_eq!(out.remaining, m(&s(&e["remaining"])), "{name}: remaining");
                let added: Vec<TransactionId> = r
                    .state
                    .ledger
                    .transactions()
                    .iter()
                    .map(DemoLedger::id_of)
                    .filter(|id| !before.contains(id))
                    .collect();
                match e["lines"].as_array() {
                    Some(l) if l.is_empty() => {
                        assert!(added.is_empty(), "{name}: a credit posts nothing");
                    }
                    Some(_) => check_lines(&mut r, added[0], &e["lines"], &name),
                    None => {}
                }
                check_common(&mut r, e, &name);
            }
            "deposit" => {
                let clearing = clearing(&mut r.state).expect("c");
                let mut payments = Vec::new();
                for which in step["payments"].as_array().into_iter().flatten() {
                    let p = if s(which) == "this" {
                        r.payment
                    } else {
                        r.other_payment
                    }
                    .expect("p");
                    let rec = record_of(&r.state, p).expect("rec");
                    payments.push(GroupedPayment {
                        source: ledger_of(&r.state, p).expect("l"),
                        kind: DepositSource::Payment,
                        account: clearing,
                        amount: rec.amount,
                        contact: Some(rec.customer_id),
                        memo: None,
                    });
                }
                let account = role(&mut r, &s(&step["account"]));
                let dep = DepositDraft {
                    account,
                    date: d(&s(&step["date"])),
                    memo: None,
                    payments,
                    lines: Vec::new(),
                    deductions: Vec::new(),
                    cash_back: None,
                };
                assert_eq!(dep.total(), Some(m(&s(&e["total"]))), "{name}: deposit");
                r.deposit = Some(
                    super::super::deposits::create(&mut r.state, &dep, None).expect("deposit"),
                );
            }
            "bounce" | "bounceAgain" => {
                let p = r.payment.expect("payment");
                let rec = record_of(&r.state, p).expect("rec");
                let br = BounceRequest {
                    date: d(&s(&req["date"])),
                    bank_fee_amount: req["bankFeeAmount"].as_str().map(m),
                    bank_fee_account_id: None,
                    bank_fee_bank_account_id: None,
                    memo: None,
                    expected_updated_at: rec.updated_at,
                };
                match bounce(&mut r.state, p, &br, None, None) {
                    Ok(out) => {
                        assert!(e["error"].is_null(), "{name}: expected an error");
                        let held = role(&mut r, &s(&e["heldIn"]));
                        assert_eq!(out.held_in.account_id, held, "{name}: held in");
                        check_lines(&mut r, out.return_id, &e["lines"], &name);
                        if !e["feeLines"].is_null() {
                            check_lines(&mut r, out.fee_id.expect("fee"), &e["feeLines"], &name);
                        }
                        check_common(&mut r, e, &name);
                        if let Some(n) = e["credits"].as_u64() {
                            assert_eq!(
                                credits_of(&r.state, r.client, None).len() as u64,
                                n,
                                "{name}: credits"
                            );
                        }
                        if e["otherStillDeposited"].as_bool() == Some(true) {
                            let other =
                                ledger_of(&r.state, r.other_payment.expect("o")).expect("l");
                            assert!(r.state.ledger.deposited_in(other).is_some(), "{name}");
                        }
                        if e["pendingIncludesPayment"].as_bool() == Some(false) {
                            let l = ledger_of(&r.state, p).expect("l");
                            assert!(
                                !super::super::deposits::pending(&mut r.state)
                                    .iter()
                                    .any(|x| x.source == l),
                                "{name}: pending"
                            );
                            assert_eq!(
                                super::super::deposits::clearing_balance(&mut r.state),
                                super::super::deposits::clearing_balance(&mut DemoState::new()),
                                "{name}: clearing nets to 0"
                            );
                        }
                    }
                    Err(err) => assert_eq!(code_of(&err), s(&e["error"]), "{name}"),
                }
            }
            "voidDeposit" => {
                let dep = r.deposit.expect("deposit");
                let err =
                    super::super::deposits::void(&mut r.state, dep, None).expect_err("refused");
                assert_eq!(code_of(&err), s(&e["error"]), "{name}");
            }
            other => panic!("{name}: op {other}"),
        }
    }
}

#[test]
fn every_golden_case_balances_in_the_demo_engine() {
    let g = golden();
    let cases = g["cases"].as_array().expect("cases");
    assert_eq!(cases.len(), 10, "cases 1-9 and 11 (Q-P21t dropped 10)");
    for case in cases {
        run_case(case);
    }
}

#[test]
fn the_story_seed_is_the_mockup() {
    let mut state = DemoState::new();
    seed(&mut state);
    let client = story_client(&mut state).expect("Neosho");
    let open = receivables_of(&state, client);
    assert_eq!(open.len(), 2);
    assert_eq!(open[0].number, "1091");
    assert_eq!(open[1].number, "1095");
    assert_eq!(
        money(contact_balance(&state, client)).expect("m"),
        m("5645")
    );
    let uf = clearing(&mut state);
    let p = preview(
        &mut state,
        &PaymentPreviewRequest {
            customer_id: client,
            date: DemoState::today(),
            amount: Some(m("4000")),
            deposit_to: uf,
            applications: None,
            credit_applications: Vec::new(),
            hold_as_deposit: false,
            editing_payment_id: None,
        },
    )
    .expect("preview");
    assert_eq!(p.contact_balance_after, m("1645"));
    assert_eq!(p.average_days_to_pay, Some(24));
    // The sample's 2 payments waiting plus this one.
    assert_eq!(p.pending_deposits.as_ref().map(|x| x.count), Some(3));
    // Seeding twice changes nothing; Create Invoice's numbering is untouched.
    seed(&mut state);
    assert_eq!(receivables_of(&state, client).len(), 2);
    assert_eq!(super::super::invoices::next_number(&state), "1042");
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Auto allocation: Σapplied ≤ amount, each ≤ its balance, oldest
    /// first, unapplied = amount − Σ; a void, then a bounce of a fresh
    /// receipt, restore every invoice and the bank exactly.
    #[test]
    fn allocation_void_and_bounce_restore_exactly(
        totals in proptest::collection::vec(1i64..500_000, 1..6),
        pay in 1i64..2_000_000,
    ) {
        let case = serde_json::json!({"id": 0, "company": {}, "invoices": totals.iter().enumerate().map(|(i, t)| serde_json::json!({
            "number": format!("{}", 5000 + i), "date": format!("2026-10-{:02}", i + 1), "dueDate": "2026-11-30",
            "total": Money::from_cents(*t).expect("m").to_wire()})).collect::<Vec<_>>()});
        let mut r = new_run(&case);
        let amount = Money::from_cents(pay).expect("m");
        let clearing = clearing(&mut r.state);
        let req = PaymentPreviewRequest {
            customer_id: r.client, date: DemoState::today(), amount: Some(amount), deposit_to: clearing,
            applications: None, credit_applications: Vec::new(), hold_as_deposit: false, editing_payment_id: None,
        };
        let p = preview(&mut r.state, &req).expect("preview");
        let applied: Decimal = p.invoices.iter().map(|i| dec(i.payment)).sum();
        prop_assert!(applied <= dec(amount));
        prop_assert_eq!(dec(p.totals.unapplied), dec(amount) - applied);
        let mut short = false;
        for i in &p.invoices {
            prop_assert!(i.payment <= i.balance_due);
            // Oldest first: once one is not paid in full, the later ones get 0.
            if short { prop_assert!(i.payment.is_zero()); }
            short = short || i.payment < i.balance_due;
        }
        let before: Vec<Decimal> = receivables_of(&r.state, r.client).iter().map(|x| x.balance).collect();
        let bal_before = contact_balance(&r.state, r.client);
        let lines: Vec<Line> = p.invoices.iter().filter(|i| i.payment.is_positive())
            .map(|i| (i.invoice_id, i.payment.to_wire(), None, None)).collect();
        let client = r.client;
        let save1 = save(&mut r, client, "2026-11-04", &amount.to_wire(), &lines, false);
        let rec = receive(&mut r.state, &save1, None, None).expect("receive");
        void(&mut r.state, rec.id, None, None).expect("void");
        let after: Vec<Decimal> = receivables_of(&r.state, r.client).iter().map(|x| x.balance).collect();
        prop_assert_eq!(&after, &before);
        let save2 = save(&mut r, client, "2026-11-04", &amount.to_wire(), &lines, false);
        let rec = receive(&mut r.state, &save2, None, None).expect("receive");
        let br = BounceRequest { date: d("2026-11-12"), bank_fee_amount: None, bank_fee_account_id: None,
            bank_fee_bank_account_id: None, memo: None, expected_updated_at: rec.updated_at };
        bounce(&mut r.state, rec.id, &br, None, None).expect("bounce");
        let after: Vec<Decimal> = receivables_of(&r.state, r.client).iter().map(|x| x.balance).collect();
        prop_assert_eq!(&after, &before);
        prop_assert_eq!(contact_balance(&r.state, r.client), bal_before);
        prop_assert_eq!(record_of(&r.state, rec.id).expect("r").unapplied_remaining, Money::ZERO);
    }
}
