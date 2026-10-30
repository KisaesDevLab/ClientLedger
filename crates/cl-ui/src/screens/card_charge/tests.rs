//! Record Card Charge tests (no engine): the demo stand-in feeds the
//! screen; the demo ledger's size proves when nothing was sent.

use super::*;

use crate::company::demo::register::DEMO_CLOSING_PASSWORD;
use crate::screens::company::form::SecretText;

pub(crate) fn link() -> (tempfile::TempDir, Arc<CompanyLink>) {
    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    (dir, link)
}

pub(crate) fn today() -> CalendarDate {
    CalendarDate::from_ymd(2026, 11, 4).expect("date")
}

pub(crate) fn money(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}

/// How many transactions the demo engine holds.
pub(crate) fn posted(link: &Arc<CompanyLink>) -> usize {
    link.demo_state().expect("demo").ledger.transactions().len()
}

/// The demo engine's record of `id`: (account number, debit, credit).
pub(crate) fn lines_of(
    link: &Arc<CompanyLink>,
    id: cl_core::ids::TransactionId,
) -> (cl_api::TxnType, Vec<(String, Money, Money)>) {
    let mut demo = link.demo_state().expect("demo");
    let chart = demo
        .chart
        .get_or_insert_with(crate::company::demo::chart)
        .clone();
    let contacts = demo.contacts_mut().clone();
    let t = demo.ledger.get(&chart, &contacts, id).expect("txn").record;
    (
        t.txn_type,
        t.lines
            .iter()
            .map(|l| {
                (
                    l.account_number.clone().unwrap_or_default(),
                    l.debit,
                    l.credit,
                )
            })
            .collect(),
    )
}

pub(super) fn screen(link: &Arc<CompanyLink>) -> CardChargeScreen {
    let mut s = CardChargeScreen::new(Arc::clone(link), None, today());
    s.poll();
    s
}

