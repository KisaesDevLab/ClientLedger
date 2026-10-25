//! Bills: payee-panel readers (P13) and Enter Bill (P17 WP2) against a
//! stub engine. Money is checked as exact strings; capability gates must
//! send nothing; lost replies resolve by the D2 probe.

use cl_core::ids::{AccountId, ContactId, ItemId, TagId, TransactionId};
use cl_core::{CalendarDate, Money, Quantity, Secret};
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;
use crate::errors::{ConflictReason, LocalValidation};
use crate::resources::ap_doc::{BillLineDraft, BillTerms, Percent};
use crate::resources::posting::ClosedPeriodAuthorization;
use crate::resources::testkit::{company_session, json, query, requests, session_with_caps};
use crate::resources::vendor_credits::VendorCreditDraft;
use crate::resources::versioning::{EditableFields, RecordVersion};

const VENDOR: &str = "3a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const ID: &str = "4b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const A: &str = "2a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const B: &str = "2b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const AP: &str = "2c0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const TAG: &str = "6a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const ITEM: &str = "6b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const CUSTOMER: &str = "3b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";

fn vendor() -> ContactId {
    ContactId::parse(VENDOR).expect("id")
}

fn id() -> TransactionId {
    TransactionId::parse(ID).expect("id")
}

fn m(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}

fn acct(s: &str) -> AccountId {
    AccountId::parse(s).expect("account")
}

fn day() -> CalendarDate {
    CalendarDate::from_ymd(2026, 11, 3).expect("date")
}

/// The mockup's bill: Midwest, INV-55871, 1,330 + 150 = 1,480 (synthetic).
fn draft() -> BillDraft {
    BillDraft {
        vendor: vendor(),
        date: day(),
        vendor_invoice_number: Some(" INV-55871 ".into()),
        terms: Some(BillTerms::code("net_15")),
        due_date: None,
        memo: None,
        amount_due: m("1480.00"),
        lines: vec![
            BillLineDraft::expense(acct(A), m("1330.00")),
            BillLineDraft::expense(acct(B), m("150.00")),
        ],
        received: true,
        draft_attachment: None,
    }
}

/// A stored bill as `GET /bills/{id}` sends it: the two expense lines and
/// the engine's A/P line (which carries the vendor's default tag).
fn record(reference: &str, extra: &str) -> String {
    format!(
        r#"{{"id":"{ID}","txnType":"bill","txnNumber":"BILL-00001","txnDate":"2026-11-03","status":"posted",
        "basis":"both","contactId":"{VENDOR}","contactName":"Canary Rental","memo":null,"total":"1480.0000",
        "dueDate":"2026-11-18","paymentTerms":"net_15","termsDays":30,"vendorInvoiceNumber":"{reference}",
        "balanceDue":"1480.0000","amountPaid":"0.0000","creditsApplied":"0.0000","billStatus":"unpaid",
        "createdAt":"2199-01-01T00:00:00.000Z","updatedAt":"2026-11-03T10:00:00.120Z"{extra},"lines":[
        {{"id":"8a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01","accountId":"{A}","debit":"1330.0000","credit":"0.0000",
          "description":null,"lineOrder":0,"tagId":null,"contactId":null,"itemId":null,"quantity":null,"unitPrice":null}},
        {{"id":"8b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01","accountId":"{B}","debit":"150.0000","credit":"0.0000",
          "description":null,"lineOrder":1,"tagId":null,"contactId":null,"itemId":null,"quantity":null,"unitPrice":null}},
        {{"id":"8c0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01","accountId":"{AP}","debit":"0.0000","credit":"1480.0000",
          "description":null,"lineOrder":2,"tagId":"{TAG}","contactId":null,"itemId":null,"quantity":null,"unitPrice":null}}]}}"#
    )
}

