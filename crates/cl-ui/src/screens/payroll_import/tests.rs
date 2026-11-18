//! Import Payroll against the demo engine (P30): the whole wizard per
//! fixture, the tag gate on and off, duplicates, overlaps, the closing
//! password, no automatic resend, generic re-import without remapping, and
//! a restricted viewer. The scenarios drive the drawn window.

use std::sync::Arc;

use super::*;
use crate::company::CompanyLink;

fn today() -> CalendarDate {
    CalendarDate::from_ymd(2025, 11, 4).expect("date")
}

struct Fixture {
    _dir: tempfile::TempDir,
    link: Arc<CompanyLink>,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    {
        // The demo story's books are closed after every payroll fixture.
        let mut d = link.demo_state().expect("demo");
        let open = d.open.expect("open");
        if let Some(c) = d.companies.iter_mut().find(|c| c.tenant_id == open) {
            c.closed_through = None;
        }
    }
    Fixture { _dir: dir, link }
}

fn screen(f: &Fixture) -> PayrollImportScreen {
    let mut s = PayrollImportScreen::new(Arc::clone(&f.link), today());
    s.poll();
    s
}

fn cmd(s: &mut PayrollImportScreen, id: &str) {
    let id: &'static str = Box::leak(id.to_string().into_boxed_str());
    assert!(s.command(CommandId::new(id)), "{id}");
    s.poll();
}

/// Next until `step` (each answer is taken at once from the demo).
fn next_to(s: &mut PayrollImportScreen, step: Step) {
    for _ in 0..8 {
        if s.step == step {
            return;
        }
        if s.step == Step::Accounts {
            choose_unmatched(s);
        }
        assert!(s.can_next(), "stuck on {:?} ({:?})", s.step, s.trouble);
        s.next();
        s.poll();
    }
    assert_eq!(s.step, step, "{}", s.state());
}

/// The demo chart has no reimbursement account: nothing is pre-selected
/// for that line (QA-1), so the owner (here, the test) picks one.
fn choose_unmatched(s: &mut PayrollImportScreen) {
    let missing: Vec<&str> = s
        .needed_lines()
        .into_iter()
        .filter(|l| !s.line_accounts.contains_key(*l))
        .collect();
    assert!(
        missing.iter().all(|l| *l == "reimbursement_expense"),
        "only reimbursements lack a match on the demo chart: {missing:?}"
    );
    if !missing.is_empty() {
        assert!(!s.can_next(), "Next waits for every line");
        let telephone = s
            .chart
            .iter()
            .find(|a| a.fields.name == "Telephone and Internet")
            .map(|a| a.id)
            .expect("an expense account");
        s.line_accounts
            .insert("reimbursement_expense".into(), telephone);
    }
}
fn state_text(s: &PayrollImportScreen) -> String {
    s.state().to_string()
}

#[test]
fn every_fixture_posts_one_entry_per_pay_date() {
    for (stem, entries) in [
        ("gusto", 2),
        ("adp_run_gli", 1),
        ("quickbooks_payroll", 1),
        ("paychex_flex_gls", 1),
        ("square", 1),
        ("onpay_gl_summary", 1),
        ("generic_period1", 1),
    ] {
        let f = fixture();
        let mut s = screen(&f);
        cmd(&mut s, &format!("dev.payroll_demo_file_{stem}"));
        if s.provider.is_generic() {
            s.next();
            s.poll();
            assert_eq!(s.step, Step::Map, "{stem}: no saved layout yet");
            for (field, header) in [
                ("employee_name", "Worker"),
                ("check_date", "Paid On"),
                ("gross_pay", "Wages"),
                ("federal_income_tax", "Fed WH"),
                ("state_income_tax", "State WH"),
                ("social_security_ee", "SS EE"),
                ("medicare_ee", "Medicare EE"),
                ("social_security_er", "SS ER"),
                ("medicare_er", "Medicare ER"),
                ("net_pay", "Take Home"),
            ] {
                s.set_column(field, Some(header.into()));
            }
        }
        next_to(&mut s, Step::Preview);
        let p = s.preview.clone().expect("preview");
        assert_eq!(p.entries.len(), entries, "{stem}");
        assert!(p.is_balanced(), "{stem}");
        next_to(&mut s, Step::Done);
        assert_eq!(s.posted.as_ref().map(Vec::len), Some(entries), "{stem}");
        assert_eq!(s.posts_sent, 1, "{stem}: sent once");
        let st = state_text(&s);
        assert!(
            !st.contains("Test Worker") && !st.contains(".csv"),
            "{stem}: {st}"
        );
    }
}

#[test]
fn gusto_departments_split_expenses_and_tag_them() {
    let f = fixture();
    let tag = {
        let d = f.link.demo_state().expect("demo");
        d.tags.clone().unwrap_or_default().first().map(|t| t.id)
    };
    let mut s = screen(&f);
    cmd(&mut s, "dev.payroll_demo_file_gusto");
    next_to(&mut s, Step::Map);
    assert_eq!(s.layout_choice, Some(LayoutChoice::Builtin));
    assert_eq!(
        s.departments,
        vec!["Front".to_string(), "Kitchen".to_string()]
    );
    s.department_tags.insert("Kitchen".into(), tag);
    assert_eq!(
        s.sample("employee_name"),
        None,
        "the worker column is never shown"
    );
    assert!(s.sample("gross_pay").is_some());
    next_to(&mut s, Step::Preview);
    let p = s.preview.clone().expect("preview");
    for e in &p.entries {
        let clearing = e
            .lines
            .iter()
            .filter(|l| l.line_type == "payroll_clearing")
            .count();
        assert_eq!(clearing, 1, "net pay stays one line");
        assert!(e.lines.iter().filter(|l| l.department.is_some()).count() >= 3);
        if tag.is_some() {
            assert!(
                e.lines
                    .iter()
                    .any(|l| l.department.as_deref() == Some("Kitchen") && l.tag == tag)
            );
        }
    }
}

