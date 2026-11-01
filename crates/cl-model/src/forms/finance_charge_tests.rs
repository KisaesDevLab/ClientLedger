//! Finance charge form: ticks (open-credit rows start unticked, Q-P22s),
//! the setup problem and a plan that copies the preview's charge strings
//! (P22 plan §6.2, §6.3).

use super::*;
use proptest::prelude::*;

fn m(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}

fn d(m: u32, day: u32) -> CalendarDate {
    CalendarDate::from_ymd(2026, m, day).expect("date")
}

fn client(n: usize) -> ContactId {
    ContactId::parse(&format!("3a0c4e8e-6f6e-4c1d-8a37-{n:012}")).expect("id")
}

fn row(n: usize, charge: &str, chargeable: bool, credits: bool) -> ChargeRowFigure {
    ChargeRowFigure {
        client: client(n),
        charge: m(charge),
        chargeable,
        has_open_credits: credits,
    }
}

/// Golden case 6 on 10/31: Prairie Fork, Low Water, Cedar Gap (credits).
fn golden6() -> ChargeFigures {
    ChargeFigures {
        period: "2026-10".into(),
        rows: vec![
            row(1, "15.68", true, false),
            row(2, "2.00", true, false),
            row(3, "9.17", true, true),
            row(4, "0.00", false, false),
        ],
        not_set_up: false,
    }
}

#[test]
fn open_credit_rows_start_unticked_and_the_plan_copies_charges() {
    let mut f = FinanceChargeForm::new(Some(d(10, 31)));
    assert_eq!(f.assess_plan(), Err(FinanceChargeProblem::PreviewPending));
    assert!(f.apply_preview(d(10, 31), golden6()));
    assert!(f.is_ticked(client(1)) && f.is_ticked(client(2)));
    assert!(!f.is_ticked(client(3)), "Cedar Gap has open credits");
    f.toggle(client(4));
    assert!(!f.is_ticked(client(4)), "nothing to charge");
    let plan = f.assess_plan().expect("plan");
    assert_eq!(
        plan.chunks[0].clients,
        vec![(client(1), m("15.68")), (client(2), m("2.00"))]
    );
    f.toggle(client(3));
    assert_eq!(f.assess_plan().expect("plan").chunks[0].clients.len(), 3);
}

#[test]
fn not_set_up_another_date_and_nothing_ticked() {
    let mut f = FinanceChargeForm::new(Some(d(10, 31)));
    assert!(!f.apply_preview(d(11, 30), golden6()), "another date");
    let mut unset = golden6();
    unset.not_set_up = true;
    f.apply_preview(d(10, 31), unset);
    assert_eq!(f.assess_plan(), Err(FinanceChargeProblem::NotSetUp));
    f.apply_preview(d(10, 31), golden6());
    f.toggle(client(1));
    f.toggle(client(2));
    assert_eq!(f.assess_plan(), Err(FinanceChargeProblem::NothingTicked));
    f.set_date(Some(d(11, 30)));
    assert!(f.figures().is_none());
    f.set_date(None);
    assert_eq!(f.assess_plan(), Err(FinanceChargeProblem::NoDate));
}

proptest! {
    /// Every expectedCharge is the preview's string; chunks hold at most
    /// 500 clients.
    #[test]
    fn plan_charges_are_the_previews(cents in proptest::collection::vec(1i64..1_000_000, 1..1200)) {
        let mut f = FinanceChargeForm::new(Some(d(10, 31)));
        let rows: Vec<ChargeRowFigure> = cents.iter().enumerate().map(|(i, c)| row(i, &Money::from_cents(*c).expect("m").to_wire(), true, false)).collect();
        f.apply_preview(d(10, 31), ChargeFigures { period: "2026-10".into(), rows: rows.clone(), not_set_up: false });
        let plan = f.assess_plan().expect("plan");
        let sent: Vec<(ContactId, Money)> = plan.chunks.iter().flat_map(|c| c.clients.clone()).collect();
        prop_assert!(plan.chunks.iter().all(|c| c.clients.len() <= ASSESS_CHUNK));
        prop_assert_eq!(sent.len(), rows.len());
        for ((c, x), r) in sent.iter().zip(&rows) {
            prop_assert_eq!(*c, r.client);
            prop_assert_eq!(x.to_wire(), r.charge.to_wire());
        }
    }
}
