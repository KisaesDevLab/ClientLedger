//! Write Check screen tests (no engine): the demo stand-in feeds the
//! screen, and its write counter proves when nothing was sent.

use super::*;
use cl_api::SplitLine;
use cl_core::ids::ContactId;
use cl_model::forms::SaveIntent;
use cl_model::forms::check::{CheckedCheck, check};
use cl_model::forms::lines::{FormLine, PostingLine};

use crate::screens::company::form::SecretText;

fn link() -> (tempfile::TempDir, Arc<CompanyLink>) {
    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    (dir, link)
}

fn today() -> CalendarDate {
    CalendarDate::from_ymd(2026, 11, 4).expect("date")
}

fn money(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}

fn screen(link: &Arc<CompanyLink>) -> WriteCheckScreen {
    let mut s = WriteCheckScreen::new(Arc::clone(link), None, today());
    s.poll();
    s
}

fn writes(link: &Arc<CompanyLink>) -> u32 {
    link.demo_state().expect("demo").checks.writes
}

fn account(s: &WriteCheckScreen, n: &str) -> AccountId {
    s.accounts
        .iter()
        .find(|a| a.fields.account_number.as_deref() == Some(n))
        .map_or_else(|| panic!("{n}"), |a| a.id)
}

fn contact(s: &WriteCheckScreen, name: &str) -> ContactId {
    s.contacts
        .iter()
        .find(|c| c.fields.display_name == name)
        .map_or_else(|| panic!("{name}"), |c| c.id)
}

