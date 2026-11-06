//! Unit tests of `inventory.rs` and of the P24 reader filters (wiremock;
//! exact wire strings).

use super::*;
use crate::resources::testkit::{company_session, json, query, requests, session_with_caps};
use crate::resources::transactions::{Transaction, is_inventory_derived};
use crate::resources::versioning::EditableFields;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer};

const ITEM: &str = "6b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const ITEM2: &str = "6b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a02";
const SHRINK: &str = "2d0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const ASSET: &str = "2e0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const COGS: &str = "2f0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const ADJ: &str = "4c0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";

fn m(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}
fn q(s: &str) -> Quantity {
    Quantity::parse_wire(s).expect("qty")
}
fn d(s: &str) -> CalendarDate {
    CalendarDate::parse_wire(s).expect("date")
}
fn item() -> ItemId {
    ItemId::parse(ITEM).expect("item")
}
fn account(s: &str) -> AccountId {
    AccountId::parse(s).expect("account")
}

const ALL: [&str; 3] = [
    Capabilities::INVENTORY_ADJUST,
    Capabilities::INVENTORY_REPORTS,
    Capabilities::REORDER_ALERTS,
];

fn adjustment() -> NewAdjustment {
    NewAdjustment {
        date: d("2026-01-20"),
        adjustment_account_id: account(SHRINK),
        memo: Some("Broken in the shop".into()),
        lines: vec![AdjustmentLine {
            item_id: item(),
            quantity_change: Some(q("-1")),
            value_change: None,
        }],
    }
}

fn record_json(extra: &str) -> String {
    format!(
        r#"{{"adjustment":{{"id":"{ADJ}","txnType":"inventory_adjustment","date":"2026-01-20",
        "adjustmentAccountId":"{SHRINK}","memo":"Broken in the shop","status":"posted",
        "updatedAt":"2026-01-20T10:00:00.000Z",
        "lines":[{{"itemId":"{ITEM}","quantityChange":"-1.0000","value":"-3.80"}}]}}{extra}}}"#
    )
}

#[test]
fn negative_stock_reads_flat_and_nested_and_never_guesses() {
    let body = serde_json::json!({
        "warnings": [
            {"code": "NEGATIVE_STOCK", "itemId": ITEM, "date": "2026-01-12", "onHand": "-3.0000",
             "message": "m"},
            {"code": "NEGATIVE_STOCK", "details": {"itemId": ITEM2, "date": "2026-01-15", "onHand": "-1"}},
            {"code": "NEGATIVE_STOCK", "itemId": ITEM, "date": "2026-01-10", "onHand": "-1.0000"},
            {"code": "NEGATIVE_STOCK", "itemId": ITEM, "date": "2026-01-12", "onHand": -3},
            {"code": "DOCUMENT_NUMBER_IN_USE", "txnNumber": "INV-7"},
            {"message": "no code"},
        ],
    });
    let all = problems_in(&body, "warnings");
    assert_eq!(all.len(), 5, "an entry without a code is skipped");
    assert_eq!(
        all[3],
        Problem::Other("NEGATIVE_STOCK".into()),
        "a JSON number is not guessed"
    );
    assert_eq!(all[4].code(), "DOCUMENT_NUMBER_IN_USE");
    let w = stock_warnings(&body);
    assert_eq!(w.len(), 2, "one per item");
    assert_eq!(w[0].item_id, item());
    assert_eq!(w[0].date, d("2026-01-10"), "the earliest date");
    assert_eq!(w[0].on_hand, q("-1"));
    assert_eq!(w[1].on_hand.to_wire(), "-1.0000");
    assert!(
        !format!("{:?}", w[0]).contains("on_hand"),
        "no quantity in Debug"
    );
    // A preview's problems[] read the same way.
    let preview = serde_json::json!({"problems": [
        {"code": "NEGATIVE_STOCK", "itemId": ITEM, "date": "2026-01-12", "onHand": "-3"}]});
    assert_eq!(stock_warnings(&preview).len(), 1);
    assert!(stock_warnings_from(b"not json").is_empty(), "unreadable");
    assert!(
        stock_warnings(&serde_json::json!({"warnings": null})).is_empty(),
        "null"
    );
    // Number warnings never pick up a stock warning (P24).
    let numbers = crate::resources::printing::number_warnings(&body);
    assert_eq!(numbers.len(), 1);
    assert_eq!(numbers[0].txn_number, "INV-7");
}