pub(super) fn fill(s: &mut CardChargeScreen, amount: &str, lines: &[(&str, &str)]) {
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
fn opens_on_the_card_with_the_balance_owed() {
    let (_d, link) = link();
    let s = screen(&link);
    assert_eq!(s.card(), s.books.by_number("2050"));
    assert_eq!(s.title(), "Record Card Charge");
    let st = s.state();
    assert_eq!(st["screen"], "card_charge");
    assert_eq!(st["mode"], "charge");
    assert_eq!(st["balance_owed_shown"], true);
    assert!(!s.is_dirty());
}

#[test]
fn a_split_off_by_a_dime_saves_nothing_then_saves_exactly() {
    let (_d, link) = link();
    let mut s = screen(&link);
    let mut register = link.bus().subscribe();
    fill(&mut s, "64.20", &[("6250", "40.00"), ("6400", "24.10")]);
    assert_eq!(s.state()["totals"], "remaining");
    let before = posted(&link);
    assert!(!s.start_save(SaveIntent::SaveAndClose, None));
    assert_eq!(posted(&link), before, "nothing reached the engine");
    assert_eq!(s.state()["save"]["refused"], "lines_disagree");
    // Fixing the amount takes the refusal away at once (never shown next
    // to the agreeing total).
    fill(&mut s, "64.10", &[("6250", "40.00"), ("6400", "24.10")]);
    s.recheck_problems();
    assert!(s.state()["save"]["refused"].is_null());
    assert_eq!(s.state()["save"]["problems"], 0);
    // Fix the amount: saved, and every window hears the card and the lines.
    fill(&mut s, "64.10", &[("6250", "40.00"), ("6400", "24.10")]);
    assert!(s.start_save(SaveIntent::Save, None));
    assert_eq!(posted(&link), before + 1);
    let id = s.flow.last.expect("saved").id;
    let (kind, lines) = lines_of(&link, id);
    assert_eq!(kind, cl_api::TxnType::Expense);
    assert!(lines.contains(&("2050".into(), Money::ZERO, money("64.10"))));
    assert!(lines.contains(&("6250".into(), money("40.00"), Money::ZERO)));
    let heard = link.bus().take(&mut register);
    let card = s.card().expect("card");
    assert!(heard.ledger_touches(card), "the open card register reloads");
    assert!(heard.ledger_touches(s.books.by_number("6400").expect("6400")));
    assert_eq!(s.state()["published_accounts"], 3);
    assert!(!s.is_dirty());
}

#[test]
fn a_credit_posts_as_a_deposit_into_the_card() {
    let (_d, link) = link();
    let mut s = screen(&link);
    s.header.edit_step(|h| h.ref_no = "INV-7".into());
    s.set_mode(CardMode::Credit);
    assert_eq!(s.title(), "Record Card Credit");
    assert_eq!(
        s.header.current().ref_no,
        "",
        "a credit has no ref no.: emptied, not dropped at the save"
    );
    // Back to Charge with Undo: the number returns.
    assert!(s.header.undo());
    assert_eq!(s.header.current().ref_no, "INV-7");
    assert!(s.header.redo());
    fill(&mut s, "15.00", &[("6250", "15.00")]);
    // A billable line is refused on a credit (nothing sent).
    s.lines.form_mut().edit_step(|v| v.lines[0].billable = true);
    let before = posted(&link);
    assert!(!s.start_save(SaveIntent::Save, None));
    assert_eq!(posted(&link), before);
    assert_eq!(s.state()["save"]["refused"], "billable_not_allowed");
    s.lines
        .form_mut()
        .edit_step(|v| v.lines[0].billable = false);
    assert!(s.start_save(SaveIntent::Save, None));
    let (kind, lines) = lines_of(&link, s.flow.last.expect("saved").id);
    assert_eq!(kind, cl_api::TxnType::Deposit);
    assert!(lines.contains(&("2050".into(), money("15.00"), Money::ZERO)));
    assert!(lines.contains(&("6250".into(), Money::ZERO, money("15.00"))));
}

#[test]
fn a_closed_period_asks_for_the_password_and_void_works_on_the_saved_one() {
    let (_d, link) = link();
    let mut s = screen(&link);
    fill(&mut s, "12.00", &[("6250", "12.00")]);
    let closed = CalendarDate::from_ymd(2026, 9, 15).expect("date");
    s.header.edit_step(|h| h.date = Some(closed));
    s.sync_inputs();
    let before = posted(&link);
    assert!(!s.start_save(SaveIntent::Save, None));
    assert_eq!(posted(&link), before, "asked first, nothing sent");
    assert!(s.flow.closing_prompt.dialog_open());
    s.flow.closing_prompt.password = SecretText::from("wrong");
    s.submit_closing();
    assert_eq!(posted(&link), before, "a wrong password saves nothing");
    s.flow.closing_prompt.password = SecretText::from(DEMO_CLOSING_PASSWORD);
    s.submit_closing();
    assert_eq!(posted(&link), before + 1);
    assert!(!s.flow.closing_prompt.dialog_open());
    // Void: a reason, then the password (closed period).
    assert!(s.command(CommandId::new("edit.void_transaction")));
    assert_eq!(s.state()["void_open"], true);
    s.flow.void_reason = "entered twice".into();
    s.void_now(None);
    assert!(s.flow.closing_prompt.dialog_open());
    s.flow.closing_prompt.password = SecretText::from(DEMO_CLOSING_PASSWORD);
    s.submit_closing();
    assert_eq!(s.state()["last_action"], "void");
}

/// Saves a charge of `amount` on one 6250 line with Save & New; its id.
fn save_charge(s: &mut CardChargeScreen, amount: &str) -> cl_core::ids::TransactionId {
    fill(s, amount, &[("6250", amount)]);
    assert!(s.start_save(SaveIntent::SaveAndNew, None), "{}", s.state());
    s.flow.last.expect("saved").id
}

#[test]
fn void_and_edit_tell_the_accounts_the_record_touches_in_the_books() {
    let (_d, link) = link();
    let mut s = screen(&link);
    fill(&mut s, "8.00", &[("6400", "8.00")]);
    assert!(s.start_save(SaveIntent::SaveAndNew, None));
    let first = s.flow.last.expect("saved").id;
    save_charge(&mut s, "9.00");
    let a6400 = s.books.by_number("6400").expect("6400");
    let a6250 = s.books.by_number("6250").expect("6250");
    assert!(s.open_record(first));
    // The owner moves the line to 6250 but doesn't save: an edit's save
    // would tell 6400 (moved off) too.
    s.lines
        .form_mut()
        .edit_step(|v| v.lines[0].account = Some(a6250));
    assert!(s.flow.record_accounts.contains(&a6400));
    // Void tells the record's accounts in the books (6400), not the form's.
    let mut other = link.bus().subscribe();
    s.flow.void_reason = "wrong card".into();
    s.void_now(None);
    assert_eq!(s.state()["last_action"], "void");
    let heard = link.bus().take(&mut other);
    assert!(heard.ledger_touches(a6400), "the voided line's account");
    assert!(heard.ledger_touches(s.card().expect("card")));
}

#[test]
fn an_edit_tells_the_account_a_line_moved_off() {
    let (_d, link) = link();
    let mut s = screen(&link);
    fill(&mut s, "8.00", &[("6400", "8.00")]);
    assert!(s.start_save(SaveIntent::Save, None));
    let a6400 = s.books.by_number("6400").expect("6400");
    let a6250 = s.books.by_number("6250").expect("6250");
    s.lines
        .form_mut()
        .edit_step(|v| v.lines[0].account = Some(a6250));
    let mut other = link.bus().subscribe();
    assert!(s.start_save(SaveIntent::Save, None), "{}", s.state());
    let heard = link.bus().take(&mut other);
    assert!(heard.ledger_touches(a6400) && heard.ledger_touches(a6250));
}

#[test]
fn prev_and_next_step_through_saved_charges_with_exact_amounts() {
    let (_d, link) = link();
    let mut s = screen(&link);
    let first = save_charge(&mut s, "11.25");
    let second = save_charge(&mut s, "22.50");
    assert_eq!(s.state()["record"], "new");
    // Prev: the newest saved one, read back exactly.
    assert!(s.command(CommandId::new("form.prev_record")));
    assert_eq!(s.flow.editing.map(|e| e.id), Some(second));
    assert_eq!(s.state()["record"], "edit");
    assert_eq!(s.header.current().amount, Some(money("22.50")));
    let row = s.lines.value().used().next().expect("line").clone();
    assert_eq!(row.amount, Some(money("22.50")));
    assert_eq!(row.account, s.books.by_number("6250"));
    assert_eq!(s.state()["totals"], "balanced");
    assert!(!s.is_dirty(), "an opened record is clean");
    // Prev again: the older one; Next: back; Next again: a new form.
    s.command(CommandId::new("form.prev_record"));
    assert_eq!(s.flow.editing.map(|e| e.id), Some(first));
    s.command(CommandId::new("form.next_record"));
    assert_eq!(s.flow.editing.map(|e| e.id), Some(second));
    s.command(CommandId::new("form.next_record"));
    assert_eq!(s.state()["record"], "new");
    assert_eq!(s.header.current().amount, None);
}

#[test]
fn an_opened_charge_is_changed_in_place_and_voided() {
    let (_d, link) = link();
    let mut s = screen(&link);
    let id = save_charge(&mut s, "40.00");
    // Open it from the register (CompanyEvent::OpenRecord).
    let mut other = screen(&link);
    assert!(Screen::open_record(&mut other, id));
    assert_eq!(other.flow.editing.map(|e| e.id), Some(id));
    let before = posted(&link);
    fill(&mut other, "41.10", &[("6250", "41.10")]);
    assert!(
        other.start_save(SaveIntent::Save, None),
        "{}",
        other.state()
    );
    assert_eq!(posted(&link), before, "the same record, not a new one");
    let (_, lines) = lines_of(&link, id);
    assert!(lines.contains(&("2050".into(), Money::ZERO, money("41.10"))));
    assert!(lines.contains(&("6250".into(), money("41.10"), Money::ZERO)));
    // Void works on the opened record, not only one saved in this window.
    let mut third = screen(&link);
    assert!(third.open_saved(id));
    assert!(third.command(CommandId::new("edit.void_transaction")));
    third.flow.void_reason = "duplicate".into();
    third.void_now(None);
    assert!(link.demo_state().expect("demo").ledger.is_void(id));
    assert_eq!(third.state()["record"], "new");
    // A void record opens read-only and refuses a save.
    let mut fourth = screen(&link);
    fourth.open_saved(id);
    assert_eq!(fourth.state()["read_only"], "helper.read_only.void");
    fill(&mut fourth, "5.00", &[("6250", "5.00")]);
    assert!(!fourth.start_save(SaveIntent::Save, None));
    assert_eq!(fourth.state()["notice"], "helper.read_only.void");
}

#[test]
fn a_record_changed_elsewhere_is_not_overwritten() {
    let (_d, link) = link();
    let mut s = screen(&link);
    let id = save_charge(&mut s, "30.00");
    let mut a = screen(&link);
    a.open_saved(id);
    let mut b = screen(&link);
    b.open_saved(id);
    fill(&mut b, "31.00", &[("6250", "31.00")]);
    assert!(b.start_save(SaveIntent::Save, None));
    // `a` still holds the old version: the stale check refuses its save.
    fill(&mut a, "32.00", &[("6250", "32.00")]);
    assert!(!a.start_save(SaveIntent::Save, None));
    assert_eq!(a.state()["stale"], true);
    let (_, lines) = lines_of(&link, id);
    assert!(lines.contains(&("6250".into(), money("31.00"), Money::ZERO)));
    // Reload reads the latest; the change can then be made.
    a.reload();
    assert_eq!(a.header.current().amount, Some(money("31.00")));
    assert_eq!(a.state()["stale"], false);
}

#[test]
fn editing_a_closed_period_record_asks_for_the_password() {
    let (_d, link) = link();
    let mut s = screen(&link);
    fill(&mut s, "12.00", &[("6250", "12.00")]);
    let closed = CalendarDate::from_ymd(2026, 9, 15).expect("date");
    s.header.edit_step(|h| h.date = Some(closed));
    assert!(!s.start_save(SaveIntent::Save, None));
    s.flow.closing_prompt.password = SecretText::from(DEMO_CLOSING_PASSWORD);
    s.submit_closing();
    let id = s.flow.last.expect("saved").id;
    // Open it and move it out of the closed period: the old date counts.
    let mut e = screen(&link);
    e.open_saved(id);
    assert_eq!(e.state()["record"], "edit");
    e.header.edit_step(|h| h.memo = "moved".into());
    e.header.edit_step(|h| h.date = Some(today()));
    assert!(!e.start_save(SaveIntent::Save, None));
    assert!(e.flow.closing_prompt.dialog_open(), "the password first");
    e.flow.closing_prompt.password = SecretText::from(DEMO_CLOSING_PASSWORD);
    e.submit_closing();
    assert!(!e.flow.closing_prompt.dialog_open());
    assert_eq!(e.state()["saved_count"], 1);
}

#[test]
fn prev_on_a_dirty_form_asks_save_discard_or_keep() {
    let (_d, link) = link();
    let mut s = screen(&link);
    let id = save_charge(&mut s, "9.00");
    fill(&mut s, "3.00", &[("6250", "3.00")]);
    s.command(CommandId::new("form.prev_record"));
    assert_eq!(s.state()["nav_prompt"], true);
    // Esc is Keep editing.
    assert!(s.command(CommandId::new("form.revert")));
    assert_eq!(s.state()["nav_prompt"], false);
    assert!(s.is_dirty());
    // Discard: the edits go, the move happens.
    s.command(CommandId::new("form.prev_record"));
    s.answer_nav(PromptAnswer::Discard);
    assert_eq!(s.flow.editing.map(|e| e.id), Some(id));
    // Save: the edit is saved, then the move happens.
    let before = posted(&link);
    s.command(CommandId::new("form.next_record"));
    fill(&mut s, "4.00", &[("6250", "4.00")]);
    s.command(CommandId::new("form.prev_record"));
    s.answer_nav(PromptAnswer::Save);
    assert_eq!(posted(&link), before + 1, "the new charge was saved");
    assert_eq!(s.state()["record"], "edit", "then Prev opened one");
}

#[test]
fn esc_reverts_the_focused_field_then_lets_the_window_close() {
    let (_d, link) = link();
    let mut s = screen(&link);
    s.focused = Some(field::MEMO);
    s.header.edit(field::MEMO, |h| h.memo = "typed".into());
    assert!(s.command(CommandId::new("form.revert")), "reverts the memo");
    assert_eq!(s.header.current().memo, "");
    assert!(
        !s.command(CommandId::new("form.revert")),
        "nothing left: the shell closes the window"
    );
    // Text typed but not yet taken is dropped first.
    s.focused = Some(field::AMOUNT);
    s.inputs.amount.text = "12".into();
    assert!(s.command(CommandId::new("form.revert")));
    assert_eq!(s.inputs.amount.text, "");
    assert!(!s.is_dirty());
}

#[test]
fn header_tags_default_onto_lines_without_their_own() {
    let (_d, link) = link();
    let mut s = screen(&link);
    let tag = s.books.tags.iter().find(|t| t.is_active).expect("tag").id;
    fill(&mut s, "8.00", &[("6250", "8.00")]);
    let tags = s.books.tags.clone();
    s.header.edit_step(|h| {
        kit::apply_tag_edit(&tags, &mut h.header_tags, kit::TagEdit::Add(tag));
    });
    assert_eq!(s.state()["header_tags"], 1);
    let c = check(&s.compose(), &s.books.tag_list).expect("checked");
    assert_eq!(c.lines[0].tag, Some(tag), "Q-178");
    s.header.edit_step(|h| {
        kit::apply_tag_edit(&tags, &mut h.header_tags, kit::TagEdit::Remove(tag));
    });
    assert_eq!(s.state()["header_tags"], 0);
}

#[test]
fn the_payee_prefills_from_their_last_charge_never_the_amounts() {
    let (_d, link) = link();
    let mut s = screen(&link);
    let payee = s
        .books
        .contacts
        .iter()
        .find(|c| c.is_active)
        .expect("contact")
        .id;
    s.apply_payee(NameOutcome::Picked(payee));
    fill(&mut s, "64.10", &[("6250", "40.00"), ("6400", "24.10")]);
    s.header.edit_step(|h| h.memo = "supplies".into());
    assert!(s.start_save(SaveIntent::SaveAndNew, None), "{}", s.state());
    // A new charge to the same payee: accounts and memo, no amounts.
    s.apply_payee(NameOutcome::Picked(payee));
    assert_eq!(s.state()["prefill"]["source"], "last");
    let used: Vec<_> = s.lines.value().used().cloned().collect();
    assert_eq!(used.len(), 2);
    assert!(used.iter().all(|l| l.amount.is_none()), "never an amount");
    assert_eq!(used[0].account, s.books.by_number("6250"));
    assert_eq!(s.header.current().memo, "supplies");
    assert_eq!(s.header.current().amount, None);
    // The panel lists the recent charge to them.
    s.poll();
    assert!(s.state()["recent_charges"].as_u64().unwrap_or(0) >= 1);
}

#[test]
fn without_history_the_vendor_default_account_fills_the_first_line() {
    let (_d, link) = link();
    let mut s = screen(&link);
    let payee = s.books.contacts[0].id;
    let account = s.books.by_number("6400").expect("6400");
    if let Some(c) = s.books.contacts.iter_mut().find(|c| c.id == payee) {
        c.fields.default_expense_account_id = Some(account);
    }
    s.header.edit_step(|h| h.amount = Some(money("7.00")));
    s.apply_default(payee, None);
    let first = s.lines.value().used().next().expect("line").clone();
    assert_eq!(first.account, Some(account), "Q-192");
    assert_eq!(
        first.amount,
        Some(money("7.00")),
        "the typed amount follows"
    );
    assert_eq!(s.state()["prefill"]["source"], "default");
}

#[test]
fn void_needs_a_saved_record_first() {
    let (_d, link) = link();
    let mut s = screen(&link);
    assert!(s.command(CommandId::new("edit.void_transaction")));
    assert_eq!(s.state()["notice"], "helper.void.saved_only");
}

#[test]
fn state_never_carries_amounts_or_names() {
    let (_d, link) = link();
    let mut s = screen(&link);
    fill(&mut s, "1286.43", &[("6250", "1286.43")]);
    s.header.edit_step(|h| h.memo = "secret memo".into());
    let text = s.state().to_string();
    assert!(!text.contains("1286") && !text.contains("secret"), "{text}");
}

/// One frame of `screen` at `size`; the AccessKit nodes it gave.
fn frame(
    screen: &mut dyn Screen,
    ctx: &egui::Context,
    size: (f32, f32),
) -> Vec<(egui::accesskit::NodeId, egui::accesskit::Node)> {
    use eframe::egui::{Pos2, RawInput, Rect, vec2};
    let input = RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(size.0, size.1))),
        ..RawInput::default()
    };
    let mut out = ctx.run_ui(input, |ui| {
        egui::CentralPanel::default().show(ui, |ui| screen.ui(ui));
    });
    out.textures_delta.clear();
    out.platform_output
        .accesskit_update
        .map(|u| u.nodes)
        .unwrap_or_default()
}

