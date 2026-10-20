//! Bank postings: local refusals, exact string bodies, the fingerprint the
//! engine's record will have, and the closing-date password path.

use super::*;
use crate::errors::ConflictReason;
use crate::resources::posting::ClosedPeriodAuthorization;
use crate::resources::testkit::{company_session, json, requests, session_with_caps};
use crate::resources::transactions::{DeleteOptions, TxnStatus};
use crate::resources::versioning::EditableFields;
use crate::wire::Capabilities;
use cl_core::Secret;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer};

const BANK: &str = "2a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const EXP: &str = "2b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const EXP2: &str = "2c0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const PAYEE: &str = "4a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const TAG: &str = "3a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const TXN: &str = "7a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";

fn m(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}

fn a(s: &str) -> AccountId {
    AccountId::parse(s).expect("id")
}

fn day() -> CalendarDate {
    CalendarDate::from_ymd(2026, 9, 14).expect("date")
}

fn split(account: &str, amount: &str, memo: Option<&str>) -> SplitLine {
    SplitLine {
        account: a(account),
        amount: m(amount),
        memo: memo.map(str::to_string),
        contact: None,
        tag: None,
    }
}

fn check_draft() -> CheckDraft {
    CheckDraft {
        bank: a(BANK),
        date: day(),
        payee: Some(ContactId::parse(PAYEE).expect("id")),
        payee_name: "  Sample Supply Co ".into(),
        check_number: Some(1042),
        memo: Some("September".into()),
        amount: m("1084.6612"),
        lines: vec![
            SplitLine {
                tag: Some(TagId::parse(TAG).expect("tag")),
                ..split(EXP, "1000.0012", Some("Paper"))
            },
            split(EXP2, "84.66", None),
        ],
    }
}

fn card_draft() -> CardChargeDraft {
    CardChargeDraft {
        pay_from: a(BANK),
        date: day(),
        number: Some(" R-77 ".into()),
        payee: None,
        memo: Some("Fuel".into()),
        lines: vec![
            split(EXP, "40.0000", None),
            split(EXP2, "2.5", Some("Snacks")),
        ],
    }
}

fn deposit_draft() -> DepositDraft {
    DepositDraft {
        account: a(BANK),
        date: day(),
        memo: None,
        lines: vec![
            SplitLine {
                contact: Some(ContactId::parse(PAYEE).expect("id")),
                ..split(EXP, "1100", Some("Invoice 7"))
            },
            split(EXP2, "0.0001", Some("")),
        ],
    }
}

fn transfer_draft() -> TransferDraft {
    TransferDraft {
        from: a(BANK),
        to: a(EXP),
        date: day(),
        amount: m("250"),
        memo: Some("To savings".into()),
    }
}

#[test]
fn bad_drafts_are_refused_locally() {
    use LocalValidation as L;
    let mut d = check_draft();
    d.amount = m("1084.6611");
    assert_eq!(BankPosting::check(d), Err(L::TotalMismatch));
    let mut d = check_draft();
    d.lines.clear();
    assert_eq!(BankPosting::check(d), Err(L::TooFewLines));
    let mut d = check_draft();
    d.lines[1].amount = Money::ZERO;
    assert_eq!(BankPosting::check(d), Err(L::NonPositiveAmount));
    let mut d = check_draft();
    d.lines[0].account = a(BANK);
    assert_eq!(BankPosting::check(d), Err(L::SameAccount));
    let mut d = check_draft();
    d.payee_name = "   ".into();
    assert_eq!(BankPosting::check(d), Err(L::InvalidValue));
    let mut d = check_draft();
    d.check_number = Some(0);
    assert_eq!(BankPosting::check(d), Err(L::InvalidValue));
    let mut d = check_draft();
    d.lines[0].contact = Some(ContactId::parse(PAYEE).expect("id"));
    assert_eq!(BankPosting::check(d), Err(L::InvalidValue));
    let mut d = check_draft();
    d.lines = vec![split(EXP, "1", None); MAX_LINES + 1];
    assert_eq!(BankPosting::check(d), Err(L::TooManyLines));

    let mut d = transfer_draft();
    d.to = d.from;
    assert_eq!(BankPosting::transfer(d), Err(L::SameAccount));
    let mut d = transfer_draft();
    d.amount = m("-1");
    assert_eq!(BankPosting::transfer(d), Err(L::NonPositiveAmount));

    let mut d = deposit_draft();
    d.lines[0].amount = m("-5");
    assert_eq!(BankPosting::deposit(d), Err(L::NonPositiveAmount));
    let mut d = card_draft();
    d.lines[0].contact = Some(ContactId::parse(PAYEE).expect("id"));
    assert_eq!(BankPosting::card_charge(d), Err(L::InvalidValue));
    let mut d = card_draft();
    d.number = Some("x".repeat(51));
    assert_eq!(BankPosting::card_charge(d), Err(L::InvalidValue));
    let mut d = card_draft();
    d.lines = vec![split(EXP, "999999999999999.9999", None); 2];
    assert_eq!(BankPosting::card_charge(d), Err(L::InvalidValue));
}

