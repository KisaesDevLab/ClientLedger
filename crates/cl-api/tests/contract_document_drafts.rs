//! Document drafts and vendor layouts against the real engine (A03 WP-F,
//! step 16; engine U-92 / U-92a / U-92b, `target/plans/A03.md` §6
//! "Contract"). One binary, one engine, one fresh cluster; tests run in the
//! order listed in `main`.
//!
//! Every draft test is PENDING by `document-drafts-v1` on an engine without
//! it and then pins today's behaviour instead: the typed calls send nothing
//! (`Local(Unsupported)`) and the route is the engine's JSON 404. The money
//! table (N4) runs on any engine with `bill-ocr-v2`.
//!
//! Not here (the engine's own vitest covers them, `document-drafts.routes
//! .test.ts`): T6's permission check on a replayed key (needs per-user
//! permission overrides), and the transfer manifest's `omitted` text (the
//! archive is encrypted; its effect is checked here instead).
//!
//! A04 (engine U-95, `document-drafts-attach-v1`): the attach matrix (T9),
//! the exact amount filter, and the live side of the shared draft cases
//! (`fixtures/docs/draft-cases.json`, T16), each PENDING(U-95) without the
//! capability (the shared cases still run their claim, store, discard and
//! convert sections).
//!
//! Run: `cargo xtask engine && cargo xtask pg-fetch`, then
//! `cargo test -p cl-api --test contract_document_drafts` (`CL_API_REQUIRE=1`
//! in CI). Synthetic data only.

#![allow(
    clippy::too_many_lines,
    clippy::many_single_char_names,
    reason = "contract scenarios: h, s, v, d as in the other contract binaries"
)]

#[path = "support/mod.rs"]
mod support;

use std::time::{Duration, Instant};

use cl_api::{
    AccountFields, AccountType, ApiError, ArchiveRequest, AttachTo, AttachmentId, BankPosting,
    BillDraft, BillLineDraft, Capabilities, CardChargeDraft, CardLast4, CheckDraft, ConflictReason,
    ContactFields, ContactType, ConvertBody, DocumentDraft, DocumentDraftId, DraftConversion,
    DraftKind, DraftListQuery, DraftReader, DraftReading, DraftRefusal, DraftStatus,
    IdempotencyKey, ImportRequest, JobState, LocalValidation, MatchReason, MatchSource, Method,
    NewContact, PostingOptions, ReportBasis, Session, SplitLine, TemplateUpsert, layout_key,
};
use cl_core::ids::{AccountId, ContactId, TenantId, TransactionId};
use cl_core::{CalendarDate, Money, Secret};
use serde_json::{Map, Value, json};

use support::{EngineMode, Harness, TestFn, run_suite, unique};

const BASE: &str = "/api/v1/document-drafts";
const U92: &str = "U-92";
const RULE_CASES: &str = include_str!("../../../fixtures/docs/rule-cases.json");

// ---------------------------------------------------------------- builders

fn m(s: &str) -> Money {
    Money::parse_wire(s).unwrap_or_else(|e| panic!("money {s}: {e:?}"))
}

fn date(y: i32, mo: u32, d: u32) -> CalendarDate {
    CalendarDate::from_ymd(y, mo, d).expect("date")
}

fn key() -> IdempotencyKey {
    IdempotencyKey::new()
}

fn account(h: &Harness, s: &Session, t: AccountType, detail: Option<&str>) -> AccountId {
    h.run(s.accounts().create(
        &AccountFields {
            name: unique("Drafts account"),
            account_number: None,
            account_type: t,
            detail_type: detail.map(str::to_string),
            description: None,
            parent_id: None,
        },
        key(),
    ))
    .unwrap_or_else(|e| panic!("account: {e}"))
    .record
    .id
}

fn vendor(h: &Harness, s: &Session) -> ContactId {
    h.run(s.contacts().create(
        &NewContact {
            fields: ContactFields::named(ContactType::Vendor, unique("Synthetic Supply")),
            opening_balance: Money::ZERO,
            opening_balance_date: None,
        },
        key(),
    ))
    .unwrap_or_else(|e| panic!("vendor: {e}"))
    .record
    .id
}

/// A synthetic PDF whose bytes are unique (its own claim).
fn pdf() -> Vec<u8> {
    format!(
        "%PDF-1.4\n% synthetic drafts contract {}\n%%EOF\n",
        key().as_header()
    )
    .into_bytes()
}

fn upload(h: &Harness, s: &Session, bytes: &[u8]) -> AttachmentId {
    h.run(
        s.attachments()
            .upload_draft(cl_api::DraftKey::new(), "document.pdf", bytes, key()),
    )
    .unwrap_or_else(|e| panic!("upload: {e}"))
    .id
}

fn claim(h: &Harness, s: &Session, file: AttachmentId) -> DocumentDraft {
    h.run(s.document_drafts().create(file, DraftKind::Unknown, key()))
        .unwrap_or_else(|e| panic!("claim: {e:?}"))
}

/// A fresh file claimed as a draft.
fn new_draft(h: &Harness, s: &Session) -> (AttachmentId, DocumentDraft) {
    let file = upload(h, s, &pdf());
    (file, claim(h, s, file))
}

fn reading(
    vendor: Option<ContactId>,
    reference: Option<&str>,
    total: Option<&str>,
    on: Option<CalendarDate>,
) -> DraftReading {
    let mut fields = Map::new();
    if let Some(t) = total {
        fields.insert(
            "total".into(),
            json!({"value": t, "confidence": 9000, "spot": {"page": 0, "bbox": [10, 10, 20, 20]}}),
        );
    }
    DraftReading {
        kind: DraftKind::Bill,
        reader: DraftReader::TextPdf,
        fields,
        lines: Vec::new(),
        checks: Vec::new(),
        suggested_contact: vendor,
        doc_reference: reference.map(str::to_string),
        doc_total: total.map(m),
        doc_date: on,
    }
}

fn store(h: &Harness, s: &Session, d: &DocumentDraft, r: &DraftReading) -> DocumentDraft {
    h.run(s.document_drafts().store_reading(d.id, &d.updated_at, r))
        .unwrap_or_else(|e| panic!("store reading: {e:?}"))
}

fn bill(
    v: ContactId,
    expense: AccountId,
    on: CalendarDate,
    reference: &str,
    total: &str,
) -> BillDraft {
    BillDraft {
        vendor: v,
        date: on,
        vendor_invoice_number: Some(reference.to_string()).filter(|r| !r.trim().is_empty()),
        terms: None,
        due_date: None,
        memo: None,
        amount_due: m(total),
        lines: vec![BillLineDraft::expense(expense, m(total))],
        received: true,
        draft_attachment: None,
    }
}

fn conversion(d: &DocumentDraft, body: ConvertBody) -> DraftConversion {
    DraftConversion {
        body,
        expected_updated_at: d.updated_at.clone(),
        acknowledge: Vec::new(),
    }
}

fn convert(h: &Harness, s: &Session, d: &DocumentDraft, body: ConvertBody) -> TransactionId {
    h.run(s.document_drafts().convert(
        d.id,
        &conversion(d, body),
        &PostingOptions::default(),
        key(),
    ))
    .unwrap_or_else(|e| panic!("convert: {e:?}"))
    .transaction
}

fn get(h: &Harness, s: &Session, id: DocumentDraftId) -> DocumentDraft {
    h.run(s.document_drafts().get(id))
        .unwrap_or_else(|e| panic!("get draft: {e:?}"))
}

fn split(account: AccountId, amount: &str) -> SplitLine {
    SplitLine {
        account,
        amount: m(amount),
        memo: None,
        contact: None,
        billable: false,
        tag: None,
    }
}

fn is_claimed(e: &ApiError, by: Option<DocumentDraftId>) -> bool {
    match e {
        ApiError::DocumentDraft(DraftRefusal::AlreadyClaimed { draft }) => {
            by.is_none_or(|b| *draft == Some(b))
        }
        _ => false,
    }
}

/// `true` with `document-drafts-v1`. Without it, records PENDING(U-92) and
/// checks today's behaviour: the typed calls send nothing and the route is
/// the engine's JSON 404.
fn live(h: &Harness, s: &Session) -> bool {
    if h.require_cap(Capabilities::DOCUMENT_DRAFTS, U92) {
        return true;
    }
    let before = h.observer.total();
    assert_eq!(
        h.run(s.document_drafts().list(&DraftListQuery::default())),
        Err(ApiError::Local(LocalValidation::Unsupported))
    );
    assert_eq!(
        h.run(s.vendor_templates().list(None)),
        Err(ApiError::Local(LocalValidation::Unsupported))
    );
    assert_eq!(
        h.observer.total(),
        before,
        "nothing sent without the capability"
    );
    let raw = h.run(s.raw(Method::Get, BASE, &[], None)).expect("raw");
    assert_eq!(raw.status, 404, "{}", raw.body);
    assert_eq!(raw.body["error"]["code"], "NOT_FOUND", "{}", raw.body);
    false
}

// ------------------------------------------------------------------- tests

/// Claims: a file once; the same bytes again (another upload) name the
/// first draft; a key replays; a filed file is refused; two sessions
/// claiming one file at once get one draft; discarding frees the claim and
/// a second discard changes nothing.
fn claims(h: &Harness) {
    let s = &h.s1;
    if !live(h, s) {
        return;
    }
    let bytes = pdf();
    let file = upload(h, s, &bytes);
    let k = key();
    let first = h
        .run(s.document_drafts().create(file, DraftKind::Bill, k.clone()))
        .expect("claim");
    assert_eq!(first.status, DraftStatus::New);
    assert_eq!(first.kind, DraftKind::Bill);
    assert_eq!(first.attachment_id, Some(file));
    assert_eq!(first.content_sha256.len(), 64);
    // The same key replays the same draft.
    let again = h
        .run(s.document_drafts().create(file, DraftKind::Bill, k))
        .expect("replay");
    assert_eq!(again.id, first.id, "a keyed claim replays");
    // A new key: claimed, naming the draft.
    let e = h
        .run(s.document_drafts().create(file, DraftKind::Bill, key()))
        .expect_err("claimed");
    assert!(is_claimed(&e, Some(first.id)), "{e:?}");
    // The same bytes uploaded again are the same document.
    let copy = upload(h, s, &bytes);
    let e = h
        .run(s.document_drafts().create(copy, DraftKind::Unknown, key()))
        .expect_err("same bytes");
    assert!(is_claimed(&e, Some(first.id)), "{e:?}");

    // A file filed on a record is not a draft file.
    let v = vendor(h, s);
    let exp = account(h, s, AccountType::Expense, None);
    let posted = h
        .run(s.bills().create(
            &bill(v, exp, date(2026, 11, 3), &unique("FILED"), "12.00"),
            &PostingOptions::default(),
            key(),
        ))
        .expect("bill")
        .record
        .txn
        .id;
    let filed = h
        .run(
            s.attachments()
                .upload(AttachTo::Bill(posted), "filed.pdf", &pdf(), key()),
        )
        .expect("upload to the bill")
        .id;
    let e = h
        .run(s.document_drafts().create(filed, DraftKind::Bill, key()))
        .expect_err("filed");
    assert_eq!(e, ApiError::DocumentDraft(DraftRefusal::NotADraftFile));

    // Two sessions claim one fresh file at once: one draft.
    let s2 = h.s2();
    let race = upload(h, s, &pdf());
    let (one, two) = (s.document_drafts(), s2.document_drafts());
    let (a, b) = h.run(async {
        tokio::join!(
            one.create(race, DraftKind::Unknown, key()),
            two.create(race, DraftKind::Unknown, key()),
        )
    });
    let (won, lost) = match (a, b) {
        (Ok(d), Err(e)) | (Err(e), Ok(d)) => (d, e),
        other => panic!("exactly one claim wins: {other:?}"),
    };
    assert!(is_claimed(&lost, Some(won.id)), "{lost:?}");

    // Discard frees the claim; a second discard is fine.
    let gone = h
        .run(s.document_drafts().discard(first.id))
        .expect("discard");
    assert_eq!(gone.status, DraftStatus::Discarded);
    let twice = h.run(s.document_drafts().discard(first.id)).expect("again");
    assert_eq!(twice.status, DraftStatus::Discarded);
    let reclaimed = claim(h, s, copy);
    assert_ne!(reclaimed.id, first.id, "a discarded file is read again");
    assert_eq!(reclaimed.content_sha256, first.content_sha256);
}

