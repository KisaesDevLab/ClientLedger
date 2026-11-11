//! The sales tax liability report and owed figures against the real engine
//! (P23 plan §2.5, goldens 6 and 7). Every figure is the engine's decimal
//! string; the tests compare them with the golden values.
//!
//! - `liability_reconciles_to_gl` (AC1, accrual): goldens 1-4 plus INV-F
//!   (Res) and INV-G give State 80.70 (Non 1,170.00), City 74.02 and
//!   Springfield 105.05 (Res 500.00); the group invoice and the memo
//!   cancel; the agencies total 185.75 and 74.02; the balance check holds
//!   (STP = owed + unassigned + other entries + opening paid); owed by
//!   agency matches.
//! - `cash_running_total_golden_7` (Q-P23e): INV-E paid 362.92, 362.92 and
//!   362.91 is collected 29.58 + 29.59 in November and 29.58 in December
//!   (per-share rounding would lose a cent); unpaid invoices count nothing
//!   on cash; the November rounding difference is reported (6 places).

use cl_api::{SalesTaxBasis, SixPlaceAmount};
use cl_core::Money;

use crate::common::{date, m};
use crate::contract_sales_tax_setup::{Monett, caps, monett, row};
use crate::support::Harness;

fn sum(a: Money, b: Money) -> Money {
    a.checked_add(b).expect("sum")
}

/// The golden 6 documents (without the sales receipt and daily sales).
fn november(h: &Harness, x: &Monett) -> cl_core::ids::TransactionId {
    let e = x.plants(h, x.ozark, date(2026, 11, 2), "1000.00");
    x.plants(h, x.hollister, date(2026, 11, 3), "6.00");
    x.invoice(
        h,
        x.hollister,
        date(2026, 11, 4),
        vec![
            Monett::line(x.install_labor, "18", "65.00"),
            Monett::line(x.shrubs, "24", "32.50"),
            Monett::line(x.mulch, "6", "48.00"),
            Monett::line(x.perennials, "1", "842.15"),
        ],
    );
    crate::contract_credit_memos::create(
        h,
        &x.s,
        &cl_api::NewCreditMemo::new(
            x.hollister,
            date(2026, 11, 10),
            vec![Monett::line(x.plants, "1", "6.00")],
        ),
    );
    let f = x.plants(h, x.cedar, date(2026, 11, 14), "500.00");
    assert_eq!(f.tax_amount, Some(m("0.00")), "Res is not taxed");
    let g = x.plants(h, x.ozark, date(2026, 11, 18), "183.70");
    assert_eq!(g.tax_amount, Some(m("16.30")));
    e.txn.id
}

/// AC1: the report reconciles to the GL on accrual.
pub fn liability_reconciles_to_gl(h: &Harness) {
    if !caps(h) {
        return;
    }
    let x = monett(h, "Sales tax liability", true);
    november(h, &x);
    let l = x.liability(
        h,
        SalesTaxBasis::Accrual,
        date(2026, 11, 1),
        date(2026, 11, 30),
    );
    assert_eq!(l.basis, SalesTaxBasis::Accrual);
    let state = row(&l, x.state);
    assert_eq!(state.taxable_sales, m("1910.15"), "INV-B and CM-C cancel");
    assert_eq!(state.tax.collected, m("80.70"));
    assert_eq!(state.tax.owed_at_end, m("80.70"));
    assert_eq!(
        state.rate.map(cl_core::TaxRate::to_wire6),
        Some("0.042250".into())
    );
    let non: Vec<_> = state
        .non_taxable
        .iter()
        .map(|n| (n.code.clone(), n.amount))
        .collect();
    assert_eq!(non, vec![("Non".to_string(), m("1170.00"))]);
    let city = row(&l, x.city);
    assert_eq!(city.taxable_sales, m("1910.15"));
    assert_eq!(city.tax.collected, m("74.02"));
    let spr = row(&l, x.springfield);
    assert_eq!(spr.taxable_sales, m("1183.70"));
    assert_eq!(spr.tax.collected, m("105.05"));
    assert_eq!(spr.tax.discounts, Money::ZERO);
    assert_eq!(spr.tax.paid, Money::ZERO);
    let res: Vec<_> = spr.non_taxable.iter().map(|n| n.amount).collect();
    assert_eq!(res, vec![m("500.00")], "INV-F under Res");
    // The agencies' totals (the company's other P23 tests have their own
    // agencies; GL figures are not split by agency).
    let st = by_agency(h, &x, SalesTaxBasis::Accrual, x.state_agency);
    assert_eq!(st.agencies.len(), 1);
    assert_eq!(
        st.agencies[0].totals.collected,
        m("185.75"),
        "80.70 + 105.05"
    );
    assert_eq!(st.agencies[0].totals.owed_at_end, m("185.75"));
    assert!(st.gl_movement.is_none(), "no GL figures by agency");
    let ct = by_agency(h, &x, SalesTaxBasis::Accrual, x.city_agency);
    assert_eq!(ct.agencies[0].totals.collected, m("74.02"));
    // AC1 over the company: STP = owed + unassigned + other entries +
    // opening paid.
    let check = l.balance_check.as_ref().expect("balance check");
    let parts = [
        check.owed_at_end,
        check.unassigned,
        check.other_entries_cumulative,
        check.opening_paid,
    ]
    .into_iter()
    .fold(Money::ZERO, sum);
    assert_eq!(check.stp_balance, parts, "the balance check holds");
    assert!(l.gl_movement.is_some(), "the GL movement over the company");
    // Owed by agency (S1).
    assert_eq!(
        x.owed(h, x.state_agency, date(2026, 11, 30)),
        sum(m("80.70"), m("105.05"))
    );
    assert_eq!(x.owed(h, x.city_agency, date(2026, 11, 30)), m("74.02"));
}

