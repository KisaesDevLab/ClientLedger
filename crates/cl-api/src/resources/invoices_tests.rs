//! Unit tests of `invoices.rs` (wiremock; exact wire strings).

use super::*;
use crate::errors::{ConflictReason, ErrorCode, UserText, ValidationError};
use crate::resources::posting::ClosedPeriodAuthorization;
use crate::resources::sales::InvoiceLine;
use crate::resources::testkit::{company_session, json, query, requests, session_with_caps};
use cl_core::Secret;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer};

const CLIENT: &str = "3a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const ID: &str = "4b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const INCOME: &str = "2a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const AR: &str = "2b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const TAX: &str = "2c0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const ITEM: &str = "6b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const TAG: &str = "6a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const SOURCE_LINE: &str = "9a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const LEVEL: &str = "7a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const OUTBOX: &str = "7b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";

/// Every E1 capability (engine PR #197).
const E1: [&str; 7] = [
    Capabilities::SALES_CENTS,
    Capabilities::INVOICE_EDIT,
    Capabilities::IDEMPOTENCY_INVOICE,
    Capabilities::INVOICE_TERMS,
    Capabilities::INVOICE_FIELDS,
    Capabilities::INVOICE_SENT_HISTORY,
    Capabilities::CONTACT_OPEN_BALANCE,
];

fn all_caps() -> Vec<&'static str> {
    let mut v = E1.to_vec();
    v.extend([
        Capabilities::CLOSING_PASSWORD,
        Capabilities::INVOICE_PRICING,
        Capabilities::BILLABLE_COSTS,
        Capabilities::INVOICE_EMAIL,
        Capabilities::PRICE_LEVELS,
    ]);
    v
}

fn client() -> ContactId {
    ContactId::parse(CLIENT).expect("id")
}

fn id() -> TransactionId {
    TransactionId::parse(ID).expect("id")
}

fn m(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}

fn q(s: &str) -> Quantity {
    Quantity::parse_wire(s).expect("qty")
}

fn day() -> CalendarDate {
    CalendarDate::from_ymd(2026, 11, 3).expect("date")
}

fn income() -> AccountId {
    AccountId::parse(INCOME).expect("account")
}

fn item() -> ItemId {
    ItemId::parse(ITEM).expect("item")
}

fn password() -> PostingOptions {
    PostingOptions {
        closed_period: Some(ClosedPeriodAuthorization::with_password(Secret::new(
            "pw-pw-pw-pw",
        ))),
    }
}

fn plain() -> NewInvoice {
    NewInvoice::new(
        client(),
        day(),
        vec![InvoiceLine::account(income(), q("1"), m("1"))],
    )
}

/// Two of the mockup's lines (synthetic): an item line priced by hand with
/// a service date and a plain account line.
fn invoice() -> NewInvoice {
    NewInvoice {
        po_number: Some("PO-7781".into()),
        customer_message: Some("Thank you for your business.".into()),
        memo: Some("internal".into()),
        tag_ids: Some(vec![TagId::parse(TAG).expect("tag")]),
        hide_zero_lines: Some(true),
        payment_terms: Some("net_15".into()),
        ..NewInvoice::new(
            client(),
            day(),
            vec![
                InvoiceLine {
                    description: Some("Shrubs, 3 gal\nplanted".into()),
                    service_date: Some(CalendarDate::from_ymd(2026, 10, 31).expect("d")),
                    unit_price: Some(m("32.50")),
                    ..InvoiceLine::item(item(), q("24"))
                },
                InvoiceLine {
                    is_taxable: Some(false),
                    ..InvoiceLine::account(income(), q("18"), m("65.00"))
                },
            ],
        )
    }
}

