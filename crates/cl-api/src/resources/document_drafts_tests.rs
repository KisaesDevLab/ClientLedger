//! Wiremock tests of the document draft client (A03 WP-F).

use super::*;
use crate::resources::ap_doc::BillLineDraft;
use crate::resources::bank_postings::{CardChargeDraft, CheckDraft, SplitLine};
use crate::resources::posting::ClosedPeriodAuthorization;
use crate::resources::testkit::{company_session, json as reply, requests, session_with_caps};
use crate::{ConflictReason, KeySet};
use cl_core::ids::AccountId;
use cl_core::{OwnerFacing, Secret};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer};

const BASE: &str = "/api/v1/document-drafts";
const DRAFT: &str = "00000000-0000-4000-8000-0000000000d1";
const OTHER: &str = "00000000-0000-4000-8000-0000000000d2";
const FILE: &str = "00000000-0000-4000-8000-0000000000f1";
const VENDOR: &str = "00000000-0000-4000-8000-0000000000a1";
const TXN: &str = "00000000-0000-4000-8000-0000000000e1";
const EXPENSE: &str = "00000000-0000-4000-8000-0000000000c1";
const CARD: &str = "00000000-0000-4000-8000-0000000000c2";
/// More digits than JavaScript prints: it must come back byte for byte.
const TOKEN: &str = "2026-11-04T10:00:00.123456Z";

fn id() -> DocumentDraftId {
    DocumentDraftId::parse(DRAFT).expect("id")
}

fn draft_json(status: &str, extra: &str) -> String {
    format!(
        r#"{{"id":"{DRAFT}","companyId":"0b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01","attachmentId":"{FILE}",
        "contentSha256":"{}","kind":"bill","status":"{status}","reader":"text_pdf",
        "fields":{{"vendor":{{"value":"Canary Plumbing"}}}},"lines":[],"checks":[],"failureReason":null,
        "suggestedContactId":"{VENDOR}","docReference":"INV-CANARY-7","docTotal":"1480.0000",
        "docDate":"2026-11-03","convertedTransactionId":null,"convertedType":null,"convertedAt":null,
        "discardedAt":null,"createdBy":null,"createdAt":"{TOKEN}","updatedAt":"{TOKEN}"{extra}}}"#,
        "ab".repeat(32)
    )
}

fn one(status: &str) -> String {
    format!(r#"{{"draft":{}}}"#, draft_json(status, ""))
}

async fn served(server: &MockServer) -> Session {
    session_with_caps(
        server,
        &[
            Capabilities::DOCUMENT_DRAFTS,
            Capabilities::IDEMPOTENCY_DOCS,
        ],
    )
    .await
}

fn body_of(r: &wiremock::Request) -> Value {
    serde_json::from_slice(&r.body).expect("json body")
}

fn money(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}

fn acct(s: &str) -> AccountId {
    AccountId::parse(s).expect("account")
}

fn bill() -> BillDraft {
    BillDraft {
        vendor: ContactId::parse(VENDOR).expect("vendor"),
        date: CalendarDate::from_ymd(2026, 11, 3).expect("date"),
        vendor_invoice_number: Some(" INV-CANARY-7 ".into()),
        terms: None,
        due_date: None,
        memo: None,
        amount_due: money("1480.00"),
        lines: vec![BillLineDraft::expense(acct(EXPENSE), money("1480.00"))],
        received: true,
        // A draft key on the form is never sent: convert moves the file.
        draft_attachment: Some(crate::resources::attachments::DraftKey::new()),
    }
}

fn reading() -> DraftReading {
    let mut fields = Map::new();
    fields.insert(
        "vendor".into(),
        json!({"value": "Canary Plumbing", "confidence": 9000}),
    );
    DraftReading {
        kind: DraftKind::Bill,
        reader: DraftReader::TextPdf,
        fields,
        lines: vec![json!({"amount": {"value": "1480.00"}})],
        checks: Vec::new(),
        suggested_contact: Some(ContactId::parse(VENDOR).expect("vendor")),
        doc_reference: Some("  INV-CANARY-7 ".into()),
        doc_total: Some(money("1480.00")),
        doc_date: CalendarDate::from_ymd(2026, 11, 3).ok(),
    }
}

#[tokio::test]
async fn create_claims_the_file_with_a_key() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(BASE))
        .respond_with(reply(201, &one("new")))
        .mount(&server)
        .await;
    let s = served(&server).await;
    let key = IdempotencyKey::new();
    let d = s
        .document_drafts()
        .create(
            AttachmentId::parse(FILE).expect("file"),
            DraftKind::Unknown,
            key.clone(),
        )
        .await
        .expect("claim");
    assert_eq!(d.id, id());
    assert_eq!(d.status, DraftStatus::New);
    assert_eq!(d.reader, Some(DraftReader::TextPdf));
    assert_eq!(d.doc_total, Some(money("1480")));
    assert_eq!(d.updated_at, TOKEN, "the token is kept exactly");
    let posts = requests(&server, "POST").await;
    assert_eq!(posts.len(), 1);
    let b = body_of(&posts[0]);
    assert_eq!(b["attachmentId"], FILE);
    assert_eq!(b["kindHint"], "unknown");
    assert_eq!(
        posts[0]
            .headers
            .get("idempotency-key")
            .and_then(|v| v.to_str().ok()),
        Some(key.as_header().as_str()),
        "keyed under idempotency-docs-v1"
    );
    assert!(posts[0].headers.get("x-company-id").is_some());
}

