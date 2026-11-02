//! Unit tests of Pay Sales Tax, link, recoveries and the liability report
//! (wiremock; exact wire strings; P23 plan §6.3, golden 9's figures).

use super::*;
use crate::resources::recoveries::{
    NewRecovery, RecoveryRefusal, recovery_available, recovery_refusal, recovery_refusal_codes,
};
use crate::resources::sales_tax::SalesTaxBasis;
use crate::resources::sales_tax_reports::{LiabilityQuery, SalesTaxLiability, SixPlaceAmount};
use crate::resources::testkit::{json, query, requests, session_with_caps};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer};

const AGENCY: &str = "7a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const STATE: &str = "7c0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const SPRINGFIELD: &str = "7d0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const BANK: &str = "2b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const DISCOUNTS: &str = "2c0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const PAY: &str = "8a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const INV: &str = "5a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";

const ALL: [&str; 5] = [
    Capabilities::SALES_TAX_PAY,
    Capabilities::IDEMPOTENCY_SALES_TAX_PAY,
    Capabilities::SALES_TAX_LIABILITY,
    Capabilities::BAD_DEBT_RECOVERY,
    Capabilities::IDEMPOTENCY_RECOVERY,
];

fn m(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}

fn d(mo: u32, day: u32) -> CalendarDate {
    CalendarDate::from_ymd(2026, mo, day).expect("date")
}

fn item(s: &str) -> SalesTaxItemId {
    SalesTaxItemId::parse(s).expect("id")
}

fn refusal(code: &str) -> ApiError {
    crate::resources::sales_tax::tests::refusal(code)
}

/// Golden 9: State Revenue through 11/30, paid 12/10, Springfield lowered
/// by a 1.96 timely-filing discount.
fn golden9() -> PaySalesTaxRequest {
    PaySalesTaxRequest {
        agency_id: SalesTaxAgencyId::parse(AGENCY).expect("id"),
        through_date: d(11, 30),
        date: d(12, 10),
        bank_account_id: AccountId::parse(BANK).expect("id"),
        method: SalesTaxPayMethod::Check,
        lines: None,
        adjustments: vec![PayAdjustment {
            tax_item_id: item(SPRINGFIELD),
            account_id: AccountId::parse(DISCOUNTS).expect("id"),
            amount: m("-1.96"),
            reason: "Timely filing discount".into(),
        }],
        print_later: true,
        check_number: None,
        memo: None,
        expected_owed: None,
    }
}

fn preview_json() -> String {
    format!(
        r#"{{"preview":{{"lines":[
          {{"taxItemId":"{STATE}","name":"State","owed":"80.70","adjustment":"0.00","amount":"80.70","expectedOwed":"80.70"}},
          {{"taxItemId":"{SPRINGFIELD}","name":"Springfield","owed":"131.68","adjustment":"-1.96","amount":"129.72","expectedOwed":"131.68"}},
          {{"taxItemId":null,"name":"Unassigned","owed":"3.00","adjustment":"0.00","amount":"0.00","expectedOwed":"3.00"}}],
          "owed":"215.38","total":"210.42","problems":[{{"code":"LEGACY_UNASSIGNED","message":"m","details":{{"amount":"3.00"}}}},{{"code":"SOMETHING_NEW"}},
          {{"code":"AMOUNT_MORE_THAN_OWED","message":"m","details":{{"taxItemId":"{STATE}"}}}}]}}}}"#
    )
}

#[allow(clippy::needless_pass_by_value, reason = "takes the call result as is")]
fn local(r: Result<Value, ApiError>) -> Option<LocalValidation> {
    match r {
        Err(ApiError::Local(l)) => Some(l),
        _ => None,
    }
}