/// Readings: stored with the token as read, refused with an old one; the
/// summary list leaves the reading out; a fractional number is refused by
/// the engine too; a failure is recorded.
fn readings_and_tokens(h: &Harness) {
    let s = &h.s1;
    if !live(h, s) {
        return;
    }
    let v = vendor(h, s);
    let (_, d) = new_draft(h, s);
    let r = reading(
        Some(v),
        Some(" INV-READ-1 "),
        Some("1480.00"),
        Some(date(2026, 11, 3)),
    );
    let ready = store(h, s, &d, &r);
    assert_eq!(ready.status, DraftStatus::Ready);
    assert_eq!(ready.reader, Some(DraftReader::TextPdf));
    assert_eq!(ready.suggested_contact_id, Some(v));
    assert_eq!(ready.doc_reference.as_deref(), Some("INV-READ-1"));
    assert_eq!(ready.doc_total, Some(m("1480")));
    assert_eq!(ready.doc_date, Some(date(2026, 11, 3)));
    assert_ne!(ready.updated_at, d.updated_at, "each save moves the token");
    assert_eq!(
        ready.fields.as_ref().map(|f| f["total"]["value"].clone()),
        Some(json!("1480.00"))
    );

    // The old token is stale and changes nothing.
    let e = h
        .run(s.document_drafts().store_reading(d.id, &d.updated_at, &r))
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
    assert_eq!(get(h, s, d.id), ready);

    // The engine refuses a fractional number itself.
    let raw = h
        .run(s.raw(
            Method::Put,
            &format!("{BASE}/{}", d.id),
            &[],
            Some(
                &json!({"expectedUpdatedAt": ready.updated_at, "status": "ready",
                "fields": {"total": 1480.5}}),
            ),
        ))
        .expect("raw");
    assert_eq!(raw.status, 400, "{}", raw.body);

    // The summary view has no reading; the draft is listed.
    let page = h
        .run(s.document_drafts().list(&DraftListQuery {
            statuses: vec![DraftStatus::Ready],
            summary: true,
            limit: 200,
            ..DraftListQuery::default()
        }))
        .expect("list");
    let row = page.drafts.iter().find(|x| x.id == d.id).expect("listed");
    assert_eq!(row.fields, None);
    assert!(page.total >= 1);

    let failed = h
        .run(
            s.document_drafts()
                .store_failure(d.id, &ready.updated_at, "unreadable"),
        )
        .expect("failure");
    assert_eq!(failed.status, DraftStatus::Failed);
    assert_eq!(failed.failure_reason.as_deref(), Some("unreadable"));
}

/// T1, T2: a live draft's file can't be deleted (nothing changes); after a
/// convert the file is on the bill and deletes as usual, the draft keeps
/// its claim, and the same bytes are still claimed.
fn files_of_live_and_converted_drafts(h: &Harness) {
    let s = &h.s1;
    if !live(h, s) {
        return;
    }
    let bytes = pdf();
    let file = upload(h, s, &bytes);
    let d = claim(h, s, file);
    let e = h
        .run(s.attachments().delete(file, key()))
        .expect_err("in use");
    assert!(
        matches!(&e, ApiError::Conflict { reason: ConflictReason::InUse, code: Some(c), .. }
            if c.as_str() == cl_api::ATTACHMENT_IN_USE),
        "{e:?}"
    );
    let still = h.run(s.attachments().get(file)).expect("the file is kept");
    assert_eq!(still.attachable_type, "draft");
    assert_eq!(get(h, s, d.id).attachment_id, Some(file));

    let v = vendor(h, s);
    let exp = account(h, s, AccountType::Expense, None);
    let txn = convert(
        h,
        s,
        &d,
        ConvertBody::Bill(bill(v, exp, date(2026, 11, 3), &unique("T2"), "75.00")),
    );
    let moved = h.run(s.attachments().get(file)).expect("file");
    assert_eq!(moved.attachable_type, "bill");
    assert_eq!(moved.attachable_id, txn.to_string());
    h.run(s.attachments().delete(file, key()))
        .expect("a converted draft's file deletes");
    let after = get(h, s, d.id);
    assert_eq!(after.attachment_id, None);
    assert_eq!(after.content_sha256, d.content_sha256, "the claim is kept");
    let copy = upload(h, s, &bytes);
    let e = h
        .run(s.document_drafts().create(copy, DraftKind::Bill, key()))
        .expect_err("still claimed");
    assert!(is_claimed(&e, Some(d.id)), "{e:?}");
}

/// Each kind posts exactly the body: the record's type, total, vendor and
/// pay-from account, the file moved onto it; the same key replays; a new
/// key on a converted draft is closed; a converted draft can't be changed
/// or discarded.
fn each_kind_posts_its_body(h: &Harness) {
    let s = &h.s1;
    if !live(h, s) {
        return;
    }
    let v = vendor(h, s);
    let exp = account(h, s, AccountType::Expense, None);
    let bank = account(h, s, AccountType::Asset, Some("bank"));
    let card = account(h, s, AccountType::Liability, Some("credit_card"));
    let on = date(2026, 11, 3);
    let charge = |from: AccountId, number: &str, amount: &str| {
        BankPosting::card_charge(CardChargeDraft {
            pay_from: from,
            date: on,
            number: Some(number.to_string()),
            payee: Some(v),
            memo: None,
            lines: vec![split(exp, amount)],
        })
        .expect("charge")
    };
    let check = BankPosting::check(CheckDraft {
        bank,
        date: on,
        payee: Some(v),
        payee_name: "Synthetic Supply".into(),
        check_number: None,
        print_later: true,
        payee_address: None,
        memo: None,
        printed_memo: None,
        tag_ids: Vec::new(),
        draft_attachment: None,
        amount: m("33.10"),
        lines: vec![split(exp, "33.10")],
    })
    .expect("check");
    let cases = [
        (
            ConvertBody::Bill(bill(v, exp, on, &unique("KIND"), "48.25")),
            "48.25",
            cl_api::TxnType::Bill,
            "bill",
            None,
        ),
        (
            ConvertBody::CardCharge(charge(card, &unique("R"), "21.40")),
            "21.40",
            cl_api::TxnType::Expense,
            "expense",
            Some(card),
        ),
        (
            ConvertBody::CashExpense(charge(bank, &unique("C"), "22.75")),
            "22.75",
            cl_api::TxnType::Expense,
            "expense",
            Some(bank),
        ),
        (
            ConvertBody::Check(check),
            "33.10",
            cl_api::TxnType::Expense,
            "expense",
            Some(bank),
        ),
    ];
    for (body, total, txn_type, attach_type, from) in cases {
        let kind = body.kind();
        let (file, d) = new_draft(h, s);
        let k = key();
        let conv = conversion(&d, body);
        let done = h
            .run(
                s.document_drafts()
                    .convert(d.id, &conv, &PostingOptions::default(), k.clone()),
            )
            .unwrap_or_else(|e| panic!("{kind:?}: {e:?}"));
        assert_eq!(done.draft.status, DraftStatus::Converted, "{kind:?}");
        assert_eq!(done.draft.converted_transaction_id, Some(done.transaction));
        let t = h
            .run(s.transactions().get(done.transaction))
            .expect("posted")
            .record;
        assert_eq!(t.txn_type, txn_type, "{kind:?}");
        assert_eq!(t.total, Some(m(total)), "{kind:?}");
        assert_eq!(t.contact_id, Some(v), "{kind:?}");
        if let Some(acct) = from {
            assert!(
                t.lines.iter().any(|l| l.account_id == acct),
                "{kind:?}: paid from the given account"
            );
        }
        let f = h.run(s.attachments().get(file)).expect("file");
        assert_eq!(f.attachable_type, attach_type, "{kind:?}");
        assert_eq!(f.attachable_id, done.transaction.to_string(), "{kind:?}");

        // The same key replays the same record.
        let replay = h
            .run(
                s.document_drafts()
                    .convert(d.id, &conv, &PostingOptions::default(), k),
            )
            .expect("replay");
        assert_eq!(replay.transaction, done.transaction, "{kind:?}");
        // A new key: closed, naming the record.
        let e = h
            .run(
                s.document_drafts()
                    .convert(d.id, &conv, &PostingOptions::default(), key()),
            )
            .expect_err("closed");
        assert_eq!(
            e,
            ApiError::DocumentDraft(DraftRefusal::Closed {
                transaction: Some(done.transaction)
            }),
            "{kind:?}"
        );
    }
    // A converted draft can't be changed or discarded.
    let (_, d) = new_draft(h, s);
    convert(
        h,
        s,
        &d,
        ConvertBody::Bill(bill(v, exp, on, &unique("CLOSED"), "9.00")),
    );
    let now = get(h, s, d.id);
    let e = h
        .run(s.document_drafts().store_reading(
            d.id,
            &now.updated_at,
            &reading(None, None, None, None),
        ))
        .expect_err("closed");
    assert!(
        matches!(e, ApiError::DocumentDraft(DraftRefusal::Closed { .. })),
        "{e:?}"
    );
    let e = h
        .run(s.document_drafts().discard(d.id))
        .expect_err("closed");
    assert!(
        matches!(e, ApiError::DocumentDraft(DraftRefusal::Closed { .. })),
        "{e:?}"
    );
}