#[tokio::test]
async fn a_claimed_file_names_its_draft() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(BASE))
        .respond_with(reply(
            409,
            &format!(
                r#"{{"error":{{"message":"Already waiting","code":"DOCUMENT_ALREADY_CLAIMED","details":{{"draftId":"{OTHER}"}}}}}}"#
            ),
        ))
        .mount(&server)
        .await;
    let e = served(&server)
        .await
        .document_drafts()
        .create(
            AttachmentId::parse(FILE).expect("file"),
            DraftKind::Bill,
            IdempotencyKey::new(),
        )
        .await
        .expect_err("claimed");
    assert_eq!(
        e,
        ApiError::DocumentDraft(DraftRefusal::AlreadyClaimed {
            draft: DocumentDraftId::parse(OTHER).ok()
        })
    );
    assert_eq!(requests(&server, "POST").await.len(), 1, "never retried");
    assert_eq!(e.owner_message().key, "error.api.document_claimed");
}

#[tokio::test]
async fn list_sends_the_filters() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(BASE))
        .respond_with(reply(
            200,
            &format!(
                r#"{{"drafts":[{}],"total":7}}"#,
                draft_json("ready", "").replace(
                    r#""fields":{"vendor":{"value":"Canary Plumbing"}},"lines":[],"checks":[],"#,
                    ""
                )
            ),
        ))
        .mount(&server)
        .await;
    let s = served(&server).await;
    let page = s
        .document_drafts()
        .list(&DraftListQuery {
            statuses: vec![DraftStatus::New, DraftStatus::Ready],
            kind: Some(DraftKind::CardCharge),
            limit: 20,
            offset: 40,
            summary: true,
        })
        .await
        .expect("list");
    assert_eq!(page.total, 7);
    assert_eq!(page.drafts[0].fields, None, "summary view");
    let gets = requests(&server, "GET").await;
    let get = gets.iter().find(|r| r.url.path() == BASE).expect("sent");
    assert_eq!(
        get.url.query(),
        Some("status=new%2Cready&kind=card_charge&limit=20&offset=40&view=summary")
    );
    for bad in [
        DraftListQuery {
            limit: 0,
            ..DraftListQuery::default()
        },
        DraftListQuery {
            limit: 201,
            ..DraftListQuery::default()
        },
        DraftListQuery {
            offset: 10_001,
            ..DraftListQuery::default()
        },
    ] {
        assert_eq!(
            s.document_drafts().list(&bad).await,
            Err(ApiError::Local(LocalValidation::InvalidValue))
        );
    }
}

