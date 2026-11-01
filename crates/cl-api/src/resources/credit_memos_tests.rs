//! Unit tests of the P22 resources: credit memos, sales receipts, refunds,
//! customer statements, finance charges, the exact A/R aging and the
//! Outbox kinds (wiremock; exact wire strings; P22 plan §6.3).

use super::*;
use crate::errors::{ErrorCode, UserText};
use crate::resources::customer_refunds::{
    NewRefund, RefundCheck, RefundMethod, RefundRefusal, RefundSource, refund_refusal,
    refund_refusal_codes,
};
use crate::resources::customer_statements::{
    CustomerStatement, IssuedStatementRecord, NewStatementRun, StatementFormat, StatementQuery,
    statement_pdf_name, statement_refusal, statement_refusal_codes,
};
use crate::resources::finance_charges::{
    AnnualRate, AssessContact, AssessRequest, FinanceChargePreview, FinanceChargeRefusal,
    FinanceChargeSettingsUpdate, finance_charge_refusal, finance_charge_refusal_codes,
};
use crate::resources::invoices::{OutboxItem, OutboxKind};
use crate::resources::reports::ArAgingSummary;
use crate::resources::sales_receipts::{NewSalesReceipt, PaymentMethod, check_sales_receipt};
use crate::resources::testkit::{json, requests, session_with_caps};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer};

const CLIENT: &str = "3a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const MEMO: &str = "4b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const INV_A: &str = "5a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const INV_B: &str = "5b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const INCOME: &str = "2a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const BANK: &str = "2b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const STMT: &str = "6a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const RUN: &str = "6b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";

const ALL: [&str; 12] = [
    Capabilities::CREDIT_MEMO_APPLY,
    Capabilities::IDEMPOTENCY_CREDIT_MEMO,
    Capabilities::CREDIT_MEMO_EDIT,
    Capabilities::SALES_RECEIPT_LINES,
    Capabilities::PAYMENT_SETTLE,
    Capabilities::REFUND_CREDIT_MEMO,
    Capabilities::REFUND_CHECKS,
    Capabilities::CUSTOMER_STATEMENT,
    Capabilities::CUSTOMER_STATEMENT_SEND,
    Capabilities::IDEMPOTENCY_CUSTOMER_STATEMENTS,
    Capabilities::FINANCE_CHARGES,
    Capabilities::AR_AGING_EXACT,
];

fn m(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}

fn tid(s: &str) -> TransactionId {
    TransactionId::parse(s).expect("id")
}

fn cid() -> ContactId {
    ContactId::parse(CLIENT).expect("id")
}

fn day(m: u32, d: u32) -> CalendarDate {
    CalendarDate::from_ymd(2026, m, d).expect("date")
}

fn q(s: &str) -> Quantity {
    Quantity::parse_wire(s).expect("qty")
}

/// Golden case 1's memo lines: 2 x 150.00 taxable, 1 x 75.00 not.
fn golden1_memo() -> NewCreditMemo {
    let acct = AccountId::parse(INCOME).expect("id");
    let mut taxable = InvoiceLine::account(acct, q("2"), m("150.00"));
    taxable.is_taxable = Some(true);
    let mut plain = InvoiceLine::account(acct, q("1"), m("75.00"));
    plain.is_taxable = Some(false);
    NewCreditMemo::new(cid(), day(11, 2), vec![taxable, plain])
}

/// Golden case 2's preview.
fn golden2_preview() -> String {
    format!(
        r#"{{"lines":[{{"amount":"300.00","taxAmount":"24.75"}},{{"amount":"75.00","taxAmount":"0.00"}}],
        "subtotal":"375.00","taxableBase":"300.00","taxRate":"0.0825","taxTotal":"24.75","total":"399.75",
        "contactBalanceAfter":"350.25",
        "openInvoices":[
          {{"invoiceId":"{INV_A}","number":"1201","date":"2026-09-15","dueDate":"2026-10-15","balanceDue":"250.00","balanceDueExact":"250.0000"}},
          {{"invoiceId":"{INV_B}","number":"1202","date":"2026-10-20","dueDate":"2026-11-19","balanceDue":"500.00","balanceDueExact":"500.0049"}}],
        "suggestedApplications":[{{"invoiceId":"{INV_A}","amount":"250.00"}},{{"invoiceId":"{INV_B}","amount":"149.75"}}],
        "remainingCredit":"0.00","problems":[]}}"#
    )
}