/// Every lane C screen draws in light and dark at the two scenario sizes,
/// and its inputs carry their visible labels as accessible names.
#[test]
fn every_helper_draws_with_labelled_inputs() {
    let (_d, link) = link();
    let kinds: [(&str, &[&str]); 4] = [
        (KIND, &["Card", "Purchased from", "Amount", "Date"]),
        (
            crate::screens::loan_payment::KIND,
            &["Loan account", "Lender", "Payment", "Principal"],
        ),
        (
            crate::screens::owner_money::KIND,
            &["Who", "Bank account", "Amount", "Recorded in"],
        ),
        (
            crate::screens::personal_funds::KIND,
            &["Who paid", "Paid to (optional)", "Amount", "Recorded in"],
        ),
    ];
    for theme in [
        crate::theme::ThemeChoice::Light,
        crate::theme::ThemeChoice::Dark,
    ] {
        for (kind, labels) in kinds {
            let cmd = cl_model::commands::find(kind).expect("command").id;
            let mut s = crate::screens::P14_SCREENS
                .iter()
                .find_map(|make| make(cmd, &link, None, today()))
                .expect("screen");
            for size in [(1440.0, 940.0), (960.0, 640.0)] {
                let ctx = egui::Context::default();
                crate::theme::apply(&ctx, theme);
                crate::fonts::install(&ctx);
                ctx.enable_accesskit();
                let mut nodes = Vec::new();
                for _ in 0..3 {
                    nodes.extend(frame(s.as_mut(), &ctx, size));
                }
                for label in labels {
                    let id = nodes
                        .iter()
                        .find(|(_, n)| {
                            n.role() == egui::accesskit::Role::Label && n.value() == Some(*label)
                        })
                        .map_or_else(|| panic!("{kind}: label {label}"), |(id, _)| *id);
                    assert!(
                        nodes.iter().any(|(_, n)| n.labelled_by().contains(&id)),
                        "{kind}: {label} labels an input"
                    );
                }
            }
        }
    }
}