#[test]
fn bodies_carry_exact_strings_and_explicit_tags() {
    let check = BankPosting::check(check_draft()).expect("check");
    assert_eq!(check.kind(), BankPostingKind::Check);
    let v = check.create_value();
    assert_eq!(v["amount"], "1084.6612");
    assert_eq!(v["payeeNameOnCheck"], "Sample Supply Co");
    assert_eq!(v["checkNumber"], 1042);
    assert_eq!(v["printLater"], false);
    assert_eq!(v["lines"][0]["amount"], "1000.0012");
    assert_eq!(v["lines"][0]["tagId"], TAG);
    assert_eq!(v["lines"][0]["description"], "Paper");
    assert_eq!(v["lines"][1]["amount"], "84.6600");
    assert_eq!(v["lines"][1]["tagId"], Value::Null, "tag sent as null");
    assert!(v["lines"][1].get("description").is_none());
    // On update a check is an expense; its number is left alone.
    let u = check.update_value();
    assert_eq!(u["txnType"], "expense");
    assert_eq!(u["payFromAccountId"], BANK);
    assert_eq!(u["lines"][0]["expenseAccountId"], EXP);
    assert!(u.get("txnNumber").is_none() && u.get("checkNumber").is_none());

    let card = BankPosting::card_charge(card_draft()).expect("card");
    let v = card.create_value();
    assert_eq!(v["txnNumber"], "R-77");
    assert_eq!(v["lines"][1]["amount"], "2.5000");
    let dep = BankPosting::deposit(deposit_draft()).expect("deposit");
    let v = dep.create_value();
    assert_eq!(v["txnType"], "deposit");
    assert_eq!(v["depositToAccountId"], BANK);
    assert_eq!(v["lines"][0]["contactId"], PAYEE);
    assert_eq!(v["lines"][1]["contactId"], Value::Null);
    assert!(
        v["lines"][1].get("description").is_none(),
        "empty memo dropped"
    );
    let tr = BankPosting::transfer(transfer_draft()).expect("transfer");
    let v = tr.create_value();
    assert_eq!(v["amount"], "250.0000");
    assert_eq!(
        (v["fromAccountId"].as_str(), v["toAccountId"].as_str()),
        (Some(BANK), Some(EXP))
    );
    // No money anywhere is a JSON number.
    for p in [&check, &card, &dep, &tr] {
        for body in [p.create_value(), p.update_value()] {
            let text = body.to_string();
            assert!(!text.contains("\"amount\":1"), "{text}");
            // NN8: no way past a closed period but the password (U-15).
            for bad in ["overrideConfirmed", "lockDate", "closingPassword"] {
                assert!(!text.contains(bad), "{bad} in {text}");
            }
        }
    }
}

