//! A05: the receipt's coding (Q-A05b) and the list's offers.

#![allow(
    clippy::needless_pass_by_value,
    clippy::assert_is_empty,
    reason = "test builders and readable checks"
)]

use super::*;
use cl_docread::model::{Bbox, Confidence, Found, Header, Line, RuleId, Spot};

fn money(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}

fn found<T>(value: T) -> Found<T> {
    Found {
        value,
        spot: Spot {
            page: 0,
            bbox: Bbox::PAGE,
        },
        rule: RuleId::new("test"),
        confidence: Confidence::HIGH,
    }
}

fn line(desc: &str, amount: &str) -> serde_json::Value {
    serde_json::to_value(Line {
        description: Some(found(desc.to_string())),
        quantity: None,
        unit_price: None,
        amount: found(money(amount)),
    })
    .expect("line")
}

fn draft(lines: Vec<serde_json::Value>, reference: Option<&str>) -> DocumentDraft {
    let header = Header {
        vendor: Some(found("Synthetic Hardware Co.".to_string())),
        ..Header::default()
    };
    serde_json::from_value(serde_json::json!({
        "id": "d0000000-0000-4000-8000-dd0000000001",
        "kind": "cash_expense",
        "status": "ready",
        "fields": serde_json::to_value(header).expect("header"),
        "lines": lines,
        "docReference": reference,
        "docTotal": "84.1700",
        "docDate": "2026-11-03",
        "suggestedContactId": "c0000000-0000-4000-8000-000000000001",
        "updatedAt": "2026-11-08T10:00:00.000Z",
    }))
    .expect("draft")
}

#[test]
fn lines_become_splits_only_when_they_add_up_exactly_in_cents() {
    let d = draft(
        vec![line("Shop supplies", "60.00"), line("Hardware", "24.17")],
        Some("R-1042"),
    );
    let c = coding(&d, money("84.17"));
    assert_eq!(c.lines.len(), 2);
    assert_eq!(c.lines[1].0, money("24.17"));
    assert_eq!(c.lines[0].1.as_deref(), Some("Shop supplies"));
    assert_eq!(
        c.memo.as_deref(),
        Some("Synthetic Hardware Co. \u{b7} R-1042")
    );
    assert!(c.contact.is_some());
    // Either sign of the bank amount.
    assert_eq!(coding(&d, money("-84.17")).lines.len(), 2);

    // One cent off (tax not on a line): one account.
    let off = draft(
        vec![line("Shop supplies", "60.00"), line("Hardware", "24.16")],
        None,
    );
    let c = coding(&off, money("84.17"));
    assert!(c.lines.is_empty());
    assert_eq!(c.memo.as_deref(), Some("Synthetic Hardware Co."));
    // A line with a fourth decimal can't be a split.
    let fine = draft(vec![line("A", "60.0050"), line("B", "24.1650")], None);
    assert!(coding(&fine, money("84.17")).lines.is_empty());
    // One line: no split.
    let one = draft(vec![line("All", "84.17")], None);
    assert!(coding(&one, money("84.17")).lines.is_empty());
}

#[test]
fn the_choice_puts_every_line_on_the_panels_account() {
    let d = draft(
        vec![line("Shop supplies", "60.00"), line("Hardware", "24.17")],
        Some("R-1042"),
    );
    let account: AccountId = "a0000000-0000-4000-8000-000000000001".parse().expect("a");
    let c = choice_of(&coding(&d, money("84.17")), account, None);
    assert_eq!(c.account, Some(account));
    assert_eq!(c.splits.len(), 2);
    assert!(c.splits.iter().all(|l| l.account == account));
    assert_eq!(
        Money::sum(c.splits.iter().map(|l| l.amount)).expect("sum"),
        money("84.17")
    );
    // No vendor matched: the line's payee stays.
    let mut plain = coding(&d, money("84.17"));
    plain.contact = None;
    let payee: ContactId = "c0000000-0000-4000-8000-000000000009".parse().expect("c");
    assert_eq!(choice_of(&plain, account, Some(payee)).contact, Some(payee));
}

#[test]
fn statements_and_closed_drafts_are_never_offered() {
    let mut d = draft(Vec::new(), None);
    assert!(!facts(&d).statement);
    d.kind = DraftKind::Statement;
    assert!(facts(&d).statement);
    d.kind = DraftKind::CashExpense;
    d.status = DraftStatus::Converted;
    assert!(facts(&d).statement, "not live");
}

#[test]
fn coding_debug_has_no_owner_data() {
    let d = draft(
        vec![line("Secret item", "60.00"), line("B", "24.17")],
        Some("R-1042"),
    );
    let shown = format!("{:?}", coding(&d, money("84.17")));
    assert!(!shown.contains("Secret"));
    assert!(!shown.contains("60"));
    assert!(!shown.contains("R-1042"));
}

// ------------------------------------------- the panel on the demo engine

mod panel {
    use std::sync::Arc;

    use cl_model::CommandId;

    use super::super::super::BankFeedScreen;
    use super::super::super::send::{self, Send};
    use super::super::*;
    use crate::company::CompanyLink;
    use crate::windows::Screen;

    /// The demo's receipt line in the panel, its receipt read in full.
    fn seeded() -> (tempfile::TempDir, BankFeedScreen, FeedRow, egui::Context) {
        let dir = tempfile::tempdir().expect("dir");
        let link = Arc::new(CompanyLink::demo(dir.path()));
        let mut s = BankFeedScreen::new(link, None);
        assert!(s.command(CommandId::new("dev.demo_feed_receipt")));
        s.poll();
        let line = *s
            .receipts
            .offers
            .keys()
            .next()
            .expect("a line with a receipt");
        let r = s.rows.iter().find(|r| r.id == line).cloned().expect("row");
        s.detail.row = Some(line);
        s.detail.account = r.suggested_account.as_ref().map(|a| a.id);
        reset(&mut s, line);
        let ctx = egui::Context::default();
        follow(&mut s, &ctx);
        assert!(s.detail.receipt.full.is_some(), "read in full");
        (dir, s, r, ctx)
    }