fn bill_body(reference: &str) -> String {
    format!(r#"{{"bill":{}}}"#, record(reference, ""))
}

async fn body_of(server: &MockServer, m: &str, n: usize) -> Value {
    serde_json::from_slice(&requests(server, m).await[n].body).expect("json")
}

#[test]
fn open_only_asks_for_the_states_that_still_owe() {
    // P17 WP8 (review S4): open bills from `/bills` with a status filter,
    // received or not, never from `/bills/payable`.
    let q = BillListQuery {
        vendor: Some(vendor()),
        open_only: true,
        sort: Some((BillSort::DueDate, SortDir::Asc)),
        ..BillListQuery::default()
    };
    let pairs = q.pairs();
    assert!(pairs.contains(&("billStatus".into(), "unpaid,partial,overdue".into())));
    assert!(pairs.contains(&("sortBy".into(), "dueDate".into())));
    assert!(
        !BillListQuery::default()
            .pairs()
            .iter()
            .any(|(k, _)| k == "billStatus")
    );
}

#[tokio::test]
async fn summary_and_payable_read_exact_strings() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/bills/vendor-summary"))
        .respond_with(json(
            200,
            r#"{"summary":{"unpaidCount":1,"totalOwed":"1286.4000","overdueCount":0,
                "totalOverdue":"0","availableCreditsCount":0,"totalAvailableCredits":"0"}}"#,
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/bills/payable"))
        .respond_with(json(
            200,
            &format!(
                r#"{{"bills":[{{"id":"{ID}","txnNumber":null,
                "contactId":"{VENDOR}","contactName":"Canary Supply","txnDate":"2026-10-06",
                "dueDate":"2026-11-05","vendorInvoiceNumber":"F-77","total":"1286.4000",
                "amountPaid":"0.0000","creditsApplied":"0.0000","balanceDue":"1286.4000",
                "billStatus":"unpaid","paymentTerms":"net_30","memo":"canary memo","daysOverdue":0}}],
                "credits":[]}}"#
            ),
        ))
        .mount(&server)
        .await;
    let s = company_session(&server);
    let sum = s.bills().vendor_summary(vendor()).await.expect("summary");
    assert_eq!(sum.unpaid_count, 1);
    assert_eq!(sum.total_owed.to_wire(), "1286.4000");
    assert_eq!(sum.total_overdue, Money::ZERO);
    let shown = format!("{sum:?}");
    assert!(!shown.contains("1286"), "{shown}");

    let bills = s.bills().payable(vendor()).await.expect("payable");
    assert_eq!(bills.len(), 1);
    assert_eq!(bills[0].amount_due.to_wire(), "1286.4000");
    assert_eq!(bills[0].due, CalendarDate::from_ymd(2026, 11, 5).ok());
    assert_eq!(bills[0].vendor_invoice_number.as_deref(), Some("F-77"));
    let shown = format!("{:?}", bills[0]);
    assert!(
        !shown.contains("1286") && !shown.contains("canary"),
        "{shown}"
    );
    for r in requests(&server, "GET").await {
        assert_eq!(query(&r), format!("contactId={VENDOR}"));
    }
}

#[tokio::test]
async fn a_number_where_money_belongs_is_refused() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/bills/vendor-summary"))
        .respond_with(json(
            200,
            r#"{"summary":{"unpaidCount":1,"totalOwed":1286.4,"overdueCount":0,"totalOverdue":"0"}}"#,
        ))
        .mount(&server)
        .await;
    let s = company_session(&server);
    let e = s.bills().vendor_summary(vendor()).await.expect_err("float");
    assert_eq!(
        e,
        ApiError::Decode {
            what: "vendor summary"
        }
    );
}

#[tokio::test]
async fn create_sends_exact_strings_and_null_tags_and_reads_the_record() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/bills"))
        .respond_with(json(201, &bill_body("INV-55871")))
        .mount(&server)
        .await;
    let s = company_session(&server);
    let saved = s
        .bills()
        .create(&draft(), &PostingOptions::default(), IdempotencyKey::new())
        .await
        .expect("create");
    let b = body_of(&server, "POST", 0).await;
    assert_eq!(
        b,
        json!({
            "contactId": VENDOR, "txnDate": "2026-11-03", "vendorInvoiceNumber": "INV-55871",
            "paymentTerms": "net_15",
            "lines": [
                {"accountId": A, "amount": "1330.0000", "tagId": null},
                {"accountId": B, "amount": "150.0000", "tagId": null},
            ],
        }),
        "no memo, no due date, no PR-A fields on the pin"
    );
    let r = &saved.record;
    assert_eq!(r.kind(), ApKind::Bill);
    assert_eq!(r.lines.len(), 2, "the A/P line is not a document line");
    assert_eq!(r.lines[1].amount.to_wire(), "150.0000");
    assert_eq!(
        r.txn.total.map(Money::to_wire).as_deref(),
        Some("1480.0000")
    );
    assert_eq!(
        r.balance_due.map(Money::to_wire).as_deref(),
        Some("1480.0000")
    );
    assert_eq!(r.bill_status, Some(BillStatus::Unpaid));
    assert!(
        r.bill_received,
        "true from engines without bill-received-v1"
    );
    assert!(!r.has_payments());
    // What the form saved is what the engine stored.
    assert_eq!(r.fingerprint(), draft().fingerprint());
    assert_eq!(
        saved.version,
        RecordVersion::Fingerprint(draft().fingerprint())
    );
    let shown = format!("{r:?}");
    for canary in ["Canary", "1480", "55871"] {
        assert!(!shown.contains(canary), "{shown}");
    }
    assert!(!format!("{:?}", draft()).contains("55871"));
}

