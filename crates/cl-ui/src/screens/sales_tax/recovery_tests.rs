//! Recover a Bad Debt over the demo engine (golden 13's write-off).

use super::*;
use crate::company::demo::DemoState;

#[test]
fn a_recovery_reopens_the_invoice_and_too_much_offers_what_is_left() {
    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    let mut s = RecoveryScreen::new(Arc::clone(&link), DemoState::today());
    assert_eq!(s.state()["invoices"], 0, "nothing written off yet");
    let Some(invoice) = link
        .demo_state()
        .and_then(|mut d| crate::company::demo::sales_tax_owed::write_off_story(&mut d))
    else {
        panic!("story");
    };
    s.set_client(None);
    assert_eq!(s.state()["invoices"], 1);
    assert_eq!(s.form.invoice, Some(invoice));
    s.set_amount("500.00");
    s.save();
    assert_eq!(s.state()["problem"], true, "more than was written off");
    assert_eq!(s.form.amount_text(), "117.75");
    s.set_amount("50.00");
    s.save();
    let v = s.state();
    assert_eq!(v["done"]["balance"], "50.0000", "{v}");
    assert_eq!(v["done"]["tax"], "4.0800", "golden 13: x = 4.08");
}

#[test]
fn every_widget_is_named_in_both_themes() {
    for theme in super::super::THEMES {
        let dir = tempfile::tempdir().expect("tmp");
        let link = Arc::new(CompanyLink::demo(dir.path()));
        let mut s = RecoveryScreen::new(Arc::clone(&link), DemoState::today());
        if let Some(mut d) = link.demo_state() {
            let _ = crate::company::demo::sales_tax_owed::write_off_story(&mut d);
        }
        s.set_client(None);
        let unnamed = super::super::unnamed_in(&mut s, theme);
        assert!(unnamed.is_empty(), "{theme:?} before: {unnamed:?}");
        s.set_amount("10.00");
        s.save();
        assert!(s.done.is_some());
        let unnamed = super::super::unnamed_in(&mut s, theme);
        assert!(unnamed.is_empty(), "{theme:?} saved: {unnamed:?}");
    }
}

#[test]
fn a_typed_amount_is_unsaved_until_recorded() {
    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    let mut s = RecoveryScreen::new(Arc::clone(&link), DemoState::today());
    if let Some(mut d) = link.demo_state() {
        let _ = crate::company::demo::sales_tax_owed::write_off_story(&mut d);
    }
    s.set_client(None);
    assert!(!Screen::dirty(&s));
    s.set_amount("10.00");
    assert!(Screen::dirty(&s), "an amount typed is unsaved");
    assert!(
        !s.command(CommandId::new("form.revert")),
        "Esc: the shell closes the window, asking first"
    );
    assert!(Screen::save(&mut s), "the close prompt's Save records it");
    assert!(s.done.is_some());
    assert!(!Screen::dirty(&s));
}