/// T3, T4: a posted bill of the same vendor and reference blocks a convert
/// until it is acknowledged; two computers converting two copies of one
/// invoice at once post one; the amount window is 60 days and totals
/// compare as numbers (10.1 = 10.10).
fn duplicates_block_until_acknowledged(h: &Harness) {
    let s = &h.s1;
    if !live(h, s) {
        return;
    }
    let v = vendor(h, s);
    let exp = account(h, s, AccountType::Expense, None);
    let posted = h
        .run(s.bills().create(
            &bill(v, exp, date(2026, 10, 1), "INV-00123", "100.00"),
            &PostingOptions::default(),
            key(),
        ))
        .expect("bill")
        .record
        .txn
        .id;
    let (_, d) = new_draft(h, s);
    let body = || ConvertBody::Bill(bill(v, exp, date(2026, 11, 20), "inv 123", "250.00"));
    let e = h
        .run(s.document_drafts().convert(
            d.id,
            &conversion(&d, body()),
            &PostingOptions::default(),
            key(),
        ))
        .expect_err("duplicate");
    let ApiError::DocumentDraft(DraftRefusal::PossibleDuplicate { matches, .. }) = &e else {
        panic!("{e:?}");
    };
    let hit = matches
        .iter()
        .find(|x| x.source == MatchSource::Transaction(posted))
        .unwrap_or_else(|| panic!("the posted bill is named: {matches:?}"));
    assert_eq!(hit.reason, MatchReason::Reference);
    assert_eq!(get(h, s, d.id).status, DraftStatus::New, "nothing posted");
    let ok = h
        .run(s.document_drafts().convert(
            d.id,
            &DraftConversion {
                acknowledge: vec![posted],
                ..conversion(&d, body())
            },
            &PostingOptions::default(),
            key(),
        ))
        .expect("acknowledged");
    assert_eq!(ok.draft.status, DraftStatus::Converted);

    // T3: two copies, two sessions, at once: one posts.
    let v2 = vendor(h, s);
    let reference = unique("RACE").replace(' ', "-");
    let (_, a) = new_draft(h, s);
    let (_, b) = new_draft(h, s);
    let s2 = h.s2();
    let ca = conversion(
        &a,
        ConvertBody::Bill(bill(v2, exp, date(2026, 11, 3), &reference, "64.00")),
    );
    let cb = conversion(
        &b,
        ConvertBody::Bill(bill(v2, exp, date(2026, 11, 3), &reference, "64.00")),
    );
    let opts = PostingOptions::default();
    let (one, two) = (s.document_drafts(), s2.document_drafts());
    let (ra, rb) = h.run(async {
        tokio::join!(
            one.convert(a.id, &ca, &opts, key()),
            two.convert(b.id, &cb, &opts, key()),
        )
    });
    let (won, lost) = match (ra, rb) {
        (Ok(w), Err(l)) | (Err(l), Ok(w)) => (w, l),
        other => panic!("exactly one posts: {other:?}"),
    };
    match &lost {
        ApiError::DocumentDraft(DraftRefusal::PossibleDuplicate { matches, .. }) => assert!(
            matches
                .iter()
                .any(|x| x.source == MatchSource::Transaction(won.transaction)),
            "{matches:?}"
        ),
        other => panic!("{other:?}"),
    }

    // T4: the same total within 60 days, by number.
    let v3 = vendor(h, s);
    let base = date(2026, 6, 1);
    let near = h
        .run(s.bills().create(
            &bill(v3, exp, date(2026, 4, 3), "", "10.10"),
            &PostingOptions::default(),
            key(),
        ))
        .expect("bill 59 days before")
        .record
        .txn
        .id;
    let far = h
        .run(s.bills().create(
            &bill(v3, exp, date(2026, 8, 1), "", "10.10"),
            &PostingOptions::default(),
            key(),
        ))
        .expect("bill 61 days after")
        .record
        .txn
        .id;
    let (_, d3) = new_draft(h, s);
    store(
        h,
        s,
        &d3,
        &reading(Some(v3), None, Some("10.1"), Some(base)),
    );
    let found = h.run(s.document_drafts().duplicates(d3.id)).expect("dupes");
    assert!(
        found
            .iter()
            .any(|x| x.source == MatchSource::Transaction(near)
                && x.reason == MatchReason::AmountDate),
        "59 days matches: {found:?}"
    );
    assert!(
        found
            .iter()
            .all(|x| x.source != MatchSource::Transaction(far)),
        "61 days does not: {found:?}"
    );
    // Without a vendor there is nothing to compare.
    let (_, d4) = new_draft(h, s);
    store(
        h,
        s,
        &d4,
        &reading(None, Some("INV-00123"), Some("100.00"), Some(base)),
    );
    assert_eq!(
        h.run(s.document_drafts().duplicates(d4.id))
            .expect("dupes")
            .len(),
        0
    );
}

/// T5 and the top-level password: in a closed period a convert is refused
/// and leaves nothing (no record, the draft and its file as they were); a
/// password inside `body` is ignored; the top-level password posts it.
/// Missing kind is 400 `INVALID_KIND`; another company's draft is 404.
fn closed_period_rollback_and_scope(h: &Harness) {
    let a = &h.s1;
    if !live(h, a) {
        return;
    }
    // Another company sees nothing of Contract Co's drafts or files.
    let (file, d) = new_draft(h, a);
    let b = h.s3();
    let e = h
        .run(b.document_drafts().get(d.id))
        .expect_err("other company");
    assert!(matches!(e, ApiError::NotFound { .. }), "{e:?}");
    let e = h
        .run(b.document_drafts().create(file, DraftKind::Bill, key()))
        .expect_err("other company's file");
    assert!(matches!(e, ApiError::NotFound { .. }), "{e:?}");
    // No kind: 400 INVALID_KIND, before anything else.
    let raw = h
        .run(a.raw(
            Method::Post,
            &format!("{BASE}/{}/convert", d.id),
            &[],
            Some(&json!({"body": {}, "expectedUpdatedAt": d.updated_at})),
        ))
        .expect("raw");
    assert_eq!(raw.status, 400, "{}", raw.body);
    assert_eq!(raw.body["error"]["code"], "INVALID_KIND", "{}", raw.body);

    if !h.require_cap(Capabilities::CLOSING_PASSWORD, "U-15") {
        return;
    }
    let v = vendor(h, &b);
    let exp = account(h, &b, AccountType::Expense, None);
    let (file, d) = new_draft(h, &b);
    let tb = |s: &Session| {
        h.run(s.reports().trial_balance_csv(
            date(2025, 1, 1),
            date(2026, 12, 31),
            ReportBasis::Accrual,
        ))
        .expect("trial balance")
    };
    h.run(b.raw_mapped(
        Method::Put,
        "/api/v1/company/settings",
        Some(&json!({"lockDate": "2026-01-31"})),
    ))
    .expect("set the closing date");
    let pw = Secret::new(format!("closing-{}", key().as_header()));
    h.run(b.set_closing_password(None, Some(&pw), key()))
        .expect("set the closing password");
    let before = tb(&b);
    let body = bill(v, exp, date(2026, 1, 15), &unique("LOCKED"), "40.00");
    let conv = conversion(&d, ConvertBody::Bill(body.clone()));

    let e = h
        .run(
            b.document_drafts()
                .convert(d.id, &conv, &PostingOptions::default(), key()),
        )
        .expect_err("closed period");
    assert!(
        matches!(
            e,
            ApiError::ClosedPeriod(_) | ApiError::ClosingPasswordRequired
        ),
        "{e:?}"
    );
    // A password inside `body` is dropped.
    let nested = json!({
        "kind": "bill",
        "expectedUpdatedAt": d.updated_at,
        "body": {
            "contactId": v, "txnDate": "2026-01-15",
            "lines": [{"accountId": exp, "amount": "40.0000"}],
            "closingPassword": pw.expose(),
        },
    });
    let raw = h
        .run(b.raw(
            Method::Post,
            &format!("{BASE}/{}/convert", d.id),
            &[],
            Some(&nested),
        ))
        .expect("raw");
    assert!(matches!(raw.status, 423 | 403), "{}", raw.body);
    // T5: nothing was written.
    assert_eq!(tb(&b), before, "no journal lines");
    let after = get(h, &b, d.id);
    assert_eq!(after.status, DraftStatus::New);
    assert_eq!(after.updated_at, d.updated_at, "the draft is unchanged");
    assert_eq!(
        h.run(b.attachments().get(file))
            .expect("file")
            .attachable_type,
        "draft",
        "the file stays on its draft key"
    );
    // The top-level password posts it.
    let done = h
        .run(b.document_drafts().convert(
            d.id,
            &conv,
            &PostingOptions {
                closed_period: Some(cl_api::ClosedPeriodAuthorization::with_password(pw.clone())),
            },
            key(),
        ))
        .expect("the password opens the period");
    assert_eq!(done.draft.status, DraftStatus::Converted);
    assert_ne!(tb(&b), before);

    // Clean up: no password, no closing date.
    h.run(b.set_closing_password(Some(&pw), None, key()))
        .expect("clear the password");
    h.run(b.raw_mapped(
        Method::Put,
        "/api/v1/company/settings",
        Some(&json!({"lockDate": null})),
    ))
    .expect("clear the closing date");
}

/// Drafts never touch the books: the trial balance and the A/P aging are
/// the same before and after 20 drafts with readings.
fn drafts_leave_the_books_alone(h: &Harness) {
    let s = &h.s1;
    if !live(h, s) {
        return;
    }
    let books = || {
        let tb = h
            .run(s.reports().trial_balance_csv(
                date(2020, 1, 1),
                date(2030, 12, 31),
                ReportBasis::Accrual,
            ))
            .expect("trial balance");
        let aging = h
            .run(s.raw(
                Method::Get,
                "/api/v1/reports/ap-aging-summary",
                &[("as_of_date", "2026-12-31")],
                None,
            ))
            .expect("aging");
        assert_eq!(aging.status, 200, "{}", aging.body);
        (tb, aging.body)
    };
    let before = books();
    let v = vendor(h, s);
    for i in 0..20 {
        let (_, d) = new_draft(h, s);
        store(
            h,
            s,
            &d,
            &reading(
                Some(v),
                Some(&format!("BOOKS-{i}")),
                Some("999.99"),
                Some(date(2026, 11, 3)),
            ),
        );
    }
    assert_eq!(
        books(),
        before,
        "balances, trial balance and A/P aging unchanged"
    );
}

