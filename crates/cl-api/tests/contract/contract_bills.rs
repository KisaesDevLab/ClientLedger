//! Enter Bill and vendor credits against the live engine (P17 WP2, plan Â§6
//! AC1 and the phase tests). Tests that need engine PR-A (U-83..U-87) or
//! PR-B (U-89) run for real from pin f3ce33fd; against an older engine they
//! are PENDING by capability.
//!
//! Money is checked as exact strings; each test uses its own vendor so the
//! engine's figures can be compared exactly.

use std::time::{Duration, Instant};

use cl_api::{
    AccountFilter, AccountType, ApWriteOptions, ApiError, ArchiveRequest, AttachTo, BillDraft,
    BillLineDraft, BillListQuery, BillTerms, Capabilities, ContactFields, ContactType, DraftKey,
    EditableFields, ImportRequest, JobState, LocalValidation, Method, NewContact, PageRequest,
    PostingOptions, Session, TxnStatus, VendorCreditDraft,
};
use cl_core::ids::{AccountId, ContactId, TransactionId};
use cl_core::{Money, Secret};
use serde_json::json;

use crate::common::{account, date, key, m};
use crate::contract_bank_postings::detail_account;
use crate::support::{Harness, unique};

const PDF: &[u8] = b"%PDF-1.4\n% synthetic contract bill\n%%EOF\n";

fn vendor(h: &Harness, s: &Session) -> ContactId {
    h.run(s.contacts().create(
        &NewContact {
            fields: ContactFields::named(ContactType::Vendor, unique("Midwest Equipment Rental")),
            opening_balance: Money::ZERO,
            opening_balance_date: None,
        },
        key(),
    ))
    .unwrap_or_else(|e| panic!("vendor: {e}"))
    .record
    .id
}

/// The mockup's bill (synthetic): 1,330.00 + 150.00 = 1,480.00.
fn mockup_bill(v: ContactId, a: AccountId, b: AccountId, reference: &str) -> BillDraft {
    BillDraft {
        vendor: v,
        date: date(2026, 11, 3),
        vendor_invoice_number: Some(reference.to_string()),
        terms: Some(BillTerms::code("net_15")),
        due_date: None,
        memo: None,
        amount_due: m("1480.00"),
        lines: vec![
            BillLineDraft {
                description: Some("Excavator, 1 day".into()),
                ..BillLineDraft::expense(a, m("1330.00"))
            },
            BillLineDraft::expense(b, m("150.00")),
        ],
        received: true,
        draft_attachment: None,
    }
}

fn payables_account(h: &Harness, s: &Session) -> AccountId {
    h.run(s.accounts().list_all(&AccountFilter::default()))
        .expect("accounts")
        .into_iter()
        .find(|a| a.system_tag.as_deref() == Some("accounts_payable"))
        .expect("the A/P system account")
        .id
}

fn balance(h: &Harness, s: &Session, a: AccountId) -> Money {
    h.run(s.accounts().get(a))
        .expect("account")
        .record
        .balance
        .unwrap_or(Money::ZERO)
}

/// AC1 (expense half): the bill posts Dr each expense, Cr Accounts Payable
/// with the exact total; the saved record is what the form wrote.
pub fn expense_bill_posts_to_ap_live(h: &Harness) {
    let s = &h.s1;
    let rental = account(h, s, AccountType::Expense, "Equipment rental");
    let delivery = account(h, s, AccountType::Expense, "Delivery");
    let ap = payables_account(h, s);
    let ap_before = balance(h, s, ap);
    let v = vendor(h, s);
    let reference = unique("INV-55871").replace(' ', "-");
    let draft = mockup_bill(v, rental.id, delivery.id, &reference);
    let saved = h
        .run(s.bills().create(&draft, &PostingOptions::default(), key()))
        .unwrap_or_else(|e| panic!("create: {e:?}"));
    let id = saved.record.txn.id;
    assert_eq!(saved.record.txn.total, Some(m("1480.0000")));
    assert_eq!(saved.record.balance_due, Some(m("1480.0000")));
    assert_eq!(
        saved.record.vendor_invoice_number.as_deref(),
        Some(reference.as_str())
    );
    assert_eq!(saved.record.due_date, Some(date(2026, 11, 18)), "net 15");
    assert_eq!(
        saved.record.fingerprint(),
        draft.fingerprint(),
        "the record is what the form wrote"
    );
    let read = h.run(s.bills().get(id)).expect("get");
    assert_eq!(
        read.version, saved.version,
        "the read has the save's version"
    );

    let txn = h.run(s.transactions().get(id)).expect("txn").record;
    let mut lines: Vec<_> = txn.lines.iter().collect();
    lines.sort_by_key(|l| l.line_order.unwrap_or(0));
    let got: Vec<(AccountId, String, String)> = lines
        .iter()
        .map(|l| (l.account_id, l.debit.to_wire(), l.credit.to_wire()))
        .collect();
    assert_eq!(
        got,
        vec![
            (rental.id, "1330.0000".into(), "0.0000".into()),
            (delivery.id, "150.0000".into(), "0.0000".into()),
            (ap, "0.0000".into(), "1480.0000".into()),
        ]
    );
    assert_eq!(balance(h, s, rental.id), m("1330.0000"));
    assert_eq!(balance(h, s, delivery.id), m("150.0000"));
    let moved = balance(h, s, ap)
        .checked_sub(ap_before)
        .expect("delta")
        .abs();
    assert_eq!(moved, m("1480.0000"), "A/P moved by exactly the total");
    // Clean up: the shared engine's later tests read A/P too.
    h.run(
        s.bills()
            .void(id, "contract clean-up", &ApWriteOptions::default(), key()),
    )
    .expect("void");
}