/// A saved invoice as `GET /invoices/{id}` sends it: the A/R line, a sale
/// line, a discount line (a debit) and the sales tax line.
fn record(extra: &str) -> String {
    format!(
        r#"{{"id":"{ID}","txnType":"invoice","txnNumber":"INV-1042","txnDate":"2026-11-03","status":"posted",
        "contactId":"{CLIENT}","memo":"internal","total":"88.6800","subtotal":"82.0000","taxAmount":"6.6800",
        "balanceDue":"88.6800","amountPaid":"0.0000","invoiceStatus":"sent","dueDate":"2026-11-18",
        "dueDateOverridden":false,"paymentTerms":"net_15","termsDays":null,"poNumber":"PO-7781",
        "customerMessage":"Thanks","hideZeroLines":true,"tagIds":["{TAG}"],"sentAt":"2026-11-03T12:00:00.000Z",
        "sendHistory":[{{"id":"x","method":"email","recipients":["ap@example.test"],"sentAt":"2026-11-03T12:00:00.000Z","userId":null}}],
        "printLater":false,"updatedAt":"2026-11-03T10:00:00.120Z"{extra},"lines":[
        {{"id":"8a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01","accountId":"{AR}","debit":"88.6800","credit":"0.0000",
          "description":null,"lineOrder":0,"tagId":null,"quantity":null,"unitPrice":null,"isTaxable":false,"taxRate":"0.0000","taxAmount":"0.0000"}},
        {{"id":"8b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01","accountId":"{INCOME}","debit":"0.0000","credit":"97.0000",
          "description":"Mulch","lineOrder":1,"tagId":null,"itemId":"{ITEM}","quantity":"2.0000","unitPrice":"48.5000",
          "isTaxable":true,"taxRate":"0.0810","taxAmount":"7.8600","serviceDate":"2026-10-31"}},
        {{"id":"8c0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01","accountId":"{INCOME}","debit":"15.0000","credit":"0.0000",
          "description":"Discount","lineOrder":2,"tagId":null,"quantity":"1.0000","unitPrice":"-15.0000",
          "isTaxable":true,"taxRate":"0.0810","taxAmount":"-1.2100"}},
        {{"id":"8d0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01","accountId":"{TAX}","debit":"0.0000","credit":"6.6800",
          "description":"Sales Tax","lineOrder":3,"tagId":null,"quantity":null,"unitPrice":null,"isTaxable":false,"taxRate":"0.0000","taxAmount":"0.0000"}}]}}"#
    )
}

fn saved(warnings: &str) -> String {
    format!(r#"{{"invoice":{},"warnings":{warnings}}}"#, record(""))
}

async fn body_of(server: &MockServer, verb: &str, n: usize) -> Value {
    serde_json::from_slice(&requests(server, verb).await[n].body).expect("json")
}

#[tokio::test]
async fn create_sends_exact_strings_and_reads_the_engine_figures() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/invoices"))
        .respond_with(json(
            201,
            &saved(
                r#"[{"code":"DOCUMENT_NUMBER_IN_USE","txnNumber":"INV-1042","transactionId":null}]"#,
            ),
        ))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &E1).await;
    let saved = s
        .invoices()
        .create(
            &invoice(),
            &PostingOptions::default(),
            IdempotencyKey::new(),
        )
        .await
        .expect("create");
    let b = body_of(&server, "POST", 0).await;
    assert_eq!(
        b,
        json!({
            "txnDate": "2026-11-03",
            "contactId": CLIENT,
            "paymentTerms": "net_15",
            "memo": "internal",
            "customerMessage": "Thank you for your business.",
            "poNumber": "PO-7781",
            "tagIds": [TAG],
            "hideZeroLines": true,
            "lines": [
                {"itemId": ITEM, "description": "Shrubs, 3 gal\nplanted", "serviceDate": "2026-10-31",
                 "quantity": "24.0000", "unitPrice": "32.5000"},
                {"accountId": INCOME, "quantity": "18.0000", "unitPrice": "65.0000", "isTaxable": false},
            ],
        })
    );
    assert!(
        requests(&server, "POST").await[0]
            .headers
            .get("idempotency-key")
            .is_some()
    );
    let r = &saved.invoice.record;
    assert_eq!(saved.warnings.len(), 1);
    assert_eq!(saved.warnings[0].txn_number, "INV-1042");
    // The A/R and sales tax lines are the engine's: two document lines.
    assert_eq!(r.lines.len(), 2);
    assert_eq!(r.lines[0].amount.to_wire(), "97.0000");
    assert_eq!(r.lines[0].item, Some(item()));
    assert_eq!(
        r.lines[0].tax_amount.map(Money::to_wire).as_deref(),
        Some("7.8600")
    );
    assert_eq!(
        r.lines[0].service_date,
        Some(CalendarDate::from_ymd(2026, 10, 31).expect("d"))
    );
    // A discount line is a debit: it reads negative.
    assert_eq!(r.lines[1].amount.to_wire(), "-15.0000");
    assert_eq!(
        r.lines[1].tax_amount.map(Money::to_wire).as_deref(),
        Some("-1.2100")
    );
    assert_eq!(r.subtotal.map(Money::to_wire).as_deref(), Some("82.0000"));
    assert_eq!(r.tax_amount.map(Money::to_wire).as_deref(), Some("6.6800"));
    assert_eq!(r.txn.total.map(Money::to_wire).as_deref(), Some("88.6800"));
    assert_eq!(r.invoice_status, Some(InvoiceStatus::Sent));
    assert!(r.was_sent());
    assert_eq!(r.send_history[0].method, SendMethod::Email);
    assert_eq!(r.send_history[0].recipients, vec!["ap@example.test"]);
    assert!(r.hide_zero_lines);
    assert_eq!(r.po_number.as_deref(), Some("PO-7781"));
    // Debug leaks no owner data.
    let dbg = format!("{r:?} {:?}", invoice());
    for secret in [
        "PO-7781",
        "Thanks",
        "ap@example",
        "97.0000",
        "Shrubs",
        "internal",
    ] {
        assert!(!dbg.contains(secret), "{secret} in {dbg}");
    }
}