#[test]
fn adjustments_are_checked_before_sending() {
    assert!(adjustment().check().is_ok());
    let bad = |f: &dyn Fn(&mut NewAdjustment)| {
        let mut a = adjustment();
        f(&mut a);
        matches!(
            a.check(),
            Err(ApiError::Local(LocalValidation::InvalidValue))
        )
    };
    assert!(bad(&|a| a.lines.clear()));
    assert!(bad(&|a| a.lines[0].quantity_change = Some(Quantity::ZERO)));
    assert!(bad(&|a| a.lines[0].quantity_change = None));
    assert!(
        bad(&|a| a.lines[0].value_change = Some(m("1.005"))),
        "whole cents"
    );
    assert!(bad(&|a| a.lines[0].value_change = Some(Money::ZERO)));
    assert!(bad(&|a| {
        let l = a.lines[0].clone();
        a.lines.push(l);
    }));
    assert!(bad(&|a| a.memo = Some("x".repeat(ADJUSTMENT_MEMO_MAX + 1))));
    let mut both = adjustment();
    both.lines[0].value_change = Some(m("-2.50"));
    assert!(both.check().is_ok(), "a quantity at a given value");
    let body = serde_json::to_value(&both).expect("json");
    assert_eq!(
        body["lines"][0],
        serde_json::json!({"itemId": ITEM, "quantityChange": "-1.0000", "valueChange": "-2.50"})
    );
    assert!(!format!("{both:?}").contains("Broken"), "no memo in Debug");
    let count = NewCount {
        as_of: d("2026-01-31"),
        adjustment_account_id: account(SHRINK),
        memo: None,
        lines: vec![CountLine {
            item_id: item(),
            sheet_on_hand: q("4"),
            counted_quantity: q("3.5"),
        }],
    };
    assert!(count.check().is_ok());
    let mut neg = count.clone();
    neg.lines[0].counted_quantity = q("-1");
    assert!(neg.check().is_err());
    let mut empty = count;
    empty.lines.clear();
    assert!(empty.check().is_err());
}

#[test]
fn refusals_classify() {
    let conflict = |code: &str, details: Value| ApiError::Conflict {
        reason: crate::ConflictReason::Unspecified,
        current_version: None,
        code: crate::errors::ErrorCode::parse(code),
        message: crate::errors::UserText::new("m"),
        details: crate::ConflictDetails::new(details),
    };
    for (code, r) in REFUSALS {
        assert_eq!(
            inventory_refusal(&conflict(code, Value::Null)),
            Some(*r),
            "{code}"
        );
    }
    assert_eq!(inventory_refusal_codes().len(), REFUSALS.len());
    assert_eq!(inventory_refusal(&conflict("OTHER", Value::Null)), None);
    let stale = conflict(
        "STALE",
        serde_json::json!({"items": [{"itemId": ITEM, "onHand": "3"}, ITEM2, {"x": 1}]}),
    );
    assert_eq!(
        count_stale_items(&stale),
        Some(vec![item(), ItemId::parse(ITEM2).expect("id")])
    );
    assert_eq!(
        count_stale_items(&conflict("INVENTORY_PERIOD_CLOSED", Value::Null)),
        None
    );
}