/// Ozark Feed & Seed, `amount`, and one line per `(account, amount)`.
fn fill(s: &mut WriteCheckScreen, amount: &str, lines: &[(&str, &str)]) {
    let payee = contact(s, "Ozark Feed & Seed");
    s.pick_payee(payee);
    let amount = money(amount);
    s.header.edit_step(|h| h.amount = Some(amount));
    let seeds: Vec<FormLine> = lines
        .iter()
        .map(|(a, m)| {
            let account = account(s, a);
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
fn opens_on_the_bank_account_with_the_next_number_and_the_engine_balance() {
    let (_d, link) = link();
    let s = screen(&link);
    assert_eq!(s.bank(), Some(account(&s, "1010")));
    assert_eq!(
        s.header.current().number,
        NumberField::Proposed(4418),
        "the highest check in 1010 is 4417"
    );
    assert!(!s.is_dirty(), "the proposal is part of the blank form");
    assert_eq!(s.title(), "Write Check \u{b7} #4418");
    // The demo engine's account balance, as sent (the mockup figure).
    assert_eq!(s.balance, Some(money("84210.55")));
    let st = s.state();
    assert_eq!(st["screen"], "write_check");
    assert_eq!(st["number"], 4418);
    assert_eq!(st["number_proposed"], true);
    assert_eq!(st["ending_balance_shown"], true);
    assert_eq!(st["line_billable"], true);
    assert_eq!(s.company.name, "Ozark Ridge Landscaping LLC");
}

#[test]
fn the_demo_tab_is_the_mockup_draft_unsaved() {
    let (_d, link) = link();
    let s = WriteCheckScreen::demo_draft(Arc::clone(&link), None, today());
    assert!(s.is_dirty());
    assert_eq!(s.title(), "Write Check \u{b7} #4418");
    let f = s.compose();
    assert!(!f.print_later);
    assert_eq!(f.amount, Some(money("1286.40")));
    assert_eq!(f.lines.used().count(), 2);
    assert_eq!(
        f.totals_status(),
        Some(cl_model::forms::check::TotalsStatus::Balanced)
    );
    assert_eq!(s.state()["words"], true);
}

#[test]
fn lines_that_disagree_by_a_cent_save_nothing() {
    let (_d, link) = link();
    let mut s = screen(&link);
    fill(&mut s, "1286.40", &[("5100", "842.15"), ("5110", "444.24")]);
    let before = writes(&link);
    assert!(!s.start_save(SaveIntent::SaveAndClose, None));
    assert_eq!(writes(&link), before, "nothing reached the engine");
    assert_eq!(s.state()["save"]["refused"], "lines_disagree");
    assert_eq!(s.state()["saved_count"], 0);
    assert!(s.is_dirty(), "the owner's entries stay");
    assert_eq!(s.state()["totals"], "remaining");
    // Over by a cent is refused too.
    s.lines
        .form_mut()
        .edit_step(|v| v.lines[1].amount = Some(money("444.26")));
    assert!(!s.start_save(SaveIntent::SaveAndNew, None));
    assert_eq!(writes(&link), before);
    assert_eq!(s.state()["totals"], "over");
}

#[test]
fn save_and_new_posts_the_check_and_proposes_the_next_number() {
    let (_d, link) = link();
    let mut s = screen(&link);
    let date = today();
    fill(&mut s, "1286.40", &[("5100", "842.15"), ("5110", "444.25")]);
    assert_eq!(s.state()["totals"], "balanced");
    assert!(s.start_save(SaveIntent::SaveAndNew, None));
    s.poll();
    assert_eq!(s.state()["saved_count"], 1);
    assert_eq!(s.state()["last_saved"]["number"], 4418);
    assert_eq!(s.state()["last_saved"]["print_status"], "hand_written");
    // The engine's balance after the save (84,210.55 − 1,286.40).
    assert_eq!(s.balance, Some(money("82924.15")));
    // A blank check on the same account and date with the next number.
    assert!(!s.is_dirty());
    assert_eq!(s.header.current().number, NumberField::Proposed(4419));
    assert_eq!(s.header.current().date, Some(date));
    assert_eq!(s.bank(), Some(account(&s, "1010")));
    assert!(s.compose().lines.used().next().is_none());
    // The GL effect in the demo ledger: the check's lines as typed.
    let mut demo = link.demo_state().expect("demo");
    let bank = s.bank().expect("bank");
    let saved = crate::company::demo::checks::list(&mut demo, bank)
        .into_iter()
        .find(|c| c.number == Some(4418))
        .expect("4418");
    let record = crate::company::demo::checks::get(&mut demo, saved.id)
        .expect("get")
        .record;
    let debits: Vec<Money> = record
        .txn
        .lines
        .iter()
        .filter(|l| l.debit.is_positive())
        .map(|l| l.debit)
        .collect();
    assert_eq!(debits, [money("842.15"), money("444.25")]);
    let credit: Vec<Money> = record
        .txn
        .lines
        .iter()
        .filter(|l| l.credit.is_positive())
        .map(|l| l.credit)
        .collect();
    assert_eq!(credit, [money("1286.40")]);
}

#[test]
fn a_used_number_offers_the_next_one_and_never_changes_it_silently() {
    let (_d, link) = link();
    let mut s = screen(&link);
    fill(&mut s, "10.00", &[("5110", "10.00")]);
    assert!(s.start_save(SaveIntent::SaveAndNew, None));
    s.poll();
    fill(&mut s, "20.00", &[("5110", "20.00")]);
    s.header
        .edit_step(|h| h.number = NumberField::Typed("4418".into()));
    assert!(!s.start_save(SaveIntent::SaveAndNew, None));
    s.poll();
    assert_eq!(s.save.duplicate.map(|(n, _)| n), Some(4418));
    assert_eq!(s.state()["save"]["duplicate"], 4419);
    assert_eq!(
        s.header.current().number,
        NumberField::Typed("4418".into()),
        "the typed number stays until the owner takes the offer"
    );
    s.use_offered_number();
    assert_eq!(s.header.current().number, NumberField::Typed("4419".into()));
    assert!(s.start_save(SaveIntent::SaveAndNew, None));
    assert_eq!(s.state()["last_saved"]["number"], 4419);
}

#[test]
fn a_closed_period_check_asks_for_the_password() {
    let (_d, link) = link();
    let mut s = screen(&link);
    fill(&mut s, "18.25", &[("5110", "18.25")]);
    let locked = CalendarDate::from_ymd(2026, 9, 15).expect("d");
    s.header.edit_step(|h| h.date = Some(locked));
    let before = writes(&link);
    assert!(!s.start_save(SaveIntent::SaveAndNew, None));
    assert_eq!(s.state()["closing"], "asking");
    assert_eq!(writes(&link), before, "asked before sending");
    // Cancel saves nothing.
    s.closing_prompt.cancel();
    assert_eq!(s.state()["saved_count"], 0);
    // A wrong password: the dialog stays, nothing saved.
    assert!(!s.start_save(SaveIntent::SaveAndNew, None));
    s.closing_prompt.password = SecretText::from("wrong");
    s.submit_closing();
    assert_eq!(s.state()["closing"], "rejected");
    assert_eq!(s.state()["saved_count"], 0);
    s.closing_prompt.password =
        SecretText::from(crate::company::demo::register::DEMO_CLOSING_PASSWORD);
    s.submit_closing();
    assert_eq!(s.state()["closing"], "done");
    assert_eq!(s.state()["saved_count"], 1);
}

#[test]
fn print_later_shows_to_print_and_saves_unnumbered() {
    let (_d, link) = link();
    let mut s = screen(&link);
    fill(&mut s, "100.00", &[("5110", "100.00")]);
    s.set_print_later(true);
    assert_eq!(s.state()["number_text"], "To print");
    assert_eq!(s.title(), "Write Check \u{b7} To print");
    assert!(s.start_save(SaveIntent::SaveAndNew, None));
    s.poll();
    assert_eq!(s.state()["last_saved"]["print_status"], "queue");
    assert_eq!(s.state()["last_saved"]["number"], Value::Null);
    assert_eq!(
        s.header.current().number,
        NumberField::Proposed(4418),
        "a queued check takes no number"
    );
    // Unticking brings the number back.
    s.set_print_later(true);
    s.set_print_later(false);
    assert_eq!(s.header.current().number, NumberField::Proposed(4418));
}

#[test]
fn the_check_amount_follows_a_single_line_and_takes_the_lines_when_blank() {
    let (_d, link) = link();
    let mut s = screen(&link);
    fill(&mut s, "50.00", &[("5110", "50.00")]);
    s.amount_typed(Some(money("75.00")));
    assert_eq!(s.lines.value().lines[0].amount, Some(money("75.00")));
    // A blank check amount takes the lines total when the owner leaves
    // the grid (Q-P13r b).
    s.amount_typed(None);
    s.lines
        .form_mut()
        .edit_step(|v| v.lines[0].amount = Some(money("12.34")));
    s.left_lines();
    assert_eq!(s.header.current().amount, Some(money("12.34")));
    assert!(s.header.current().amount_from_lines);
}

#[test]
fn prev_opens_the_newest_check_and_void_frees_its_number() {
    let (_d, link) = link();
    let mut s = screen(&link);
    fill(&mut s, "10.00", &[("5110", "10.00")]);
    assert!(s.start_save(SaveIntent::SaveAndNew, None));
    s.poll();
    s.navigate(true);
    s.poll();
    assert_eq!(s.state()["mode"], "edit");
    assert_eq!(s.state()["number"], 4418);
    assert!(!s.is_dirty());
    assert_eq!(s.compose().amount, Some(money("10.00")));
    // Edit and save it again.
    s.header.edit_step(|h| h.memo = "changed".into());
    assert!(s.start_save(SaveIntent::Save, None));
    assert_eq!(s.state()["mode"], "edit");
    // Void (reason required), then the number is free again.
    assert!(s.start_void());
    assert!(s.void.open);
    s.send_void("  ", None);
    assert_eq!(s.save.notice, Some("check.void.reason_needed"));
    s.send_void("Wrong vendor", None);
    s.poll();
    assert_eq!(s.state()["last_action"], "void");
    assert_eq!(s.state()["mode"], "new");
    assert_eq!(s.header.current().number, NumberField::Proposed(4418));
    // Next from a new check stays; Prev again shows the void check read-only.
    s.navigate(true);
    s.poll();
    assert_eq!(s.state()["read_only"], true);
    assert!(!s.start_save(SaveIntent::Save, None));
    s.toggle_history();
    assert!(s.history.rows.len() >= 2);
}

#[test]
fn moving_away_from_unsaved_work_is_refused() {
    let (_d, link) = link();
    let mut s = screen(&link);
    fill(&mut s, "10.00", &[("5110", "10.00")]);
    s.navigate(true);
    assert_eq!(s.save.notice, Some("check.nav.unsaved"));
    assert_eq!(s.state()["mode"], "new");
    s.clear();
    assert!(!s.is_dirty());
    assert_eq!(s.state()["last_action"], "clear");
}

#[test]
fn the_draft_is_the_checked_check_field_by_field() {
    let account = AccountId::parse("00000000-0000-0000-0000-000000005100").expect("id");
    let bank = AccountId::parse("00000000-0000-0000-0000-000000001010").expect("id");
    let payee = ContactId::parse("00000000-0000-0000-0000-000000000007").expect("id");
    let job = ContactId::parse("00000000-0000-0000-0000-000000000009").expect("id");
    let tag = TagId::parse("00000000-0000-0000-0000-000000000003").expect("id");
    let c = CheckedCheck {
        bank,
        number: Some(4418),
        print_later: false,
        date: today(),
        payee,
        payee_name: String::new(),
        address: Some("1200 Commerce Dr".into()),
        amount: money("1286.40"),
        memo: Some("Acct 7712".into()),
        header_tags: vec![tag],
        lines: vec![PostingLine {
            account,
            amount: money("1286.40"),
            memo: Some("Perennials".into()),
            contact: Some(job),
            billable: true,
            tag: Some(tag),
        }],
        draft_attachment: None,
    };
    let d = to_draft(&c, "Ozark Feed & Seed").expect("draft");
    assert_eq!(d.bank, bank);
    assert_eq!(d.date, today());
    assert_eq!(d.payee, Some(payee));
    assert_eq!(
        d.payee_name, "Ozark Feed & Seed",
        "a blank name takes the display name"
    );
    assert_eq!(d.check_number, Some(4418));
    assert!(!d.print_later);
    assert_eq!(d.payee_address.as_deref(), Some("1200 Commerce Dr"));
    assert_eq!(d.memo.as_deref(), Some("Acct 7712"));
    assert_eq!(d.printed_memo, d.memo, "Q-P13n");
    assert_eq!(d.tag_ids, [tag]);
    assert_eq!(d.amount, money("1286.40"));
    assert_eq!(
        d.lines,
        [SplitLine {
            account,
            amount: money("1286.40"),
            memo: Some("Perennials".into()),
            contact: Some(job),
            billable: true,
            tag: Some(tag),
        }]
    );
    let named = CheckedCheck {
        payee_name: "Ozark Feed and Seed Co.".into(),
        draft_attachment: Some(cl_model::forms::check::AttachmentRef {
            id: "not-a-uuid".into(),
            name: "r.pdf".into(),
        }),
        ..c.clone()
    };
    assert!(
        to_draft(&named, "x").is_err(),
        "an unknown draft id is refused"
    );
    let named = CheckedCheck {
        draft_attachment: None,
        ..named
    };
    assert_eq!(
        to_draft(&named, "x").expect("draft").payee_name,
        "Ozark Feed and Seed Co."
    );
    // The posting builder takes it, and refuses billable lines without
    // the capability (nothing is sent).
    assert!(BankPosting::check_with(d.clone(), true).is_ok());
    assert_eq!(
        BankPosting::check_with(d, false),
        Err(cl_api::LocalValidation::LineBillableUnavailable)
    );
}

#[test]
fn every_problem_has_a_name_and_a_message() {
    let (_d, link) = link();
    let s = screen(&link);
    let ctx = cl_model::forms::check::CheckContext { tags: &s.tag_list };
    let Err(problems) = check(&s.compose(), &ctx) else {
        panic!("a blank check is refused");
    };
    assert!(problems.contains(&CheckProblem::PayeeMissing));
    for p in problems {
        assert_ne!(problem_name(p), "");
        assert!(crate::i18n::has(p.key().0), "{}", p.key().0);
    }
}

use cl_api::BankPosting;

/// Draws the screen a few frames and returns every accessible (role,
/// name, disabled), a text input's name being the label it is labelled by.
fn tree(s: &mut WriteCheckScreen, width: f32) -> Vec<(egui::accesskit::Role, String, bool)> {
    use eframe::egui::{Pos2, RawInput, Rect, vec2};
    let ctx = egui::Context::default();
    crate::theme::apply(&ctx, crate::theme::ThemeChoice::Light);
    crate::fonts::install(&ctx);
    ctx.enable_accesskit();
    let mut all = std::collections::HashMap::new();
    for _ in 0..3 {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(width, 940.0))),
            ..RawInput::default()
        };
        let mut out = ctx.run_ui(input, |ui| {
            egui::CentralPanel::default().show(ui, |ui| s.ui(ui));
        });
        out.textures_delta.clear();
        if let Some(t) = out.platform_output.accesskit_update {
            all.extend(t.nodes);
        }
    }
    let nodes: Vec<_> = all.into_iter().collect();
    let labels: std::collections::HashMap<_, _> = nodes
        .iter()
        .filter_map(|(id, n)| n.label().or(n.value()).map(|l| (*id, l.to_string())))
        .collect();
    nodes
        .iter()
        .map(|(_, n)| {
            let own = n.label().or(if n.role() == egui::accesskit::Role::Label {
                n.value()
            } else {
                None
            });
            let name = own.map_or_else(
                || {
                    n.labelled_by()
                        .iter()
                        .filter_map(|id| labels.get(id).cloned())
                        .collect::<Vec<_>>()
                        .join(" ")
                },
                str::to_string,
            );
            (n.role(), name, n.is_disabled())
        })
        .collect()
}