#[tokio::test]
async fn a_reading_replaces_every_field_and_echoes_the_token() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path(format!("{BASE}/{DRAFT}")))
        .respond_with(reply(200, &one("ready")))
        .mount(&server)
        .await;
    let s = served(&server).await;
    s.document_drafts()
        .store_reading(id(), TOKEN, &reading())
        .await
        .expect("stored");
    let mut credit = reading();
    credit.doc_total = Some(money("-25.00"));
    credit.doc_reference = Some("x".repeat(101));
    credit.suggested_contact = None;
    s.document_drafts()
        .store_reading(id(), TOKEN, &credit)
        .await
        .expect("stored");
    let puts = requests(&server, "PUT").await;
    let b = body_of(&puts[0]);
    assert!(String::from_utf8_lossy(&puts[0].body).contains(TOKEN));
    assert_eq!(b["expectedUpdatedAt"], TOKEN);
    assert_eq!(b["status"], "ready");
    assert_eq!(b["kind"], "bill");
    assert_eq!(b["reader"], "text_pdf");
    assert_eq!(b["docReference"], "INV-CANARY-7", "trimmed");
    assert_eq!(b["docTotal"], "1480.0000", "a decimal string");
    assert_eq!(b["docDate"], "2026-11-03");
    assert_eq!(b["suggestedContactId"], VENDOR);
    assert_eq!(b["fields"]["vendor"]["confidence"], 9000);
    assert_eq!(b["lines"][0]["amount"]["value"], "1480.00");
    assert_eq!(b["failureReason"], Value::Null);
    let c = body_of(&puts[1]);
    assert_eq!(c["docTotal"], Value::Null, "a negative total is left out");
    assert_eq!(c["docReference"], Value::Null, "too long");
    assert_eq!(c["suggestedContactId"], Value::Null, "cleared");
}

#[tokio::test]
async fn readings_over_the_engine_bounds_are_refused_before_sending() {
    let server = MockServer::start().await;
    let s = served(&server).await;
    let mut float = reading();
    float.fields.insert("x".into(), json!(1.5));
    let mut big_int = reading();
    big_int.lines = vec![json!({"n": 1_000_000_001_u64})];
    let mut long = reading();
    long.fields.insert("t".into(), json!("é".repeat(2001)));
    let mut deep = reading();
    deep.checks = vec![json!({"a":{"b":{"c":{"d":{"e":{"f":1}}}}}})];
    let mut many = reading();
    many.lines = vec![json!({}); 501];
    let mut huge = reading();
    huge.fields
        .insert("big".into(), json!(vec!["x".repeat(2000); 40]));
    for r in [float, big_int, long, deep, many, huge] {
        assert_eq!(
            s.document_drafts().store_reading(id(), TOKEN, &r).await,
            Err(ApiError::Local(LocalValidation::InvalidValue)),
            "{r:?}"
        );
    }
    for bad in ["", "Unreadable", "too-long-reason", &"x".repeat(41)] {
        assert_eq!(
            s.document_drafts().store_failure(id(), TOKEN, bad).await,
            Err(ApiError::Local(LocalValidation::InvalidValue)),
            "{bad}"
        );
    }
    assert!(requests(&server, "PUT").await.is_empty(), "nothing sent");
}

#[tokio::test]
async fn failure_discard_stale_and_closed() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path(format!("{BASE}/{DRAFT}")))
        .respond_with(reply(
            409,
            r#"{"error":{"message":"changed","code":"STALE","details":{"updatedAt":"2026-11-04T10:00:01.000Z"}}}"#,
        ))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("{BASE}/{DRAFT}/discard")))
        .respond_with(reply(
            409,
            &format!(
                r#"{{"error":{{"message":"entered","code":"DOCUMENT_DRAFT_CLOSED","details":{{"status":"converted","transactionId":"{TXN}"}}}}}}"#
            ),
        ))
        .mount(&server)
        .await;
    let s = served(&server).await;
    let e = s
        .document_drafts()
        .store_failure(id(), TOKEN, "unreadable")
        .await
        .expect_err("stale");
    assert!(
        matches!(
            e,
            ApiError::Conflict {
                reason: ConflictReason::Stale,
                ..
            }
        ),
        "{e:?}"
    );
    let b = body_of(&requests(&server, "PUT").await[0]);
    assert_eq!(
        b,
        json!({"expectedUpdatedAt": TOKEN, "status": "failed", "failureReason": "unreadable"})
    );
    let e = s.document_drafts().discard(id()).await.expect_err("closed");
    assert_eq!(
        e,
        ApiError::DocumentDraft(DraftRefusal::Closed {
            transaction: TransactionId::parse(TXN).ok()
        })
    );
}

