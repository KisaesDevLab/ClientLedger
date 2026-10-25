//! Request idempotency against the real engine (U-18 `idempotency-v1`,
//! engine PR #171). PENDING until the engine pin carries the capability
//! (pin 7b267e48 does not); `CL_ENGINE_EXE` pointed at an engine main build
//! runs them today.
//!
//! - `lost_reply_replays_one_posting`: a `POST /transactions` whose reply
//!   is lost is sent again by cl-api with the same key; the engine answers
//!   with the first result (`Idempotent-Replayed: true`) and there is
//!   exactly one posting.
//! - `reused_key_is_refused`: the same key with a different body is 422
//!   `IDEMPOTENCY_KEY_REUSED`, naming what the first request saved, and
//!   nothing more is posted.
//! - `keyed_routes_replay`: one keyable route of each kind (Write Check,
//!   Enter Bill, a template's Enter now, a daily-sales post) loses its
//!   reply and is replayed to exactly one effect.

use cl_api::{
    AccountType, ApiError, BankPosting, BillLine, Capabilities, CardChargeDraft, ContactFields,
    ContactType, DailyPostOptions, EditableFields, EntryStatus, Method, NewBill, NewContact,
    NewTemplate, PageRequest, PostingOptions, SplitLine, TemplateDraft, TransactionFilter, TxnType,
};
use cl_core::Money;

use crate::common::{account, date, je2, key, m, pair};
use crate::support::{Harness, unique};

const TXNS: &str = "/api/v1/transactions";

fn ready(h: &Harness) -> bool {
    h.require_cap(Capabilities::IDEMPOTENCY, "U-18 idempotency-v1")
}

/// Loses the next 2xx reply to `route`, runs `f`, and checks that cl-api
/// sent it once more and the engine replayed it once.
fn lost_then_replayed<T>(h: &Harness, method: Method, route: &str, f: impl FnOnce() -> T) -> T {
    let sent = h.observer.sent(method, route);
    let replays = h.observer.replays(method, route);
    let dropped = h.observer.dropped();
    h.observer.drop_next_reply(method, route);
    let out = f();
    assert_eq!(h.observer.dropped() - dropped, 1, "the reply was not lost");
    assert_eq!(
        h.observer.sent(method, route) - sent,
        2,
        "sent again with the same key"
    );
    assert_eq!(
        h.observer.replays(method, route) - replays,
        1,
        "the engine replayed the first answer"
    );
    out
}

pub fn lost_reply_replays_one_posting(h: &Harness) {
    if !ready(h) {
        return;
    }
    let s = &h.s1;
    let (cash, supplies) = pair(h, s);
    let memo = unique("lost keyed");
    let on = date(2026, 7, 9);
    let entry = je2(on, &memo, supplies.id, cash.id, "31.4100");
    let made = lost_then_replayed(h, Method::Post, TXNS, || {
        h.run(
            s.transactions()
                .create_journal_entry(&entry, &PostingOptions::default(), key()),
        )
        .unwrap_or_else(|e| panic!("replayed: {e}"))
    });
    assert_eq!(made.record.fingerprint(), entry.fingerprint());
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
    assert_eq!(page.data.len(), 1, "exactly one posting");
    assert_eq!(page.data[0].id, made.record.id);
}

pub fn reused_key_is_refused(h: &Harness) {
    if !ready(h) {
        return;
    }
    let s = &h.s1;
    let (cash, supplies) = pair(h, s);
    let memo = unique("reused key");
    let on = date(2026, 7, 10);
    let k = key();
    let first = h
        .run(s.transactions().create_journal_entry(
            &je2(on, &memo, supplies.id, cash.id, "5.0000"),
            &PostingOptions::default(),
            k.clone(),
        ))
        .expect("first");
    let e = h
        .run(s.transactions().create_journal_entry(
            &je2(on, &memo, supplies.id, cash.id, "6.0000"),
            &PostingOptions::default(),
            k,
        ))
        .expect_err("reused");
    let ApiError::IdempotencyKeyReused { entity } = e else {
        panic!("expected IdempotencyKeyReused, got {e:?}");
    };
    let entity = entity.expect("the engine names what was saved");
    assert_eq!(entity.entity_id, first.record.id.to_string());
    let filter = TransactionFilter {
        start_date: Some(on),
        end_date: Some(on),
        search: Some(memo),
        ..TransactionFilter::default()
    };
    let page = h
        .run(s.transactions().list(&filter, PageRequest::first_max()))
        .expect("list");
    assert_eq!(page.data.len(), 1, "the second body posted nothing");
}

