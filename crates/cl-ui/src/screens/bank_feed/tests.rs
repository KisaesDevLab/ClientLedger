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

/// P26 review m3: A on a pending row with a suggestion goes through the
/// one accept route as proposed (a staged split posts as staged), and its
/// Undo takes back the accept group.
#[test]
fn a_on_a_suggested_row_accepts_as_proposed() {
    let (_d, mut s) = screen();
    s.segment = Segment::Ready;
    s.reload();
    let id = s.rows[0].id;
    assert_eq!(s.rows[0].status, FeedStatus::Pending);
    assert!(s.rows[0].suggested_account.is_some());
    cmd(&mut s, "feed.accept");
    assert_eq!(s.last_send, Some("accept"));
    assert_eq!(s.counts.accepted, 1);
    let target = s.toast.as_ref().map(|x| x.target.clone()).expect("toast");
    assert!(matches!(target, cl_api::UndoTarget::Group(_)), "{id}");
}

/// P26 QA-1, QA-23: every column keeps room at every width; the amount
/// never overlaps its neighbours.
#[test]
fn columns_fit_from_zoomed_to_wide() {
    for w in (240..=1600).step_by(10) {
        #[allow(clippy::cast_precision_loss, reason = "test widths are small")]
        let w = w as f32;
        let c = view::columns(w);
        assert!(c.bank.0 <= c.bank.1, "{w}: {c:?}");
        assert!(c.recorded.0 <= c.recorded.1, "{w}: {c:?}");
        assert!(c.amount_right <= w, "{w}: {c:?}");
        assert!(c.why < w, "{w}: {c:?}");
        if c.compact {
            assert!(c.bank.1 <= c.amount_right - 96.0, "{w}: {c:?}");
            assert!(c.recorded.1 <= c.why, "{w}: {c:?}");
        } else {
            assert!(c.bank.1 - c.bank.0 >= 110.0, "{w}: bank {c:?}");
            assert!(c.recorded.1 - c.recorded.0 >= 140.0, "{w}: recorded {c:?}");
            assert!(c.bank.1 + 96.0 <= c.amount_right, "{w}: {c:?}");
            assert!(c.amount_right + 20.0 <= c.recorded.0, "{w}: {c:?}");
            assert!(c.recorded.1 <= c.why, "{w}: {c:?}");
        }
    }
    assert!(view::columns(500.0).compact);
    assert!(!view::columns(800.0).compact);
}

/// P26 QA-8: Show › Accepted says Accepted, not "Needs you".
#[test]
fn accepted_rows_say_accepted() {
    let (_d, mut s) = screen();
    s.segment = Segment::Ready;
    s.reload();
    cmd(&mut s, "feed.accept");
    s.show = Show::Accepted;
    s.reload();
    assert_eq!(s.rows.len(), 1);
    assert_eq!(s.why(&s.rows[0]), Why::Accepted);
    let st = s.state();
    assert_eq!(st["badges"]["accepted"], 1);
    assert_eq!(st["badges"]["needs_you"], 0);
}

/// P26 QA-9, QA-15: what Undo will do, by what the accept was.
#[test]
fn accepted_text_follows_what_was_done() {
    use cl_api::FeedStatus as F;
    assert_eq!(
        detail::accepted_text(F::Matched, Some("deposit")),
        "bank_feed.detail.deposited"
    );
    assert_eq!(
        detail::accepted_text(F::Matched, Some("transfer")),
        "bank_feed.detail.transferred"
    );
    assert_eq!(
        detail::accepted_text(F::Matched, Some("match")),
        "bank_feed.detail.matched"
    );
    assert_eq!(
        detail::accepted_text(F::Matched, None),
        "bank_feed.detail.linked"
    );
    assert_eq!(
        detail::accepted_text(F::Categorized, None),
        "bank_feed.detail.posted"
    );
}

/// U-178: the engine's `acceptKind` says what the accept was, also for a
/// row this window didn't accept (another window, a restart); without it
/// the window's own memory, else both outcomes.
#[test]
fn the_accept_kind_names_what_undo_will_do() {
    use cl_api::{AcceptKind as K, FeedStatus as F};
    let (_d, mut s) = screen();
    s.reload();
    let mut r = s.rows[0].clone();
    r.status = F::Matched;
    for (k, how, key) in [
        (K::Deposit, "deposit", "bank_feed.detail.deposited"),
        (K::Transfer, "transfer", "bank_feed.detail.transferred"),
        (K::Match, "match", "bank_feed.detail.matched"),
        (K::Payment, "match", "bank_feed.detail.matched"),
    ] {
        r.accept_kind = Some(k);
        assert_eq!(s.accepted_how(&r), Some(how));
        assert_eq!(detail::accepted_text(r.status, s.accepted_how(&r)), key);
    }
    r.accept_kind = None;
    assert_eq!(
        detail::accepted_text(r.status, s.accepted_how(&r)),
        "bank_feed.detail.linked"
    );
    s.accepted_as.insert(r.id, "deposit");
    assert_eq!(s.accepted_how(&r), Some("deposit"));
    // The engine wins over this window's memory.
    r.accept_kind = Some(K::Match);
    assert_eq!(s.accepted_how(&r), Some("match"));
    r.status = F::Categorized;
    r.accept_kind = Some(K::Post);
    assert_eq!(
        detail::accepted_text(r.status, s.accepted_how(&r)),
        "bank_feed.detail.posted"
    );
}