/// Card digits: set on a credit card account, refused elsewhere and in
/// another form, cleared with `None`.
fn card_last4(h: &Harness) {
    let s = &h.s1;
    if !live(h, s) {
        return;
    }
    let card = account(h, s, AccountType::Liability, Some("credit_card"));
    let bank = account(h, s, AccountType::Asset, Some("bank"));
    let v = h.run(s.accounts().get(card)).expect("card");
    assert_eq!(v.record.card_last4, None);
    let set = h
        .run(
            s.accounts()
                .set_card_last4(&v, CardLast4::parse("4821"), key()),
        )
        .expect("set");
    assert_eq!(set.record.card_last4, CardLast4::parse("4821"));
    assert_eq!(set.record.fields, v.record.fields, "nothing else changed");
    assert_eq!(
        h.run(s.accounts().get(card))
            .expect("card")
            .record
            .card_last4,
        CardLast4::parse("4821")
    );
    for (id, digits) in [
        (card, json!("48a1")),
        (card, json!("123")),
        (bank, json!("4821")),
    ] {
        let raw = h
            .run(s.raw(
                Method::Put,
                &format!("/api/v1/accounts/{id}"),
                &[],
                Some(&json!({"cardLast4": digits})),
            ))
            .expect("raw");
        assert_eq!(raw.status, 400, "{digits}: {}", raw.body);
    }
    let b = h.run(s.accounts().get(bank)).expect("bank");
    assert_eq!(
        h.run(
            s.accounts()
                .set_card_last4(&b, CardLast4::parse("4821"), key())
        ),
        Err(ApiError::Local(LocalValidation::InvalidValue))
    );
    let cleared = h
        .run(s.accounts().set_card_last4(&set, None, key()))
        .expect("clear");
    assert_eq!(cleared.record.card_last4, None);
}

fn regions(label: &str) -> Map<String, Value> {
    let mut r = Map::new();
    r.insert("v".into(), json!(1));
    r.insert(
        "template".into(),
        json!({"fields": [{"field": "total", "page": 0, "region": [100, 200, 900, 260], "label": label}]}),
    );
    r
}

/// Vendor layouts: create only once, replace with the token, stale tokens
/// refused, delete.
fn templates_crud(h: &Harness) {
    let s = &h.s1;
    if !live(h, s) {
        return;
    }
    let v = vendor(h, s);
    let fp = layout_key(&[unique("layout"), "aspect:12941".to_string()]);
    let new = TemplateUpsert {
        contact: v,
        fingerprint: fp.clone(),
        field_regions: regions("total"),
        line_table: None,
        samples: 1,
        expected_updated_at: None,
    };
    let made = h.run(s.vendor_templates().put(&new)).expect("create");
    assert_eq!(made.contact_id, v);
    assert_eq!(made.fingerprint, fp);
    assert_eq!(made.field_regions, regions("total"));
    let e = h
        .run(s.vendor_templates().put(&new))
        .expect_err("create only");
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
    let replaced = h
        .run(s.vendor_templates().put(&TemplateUpsert {
            samples: 2,
            line_table: Some(Map::from_iter([("page".to_string(), json!(0))])),
            expected_updated_at: Some(made.updated_at.clone()),
            ..new.clone()
        }))
        .expect("replace");
    assert_eq!(replaced.id, made.id);
    assert_eq!(replaced.samples, 2);
    assert!(replaced.line_table.is_some());
    let listed = h.run(s.vendor_templates().list(Some(v))).expect("list");
    assert_eq!(listed, vec![replaced.clone()]);
    let e = h
        .run(s.vendor_templates().delete(made.id, Some(&made.updated_at)))
        .expect_err("stale delete");
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
    h.run(
        s.vendor_templates()
            .delete(made.id, Some(&replaced.updated_at)),
    )
    .expect("delete");
    assert_eq!(
        h.run(s.vendor_templates().list(Some(v)))
            .expect("list")
            .len(),
        0
    );
}

/// T9: merging vendors moves drafts and layouts; a layout both had for the
/// same fingerprint becomes one row with the samples added.
fn contact_merge_moves_drafts_and_layouts(h: &Harness) {
    let s = &h.s1;
    if !live(h, s) {
        return;
    }
    let from = vendor(h, s);
    let into = vendor(h, s);
    let (_, d) = new_draft(h, s);
    store(
        h,
        s,
        &d,
        &reading(
            Some(from),
            Some("MERGE-1"),
            Some("5.00"),
            Some(date(2026, 11, 3)),
        ),
    );
    let fp = layout_key(&[unique("merge layout")]);
    for (c, samples) in [(from, 2), (into, 3)] {
        h.run(s.vendor_templates().put(&TemplateUpsert {
            contact: c,
            fingerprint: fp.clone(),
            field_regions: regions("total"),
            line_table: None,
            samples,
            expected_updated_at: None,
        }))
        .expect("layout");
    }
    h.run(s.contacts().merge(from, into, None, key()))
        .expect("merge");
    assert_eq!(get(h, s, d.id).suggested_contact_id, Some(into));
    assert_eq!(
        h.run(s.vendor_templates().list(Some(from)))
            .expect("list")
            .len(),
        0
    );
    let kept = h.run(s.vendor_templates().list(Some(into))).expect("list");
    assert_eq!(kept.len(), 1, "{kept:?}");
    assert_eq!(kept[0].fingerprint, fp);
    assert_eq!(kept[0].samples, 5);
}

/// Polls an archive job to its end.
fn finish_job(h: &Harness, s: &Session, job: &cl_api::JobId) -> cl_api::ArchiveJobStatus {
    let deadline = Instant::now() + Duration::from_secs(180);
    loop {
        let st = h.run(s.tenants().archive_job(job)).expect("job");
        match st.status {
            JobState::Succeeded | JobState::Failed => return st,
            JobState::Queued | JobState::Running => {
                assert!(Instant::now() < deadline, "job did not finish");
                std::thread::sleep(Duration::from_millis(300));
            }
        }
    }
}

/// Exports `co` (from the workspace) and imports it as a new company.
fn export_import(h: &Harness, s: &Session, co: TenantId, transfer: bool) -> TenantId {
    let passphrase = Secret::new(format!("drafts-{}", unique("pass")));
    let job = h
        .run(s.tenants().export_archive(
            co,
            &ArchiveRequest {
                include_attachments: true,
                include_audit: true,
                omit_intake_settings: false,
                omit_document_drafts: transfer,
                host: json!({"app": "ClientLedger", "appVersion": "test",
                    "kind": if transfer { "transfer" } else { "backup" }}),
                passphrase: passphrase.clone(),
            },
        ))
        .expect("export job");
    let st = finish_job(h, s, &job.job_id);
    assert_eq!(st.status, JobState::Succeeded, "{:?}", st.error);
    let path = st
        .export_result()
        .expect("export result")
        .path
        .clone()
        .expect("path");
    let job = h
        .run(s.tenants().import_archive(&ImportRequest {
            path,
            passphrase,
            scratch: false,
            reset_document_drafts: transfer,
        }))
        .expect("import job");
    let st = finish_job(h, s, &job.job_id);
    assert_eq!(st.status, JobState::Succeeded, "{:?}", st.error);
    st.import_result().expect("import result").tenant_id
}

fn statuses(h: &Harness, s: &Session) -> Vec<DraftStatus> {
    let mut all: Vec<DraftStatus> = h
        .run(s.document_drafts().list(&DraftListQuery {
            limit: 200,
            summary: true,
            ..DraftListQuery::default()
        }))
        .expect("list")
        .drafts
        .into_iter()
        .map(|d| d.status)
        .collect();
    all.sort_by_key(|s| s.as_str());
    all
}

/// T7, T8: a backup keeps drafts, layouts and the claim index; a transfer
/// file (omit on export, reset on import) keeps only converted drafts, with
/// their files, and no layouts.
fn backups_keep_and_transfers_omit(h: &Harness) {
    let s = h.s3();
    if !live(h, &s) {
        return;
    }
    let co = h.tenant_b();
    let v = vendor(h, &s);
    let exp = account(h, &s, AccountType::Expense, None);
    let (_, done) = new_draft(h, &s);
    convert(
        h,
        &s,
        &done,
        ConvertBody::Bill(bill(v, exp, date(2026, 11, 3), &unique("MOVE"), "18.00")),
    );
    let live_bytes = pdf();
    let live_file = upload(h, &s, &live_bytes);
    let waiting = claim(h, &s, live_file);
    store(
        h,
        &s,
        &waiting,
        &reading(
            Some(v),
            Some("WAIT-1"),
            Some("7.00"),
            Some(date(2026, 11, 3)),
        ),
    );
    h.run(s.vendor_templates().put(&TemplateUpsert {
        contact: v,
        fingerprint: layout_key(&[unique("archive layout")]),
        field_regions: regions("total"),
        line_table: None,
        samples: 1,
        expected_updated_at: None,
    }))
    .expect("layout");
    let here = statuses(h, &s);
    assert!(here.contains(&DraftStatus::Converted) && here.contains(&DraftStatus::Ready));
    h.run(s.close_company()).expect("workspace");

    // A backup keeps everything; the claim index comes with it.
    let kept = export_import(h, &s, co, false);
    h.run(s.open_company(kept)).expect("open restored");
    assert_eq!(statuses(h, &s), here, "every draft restored");
    assert_eq!(
        h.run(s.vendor_templates().list(None))
            .expect("layouts")
            .len(),
        1
    );
    let again = upload(h, &s, &live_bytes);
    let e = h
        .run(s.document_drafts().create(again, DraftKind::Bill, key()))
        .expect_err("claimed after the round trip");
    assert!(is_claimed(&e, None), "{e:?}");
    h.run(s.close_company()).expect("workspace");

    // A transfer file: converted drafts only, no layouts.
    let moved = export_import(h, &s, co, true);
    h.run(s.open_company(moved)).expect("open transferred");
    let left = statuses(h, &s);
    assert!(!left.is_empty(), "converted drafts travel");
    assert!(
        left.iter().all(|st| *st == DraftStatus::Converted),
        "{left:?}"
    );
    assert_eq!(
        h.run(s.vendor_templates().list(None))
            .expect("layouts")
            .len(),
        0
    );
    let converted = h
        .run(s.document_drafts().list(&DraftListQuery {
            statuses: vec![DraftStatus::Converted],
            limit: 200,
            ..DraftListQuery::default()
        }))
        .expect("list");
    let file = converted
        .drafts
        .iter()
        .find_map(|d| d.attachment_id)
        .expect("a converted draft keeps its file");
    h.run(s.attachments().get(file))
        .expect("the converted draft's file travels");
    // The live draft's file was left out: the same bytes read again.
    let fresh = upload(h, &s, &live_bytes);
    claim(h, &s, fresh);
    h.run(s.close_company()).expect("workspace");
}