#[test]
fn owner_money_words_get_a_plain_tooltip_for_each_entity_type() {
    use cl_model::forms::owner_money::Entity;
    assert_eq!(kit::jargon_of(None), "jargon.owner_money");
    assert_eq!(
        kit::jargon_of(Some(Entity::SingleMemberLlc)),
        "jargon.owner_money"
    );
    assert_eq!(
        kit::jargon_of(Some(Entity::Partnership)),
        "jargon.partner_money"
    );
    assert_eq!(
        kit::jargon_of(Some(Entity::SCorp)),
        "jargon.shareholder_money"
    );
    // Every glossary entry is a plain sentence in the catalog.
    for key in [
        "jargon.undeposited_funds",
        "jargon.cash_over_short",
        "jargon.equity",
        "jargon.principal",
        "jargon.interest",
        "jargon.owner_money",
        "jargon.partner_money",
        "jargon.shareholder_money",
    ] {
        let text = crate::i18n::t(key);
        assert_ne!(text, key, "{key} is in the catalog");
        assert!(text.ends_with('.'), "{key}");
    }
    // Owners see Undeposited Funds everywhere (Q-202), so the glossary
    // never mentions the engine's name for it.
    assert!(!crate::i18n::t("jargon.undeposited_funds").contains("Payments Clearing"));
}