/// AC1: the vendor's open balance (the engine's vendor summary) moves by
/// exactly the bill total.
pub fn vendor_summary_moves_by_the_total_live(h: &Harness) {
    let s = &h.s1;
    let rental = account(h, s, AccountType::Expense, "Rental");
    let delivery = account(h, s, AccountType::Expense, "Freight");
    let v = vendor(h, s);
    let before = h.run(s.bills().vendor_summary(v)).expect("summary");
    assert_eq!((before.unpaid_count, before.total_owed), (0, Money::ZERO));
    let reference = unique("INV").replace(' ', "-");
    h.run(s.bills().create(
        &mockup_bill(v, rental.id, delivery.id, &reference),
        &PostingOptions::default(),
        key(),
    ))
    .expect("create");
    let after = h.run(s.bills().vendor_summary(v)).expect("summary");
    assert_eq!(after.unpaid_count, before.unpaid_count + 1);
    assert_eq!(
        after
            .total_owed
            .checked_sub(before.total_owed)
            .expect("delta"),
        m("1480.0000")
    );
    // The bills list finds it by reference (the D3 duplicate source).
    let found = h
        .run(s.bills().list_capped(
            &BillListQuery {
                vendor: Some(v),
                search: Some(reference.clone()),
                ..BillListQuery::default()
            },
            50,
            20,
        ))
        .expect("list");
    assert!(found.complete);
    assert_eq!(found.rows.len(), 1);
    assert_eq!(found.rows[0].status, TxnStatus::Posted);
    assert_eq!(found.rows[0].total, Some(m("1480.0000")));
}

/// U-84d: a voided unpaid bill leaves the vendor summary (PENDING until the
/// pin carries `ap-edit-v1`; the pinned engine still counts it, plan R7).
pub fn void_bill_drops_from_summary_u84(h: &Harness) {
    if !h.require_cap(Capabilities::AP_EDIT, "U-84") {
        return;
    }
    let s = &h.s1;
    let a = account(h, s, AccountType::Expense, "Void rental");
    let b = account(h, s, AccountType::Expense, "Void freight");
    let v = vendor(h, s);
    let saved = h
        .run(s.bills().create(
            &mockup_bill(v, a.id, b.id, &unique("INV").replace(' ', "-")),
            &PostingOptions::default(),
            key(),
        ))
        .expect("create");
    h.run(s.bills().void(
        saved.record.txn.id,
        "typed twice",
        &ApWriteOptions {
            posting: PostingOptions::default(),
            expected_updated_at: saved.record.txn.updated_at,
        },
        key(),
    ))
    .expect("void");
    let sum = h.run(s.bills().vendor_summary(v)).expect("summary");
    assert_eq!((sum.unpaid_count, sum.total_owed), (0, Money::ZERO));
}

/// Void is refused while a payment is applied (the payment is made with a
/// raw `POST /bill-payments`; Pay Bills is P18).
pub fn void_refused_when_paid_live(h: &Harness) {
    let s = &h.s1;
    let a = account(h, s, AccountType::Expense, "Paid rental");
    let b = account(h, s, AccountType::Expense, "Paid freight");
    let bank = detail_account(h, s, AccountType::Asset, "bank");
    let v = vendor(h, s);
    let saved = h
        .run(s.bills().create(
            &mockup_bill(v, a.id, b.id, &unique("INV").replace(' ', "-")),
            &PostingOptions::default(),
            key(),
        ))
        .expect("create");
    let id = saved.record.txn.id;
    let paid = h
        .run(s.raw(
            Method::Post,
            "/api/v1/bill-payments",
            &[],
            Some(&json!({
                "bankAccountId": bank.to_string(), "txnDate": "2026-11-10", "method": "ach",
                "bills": [{ "billId": id.to_string(), "amount": "1480.00" }],
            })),
        ))
        .expect("raw pay");
    assert_eq!(paid.status, 201, "{}", paid.body);
    let e = h
        .run(
            s.bills()
                .void(id, "wrong vendor", &ApWriteOptions::default(), key()),
        )
        .expect_err("a paid bill is not voided");
    if h.require_cap(Capabilities::AP_EDIT, "U-84") {
        // `ap-edit-v1`: 409 `BILL_HAS_PAYMENTS`.
        assert_eq!(
            cl_api::ap_refusal(&e),
            Some(cl_api::ApRefusal::BillHasPayments),
            "{e:?}"
        );
    } else {
        assert!(matches!(e, ApiError::Validation(_)), "{e:?}");
    }
    let after = h.run(s.bills().get(id)).expect("get").record;
    assert_eq!(after.txn.status, TxnStatus::Posted);
    assert!(after.has_payments());
    assert_eq!(after.balance_due, Some(m("0.0000")));
}

