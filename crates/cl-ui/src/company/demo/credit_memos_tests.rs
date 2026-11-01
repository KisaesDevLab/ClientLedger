//! The demo credit memo, refund and sales receipt engine: golden cases 1-4
//! and 7 of `fixtures/receivables/golden.json`, the date and in-use rules,
//! and properties of remaining credit and voids (P22 plan §6.3).

use cl_api::{
    ApplyInvoice, CreditMemoApplication, CreditRefusal, InvoiceLine, RefundRefusal, credit_refusal,
    refund_refusal,
};
use cl_core::{Quantity, Rate};
use proptest::prelude::*;
use serde_json::Value;

use super::*;

fn golden() -> Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/receivables/golden.json");
    serde_json::from_slice(&std::fs::read(path).expect("golden file")).expect("json")
}

fn m(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}

fn d(s: &str) -> CalendarDate {
    CalendarDate::parse_wire(s).expect("date")
}

fn expect_str(v: &Value) -> Money {
    m(v.as_str().expect("string"))
}

pub(crate) fn fresh() -> (DemoState, ContactId) {
    let mut s = DemoState::new();
    seed(&mut s);
    let c = client_named(&mut s, BLUESTEM).expect("Bluestem");
    (s, c)
}

pub(crate) fn line(qty: &str, price: &str, taxable: bool) -> InvoiceLine {
    let mut l = InvoiceLine::account(
        super::super::account_id(INCOME),
        Quantity::parse_wire(qty).expect("q"),
        m(price),
    );
    l.is_taxable = Some(taxable);
    // The golden cases' 8.25 % (the demo company's own rate is 8.1 %).
    l.tax_rate = Some(Rate::parse_wire("0.0825").expect("rate"));
    l
}

/// Golden case 1's memo: 2 x 150.00 taxable, 1 x 75.00 not, 11/02.
pub(crate) fn case1(c: ContactId) -> NewCreditMemo {
    NewCreditMemo::new(
        c,
        d("2026-11-02"),
        vec![line("2", "150.00", true), line("1", "75.00", false)],
    )
}

fn inv(n: u32) -> TransactionId {
    seed_invoice(n).expect("id")
}

fn balance(s: &DemoState, n: u32) -> Money {
    money(receivable(s, inv(n)).expect("invoice").balance).expect("m")
}

pub(crate) fn client_balance(s: &DemoState, c: ContactId) -> Money {
    money(super::super::receivables::contact_balance(s, c)).expect("m")
}

fn bank() -> AccountId {
    super::super::account_id(super::super::receivables::OPERATING)
}

pub(crate) fn refund(
    memo: TransactionId,
    c: ContactId,
    date: &str,
    x: &str,
    check: bool,
) -> NewRefund {
    NewRefund {
        customer_id: c,
        date: d(date),
        from_account: bank(),
        amount: m(x),
        source: RefundSource::CreditMemo(memo),
        method: if check {
            RefundMethod::Check
        } else {
            RefundMethod::Cash
        },
        check: if check {
            RefundCheck::PrintLater
        } else {
            RefundCheck::NotACheck
        },
        ref_no: None,
        memo: None,
    }
}

#[test]
fn golden_1_credit_memo_with_items_and_tax() {
    let g = golden();
    let want = &g["cases"][0]["steps"][0]["expect"];
    let (mut s, c) = fresh();
    assert_eq!(client_balance(&s, c), m("750.00"));
    let p = preview_memo(&mut s, &case1(c), None).expect("preview");
    assert_eq!(p.figures.subtotal, expect_str(&want["subtotal"]));
    assert_eq!(p.figures.tax_total, expect_str(&want["taxTotal"]));
    assert_eq!(p.figures.total, expect_str(&want["total"]));
    assert_eq!(p.figures.lines[0].amount, m("300.00"));
    assert_eq!(p.suggested_applications.len(), 2);
    assert_eq!(p.suggested_applications[0].invoice_id, inv(1201));
    assert_eq!(
        p.suggested_applications[0].amount,
        expect_str(&want["suggestedApplications"]["A"])
    );
    assert_eq!(
        p.suggested_applications[1].amount,
        expect_str(&want["suggestedApplications"]["B"])
    );
    assert_eq!(p.remaining_credit, m("0.00"));
    assert_eq!(p.figures.contact_balance_after, Some(m("350.25")));
    let saved = create_memo(&mut s, &case1(c), None).expect("create");
    assert_eq!(saved.total, m("399.75"));
    assert_eq!(saved.tax_amount, m("24.75"));
    assert_eq!(saved.remaining, m("399.75"));
    assert_eq!(client_balance(&s, c), m("350.25"));
    assert!(credits(&mut s, c).iter().any(|x| x.source_id == saved.id
        && x.kind == CreditKind::CreditMemo
        && x.remaining == m("399.75")));
}