fn memo_record() -> String {
    format!(
        r#"{{"creditMemo":{{"id":"{MEMO}","number":"CM-1007","date":"2026-11-02","customerId":"{CLIENT}",
        "lines":[{{"accountId":"{INCOME}","description":"Returned leash","quantity":"2.0000","unitPrice":"150.0000",
          "isTaxable":true,"amount":"300.00","taxAmount":"24.75"}},
          {{"accountId":"{INCOME}","quantity":"1.0000","unitPrice":"75.0000","isTaxable":false,"amount":"75.00"}}],
        "subtotal":"375.00","taxAmount":"24.75","total":"399.75","memo":"Kennel return",
        "updatedAt":"2026-11-02T09:00:00.000Z","status":"posted",
        "applications":[{{"invoiceId":"{INV_A}","invoiceNumber":"1201","amount":"250.00","appliedDate":null,"voided":false}},
          {{"invoiceId":"{INV_B}","invoiceNumber":"1202","amount":"149.75","appliedDate":null,"voided":false}}],
        "refunds":[],"remaining":"0.00"}}}}"#
    )
}

#[test]
fn the_preview_reads_engine_strings_and_debug_leaks_nothing() {
    let p: CreditMemoPreview = serde_json::from_str(&golden2_preview()).expect("preview");
    assert_eq!(p.figures.total, m("399.75"));
    assert_eq!(p.figures.tax_total, m("24.75"));
    assert_eq!(p.suggested_applications[1].amount, m("149.75"));
    assert_eq!(
        p.open_invoice(tid(INV_B)).expect("B").balance_due_exact,
        BalanceToken::from_wire("500.0049")
    );
    let dbg = format!("{p:?}");
    for leak in ["399", "149", "1201", "500", "24.75"] {
        assert!(!dbg.contains(leak), "{leak} in {dbg}");
    }
}