#[test]
fn the_fingerprint_follows_what_the_form_writes_only() {
    let base: BillRecord = serde_json::from_str(&record("INV-55871", "")).expect("record");
    let fp = base.fingerprint();
    // Engine-maintained fields do not move it.
    let paid: BillRecord = serde_json::from_str(
        &record("INV-55871", "")
            .replace(r#""balanceDue":"1480.0000""#, r#""balanceDue":"0.0000""#)
            .replace(r#""dueDate":"2026-11-18""#, r#""dueDate":"2026-11-17""#)
            .replace(r#""termsDays":30"#, r#""termsDays":45"#)
            .replace(
                r#""updatedAt":"2026-11-03T10:00:00.120Z""#,
                r#""updatedAt":"2026-11-04T10:00:00.000Z""#,
            ),
    )
    .expect("record");
    assert_eq!(paid.fingerprint(), fp);
    // The reference, a line amount and a line tag do.
    let other: BillRecord = serde_json::from_str(&record("INV-55872", "")).expect("r");
    assert_ne!(other.fingerprint(), fp);
    let mut d = draft();
    d.lines[0].tag = Some(TagId::parse(TAG).expect("tag"));
    assert_ne!(d.fingerprint(), fp);
    // A blank line description takes the memo, as the engine stores it.
    let mut d = draft();
    d.memo = Some("Excavator".into());
    let with_memo: BillRecord = serde_json::from_str(
        &record("INV-55871", "")
            .replace(r#""memo":null"#, r#""memo":"Excavator""#)
            .replace(
                r#""debit":"1330.0000","credit":"0.0000",
          "description":null"#,
                r#""debit":"1330.0000","credit":"0.0000",
          "description":"Excavator""#,
            )
            .replace(
                r#""debit":"150.0000","credit":"0.0000",
          "description":null"#,
                r#""debit":"150.0000","credit":"0.0000",
          "description":"Excavator""#,
            ),
    )
    .expect("record");
    assert_eq!(with_memo.fingerprint(), d.fingerprint());
    // A record that is not a bill or credit is refused.
    let je = record("x", "").replace(r#""txnType":"bill""#, r#""txnType":"journal_entry""#);
    assert!(serde_json::from_str::<BillRecord>(&je).is_err());
    // Money as a JSON number is refused.
    let num = record("x", "").replace(r#""total":"1480.0000""#, r#""total":1480"#);
    assert!(serde_json::from_str::<BillRecord>(&num).is_err());
    assert!(BillRecord::EDITABLE.contains(&"lines.amount"));
}

fn item_line() -> BillLineDraft {
    BillLineDraft {
        item: Some(ItemId::parse(ITEM).expect("item")),
        quantity: Some(Quantity::parse_wire("3").expect("q")),
        unit_price: Some(m("0.3333")),
        amount: m("1.00"),
        ..BillLineDraft::expense(acct(A), m("1.00"))
    }
}

#[tokio::test]
async fn pr_a_fields_without_their_capability_send_nothing() {
    let server = MockServer::start().await;
    let s = company_session(&server);
    let gated = |e: ApiError| assert_eq!(e, ApiError::Local(LocalValidation::Unsupported));
    let send = |d: BillDraft, opts: PostingOptions| {
        let s = s.clone();
        async move { s.bills().create(&d, &opts, IdempotencyKey::new()).await }
    };
    let none = PostingOptions::default;

    let mut d = draft();
    d.lines = vec![item_line()];
    d.amount_due = m("1.00");
    gated(send(d, none()).await.expect_err("item"));

    let mut d = draft();
    d.lines[0].customer = Some(ContactId::parse(CUSTOMER).expect("c"));
    gated(send(d, none()).await.expect_err("customer"));

    let mut d = draft();
    d.received = false;
    gated(send(d, none()).await.expect_err("received"));

    let mut d = draft();
    d.terms = Some(BillTerms {
        code: "net_30".into(),
        days: None,
        discount_percent: Some(Percent::parse("2").expect("p")),
        discount_days: Some(10),
    });
    gated(send(d, none()).await.expect_err("discount"));

    let mut d = draft();
    d.terms = Some(BillTerms::code("2_10_net_30"));
    gated(send(d, none()).await.expect_err("new code"));

    let mut d = draft();
    d.draft_attachment = Some(DraftKey::new());
    gated(send(d, none()).await.expect_err("draft"));

    let password = PostingOptions {
        closed_period: Some(ClosedPeriodAuthorization::with_password(Secret::new("pw"))),
    };
    gated(send(draft(), password).await.expect_err("closing password"));

    let credit = VendorCreditDraft {
        vendor: vendor(),
        date: day(),
        vendor_invoice_number: None,
        memo: None,
        amount: m("10.00"),
        lines: vec![BillLineDraft {
            tag: Some(TagId::parse(TAG).expect("tag")),
            ..BillLineDraft::expense(acct(A), m("10.00"))
        }],
        draft_attachment: None,
    };
    gated(
        s.vendor_credits()
            .create(&credit, &none(), IdempotencyKey::new())
            .await
            .expect_err("credit tag"),
    );
    gated(
        s.bills()
            .terms_dates(&TermsDatesQuery {
                date: day(),
                terms: None,
                vendor: None,
            })
            .await
            .expect_err("terms dates"),
    );
    gated(s.bills().payment_terms().await.expect_err("payment terms"));
    gated(
        s.bills()
            .delete(
                id(),
                None,
                &ApWriteOptions::default(),
                IdempotencyKey::new(),
            )
            .await
            .expect_err("delete"),
    );
    gated(
        s.bills()
            .void(
                id(),
                "x",
                &ApWriteOptions {
                    posting: PostingOptions {
                        closed_period: Some(ClosedPeriodAuthorization::with_password(Secret::new(
                            "pw",
                        ))),
                    },
                    expected_updated_at: None,
                },
                IdempotencyKey::new(),
            )
            .await
            .expect_err("void password"),
    );
    assert!(
        server.received_requests().await.expect("r").is_empty(),
        "nothing may be sent"
    );
}

#[tokio::test]
async fn local_rules_refuse_before_sending() {
    let server = MockServer::start().await;
    let s = session_with_caps(
        &server,
        &[Capabilities::BILL_LINES, Capabilities::BILL_TERMS],
    )
    .await;
    let refused = |d: BillDraft| {
        let s = s.clone();
        async move {
            match s
                .bills()
                .create(&d, &PostingOptions::default(), IdempotencyKey::new())
                .await
            {
                Err(ApiError::Local(v)) => v,
                other => panic!("not refused locally: {other:?}"),
            }
        }
    };
    let mut d = draft();
    d.amount_due = m("1480.01");
    assert_eq!(refused(d).await, LocalValidation::TotalMismatch);
    let mut d = draft();
    d.lines.clear();
    assert_eq!(refused(d).await, LocalValidation::TooFewLines);
    let mut d = draft();
    d.lines[1].amount = Money::ZERO;
    assert_eq!(refused(d).await, LocalValidation::NonPositiveAmount);
    let mut d = draft();
    d.vendor_invoice_number = Some("x".repeat(101));
    assert_eq!(refused(d).await, LocalValidation::InvalidValue);
    // Qty × cost: 3 × 0.3333 = 0.9999 → 1.00 (half away from zero).
    let mut d = draft();
    d.lines = vec![BillLineDraft {
        amount: m("0.99"),
        ..item_line()
    }];
    d.amount_due = m("0.99");
    assert_eq!(refused(d).await, LocalValidation::LineAmountMismatch);
    let mut d = draft();
    d.lines = vec![BillLineDraft {
        quantity: None,
        ..item_line()
    }];
    d.amount_due = m("1.00");
    assert_eq!(
        refused(d).await,
        LocalValidation::LineAmountMismatch,
        "cost without quantity"
    );
    let mut d = draft();
    d.lines[0].billable = true;
    assert_eq!(
        refused(d).await,
        LocalValidation::InvalidValue,
        "billable needs a customer"
    );
    let mut d = draft();
    d.terms = Some(BillTerms::code(TERMS_CUSTOM_CODE));
    assert_eq!(
        refused(d).await,
        LocalValidation::InvalidValue,
        "custom needs days"
    );
    let mut d = draft();
    d.terms = Some(BillTerms {
        code: "custom".into(),
        days: Some(10),
        discount_percent: Some(Percent::parse("2").expect("p")),
        discount_days: Some(10),
    });
    assert_eq!(
        refused(d).await,
        LocalValidation::InvalidValue,
        "discount days < net days"
    );
    assert!(requests(&server, "POST").await.is_empty());
}

const TERMS_CUSTOM_CODE: &str = "custom";

#[tokio::test]
async fn with_pr_a_the_new_fields_go_out() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/bills"))
        .respond_with(json(201, &bill_body("INV-55871")))
        .mount(&server)
        .await;
    let s = session_with_caps(
        &server,
        &[
            Capabilities::BILL_LINES,
            Capabilities::BILL_TERMS,
            Capabilities::BILL_RECEIVED,
            Capabilities::AP_CLOSING,
            Capabilities::CLOSING_PASSWORD,
        ],
    )
    .await;
    let draft_key = DraftKey::new();
    let d = BillDraft {
        terms: Some(BillTerms {
            code: "2_10_net_30".into(),
            days: None,
            discount_percent: Some(Percent::parse("2").expect("p")),
            discount_days: Some(10),
        }),
        due_date: CalendarDate::from_ymd(2026, 12, 1).ok(),
        received: false,
        draft_attachment: Some(draft_key),
        lines: vec![BillLineDraft {
            customer: Some(ContactId::parse(CUSTOMER).expect("c")),
            billable: true,
            ..item_line()
        }],
        amount_due: m("1.00"),
        memo: Some("Install".into()),
        ..draft()
    };
    let opts = PostingOptions {
        closed_period: Some(ClosedPeriodAuthorization::with_password(Secret::new("pw"))),
    };
    // The stub answers with another record: only the request matters.
    s.bills()
        .create(&d, &opts, IdempotencyKey::new())
        .await
        .expect("create");
    let b = body_of(&server, "POST", 0).await;
    assert_eq!(
        b["lines"][0],
        json!({"accountId": A, "amount": "1.0000", "tagId": null, "itemId": ITEM,
               "quantity": "3.0000", "unitPrice": "0.3333", "contactId": CUSTOMER, "isBillable": true})
    );
    assert_eq!(b["paymentTerms"], "2_10_net_30");
    assert_eq!(b["discountPercent"], "2");
    assert_eq!(b["discountDays"], 10);
    assert_eq!(b["dueDate"], "2026-12-01");
    assert_eq!(b["billReceived"], false);
    assert_eq!(b["draftAttachmentId"], draft_key.to_string());
    assert_eq!(b["memo"], "Install");
    assert_eq!(b["closingPassword"], "pw");
}

#[tokio::test]
async fn update_is_fingerprint_guarded_and_reads_back_on_the_pin() {
    let server = MockServer::start().await;
    // The pre-read and the read-back.
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/bills/{ID}")))
        .respond_with(json(200, &bill_body("INV-55871")))
        .mount(&server)
        .await;
    // The pin answers with the row before its second write (review B3).
    Mock::given(method("PUT"))
        .and(path(format!("/api/v1/bills/{ID}")))
        .respond_with(json(200, &bill_body("OLD-REF")))
        .mount(&server)
        .await;
    let s = company_session(&server);
    let read = s.bills().get(id()).await.expect("get");
    let mut d = draft();
    d.memo = None;
    let saved = s
        .bills()
        .update(
            id(),
            &read,
            &d,
            &PostingOptions::default(),
            IdempotencyKey::new(),
        )
        .await
        .expect("update");
    assert_eq!(
        saved.record.vendor_invoice_number.as_deref(),
        Some("INV-55871"),
        "the read-back, not the stale answer"
    );
    let b = body_of(&server, "PUT", 0).await;
    assert_eq!(b["memo"], "", "an edit names the memo");
    assert_eq!(b["vendorInvoiceNumber"], "INV-55871");
    assert!(b.get("expectedUpdatedAt").is_none(), "only with ap-edit-v1");
    assert!(b.get("draftAttachmentId").is_none());

    // Someone changed it since: nothing is sent.
    let stale = Versioned {
        record: read.record.clone(),
        version: RecordVersion::Fingerprint([7; 32]),
    };
    let e = s
        .bills()
        .update(
            id(),
            &stale,
            &d,
            &PostingOptions::default(),
            IdempotencyKey::new(),
        )
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
    assert_eq!(requests(&server, "PUT").await.len(), 1);
}

#[tokio::test]
async fn with_ap_edit_the_stale_token_goes_on_edit_void_and_delete() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/bills/{ID}")))
        .respond_with(json(200, &bill_body("INV-55871")))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path(format!("/api/v1/bills/{ID}")))
        .respond_with(json(
            409,
            r#"{"error":{"code":"STALE","message":"changed","details":{"updatedAt":"2026-11-04T00:00:00.000Z"}}}"#,
        ))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("/api/v1/bills/{ID}/void")))
        .respond_with(json(200, &bill_body("INV-55871")))
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path(format!("/api/v1/transactions/{ID}")))
        .respond_with(json(
            409,
            r#"{"error":{"code":"BILL_HAS_PAYMENTS","message":"paid"}}"#,
        ))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &[Capabilities::AP_EDIT, Capabilities::TXN_DELETE]).await;
    let read = s.bills().get(id()).await.expect("get");
    let e = s
        .bills()
        .update(
            id(),
            &read,
            &draft(),
            &PostingOptions::default(),
            IdempotencyKey::new(),
        )
        .await
        .expect_err("stale");
    assert_eq!(
        crate::ap_refusal(&e),
        Some(crate::ApRefusal::Stale),
        "{e:?}"
    );
    let b = body_of(&server, "PUT", 0).await;
    assert_eq!(b["expectedUpdatedAt"], "2026-11-03T10:00:00.120Z");

    let opts = ApWriteOptions {
        posting: PostingOptions::default(),
        expected_updated_at: read.record.txn.updated_at,
    };
    s.bills()
        .void(id(), " duplicate ", &opts, IdempotencyKey::new())
        .await
        .expect("void");
    let v = body_of(&server, "POST", 0).await;
    assert_eq!(
        v,
        json!({"reason": "duplicate", "expectedUpdatedAt": "2026-11-03T10:00:00.120Z"})
    );
    let e = s
        .bills()
        .delete(id(), Some("typed twice"), &opts, IdempotencyKey::new())
        .await
        .expect_err("paid");
    assert_eq!(
        crate::ap_refusal(&e),
        Some(crate::ApRefusal::BillHasPayments)
    );
    let del = body_of(&server, "DELETE", 0).await;
    assert_eq!(
        del,
        json!({"reason": "typed twice", "expectedUpdatedAt": "2026-11-03T10:00:00.120Z"})
    );
}