#[test]
fn every_refusal_code_classifies() {
    for code in pay_sales_tax_refusal_codes() {
        assert!(pay_sales_tax_refusal(&refusal(code)).is_some(), "{code}");
    }
    for code in recovery_refusal_codes() {
        assert!(recovery_refusal(&refusal(code)).is_some(), "{code}");
    }
    assert_eq!(
        pay_sales_tax_refusal(&refusal("SALES_TAX_OWED_CHANGED")),
        Some(PaySalesTaxRefusal::OwedChanged)
    );
    assert_eq!(
        crate::payment_refusal(&refusal("WRITE_OFF_RECOVERED")),
        Some(crate::PaymentRefusal::WriteOffRecovered)
    );
}

#[test]
fn the_request_body_is_exact_and_checked() {
    let body = golden9().body().expect("body");
    assert_eq!(
        body,
        json!({"agencyId":AGENCY,"throughDate":"2026-11-30","date":"2026-12-10",
               "bankAccountId":BANK,"method":"check","printLater":true,
               "adjustments":[{"taxItemId":SPRINGFIELD,"accountId":DISCOUNTS,
                               "amount":"-1.96","reason":"Timely filing discount"}]})
    );
    let mut r = golden9();
    r.check_number = Some(1042);
    assert_eq!(
        local(r.body()),
        Some(LocalValidation::InvalidValue),
        "number and print later"
    );
    r.print_later = false;
    assert_eq!(
        r.body().expect("typed number")["checkNumber"],
        json!(1042),
        "a JSON whole number"
    );
    r.check_number = Some(0);
    assert_eq!(local(r.body()), Some(LocalValidation::InvalidValue), "zero");
    r.check_number = Some(1042);
    r.method = SalesTaxPayMethod::Electronic;
    assert_eq!(
        local(r.body()),
        Some(LocalValidation::InvalidValue),
        "number on electronic"
    );
    let mut r = golden9();
    r.method = SalesTaxPayMethod::Electronic;
    assert_eq!(
        local(r.body()),
        Some(LocalValidation::InvalidValue),
        "print later on electronic"
    );
    let mut r = golden9();
    r.adjustments[0].amount = Money::ZERO;
    assert_eq!(local(r.body()), Some(LocalValidation::InvalidValue));
    let mut r = golden9();
    r.adjustments[0].reason = " ".into();
    assert_eq!(local(r.body()), Some(LocalValidation::InvalidValue));
    let mut r = golden9();
    r.adjustments[0].amount = m("-1.955");
    assert_eq!(
        local(r.body()),
        Some(LocalValidation::InvalidValue),
        "sub-cent"
    );
    let line = |a: &str| PayLine {
        tax_item_id: item(STATE),
        amount: Some(m(a)),
        expected_owed: None,
    };
    let mut r = golden9();
    r.lines = Some(vec![line("-1.00")]);
    assert_eq!(
        local(r.body()),
        Some(LocalValidation::InvalidValue),
        "negative line"
    );
    // An untyped item goes without `amount`: the engine pays what it owes
    // plus its adjustments (review finding 5).
    r.lines = Some(vec![
        line("10.00"),
        PayLine {
            tax_item_id: item(SPRINGFIELD),
            amount: None,
            expected_owed: Some(m("131.68")),
        },
    ]);
    let body = r.body().expect("body");
    assert_eq!(
        body["lines"],
        json!([{"taxItemId":STATE,"amount":"10.00"},
               {"taxItemId":SPRINGFIELD,"expectedOwed":"131.68"}])
    );
    r.lines = Some(vec![line("1.00"), line("2.00")]);
    assert_eq!(
        local(r.body()),
        Some(LocalValidation::InvalidValue),
        "same item twice"
    );
    assert!(!format!("{:?}", golden9()).contains("Timely"));
}