/// A vendor credit posts Dr A/P, Cr the expense, and shows as available
/// with exactly its amount.
pub fn vendor_credit_create_and_available_live(h: &Harness) {
    let s = &h.s1;
    let rental = account(h, s, AccountType::Expense, "Credit rental");
    let ap = payables_account(h, s);
    let v = vendor(h, s);
    let draft = VendorCreditDraft {
        vendor: v,
        date: date(2026, 11, 4),
        vendor_invoice_number: Some(unique("CM").replace(' ', "-")),
        memo: Some("Returned bucket".into()),
        amount: m("150.00"),
        lines: vec![BillLineDraft::expense(rental.id, m("150.00"))],
        draft_attachment: None,
    };
    let saved = h
        .run(
            s.vendor_credits()
                .create(&draft, &PostingOptions::default(), key()),
        )
        .unwrap_or_else(|e| panic!("credit: {e:?}"));
    let id = saved.record.txn.id;
    assert_eq!(saved.record.txn.total, Some(m("150.0000")));
    assert_eq!(saved.record.lines.len(), 1);
    assert_eq!(
        saved.record.lines[0].description.as_deref(),
        Some("Returned bucket"),
        "a blank line description takes the memo"
    );
    let txn = h.run(s.transactions().get(id)).expect("txn").record;
    let mut lines: Vec<_> = txn
        .lines
        .iter()
        .map(|l| (l.account_id, l.debit.to_wire(), l.credit.to_wire()))
        .collect();
    lines.sort();
    let mut want = vec![
        (ap, "150.0000".to_string(), "0.0000".to_string()),
        (rental.id, "0.0000".to_string(), "150.0000".to_string()),
    ];
    want.sort();
    assert_eq!(lines, want);
    let avail = h.run(s.vendor_credits().available(v)).expect("available");
    assert_eq!(avail.len(), 1);
    assert_eq!((avail[0].id, avail[0].balance_due), (id, m("150.0000")));
    let sum = h.run(s.bills().vendor_summary(v)).expect("summary");
    assert_eq!(
        (sum.available_credits_count, sum.total_available_credits),
        (1, m("150.0000"))
    );
    // Before ap-edit-v1 a saved credit can only be voided.
    if !s.client().capabilities().has(Capabilities::AP_EDIT) {
        let before = h.observer.total();
        let e = h
            .run(
                s.vendor_credits()
                    .update(id, &saved, &draft, &PostingOptions::default(), key()),
            )
            .expect_err("gated");
        assert_eq!(e, ApiError::Local(LocalValidation::Unsupported));
        assert_eq!(h.observer.total(), before, "nothing may be sent");
    }
    h.run(
        s.vendor_credits()
            .void(id, "contract clean-up", &ApWriteOptions::default(), key()),
    )
    .expect("void");
    assert_eq!(
        h.run(s.vendor_credits().available(v))
            .expect("available")
            .len(),
        0
    );
}

/// The two-call attachment path before `bill-lines-v1`: a file uploaded to
/// a draft moves onto the saved bill with `attach-draft`; again moves
/// nothing.
pub fn attach_draft_live(h: &Harness) {
    let s = &h.s1;
    let a = account(h, s, AccountType::Expense, "Attach rental");
    let b = account(h, s, AccountType::Expense, "Attach freight");
    let v = vendor(h, s);
    let draft_key = DraftKey::new();
    let up = h
        .run(
            s.attachments()
                .upload_draft(draft_key, "midwest_inv_55871.pdf", PDF, key()),
        )
        .expect("upload");
    let saved = h
        .run(s.bills().create(
            &mockup_bill(v, a.id, b.id, &unique("INV").replace(' ', "-")),
            &PostingOptions::default(),
            key(),
        ))
        .expect("create");
    let id = saved.record.txn.id;
    let moved = h
        .run(s.bills().attach_draft(id, draft_key, key()))
        .expect("attach draft");
    assert_eq!(moved, 1);
    let first = PageRequest::new(50, 0).expect("page");
    let on_bill = h
        .run(s.attachments().list(AttachTo::Bill(id), first))
        .expect("list");
    assert_eq!(on_bill.total, 1);
    assert_eq!(on_bill.data[0].id, up.id);
    assert_eq!(on_bill.data[0].attachable_type, "bill");
    assert_eq!(h.run(s.attachments().download(up.id)).expect("bytes"), PDF);
    assert_eq!(
        h.run(s.bills().attach_draft(id, draft_key, key()))
            .expect("again"),
        0,
        "nothing left on the draft"
    );
    let got = h.run(s.attachments().get(up.id)).expect("get");
    assert_eq!(got.attachable_id, id.to_string());
}

/// U-90 (`attachments-company-v1`): an upload is stamped with the open
/// company (cl-api sends `X-Company-Id` on every attachment route), reads
/// and the download work in that company, and another company's session
/// does not see the file.
pub fn attachments_company_u90_live(h: &Harness) {
    if !h.require_cap(Capabilities::ATTACHMENTS_COMPANY, "U-90") {
        return;
    }
    let s = &h.s1;
    let cl_api::Scope::Company { company_id, .. } = s.scope() else {
        panic!("Contract Co is open");
    };
    let up = h
        .run(
            s.attachments()
                .upload_draft(DraftKey::new(), "u90.pdf", PDF, key()),
        )
        .expect("upload");
    let path = format!("/api/v1/attachments/{}", up.id);
    let got = h
        .run(s.raw(Method::Get, &path, &[], None))
        .expect("get raw");
    assert_eq!(got.status, 200, "{}", got.body);
    assert_eq!(
        got.body["attachment"]["companyId"].as_str(),
        Some(company_id.to_string().as_str()),
        "the upload carries the open company"
    );
    assert_eq!(h.run(s.attachments().download(up.id)).expect("bytes"), PDF);
    let other = h.s3();
    let e = h
        .run(other.attachments().get(up.id))
        .expect_err("another company's file");
    assert!(matches!(e, ApiError::NotFound { .. }), "{e:?}");
    h.run(s.attachments().delete(up.id, key())).expect("delete");
}