/// S3: departments past the sample rows come from the check and can be
/// tagged there; a saved mapping for a department the samples don't show is
/// kept and sent.
#[test]
fn every_department_in_the_file_is_listed_and_tagged() {
    let f = fixture();
    // The demo passes the tag id through (the engine checks it exists).
    let tag: TagId = "4b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a02".parse().expect("tag");
    let mut s = screen(&f);
    cmd(&mut s, "dev.payroll_demo_file_gusto");
    next_to(&mut s, Step::Map);
    // As if neither department were in the first rows, with a mapping the
    // saved layout remembers for Front.
    s.department_tags.insert("Front".into(), Some(tag));
    if let Some(u) = &mut s.upload {
        u.samples.0.clear();
    }
    s.refresh_departments();
    assert_eq!(s.departments, Vec::<String>::new());
    assert_eq!(
        s.department_tags.get("Front"),
        Some(&Some(tag)),
        "kept though the samples don't show it"
    );
    next_to(&mut s, Step::Check);
    assert_eq!(s.departments, ["Front", "Kitchen"]);
    assert_eq!(s.state()["departments_from_check"], true);
    s.department_tags.insert("Kitchen".into(), Some(tag));
    let sent = s.departments_sent();
    assert_eq!(sent.len(), 2);
    next_to(&mut s, Step::Preview);
    let p = s.preview.clone().expect("preview");
    for d in ["Front", "Kitchen"] {
        assert!(
            p.entries
                .iter()
                .flat_map(|e| &e.lines)
                .any(|l| l.department.as_deref() == Some(d) && l.tag == Some(tag)),
            "{d} tagged"
        );
    }
    // The tags chosen on Check are saved with the layout.
    let saved = s
        .templates_list()
        .iter()
        .find_map(|t| t.column_map.as_ref())
        .map(|m| m.department_tags.clone())
        .expect("saved layout");
    assert_eq!(saved.get("Kitchen"), Some(&Some(tag)));
    assert!(!state_text(&s).contains("Kitchen"), "counts only");
}

#[test]
fn the_tag_gate_refuses_when_required_and_posts_when_off_or_defaulted() {
    let f = fixture();
    let first_tag = f
        .link
        .demo_state()
        .expect("demo")
        .tags_mut()
        .first()
        .map(|t| t.id);
    assert!(first_tag.is_some(), "the demo has tags");
    {
        let mut d = f.link.demo_state().expect("demo");
        d.require_tag_every_line = true;
        d.default_tag = None;
    }
    let mut s = screen(&f);
    cmd(&mut s, "dev.payroll_demo_file_adp_run_gli");
    next_to(&mut s, Step::Preview);
    s.next();
    s.poll();
    assert_eq!(s.step, Step::Preview, "refused before /post");
    assert!(s.untagged.is_some(), "names the line");
    assert_eq!(s.posts_sent, 0);
    // Gate on with a default tag: it fills the lines and posts (Post
    // reads the company's default again, QA-14).
    if let Some(tag) = first_tag {
        f.link.demo_state().expect("demo").default_tag = Some(tag);
        s.next();
        s.poll();
        assert_eq!(s.step, Step::Done);
    }
    // Gate off: posts without tags.
    let f2 = fixture();
    f2.link.demo_state().expect("demo").require_tag_every_line = false;
    let mut s2 = screen(&f2);
    cmd(&mut s2, "dev.payroll_demo_file_adp_run_gli");
    next_to(&mut s2, Step::Done);
}

#[test]
fn the_same_file_twice_is_refused_and_points_to_history() {
    let f = fixture();
    let mut s = screen(&f);
    cmd(&mut s, "dev.payroll_demo_file_onpay_gl_summary");
    next_to(&mut s, Step::Done);
    cmd(&mut s, "dev.payroll_start_over");
    cmd(&mut s, "dev.payroll_demo_file_onpay_gl_summary");
    next_to(&mut s, Step::Accounts);
    assert_eq!(s.state()["duplicate_warning"], true);
    next_to(&mut s, Step::Preview);
    s.next();
    s.poll();
    assert_eq!(s.state()["trouble"], "duplicate");
    assert_eq!(s.step, Step::Preview);
}

#[test]
fn an_overlap_asks_and_post_anyway_uses_a_new_key() {
    let f = fixture();
    let mut s = screen(&f);
    cmd(&mut s, "dev.payroll_demo_file_generic_period1");
    s.next();
    s.poll();
    for (field, header) in [
        ("employee_name", "Worker"),
        ("gross_pay", "Wages"),
        ("net_pay", "Take Home"),
        ("check_date", "Paid On"),
    ] {
        s.set_column(field, Some(header.into()));
    }
    // Map only these: the other columns stay unmatched, so this posts an
    // unbalanced entry in the demo? No: the demo's entries are the golden's.
    next_to(&mut s, Step::Done);
    cmd(&mut s, "dev.payroll_start_over");
    cmd(&mut s, "dev.payroll_demo_file_generic_period2");
    s.period_text = [
        "07/01/2025".into(),
        "07/25/2025".into(),
        "07/25/2025".into(),
    ];
    next_to(&mut s, Step::Preview);
    s.next();
    s.poll();
    assert_eq!(s.state()["overlap_open"], 1);
    let first = s.key.as_ref().map(|k| k.1.clone()).expect("key");
    s.post_anyway();
    s.poll();
    assert_eq!(s.step, Step::Done);
    assert_ne!(
        s.key.as_ref().map(|k| k.1.clone()),
        Some(first),
        "a changed body gets a new key"
    );
    assert_eq!(s.posts_sent, 2);
}

#[test]
fn a_generic_file_re_imports_without_remapping() {
    let f = fixture();
    let mut s = screen(&f);
    cmd(&mut s, "dev.payroll_demo_file_generic_period1");
    s.next();
    s.poll();
    assert_eq!(s.step, Step::Map);
    for (field, header) in [
        ("employee_name", "Worker"),
        ("check_date", "Paid On"),
        ("gross_pay", "Wages"),
        ("net_pay", "Take Home"),
    ] {
        s.set_column(field, Some(header.into()));
    }
    next_to(&mut s, Step::Done);
    cmd(&mut s, "dev.payroll_start_over");
    cmd(&mut s, "dev.payroll_demo_file_generic_period2");
    s.next();
    s.poll();
    assert_eq!(s.step, Step::Accounts, "the saved layout is applied");
    assert_eq!(s.state()["layout"], "saved");
    next_to(&mut s, Step::Done);
    let visited = s.state()["steps_visited"].to_string();
    assert!(!visited.contains("map"), "{visited}");
}