#[test]
fn post_lines_copy_the_preview_strings() {
    let p: PayPreview = decode_one(preview_json().as_bytes(), "preview", "p").expect("preview");
    assert_eq!(p.total.to_wire(), "210.4200");
    let lines = p.post_lines();
    assert_eq!(lines.len(), 2, "the unassigned line can't be paid here");
    let v = serde_json::to_value(&lines).expect("json");
    assert_eq!(
        v,
        json!([{"taxItemId":STATE,"amount":"80.70","expectedOwed":"80.70"},
               {"taxItemId":SPRINGFIELD,"amount":"129.72","expectedOwed":"131.68"}])
    );
    assert_eq!(
        p.problems[0].kind(),
        SalesTaxPayProblemKind::LegacyUnassigned
    );
    assert_eq!(p.problems[1].kind(), SalesTaxPayProblemKind::Other);
    assert_eq!(
        p.problems[2].tax_item_id,
        Some(item(STATE)),
        "the item comes from details"
    );
    assert_eq!(
        p.expected_owed_total().map(Money::to_wire).as_deref(),
        Some("215.3800"),
        "owed when the engine sends no expectedOwed"
    );
    assert!(
        !format!("{:?}", p.problems).contains("3.00"),
        "details amounts are not kept"
    );
    assert!(!format!("{p:?}").contains("131"));
}

#[tokio::test]
async fn pay_previews_then_posts_with_a_key() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/sales-tax/payments/preview"))
        .respond_with(json(200, &preview_json()))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/sales-tax/payments"))
        .respond_with(json(
            201,
            &format!(r#"{{"id":"{PAY}","number":"TAX-1","printStatus":"queue"}}"#),
        ))
        .mount(&server)
        .await;
    let none = MockServer::start().await;
    let s = session_with_caps(&none, &[]).await;
    assert_eq!(
        s.sales_tax_payments()
            .preview(&golden9())
            .await
            .expect_err("cap"),
        ApiError::Local(LocalValidation::Unsupported)
    );
    let s = session_with_caps(&server, &ALL).await;
    let p = s
        .sales_tax_payments()
        .preview(&golden9())
        .await
        .expect("preview");
    let mut r = golden9();
    assert_eq!(
        s.sales_tax_payments()
            .create(&r, &PostingOptions::default(), IdempotencyKey::new())
            .await
            .expect_err("no expectedOwed"),
        ApiError::Local(LocalValidation::InvalidValue)
    );
    r.lines = Some(p.post_lines());
    assert_eq!(
        s.sales_tax_payments()
            .create(&r, &PostingOptions::default(), IdempotencyKey::new())
            .await
            .expect_err("no top-level expectedOwed"),
        ApiError::Local(LocalValidation::InvalidValue)
    );
    r.expected_owed = p.expected_owed_total();
    let done = s
        .sales_tax_payments()
        .create(&r, &PostingOptions::default(), IdempotencyKey::new())
        .await
        .expect("pay");
    assert_eq!(done.print_status.as_deref(), Some("queue"));
    let posts = requests(&server, "POST").await;
    assert_eq!(posts.len(), 2, "one preview and one post");
    assert!(posts[1].headers.get("idempotency-key").is_some());
    let body: Value = serde_json::from_slice(&posts[1].body).expect("json");
    assert_eq!(body["lines"][1]["expectedOwed"], "131.68");
    assert_eq!(body["expectedOwed"], "215.38", "the preview's owed, copied");
}

#[test]
fn link_lines_are_checked() {
    let r = LinkPaymentRequest {
        transaction_id: TransactionId::parse(PAY).expect("id"),
        agency_id: SalesTaxAgencyId::parse(AGENCY).expect("id"),
        lines: vec![LinkLine {
            tax_item_id: item(STATE),
            amount: m("24.02"),
        }],
    };
    assert!(r.check().is_ok());
    assert_eq!(
        serde_json::to_value(&r).expect("json"),
        json!({"transactionId":PAY,"agencyId":AGENCY,"lines":[{"taxItemId":STATE,"amount":"24.02"}]})
    );
    let mut bad = r.clone();
    bad.lines.clear();
    assert_eq!(
        bad.check(),
        Err(ApiError::Local(LocalValidation::TooFewLines))
    );
    let mut bad = r.clone();
    bad.lines[0].amount = Money::ZERO;
    assert_eq!(
        bad.check(),
        Err(ApiError::Local(LocalValidation::InvalidValue))
    );
    let mut bad = r;
    bad.lines.push(bad.lines[0].clone());
    assert_eq!(
        bad.check(),
        Err(ApiError::Local(LocalValidation::InvalidValue))
    );
}

