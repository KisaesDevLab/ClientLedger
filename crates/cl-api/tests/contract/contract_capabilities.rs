//! Capability probes (Q-220): each asserts the engine's *current* behaviour
//! for its capability state, deterministically, and prints `PENDING(U-xx)`
//! while the capability is not advertised. When a U-item merges and the
//! engine advertises the capability, the other branch runs by itself;
//! `CL_API_REQUIRE_CAPS` then makes its absence a failure.

use cl_api::{Capabilities, Method, PageRequest, PostingOptions, TransactionFilter, TxnType};
use serde_json::{Value, json};

use crate::common::{date, je2, key, pair};
use crate::support::{Harness, unique};

const JE_LINE_CONTACT: &str = "je-line-contact-v1";
const EXACT_MONEY: &str = "exact-money-v1";

/// U-21: the engine's balance check answers 400 without a code until
/// `error-codes-v1` adds `TXN_UNBALANCED`.
pub fn error_codes_u21(h: &Harness) {
    let s = &h.s1;
    let (a, b) = pair(h, s);
    let unbalanced = json!({
        "txnType": "journal_entry",
        "txnDate": "2026-07-04",
        "lines": [
            {"accountId": a.id, "debit": "11.0000", "credit": "0"},
            {"accountId": b.id, "debit": "0", "credit": "10.0000"},
        ],
    });
    let raw = h
        .run(s.raw(Method::Post, "/api/v1/transactions", &[], Some(&unbalanced)))
        .expect("raw");
    assert_eq!(raw.status, 400);
    if h.require_cap(Capabilities::ERROR_CODES, "U-21") {
        assert_eq!(raw.body["error"]["code"], "TXN_UNBALANCED");
    } else {
        assert!(raw.body["error"]["code"].is_null(), "unexpected code");
    }
}

/// U-18: without `idempotency-v1` the engine ignores `Idempotency-Key`, so
/// the same key sent twice posts twice (which is why cl-api never re-sends
/// a mutation blindly). With it, the second send is a replay.
pub fn idempotency_u18(h: &Harness) {
    let s = &h.s1;
    let (cash, supplies) = pair(h, s);
    let memo = unique("same key");
    let on = date(2026, 7, 5);
    let entry = je2(on, &memo, supplies.id, cash.id, "1.2300");
    let k = key();
    for _ in 0..2 {
        h.run(
            s.transactions()
                .create_journal_entry(&entry, &PostingOptions::default(), k.clone()),
        )
        .expect("post");
    }
    let filter = TransactionFilter {
        txn_type: Some(TxnType::JournalEntry),
        start_date: Some(on),
        end_date: Some(on),
        search: Some(memo),
        ..TransactionFilter::default()
    };
    let page = h
        .run(s.transactions().list(&filter, PageRequest::first_max()))
        .expect("list");
    if h.require_cap(Capabilities::IDEMPOTENCY, "U-18") {
        assert_eq!(page.data.len(), 1, "same key must replay");
    } else {
        assert_eq!(
            page.data.len(),
            2,
            "the engine is expected to ignore the key"
        );
    }
}

/// U-22: journal-line `contactId` is stripped by the engine's schema until
/// `je-line-contact-v1`.
pub fn je_line_contact_u22(h: &Harness) {
    let s = &h.s1;
    let (cash, supplies) = pair(h, s);
    let contact = h
        .run(s.contacts().create(
            &cl_api::NewContact {
                fields: cl_api::ContactFields::named(cl_api::ContactType::Vendor, unique("Line")),
                opening_balance: cl_core::Money::ZERO,
                opening_balance_date: None,
            },
            key(),
        ))
        .expect("contact")
        .record;
    let body = json!({
        "txnType": "journal_entry",
        "txnDate": "2026-07-06",
        "lines": [
            {"accountId": supplies.id, "debit": "2.0000", "credit": "0", "contactId": contact.id},
            {"accountId": cash.id, "debit": "0", "credit": "2.0000"},
        ],
    });
    let raw = h
        .run(s.raw(Method::Post, "/api/v1/transactions", &[], Some(&body)))
        .expect("raw");
    assert_eq!(raw.status, 201);
    let id = raw.body["transaction"]["id"]
        .as_str()
        .and_then(|v| cl_core::ids::TransactionId::parse(v).ok())
        .expect("id");
    let got = h.run(s.transactions().get(id)).expect("get").record;
    let with_contact = got.lines.iter().filter(|l| l.contact_id.is_some()).count();
    if h.require_cap(JE_LINE_CONTACT, "U-22") {
        assert_eq!(with_contact, 1);
    } else {
        assert_eq!(with_contact, 0, "the engine is expected to drop contactId");
    }
}

fn has_number(v: &Value) -> bool {
    match v {
        Value::Number(_) => true,
        Value::Array(a) => a.iter().any(has_number),
        Value::Object(o) => o.values().any(has_number),
        _ => false,
    }
}

/// U-14: reports still carry JSON numbers (float sums) until
/// `exact-money-v1`; cl-api reads them only as display-only `ReportAmount`.
pub fn report_numbers_u14(h: &Harness) {
    let s = &h.s1;
    let raw = h
        .run(s.raw(
            Method::Get,
            "/api/v1/reports/profit-loss",
            &[("start_date", "2026-01-01"), ("end_date", "2026-12-31")],
            None,
        ))
        .expect("raw");
    assert_eq!(raw.status, 200);
    if !h.require_cap(EXACT_MONEY, "U-14") {
        assert!(has_number(&raw.body), "expected JSON-number report amounts");
    }
}