#[tokio::test]
async fn fields_and_pricing_need_their_capabilities_and_nothing_is_sent() {
    // One server per capability set: the first `/desktop/info` mock wins.
    let none = PostingOptions::default();
    let server = MockServer::start().await;
    let s = session_with_caps(&server, &[Capabilities::SALES_CENTS]).await;
    let e = s
        .invoices()
        .create(&invoice(), &none, IdempotencyKey::new())
        .await
        .expect_err("fields");
    assert_eq!(e, ApiError::Local(LocalValidation::Unsupported));
    assert!(requests(&server, "POST").await.is_empty());
    let server = MockServer::start().await;
    let s = session_with_caps(&server, &E1).await;
    let mut unpriced = NewInvoice::new(client(), day(), vec![InvoiceLine::item(item(), q("1"))]);
    let e = s
        .invoices()
        .create(&unpriced, &none, IdempotencyKey::new())
        .await
        .expect_err("pricing");
    assert_eq!(e, ApiError::Local(LocalValidation::Unsupported));
    unpriced.lines[0].unit_price = Some(m("1"));
    unpriced.lines[0].billable_source_line_id =
        Some(JournalLineId::parse(SOURCE_LINE).expect("line"));
    let e = s
        .invoices()
        .create(&unpriced, &none, IdempotencyKey::new())
        .await
        .expect_err("billable");
    assert_eq!(e, ApiError::Local(LocalValidation::Unsupported));
    assert!(requests(&server, "POST").await.is_empty());
    let server = MockServer::start().await;
    let s = session_with_caps(&server, &[Capabilities::CLOSING_PASSWORD]).await;
    let e = s
        .invoices()
        .create(&plain(), &password(), IdempotencyKey::new())
        .await
        .expect_err("closing password needs invoice-edit-v1");
    assert_eq!(e, ApiError::Local(LocalValidation::Unsupported));
    assert!(requests(&server, "POST").await.is_empty());
}

