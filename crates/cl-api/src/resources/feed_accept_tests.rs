use super::*;
use cl_core::Money;

use crate::resources::feed_review::FeedSplitLine;

const A: &str = "11111111-1111-4111-8111-111111111111";
const B: &str = "22222222-2222-4222-8222-222222222222";
const G: &str = "99999999-9999-4999-8999-999999999999";
const T: &str = "33333333-3333-4333-8333-333333333333";
const L: &str = "44444444-4444-4444-8444-444444444444";
const ACCT: &str = "55555555-5555-4555-8555-555555555555";

fn fid(s: &str) -> FeedItemId {
    FeedItemId::parse(s).expect("id")
}

#[test]
fn items_body_carries_values_matches_and_bare_ids() {
    let choice = FeedChoice {
        account: Some(ACCT.parse().expect("acct")),
        splits: vec![
            FeedSplitLine {
                account: ACCT.parse().expect("acct"),
                amount: Money::parse_wire("60.00").expect("m"),
                tag: None,
                memo: None,
            },
            FeedSplitLine {
                account: ACCT.parse().expect("acct"),
                amount: Money::parse_wire("24.17").expect("m"),
                tag: None,
                memo: None,
            },
        ],
        ..FeedChoice::default()
    };
    let body = accept_body(&AcceptRequest::Items(vec![
        AcceptItem::AsProposed(fid(A)),
        AcceptItem::Values(fid(B), choice),
        AcceptItem::Match {
            id: fid(G),
            transaction: T.parse().expect("t"),
            line: Some(L.parse().expect("l")),
        },
    ]))
    .expect("body");
    let items = body["items"].as_array().expect("items");
    assert_eq!(items[0], json!({ "id": A }));
    assert_eq!(items[1]["accountId"], ACCT);
    // Split amounts are exact strings with 2 decimals, never numbers.
    assert_eq!(items[1]["splits"][0]["amount"], "60.00");
    assert_eq!(items[1]["splits"][1]["amount"], "24.17");
    assert_eq!(items[2]["matchTransactionId"], T);
    assert_eq!(items[2]["journalLineId"], L);
    assert!(items[2].get("accountId").is_none());
}

#[test]
fn items_body_refuses_empty_repeats_and_too_many() {
    let bad = |r: AcceptRequest| {
        matches!(
            accept_body(&r),
            Err(ApiError::Local(LocalValidation::InvalidValue))
        )
    };
    assert!(bad(AcceptRequest::Items(vec![])));
    assert!(bad(AcceptRequest::Items(vec![
        AcceptItem::AsProposed(fid(A)),
        AcceptItem::AsProposed(fid(A)),
    ])));
    let many = (0..=ACCEPT_MAX)
        .map(|n| AcceptItem::AsProposed(fid(&format!("aaaaaaaa-0000-4000-8000-{n:012}"))))
        .collect();
    assert!(bad(AcceptRequest::Items(many)));
}

#[test]
fn rule_body_sends_the_listed_token_exactly() {
    let body = accept_body(&AcceptRequest::Rule(RuleAccept {
        rule_id: A.into(),
        kind: "legacy".into(),
        updated_at: "2026-10-01T09:00:00.123Z".into(),
        connection: Some(BankConnectionId::parse(B).expect("c")),
        after: Some(fid(G)),
    }))
    .expect("body");
    assert_eq!(
        body,
        json!({
            "ruleId": A,
            "ruleKind": "legacy",
            "ruleUpdatedAt": "2026-10-01T09:00:00.123Z",
            "bankConnectionId": B,
            "afterId": G,
        })
    );
    let empty = accept_body(&AcceptRequest::Rule(RuleAccept {
        rule_id: A.into(),
        kind: "conditional".into(),
        updated_at: String::new(),
        connection: None,
        after: None,
    }));
    assert!(empty.is_err());
}

#[test]
fn undo_body_items_group_and_force() {
    assert_eq!(
        undo_body(&UndoTarget::Items(vec![fid(A), fid(B)]), false).expect("b"),
        json!({ "feedItemIds": [A, B] })
    );
    assert_eq!(
        undo_body(&UndoTarget::Group(UndoGroupId::parse(G).expect("g")), true).expect("b"),
        json!({ "undoGroupId": G, "force": true })
    );
    assert!(undo_body(&UndoTarget::Items(vec![]), false).is_err());
    assert!(undo_body(&UndoTarget::Items(vec![fid(A), fid(A)]), false).is_err());
}