/// The record the engine stores, as `GET /transactions/{id}` shows it.
fn stored(
    txn_type: &str,
    contact: Option<&str>,
    memo: Option<&str>,
    lines: &[Value],
) -> Transaction {
    let v = serde_json::json!({
        "id": TXN,
        "txnType": txn_type,
        "txnDate": "2026-09-14",
        "status": "posted",
        "basis": "both",
        "contactId": contact,
        "memo": memo,
        "checkNumber": 1042,
        "lines": lines,
    });
    serde_json::from_value(v).expect("transaction")
}

fn sl(
    order: i64,
    account: &str,
    debit: &str,
    credit: &str,
    description: Option<&str>,
    tag: Option<&str>,
    contact: Option<&str>,
) -> Value {
    serde_json::json!({
        "id": format!("8a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a{order:02}"),
        "accountId": account, "debit": debit, "credit": credit,
        "description": description, "lineOrder": order, "tagId": tag, "contactId": contact,
    })
}

#[test]
fn fingerprints_match_what_the_engine_stores() {
    // Check: split lines first (descriptions as typed), bank credit last.
    let check = BankPosting::check(check_draft()).expect("check");
    let rec = stored(
        "expense",
        Some(PAYEE),
        Some("September"),
        &[
            sl(
                0,
                EXP,
                "1000.0012",
                "0.0000",
                Some("Paper"),
                Some(TAG),
                None,
            ),
            sl(1, EXP2, "84.6600", "0.0000", None, None, None),
            sl(2, BANK, "0.0000", "1084.6612", None, None, None),
        ],
    );
    assert_eq!(rec.fingerprint(), check.created_fingerprint());
    // Updated through the expense path: an empty description takes the memo.
    let rec = stored(
        "expense",
        Some(PAYEE),
        Some("September"),
        &[
            sl(
                0,
                EXP,
                "1000.0012",
                "0.0000",
                Some("Paper"),
                Some(TAG),
                None,
            ),
            sl(1, EXP2, "84.6600", "0.0000", Some("September"), None, None),
            sl(2, BANK, "0.0000", "1084.6612", None, None, None),
        ],
    );
    assert_eq!(rec.fingerprint(), check.updated_fingerprint());

    let card = BankPosting::card_charge(card_draft()).expect("card");
    let rec = stored(
        "expense",
        None,
        Some("Fuel"),
        &[
            sl(0, EXP, "40.0000", "0.0000", Some("Fuel"), None, None),
            sl(1, EXP2, "2.5000", "0.0000", Some("Snacks"), None, None),
            sl(2, BANK, "0.0000", "42.5000", None, None, None),
        ],
    );
    assert_eq!(rec.fingerprint(), card.created_fingerprint());
    assert_eq!(card.created_fingerprint(), card.updated_fingerprint());

    let dep = BankPosting::deposit(deposit_draft()).expect("deposit");
    let rec = stored(
        "deposit",
        None,
        None,
        &[
            sl(0, BANK, "1100.0001", "0.0000", None, None, None),
            sl(
                1,
                EXP,
                "0.0000",
                "1100.0000",
                Some("Invoice 7"),
                None,
                Some(PAYEE),
            ),
            sl(2, EXP2, "0.0000", "0.0001", None, None, None),
        ],
    );
    assert_eq!(rec.fingerprint(), dep.created_fingerprint());

    let tr = BankPosting::transfer(transfer_draft()).expect("transfer");
    let rec = stored(
        "transfer",
        None,
        Some("To savings"),
        &[
            sl(0, EXP, "250.0000", "0.0000", None, None, None),
            sl(1, BANK, "0.0000", "250.0000", None, None, None),
        ],
    );
    assert_eq!(rec.fingerprint(), tr.created_fingerprint());
    assert_ne!(tr.created_fingerprint(), dep.created_fingerprint());
}

