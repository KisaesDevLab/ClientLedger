//! The demo finance-charge engine: golden case 6 of
//! `fixtures/receivables/golden.json` (formula, minimum, grace, open
//! credits, the period guard, charged-through, backdating, void frees the
//! days) and its compounding variant (P22 plan §6.1, §6.3).

use cl_api::{AssessContact, FinanceChargeRefusal, finance_charge_refusal};
use serde_json::Value;

use super::super::credit_memos::tests::fresh;
use super::super::credit_memos::{CEDAR_GAP, LOW_WATER, PRAIRIE_FORK, client_named};
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

fn set_up(s: &mut DemoState, compounding: bool) {
    set_settings(
        s,
        &FinanceChargeSettingsUpdate {
            annual_rate: Some(Some(AnnualRate::parse("18.00").expect("rate"))),
            minimum: Some(m("2.00")),
            grace_days: Some(10),
            on_finance_charges: Some(compounding),
            income_account_id: Some(Some(crate::company::demo::account_id(4000))),
            print_later: None,
        },
    );
}

fn row(p: &FinanceChargePreview, c: ContactId) -> &FinanceChargeRow {
    p.rows.iter().find(|r| r.contact_id == c).expect("row")
}

fn amounts(r: &FinanceChargeRow) -> Vec<(u32, Money)> {
    r.sources.iter().map(|x| (x.days, x.amount)).collect()
}

fn assess_one(
    s: &mut DemoState,
    date: &str,
    c: ContactId,
    charge: &str,
    key: &str,
) -> Result<AssessResult, ApiError> {
    assess(
        s,
        &AssessRequest {
            assessment_date: d(date),
            contacts: vec![AssessContact {
                contact_id: c,
                expected_charge: m(charge),
            }],
        },
        key,
        None,
    )
}

#[test]
fn golden_6_finance_charges() {
    let g = golden();
    let rows = &g["cases"][5]["steps"][0]["expect"]["rows"];
    let (mut s, _) = fresh();
    let pf = client_named(&mut s, PRAIRIE_FORK).expect("pf");
    let lw = client_named(&mut s, LOW_WATER).expect("lw");
    let cg = client_named(&mut s, CEDAR_GAP).expect("cg");
    let only = [pf, lw, cg];
    // Blank rate: the setup comes first (Q-P22a).
    let p = preview(&mut s, d("2026-10-31"), Some(&only)).expect("preview");
    assert!(p.not_set_up() && p.rows.is_empty());
    let e = assess_one(&mut s, "2026-10-31", pf, "15.68", "k0").expect_err("not set up");
    assert_eq!(
        finance_charge_refusal(&e),
        Some(FinanceChargeRefusal::NotSetUp)
    );
    set_up(&mut s, false);
    let p = preview(&mut s, d("2026-10-31"), Some(&only)).expect("preview");
    assert_eq!(p.period, "2026-10");
    let prairie = row(&p, pf);
    assert_eq!(prairie.charge, m(rows[0]["charge"].as_str().expect("s")));
    assert_eq!(amounts(prairie), vec![(31, m("15.29")), (16, m("0.39"))]);
    let low = row(&p, lw);
    assert_eq!(low.charge, m("2.00"));
    assert_eq!(amounts(low), vec![(16, m("0.24"))]);
    let cedar = row(&p, cg);
    assert_eq!(cedar.charge, m("9.17"));
    assert_eq!(cedar.has_open_credits, Some(m("400.00")));
    // Changed since the preview, then assessed; again with another key is
    // refused; the first key again answers the same invoices.
    let e = assess_one(&mut s, "2026-10-31", pf, "15.00", "kx").expect_err("changed");
    assert_eq!(
        finance_charge_refusal(&e),
        Some(FinanceChargeRefusal::Changed)
    );
    let oct = assess_one(&mut s, "2026-10-31", pf, "15.68", "k1").expect("assess");
    let fc_oct = oct.invoices[0].invoice_id;
    let inv = receivable(&s, fc_oct).expect("invoice");
    assert_eq!(
        (inv.date, inv.due),
        (d("2026-10-31"), Some(d("2026-10-31")))
    );
    assert_eq!(oct.invoices[0].total, m("15.68"));
    let e = assess_one(&mut s, "2026-10-31", pf, "15.68", "k2").expect_err("assessed");
    assert_eq!(
        finance_charge_refusal(&e),
        Some(FinanceChargeRefusal::Assessed)
    );
    assert_eq!(
        assess_one(&mut s, "2026-10-31", pf, "15.68", "k1").expect("replay"),
        oct
    );
    let again = preview(&mut s, d("2026-10-31"), Some(&[pf])).expect("preview");
    assert_eq!(
        row(&again, pf)
            .already_assessed
            .as_ref()
            .map(|a| a.invoice_id),
        Some(fc_oct)
    );
    // November from the charged-through date; the October charge itself is
    // not charged (compounding off).
    let p = preview(&mut s, d("2026-11-30"), Some(&[pf])).expect("preview");
    assert_eq!(row(&p, pf).charge, m("15.53"));
    assert_eq!(
        amounts(row(&p, pf)),
        vec![(30, m("14.79")), (30, m("0.74"))]
    );
    let nov = assess_one(&mut s, "2026-11-30", pf, "15.53", "k3").expect("assess");
    // Backdated after a later charge: nothing for days already charged.
    super::super::invoices::void(&mut s, fc_oct, None).expect("void October");
    let p = preview(&mut s, d("2026-10-31"), Some(&[pf])).expect("preview");
    assert_eq!(row(&p, pf).charge, m("0.00"));
    assert_eq!(row(&p, pf).sources.len(), 0);
    // Voiding November too frees the days: from the due dates again.
    super::super::invoices::void(&mut s, nov.invoices[0].invoice_id, None).expect("void Nov");
    let p = preview(&mut s, d("2026-11-30"), Some(&[pf])).expect("preview");
    assert_eq!(row(&p, pf).charge, m("31.21"));
    assert_eq!(
        amounts(row(&p, pf)),
        vec![(61, m("30.08")), (46, m("1.13"))]
    );
    assert_eq!(list(&s, "2026-10").len(), 1);
    assert!(list(&s, "2026-10")[0].voided);
}

#[test]
fn golden_6_compounding_charges_the_october_charge() {
    let (mut s, _) = fresh();
    let pf = client_named(&mut s, PRAIRIE_FORK).expect("pf");
    set_up(&mut s, true);
    let oct = assess_one(&mut s, "2026-10-31", pf, "15.68", "k1").expect("assess");
    let p = preview(&mut s, d("2026-11-30"), Some(&[pf])).expect("preview");
    let r = row(&p, pf);
    assert_eq!(r.charge, m("15.76"));
    assert_eq!(
        amounts(r),
        vec![(30, m("14.79")), (30, m("0.74")), (30, m("0.23"))]
    );
    assert_eq!(r.sources[2].invoice_id, oct.invoices[0].invoice_id);
}
