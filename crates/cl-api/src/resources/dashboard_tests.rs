//! Stub tests of the dashboard reads: exact strings only, a JSON number is
//! refused, the gate sends nothing, and every call carries `exact=1` and
//! the local `asOf`.

use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer};

use super::*;
use crate::resources::testkit::{
    company_session, json as reply, query, requests, session_with_caps,
};

const ACC1: &str = "3a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const ACC2: &str = "3a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a02";

fn d(y: i32, m: u32, dd: u32) -> CalendarDate {
    CalendarDate::from_ymd(y, m, dd).expect("date")
}

fn m(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}

fn cash() -> String {
    format!(
        r#"{{"bankAccounts":[{{"id":"{ACC1}","name":"Operating Checking","accountNumber":"1010","balance":"84210.5500"}}],
        "creditCards":[{{"id":"{ACC2}","name":"Business Card","accountNumber":null,"balance":"-6388.2100"}}],
        "totalBank":"84210.5500","totalCC":"6388.2100"}}"#
    )
}

const RECEIVABLES: &str = r#"{"totalOutstanding":"48912.4000","overdueCount":2,"overdueAmount":"12345.6700","invoiceCount":9}"#;

const PAYABLES: &str = r#"{"totalOwed":"9000.0000","billCount":5,"overdueCount":2,"overdueAmount":"1528.1000",
    "dueThisWeekCount":1,"dueThisWeekAmount":"7471.9000","dueWithinDays":14,"creditCount":0,
    "creditAmount":"0.0000","apBalance":"9000.0000"}"#;

const ACTIONS: &str = r#"{"pendingFeedCount":37,"overdueInvoiceCount":2,
    "staleReconciliations":[{"accountName":"Operating Checking","lastReconciled":"2026-09-30T05:00:00.000Z"}],
    "pendingDepositCount":1,"pendingDepositAmount":"250.0000","printQueueCount":0,"printQueueAmount":"0.0000"}"#;

fn summary() -> String {
    format!(
        r#"{{"asOf":"2026-11-04","snapshot":{{"mtd":{{"revenue":1.5,"expenses":2,"netIncome":-0.5}}}},
        "trend":{{"data":[{{"month":"Nov 26","revenue":0.1,"expenses":0.2}}]}},
        "cashPosition":{},"receivables":{RECEIVABLES},"payables":null,"actionItems":{ACTIONS},
        "budgetPerformance":{{"mtd":{{"revenueBudget":10.25}}}},"bankingHealth":null,"portalActivity":null,
        "workQueue":null,"errors":["Payables"]}}"#,
        cash()
    )
}

#[test]
fn every_panel_reads_exact_strings() {
    let cash_pos = cash_position_from_json(cash().as_bytes()).expect("cash");
    assert_eq!(cash_pos.bank_accounts[0].balance, m("84210.55"));
    assert_eq!(
        cash_pos.bank_accounts[0].account_number.as_deref(),
        Some("1010")
    );
    assert_eq!(cash_pos.credit_cards[0].balance.to_wire(), "-6388.2100");
    assert_eq!(cash_pos.credit_cards[0].account_number, None);
    assert_eq!(cash_pos.total_bank, m("84210.55"));
    assert_eq!(cash_pos.total_cards, m("6388.21"));
    let recv = receivables_from_json(RECEIVABLES.as_bytes()).expect("receivables");
    assert_eq!(recv.overdue_amount, m("12345.67"));
    assert_eq!((recv.overdue_count, recv.invoice_count), (2, 9));
    let pay = payables_from_json(PAYABLES.as_bytes()).expect("payables");
    assert_eq!(pay.due_soon_amount, m("7471.90"));
    assert_eq!((pay.due_soon_count, pay.due_within_days), (1, 14));
    assert_eq!(pay.ap_balance, m("9000"));
    let acts = action_items_from_json(ACTIONS.as_bytes()).expect("actions");
    assert_eq!(acts.pending_feed_count, 37);
    assert_eq!(
        acts.stale_reconciliations[0].account_name,
        "Operating Checking"
    );
    assert_eq!(acts.pending_deposit_amount, m("250"));
    let sum = summary_from_json(summary().as_bytes()).expect("summary");
    assert_eq!(sum.as_of, d(2026, 11, 4));
    assert_eq!(sum.cash_position.as_ref(), Some(&cash_pos));
    assert_eq!(sum.receivables.as_ref(), Some(&recv));
    assert_eq!(sum.payables, None, "a failed panel is None");
    assert_eq!(sum.errors, ["Payables"]);
    assert_eq!(sum.action_items.as_ref(), Some(&acts));
    // Debug never prints an amount or a name.
    let dbg = format!("{sum:?} {pay:?}");
    for leak in [
        "84210",
        "6388",
        "12345",
        "7471",
        "Operating",
        "Business",
        "250",
    ] {
        assert!(!dbg.contains(leak), "{leak} in {dbg}");
    }
}