#[test]
fn payment_records_decode_with_linked() {
    let rec: SalesTaxPaymentRecord = decode_one(
        format!(
            r#"{{"payment":{{"id":"{PAY}","agencyId":"{AGENCY}","date":"2026-12-10","throughDate":"2026-11-30",
                "method":"check","total":"210.42","linked":false,"lines":[
                {{"taxItemId":"{STATE}","kind":"paid","amount":"80.70"}},
                {{"taxItemId":"{SPRINGFIELD}","kind":"adjustment","amount":"-1.96","accountId":"{DISCOUNTS}","reason":"Timely"}}]}}}}"#
        )
        .as_bytes(),
        "payment",
        "p",
    )
    .expect("record");
    assert_eq!(rec.lines[1].kind, PaymentLineKind::Adjustment);
    assert_eq!(rec.total.to_wire(), "210.4200");
    assert!(!format!("{rec:?}").contains("Timely"));
}

#[tokio::test]
async fn recoveries_are_keyed_and_read_the_available_figure() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/payments/write-off-recoveries"))
        .respond_with(json(
            201,
            &format!(r#"{{"id":"{PAY}","invoiceBalanceDue":"50.00","recoveredTax":"4.08"}}"#),
        ))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &ALL).await;
    let r = NewRecovery {
        invoice_id: TransactionId::parse(INV).expect("id"),
        date: d(12, 15),
        amount: m("50.00"),
        memo: None,
    };
    let done = s
        .recoveries()
        .create(&r, &PostingOptions::default(), IdempotencyKey::new())
        .await
        .expect("recovery");
    assert_eq!(done.invoice_balance_due.to_wire(), "50.0000");
    assert_eq!(
        done.recovered_tax.map(Money::to_wire).as_deref(),
        Some("4.0800")
    );
    let post = &requests(&server, "POST").await[0];
    assert!(post.headers.get("idempotency-key").is_some());
    let body: Value = serde_json::from_slice(&post.body).expect("json");
    assert_eq!(
        body,
        json!({"invoiceId":INV,"date":"2026-12-15","amount":"50.00"})
    );
    let mut zero = r.clone();
    zero.amount = Money::ZERO;
    assert_eq!(
        zero.check(),
        Err(ApiError::Local(LocalValidation::NonPositiveAmount))
    );
    let e = ApiError::Conflict {
        reason: ConflictReason::Unspecified,
        current_version: None,
        code: crate::errors::ErrorCode::parse("RECOVERY_EXCEEDS_WRITE_OFF"),
        message: crate::errors::UserText::default(),
        details: crate::ConflictDetails::new(json!({"available":"67.75"})),
    };
    assert_eq!(recovery_refusal(&e), Some(RecoveryRefusal::ExceedsWriteOff));
    assert_eq!(recovery_available(&e), Some(m("67.75")));
    assert_eq!(recovery_available(&refusal("RECOVERY_IN_USE")), None);
}