#[test]
fn golden_2_apply_at_save_oldest_first() {
    let (mut s, c) = fresh();
    let p = preview_memo(&mut s, &case1(c), None).expect("preview");
    let mut memo = case1(c);
    memo.applications = p
        .suggested_applications
        .iter()
        .map(|a| CreditMemoApplication {
            invoice_id: a.invoice_id,
            amount: a.amount,
            expected_balance_due: p
                .open_invoice(a.invoice_id)
                .expect("open")
                .balance_due_exact
                .clone(),
        })
        .collect();
    // Sent newest first, listed oldest invoice first (the engine's order).
    memo.applications.reverse();
    let saved = create_memo(&mut s, &memo, None).expect("create");
    assert_eq!(saved.remaining, m("0.00"));
    assert!(saved.applications.iter().all(|a| a.applied_date.is_none()));
    let order: Vec<_> = saved.applications.iter().map(|a| a.invoice_id).collect();
    assert_eq!(order, vec![inv(1201), inv(1202)]);
    assert_eq!(balance(&s, 1201), m("0.00"));
    assert_eq!(balance(&s, 1202), m("350.25"));
    assert_eq!(client_balance(&s, c), m("350.25"));
    // The same tokens again: the balances moved, 409.
    let e = create_memo(&mut s, &memo, None).expect_err("stale tokens");
    assert_eq!(
        credit_refusal(&e),
        Some(CreditRefusal::InvoiceBalanceChanged)
    );
}

#[test]
fn golden_3_keep_as_credit_and_refund_by_check() {
    let (mut s, c) = fresh();
    let memo = create_memo(&mut s, &case1(c), None).expect("create");
    let r = create_refund(
        &mut s,
        &refund(memo.id, c, "2026-11-05", "399.75", true),
        None,
    )
    .expect("refund");
    assert_eq!(get_memo(&s, memo.id).expect("memo").remaining, m("0.00"));
    assert_eq!(client_balance(&s, c), m("750.00"));
    let q = refund_print_queue(&s);
    assert_eq!(q, vec![(r, c, m("399.75"))]);
    let rec = get_refund(&s, r).expect("refund");
    assert_eq!(rec.print_status, Some(RefundPrintStatus::Queue));
    assert_eq!(rec.source.expect("source").kind, CreditKind::CreditMemo);
    void_refund(&mut s, r, None).expect("void");
    assert_eq!(get_memo(&s, memo.id).expect("memo").remaining, m("399.75"));
    assert_eq!(client_balance(&s, c), m("350.25"));
    assert_eq!(refund_print_queue(&s).len(), 0);
}

#[test]
fn golden_4_partial_refund_apply_rest_and_over_refund() {
    let (mut s, c) = fresh();
    let memo = create_memo(&mut s, &case1(c), None).expect("create");
    let r = create_refund(
        &mut s,
        &refund(memo.id, c, "2026-11-05", "100.00", true),
        None,
    )
    .expect("refund");
    assert_eq!(get_memo(&s, memo.id).expect("memo").remaining, m("299.75"));
    let applied = apply_memo(
        &mut s,
        memo.id,
        &ApplyRequest {
            date: d("2026-11-10"),
            applications: vec![ApplyInvoice {
                invoice_id: inv(1202),
                amount: None,
                expected_balance_due: None,
            }],
            expected_updated_at: None,
        },
        None,
    )
    .expect("apply");
    assert_eq!(applied.applied[0].amount, m("299.75"));
    assert_eq!(applied.remaining, m("0.00"));
    assert_eq!(balance(&s, 1202), m("200.25"));
    let e = create_refund(
        &mut s,
        &refund(memo.id, c, "2026-11-12", "0.01", false),
        None,
    )
    .expect_err("over-refund");
    assert_eq!(refund_refusal(&e), Some(RefundRefusal::CreditNotAvailable));
    let e = void_memo(&mut s, memo.id, None).expect_err("refunded");
    assert_eq!(credit_refusal(&e), Some(CreditRefusal::CreditRefunded));
    void_refund(&mut s, r, None).expect("void refund");
    assert_eq!(get_memo(&s, memo.id).expect("memo").remaining, m("100.00"));
    void_memo(&mut s, memo.id, None).expect("void memo");
    assert_eq!(balance(&s, 1201), m("250.00"));
    assert_eq!(balance(&s, 1202), m("500.00"));
    assert_eq!(client_balance(&s, c), m("750.00"));
}

