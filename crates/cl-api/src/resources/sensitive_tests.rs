//! Sensitive accounts (P29, U-33): the flag on accounts, the Admin's
//! set-sensitive call, the typed 403 refusals, `hiddenNotice` on reports,
//! `hiddenSplit` on registers and hidden references on saved reports.

use crate::IdempotencyKey;
use crate::errors::{
    ApiError, SensitiveRefusal, from_response_for_tests, is_sensitive_not_allowed,
};
use crate::resources::testkit::{json, requests, session_with_caps};
use crate::resources::versioning::EditableFields;
use crate::wire::Capabilities;
use cl_core::OwnerFacing;
use cl_core::ids::AccountId;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer};

const ACCT: &str = "2a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const REGISTER: &str = include_str!("../../../../fixtures/engine/register-exact.json");

fn body(code: &str, message: &str) -> String {
    format!(r#"{{"error":{{"message":"{message}","code":"{code}"}}}}"#)
}

/// P29 review: placing a bank or card account under a sensitive one (422)
/// and the owner-only audit export (403) read in plain words.
#[test]
fn not_allowed_placement_and_owner_only_refusals_have_plain_words() {
    let e = from_response_for_tests(
        422,
        body("SENSITIVE_NOT_ALLOWED", "Not under a sensitive account").as_bytes(),
    );
    assert!(is_sensitive_not_allowed(&e), "{e:?}");
    let m = e.owner_message();
    assert_eq!(m.key, "error.api.sensitive_not_allowed");
    assert_eq!(m.suggestion_key, Some("fix.api.sensitive_not_allowed"));
    let e = from_response_for_tests(403, body("OWNER_REQUIRED", "Owner only").as_bytes());
    let m = e.owner_message();
    assert_eq!(m.key, "error.api.owner_required");
    assert_eq!(m.suggestion_key, Some("fix.api.owner_required"));
}

#[test]
fn the_two_403_codes_are_typed_refusals_with_plain_words() {
    let e = from_response_for_tests(
        403,
        body(
            "SENSITIVE_ACCOUNT",
            "This transaction uses an account hidden for your role",
        )
        .as_bytes(),
    );
    assert_eq!(e, ApiError::Sensitive(SensitiveRefusal::Transaction));
    let m = e.owner_message();
    assert_eq!(m.key, "error.api.sensitive_transaction");
    assert_eq!(m.suggestion_key, Some("fix.api.ask_admin"));
    // Never an unknown save: nothing to check again.
    assert!(!e.needs_recheck());

    let e = from_response_for_tests(
        403,
        body(
            "SENSITIVE_HIDDEN",
            "This needs access to sensitive accounts",
        )
        .as_bytes(),
    );
    assert_eq!(e, ApiError::Sensitive(SensitiveRefusal::Report));
    assert_eq!(e.owner_message().key, "error.api.sensitive_report");

    // Any other 403 stays a rights refusal.
    let e = from_response_for_tests(403, body("PERMISSION_DENIED", "No").as_bytes());
    assert!(matches!(e, ApiError::Forbidden { .. }));
}

#[test]
fn a_refused_mark_is_recognised_by_its_code() {
    let e = from_response_for_tests(
        422,
        body(
            "SENSITIVE_NOT_ALLOWED",
            "A bank account can not be marked sensitive",
        )
        .as_bytes(),
    );
    assert!(is_sensitive_not_allowed(&e));
    let other = from_response_for_tests(422, body("VALIDATION_ERROR", "x").as_bytes());
    assert!(!is_sensitive_not_allowed(&other));
    assert!(!is_sensitive_not_allowed(&ApiError::Timeout));
}

#[test]
fn a_saved_report_naming_a_hidden_account_is_refused_by_name() {
    use crate::resources::reports::definitions::{DefinitionRefusal, definition_refusal};
    let e = from_response_for_tests(
        403,
        body(
            "SENSITIVE_DEFINITION",
            "This saved report uses an account hidden",
        )
        .as_bytes(),
    );
    assert_eq!(definition_refusal(&e), Some(DefinitionRefusal::Hidden));
}

#[test]
fn accounts_carry_the_flag_and_default_to_not_sensitive() {
    let base = format!(
        r#"{{"id":"{ACCT}","name":"Payroll Wages","accountType":"expense","isActive":true"#
    );
    let a: crate::Account = serde_json::from_str(&format!("{base}}}")).expect("old engine");
    assert!(!a.is_sensitive);
    let a: crate::Account =
        serde_json::from_str(&format!(r#"{base},"isSensitive":null}}"#)).expect("null");
    assert!(!a.is_sensitive);
    let a: crate::Account =
        serde_json::from_str(&format!(r#"{base},"isSensitive":true}}"#)).expect("marked");
    assert!(a.is_sensitive);
    // An account save never sends the flag (only the flag route sets it).
    assert!(!crate::Account::EDITABLE.contains(&"isSensitive"));
}

#[tokio::test]
async fn set_sensitive_needs_the_capability_and_puts_only_the_flag() {
    let server = MockServer::start().await;
    let id = AccountId::parse(ACCT).expect("id");
    // Without `sensitive-accounts-v1`: nothing is sent.
    let s = session_with_caps(&server, &[]).await;
    let e = s
        .accounts()
        .set_sensitive(id, true, IdempotencyKey::new())
        .await
        .expect_err("refused locally");
    assert!(matches!(e, ApiError::Local(_)));
    assert!(requests(&server, "PUT").await.is_empty());

    let server = MockServer::start().await;
    let route = format!("/api/v1/accounts/{ACCT}/sensitive");
    Mock::given(method("PUT"))
        .and(path(route.as_str()))
        .respond_with(json(
            200,
            &format!(
                r#"{{"account":{{"id":"{ACCT}","name":"Payroll Wages","accountType":"expense","isActive":true,"isSensitive":true}}}}"#
            ),
        ))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &[Capabilities::ACCOUNT_SENSITIVE]).await;
    let a = s
        .accounts()
        .set_sensitive(id, true, IdempotencyKey::new())
        .await
        .expect("marked");
    assert!(a.is_sensitive);
    let put = requests(&server, "PUT").await;
    assert_eq!(put.len(), 1);
    let sent: serde_json::Value = serde_json::from_slice(&put[0].body).expect("json");
    assert_eq!(sent, serde_json::json!({"isSensitive": true}));
}

#[tokio::test]
async fn a_refused_mark_comes_back_as_the_plain_validation_refusal() {
    let server = MockServer::start().await;
    let route = format!("/api/v1/accounts/{ACCT}/sensitive");
    Mock::given(method("PUT"))
        .and(path(route.as_str()))
        .respond_with(json(
            422,
            &body(
                "SENSITIVE_NOT_ALLOWED",
                "A bank account can not be marked sensitive",
            ),
        ))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &[Capabilities::ACCOUNT_SENSITIVE]).await;
    let e = s
        .accounts()
        .set_sensitive(
            AccountId::parse(ACCT).expect("id"),
            true,
            IdempotencyKey::new(),
        )
        .await
        .expect_err("refused");
    assert!(is_sensitive_not_allowed(&e), "{e:?}");
}

#[test]
fn a_register_row_with_a_hidden_other_side_reads_hidden() {
    use crate::resources::register::{Category, page_from_json};
    let mut v: serde_json::Value = serde_json::from_str(REGISTER).expect("fixture");
    let line = &mut v["lines"][0];
    line["categoryName"] = serde_json::Value::Null;
    line["categoryAccountId"] = serde_json::Value::Null;
    line["hiddenSplit"] = serde_json::Value::Bool(true);
    let page = page_from_json(v.to_string().as_bytes()).expect("page");
    assert_eq!(page.lines[0].category, Category::Hidden);
    // The row keeps its own amounts and balance exactly as sent.
    assert!(page.lines[0].payment.is_some());
    assert!(page.exact);
    // Without the key the other rows read as before.
    let plain = page_from_json(REGISTER.as_bytes()).expect("page");
    assert!(plain.lines.iter().all(|l| l.category != Category::Hidden));
}

#[test]
fn every_report_answer_may_carry_the_notice() {
    use crate::resources::reports::adapters::hidden_notice;
    assert!(hidden_notice(&serde_json::json!({"hiddenNotice": true})));
    assert!(!hidden_notice(&serde_json::json!({"hiddenNotice": null})));
    assert!(!hidden_notice(&serde_json::json!({})));
    assert!(!hidden_notice(&serde_json::json!({"hiddenNotice": "true"})));
}

#[test]
fn a_saved_report_says_where_it_names_hidden_accounts() {
    use crate::resources::reports::definitions::DefinitionRow;
    let row = r#"{"id":"d1","name":"Wages watch","kind":"builder","definition":{},"version":2,
        "canEdit":false,"hiddenRefs":true,
        "hidden":[{"path":"definition.nodes.0.filters.accountIds","nodeId":"n1","field":"accountIds","kind":"account","ids":["2a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01"]}]}"#;
    let r: DefinitionRow = serde_json::from_str(row).expect("row");
    assert!(r.hidden_refs && !r.can_edit);
    assert_eq!(r.hidden.len(), 1);
    assert_eq!(r.hidden[0].kind, "account");
    // Debug never prints the ids.
    assert!(!format!("{:?}", r.hidden[0]).contains("2a0c4e8e"));
    let plain: DefinitionRow = serde_json::from_str(
        r#"{"id":"d1","name":"x","kind":"builder","definition":{},"version":2}"#,
    )
    .expect("row");
    assert!(!plain.hidden_refs && plain.hidden.is_empty());
}