/// Exactly 10 MB is accepted; 10 MB + 1 is refused by cl-api with nothing
/// sent, and by the engine (a raw multipart upload).
pub fn attachment_10mb_edge_live(h: &Harness) {
    let s = &h.s1;
    let draft_key = DraftKey::new();
    let mut exact = b"%PDF-1.4\n".to_vec();
    exact.resize(cl_api::ATTACHMENT_MAX_BYTES, b' ');
    let up = h
        .run(
            s.attachments()
                .upload_draft(draft_key, "exact.pdf", &exact, key()),
        )
        .unwrap_or_else(|e| panic!("10 MB: {e:?}"));
    assert_eq!(up.file_size, Some(cl_api::ATTACHMENT_MAX_BYTES as u64));
    let mut over = exact.clone();
    over.push(b' ');
    let before = h.observer.total();
    let e = h
        .run(
            s.attachments()
                .upload_draft(draft_key, "over.pdf", &over, key()),
        )
        .expect_err("over");
    assert_eq!(e, ApiError::Local(LocalValidation::AttachmentTooLarge));
    assert_eq!(h.observer.total(), before, "nothing may be sent");

    let boundary = "cl-contract-10mb-edge";
    let mut body = Vec::with_capacity(over.len() + 512);
    for (field, value) in [
        ("attachableType", "draft".to_string()),
        ("attachableId", draft_key.to_string()),
    ] {
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{field}\"\r\n\r\n{value}\r\n"
            )
            .as_bytes(),
        );
    }
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"over.pdf\"\r\nContent-Type: application/pdf\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(&over);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let refused = h.run(s.raw_multipart_mapped("/api/v1/attachments", boundary, body));
    assert!(refused.is_err(), "the engine took 10 MB + 1: {refused:?}");
    let on_draft = h
        .run(s.attachments().list(
            AttachTo::Draft(draft_key),
            PageRequest::new(50, 0).expect("p"),
        ))
        .expect("list");
    assert_eq!(on_draft.total, 1, "only the 10 MB file is stored");
    h.run(s.attachments().delete(up.id, key())).expect("delete");
}

fn finish_job(h: &Harness, s: &Session, job: &cl_api::JobId) -> cl_api::ArchiveJobStatus {
    let deadline = Instant::now() + Duration::from_secs(120);
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

/// A backup (the engine's tenant archive with attachments) and its restore
/// keep a bill and its file byte for byte. Runs on tenant B: one more
/// company would exceed the run's create-tenant budget.
pub fn backup_restore_keeps_the_bill_file_live(h: &Harness) {
    if !h.require_cap(Capabilities::TENANT_ARCHIVE, "U-05") {
        return;
    }
    let s = h.s3();
    let co = h.tenant_b();
    let a = account(h, &s, AccountType::Expense, "Archive rental");
    let b = account(h, &s, AccountType::Expense, "Archive freight");
    let v = vendor(h, &s);
    let reference = unique("ARCH").replace(' ', "-");
    let draft_key = DraftKey::new();
    h.run(
        s.attachments()
            .upload_draft(draft_key, "bill.pdf", PDF, key()),
    )
    .expect("upload");
    let saved = h
        .run(s.bills().create(
            &mockup_bill(v, a.id, b.id, &reference),
            &PostingOptions::default(),
            key(),
        ))
        .expect("create");
    h.run(
        s.bills()
            .attach_draft(saved.record.txn.id, draft_key, key()),
    )
    .expect("attach");
    h.run(s.close_company()).expect("workspace");
    let passphrase = Secret::new(format!("contract-{}", unique("pass")));
    let job = h
        .run(s.tenants().export_archive(
            co,
            &ArchiveRequest {
                include_attachments: true,
                include_audit: true,
                omit_intake_settings: false,
                host: json!({"app": "ClientLedger", "appVersion": "test", "kind": "backup"}),
                passphrase: passphrase.clone(),
            },
        ))
        .expect("export job");
    let st = finish_job(h, &s, &job.job_id);
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
        }))
        .expect("import job");
    let st = finish_job(h, &s, &job.job_id);
    // U-91 (engine #176, pinned): the content check compares id-keyed jsonb
    // such as `companies.check_settings.nextCheckNumbers` semantically, so a
    // company with checks from several bank accounts (tenant B by now) must
    // restore every time.
    assert_eq!(st.status, JobState::Succeeded, "{:?}", st.error);
    let restored = st.import_result().expect("import result").tenant_id;
    h.run(s.open_company(restored)).expect("open restored");
    let rows = h
        .run(s.bills().list_capped(
            &BillListQuery {
                search: Some(reference.clone()),
                ..BillListQuery::default()
            },
            50,
            1,
        ))
        .expect("bills")
        .rows;
    assert_eq!(rows.len(), 1, "the bill is restored");
    let bill: TransactionId = rows[0].id;
    let restored_bill = h.run(s.bills().get(bill)).expect("get").record;
    assert_eq!(restored_bill.txn.total, Some(m("1480.0000")));
    assert_eq!(restored_bill.lines.len(), 2);
    let files = h
        .run(
            s.attachments()
                .list(AttachTo::Bill(bill), PageRequest::new(50, 0).expect("p")),
        )
        .expect("files");
    assert_eq!(files.total, 1, "the bill's file is restored");
    assert_eq!(
        h.run(s.attachments().download(files.data[0].id))
            .expect("bytes"),
        PDF
    );
    h.run(s.close_company()).expect("workspace");
}