/// S4: a saved layout that doesn't cover a file's amount columns stops on
/// Match columns and names them; the answer is saved, so the next file of
/// that shape skips the step; "Match columns again" returns to it from a
/// later step even when the layout was applied without it.
#[test]
fn new_amount_columns_stop_on_match_columns() {
    let f = fixture();
    let mut s = screen(&f);
    cmd(&mut s, "dev.payroll_demo_file_generic_period1");
    s.next();
    s.poll();
    for (field, header) in [
        ("employee_name", "Worker"),
        ("check_date", "Paid On"),
        ("gross_pay", "Wages"),
        ("net_pay", "Take Home"),
    ] {
        s.set_column(field, Some(header.into()));
    }
    next_to(&mut s, Step::Done);
    // As a layout saved before left-out columns were kept.
    cmd(&mut s, "dev.payroll_demo_forget_left_out");
    cmd(&mut s, "dev.payroll_start_over");
    cmd(&mut s, "dev.payroll_demo_file_generic_period2");
    s.next();
    s.poll();
    assert_eq!(s.step, Step::Map, "{}", state_text(&s));
    assert_eq!(s.state()["layout"], "saved");
    assert_eq!(
        s.new_columns,
        [
            "Fed WH",
            "State WH",
            "SS EE",
            "Medicare EE",
            "SS ER",
            "Medicare ER"
        ]
    );
    assert!(s.save_layout, "the answer is saved with the layout");
    assert!(!state_text(&s).contains("SS ER"), "counts only in state");
    s.set_column("social_security_er", Some("SS ER".into()));
    s.next();
    s.poll();
    assert_eq!(s.step, Step::Accounts);
    assert_eq!(s.new_columns, Vec::<String>::new());
    let t = s.templates_list()[0].column_map.clone().expect("map");
    assert_eq!(
        t.left_out.as_deref().map(<[String]>::len),
        Some(5),
        "the other new columns are remembered as left out"
    );
    s.cancel();
    // The same shape again: the layout applies with no stop.
    cmd(&mut s, "dev.payroll_start_over");
    cmd(&mut s, "dev.payroll_demo_file_generic_period2");
    s.next();
    s.poll();
    assert_eq!(s.step, Step::Accounts, "{}", state_text(&s));
    assert!(!s.visited.contains(&Step::Map));
    // Match columns again, from the entries.
    next_to(&mut s, Step::Preview);
    assert!(s.can_match_again());
    s.match_again();
    assert_eq!(s.step, Step::Map);
    assert!(s.save_layout && s.preview.is_none() && s.validation.is_none());
    next_to(&mut s, Step::Preview);
    // Not while a post is out.
    s.post_job = Some(Job::Done(None));
    assert!(!s.can_match_again());
    s.match_again();
    assert_eq!(s.step, Step::Preview);
}

#[test]
fn a_closed_period_asks_for_the_password() {
    let f = fixture();
    {
        let mut d = f.link.demo_state().expect("demo");
        let open = d.open.expect("open");
        if let Some(c) = d.companies.iter_mut().find(|c| c.tenant_id == open) {
            c.closed_through = CalendarDate::from_ymd(2025, 3, 31).ok();
        }
    }
    let mut s = screen(&f);
    cmd(&mut s, "dev.payroll_demo_file_adp_run_gli");
    next_to(&mut s, Step::Preview);
    s.next();
    s.poll();
    assert_eq!(s.state()["password_open"], true);
    s.password_text = "wrong".into();
    s.give_password();
    s.poll();
    assert_eq!(s.state()["trouble"], "password_wrong");
    assert!(
        s.key.is_none(),
        "another password is another body: a new key"
    );
    s.password_text = crate::company::demo::register::DEMO_CLOSING_PASSWORD.into();
    s.give_password();
    s.poll();
    assert_eq!(s.step, Step::Done);
    assert_eq!(s.posts_sent, 3, "one send per click, never on its own");
    // Reverse it from History: the password again (its own key).
    s.show_history();
    s.poll();
    let id = s.history.as_ref().expect("history").sessions[0].id;
    s.reverse(id, "Wrong file");
    s.poll();
    assert_eq!(s.state()["password_open"], true);
    s.password_text = crate::company::demo::register::DEMO_CLOSING_PASSWORD.into();
    s.give_password();
    s.poll();
    s.poll();
    assert_eq!(s.reversed, Some(1));
}

fn lost() -> ApiError {
    ApiError::OutcomeUnknown {
        request_id: cl_core::ids::RequestId::parse("00000000-0000-4000-8000-000000000002")
            .expect("id"),
        idempotency_key: Some(IdempotencyKey::new()),
        keyed: true,
    }
}

/// A post that is out: a request that never answers.
fn post_in_flight(s: &mut PayrollImportScreen) {
    s.post_job = Some(Job::Done(None));
    s.posts_sent += 1;
}

fn discards(f: &Fixture) -> u32 {
    f.link.demo_state().expect("demo").payroll.discards_received
}

/// M3: the lost answer comes through the post's own answer path (not set
/// by hand); nothing is sent again without a click; Try again reuses the
/// key, and Check status saying "not posted" keeps Try again (M2).
#[test]
fn a_lost_answer_is_never_sent_again_on_its_own() {
    // 1. The answer was lost after the engine posted: Check status finds
    // it posted (nothing sent again).
    let f = fixture();
    let mut s = screen(&f);
    cmd(&mut s, "dev.payroll_demo_file_adp_run_gli");
    next_to(&mut s, Step::Preview);
    s.post();
    let _ = s.post_job.take();
    s.after_post(Err(lost()));
    assert_eq!(s.state()["trouble"], "unknown");
    assert!(s.post_unsettled);
    for _ in 0..5 {
        s.poll();
    }
    assert_eq!(s.posts_sent, 1, "nothing goes out without a click");
    assert_eq!(f.link.demo_state().expect("demo").payroll.posts_received, 1);
    s.check_status();
    s.poll();
    assert_eq!(s.status_answer, Some(true));
    assert_eq!(s.step, Step::Done);
    assert!(!s.post_unsettled);
    s.try_again();
    s.poll();
    assert_eq!(s.posts_sent, 1, "settled: Try again sends nothing");

    // 2. Lost before it reached the engine: Check status says not posted,
    // and Try again stays offered (M2) and posts once.
    let f = fixture();
    let mut s = screen(&f);
    cmd(&mut s, "dev.payroll_demo_file_adp_run_gli");
    next_to(&mut s, Step::Preview);
    s.posts_sent += 1;
    s.after_post(Err(lost()));
    s.check_status();
    s.poll();
    assert_eq!(s.status_answer, Some(false));
    assert_eq!(s.state()["trouble"], "unknown", "Try again is still shown");
    assert!(s.post_unsettled, "the lost post may still land");
    assert_eq!(f.link.demo_state().expect("demo").payroll.posts_received, 0);
    s.try_again();
    let key = s.key.as_ref().map(|k| k.1.clone()).expect("key");
    s.poll();
    assert_eq!(s.step, Step::Done);
    assert_eq!(f.link.demo_state().expect("demo").payroll.posts_received, 1);
    // A replay with the same key answers the same: one import, posted once.
    let o = f.link.demo_state().expect("demo").payroll.sessions(None);
    assert_eq!(o.sessions.len(), 1, "one import, posted once");
    assert_eq!(s.key.as_ref().map(|k| k.1.clone()), Some(key));
}

