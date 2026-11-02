//! SSUTA discounts, bad debts and recoveries against the real engine (P23
//! plan §2.8, goldens 12-14; Q-P23m, Q-P23w, Q-SPF3).
//!
//! - `ssuta_discount_and_bounce` (goldens 12, 14): 426.79 with an 8.71
//!   discount on INV-H (435.50) carries 0.71 tax: Dr bank 426.79, Dr STP
//!   0.71, Dr Sales discounts 8.00, Cr A/R 435.50; owed 34.79 on both
//!   bases. The bounce gives the shares back (Cr STP 0.71, Cr discounts
//!   8.00): accrual owed 35.50, cash collected 0.00.
//! - `bad_debt_recovery` (golden 13): 100.00 with 117.75 written off on
//!   INV-J (217.75): the write-off carries 9.60 tax (Dr Bad debts 108.15);
//!   accrual owes 8.15, cash shows no bad debt and collects 8.15 on 91.85.
//!   A 50.00 recovery (replayed once) takes back 4.08 (Dr A/R 50.00, Cr Bad
//!   debts 45.92, Cr STP 4.08) and reopens the invoice; more than is left
//!   is 409 `RECOVERY_EXCEEDS_WRITE_OFF` with what is available; paying
//!   the reopened 50.00 collects 4.08 on cash; a bounce of the write-off
//!   payment is 409 `WRITE_OFF_RECOVERED`.
//! - `ssuta_off_and_forward_only` (golden 12 variant, Q-P23w): with the
//!   switch off a discount carries no tax (Dr Sales discounts 8.71); turned
//!   on, the next payment's discount does; turned off again, rows written
//!   while it was on stay.

use cl_api::{
    BounceRequest, Capabilities, NewRecovery, PaymentRefusal, PostingOptions, RecoveryRefusal,
    SalesTaxBasis, payment_refusal, recovery_available, recovery_refusal,
};
use cl_core::Money;

use crate::common::{date, key, m};
use crate::contract_sales_tax_setup::{Monett, caps, monett, row};
use crate::support::Harness;

fn ssuta_caps(h: &Harness) -> bool {
    caps(h)
        && h.require_cap(Capabilities::SALES_TAX_SSUTA, "U-153")
        && h.require_cap(Capabilities::CASH_BASIS_SPF, "U-152")
}

fn nov(h: &Harness, x: &Monett, basis: SalesTaxBasis) -> cl_api::SalesTaxLiability {
    x.liability(h, basis, date(2026, 11, 1), date(2026, 11, 30))
}

fn dec(h: &Harness, x: &Monett, basis: SalesTaxBasis) -> cl_api::SalesTaxLiability {
    x.liability(h, basis, date(2026, 12, 1), date(2026, 12, 31))
}

/// Goldens 12 and 14.
pub fn ssuta_discount_and_bounce(h: &Harness) {
    if !ssuta_caps(h) || !h.require_cap(Capabilities::PAYMENT_BOUNCE, "U-136") {
        return;
    }
    let x = monett(h, "SSUTA discount", true);
    let inv = x.plants(h, x.ozark, date(2026, 11, 3), "400.00");
    assert_eq!(inv.tax_amount, Some(m("35.50")));
    let id = inv.txn.id;
    let (p, pmt) = x.receive(
        h,
        x.ozark,
        date(2026, 11, 8),
        "426.79",
        &[(id, "426.79", Some("8.71"), None)],
    );
    assert_eq!(p.sales_tax_ssuta, Some(true));
    assert_eq!(p.invoice(id).and_then(|r| r.discount_tax), Some(m("0.71")));
    assert_eq!(p.totals.discount_tax, Some(m("0.71")));
    let t = x.txn(h, pmt.id);
    assert_eq!(Monett::net(&t, x.checking), m("426.79"));
    assert_eq!(Monett::net(&t, x.stp), m("0.71"));
    assert_eq!(Monett::net(&t, x.sales_discounts), m("8.00"));
    assert_eq!(Monett::net(&t, x.ar), m("-435.50"));
    for basis in [SalesTaxBasis::Accrual, SalesTaxBasis::Cash] {
        let l = nov(h, &x, basis);
        let r = row(&l, x.springfield);
        assert_eq!(r.tax.collected, m("35.50"), "{basis:?}");
        assert_eq!(r.tax.discounts, m("-0.71"), "{basis:?}");
        assert_eq!(r.tax.owed_at_end, m("34.79"), "{basis:?}");
    }

    // Golden 14: the bounce gives back the stored shares.
    let b = h
        .run(x.s.customer_payments().bounce(
            pmt.id,
            &BounceRequest {
                date: date(2026, 11, 25),
                bank_fee_amount: None,
                bank_fee_account_id: None,
                bank_fee_bank_account_id: None,
                memo: None,
                expected_updated_at: pmt.updated_at,
            },
            &PostingOptions::default(),
            key(),
        ))
        .expect("bounce");
    let t = x.txn(h, b.return_id);
    assert_eq!(Monett::net(&t, x.ar), m("435.50"));
    assert_eq!(Monett::net(&t, x.checking), m("-426.79"));
    assert_eq!(Monett::net(&t, x.stp), m("-0.71"));
    assert_eq!(Monett::net(&t, x.sales_discounts), m("-8.00"));
    let acc = nov(h, &x, SalesTaxBasis::Accrual);
    assert_eq!(row(&acc, x.springfield).tax.discounts, Money::ZERO);
    assert_eq!(row(&acc, x.springfield).tax.owed_at_end, m("35.50"));
    let cash = nov(h, &x, SalesTaxBasis::Cash);
    assert_eq!(row(&cash, x.springfield).tax.collected, Money::ZERO);
    assert_eq!(row(&cash, x.springfield).tax.owed_at_end, Money::ZERO);
}