/// D2: a create whose reply was lost resolves to the one new bill of this
/// vendor, date, reference and total; a different total is never claimed.
#[tokio::test]
async fn a_lost_create_resolves_by_vendor_date_reference_and_total() {
    let row = |total: &str| {
        format!(
            r#"{{"data":[{{"id":"{ID}","txnNumber":"BILL-00001","txnDate":"2026-11-03","dueDate":"2026-11-18",
            "contactId":"{VENDOR}","vendorInvoiceNumber":"INV-55871","total":"{total}","balanceDue":"{total}",
            "billStatus":"unpaid","status":"posted","createdAt":"2199-01-01T00:00:00.000Z"}}],"total":1}}"#
        )
    };
    for (total, committed) in [("1480.0000", true), ("1480.0100", false)] {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/ledger-version"))
            .respond_with(json(200, r#"{"stamp":5}"#))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v1/bills"))
            .respond_with(ResponseTemplate::new(502))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v1/bills"))
            .respond_with(json(200, &row(total)))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/api/v1/bills/{ID}")))
            .respond_with(json(200, &bill_body("INV-55871")))
            .mount(&server)
            .await;
        let s = company_session(&server);
        let got = s
            .bills()
            .create(&draft(), &PostingOptions::default(), IdempotencyKey::new())
            .await;
        if committed {
            let saved = got.expect("reconciled");
            assert_eq!(saved.record.txn.id, id());
            let list = requests(&server, "GET")
                .await
                .into_iter()
                .find(|r| r.url.path() == "/api/v1/bills")
                .expect("searched");
            assert!(
                query(&list).starts_with(&format!(
                    "contactId={VENDOR}&startDate=2026-11-03&endDate=2026-11-03"
                )),
                "{}",
                query(&list)
            );
        } else {
            assert!(
                matches!(got, Err(ApiError::OutcomeUnknown { .. })),
                "{got:?}"
            );
        }
        assert_eq!(requests(&server, "POST").await.len(), 1, "never re-sent");
    }
}

#[tokio::test]
async fn list_terms_and_attach_draft() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/bills"))
        .respond_with(json(
            200,
            &format!(
                r#"{{"data":[{{"id":"{ID}","txnNumber":"BILL-00001","txnDate":"2026-11-03","dueDate":null,
                "contactId":"{VENDOR}","contactName":"Canary","vendorInvoiceNumber":"INV-55871","paymentTerms":null,
                "total":"1480.0000","amountPaid":"0.0000","creditsApplied":"0.0000","balanceDue":"1480.0000",
                "billStatus":"unpaid","memo":"canary","status":"void","createdAt":"2026-11-03T10:00:00.000Z","daysOverdue":0}}],
                "total":1}}"#
            ),
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/bills/terms-dates"))
        .respond_with(json(
            200,
            r#"{"dueDate":"2026-12-03","discountDate":"2026-11-13","paymentTerms":"2_10_net_30",
            "termsDays":30,"discountPercent":"2.0000","discountDays":10,"source":"vendor"}"#,
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/payment-terms"))
        .respond_with(json(
            200,
            r#"{"terms":[{"code":"net_30","netDays":30},{"code":"2_10_net_30","netDays":30,
            "discountPercent":"2","discountDays":10},{"code":"custom","netDays":null}]}"#,
        ))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("/api/v1/bills/{ID}/attach-draft")))
        .respond_with(json(200, r#"{"reassigned":2}"#))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &[Capabilities::BILL_TERMS]).await;
    let q = BillListQuery {
        vendor: Some(vendor()),
        search: Some("INV-55871".into()),
        start: CalendarDate::from_ymd(2025, 5, 3).ok(),
        end: CalendarDate::from_ymd(2028, 5, 3).ok(),
        sort: Some((BillSort::Date, SortDir::Desc)),
        open_only: false,
    };
    let page = s
        .bills()
        .list(&q, PageRequest::new(50, 0).expect("page"))
        .await
        .expect("list");
    assert_eq!(page.data[0].status, TxnStatus::Void);
    assert_eq!(
        page.data[0].vendor_invoice_number.as_deref(),
        Some("INV-55871")
    );
    assert_eq!(
        page.data[0].balance_due.map(Money::to_wire).as_deref(),
        Some("1480.0000")
    );
    assert!(!format!("{:?}", page.data[0]).contains("55871"));
    let capped = s.bills().list_capped(&q, 50, 20).await.expect("capped");
    assert!(capped.complete);
    let gets = requests(&server, "GET").await;
    let list_q = gets
        .iter()
        .find(|r| r.url.path() == "/api/v1/bills")
        .map(query)
        .expect("list");
    assert_eq!(
        list_q,
        format!(
            "contactId={VENDOR}&search=INV-55871&startDate=2025-05-03&endDate=2028-05-03&sortBy=date&sortDir=desc&limit=50&offset=0"
        )
    );

    let dates = s
        .bills()
        .terms_dates(&TermsDatesQuery {
            date: day(),
            terms: None,
            vendor: Some(vendor()),
        })
        .await
        .expect("dates");
    assert_eq!(dates.due_date, CalendarDate::from_ymd(2026, 12, 3).ok());
    assert_eq!(
        dates.discount_percent.map(Percent::to_wire).as_deref(),
        Some("2")
    );
    assert_eq!(dates.source, TermsSource::Vendor);
    let terms = s.bills().payment_terms().await.expect("terms");
    assert_eq!(terms.len(), 3);
    assert_eq!(terms[1].discount_days, Some(10));
    let moved = s
        .bills()
        .attach_draft(id(), DraftKey::new(), IdempotencyKey::new())
        .await
        .expect("attach");
    assert_eq!(moved, 2);
}

#[tokio::test]
async fn vendor_credits_edit_needs_ap_edit_and_lists_read_exact() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/vendor-credits/available/{VENDOR}")))
        .respond_with(json(
            200,
            &format!(
                r#"{{"credits":[{{"id":"{ID}","txnNumber":"VC-00001","txnDate":"2026-11-03",
                "total":"25.0000","balanceDue":"25.0000","memo":"canary"}}]}}"#
            ),
        ))
        .mount(&server)
        .await;
    let s = company_session(&server);
    let credit = VendorCreditDraft {
        vendor: vendor(),
        date: day(),
        vendor_invoice_number: Some("CM-1".into()),
        memo: None,
        amount: m("25.00"),
        lines: vec![BillLineDraft::expense(acct(A), m("25.00"))],
        draft_attachment: None,
    };
    let read: BillRecord = serde_json::from_str(
        &record("CM-1", "").replace(r#""txnType":"bill""#, r#""txnType":"vendor_credit""#),
    )
    .expect("credit record");
    assert_eq!(read.kind(), ApKind::VendorCredit);
    assert_eq!(read.lines.len(), 1, "the credit side is the A/P line here");
    let versioned = Versioned {
        version: RecordVersion::Fingerprint(read.fingerprint()),
        record: read,
    };
    let e = s
        .vendor_credits()
        .update(
            id(),
            &versioned,
            &credit,
            &PostingOptions::default(),
            IdempotencyKey::new(),
        )
        .await
        .expect_err("gated");
    assert_eq!(e, ApiError::Local(LocalValidation::Unsupported));
    let avail = s
        .vendor_credits()
        .available(vendor())
        .await
        .expect("available");
    assert_eq!(avail[0].balance_due.to_wire(), "25.0000");
    assert!(!format!("{:?}", avail[0]).contains("25"));
    assert_eq!(requests(&server, "PUT").await.len(), 0);
}