fn conversion(body: ConvertBody) -> DraftConversion {
    DraftConversion {
        body,
        expected_updated_at: TOKEN.into(),
        acknowledge: vec![TransactionId::parse(TXN).expect("txn")],
    }
}

#[tokio::test]
async fn convert_sends_the_forms_body_and_the_password_at_the_top() {
    let server = MockServer::start().await;
    let converted = format!(
        r#"{{"draft":{},"transaction":{{"id":"{TXN}","txnType":"bill"}},"draftMatches":[
            {{"source":"draft","id":"{OTHER}","reason":"reference","txnType":null,"kind":"bill",
              "date":"2026-11-03","total":"1480.0000","reference":"INV-CANARY-7"}}]}}"#,
        draft_json("converted", "").replace(
            r#""convertedTransactionId":null"#,
            &format!(r#""convertedTransactionId":"{TXN}""#)
        )
    );
    Mock::given(method("POST"))
        .and(path(format!("{BASE}/{DRAFT}/convert")))
        .respond_with(reply(201, &converted))
        .mount(&server)
        .await;
    let s = session_with_caps(
        &server,
        &[
            Capabilities::DOCUMENT_DRAFTS,
            Capabilities::IDEMPOTENCY_DOCS,
            Capabilities::CLOSING_PASSWORD,
            Capabilities::BILL_LINES,
        ],
    )
    .await;
    let opts = PostingOptions {
        closed_period: Some(ClosedPeriodAuthorization::with_password(Secret::new(
            "closing-canary",
        ))),
    };
    let key = IdempotencyKey::new();
    let done = s
        .document_drafts()
        .convert(
            id(),
            &conversion(ConvertBody::Bill(bill())),
            &opts,
            key.clone(),
        )
        .await
        .expect("converted");
    assert_eq!(done.transaction, TransactionId::parse(TXN).expect("txn"));
    assert_eq!(done.draft.status, DraftStatus::Converted);
    assert_eq!(done.draft_matches.len(), 1);
    assert_eq!(
        done.draft_matches[0].source,
        MatchSource::Draft(DocumentDraftId::parse(OTHER).expect("id"))
    );
    assert_eq!(done.draft_matches[0].reason, MatchReason::Reference);
    assert_eq!(done.draft_matches[0].total, Some(money("1480")));

    let posts = requests(&server, "POST").await;
    let b = body_of(&posts[0]);
    assert_eq!(b["kind"], "bill");
    assert_eq!(b["expectedUpdatedAt"], TOKEN);
    assert_eq!(b["acknowledgeDuplicateOf"], json!([TXN]));
    assert_eq!(b["closingPassword"], "closing-canary", "top level");
    assert!(b["body"].get("closingPassword").is_none());
    assert!(
        b["body"].get("draftAttachmentId").is_none(),
        "convert moves the file"
    );
    assert_eq!(b["body"]["contactId"], VENDOR);
    assert_eq!(b["body"]["vendorInvoiceNumber"], "INV-CANARY-7");
    assert_eq!(b["body"]["lines"][0]["amount"], "1480.0000");
    assert_eq!(
        posts[0]
            .headers
            .get("idempotency-key")
            .and_then(|v| v.to_str().ok()),
        Some(key.as_header().as_str())
    );
    assert_eq!(DOCUMENT_DRAFT_CONVERT.key_set(), Some(KeySet::DocsV1));
}

