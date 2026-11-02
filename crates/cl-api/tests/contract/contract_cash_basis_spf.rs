//! Pure-cash receipts on the cash basis against the real engine (P23 plan
//! §2.3, goldens 8, 13 (cash part) and 15; Q-SPF1, Q-SPF3, Q-SPF7, Q-P23y).
//! Report figures are read as the engine's number text (`ReportAmount`),
//! never as floats.
//!
//! - `unapplied_receipts_no_ar_on_cash` (golden 8 a, c): a 200.00 payment
//!   kept as a credit on 10/20 and a 100.00 held deposit on 10/05 are
//!   income (Unapplied receipts) on the cash basis, never A/R or Customer
//!   Deposits; the credit applied on 10/28 to INV-G (dated 11/18) moves
//!   183.70 to Sales and 16.30 to Sales Tax Payable on 10/28, so the cash
//!   sales tax report collects Springfield's 16.30 in October. The cash
//!   Balance Sheet at 10/31: bank 300.00, STP 16.30, equity 283.70, no A/R,
//!   no Customer Deposits, and it balances.
//! - `cash_write_off_and_bill_discount` (golden 13 cash, golden 15; in
//!   August, which no other P23 test posts in): a
//!   write-off never shows on cash (income 91.85 on 100.00 received of
//!   217.75, no bad-debt expense); a bill of 100.00 paid with 98.00 and a
//!   2.00 discount clears A/P on cash (Supplies 100.00, discount income
//!   2.00); a 30.00 vendor credit refunded in cash leaves no A/P residue.

use cl_api::{
    AccountType, ApplyInvoice, ApplyRequest, BalanceSheet, BillDraft, BillLineDraft, BillToPay,
    Capabilities, PostingOptions, ReportAmount, ReportBasis, SalesTaxBasis, VendorRefundDraft,
};
use cl_core::ids::AccountId;
use cl_core::{CalendarDate, Money};

use crate::common::{account, date, key, m};
use crate::contract_receive_payment::system_account;
use crate::contract_sales_tax_setup::{Monett, caps, monett, row};
use crate::support::{Harness, unique};

fn spf_caps(h: &Harness) -> bool {
    caps(h)
        && h.require_cap(Capabilities::CASH_BASIS_SPF, "U-152")
        && h.require_cap(Capabilities::CUSTOMER_DEPOSITS, "U-135")
        && h.require_cap(Capabilities::PAYMENT_SETTLE, "U-133")
}

fn amount(a: &ReportAmount) -> Money {
    m(&a.display(2).expect("a report amount"))
}

/// An account's balance on a balance sheet (0.00 when it is not listed).
fn on_sheet(bs: &BalanceSheet, a: AccountId) -> Money {
    bs.assets
        .iter()
        .chain(&bs.liabilities)
        .chain(&bs.equity)
        .find(|e| e.account_id == Some(a))
        .map_or(Money::ZERO, |e| amount(&e.balance))
}

fn cash_sheet(h: &Harness, x: &Monett, on: CalendarDate) -> BalanceSheet {
    h.run(x.s.reports().balance_sheet(on, ReportBasis::Cash))
        .unwrap_or_else(|e| panic!("cash balance sheet: {e}"))
}

fn cash_net_income(h: &Harness, x: &Monett, from: CalendarDate, to: CalendarDate) -> Money {
    let pl = h
        .run(x.s.reports().profit_loss(from, to, ReportBasis::Cash))
        .unwrap_or_else(|e| panic!("cash P&L: {e}"));
    amount(&pl.net_income)
}

