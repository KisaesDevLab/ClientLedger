//! Reconcile against the documented contract (plan §2): exact string
//! amounts, a JSON number refused, the bodies Begin, PATCH and the ticks
//! send, the summary view, the closing password and the named refusals.

use super::*;
use crate::resources::testkit::{json, query, requests, session_with_caps};
use cl_core::Secret;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer};

const REC: &str = "5a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const ACCT: &str = "2a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const EXP: &str = "2b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const INC: &str = "2c0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const JL: &str = "6a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const TXN: &str = "7a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";

const CAPS: [&str; 4] = [
    Capabilities::RECONCILE_EXACT,
    Capabilities::RECONCILE_BEGIN,
    Capabilities::RECONCILE_UNDO,
    Capabilities::RECONCILE_DISCREPANCY,
];

fn m(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}

fn day(d: u32) -> CalendarDate {
    CalendarDate::from_ymd(2026, 10, d).expect("date")
}

fn header(lines: &str) -> String {
    format!(
        r#"{{"reconciliation":{{
        "id":"{REC}","accountId":"{ACCT}","status":"in_progress",
        "statementDate":"2026-10-31","beginningBalance":"71436.2000",
        "statementEndingBalance":"86902.1300","clearedBalance":"87314.3100",
        "difference":"-412.1800",
        "clearedDeposits":{{"count":6,"total":"40107.8400"}},
        "clearedPayments":{{"count":8,"total":"-24229.7300"}},
        "serviceChargeAmount":"48.0000","serviceChargeDate":"2026-10-31",
        "serviceChargeAccountId":"{EXP}","serviceChargeTxnId":null,
        "interestAmount":"12.8400","interestDate":"2026-10-31",
        "interestAccountId":"{INC}","interestTxnId":null,
        "includeLaterLines":true,"beginningBalanceDrift":null,
        "continuityWarning":null,"postingLocked":false,
        "createdAt":"2026-11-04T15:00:00.000Z"{lines}}}}}"#
    )
}

fn line_json(amount: &str) -> String {
    format!(
        r#","lines":[{{"id":"x","journal_line_id":"{JL}","transaction_id":"{TXN}",
        "is_cleared":false,"txn_date":"2026-10-27","txn_type":"check",
        "check_number":4409,"txn_number":null,"contact_name":"Monett Tire & Lube",
        "amount":{amount},"afterStatementDate":false,"fromBankFeed":false,
        "likely_duplicate":false}}]"#
    )
}

#[test]
fn parses_exact_strings_and_lines() {
    let r = reconciliation_from(header(&line_json("\"-412.1800\"")).as_bytes()).expect("parsed");
    assert_eq!(r.difference, m("-412.18"));
    assert_eq!(r.cleared_balance, m("87314.31"));
    assert_eq!(r.cleared_deposits.count, 6);
    assert_eq!(r.cleared_payments.total, m("-24229.73"));
    assert!(!r.ties_out());
    let svc = r.service_charge.expect("charge");
    assert_eq!(svc.charge.amount, m("48"));
    assert!(svc.posted.is_none());
    assert_eq!(r.interest.expect("interest").charge.amount, m("12.84"));
    let lines = r.lines.expect("lines");
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].number.as_deref(), Some("4409"));
    assert_eq!(lines[0].amount, m("-412.18"));
    assert_eq!(lines[0].date, day(27));
    assert!(r.started.is_some());
}

#[test]
fn a_json_number_amount_is_a_decode_error() {
    let e = reconciliation_from(header(&line_json("-412.18")).as_bytes()).expect_err("number");
    assert!(matches!(e, ApiError::Decode { .. }), "{e:?}");
    let body = header("").replace("\"-412.1800\"", "-412.18");
    let e = reconciliation_from(body.as_bytes()).expect_err("number");
    assert!(matches!(e, ApiError::Decode { .. }), "{e:?}");
}

#[test]
fn exact_fields_win_over_legacy_numbers() {
    // The engine keeps JSON numbers under the old names for older clients
    // and sends the exact figures as `*Exact` strings (U-188, R2).
    let body = header("")
        .replace(
            "\"clearedBalance\":\"87314.3100\"",
            "\"clearedBalance\":87314.31,\"clearedBalanceExact\":\"87314.3100\"",
        )
        .replace(
            "\"difference\":\"-412.1800\"",
            "\"difference\":-412.18,\"differenceExact\":\"-412.1800\"",
        )
        .replace(
            "\"continuityWarning\":null",
            "\"continuityWarning\":{\"expected\":1.5,\"actual\":2,\"delta\":0.5}",
        );
    let r = reconciliation_from(body.as_bytes()).expect("parsed");
    assert_eq!(r.difference, m("-412.18"));
    assert_eq!(r.cleared_balance, m("87314.31"));
    assert_eq!(r.continuity, None, "an inexact warning is left out");
}

