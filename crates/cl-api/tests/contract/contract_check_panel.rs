//! The Write Check payee panel, attachments and suggestions against the
//! live engine (P13 WP2, plan §6.2): the vendor's open-bill summary and
//! payable bills after a bill is entered, the suggested account (default
//! vs recent), the check number on the payee's transactions (PENDING E5
//! while the engine list lacks it), a draft attachment moved onto a check
//! by the check create, local file refusals that send nothing, rule
//! suggestions (PENDING `rules-evaluate-v1`) and the AI preview refused
//! while AI is off (no network: the engine refuses before any provider).
//!
//! Checks are written with raw `POST /checks` bodies here so this file does
//! not depend on the check builder WP1 extends.

use cl_api::{
    AccountType, AiGate, AiPreviewRow, ApiError, AttachTo, BillLine, Capabilities, Contact,
    ContactFields, ContactType, DraftKey, LocalValidation, Method, NewBill, NewContact,
    PageRequest, RuleQuery, Session, SuggestSource, TxnType,
};
use cl_core::Money;
use cl_core::ids::{AccountId, TransactionId};
use serde_json::{Value, json};

use crate::common::{account, date, key, m};
use crate::contract_bank_postings::detail_account;
use crate::support::{Harness, unique};

const PDF: &[u8] = b"%PDF-1.4\n% synthetic contract fixture\n%%EOF\n";

fn vendor(h: &Harness, s: &Session, default_account: Option<AccountId>) -> Contact {
    let mut fields = ContactFields::named(ContactType::Vendor, unique("Feed & Seed"));
    fields.default_expense_account_id = default_account;
    h.run(s.contacts().create(
        &NewContact {
            fields,
            opening_balance: Money::ZERO,
            opening_balance_date: None,
        },
        key(),
    ))
    .unwrap_or_else(|e| panic!("create vendor: {e}"))
    .record
}

/// A check through the raw route: `(id, body)`.
fn raw_check(h: &Harness, s: &Session, body: &Value) -> (TransactionId, Value) {
    let reply = h
        .run(s.raw(Method::Post, "/api/v1/checks", &[], Some(body)))
        .expect("raw check");
    assert_eq!(reply.status, 201, "{}", reply.body);
    let id = reply.body["check"]["id"]
        .as_str()
        .and_then(|s| TransactionId::parse(s).ok())
        .expect("check id");
    (id, reply.body)
}