/// Golden 8 (a) and (c) with the cash Balance Sheet.
pub fn unapplied_receipts_no_ar_on_cash(h: &Harness) {
    if !spf_caps(h) {
        return;
    }
    let x = monett(h, "Pure cash", true);
    let unapplied = system_account(h, &x.s, "unapplied_receipts");
    let g = x.plants(h, x.ozark, date(2026, 11, 18), "183.70");
    assert_eq!(g.tax_amount, Some(m("16.30")));
    // (a) 200.00 kept as a credit on 10/20.
    let (_, pmt) = x.receive(h, x.ozark, date(2026, 10, 20), "200.00", &[]);
    assert_eq!(pmt.unapplied_remaining, m("200.00"));
    let bs = cash_sheet(h, &x, date(2026, 10, 27));
    assert_eq!(on_sheet(&bs, x.checking), m("200.00"));
    assert_eq!(on_sheet(&bs, x.ar), Money::ZERO, "no A/R on cash");
    assert_eq!(
        cash_net_income(h, &x, date(2026, 10, 1), date(2026, 10, 27)),
        m("200.00"),
        "Unapplied receipts income"
    );
    // Applied 10/28 to INV-G (dated 11/18).
    h.run(x.s.customer_payments().apply(
        pmt.id,
        &ApplyRequest {
            date: date(2026, 10, 28),
            applications: vec![ApplyInvoice {
                invoice_id: g.txn.id,
                amount: Some(m("200.00")),
                expected_balance_due: None,
            }],
            expected_updated_at: Some(pmt.updated_at),
        },
        &PostingOptions::default(),
        key(),
    ))
    .expect("apply the credit");
    // (c) A held deposit of 100.00 on 10/05.
    let mut r = crate::contract_receive_payment::request(
        x.ozark,
        date(2026, 10, 5),
        Some("100.00"),
        Some(x.checking),
    );
    r.hold_as_deposit = true;
    let p = crate::contract_receive_payment::preview(h, &x.s, &r);
    let mut save =
        crate::contract_receive_payment::save_of(&p, x.ozark, date(2026, 10, 5), x.checking);
    save.hold_as_deposit = true;
    let dep = h
        .run(
            x.s.customer_payments()
                .receive(&save, &PostingOptions::default(), key()),
        )
        .expect("held deposit");
    assert!(dep.holds_deposit);
    // The system account exists once a deposit is held.
    let cd = system_account(h, &x.s, "customer_deposits");
    assert_eq!(
        Monett::net(&x.txn(h, dep.id), cd),
        m("-100.00"),
        "GL: Cr Customer Deposits"
    );

    let oct = x.liability(
        h,
        SalesTaxBasis::Cash,
        date(2026, 10, 1),
        date(2026, 10, 31),
    );
    let spr = row(&oct, x.springfield);
    assert_eq!(spr.taxable_sales, m("183.70"));
    assert_eq!(spr.tax.collected, m("16.30"), "tax at the apply date");
    assert_eq!(oct.total, m("16.30"));
    let acc = x.liability(
        h,
        SalesTaxBasis::Accrual,
        date(2026, 10, 1),
        date(2026, 10, 31),
    );
    assert_eq!(row(&acc, x.springfield).tax.collected, Money::ZERO);

    let bs = cash_sheet(h, &x, date(2026, 10, 31));
    assert_eq!(on_sheet(&bs, x.checking), m("300.00"));
    assert_eq!(on_sheet(&bs, x.stp), m("16.30"));
    assert_eq!(on_sheet(&bs, x.ar), Money::ZERO, "no A/R on cash");
    assert_eq!(
        on_sheet(&bs, cd),
        Money::ZERO,
        "no Customer Deposits on cash"
    );
    assert_eq!(on_sheet(&bs, unapplied), Money::ZERO, "an income account");
    assert_eq!(amount(&bs.total_assets), m("300.00"));
    assert_eq!(amount(&bs.total_equity), m("283.70"));
    assert_eq!(
        amount(&bs.total_liabilities_and_equity),
        amount(&bs.total_assets),
        "the cash Balance Sheet balances"
    );
    assert_eq!(
        cash_net_income(h, &x, date(2026, 10, 1), date(2026, 10, 31)),
        m("283.70"),
        "Sales 183.70 + Unapplied receipts 100.00"
    );
}