/// B1: while a post is out, Cancel, Back, closing the window and History's
/// Discard send no DELETE; the close prompt says it may be posted.
#[test]
fn a_post_in_flight_is_never_discarded() {
    let f = fixture();
    let mut s = screen(&f);
    cmd(&mut s, "dev.payroll_demo_file_adp_run_gli");
    next_to(&mut s, Step::Preview);
    post_in_flight(&mut s);
    assert!(s.in_flight());
    assert!(s.save_unconfirmed(), "the close prompt warns");
    s.back();
    assert_eq!(s.step, Step::Preview, "Back waits for the answer");
    s.cancel();
    assert!(s.upload.is_some(), "Cancel waits for the answer");
    let id = s.upload.as_ref().map(|u| u.session).expect("upload");
    assert!(!s.may_discard_session(id));
    s.discard(id);
    assert_eq!(discards(&f), 0);
    // The window closes (prompt "Close anyway", or quit) with the post out.
    drop(s);
    assert_eq!(discards(&f), 0, "no DELETE while a post is out");
    assert_eq!(f.link.demo_state().expect("demo").payroll.unposted(), 1);
}

/// B1, QA-15: after a lost answer Cancel and Back do nothing (Check status
/// first), and closing keeps the draft (History's Check status settles it).
#[test]
fn a_lost_answer_keeps_the_draft_on_cancel_back_and_close() {
    for how in ["cancel", "back", "close", "check_then_close"] {
        let f = fixture();
        let mut s = screen(&f);
        cmd(&mut s, "dev.payroll_demo_file_adp_run_gli");
        next_to(&mut s, Step::Preview);
        post_in_flight(&mut s);
        let _ = s.post_job.take();
        s.after_post(Err(lost()));
        assert!(!s.in_flight());
        assert!(s.save_unconfirmed(), "{how}: the close prompt warns");
        match how {
            "cancel" => {
                let _ = f.link.drain();
                s.cancel();
                assert!(f.link.drain().is_empty(), "{how}: Cancel is off");
                assert!(s.upload.is_some(), "{how}");
            }
            "back" => {
                for _ in 0..4 {
                    s.back();
                }
                assert_eq!(s.step, Step::Preview, "{how}: Back is off");
                assert!(s.upload.is_some(), "{how}: still this window's");
            }
            "check_then_close" => {
                s.check_status();
                s.poll();
                assert_eq!(s.status_answer, Some(false));
                drop(s);
            }
            _ => drop(s),
        }
        assert_eq!(discards(&f), 0, "{how}: no DELETE");
        assert_eq!(
            f.link.demo_state().expect("demo").payroll.unposted(),
            1,
            "{how}: kept for History"
        );
    }
}

/// S2: the reverse key covers the reason and the password (as a hash); a
/// lost answer locks the reason and Try again resends the same body.
#[test]
fn a_reverse_key_covers_reason_and_password() {
    let f = fixture();
    let mut s = screen(&f);
    cmd(&mut s, "dev.payroll_demo_file_adp_run_gli");
    next_to(&mut s, Step::Done);
    s.show_history();
    s.poll();
    let id = s.history.as_ref().expect("history").sessions[0].id;
    let key_of = |s: &PayrollImportScreen| s.reverse_key.as_ref().map(|k| k.1.clone());
    // The first reverse's answer is lost.
    s.reverse(id, "Wrong file");
    let first = key_of(&s).expect("key");
    let _ = s.reverse_job.take();
    s.reverse_job = Some(Job::Done(Some(Err(lost()))));
    s.poll();
    assert!(s.reverse_unsettled && s.reverse_outstanding());
    // Leaving for the wizard keeps it (reason locked).
    s.show_import();
    s.view = View::History;
    assert_eq!(
        s.reversing.as_ref().map(|r| r.1.as_str()),
        Some("Wrong file")
    );
    s.try_again();
    assert_eq!(key_of(&s), Some(first), "Try again: the same body and key");
    s.poll();
    assert_eq!(key_of(&s), None, "settled: reversed");
    assert_eq!(s.reversed, Some(1));
    assert!(!s.reverse_outstanding());
    // Same import and password, another reason: another key.
    let plain = ReversePrint::of(id, "Wrong file", None);
    let other = ReversePrint::of(id, "Duplicate", None);
    assert!(plain != other);
    let pw = |p: &str| Secret::new(p.to_string());
    let one = ReversePrint::of(id, "Wrong file", Some(&pw("one")));
    let two = ReversePrint::of(id, "Wrong file", Some(&pw("two")));
    assert!(one != two && plain != one);
    assert!(one == ReversePrint::of(id, "Wrong file", Some(&pw("one"))));
}

/// A definite refusal settles the post: Cancel discards again.
#[test]
fn a_definite_answer_lets_cancel_discard() {
    let f = fixture();
    let mut s = screen(&f);
    cmd(&mut s, "dev.payroll_demo_file_adp_run_gli");
    next_to(&mut s, Step::Preview);
    s.after_post(Err(lost()));
    s.after_post(Err(ApiError::ClosingPasswordRejected));
    assert!(!s.post_unsettled);
    s.cancel();
    // QA-8: Cancel asks first; the prompt's Don't save closes (Drop).
    assert!(
        f.link
            .drain()
            .iter()
            .any(|e| matches!(e, CompanyEvent::RequestClose(_)))
    );
    drop(s);
    assert_eq!(discards(&f), 1);
}

