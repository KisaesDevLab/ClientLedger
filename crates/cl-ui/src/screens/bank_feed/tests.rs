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