    fn values(s: &BankFeedScreen, r: &FeedRow) -> Send {
        Send::Categorize(
            r.id,
            Some(FeedChoice {
                account: s.detail.account,
                ..FeedChoice::default()
            }),
        )
    }

    fn carried(send: Option<&Send>) -> Option<&AcceptItem> {
        match send {
            Some(Send::Accept(items)) => items.first().filter(|i| i.draft().is_some()),
            _ => None,
        }
    }

    /// Review 5: a draft whose file is gone is never offered (the engine
    /// would refuse it every time).
    #[test]
    fn a_draft_without_its_file_is_never_offered() {
        let (_d, s, _r, _ctx) = seeded();
        assert_eq!(offers_for(&s.rows, &s.receipts.drafts).len(), 1);
        let mut drafts = s.receipts.drafts.clone();
        for d in &mut drafts {
            d.attachment_id = None;
        }
        assert!(offers_for(&s.rows, &drafts).is_empty());
    }

    /// Review 2 (Q-A05c): a total edited since the list was read doesn't
    /// travel with the line; the line posts without it.
    #[test]
    fn a_receipt_whose_total_changed_is_not_carried() {
        let (_d, mut s, r, _ctx) = seeded();
        let send = with_receipt(&s, &r, Some(values(&s, &r)));
        assert!(carried(send.as_ref()).is_some(), "as read: carried");
        if let Some(full) = s.detail.receipt.full.as_mut() {
            full.doc_total = full
                .doc_total
                .and_then(|m| m.checked_add(Money::parse_wire("0.01").expect("m")).ok());
        }
        let send = with_receipt(&s, &r, Some(values(&s, &r)));
        assert!(matches!(send, Some(Send::Categorize(..))), "posts alone");
    }

    /// Nit: a row staged by a rule posts as staged, the receipt filed on it
    /// (its split and memo kept).
    #[test]
    fn a_staged_row_posts_as_staged_with_the_receipt() {
        let (_d, s, r, _ctx) = seeded();
        let send = with_receipt(&s, &r, Some(Send::Categorize(r.id, None)));
        let item = carried(send.as_ref()).expect("carried");
        assert_eq!(item.inner(), &AcceptItem::AsProposed(r.id));
    }

    /// Nit: a receipt that couldn't be read holds Accept; Try again reads
    /// it again.
    #[test]
    fn a_receipt_that_couldnt_be_read_holds_accept_until_read_again() {
        let (_d, mut s, r, ctx) = seeded();
        s.detail.receipt.failed = true;
        assert!(with_receipt(&s, &r, Some(values(&s, &r))).is_none());
        read_again(&mut s);
        follow(&mut s, &ctx);
        assert!(!s.detail.receipt.failed);
        assert!(carried(with_receipt(&s, &r, Some(values(&s, &r))).as_ref()).is_some());
    }

    /// Review 1: a STALE refusal reads the receipt again, so the next A
    /// carries the fresh change token and posts.
    #[test]
    fn a_stale_receipt_is_read_again_and_the_next_accept_posts() {
        let (_d, mut s, r, ctx) = seeded();
        if let Some(full) = s.detail.receipt.full.as_mut() {
            full.updated_at = "2020-01-01T00:00:00.000Z".into();
        }
        let x = with_receipt(&s, &r, Some(values(&s, &r))).expect("send");
        send::start(&mut s, x, None);
        s.poll();
        assert_eq!(s.counts.accepted, 0, "nothing posted");
        assert_eq!(
            s.notice.map(|n| n.0),
            Some("bank_feed.notice.receipt_changed")
        );
        // The panel reads it again: the stored token, not the stale one.
        follow(&mut s, &ctx);
        let fresh = s.detail.receipt.full.as_ref().expect("read again");
        assert_ne!(fresh.updated_at, "2020-01-01T00:00:00.000Z");
        let x = with_receipt(&s, &r, Some(values(&s, &r))).expect("send");
        send::start(&mut s, x, None);
        s.poll();
        assert_eq!(s.counts.accepted, 1, "the next A posts");
    }

    /// Review 1: a list showing a newer token than the one read reads the
    /// receipt again.
    #[test]
    fn a_newer_listed_token_reads_the_receipt_again() {
        let (_d, mut s, _r, ctx) = seeded();
        let pick = s.detail.receipt.pick.expect("pick");
        if let Some(d) = s.receipts.drafts.iter_mut().find(|d| d.id == pick) {
            d.updated_at = "2099-01-01T00:00:00.000Z".into();
        }
        refresh(&mut s);
        assert!(s.detail.receipt.full.is_none(), "read again");
        follow(&mut s, &ctx);
        assert!(s.detail.receipt.full.is_some());
    }

    /// Review 3: refused for permission, the panel stops offering it.
    #[test]
    fn forbidden_declines_the_receipt() {
        let (_d, mut s, r, _ctx) = seeded();
        forbidden(&mut s);
        assert!(s.detail.receipt.declined);
        let x = with_receipt(&s, &r, Some(values(&s, &r)));
        assert!(matches!(x, Some(Send::Categorize(..))));
        refresh(&mut s);
        assert!(s.detail.receipt.pick.is_none(), "stays declined");
    }
}
