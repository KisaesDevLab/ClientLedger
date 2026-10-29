//! Form printing against the live engine (P19; U-106 to U-111, U-113).
//!
//! - `check_print_cycle`: queued checks list with their bank account and
//!   mailing rows; the check style per bank account round-trips without
//!   touching another account's; printing records the numbers; a second
//!   print over the same numbers is refused; a requeue spoils the number,
//!   which may be printed again; a whole batch goes back with reprint.
//! - `reprint_closed_period`: requeue of a check dated on or before the
//!   closing date needs the closing-date password (423, 403 wrong).
//! - `logo_and_footers`: the logo uploads and reads back byte for byte;
//!   footer overrides resolve per type.
//! - `document_numbers_live`: prefix and next number per type; claim
//!   takes one.
//! - `forms_queue_live`: an invoice created with Print later is listed
//!   until it is marked printed.

use std::collections::BTreeMap;

use cl_api::{
    AccountType, ApiError, BankPosting, Capabilities, CheckFormat, CheckLayout,
    ClosedPeriodAuthorization, ContactFields, ContactType, DocType, DocumentNumberUpdate, Method,
    NewContact, PostingOptions, PrintChecks, PrintProblem, QueuedCheckKind, Session, SpoiledReason,
    print_problem,
};
use cl_core::ids::{AccountId, TransactionId};
use cl_core::{Money, Secret};
use serde_json::json;

use crate::common::{account, date, key, m};
use crate::contract_bank_postings::detail_account;
use crate::contract_write_check::{books, story};
use crate::support::{Harness, unique};

/// A check waiting to print on `bank`.
fn queued(h: &Harness, s: &Session, bank: AccountId, on: cl_core::CalendarDate) -> TransactionId {
    let mut b = books(h, s);
    b.bank = bank;
    let mut d = story(&b, on, None, false);
    d.print_later = true;
    let posting = BankPosting::check(d).expect("check");
    h.run(
        s.transactions()
            .create_bank_posting(&posting, &PostingOptions::default(), key()),
    )
    .unwrap_or_else(|e| panic!("queued check: {e:?}"))
    .record
    .id
}

fn print(
    h: &Harness,
    s: &Session,
    bank: AccountId,
    checks: Vec<TransactionId>,
    first: u64,
) -> Result<cl_api::PrintedBatch, ApiError> {
    h.run(s.printing().print_checks(
        &PrintChecks {
            bank_account: bank,
            checks,
            starting_number: first,
            format: CheckFormat::Voucher,
        },
        &PostingOptions::default(),
        cl_api::IdempotencyKey::new(),
    ))
}