#[test]
fn a_json_number_or_a_fraction_of_a_cent_is_refused() {
    let number = golden2_preview().replace(r#""remainingCredit":"0.00""#, r#""remainingCredit":0"#);
    assert!(serde_json::from_str::<CreditMemoPreview>(&number).is_err());
    let sub_cent = golden2_preview().replace(r#""amount":"149.75""#, r#""amount":"149.755""#);
    assert!(serde_json::from_str::<CreditMemoPreview>(&sub_cent).is_err());
    let rec = memo_record().replace(r#""remaining":"0.00""#, r#""remaining":0.0"#);
    assert!(saved_credit_memo(rec.as_bytes()).is_err());
    let credits = format!(
        r#"[{{"sourceId":"{MEMO}","kind":"credit_memo","date":"2026-11-02","total":"399.75","remaining":399.75}}]"#
    );
    assert!(serde_json::from_str::<Vec<OpenCredit>>(&credits).is_err());
}

#[tokio::test]
async fn create_copies_preview_strings_and_tokens_byte_for_byte() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/credit-memos"))
        .respond_with(json(201, &memo_record()))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &ALL).await;
    let p: CreditMemoPreview = serde_json::from_str(&golden2_preview()).expect("preview");
    let mut memo = golden1_memo();
    memo.applications = p
        .suggested_applications
        .iter()
        .map(|a| CreditMemoApplication {
            invoice_id: a.invoice_id,
            amount: a.amount,
            expected_balance_due: p
                .open_invoice(a.invoice_id)
                .expect("open")
                .balance_due_exact
                .clone(),
        })
        .collect();
    let saved = s
        .credit_memos()
        .create(&memo, &PostingOptions::default(), IdempotencyKey::new())
        .await
        .expect("create");
    let req = &requests(&server, "POST").await[0];
    assert!(req.headers.get("idempotency-key").is_some());
    let b: Value = serde_json::from_slice(&req.body).expect("json");
    assert_eq!(
        b["applications"],
        json!([{"invoiceId": INV_A, "amount": "250.00", "expectedBalanceDue": "250.0000"},
               {"invoiceId": INV_B, "amount": "149.75", "expectedBalanceDue": "500.0049"}])
    );
    assert_eq!(b["txnDate"], "2026-11-02");
    assert_eq!(b["contactId"], CLIENT);
    assert_eq!(b["lines"][0]["quantity"], "2.0000");
    assert_eq!(saved.remaining, m("0.00"));
    assert_eq!(saved.live_applications().count(), 2);
    assert!(saved.applications.iter().all(|a| a.applied_date.is_none()));
    let dbg = format!("{saved:?} {memo:?}");
    for leak in ["Kennel", "leash", "399", "CM-1007", "1201"] {
        assert!(!dbg.contains(leak), "{leak} in {dbg}");
    }
}

#[tokio::test]
async fn billable_lines_and_missing_capabilities_send_nothing() {
    let server = MockServer::start().await;
    let s = session_with_caps(&server, &[Capabilities::CREDIT_MEMO_EDIT]).await;
    let mut memo = golden1_memo();
    memo.lines[0].markup_percent = Some(SignedPercent::parse("10").expect("pct"));
    let e = s
        .credit_memos()
        .preview(&memo, None)
        .await
        .expect_err("billable");
    assert_eq!(e, ApiError::Local(LocalValidation::InvalidValue));
    let mut memo = golden1_memo();
    memo.applications = vec![CreditMemoApplication {
        invoice_id: tid(INV_A),
        amount: m("1.00"),
        expected_balance_due: BalanceToken::from_wire("250.0000"),
    }];
    let e = s
        .credit_memos()
        .create(&memo, &PostingOptions::default(), IdempotencyKey::new())
        .await
        .expect_err("needs apply capability");
    assert_eq!(e, ApiError::Local(LocalValidation::Unsupported));
    let e = s
        .credit_memos()
        .credits(cid())
        .await
        .expect_err("needs apply capability");
    assert_eq!(e, ApiError::Local(LocalValidation::Unsupported));
    assert!(requests(&server, "POST").await.is_empty());
}

#[tokio::test]
async fn credits_apply_and_unapply_use_their_routes() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/payments/credits"))
        .respond_with(json(
            200,
            &format!(
                r#"{{"data":[{{"sourceId":"{MEMO}","kind":"credit_memo","date":"2026-11-02","number":"CM-1007",
                "total":"399.75","remaining":"399.75","intendedInvoiceId":"{INV_B}"}}],"total":1}}"#
            ),
        ))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("/api/v1/credit-memos/{MEMO}/apply")))
        .respond_with(json(
            200,
            &format!(
                r#"{{"applied":[{{"invoiceId":"{INV_B}","amount":"299.75"}}],"remaining":"0.00"}}"#
            ),
        ))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("/api/v1/credit-memos/{MEMO}/unapply")))
        .respond_with(json(200, r#"{"remaining":"299.75"}"#))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &ALL).await;
    let credits = s.credit_memos().credits(cid()).await.expect("credits");
    assert_eq!(credits[0].kind, CreditKind::CreditMemo);
    assert_eq!(credits[0].intended_invoice_id, Some(tid(INV_B)));
    let r = ApplyRequest {
        date: day(11, 10),
        applications: vec![crate::resources::customer_payments::ApplyInvoice {
            invoice_id: tid(INV_B),
            amount: None,
            expected_balance_due: Some(BalanceToken::from_wire("500.0000")),
        }],
        expected_updated_at: None,
    };
    let applied = s
        .credit_memos()
        .apply(
            tid(MEMO),
            &r,
            &PostingOptions::default(),
            IdempotencyKey::new(),
        )
        .await
        .expect("apply");
    assert_eq!(applied.remaining, m("0.00"));
    let left = s
        .credit_memos()
        .unapply(
            tid(MEMO),
            tid(INV_B),
            &CreditWriteOptions::default(),
            IdempotencyKey::new(),
        )
        .await
        .expect("unapply");
    assert_eq!(left, m("299.75"));
    let posts = requests(&server, "POST").await;
    let b: Value = serde_json::from_slice(&posts[0].body).expect("json");
    assert_eq!(
        b,
        json!({"date": "2026-11-10", "applications": [{"invoiceId": INV_B, "expectedBalanceDue": "500.0000"}]})
    );
    let b: Value = serde_json::from_slice(&posts[1].body).expect("json");
    assert_eq!(b, json!({"invoiceId": INV_B}));
}

fn refusal(code: &str) -> ApiError {
    ApiError::Conflict {
        reason: ConflictReason::Unspecified,
        current_version: None,
        code: ErrorCode::parse(code),
        message: UserText::default(),
        details: crate::ConflictDetails::default(),
    }
}

/// The 400s naming `assessmentDate` / `checkNumber` map whatever the code
/// (P22 review: future assessment date; check number with Print later).
#[test]
fn field_shaped_400s_map_to_their_refusals() {
    let bad = |field: &str| {
        crate::engine_refusal(
            400,
            &json!({"error": {"message": "x", "code": "VALIDATION_ERROR", "details": {"field": field}}}),
        )
    };
    assert_eq!(
        finance_charge_refusal(&bad("assessmentDate")),
        Some(crate::FinanceChargeRefusal::FutureDate)
    );
    assert_eq!(
        refund_refusal(&bad("checkNumber")),
        Some(crate::RefundRefusal::CheckNumberWithPrintLater)
    );
    assert_eq!(refund_refusal(&bad("memo")), None);
}

#[test]
fn every_refusal_code_maps() {
    for code in credit_refusal_codes() {
        assert!(credit_refusal(&refusal(code)).is_some(), "{code}");
    }
    for code in refund_refusal_codes() {
        assert!(refund_refusal(&refusal(code)).is_some(), "{code}");
    }
    for code in statement_refusal_codes() {
        assert!(statement_refusal(&refusal(code)).is_some(), "{code}");
    }
    for code in finance_charge_refusal_codes() {
        assert!(finance_charge_refusal(&refusal(code)).is_some(), "{code}");
    }
    for (code, want) in [
        (
            "APPLICATION_HAS_ADJUSTMENTS",
            crate::PaymentRefusal::ApplicationHasAdjustments,
        ),
        (
            "APPLY_DATE_BEFORE_SOURCE",
            crate::PaymentRefusal::ApplyDateBeforeSource,
        ),
        (
            "SOURCE_DATE_AFTER_USE",
            crate::PaymentRefusal::SourceDateAfterUse,
        ),
    ] {
        assert_eq!(crate::payment_refusal(&refusal(code)), Some(want));
    }
    assert_eq!(
        credit_refusal(&refusal("CREDIT_REFUNDED")),
        Some(CreditRefusal::CreditRefunded)
    );
    assert_eq!(
        refund_refusal(&refusal("REFUND_DATE_BEFORE_SOURCE")),
        Some(RefundRefusal::DateBeforeSource)
    );
    assert_eq!(
        finance_charge_refusal(&refusal("FINANCE_CHARGE_ASSESSED")),
        Some(FinanceChargeRefusal::Assessed)
    );
    assert_eq!(credit_refusal(&ApiError::NoCompanyOpen), None);
}

#[test]
fn a_sales_receipt_sends_its_deposit_method_and_lines() {
    let acct = AccountId::parse(INCOME).expect("id");
    let mut r = NewSalesReceipt::new(
        day(11, 3),
        vec![InvoiceLine::account(acct, q("2"), m("49.99"))],
    );
    r.payment_method = Some(PaymentMethod::CreditCard);
    r.deposit_to_account_id = Some(AccountId::parse(BANK).expect("id"));
    let caps = [Capabilities::SALES_RECEIPT_LINES]
        .into_iter()
        .collect::<Capabilities>();
    assert!(check_sales_receipt(&r, &caps).is_ok());
    let v = serde_json::to_value(&r).expect("json");
    assert_eq!(v["paymentMethod"], "credit_card");
    assert_eq!(v["depositToAccountId"], BANK);
    assert_eq!(v["lines"][0]["unitPrice"], "49.9900");
    assert!(v.get("contactId").is_none());
    assert_eq!(
        check_sales_receipt(&r, &Capabilities::default()),
        Err(ApiError::Local(LocalValidation::Unsupported))
    );
}

#[test]
fn a_refund_body_names_its_source_and_check_option() {
    let caps: Capabilities = [
        Capabilities::REFUND_CREDIT_MEMO,
        Capabilities::REFUND_CHECKS,
    ]
    .into_iter()
    .collect();
    let mut r = NewRefund {
        customer_id: cid(),
        date: day(11, 5),
        from_account: AccountId::parse(BANK).expect("id"),
        amount: m("399.75"),
        source: RefundSource::CreditMemo(tid(MEMO)),
        method: RefundMethod::Check,
        check: RefundCheck::PrintLater,
        ref_no: None,
        memo: Some("Returned kennel".into()),
    };
    let b = r.body(&caps).expect("body");
    assert_eq!(
        b,
        json!({"txnDate": "2026-11-05", "contactId": CLIENT, "refundFromAccountId": BANK,
            "amount": "399.75", "sourcePaymentId": MEMO, "paymentMethod": "check",
            "printLater": true, "memo": "Returned kennel"})
    );
    let dbg = format!("{r:?}");
    assert!(!dbg.contains("399") && !dbg.contains("kennel"), "{dbg}");
    r.check = RefundCheck::Number("1042".into());
    let b = r.body(&caps).expect("body");
    // The engine takes the check number as a JSON integer.
    assert_eq!(b["checkNumber"], 1042);
    assert_eq!(b["printLater"], false);
    assert!(!format!("{r:?}").contains("1042"));
    r.check = RefundCheck::Number("10-42".into());
    assert_eq!(
        r.body(&caps),
        Err(ApiError::Local(LocalValidation::InvalidValue))
    );
    r.check = RefundCheck::Number("1042".into());
    r.method = RefundMethod::Cash;
    assert_eq!(
        r.body(&caps),
        Err(ApiError::Local(LocalValidation::InvalidValue))
    );
    r.check = RefundCheck::NotACheck;
    r.amount = m("0.00");
    assert_eq!(
        r.body(&caps),
        Err(ApiError::Local(LocalValidation::InvalidValue))
    );
    r.amount = m("1.005");
    assert_eq!(
        r.body(&caps),
        Err(ApiError::Local(LocalValidation::InvalidValue))
    );
    r.amount = m("1.00");
    assert_eq!(
        r.body(&Capabilities::default()),
        Err(ApiError::Local(LocalValidation::Unsupported))
    );
}

fn statement_json(with_ids: bool) -> String {
    let ids = if with_ids {
        format!(r#""id":"{CLIENT}","#)
    } else {
        String::new()
    };
    let txn = if with_ids {
        format!(r#""txnId":"{INV_A}","#)
    } else {
        String::new()
    };
    format!(
        r#"{{"contact":{{{ids}"name":"Bluestem Veterinary Clinic","address":["12 Prairie Rd","Neosho, MO"],"email":"office@bluestem.example"}},
        "asOf":"2026-11-30","format":"open_item",
        "lines":[{{"date":"2026-09-15",{txn}"txnType":"invoice","number":"1201","dueDate":"2026-10-15","amount":"250.00","open":"250.00","balance":"250.00"}},
          {{"date":"2026-11-02","txnType":"credit_memo","number":"CM-1007","amount":"-399.75","open":"-399.75","balance":"-149.75"}}],
        "aging":{{"current":"-50.00","days1to30":"100.25","days31to60":"250.00","days61to90":"0.00","over90":"0.00","total":"300.25"}},
        "amountDue":"300.25","difference":"0.00"}}"#
    )
}

#[tokio::test]
async fn a_statement_reads_its_aging_and_debug_redacts_the_address() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/reports/customer-statement"))
        .respond_with(json(200, &statement_json(true)))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &ALL).await;
    let st = s
        .customer_statements()
        .statement(&StatementQuery {
            contact: cid(),
            as_of: day(11, 30),
            format: StatementFormat::OpenItem,
            start: None,
        })
        .await
        .expect("statement");
    assert_eq!(st.aging.total, m("300.25"));
    assert_eq!(st.aging.buckets.current, m("-50.00"));
    assert_eq!(st.amount_due, st.aging.total);
    assert!(!st.has_difference());
    let gets = requests(&server, "GET").await;
    let req = gets
        .iter()
        .find(|r| r.url.path() == "/api/v1/reports/customer-statement")
        .expect("statement request");
    let qs = crate::resources::testkit::query(req);
    assert!(
        qs.contains("format=open_item") && qs.contains("asOf=2026-11-30"),
        "{qs}"
    );
    let dbg = format!("{st:?}");
    for leak in ["Bluestem", "Prairie", "office@", "300.25", "1201"] {
        assert!(!dbg.contains(leak), "{leak} in {dbg}");
    }
}

