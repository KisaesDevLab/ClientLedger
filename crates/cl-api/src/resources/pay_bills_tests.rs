//! Pay Bills (P18): the payable list, the pay body, the read-back and the
//! refusals, against stub engines.

use super::*;
use crate::errors::{ConflictReason, ErrorCode, UserText, ValidationError};
use crate::resources::testkit::{company_session, json, query, requests, session_with_caps};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer};

const VENDOR: &str = "3a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const BILL_A: &str = "4b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const BILL_B: &str = "4b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a02";
const CREDIT: &str = "4b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a03";
const BANK: &str = "5b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const DISC: &str = "5b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a02";

fn m(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}

fn tid(s: &str) -> TransactionId {
    TransactionId::parse(s).expect("id")
}

fn day(y: i32, mo: u32, d: u32) -> CalendarDate {
    CalendarDate::from_ymd(y, mo, d).expect("date")
}

const PAYABLE: &str = r#"{"bills":[
  {"id":"4b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01","txnNumber":"BILL-0007","contactId":"3a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01",
   "contactName":"canary vendor","txnDate":"2026-10-26","dueDate":"2026-11-10","discountDate":"2026-11-06",
   "discountPercent":"2.0000","vendorInvoiceNumber":"7781","total":"218.4400","amountPaid":"0.0000",
   "creditsApplied":"0.0000","balanceDue":"218.4400","billStatus":"unpaid","paymentTerms":"custom",
   "memo":"canary memo","daysOverdue":0,"discountAvailable":"4.37","updatedAt":"2026-10-26T10:00:00.000Z"},
  {"id":"4b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a02","txnNumber":null,"contactId":"3a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01",
   "contactName":null,"txnDate":"2026-10-12","dueDate":"2026-11-11","discountDate":null,"discountPercent":"0.0000",
   "vendorInvoiceNumber":"8813","total":"3915.0000","amountPaid":"0.0000","creditsApplied":"0.0000",
   "balanceDue":"3915.0000","billStatus":"unpaid","paymentTerms":"net_30","memo":null,"daysOverdue":0}],
 "credits":[{"id":"4b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a03","txnNumber":"VC-0001",
   "contactId":"3a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01","contactName":"canary vendor","txnDate":"2026-10-20",
   "total":"250.0000","balanceDue":"250.0000","memo":"CM-114"}]}"#;

#[tokio::test]
async fn the_payable_list_reads_exact_and_sends_the_payment_date_only_with_discounts() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/bills/payable"))
        .respond_with(json(200, PAYABLE))
        .mount(&server)
        .await;
    let vendor = ContactId::parse(VENDOR).expect("v");
    let filter = PayableFilter {
        vendor: Some(vendor),
        due_on_or_before: Some(day(2026, 11, 11)),
        payment_date: Some(day(2026, 11, 4)),
    };
    // An engine without `bill-discounts-v1`: no paymentDate.
    let s = company_session(&server);
    let list = s.bills().payable_list(&filter).await.expect("list");
    assert_eq!(
        query(&requests(&server, "GET").await[0]),
        format!("contactId={VENDOR}&dueOnOrBefore=2026-11-11")
    );
    assert_eq!(list.bills.len(), 2);
    let a = &list.bills[0];
    assert_eq!(a.balance_due, m("218.44"));
    assert_eq!(a.discount_available, Some(m("4.37")));
    assert_eq!(
        a.discount_percent.map(Percent::to_wire).as_deref(),
        Some("2")
    );
    assert_eq!(a.discount_date, Some(day(2026, 11, 6)));
    assert!(a.updated_at.is_some());
    assert_eq!(a.memo.as_deref(), Some("canary memo"));
    // A zero percent is no discount, not a decode error.
    assert_eq!(list.bills[1].discount_percent, None);
    assert_eq!(list.bills[1].discount_available, None);
    assert_eq!(list.credits.len(), 1);
    assert_eq!(list.credits[0].balance_due, m("250.00"));
    // Debug: no amounts, names, numbers or memos.
    let shown = format!("{list:?}");
    for owner in ["218", "4.37", "canary", "7781", "250", "CM-114"] {
        assert!(!shown.contains(owner), "{owner}: {shown}");
    }

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/bills/payable"))
        .respond_with(json(200, PAYABLE))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &[Capabilities::BILL_DISCOUNTS]).await;
    s.bills().payable_list(&filter).await.expect("list");
    let gets = requests(&server, "GET").await;
    assert_eq!(
        query(gets.last().expect("payable")),
        format!("contactId={VENDOR}&dueOnOrBefore=2026-11-11&paymentDate=2026-11-04")
    );
}

