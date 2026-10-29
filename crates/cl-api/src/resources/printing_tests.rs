//! Stub tests of the printing calls (P19).

use std::collections::BTreeMap;

use cl_core::Secret;
use serde_json::Value;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;
use crate::resources::posting::ClosedPeriodAuthorization;
use crate::resources::testkit::{json, query, requests, session_with_caps};

const BANK: &str = "5a7c1b6e-0000-4000-8000-000000000001";
const OTHER_BANK: &str = "5a7c1b6e-0000-4000-8000-000000000002";
const C1: &str = "7d1e3c2a-0000-4000-8000-000000000011";
const C2: &str = "7d1e3c2a-0000-4000-8000-000000000012";
const BATCH: &str = "9f0e8d7c-0000-4000-8000-0000000000aa";

fn tid(s: &str) -> TransactionId {
    TransactionId::parse(s).expect("id")
}

fn bank() -> AccountId {
    AccountId::parse(BANK).expect("bank")
}

fn m(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}

async fn body_of(server: &MockServer, verb: &str, n: usize) -> Value {
    let r = requests(server, verb).await;
    serde_json::from_slice(&r[n].body).expect("json body")
}

#[test]
fn the_queue_reads_exact_amounts_payees_and_mailing_rows() {
    let body = format!(
        r#"{{"data":[
        {{"id":"{C1}","txnDate":"2026-11-04","amount":"1234.56","memo":"Oct",
          "payeeNameOnCheck":"","printedMemo":null,"contactName":"Table Rock Nursery",
          "createdAt":"x","source":null,"txnType":"bill_payment","bankAccountId":"{BANK}",
          "payeeAddress":["12 Elm St","","Branson, MO 65616"]}},
        {{"id":"{C2}","txnDate":"2026-11-05T00:00:00.000Z","amount":"0.10","memo":"rent",
          "payeeNameOnCheck":"Ozark Rentals LLC","printedMemo":"Unit 4","contactName":"Ozark",
          "createdAt":"x","source":null}}]}}"#
    );
    let q = print_queue_from_json(body.as_bytes()).expect("queue");
    assert_eq!(q.len(), 2);
    assert_eq!(q[0].amount, m("1234.56"));
    assert_eq!(
        q[0].payee, "Table Rock Nursery",
        "blank printed name falls back"
    );
    assert_eq!(q[0].memo, "Oct");
    assert_eq!(q[0].kind, QueuedCheckKind::BillPayment);
    assert_eq!(q[0].bank_account, Some(bank()));
    assert_eq!(q[0].address, ["12 Elm St", "Branson, MO 65616"]);
    assert_eq!(q[1].payee, "Ozark Rentals LLC");
    assert_eq!(q[1].memo, "Unit 4");
    assert_eq!(q[1].kind, QueuedCheckKind::Check);
    assert_eq!(q[1].date.to_wire(), "2026-11-05");
    assert_eq!(q[1].amount, m("0.10"));
    assert!(q[1].address.is_empty() && q[1].bank_account.is_none());
    let dbg = format!("{:?}", q[0]);
    assert!(!dbg.contains("Nursery") && !dbg.contains("1234"), "{dbg}");
    assert!(print_queue_from_json(br#"{"data":[{"id":"x"}]}"#).is_err());
}

#[test]
fn check_formats_round_trip_and_standard_prints_as_voucher() {
    for f in [
        CheckFormat::Standard,
        CheckFormat::Voucher,
        CheckFormat::ZFold,
        CheckFormat::CheckMiddle,
    ] {
        assert_eq!(CheckFormat::from_wire(f.as_wire()), f);
    }
    assert_eq!(CheckFormat::from_wire("odd"), CheckFormat::Voucher);
    assert_eq!(CheckFormat::Standard.print_wire(), "voucher");
    assert_eq!(CheckFormat::ZFold.print_wire(), "z_fold");
    assert_eq!(
        CheckLayout::default().format,
        CheckFormat::Voucher,
        "Q-P19q"
    );
}

#[tokio::test]
async fn print_records_numbers_and_classifies_refusals() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/checks/print"))
        .respond_with(json(
            200,
            &format!(r#"{{"batchId":"{BATCH}","checksPrinted":2,"checkNumberRange":"1001–1002"}}"#),
        ))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/checks/print"))
        .respond_with(json(
            409,
            r#"{"error":{"message":"Check #1001 already exists","code":"CHECK_NUMBER_IN_USE","details":{"checkNumber":"1001"}}}"#,
        ))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &[Capabilities::CLOSING_PASSWORD]).await;
    let req = PrintChecks {
        bank_account: bank(),
        checks: vec![tid(C1), tid(C2)],
        starting_number: 1001,
        format: CheckFormat::Standard,
    };
    let opts = PostingOptions {
        closed_period: Some(ClosedPeriodAuthorization::with_password(Secret::new(
            "pw-canary",
        ))),
    };
    let b = s.printing().print_checks(&req, &opts).await.expect("print");
    assert_eq!(b.batch_id.to_string(), BATCH);
    assert_eq!(b.checks_printed, 2);
    let body = body_of(&server, "POST", 0).await;
    assert_eq!(body["bankAccountId"], BANK);
    assert_eq!(body["checkIds"], serde_json::json!([C1, C2]));
    assert_eq!(body["startingCheckNumber"], 1001);
    assert_eq!(body["format"], "voucher");
    assert_eq!(body["closingPassword"], "pw-canary");
    let e = s
        .printing()
        .print_checks(&req, &PostingOptions::default())
        .await
        .expect_err("409");
    assert_eq!(print_problem(&e), Some(PrintProblem::NumberInUse));
    // Nothing is sent for an empty run or a zero first number.
    let empty = PrintChecks {
        checks: Vec::new(),
        ..req.clone()
    };
    assert_eq!(
        s.printing()
            .print_checks(&empty, &PostingOptions::default())
            .await,
        Err(ApiError::Local(LocalValidation::InvalidValue))
    );
    let zero = PrintChecks {
        starting_number: 0,
        ..req
    };
    assert!(
        s.printing()
            .print_checks(&zero, &PostingOptions::default())
            .await
            .is_err()
    );
    assert_eq!(requests(&server, "POST").await.len(), 2);
}