/// Golden 6 (accrual, November) and golden 13's bad-debt column, as the
/// v2 report would send them; the v1 numbers are ignored.
fn liability_json(basis: &str) -> String {
    format!(
        r#"{{"title":"Sales Tax Liability","totalSales":5974.0,"totalTax":286.4,"basis":"{basis}",
          "startDate":"2026-11-01","endDate":"2026-11-30",
          "agencies":[{{"agencyId":"{AGENCY}","name":"Sample State Revenue Dept.","items":[
            {{"taxItemId":"{STATE}","name":"State","rate":"0.042250","taxableSales":"1910.15",
              "nonTaxable":[{{"codeId":null,"code":"Non","amount":"1170.00"}}],
              "collected":"80.70","discounts":"0.00","badDebts":"0.00","recoveries":"0.00",
              "adjustments":"0.00","paid":"0.00","owedAtEnd":"80.70"}},
            {{"taxItemId":"{SPRINGFIELD}","name":"Springfield","rate":"0.088750","taxableSales":"1483.70",
              "nonTaxable":[],"collected":"131.68","discounts":"-0.71","badDebts":"-9.60",
              "recoveries":"0.00","adjustments":"0.00","paid":"0.00","owedAtEnd":"121.37"}}],
            "totals":{{"collected":"212.38","discounts":"-0.71","badDebts":"-9.60","owedAtEnd":"202.07"}}}}],
          "totals":{{"collected":"212.38","discounts":"-0.71","badDebts":"-9.60","owedAtEnd":"202.07"}},"total":"202.07",
          "unassigned":"0.00","openingPaid":"0.00","otherEntries":"0.00","glMovement":"202.07",
          "roundingDifference":"-0.00278",
          "balanceCheck":{{"stpBalance":"202.07","owedAtEnd":"202.07","unassigned":"0.00","otherEntriesCumulative":"0.00","openingPaid":"0.00","roundingDifferenceCumulative":"-0.002790"}}}}"#
    )
}

#[tokio::test]
async fn the_liability_report_reads_string_columns_only() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/reports/sales-tax-liability"))
        .respond_with(json(200, &liability_json("accrual")))
        .mount(&server)
        .await;
    let none = MockServer::start().await;
    let s = session_with_caps(&none, &[]).await;
    let q = LiabilityQuery {
        start_date: d(11, 1),
        end_date: d(11, 30),
        basis: Some(SalesTaxBasis::Accrual),
        agency_id: None,
    };
    assert_eq!(
        s.reports()
            .sales_tax_liability(q)
            .await
            .expect_err("v1 only"),
        ApiError::Local(LocalValidation::Unsupported)
    );
    let s = session_with_caps(&server, &ALL).await;
    let bad = LiabilityQuery {
        start_date: d(11, 30),
        end_date: d(11, 1),
        basis: None,
        agency_id: None,
    };
    assert_eq!(
        s.reports()
            .sales_tax_liability(bad)
            .await
            .expect_err("range"),
        ApiError::Local(LocalValidation::BadRange)
    );
    let r = s.reports().sales_tax_liability(q).await.expect("report");
    let a = &r.agencies[0];
    assert_eq!(a.items[0].tax.collected.to_wire(), "80.7000");
    assert_eq!(a.items[0].non_taxable[0].amount.to_wire(), "1170.0000");
    assert_eq!(a.items[1].tax.bad_debts.to_wire(), "-9.6000");
    assert_eq!(
        a.items[1]
            .rate
            .map(cl_core::TaxRate::format_percent)
            .as_deref(),
        Some("8.875%")
    );
    assert_eq!(a.totals.paid, Money::ZERO, "absent columns read 0.00");
    let check = r.balance_check.as_ref().expect("company-wide");
    assert_eq!(check.stp_balance.to_wire(), "202.0700");
    assert_eq!(check.unassigned, Money::ZERO);
    assert_eq!(
        check
            .rounding_difference_cumulative
            .map(SixPlaceAmount::to_text)
            .as_deref(),
        Some("-0.00279")
    );
    assert_eq!(r.totals.collected.to_wire(), "212.3800");
    assert_eq!(r.total.to_wire(), "202.0700");
    assert_eq!(
        r.gl_movement.map(Money::to_wire).as_deref(),
        Some("202.0700")
    );
    assert_eq!(
        r.rounding_difference
            .map(SixPlaceAmount::to_text)
            .as_deref(),
        Some("-0.00278")
    );
    assert!(!r.hides_bad_debts());
    let dbg = format!("{r:?}");
    assert!(!dbg.contains("80.7") && !dbg.contains("Revenue"), "{dbg}");
    let qs = query(&requests(&server, "GET").await[1]);
    assert!(
        qs.contains("start_date=2026-11-01")
            && qs.contains("end_date=2026-11-30")
            && qs.contains("basis=accrual"),
        "{qs}"
    );
    let cash: SalesTaxLiability = serde_json::from_str(&liability_json("cash")).expect("cash");
    assert!(cash.hides_bad_debts());
}

