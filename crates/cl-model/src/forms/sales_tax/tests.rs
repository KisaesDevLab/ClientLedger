//! Sales tax form models (P23 plan §6.3): the editors, the pay and link
//! machines and the recovery dialog. Golden 9's figures.

use cl_core::ids::{AccountId, ContactId, SalesTaxAgencyId, SalesTaxItemId, TransactionId};
use cl_core::{CalendarDate, Money, TaxRate};

use super::exemption::{ExemptionForm, ExemptionProblem, ExemptionStrip};
use super::pay::{
    LinkForm, LinkProblem, PayFigures, PayForm, PayLineFigure, PayMethod, PayProblem,
};
use super::recovery::{RecoveryForm, RecoveryProblem};
use super::setup::{CodeEditor, GroupEditor, ItemEditor, ItemPlan, SetupProblem};

fn m(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}

fn d(mo: u32, day: u32) -> CalendarDate {
    CalendarDate::from_ymd(2026, mo, day).expect("date")
}

fn item(n: u8) -> SalesTaxItemId {
    SalesTaxItemId::parse(&format!("7c0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a{n:02}")).expect("id")
}

fn agency() -> SalesTaxAgencyId {
    SalesTaxAgencyId::parse("7a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01").expect("id")
}

fn account(n: u8) -> AccountId {
    AccountId::parse(&format!("2b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a{n:02}")).expect("id")
}

fn txn() -> TransactionId {
    TransactionId::parse("8a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01").expect("id")
}

#[test]
fn item_editor_parses_percents_exactly() {
    let mut e = ItemEditor {
        name: " Springfield ".into(),
        percent: "8.875".into(),
        agency: Some(agency()),
        ..ItemEditor::default()
    };
    assert_eq!(e.rate_label().as_deref(), Some("8.875%"));
    match e.plan().expect("plan") {
        ItemPlan::Item {
            name,
            rate,
            description,
            ..
        } => {
            assert_eq!(name, "Springfield");
            assert_eq!(rate, TaxRate::parse_wire("0.08875").expect("r"));
            assert_eq!(description, None);
        }
        ItemPlan::Group { .. } => panic!("an item"),
    }
    e.percent = "8.87501".into();
    assert_eq!(e.plan(), Err(SetupProblem::BadRate));
    e.percent = "100".into();
    assert_eq!(e.plan(), Err(SetupProblem::BadRate));
    e.percent = "8.875".into();
    e.agency = None;
    assert_eq!(e.plan(), Err(SetupProblem::AgencyRequired));
    e.name = " ".into();
    assert_eq!(e.plan(), Err(SetupProblem::NameRequired));
    let saved = ItemEditor::from_saved(
        "State",
        Some("Missouri"),
        TaxRate::parse_wire("0.04225").expect("r"),
        Some(agency()),
        true,
    );
    assert_eq!(saved.percent, "4.225");
    let mut edited = saved.clone();
    assert!(!edited.shows_forward_note(&saved));
    edited.percent = "4.5".into();
    assert!(
        edited.shows_forward_note(&saved),
        "in use: new documents only"
    );
}

#[test]
fn group_editor_holds_one_to_ten_distinct_items() {
    let mut g = GroupEditor::new("Monett", &[item(1), item(2), item(1)]);
    assert_eq!(g.members(), &[item(1), item(2)]);
    for n in 3..=10 {
        assert!(g.add(item(n)));
    }
    assert!(!g.add(item(11)), "full");
    assert!(g.plan().is_ok());
    let empty = GroupEditor::new("Empty", &[]);
    assert_eq!(empty.plan(), Err(SetupProblem::GroupEmpty));
    g.remove(item(1));
    assert_eq!(g.members().len(), 9);
}

#[test]
fn code_editor_takes_eight_characters_and_skips_system_codes() {
    let mut c = CodeEditor {
        code: " Res ".into(),
        taxable: false,
        ..CodeEditor::default()
    };
    let p = c.plan().expect("plan").expect("some");
    assert_eq!((p.code.as_str(), p.name.as_str()), ("Res", "Res"));
    c.code = "ABCDEFGHI".into();
    assert_eq!(c.plan(), Err(SetupProblem::CodeTooLong));
    c.is_system = true;
    assert_eq!(c.plan(), Ok(None));
}