#[test]
fn accept_answer_reads_skips_failures_and_paging() {
    let body = json!({
        "undoGroupId": G,
        "posted": 2,
        "matched": 1,
        "skipped": [
            { "id": A, "reason": "has_match" },
            { "id": B, "reason": "something_new" },
        ],
        "failures": [
            { "id": T, "code": "TB_PERIOD_LOCKED", "error": "Closed", "details": { "closingDate": "2026-10-31" } },
            { "id": L, "code": "SPLITS_DONT_BALANCE", "error": "x" },
        ],
        "remaining": 740,
        "nextAfterId": L,
    });
    let o = accept_outcome_from(body.to_string().as_bytes()).expect("outcome");
    assert_eq!(o.undo_group.to_string(), G);
    assert_eq!((o.posted, o.matched, o.remaining), (2, 1, 740));
    assert_eq!(o.skipped[0], (fid(A), AcceptSkip::HasMatch));
    assert_eq!(o.skipped[1].1, AcceptSkip::Other);
    assert!(!AcceptSkip::HasMatch.is_stale());
    assert!(AcceptSkip::InUse.is_stale());
    assert_eq!(o.locked(), vec![fid(T)]);
    assert_eq!(
        o.failures[0].details.text("closingDate"),
        Some("2026-10-31")
    );
    assert_eq!(o.next_after, Some(fid(L)));
    let last = json!({ "undoGroupId": G, "nextAfterId": null });
    let o = accept_outcome_from(last.to_string().as_bytes()).expect("last");
    assert_eq!((o.posted, o.remaining, o.next_after), (0, 0, None));
    assert!(accept_outcome_from(b"{}").is_err());
}

#[test]
fn undo_answer_reads_deleted_kept_and_failures() {
    let body = json!({
        "undone": [
            { "id": A, "kind": "categorize", "transactionId": T, "entry": "deleted", "undoGroupId": G },
            { "id": B, "kind": "match", "transactionId": T, "entry": "kept", "undoGroupId": G },
        ],
        "failures": [
            { "id": L, "code": "FEED_UNDO_RECONCILED", "error": "Reconciled" },
        ],
    });
    let o = undo_outcome_from(body.to_string().as_bytes()).expect("undo");
    assert_eq!(o.undone.len(), 2);
    assert!(o.undone[0].deleted);
    assert!(!o.undone[1].deleted);
    assert_eq!(o.failures[0].code, AcceptFailure::UNDO_RECONCILED);
    assert_eq!(o.locked().len(), 0);
}

#[test]
fn undo_group_is_read_from_posting_answers() {
    let body = json!({ "transaction": { "id": T }, "items": [], "undoGroupId": G });
    assert_eq!(
        undo_group_of(body.to_string().as_bytes()).map(|g| g.to_string()),
        Some(G.to_string())
    );
    assert_eq!(undo_group_of(b"{}"), None);
    let posted =
        crate::resources::feed_review::posted_from(body.to_string().as_bytes()).expect("posted");
    assert_eq!(
        posted.undo_group.map(|g| g.to_string()),
        Some(G.to_string())
    );
}

#[test]
fn debug_prints_no_owner_data() {
    let r = RuleAccept {
        rule_id: "secret-rule".into(),
        kind: "conditional".into(),
        updated_at: "2026".into(),
        connection: None,
        after: None,
    };
    assert!(!format!("{r:?}").contains("secret"));
    let f = AcceptFailure {
        id: fid(A),
        code: "X".into(),
        details: ConflictDetails::new(json!({ "difference": "12.34" })),
    };
    assert!(!format!("{f:?}").contains("12.34"));
}

// ------------------------------------------------------------- A05 receipts

const D: &str = "66666666-6666-4666-8666-666666666666";
const TOKEN: &str = "2026-11-08T10:00:00.123Z";

fn link() -> DraftLink {
    DraftLink {
        id: DocumentDraftId::parse(D).expect("draft"),
        expected_updated_at: TOKEN.into(),
    }
}

/// U-181: a receipt rides on its item as `documentDraft {id,
/// expectedUpdatedAt}`, the token sent exactly as read.
#[test]
fn a_receipt_item_carries_the_draft_and_its_token_exactly() {
    let choice = FeedChoice {
        account: Some(ACCT.parse().expect("acct")),
        memo: Some("Receipt 4411".into()),
        ..FeedChoice::default()
    };
    let body = accept_body(&AcceptRequest::Items(vec![
        AcceptItem::Receipt {
            item: Box::new(AcceptItem::Values(fid(A), choice)),
            draft: link(),
        },
        AcceptItem::Receipt {
            item: Box::new(AcceptItem::Match {
                id: fid(B),
                transaction: T.parse().expect("t"),
                line: None,
            }),
            draft: DraftLink {
                id: DocumentDraftId::parse("77777777-7777-4777-8777-777777777777").expect("draft"),
                expected_updated_at: TOKEN.into(),
            },
        },
    ]))
    .expect("body");
    let items = body["items"].as_array().expect("items");
    assert_eq!(items[0]["id"], A);
    assert_eq!(items[0]["accountId"], ACCT);
    assert_eq!(
        items[0]["documentDraft"],
        json!({ "id": D, "expectedUpdatedAt": TOKEN })
    );
    assert_eq!(items[1]["matchTransactionId"], T);
    assert_eq!(items[1]["documentDraft"]["expectedUpdatedAt"], TOKEN);
    assert!(items[1].get("accountId").is_none());
}

