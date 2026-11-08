//! The Bank Feed window on the demo engine (P26 DI-1): the sample's
//! 28 / 6 / 3, keyboard triage, Accept selected, view-only and another
//! computer taking a row.

use std::sync::Arc;

use cl_model::{CommandId, Scope};

use super::*;
use crate::company::CompanyLink;

fn screen() -> (tempfile::TempDir, BankFeedScreen) {
    let dir = tempfile::tempdir().expect("dir");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    let s = BankFeedScreen::new(link, None);
    (dir, s)
}

fn cmd(s: &mut BankFeedScreen, id: &'static str) {
    assert!(s.command(CommandId::new(id)), "{id}");
    s.act_on_keys();
    s.poll();
}

#[test]
fn opens_on_the_samples_waiting_rows() {
    let (_d, s) = screen();
    assert_eq!(s.scope(), Scope::Feed);
    assert_eq!(
        (
            s.counts.waiting,
            s.counts.ready,
            s.counts.review,
            s.counts.matches
        ),
        (37, 28, 6, 3)
    );
    assert_eq!(s.rows.len(), 37);
    assert_eq!(s.list.cursor(), Some(0));
    let st = s.state();
    assert_eq!(st["screen"], "bank_feed");
    assert!(st["ai_badges"].as_u64().is_some_and(|n| n > 0));
    assert!(st["badges"]["rule"].as_u64().is_some_and(|n| n > 0));
    assert_eq!(st["badges"]["match"], 3);
}

#[test]
fn ai_off_shows_no_ai_badges() {
    let (_d, mut s) = screen();
    assert!(s.command(CommandId::new("dev.demo_ai_off")));
    s.poll();
    assert!(!s.ai_on);
    assert_eq!(s.state()["ai_badges"], 0);
    assert_eq!(s.rows.len(), 37, "rules still work");
}

#[test]
fn keys_accept_and_accept_selected() {
    let (_d, mut s) = screen();
    s.segment = Segment::Ready;
    s.reload();
    assert_eq!(s.rows.len(), 28);
    cmd(&mut s, "feed.select");
    cmd(&mut s, "feed.next");
    cmd(&mut s, "feed.select");
    cmd(&mut s, "feed.next");
    assert_eq!(s.list.selected().len(), 2);
    cmd(&mut s, "feed.accept_selected");
    assert_eq!(s.counts.ready, 26);
    assert_eq!(s.counts.accepted, 2);
    // A on the cursor's ready row records it.
    cmd(&mut s, "feed.accept");
    assert_eq!(s.counts.accepted, 3);
    assert_eq!(s.counts.waiting, 34);
    // S leaves the row for the detail panel.
    cmd(&mut s, "feed.split");
    assert_eq!(s.state()["detail_request"], "split");
}

#[test]
fn view_only_and_another_computer() {
    let (_d, mut s) = screen();
    assert!(s.command(CommandId::new("dev.demo_feed_conflict")));
    s.poll();
    s.segment = Segment::Ready;
    s.reload();
    cmd(&mut s, "feed.select");
    cmd(&mut s, "feed.accept_selected");
    assert_eq!(s.notice.map(|n| n.0), Some("bank_feed.notice.changed"));
    assert!(s.list.selected().is_empty(), "the row left the selection");
    assert!(s.command(CommandId::new("dev.demo_feed_view_only")));
    s.poll();
    assert!(s.view_only);
    let before = s.counts.accepted;
    cmd(&mut s, "feed.accept");
    assert_eq!(s.counts.accepted, before, "nothing written");
    cmd(&mut s, "feed.next");
    assert_eq!(s.list.cursor(), Some(1), "moving still works");
}

/// DI-2: A records the row, the Undo toast takes it back (Q-P26x: a real
/// demo entry is made and removed).
#[test]
fn accept_then_undo_from_the_toast() {
    let (_d, mut s) = screen();
    s.segment = Segment::Ready;
    s.reload();
    cmd(&mut s, "feed.accept");
    assert_eq!(s.counts.accepted, 1);
    let target = s.toast.as_ref().map(|x| x.target.clone()).expect("toast");
    send::start(&mut s, send::Send::Undo(target, false), None);
    s.poll();
    assert_eq!(s.counts.accepted, 0);
    assert_eq!(s.counts.waiting, 37);
    assert!(s.toast.is_none());
    assert_eq!(s.notice.map(|n| n.0), Some("bank_feed.notice.undone"));
}