#[tokio::test]
async fn every_route_needs_its_capability() {
    let server = MockServer::start().await;
    let s = company_session(&server);
    let inv = s.inventory();
    let unsupported = |e: ApiError| matches!(e, ApiError::Local(LocalValidation::Unsupported));
    let o = PostingOptions::default();
    assert!(unsupported(
        inv.create_adjustment(&adjustment(), &o, IdempotencyKey::new())
            .await
            .expect_err("adjust")
    ));
    assert!(unsupported(
        inv.count_sheet(d("2026-01-31")).await.expect_err("sheet")
    ));
    assert!(unsupported(
        inv.valuation(d("2026-01-31")).await.expect_err("report")
    ));
    assert!(unsupported(
        inv.stock_status(d("2026-01-31")).await.expect_err("status")
    ));
    assert!(unsupported(inv.reorder_alerts().await.expect_err("alerts")));
    assert!(unsupported(inv.reorder_report().await.expect_err("report")));
    assert!(
        server
            .received_requests()
            .await
            .expect("recorded")
            .is_empty(),
        "nothing sent"
    );
}

#[tokio::test]
async fn adjustments_round_trip_with_keys_warnings_and_the_stale_guard() {
    let server = MockServer::start().await;
    let warn = r#","warnings":[{"code":"NEGATIVE_STOCK","itemId":"6b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01","date":"2026-01-20","onHand":"-1.0000"}]"#;
    Mock::given(method("POST"))
        .and(path("/api/v1/inventory/adjustments"))
        .respond_with(json(201, &record_json(warn)))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/inventory/adjustments/{ADJ}")))
        .respond_with(json(200, &record_json("")))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path(format!("/api/v1/inventory/adjustments/{ADJ}")))
        .respond_with(json(200, &record_json("")))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("/api/v1/inventory/adjustments/{ADJ}/void")))
        .respond_with(json(200, &format!(r#"{{"ok":true{warn}}}"#)))
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path(format!("/api/v1/inventory/adjustments/{ADJ}")))
        .respond_with(json(200, r#"{"message":"Deleted"}"#))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &ALL).await;
    let inv = s.inventory();
    let o = PostingOptions::default();
    let saved = inv
        .create_adjustment(&adjustment(), &o, IdempotencyKey::new())
        .await
        .expect("create");
    assert_eq!(saved.adjustment.lines[0].value, Some(m("-3.80")));
    assert_eq!(saved.warnings.len(), 1);
    let post = &requests(&server, "POST").await[0];
    assert!(post.headers.get("idempotency-key").is_some());
    let body: Value = serde_json::from_slice(&post.body).expect("json");
    assert_eq!(body["adjustmentAccountId"], SHRINK);
    assert_eq!(body["lines"][0]["quantityChange"], "-1.0000");
    assert!(body.get("closingPassword").is_none());

    let id = saved.adjustment.id;
    let got = inv.adjustment(id).await.expect("get");
    let at = got.updated_at.expect("updated at");
    inv.update_adjustment(id, at, &adjustment(), &o, IdempotencyKey::new())
        .await
        .expect("edit");
    let put: Value = serde_json::from_slice(&requests(&server, "PUT").await[0].body).expect("json");
    assert_eq!(put["expectedUpdatedAt"], "2026-01-20T10:00:00.000Z");

    let w = inv
        .void_adjustment(id, &o, IdempotencyKey::new())
        .await
        .expect("void");
    assert_eq!(w.len(), 1, "a void answers its warnings (S6)");
    let w = inv
        .delete_adjustment(id, &o, IdempotencyKey::new())
        .await
        .expect("delete");
    assert!(w.is_empty(), "no warnings");
    // The closing-date password needs closing-password-v1 (nothing sent).
    let locked = PostingOptions {
        closed_period: Some(crate::ClosedPeriodAuthorization::with_password(
            cl_core::Secret::new("correct horse battery"),
        )),
    };
    let before = server.received_requests().await.expect("recorded").len();
    let e = inv
        .create_adjustment(&adjustment(), &locked, IdempotencyKey::new())
        .await
        .expect_err("no closing-password-v1");
    assert!(matches!(e, ApiError::Local(_)), "{e:?}");
    assert_eq!(
        server.received_requests().await.expect("recorded").len(),
        before
    );
}

#[tokio::test]
async fn counts_post_or_answer_stale_items() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/inventory/count-sheet"))
        .respond_with(json(
            200,
            &format!(
                r#"{{"countSheet":{{"asOf":"2026-01-31","rows":[
                {{"itemId":"{ITEM}","name":"Widget","unitLabel":"each","onHand":"4.0000",
                  "averageCost":"4.050000","value":"16.20"}}]}}}}"#
            ),
        ))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/inventory/counts"))
        .respond_with(json(201, &record_json("")))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/inventory/counts"))
        .respond_with(json(
            409,
            &format!(
                r#"{{"error":{{"message":"On hand changed","code":"STALE",
                "details":{{"items":[{{"itemId":"{ITEM}","onHand":"3.0000"}}]}}}}}}"#
            ),
        ))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &ALL).await;
    let inv = s.inventory();
    let sheet = inv.count_sheet(d("2026-01-31")).await.expect("sheet");
    assert_eq!(query(&requests(&server, "GET").await[1]), "asOf=2026-01-31");
    assert_eq!(sheet.rows[0].on_hand, q("4"));
    assert_eq!(
        sheet.rows[0].average_cost.map(UnitCost::to_wire).as_deref(),
        Some("4.050000")
    );
    let count = NewCount {
        as_of: sheet.as_of,
        adjustment_account_id: account(SHRINK),
        memo: None,
        lines: vec![CountLine {
            item_id: item(),
            sheet_on_hand: sheet.rows[0].on_hand,
            counted_quantity: q("3.5"),
        }],
    };
    let o = PostingOptions::default();
    let r = inv
        .post_count(&count, &o, IdempotencyKey::new())
        .await
        .expect("posted");
    assert!(r.adjustment.is_some());
    let body: Value =
        serde_json::from_slice(&requests(&server, "POST").await[0].body).expect("json");
    assert_eq!(
        body["lines"][0],
        serde_json::json!({"itemId": ITEM, "sheetOnHand": "4.0000", "countedQuantity": "3.5000"})
    );
    let e = inv
        .post_count(&count, &o, IdempotencyKey::new())
        .await
        .expect_err("stale");
    assert_eq!(count_stale_items(&e), Some(vec![item()]));
}