/// N4, reference table: the engine's duplicate check normalizes references
/// exactly like `cl-docread::dupes::normalize_reference`. A posted bill
/// with reference `in` matches a draft whose reference is `out`, the
/// normalized form, exactly when `out` is not empty.
fn reference_table_matches_the_engine(h: &Harness) {
    let s = &h.s1;
    if !live(h, s) {
        return;
    }
    let table: Value = serde_json::from_str(RULE_CASES).expect("rule-cases.json");
    let cases = table["reference"].as_array().expect("reference table");
    assert!(cases.len() >= 10);
    let exp = account(h, s, AccountType::Expense, None);
    for (i, case) in cases.iter().enumerate() {
        let input = case["in"].as_str().expect("in");
        let out = case["out"].as_str().expect("out");
        let v = vendor(h, s);
        let posted = h
            .run(s.bills().create(
                &bill(v, exp, date(2026, 6, 1), input, &format!("{}.00", 100 + i)),
                &PostingOptions::default(),
                key(),
            ))
            .unwrap_or_else(|e| panic!("bill {input:?}: {e:?}"))
            .record
            .txn
            .id;
        let (_, d) = new_draft(h, s);
        // An empty normalized form is sent as text that normalizes to
        // nothing (the engine refuses an empty reference).
        let draft_ref = if out.is_empty() { "---" } else { out };
        store(
            h,
            s,
            &d,
            &reading(
                Some(v),
                Some(draft_ref),
                Some("5000.00"),
                Some(date(2025, 1, 1)),
            ),
        );
        let found = h.run(s.document_drafts().duplicates(d.id)).expect("dupes");
        let matched = found.iter().any(|x| {
            x.source == MatchSource::Transaction(posted) && x.reason == MatchReason::Reference
        });
        assert_eq!(
            matched,
            !out.is_empty(),
            "reference {input:?} -> {out:?}: {found:?}"
        );
    }
}

/// A 1x1 PNG (synthetic): the canned model reads it.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
    0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE,
    0x42, 0x60, 0x82,
];

/// N4, money table: the engine's `normalizeBillMoney` gives exactly the
/// table's `out` (or nothing) for each `in`, as `cl-docread::amount`
/// does. Each input goes in as a line amount of a canned loopback model
/// reply; the engine's normalized line amounts come back in order.
fn money_table_matches_the_engine(h: &Harness) {
    use cl_api::{AiConfigUpdate, AiTaskToggles, OllamaTarget, SessionSecrets};
    if !(h.require_cap(Capabilities::BILL_OCR_V2, "U-89")
        && h.require_cap(Capabilities::AI_DESKTOP, "U-09"))
    {
        return;
    }
    let s = &h.s1;
    let table: Value = serde_json::from_str(RULE_CASES).expect("rule-cases.json");
    let cases = table["money"].as_array().expect("money table").clone();
    assert!(cases.len() >= 30);
    let items: Vec<Value> = cases
        .iter()
        .enumerate()
        .map(|(i, c)| {
            json!({"description": format!("case {i}"), "amount": c["in"], "quantity": null,
                "confidence": 0.9})
        })
        .collect();
    let canned = json!({
        "vendor": "Synthetic Table Supply", "vendor_invoice_number": "T-1",
        "bill_date": "2026-11-03", "due_date": null, "payment_terms": null,
        "subtotal": null, "tax": null, "total": null, "line_items": items,
        "confidence": 0.9, "field_confidence": {},
    })
    .to_string();
    let stub = support::OllamaStub::shared(h);
    stub.serve_chat(h, &canned);
    h.run(s.accept_ai_disclosure(key())).expect("disclosure");
    h.run(s.update_ai_config(
        &AiConfigUpdate {
            ollama_base_url: Some(stub.base_url.clone()),
            categorization_model: Some("llama3.1:8b".into()),
            ocr_model: Some("llama3.1:8b".into()),
            ..AiConfigUpdate::for_provider("ollama", true)
        },
        key(),
    ))
    .expect("AI on");
    h.run(s.put_session_secrets(&SessionSecrets {
        ollama: Some(OllamaTarget {
            base_url: stub.base_url.clone(),
            model: None,
        }),
        ..SessionSecrets::default()
    }))
    .expect("session ollama");
    h.run(s.accept_company_ai_consent(key())).expect("consent");
    h.run(s.set_ai_tasks(
        &AiTaskToggles {
            receipt_ocr: Some(true),
            ..AiTaskToggles::default()
        },
        key(),
    ))
    .expect("bill reading on");
    let up = h
        .run(
            s.attachments()
                .upload_draft(cl_api::DraftKey::new(), "table.png", PNG, key()),
        )
        .expect("upload");
    let read = h.run(s.raw(
        Method::Post,
        &format!("/api/v1/bills/extract-from-attachment/{}", up.id),
        &[],
        None,
    ));
    // Switch AI off before asserting, so a failure leaves the product default.
    h.run(s.update_ai_config(
        &AiConfigUpdate {
            is_enabled: Some(false),
            ..AiConfigUpdate::default()
        },
        key(),
    ))
    .expect("AI off");
    let read = read.expect("extract");
    assert_eq!(read.status, 200, "{}", read.body);
    let lines = read.body["extraction"]["lineItems"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        lines.len(),
        cases.len(),
        "every case comes back: {}",
        read.body
    );
    let mut wrong = Vec::new();
    for (case, line) in cases.iter().zip(&lines) {
        let want = case["out"].as_str();
        let got = line["amount"].as_str();
        if want != got {
            wrong.push(format!("{} -> engine {got:?}, table {want:?}", case["in"]));
        }
    }
    assert!(
        wrong.is_empty(),
        "the engine disagrees with the table:\n{}",
        wrong.join("\n")
    );
    h.run(s.attachments().delete(up.id, key()))
        .expect("clean up");
}

// ------------------------------------------------------------- A04 (U-95)

const U95: &str = "U-95";
const DRAFT_CASES: &str = include_str!("../../../fixtures/docs/draft-cases.json");

/// `true` with `document-drafts-v1` and `document-drafts-attach-v1`.
/// Without the second, records PENDING(U-95) and checks today's behaviour:
/// attach and the amount filter send nothing, and the engine has no attach
/// route.
fn live_attach(h: &Harness, s: &Session) -> bool {
    if !live(h, s) {
        return false;
    }
    if h.require_cap(Capabilities::DOCUMENT_DRAFTS_ATTACH, U95) {
        return true;
    }
    let (_, d) = new_draft(h, s);
    let someone = TransactionId::parse("00000000-0000-4000-8000-0000000000e1").expect("id");
    let before = h.observer.total();
    assert_eq!(
        h.run(
            s.document_drafts()
                .attach(d.id, someone, &d.updated_at, key())
        ),
        Err(ApiError::Local(LocalValidation::Unsupported))
    );
    let filter = cl_api::TransactionFilter {
        amount: Some(m("10.10")),
        ..cl_api::TransactionFilter::default()
    };
    assert_eq!(
        h.run(
            s.transactions()
                .list(&filter, cl_api::PageRequest::first_max())
        )
        .map(|_| ()),
        Err(ApiError::Local(LocalValidation::Unsupported))
    );
    assert_eq!(
        h.observer.total(),
        before,
        "nothing sent without the capability"
    );
    let raw = h
        .run(s.raw(
            Method::Post,
            &format!("{BASE}/{}/attach", d.id),
            &[],
            Some(&json!({"transactionId": someone, "expectedUpdatedAt": d.updated_at})),
        ))
        .expect("raw");
    assert_eq!(raw.status, 404, "{}", raw.body);
    h.run(s.document_drafts().discard(d.id)).expect("discard");
    false
}

/// A card charge of `amount` from `from`, posted.
fn charge(
    h: &Harness,
    s: &Session,
    from: AccountId,
    exp: AccountId,
    on: CalendarDate,
    amount: &str,
) -> TransactionId {
    let p = BankPosting::card_charge(CardChargeDraft {
        pay_from: from,
        date: on,
        number: None,
        payee: None,
        memo: None,
        lines: vec![split(exp, amount)],
    })
    .expect("charge");
    h.run(
        s.transactions()
            .create_bank_posting(&p, &PostingOptions::default(), key()),
    )
    .unwrap_or_else(|e| panic!("charge: {e:?}"))
    .record
    .id
}

fn attach(
    h: &Harness,
    s: &Session,
    d: &DocumentDraft,
    to: TransactionId,
) -> Result<cl_api::AttachedDraft, ApiError> {
    h.run(s.document_drafts().attach(d.id, to, &d.updated_at, key()))
}

fn file_type(h: &Harness, s: &Session, file: AttachmentId) -> (String, String) {
    let a = h.run(s.attachments().get(file)).expect("file");
    (a.attachable_type, a.attachable_id)
}