/// payroll-import-review-v1: a discard that wins the race answers the
/// waiting post 404. The window says nothing was posted, lets go of the
/// draft without a DELETE and starts at the file step; Check status after
/// a lost answer reads a 404 the same way.
#[test]
fn a_post_answered_404_after_a_discard_is_not_posted() {
    for via_status in [false, true] {
        let f = fixture();
        let mut s = screen(&f);
        cmd(&mut s, "dev.payroll_demo_file_adp_run_gli");
        next_to(&mut s, Step::Preview);
        let id = s.upload.as_ref().map(|u| u.session).expect("upload");
        // Another window discarded it.
        f.link
            .demo_state()
            .expect("demo")
            .payroll
            .discard(id)
            .expect("discard");
        let before = discards(&f);
        if via_status {
            s.after_post(Err(lost()));
            s.check_status();
            s.poll();
        } else {
            s.after_post(Err(ApiError::NotFound {
                code: cl_api::ErrorCode::parse("NOT_FOUND"),
            }));
        }
        assert_eq!(s.state()["trouble"], "discarded", "status={via_status}");
        assert_eq!(
            t("payroll.refused.discarded"),
            "This import was discarded, so nothing was posted. Choose the file again to import it."
        );
        assert!(!s.post_unsettled);
        assert!(s.posted.is_none());
        assert!(s.upload.is_none());
        assert_eq!(s.step, Step::Choose);
        assert_eq!(f.link.demo_state().expect("demo").payroll.posts_received, 0);
        drop(s);
        assert_eq!(discards(&f), before, "no second DELETE");
    }
}

/// 422 `PAYROLL_DATE_INVALID` names the rows by number only.
#[test]
fn undated_rows_are_named_by_number() {
    let r = |rows: Vec<u64>| draw::undated_rows_text(&rows);
    assert_eq!(
        r(vec![4, 9]),
        "These rows in the file have no date the books can read: 4, 9. Add the dates in the file, then choose it again. Nothing was posted."
    );
    let many = r((1..=13).collect());
    assert!(
        many.contains("1, 2, 3, 4, 5, 6, 7, 8, 9, 10 and 3 more"),
        "{many}"
    );
    assert!(r(vec![]).starts_with("Some rows in the file have no date"));
    assert_eq!(
        Trouble::Refused(PayrollRefusal::DateInvalid { rows: vec![2] }).code(),
        "date_invalid"
    );
    let f = fixture();
    let mut s = screen(&f);
    cmd(&mut s, "dev.payroll_demo_file_adp_run_gli");
    next_to(&mut s, Step::Preview);
    let e = ApiError::Validation(cl_api::ValidationError::coded(
        "PAYROLL_DATE_INVALID",
        serde_json::json!({"rows": [{"rowNumber": 5, "code": "DATE_INVALID"}]}),
    ));
    s.after_post(Err(e));
    assert_eq!(s.state()["trouble"], "date_invalid");
    assert!(!s.post_unsettled);
}

#[test]
fn a_viewer_with_hidden_accounts_cannot_import() {
    let f = fixture();
    let mut s = screen(&f);
    cmd(&mut s, "dev.payroll_demo_restricted");
    assert!(!s.allowed());
    cmd(&mut s, "dev.payroll_demo_file_gusto");
    assert!(!s.can_next());
    assert_eq!(s.state()["trouble"], "restricted");
    assert!(
        !f.link.rights().door_open(Door::PayrollImport),
        "Home asks the same door"
    );
}

#[test]
fn cancel_after_upload_discards_the_draft() {
    let f = fixture();
    let mut s = screen(&f);
    cmd(&mut s, "dev.payroll_demo_file_square");
    next_to(&mut s, Step::Map);
    assert_eq!(f.link.demo_state().expect("demo").payroll.unposted(), 1);
    s.cancel();
    // QA-8: Cancel asks first; the prompt's Don't save closes (Drop).
    assert!(
        f.link
            .drain()
            .iter()
            .any(|e| matches!(e, CompanyEvent::RequestClose(_)))
    );
    assert_eq!(f.link.demo_state().expect("demo").payroll.unposted(), 1);
    drop(s);
    assert_eq!(f.link.demo_state().expect("demo").payroll.unposted(), 0);
}

/// Closing the window with an unposted draft (the prompt's "Don't save",
/// or quitting) is Cancel: the draft is discarded (Q-P30c).
#[test]
fn closing_the_window_discards_an_unposted_draft() {
    let f = fixture();
    let mut s = screen(&f);
    cmd(&mut s, "dev.payroll_demo_file_square");
    next_to(&mut s, Step::Map);
    assert!(s.dirty(), "closing asks first");
    assert_eq!(f.link.demo_state().expect("demo").payroll.unposted(), 1);
    drop(s);
    assert_eq!(f.link.demo_state().expect("demo").payroll.unposted(), 0);
}

/// A posted import is not touched when its window closes.
#[test]
fn closing_after_posting_keeps_the_import() {
    let f = fixture();
    let mut s = screen(&f);
    cmd(&mut s, "dev.payroll_demo_file_square");
    next_to(&mut s, Step::Done);
    assert!(!s.dirty());
    drop(s);
    let d = f.link.demo_state().expect("demo");
    assert_eq!(d.payroll.unposted(), 0);
    assert_eq!(
        d.payroll
            .sessions(None)
            .sessions
            .iter()
            .filter(|x| x.status == SessionStatus::Posted)
            .count(),
        1
    );
}

/// Home's "payroll import not posted yet" link opens History filtered to
/// unposted imports, in a new window and in one already open.
#[test]
fn home_link_opens_history_on_unposted_imports() {
    let f = fixture();
    let mut s = screen(&f);
    cmd(&mut s, "dev.payroll_demo_file_square");
    next_to(&mut s, Step::Map);
    // The draft stays when this window is set aside (not dropped).
    let draft = s.upload.take();
    drop(s);
    assert!(draft.is_some());

    open_history_unposted();
    let made = make(CommandId::new(KIND), &f.link, None, today()).expect("window");
    assert_eq!(made.state()["view"], "history");

    // Already open (on the wizard): its next frame switches.
    let mut open = screen(&f);
    assert_eq!(open.view, View::Import);
    open_history_unposted();
    open.take_history_request();
    open.poll();
    assert_eq!(open.view, View::History);
    assert!(open.unposted_only);
    let st = open.state();
    assert_eq!(st["history"]["rows"], 1, "{st}");
    assert_eq!(st["history"]["unposted"], 1, "{st}");
    // Asked once: the next window opens on the wizard.
    assert_eq!(
        make(CommandId::new(KIND), &f.link, None, today())
            .expect("window")
            .state()["view"],
        "import"
    );
}