#[test]
fn a_json_number_in_any_money_field_is_refused() {
    let money_fields: [(&str, &str); 4] = [
        ("cash", "\"84210.5500\""),
        ("receivables", "\"12345.6700\""),
        ("payables", "\"7471.9000\""),
        ("actions", "\"250.0000\""),
    ];
    for (which, field) in money_fields {
        let number = field.trim_matches('"');
        let body = match which {
            "cash" => cash().replacen(field, number, 1),
            "receivables" => RECEIVABLES.replacen(field, number, 1),
            "payables" => PAYABLES.replacen(field, number, 1),
            _ => ACTIONS.replacen(field, number, 1),
        };
        let err = match which {
            "cash" => cash_position_from_json(body.as_bytes()).err(),
            "receivables" => receivables_from_json(body.as_bytes()).err(),
            "payables" => payables_from_json(body.as_bytes()).err(),
            _ => action_items_from_json(body.as_bytes()).err(),
        };
        assert!(
            matches!(err, Some(ApiError::Decode { .. })),
            "{which}: a number must be refused"
        );
    }
    // The pre-U-76 shape (every amount a float) never decodes.
    let old = r#"{"bankAccounts":[],"creditCards":[],"totalBank":0,"totalCC":0}"#;
    assert!(cash_position_from_json(old.as_bytes()).is_err());
    // Inside the summary too.
    let bad = summary().replacen("\"48912.4000\"", "48912.4", 1);
    assert!(summary_from_json(bad.as_bytes()).is_err());
}

#[tokio::test]
async fn nothing_is_sent_without_the_capability() {
    let server = MockServer::start().await;
    let s = company_session(&server);
    let today = d(2026, 11, 4);
    let gated = ApiError::Local(LocalValidation::DashboardUnavailable);
    assert!(!s.dashboard().exact());
    assert_eq!(
        s.dashboard().cash_position(today).await.err(),
        Some(gated.clone())
    );
    assert_eq!(
        s.dashboard().receivables(today).await.err(),
        Some(gated.clone())
    );
    assert_eq!(
        s.dashboard().payables(today, 7).await.err(),
        Some(gated.clone())
    );
    assert_eq!(
        s.dashboard().action_items(today).await.err(),
        Some(gated.clone())
    );
    assert_eq!(s.dashboard().summary(today, 7).await.err(), Some(gated));
    assert!(requests(&server, "GET").await.is_empty(), "nothing sent");
}

#[tokio::test]
async fn every_call_sends_exact_and_the_local_date() {
    let server = MockServer::start().await;
    for (route, body) in [
        ("/api/v1/dashboard/cash-position", cash()),
        ("/api/v1/dashboard/receivables", RECEIVABLES.to_string()),
        ("/api/v1/dashboard/payables", PAYABLES.to_string()),
        ("/api/v1/dashboard/action-items", ACTIONS.to_string()),
        ("/api/v1/dashboard/summary", summary()),
    ] {
        Mock::given(method("GET"))
            .and(path(route))
            .respond_with(reply(200, &body))
            .mount(&server)
            .await;
    }
    let s = session_with_caps(&server, &[Capabilities::DASHBOARD_EXACT]).await;
    let dash = s.dashboard();
    assert!(dash.exact());
    let today = d(2026, 11, 4);
    dash.cash_position(today).await.expect("cash");
    dash.receivables(today).await.expect("receivables");
    assert_eq!(
        dash.payables(today, 14)
            .await
            .expect("payables")
            .due_within_days,
        14
    );
    dash.action_items(today).await.expect("actions");
    dash.summary(today, 7).await.expect("summary");
    for days in [0, 91] {
        assert_eq!(
            dash.payables(today, days).await.err(),
            Some(ApiError::Local(LocalValidation::DueWindow))
        );
        assert_eq!(
            dash.summary(today, days).await.err(),
            Some(ApiError::Local(LocalValidation::DueWindow))
        );
    }
    let asked: Vec<(String, String)> = requests(&server, "GET")
        .await
        .iter()
        .filter(|r| r.url.path().starts_with("/api/v1/dashboard"))
        .map(|r| (r.url.path().to_string(), query(r)))
        .collect();
    let q = "exact=1&asOf=2026-11-04";
    assert_eq!(
        asked,
        [
            ("/api/v1/dashboard/cash-position".to_string(), q.to_string()),
            ("/api/v1/dashboard/receivables".into(), q.into()),
            (
                "/api/v1/dashboard/payables".into(),
                format!("{q}&dueWithinDays=14")
            ),
            ("/api/v1/dashboard/action-items".into(), q.into()),
            (
                "/api/v1/dashboard/summary".into(),
                format!("{q}&dueWithinDays=7")
            ),
        ]
    );
}

#[tokio::test]
async fn a_number_from_the_engine_is_a_decode_error_not_a_value() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/dashboard/receivables"))
        .respond_with(reply(
            200,
            r#"{"totalOutstanding":48912.4,"overdueCount":2,"overdueAmount":12345.67,"invoiceCount":9}"#,
        ))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &[Capabilities::DASHBOARD_EXACT]).await;
    assert!(matches!(
        s.dashboard().receivables(d(2026, 11, 4)).await,
        Err(ApiError::Decode { .. })
    ));
}
