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