/// T9 (U-95 a): attach files the document on a posted bill or expense and
/// closes the draft (kind unchanged, the record's type); the same key
/// replays, a new key is closed, the claim stays; a stale token, a void
/// record, another company's record, an unknown record and a discarded
/// draft are refused and nothing moves; a live draft's file can't be
/// linked anywhere (the guard).
fn attach_matrix(h: &Harness) {
    let s = &h.s1;
    if !live_attach(h, s) {
        return;
    }
    let v = vendor(h, s);
    let exp = account(h, s, AccountType::Expense, None);
    let card = account(h, s, AccountType::Liability, Some("credit_card"));
    let on = date(2026, 11, 4);

    // A bill posted from another draft, then a second document attached to it.
    let (_, first) = new_draft(h, s);
    let bill_id = convert(
        h,
        s,
        &first,
        ConvertBody::Bill(bill(v, exp, on, &unique("ATT"), "61.20")),
    );
    let bytes = pdf();
    let file = upload(h, s, &bytes);
    let d = claim(h, s, file);
    let k = key();
    let done = h
        .run(
            s.document_drafts()
                .attach(d.id, bill_id, &d.updated_at, k.clone()),
        )
        .expect("attached to the bill");
    assert_eq!(done.transaction, bill_id);
    assert_eq!(done.draft.status, DraftStatus::Converted);
    assert_eq!(done.draft.kind, DraftKind::Unknown, "kind unchanged");
    assert_eq!(done.draft.converted_transaction_id, Some(bill_id));
    assert_eq!(done.draft.converted_type.as_deref(), Some("bill"));
    assert_eq!(
        file_type(h, s, file),
        ("bill".to_string(), bill_id.to_string())
    );
    // The same key replays; a new key is closed, naming the record.
    let again = h
        .run(s.document_drafts().attach(d.id, bill_id, &d.updated_at, k))
        .expect("replay");
    assert_eq!(again.transaction, bill_id);
    let e = attach(h, s, &d, bill_id).expect_err("closed");
    assert_eq!(
        cl_api::attach_problem(&e),
        Some(cl_api::AttachProblem::Closed {
            transaction: Some(bill_id)
        }),
        "{e:?}"
    );
    // The claim stays (no undo, Q-A04e).
    let copy = upload(h, s, &bytes);
    let e = h
        .run(s.document_drafts().create(copy, DraftKind::Unknown, key()))
        .expect_err("still claimed");
    assert!(is_claimed(&e, Some(d.id)), "{e:?}");

    // An expense; a stale token is refused and nothing moves.
    let t = charge(h, s, card, exp, on, "18.75");
    let (file, d) = new_draft(h, s);
    let read = store(h, s, &d, &reading(None, None, Some("18.75"), Some(on)));
    let e = attach(h, s, &d, t).expect_err("stale");
    assert_eq!(
        cl_api::attach_problem(&e),
        Some(cl_api::AttachProblem::Stale)
    );
    assert_eq!(file_type(h, s, file).0, "draft");
    // The guard: a live draft's file can't be linked onto the record.
    let e = h
        .run(
            s.attachments()
                .link(file, AttachTo::Txn(cl_api::TxnType::Expense, t), key()),
        )
        .expect_err("in use");
    assert_eq!(
        cl_api::attach_problem(&e),
        Some(cl_api::AttachProblem::FileInUse),
        "{e:?}"
    );
    assert_eq!(file_type(h, s, file).0, "draft");
    // Unknown and another company's records are not found.
    let nobody = TransactionId::parse("00000000-0000-4000-8000-0000000000e9").expect("id");
    let e = attach(h, s, &read, nobody).expect_err("unknown");
    assert_eq!(
        cl_api::attach_problem(&e),
        Some(cl_api::AttachProblem::NotFound)
    );
    let b = h.s3();
    let b_exp = account(h, &b, AccountType::Expense, None);
    let b_card = account(h, &b, AccountType::Liability, Some("credit_card"));
    let theirs = charge(h, &b, b_card, b_exp, on, "18.75");
    let e = attach(h, s, &read, theirs).expect_err("other company");
    assert_eq!(
        cl_api::attach_problem(&e),
        Some(cl_api::AttachProblem::NotFound)
    );
    assert_eq!(file_type(h, s, file).0, "draft");
    // The ready draft attaches with its current token.
    let done = attach(h, s, &read, t).expect("attached to the expense");
    assert_eq!(done.draft.converted_type.as_deref(), Some("expense"));
    assert_eq!(done.draft.kind, DraftKind::Bill, "the reading's kind stays");
    assert_eq!(
        file_type(h, s, file),
        ("expense".to_string(), t.to_string())
    );

    // A void record is not posted.
    let gone = charge(h, s, card, exp, on, "18.76");
    h.run(
        s.transactions()
            .void(gone, "Synthetic void", &PostingOptions::default(), key()),
    )
    .expect("void");
    let (file, d) = new_draft(h, s);
    let e = attach(h, s, &d, gone).expect_err("void");
    assert_eq!(
        cl_api::attach_problem(&e),
        Some(cl_api::AttachProblem::NotPosted),
        "{e:?}"
    );
    assert_eq!(file_type(h, s, file).0, "draft");
    assert_eq!(get(h, s, d.id).status, DraftStatus::New, "nothing changed");
    // A discarded draft is closed.
    let gone = h.run(s.document_drafts().discard(d.id)).expect("discard");
    let e = attach(h, s, &gone, t).expect_err("discarded");
    assert!(
        matches!(
            cl_api::attach_problem(&e),
            Some(cl_api::AttachProblem::Closed { .. })
        ),
        "{e:?}"
    );
}

/// U-95 c: `amount` matches the total exactly as a number (`10.1` =
/// `10.10`, never `10.01`), posted only with `status`, within the dates,
/// and `accountId` keeps the records with a line on that account.
fn amount_filter_is_exact(h: &Harness) {
    let s = &h.s1;
    if !live_attach(h, s) {
        return;
    }
    let exp = account(h, s, AccountType::Expense, None);
    let card = account(h, s, AccountType::Liability, Some("credit_card"));
    let bank = account(h, s, AccountType::Asset, Some("bank"));
    let on = date(2026, 9, 14);
    let a = charge(h, s, card, exp, on, "7310.10");
    let b = charge(h, s, bank, exp, on, "7310.10");
    let c = charge(h, s, card, exp, on, "7310.01");
    let late = charge(h, s, card, exp, date(2026, 9, 30), "7310.10");
    let search = |amount: &str, account: Option<AccountId>| -> Vec<TransactionId> {
        let filter = cl_api::TransactionFilter {
            amount: Some(m(amount)),
            status: Some(cl_api::TxnStatus::Posted),
            start_date: Some(date(2026, 9, 7)),
            end_date: Some(date(2026, 9, 21)),
            account_id: account,
            ..cl_api::TransactionFilter::default()
        };
        let mut ids: Vec<TransactionId> = h
            .run(
                s.transactions()
                    .list(&filter, cl_api::PageRequest::first_max()),
            )
            .expect("list")
            .data
            .into_iter()
            .map(|t| t.id)
            .collect();
        ids.sort();
        ids
    };
    let mut both = vec![a, b];
    both.sort();
    assert_eq!(search("7310.1", None), both, "10.1 = 10.10");
    assert_eq!(search("7310.1000", None), both);
    assert_eq!(search("7310.01", None), vec![c], "never 10.01");
    assert_eq!(search("7310.10", Some(card)), vec![a], "the card's only");
    assert!(
        !search("7310.10", None).contains(&late),
        "the dates bound it"
    );
    h.run(
        s.transactions()
            .void(b, "Synthetic void", &PostingOptions::default(), key()),
    )
    .expect("void");
    assert_eq!(search("7310.10", None), vec![a], "posted only");
    // An amount the engine can't take as a decimal string is never sent:
    // `Money` always writes four places.
    let raw = h
        .run(s.raw(
            Method::Get,
            "/api/v1/transactions",
            &[("amount", "7310.10000")],
            None,
        ))
        .expect("raw");
    assert_eq!(raw.status, 400, "{}", raw.body);
}

// ------------------------------------------------- the shared draft cases

/// The live side of the shared document draft cases
/// (`fixtures/docs/draft-cases.json`, T16): the same steps and arguments
/// as the demo runner (`cl-ui` `company/demo/drafts_tests.rs`, whose header
/// documents them), against the engine. The cases run in tenant B (a new
/// company per case would spend the rate-limited `create-tenant` budget)
/// with one expense account, credit card and checking account; each case
/// gets its own "story" vendor (so the vendor duplicate rule sees only the
/// case's records) and its own file contents (the case's salt), and leaves
/// no live draft, closing date or closing password behind. The amounts the
/// cases search for appear in no other case.
///
/// Live differences, each with the same visible answer:
/// - `plain_relink {file, transaction}`: the engine has no plain relink
///   onto an existing expense (`POST /bills/:id/attach-draft` takes bills
///   only), so the live runner saves a new bill with
///   `draftAttachmentId` = the file's draft key (a plain caller the guard
///   covers); `transaction` is not used. `expect.file` is checked as usual.
/// - `set_closing_date` also sets a closing password ("right").
/// - `search_amount` always asks for posted records (the demo leaves void
///   ones out too).
struct CaseWorld<'h> {
    h: &'h Harness,
    s: Session,
    exp: AccountId,
    card: AccountId,
    bank: AccountId,
    story: ContactId,
    /// Added to every file's content, so the same bytes collide only
    /// within the case (claims are per company).
    salt: String,
    password: Option<Secret>,
    labels: std::collections::HashMap<String, Value>,
    keys: std::collections::HashMap<String, IdempotencyKey>,
    draft_keys: std::collections::HashMap<String, cl_api::DraftKey>,
}

/// What a step answered.
struct Answer {
    status: u16,
    code: Option<String>,
    draft: Option<DocumentDraft>,
    label: Value,
    total: Option<u64>,
    count: Option<usize>,
}

impl Answer {
    fn ok(status: u16, label: Value) -> Answer {
        Answer {
            status,
            code: None,
            draft: None,
            label,
            total: None,
            count: None,
        }
    }

    fn draft(status: u16, d: DocumentDraft) -> Answer {
        let label = json!({"id": d.id, "updatedAt": d.updated_at});
        Answer {
            draft: Some(d),
            ..Answer::ok(status, label)
        }
    }

    fn of(status: u16, r: Result<DocumentDraft, ApiError>) -> Answer {
        match r {
            Ok(d) => Answer::draft(status, d),
            Err(e) => Answer::err(&e),
        }
    }

    fn err(e: &ApiError) -> Answer {
        let (status, code) = status_of(e);
        Answer {
            code,
            ..Answer::ok(status, Value::Null)
        }
    }
}

/// The HTTP status and code an [`ApiError`] stands for.
fn status_of(e: &ApiError) -> (u16, Option<String>) {
    let code = |c: &str| Some(c.to_string());
    match e {
        ApiError::DocumentDraft(r) => match r {
            DraftRefusal::AlreadyClaimed { .. } => (409, code(cl_api::DOCUMENT_ALREADY_CLAIMED)),
            DraftRefusal::Closed { .. } => (409, code(cl_api::DOCUMENT_DRAFT_CLOSED)),
            DraftRefusal::PossibleDuplicate { .. } => (409, code(cl_api::POSSIBLE_DUPLICATE)),
            DraftRefusal::NotADraftFile => (422, code(cl_api::ATTACHMENT_NOT_DRAFT)),
            DraftRefusal::TemplateLimit => (409, code(cl_api::TEMPLATE_LIMIT)),
        },
        ApiError::Conflict { code: c, .. } => (409, c.as_ref().map(|c| c.as_str().to_string())),
        ApiError::Validation(v) => {
            let c = v.code().map(|c| c.as_str().to_string());
            let status = if c.as_deref() == Some("VALIDATION_ERROR") {
                400
            } else {
                422
            };
            (status, c)
        }
        ApiError::ClosedPeriod(_) | ApiError::ClosingPasswordRequired => {
            (423, code("TB_PERIOD_LOCKED"))
        }
        ApiError::ClosingPasswordRejected => (403, code("CLOSING_PASSWORD_INVALID")),
        ApiError::NotFound { code: c } => (404, c.as_ref().map(|c| c.as_str().to_string())),
        ApiError::IdempotencyKeyReused { .. } => (422, code("IDEMPOTENCY_KEY_REUSED")),
        ApiError::Forbidden { code: c } => (403, c.as_ref().map(|c| c.as_str().to_string())),
        other => panic!("not an engine answer: {other:?}"),
    }
}