#[tokio::test]
async fn requeue_and_reprint_carry_the_closing_password_and_read_spoiled_numbers() {
    let server = MockServer::start().await;
    let reply = format!(
        r#"{{"message":"Checks returned to queue","checks":[
            {{"id":"{C1}","spoiledCheckNumber":1002,"bankAccountId":"{BANK}"}},
            {{"id":"{C2}","spoiledCheckNumber":null,"bankAccountId":null}}]}}"#
    );
    Mock::given(method("POST"))
        .and(path("/api/v1/checks/requeue"))
        .respond_with(json(200, &reply))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("/api/v1/checks/reprint/{BATCH}")))
        .respond_with(json(200, &reply))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &[Capabilities::CLOSING_PASSWORD]).await;
    let opts = PostingOptions {
        closed_period: Some(ClosedPeriodAuthorization::with_password(Secret::new("pw"))),
    };
    let r = s
        .printing()
        .requeue(&[tid(C1), tid(C2)], &opts)
        .await
        .expect("requeue");
    assert_eq!(r.checks[0].spoiled_number, Some(1002));
    assert_eq!(r.checks[0].bank_account, Some(bank()));
    assert_eq!(r.checks[1].spoiled_number, None);
    assert!(r.skipped.is_empty(), "older engines send no skipped list");
    let body = body_of(&server, "POST", 0).await;
    assert_eq!(body["checkIds"], serde_json::json!([C1, C2]));
    assert_eq!(body["closingPassword"], "pw");
    let batch = Uuid::parse_str(BATCH).expect("uuid");
    let r = s
        .printing()
        .reprint_batch(batch, &PostingOptions::default())
        .await
        .expect("reprint");
    assert_eq!(r.checks.len(), 2);
    let body = body_of(&server, "POST", 1).await;
    assert!(body.get("closingPassword").is_none());
    assert_eq!(
        s.printing().requeue(&[], &PostingOptions::default()).await,
        Err(ApiError::Local(LocalValidation::InvalidValue))
    );
}