#[test]
#[allow(clippy::many_single_char_names)]
fn golden_7_sales_receipt_with_an_item_and_tax() {
    let g = golden();
    let want = &g["cases"][6]["steps"][0]["expect"];
    let (mut s, _) = fresh();
    let r = NewSalesReceipt::new(d("2026-11-03"), vec![line("2", "49.99", true)]);
    let p = preview_receipt(&mut s, &r).expect("preview");
    assert_eq!(p.figures.subtotal, expect_str(&want["subtotal"]));
    assert_eq!(p.figures.tax_total, expect_str(&want["taxTotal"]));
    assert_eq!(p.figures.total, expect_str(&want["total"]));
    assert_eq!(p.problems.len(), 0);
    let saved = save_receipt(&mut s, &r, None, None).expect("save");
    assert_eq!(saved.total, m("108.23"));
    assert_eq!(
        saved.deposit_to_account_id,
        super::super::receivables::clearing(&mut s)
    );
    let back = NewSalesReceipt::new(d("2026-11-03"), vec![line("1", "-20.00", false)]);
    let p = preview_receipt(&mut s, &back).expect("preview");
    assert_eq!(p.problems[0].code, "NEGATIVE_RECEIPT_NEEDS_BANK");
    let e = save_receipt(&mut s, &back, None, None).expect_err("needs a bank");
    assert_eq!(
        credit_refusal(&e),
        Some(CreditRefusal::NegativeReceiptNeedsBank)
    );
    let mut to_bank = back.clone();
    to_bank.deposit_to_account_id = Some(bank());
    let saved = save_receipt(&mut s, &to_bank, None, None).expect("a return to the bank");
    assert_eq!(saved.total, m("-20.00"));
    // An edit at the old version works; at a stale one it is 409.
    let edited = save_receipt(
        &mut s,
        &to_bank,
        Some((saved.id, Some(saved.updated_at))),
        None,
    )
    .expect("edit");
    let e = save_receipt(
        &mut s,
        &to_bank,
        Some((saved.id, Some(saved.updated_at))),
        None,
    )
    .expect_err("stale");
    assert_eq!(credit_refusal(&e), Some(CreditRefusal::Stale));
    assert_eq!(get_receipt(&s, edited.id).expect("get").total, m("-20.00"));
}

#[test]
fn date_and_in_use_rules() {
    let (mut s, c) = fresh();
    let memo = create_memo(&mut s, &case1(c), None).expect("create");
    let before = ApplyRequest {
        date: d("2026-11-01"),
        applications: vec![ApplyInvoice {
            invoice_id: inv(1201),
            amount: Some(m("10.00")),
            expected_balance_due: None,
        }],
        expected_updated_at: None,
    };
    let e = apply_memo(&mut s, memo.id, &before, None).expect_err("before the memo");
    assert_eq!(
        credit_refusal(&e),
        Some(CreditRefusal::ApplyDateBeforeSource)
    );
    let ok = ApplyRequest {
        date: d("2026-11-05"),
        ..before
    };
    apply_memo(&mut s, memo.id, &ok, None).expect("apply");
    let e = create_refund(
        &mut s,
        &refund(memo.id, c, "2026-11-01", "1.00", false),
        None,
    )
    .expect_err("refund before the memo");
    assert_eq!(refund_refusal(&e), Some(RefundRefusal::DateBeforeSource));
    let mut later = case1(c);
    later.txn_date = d("2026-11-06");
    let v = get_memo(&s, memo.id).expect("memo").updated_at;
    let e = update_memo(&mut s, memo.id, &later, Some(v), None).expect_err("after first use");
    assert_eq!(credit_refusal(&e), Some(CreditRefusal::SourceDateAfterUse));
    let small = NewCreditMemo::new(c, d("2026-11-02"), vec![line("1", "5.00", false)]);
    let e = update_memo(&mut s, memo.id, &small, Some(v), None).expect_err("below used");
    assert_eq!(credit_refusal(&e), Some(CreditRefusal::CreditInUse));
    let old = DemoState::now();
    let e = update_memo(&mut s, memo.id, &case1(c), Some(old), None).expect_err("stale");
    assert_eq!(credit_refusal(&e), Some(CreditRefusal::Stale));
    let saved = update_memo(&mut s, memo.id, &case1(c), Some(v), None).expect("same memo");
    assert_eq!(saved.remaining, m("389.75"));
    unapply_memo(&mut s, memo.id, inv(1201), None, None).expect("unapply");
    assert_eq!(balance(&s, 1201), m("250.00"));
    let zero = NewCreditMemo::new(c, d("2026-11-02"), vec![line("1", "-5.00", false)]);
    let e = create_memo(&mut s, &zero, None).expect_err("not positive");
    assert_eq!(credit_refusal(&e), Some(CreditRefusal::TotalNotPositive));
    let bad = NewRefund {
        from_account: super::super::receivables::clearing(&mut s).expect("clearing"),
        ..refund(memo.id, c, "2026-11-05", "1.00", false)
    };
    let e = create_refund(&mut s, &bad, None).expect_err("not a bank");
    assert_eq!(refund_refusal(&e), Some(RefundRefusal::AccountInvalid));
}