/// QA-16 rest (U-177): the list and the counts ask with the owner's
/// transfer window.
#[test]
fn the_list_asks_with_the_owners_transfer_window() {
    let (_d, mut s) = screen();
    s.transfer_window = 7;
    assert_eq!(s.query().transfer_window_days, Some(7));
}

/// P26 QA-24: a Match row opens on Match, and A on an unfinished row says
/// what's missing instead of doing nothing.
#[test]
fn match_rows_open_on_match_and_a_is_never_silent() {
    let (_d, mut s) = screen();
    s.segment = Segment::Matches;
    s.reload();
    assert_eq!(s.rows.len(), 3);
    for i in 0..s.rows.len() {
        s.list.focus(s.rows[i].id);
        detail::follow(&mut s);
        assert_eq!(s.detail.mode, detail::Mode::Match, "row {i}");
    }
    s.segment = Segment::Review;
    s.reload();
    let needs = s
        .rows
        .iter()
        .find(|r| s.why(r) == Why::NeedsYou)
        .map(|r| r.id)
        .expect("a needs-you row");
    s.list.focus(needs);
    detail::follow(&mut s);
    cmd(&mut s, "feed.accept");
    assert_eq!(s.notice.map(|n| n.0), Some("bank_feed.notice.pick_account"));
}

/// P26 QA-18: C opens the account box; the chart narrows as you type.
#[test]
fn c_opens_the_account_box() {
    let (_d, mut s) = screen();
    s.segment = Segment::Review;
    s.reload();
    detail::follow(&mut s);
    assert!(s.command(CommandId::new("feed.choose_account")));
    assert!(s.detail.find.is_some());
    assert_eq!(s.detail.mode, detail::Mode::Categorize);
    assert!(s.command(CommandId::new("form.revert")), "Esc closes it");
    assert!(s.detail.find.is_none());
    let found = detail::find_accounts(&s.chart, "bank fees");
    assert!(found.iter().all(|(_, n)| {
        let n = n.to_lowercase();
        n.contains("bank") && n.contains("fees")
    }));
    assert_ne!(found.len(), 0);
    assert_eq!(detail::find_accounts(&s.chart, "zzzz no such").len(), 0);
}

/// P26 QA-10, QA-11: R carries the account just chosen; the saved rule
/// sets up the waiting line at once and offers "Accept all".
#[test]
fn a_rule_from_a_line_sets_up_the_waiting_lines() {
    let (_d, mut s) = screen();
    s.segment = Segment::Review;
    s.reload();
    let id = s
        .rows
        .iter()
        .find(|r| s.why(r) == Why::NeedsYou)
        .map(|r| r.id)
        .expect("a needs-you row");
    s.list.focus(id);
    detail::follow(&mut s);
    let account = s
        .chart
        .iter()
        .find(|(_, n)| n.contains("6050"))
        .map(|c| c.0)
        .expect("6050");
    s.detail.account = Some(account);
    cmd(&mut s, "feed.rule");
    let draft = s.rules.draft.as_mut().expect("draft");
    assert_eq!(
        draft.rule.account,
        Some(account),
        "R keeps the chosen account"
    );
    draft.rule.auto_accept = true;
    rules::save_draft(&mut s);
    s.poll();
    assert!(
        s.rules.applied.is_some_and(|n| n >= 1),
        "{:?}",
        s.rules.applied
    );
    s.segment = Segment::All;
    s.reload();
    let row = s.row(id).cloned().expect("still waiting");
    assert_eq!(s.why(&row), Why::Rule);
    assert!(row.suggested_rule.as_ref().is_some_and(|r| r.auto_accept));
}

/// P26 QA-7: same-size payments can be told apart.
#[test]
fn waiting_payments_name_their_payer() {
    assert_eq!(
        detail::payer_label(Some("Barn Owl Books"), None, None),
        "Barn Owl Books"
    );
    assert_eq!(
        detail::payer_label(Some("Barn Owl Books"), Some("1042"), None),
        "Barn Owl Books #1042"
    );
    assert_eq!(
        detail::payer_label(None, Some("1042"), Some("customer_payment")),
        "Payment #1042"
    );
    assert_eq!(
        detail::payer_label(Some("  "), None, Some("journal_entry")),
        "Journal entry"
    );
    assert_eq!(
        detail::payer_label(None, None, Some("no_such_type")),
        "Payment"
    );
}

/// P26 QA-16: a pre-picked partner outside the owner's window is dropped
/// when the panel's own read (with the window) doesn't list it.
#[test]
fn a_partner_outside_the_window_is_not_pre_picked() {
    let mut d = detail::Detail::default();
    let other: cl_api::FeedItemId = "d26f9300-0000-4000-8000-000000000001".parse().expect("id");
    d.pick_transfer = Some(other);
    d.take_data(detail::DetailData::default());
    assert_eq!(d.pick_transfer, None);
}

/// P26 QA-13: "From the bank" shows the bank's own text.
#[test]
fn the_bank_text_is_the_banks_own() {
    let (_d, s) = screen();
    let mut r = s.rows[0].clone();
    r.description = "Online".into();
    r.original_description = Some("ONLINE TRANSFER TO CARD".into());
    assert_eq!(bank_text(&r), "ONLINE TRANSFER TO CARD");
    r.original_description = Some("  ".into());
    assert_eq!(bank_text(&r), "Online");
}