#[test]
fn a_summary_answer_has_no_lines() {
    let r = reconciliation_from(header("").as_bytes()).expect("parsed");
    assert!(r.lines.is_none());
}

#[test]
fn start_body_sends_typed_figures_as_strings() {
    let s = ReconStart {
        account: AccountId::parse(ACCT).expect("id"),
        statement_date: day(31),
        statement_ending_balance: m("-86902.1"),
        service_charge: Some(StatementCharge {
            amount: m("48"),
            date: day(31),
            account: AccountId::parse(EXP).expect("id"),
        }),
        interest: None,
    };
    let b = start_body(&s).expect("body");
    assert_eq!(b["statementEndingBalance"], "-86902.10");
    assert_eq!(b["serviceCharge"]["amount"], "48.00");
    assert_eq!(b["serviceCharge"]["date"], "2026-10-31");
    assert!(b.get("interest").is_none());
    assert_eq!(b["includeLaterLines"], true);
    assert_eq!(b["precheck"], "bank_feed");
    // A charge must be above zero with at most 2 decimals.
    let mut bad = s.clone();
    bad.service_charge = Some(StatementCharge {
        amount: m("0"),
        ..s.service_charge.expect("c")
    });
    assert!(start_body(&bad).is_err());
    bad.service_charge = Some(StatementCharge {
        amount: m("1.005"),
        ..s.service_charge.expect("c")
    });
    assert!(start_body(&bad).is_err());
    let mut bad = s;
    bad.statement_ending_balance = m("1.001");
    assert!(start_body(&bad).is_err());
}

#[test]
fn patch_body_removes_and_refuses_empty() {
    assert!(patch_body(&ReconPatch::default()).is_err());
    let b = patch_body(&ReconPatch {
        interest: Some(None),
        statement_date: Some(day(30)),
        ..ReconPatch::default()
    })
    .expect("body");
    assert!(b["interest"].is_null());
    assert_eq!(b["statementDate"], "2026-10-30");
    assert!(b.get("serviceCharge").is_none());
}

#[test]
fn lines_body_bounds() {
    let id = JournalLineId::parse(JL).expect("id");
    assert!(lines_body(&[]).is_err());
    let many = vec![(id, true); LINES_MAX + 1];
    assert!(lines_body(&many).is_err());
    let b = lines_body(&[(id, true)]).expect("body");
    assert_eq!(b["lines"][0]["isCleared"], true);
}

#[test]
fn refusals_are_named_by_code() {
    let conflict = |code: &str, details: Value| ApiError::Conflict {
        reason: crate::errors::ConflictReason::Unspecified,
        current_version: None,
        code: crate::errors::ErrorCode::parse(code),
        message: crate::errors::UserText::default(),
        details: crate::errors::ConflictDetails::new(details),
    };
    assert_eq!(
        reconcile_refusal(&conflict(
            "RECONCILIATION_IN_PROGRESS",
            json!({"reconciliationId": REC})
        )),
        Some(ReconcileRefusal::InProgress {
            reconciliation: ReconciliationId::parse(REC).ok()
        })
    );
    assert_eq!(
        reconcile_refusal(&conflict("RECONCILIATION_NOT_LATEST", json!({}))),
        Some(ReconcileRefusal::NotLatest)
    );
    assert_eq!(
        reconcile_refusal(&conflict("RECONCILE_STATEMENT_DATE_ORDER", json!({}))),
        Some(ReconcileRefusal::StatementDateOrder)
    );
    assert_eq!(
        reconcile_refusal(&ApiError::Forbidden {
            code: crate::errors::ErrorCode::parse("RECONCILE_ADMIN_REQUIRED")
        }),
        Some(ReconcileRefusal::AdminRequired)
    );
    assert_eq!(reconcile_refusal(&ApiError::Forbidden { code: None }), None);
    assert_eq!(reconcile_refusal(&conflict("STALE", json!({}))), None);
}

#[test]
fn discrepancy_reads_field_names_and_exact_effects() {
    let body = format!(
        r#"{{"rows":[{{"kind":"changed","reconciliationId":"{REC}",
        "statementDate":"2026-09-30","transactionId":"{TXN}","txnDate":"2026-09-12",
        "txnType":"expense","number":null,"payee":"Ozark Feed & Seed",
        "reconciledAmount":"-250.0000","effect":"0.0000",
        "changedAt":"2026-11-04T15:00:00.000Z","changedBy":"Kurt",
        "fields":["accountId"]}}],"drift":null}}"#
    );
    let r = discrepancy_from(body.as_bytes()).expect("parsed");
    assert_eq!(r.rows.len(), 1);
    assert_eq!(r.rows[0].fields, vec!["accountId".to_string()]);
    assert_eq!(r.rows[0].effect, Some(m("0")));
    let bad = body.replace("\"0.0000\"", "0");
    assert!(discrepancy_from(bad.as_bytes()).is_err());
}