#[test]
fn an_issued_statement_reads_a_snapshot_without_ids() {
    let rec = format!(
        r#"{{"id":"{STMT}","number":"1007","contactId":"{CLIENT}","asOf":"2026-11-30","amountDue":"300.25",
        "snapshot":{},"sendHistory":[{{"method":"email","recipients":["office@bluestem.example"]}}]}}"#,
        statement_json(false)
    );
    let r: IssuedStatementRecord = serde_json::from_str(&rec).expect("record");
    assert!(r.snapshot.contact.id.is_none());
    assert!(r.snapshot.lines.iter().all(|l| l.txn_id.is_none()));
    let dbg = format!("{r:?}");
    assert!(
        !dbg.contains("office@") && !dbg.contains("Bluestem"),
        "{dbg}"
    );
    assert_eq!(statement_pdf_name("1007"), "Statement-1007.pdf");
    assert_eq!(statement_pdf_name("../x"), "Statement-x.pdf");
    let legacy = statement_json(true).replace(r#""difference":"0.00""#, r#""difference":"0.0049""#);
    let st: CustomerStatement = serde_json::from_str(&legacy).expect("legacy");
    assert!(st.has_difference());
}

#[tokio::test]
async fn a_run_is_keyed_and_refused_over_500_clients() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/customer-statement-runs"))
        .respond_with(json(
            201,
            &format!(
                r#"{{"runId":"{RUN}","statements":[{{"statementId":"{STMT}","contactId":"{CLIENT}","number":"1007","amountDue":"300.25"}}]}}"#
            ),
        ))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &ALL).await;
    let mut run = NewStatementRun {
        as_of: day(11, 30),
        format: StatementFormat::BalanceForward,
        start: Some(day(11, 1)),
        contact_ids: vec![cid()],
    };
    let r = s
        .customer_statements()
        .create_run(&run, IdempotencyKey::new())
        .await
        .expect("run");
    assert_eq!(r.statements[0].number, "1007");
    let req = &requests(&server, "POST").await[0];
    assert!(req.headers.get("idempotency-key").is_some());
    let b: Value = serde_json::from_slice(&req.body).expect("json");
    assert_eq!(
        b,
        json!({"asOf": "2026-11-30", "format": "balance_forward", "start": "2026-11-01", "contactIds": [CLIENT]})
    );
    run.contact_ids = vec![cid(); 501];
    assert_eq!(
        s.customer_statements()
            .create_run(&run, IdempotencyKey::new())
            .await,
        Err(ApiError::Local(LocalValidation::TooManyLines))
    );
    assert_eq!(requests(&server, "POST").await.len(), 1);
}