/// Vendor summary and payable bills after a bill; the suggested account
/// moves from none to the bill's account (recent), and a vendor default
/// wins (Q-192).
pub fn panel_readers_after_a_bill(h: &Harness) {
    let s = &h.s1;
    let mulch = account(h, s, AccountType::Expense, "Mulch & stone");
    let plants = account(h, s, AccountType::Expense, "Plants");
    let v = vendor(h, s, None);
    assert_eq!(
        h.run(s.contacts().suggest_account(v.id)).expect("suggest"),
        None,
        "no default and no history"
    );
    let empty = h.run(s.bills().vendor_summary(v.id)).expect("summary");
    assert_eq!((empty.unpaid_count, empty.total_owed), (0, Money::ZERO));
    assert_eq!(h.run(s.bills().payable(v.id)).expect("payable").len(), 0);

    h.run(s.create_bill(
        &NewBill {
            contact_id: v.id,
            txn_date: date(2026, 10, 6),
            due_date: Some(date(2099, 11, 5)),
            vendor_invoice_number: Some(unique("F")),
            memo: None,
            lines: vec![BillLine {
                account_id: mulch.id,
                description: None,
                amount: m("1286.4000"),
            }],
        },
        key(),
    ))
    .expect("bill");

    let sum = h.run(s.bills().vendor_summary(v.id)).expect("summary");
    assert_eq!(sum.unpaid_count, 1);
    assert_eq!(sum.total_owed.to_wire(), "1286.4000");
    assert_eq!((sum.overdue_count, sum.total_overdue), (0, Money::ZERO));
    let bills = h.run(s.bills().payable(v.id)).expect("payable");
    assert_eq!(bills.len(), 1);
    assert_eq!(bills[0].amount_due.to_wire(), "1286.4000");
    assert_eq!(bills[0].due, Some(date(2099, 11, 5)));
    assert_eq!(bills[0].days_overdue, 0);

    // Money is a JSON string at the raw level, never a number.
    let raw = h
        .run(s.raw(
            Method::Get,
            "/api/v1/bills/vendor-summary",
            &[("contactId", &v.id.to_string())],
            None,
        ))
        .expect("raw summary");
    assert!(raw.body["summary"]["totalOwed"].is_string(), "{}", raw.body);
    let raw = h
        .run(s.raw(
            Method::Get,
            "/api/v1/bills/payable",
            &[("contactId", &v.id.to_string())],
            None,
        ))
        .expect("raw payable");
    assert!(
        raw.body["bills"][0]["balanceDue"].is_string(),
        "{}",
        raw.body
    );

    let recent = h
        .run(s.contacts().suggest_account(v.id))
        .expect("suggest")
        .expect("a suggestion");
    assert_eq!(
        (recent.account, recent.source),
        (mulch.id, SuggestSource::Recent)
    );
    let with_default = vendor(h, s, Some(plants.id));
    let d = h
        .run(s.contacts().suggest_account(with_default.id))
        .expect("suggest")
        .expect("default");
    assert_eq!((d.account, d.source), (plants.id, SuggestSource::Default));
}

/// Recent checks: the payee's transactions list the check; its number is
/// read when the engine sends it (plan E5 otherwise).
pub fn recent_checks_carry_the_number(h: &Harness) {
    let s = &h.s1;
    let bank = detail_account(h, s, AccountType::Asset, "bank");
    let mulch = account(h, s, AccountType::Expense, "Recent mulch");
    let v = vendor(h, s, None);
    let (id, _) = raw_check(
        h,
        s,
        &json!({
            "bankAccountId": bank, "contactId": v.id,
            "payeeNameOnCheck": v.fields.display_name, "txnDate": "2026-09-18",
            "amount": "1904.1100", "checkNumber": 4392,
            "lines": [{"accountId": mulch.id, "amount": "1904.1100"}],
        }),
    );
    let page = h
        .run(
            s.contacts()
                .transactions(v.id, PageRequest::new(10, 0).expect("page")),
        )
        .expect("history");
    let row = page
        .data
        .iter()
        .find(|r| r.id == id)
        .expect("the check is listed");
    assert_eq!(row.txn_type, TxnType::Expense);
    assert_eq!(
        row.display_total.map(Money::to_wire).as_deref(),
        Some("1904.1100")
    );
    match row.check_number {
        Some(n) => assert_eq!(n, 4392),
        None => h.pending(
            "E5",
            "contacts/{id}/transactions does not carry checkNumber; recent checks show a dash",
        ),
    }
}