#[test]
fn report_amounts_stay_text() {
    let body = r#"{"title":"Reconciliation Detail","reconciliation":{"id":"x",
        "accountName":"Operating Checking","accountNumber":"1010",
        "statementDate":"2026-10-31","beginningBalance":71436.2,
        "statementEndingBalance":86902.13,"clearedBalance":86902.13,"difference":0},
        "cleared":[{"txnDate":"2026-10-03","txnType":"deposit","txnNumber":null,
        "description":"Branson Hills HOA","payment":null,"deposit":8450}],
        "uncleared":[],"totals":{"clearedPayments":24277.73,"clearedDeposits":40120.68,
        "unclearedPayments":0,"unclearedDeposits":0}}"#;
    let r = report_from(body.as_bytes()).expect("parsed");
    assert_eq!(r.account_name, "Operating Checking");
    assert_eq!(
        r.beginning_balance.as_ref().and_then(|a| a.display(2)),
        Some("71436.20".into())
    );
    assert_eq!(r.cleared.len(), 1);
    assert_eq!(
        r.cleared[0].deposit.as_ref().map(ReportAmount::text),
        Some("8450")
    );
}

#[tokio::test]
async fn needs_the_capabilities() {
    let server = MockServer::start().await;
    let s = session_with_caps(&server, &[]).await;
    let e = s
        .reconciliations()
        .get(ReconciliationId::parse(REC).expect("id"))
        .await
        .expect_err("refused");
    assert_eq!(e, ApiError::Local(LocalValidation::Unsupported));
    let e = s
        .reconciliations()
        .discrepancy(AccountId::parse(ACCT).expect("id"))
        .await
        .expect_err("refused");
    assert_eq!(e, ApiError::Local(LocalValidation::Unsupported));
}

#[tokio::test]
async fn ticks_ask_for_the_summary_view() {
    let server = MockServer::start().await;
    let s = session_with_caps(&server, &CAPS).await;
    Mock::given(method("PUT"))
        .and(path(format!("/api/v1/banking/reconciliations/{REC}/lines")))
        .respond_with(json(200, &header("")))
        .mount(&server)
        .await;
    let r = s
        .reconciliations()
        .tick(
            ReconciliationId::parse(REC).expect("id"),
            &[(JournalLineId::parse(JL).expect("id"), true)],
        )
        .await
        .expect("ticked");
    assert!(r.lines.is_none());
    let sent = requests(&server, "PUT").await;
    assert_eq!(query(&sent[0]), "view=summary");
}

#[tokio::test]
async fn complete_and_undo_carry_the_password_and_mode() {
    let server = MockServer::start().await;
    let s = session_with_caps(&server, &CAPS).await;
    for tail in ["complete", "undo"] {
        Mock::given(method("POST"))
            .and(path(format!(
                "/api/v1/banking/reconciliations/{REC}/{tail}"
            )))
            .respond_with(json(200, r#"{"message":"ok"}"#))
            .mount(&server)
            .await;
    }
    let id = ReconciliationId::parse(REC).expect("id");
    let auth = ClosedPeriodAuthorization::with_password(Secret::new("pw"));
    s.reconciliations()
        .complete(id, Some(&auth))
        .await
        .expect("complete");
    s.reconciliations()
        .undo(id, UndoMode::KeepChecks, None)
        .await
        .expect("undo");
    let sent = requests(&server, "POST").await;
    let first: Value = serde_json::from_slice(&sent[0].body).expect("json");
    assert_eq!(first["closingPassword"], "pw");
    let second: Value = serde_json::from_slice(&sent[1].body).expect("json");
    assert_eq!(second["mode"], "keep_checks");
    assert!(second.get("closingPassword").is_none());
}

#[tokio::test]
async fn history_reads_rows() {
    let server = MockServer::start().await;
    let s = session_with_caps(&server, &CAPS).await;
    Mock::given(method("GET"))
        .and(path("/api/v1/banking/reconciliations"))
        .respond_with(json(
            200,
            &format!(
                r#"{{"reconciliations":[{{"id":"{REC}","status":"complete",
                "statementDate":"2026-09-30","statementEndingBalance":"71436.2000",
                "completedAt":"2026-10-04T15:00:00.000Z"}}]}}"#
            ),
        ))
        .mount(&server)
        .await;
    let rows = s
        .reconciliations()
        .history(AccountId::parse(ACCT).expect("id"))
        .await
        .expect("rows");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].status, ReconStatus::Complete);
    assert_eq!(rows[0].statement_ending_balance, Some(m("71436.2")));
}