/// The chosen file is read on a background thread; the step waits (busy)
/// and then has the file. Over 10 MB is refused before any read.
#[test]
fn the_file_is_read_off_the_egui_thread() {
    let f = fixture();
    let mut s = screen(&f);
    let dir = tempfile::tempdir().expect("tmp");
    let path = dir.path().join("payroll.csv");
    std::fs::write(
        &path,
        crate::company::demo::payroll::fixture_bytes("square").expect("fixture"),
    )
    .expect("write");
    s.take_path(&path);
    assert!(s.busy(), "reading");
    for _ in 0..200 {
        s.poll();
        if !s.busy() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(s.file.is_some(), "{}", state_text(&s));
    assert_eq!(s.file_error, None);

    let big = dir.path().join("big.csv");
    let file = std::fs::File::create(&big).expect("create");
    file.set_len(cl_api::payroll::MAX_UPLOAD_BYTES as u64 + 1)
        .expect("size");
    s.take_path(&big);
    for _ in 0..200 {
        s.poll();
        if !s.busy() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(s.file_error, Some("payroll.file.too_large"));
    assert!(s.file.is_none());
}

/// One frame of the window at 1600 × 1000 with `events`; its AccessKit
/// nodes.
fn draw_frame(
    s: &mut PayrollImportScreen,
    ctx: &eframe::egui::Context,
    events: Vec<eframe::egui::Event>,
) -> Vec<(
    eframe::egui::accesskit::NodeId,
    eframe::egui::accesskit::Node,
)> {
    use crate::windows::Screen as _;
    use eframe::egui::{self, Pos2, RawInput, Rect, vec2};
    let input = RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1600.0, 1000.0))),
        events,
        ..RawInput::default()
    };
    let mut out = ctx.run_ui(input, |ui| {
        egui::CentralPanel::default().show(ui, |ui| s.ui(ui));
    });
    out.textures_delta.clear();
    out.platform_output
        .accesskit_update
        .map(|u| u.nodes)
        .unwrap_or_default()
}

/// A press and release at the centre of `node`'s box, over two frames.
fn click_node(
    s: &mut PayrollImportScreen,
    ctx: &eframe::egui::Context,
    node: &eframe::egui::accesskit::Node,
) -> Vec<(
    eframe::egui::accesskit::NodeId,
    eframe::egui::accesskit::Node,
)> {
    use eframe::egui::{Event, Modifiers, PointerButton, pos2};
    let b = node.bounds().expect("bounds");
    #[allow(clippy::cast_possible_truncation)] // pixel positions
    let at = pos2(
        f64::midpoint(b.x0, b.x1) as f32,
        f64::midpoint(b.y0, b.y1) as f32,
    );
    let press = |pressed| Event::PointerButton {
        pos: at,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::default(),
    };
    draw_frame(s, ctx, vec![Event::PointerMoved(at), press(true)]);
    draw_frame(s, ctx, vec![press(false)]);
    draw_frame(s, ctx, Vec::new())
}

/// QA-2: every service in the list can be picked with the pointer, in
/// turn, including the last two (egui's default list height cut them off).
#[test]
fn every_payroll_service_can_be_picked_from_the_list() {
    use eframe::egui::accesskit::Role;
    let f = fixture();
    let mut s = screen(&f);
    let ctx = eframe::egui::Context::default();
    crate::theme::apply(&ctx, crate::theme::ThemeChoice::Light);
    crate::fonts::install(&ctx);
    ctx.enable_accesskit();
    let mut nodes = Vec::new();
    for _ in 0..3 {
        nodes = draw_frame(&mut s, &ctx, Vec::new());
    }
    for p in Provider::ALL.into_iter().rev().chain([Provider::Gusto]) {
        let label = nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Label && n.value() == Some("Payroll service"))
            .map(|(id, _)| *id)
            .expect("service label");
        let combo = nodes
            .iter()
            .find(|(_, n)| n.labelled_by().contains(&label))
            .map(|(_, n)| n.clone())
            .expect("service list");
        let open = click_node(&mut s, &ctx, &combo);
        let name = t(&format!("payroll.provider.{}", p.key()));
        let item = open
            .iter()
            .find(|(_, n)| {
                !matches!(n.role(), Role::Label | Role::ComboBox)
                    && (n.label() == Some(name.as_str()) || n.value() == Some(name.as_str()))
            })
            .map_or_else(|| panic!("{name} in the open list"), |(_, n)| n.clone());

        nodes = click_node(&mut s, &ctx, &item);
        assert_eq!(s.provider, p, "{name} picked");
    }
}

/// QA-1 on the demo: the Accounts step starts on the payroll accounts by
/// name, never on the first account of a kind; a line nothing matches
/// says "Choose an account" and keeps Next off until it is chosen.
#[test]
fn accounts_start_on_payroll_accounts_or_empty() {
    let f = fixture();
    let mut s = screen(&f);
    cmd(&mut s, "dev.payroll_demo_file_gusto");
    s.next();
    s.poll();
    s.next();
    s.poll();
    assert_eq!(s.step, Step::Accounts);
    let name = |s: &PayrollImportScreen, line: &str| {
        s.line_accounts.get(line).and_then(|id| {
            s.chart
                .iter()
                .find(|a| a.id == *id)
                .map(|a| a.fields.name.clone())
        })
    };
    assert_eq!(name(&s, "gross_wages_expense").as_deref(), Some("Wages"));
    assert_eq!(
        name(&s, "employer_tax_expense").as_deref(),
        Some("Payroll Taxes")
    );
    for l in ["fit_payable", "ss_payable", "retirement_payable"] {
        assert_eq!(name(&s, l).as_deref(), Some("Payroll Liabilities"), "{l}");
    }
    assert_eq!(
        name(&s, cl_model::payroll_import::NET_PAY_LINE).as_deref(),
        Some("Operating Checking")
    );
    assert_eq!(name(&s, "reimbursement_expense"), None);
    assert!(!s.accounts_complete());
    assert!(!s.can_next());
    choose_unmatched(&mut s);
    assert!(s.can_next());
}