/// U-83: a bill dated on or before the closing date is refused by the
/// engine; with `ap-closing-v1` and a closing-date password set on tenant B
/// the save without it asks for it, a wrong one is refused, the right one
/// saves and the override is audited. Without `ap-closing-v1` a password is
/// refused locally with nothing sent. Runs on tenant B; the test sets (and
/// afterwards clears) its own closing date and password.
pub fn closed_period_u83(h: &Harness) {
    let s = h.s3();
    let settings = |lock: serde_json::Value| {
        h.run(s.raw_mapped(
            Method::Put,
            "/api/v1/company/settings",
            Some(&json!({ "lockDate": lock })),
        ))
        .expect("closing date");
    };
    let (lock, set_here) = if let Some(d) = h.run(s.company_settings()).expect("settings").lock_date
    {
        (d, false)
    } else {
        settings(json!("2026-09-30"));
        (date(2026, 9, 30), true)
    };
    let a = account(h, &s, AccountType::Expense, "Closed rental");
    let b = account(h, &s, AccountType::Expense, "Closed freight");
    let v = vendor(h, &s);
    let mut closed = mockup_bill(v, a.id, b.id, &unique("INV").replace(' ', "-"));
    closed.date = lock;
    let e = h
        .run(s.bills().create(&closed, &PostingOptions::default(), key()))
        .expect_err("closed period");
    assert!(
        matches!(&e, ApiError::ClosedPeriod(c) if c.lock_date == Some(lock)),
        "{e:?}"
    );
    let with_password = PostingOptions {
        closed_period: Some(cl_api::ClosedPeriodAuthorization::with_password(
            Secret::new("not-sent"),
        )),
    };
    if h.require_cap(Capabilities::AP_CLOSING, "U-83")
        && h.require_cap(Capabilities::CLOSING_PASSWORD, "U-15")
    {
        let pw = Secret::new(unique("closing-bill"));
        assert!(
            h.run(s.set_closing_password(None, Some(&pw), key()))
                .expect("set password")
        );
        let e = h
            .run(s.bills().create(&closed, &PostingOptions::default(), key()))
            .expect_err("password required");
        assert!(
            matches!(&e, ApiError::ClosedPeriod(c) if c.password_required == Some(true)),
            "{e:?}"
        );
        let e = h
            .run(s.bills().create(&closed, &with_password, key()))
            .expect_err("wrong password");
        assert_eq!(e, ApiError::ClosingPasswordRejected);
        let right = PostingOptions {
            closed_period: Some(cl_api::ClosedPeriodAuthorization::with_password(pw.clone())),
        };
        let saved = h
            .run(s.bills().create(&closed, &right, key()))
            .expect("right password");
        assert_eq!(saved.record.txn.txn_date, lock);
        let rows = h
            .run(s.audit().for_record(
                cl_api::AuditEntity::ClosingDateOverride,
                *saved.record.txn.id.as_uuid(),
            ))
            .expect("audit");
        assert!(
            rows.iter()
                .any(|r| r.action == cl_api::AuditAction::Override),
            "the closed-period bill is audited"
        );
        assert!(
            !h.run(s.set_closing_password(Some(&pw), None, key()))
                .expect("clear password")
        );
    } else {
        let before = h.observer.total();
        let e = h
            .run(s.bills().create(&closed, &with_password, key()))
            .expect_err("gated");
        assert_eq!(e, ApiError::Local(LocalValidation::Unsupported));
        assert_eq!(h.observer.total(), before, "nothing may be sent");
    }
    if set_here {
        settings(serde_json::Value::Null);
    }
}

fn plain_bill(h: &Harness, s: &Session, v: ContactId) -> cl_api::Versioned<cl_api::BillRecord> {
    let a = account(h, s, AccountType::Expense, "PR-A rental");
    let b = account(h, s, AccountType::Expense, "PR-A freight");
    h.run(s.bills().create(
        &mockup_bill(v, a.id, b.id, &unique("INV").replace(' ', "-")),
        &PostingOptions::default(),
        key(),
    ))
    .expect("create")
}

fn draft_of(r: &cl_api::BillRecord) -> BillDraft {
    BillDraft {
        vendor: r.txn.contact_id.expect("vendor"),
        date: r.txn.txn_date,
        vendor_invoice_number: r.vendor_invoice_number.clone(),
        terms: r.payment_terms.as_deref().map(BillTerms::code),
        due_date: None,
        memo: r.txn.memo.clone(),
        amount_due: r.txn.total.expect("total"),
        lines: r
            .lines
            .iter()
            .map(|l| BillLineDraft {
                description: l.description.clone(),
                tag: l.tag,
                ..BillLineDraft::expense(l.account, l.amount)
            })
            .collect(),
        received: r.bill_received,
        draft_attachment: None,
    }
}