#[test]
fn local_checks_refuse_what_the_engine_would() {
    let caps: Capabilities = all_caps().into_iter().collect();
    assert_eq!(check_invoice(&invoice(), &caps), Ok(()));
    // Negative quantity and price are fine (a return, a discount line).
    let neg = NewInvoice::new(
        client(),
        day(),
        vec![InvoiceLine::account(income(), q("-1"), m("-15"))],
    );
    assert_eq!(check_invoice(&neg, &caps), Ok(()));
    let one = InvoiceLine::account(income(), q("1"), m("1"));
    let cases: Vec<(NewInvoice, LocalValidation)> = vec![
        (
            NewInvoice::new(client(), day(), vec![]),
            LocalValidation::TooFewLines,
        ),
        (
            NewInvoice::new(client(), day(), vec![one.clone(); MAX_LINES + 1]),
            LocalValidation::TooManyLines,
        ),
        (
            NewInvoice::new(
                client(),
                day(),
                vec![InvoiceLine {
                    account_id: None,
                    ..one.clone()
                }],
            ),
            LocalValidation::InvalidValue,
        ),
        (
            NewInvoice::new(
                client(),
                day(),
                vec![InvoiceLine {
                    markup_percent: Some(SignedPercent::parse("10").expect("p")),
                    ..one.clone()
                }],
            ),
            LocalValidation::InvalidValue,
        ),
        (
            NewInvoice {
                payment_terms: Some("custom".into()),
                ..plain()
            },
            LocalValidation::InvalidValue,
        ),
        (
            NewInvoice {
                po_number: Some("x".repeat(PO_NUMBER_MAX + 1)),
                ..plain()
            },
            LocalValidation::InvalidValue,
        ),
    ];
    for (n, (inv, want)) in cases.into_iter().enumerate() {
        assert_eq!(
            check_invoice(&inv, &caps),
            Err(ApiError::Local(want)),
            "case {n}"
        );
    }
}

#[tokio::test]
async fn update_sends_the_stale_guard_and_its_key_to_put() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/invoices/{ID}")))
        .respond_with(json(200, &format!(r#"{{"invoice":{}}}"#, record(""))))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path(format!("/api/v1/invoices/{ID}")))
        .respond_with(json(200, &saved("[]")))
        .mount(&server)
        .await;
    let s = company_session(&server);
    let read = s.invoices().get(id()).await.expect("get");
    let none = PostingOptions::default();
    let e = s
        .invoices()
        .update(id(), &read.record, &invoice(), &none, IdempotencyKey::new())
        .await
        .expect_err("needs invoice-edit-v1");
    assert_eq!(e, ApiError::Local(LocalValidation::Unsupported));
    let s = session_with_caps(&server, &E1).await;
    let saved = s
        .invoices()
        .update(id(), &read.record, &invoice(), &none, IdempotencyKey::new())
        .await
        .expect("update");
    assert_eq!(saved.warnings, Vec::new());
    let puts = requests(&server, "PUT").await;
    assert_eq!(puts.len(), 1);
    assert!(puts[0].headers.get("idempotency-key").is_some());
    let b: Value = serde_json::from_slice(&puts[0].body).expect("json");
    assert_eq!(b["expectedUpdatedAt"], "2026-11-03T10:00:00.120Z");
    assert_eq!(b["lines"][0]["unitPrice"], "32.5000");
}

#[tokio::test]
async fn void_takes_a_reason_the_guard_and_the_closing_password() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!("/api/v1/invoices/{ID}/void")))
        .respond_with(json(
            200,
            &format!(
                r#"{{"invoice":{}}}"#,
                record("").replace(r#""status":"posted""#, r#""status":"void""#)
            ),
        ))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &all_caps()).await;
    let e = s
        .invoices()
        .void(
            id(),
            "  ",
            &InvoiceWriteOptions::default(),
            IdempotencyKey::new(),
        )
        .await
        .expect_err("reason");
    assert_eq!(e, ApiError::Local(LocalValidation::MissingReason));
    let opts = InvoiceWriteOptions {
        posting: password(),
        expected_updated_at: Some(Timestamp::parse_wire("2026-11-03T10:00:00.120Z").expect("t")),
    };
    let v = s
        .invoices()
        .void(id(), " duplicate ", &opts, IdempotencyKey::new())
        .await
        .expect("void");
    assert!(v.record.is_void());
    let b = body_of(&server, "POST", 0).await;
    assert_eq!(b["reason"], "duplicate");
    assert_eq!(b["closingPassword"], "pw-pw-pw-pw");
    assert!(
        b["expectedUpdatedAt"]
            .as_str()
            .is_some_and(|t| t.starts_with("2026-11-03T10:00:00.12"))
    );
}

