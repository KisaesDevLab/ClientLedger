//! Pay Sales Tax over the demo engine.

use super::*;
use crate::company::demo::DemoState;

fn screen() -> (tempfile::TempDir, PayScreen) {
    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    if let Some(mut d) = link.demo_state() {
        crate::company::demo::sales_tax_owed::story(&mut d);
    }
    let mut s = PayScreen::new(link, DemoState::today());
    // Through the end of November (the story is dated 11/02-11/04).
    s.through
        .set(CalendarDate::from_ymd(2026, 11, 30).expect("d"));
    s.form.through = s.through.value;
    s.form.changed();
    s.preview();
    (dir, s)
}

#[test]
fn pay_posts_the_previews_lines_then_owed_is_zero_and_void_restores() {
    let (_d, mut s) = screen();
    let v = s.state();
    assert_eq!(v["screen"], "pay_sales_tax");
    assert_eq!(v["agency"], true, "the first agency is picked");
    assert_eq!(v["print_later"], true, "a check printed later by default");
    let total = v["total"].as_str().expect("total").to_string();
    assert_ne!(total, "0.0000");
    s.pay();
    let v = s.state();
    assert_eq!(v["problem"], Value::Null, "{v}");
    assert_eq!(v["paid"]["queued"], true);
    assert_eq!(v["history"], 1);
    assert_eq!(v["total"], "0.0000", "everything owed was paid");
    let id = s.history[0].id;
    s.void(id);
    let v = s.state();
    assert_eq!(v["voided"], 1);
    assert_eq!(v["total"], total, "owed again");
}

#[test]
fn an_adjustment_lowers_the_check_and_needs_a_reason() {
    let (_d, mut s) = screen();
    let item = s.form.figures().and_then(|f| f.lines[0].item);
    assert!(item.is_some());
    let i = s.form.add_adjustment();
    s.form.edit_adjustment(i, |a| {
        a.item = item;
        a.amount = "-1.00".into();
    });
    s.pay();
    assert_eq!(s.state()["problem"], "pay_sales_tax.problem.bad_adjustment");
    let account = s.accounts.first().map(|a| a.id);
    s.form.edit_adjustment(i, |a| {
        a.account = account;
        a.reason = "Timely filing discount".into();
    });
    s.preview();
    assert!(
        s.form.figures().is_some(),
        "{} {:?}",
        s.state(),
        s.form.request_plan()
    );
    s.pay();
    assert_eq!(s.state()["paid"]["queued"], true, "{}", s.state());
}

#[test]
fn a_stale_expected_owed_previews_again_with_a_message() {
    let (_d, mut s) = screen();
    let plan = s.form.post_plan().expect("plan");
    // Another window pays first.
    s.send(Pending::Pay(plan.clone()), None);
    assert!(s.paid.is_some());
    s.paid = None;
    s.key = None;
    s.send(
        Pending::Pay(PayPostPlan {
            attempt: plan.attempt + 99,
            ..plan
        }),
        None,
    );
    assert_eq!(s.state()["notice"], "sales_tax.pay.owed_changed");
}

#[test]
fn a_check_with_no_number_is_queued_and_a_typed_one_is_not() {
    let (_d, mut s) = screen();
    s.form.print_later = false;
    s.form.check_number = "5001".into();
    s.form.changed();
    s.preview();
    s.pay();
    let v = s.state();
    assert_eq!(v["paid"]["queued"], false, "{v}");
    assert_eq!(s.history[0].check_number, Some(5001));
}

#[test]
fn mark_paid_through_asks_first_then_nothing_is_owed() {
    let (_d, mut s) = screen();
    let total = s.state()["total"].as_str().expect("total").to_string();
    assert_ne!(total, "0.0000");
    s.ask_mark_paid();
    let v = s.state();
    assert_eq!(
        v["confirm_mark"], true,
        "a confirmation, nothing written yet"
    );
    assert_eq!(v["total"], total);
    s.confirm_mark_paid();
    let v = s.state();
    assert_eq!(v["confirm_mark"], false);
    assert_eq!(v["notice"], "sales_tax.pay.marked_paid", "{v}");
    assert_eq!(v["total"], "0.0000", "owed through the date is now zero");
    assert_eq!(v["history"], 0, "no check or payment is made");
}

#[test]
fn a_pay_sales_tax_payment_is_voided_not_unlinked() {
    let (_d, mut s) = screen();
    let total = s.state()["total"].as_str().expect("total").to_string();
    s.pay();
    let paid = s.history[0].id;
    s.unlink(paid);
    assert_eq!(s.state()["problem"], "sales_tax.pay.refusal.not_linked");
    s.void(paid);
    assert_eq!(s.state()["total"], total, "owed again");
}