/// Golden 13 (cash) and golden 15, dated August 2026 (no other P23 test
/// posts in August, so the company's cash P&L is this test's alone).
#[allow(clippy::too_many_lines, reason = "goldens 13 (cash) and 15")]
pub fn cash_write_off_and_bill_discount(h: &Harness) {
    if !spf_caps(h)
        || !h.require_cap(Capabilities::BILL_DISCOUNTS, "U-101")
        || !h.require_cap(Capabilities::VENDOR_REFUND, "U-54")
    {
        return;
    }
    let x = monett(h, "Cash write-off", true);
    let inv = x.plants(h, x.ozark, date(2026, 8, 4), "200.00");
    assert_eq!(inv.tax_amount, Some(m("17.75")));
    x.receive(
        h,
        x.ozark,
        date(2026, 8, 20),
        "100.00",
        &[(inv.txn.id, "100.00", None, Some("117.75"))],
    );
    let aug = (date(2026, 8, 1), date(2026, 8, 31));
    assert_eq!(
        cash_net_income(h, &x, aug.0, aug.1),
        m("91.85"),
        "200 × 100 / 217.75; no bad-debt expense"
    );
    let pl = h
        .run(x.s.reports().profit_loss(aug.0, aug.1, ReportBasis::Cash))
        .expect("P&L");
    assert_eq!(
        pl.total_expenses.as_ref().map_or(Money::ZERO, amount),
        Money::ZERO,
        "the write-off is invisible on cash"
    );
    let acc = h
        .run(
            x.s.reports()
                .profit_loss(aug.0, aug.1, ReportBasis::Accrual),
        )
        .expect("accrual P&L");
    assert_eq!(
        acc.total_expenses.as_ref().map_or(Money::ZERO, amount),
        m("108.15"),
        "accrual keeps the bad debt"
    );
    let bs = cash_sheet(h, &x, aug.1);
    assert_eq!(on_sheet(&bs, x.ar), Money::ZERO, "no A/R on cash");
    // Golden 15: a bill paid with 98.00 and a 2.00 discount.
    let supplies = account(h, &x.s, AccountType::Expense, "Supplies").id;
    let discounts = account(h, &x.s, AccountType::OtherRevenue, "Bill discounts").id;
    let ap = crate::contract_bills::payables_account(h, &x.s);
    let vendor = crate::contract_pay_bills::vendor(h, &x.s, "Ozark Feed & Seed");
    let bill = h
        .run(x.s.bills().create(
            &BillDraft {
                vendor,
                date: date(2026, 8, 2),
                vendor_invoice_number: Some(unique("BILL").replace(' ', "-")),
                terms: None,
                due_date: Some(date(2026, 8, 30)),
                memo: None,
                amount_due: m("100.00"),
                lines: vec![BillLineDraft::expense(supplies, m("100.00"))],
                received: true,
                draft_attachment: None,
            },
            &PostingOptions::default(),
            key(),
        ))
        .unwrap_or_else(|e| panic!("bill: {e:?}"))
        .record
        .txn
        .id;
    let mut req = crate::contract_pay_bills::request(
        x.checking,
        vec![BillToPay {
            bill,
            cash: m("98.00"),
            discount: m("2.00"),
        }],
        Vec::new(),
    );
    req.date = date(2026, 8, 5);
    req.discount_account = Some(discounts);
    h.run(
        x.s.bill_payments()
            .pay(&req, &PostingOptions::default(), key()),
    )
    .unwrap_or_else(|e| panic!("pay the bill: {e:?}"));
    assert_eq!(
        cash_net_income(h, &x, aug.0, aug.1),
        m("-6.15"),
        "91.85 − Supplies 100.00 + discount 2.00 (not − 96.00)"
    );
    let bs = cash_sheet(h, &x, aug.1);
    assert_eq!(on_sheet(&bs, ap), Money::ZERO, "A/P clears on cash");
    // A 30.00 vendor credit refunded in cash: no A/P residue.
    let made = h
        .run(x.s.raw_mapped(
            cl_api::Method::Post,
            "/api/v1/vendor-credits",
            Some(&serde_json::json!({
                "contactId": vendor.to_string(),
                "txnDate": "2026-08-10",
                "lines": [{ "accountId": supplies.to_string(), "amount": "30.00" }],
            })),
        ))
        .expect("vendor credit");
    let credit = made["credit"]["id"]
        .as_str()
        .expect("credit id")
        .parse()
        .expect("uuid");
    let refund = VendorRefundDraft {
        vendor,
        deposit_to: x.checking,
        payables: ap,
        date: date(2026, 8, 12),
        amount: m("30.00"),
        applies_to: credit,
        memo: None,
    }
    .posting(crate::contract_deposits::caps(h))
    .expect("refund");
    h.run(
        x.s.payments()
            .create_deposit(&refund, &PostingOptions::default(), key()),
    )
    .unwrap_or_else(|e| panic!("vendor refund: {e}"));
    let bs = cash_sheet(h, &x, aug.1);
    assert_eq!(on_sheet(&bs, ap), Money::ZERO, "no A/P residue on cash");
    assert_eq!(
        cash_net_income(h, &x, aug.0, aug.1),
        m("23.85"),
        "Supplies nets to 70.00"
    );
}