fn golden9_figures() -> PayFigures {
    PayFigures {
        lines: vec![
            PayLineFigure {
                item: Some(item(1)),
                owed: m("80.70"),
                adjustment: Money::ZERO,
                amount: m("80.70"),
                expected_owed: m("80.70"),
            },
            PayLineFigure {
                item: Some(item(2)),
                owed: m("131.68"),
                adjustment: m("-1.96"),
                amount: m("129.72"),
                expected_owed: m("131.68"),
            },
            PayLineFigure {
                item: None,
                owed: m("3.00"),
                adjustment: Money::ZERO,
                amount: Money::ZERO,
                expected_owed: m("3.00"),
            },
        ],
        total: m("210.42"),
        owed: Some(m("212.38")),
        legacy_unassigned: true,
        more_than_owed: false,
    }
}

fn golden9_form() -> PayForm {
    let mut f = PayForm::new(Some(agency()), Some(d(11, 30)), Some(d(12, 10)));
    f.bank = Some(account(1));
    let i = f.add_adjustment();
    f.edit_adjustment(i, |a| {
        a.item = Some(item(2));
        a.account = Some(account(2));
        a.amount = "-1.96".into();
        a.reason = "Timely filing discount".into();
    });
    f
}

#[test]
fn pay_posts_the_previews_lines_and_expected_owed() {
    let mut f = golden9_form();
    let req = f.request_plan().expect("request");
    assert!(req.print_later && req.check_number.is_none());
    assert_eq!(req.amounts, None, "everything owed");
    assert_eq!(req.adjustments[0].amount, m("-1.96"));
    assert_eq!(f.post_plan(), Err(PayProblem::PreviewPending));
    let rev = f.revision();
    assert!(f.apply_preview(rev, golden9_figures()));
    let plan = f.post_plan().expect("post");
    assert_eq!(
        plan.lines,
        vec![(item(1), None, m("80.70")), (item(2), None, m("131.68")),],
        "nothing typed: no amounts (the engine pays owed plus adjustments); unassigned tax is not paid here"
    );
    assert_eq!(
        plan.expected_owed,
        m("212.38"),
        "the preview's owed, copied"
    );
    // A typed amount is sent with every other item without an amount
    // (the engine pays only the items sent, the rest at owed plus
    // adjustments, never frozen at an older preview's figure).
    f.set_amount(item(2), "100.00");
    assert_eq!(
        f.request_plan().expect("r").amounts,
        Some(vec![(item(1), None), (item(2), Some(m("100.00")))])
    );
    let rev = f.revision();
    let mut typed = golden9_figures();
    typed.lines[1].amount = m("100.00");
    assert!(f.apply_preview(rev, typed));
    assert_eq!(
        f.post_plan().expect("post").lines,
        vec![
            (item(1), None, m("80.70")),
            (item(2), Some(m("100.00")), m("131.68")),
        ]
    );
    f.set_amount(item(2), "");
    // Any change makes the figures stale; an old answer is dropped.
    f.set_amount(item(1), "50.00");
    assert_eq!(f.post_plan(), Err(PayProblem::PreviewPending));
    assert!(!f.apply_preview(rev, golden9_figures()));
    assert_eq!(
        f.request_plan().expect("r").amounts,
        Some(vec![(item(1), Some(m("50.00"))), (item(2), None)])
    );
    f.set_amount(item(1), "50.005");
    assert_eq!(f.request_plan(), Err(PayProblem::BadAmount), "no rounding");
    f.set_amount(item(1), "-1");
    assert_eq!(f.request_plan(), Err(PayProblem::BadAmount));
    f.set_amount(item(1), "");
    let rev = f.revision();
    let mut zero = golden9_figures();
    zero.total = Money::ZERO;
    assert!(f.apply_preview(rev, zero));
    assert_eq!(f.post_plan(), Err(PayProblem::NothingToPay));
}

#[test]
fn an_amount_typed_before_any_preview_still_sends_every_item() {
    // Review finding 5: with no preview yet the agency's items come from
    // the item list, so the untyped ones are not dropped.
    let mut f = golden9_form();
    f.set_items(vec![item(1), item(2)]);
    assert!(f.has_edits(), "the adjustment row is an edit");
    f.set_amount(item(1), "10.00");
    assert_eq!(
        f.request_plan().expect("r").amounts,
        Some(vec![(item(1), Some(m("10.00"))), (item(2), None)]),
        "the adjusted, untyped item goes without an amount"
    );
    // Another agency's lines go with it.
    f.set_agency(Some(
        SalesTaxAgencyId::parse("7a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a02").expect("id"),
    ));
    assert!(f.figures().is_none());
    assert!(!f.has_edits());
}