#[tokio::test]
async fn a_float_where_money_belongs_is_refused() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/bills/payable"))
        .respond_with(json(
            200,
            r#"{"bills":[{"id":"4b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01","txnDate":"2026-10-26",
            "balanceDue":218.44}],"credits":[]}"#,
        ))
        .mount(&server)
        .await;
    let s = company_session(&server);
    let e = s
        .bills()
        .payable_list(&PayableFilter::default())
        .await
        .expect_err("float");
    assert_eq!(
        e,
        ApiError::Decode {
            what: "payable bills"
        }
    );
}

/// The mockup's run: 7781 with its 4.37 discount, 8813 with the 250.00
/// credit (CM-114). `amount` is cash plus credits (the engine's meaning).
fn mockup_request(numbering: CheckNumbering) -> PayBillsRequest {
    with_method(PayMethod::Check(numbering))
}

fn with_method(method: PayMethod) -> PayBillsRequest {
    PayBillsRequest {
        bank_account: AccountId::parse(BANK).expect("bank"),
        date: day(2026, 11, 4),
        bills: vec![
            BillToPay {
                bill: tid(BILL_A),
                cash: m("214.07"),
                discount: m("4.37"),
            },
            BillToPay {
                bill: tid(BILL_B),
                cash: m("3665.00"),
                discount: Money::ZERO,
            },
        ],
        credits: vec![CreditToApply {
            credit: tid(CREDIT),
            bill: tid(BILL_B),
            amount: m("250.00"),
        }],
        discount_account: Some(AccountId::parse(DISC).expect("disc")),
        method,
        memo: Some(" canary ".into()),
    }
}

#[test]
fn the_pay_body_sends_coverage_discount_and_credits() {
    let caps: Capabilities = [
        Capabilities::BILL_DISCOUNTS,
        Capabilities::BILL_PAYMENT_CHECK_NUMBER,
    ]
    .into_iter()
    .collect();
    let body = pay_body(&mockup_request(CheckNumbering::PrintLater), &caps).expect("body");
    assert_eq!(body["method"], "check");
    assert_eq!(body["printLater"], true);
    assert!(body.get("checkNumber").is_none());
    assert_eq!(body["bankAccountId"], BANK);
    assert_eq!(body["discountAccountId"], DISC);
    assert_eq!(body["txnDate"], "2026-11-04");
    assert_eq!(body["memo"], "canary");
    let amount = |i: usize, k: &str| m(body["bills"][i][k].as_str().expect(k));
    assert_eq!(amount(0, "amount"), m("214.07"));
    assert_eq!(amount(0, "discountAmount"), m("4.37"));
    // Two places on the wire (the engine refuses more on discountAmount).
    assert_eq!(body["bills"][0]["discountAmount"], "4.37");
    assert_eq!(body["bills"][1]["amount"], "3915.00");
    assert_eq!(body["credits"][0]["amount"], "250.00");
    assert_eq!(amount(1, "amount"), m("3915.00"), "cash 3,665.00 + credit");
    assert!(body["bills"][1].get("discountAmount").is_none());
    assert_eq!(body["credits"][0]["creditId"], CREDIT);
    assert_eq!(body["credits"][0]["billId"], BILL_B);
    assert_eq!(
        m(body["credits"][0]["amount"].as_str().expect("a")),
        m("250.00")
    );
    let numbered = pay_body(
        &mockup_request(CheckNumbering::Assign { first: 4419 }),
        &caps,
    )
    .expect("body");
    assert_eq!(numbered["printLater"], false);
    assert_eq!(numbered["checkNumber"], "4419");
    // Debug never shows amounts or the memo.
    let shown = format!("{:?}", mockup_request(CheckNumbering::PrintLater));
    assert!(
        !shown.contains("214") && !shown.contains("canary"),
        "{shown}"
    );
}

/// Owner answer 2026-10-28: a card payment is `method: "credit_card"` from
/// the card account, with the reference and no check fields.
#[test]
fn a_card_payment_sends_credit_card_and_its_reference() {
    let caps: Capabilities = [Capabilities::BILL_DISCOUNTS].into_iter().collect();
    let card = with_method(PayMethod::CreditCard {
        reference: Some(" AUTH-77 ".into()),
    });
    let body = pay_body(&card, &caps).expect("body");
    assert_eq!(body["method"], "credit_card");
    assert_eq!(body["referenceNumber"], "AUTH-77");
    assert_eq!(body["bankAccountId"], BANK, "the card account");
    for k in ["printLater", "checkNumber"] {
        assert!(body.get(k).is_none(), "{k}");
    }
    let plain = with_method(PayMethod::CreditCard { reference: None });
    assert!(
        pay_body(&plain, &caps)
            .expect("body")
            .get("referenceNumber")
            .is_none()
    );
    let long = with_method(PayMethod::CreditCard {
        reference: Some("x".repeat(101)),
    });
    assert_eq!(
        pay_body(&long, &caps),
        Err(ApiError::Local(LocalValidation::InvalidValue))
    );
    let shown = format!("{card:?}");
    assert!(!shown.contains("AUTH"), "{shown}");
}

