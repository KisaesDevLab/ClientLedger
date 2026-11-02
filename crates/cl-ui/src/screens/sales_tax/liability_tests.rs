//! Sales Tax Owed over the demo engine.

use super::*;
use crate::company::demo::DemoState;

fn screen() -> (tempfile::TempDir, OwedScreen) {
    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    let s = OwedScreen::new(link, DemoState::today());
    (dir, s)
}

#[test]
fn the_story_reports_accrual_then_cash_hides_bad_debts() {
    let (_d, mut s) = screen();
    assert_eq!(s.state()["screen"], "sales_tax_owed");
    assert!(s.command(CommandId::new("dev.demo_sales_tax_story")));
    let v = s.state();
    assert_eq!(v["basis"], "accrual");
    assert_eq!(v["bad_debt_columns"], true);
    let accrual_state = v["state_item"]["collected"].clone();
    let accrual_springfield = v["springfield"]["collected"].clone();
    assert_ne!(accrual_state, "0.0000", "the Monett invoice is billed");
    assert_ne!(v["city"]["collected"], "0.0000");
    s.basis = Some(SalesTaxBasis::Cash);
    s.refresh();
    let v = s.state();
    assert_eq!(v["basis"], "cash");
    assert_eq!(v["bad_debt_columns"], false);
    assert_ne!(
        v["state_item"]["collected"], accrual_state,
        "only the sales receipt counts on cash; the Monett invoice is unpaid"
    );
    assert_eq!(
        v["springfield"]["collected"], accrual_springfield,
        "the Springfield invoice was paid in full on 11/04"
    );
}

#[test]
fn a_bad_range_is_a_plain_problem() {
    let (_d, mut s) = screen();
    s.from.set(DemoState::today());
    s.to.set(CalendarDate::from_ymd(2026, 1, 1).expect("d"));
    s.refresh();
    assert_eq!(s.state()["problem"], "sales_tax.owed.bad_range");
    assert_eq!(
        month_of(CalendarDate::from_ymd(2026, 12, 9).expect("d")).1,
        CalendarDate::from_ymd(2026, 12, 31).expect("d")
    );
}