/// Golden 13.
#[allow(clippy::too_many_lines, reason = "the golden 13 walk")]
pub fn bad_debt_recovery(h: &Harness) {
    if !ssuta_caps(h)
        || !h.require_cap(Capabilities::BAD_DEBT_RECOVERY, "U-154")
        || !h.require_cap(Capabilities::IDEMPOTENCY_RECOVERY, "U-154")
    {
        return;
    }
    let x = monett(h, "SSUTA bad debt", true);
    let inv = x.plants(h, x.ozark, date(2026, 11, 4), "200.00");
    assert_eq!(inv.tax_amount, Some(m("17.75")));
    let id = inv.txn.id;
    let (p, pmt) = x.receive(
        h,
        x.ozark,
        date(2026, 11, 20),
        "100.00",
        &[(id, "100.00", None, Some("117.75"))],
    );
    assert_eq!(p.invoice(id).and_then(|r| r.write_off_tax), Some(m("9.60")));
    let t = x.txn(h, pmt.id);
    assert_eq!(Monett::net(&t, x.stp), m("9.60"));
    assert_eq!(Monett::net(&t, x.bad_debts), m("108.15"));
    assert_eq!(Monett::net(&t, x.ar), m("-217.75"));
    let acc = nov(h, &x, SalesTaxBasis::Accrual);
    let r = row(&acc, x.springfield);
    assert_eq!(r.tax.collected, m("17.75"));
    assert_eq!(r.tax.bad_debts, m("-9.60"));
    assert_eq!(r.tax.owed_at_end, m("8.15"));
    let cash = nov(h, &x, SalesTaxBasis::Cash);
    let r = row(&cash, x.springfield);
    assert_eq!(r.taxable_sales, m("91.85"));
    assert_eq!(r.tax.collected, m("8.15"));
    assert_eq!(r.tax.bad_debts, Money::ZERO, "no bad debts on cash");
    assert_eq!(r.tax.recoveries, Money::ZERO);

    // The write-off can be recovered.
    let rec = x.s.recoveries();
    let offs = h
        .run(rec.write_offs(Some(x.ozark), false))
        .expect("write-offs");
    let w = offs.iter().find(|w| w.invoice_id == id).expect("INV-J");
    assert_eq!(w.written_off, m("117.75"));
    assert_eq!(w.recoverable, m("117.75"));
    let new = NewRecovery {
        invoice_id: id,
        date: date(2026, 12, 15),
        amount: m("50.00"),
        memo: None,
    };
    let k = key();
    let got = h
        .run(rec.create(&new, &PostingOptions::default(), k.clone()))
        .expect("recovery");
    assert_eq!(got.recovered_tax, Some(m("4.08")));
    assert_eq!(got.invoice_balance_due, m("50.00"));
    let again = h
        .run(rec.create(&new, &PostingOptions::default(), k))
        .expect("replay");
    assert_eq!(again.id, got.id, "posted once");
    let t = x.txn(h, got.id);
    assert_eq!(Monett::net(&t, x.ar), m("50.00"));
    assert_eq!(Monett::net(&t, x.bad_debts), m("-45.92"));
    assert_eq!(Monett::net(&t, x.stp), m("-4.08"));
    assert_eq!(
        crate::contract_receive_payment::invoice_balance(h, &x.s, id),
        m("50.00"),
        "reopened"
    );
    let too_much = h
        .run(rec.create(
            &NewRecovery {
                amount: m("70.00"),
                ..new.clone()
            },
            &PostingOptions::default(),
            key(),
        ))
        .expect_err("more than is left");
    assert_eq!(
        recovery_refusal(&too_much),
        Some(RecoveryRefusal::ExceedsWriteOff)
    );
    assert_eq!(recovery_available(&too_much), Some(m("67.75")));
    let acc = dec(h, &x, SalesTaxBasis::Accrual);
    let r = row(&acc, x.springfield);
    assert_eq!(r.tax.recoveries, m("4.08"));
    assert_eq!(r.tax.owed_at_end, m("12.23"));

    // The reopened 50.00 is paid: on cash it collects 4.08.
    x.receive(
        h,
        x.ozark,
        date(2026, 12, 20),
        "50.00",
        &[(id, "50.00", None, None)],
    );
    let cash = dec(h, &x, SalesTaxBasis::Cash);
    let r = row(&cash, x.springfield);
    assert_eq!(r.tax.collected, m("4.08"));
    assert_eq!(r.tax.recoveries, Money::ZERO);
    assert_eq!(r.tax.owed_at_end, m("12.23"));

    // The payment that wrote it off can't bounce now.
    let now = h.run(x.s.customer_payments().get(pmt.id)).expect("PMT-J");
    let e = h
        .run(x.s.customer_payments().bounce(
            pmt.id,
            &BounceRequest {
                date: date(2026, 12, 21),
                bank_fee_amount: None,
                bank_fee_account_id: None,
                bank_fee_bank_account_id: None,
                memo: None,
                expected_updated_at: now.updated_at,
            },
            &PostingOptions::default(),
            key(),
        ))
        .expect_err("a recovered write-off");
    assert_eq!(
        payment_refusal(&e),
        Some(PaymentRefusal::WriteOffRecovered),
        "{e:?}"
    );
}

