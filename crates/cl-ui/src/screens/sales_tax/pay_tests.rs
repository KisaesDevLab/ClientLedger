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

/// Picks an agency whose preview has two items or more.
fn two_items(s: &mut PayScreen) -> (SalesTaxItemId, SalesTaxItemId) {
    let agencies: Vec<SalesTaxAgencyId> = s.agencies.iter().map(|a| a.id).collect();
    for a in agencies {
        s.set_agency(Some(a));
        let items: Vec<SalesTaxItemId> = s
            .form
            .figures()
            .map(|f| f.lines.iter().filter_map(|l| l.item).collect())
            .unwrap_or_default();
        if items.len() >= 2 {
            return (items[0], items[1]);
        }
    }
    panic!("no agency with two items in the story");
}

fn owed_now(s: &PayScreen, item: SalesTaxItemId) -> Money {
    let mut d = s.link.demo_state().expect("demo");
    let o = crate::company::demo::sales_tax_owed::owed(
        &mut d,
        CalendarDate::from_ymd(2026, 11, 30).expect("d"),
        None,
        s.form.agency,
    )
    .expect("owed");
    o.agencies
        .iter()
        .flat_map(|a| a.items.iter())
        .find(|i| i.tax_item_id == Some(item))
        .map_or(Money::ZERO, |i| i.owed)
}

#[test]
fn an_untyped_item_is_paid_at_owed_plus_its_adjustment_not_a_frozen_figure() {
    // Review finding 5: the owner types an amount on one item, then adds a
    // -2.00 discount on another. The untyped item goes without an amount,
    // so the check is lowered by the discount and nothing ends at -2.00.
    let (_d, mut s) = screen();
    let (a, b) = two_items(&mut s);
    let owed_a = owed_now(&s, a);
    let owed_b = owed_now(&s, b);
    assert!(owed_b.is_positive());
    s.form.set_amount(a, "1.00");
    s.preview();
    assert!(s.form.figures().is_some());
    let row = s.form.add_adjustment();
    let account = s.accounts.first().map(|acct| acct.id);
    s.form.edit_adjustment(row, |adj| {
        adj.item = Some(b);
        adj.account = account;
        adj.amount = "-2.00".into();
        adj.reason = "Timely filing discount".into();
    });
    s.preview();
    let figs = s.form.figures().cloned().expect("figures");
    let line_b = figs.lines.iter().find(|l| l.item == Some(b)).expect("b");
    assert_eq!(
        line_b.amount,
        owed_b
            .checked_sub(Money::parse_wire("2.00").expect("m"))
            .expect("m"),
        "the engine's owed plus adjustment"
    );
    assert!(!figs.more_than_owed);
    let plan = s.form.post_plan().expect("plan");
    assert!(
        plan.lines
            .iter()
            .any(|(item, amt, _)| *item == b && amt.is_none()),
        "the untyped item is posted without an amount: {plan:?}"
    );
    s.pay();
    assert_eq!(s.state()["problem"], Value::Null, "{}", s.state());
    assert!(s.paid.is_some());
    assert_eq!(owed_now(&s, b), Money::ZERO, "no -2.00 left owed");
    assert_eq!(
        owed_now(&s, a),
        owed_a
            .checked_sub(Money::parse_wire("1.00").expect("m"))
            .expect("m")
    );
}

#[test]
fn an_amount_typed_before_the_first_preview_keeps_the_other_items() {
    let (_d, mut s) = screen();
    let (a, b) = two_items(&mut s);
    // A fresh form for the agency, typed before any preview came back.
    let agency = s.form.agency;
    s.set_agency(None);
    s.form.set_agency(agency);
    s.items_job = None;
    let items: Vec<SalesTaxItemId> = s
        .items
        .iter()
        .filter(|i| i.agency_id == agency)
        .map(|i| i.id)
        .collect();
    s.form.set_items(items);
    s.form.set_amount(a, "1.00");
    let r = s.form.request_plan().expect("plan");
    let sent = r.amounts.expect("lines");
    assert!(sent.contains(&(b, None)), "{sent:?}");
}