#[test]
fn the_pay_body_refuses_what_the_engine_cannot_take() {
    let none = Capabilities::default();
    let unsupported = Err(ApiError::Local(LocalValidation::Unsupported));
    let invalid = Err(ApiError::Local(LocalValidation::InvalidValue));
    // A discount or a discount account without `bill-discounts-v1`.
    assert_eq!(
        pay_body(&mockup_request(CheckNumbering::PrintLater), &none),
        unsupported
    );
    let mut plain = mockup_request(CheckNumbering::PrintLater);
    plain.bills[0].discount = Money::ZERO;
    plain.bills[0].cash = m("218.44");
    plain.discount_account = None;
    let body = pay_body(&plain, &none).expect("no discount needs no capability");
    assert!(body["bills"][0].get("discountAmount").is_none());
    // Numbers now without `bill-payment-check-number-v1`.
    let mut numbered = plain.clone();
    numbered.method = PayMethod::Check(CheckNumbering::Assign { first: 4419 });
    assert_eq!(pay_body(&numbered, &none), unsupported);
    // Nothing to pay, fractions of a cent, a negative cash, a stray credit.
    let mut empty = plain.clone();
    empty.bills.clear();
    assert_eq!(pay_body(&empty, &none), invalid);
    let mut zero = plain.clone();
    zero.bills[0].cash = Money::ZERO;
    assert_eq!(pay_body(&zero, &none), invalid);
    let mut frac = plain.clone();
    frac.bills[0].cash = m("218.445");
    assert_eq!(pay_body(&frac, &none), invalid);
    let mut neg = plain.clone();
    neg.bills[0].cash = m("-1.00");
    assert_eq!(pay_body(&neg, &none), invalid);
    let mut stray = plain;
    stray.credits[0].bill = tid("4b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a09");
    assert_eq!(pay_body(&stray, &none), invalid);
    // With discounts, a bill covered by its discount and credits alone (no
    // cash) is sent.
    let disc: Capabilities = [Capabilities::BILL_DISCOUNTS].into_iter().collect();
    let mut only_discount = mockup_request(CheckNumbering::PrintLater);
    only_discount.bills[0].cash = Money::ZERO;
    let body = pay_body(&only_discount, &disc).expect("discount only");
    assert_eq!(
        m(body["bills"][0]["amount"].as_str().expect("a")),
        Money::ZERO
    );
}

#[tokio::test]
async fn a_pay_posts_once_and_reads_the_payments() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/bill-payments"))
        .respond_with(json(
            201,
            &format!(
                r#"{{"payments":[{{"id":"{BILL_A}","contactId":"{VENDOR}","txnDate":"2026-11-04",
                "checkNumber":null,"printStatus":"queue","total":"3879.0700","netPayment":"3879.0700",
                "billsPaid":2,"creditsApplied":1,"discountTotal":"4.3700","memo":"canary"}}]}}"#
            ),
        ))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &[Capabilities::BILL_DISCOUNTS]).await;
    let paid = s
        .bill_payments()
        .pay(
            &mockup_request(CheckNumbering::PrintLater),
            &PostingOptions::default(),
            IdempotencyKey::new(),
        )
        .await
        .expect("paid");
    assert_eq!(paid.len(), 1);
    assert_eq!(paid[0].net_payment, m("3879.07"));
    assert_eq!(paid[0].discount_total, Some(m("4.37")));
    assert_eq!(paid[0].print_status.as_deref(), Some("queue"));
    assert_eq!((paid[0].bills_paid, paid[0].credits_applied), (2, 1));
    let shown = format!("{paid:?}");
    assert!(!shown.contains("3879"), "{shown}");
    let posts = requests(&server, "POST").await;
    assert_eq!(posts.len(), 1, "sent once");
    // Nothing sent for a local refusal.
    let mut numbered = mockup_request(CheckNumbering::Assign { first: 4419 });
    numbered.discount_account = None;
    let e = s
        .bill_payments()
        .pay(&numbered, &PostingOptions::default(), IdempotencyKey::new())
        .await
        .expect_err("no numbering capability");
    assert_eq!(e, ApiError::Local(LocalValidation::Unsupported));
    assert_eq!(requests(&server, "POST").await.len(), 1);
}