/// PR-2 S2 (U-117): a requeue skips void checks and lists them (as ids or
/// rows).
#[tokio::test]
async fn a_requeue_reads_the_skipped_checks() {
    let server = MockServer::start().await;
    let reply = format!(
        r#"{{"checks":[{{"id":"{C1}","spoiledCheckNumber":1001}}],
            "skipped":["{C2}",{{"id":"{C1}","reason":"void"}}]}}"#
    );
    Mock::given(method("POST"))
        .and(path("/api/v1/checks/requeue"))
        .respond_with(json(200, &reply))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &[]).await;
    let r = s
        .printing()
        .requeue(&[tid(C1), tid(C2)], &PostingOptions::default())
        .await
        .expect("requeue");
    assert_eq!(r.checks.len(), 1);
    assert_eq!(r.skipped, vec![tid(C2), tid(C1)]);
}

#[tokio::test]
async fn a_closing_password_without_the_engine_feature_sends_nothing() {
    let server = MockServer::start().await;
    let s = session_with_caps(&server, &[]).await;
    let opts = PostingOptions {
        closed_period: Some(ClosedPeriodAuthorization::with_password(Secret::new("pw"))),
    };
    assert_eq!(
        s.printing().requeue(&[tid(C1)], &opts).await,
        Err(ApiError::Local(
            LocalValidation::ClosedPeriodOverrideUnavailable
        ))
    );
    assert!(requests(&server, "POST").await.is_empty());
}

#[tokio::test]
async fn refusals_of_requeue_are_classified() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/checks/requeue"))
        .respond_with(json(
            400,
            r#"{"error":{"message":"hand written","code":"CHECK_NOT_PRINTED"}}"#,
        ))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/checks/requeue"))
        .respond_with(json(
            423,
            r#"{"error":{"message":"locked","code":"TB_PERIOD_LOCKED","details":{"lockDate":"2026-10-31","closingPasswordRequired":true}}}"#,
        ))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &[]).await;
    let e = s
        .printing()
        .requeue(&[tid(C1)], &PostingOptions::default())
        .await
        .expect_err("400");
    assert_eq!(print_problem(&e), Some(PrintProblem::NotPrinted));
    let e = s
        .printing()
        .requeue(&[tid(C1)], &PostingOptions::default())
        .await
        .expect_err("423");
    assert!(matches!(e, ApiError::ClosedPeriod(_)), "{e:?}");
    assert_eq!(print_problem(&e), None);
}