/// U-106, U-107, U-109.
pub fn check_print_cycle(h: &Harness) {
    if !h.require_cap(Capabilities::CHECK_REPRINT_AUDIT, "U-106") {
        return;
    }
    let s = &h.s1;
    let bank = detail_account(h, s, AccountType::Asset, "bank");
    let other = detail_account(h, s, AccountType::Asset, "bank");
    let a = queued(h, s, bank, date(2026, 11, 4));
    let b = queued(h, s, bank, date(2026, 11, 5));
    let q = h
        .run(s.printing().check_queue(Some(bank)))
        .expect("print queue");
    let row = q.iter().find(|r| r.id == a).expect("queued");
    assert_eq!(row.amount, m("1286.40"), "exact amount");
    assert_eq!(row.bank_account, Some(bank));
    assert_eq!(row.kind, QueuedCheckKind::Check);
    assert_eq!(row.address, ["1200 Commerce Dr", "Aurora, MO 65605"]);
    assert!(q.iter().any(|r| r.id == b));

    // The style of a new account is voucher (Q-P19q); one account's style
    // never changes another's.
    let (_, layout) = h.run(s.printing().check_layout(bank)).expect("layout");
    assert_eq!(layout.format, CheckFormat::Voucher);
    assert!(!layout.print_company_block && !layout.own);
    let zfold = CheckLayout {
        format: CheckFormat::ZFold,
        print_company_block: true,
        own: true,
    };
    h.run(s.printing().set_check_layout(other, zfold))
        .expect("other style");
    let standard = CheckLayout {
        format: CheckFormat::Standard,
        print_company_block: false,
        own: true,
    };
    h.run(s.printing().set_check_layout(bank, standard))
        .expect("style");
    let (_, read) = h.run(s.printing().check_layout(bank)).expect("layout");
    assert_eq!(read, standard);
    let (_, read) = h.run(s.printing().check_layout(other)).expect("layout");
    assert_eq!(read, zfold, "the other account keeps its style");

    // Numbers are recorded at Print.
    let batch = print(h, s, bank, vec![a, b], 1001).expect("print");
    assert_eq!(batch.checks_printed, 2);
    let q = h
        .run(s.printing().check_queue(Some(bank)))
        .expect("print queue");
    assert!(!q.iter().any(|r| r.id == a || r.id == b), "printed");
    let (settings, _) = h.run(s.printing().check_layout(bank)).expect("layout");
    assert_eq!(settings.next_for(bank), Some(1003));
    let c = queued(h, s, bank, date(2026, 11, 6));
    let e = print(h, s, bank, vec![c], 1002).expect_err("number used");
    assert_eq!(print_problem(&e), Some(PrintProblem::NumberInUse), "{e:?}");

    // "Did all checks print correctly?" — first bad number 1002.
    let r = h
        .run(s.printing().requeue(&[b], &PostingOptions::default()))
        .expect("requeue");
    assert_eq!(r.checks[0].spoiled_number, Some(1002));
    let spoiled = h.run(s.printing().spoiled(Some(bank))).expect("spoiled");
    let sp = spoiled.iter().find(|x| x.check == b).expect("logged");
    assert_eq!((sp.number, sp.reason), (1002, SpoiledReason::Requeue));
    // The spoiled number may be used again (owner 2026-10-29).
    print(h, s, bank, vec![b], 1002).expect("reuse 1002");
    let read = h.run(s.checks().get(b)).expect("check");
    assert_eq!(read.record.check_number, Some(1002));

    // A whole batch goes back; the payment stays.
    let before = h.run(s.accounts().get(bank)).expect("bank").record.balance;
    let r = h
        .run(
            s.printing()
                .reprint_batch(batch.batch_id, &PostingOptions::default()),
        )
        .expect("reprint batch");
    assert!(
        r.checks
            .iter()
            .any(|x| x.id == a && x.spoiled_number == Some(1001))
    );
    let after = h.run(s.accounts().get(bank)).expect("bank").record.balance;
    assert_eq!(before, after, "a reprint never voids");
    let q = h
        .run(s.printing().check_queue(Some(bank)))
        .expect("print queue");
    assert!(q.iter().any(|x| x.id == a), "back in the queue");
    // A hand-written check can't be requeued.
    let mut bk = books(h, s);
    bk.bank = bank;
    let hand = BankPosting::check(story(&bk, date(2026, 11, 7), Some(5001), false)).expect("check");
    let hand = h
        .run(
            s.transactions()
                .create_bank_posting(&hand, &PostingOptions::default(), key()),
        )
        .expect("hand written")
        .record
        .id;
    let e = h
        .run(s.printing().requeue(&[hand], &PostingOptions::default()))
        .expect_err("hand written");
    assert_eq!(print_problem(&e), Some(PrintProblem::NotPrinted), "{e:?}");
}

/// U-106 with the closing date (Q-P19ac), on the second company.
pub fn reprint_closed_period(h: &Harness) {
    if !h.require_cap(Capabilities::CHECK_REPRINT_AUDIT, "U-106")
        || !h.require_cap(Capabilities::CLOSING_PASSWORD, "U-15")
    {
        return;
    }
    let s = h.s3();
    let bank = detail_account(h, &s, AccountType::Asset, "bank");
    let id = queued(h, &s, bank, date(2026, 9, 15));
    print(h, &s, bank, vec![id], 2001).expect("print");
    let lock = |v: serde_json::Value| {
        h.run(s.raw_mapped(
            Method::Put,
            "/api/v1/company/settings",
            Some(&json!({ "lockDate": v })),
        ))
        .expect("closing date");
    };
    lock(json!("2026-09-30"));
    let pw = Secret::new(unique("closing-reprint"));
    assert!(
        h.run(s.set_closing_password(None, Some(&pw), key()))
            .expect("set password")
    );
    let with = |p: &Secret| PostingOptions {
        closed_period: Some(ClosedPeriodAuthorization::with_password(p.clone())),
    };
    let e = h
        .run(s.printing().requeue(&[id], &PostingOptions::default()))
        .expect_err("closed");
    assert!(
        matches!(&e, ApiError::ClosedPeriod(c) if c.password_required == Some(true)),
        "{e:?}"
    );
    let e = h
        .run(
            s.printing()
                .requeue(&[id], &with(&Secret::new("not-the-password"))),
        )
        .expect_err("wrong");
    assert_eq!(e, ApiError::ClosingPasswordRejected);
    let r = h
        .run(s.printing().requeue(&[id], &with(&pw)))
        .expect("with the password");
    assert_eq!(r.checks[0].spoiled_number, Some(2001));
    assert!(
        !h.run(s.set_closing_password(Some(&pw), None, key()))
            .expect("clear password")
    );
    lock(serde_json::Value::Null);
}

/// A 1×1 PNG.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
    0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0xF8, 0xFF, 0xFF, 0x3F,
    0x00, 0x05, 0xFE, 0x02, 0xFE, 0xA7, 0x35, 0x81, 0x84, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E,
    0x44, 0xAE, 0x42, 0x60, 0x82,
];