/// November's report of one agency.
fn by_agency(
    h: &Harness,
    x: &Monett,
    basis: SalesTaxBasis,
    agency: cl_api::SalesTaxAgencyId,
) -> cl_api::SalesTaxLiability {
    h.run(x.s.reports().sales_tax_liability(cl_api::LiabilityQuery {
        start_date: date(2026, 11, 1),
        end_date: date(2026, 11, 30),
        basis: Some(basis),
        agency_id: Some(agency),
    }))
    .expect("one agency")
}

/// Golden 7: cash collected is the running-total share of each payment.
pub fn cash_running_total_golden_7(h: &Harness) {
    if !caps(h) || !h.require_cap(cl_api::Capabilities::CASH_BASIS_SPF, "U-152") {
        return;
    }
    let x = monett(h, "Sales tax cash", true);
    let e = november(h, &x);
    for (d, amt) in [
        (date(2026, 11, 20), "362.92"),
        (date(2026, 11, 25), "362.92"),
        (date(2026, 12, 2), "362.91"),
    ] {
        x.receive(h, x.ozark, d, amt, &[(e, amt, None, None)]);
    }
    let nov = x.liability(
        h,
        SalesTaxBasis::Cash,
        date(2026, 11, 1),
        date(2026, 11, 30),
    );
    assert_eq!(nov.basis, SalesTaxBasis::Cash);
    assert!(nov.hides_bad_debts());
    let spr = row(&nov, x.springfield);
    assert_eq!(spr.tax.collected, m("59.17"), "29.58 + 29.59");
    assert_eq!(spr.taxable_sales, m("666.67"));
    assert_eq!(spr.tax.bad_debts, Money::ZERO);
    assert_eq!(row(&nov, x.state).tax.collected, Money::ZERO, "unpaid");
    assert_eq!(row(&nov, x.city).tax.collected, Money::ZERO, "unpaid");
    // U-209: the cash ledger moves the tax in whole cents per settlement and
    // the exact column is that, so nothing is left over (golden 7's
    // roundingDifference 0.000000; it was -0.002790 before).
    let st = by_agency(h, &x, SalesTaxBasis::Cash, x.state_agency);
    assert_eq!(st.agencies[0].totals.collected, m("59.17"));
    if let Some(rd) = st.rounding_difference {
        assert_eq!(
            rd,
            SixPlaceAmount::parse("0.000000").expect("six places"),
            "{}",
            rd.to_text()
        );
    }
    let dec = x.liability(
        h,
        SalesTaxBasis::Cash,
        date(2026, 12, 1),
        date(2026, 12, 31),
    );
    assert_eq!(row(&dec, x.springfield).tax.collected, m("29.58"));
    assert_eq!(
        row(&dec, x.springfield).tax.owed_at_end,
        m("88.75"),
        "fully collected"
    );
    // Accrual is unchanged by the payments.
    let acc = x.liability(
        h,
        SalesTaxBasis::Accrual,
        date(2026, 11, 1),
        date(2026, 11, 30),
    );
    assert_eq!(row(&acc, x.springfield).tax.collected, m("105.05"));
}