/// Golden 12's SSUTA-off variant and the forward-only switch (Q-P23w).
pub fn ssuta_off_and_forward_only(h: &Harness) {
    if !ssuta_caps(h) {
        return;
    }
    let x = monett(h, "SSUTA off", false);
    let first = x.plants(h, x.ozark, date(2026, 11, 3), "400.00").txn.id;
    let (p, pmt) = x.receive(
        h,
        x.ozark,
        date(2026, 11, 8),
        "426.79",
        &[(first, "426.79", Some("8.71"), None)],
    );
    assert!(
        p.invoice(first)
            .and_then(|r| r.discount_tax)
            .is_none_or(|d| d == Money::ZERO),
        "no tax share with SSUTA off"
    );
    let t = x.txn(h, pmt.id);
    assert_eq!(Monett::net(&t, x.stp), Money::ZERO);
    assert_eq!(Monett::net(&t, x.sales_discounts), m("8.71"));
    let l = nov(h, &x, SalesTaxBasis::Accrual);
    assert_eq!(row(&l, x.springfield).tax.discounts, Money::ZERO);
    assert_eq!(row(&l, x.springfield).tax.owed_at_end, m("35.50"));

    // On: the next payment's discount carries its tax.
    x.set_ssuta(h, true);
    let second = x.plants(h, x.ozark, date(2026, 11, 9), "400.00").txn.id;
    let (p, pmt2) = x.receive(
        h,
        x.ozark,
        date(2026, 11, 12),
        "426.79",
        &[(second, "426.79", Some("8.71"), None)],
    );
    assert_eq!(
        p.invoice(second).and_then(|r| r.discount_tax),
        Some(m("0.71"))
    );
    assert_eq!(Monett::net(&x.txn(h, pmt2.id), x.stp), m("0.71"));
    // Off again: what was posted while on stays; the first payment is
    // still without a share.
    x.set_ssuta(h, false);
    let l = nov(h, &x, SalesTaxBasis::Accrual);
    let r = row(&l, x.springfield);
    assert_eq!(r.tax.collected, m("71.00"));
    assert_eq!(r.tax.discounts, m("-0.71"));
    assert_eq!(r.tax.owed_at_end, m("70.29"));
    assert_eq!(Monett::net(&x.txn(h, pmt.id), x.stp), Money::ZERO);
}