/// U-110 and the U-111 footers.
pub fn logo_and_footers(h: &Harness) {
    let s = &h.s1;
    if h.require_cap(Capabilities::COMPANY_LOGO, "U-110") {
        h.run(s.printing().upload_logo(PNG)).expect("upload");
        let back = h.run(s.printing().logo()).expect("logo");
        assert_eq!(back.as_deref(), Some(PNG), "byte for byte");
    }
    if !h.require_cap(Capabilities::DOCUMENT_NUMBERS, "U-111") {
        return;
    }
    let before = h.run(s.printing().footers()).expect("footers");
    let mut o = BTreeMap::new();
    o.insert(DocType::Invoice, "Net 30 · pay by check".to_string());
    o.insert(DocType::Statement, "   ".to_string());
    let after = h
        .run(s.printing().set_footers("Thank you!", &o))
        .expect("set footers");
    assert_eq!(after.footer_text.as_deref(), Some("Thank you!"));
    assert_eq!(
        after.resolved_for(DocType::Invoice),
        "Net 30 · pay by check"
    );
    assert_eq!(after.resolved_for(DocType::Statement), "Thank you!");
    assert_eq!(after.resolved_for(DocType::SalesReceipt), "Thank you!");
    let read = h.run(s.printing().footers()).expect("footers");
    assert_eq!(read, after);
    h.run(s.printing().set_footers(
        before.footer_text.as_deref().unwrap_or(""),
        &before.overrides,
    ))
    .expect("restore footers");
}

/// U-111 numbering.
pub fn document_numbers_live(h: &Harness) {
    if !h.require_cap(Capabilities::DOCUMENT_NUMBERS, "U-111") {
        return;
    }
    let s = &h.s1;
    let all = h.run(s.printing().document_numbers()).expect("list");
    assert_eq!(all.len(), 4, "{all:?}");
    let set = h
        .run(s.printing().set_document_number(
            DocType::Statement,
            &DocumentNumberUpdate {
                prefix: Some("ST-".into()),
                next_number: Some(700),
            },
        ))
        .expect("set");
    assert_eq!(set.next, "ST-700");
    let next = h
        .run(s.printing().next_number(DocType::Statement))
        .expect("next");
    assert_eq!(next.next_number, 700, "peeking takes nothing");
    let c = h
        .run(s.printing().claim_number(DocType::Statement))
        .expect("claim");
    assert_eq!((c.number, c.txn_number.as_str()), (700, "ST-700"));
    let next = h
        .run(s.printing().next_number(DocType::Statement))
        .expect("next");
    assert_eq!(next.next, "ST-701");
}

/// U-113.
pub fn forms_queue_live(h: &Harness) {
    if !h.require_cap(Capabilities::FORMS_PRINT_QUEUE, "U-113") {
        return;
    }
    let s = &h.s1;
    let income = account(h, s, AccountType::Revenue, "Print later sales");
    let client = h
        .run(s.contacts().create(
            &NewContact {
                fields: ContactFields::named(ContactType::Customer, unique("Print Later Co")),
                opening_balance: Money::ZERO,
                opening_balance_date: None,
            },
            key(),
        ))
        .expect("client")
        .record
        .id;
    let created = h
        .run(s.raw_mapped(
            Method::Post,
            "/api/v1/invoices",
            Some(&json!({
                "txnDate": "2026-11-03",
                "contactId": client.to_string(),
                "printLater": true,
                "lines": [{
                    "accountId": income.id.to_string(),
                    "quantity": "1",
                    "unitPrice": "1500.00",
                    "isTaxable": false,
                    "taxRate": "0",
                }],
            })),
        ))
        .expect("invoice");
    let id = created
        .pointer("/invoice/id")
        .or_else(|| created.get("id"))
        .and_then(serde_json::Value::as_str)
        .and_then(|v| TransactionId::parse(v).ok())
        .unwrap_or_else(|| panic!("invoice id in {created}"));
    let q = h
        .run(s.printing().form_queue(Some(DocType::Invoice)))
        .expect("queue");
    let row = q.iter().find(|r| r.id == id).expect("listed");
    assert_eq!(row.total, Some(m("1500.00")));
    assert_eq!(row.kind, DocType::Invoice);
    let r = h.run(s.printing().mark_forms_printed(&[id])).expect("mark");
    assert_eq!((r.marked, r.already_printed), (1, 0));
    let q = h.run(s.printing().form_queue(None)).expect("queue");
    assert!(
        !q.iter().any(|r| r.id == id),
        "printed forms leave the queue"
    );
    let r = h
        .run(s.printing().mark_forms_printed(&[id]))
        .expect("mark again");
    assert_eq!((r.marked, r.already_printed), (0, 1));
}