/// A file uploaded to a draft moves onto the check the create makes; the
/// download returns the same bytes. Refused files send nothing.
pub fn draft_attachment_moves_onto_the_check(h: &Harness) {
    let s = &h.s1;
    let bank = detail_account(h, s, AccountType::Asset, "bank");
    let mulch = account(h, s, AccountType::Expense, "Attach mulch");
    let draft = DraftKey::new();
    let up = h
        .run(
            s.attachments()
                .upload_draft(draft, "receipt.pdf", PDF, key()),
        )
        .unwrap_or_else(|e| panic!("upload: {e}"));
    assert_eq!(up.attachable_type, "draft");
    assert_eq!(up.file_size, Some(PDF.len() as u64));
    let first = PageRequest::new(50, 0).expect("page");
    let on_draft = h
        .run(s.attachments().list(AttachTo::Draft(draft), first))
        .expect("draft list");
    assert_eq!(on_draft.total, 1);

    let (check, _) = raw_check(
        h,
        s,
        &json!({
            "bankAccountId": bank, "payeeNameOnCheck": unique("Payee"),
            "txnDate": "2026-10-20", "amount": "12.5000",
            "lines": [{"accountId": mulch.id, "amount": "12.5000"}],
            "draftAttachmentId": draft,
        }),
    );
    let on_check = h
        .run(s.attachments().list(AttachTo::Expense(check), first))
        .expect("check list");
    assert_eq!(on_check.total, 1, "the draft file moved onto the check");
    assert_eq!(on_check.data[0].id, up.id);
    assert_eq!(on_check.data[0].attachable_type, "expense");
    let left = h
        .run(s.attachments().list(AttachTo::Draft(draft), first))
        .expect("draft list");
    assert_eq!(left.total, 0);
    let bytes = h.run(s.attachments().download(up.id)).expect("download");
    assert_eq!(bytes, PDF);

    // Posted after save: straight onto the check.
    let png = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0];
    h.run(s.attachments().attach(check, "scan.png", &png, key()))
        .expect("attach");
    let on_check = h
        .run(s.attachments().list(AttachTo::Expense(check), first))
        .expect("check list");
    assert_eq!(on_check.total, 2);

    let before = h.observer.total();
    let mut big = b"%PDF-".to_vec();
    big.resize(cl_api::ATTACHMENT_MAX_BYTES + 1, b' ');
    let e = h
        .run(s.attachments().attach(check, "big.pdf", &big, key()))
        .expect_err("too big");
    assert_eq!(e, ApiError::Local(LocalValidation::AttachmentTooLarge));
    let e = h
        .run(s.attachments().attach(check, "x.exe", b"MZ\x90\0", key()))
        .expect_err("type");
    assert_eq!(e, ApiError::Local(LocalValidation::AttachmentType));
    assert_eq!(h.observer.total(), before, "nothing may be sent");
}

/// Rule suggestions: PENDING until the engine advertises
/// `rules-evaluate-v1`; meanwhile refused locally with nothing sent.
pub fn rules_evaluate(h: &Harness) {
    let s = &h.s1;
    let bank = detail_account(h, s, AccountType::Asset, "bank");
    let q = RuleQuery {
        description: unique("Ozark Feed"),
        amount: m("1286.4000"),
        contact: None,
        bank_account: bank,
    };
    if h.require_cap(Capabilities::RULES_EVALUATE, "rules-evaluate-v1") {
        // A fresh company has no rules: nothing matches.
        assert_eq!(h.run(s.rules().evaluate(&q)).expect("evaluate"), None);
    } else {
        let before = h.observer.total();
        assert_eq!(
            h.run(s.rules().evaluate(&q)).expect_err("gated"),
            ApiError::Local(LocalValidation::RulesEvaluateUnavailable)
        );
        assert_eq!(h.observer.total(), before, "nothing may be sent");
    }
}

/// AI preview while AI is off: the owner-side gate sends nothing; with the
/// gate claimed open, the engine's own AI gate refuses before any provider
/// is called, and that maps to `AiUnavailable`.
pub fn ai_preview_is_refused_while_ai_is_off(h: &Harness) {
    let s = &h.s1;
    let rows = [AiPreviewRow {
        description: unique("Ozark Feed"),
        amount: m("1286.4000"),
    }];
    let before = h.observer.total();
    assert_eq!(
        h.run(s.ai_categorize_preview(AiGate::default(), &rows))
            .expect_err("off"),
        ApiError::AiUnavailable
    );
    assert_eq!(h.observer.total(), before, "nothing may be sent");
    let claimed = AiGate {
        ai_enabled: true,
        consented: true,
        categorization_on: true,
        provider_reachable: true,
    };
    assert_eq!(
        h.run(s.ai_categorize_preview(claimed, &rows))
            .expect_err("engine gate"),
        ApiError::AiUnavailable
    );
}