#[tokio::test]
async fn a_payment_reads_back_with_its_applications() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/bill-payments/{BILL_A}")))
        .respond_with(json(
            200,
            &format!(
                r#"{{"payment":{{"id":"{BILL_A}","txnType":"bill_payment","txnDate":"2026-11-04",
                "contactId":"{VENDOR}","total":"3879.0700","checkNumber":4419,"printStatus":"hand_written",
                "status":"posted","memo":"canary","updatedAt":"2026-11-04T10:00:00.000Z",
                "billApplications":[{{"id":"x","paymentId":"{BILL_A}","billId":"{BILL_A}","amount":"214.0700","discountAmount":"4.3700"}},
                  {{"id":"y","paymentId":"{BILL_A}","billId":"{BILL_B}","amount":"3665.0000","discountAmount":"0.0000"}}],
                "creditApplications":[{{"id":"z","paymentId":"{BILL_A}","creditId":"{CREDIT}","billId":"{BILL_B}","amount":"250.0000"}}]}}}}"#
            ),
        ))
        .mount(&server)
        .await;
    let s = company_session(&server);
    let p = s.bill_payments().get(tid(BILL_A)).await.expect("get");
    assert_eq!(p.check_number, Some(4419));
    assert_eq!(p.bill_applications.len(), 2);
    assert_eq!(p.bill_applications[0].amount, m("214.07"), "cash");
    assert_eq!(p.bill_applications[0].discount_amount, Some(m("4.37")));
    assert_eq!(p.credit_applications[0].amount, m("250.00"));
    let shown = format!("{p:?}");
    assert!(
        !shown.contains("214") && !shown.contains("canary"),
        "{shown}"
    );
}

#[test]
fn pay_refusals_are_classified() {
    let validation = |code: Option<&str>| {
        ApiError::Validation(ValidationError::for_field(code.unwrap_or(""), "bills", "x"))
    };
    let conflict = |code: &str| ApiError::Conflict {
        reason: ConflictReason::Unspecified,
        current_version: None,
        code: ErrorCode::parse(code),
        message: UserText::default(),
    };
    assert_eq!(
        pay_problem(&validation(Some(DISCOUNT_ACCOUNT_REQUIRED))),
        Some(PayProblem::DiscountAccountRequired)
    );
    assert_eq!(
        pay_problem(&validation(Some(DISCOUNT_ACCOUNT_INVALID))),
        Some(PayProblem::DiscountAccountInvalid)
    );
    assert_eq!(pay_problem(&validation(Some("VALIDATION_ERROR"))), None);
    assert_eq!(
        pay_problem(&conflict(CHECK_NUMBER_IN_USE)),
        Some(PayProblem::CheckNumberInUse)
    );
    // Q-P18g: the pinned engine's uncoded "exceeds balance due".
    assert_eq!(
        pay_problem(&validation(None)),
        Some(PayProblem::BillsChanged)
    );
    for c in [BILL_NOT_PAYABLE, CREDIT_NOT_AVAILABLE] {
        assert_eq!(
            pay_problem(&validation(Some(c))),
            Some(PayProblem::BillsChanged)
        );
    }
    assert_eq!(
        pay_problem(&conflict(BILL_NOT_RECEIVED)),
        Some(PayProblem::BillsChanged)
    );
    assert_eq!(pay_problem(&conflict("OTHER")), None);
    assert_eq!(pay_problem(&ApiError::Timeout), None);
}

#[tokio::test]
async fn the_discount_account_setting_needs_its_capability() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/api/v1/company/settings"))
        .respond_with(json(
            200,
            &format!(r#"{{"settings":{{"billDiscountAccountId":"{DISC}"}}}}"#),
        ))
        .mount(&server)
        .await;
    let s = company_session(&server);
    let disc = AccountId::parse(DISC).expect("disc");
    assert_eq!(
        s.set_bill_discount_account(Some(disc), IdempotencyKey::new())
            .await,
        Err(ApiError::Local(LocalValidation::Unsupported))
    );
    assert!(requests(&server, "PUT").await.is_empty());
    let s = session_with_caps(&server, &[Capabilities::BILL_DISCOUNTS]).await;
    let saved = s
        .set_bill_discount_account(Some(disc), IdempotencyKey::new())
        .await
        .expect("saved");
    assert_eq!(saved.bill_discount_account_id, Some(disc));
    let puts = requests(&server, "PUT").await;
    let body: Value = serde_json::from_slice(&puts[0].body).expect("json");
    assert_eq!(body, json!({ "billDiscountAccountId": DISC }));
}
