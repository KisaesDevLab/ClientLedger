//! Enter Bill tests (WP4 extends; WP0 checks the skeleton's wiring).

use std::sync::Arc;

use cl_model::Scope;

use super::*;
use crate::windows::OpenPayload;

fn link() -> (tempfile::TempDir, Arc<CompanyLink>) {
    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    (dir, link)
}

#[test]
fn ctrl_b_opens_the_built_enter_bill_screen() {
    let (_d, link) = link();
    let today = CalendarDate::from_ymd(2026, 11, 3).expect("date");
    let cmd = cl_model::commands::find(KIND).expect("vendors.enter_bill");
    assert!(cmd.opens && cmd.arrives.is_none());
    let mut s = make(cmd.id, &link, None, today).expect("the screen");
    assert_eq!(s.title(), "Enter Bill");
    assert_eq!(s.scope(), Scope::Form);
    assert_eq!(s.state()["screen"], "enter_bill");
    // The skeleton takes no payload yet (WP4 adds BillPrefill / InboxDraft).
    let prefill = BillPrefill {
        kind: BillKind::Credit,
        vendor: ContactId::parse("00000000-0000-4000-8000-000000000001").expect("id"),
        date: today,
        ref_no: "INV-55871".into(),
        amount: Money::ZERO,
    };
    assert!(!format!("{prefill:?}").contains("INV"));
    assert!(!s.open_with(OpenPayload::BillPrefill(prefill)));
}