#[test]
fn every_input_is_named_by_its_label_and_the_toolbar_matches_the_plan() {
    use egui::accesskit::Role;
    let (_d, link) = link();
    for width in [1440.0, 960.0] {
        let mut s = WriteCheckScreen::demo_draft(Arc::clone(&link), None, today());
        let nodes = tree(&mut s, width);
        let has = |role: Role, name: &str| nodes.iter().any(|(r, n, _)| *r == role && n == name);
        for name in [
            "No.",
            "Date",
            "Pay to the order of",
            "Amount ($)",
            "Address",
            "Memo",
        ] {
            assert!(
                has(Role::TextInput, name) || has(Role::MultilineTextInput, name),
                "{name}: {nodes:#?}"
            );
        }
        for name in [
            "Clear",
            "Save & New",
            "Save & Close",
            "Find",
            "History",
            "Prev",
            "Next",
        ] {
            assert!(has(Role::Button, name), "{name}");
        }
        assert!(has(Role::CheckBox, "Print later"));
        let print = nodes
            .iter()
            .find(|(r, n, _)| *r == Role::Button && n == "Print")
            .expect("Print");
        assert!(print.2, "Print is off until P19");
        for name in ["Memorize", "Pay online", "Items"] {
            assert!(!nodes.iter().any(|(_, n, _)| n.starts_with(name)), "{name}");
        }
        let named = |want: &str| nodes.iter().any(|(_, n, _)| n.starts_with(want));
        assert!(named("One thousand two hundred eighty-six and 40/100"));
        assert!(named("Lines total 1,286.40 = check amount"));
        assert!(named("84,210.55"), "the ending balance");
        assert!(named("Expenses 1,286.40"));
    }
}
