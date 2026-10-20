//! Record Card Charge tests (no engine): the demo stand-in feeds the
//! screen; the demo ledger's size proves when nothing was sent.

use super::*;
use cl_model::forms::lines::FormLine;

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

fn screen(link: &Arc<CompanyLink>) -> CardChargeScreen {
    let mut s = CardChargeScreen::new(Arc::clone(link), None, today());
    s.poll();
    s
}

fn fill(s: &mut CardChargeScreen, amount: &str, lines: &[(&str, &str)]) {
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
    s.set_mode(CardMode::Credit);
    assert_eq!(s.title(), "Record Card Credit");
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