/// U-84c / review B3: an edit with the token read goes through, and the
/// answer is the final row (a second edit is not falsely stale); an edit
/// with an old token is refused 409 `STALE`.
pub fn stale_u84(h: &Harness) {
    if !h.require_cap(Capabilities::AP_EDIT, "U-84") {
        return;
    }
    let s = &h.s1;
    let v = vendor(h, s);
    let first = plain_bill(h, s, v);
    let mut d = draft_of(&first.record);
    d.vendor_invoice_number = Some(unique("EDIT").replace(' ', "-"));
    let second = h
        .run(s.bills().update(
            first.record.txn.id,
            &first,
            &d,
            &PostingOptions::default(),
            key(),
        ))
        .expect("edit");
    assert_eq!(second.record.vendor_invoice_number, d.vendor_invoice_number);
    d.memo = Some("second edit".into());
    let third = h
        .run(s.bills().update(
            first.record.txn.id,
            &second,
            &d,
            &PostingOptions::default(),
            key(),
        ))
        .expect("no false stale (B3)");
    // The fingerprint matches today, but the token is the first read's.
    let old_token = cl_api::Versioned {
        record: cl_api::BillRecord {
            txn: cl_api::Transaction {
                updated_at: first.record.txn.updated_at,
                ..third.record.txn.clone()
            },
            ..third.record.clone()
        },
        version: third.version,
    };
    let e = h
        .run(s.bills().update(
            first.record.txn.id,
            &old_token,
            &d,
            &PostingOptions::default(),
            key(),
        ))
        .expect_err("stale");
    assert_eq!(
        cl_api::ap_refusal(&e),
        Some(cl_api::ApRefusal::Stale),
        "{e:?}"
    );
}

/// U-84b: an unpaid bill deletes (`DELETE /transactions/{id}`) and leaves
/// the vendor's open balance.
pub fn delete_u84(h: &Harness) {
    if !h.require_cap(Capabilities::AP_EDIT, "U-84") {
        return;
    }
    let s = &h.s1;
    let v = vendor(h, s);
    let bill = plain_bill(h, s, v);
    h.run(s.bills().delete(
        bill.record.txn.id,
        Some("typed twice"),
        &ApWriteOptions {
            posting: PostingOptions::default(),
            expected_updated_at: bill.record.txn.updated_at,
        },
        key(),
    ))
    .expect("delete");
    let t = h
        .run(s.transactions().get(bill.record.txn.id))
        .expect("txn");
    assert_eq!(t.record.status, TxnStatus::Deleted);
    let sum = h.run(s.bills().vendor_summary(v)).expect("summary");
    assert_eq!((sum.unpaid_count, sum.total_owed), (0, Money::ZERO));
}

/// U-84a / S7: an unapplied credit is edited and its open balance follows.
pub fn vendor_credit_edit_u84(h: &Harness) {
    if !h.require_cap(Capabilities::AP_EDIT, "U-84") {
        return;
    }
    let s = &h.s1;
    let a = account(h, s, AccountType::Expense, "Credit edit");
    let v = vendor(h, s);
    let mut draft = VendorCreditDraft {
        vendor: v,
        date: date(2026, 11, 4),
        vendor_invoice_number: Some(unique("CM").replace(' ', "-")),
        memo: None,
        amount: m("150.00"),
        lines: vec![BillLineDraft::expense(a.id, m("150.00"))],
        draft_attachment: None,
    };
    let saved = h
        .run(
            s.vendor_credits()
                .create(&draft, &PostingOptions::default(), key()),
        )
        .expect("credit");
    draft.amount = m("100.00");
    draft.lines[0].amount = m("100.00");
    let edited = h
        .run(s.vendor_credits().update(
            saved.record.txn.id,
            &saved,
            &draft,
            &PostingOptions::default(),
            key(),
        ))
        .expect("edit");
    assert_eq!(edited.record.txn.total, Some(m("100.0000")));
    assert_eq!(edited.record.balance_due, Some(m("100.0000")), "S7");
    let avail = h.run(s.vendor_credits().available(v)).expect("available");
    assert_eq!(avail[0].balance_due, m("100.0000"));
}

/// U-85 and AC1 (item half): the mockup's two expense lines plus an item
/// line of 3 × 0.3333 (posts the rounded 1.00) make a 1,481.00 bill. A/P is
/// credited exactly the total, the vendor's open balance moves by exactly
/// the total, and the item line reads back its item, quantity and cost.
pub fn item_lines_u85(h: &Harness) {
    if !h.require_cap(Capabilities::BILL_LINES, "U-85") {
        return;
    }
    let s = &h.s1;
    let rental = account(h, s, AccountType::Expense, "Item rental");
    let delivery = account(h, s, AccountType::Expense, "Item delivery");
    let parts = account(h, s, AccountType::Expense, "Item parts");
    let income = account(h, s, AccountType::Revenue, "Item income");
    let item = h
        .run(s.items().create(
            &cl_api::ItemFields {
                name: unique("Hydraulic filter"),
                description: None,
                unit_price: None,
                income_account_id: income.id,
                is_taxable: false,
            },
            key(),
        ))
        .expect("item")
        .record;
    let ap = payables_account(h, s);
    let v = vendor(h, s);
    let before = h.run(s.bills().vendor_summary(v)).expect("summary");
    let mut d = mockup_bill(v, rental.id, delivery.id, &unique("ITEM").replace(' ', "-"));
    d.amount_due = m("1481.00");
    d.lines.push(BillLineDraft {
        description: Some("Hydraulic filter".into()),
        item: Some(item.id),
        quantity: Some(cl_core::Quantity::parse_wire("3").expect("q")),
        unit_price: Some(m("0.3333")),
        ..BillLineDraft::expense(parts.id, m("1.00"))
    });
    let saved = h
        .run(s.bills().create(&d, &PostingOptions::default(), key()))
        .expect("item bill");
    let id = saved.record.txn.id;
    assert_eq!(saved.record.txn.total, Some(m("1481.0000")));
    let line = saved
        .record
        .lines
        .iter()
        .find(|l| l.item == Some(item.id))
        .expect("the item line");
    assert_eq!(line.amount, m("1.0000"));
    assert_eq!(line.unit_price, Some(m("0.3333")));
    assert_eq!(
        line.quantity.map(cl_core::Quantity::to_wire).as_deref(),
        Some("3.0000")
    );
    assert_eq!(saved.record.fingerprint(), d.fingerprint());

    // A/P's credit for this bill is exactly the total.
    let txn = h.run(s.transactions().get(id)).expect("txn").record;
    let ap_lines: Vec<(String, String)> = txn
        .lines
        .iter()
        .filter(|l| l.account_id == ap)
        .map(|l| (l.debit.to_wire(), l.credit.to_wire()))
        .collect();
    assert_eq!(
        ap_lines,
        vec![("0.0000".to_string(), "1481.0000".to_string())],
        "A/P is credited exactly the bill total"
    );
    assert_eq!(
        balance(h, s, parts.id),
        m("1.0000"),
        "the item line posts 1.00"
    );

    // The vendor's open balance moves by exactly the total.
    let after = h.run(s.bills().vendor_summary(v)).expect("summary");
    assert_eq!(after.unpaid_count, before.unpaid_count + 1);
    assert_eq!(
        after
            .total_owed
            .checked_sub(before.total_owed)
            .expect("delta"),
        m("1481.0000")
    );

    // Clean up: the shared engine's later tests read A/P too.
    h.run(
        s.bills()
            .void(id, "contract clean-up", &ApWriteOptions::default(), key()),
    )
    .expect("void");
}

