//! Spreadsheet transaction import against the live engine (P31 plan §5,
//! "CSV import (step 6)"): a file with a bad row posts nothing; a good file
//! commits as one batch and Undo removes it; after Undo the same file
//! imports again (the re-import refusal is freed).
//!
//! Every test makes its own company so the re-import check never trips on
//! a rerun.
//!
//! Runs on engines with `imports-batch-v1` (U-261) and
//! `imports-transactions-v1` (U-268; pin 1f2c9bb3 on).

use std::time::Duration;

use cl_api::import_transactions::{
    BatchRunRequest, BatchState, BatchStatus, ImportBatchId, ImportRefusal, import_refusal,
};
use cl_api::{
    AccountType, Capabilities, ContactFields, ContactType, NewCompany, NewContact, Session,
};
use cl_core::Money;

use crate::common::{account, key};
use crate::support::{Harness, unique};

fn caps(h: &Harness) -> bool {
    h.require_cap(Capabilities::IMPORTS_BATCH, "U-261")
        && h.require_cap(Capabilities::IMPORTS_TRANSACTIONS, "U-268")
}

fn fresh_company(h: &Harness, name: &str) -> Session {
    h.auth_slots(1);
    let tenant = h
        .run(h.s1.create_company(&NewCompany {
            company_name: unique(name),
            business_type: Some("default".into()),
            entity_type: None,
            system_accounts_only: None,
        }))
        .unwrap_or_else(|e| panic!("create {name}: {e}"))
        .tenant_id;
    let s = h.s3();
    h.run(s.open_company(tenant))
        .unwrap_or_else(|e| panic!("open {name}: {e}"));
    s
}

fn contact(h: &Harness, s: &Session, t: ContactType, name: &str) -> String {
    let name = unique(name);
    h.run(s.contacts().create(
        &NewContact {
            fields: ContactFields::named(t, name.clone()),
            opening_balance: Money::ZERO,
            opening_balance_date: None,
        },
        key(),
    ))
    .unwrap_or_else(|e| panic!("create contact: {e}"));
    name
}

/// The names a file refers to.
struct Names {
    customer: String,
    vendor: String,
    sales: String,
    supplies: String,
    rent: String,
    cash: String,
}

fn names(h: &Harness, s: &Session) -> Names {
    Names {
        customer: contact(h, s, ContactType::Customer, "Pine Co"),
        vendor: contact(h, s, ContactType::Vendor, "Oak Supply"),
        sales: account(h, s, AccountType::Revenue, "Sales").fields.name,
        supplies: account(h, s, AccountType::Expense, "Supplies").fields.name,
        rent: account(h, s, AccountType::Expense, "Rent").fields.name,
        cash: account(h, s, AccountType::Asset, "Cash").fields.name,
    }
}

/// The canonical file: one invoice of two lines, one bill, one journal
/// entry. `credit` is the entry's credit (500.00 balances).
fn file(n: &Names, credit: &str) -> Vec<u8> {
    let h = "group_id,type,date,number,name,due_date,memo,line_account,line_item,line_description,line_amount,line_debit,line_credit,line_tag";
    format!(
        "{h}\r\n\
         A,invoice,2026-03-02,1001,{c},2026-04-01,,{sales},,Design,100.00,,,\r\n\
         A,invoice,2026-03-02,1001,{c},2026-04-01,,{sales},,Hosting,25.50,,,\r\n\
         B,bill,2026-03-05,B-7,{v},,,{sup},,,40.00,,,\r\n\
         C,journal_entry,2026-03-31,,,,,{rent},,,,500.00,,\r\n\
         C,journal_entry,2026-03-31,,,,,{cash},,,,,{credit},\r\n",
        c = n.customer,
        v = n.vendor,
        sales = n.sales,
        sup = n.supplies,
        rent = n.rent,
        cash = n.cash,
    )
    .into_bytes()
}

