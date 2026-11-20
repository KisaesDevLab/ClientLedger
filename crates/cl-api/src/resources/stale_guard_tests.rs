//! Where each stale-checked write puts its stamp, against a recording mock
//! (P33, U-291/U-292). The source scan in `tests/stale_stamps.rs` checks
//! that every stale route's method carries a stamp at all; these check the
//! wire: the body for a `DELETE` whose handler reads the body (review S3),
//! the query only where the handler parses it, per-item `stamps` on
//! merges, and nothing new on an engine without `stale-all-v1`.

use cl_core::ids::{AccountId, ContactId, TagGroupId, TagId, TransactionId};
use serde_json::Value;
use wiremock::matchers::{any, method, path};
use wiremock::{Mock, MockServer, Request};

use crate::idempotency::IdempotencyKey;
use crate::resources::testkit::{json, session_with_caps};
use crate::resources::versioning::{RecordStamp, RecordVersion};
use crate::session::Session;

const STAMP: &str = "2026-11-20T10:00:00.123Z";
const OTHER: &str = "2026-11-20T09:00:00.456Z";
const ID: &str = "2a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const ID2: &str = "3a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";

/// Every capability the pinned client knows (so no write is refused
/// locally), with or without `stale-all-v1`.
fn all_caps(stale_all: bool) -> Vec<&'static str> {
    include_str!("../wire.rs")
        .lines()
        .filter_map(|l| {
            let l = l.trim();
            let rest = l.strip_prefix("pub const ")?;
            let (_, value) = rest.split_once(": &'static str = \"")?;
            value.strip_suffix("\";")
        })
        .filter(|c| stale_all || *c != crate::wire::Capabilities::STALE_ALL)
        .collect()
}