/// U-86: the engine's dates with calendar arithmetic across the March DST
/// change, and discount terms on a saved bill.
pub fn terms_dates_u86(h: &Harness) {
    if !h.require_cap(Capabilities::BILL_TERMS, "U-86") {
        return;
    }
    let s = &h.s1;
    let dates = h
        .run(s.bills().terms_dates(&cl_api::TermsDatesQuery {
            date: date(2026, 3, 1),
            terms: Some(BillTerms::code("net_15")),
            vendor: None,
        }))
        .expect("terms dates");
    assert_eq!(dates.due_date, Some(date(2026, 3, 16)));
    let codes: Vec<String> = h
        .run(s.bills().payment_terms())
        .expect("terms")
        .into_iter()
        .map(|t| t.code)
        .collect();
    assert!(codes.iter().any(|c| c == "2_10_net_30"), "{codes:?}");
    let a = account(h, s, AccountType::Expense, "Terms rental");
    let v = vendor(h, s);
    let d = BillDraft {
        terms: Some(BillTerms::code("2_10_net_30")),
        ..mockup_bill(v, a.id, a.id, &unique("TERMS").replace(' ', "-"))
    };
    let saved = h
        .run(s.bills().create(&d, &PostingOptions::default(), key()))
        .expect("bill");
    assert_eq!(saved.record.due_date, Some(date(2026, 12, 3)));
    assert_eq!(saved.record.discount_date, Some(date(2026, 11, 13)));
}

/// U-87: a bill not received yet stays in A/P and the vendor summary but is
/// left out of the payable list.
pub fn received_u87(h: &Harness) {
    if !h.require_cap(Capabilities::BILL_RECEIVED, "U-87") {
        return;
    }
    let s = &h.s1;
    let a = account(h, s, AccountType::Expense, "Received rental");
    let v = vendor(h, s);
    let d = BillDraft {
        received: false,
        ..mockup_bill(v, a.id, a.id, &unique("RCV").replace(' ', "-"))
    };
    let saved = h
        .run(s.bills().create(&d, &PostingOptions::default(), key()))
        .expect("bill");
    assert!(!saved.record.bill_received);
    assert_eq!(h.run(s.bills().payable(v)).expect("payable").len(), 0);
    let sum = h.run(s.bills().vendor_summary(v)).expect("summary");
    assert_eq!(sum.total_owed, m("1480.0000"));
}

/// A 1x1 PNG (synthetic): an image goes to the vision model as it is.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
    0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE,
    0x42, 0x60, 0x82,
];

/// What the canned model answers for the bill (synthetic): one amount
/// with a sign and commas, one in a European spelling (unreadable), a JSON
/// number total, an impossible due date, and per-field scores.
const CANNED_BILL: &str = r#"{"vendor":"Midwest Equipment Rental","vendor_invoice_number":"INV-1001","bill_date":"2026-11-03","due_date":"2026-02-30","payment_terms":"Net 15","subtotal":"1,330.00","tax":null,"total":1480,"line_items":[{"description":"Excavator, 1 day","amount":"$1,330.00","quantity":1,"confidence":0.95},{"description":"Delivery","amount":"12,50","quantity":null,"confidence":0.7}],"confidence":0.9,"field_confidence":{"vendor":0.95,"bill_date":0.7,"vendor_invoice_number":0.4}}"#;

