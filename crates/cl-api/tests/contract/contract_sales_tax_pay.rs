//! Pay Sales Tax, linked payments and "Mark paid through" against the real
//! engine (P23 plan §2.6, golden 9). Every figure is the engine's; the
//! post copies the preview's strings (`expectedOwed`) unchanged.
//!
//! - `pay_reduces_owed_exactly` (golden 9): the State agency through 11/30
//!   with a −1.96 filing discount pays 167.49 (lines sent without amounts:
//!   owed + adjustments, worked out by the engine); Dr STP 169.45, Cr Sales
//!   tax discounts 1.96, Cr bank 167.49; a replay with the same key posts
//!   once; owed is 0.00 and a later re-preview through 11/30 is 0.00 (S1).
//!   A stale `expectedOwed` is 409 `SALES_TAX_OWED_CHANGED`; the City pays
//!   50.00 electronically (a line amount) and owes 24.02; a 24.02 check
//!   debiting STP linked to it leaves 0.00, a second link is 409
//!   `ALREADY_LINKED`, moving its date is 409 `SALES_TAX_LINKED`, unlink
//!   owes 24.02 again; a void of the payment restores what it paid.
//! - `mark_paid_through_and_undo`: a run records what is owed as paid
//!   (no GL), a replay answers the same run, undo owes it again, a second
//!   undo is 409 `ALREADY_UNDONE`, an unknown run 404.

use cl_api::{
    ApiError, Capabilities, ConflictReason, LinkLine, LinkPaymentRequest, Method, PayAdjustment,
    PayLine, PaySalesTaxRefusal, PaySalesTaxRequest, PostingOptions, SalesTaxBasis,
    SalesTaxPayMethod, SalesTaxPaymentQuery, pay_sales_tax_refusal,
};
use cl_core::Money;
use cl_core::ids::TransactionId;
use serde_json::json;

use crate::common::{date, key, m};
use crate::contract_sales_tax_setup::{Monett, caps, code_of, monett, row};
use crate::support::Harness;

fn pay_caps(h: &Harness) -> bool {
    caps(h)
        && h.require_cap(Capabilities::SALES_TAX_PAY, "U-150")
        && h.require_cap(Capabilities::IDEMPOTENCY_SALES_TAX_PAY, "U-150")
}

fn request(
    x: &Monett,
    agency: cl_api::SalesTaxAgencyId,
    on: (i32, u32, u32),
) -> PaySalesTaxRequest {
    PaySalesTaxRequest {
        agency_id: agency,
        through_date: date(2026, 11, 30),
        date: date(on.0, on.1, on.2),
        bank_account_id: x.checking,
        method: SalesTaxPayMethod::Check,
        lines: None,
        adjustments: Vec::new(),
        print_later: true,
        check_number: None,
        memo: None,
        expected_owed: None,
    }
}

/// INV-A (State 80.70, City 74.02) and INV-E (Springfield 88.75).
fn seed(h: &Harness, label: &str) -> Monett {
    let x = monett(h, label, true);
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
    x.plants(h, x.ozark, date(2026, 11, 2), "1000.00");
    x
}

/// A check from Operating checking whose one line debits STP.
fn stp_check(
    h: &Harness,
    x: &Monett,
    on: &str,
    amount: &str,
) -> (TransactionId, serde_json::Value) {
    let body = json!({
        "bankAccountId": x.checking.to_string(),
        "payeeNameOnCheck": "Sample City Revenue Office",
        "txnDate": on,
        "amount": amount,
        "checkNumber": 7001,
        "lines": [{ "accountId": x.stp.to_string(), "amount": amount }],
    });
    let v = h
        .run(x.s.raw_mapped(Method::Post, "/api/v1/checks", Some(&body)))
        .unwrap_or_else(|e| panic!("check: {e}"));
    let id = v["check"]["id"]
        .as_str()
        .or_else(|| v["check"]["transaction"]["id"].as_str())
        .expect("check id")
        .parse()
        .expect("uuid");
    (id, body)
}

