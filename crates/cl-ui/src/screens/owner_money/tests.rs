//! Pay Yourself or Put Money In tests (no engine).

use super::*;
use crate::screens::card_charge::tests::{lines_of, link, money, posted, today};
use cl_core::Money;
use cl_model::forms::owner_money::{Entity, Role};

fn screen(link: &Arc<CompanyLink>) -> OwnerMoneyScreen {
    let mut s = OwnerMoneyScreen::new(Arc::clone(link), None, today());
    s.poll();
    s
}

fn type_amount(s: &mut OwnerMoneyScreen, amount: &str) {
    let m = money(amount);
    s.header.edit_step(|h| h.amount = Some(m));
    s.sync_inputs();
}

#[test]
fn a_single_member_llc_pays_itself_from_owner_draws() {
    let (_d, link) = link();
    let mut s = screen(&link);
    assert_eq!(s.title(), "Pay Yourself or Put Money In");
    let st = s.state();
    assert_eq!(st["entity_type"], "single_member_llc");
    assert_eq!(st["note"], "owner_money.note.owner");
    assert_eq!(st["equity_source"], "engine");
    let draws = s.books.by_number("30170").expect("30170");
    assert_eq!(
        s.header.current().equity,
        Some(draws),
        "Recorded in: Owner's Draw"
    );
    type_amount(&mut s, "500.00");
    let mut register = link.bus().subscribe();
    let before = posted(&link);
    assert!(s.start_save(SaveIntent::Save, None), "{}", s.state());
    assert_eq!(posted(&link), before + 1);
    let (_, lines) = lines_of(&link, s.flow.last.expect("saved").id);
    assert!(lines.contains(&("30170".into(), money("500.00"), Money::ZERO)));
    assert!(lines.contains(&("1010".into(), Money::ZERO, money("500.00"))));
    let heard = link.bus().take(&mut register);
    assert!(heard.ledger_touches(draws));
    assert!(heard.ledger_touches(s.books.by_number("1010").expect("1010")));
}

#[test]
fn putting_money_in_deposits_from_owner_contributions() {
    let (_d, link) = link();
    let mut s = screen(&link);
    s.set_direction(Direction::PutMoneyIn);
    let contributions = s.books.by_number("30160").expect("30160");
    assert_eq!(s.header.current().equity, Some(contributions));
    type_amount(&mut s, "1000.00");
    assert!(s.start_save(SaveIntent::Save, None));
    let (kind, lines) = lines_of(&link, s.flow.last.expect("saved").id);
    assert_eq!(kind, cl_api::TxnType::Deposit);
    assert!(lines.contains(&("1010".into(), money("1000.00"), Money::ZERO)));
    assert!(lines.contains(&("30160".into(), Money::ZERO, money("1000.00"))));
}

#[test]
fn defaults_for_every_entity_type_and_the_remembered_choice() {
    let (_d, link) = link();
    let mut s = screen(&link);
    let a = |n: &str| s.books.by_number(n).expect("account");
    let (c30160, c30170) = (a("30160"), a("30170"));
    // The engine's roles for each type (as `equity-roles-v1` gives them).
    for (entity, out, inn) in [
        (Entity::SoleProp, Role::OwnerDraw, Role::OwnerContribution),
        (
            Entity::SingleMemberLlc,
            Role::OwnerDraw,
            Role::OwnerContribution,
        ),
        (
            Entity::Partnership,
            Role::PartnerDraws,
            Role::PartnerContributions,
        ),
        (
            Entity::SCorp,
            Role::ShareholderDistributions,
            Role::PaidInCapital,
        ),
        (
            Entity::CCorp,
            Role::ShareholderDistributions,
            Role::PaidInCapital,
        ),
    ] {
        s.books.entity = Some(entity);
        s.books.roles = [(out, c30170), (inn, c30160)].into();
        assert_eq!(
            s.default_equity(Direction::PayYourself),
            Some((c30170, EquitySource::Engine)),
            "{entity:?}"
        );
        assert_eq!(
            s.default_equity(Direction::PutMoneyIn),
            Some((c30160, EquitySource::Engine)),
            "{entity:?}"
        );
        assert_ne!(s.note_key(), "owner_money.note.unknown");
    }
    // Without the capability the owner picks once; it is remembered.
    s.books.roles.clear();
    s.books.roles_supported = false;
    assert_eq!(s.default_equity(Direction::PayYourself), None);
    s.set_equity(c30160);
    assert_eq!(
        s.default_equity(Direction::PayYourself),
        Some((c30160, EquitySource::Remembered))
    );
    let mut again = screen(&link);
    again.books.roles.clear();
    assert_eq!(
        again.default_equity(Direction::PayYourself),
        Some((c30160, EquitySource::Remembered)),
        "a new window starts on the remembered account"
    );
}

#[test]
fn a_check_needs_who_and_the_equity_cannot_be_the_bank() {
    let (_d, link) = link();
    let mut s = screen(&link);
    s.header
        .edit_step(|h| h.method = OwnerMethod::Check(String::new()));
    type_amount(&mut s, "25.00");
    let before = posted(&link);
    assert!(!s.start_save(SaveIntent::Save, None));
    assert_eq!(s.state()["save"]["refused"], "who_missing");
    let bank = s.header.current().bank;
    s.header.edit_step(|h| {
        h.method = OwnerMethod::Electronic;
        h.equity = bank;
    });
    assert!(!s.start_save(SaveIntent::Save, None));
    assert_eq!(s.state()["save"]["refused"], "same_account");
    assert_eq!(posted(&link), before);
}

#[test]
fn state_never_carries_amounts_or_names() {
    let (_d, link) = link();
    let mut s = screen(&link);
    type_amount(&mut s, "4321.09");
    let text = s.state().to_string();
    assert!(!text.contains("4321") && !text.contains("Kurt"), "{text}");
}
