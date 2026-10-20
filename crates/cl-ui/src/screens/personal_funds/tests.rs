//! Paid with Personal Funds tests (no engine).

use super::*;
use crate::screens::card_charge::tests::{lines_of, link, money, posted, today};
use cl_model::forms::lines::FormLine;
use cl_model::forms::owner_money::Role;

fn screen(link: &Arc<CompanyLink>) -> PersonalFundsScreen {
    let mut s = PersonalFundsScreen::new(Arc::clone(link), None, today());
    s.poll();
    s
}

fn fill(s: &mut PersonalFundsScreen, amount: &str, lines: &[(&str, &str)]) {
    let amount = money(amount);
    s.header.edit_step(|h| h.amount = Some(amount));
    let seeds: Vec<FormLine> = lines
        .iter()
        .map(|(a, m)| {
            let account = s.books.by_number(a).expect("account");
            let key = s.lines.form_mut().adapter_mut().keys.next_key();
            FormLine {
                account: Some(account),
                amount: Some(money(m)),
                ..FormLine::blank(key)
            }
        })
        .collect();
    let entry = FormLine::blank(s.lines.form_mut().adapter_mut().keys.next_key());
    s.lines.form_mut().edit_step(|v| {
        v.lines = seeds;
        v.lines.push(entry);
    });
    s.sync_inputs();
}

#[test]
fn a_single_member_llc_defaults_to_money_put_in_and_posts_from_3100() {
    let (_d, link) = link();
    let mut s = screen(&link);
    assert_eq!(s.title(), "Paid with Personal Funds");
    let st = s.state();
    assert_eq!(st["treatment"], "put_in");
    assert_eq!(st["default_label"], "personal_funds.default.smllc");
    assert_eq!(
        t("personal_funds.default.smllc"),
        "Default for a single-member LLC"
    );
    let c3100 = s.books.by_number("3100").expect("3100");
    assert_eq!(s.header.current().offset, Some(c3100));
    fill(&mut s, "42.18", &[("6250", "42.18")]);
    let mut register = link.bus().subscribe();
    let before = posted(&link);
    assert!(s.start_save(SaveIntent::Save, None), "{}", s.state());
    assert_eq!(posted(&link), before + 1);
    let (_, lines) = lines_of(&link, s.flow.last.expect("saved").id);
    assert!(lines.contains(&("3100".into(), Money::ZERO, money("42.18"))));
    assert!(lines.contains(&("6250".into(), money("42.18"), Money::ZERO)));
    assert!(
        link.bus().take(&mut register).ledger_touches(c3100),
        "the open 3100 register reloads"
    );
}

#[test]
fn defaults_for_all_five_entity_types() {
    let (_d, link) = link();
    let mut s = screen(&link);
    let c3100 = s.books.by_number("3100").expect("3100");
    for (entity, want) in [
        (Entity::SoleProp, Treatment::PutIn),
        (Entity::SingleMemberLlc, Treatment::PutIn),
        (Entity::Partnership, Treatment::PutIn),
        (Entity::SCorp, Treatment::PayBack),
        (Entity::CCorp, Treatment::PayBack),
    ] {
        s.books.entity = Some(entity);
        s.books.roles = [(Role::OwnerContribution, c3100)].into();
        assert_eq!(default_treatment(Some(entity)), want, "{entity:?}");
        assert_ne!(default_label(Some(entity)), default_label(None));
        let st = s.state_json();
        assert_eq!(st["treatment_default"], want.name());
    }
}

#[test]
fn pay_back_uses_a_liability_and_the_quick_add_is_remembered() {
    let (_d, link) = link();
    let mut s = screen(&link);
    s.set_treatment(Treatment::PayBack);
    assert_eq!(s.state()["treatment"], "pay_back");
    s.quick_add_due_to_owner();
    s.poll();
    let due = s.header.current().offset.expect("Due to owner picked");
    assert_eq!(s.books.class(due), Some(AccountClass::OtherLiability));
    // Remembered for this company: a new window starts on it.
    let mut again = screen(&link);
    again.set_treatment(Treatment::PayBack);
    assert_eq!(again.header.current().offset, Some(due));
    fill(&mut s, "19.99", &[("6250", "19.99")]);
    assert!(s.start_save(SaveIntent::Save, None), "{}", s.state());
    let (_, lines) = lines_of(&link, s.flow.last.expect("saved").id);
    assert!(lines.contains(&("2410".into(), Money::ZERO, money("19.99"))));
}

#[test]
fn lines_off_by_a_cent_save_nothing() {
    let (_d, link) = link();
    let mut s = screen(&link);
    fill(&mut s, "42.18", &[("6250", "42.17")]);
    let before = posted(&link);
    assert!(!s.start_save(SaveIntent::SaveAndClose, None));
    assert_eq!(posted(&link), before);
    assert_eq!(s.state()["save"]["refused"], "lines_disagree");
}

#[test]
fn a_saved_one_opens_with_prev_changes_in_place_and_voids() {
    let (_d, link) = link();
    let mut s = screen(&link);
    fill(&mut s, "42.18", &[("6250", "42.18")]);
    assert!(s.start_save(SaveIntent::SaveAndNew, None));
    let id = s.flow.last.expect("saved").id;
    s.command(CommandId::new("form.prev_record"));
    assert_eq!(s.flow.editing.map(|e| e.id), Some(id));
    let f = s.compose();
    assert_eq!(f.amount, Some(money("42.18")));
    assert_eq!(f.treatment, Treatment::PutIn);
    assert_eq!(f.offset, s.books.by_number("3100"));
    assert_eq!(s.state()["totals"], "balanced");
    let before = posted(&link);
    fill(&mut s, "40.00", &[("6250", "40.00")]);
    assert!(s.start_save(SaveIntent::Save, None), "{}", s.state());
    assert_eq!(posted(&link), before);
    let (_, lines) = lines_of(&link, id);
    assert!(lines.contains(&("3100".into(), Money::ZERO, money("40.00"))));
    s.command(CommandId::new("edit.void_transaction"));
    s.flow.void_reason = "paid twice".into();
    s.void_now(None);
    assert!(link.demo_state().expect("demo").ledger.is_void(id));
    assert_eq!(s.state()["record"], "new");
}

#[test]
fn esc_reverts_the_focused_field_first() {
    let (_d, link) = link();
    let mut s = screen(&link);
    s.focused = Some(field::MEMO);
    s.header.edit(field::MEMO, |h| h.memo = "x".into());
    assert!(s.command(CommandId::new("form.revert")));
    assert!(!s.is_dirty());
    assert!(!s.command(CommandId::new("form.revert")));
}