/// Golden 9.
#[allow(clippy::too_many_lines, reason = "the golden 9 walk")]
pub fn pay_reduces_owed_exactly(h: &Harness) {
    if !pay_caps(h) {
        return;
    }
    let x = seed(h, "Pay sales tax");
    let pays = x.s.sales_tax_payments();

    // State agency: State 80.70, Springfield 88.75 with a −1.96 discount.
    let mut r = request(&x, x.state_agency, (2026, 12, 10));
    r.adjustments = vec![PayAdjustment {
        tax_item_id: x.springfield,
        account_id: x.sales_tax_discounts,
        amount: m("-1.96"),
        reason: "Timely filing discount".into(),
    }];
    let p = h.run(pays.preview(&r)).expect("preview");
    assert!(p.problems.is_empty() || p.problems.iter().all(|q| q.code == "NO_VENDOR_ADDRESS"));
    let lines: Vec<_> = p
        .lines
        .iter()
        .map(|l| (l.tax_item_id, l.owed, l.adjustment, l.amount))
        .collect();
    assert!(lines.contains(&(Some(x.state), m("80.70"), Money::ZERO, m("80.70"))));
    assert!(lines.contains(&(Some(x.springfield), m("88.75"), m("-1.96"), m("86.79"))));
    assert_eq!(p.total, m("167.49"));
    // Post with the lines' expectedOwed only: the engine pays owed +
    // adjustments.
    r.lines = Some(
        p.lines
            .iter()
            .filter_map(|l| {
                Some(PayLine {
                    tax_item_id: l.tax_item_id?,
                    amount: None,
                    expected_owed: Some(l.expected_owed),
                })
            })
            .collect(),
    );
    r.expected_owed = p.expected_owed_total();
    assert_eq!(r.expected_owed, Some(m("169.45")));
    let k = key();
    let paid = h
        .run(pays.create(&r, &PostingOptions::default(), k.clone()))
        .expect("pay");
    assert_eq!(paid.print_status.as_deref(), Some("queue"));
    let replay = h
        .run(pays.create(&r, &PostingOptions::default(), k))
        .expect("replay");
    assert_eq!(replay.id, paid.id, "the replay answers the same payment");
    let listed = h
        .run(pays.list(SalesTaxPaymentQuery {
            agency_id: Some(x.state_agency),
            start_date: Some(date(2026, 12, 1)),
            end_date: Some(date(2026, 12, 31)),
        }))
        .expect("list");
    assert_eq!(
        listed.iter().filter(|p| p.id == paid.id).count(),
        1,
        "posted once"
    );
    let t = x.txn(h, paid.id);
    assert_eq!(Monett::net(&t, x.stp), m("169.45"), "Dr STP 80.70 + 88.75");
    assert_eq!(Monett::net(&t, x.sales_tax_discounts), m("-1.96"));
    assert_eq!(Monett::net(&t, x.checking), m("-167.49"));
    let rec = h.run(pays.get(paid.id)).expect("GET");
    assert_eq!(rec.total, m("167.49"));
    assert_eq!(rec.agency_id, x.state_agency);
    assert!(!rec.linked);
    assert!(!rec.stub_lines.is_empty(), "stub lines for the check");
    assert_eq!(x.owed(h, x.state_agency, date(2026, 11, 30)), Money::ZERO);
    // S1: a re-preview through 11/30 (any later date) owes nothing.
    let again = h
        .run(pays.preview(&request(&x, x.state_agency, (2026, 12, 11))))
        .expect("re-preview");
    assert_eq!(again.total, Money::ZERO);

    // City: a stale expectedOwed is refused.
    let mut c = request(&x, x.city_agency, (2026, 12, 10));
    c.method = SalesTaxPayMethod::Electronic;
    c.print_later = false;
    c.lines = Some(vec![PayLine {
        tax_item_id: x.city,
        amount: Some(m("50.00")),
        expected_owed: Some(m("70.00")),
    }]);
    c.expected_owed = Some(m("70.00"));
    let stale = h
        .run(pays.create(&c, &PostingOptions::default(), key()))
        .expect_err("stale expectedOwed");
    assert_eq!(
        pay_sales_tax_refusal(&stale),
        Some(PaySalesTaxRefusal::OwedChanged),
        "{stale:?}"
    );
    // 50.00 of 74.02, with a typed line amount.
    c.lines = Some(vec![PayLine {
        tax_item_id: x.city,
        amount: Some(m("50.00")),
        expected_owed: None,
    }]);
    c.expected_owed = None;
    let cp = h.run(pays.preview(&c)).expect("city preview");
    assert_eq!(cp.total, m("50.00"));
    c.lines = Some(cp.post_lines());
    c.expected_owed = cp.expected_owed_total();
    h.run(pays.create(&c, &PostingOptions::default(), key()))
        .expect("pay City 50.00");
    assert_eq!(x.owed(h, x.city_agency, date(2026, 11, 30)), m("24.02"));

    // A check made outside Pay Sales Tax, linked as the City's payment.
    let (chk, body) = stp_check(h, &x, "2026-12-12", "24.02");
    let link = LinkPaymentRequest {
        transaction_id: chk,
        agency_id: x.city_agency,
        lines: vec![LinkLine {
            tax_item_id: x.city,
            amount: m("24.02"),
        }],
    };
    let lk = key();
    h.run(pays.link(&link, &PostingOptions::default(), lk.clone()))
        .expect("link");
    h.run(pays.link(&link, &PostingOptions::default(), lk))
        .expect("a replayed link");
    assert_eq!(x.owed(h, x.city_agency, date(2026, 11, 30)), Money::ZERO);
    let twice = h
        .run(pays.link(&link, &PostingOptions::default(), key()))
        .expect_err("linked twice");
    assert_eq!(
        pay_sales_tax_refusal(&twice),
        Some(PaySalesTaxRefusal::AlreadyLinked)
    );
    assert!(h.run(pays.get(chk)).expect("linked record").linked);
    let dec = x.liability(
        h,
        SalesTaxBasis::Accrual,
        date(2026, 12, 1),
        date(2026, 12, 31),
    );
    assert_eq!(
        dec.other_entries,
        Some(Money::ZERO),
        "the check is a payment"
    );
    assert_eq!(row(&dec, x.city).tax.paid, m("74.02"), "50.00 + 24.02");
    // Moving the linked check's date is refused (unlink first).
    let mut moved = body.clone();
    moved["txnDate"] = json!("2026-12-13");
    let e = h
        .run(x.s.raw_mapped(Method::Put, &format!("/api/v1/checks/{chk}"), Some(&moved)))
        .expect_err("a linked check's date");
    assert!(
        matches!(
            e,
            ApiError::Conflict {
                reason: ConflictReason::SalesTaxLinked,
                ..
            }
        ),
        "{e:?}"
    );
    let un = h
        .run(pays.unlink(chk, &PostingOptions::default(), key()))
        .expect("unlink");
    assert_eq!(un.transaction_id, chk);
    assert_eq!(x.owed(h, x.city_agency, date(2026, 11, 30)), m("24.02"));
    // Unlinked, the date can move.
    h.run(x.s.raw_mapped(Method::Put, &format!("/api/v1/checks/{chk}"), Some(&moved)))
        .expect("edit after unlink");

    // A void gives back what the State payment paid.
    h.run(
        x.s.transactions()
            .void(paid.id, "Paid twice", &PostingOptions::default(), key()),
    )
    .expect("void");
    assert_eq!(x.owed(h, x.state_agency, date(2026, 11, 30)), m("169.45"));
    assert!(h.run(pays.get(paid.id)).expect("voided").is_void());
}