async fn mock(stale_all: bool) -> (MockServer, Session) {
    let server = MockServer::start().await;
    let s = session_with_caps(&server, &all_caps(stale_all)).await;
    Mock::given(method("GET"))
        .and(path("/api/v1/company"))
        .respond_with(json(
            200,
            &format!(r#"{{"company":{{"id":"{ID}","updatedAt":"{STAMP}"}}}}"#),
        ))
        .mount(&server)
        .await;
    Mock::given(any())
        .respond_with(json(200, "{}"))
        .mount(&server)
        .await;
    (server, s)
}

fn stamp(s: &str) -> RecordStamp {
    RecordStamp::parse(s).expect("stamp")
}

fn version(s: &str) -> RecordVersion {
    RecordVersion::Stamped {
        fingerprint: [0; 32],
        stamp: stamp(s),
    }
}

/// The first request with `method` whose path ends with `tail`.
async fn sent(server: &MockServer, m: &str, tail: &str) -> Request {
    server
        .received_requests()
        .await
        .expect("recorded")
        .into_iter()
        .find(|r| r.method.as_str() == m && r.url.path().ends_with(tail))
        .unwrap_or_else(|| panic!("no {m} …{tail}"))
}

fn body(r: &Request) -> Value {
    serde_json::from_slice(&r.body).unwrap_or(Value::Null)
}

fn query(r: &Request) -> String {
    r.url.query().unwrap_or("").to_string()
}

/// The stamp is in the body and nowhere in the query.
fn in_body(r: &Request) {
    assert_eq!(
        body(r)["expectedUpdatedAt"],
        STAMP,
        "{} {}",
        r.method,
        r.url
    );
    assert!(!query(r).contains("expectedUpdatedAt"), "{}", r.url);
}

#[tokio::test]
async fn deletes_and_deactivations_send_the_stamp_in_the_body() {
    let (server, s) = mock(true).await;
    let v = version(STAMP);
    let account = AccountId::parse(ID).expect("id");
    let _ = s
        .accounts()
        .deactivate(account, &v, IdempotencyKey::new())
        .await;
    in_body(&sent(&server, "DELETE", &format!("/accounts/{ID}")).await);
    let contact = ContactId::parse(ID).expect("id");
    let _ = s
        .contacts()
        .deactivate(contact, &v, IdempotencyKey::new())
        .await;
    in_body(&sent(&server, "DELETE", &format!("/contacts/{ID}")).await);
    let tag = TagId::parse(ID).expect("id");
    let _ = s.tags().delete(tag, &v, IdempotencyKey::new()).await;
    in_body(&sent(&server, "DELETE", &format!("/tags/{ID}")).await);
    let group = TagGroupId::parse(ID2).expect("id");
    let _ = s
        .tags()
        .delete_group(group, Some(stamp(STAMP)), IdempotencyKey::new())
        .await;
    in_body(&sent(&server, "DELETE", &format!("/tags/groups/{ID2}")).await);
    let _ = s
        .team()
        .delete_template(uuid::Uuid::parse_str(ID).expect("id"), Some(stamp(STAMP)))
        .await;
    in_body(&sent(&server, "DELETE", &format!("/permission-templates/{ID}")).await);
}

#[tokio::test]
async fn the_transaction_delete_and_voids_carry_the_stamp_in_the_body() {
    let (server, s) = mock(true).await;
    let txn = TransactionId::parse(ID).expect("id");
    let _ = s
        .transactions()
        .delete(
            txn,
            Some(stamp(STAMP)),
            &crate::resources::transactions::DeleteOptions::default(),
            IdempotencyKey::new(),
        )
        .await;
    in_body(&sent(&server, "DELETE", &format!("/transactions/{ID}")).await);
    let _ = s
        .transactions()
        .void(
            txn,
            Some(stamp(STAMP)),
            "wrong payee",
            &crate::resources::posting::PostingOptions::default(),
            IdempotencyKey::new(),
        )
        .await;
    in_body(&sent(&server, "POST", &format!("/transactions/{ID}/void")).await);
    let _ = s
        .ap_voids()
        .bill(txn, Some(stamp(STAMP)), "duplicate", IdempotencyKey::new())
        .await;
    in_body(&sent(&server, "POST", &format!("/bills/{ID}/void")).await);
}

#[tokio::test]
async fn edits_without_a_version_column_send_expected_updated_at() {
    let (server, s) = mock(true).await;
    let account = AccountId::parse(ID).expect("id");
    let _ = s
        .accounts()
        .set_sensitive(account, &version(STAMP), true, IdempotencyKey::new())
        .await;
    let r = sent(&server, "PUT", &format!("/accounts/{ID}/sensitive")).await;
    assert_eq!(body(&r)["expectedUpdatedAt"], STAMP);
    assert_eq!(body(&r)["isSensitive"], true);
    let group = TagGroupId::parse(ID2).expect("id");
    let fields = crate::resources::tags::TagGroupFields {
        name: "Regions".into(),
        description: None,
        is_single_select: false,
    };
    let _ = s
        .tags()
        .update_group(group, Some(stamp(STAMP)), &fields, IdempotencyKey::new())
        .await;
    let r = sent(&server, "PUT", &format!("/tags/groups/{ID2}")).await;
    assert_eq!(body(&r)["expectedUpdatedAt"], STAMP);
    assert_eq!(body(&r)["name"], "Regions");
}

#[tokio::test]
async fn version_deletes_send_version_in_the_query() {
    let (server, s) = mock(true).await;
    let _ = s.budgets().delete(ID, 7).await;
    let r = sent(&server, "DELETE", &format!("/budgets/{ID}")).await;
    assert_eq!(query(&r), "version=7");
}

#[tokio::test]
async fn merges_send_both_records_stamps() {
    let (server, s) = mock(true).await;
    let (a, b) = (
        ContactId::parse(ID).expect("id"),
        ContactId::parse(ID2).expect("id"),
    );
    let _ = s
        .contacts()
        .merge(
            (a, &version(STAMP)),
            (b, &version(OTHER)),
            None,
            IdempotencyKey::new(),
        )
        .await;
    let r = sent(&server, "POST", "/contacts/merge").await;
    assert_eq!(body(&r)["stamps"][ID], STAMP);
    assert_eq!(body(&r)["stamps"][ID2], OTHER);
    // No stamp read: no map (an empty one would pass LAN mode unchecked).
    let (t1, t2) = (
        TagId::parse(ID).expect("id"),
        TagId::parse(ID2).expect("id"),
    );
    let bare = RecordVersion::Fingerprint([0; 32]);
    let _ = s
        .tags()
        .merge((t1, &bare), (t2, &bare), None, IdempotencyKey::new())
        .await;
    let r = sent(&server, "POST", "/tags/merge").await;
    assert!(body(&r).get("stamps").is_none(), "{}", body(&r));
}

#[tokio::test]
async fn company_row_writes_carry_the_company_stamp_read_before_the_write() {
    let (server, s) = mock(true).await;
    let _ = s.set_inventory_enabled(true, IdempotencyKey::new()).await;
    let r = sent(&server, "PUT", "/company/settings").await;
    assert_eq!(body(&r)["expectedUpdatedAt"], STAMP);
    assert_eq!(body(&r)["inventoryEnabled"], true);
    let new = cl_core::Secret::new("long enough".to_string());
    let _ = s
        .set_closing_password(None, Some(&new), IdempotencyKey::new())
        .await;
    let r = sent(&server, "PUT", "/company/closing-password").await;
    assert_eq!(body(&r)["expectedUpdatedAt"], STAMP);
}

#[tokio::test]
async fn without_stale_all_nothing_new_is_sent() {
    let (server, s) = mock(false).await;
    let account = AccountId::parse(ID).expect("id");
    let _ = s
        .accounts()
        .deactivate(account, &version(STAMP), IdempotencyKey::new())
        .await;
    let r = sent(&server, "DELETE", &format!("/accounts/{ID}")).await;
    assert!(r.body.is_empty(), "no body without stale-all-v1");
    let _ = s.set_inventory_enabled(true, IdempotencyKey::new()).await;
    let r = sent(&server, "PUT", "/company/settings").await;
    assert!(body(&r).get("expectedUpdatedAt").is_none());
    let reads = server
        .received_requests()
        .await
        .expect("recorded")
        .into_iter()
        .filter(|r| r.method.as_str() == "GET" && r.url.path() == "/api/v1/company")
        .count();
    assert_eq!(reads, 0, "no company read without stale-all-v1");
}
