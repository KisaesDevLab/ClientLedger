//! Unit tests of `customer_payments.rs` (wiremock; exact wire strings).

use super::*;
use crate::errors::{ErrorCode, UserText};
use crate::resources::posting::ClosedPeriodAuthorization;
use crate::resources::testkit::{json, requests, session_with_caps};
use cl_core::Secret;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer};

const CLIENT: &str = "3a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const PAY: &str = "4b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const INV1: &str = "5a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const INV2: &str = "5b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const UF: &str = "2a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const BANK: &str = "2b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const RET: &str = "4c0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";

const ALL: [&str; 8] = [
    Capabilities::RECEIVE_PAYMENT,
    Capabilities::IDEMPOTENCY_RECEIVE_PAYMENT,
    Capabilities::PAYMENT_BOUNCE,
    Capabilities::PAYMENT_SETTLE,
    Capabilities::PAYMENT_DISCOUNTS,
    Capabilities::CUSTOMER_DEPOSITS,
    Capabilities::FEED_CUSTOMER_PAYMENTS,
    Capabilities::CLOSING_PASSWORD,
];

fn m(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}

fn tid(s: &str) -> TransactionId {
    TransactionId::parse(s).expect("id")
}

fn day() -> CalendarDate {
    CalendarDate::from_ymd(2026, 11, 4).expect("date")
}

/// The mockup's preview (golden case 1).
fn mockup_preview() -> String {
    format!(
        r#"{{"contactBalanceBefore":"5645.00","contactBalanceAfter":"1645.00","amount":"4000.00",
        "amountSource":"typed","invoices":[
        {{"invoiceId":"{INV1}","invoiceNumber":"1091","txnDate":"2026-10-05","dueDate":"2026-11-04",
          "total":"4385.0000","balanceDue":"4385.00","balanceDueExact":"4385.0000","payment":"4000.00","remaining":"385.00"}},
        {{"invoiceId":"{INV2}","invoiceNumber":"1095","txnDate":"2026-10-28","dueDate":"2026-11-27",
          "total":"1260.0000","balanceDue":"1260.00","balanceDueExact":"1260.0000","payment":"0.00","remaining":"1260.00"}}],
        "totals":{{"amountDue":"5645.00","applied":"4000.00","discountsAndCredits":"0.00","unapplied":"0.00"}},
        "shortfalls":[{{"invoiceId":"{INV1}","amount":"385.00","withinWriteOffLimit":false}}],
        "pendingDeposits":{{"count":2,"total":"10210.00"}},"averageDaysToPay":24,"problems":[]}}"#
    )
}

fn record() -> String {
    format!(
        r#"{{"payment":{{"id":"{PAY}","customerId":"{CLIENT}","date":"2026-11-04","amount":"4000.00",
        "depositTo":"{UF}","paymentMethod":"check","refNo":"5521","memo":"Thanks","updatedAt":"2026-11-04T08:10:00.000Z",
        "status":"posted","applications":[{{"invoiceId":"{INV1}","invoiceNumber":"1091","amount":"4000.00",
        "discount":"0.00","writeOff":"0.00","appliedDate":"2026-11-04","voided":false,"reversed":false}}],
        "unappliedRemaining":"0.00","holdsDeposit":false,"depositRemaining":"0.00"}}}}"#
    )
}

fn save_from(p: &PaymentPreview) -> NewCustomerPayment {
    NewCustomerPayment {
        customer_id: ContactId::parse(CLIENT).expect("id"),
        date: day(),
        amount: p.amount,
        deposit_to: AccountId::parse(UF).expect("id"),
        payment_method: Some("check".into()),
        ref_no: Some("5521".into()),
        memo: None,
        applications: p
            .invoices
            .iter()
            .filter(|i| i.payment.is_positive())
            .map(|i| SaveApplication {
                invoice_id: i.invoice_id,
                amount: i.payment,
                expected_balance_due: i.balance_due_exact.clone(),
                discount_amount: None,
                write_off_amount: None,
            })
            .collect(),
        credit_applications: Vec::new(),
        write_off_account_id: None,
        discount_account_id: None,
        hold_as_deposit: false,
        bank_feed_item_id: None,
    }
}