#[test]
fn an_edit_refuses_applications_like_the_engine() {
    let (mut s, c) = fresh();
    let preview = preview_memo(&mut s, &case1(c), None).expect("preview");
    let token = preview
        .open_invoice(inv(1201))
        .expect("open")
        .balance_due_exact
        .clone();
    let memo = create_memo(&mut s, &case1(c), None).expect("create");
    let v = get_memo(&s, memo.id).expect("memo").updated_at;
    let mut with_apps = case1(c);
    with_apps.applications = vec![CreditMemoApplication {
        invoice_id: inv(1201),
        amount: m("10.00"),
        expected_balance_due: token,
    }];
    let e = update_memo(&mut s, memo.id, &with_apps, Some(v), None).expect_err("applications");
    assert!(matches!(e, cl_api::ApiError::Validation(_)), "{e:?}");
    assert_eq!(balance(&s, 1201), m("250.00"), "nothing applied");
    assert_eq!(
        get_memo(&s, memo.id).expect("memo").updated_at,
        v,
        "nothing saved"
    );
}

#[test]
fn the_seed_leaves_p21s_story_alone() {
    let (mut s, _) = fresh();
    super::super::receivables::seed(&mut s);
    let neosho = super::super::receivables::story_client(&mut s).expect("Neosho");
    assert_eq!(client_balance(&s, neosho), m("5645.00"));
    let cedar = client_named(&mut s, CEDAR_GAP).expect("Cedar Gap");
    assert_eq!(client_balance(&s, cedar), m("200.00"));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    /// Σ applications ≤ the credit; remaining = total − applied −
    /// refunded; voiding the refund and the memo restores every invoice
    /// and the client balance exactly.
    #[test]
    fn remaining_and_voids_are_exact(
        cents in 1i64..120_000,
        refund_share in 0i64..100,
        apply_a in proptest::bool::ANY,
    ) {
        let (mut s, c) = fresh();
        let price = Money::from_cents(cents).expect("m").to_wire();
        let memo = NewCreditMemo::new(c, d("2026-11-02"), vec![line("1", &price, false)]);
        let saved = create_memo(&mut s, &memo, None).expect("create");
        let total = dec(saved.total);
        let r = Money::from_cents(cents * refund_share / 100).expect("m");
        let rid = if r.is_positive() {
            Some(create_refund(&mut s, &NewRefund { amount: r, ..refund(saved.id, c, "2026-11-03", "1.00", false) }, None).expect("refund"))
        } else { None };
        let target = if apply_a { 1201 } else { 1202 };
        let left = dec(get_memo(&s, saved.id).expect("memo").remaining);
        prop_assert_eq!(left, total - dec(r));
        if left > Decimal::ZERO {
            let res = apply_memo(&mut s, saved.id, &ApplyRequest {
                date: d("2026-11-04"),
                applications: vec![ApplyInvoice { invoice_id: inv(target), amount: None, expected_balance_due: None }],
                expected_updated_at: None,
            }, None).expect("apply");
            let applied: Decimal = res.applied.iter().map(|a| dec(a.amount)).sum();
            prop_assert!(applied <= left);
            prop_assert_eq!(dec(res.remaining), left - applied);
            let rec = get_memo(&s, saved.id).expect("memo");
            let live: Decimal = rec.live_applications().map(|a| dec(a.amount)).sum();
            let refunded: Decimal = rec.live_refunds().map(|x| dec(x.amount)).sum();
            prop_assert_eq!(dec(rec.remaining), total - live - refunded);
        }
        prop_assert_eq!(super::super::statements::items_total(&s, d("2027-12-31"), c), dec(client_balance(&s, c)));
        if let Some(rid) = rid { void_refund(&mut s, rid, None).expect("void refund"); }
        void_memo(&mut s, saved.id, None).expect("void memo");
        prop_assert_eq!(balance(&s, 1201), m("250.00"));
        prop_assert_eq!(balance(&s, 1202), m("500.00"));
        prop_assert_eq!(client_balance(&s, c), m("750.00"));
    }
}