/// "Mark paid through" and its undo (the P23 review fix pass).
pub fn mark_paid_through_and_undo(h: &Harness) {
    if !pay_caps(h) {
        return;
    }
    let x = monett(h, "Mark paid through", true);
    x.plants(h, x.ozark, date(2026, 11, 2), "1000.00");
    let pays = x.s.sales_tax_payments();
    assert_eq!(x.owed(h, x.state_agency, date(2026, 11, 30)), m("88.75"));
    let k = key();
    let run = h
        .run(pays.mark_paid_through(
            x.state_agency,
            date(2026, 11, 30),
            Some("Filed before ClientLedger"),
            &PostingOptions::default(),
            k.clone(),
        ))
        .expect("mark paid through");
    assert_eq!(run.agency_id, x.state_agency);
    assert_eq!(run.through_date, date(2026, 11, 30));
    let lines: Vec<_> = run
        .lines
        .iter()
        .map(|l| (l.tax_item_id, l.amount))
        .collect();
    assert_eq!(lines, vec![(x.springfield, m("88.75"))]);
    assert_eq!(run.total, Some(m("88.75")));
    let run_id = run.run_id.clone().expect("runId");
    assert_eq!(x.owed(h, x.state_agency, date(2026, 11, 30)), Money::ZERO);
    let replay = h
        .run(pays.mark_paid_through(
            x.state_agency,
            date(2026, 11, 30),
            Some("Filed before ClientLedger"),
            &PostingOptions::default(),
            k,
        ))
        .expect("replay");
    assert_eq!(replay.run_id.as_deref(), Some(run_id.as_str()), "same run");
    assert_eq!(x.owed(h, x.state_agency, date(2026, 11, 30)), Money::ZERO);
    let undone = h
        .run(pays.undo_mark_paid_through(&run_id, &PostingOptions::default(), key()))
        .expect("undo");
    assert_eq!(undone.run_id.as_deref(), Some(run_id.as_str()));
    assert!(undone.undone_at.is_some());
    assert_eq!(x.owed(h, x.state_agency, date(2026, 11, 30)), m("88.75"));
    let twice = h
        .run(pays.undo_mark_paid_through(&run_id, &PostingOptions::default(), key()))
        .expect_err("undone twice");
    assert_eq!(
        code_of(&twice).as_deref(),
        Some("ALREADY_UNDONE"),
        "{twice:?}"
    );
    let unknown = h
        .run(pays.undo_mark_paid_through(
            "00000000-0000-4000-8000-000000000000",
            &PostingOptions::default(),
            key(),
        ))
        .expect_err("an unknown run");
    assert!(matches!(unknown, ApiError::NotFound { .. }), "{unknown:?}");
}