#[test]
fn preview_reads_engine_strings_and_debug_leaks_nothing() {
    let p: PaymentPreview = serde_json::from_str(&mockup_preview()).expect("preview");
    assert_eq!(p.amount, m("4000"));
    assert_eq!(p.amount_source, AmountSource::Typed);
    assert_eq!(p.invoices[0].payment, m("4000"));
    assert_eq!(p.invoices[0].remaining, m("385"));
    assert_eq!(p.totals.amount_due, m("5645"));
    assert_eq!(p.shortfalls[0].amount, m("385"));
    assert_eq!(p.contact_balance_after, m("1645"));
    assert_eq!(p.pending_deposits.as_ref().map(|d| d.count), Some(2));
    assert_eq!(p.average_days_to_pay, Some(24));
    assert_eq!(p.invoices[0].balance_due_exact.as_wire(), "4385.0000");
    // Fields the later engine rows add read as zero or empty.
    assert!(p.credits.is_empty() && p.deposits_on_hand.is_empty());
    assert!(!p.hold_as_deposit_offered);
    assert_eq!(p.invoices[0].discount_available, Money::ZERO);
    let dbg = format!("{p:?} {:?} {:?}", p.invoices, p.totals);
    for leak in ["4000", "385", "5645", "1091", "1645"] {
        assert!(!dbg.contains(leak), "{leak} in {dbg}");
    }
}

#[test]
fn a_json_number_or_a_fraction_of_a_cent_is_refused() {
    let number = mockup_preview().replace(r#""amount":"4000.00""#, r#""amount":4000.00"#);
    assert!(serde_json::from_str::<PaymentPreview>(&number).is_err());
    let sub_cent = mockup_preview().replace(r#""payment":"4000.00""#, r#""payment":"4000.005""#);
    assert!(serde_json::from_str::<PaymentPreview>(&sub_cent).is_err());
    // A stored legacy total may carry four places (display only).
    let legacy = mockup_preview().replace("4385.0000", "4385.0049");
    let p: PaymentPreview = serde_json::from_str(&legacy).expect("legacy");
    assert_eq!(p.invoices[0].total.to_wire(), "4385.0049");
    assert_eq!(p.invoices[0].balance_due_exact.as_wire(), "4385.0049");
    assert_eq!(cents_wire(m("4000")).as_deref().ok(), Some("4000.00"));
    assert_eq!(cents_wire(m("-55.1")).as_deref().ok(), Some("-55.10"));
    assert!(cents_wire(m("0.005")).is_err());
}

#[tokio::test]
async fn preview_sends_auto_without_applications_and_explicit_fill_as_null() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/payments/preview"))
        .respond_with(json(200, &mockup_preview()))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &ALL).await;
    let mut r = PaymentPreviewRequest {
        customer_id: ContactId::parse(CLIENT).expect("id"),
        date: day(),
        amount: Some(m("4000")),
        deposit_to: Some(AccountId::parse(UF).expect("id")),
        applications: None,
        credit_applications: Vec::new(),
        hold_as_deposit: false,
        editing_payment_id: None,
    };
    s.customer_payments().preview(&r).await.expect("auto");
    r.amount = None;
    r.applications = Some(vec![
        ApplicationRequest {
            invoice_id: tid(INV2),
            amount: None,
            discount_amount: None,
            write_off_amount: None,
        },
        ApplicationRequest {
            invoice_id: tid(INV1),
            amount: Some(m("384.25")),
            discount_amount: None,
            write_off_amount: Some(m("0.75")),
        },
    ]);
    s.customer_payments().preview(&r).await.expect("explicit");
    let sent = requests(&server, "POST").await;
    let a: Value = serde_json::from_slice(&sent[0].body).expect("json");
    assert_eq!(
        a,
        json!({"customerId": CLIENT, "date": "2026-11-04", "amount": "4000.00", "depositTo": UF})
    );
    let b: Value = serde_json::from_slice(&sent[1].body).expect("json");
    assert_eq!(
        b,
        json!({"customerId": CLIENT, "date": "2026-11-04", "depositTo": UF, "applications": [
            {"invoiceId": INV2, "amount": null},
            {"invoiceId": INV1, "amount": "384.25", "writeOffAmount": "0.75"},
        ]})
    );
}