fn settle(h: &Harness, s: &Session, id: ImportBatchId) -> BatchState {
    for _ in 0..240 {
        let b = h
            .run(s.import_transactions().batch(id))
            .unwrap_or_else(|e| panic!("batch: {e}"));
        if !b.status.is_running() {
            return b;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    panic!("the batch never settled");
}

fn run_req() -> BatchRunRequest {
    BatchRunRequest {
        backup_ref: "contract-before-import.clbackup".into(),
        closed_period: None,
    }
}

fn import(h: &Harness, s: &Session, csv: &[u8]) -> (ImportBatchId, BatchState) {
    let it = s.import_transactions();
    let up = h
        .run(it.upload(csv))
        .unwrap_or_else(|e| panic!("upload: {e}"));
    let id = h
        .run(it.create_batch(up.session))
        .unwrap_or_else(|e| panic!("batch: {e}"));
    h.run(it.commit(id, &run_req(), key()))
        .unwrap_or_else(|e| panic!("commit: {e}"));
    (id, settle(h, s, id))
}

/// U-268 as built: an unbalanced entry is an upload row problem
/// (`IMPORT_UNBALANCED` on the document's first row, no column); the
/// commit is refused 409 `IMPORT_HAS_ERRORS` before anything starts; the
/// same bytes again while that upload is open are 409
/// `IMPORT_SESSION_ACTIVE` until it is discarded.
pub fn bad_row_posts_nothing(h: &Harness) {
    if !caps(h) {
        return;
    }
    let s = fresh_company(h, "Txn Import Bad");
    let n = names(h, &s);
    let it = s.import_transactions();
    let bad = file(&n, "499.99");
    let up = h
        .run(it.upload(&bad))
        .unwrap_or_else(|e| panic!("upload: {e}"));
    let p = up
        .problems
        .iter()
        .find(|p| p.code == "IMPORT_UNBALANCED")
        .unwrap_or_else(|| panic!("{:?}", up.problems));
    assert_eq!(p.row_number, 5, "the entry's first row (header = 1)");
    assert_eq!(up.total_rows, 5, "data rows");
    // The engine counts the documents it could read: the unbalanced entry
    // is not one of them.
    assert_eq!(up.documents, Some(2));
    let id = h
        .run(it.create_batch(up.session))
        .unwrap_or_else(|e| panic!("batch: {e}"));
    let e = h
        .run(it.commit(id, &run_req(), key()))
        .expect_err("refused before the job");
    match import_refusal(&e) {
        Some(ImportRefusal::HasErrors { errors, .. }) => {
            assert!(
                errors.iter().any(|x| x.code == "IMPORT_UNBALANCED"),
                "{errors:?}"
            );
        }
        other => panic!("{other:?} {e}"),
    }
    let e = h.run(it.upload(&bad)).expect_err("already open");
    let open = match import_refusal(&e) {
        Some(ImportRefusal::SessionActive {
            session: Some(open),
            ..
        }) => open,
        other => panic!("{other:?} {e}"),
    };
    assert_eq!(open, up.session);
    h.run(it.discard(open))
        .unwrap_or_else(|e| panic!("discard: {e}"));
    assert!(h.run(it.upload(&bad)).is_ok(), "free once discarded");
    // The good file still imports: nothing of the bad one is in the way.
    let (_, b) = import(h, &s, &file(&n, "500.00"));
    assert_eq!(b.status, BatchStatus::Committed);
}

/// Nothing is created on import: a customer, item or tag the books lack
/// is `IMPORT_UNKNOWN_REF` on its row and column at upload; one that goes
/// away after the upload fails the batch with `field` and `details.refs`.
pub fn unknown_refs(h: &Harness) {
    if !caps(h) {
        return;
    }
    let s = fresh_company(h, "Txn Import Refs");
    let n = names(h, &s);
    let it = s.import_transactions();
    let hd = "group_id,type,date,number,name,due_date,memo,line_account,line_item,line_description,line_amount,line_debit,line_credit,line_tag";
    let csv = format!(
        "{hd}\r\n\
         A,invoice,2026-03-02,2001,{nobody},,,{sales},,,10.00,,,\r\n\
         B,invoice,2026-03-03,2002,{c},,,{sales},{noitem},,10.00,,,\r\n\
         C,bill,2026-03-04,X-1,{v},,,{sup},,,5.00,,,{notag}\r\n",
        nobody = unique("Nobody"),
        noitem = unique("No Item"),
        notag = unique("No Tag"),
        c = n.customer,
        v = n.vendor,
        sales = n.sales,
        sup = n.supplies,
    );
    let up = h
        .run(it.upload(csv.as_bytes()))
        .unwrap_or_else(|e| panic!("upload: {e}"));
    let refs: Vec<(u64, Option<&str>)> = up
        .problems
        .iter()
        .filter(|p| p.code == "IMPORT_UNKNOWN_REF")
        .map(|p| (p.row_number, p.field.as_deref()))
        .collect();
    assert_eq!(
        refs,
        vec![
            (2, Some("name")),
            (3, Some("line_item")),
            (4, Some("line_tag"))
        ],
        "{:?}",
        up.problems
    );
    // Rechecked at commit, so the precheck lets it through and the job
    // fails with every reference listed; nothing is posted.
    let id = h
        .run(it.create_batch(up.session))
        .unwrap_or_else(|e| panic!("batch: {e}"));
    h.run(it.commit(id, &run_req(), key()))
        .unwrap_or_else(|e| panic!("commit: {e}"));
    let b = settle(h, &s, id);
    assert_eq!(b.status, BatchStatus::Failed);
    let err = b.error.expect("error");
    assert_eq!(err.code, "IMPORT_UNKNOWN_REF");
    assert_eq!(err.field.as_deref(), Some("name"));
    match err.refusal() {
        Some(ImportRefusal::UnknownRef {
            refs, ref_count, ..
        }) => {
            assert_eq!(ref_count, 3);
            assert_eq!(refs.len(), 3);
            assert!(refs.iter().all(|r| r.reason.is_some()), "{refs:?}");
        }
        other => panic!("{other:?}"),
    }
    // The shared batch reader decodes a failed transactions batch.
    let st = h
        .run(s.imports().batch(id))
        .unwrap_or_else(|e| panic!("imports batch: {e}"));
    assert_eq!(st.batch.status, BatchStatus::Failed);
}

/// A document dated on or before the closing date: the commit answers 423
/// (before the job), and the closing-date password lets it through.
pub fn closed_period(h: &Harness) {
    if !caps(h) || !h.require_cap(Capabilities::CLOSING_PASSWORD, "U-15") {
        return;
    }
    let s = fresh_company(h, "Txn Import Closed");
    let n = names(h, &s);
    let pw = cl_core::Secret::new(unique("closing-txn"));
    h.run(s.raw_mapped(
        cl_api::Method::Put,
        "/api/v1/company/settings",
        Some(&serde_json::json!({ "lockDate": "2026-03-31" })),
    ))
    .expect("closing date");
    assert!(
        h.run(s.set_closing_password(None, Some(&pw), key()))
            .unwrap_or_else(|e| panic!("closing password: {e}"))
    );
    let it = s.import_transactions();
    let up = h
        .run(it.upload(&file(&n, "500.00")))
        .unwrap_or_else(|e| panic!("upload: {e}"));
    assert!(up.problems.is_empty(), "{:?}", up.problems);
    let id = h
        .run(it.create_batch(up.session))
        .unwrap_or_else(|e| panic!("batch: {e}"));
    let e = h.run(it.commit(id, &run_req(), key())).expect_err("closed");
    assert!(matches!(&e, cl_api::ApiError::ClosedPeriod(_)), "{e:?}");
    let with = BatchRunRequest {
        closed_period: Some(cl_api::ClosedPeriodAuthorization::with_password(pw)),
        ..run_req()
    };
    h.run(it.commit(id, &with, key()))
        .unwrap_or_else(|e| panic!("commit with password: {e}"));
    assert_eq!(settle(h, &s, id).status, BatchStatus::Committed);
}
/// Commit, then Undo: the batch ends `undone`; a second Undo is refused.
pub fn undo(h: &Harness) {
    if !caps(h) {
        return;
    }
    let s = fresh_company(h, "Txn Import Undo");
    let n = names(h, &s);
    let (id, b) = import(h, &s, &file(&n, "500.00"));
    assert_eq!(b.status, BatchStatus::Committed);
    // History and the backup protection read every batch with the strict
    // shared reader: a committed transactions batch must decode.
    let st = h
        .run(s.imports().batch(id))
        .unwrap_or_else(|e| panic!("imports batch: {e}"));
    assert_eq!(st.batch.status, BatchStatus::Committed);
    let all = h
        .run(s.imports().batches_all())
        .unwrap_or_else(|e| panic!("history: {e}"));
    assert!(all.iter().any(|b| b.id == id));
    h.run(s.import_transactions().undo(id, &run_req(), key()))
        .unwrap_or_else(|e| panic!("undo: {e}"));
    assert_eq!(settle(h, &s, id).status, BatchStatus::Undone);
}

/// The same file is refused while imported and imports again after Undo.
pub fn recommit(h: &Harness) {
    if !caps(h) {
        return;
    }
    let s = fresh_company(h, "Txn Import Again");
    let n = names(h, &s);
    let csv = file(&n, "500.00");
    let (id, b) = import(h, &s, &csv);
    assert_eq!(b.status, BatchStatus::Committed);
    let e = h
        .run(s.import_transactions().upload(&csv))
        .expect_err("refused while imported");
    assert!(matches!(
        import_refusal(&e),
        Some(ImportRefusal::AlreadyCommitted { batch: Some(b), .. }) if b == id
    ));
    h.run(s.import_transactions().undo(id, &run_req(), key()))
        .unwrap_or_else(|e| panic!("undo: {e}"));
    assert_eq!(settle(h, &s, id).status, BatchStatus::Undone);
    let (_, again) = import(h, &s, &csv);
    assert_eq!(again.status, BatchStatus::Committed);
}

/// P31 QA: a spreadsheet bill with a negative line (212.40 and (12.40))
/// and an invoice with a discount line read back with every line, and the
/// lines add up to the engine's total. The engine stores the bill's
/// negative line as a negative debit; the bill reader once kept only
/// positive debits and dropped it.
pub fn negative_lines_read_back(h: &Harness) {
    if !caps(h) {
        return;
    }
    let s = fresh_company(h, "Txn Import Negative");
    let n = names(h, &s);
    let head = "group_id,type,date,number,name,due_date,memo,line_account,line_item,line_description,line_amount,line_debit,line_credit,line_tag";
    let csv = format!(
        "{head}\r\n\
         A,invoice,2026-03-02,3001,{c},2026-04-01,,{sales},,Mowing,480.00,,,\r\n\
         A,invoice,2026-03-02,3001,{c},2026-04-01,,{sales},,Discount,(30.00),,,\r\n\
         B,bill,2026-03-05,HF-1,{v},,,{sup},,Fuel,212.40,,,\r\n\
         B,bill,2026-03-05,HF-1,{v},,,{sup},,Refund,(12.40),,,\r\n",
        c = n.customer,
        v = n.vendor,
        sales = n.sales,
        sup = n.supplies,
    );
    let (_, b) = import(h, &s, csv.as_bytes());
    assert_eq!(b.status, BatchStatus::Committed, "{:?}", b.error);
    let m = |v: &str| Money::parse_wire(v).expect("money");
    let page = cl_api::PageRequest::new(50, 0).expect("page");

    let bills = h
        .run(s.bills().list(&cl_api::BillListQuery::default(), page))
        .unwrap_or_else(|e| panic!("bills: {e}"));
    assert_eq!(bills.data.len(), 1);
    let bill = h
        .run(s.bills().get(bills.data[0].id))
        .unwrap_or_else(|e| panic!("bill: {e}"))
        .record;
    let amounts: Vec<Money> = bill.lines.iter().map(|l| l.amount).collect();
    assert_eq!(amounts, vec![m("212.40"), m("-12.40")]);
    assert_eq!(bill.txn.total, Some(m("200.00")));

    let invoices = h
        .run(
            s.invoices()
                .list(&cl_api::InvoiceListQuery::default(), page),
        )
        .unwrap_or_else(|e| panic!("invoices: {e}"));
    assert_eq!(invoices.data.len(), 1);
    let inv = h
        .run(s.invoices().get(invoices.data[0].id))
        .unwrap_or_else(|e| panic!("invoice: {e}"))
        .record;
    let amounts: Vec<Money> = inv.lines.iter().map(|l| l.amount).collect();
    assert_eq!(amounts, vec![m("480.00"), m("-30.00")]);
    assert_eq!(inv.txn.total, Some(m("450.00")));
}

/// Review B1: the desktop sends the owner's own amount text (the engine's
/// parser decides): `$1,234.50`, `(25.50)` on an invoice line, NBSP
/// thousands, U+2212 and a trailing `CR` in a signed journal `line_amount`,
/// and a debit next to a `-` (the accounting zero). All read with no row
/// problem, and the entry balances; `12,34` is a row problem, never 1234.
pub fn owner_amount_text_reads_exactly(h: &Harness) {
    if !caps(h) {
        return;
    }
    let s = fresh_company(h, "Txn Import Text");
    let n = names(h, &s);
    let it = s.import_transactions();
    let head = "group_id,type,date,number,name,due_date,memo,line_account,line_item,line_description,line_amount,line_debit,line_credit,line_tag";
    let good = format!(
        "{head}\r\n\
         A,invoice,2026-03-02,1001,{c},2026-04-01,,{sales},,Design,\"$1,234.50\",,,\r\n\
         A,invoice,2026-03-02,1001,{c},2026-04-01,,{sales},,Credit,(25.50),,,\r\n\
         C,journal_entry,2026-03-31,,,,,{rent},,,1\u{a0}000.00,,,\r\n\
         C,journal_entry,2026-03-31,,,,,{cash},,,\u{2212}600.00,,,\r\n\
         C,journal_entry,2026-03-31,,,,,{cash},,,400.00CR,,,\r\n\
         D,journal_entry,2026-03-31,,,,,{rent},,,,5.00,-,\r\n\
         D,journal_entry,2026-03-31,,,,,{cash},,,,,5.00,\r\n",
        c = n.customer,
        sales = n.sales,
        rent = n.rent,
        cash = n.cash,
    );
    let up = h
        .run(it.upload(good.as_bytes()))
        .unwrap_or_else(|e| panic!("upload: {e}"));
    assert!(up.problems.is_empty(), "{:?}", up.problems);
    assert_eq!(up.documents, Some(3));
    let _ = h.run(it.discard(up.session));
    let bad = format!(
        "{head}\r\nA,invoice,2026-03-02,1002,{c},2026-04-01,,{sales},,Design,\"12,34\",,,\r\n",
        c = n.customer,
        sales = n.sales,
    );
    let up = h
        .run(it.upload(bad.as_bytes()))
        .unwrap_or_else(|e| panic!("upload: {e}"));
    assert!(
        up.problems
            .iter()
            .any(|p| p.code == "IMPORT_AMOUNT_INVALID"),
        "{:?}",
        up.problems
    );
    let _ = h.run(it.discard(up.session));
}