fn lose_the_reply(s: &mut CardChargeScreen) {
    let lists = Arc::clone(&s.books.lists);
    let _ = s.lines.form_mut().begin_save(SaveIntent::Save, &*lists);
    let _ = s.header.begin_save(SaveIntent::Save);
    s.flow.intent = Some(SaveIntent::Save);
    s.save_failed(ApiError::OutcomeUnknown {
        request_id: cl_core::ids::RequestId::parse("00000000-0000-4000-8000-000000000005")
            .expect("id"),
        idempotency_key: Some(cl_api::IdempotencyKey::new()),
        keyed: false,
    });
}

#[test]
fn b2_helper_forms_send_nothing_after_an_unknown_outcome_until_the_recheck_answers() {
    let (_d, link) = link();
    let mut s = screen(&link);
    fill(&mut s, "64.10", &[("6250", "40.00"), ("6400", "24.10")]);
    lose_the_reply(&mut s);
    assert!(s.flow.lost.pending());
    assert!(s.flow.error.is_none(), "the banner speaks");
    let before = posted(&link);
    for intent in [
        SaveIntent::Save,
        SaveIntent::SaveAndNew,
        SaveIntent::SaveAndClose,
    ] {
        assert!(!s.start_save(intent, None));
    }
    assert_eq!(posted(&link), before, "nothing sent");
    s.flow
        .lost
        .recheck_answering(&mut s.header, cl_api::Recheck::Unknown);
    s.poll();
    assert!(!s.start_save(SaveIntent::Save, None));
    assert_eq!(posted(&link), before);
    s.flow
        .lost
        .recheck_answering(&mut s.header, cl_api::Recheck::NotCommitted);
    s.poll();
    assert!(s.start_save(SaveIntent::Save, None));
    assert_eq!(posted(&link), before + 1);
}