#[tokio::test]
async fn a_check_style_change_sends_only_that_accounts_style() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/checks/settings"))
        .respond_with(json(
            200,
            &format!(
                r#"{{"settings":{{"routingNumber":"secret-routing","nextCheckNumbers":{{"{BANK}":1042}},
                  "layouts":{{"{OTHER_BANK}":{{"format":"z_fold","printCompanyBlock":true}}}}}},
                  "layout":{{"format":"standard","printCompanyBlock":true,"source":"bank_account"}}}}"#
            ),
        ))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/api/v1/checks/settings"))
        .respond_with(json(200, r#"{"settings":{}}"#))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &[Capabilities::CHECK_REPRINT_AUDIT]).await;
    let (settings, layout) = s.printing().check_layout(bank()).await.expect("get");
    assert_eq!(settings.next_for(bank()), Some(1042));
    assert_eq!(layout.format, CheckFormat::Standard);
    assert!(layout.print_company_block && layout.own);
    assert!(query(&requests(&server, "GET").await[1]).contains(BANK));
    s.printing()
        .set_check_layout(
            bank(),
            CheckLayout {
                format: CheckFormat::Voucher,
                print_company_block: false,
                own: true,
            },
        )
        .await
        .expect("put");
    let gets = requests(&server, "GET").await.len();
    assert_eq!(gets, 2, "no read before the save (S3)");
    let body = body_of(&server, "PUT", 0).await;
    assert!(
        body["layouts"].get(OTHER_BANK).is_none(),
        "only this account's entry"
    );
    assert_eq!(body["layouts"].as_object().map(Map::len), Some(1));
    assert_eq!(body["layouts"][BANK]["format"], "voucher");
    assert_eq!(body["layouts"][BANK]["printCompanyBlock"], false);
    assert!(body.get("routingNumber").is_none(), "only layouts is sent");
    assert_eq!(body.as_object().map(Map::len), Some(1));
}

#[tokio::test]
async fn the_style_change_needs_the_engine_feature() {
    let server = MockServer::start().await;
    let s = session_with_caps(&server, &[]).await;
    assert_eq!(
        s.printing()
            .set_check_layout(bank(), CheckLayout::default())
            .await,
        Err(ApiError::Local(LocalValidation::Unsupported))
    );
    assert_eq!(
        s.printing().logo().await,
        Err(ApiError::Local(LocalValidation::Unsupported))
    );
    assert_eq!(
        s.printing().document_numbers().await,
        Err(ApiError::Local(LocalValidation::Unsupported))
    );
    assert_eq!(
        s.printing().form_queue(None).await,
        Err(ApiError::Local(LocalValidation::Unsupported))
    );
    assert_eq!(server.received_requests().await.map_or(0, |r| r.len()), 1);
}

#[tokio::test]
async fn the_logo_reads_back_or_is_absent_and_uploads_as_multipart() {
    let server = MockServer::start().await;
    let png = b"\x89PNG\r\n\x1a\nrest-of-file".to_vec();
    Mock::given(method("GET"))
        .and(path("/api/v1/company/logo"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(png.clone(), "image/png"))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/company/logo"))
        .respond_with(json(
            404,
            r#"{"error":{"message":"This company has no logo","code":"NOT_FOUND"}}"#,
        ))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/company/logo"))
        .respond_with(json(200, r#"{"company":{}}"#))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &[Capabilities::COMPANY_LOGO]).await;
    assert_eq!(s.printing().logo().await, Ok(Some(png.clone())));
    assert_eq!(s.printing().logo().await, Ok(None));
    s.printing().upload_logo(&png).await.expect("upload");
    let posts = requests(&server, "POST").await;
    let ct = posts[0]
        .headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(ct.starts_with("multipart/form-data; boundary="), "{ct}");
    let text = String::from_utf8_lossy(&posts[0].body);
    assert!(text.contains("name=\"logo\"; filename=\"logo.png\""));
    assert!(text.contains("Content-Type: image/png"));
    for bad in [&b""[..], b"GIF89a....", b"RIFF....WEBP"] {
        assert_eq!(
            s.printing().upload_logo(bad).await,
            Err(ApiError::Local(LocalValidation::InvalidValue))
        );
    }
    let big = [&[0xFF, 0xD8, 0xFF][..], &vec![0; MAX_LOGO_UPLOAD_BYTES]].concat();
    assert!(s.printing().upload_logo(&big).await.is_err());
    assert_eq!(requests(&server, "POST").await.len(), 1);
}

#[tokio::test]
async fn footers_read_and_save_with_blank_overrides_cleared() {
    let server = MockServer::start().await;
    let reply = r#"{"template":{"footerText":"Thanks!","footerOverrides":{"invoice":"Net 30","statement":""}},
        "resolvedFooters":{"invoice":"Net 30","cash_sale":"Thanks!","credit_memo":"Thanks!","statement":"Thanks!"}}"#;
    Mock::given(method("GET"))
        .and(path("/api/v1/company/invoice-template"))
        .respond_with(json(200, reply))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/api/v1/company/invoice-template"))
        .respond_with(json(200, reply))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &[Capabilities::DOCUMENT_NUMBERS]).await;
    let f = s.printing().footers().await.expect("get");
    assert_eq!(f.footer_text.as_deref(), Some("Thanks!"));
    assert_eq!(f.overrides.len(), 1, "a blank override is no override");
    assert_eq!(f.overrides[&DocType::Invoice], "Net 30");
    assert_eq!(f.resolved_for(DocType::SalesReceipt), "Thanks!");
    let mut o = BTreeMap::new();
    o.insert(DocType::Invoice, " Net 30 ".to_string());
    o.insert(DocType::CreditMemo, "  ".to_string());
    s.printing().set_footers("", &o).await.expect("put");
    let body = body_of(&server, "PUT", 0).await;
    assert_eq!(body["footerText"], Value::Null);
    assert_eq!(body["footerOverrides"]["invoice"], "Net 30");
    assert_eq!(body["footerOverrides"]["credit_memo"], Value::Null);
    assert_eq!(body["footerOverrides"]["cash_sale"], Value::Null);
    let long = "x".repeat(MAX_FOOTER_CHARS + 1);
    assert!(s.printing().set_footers(&long, &o).await.is_err());
    assert_eq!(requests(&server, "PUT").await.len(), 1);
    // A company without a template yet.
    assert_eq!(
        footers_from_body(
            r#"{"template":null,"resolvedFooters":{"invoice":"Thank you for your business!"}}"#
        )
        .footer_text,
        None
    );
}

fn footers_from_body(body: &str) -> FormFooters {
    let v: Value = serde_json::from_str(body).expect("json");
    let template = v.get("template").filter(|t| t.is_object());
    FormFooters {
        footer_text: template
            .and_then(|t| t.get("footerText"))
            .and_then(Value::as_str)
            .map(str::to_string),
        ..FormFooters::default()
    }
}

#[tokio::test]
async fn document_numbers_list_peek_set_and_claim() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/document-numbers"))
        .respond_with(json(
            200,
            r#"{"numbers":[{"type":"invoice","prefix":"INV-","nextNumber":1042,"next":"INV-1042"},
                {"type":"cash_sale","prefix":"","nextNumber":1001,"next":"1001"},
                {"type":"future_type","prefix":"","nextNumber":1,"next":"1"}]}"#,
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/document-numbers/next"))
        .respond_with(json(
            200,
            r#"{"type":"credit_memo","prefix":"CM","nextNumber":7,"next":"CM7"}"#,
        ))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/api/v1/document-numbers/cash_sale"))
        .respond_with(json(
            200,
            r#"{"type":"cash_sale","prefix":"SR-","nextNumber":2001,"next":"SR-2001"}"#,
        ))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/document-numbers/claim"))
        .respond_with(json(
            201,
            r#"{"type":"statement","number":12,"txnNumber":"ST12"}"#,
        ))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &[Capabilities::DOCUMENT_NUMBERS]).await;
    let all = s.printing().document_numbers().await.expect("list");
    assert_eq!(all.len(), 2, "unknown types are skipped");
    assert_eq!(all[0].next, "INV-1042");
    let n = s
        .printing()
        .next_number(DocType::CreditMemo)
        .await
        .expect("next");
    assert_eq!((n.prefix.as_str(), n.next_number), ("CM", 7));
    assert!(query(&requests(&server, "GET").await[2]).contains("type=credit_memo"));
    let set = s
        .printing()
        .set_document_number(
            DocType::SalesReceipt,
            &DocumentNumberUpdate {
                prefix: Some("SR-".into()),
                next_number: Some(2001),
            },
        )
        .await
        .expect("put");
    assert_eq!(set.next, "SR-2001");
    let body = body_of(&server, "PUT", 0).await;
    assert_eq!(body, serde_json::json!({"prefix":"SR-","nextNumber":2001}));
    for bad in [
        DocumentNumberUpdate::default(),
        DocumentNumberUpdate {
            prefix: Some("A B".into()),
            next_number: None,
        },
        DocumentNumberUpdate {
            prefix: Some("X".repeat(21)),
            next_number: None,
        },
        DocumentNumberUpdate {
            prefix: None,
            next_number: Some(0),
        },
        DocumentNumberUpdate {
            prefix: None,
            next_number: Some(MAX_NEXT_NUMBER + 1),
        },
    ] {
        assert_eq!(
            s.printing()
                .set_document_number(DocType::Invoice, &bad)
                .await,
            Err(ApiError::Local(LocalValidation::InvalidValue))
        );
    }
    assert_eq!(requests(&server, "PUT").await.len(), 1);
    let c = s
        .printing()
        .claim_number(DocType::Statement)
        .await
        .expect("claim");
    assert_eq!((c.number, c.txn_number.as_str()), (12, "ST12"));
    assert_eq!(body_of(&server, "POST", 0).await["type"], "statement");
}