#[tokio::test]
async fn receive_copies_preview_strings_and_tokens_byte_for_byte() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/payments/receive"))
        .respond_with(json(201, &record()))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &ALL).await;
    let legacy = mockup_preview().replace("\"4385.0000\"", "\"4385.0049\"");
    let p: PaymentPreview = serde_json::from_str(&legacy).expect("preview");
    let opts = PostingOptions {
        closed_period: Some(ClosedPeriodAuthorization::with_password(Secret::new(
            "pw-pw-pw-pw",
        ))),
    };
    let saved = s
        .customer_payments()
        .receive(&save_from(&p), &opts, IdempotencyKey::new())
        .await
        .expect("receive");
    let req = &requests(&server, "POST").await[0];
    assert!(req.headers.get("idempotency-key").is_some());
    let b: Value = serde_json::from_slice(&req.body).expect("json");
    assert_eq!(
        b,
        json!({"customerId": CLIENT, "date": "2026-11-04", "amount": "4000.00", "depositTo": UF,
            "paymentMethod": "check", "refNo": "5521",
            "applications": [{"invoiceId": INV1, "amount": "4000.00", "expectedBalanceDue": "4385.0049"}],
            "closingPassword": "pw-pw-pw-pw"})
    );
    assert_eq!(saved.id, tid(PAY));
    assert_eq!(saved.live_applications().count(), 1);
    assert!(!saved.is_bounced() && !saved.is_void());
    let dbg = format!("{saved:?} {:?}", save_from(&p));
    for leak in ["5521", "Thanks", "4000", "1091", "4385"] {
        assert!(!dbg.contains(leak), "{leak} in {dbg}");
    }
}

#[tokio::test]
async fn later_fields_need_their_capabilities_and_nothing_is_sent() {
    let server = MockServer::start().await;
    let s = session_with_caps(&server, &[Capabilities::RECEIVE_PAYMENT]).await;
    let p: PaymentPreview = serde_json::from_str(&mockup_preview()).expect("preview");
    let mut new = save_from(&p);
    new.applications[0].write_off_amount = Some(m("0.75"));
    let e = s
        .customer_payments()
        .receive(&new, &PostingOptions::default(), IdempotencyKey::new())
        .await;
    assert_eq!(e.err(), Some(ApiError::Local(LocalValidation::Unsupported)));
    let mut hold = save_from(&p);
    hold.hold_as_deposit = true;
    hold.applications.clear();
    assert_eq!(
        check_payment(&hold, &s.client().capabilities()),
        Err(ApiError::Local(LocalValidation::Unsupported))
    );
    let bounce = BounceRequest {
        date: day(),
        bank_fee_amount: None,
        bank_fee_account_id: None,
        bank_fee_bank_account_id: None,
        memo: None,
        expected_updated_at: Timestamp::parse_wire("2026-11-04T08:10:00.000Z").expect("t"),
    };
    let e = s
        .customer_payments()
        .bounce(
            tid(PAY),
            &bounce,
            &PostingOptions::default(),
            IdempotencyKey::new(),
        )
        .await;
    assert_eq!(e.err(), Some(ApiError::Local(LocalValidation::Unsupported)));
    assert!(requests(&server, "POST").await.is_empty());
    // A fraction of a cent never goes out.
    let mut odd = save_from(&p);
    odd.amount = m("4000.001");
    assert_eq!(
        check_payment(&odd, &Capabilities::from_iter(ALL)),
        Err(ApiError::Local(LocalValidation::InvalidValue))
    );
}