#[test]
fn paying_more_than_owed_needs_a_confirm_for_exactly_what_is_shown() {
    let mut f = golden9_form();
    f.set_amount(item(1), "90.00");
    let rev = f.revision();
    let mut over = golden9_figures();
    over.lines[0].amount = m("90.00");
    over.more_than_owed = true;
    assert!(f.apply_preview(rev, over.clone()));
    assert_eq!(f.post_plan(), Err(PayProblem::MoreThanOwed));
    f.confirm_more_than_owed();
    assert!(f.post_plan().is_ok());
    // Any change asks again.
    f.set_amount(item(1), "95.00");
    let rev = f.revision();
    assert!(f.apply_preview(rev, over));
    assert_eq!(f.post_plan(), Err(PayProblem::MoreThanOwed));
}

#[test]
fn pay_checks_the_check_and_adjustments() {
    let mut f = golden9_form();
    f.check_number = "1042".into();
    assert_eq!(f.request_plan(), Err(PayProblem::CheckNumberOrPrintLater));
    f.print_later = false;
    let r = f.request_plan().expect("r");
    assert_eq!((r.check_number, r.print_later), (Some(1042), false));
    f.check_number = "10a".into();
    assert_eq!(f.request_plan(), Err(PayProblem::BadCheckNumber));
    f.check_number = "0".into();
    assert_eq!(f.request_plan(), Err(PayProblem::BadCheckNumber));
    f.check_number.clear();
    let r = f.request_plan().expect("no number: the print queue");
    assert!(r.print_later && r.check_number.is_none());
    f.method = PayMethod::Electronic;
    let r = f.request_plan().expect("electronic");
    assert!(!r.print_later && r.check_number.is_none());
    f.edit_adjustment(0, |a| a.reason = " ".into());
    assert_eq!(f.request_plan(), Err(PayProblem::BadAdjustment));
    f.edit_adjustment(0, |a| {
        a.reason = "Penalty".into();
        a.amount = "0".into();
    });
    assert_eq!(f.request_plan(), Err(PayProblem::BadAdjustment));
    f.remove_adjustment(0);
    assert!(f.request_plan().is_ok());
    f.set_agency(None);
    assert_eq!(f.request_plan(), Err(PayProblem::NoAgency));
    assert!(!format!("{f:?}").contains("Penalty"));
}

#[test]
fn link_needs_a_payment_and_positive_parts() {
    let mut l = LinkForm::default();
    assert_eq!(l.plan(), Err(LinkProblem::NoPayment));
    l.payment = Some(txn());
    l.agency = Some(agency());
    assert_eq!(l.plan(), Err(LinkProblem::NoLines));
    l.set_amount(item(3), "24.02");
    assert_eq!(l.plan().expect("plan").lines, vec![(item(3), m("24.02"))]);
    l.set_amount(item(3), "0");
    assert_eq!(l.plan(), Err(LinkProblem::BadAmount));
}

#[test]
fn exemptions_check_number_and_state() {
    let mut e = ExemptionForm {
        client: Some(ContactId::parse("3a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01").expect("id")),
        number: " EX-99887766 ".into(),
        state: "mo".into(),
        expires_on: Some(d(12, 15)),
        ..ExemptionForm::default()
    };
    let p = e.plan().expect("plan");
    assert_eq!(p.state.as_deref(), Some("MO"));
    assert_eq!(p.number, "EX-99887766");
    assert!(!format!("{e:?}{p:?}").contains("99887766"));
    e.state = "Missouri".into();
    assert_eq!(e.plan(), Err(ExemptionProblem::BadState));
    e.state.clear();
    e.number.clear();
    assert_eq!(e.plan(), Err(ExemptionProblem::NumberRequired));
    assert_eq!(
        ExemptionStrip::from_codes(["EXEMPTION_MISSING", "EXEMPTION_EXPIRED"]),
        Some(ExemptionStrip::Expired)
    );
    assert_eq!(
        ExemptionStrip::from_codes(["EXEMPTION_EXPIRING", "OTHER"]),
        Some(ExemptionStrip::Expiring)
    );
    assert_eq!(ExemptionStrip::from_codes(["RATE_CHANGED"]), None);
}

#[test]
fn recovery_offers_the_engines_available_figure() {
    let mut r = RecoveryForm::new(txn(), d(12, 15));
    assert_eq!(r.plan(), Err(RecoveryProblem::BadAmount));
    r.set_amount("50.00");
    let first = r.plan().expect("plan");
    assert_eq!(first.amount, m("50.00"));
    r.apply_available(m("1234.50"));
    assert_eq!(r.amount_text(), "1234.50");
    let second = r.plan().expect("plan");
    assert_eq!(second.amount, m("1234.50"));
    assert_ne!(first.attempt, second.attempt, "a new save attempt");
    r.set_amount("12.345");
    assert_eq!(r.plan(), Err(RecoveryProblem::BadAmount));
}