#[test]
fn number_warnings_are_optional() {
    let with: Value = serde_json::from_str(&format!(
        r#"{{"invoice":{{}},"warnings":[{{"code":"DOCUMENT_NUMBER_IN_USE","txnNumber":"INV-7","transactionId":"{C1}","message":"m"}}]}}"#
    ))
    .expect("json");
    let w = number_warnings(&with);
    assert_eq!(w.len(), 1);
    assert_eq!(w[0].txn_number, "INV-7");
    assert_eq!(w[0].transaction_id, Some(tid(C1)));
    // An idempotent replay has no `warnings`.
    assert_eq!(
        number_warnings(&serde_json::json!({"invoice":{}})),
        Vec::new()
    );
}

#[tokio::test]
async fn the_forms_queue_reads_nullable_fields_and_marks_printed() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/forms/print-queue"))
        .respond_with(json(
            200,
            &format!(
                r#"{{"data":[{{"id":"{C1}","type":"invoice","txnNumber":"INV-1042","txnDate":"2026-11-03",
                    "contactName":"Acme","total":"1500.00"}},
                  {{"id":"{C2}","type":"cash_sale","txnNumber":null,"txnDate":"2026-11-04",
                    "contactName":null,"total":null}}]}}"#
            ),
        ))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/forms/mark-printed"))
        .respond_with(json(200, r#"{"marked":1,"alreadyPrinted":1}"#))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &[Capabilities::FORMS_PRINT_QUEUE]).await;
    let q = s
        .printing()
        .form_queue(Some(DocType::Invoice))
        .await
        .expect("queue");
    assert!(query(&requests(&server, "GET").await[1]).contains("type=invoice"));
    assert_eq!(q[0].total, Some(m("1500.00")));
    assert_eq!(q[0].number.as_deref(), Some("INV-1042"));
    assert_eq!(q[1].kind, DocType::SalesReceipt);
    assert_eq!(
        (q[1].number.clone(), q[1].customer.clone(), q[1].total),
        (None, None, None)
    );
    assert!(!format!("{:?}", q[0]).contains("Acme"));
    let r = s
        .printing()
        .mark_forms_printed(&[tid(C1), tid(C2)])
        .await
        .expect("mark");
    assert_eq!((r.marked, r.already_printed), (1, 1));
    assert_eq!(
        body_of(&server, "POST", 0).await["ids"],
        serde_json::json!([C1, C2])
    );
    assert!(s.printing().mark_forms_printed(&[]).await.is_err());
    let many = vec![tid(C1); MAX_MARK_PRINTED + 1];
    assert!(s.printing().mark_forms_printed(&many).await.is_err());
    assert!(
        s.printing()
            .form_queue(Some(DocType::Statement))
            .await
            .is_err()
    );
    assert_eq!(requests(&server, "POST").await.len(), 1);
}

#[tokio::test]
async fn spoiled_numbers_list_by_bank_account() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/checks/spoiled"))
        .respond_with(json(
            200,
            &format!(
                r#"{{"data":[{{"checkNumber":1002,"checkId":"{C1}","bankAccountId":"{BANK}","reason":"requeue",
                   "spoiledAt":"2026-11-04T10:00:00Z","spoiledBy":null,"spoiledByName":"Kurt","txnDate":"2026-11-04"}}]}}"#
            ),
        ))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &[Capabilities::CHECK_REPRINT_AUDIT]).await;
    let sp = s.printing().spoiled(Some(bank())).await.expect("spoiled");
    assert_eq!(sp[0].number, 1002);
    assert_eq!(sp[0].reason, SpoiledReason::Requeue);
    assert_eq!(
        sp[0].date.map(CalendarDate::to_wire).as_deref(),
        Some("2026-11-04")
    );
}