impl CaseWorld<'_> {
    fn new<'h>(h: &'h Harness, s: &Session, books: [AccountId; 3]) -> CaseWorld<'h> {
        let [exp, card, bank] = books;
        CaseWorld {
            h,
            s: s.clone(),
            exp,
            card,
            bank,
            story: vendor(h, s),
            salt: unique("case"),
            password: None,
            labels: std::collections::HashMap::new(),
            keys: std::collections::HashMap::new(),
            draft_keys: std::collections::HashMap::new(),
        }
    }

    /// After a case: every live draft discarded (so the next case's list
    /// counts only its own), and the closing date and password cleared.
    fn clean_up(&self) {
        let (h, s) = (self.h, &self.s);
        tidy(h, s);
        if let Some(pw) = &self.password {
            h.run(s.set_closing_password(Some(pw), None, key()))
                .expect("clear the closing password");
            h.run(s.raw_mapped(
                Method::Put,
                "/api/v1/company/settings",
                Some(&json!({"lockDate": null})),
            ))
            .expect("clear the closing date");
        }
    }

    /// A `$label` or `$label.field` reference, else the text itself.
    fn text(&self, v: &Value) -> String {
        let s = v.as_str().expect("a text argument");
        let Some(r) = s.strip_prefix('$') else {
            return s.to_string();
        };
        let (label, field) = r.split_once('.').unwrap_or((r, "id"));
        self.labels
            .get(label)
            .and_then(|l| l[field].as_str())
            .unwrap_or_else(|| panic!("unknown reference {s}"))
            .to_string()
    }

    fn arg(&self, args: &Value, k: &str) -> String {
        self.text(&args[k])
    }

    fn opt(&self, args: &Value, k: &str) -> Option<String> {
        (!args[k].is_null()).then(|| self.text(&args[k]))
    }

    fn key(&mut self, args: &Value) -> IdempotencyKey {
        let label = args["key"].as_str().unwrap_or("").to_string();
        self.keys.entry(label).or_default().clone()
    }

    fn draft_id(&self, args: &Value) -> DocumentDraftId {
        DocumentDraftId::parse(&self.arg(args, "draft")).expect("draft id")
    }

    fn file_id(&self, args: &Value) -> AttachmentId {
        AttachmentId::parse(&self.arg(args, "file")).expect("file id")
    }

    fn txn_id(&self, args: &Value) -> TransactionId {
        TransactionId::parse(&self.arg(args, "transaction")).expect("transaction id")
    }

    fn vendor(&self, args: &Value) -> Option<ContactId> {
        match args["vendor"].as_str()? {
            "story" => Some(self.story),
            other => panic!("vendor {other}"),
        }
    }

    fn day(&self, args: &Value, k: &str) -> CalendarDate {
        CalendarDate::parse_wire(&self.arg(args, k)).expect("date")
    }

    fn body(&self, args: &Value) -> ConvertBody {
        let on = self.day(args, "date");
        let amount = self.arg(args, "amount");
        let reference = self.opt(args, "reference");
        let vendor = self.vendor(args);
        let charge_from = |from: AccountId| {
            BankPosting::card_charge(CardChargeDraft {
                pay_from: from,
                date: on,
                number: reference.clone(),
                payee: vendor,
                memo: None,
                lines: vec![split(self.exp, &amount)],
            })
            .expect("charge")
        };
        match args["kind"].as_str().expect("kind") {
            "bill" => ConvertBody::Bill(bill(
                vendor.expect("a bill names its vendor"),
                self.exp,
                on,
                reference.as_deref().unwrap_or(""),
                &amount,
            )),
            "card_charge" => ConvertBody::CardCharge(charge_from(self.card)),
            "cash_expense" => ConvertBody::CashExpense(charge_from(self.bank)),
            "check" => ConvertBody::Check(
                BankPosting::check(CheckDraft {
                    bank: self.bank,
                    date: on,
                    payee: vendor,
                    payee_name: "Synthetic payee".into(),
                    check_number: None,
                    print_later: true,
                    payee_address: None,
                    memo: None,
                    printed_memo: None,
                    tag_ids: Vec::new(),
                    draft_attachment: None,
                    amount: m(&amount),
                    lines: vec![split(self.exp, &amount)],
                })
                .expect("check"),
            ),
            other => panic!("kind {other}"),
        }
    }

    fn password(&self, args: &Value) -> PostingOptions {
        let pw = match args["closingPassword"].as_str() {
            None => return PostingOptions::default(),
            Some("right") => self.password.clone().expect("a closing password is set"),
            Some(_) => Secret::new("not the password"),
        };
        PostingOptions {
            closed_period: Some(cl_api::ClosedPeriodAuthorization::with_password(pw)),
        }
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one arm per op of the shared vocabulary"
    )]
    fn run(&mut self, op: &str, args: &Value) -> Answer {
        let h = self.h;
        let s = self.s.clone();
        match op {
            "upload_draft_file" => {
                let content = args["content"].as_str().expect("content");
                let dk = match args["draftKey"].as_str() {
                    Some(label) => *self.draft_keys.entry(label.to_string()).or_default(),
                    None => cl_api::DraftKey::new(),
                };
                let bytes = format!("%PDF-1.4\n% {content} {}\n%%EOF\n", self.salt);
                let a = h
                    .run(
                        s.attachments()
                            .upload_draft(dk, "synthetic.pdf", bytes.as_bytes(), key()),
                    )
                    .expect("upload");
                Answer::ok(201, json!({"id": a.id}))
            }
            "claim" => {
                let file = self.file_id(args);
                let hint: DraftKind =
                    serde_json::from_value(args["kindHint"].clone()).expect("kindHint");
                let k = self.key(args);
                Answer::of(201, h.run(s.document_drafts().create(file, hint, k)))
            }
            "get" => Answer::of(200, h.run(s.document_drafts().get(self.draft_id(args)))),
            "list" => {
                let statuses = args["status"]
                    .as_str()
                    .unwrap_or("")
                    .split(',')
                    .filter(|x| !x.is_empty())
                    .map(|x| serde_json::from_value(json!(x)).expect("status"))
                    .collect();
                let q = DraftListQuery {
                    statuses,
                    limit: args["limit"]
                        .as_u64()
                        .and_then(|l| u32::try_from(l).ok())
                        .unwrap_or(50),
                    ..DraftListQuery::default()
                };
                match h.run(s.document_drafts().list(&q)) {
                    Ok(p) => Answer {
                        total: Some(p.total),
                        ..Answer::ok(200, Value::Null)
                    },
                    Err(e) => Answer::err(&e),
                }
            }
            "store_reading" => {
                let r = DraftReading {
                    kind: serde_json::from_value(args["kind"].clone()).expect("kind"),
                    reader: DraftReader::Ocr,
                    fields: Map::new(),
                    lines: Vec::new(),
                    checks: Vec::new(),
                    suggested_contact: self.vendor(args),
                    doc_reference: self.opt(args, "docReference"),
                    doc_total: self.opt(args, "docTotal").map(|t| m(&t)),
                    doc_date: self
                        .opt(args, "docDate")
                        .map(|d| CalendarDate::parse_wire(&d).expect("date")),
                };
                let expected = self.arg(args, "expectedUpdatedAt");
                Answer::of(
                    200,
                    h.run(
                        s.document_drafts()
                            .store_reading(self.draft_id(args), &expected, &r),
                    ),
                )
            }
            "store_failure" => {
                let expected = self.arg(args, "expectedUpdatedAt");
                let reason = self.arg(args, "reason");
                Answer::of(
                    200,
                    h.run(s.document_drafts().store_failure(
                        self.draft_id(args),
                        &expected,
                        &reason,
                    )),
                )
            }
            "discard" => Answer::of(200, h.run(s.document_drafts().discard(self.draft_id(args)))),
            "convert" => {
                let conversion = DraftConversion {
                    body: self.body(args),
                    expected_updated_at: self.arg(args, "expectedUpdatedAt"),
                    acknowledge: args["acknowledge"]
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .map(|t| TransactionId::parse(&self.text(t)).expect("ack"))
                                .collect()
                        })
                        .unwrap_or_default(),
                };
                let opts = self.password(args);
                let k = self.key(args);
                match h.run(
                    s.document_drafts()
                        .convert(self.draft_id(args), &conversion, &opts, k),
                ) {
                    Ok(c) => Answer::draft(201, c.draft),
                    Err(e) => Answer::err(&e),
                }
            }
            "attach" => {
                let expected = self.arg(args, "expectedUpdatedAt");
                let k = self.key(args);
                match h.run(s.document_drafts().attach(
                    self.draft_id(args),
                    self.txn_id(args),
                    &expected,
                    k,
                )) {
                    Ok(a) => Answer::draft(200, a.draft),
                    Err(e) => Answer::err(&e),
                }
            }
            "link_attachment" => {
                let id = self.txn_id(args);
                let t = h.run(s.transactions().get(id)).expect("record").record;
                match h.run(s.attachments().link(
                    self.file_id(args),
                    AttachTo::Txn(t.txn_type, id),
                    key(),
                )) {
                    Ok(()) => Answer::ok(200, Value::Null),
                    Err(e) => Answer::err(&e),
                }
            }
            "plain_relink" => {
                let (kind, on) = file_type(h, &s, self.file_id(args));
                assert_eq!(kind, "draft", "plain_relink moves a draft key");
                let dk = cl_api::DraftKey::parse(&on).expect("draft key");
                let mut b = bill(self.story, self.exp, date(2026, 11, 2), "", "1.00");
                b.draft_attachment = Some(dk);
                h.run(s.bills().create(&b, &PostingOptions::default(), key()))
                    .unwrap_or_else(|e| panic!("plain save: {e:?}"));
                Answer::ok(200, Value::Null)
            }
            "post_expense" => {
                assert_eq!(args["payFrom"].as_str(), Some("card"), "payFrom");
                let p = BankPosting::card_charge(CardChargeDraft {
                    pay_from: self.card,
                    date: self.day(args, "date"),
                    number: self.opt(args, "reference"),
                    payee: self.vendor(args),
                    memo: None,
                    lines: vec![split(self.exp, &self.arg(args, "amount"))],
                })
                .expect("charge");
                let id = h
                    .run(s.transactions().create_bank_posting(
                        &p,
                        &PostingOptions::default(),
                        key(),
                    ))
                    .unwrap_or_else(|e| panic!("post: {e:?}"))
                    .record
                    .id;
                Answer::ok(201, json!({"id": id}))
            }
            "void" => {
                h.run(s.transactions().void(
                    self.txn_id(args),
                    "Synthetic void",
                    &PostingOptions::default(),
                    key(),
                ))
                .unwrap_or_else(|e| panic!("void: {e:?}"));
                Answer::ok(200, Value::Null)
            }
            "set_closing_date" => {
                h.run(s.raw_mapped(
                    Method::Put,
                    "/api/v1/company/settings",
                    Some(&json!({"lockDate": self.arg(args, "date")})),
                ))
                .expect("set the closing date");
                if self.password.is_none() {
                    let pw = Secret::new(format!("closing-{}", key().as_header()));
                    h.run(s.set_closing_password(None, Some(&pw), key()))
                        .expect("set the closing password");
                    self.password = Some(pw);
                }
                Answer::ok(200, Value::Null)
            }
            "search_amount" => {
                let filter = cl_api::TransactionFilter {
                    amount: Some(m(&self.arg(args, "amount"))),
                    status: Some(cl_api::TxnStatus::Posted),
                    start_date: Some(self.day(args, "start")),
                    end_date: Some(self.day(args, "end")),
                    ..cl_api::TransactionFilter::default()
                };
                match h.run(
                    s.transactions()
                        .list(&filter, cl_api::PageRequest::first_max()),
                ) {
                    Ok(p) => Answer {
                        count: Some(p.data.len()),
                        ..Answer::ok(200, Value::Null)
                    },
                    Err(e) => Answer::err(&e),
                }
            }
            other => panic!("op {other} is not played by the live runner"),
        }
    }

    /// Where the step's file is: `args.file`, else the draft's own file.
    fn file_place(&self, args: &Value) -> &'static str {
        let file = if args["file"].is_null() {
            let d = self
                .h
                .run(self.s.document_drafts().get(self.draft_id(args)))
                .expect("the draft");
            d.attachment_id.expect("the draft's file")
        } else {
            self.file_id(args)
        };
        if file_type(self.h, &self.s, file).0 == "draft" {
            "draft"
        } else {
            "moved"
        }
    }
}