#[test]
fn six_place_amounts_are_strict() {
    assert_eq!(
        SixPlaceAmount::parse("-0.00278").map(SixPlaceAmount::to_text),
        Ok("-0.00278".into())
    );
    assert_eq!(
        SixPlaceAmount::parse("-0.000000").map(SixPlaceAmount::to_text),
        Ok("0".into())
    );
    for bad in ["", "1.", ".5", "1.0000001", "1e3", "+1", "1,000"] {
        assert!(SixPlaceAmount::parse(bad).is_err(), "{bad:?}");
    }
    assert!(
        serde_json::from_str::<SixPlaceAmount>("-0.00278").is_err(),
        "a JSON number"
    );
}

#[test]
fn payment_records_read_stub_lines_and_numeric_check_numbers() {
    let rec: SalesTaxPaymentRecord = decode_one(
        format!(
            r#"{{"id":"{PAY}","date":"2026-12-10","number":null,"status":"void","agencyId":"{AGENCY}",
                "throughDate":"2026-11-30","method":"check","linked":false,"bankAccountId":"{BANK}","total":"210.42",
                "checkNumber":5001,"printStatus":"hand_written","lines":[
                {{"taxItemId":"{STATE}","name":"State","kind":"paid","amount":"80.70","accountId":null,"reason":null}}],
                "stubLines":[{{"label":"State","amount":"80.70"}},{{"label":"Timely filing discount","amount":"-1.96"}}]}}"#
        )
        .as_bytes(),
        "payment",
        "p",
    )
    .expect("record");
    assert_eq!(rec.check_number, Some(5001));
    assert!(rec.is_void());
    assert_eq!(rec.lines[0].name, "State");
    assert_eq!(rec.stub_lines.len(), 2);
    assert_eq!(rec.stub_lines[1].amount.to_wire(), "-1.9600");
    assert!(!format!("{rec:?}").contains("Timely"));
    let done: PaySalesTaxResult = serde_json::from_str(&format!(
        r#"{{"id":"{PAY}","number":null,"checkNumber":null,"printStatus":"queue","total":"210.42"}}"#
    ))
    .expect("result");
    assert_eq!(
        (done.check_number, done.total.map(Money::to_wire)),
        (None, Some("210.4200".into()))
    );
}