fn fc_preview() -> String {
    format!(
        r#"{{"period":"2026-10","settings":{{"annualRate":"18","minimum":"2.00","graceDays":10,"onFinanceCharges":false,"incomeAccountId":"{INCOME}"}},
        "rows":[{{"contactId":"{CLIENT}","name":"Prairie Fork Kennels","overdueBalance":"1050.00","charge":"15.68",
          "sources":[{{"invoiceId":"{INV_A}","number":"1301","dueDate":"2026-09-30","open":"1000.00","fromDate":"2026-09-30","days":31,"amount":"15.29"}},
                     {{"invoiceId":"{INV_B}","number":"1302","dueDate":"2026-10-15","open":"50.00","fromDate":"2026-10-15","days":16,"amount":"0.39"}}]}}],
        "totals":{{"count":1,"charge":"15.68"}},"problems":[]}}"#
    )
}

#[tokio::test]
async fn assess_copies_the_preview_charge_and_is_keyed() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/finance-charges/assess"))
        .respond_with(json(
            201,
            &format!(
                r#"{{"runId":"{RUN}","period":"2026-10","invoices":[{{"contactId":"{CLIENT}","invoiceId":"{MEMO}","number":"1203","total":"15.68"}}]}}"#
            ),
        ))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &ALL).await;
    let p: FinanceChargePreview = serde_json::from_str(&fc_preview()).expect("preview");
    assert!(p.rows[0].chargeable() && !p.not_set_up());
    assert_eq!(
        p.settings.as_ref().and_then(|s| s.annual_rate),
        Some(AnnualRate::parse("18.00").expect("rate"))
    );
    let req = AssessRequest {
        assessment_date: day(10, 31),
        contacts: p
            .rows
            .iter()
            .map(|r| AssessContact {
                contact_id: r.contact_id,
                expected_charge: r.charge,
            })
            .collect(),
    };
    let res = s
        .finance_charges()
        .assess(&req, &PostingOptions::default(), IdempotencyKey::new())
        .await
        .expect("assess");
    assert_eq!(res.invoices[0].total, m("15.68"));
    let posted = &requests(&server, "POST").await[0];
    assert!(posted.headers.get("idempotency-key").is_some());
    let b: Value = serde_json::from_slice(&posted.body).expect("json");
    assert_eq!(
        b,
        json!({"assessmentDate": "2026-10-31", "contacts": [{"contactId": CLIENT, "expectedCharge": "15.68"}]})
    );
    let dbg = format!("{p:?} {req:?}");
    for leak in ["Prairie", "15.68", "1301"] {
        assert!(!dbg.contains(leak), "{leak} in {dbg}");
    }
    let unset = fc_preview().replace(
        r#""problems":[]"#,
        r#""problems":[{"code":"RATE_NOT_SET"}]"#,
    );
    let p: FinanceChargePreview = serde_json::from_str(&unset).expect("preview");
    assert!(p.not_set_up());
}