#[test]
fn paying_more_than_owed_asks_first() {
    let (_d, mut s) = screen();
    let (a, _) = two_items(&mut s);
    s.form.set_amount(a, "99999.00");
    s.preview();
    assert!(s.form.figures().is_some_and(|f| f.more_than_owed));
    s.pay();
    let v = s.state();
    assert_eq!(v["confirm_over"], true, "{v}");
    assert_eq!(v["paid"], Value::Null, "nothing posted yet");
    s.confirm_over_owed();
    let v = s.state();
    assert_eq!(v["confirm_over"], false);
    assert_eq!(v["paid"]["queued"], true, "{v}");
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

#[test]
fn each_write_target_has_its_own_idempotency_key() {
    // Review nit: a transport failure on one void must not hand its key to
    // the next, different void (or link).
    let (_d, mut s) = screen();
    let id = |s: &str| TransactionId::parse(s).expect("id");
    let (a, b) = (
        id("8a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01"),
        id("8a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a02"),
    );
    let k1 = s.key_for(KeyFor::Void(a));
    assert_eq!(
        s.key_for(KeyFor::Void(a)).as_header(),
        k1.as_header(),
        "a retry reuses it"
    );
    let k2 = s.key_for(KeyFor::Void(b));
    assert_ne!(
        k1.as_header(),
        k2.as_header(),
        "another payment gets another key"
    );
    let k3 = s.key_for(KeyFor::Unlink(b));
    assert_ne!(
        k2.as_header(),
        k3.as_header(),
        "another write on the same payment too"
    );
    let agency = s.form.agency.expect("agency");
    let d = CalendarDate::from_ymd(2026, 11, 30).expect("d");
    let k4 = s.key_for(KeyFor::MarkPaid(agency, d));
    assert_eq!(
        s.key_for(KeyFor::MarkPaid(agency, d)).as_header(),
        k4.as_header()
    );
    assert_ne!(
        s.key_for(KeyFor::MarkPaid(agency, d.add_days(-1).expect("d")))
            .as_header(),
        k4.as_header()
    );
}

#[test]
fn mark_paid_through_can_be_undone() {
    let (_d, mut s) = screen();
    let total = s.state()["total"].as_str().expect("total").to_string();
    s.ask_mark_paid();
    s.confirm_mark_paid();
    let v = s.state();
    assert_eq!(v["total"], "0.0000");
    assert_eq!(v["marked"]["undo"], true, "{v}");
    s.undo_mark_paid();
    let v = s.state();
    assert_eq!(v["notice"], "sales_tax.pay.mark_undone", "{v}");
    assert_eq!(v["marked"], Value::Null);
    assert_eq!(v["total"], total, "owed again");
}

#[test]
fn every_widget_is_named_in_both_themes() {
    for theme in super::super::THEMES {
        let (_d, mut s) = screen();
        let (a, b) = two_items(&mut s);
        s.form.set_amount(a, "1.00");
        s.amounts.insert(a, "1.00".into());
        let i = s.form.add_adjustment();
        s.form.edit_adjustment(i, |x| x.item = Some(b));
        s.preview();
        s.pay();
        s.form.remove_adjustment(i);
        s.preview();
        s.pay();
        s.open_link();
        s.confirm_mark = true;
        s.confirm_over = true;
        let unnamed = super::super::unnamed_in(&mut s, theme);
        assert!(unnamed.is_empty(), "{theme:?}: {unnamed:?}");
    }
}

#[test]
fn typed_amounts_are_unsaved_and_esc_closes_dialogs_first() {
    let (_d, mut s) = screen();
    assert!(!Screen::dirty(&s), "nothing typed");
    let (a, _) = two_items(&mut s);
    s.form.set_amount(a, "1.00");
    assert!(Screen::dirty(&s), "a typed amount is unsaved");
    s.confirm_mark = true;
    assert!(s.command(CommandId::new("form.revert")));
    assert!(!s.confirm_mark, "Esc closes the confirmation");
    s.open_link();
    assert!(s.command(CommandId::new("form.revert")));
    assert!(s.linking.is_none(), "then the match card");
    assert!(
        !s.command(CommandId::new("form.revert")),
        "then the shell closes the window (asking, as it is dirty)"
    );
    // The close prompt's Save pays; the window may close.
    s.preview();
    assert!(Screen::save(&mut s), "{}", s.state());
    assert!(!Screen::dirty(&s));
}
