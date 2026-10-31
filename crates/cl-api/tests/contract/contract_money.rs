//! Money is exact over the wire (AC2): strings in, the same strings out, the
//! boundaries of numeric(19,4), local refusal of what the engine would round
//! or tolerate, and a raw-JSON check that money fields are strings.

use std::time::Duration;

use cl_api::{
    AccountType, Basis, Capabilities, ContactFields, ContactType, ItemFields, LocalValidation,
    Method, NewContact, NewJournalEntry, PostingOptions, Side,
};
use cl_core::Money;
use serde_json::json;

use crate::common::{account, date, je2, key, line, m, pair, post, wire_lines};
use crate::support::{Harness, unique};

/// A tiny deterministic generator (no float, no external crate).
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 11
    }
}

pub fn seeded_amounts_round_trip_string_equal(h: &Harness) {
    let s = &h.s1;
    let (cash, supplies) = pair(h, s);
    let mut rng = Lcg(0x00C1_1ED6_E405);
    h.run_for(Duration::from_mins(2), async {
        for i in 0..64u32 {
            // 1 .. 10^12 ten-thousandths (up to 100,000,000.0000).
            let units = i128::from(rng.next() % 1_000_000_000_000) + 1;
            let amount = Money::from_ten_thousandths(units).expect("amount");
            let wire = amount.to_wire();
            let entry = je2(
                date(2026, 5, 1 + i % 28),
                &format!("seeded {i}"),
                supplies.id,
                cash.id,
                &wire,
            );
            let made = s
                .transactions()
                .create_journal_entry(&entry, &PostingOptions::default(), key())
                .await
                .expect("post");
            let got = s.transactions().get(made.record.id).await.expect("get");
            assert_eq!(
                wire_lines(&got.record),
                vec![
                    (wire.clone(), "0.0000".to_string()),
                    ("0.0000".to_string(), wire.clone()),
                ],
                "entry {i}"
            );
        }
    });
}

pub fn boundaries_round_trip(h: &Harness) {
    // Tenant B: the maximum amount would make Contract Co's balances huge.
    let s = h.s3();
    let (cash, supplies) = pair(h, &s);
    for amount in ["0.0001", "999999999999999.9999"] {
        let made = post(
            h,
            &s,
            &je2(
                date(2026, 6, 1),
                &unique("boundary"),
                supplies.id,
                cash.id,
                amount,
            ),
        );
        let got = h.run(s.transactions().get(made.record.id)).expect("get");
        assert_eq!(
            wire_lines(&got.record),
            vec![
                (amount.to_string(), "0.0000".to_string()),
                ("0.0000".to_string(), amount.to_string()),
            ]
        );
        h.run(s.transactions().void(
            made.record.id,
            "boundary test",
            &PostingOptions::default(),
            key(),
        ))
        .expect("void");
    }
    let new = NewContact {
        fields: ContactFields::named(ContactType::Customer, unique("Opening")),
        opening_balance: m("1234.5678"),
        opening_balance_date: Some(date(2026, 1, 2)),
    };
    let c = h.run(s.contacts().create(&new, key())).expect("contact");
    let got = h.run(s.contacts().get(c.record.id)).expect("get");
    assert_eq!(
        got.record.opening_balance.map(Money::to_wire).as_deref(),
        Some("1234.5678")
    );
    let income = account(h, &s, AccountType::Revenue, "Sales");
    let item = ItemFields {
        name: unique("Max item"),
        description: None,
        unit_price: Some(m("999999999999999.9999")),
        income_account_id: income.id,
        is_taxable: false,
        item_type: cl_api::ItemType::Service,
        discount_percent: None,
    };
    let made = h.run(s.items().create(&item, key())).expect("item");
    let got = h.run(s.items().get(made.record.id)).expect("get");
    assert_eq!(
        got.record.fields.unit_price.map(Money::to_wire).as_deref(),
        Some("999999999999999.9999")
    );
}