#[test]
fn the_rate_and_settings_are_checked() {
    for ok in ["18", "18.00", "0.5", "100", "1.2345"] {
        assert!(AnnualRate::parse(ok).is_ok(), "{ok}");
    }
    for bad in ["0", "-1", "100.01", "1.23456", "abc", "", "18."] {
        assert!(AnnualRate::parse(bad).is_err(), "{bad}");
    }
    assert_eq!(AnnualRate::parse("18.00").expect("r").to_wire(), "18");
    let caps = [Capabilities::FINANCE_CHARGES]
        .into_iter()
        .collect::<Capabilities>();
    let u = FinanceChargeSettingsUpdate {
        annual_rate: Some(Some(AnnualRate::parse("18").expect("r"))),
        minimum: Some(m("2.00")),
        grace_days: Some(10),
        on_finance_charges: Some(false),
        income_account_id: Some(None),
        print_later: Some(true),
    };
    assert_eq!(
        u.body(&caps).expect("body"),
        json!({"financeChargeAnnualRate": "18", "financeChargeMinimum": "2.00",
            "financeChargeGraceDays": 10, "financeChargeOnFinanceCharges": false,
            "financeChargeIncomeAccountId": null, "financeChargePrintLater": true})
    );
    assert_eq!(
        u.body(&Capabilities::default()),
        Err(ApiError::Local(LocalValidation::Unsupported))
    );
    let bad = FinanceChargeSettingsUpdate {
        grace_days: Some(366),
        ..FinanceChargeSettingsUpdate::default()
    };
    assert_eq!(
        bad.body(&caps),
        Err(ApiError::Local(LocalValidation::InvalidValue))
    );
    let off = FinanceChargeSettingsUpdate {
        annual_rate: Some(None),
        ..FinanceChargeSettingsUpdate::default()
    };
    assert_eq!(
        off.body(&caps).expect("body"),
        json!({"financeChargeAnnualRate": null})
    );
    let settings: crate::CompanySettings = serde_json::from_str(
        r#"{"financeChargeAnnualRate":"18.0000","financeChargeMinimum":"2.00","financeChargeGraceDays":10}"#,
    )
    .expect("settings");
    assert_eq!(settings.finance_charge_minimum, Some(m("2.00")));
    assert_eq!(settings.finance_charge_grace_days, Some(10));
}