#[test]
fn b2_a_found_charge_opens_and_nothing_is_sent() {
    let (_d, link) = link();
    let mut s = screen(&link);
    fill(&mut s, "64.10", &[("6250", "40.00"), ("6400", "24.10")]);
    assert!(s.start_save(SaveIntent::SaveAndNew, None));
    let id = s.flow.last.expect("saved").id;
    fill(&mut s, "64.10", &[("6250", "40.00"), ("6400", "24.10")]);
    lose_the_reply(&mut s);
    let before = posted(&link);
    s.flow.lost.recheck_answering(
        &mut s.header,
        cl_api::Recheck::Committed {
            transaction: Some(id),
        },
    );
    s.poll();
    assert!(!s.flow.lost.pending());
    assert_eq!(s.flow.editing.map(|e| e.id), Some(id), "the saved charge");
    assert!(!s.is_dirty());
    assert_eq!(posted(&link), before);
}

/// Attach a file by path and wait for the background read and upload.
fn attach_and_wait(s: &mut CardChargeScreen, path: &std::path::Path) {
    s.attach_path(path);
    let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while s.attach.busy() {
        assert!(std::time::Instant::now() < until, "the attach never ended");
        std::thread::sleep(std::time::Duration::from_millis(5));
        s.poll_attach();
    }
}