/// DI-2 (plan §5): locked rows in a selection ask for the password once;
/// the right one records them.
#[test]
fn closed_period_asks_for_the_password_and_retries() {
    let (_d, mut s) = screen();
    assert!(s.command(CommandId::new("dev.demo_feed_close_books")));
    s.poll();
    s.segment = Segment::Ready;
    s.reload();
    s.list.toggle_all();
    let before = s.counts.accepted;
    cmd(&mut s, "feed.accept_selected");
    assert!(s.closing.dialog_open(), "the password prompt is up");
    let open_posted = s.counts.accepted - before;
    assert!(open_posted > 0, "the open rows posted");
    s.closing.password = crate::screens::company::form::SecretText::from(
        crate::company::demo::register::DEMO_CLOSING_PASSWORD,
    );
    let (again, pw) = s.closing.submit().expect("submit");
    send::start(&mut s, again, Some(pw));
    s.poll();
    assert!(!s.closing.dialog_open());
    assert!(
        s.counts.accepted > before + open_posted,
        "the locked rows posted"
    );
}

/// DI-2 (Q-P26t): a deposit of one waiting payment less a fee line, at a
/// 0.00 difference, from the detail panel.
#[test]
fn deposit_with_a_fee_from_the_panel() {
    let (_d, mut s) = screen();
    let zelle = s
        .rows
        .iter()
        .find(|r| r.description.contains("ZELLE"))
        .map(|r| r.id)
        .expect("zelle");
    s.list.focus(zelle);
    detail::follow(&mut s);
    s.poll();
    detail::follow(&mut s);
    s.detail.mode = detail::Mode::Deposit;
    let pay = s
        .detail
        .data
        .payments
        .iter()
        .find(|p| p.amount.to_wire() == "1250.0000")
        .map(|p| p.id)
        .expect("payment");
    s.detail.picks.insert(pay);
    assert_eq!(detail::state(&s)["deposit_balanced"], false);
    s.detail.fees.push(detail::Line {
        account: Some(crate::company::demo::account_id(6050)),
        amount: "325.00".into(),
        memo: String::new(),
    });
    assert_eq!(detail::state(&s)["deposit_difference"], "0.0000");
    assert_eq!(detail::state(&s)["deposit_balanced"], true);
    let row = s.row(zelle).cloned().expect("row");
    let d = s.detail.deposit_draft(&row);
    send::start(
        &mut s,
        send::Send::Deposit(
            zelle,
            d.payments.iter().map(|(id, _)| *id).collect(),
            vec![cl_api::FeeLine {
                account: crate::company::demo::account_id(6050),
                amount: cl_core::Money::parse_wire("325.00").expect("m"),
                memo: None,
            }],
        ),
        None,
    );
    s.poll();
    assert_eq!(s.counts.accepted, 1);
    assert!(s.toast.is_some(), "a deposit can be undone");
}

/// DI-2: Ctrl+Insert / Ctrl+Delete in the split grid.
#[test]
fn split_lines_insert_and_delete() {
    let (_d, mut s) = screen();
    detail::follow(&mut s);
    cmd(&mut s, "feed.split");
    assert_eq!(s.detail.mode, detail::Mode::Split);
    assert_eq!(s.detail.split.len(), 2, "S opens two lines");
    assert!(s.command(CommandId::new("feed.insert_line")));
    assert_eq!(s.detail.split.len(), 3);
    assert!(s.command(CommandId::new("feed.delete_line")));
    assert!(s.command(CommandId::new("feed.delete_line")));
    assert_eq!(s.detail.split.len(), 1);
}

/// DI-2: R makes a rule from the cursor's line; Rules lists and edits.
#[test]
fn r_makes_a_rule_from_the_line() {
    let (_d, mut s) = screen();
    s.segment = Segment::Ready;
    s.reload();
    cmd(&mut s, "feed.rule");
    s.poll();
    assert!(s.rules.open);
    let draft = s.rules.draft.clone().expect("draft");
    assert_eq!(draft.rule.sign, cl_api::RuleSign::MoneyOut);
    assert!(draft.rule.account.is_some());
    let before = s.rules.rules.len();
    assert!(rules::state(&s)["draft_ready"].as_bool().unwrap_or(false));
    let r = s
        .rules
        .draft
        .as_ref()
        .map(|d| d.rule.clone())
        .expect("rule");
    assert!(
        crate::company::demo::feed_review::save_rule(&mut s.link.demo_state().expect("demo"), &r)
            .is_ok()
    );
    rules::read(&mut s);
    s.poll();
    assert_eq!(s.rules.rules.len(), before + 1);
}