#[test]
fn the_aging_decoder_ignores_numbers_and_keeps_the_no_client_row() {
    let body = format!(
        r#"{{"title":"AR Aging Summary","asOfDate":"2026-11-30",
        "buckets":{{"current":-50,"days1to30":100.25}},"total":310.25,
        "bucketsExact":{{"current":"-50.00","days1to30":"100.25","days31to60":"250.00","days61to90":"0.00","over90":"10.0049"}},
        "totalExact":"310.2549",
        "customers":[
          {{"contactId":"{CLIENT}","name":"Bluestem Veterinary Clinic","buckets":{{"current":"-50.00","days1to30":"100.25","days31to60":"250.00","days61to90":"0.00","over90":"0.00"}},"total":"300.25"}},
          {{"contactId":null,"name":"(no client)","buckets":{{"current":"0.00","days1to30":"0.00","days31to60":"0.00","days61to90":"0.00","over90":"10.0049"}},"total":"10.0049"}}]}}"#
    );
    let a: ArAgingSummary = serde_json::from_str(&body).expect("aging");
    assert_eq!(a.total_exact, m("310.2549"));
    assert_eq!(a.row(Some(cid())).expect("client").total, m("300.25"));
    assert_eq!(
        a.row(None).expect("no client").buckets.over_90,
        m("10.0049")
    );
    let numeric = body.replace(r#""totalExact":"310.2549""#, r#""totalExact":310.2549"#);
    assert!(serde_json::from_str::<ArAgingSummary>(&numeric).is_err());
    assert!(!format!("{a:?}").contains("Bluestem"));
}