fn check_answer(key: &str) -> String {
    let rec = serde_json::json!({
        "id": TXN, "txnType": "expense", "txnDate": "2026-09-14", "status": "posted",
        "basis": "both", "contactId": PAYEE, "memo": "September", "checkNumber": 1042,
        "lines": [
            sl(0, EXP, "1000.0012", "0.0000", Some("Paper"), Some(TAG), None),
            sl(1, EXP2, "84.6600", "0.0000", None, None, None),
            sl(2, BANK, "0.0000", "1084.6612", None, None, None),
        ],
    });
    serde_json::json!({ key: rec }).to_string()
}

#[tokio::test]
async fn a_check_goes_to_checks_and_reads_back_exactly() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/checks"))
        .respond_with(json(201, &check_answer("check")))
        .mount(&server)
        .await;
    let s = company_session(&server);
    let check = BankPosting::check(check_draft()).expect("check");
    let made = s
        .transactions()
        .create_bank_posting(&check, &PostingOptions::default(), IdempotencyKey::new())
        .await
        .expect("created");
    assert_eq!(made.record.check_number, Some(1042));
    assert_eq!(
        made.version,
        RecordVersion::Fingerprint(check.created_fingerprint())
    );
    let posts = requests(&server, "POST").await;
    assert_eq!(posts.len(), 1);
    let body: Value = serde_json::from_slice(&posts[0].body).expect("json");
    assert_eq!(body["amount"], "1084.6612");
    assert!(body.get("closingPassword").is_none());
    assert!(posts[0].headers.get("idempotency-key").is_some());
}

#[tokio::test]
async fn the_closing_password_is_sent_only_with_the_capability() {
    let opts = PostingOptions {
        closed_period: Some(ClosedPeriodAuthorization::with_password(Secret::new(
            "pw-canary-123",
        ))),
    };
    let tr = BankPosting::transfer(transfer_draft()).expect("transfer");

    let server = MockServer::start().await;
    let s = session_with_caps(&server, &[]).await;
    let e = s
        .transactions()
        .create_bank_posting(&tr, &opts, IdempotencyKey::new())
        .await
        .expect_err("gated");
    assert_eq!(
        e,
        ApiError::Local(LocalValidation::ClosedPeriodOverrideUnavailable)
    );
    assert!(requests(&server, "POST").await.is_empty(), "nothing sent");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/transactions"))
        .respond_with(json(
            403,
            r#"{"error":{"message":"wrong","code":"CLOSING_PASSWORD_INVALID"}}"#,
        ))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &[Capabilities::CLOSING_PASSWORD]).await;
    let e = s
        .transactions()
        .create_bank_posting(&tr, &opts, IdempotencyKey::new())
        .await
        .expect_err("wrong password");
    assert_eq!(e, ApiError::ClosingPasswordRejected);
    let posts = requests(&server, "POST").await;
    assert_eq!(posts.len(), 1, "a refused password is not retried");
    let body: Value = serde_json::from_slice(&posts[0].body).expect("json");
    assert_eq!(body["closingPassword"], "pw-canary-123");
    assert!(!format!("{e} {e:?} {opts:?}").contains("canary"));
}

#[tokio::test]
async fn a_stale_update_sends_nothing() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/transactions/{TXN}").as_str()))
        .respond_with(json(200, &check_answer("transaction")))
        .mount(&server)
        .await;
    let s = company_session(&server);
    let check = BankPosting::check(check_draft()).expect("check");
    let e = s
        .transactions()
        .update_bank_posting(
            TransactionId::parse(TXN).expect("id"),
            &RecordVersion::Fingerprint([7; 32]),
            &check,
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
    assert!(requests(&server, "PUT").await.is_empty());
}

fn deleted_answer() -> String {
    check_answer("transaction").replace("\"posted\"", "\"deleted\"")
}