#[tokio::test]
async fn convert_builds_each_kind_with_the_typed_builders() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(reply(
            201,
            &format!(
                r#"{{"draft":{},"transaction":{{"id":"{TXN}"}}}}"#,
                draft_json("converted", "")
            ),
        ))
        .mount(&server)
        .await;
    let s = served(&server).await;
    let line = SplitLine {
        account: acct(EXPENSE),
        amount: money("12.50"),
        memo: None,
        contact: None,
        billable: false,
        tag: None,
    };
    let charge = CardChargeDraft {
        pay_from: acct(CARD),
        date: CalendarDate::from_ymd(2026, 11, 3).expect("date"),
        number: Some("R-1".into()),
        payee: ContactId::parse(VENDOR).ok(),
        memo: None,
        lines: vec![line.clone()],
    };
    let check = CheckDraft {
        bank: acct(CARD),
        date: CalendarDate::from_ymd(2026, 11, 3).expect("date"),
        payee: ContactId::parse(VENDOR).ok(),
        payee_name: "Canary Plumbing".into(),
        check_number: Some(1042),
        print_later: false,
        payee_address: None,
        memo: None,
        printed_memo: None,
        tag_ids: Vec::new(),
        draft_attachment: Some(crate::resources::attachments::DraftKey::new()),
        amount: money("12.50"),
        lines: vec![line],
    };
    let card = BankPosting::card_charge(charge).expect("charge");
    let chk = BankPosting::check(check).expect("check");
    for (body, kind) in [
        (ConvertBody::CardCharge(card.clone()), "card_charge"),
        (ConvertBody::CashExpense(card.clone()), "cash_expense"),
        (ConvertBody::Check(chk.clone()), "check"),
    ] {
        s.document_drafts()
            .convert(
                id(),
                &conversion(body),
                &PostingOptions::default(),
                IdempotencyKey::new(),
            )
            .await
            .unwrap_or_else(|e| panic!("{kind}: {e:?}"));
    }
    let posts = requests(&server, "POST").await;
    let charge = body_of(&posts[0]);
    assert_eq!(charge["kind"], "card_charge");
    assert_eq!(charge["body"]["payFromAccountId"], CARD);
    assert!(charge["body"].get("txnType").is_none(), "no txnType");
    assert_eq!(charge["body"]["lines"][0]["expenseAccountId"], EXPENSE);
    assert_eq!(body_of(&posts[1])["kind"], "cash_expense");
    let c = body_of(&posts[2]);
    assert_eq!(c["kind"], "check");
    assert_eq!(c["body"]["bankAccountId"], CARD);
    assert_eq!(c["body"]["checkNumber"], 1042);
    assert!(c["body"].get("draftAttachmentId").is_none());
    assert!(
        posts
            .iter()
            .all(|p| body_of(p).get("closingPassword").is_none())
    );

    // A posting of the wrong kind, or too many acknowledged: nothing sent.
    let before = posts.len();
    for bad in [
        conversion(ConvertBody::Check(card)),
        conversion(ConvertBody::CardCharge(chk)),
        DraftConversion {
            acknowledge: vec![TransactionId::parse(TXN).expect("txn"); 51],
            ..conversion(ConvertBody::Bill(bill()))
        },
    ] {
        assert_eq!(
            s.document_drafts()
                .convert(
                    id(),
                    &bad,
                    &PostingOptions::default(),
                    IdempotencyKey::new()
                )
                .await,
            Err(ApiError::Local(LocalValidation::InvalidValue))
        );
    }
    let mut short = bill();
    short.amount_due = money("1.00");
    assert_eq!(
        s.document_drafts()
            .convert(
                id(),
                &conversion(ConvertBody::Bill(short)),
                &PostingOptions::default(),
                IdempotencyKey::new()
            )
            .await,
        Err(ApiError::Local(LocalValidation::TotalMismatch)),
        "the bill's own checks run first"
    );
    assert_eq!(requests(&server, "POST").await.len(), before);
}

#[tokio::test]
async fn a_possible_duplicate_lists_its_matches() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(reply(
            409,
            &format!(
                r#"{{"error":{{"message":"looks entered","code":"POSSIBLE_DUPLICATE","details":{{
                "matches":[{{"source":"transaction","id":"{TXN}","reason":"amount_date","txnType":"bill",
                  "kind":null,"date":"2026-10-01","total":"1480.0000","reference":null}},
                  {{"source":"elsewhere","id":"{TXN}"}}],
                "draftMatches":[{{"source":"draft","id":"{OTHER}","reason":"reference"}}]}}}}}}"#
            ),
        ))
        .mount(&server)
        .await;
    let e = served(&server)
        .await
        .document_drafts()
        .convert(
            id(),
            &conversion(ConvertBody::Bill(bill())),
            &PostingOptions::default(),
            IdempotencyKey::new(),
        )
        .await
        .expect_err("duplicate");
    let ApiError::DocumentDraft(DraftRefusal::PossibleDuplicate {
        matches,
        draft_matches,
    }) = &e
    else {
        panic!("{e:?}");
    };
    assert_eq!(matches.len(), 1, "an unknown source is left out");
    assert_eq!(
        matches[0].source,
        MatchSource::Transaction(TransactionId::parse(TXN).expect("txn"))
    );
    assert_eq!(matches[0].reason, MatchReason::AmountDate);
    assert_eq!(matches[0].date, CalendarDate::from_ymd(2026, 10, 1).ok());
    assert_eq!(draft_matches.len(), 1);
    assert_eq!(e.owner_message().key, "error.api.possible_duplicate");
}