/// Discards every live draft of the session's company.
fn tidy(h: &Harness, s: &Session) {
    let q = DraftListQuery {
        statuses: vec![DraftStatus::New, DraftStatus::Ready, DraftStatus::Failed],
        limit: 200,
        summary: true,
        ..DraftListQuery::default()
    };
    loop {
        let page = h.run(s.document_drafts().list(&q)).expect("live drafts");
        if page.drafts.is_empty() {
            break;
        }
        for d in page.drafts {
            h.run(s.document_drafts().discard(d.id)).expect("discard");
        }
    }
}

/// T16: the shared cases give the same answers on the engine as on the
/// demo. `claim`, `store`, `discard` and `convert` need `document-drafts-v1`;
/// `attach`, `guard` and `amount_filter` need `document-drafts-attach-v1`
/// (PENDING(U-95) before it).
fn shared_draft_cases(h: &Harness) {
    let s = &h.s1;
    if !live(h, s) {
        return;
    }
    let attach_on = h.require_cap(Capabilities::DOCUMENT_DRAFTS_ATTACH, U95);
    let v: Value = serde_json::from_str(DRAFT_CASES).expect("draft-cases.json");
    // Tenant B, on the switcher session.
    let runner = h.s3();
    tidy(h, &runner);
    let books = [
        account(h, &runner, AccountType::Expense, None),
        account(h, &runner, AccountType::Liability, Some("credit_card")),
        account(h, &runner, AccountType::Asset, Some("bank")),
    ];
    let mut ran = 0;
    let mut failed = Vec::new();
    for section in [
        "claim",
        "store",
        "discard",
        "convert",
        "attach",
        "guard",
        "amount_filter",
    ] {
        if !attach_on && matches!(section, "attach" | "guard" | "amount_filter") {
            continue;
        }
        for case in v[section].as_array().expect(section) {
            let name = case["name"].as_str().expect("name");
            let mut w = CaseWorld::new(h, &runner, books);
            for (i, step) in case["steps"].as_array().expect("steps").iter().enumerate() {
                let op = step["op"].as_str().expect("op");
                let args = &step["args"];
                let expect = &step["expect"];
                let at = format!("{name} step {i} ({op})");
                let out = w.run(op, args);
                let mut wrong = Vec::new();
                if u64::from(out.status) != expect["status"].as_u64().expect("status") {
                    wrong.push(format!("status {} code {:?}", out.status, out.code));
                }
                if let Some(code) = expect["code"].as_str()
                    && out.code.as_deref() != Some(code)
                {
                    wrong.push(format!("code {:?}", out.code));
                }
                if let Some(st) = expect["draft_status"].as_str() {
                    let got = out.draft.as_ref().map(|d| d.status.as_str());
                    if got != Some(st) {
                        wrong.push(format!("draft status {got:?}"));
                    }
                }
                if let Some(t) = expect["total"].as_u64()
                    && out.total != Some(t)
                {
                    wrong.push(format!("total {:?}", out.total));
                }
                if let Some(c) = expect["count"].as_u64()
                    && out.count.and_then(|n| u64::try_from(n).ok()) != Some(c)
                {
                    wrong.push(format!("count {:?}", out.count));
                }
                if let Some(place) = expect["file"].as_str() {
                    let got = w.file_place(args);
                    if got != place {
                        wrong.push(format!("file {got}"));
                    }
                }
                if !wrong.is_empty() {
                    failed.push(format!("{at}: {}", wrong.join(", ")));
                    break;
                }
                if let Some(label) = step["as"].as_str() {
                    w.labels.insert(label.to_string(), out.label);
                }
            }
            w.clean_up();
            ran += 1;
        }
    }
    assert!(
        failed.is_empty(),
        "the engine answers differently from the shared cases:\n{}",
        failed.join("\n")
    );
    assert!(ran >= 23, "only {ran} cases");
    eprintln!("    shared draft cases: {ran} ran on the engine");
}

// ------------------------------------------------- A06 statement drafts

const U94: &str = "U-94";

/// A06 (engine U-94, `document-drafts-statement-v1`): a file claimed as a
/// statement draft keeps the kind, its reading stores as `statement`, and
/// "Save with the vendor" files it on the vendor the way the desktop does
/// (review S1): the file can't be moved while the draft reads it, so a
/// copy is uploaded to the vendor, then the draft is discarded and its own
/// file deleted. Without the capability, records PENDING(U-94) and checks
/// today's behaviour: the typed claim sends nothing, the engine refuses the
/// kind, and a statement reading goes as `unknown`.
fn statement_drafts(h: &Harness) {
    let s = &h.s1;
    if !live(h, s) {
        return;
    }
    let mut r = reading(None, None, None, None);
    r.kind = DraftKind::Statement;
    if !h.require_cap(Capabilities::DOCUMENT_DRAFTS_STATEMENT, U94) {
        let file = upload(h, s, &pdf());
        let before = h.observer.total();
        assert_eq!(
            h.run(
                s.document_drafts()
                    .create(file, DraftKind::Statement, key())
            ),
            Err(ApiError::Local(LocalValidation::Unsupported))
        );
        assert_eq!(h.observer.total(), before, "nothing sent");
        let raw = h
            .run(s.raw(
                Method::Post,
                BASE,
                &[],
                Some(&json!({"attachmentId": file, "kindHint": "statement"})),
            ))
            .expect("raw");
        assert_eq!(raw.status, 400, "{}", raw.body);
        // An unhinted reading the reader took for a statement is kept.
        let d = claim(h, s, file);
        let stored = store(h, s, &d, &r);
        assert_eq!(stored.kind, DraftKind::Unknown);
        h.run(s.document_drafts().discard(d.id)).expect("discard");
        return;
    }
    let bytes = pdf();
    let file = upload(h, s, &bytes);
    let d = h
        .run(
            s.document_drafts()
                .create(file, DraftKind::Statement, key()),
        )
        .unwrap_or_else(|e| panic!("claim: {e:?}"));
    assert_eq!(d.kind, DraftKind::Statement);
    let d = store(h, s, &d, &r);
    assert_eq!(d.kind, DraftKind::Statement);
    assert_eq!(d.status, DraftStatus::Ready);
    let v = vendor(h, s);
    let to = AttachTo::Contact(v);
    let e = h
        .run(s.attachments().link(file, to, key()))
        .expect_err("a live draft's file doesn't move");
    assert!(
        matches!(&e, ApiError::Conflict { reason: ConflictReason::InUse, code: Some(c), .. }
            if c.as_str() == cl_api::ATTACHMENT_IN_USE),
        "{e:?}"
    );
    let name = "Statement 2026-10-01 to 2026-10-31.pdf";
    let upload_key = key();
    let kept = h
        .run(s.attachments().upload(to, name, &bytes, upload_key.clone()))
        .unwrap_or_else(|e| panic!("keep: {e:?}"));
    // Try again with the same key files nothing twice.
    let again = h
        .run(s.attachments().upload(to, name, &bytes, upload_key))
        .unwrap_or_else(|e| panic!("replay: {e:?}"));
    assert_eq!(again.id, kept.id);
    assert_eq!(kept.attachable_type, "contact");
    assert_eq!(kept.attachable_id, v.to_string());
    assert_eq!(kept.file_name, name);
    let gone = h
        .run(s.document_drafts().discard(d.id))
        .unwrap_or_else(|e| panic!("discard: {e:?}"));
    assert_eq!(gone.status, DraftStatus::Discarded);
    h.run(s.attachments().delete(file, key()))
        .expect("a discarded draft's file deletes");
    let on_vendor = h
        .run(
            s.attachments()
                .list(to, cl_api::PageRequest::new(50, 0).expect("page")),
        )
        .unwrap_or_else(|e| panic!("list: {e:?}"));
    assert_eq!(on_vendor.total, 1);
    assert_eq!(on_vendor.data[0].id, kept.id);
}

fn main() -> std::process::ExitCode {
    let tests: &[(&str, TestFn)] = &[
        ("drafts::claims", claims),
        ("drafts::readings_and_tokens", readings_and_tokens),
        (
            "drafts::files_of_live_and_converted_drafts",
            files_of_live_and_converted_drafts,
        ),
        ("drafts::each_kind_posts_its_body", each_kind_posts_its_body),
        (
            "drafts::duplicates_block_until_acknowledged",
            duplicates_block_until_acknowledged,
        ),
        (
            "drafts::closed_period_rollback_and_scope",
            closed_period_rollback_and_scope,
        ),
        (
            "drafts::drafts_leave_the_books_alone",
            drafts_leave_the_books_alone,
        ),
        ("drafts::card_last4", card_last4),
        ("drafts::templates_crud", templates_crud),
        (
            "drafts::contact_merge_moves_drafts_and_layouts",
            contact_merge_moves_drafts_and_layouts,
        ),
        (
            "drafts::backups_keep_and_transfers_omit",
            backups_keep_and_transfers_omit,
        ),
        (
            "drafts::reference_table_matches_the_engine",
            reference_table_matches_the_engine,
        ),
        (
            "drafts::money_table_matches_the_engine",
            money_table_matches_the_engine,
        ),
        // A04 (U-95).
        ("drafts::attach_matrix", attach_matrix),
        ("drafts::amount_filter_is_exact", amount_filter_is_exact),
        // A06 (U-94).
        ("drafts::statement_drafts", statement_drafts),
        // Last: it opens a fresh company per case on the switcher session.
        ("drafts::shared_draft_cases", shared_draft_cases),
    ];
    run_suite(EngineMode::Normal, tests)
}