#[test]
fn refusals_are_classified() {
    let conflict = |c: &str| ApiError::Conflict {
        reason: ConflictReason::Unspecified,
        current_version: None,
        code: ErrorCode::parse(c),
        message: UserText::default(),
    };
    assert_eq!(
        invoice_refusal(&conflict(INVOICE_HAS_PAYMENTS)),
        Some(InvoiceRefusal::HasPayments)
    );
    assert_eq!(
        invoice_refusal(&conflict(BILLABLE_ALREADY_BILLED)),
        Some(InvoiceRefusal::BillableAlreadyBilled)
    );
    assert_eq!(
        invoice_refusal(&conflict("STALE")),
        Some(InvoiceRefusal::Stale)
    );
    let v = ApiError::Validation(ValidationError::for_field(INVOICE_VOID_CODE, "id", "x"));
    assert_eq!(invoice_refusal(&v), Some(InvoiceRefusal::Void));
    let v = ApiError::Validation(ValidationError::for_field(SMTP_NOT_CONFIGURED, "smtp", "x"));
    assert_eq!(invoice_refusal(&v), Some(InvoiceRefusal::SmtpNotConfigured));
    assert_eq!(invoice_refusal(&conflict("OTHER")), None);
    assert_eq!(invoice_refusal(&ApiError::Timeout), None);
}

#[tokio::test]
async fn sent_history_and_the_unsent_list() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!("/api/v1/invoices/{ID}/sent")))
        .respond_with(json(200, &format!(r#"{{"invoice":{}}}"#, record(""))))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/invoices"))
        .respond_with(json(
            200,
            &format!(
                r#"{{"data":[{{"id":"{ID}","txnNumber":"INV-1042","txnDate":"2026-11-03","dueDate":"2026-11-18",
                "contactId":"{CLIENT}","contactName":"Hartley","total":"88.6800","balanceDue":"10.0000",
                "invoiceStatus":"partial","status":"posted"}}],"total":1}}"#
            ),
        ))
        .mount(&server)
        .await;
    let page = PageRequest::new(50, 0).expect("page");
    let s = company_session(&server);
    let printed = SendRecord {
        method: SendMethod::Printed,
        recipients: vec![],
        sent_at: None,
    };
    assert_eq!(
        s.invoices()
            .record_sent(id(), &printed, IdempotencyKey::new())
            .await
            .expect_err("cap"),
        ApiError::Local(LocalValidation::Unsupported)
    );
    let unsent = InvoiceListQuery {
        sent: Some(false),
        ..InvoiceListQuery::default()
    };
    assert_eq!(
        s.invoices().list(&unsent, page).await.expect_err("cap"),
        ApiError::Local(LocalValidation::Unsupported)
    );
    let s = session_with_caps(&server, &E1).await;
    let bad = SendRecord {
        recipients: vec!["a@example.test".into()],
        ..printed.clone()
    };
    assert_eq!(
        s.invoices()
            .record_sent(id(), &bad, IdempotencyKey::new())
            .await
            .expect_err("recipients only on email"),
        ApiError::Local(LocalValidation::InvalidValue)
    );
    s.invoices()
        .record_sent(id(), &printed, IdempotencyKey::new())
        .await
        .expect("printed");
    let email = SendRecord {
        method: SendMethod::Email,
        recipients: vec!["a@example.test".into()],
        sent_at: Some(Timestamp::parse_wire("2026-11-03T12:00:00Z").expect("t")),
    };
    s.invoices()
        .record_sent(id(), &email, IdempotencyKey::new())
        .await
        .expect("email");
    assert_eq!(
        body_of(&server, "POST", 0).await,
        json!({"method": "printed"})
    );
    let b = body_of(&server, "POST", 1).await;
    assert_eq!(b["method"], "email");
    assert_eq!(b["recipients"], json!(["a@example.test"]));
    assert!(
        b["sentAt"]
            .as_str()
            .is_some_and(|t| t.starts_with("2026-11-03T12:00:00"))
    );
    let lq = InvoiceListQuery {
        contact: Some(client()),
        statuses: vec![
            InvoiceStatus::Draft,
            InvoiceStatus::Partial,
            InvoiceStatus::Other,
        ],
        ..unsent
    };
    let list = s.invoices().list(&lq, page).await.expect("list");
    assert_eq!(
        list.data[0].balance_due.map(Money::to_wire).as_deref(),
        Some("10.0000")
    );
    assert_eq!(list.data[0].invoice_status, Some(InvoiceStatus::Partial));
    let gets = requests(&server, "GET").await;
    let qs = query(gets.last().expect("get"));
    assert!(qs.contains("sent=false"), "{qs}");
    assert!(qs.contains("invoiceStatus=draft%2Cpartial"), "{qs}");
    assert!(qs.contains(&format!("contactId={CLIENT}")), "{qs}");
}