#[tokio::test]
async fn duplicates_and_get_read_the_engine_shapes() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("{BASE}/{DRAFT}/duplicates")))
        .respond_with(reply(
            200,
            &format!(
                r#"{{"matches":[{{"source":"transaction","id":"{TXN}","reason":"reference","txnType":"expense",
                "kind":null,"date":"2026-11-01","total":"10.1000","reference":"inv123"}}]}}"#
            ),
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{BASE}/{DRAFT}")))
        .respond_with(reply(200, &one("ready")))
        .mount(&server)
        .await;
    let s = served(&server).await;
    let m = s.document_drafts().duplicates(id()).await.expect("dupes");
    assert_eq!(m[0].total, Some(money("10.10")));
    assert_eq!(m[0].txn_type.as_deref(), Some("expense"));
    let d = s.document_drafts().get(id()).await.expect("get");
    assert_eq!(d.status, DraftStatus::Ready);
    assert!(d.status.is_live());
    assert!(!DraftStatus::Converted.is_live());
}

#[tokio::test]
async fn without_the_capability_nothing_is_sent() {
    let server = MockServer::start().await;
    for s in [
        session_with_caps(&server, &["idempotency-docs-v1"]).await,
        company_session(&server),
    ] {
        let d = s.document_drafts();
        let unsupported = Err::<(), _>(ApiError::Local(LocalValidation::Unsupported));
        assert_eq!(
            d.create(
                AttachmentId::parse(FILE).expect("file"),
                DraftKind::Bill,
                IdempotencyKey::new()
            )
            .await
            .map(|_| ()),
            unsupported
        );
        assert_eq!(
            d.list(&DraftListQuery::default()).await.map(|_| ()),
            unsupported
        );
        assert_eq!(d.get(id()).await.map(|_| ()), unsupported);
        assert_eq!(d.duplicates(id()).await.map(|_| ()), unsupported);
        assert_eq!(
            d.store_reading(id(), TOKEN, &reading()).await.map(|_| ()),
            unsupported
        );
        assert_eq!(d.discard(id()).await.map(|_| ()), unsupported);
        assert_eq!(
            d.convert(
                id(),
                &conversion(ConvertBody::Bill(bill())),
                &PostingOptions::default(),
                IdempotencyKey::new()
            )
            .await
            .map(|_| ()),
            unsupported
        );
    }
    let sent = server.received_requests().await.expect("recorded");
    assert!(sent.iter().all(|r| !r.url.path().starts_with(BASE)));
}

#[test]
fn a_live_drafts_file_in_use_is_a_conflict() {
    let body = format!(
        r#"{{"error":{{"message":"in use","code":"ATTACHMENT_IN_USE","details":{{"draftId":"{DRAFT}"}}}}}}"#
    );
    let mapped = crate::errors::map_failure(
        409,
        body.as_bytes(),
        None,
        crate::errors::RouteKind::Session,
        crate::errors::FailureShape::Plain,
    );
    let crate::errors::Mapped::Final(e) = mapped else {
        panic!("final");
    };
    assert!(
        matches!(
            e,
            ApiError::Conflict {
                reason: ConflictReason::InUse,
                code: Some(ref c),
                ..
            } if c.as_str() == ATTACHMENT_IN_USE
        ),
        "{e:?}"
    );
}

#[test]
fn debug_never_shows_document_data() {
    let d: DocumentDraft = serde_json::from_str(&draft_json("ready", "")).expect("draft");
    let m = matches_from(Some(
        &json!([{"source":"transaction","id":TXN,"reason":"reference",
        "total":"1480.0000","reference":"INV-CANARY-7"}]),
    ));
    let text = format!("{d:?} {:?} {m:?}", reading());
    for secret in ["Canary", "CANARY", "1480"] {
        assert!(!text.contains(secret), "{text}");
    }
    assert!(text.contains(DRAFT));
}