/// A02 (Q-A02e): a file attached before the first save goes to a draft,
/// shows in the pane at once, and moves onto the charge (an `expense`)
/// once it is saved; the saved charge opens with it. A credit's files are
/// the deposit's. No file name in the state.
#[test]
fn files_ride_onto_a_saved_charge_and_its_credit_counterpart() {
    let (dir, link) = link();
    let mut s = screen(&link);
    assert!(s.document().is_none());
    let pdf = dir.path().join("secret-vendor.pdf");
    std::fs::write(&pdf, b"%PDF-1.4\n%synthetic\n").expect("write");
    attach_and_wait(&mut s, &pdf);
    assert_eq!(s.attach.problem, None);
    assert_eq!(s.state()["attachments"]["count"], 1);
    assert_eq!(s.state()["attachments"]["draft"], true);
    let doc = s.document().expect("the pane shows it");
    assert!(!format!("{doc:?}").contains("secret"));
    assert!(!s.state().to_string().contains("secret"));
    // A refused file is never sent.
    let text = dir.path().join("notes.txt");
    std::fs::write(&text, b"hello").expect("write");
    attach_and_wait(&mut s, &text);
    assert_eq!(s.attach.problem, Some("check.attach.wrong_type"));
    assert_eq!(s.state()["attachments"]["count"], 1);
    // Save & New: the file moves onto the charge; the new form has none.
    let id = save_charge(&mut s, "12.00");
    s.poll();
    assert_eq!(s.state()["attachments"]["count"], 0);
    assert_eq!(s.state()["attachments"]["move_failed"], false);
    assert!(s.document().is_none());
    {
        let demo = link.demo_state().expect("demo");
        let on = crate::company::demo::checks::attachments(&demo, cl_api::AttachTo::Expense(id));
        assert_eq!(on.len(), 1, "the file is the charge's");
    }
    // Opened again: its file shows.
    assert!(s.open_record(id));
    s.poll();
    s.poll_attach();
    assert_eq!(s.state()["attachments"]["count"], 1);
    assert_eq!(s.state()["attachments"]["saved_record"], true);
    assert!(s.document().is_some());
    // More files attach to the saved charge directly.
    attach_and_wait(&mut s, &pdf);
    assert_eq!(s.state()["attachments"]["count"], 2);
    // A credit keeps its files as the deposit's.
    let mut c = screen(&link);
    c.set_mode(CardMode::Credit);
    attach_and_wait(&mut c, &pdf);
    fill(&mut c, "5.00", &[("6250", "5.00")]);
    assert!(c.start_save(SaveIntent::Save, None), "{}", c.state());
    c.poll();
    let credit = c.flow.last.expect("saved").id;
    let demo = link.demo_state().expect("demo");
    let on = crate::company::demo::checks::attachments(
        &demo,
        cl_api::AttachTo::Txn(TxnType::Deposit, credit),
    );
    assert_eq!(on.len(), 1, "the file is the credit's");
}

/// Picks account `n` on line `i` (no amount), as the grid does, and runs
/// the frame's remaining fill (Q-304).
fn pick_line(s: &mut CardChargeScreen, line: usize, number: &str) {
    let picked = s.books.by_number(number).expect("account");
    let mut lines = s.lines.value().clone();
    lines.lines[line].account = Some(picked);
    lines.ensure_entry(&mut s.lines.form_mut().adapter_mut().keys);
    s.lines.form_mut().edit_step(|now| *now = lines);
    s.fill_remaining();
}

#[test]
fn a_new_card_line_takes_what_is_left_of_the_amount() {
    let (_d, link) = link();
    let mut s = screen(&link);
    s.amount_typed(Some(money("1500.01")));
    pick_line(&mut s, 0, "6400");
    assert_eq!(s.lines.value().lines[0].amount, Some(money("1500.01")));
    s.lines
        .form_mut()
        .edit_step(|v| v.lines[0].amount = Some(money("1000.00")));
    s.fill_remaining();
    pick_line(&mut s, 1, "6400");
    assert_eq!(s.lines.value().lines[0].amount, Some(money("1000.00")));
    assert_eq!(s.lines.value().lines[1].amount, Some(money("500.01")));
    pick_line(&mut s, 2, "6400");
    assert_eq!(s.lines.value().lines[2].amount, None, "nothing left");
}