#[tokio::test]
async fn preview_reads_every_figure_as_the_engine_sent_it() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/invoices/preview"))
        .respond_with(json(
            200,
            r#"{"preview":{"lines":[
              {"unitPrice":"30.0600","unitPriceSource":"price_level","amount":"721.44","taxAmount":"58.44"},
              {"unitPrice":"65.0000","unitPriceSource":"input","amount":"1170.00","taxAmount":"0.00"}],
              "subtotal":"1891.44","taxableBase":"721.44","taxRate":"0.0810","taxTotal":"58.44",
              "total":"1949.88","dueDate":"2026-11-18","contactBalanceAfter":"2949.88"}}"#,
        ))
        .mount(&server)
        .await;
    let other = MockServer::start().await;
    let s = session_with_caps(&other, &E1).await;
    assert_eq!(
        s.invoices()
            .preview(&invoice(), None)
            .await
            .expect_err("cap"),
        ApiError::Local(LocalValidation::Unsupported)
    );
    let s = session_with_caps(&server, &all_caps()).await;
    let p = s
        .invoices()
        .preview(&invoice(), Some(id()))
        .await
        .expect("preview");
    assert_eq!(
        p.lines[0].unit_price.map(Money::to_wire).as_deref(),
        Some("30.0600")
    );
    assert_eq!(p.lines[0].unit_price_source, Some(PriceSource::PriceLevel));
    assert_eq!(p.lines[1].amount.to_wire(), "1170.0000");
    assert_eq!(p.total.to_wire(), "1949.8800");
    assert_eq!(
        p.contact_balance_after.map(Money::to_wire).as_deref(),
        Some("2949.8800")
    );
    let b = body_of(&server, "POST", 0).await;
    assert_eq!(b["invoiceId"], ID);
    assert_eq!(b["lines"][1]["unitPrice"], "65.0000");
    // A reply with another line count is not a preview of this invoice.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/invoices/preview"))
        .respond_with(json(
            200,
            r#"{"lines":[],"subtotal":"0","taxTotal":"0","total":"0"}"#,
        ))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &all_caps()).await;
    assert!(matches!(
        s.invoices().preview(&invoice(), None).await,
        Err(ApiError::Decode { .. })
    ));
}