#[tokio::test]
async fn unlink_and_mark_paid_through_are_keyed() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!("/api/v1/sales-tax/payments/{PAY}/unlink")))
        .respond_with(json(
            200,
            &format!(r#"{{"transactionId":"{PAY}","unlinked":1}}"#),
        ))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/sales-tax/payments/mark-paid-through"))
        .respond_with(json(
            201,
            &format!(
                r#"{{"agencyId":"{AGENCY}","throughDate":"2026-11-30",
                    "lines":[{{"taxItemId":"{STATE}","name":"State","amount":"80.70"}}],"total":"80.70"}}"#
            ),
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/sales-tax/payments"))
        .respond_with(json(
            200,
            &format!(
                r#"{{"data":[{{"transactionId":"{PAY}","date":"2026-12-10","txnType":"check","number":"77",
                    "agencyId":"{AGENCY}","agencyName":"State","total":"24.02","paid":"24.02","adjustments":"0.00","linked":true}}]}}"#
            ),
        ))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &ALL).await;
    let id = TransactionId::parse(PAY).expect("id");
    let u = s
        .sales_tax_payments()
        .unlink(id, &PostingOptions::default(), IdempotencyKey::new())
        .await
        .expect("unlink");
    assert_eq!(u.unlinked, 1);
    let agency = SalesTaxAgencyId::parse(AGENCY).expect("id");
    let long = "x".repeat(MARK_PAID_REASON_MAX + 1);
    assert_eq!(
        s.sales_tax_payments()
            .mark_paid_through(agency, d(11, 30), Some(&long), IdempotencyKey::new())
            .await
            .expect_err("long reason"),
        ApiError::Local(LocalValidation::InvalidValue)
    );
    let m = s
        .sales_tax_payments()
        .mark_paid_through(agency, d(11, 30), None, IdempotencyKey::new())
        .await
        .expect("mark");
    assert_eq!(m.total.map(Money::to_wire).as_deref(), Some("80.7000"));
    assert_eq!(m.lines[0].tax_item_id, item(STATE));
    let posts = requests(&server, "POST").await;
    assert!(
        posts
            .iter()
            .all(|p| p.headers.get("idempotency-key").is_some())
    );
    let body: Value = serde_json::from_slice(&posts[1].body).expect("json");
    assert_eq!(body, json!({"agencyId":AGENCY,"throughDate":"2026-11-30"}));
    let list = s
        .sales_tax_payments()
        .list(SalesTaxPaymentQuery {
            agency_id: Some(agency),
            start_date: Some(d(1, 1)),
            end_date: None,
        })
        .await
        .expect("list");
    assert!(list[0].linked && !list[0].is_void());
    let qs = query(&requests(&server, "GET").await[1]);
    assert!(
        qs.contains("agency_id=") && qs.contains("start_date=2026-01-01"),
        "snake_case query: {qs}"
    );
    assert_eq!(
        pay_sales_tax_refusal(&refusal("NOT_LINKED")),
        Some(PaySalesTaxRefusal::NotLinked)
    );
}

#[test]
fn a_report_filtered_by_agency_has_no_gl_fields() {
    let filtered = liability_json("accrual")
        .replace(
            r#""otherEntries":"0.00","glMovement":"202.07""#,
            r#""otherEntries":null,"glMovement":null"#,
        )
        .replace(
            r#""balanceCheck":{"stpBalance""#,
            r#""ignored":{"stpBalance""#,
        );
    let r: SalesTaxLiability = serde_json::from_str(&filtered).expect("filtered");
    assert_eq!((r.gl_movement, r.other_entries), (None, None));
    assert!(r.balance_check.is_none());
}

#[tokio::test]
async fn write_offs_list_and_expected_owed_from_e5() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/payments/write-offs"))
        .respond_with(json(
            200,
            &format!(
                r#"{{"data":[{{"invoiceId":"{INV}","invoiceNumber":"INV-W","invoiceDate":"2026-11-01",
                    "customerId":null,"customerName":"x","writeOffDate":"2026-11-20","firstWriteOffDate":"2026-11-20",
                    "writtenOff":"117.75","writeOffTax":"10.45","recovered":"50.00","recoveredTax":"4.08",
                    "recoverable":"67.75","balanceDue":"50.00"}}]}}"#
            ),
        ))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &ALL).await;
    let w = s.recoveries().write_offs(None, false).await.expect("list");
    assert_eq!(
        (w[0].written_off.to_wire(), w[0].recoverable.to_wire()),
        ("117.7500".into(), "67.7500".into())
    );
    assert!(!format!("{:?}", w[0]).contains("117"));
    assert_eq!(
        recovery_refusal(&refusal("RECOVERY_DATE_INVALID")),
        Some(RecoveryRefusal::DateInvalid)
    );
    let p: PayPreview = serde_json::from_str(
        r#"{"lines":[],"owed":"1.00","expectedOwed":"2.00","total":"0.00","problems":[]}"#,
    )
    .expect("preview");
    assert_eq!(
        p.expected_owed_total().map(Money::to_wire).as_deref(),
        Some("2.0000"),
        "the engine's expectedOwed is sent back unchanged"
    );
}