#[test]
fn loaded_lines_and_saved_charges_are_not_filled() {
    let (_d, link) = link();
    let mut s = screen(&link);
    save_charge(&mut s, "11.25");
    save_charge(&mut s, "22.50");
    // Prev, Prev, Next: each opened charge stays as saved and clean.
    for cmd in ["form.prev_record", "form.prev_record", "form.next_record"] {
        s.command(CommandId::new(cmd));
        let before = s.lines.value().clone();
        s.fill_remaining();
        assert_eq!(*s.lines.value(), before);
        assert!(!s.is_dirty());
    }
    // Lines loaded with an account and no amount are not "just picked".
    let mut s = screen(&link);
    s.amount_typed(Some(money("80.00")));
    let a = s.books.by_number("6400");
    let key = s.lines.form_mut().adapter_mut().keys.next_key();
    let lines = FormLines::from_lines(
        vec![FormLine {
            account: a,
            ..FormLine::blank(key)
        }],
        &mut s.lines.form_mut().adapter_mut().keys,
    );
    let lists = Arc::clone(&s.books.lists);
    s.lines.load(lines, &*lists);
    s.fill_remaining();
    assert_eq!(s.lines.value().lines[0].amount, None);
    assert!(!s.lines.dirty());
}

#[test]
fn a_read_total_or_read_line_is_never_filled() {
    use cl_model::forms::doc_read::{DocReading, ReadField, ReadLine};
    let (_d, link) = link();
    let mut s = screen(&link);
    let reading = DocReading {
        total: Some(money("80.00")),
        lines: vec![ReadLine {
            description: "Fuel".into(),
            amount: None,
            account: s.books.by_number("6400"),
            confidence: None,
        }],
        ..DocReading::default()
    };
    let mut form = s.compose();
    {
        let keys = &mut s.lines.form_mut().adapter_mut().keys;
        cl_model::forms::card::apply(&reading, &mut form, keys);
    }
    let lines = std::mem::take(&mut form.lines);
    s.header.load(form);
    // Even arriving as an edit, the read line is held and the read total
    // is not the owner's (non-negotiable 4).
    s.lines.form_mut().edit_step(|v| *v = lines);
    s.fill_remaining();
    let key = s.lines.value().lines[0].key;
    assert_eq!(s.lines.value().lines[0].amount, None);
    pick_line(&mut s, 1, "6250");
    assert_eq!(s.lines.value().lines[1].amount, None, "total not accepted");
    // The total accepted: an owner line fills, the read line stays held.
    s.header.edit_step(|h| h.read.edited(ReadTarget::Total));
    pick_line(&mut s, 2, "6250");
    assert_eq!(s.lines.value().lines[2].amount, Some(money("80.00")));
    assert_eq!(s.lines.value().lines[0].amount, None);
    assert!(matches!(
        s.header.current().read.state(ReadTarget::Line(key)),
        Some(ReadField::Read { .. })
    ));
    // Clear and type it myself: the read line goes with no amount left.
    s.clear_read();
    assert!(s.lines.value().lines.iter().all(|l| l.key != key));
}

#[test]
fn a_read_only_charge_is_never_filled() {
    let (_d, link) = link();
    let mut s = screen(&link);
    let id = save_charge(&mut s, "40.00");
    let mut other = screen(&link);
    assert!(other.open_saved(id));
    other.flow.void_reason = "duplicate".into();
    other.void_now(None);
    let mut s = screen(&link);
    s.open_saved(id);
    assert_eq!(s.state()["read_only"], "helper.read_only.void");
    s.header.edit_step(|h| h.amount = Some(money("90.00")));
    let entry = s.lines.value().lines.len() - 1;
    pick_line(&mut s, entry, "6400");
    assert_eq!(s.lines.value().lines[entry].amount, None);
}

#[test]
fn a_new_card_credit_line_takes_what_is_left_of_the_amount() {
    let (_d, link) = link();
    let mut s = screen(&link);
    s.set_mode(CardMode::Credit);
    s.amount_typed(Some(money("19.99")));
    pick_line(&mut s, 0, "6250");
    assert_eq!(s.lines.value().lines[0].amount, Some(money("19.99")));
    s.lines
        .form_mut()
        .edit_step(|v| v.lines[0].amount = Some(money("15.00")));
    s.fill_remaining();
    pick_line(&mut s, 1, "6400");
    assert_eq!(s.lines.value().lines[1].amount, Some(money("4.99")));
}