#[test]
fn bill_payment_applications_read_the_stub_numbers() {
    let body = format!(
        r#"{{"id":"{C1}","txnDate":"2026-11-04","status":"posted",
        "billApplications":[{{"billId":"{C2}","amount":"100.00","bill":{{"txnNumber":"B-7","vendorInvoiceNumber":"20931",
            "txnDate":"2026-10-01","total":"250.00"}}}},
          {{"billId":"{C1}","amount":"5.00","bill":null}}],
        "creditApplications":[{{"creditId":"{C1}","billId":"{C2}","amount":"20.00",
            "credit":{{"txnNumber":null,"vendorInvoiceNumber":"CM-1","txnDate":"2026-10-02","total":"20.00"}}}}]}}"#
    );
    let r: crate::BillPaymentRecord = serde_json::from_str(&body).expect("record");
    let b = r.bill_applications[0].bill.as_ref().expect("bill");
    assert_eq!(b.vendor_invoice_number.as_deref(), Some("20931"));
    assert_eq!(b.total, Some(m("250.00")));
    assert!(r.bill_applications[1].bill.is_none());
    let c = r.credit_applications[0].credit.as_ref().expect("credit");
    assert_eq!(c.txn_number, None);
    assert_eq!(c.total, Some(m("20.00")));
}