#[tokio::test]
async fn delete_is_gated_by_its_capability_and_the_password_by_its_own() {
    let id = TransactionId::parse(TXN).expect("id");
    let server = MockServer::start().await;
    let s = session_with_caps(&server, &[]).await;
    let e = s
        .transactions()
        .delete(id, &DeleteOptions::default(), IdempotencyKey::new())
        .await
        .expect_err("gated");
    assert_eq!(e, ApiError::Local(LocalValidation::TxnDeleteUnavailable));
    assert!(requests(&server, "DELETE").await.is_empty(), "nothing sent");
    let server = MockServer::start().await;
    let s = session_with_caps(&server, &[Capabilities::TXN_DELETE]).await;
    let with_pw = DeleteOptions {
        reason: None,
        closing_password: Some(Secret::new("pw-canary-123")),
    };
    let e = s
        .transactions()
        .delete(id, &with_pw, IdempotencyKey::new())
        .await
        .expect_err("password gated");
    assert_eq!(
        e,
        ApiError::Local(LocalValidation::ClosedPeriodOverrideUnavailable)
    );
    let long = DeleteOptions {
        reason: Some("x".repeat(501)),
        closing_password: None,
    };
    let e = s
        .transactions()
        .delete(id, &long, IdempotencyKey::new())
        .await
        .expect_err("long reason");
    assert_eq!(e, ApiError::Local(LocalValidation::InvalidValue));
    assert!(requests(&server, "DELETE").await.is_empty(), "nothing sent");
    assert!(!format!("{with_pw:?}").contains("canary"));
}

#[tokio::test]
async fn delete_sends_the_reason_and_password_and_reads_the_deleted_record() {
    let id = TransactionId::parse(TXN).expect("id");
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path(format!("/api/v1/transactions/{TXN}").as_str()))
        .respond_with(json(200, &deleted_answer()))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path(format!("/api/v1/transactions/{TXN}").as_str()))
        .respond_with(json(
            409,
            r#"{"error":{"message":"cannot delete invoices","code":"TXN_DELETE_UNSUPPORTED"}}"#,
        ))
        .mount(&server)
        .await;
    let s = session_with_caps(
        &server,
        &[Capabilities::TXN_DELETE, Capabilities::CLOSING_PASSWORD],
    )
    .await;
    let opts = DeleteOptions {
        reason: Some("  Entered twice ".into()),
        closing_password: Some(Secret::new("pw-canary-123")),
    };
    let gone = s
        .transactions()
        .delete(id, &opts, IdempotencyKey::new())
        .await
        .expect("deleted");
    assert_eq!(gone.record.status, TxnStatus::Deleted);
    assert_eq!(gone.record.lines.len(), 3, "the lines come back");
    let dels = requests(&server, "DELETE").await;
    assert_eq!(dels.len(), 1);
    let body: Value = serde_json::from_slice(&dels[0].body).expect("json");
    assert_eq!(body["reason"], "Entered twice");
    assert_eq!(body["closingPassword"], "pw-canary-123");
    assert!(body.get("overrideConfirmed").is_none());
    assert!(dels[0].headers.get("idempotency-key").is_some());

    let e = s
        .transactions()
        .delete(id, &DeleteOptions::default(), IdempotencyKey::new())
        .await
        .expect_err("unsupported");
    assert!(matches!(
        e,
        ApiError::Conflict {
            reason: ConflictReason::DeleteUnsupported,
            ..
        }
    ));
    let dels = requests(&server, "DELETE").await;
    let body: Value = serde_json::from_slice(&dels[1].body).expect("json");
    assert_eq!(body, serde_json::json!({}), "no reason, no password");
}

#[test]
fn deletable_kinds_follow_q245() {
    use crate::resources::transactions::TxnType as T;
    for t in [
        T::JournalEntry,
        T::Expense,
        T::Transfer,
        T::Deposit,
        T::CashSale,
    ] {
        assert!(t.is_deletable(), "{t:?}");
    }
    for t in [
        T::Invoice,
        T::CustomerPayment,
        T::Bill,
        T::BillPayment,
        T::CreditMemo,
        T::VendorCredit,
        T::CustomerRefund,
        T::DailySales,
        T::Aje,
    ] {
        assert!(!t.is_deletable(), "{t:?}");
    }
}