/// What the engine would round or tolerate is refused before anything is
/// sent: 16 integer digits, 5 decimals, and an entry off by 0.0001.
pub fn local_refusals_send_nothing(h: &Harness) {
    let before = h.observer.total();
    assert!(Money::parse_wire("1000000000000000.0000").is_err());
    assert!(Money::parse_wire("1.00001").is_err());
    let (a, b) = (
        cl_core::ids::AccountId::parse("00000000-0000-4000-8000-000000000001").expect("id"),
        cl_core::ids::AccountId::parse("00000000-0000-4000-8000-000000000002").expect("id"),
    );
    let off = NewJournalEntry::new(
        date(2026, 6, 2),
        None,
        Basis::Both,
        vec![
            line(a, Side::Debit(m("10.0001"))),
            line(b, Side::Credit(m("10.0000"))),
        ],
    );
    assert_eq!(off.expect_err("unbalanced"), LocalValidation::Unbalanced);
    assert_eq!(h.observer.total(), before, "nothing may be sent");
}

/// Money fields are JSON strings at the raw level (not numbers).
pub fn money_fields_are_strings_on_the_wire(h: &Harness) {
    let s = &h.s1;
    let (cash, supplies) = pair(h, s);
    let made = post(
        h,
        s,
        &je2(
            date(2026, 6, 3),
            &unique("raw"),
            supplies.id,
            cash.id,
            "5.5000",
        ),
    );
    let raw = h
        .run(s.raw(
            Method::Get,
            &format!("/api/v1/transactions/{}", made.record.id),
            &[],
            None,
        ))
        .expect("raw");
    assert_eq!(raw.status, 200);
    let lines = raw.body["transaction"]["lines"]
        .as_array()
        .expect("lines array");
    assert_eq!(lines.len(), 2);
    for l in lines {
        assert!(l["debit"].is_string(), "debit not a string");
        assert!(l["credit"].is_string(), "credit not a string");
    }
    let acct = h
        .run(s.raw(
            Method::Get,
            &format!("/api/v1/accounts/{}", cash.id),
            &[],
            None,
        ))
        .expect("raw");
    let balance = &acct.body["account"]["balance"];
    assert!(
        balance.is_string() || balance.is_null(),
        "balance must be a string"
    );
}

/// Capability probe `money-strict-v1` (U-19) or `balance-exact-v1` (P15),
/// in tenant B. With either, an entry off by 0.0001 is refused. This test
/// DELIBERATELY documents the older engine's 0.0001 balance tolerance:
/// without them the engine accepts a raw entry unbalanced by 0.0001 (cl-api itself
/// never sends one, see `local_refusals_send_nothing`); the entry is voided
/// straight away. The cluster is throwaway and no other test reads tenant
/// B's totals.
pub fn engine_tolerance_probe_money_strict(h: &Harness) {
    let s = h.s3();
    let (cash, supplies) = pair(h, &s);
    let body = json!({
        "txnType": "journal_entry",
        "txnDate": "2026-06-04",
        "memo": unique("tolerance probe"),
        "lines": [
            {"accountId": supplies.id, "debit": "10.0001", "credit": "0"},
            {"accountId": cash.id, "debit": "0", "credit": "10.0000"},
        ],
    });
    let raw = h
        .run(s.raw(Method::Post, "/api/v1/transactions", &[], Some(&body)))
        .expect("raw");
    // `balance-exact-v1` (P15 part A) delivers U-19's exact 4-place balance
    // check: no tolerance, 400 "does not balance", nothing posted.
    if h.caps.has(Capabilities::MONEY_STRICT) || h.caps.has(Capabilities::BALANCE_EXACT) {
        assert_eq!(raw.status, 400, "an exact balance check must refuse");
        assert!(
            raw.body.to_string().contains("does not balance"),
            "{}",
            raw.body
        );
        return;
    }
    assert!(
        !h.require_cap(Capabilities::MONEY_STRICT, "U-19"),
        "capability state changed mid-test"
    );
    assert_eq!(
        raw.status, 201,
        "without U-19 the engine is expected to accept an entry off by 0.0001"
    );
    let id = raw.body["transaction"]["id"]
        .as_str()
        .and_then(|v| cl_core::ids::TransactionId::parse(v).ok())
        .expect("id");
    h.run(
        s.transactions()
            .void(id, "tolerance probe", &PostingOptions::default(), key()),
    )
    .expect("void the probe");
}