#[tokio::test]
async fn reports_and_reorder_alerts_read_engine_strings() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/reports/inventory-valuation"))
        .respond_with(json(
            200,
            &format!(
                r#"{{"report":{{"asOf":"2026-01-10","rows":[{{"itemId":"{ITEM}","assetAccountId":"{ASSET}",
                "onHand":"6.0000","averageCost":"3.135000","value":"18.81"}}],
                "accounts":[{{"accountId":"{ASSET}","valuation":"18.81","glBalance":"18.81","difference":"0.00"}}],
                "totalValue":"18.81","basisNote":"On cash basis, ..."}}}}"#
            ),
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/reports/inventory-valuation-detail"))
        .respond_with(json(
            200,
            &format!(
                r#"{{"rows":[{{"itemId":"{ITEM}","date":"2026-01-02","transactionId":"{ADJ}",
                "txnType":"bill","kind":"receipt","quantity":"10","value":"33.33",
                "runningQuantity":"10","runningValue":"33.33","averageCost":"3.333000"}}]}}"#
            ),
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/reports/inventory-stock-status"))
        .respond_with(json(
            200,
            &format!(r#"{{"rows":[{{"itemId":"{ITEM}","onHand":"-3","reorderPoint":"5"}}]}}"#),
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/items/reorder-alerts"))
        .respond_with(json(
            200,
            &format!(
                r#"{{"items":[{{"itemId":"{ITEM}","name":"Widget","onHand":"2","reorderPoint":"5","reorderQuantity":"20"}}]}}"#
            ),
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/reports/reorder"))
        .respond_with(json(200, r#"{"rows":[]}"#))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &ALL).await;
    let inv = s.inventory();
    let v = inv.valuation(d("2026-01-10")).await.expect("valuation");
    assert_eq!(v.rows[0].value, m("18.81"));
    assert_eq!(v.accounts[0].difference, Money::ZERO);
    assert_eq!(v.accounts[0].account_id, account(ASSET));
    assert!(v.basis_note.is_some());
    assert!(!format!("{:?}", v.accounts[0]).contains("18.81"));
    let detail = inv
        .valuation_detail(d("2026-01-01"), d("2026-01-31"), Some(item()))
        .await
        .expect("detail");
    assert_eq!(detail[0].running_value, m("33.33"));
    let detail_q = query(
        requests(&server, "GET")
            .await
            .iter()
            .find(|r| r.url.path().ends_with("valuation-detail"))
            .expect("detail request"),
    );
    assert_eq!(
        detail_q,
        format!("from=2026-01-01&to=2026-01-31&itemId={ITEM}")
    );
    assert!(matches!(
        inv.valuation_detail(d("2026-02-01"), d("2026-01-31"), None)
            .await,
        Err(ApiError::Local(LocalValidation::InvalidValue))
    ));
    let status = inv.stock_status(d("2026-01-31")).await.expect("status");
    assert!(status[0].on_hand.is_negative());
    let alerts = inv.reorder_alerts().await.expect("alerts");
    assert_eq!(alerts[0].reorder_quantity, Some(q("20")));
    assert!(
        inv.reorder_report().await.expect("report").is_empty(),
        "an empty report"
    );
}

// ---- Readers (plan §2.3 B3, §6.3) ----------------------------------------------

const TXN: &str = "4b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const INCOME: &str = "2a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const AR: &str = "2b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const BANK: &str = "2c0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";

fn line(n: u8, account: &str, debit: &str, credit: &str, extra: &str) -> String {
    format!(
        r#"{{"id":"9a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a0{n}","accountId":"{account}","debit":"{debit}",
        "credit":"{credit}","lineOrder":{n}{extra}}}"#
    )
}

/// An invoice whose `lines` carry a leaked COGS pair: the record keeps the
/// sale line only; `inventoryLines[]` are read outside `lines`.
#[test]
fn an_invoice_never_shows_engine_inventory_lines() {
    let lines = [
        line(0, AR, "40.0000", "0.0000", ""),
        line(
            1,
            INCOME,
            "0.0000",
            "40.0000",
            &format!(r#","itemId":"{ITEM}","quantity":"4.0000","unitPrice":"10.0000""#),
        ),
        line(2, COGS, "14.3500", "0.0000", r#","lineRole":"cogs""#),
        line(
            3,
            ASSET,
            "0.0000",
            "14.3500",
            r#","lineRole":"inventory_relief""#,
        ),
    ]
    .join(",");
    let body = format!(
        r#"{{"id":"{TXN}","txnType":"invoice","txnDate":"2026-01-08","status":"posted",
        "total":"40.0000","lines":[{lines}],
        "inventoryLines":[{{"role":"cogs","accountId":"{COGS}","amount":"14.35","movementId":"m1"}},
                          {{"role":"inventory_relief","accountId":"{ASSET}","amount":"-14.35"}}]}}"#
    );
    let r: crate::InvoiceRecord = serde_json::from_str(&body).expect("invoice");
    assert_eq!(r.lines.len(), 1, "the sale line only");
    assert_eq!(r.lines[0].amount, m("40"));
    assert_eq!(r.txn.inventory_lines.len(), 2);
    assert_eq!(r.txn.inventory_lines[1].amount, m("-14.35"));
    assert!(r.txn.has_inventory_lines());
    let fp = r.editable();
    assert_eq!(fp["lines"].as_array().map(Vec::len), Some(1));
    assert!(!format!("{:?}", r.txn.inventory_lines[0]).contains("14.35"));
    // The ledger fingerprint skips them too: the same transaction without
    // the leaked pair hashes the same.
    let clean: Transaction = serde_json::from_str(&body.replace(
        &format!(
            ",{},{}",
            line(2, COGS, "14.3500", "0.0000", r#","lineRole":"cogs""#),
            line(
                3,
                ASSET,
                "0.0000",
                "14.3500",
                r#","lineRole":"inventory_relief""#
            )
        ),
        "",
    ))
    .expect("clean");
    let leaked: Transaction = serde_json::from_str(&body).expect("leaked");
    assert_eq!(clean.lines.len(), 2);
    assert_eq!(clean.fingerprint(), leaked.fingerprint());
    assert_eq!(leaked.document_only().lines.len(), 2);
}

/// A bill whose correction leaked into `lines` keeps only its item line.
#[test]
fn a_bill_never_shows_a_cost_correction() {
    let ap = "2a1c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
    let body = format!(
        r#"{{"id":"{TXN}","txnType":"bill","txnDate":"2026-01-15","status":"posted",
        "total":"38.0000","lines":[{},{},{},{}]}}"#,
        line(0, ap, "0.0000", "38.0000", ""),
        line(
            1,
            ASSET,
            "38.0000",
            "0.0000",
            &format!(r#","itemId":"{ITEM}","quantity":"10","unitPrice":"3.80""#)
        ),
        line(
            2,
            ASSET,
            "0.0000",
            "0.6300",
            r#","lineRole":"cost_correction""#
        ),
        line(
            3,
            COGS,
            "0.6300",
            "0.0000",
            r#","lineRole":"cost_correction""#
        ),
    );
    let r: crate::BillRecord = serde_json::from_str(&body).expect("bill");
    assert_eq!(r.lines.len(), 1);
    assert_eq!(r.lines[0].amount, m("38"));
    assert_eq!(r.lines[0].item, Some(item()));
}

/// A check (Write Checks) with a correction: its record keeps the bank and
/// item lines.
#[test]
fn a_check_record_drops_engine_inventory_lines() {
    let body = format!(
        r#"{{"id":"{TXN}","txnType":"expense","txnDate":"2026-01-15","status":"posted",
        "isCheck":true,"printStatus":"hand_written","checkNumber":4418,
        "lines":[{},{},{}],
        "inventoryLines":[{{"role":"cost_correction","accountId":"{COGS}","amount":"0.63"}}]}}"#,
        line(0, BANK, "0.0000", "38.0000", ""),
        line(1, ASSET, "38.0000", "0.0000", ""),
        line(
            2,
            COGS,
            "0.6300",
            "0.0000",
            r#","lineRole":"cost_correction""#
        ),
    );
    let r: crate::CheckRecord = serde_json::from_str(&body).expect("check");
    assert_eq!(r.txn.lines.len(), 2);
    assert!(r.txn.lines.iter().all(|l| !l.is_inventory_derived()));
    assert_eq!(r.txn.inventory_lines.len(), 1);
    assert!(is_inventory_derived(Some("cost_residual")));
    assert!(!is_inventory_derived(Some("cash_back")));
    assert!(!is_inventory_derived(None));
}

/// Credit memo and sales receipt records keep document lines only.
#[test]
fn sales_documents_drop_leaked_inventory_lines() {
    let v = serde_json::json!([
        {"accountId": INCOME, "itemId": ITEM, "quantity": "1", "unitPrice": "10.00", "amount": "10.00"},
        {"accountId": COGS, "amount": "3.80", "lineRole": "cogs"},
    ]);
    let lines = crate::resources::credit_memos::doc_lines(v).expect("lines");
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].amount, m("10"));
}

#[test]
fn the_new_transaction_type_reads() {
    let t: TxnType = serde_json::from_str("\"inventory_adjustment\"").expect("type");
    assert_eq!(t, TxnType::InventoryAdjustment);
    let txn: Transaction = serde_json::from_value(serde_json::json!({
        "id": TXN, "txnType": "inventory_adjustment", "txnDate": "2026-01-11",
        "status": "posted", "lines": [],
    }))
    .expect("adjustment");
    assert!(!txn.has_inventory_lines());
}