#[tokio::test]
async fn email_carries_the_pdf_and_the_outbox_round_trips() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!("/api/v1/invoices/{ID}/email")))
        .respond_with(json(200, &format!(r#"{{"invoice":{}}}"#, record(""))))
        .mount(&server)
        .await;
    let item = format!(
        r#"{{"id":"{OUTBOX}","transactionId":"{ID}","txnNumber":"INV-1042","to":["ap@example.test"],
        "subject":"Invoice INV-1042","body":"Hello","createdAt":"2026-11-03T12:00:00.000Z"}}"#
    );
    Mock::given(method("POST"))
        .and(path(format!("/api/v1/invoices/{ID}/email-later")))
        .respond_with(json(201, &format!(r#"{{"item":{item}}}"#)))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/forms/email-queue"))
        .respond_with(json(200, &format!(r#"{{"data":[{item}]}}"#)))
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path(format!("/api/v1/forms/email-queue/{OUTBOX}")))
        .respond_with(json(200, r#"{"deleted":true}"#))
        .mount(&server)
        .await;
    let msg = EmailMessage {
        to: vec!["ap@example.test".into()],
        cc: vec![],
        bcc: vec!["owner@example.test".into()],
        subject: "Invoice INV-1042".into(),
        body: "Hello".into(),
    };
    let pdf_name = "Invoice-INV-1042.pdf";
    let other = MockServer::start().await;
    let s = session_with_caps(&other, &E1).await;
    assert_eq!(
        s.invoices()
            .email(id(), &msg, pdf_name, b"%PDF", IdempotencyKey::new())
            .await
            .expect_err("cap"),
        ApiError::Local(LocalValidation::Unsupported)
    );
    let s = session_with_caps(&server, &all_caps()).await;
    let nobody = EmailMessage {
        to: vec![],
        ..msg.clone()
    };
    assert_eq!(
        s.invoices()
            .email(id(), &nobody, pdf_name, b"%PDF", IdempotencyKey::new())
            .await
            .expect_err("to"),
        ApiError::Local(LocalValidation::InvalidValue)
    );
    let big = vec![0u8; EMAIL_PDF_MAX_BYTES + 1];
    assert_eq!(
        s.invoices()
            .email(id(), &msg, pdf_name, &big, IdempotencyKey::new())
            .await
            .expect_err("size"),
        ApiError::Local(LocalValidation::AttachmentTooLarge)
    );
    s.invoices()
        .email(id(), &msg, pdf_name, b"%PDF", IdempotencyKey::new())
        .await
        .expect("email");
    assert_eq!(
        body_of(&server, "POST", 0).await,
        json!({"to": ["ap@example.test"], "bcc": ["owner@example.test"],
               "subject": "Invoice INV-1042", "body": "Hello",
               "pdfBase64": "JVBERg==", "pdfFileName": "Invoice-INV-1042.pdf"})
    );
    assert!(
        requests(&server, "POST").await[0]
            .headers
            .get("idempotency-key")
            .is_some()
    );
    let queued = s
        .invoices()
        .email_later(id(), &msg, IdempotencyKey::new())
        .await
        .expect("later");
    assert_eq!(queued.id, OutboxId::parse(OUTBOX).expect("id"));
    assert!(body_of(&server, "POST", 1).await.get("pdfBase64").is_none());
    let outbox = s.invoices().outbox().await.expect("outbox");
    assert_eq!(outbox[0].message().subject, "Invoice INV-1042");
    s.invoices()
        .remove_from_outbox(queued.id)
        .await
        .expect("remove");
    assert_eq!(requests(&server, "DELETE").await.len(), 1);
    assert!(!format!("{msg:?} {queued:?}").contains("example.test"));
}

#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "one stub server for the three E2/E3 readers"
)]
async fn billable_costs_price_levels_and_client_sales() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/billable-costs"))
        .respond_with(json(
            200,
            &format!(
                r#"{{"data":[{{"lineId":"{SOURCE_LINE}","transactionId":"{ID}","txnType":"bill",
                "txnDate":"2026-10-20","vendorName":"Canary Rental","description":"Stump grinder",
                "accountId":"{INCOME}","amount":"842.15"}}],"total":1}}"#
            ),
        ))
        .mount(&server)
        .await;
    let level = format!(
        r#"{{"id":"{LEVEL}","name":"Builders","percent":"-7.5000","isActive":true,
        "itemPrices":[{{"itemId":"{ITEM}","price":"30.0000"}}]}}"#
    );
    Mock::given(method("POST"))
        .and(path("/api/v1/price-levels"))
        .respond_with(json(201, &format!(r#"{{"priceLevel":{level}}}"#)))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/price-levels"))
        .respond_with(json(200, &format!(r#"{{"data":[{level}]}}"#)))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/contacts/{CLIENT}")))
        .respond_with(json(
            200,
            &format!(
                r#"{{"contact":{{"id":"{CLIENT}","contactType":"customer","displayName":"Hartley",
                "openBalance":"1000.0000","oldestOpenDate":"2026-10-01","priceLevelId":"{LEVEL}",
                "defaultMarkupPercent":"10.0000"}}}}"#
            ),
        ))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path(format!("/api/v1/contacts/{CLIENT}")))
        .respond_with(json(200, r#"{"contact":{}}"#))
        .mount(&server)
        .await;
    let page = PageRequest::new(50, 0).expect("page");
    let s = company_session(&server);
    let refused = [
        s.invoices()
            .billable_costs(client(), page)
            .await
            .map(|_| ()),
        s.invoices().price_levels().list().await.map(|_| ()),
        s.invoices().client_sales(client()).await.map(|_| ()),
        s.invoices().set_client_sales(client(), None, None).await,
    ];
    for e in refused {
        assert_eq!(e, Err(ApiError::Local(LocalValidation::Unsupported)));
    }
    let s = session_with_caps(&server, &all_caps()).await;
    let costs = s
        .invoices()
        .billable_costs(client(), page)
        .await
        .expect("costs");
    assert_eq!(costs.data[0].amount.to_wire(), "842.1500");
    assert_eq!(costs.data[0].txn_type, Some(TxnType::Bill));
    let gets = requests(&server, "GET").await;
    let qs = query(gets.last().expect("get"));
    assert!(qs.contains(&format!("contactId={CLIENT}")), "{qs}");
    let draft = PriceLevelDraft {
        name: "Builders".into(),
        percent: SignedPercent::parse("-7.5").expect("p"),
        is_active: true,
        item_prices: vec![ItemPrice {
            item_id: item(),
            price: m("30"),
        }],
    };
    let created = s
        .invoices()
        .price_levels()
        .create(&draft)
        .await
        .expect("create");
    assert_eq!(created.percent.to_wire(), "-7.5");
    assert_eq!(
        body_of(&server, "POST", 0).await,
        json!({"name": "Builders", "percent": "-7.5", "isActive": true,
               "itemPrices": [{"itemId": ITEM, "price": "30.0000"}]})
    );
    let twice = PriceLevelDraft {
        item_prices: vec![draft.item_prices[0].clone(), draft.item_prices[0].clone()],
        ..draft.clone()
    };
    assert_eq!(
        s.invoices().price_levels().create(&twice).await,
        Err(ApiError::Local(LocalValidation::InvalidValue))
    );
    assert_eq!(
        s.invoices()
            .price_levels()
            .list()
            .await
            .expect("list")
            .len(),
        1
    );
    let sales = s.invoices().client_sales(client()).await.expect("sales");
    assert_eq!(
        sales.open_balance.map(Money::to_wire).as_deref(),
        Some("1000.0000")
    );
    assert_eq!(
        sales.oldest_open_date,
        Some(CalendarDate::from_ymd(2026, 10, 1).expect("d"))
    );
    assert_eq!(sales.price_level_id, Some(created.id));
    assert_eq!(
        sales
            .default_markup_percent
            .map(SignedPercent::to_wire)
            .as_deref(),
        Some("10")
    );
    s.invoices()
        .set_client_sales(client(), None, SignedPercent::parse("12.5").ok())
        .await
        .expect("set");
    assert_eq!(
        body_of(&server, "PUT", 0).await,
        json!({"priceLevelId": null, "defaultMarkupPercent": "12.5"})
    );
}

#[test]
fn signed_percent_is_exact_and_bounded() {
    for (s, want) in [
        ("-7.5", "-7.5"),
        ("10", "10"),
        ("0", "0"),
        ("999.9999", "999.9999"),
        ("-99.9999", "-99.9999"),
    ] {
        assert_eq!(SignedPercent::parse(s).expect(s).to_wire(), want);
    }
    for bad in [
        "-100", "1000", "1e1", "1.", ".5", "1.23456", " 2", "2%", "", "--1",
    ] {
        assert_eq!(
            SignedPercent::parse(bad),
            Err(LocalValidation::InvalidValue),
            "{bad}"
        );
    }
    let p: SignedPercent = serde_json::from_str(r#""-7.5000""#).expect("engine text");
    assert!(p.is_negative());
    assert!(
        serde_json::from_str::<SignedPercent>("10").is_err(),
        "a number"
    );
}

#[test]
fn pdf_names_are_safe_file_names() {
    assert_eq!(invoice_pdf_name("INV-1042"), "Invoice-INV-1042.pdf");
    assert_eq!(invoice_pdf_name("A/B:7"), "Invoice-A-B-7.pdf");
    assert_eq!(invoice_pdf_name("  "), "Invoice.pdf");
}

#[test]
fn a_record_that_is_not_an_invoice_is_refused() {
    let bill = record("").replace(r#""txnType":"invoice""#, r#""txnType":"bill""#);
    assert!(serde_json::from_str::<InvoiceRecord>(&bill).is_err());
    let r: InvoiceRecord = serde_json::from_str(&record("")).expect("invoice");
    assert!(!r.has_payments());
    let paid = record("").replace(r#""amountPaid":"0.0000""#, r#""amountPaid":"5.0000""#);
    let r: InvoiceRecord = serde_json::from_str(&paid).expect("invoice");
    assert!(r.has_payments());
}