/// QA-5: the engine's notes are grouped by kind with a row count, never
/// listed per row, and never block.
#[test]
fn check_notes_are_grouped_by_kind() {
    use cl_api::payroll::ValidationMessage;
    let m = |row: Option<u64>, code: &str, is_error| ValidationMessage {
        row_number: row,
        field: "check_date".into(),
        code: code.into(),
        is_error,
    };
    let msgs = vec![
        m(Some(1), "STALE_DATE", false),
        m(Some(2), "STALE_DATE", false),
        m(Some(5), "DUPLICATE_EMPLOYEE", false),
        m(Some(3), "AMOUNT_INVALID", true),
        m(None, "DUPLICATE_FILE", false),
        m(Some(3), "STALE_DATE", false),
    ];
    assert_eq!(
        draw::note_groups(&msgs),
        vec![
            ("STALE_DATE".to_string(), 3),
            ("DUPLICATE_EMPLOYEE".to_string(), 1),
            ("DUPLICATE_FILE".to_string(), 0),
        ]
    );
    assert_eq!(draw::t_count("payroll.rows", 1), "1 row");
    assert_eq!(draw::t_count("payroll.rows", 3), "3 rows");
    assert_eq!(draw::t_count("payroll.post", 1), "Post 1 entry");
    assert_eq!(draw::t_count("payroll.post", 2), "Post 2 entries");
    assert_eq!(
        draw::t_count("payroll.done.withdrawals", 1),
        "1 expected withdrawal is waiting to be matched in the Bank Feed."
    );
}

/// QA-4: an engine time shows as this computer's date, not the UTC day.
#[test]
fn posted_day_is_the_local_date() {
    let ts = "2026-11-18T00:16:00.000Z";
    let expected = cl_core::Timestamp::parse_wire(ts)
        .map(|t| t.as_utc().with_timezone(&chrono::Local).date_naive())
        .expect("time");
    let got = draw::posted_day(ts).expect("day");
    assert_eq!(
        (got.year(), got.month(), got.day()),
        (
            chrono::Datelike::year(&expected),
            chrono::Datelike::month(&expected),
            chrono::Datelike::day(&expected)
        )
    );
    assert_eq!(
        draw::posted_day("2026-11-17"),
        CalendarDate::from_ymd(2026, 11, 17).ok()
    );
}

/// QA-8: Cancel with a chosen file asks (the tab's close prompt) instead
/// of closing; QA-6: a run the engine says is posted keeps Post off.
#[test]
fn cancel_asks_and_a_posted_run_keeps_post_off() {
    let f = fixture();
    let mut s = screen(&f);
    s.cancel();
    assert!(
        f.link
            .drain()
            .iter()
            .any(|e| matches!(e, CompanyEvent::CloseWindow(_))),
        "an empty wizard closes"
    );
    cmd(&mut s, "dev.payroll_demo_file_gusto");
    s.cancel();
    let events = f.link.drain();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, CompanyEvent::RequestClose(_))),
        "a chosen file asks first"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, CompanyEvent::CloseWindow(_)))
    );
    next_to(&mut s, Step::Preview);
    assert!(s.can_next());
    if let Some(u) = &mut s.upload {
        u.is_duplicate_by_key = true;
    }
    assert!(s.posted_duplicate());
    assert!(!s.can_next(), "Post stays off for a posted run");
}

/// QA-12: when the upload says this payroll was posted before, the
/// warnings and a refusal without its own date name the earlier post's
/// local date, found in the posted imports; History shows it too.
#[test]
fn a_posted_duplicate_says_when_it_was_posted() {
    let f = fixture();
    let mut s = screen(&f);
    cmd(&mut s, "dev.payroll_demo_file_onpay_gl_summary");
    next_to(&mut s, Step::Done);
    let first = f.link.demo_state().expect("demo").payroll.sessions(None).sessions[0].clone();
    let day = super::draw::posted_day(first.posted_at.as_deref().expect("posted at"))
        .map(show_date)
        .expect("a date");
    cmd(&mut s, "dev.payroll_start_over");
    cmd(&mut s, "dev.payroll_demo_file_onpay_gl_summary");
    next_to(&mut s, Step::Accounts);
    s.poll(); // the posted imports' read (the next frame)
    // The same file: the earlier post is found by its file hash.
    let text = s.duplicate_text().expect("warned");
    assert!(text.contains(&day), "{text}");
    // The same run (the engine's run key): Post stays off and says when.
    if let Some(u) = &mut s.upload {
        u.is_duplicate_by_key = true;
    }
    let text = s.duplicate_text().expect("warned");
    assert!(text.contains(&day) && text.contains("already posted on"), "{text}");
    // Two posted imports of the run: no guess, the undated text.
    let mut twin = first.clone();
    twin.id = PayrollSessionId::parse("00000000-0000-4000-8000-0000000000aa").expect("id");
    s.prior.push(twin);
    assert_eq!(s.duplicate_text(), Some(t("payroll.duplicate_posted")));
    s.prior.retain(|p| p.id == first.id);
    // A refusal naming the earlier import but no date: its date is looked up.
    s.trouble = Some(Trouble::Refused(PayrollRefusal::DuplicateRun {
        prior: Some(first.id),
        posted_at: None,
    }));
    assert_eq!(s.prior_posted_at(), first.posted_at);
    // History: "Posted <date>" on the posted row (the demo's posted time).
    assert!(
        crate::i18n::t_with("payroll.status.posted_on", &[("date", &day)])
            .starts_with("Posted ")
    );
}