#[tokio::test]
async fn bounce_sends_the_date_fee_and_version_and_reads_the_result() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!("/api/v1/payments/{PAY}/bounce")))
        .respond_with(json(
            201,
            &format!(
                r#"{{"returnId":"{RET}","feeId":null,"heldIn":{{"accountId":"{BANK}","kind":"bank"}},
                "reopened":[{{"invoiceId":"{INV1}","balanceDue":"4385.00"}}]}}"#
            ),
        ))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &ALL).await;
    let r = BounceRequest {
        date: CalendarDate::from_ymd(2026, 11, 12).expect("d"),
        bank_fee_amount: Some(m("35")),
        bank_fee_account_id: None,
        bank_fee_bank_account_id: None,
        memo: Some("NSF".into()),
        expected_updated_at: Timestamp::parse_wire("2026-11-04T08:10:00.000Z").expect("t"),
    };
    let out = s
        .customer_payments()
        .bounce(
            tid(PAY),
            &r,
            &PostingOptions::default(),
            IdempotencyKey::new(),
        )
        .await
        .expect("bounce");
    let req = &requests(&server, "POST").await[0];
    let b: Value = serde_json::from_slice(&req.body).expect("json");
    assert_eq!(
        b,
        json!({"date": "2026-11-12", "bankFeeAmount": "35.00", "memo": "NSF",
            "expectedUpdatedAt": "2026-11-04T08:10:00.000Z"})
    );
    assert_eq!(out.return_id, tid(RET));
    assert_eq!(out.held_in.kind, HeldKind::Bank);
    assert_eq!(out.reopened[0].balance_due, m("4385"));
}