#[test]
fn outbox_rows_carry_their_kind() {
    let tx = format!(
        r#"{{"id":"{STMT}","transactionId":"{INV_A}","to":["a@b.example"],"subject":"s","body":"b"}}"#
    );
    let o: OutboxItem = serde_json::from_str(&tx).expect("legacy row");
    assert_eq!(o.kind, OutboxKind::Transaction);
    assert_eq!(o.transaction_id, Some(tid(INV_A)));
    let st = format!(
        r#"{{"id":"{STMT}","kind":"customer_statement","customerStatementId":"{STMT}","to":["a@b.example"],"subject":"s","body":"b"}}"#
    );
    let o: OutboxItem = serde_json::from_str(&st).expect("statement row");
    assert_eq!(o.kind, OutboxKind::CustomerStatement);
    assert!(o.transaction_id.is_none() && o.customer_statement_id.is_some());
    assert!(!format!("{o:?}").contains("a@b"));
}

#[test]
fn a_refund_record_reads_an_integer_check_number_and_its_bank() {
    let body = json!({"refund": {"id": MEMO, "date": "2026-11-05", "customerId": CLIENT,
        "amount": "399.75", "checkNumber": 1042, "refundFromAccountId": BANK,
        "printStatus": "printed", "updatedAt": "2026-11-05T10:00:00.000Z", "status": "posted"}});
    let r: crate::resources::customer_refunds::RefundRecord =
        crate::resources::customer_payments::decode_key(
            body.to_string().as_bytes(),
            "refund",
            "refund",
        )
        .expect("decodes");
    assert_eq!(r.check_number.as_deref(), Some("1042"));
    assert_eq!(r.from_account, Some(AccountId::parse(BANK).expect("id")));
}