pub fn keyed_routes_replay(h: &Harness) {
    if !ready(h) {
        return;
    }
    let s = &h.s1;
    let opts = PostingOptions::default();

    // Write Check (`POST /checks`): one check with that number.
    let b = crate::contract_write_check::books(h, s);
    let on = date(2026, 11, 12);
    let check = BankPosting::check(crate::contract_write_check::story(
        &b,
        on,
        Some(5511),
        false,
    ))
    .expect("check");
    let made = lost_then_replayed(h, Method::Post, "/api/v1/checks", || {
        h.run(s.transactions().create_bank_posting(&check, &opts, key()))
            .unwrap_or_else(|e| panic!("check replayed: {e}"))
    });
    assert_eq!(made.record.fingerprint(), check.created_fingerprint());
    let numbered = h
        .run(s.checks().list(
            Some(b.bank),
            None,
            None,
            PageRequest::new(50, 0).expect("page"),
        ))
        .expect("checks")
        .into_iter()
        .filter(|r| r.number == Some(5511))
        .count();
    assert_eq!(numbered, 1, "one check");

    // Enter Bill (`POST /bills`): one open bill for the vendor.
    let vendor = h
        .run(s.contacts().create(
            &NewContact {
                fields: ContactFields::named(ContactType::Vendor, unique("Keyed vendor")),
                opening_balance: Money::ZERO,
                opening_balance_date: None,
            },
            key(),
        ))
        .expect("vendor")
        .record;
    let stone = account(h, s, AccountType::Expense, "Keyed stone").id;
    let bill = NewBill {
        contact_id: vendor.id,
        txn_date: date(2026, 11, 12),
        due_date: Some(date(2099, 12, 12)),
        vendor_invoice_number: Some(unique("K")),
        memo: None,
        lines: vec![BillLine {
            account_id: stone,
            description: None,
            amount: m("212.3400"),
        }],
    };
    lost_then_replayed(h, Method::Post, "/api/v1/bills", || {
        h.run(s.create_bill(&bill, key()))
            .unwrap_or_else(|e| panic!("bill replayed: {e}"))
    });
    let open = h.run(s.bills().payable(vendor.id)).expect("payable");
    assert_eq!(open.len(), 1, "one bill");
    assert_eq!(open[0].amount_due.to_wire(), "212.3400");

    // A template's Enter now (`POST /templates/{id}/enter`): one posting.
    if h.require_cap(Capabilities::TEMPLATES, "U-62 templates-v1") {
        let card = account(h, s, AccountType::Liability, "Keyed card").id;
        let software = account(h, s, AccountType::Expense, "Keyed software").id;
        let memo = unique("keyed enter");
        let t = h
            .run(s.templates().create(&NewTemplate {
                name: unique("Keyed template"),
                group: None,
                client_form: None,
                draft: TemplateDraft::CardCharge(CardChargeDraft {
                    pay_from: card,
                    date: date(2026, 11, 12),
                    number: None,
                    payee: None,
                    memo: Some(memo.clone()),
                    lines: vec![SplitLine {
                        account: software,
                        amount: m("49.00"),
                        memo: None,
                        contact: None,
                        billable: false,
                        tag: None,
                    }],
                }),
                schedule: None,
            }))
            .unwrap_or_else(|e| panic!("template: {e}"));
        let on = date(2026, 11, 13);
        let entered = lost_then_replayed(h, Method::Post, "/api/v1/templates/{id}/enter", || {
            h.run(s.templates().enter(t.id, Some(on), &opts, key()))
                .unwrap_or_else(|e| panic!("enter replayed: {e}"))
        });
        let filter = TransactionFilter {
            start_date: Some(on),
            end_date: Some(on),
            search: Some(memo),
            ..TransactionFilter::default()
        };
        let page = h
            .run(s.transactions().list(&filter, PageRequest::first_max()))
            .expect("list");
        assert_eq!(page.data.len(), 1, "entered once");
        assert_eq!(page.data[0].id, entered.transaction);
    }

    // A daily-sales post (`POST /daily-sales/entries/{id}/post`): posted
    // once (a second run of the post would refuse a posted day).
    let l = crate::contract_daily_sales::layout(h, s);
    let ds = s.daily_sales();
    let draft = h
        .run(ds.create_draft(
            &crate::contract_daily_sales::day(&l, date(2026, 11, 14), "500.00"),
            key(),
        ))
        .expect("draft");
    let posted = lost_then_replayed(
        h,
        Method::Post,
        "/api/v1/daily-sales/entries/{id}/post",
        || {
            h.run(ds.post(draft.id, &DailyPostOptions::default(), key()))
                .unwrap_or_else(|e| panic!("post replayed: {e}"))
        },
    );
    assert_eq!(posted.status, EntryStatus::Posted);
    let again = h.run(ds.entry(draft.id)).expect("day");
    assert_eq!(again.status, EntryStatus::Posted);
    assert_eq!(again.transaction, posted.transaction, "one posting");
}