/// U-89 (`bill-ocr-v2`): a bill read through a canned Ollama reply (a
/// loopback stub; nothing leaves the computer). Amounts are exact decimal
/// strings or absent, an impossible date is absent, and each field and
/// line carries `high`, `medium` or `low`. AI is switched off again after.
fn ocr_confidence_u89(h: &Harness, s: &Session) {
    use cl_api::{AiConfigUpdate, AiTaskToggles, Confidence, OllamaTarget, SessionSecrets};
    let stub = crate::support::OllamaStub::shared(h);
    stub.serve_chat(h, CANNED_BILL);
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
                .upload_draft(DraftKey::new(), "invoice.png", PNG, key()),
        )
        .expect("upload");
    let hits = stub.hits(h);
    let open = cl_api::BillReadGate {
        ai_enabled: true,
        bill_reading_on: true,
        provider_set: true,
    };
    let read = h.run(s.bill_reading().extract(open, up.id, key()));
    // Switch AI off before asserting, so a failure leaves the product default.
    h.run(s.update_ai_config(
        &AiConfigUpdate {
            is_enabled: Some(false),
            ..AiConfigUpdate::default()
        },
        key(),
    ))
    .expect("AI off");
    let read = read.expect("read");
    assert!(stub.hits(h) > hits, "the reading asked the loopback model");
    assert_eq!(
        read.vendor_name.as_deref(),
        Some("Midwest Equipment Rental")
    );
    assert_eq!(read.bill_date, Some(date(2026, 11, 3)));
    assert_eq!(read.due_date, None, "an impossible date is not read");
    assert_eq!(read.total, Some(m("1480")));
    assert_eq!(read.subtotal, Some(m("1330.00")));
    assert_eq!(read.lines.len(), 2);
    assert_eq!(read.lines[0].amount, Some(m("1330.00")));
    assert_eq!(read.lines[0].confidence, Some(Confidence::High));
    assert_eq!(read.lines[1].amount, None, "12,50 is not guessed");
    assert_eq!(read.lines[1].confidence, Some(Confidence::Low));
    let field = |k: &str| read.field_confidence.get(k).copied();
    assert_eq!(field("vendor"), Some(Confidence::High));
    assert_eq!(field("billDate"), Some(Confidence::Medium));
    assert_eq!(field("vendorInvoiceNumber"), Some(Confidence::Low));
    assert_eq!(field("dueDate"), Some(Confidence::Low));
    assert_eq!(field("tax"), Some(Confidence::Low));
    assert_eq!(field("total"), Some(Confidence::High), "the document score");
    h.run(s.attachments().delete(up.id, key()))
        .expect("clean up");
}

/// Reading a bill while AI is off: the owner-side gate sends nothing; with
/// the gate claimed open, the engine's AI gate refuses before any provider
/// (no network) and that is `AiUnavailable`. With `bill-ocr-v2` a canned
/// provider then reads one (U-89).
pub fn reading_refused_while_ai_is_off_live(h: &Harness) {
    let s = &h.s1;
    let up = h
        .run(
            s.attachments()
                .upload_draft(DraftKey::new(), "bill.pdf", PDF, key()),
        )
        .expect("upload");
    let before = h.observer.total();
    let e = h
        .run(
            s.bill_reading()
                .extract(cl_api::BillReadGate::default(), up.id, key()),
        )
        .expect_err("gate");
    assert_eq!(e, ApiError::AiUnavailable);
    assert_eq!(h.observer.total(), before, "nothing may be sent");
    let open = cl_api::BillReadGate {
        ai_enabled: true,
        bill_reading_on: true,
        provider_set: true,
    };
    let e = h
        .run(s.bill_reading().extract(open, up.id, key()))
        .expect_err("engine gate");
    assert_eq!(e, ApiError::AiUnavailable, "{e:?}");
    h.run(s.attachments().delete(up.id, key()))
        .expect("clean up");
    if h.require_cap(Capabilities::BILL_OCR_V2, "U-89")
        && h.require_cap(Capabilities::AI_DESKTOP, "U-09")
    {
        ocr_confidence_u89(h, s);
        ocr_text_pdf_reads(h, s);
    }
}

/// A synthetic two-page PDF with a text layer (`fixtures/docs/two_pages.pdf`).
const TEXT_PDF: &[u8] = include_bytes!("../../../../fixtures/docs/two_pages.pdf");

/// A PDF is read, not `PDF_RENDER_FAILED`: the engine executable carries
/// pdf.js and its worker, so the text layer is read inside the engine and
/// only that text goes to the (canned, loopback) model. No page pictures
/// are made (that would need poppler, which is not shipped).
fn ocr_text_pdf_reads(h: &Harness, s: &Session) {
    use cl_api::{AiConfigUpdate, AiTaskToggles, OllamaTarget, SessionSecrets};
    let stub = crate::support::OllamaStub::shared(h);
    stub.serve_chat(h, CANNED_BILL);
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
                .upload_draft(DraftKey::new(), "invoice.pdf", TEXT_PDF, key()),
        )
        .expect("upload");
    let asked = stub.chat_bodies(h).len();
    let open = cl_api::BillReadGate {
        ai_enabled: true,
        bill_reading_on: true,
        provider_set: true,
    };
    let read = h.run(s.bill_reading().extract(open, up.id, key()));
    h.run(s.update_ai_config(
        &AiConfigUpdate {
            is_enabled: Some(false),
            ..AiConfigUpdate::default()
        },
        key(),
    ))
    .expect("AI off");
    let read = read.expect("a PDF is read (not PDF_RENDER_FAILED)");
    let bodies = stub.chat_bodies(h);
    assert_eq!(bodies.len(), asked + 1, "one model call");
    let sent = bodies.last().map(String::as_str).unwrap_or_default();
    assert!(
        sent.contains("Synthetic test document") && sent.contains("Second page"),
        "the PDF's text layer (both pages) went to the model"
    );
    assert!(!sent.contains("\"images\""), "no page pictures were sent");
    assert_eq!(
        read.vendor_name.as_deref(),
        Some("Midwest Equipment Rental")
    );
    assert_eq!(read.total, Some(m("1480")));
    h.run(s.attachments().delete(up.id, key()))
        .expect("clean up");
}