#[tokio::test]
async fn refund_posts_a_customer_refund_linked_to_the_credit() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/customer-refunds"))
        .respond_with(json(201, &format!(r#"{{"transaction":{{"id":"{RET}"}}}}"#)))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &ALL).await;
    let r = NewCustomerRefund {
        customer_id: ContactId::parse(CLIENT).expect("id"),
        date: day(),
        from_account: AccountId::parse(BANK).expect("id"),
        amount: m("55"),
        source_payment: tid(PAY),
        method: Some("check".into()),
        ref_no: Some("4419".into()),
        memo: None,
    };
    let id = s
        .customer_payments()
        .refund(&r, &PostingOptions::default(), IdempotencyKey::new())
        .await
        .expect("refund");
    assert_eq!(id, tid(RET));
    let req = &requests(&server, "POST").await[0];
    // The canonical route is keyed (idempotency-receive-payment-v1).
    assert!(req.headers.get("idempotency-key").is_some());
    let b: Value = serde_json::from_slice(&req.body).expect("json");
    assert_eq!(
        b,
        json!({"txnDate": "2026-11-04", "contactId": CLIENT,
            "refundFromAccountId": BANK, "amount": "55.00", "sourcePaymentId": PAY,
            "paymentMethod": "check", "refNo": "4419"})
    );
    assert!(!format!("{r:?}").contains("4419"));
}

#[test]
fn a_record_reads_its_credits_accounts_and_later_rows() {
    let later = format!(
        r#","applications":[{{"invoiceId":"{INV1}","invoiceNumber":"1091","amount":"4000.00",
        "discount":"0.00","writeOff":"0.00","appliedDate":"2026-11-04","voided":false,"reversed":false}},
        {{"invoiceId":"{INV2}","invoiceNumber":"1095","amount":"20.00","appliedDate":"2026-11-04",
        "appliedInTxnId":"{RET}","appliedLater":true}}],"creditApplications":[{{"sourcePaymentId":"{RET}",
        "invoiceId":"{INV2}","invoiceNumber":"1095","amount":"15.00","fromDeposit":false}}],"writeOffAccountId":"{BANK}","discountAccountId":null"#
    );
    let body = record().replacen(
        &format!(
            r#","applications":[{{"invoiceId":"{INV1}","invoiceNumber":"1091","amount":"4000.00",
        "discount":"0.00","writeOff":"0.00","appliedDate":"2026-11-04","voided":false,"reversed":false}}]"#
        ),
        &later,
        1,
    );
    let rec = saved_payment(body.as_bytes()).expect("record");
    assert_eq!(rec.deposit_to, Some(AccountId::parse(UF).expect("id")));
    assert_eq!(rec.credit_applications.len(), 1);
    assert_eq!(rec.credit_applications[0].source_payment_id, tid(RET));
    assert_eq!(rec.credit_applications[0].amount, m("15"));
    assert_eq!(rec.credit_applications[0].invoice_number, "1095");
    assert!(!rec.credit_applications[0].from_deposit);
    assert_eq!(
        rec.write_off_account_id,
        Some(AccountId::parse(BANK).expect("id"))
    );
    assert_eq!(rec.discount_account_id, None);
    // The same-day credit used later by another receipt is not the payment's
    // own row.
    let own: Vec<bool> = rec
        .applications
        .iter()
        .map(|a| a.is_own(rec.id, rec.date))
        .collect();
    assert_eq!(own, [true, false]);
    // An older engine sends neither: nothing extra, every row its own.
    let old = saved_payment(record().as_bytes()).expect("record");
    assert!(old.credit_applications.is_empty() && old.write_off_account_id.is_none());
    assert!(old.applications.iter().all(|a| a.is_own(old.id, old.date)));
    // A payment of 0 that only wrote off has no deposit account.
    let zero = record().replace(&format!(r#""depositTo":"{UF}""#), r#""depositTo":null"#);
    assert_eq!(
        saved_payment(zero.as_bytes()).expect("record").deposit_to,
        None
    );
    assert!(!format!("{rec:?}").contains("15.00"));
}

#[test]
fn every_refusal_code_maps() {
    for code in payment_refusal_codes() {
        let e = ApiError::Conflict {
            reason: ConflictReason::Unspecified,
            current_version: None,
            code: ErrorCode::parse(code),
            message: UserText::default(),
        };
        assert!(payment_refusal(&e).is_some(), "{code}");
    }
    let stale = ApiError::Conflict {
        reason: ConflictReason::Stale,
        current_version: None,
        code: None,
        message: UserText::default(),
    };
    assert_eq!(payment_refusal(&stale), Some(PaymentRefusal::Stale));
    let e = crate::errors::engine_refusal(
        400,
        &json!({"error": {"message": "x", "code": "OVERAPPLIED", "details": {"invoiceId": INV1}}}),
    );
    assert_eq!(payment_refusal(&e), Some(PaymentRefusal::Overapplied));
    assert_eq!(payment_refusal(&ApiError::NoCompanyOpen), None);
}

#[test]
fn receivables_settings_need_their_capabilities() {
    use crate::resources::company::ReceivablesSettingsUpdate;
    let u = ReceivablesSettingsUpdate {
        write_off_threshold: Some(Some(m("1"))),
        nsf_customer_fee: Some(None),
        ..ReceivablesSettingsUpdate::default()
    };
    assert_eq!(
        u.body(&Capabilities::from_iter(ALL)).expect("body"),
        json!({"writeOffThreshold": "1.00", "nsfCustomerFee": null})
    );
    assert_eq!(
        u.body(&Capabilities::from_iter([Capabilities::PAYMENT_SETTLE])),
        Err(ApiError::Local(LocalValidation::Unsupported))
    );
    let neg = ReceivablesSettingsUpdate {
        write_off_threshold: Some(Some(m("-1"))),
        ..ReceivablesSettingsUpdate::default()
    };
    assert!(neg.body(&Capabilities::from_iter(ALL)).is_err());
    assert!(ReceivablesSettingsUpdate::default().is_empty());
}