async fn lost_server() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/ledger-version"))
        .respond_with(json(200, r#"{"stamp":5}"#))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(502))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(502))
        .mount(&server)
        .await;
    server
}

/// D2 for the other saves of the form: a vendor credit create (its rows
/// carry no reference before `ap-edit-v1`, so the reference is left out of
/// the match), a bill edit (the record now has the requested fields) and a
/// void (the transaction reads back void). Each lost reply resolves to the
/// save, which is never sent twice.
#[tokio::test]
async fn lost_credit_create_bill_edit_and_void_are_reconciled() {
    // Credit create.
    let server = lost_server().await;
    let credit_record =
        record("CM-1", "").replace(r#""txnType":"bill""#, r#""txnType":"vendor_credit""#);
    Mock::given(method("GET"))
        .and(path("/api/v1/vendor-credits"))
        .respond_with(json(
            200,
            &format!(
                r#"{{"data":[{{"id":"{ID}","txnNumber":"VC-00001","txnDate":"2026-11-03","contactId":"{VENDOR}",
                "total":"1480.0000","balanceDue":"1480.0000","memo":null,"status":"posted",
                "createdAt":"2199-01-01T00:00:00.000Z"}}],"total":1}}"#
            ),
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/vendor-credits/{ID}")))
        .respond_with(json(200, &format!(r#"{{"credit":{credit_record}}}"#)))
        .mount(&server)
        .await;
    let s = company_session(&server);
    let credit = VendorCreditDraft {
        vendor: vendor(),
        date: day(),
        vendor_invoice_number: Some("CM-1".into()),
        memo: None,
        amount: m("1480.00"),
        lines: vec![BillLineDraft::expense(acct(A), m("1480.00"))],
        draft_attachment: None,
    };
    let saved = s
        .vendor_credits()
        .create(&credit, &PostingOptions::default(), IdempotencyKey::new())
        .await
        .expect("reconciled credit");
    assert_eq!(saved.record.kind(), ApKind::VendorCredit);
    assert_eq!(requests(&server, "POST").await.len(), 1);

    // Bill edit: the pre-read shows the old reference; after the lost PUT
    // the bill reads back with the requested fields.
    let server = lost_server().await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/bills/{ID}")))
        .respond_with(json(200, &bill_body("INV-55871")))
        .up_to_n_times(2)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/bills/{ID}")))
        .respond_with(json(200, &bill_body("INV-55872")))
        .mount(&server)
        .await;
    let s = company_session(&server);
    let read = s.bills().get(id()).await.expect("read");
    let mut d = draft();
    d.vendor_invoice_number = Some("INV-55872".into());
    let saved = s
        .bills()
        .update(
            id(),
            &read,
            &d,
            &PostingOptions::default(),
            IdempotencyKey::new(),
        )
        .await
        .expect("reconciled edit");
    assert_eq!(
        saved.record.vendor_invoice_number.as_deref(),
        Some("INV-55872")
    );
    assert_eq!(requests(&server, "PUT").await.len(), 1);

    // Void.
    let server = lost_server().await;
    let voided = record("INV-55871", "").replace(r#""status":"posted""#, r#""status":"void""#);
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/transactions/{ID}")))
        .respond_with(json(200, &format!(r#"{{"transaction":{voided}}}"#)))
        .mount(&server)
        .await;
    let s = company_session(&server);
    s.bills()
        .void(
            id(),
            "duplicate",
            &ApWriteOptions::default(),
            IdempotencyKey::new(),
        )
        .await
        .expect("reconciled void");
    s.ap_voids()
        .vendor_credit(id(), "duplicate", IdempotencyKey::new())
        .await
        .expect("reconciled credit void (same path)");
    assert_eq!(requests(&server, "POST").await.len(), 2);
}