#[test]
fn a_receipt_inside_a_receipt_or_without_a_token_is_refused() {
    let nested = AcceptItem::Receipt {
        item: Box::new(AcceptItem::Receipt {
            item: Box::new(AcceptItem::AsProposed(fid(A))),
            draft: link(),
        }),
        draft: link(),
    };
    let blank = AcceptItem::Receipt {
        item: Box::new(AcceptItem::AsProposed(fid(A))),
        draft: DraftLink {
            expected_updated_at: "  ".into(),
            ..link()
        },
    };
    for i in [nested, blank] {
        assert_eq!(
            accept_body(&AcceptRequest::Items(vec![i])),
            Err(ApiError::Local(LocalValidation::InvalidValue))
        );
    }
    // The row id is the inner item's (a repeat is still a repeat).
    let twice = vec![
        AcceptItem::AsProposed(fid(A)),
        AcceptItem::Receipt {
            item: Box::new(AcceptItem::AsProposed(fid(A))),
            draft: link(),
        },
    ];
    assert!(accept_body(&AcceptRequest::Items(twice)).is_err());
}

#[test]
fn receipt_failures_are_named() {
    let body = json!({
        "undoGroupId": G,
        "posted": 0,
        "matched": 0,
        "failures": [
            { "id": A, "code": "DOCUMENT_DRAFT_CLOSED", "details": { "transactionId": T } },
            { "id": B, "code": "STALE", "details": { "updatedAt": TOKEN } },
        ],
    });
    let o = accept_outcome_from(body.to_string().as_bytes()).expect("outcome");
    assert!(o.failures.iter().all(AcceptFailure::is_receipt));
    assert_eq!(o.failures[0].code, AcceptFailure::DRAFT_CLOSED);
    assert_eq!(o.failures[1].code, AcceptFailure::DRAFT_STALE);
    assert!(o.locked().is_empty());
    let other = AcceptFailure {
        id: fid(A),
        code: AcceptFailure::PERIOD_LOCKED.into(),
        details: ConflictDetails::default(),
    };
    assert!(!other.is_receipt());
}

/// U-180: Undo lists the drafts it put back in the review queue; an engine
/// without it says nothing (empty).
#[test]
fn undo_answer_reads_reopened_drafts() {
    let body = json!({
        "undone": [{ "id": A, "kind": "categorize", "transactionId": T,
                     "entry": "deleted", "undoGroupId": G }],
        "failures": [],
        "reopenedDrafts": [D],
    });
    let o = undo_outcome_from(body.to_string().as_bytes()).expect("undo");
    assert_eq!(
        o.reopened_drafts,
        vec![DocumentDraftId::parse(D).expect("d")]
    );
    let old = json!({ "undone": [], "failures": [] });
    let o = undo_outcome_from(old.to_string().as_bytes()).expect("undo");
    assert!(o.reopened_drafts.is_empty());
    let bad = json!({ "undone": [], "reopenedDrafts": ["nope"] });
    assert!(undo_outcome_from(bad.to_string().as_bytes()).is_err());
}

#[test]
fn a_receipt_link_debug_has_no_token() {
    let i = AcceptItem::Receipt {
        item: Box::new(AcceptItem::AsProposed(fid(A))),
        draft: link(),
    };
    let shown = format!("{i:?}");
    assert!(shown.contains(D));
    assert!(!shown.contains(TOKEN));
}

/// Without `feed-accept-draft-v1` an accept with a receipt sends nothing;
/// one without a receipt still goes.
#[tokio::test]
async fn a_receipt_needs_its_capability_and_nothing_is_sent_without_it() {
    use crate::resources::testkit::{json as reply, requests, session_with_caps};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer};

    let server = MockServer::start().await;
    let answer = json!({ "undoGroupId": G, "posted": 1, "matched": 0 });
    Mock::given(method("POST"))
        .and(path("/api/v1/banking/feed/accept"))
        .respond_with(reply(200, &answer.to_string()))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &[Capabilities::FEED_ACCEPT]).await;
    let with = AcceptRequest::Items(vec![AcceptItem::Receipt {
        item: Box::new(AcceptItem::AsProposed(fid(A))),
        draft: link(),
    }]);
    let e = s
        .feed()
        .accept(&with, None, IdempotencyKey::new())
        .await
        .expect_err("unsupported");
    assert_eq!(e, ApiError::Local(LocalValidation::Unsupported));
    assert!(requests(&server, "POST").await.is_empty());

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/banking/feed/accept"))
        .respond_with(reply(200, &answer.to_string()))
        .mount(&server)
        .await;
    let s = session_with_caps(
        &server,
        &[Capabilities::FEED_ACCEPT, Capabilities::FEED_ACCEPT_DRAFT],
    )
    .await;
    let o = s
        .feed()
        .accept(&with, None, IdempotencyKey::new())
        .await
        .expect("accepted");
    assert_eq!(o.posted, 1);
    let posts = requests(&server, "POST").await;
    assert_eq!(posts.len(), 1);
    let sent: Value = serde_json::from_slice(&posts[0].body).expect("json");
    assert_eq!(sent["items"][0]["documentDraft"]["id"], D);
}