/// QA-14: with "Require a tag on every line" on and no default tag, the
/// Check step says tags are still needed and Post is refused; a default
/// tag set in Preferences while Review is open fills the lines at once
/// (no Back/Next), and a tag the owner picked by hand stays.
#[test]
fn a_default_tag_set_while_review_is_open_applies_at_once() {
    let f = fixture();
    let tags = f.link.demo_state().expect("demo").tags_mut().clone();
    let (Some(default), Some(picked)) = (tags.first().map(|t| t.id), tags.get(1).map(|t| t.id))
    else {
        panic!("the demo has two tags");
    };
    {
        let mut d = f.link.demo_state().expect("demo");
        d.require_tag_every_line = true;
        d.default_tag = None;
    }
    let mut s = screen(&f);
    cmd(&mut s, "dev.payroll_demo_file_adp_run_gli");
    next_to(&mut s, Step::Check);
    assert!(s.tags_still_needed(), "the Check step says tags are needed");
    next_to(&mut s, Step::Preview);
    s.next();
    s.poll();
    assert!(s.untagged.is_some(), "refused: lines without a tag");
    assert_eq!(s.posts_sent, 0);
    // The owner tags one line by hand.
    s.plan.set(0, 0, Some(picked));
    // Preferences › Tags saves a default tag (it publishes Tags).
    f.link.demo_state().expect("demo").default_tag = Some(default);
    f.link.bus().publish(crate::data_bus::Change::Tags);
    s.poll();
    assert_eq!(s.step, Step::Preview, "still on Review");
    assert_eq!(s.plan.default_tag, Some(default), "applied without Back/Next");
    assert!(s.untagged.is_none(), "the refusal is worked out again");
    let first = s.plan.lines[0].clone();
    assert_eq!(s.plan.final_tag(&first), Some(picked), "the hand-picked tag stays");
    let other = s.plan.lines[1].clone();
    assert_eq!(s.plan.final_tag(&other), Some(default));
    assert!(!s.tags_still_needed());
    s.next();
    s.poll();
    assert_eq!(s.step, Step::Done, "posts with every line tagged");
}

/// QA-15, order 1: the books stop BEFORE the post is sent (cl-api answers
/// EngineUnavailable): nothing was posted, the window says so, and once
/// the books run again the message is replaced and Post posts once.
#[test]
fn engine_down_before_the_post_is_sent_lets_post_again() {
    let f = fixture();
    let mut s = screen(&f);
    cmd(&mut s, "dev.payroll_demo_file_adp_run_gli");
    next_to(&mut s, Step::Preview);
    s.posts_sent += 1;
    s.after_post(Err(ApiError::EngineUnavailable { permanent: false }));
    assert_eq!(s.state()["trouble"], "not_sent");
    assert!(!s.post_unsettled, "nothing was sent");
    let id = s.upload.as_ref().map(|u| u.session).expect("upload");
    assert!(!s.is_unsettled(id));
    // The books are running again (the demo always is): the stale line
    // goes, replaced by what to do.
    s.poll();
    assert_eq!(s.state()["trouble"], "engine_back_post");
    assert!(s.can_next(), "Post may be pressed again");
    assert_eq!(f.link.demo_state().expect("demo").payroll.posts_received, 0);
    s.next();
    s.poll();
    assert_eq!(s.step, Step::Done);
    assert_eq!(f.link.demo_state().expect("demo").payroll.posts_received, 1);
    // Down on another step: the same, without the post wording.
    let mut s = screen(&f);
    s.trouble = Some(Trouble::EngineDown);
    s.poll();
    assert_eq!(s.state()["trouble"], "engine_back");
}

/// QA-15, order 2: the books stop AFTER the post was sent (its answer is
/// lost): the outcome-unknown state (Cancel and Back off, Check status),
/// History marks the draft "Not confirmed" with Check status and no
/// Discard, in this window and another; Check status settles it.
#[test]
fn engine_down_after_the_post_is_sent_is_outcome_unknown() {
    let f = fixture();
    let mut s = screen(&f);
    cmd(&mut s, "dev.payroll_demo_file_adp_run_gli");
    next_to(&mut s, Step::Preview);
    // The engine got it and posted; the answer was lost.
    s.post();
    let _ = s.post_job.take();
    s.after_post(Err(lost()));
    assert_eq!(s.state()["trouble"], "unknown");
    s.poll();
    assert_eq!(s.state()["trouble"], "unknown", "not replaced by 'running again'");
    let id = s.upload.as_ref().map(|u| u.session).expect("upload");
    assert!(s.is_unsettled(id));
    s.back();
    assert_eq!(s.step, Step::Preview, "Back is off");
    // Another Import Payroll window: History offers no Discard for it.
    let other = screen(&f);
    assert!(other.is_unsettled(id));
    assert!(!other.may_discard_session(id));
    // History's Check status settles it (the post had landed).
    s.show_history();
    s.poll();
    s.check_draft_status(id);
    s.poll();
    s.poll();
    assert_eq!(s.status_answer, Some(true));
    assert!(!s.post_unsettled && !s.is_unsettled(id));
    assert_eq!(f.link.demo_state().expect("demo").payroll.posts_received, 1);
    let entries = f.link.demo_state().expect("demo").payroll.sessions(None).sessions;
    assert_eq!(entries.len(), 1, "posted once");
}

/// QA-13: Cancel and the tab's close button ask in payroll words (not the
/// bank import's); a post whose answer was lost keeps its own warning.
#[test]
fn the_close_prompt_speaks_of_payroll() {
    let f = fixture();
    let mut windows = crate::windows::Windows::new();
    let mut s = screen(&f);
    cmd(&mut s, "dev.payroll_demo_file_adp_run_gli");
    let id = windows.open_new(CommandId::new(KIND), Box::new(s));
    let (msg, close, keep) = crate::windows::close_prompt_text(windows.get(id), "Import Payroll");
    assert_eq!(msg, "This payroll import hasn't been posted. Discard it?");
    assert_eq!((close.as_str(), keep.as_str()), ("Discard", "Keep working"));
    assert!(!msg.contains("Bank Feed"));
    // A lost post: closing keeps the import, and the prompt says so.
    let mut s = screen(&f);
    cmd(&mut s, "dev.payroll_demo_file_adp_run_gli");
    next_to(&mut s, Step::Preview);
    post_in_flight(&mut s);
    let _ = s.post_job.take();
    s.after_post(Err(lost()));
    let id = windows.open_new(CommandId::new(KIND), Box::new(s));
    let (msg, _, _) = crate::windows::close_prompt_text(windows.get(id), "Import Payroll");
    assert!(msg.contains("History") && !msg.contains("Discard it"), "{msg}");
}
