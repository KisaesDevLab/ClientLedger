//! Spreadsheet transaction import against the live engine (P31 plan §5,
//! "CSV import (step 6)"): a file with a bad row posts nothing; a good file
//! commits as one batch and Undo removes it; after Undo the same file
//! imports again (the re-import refusal is freed).
//!
//! Every test makes its own company so the re-import check never trips on
//! a rerun.
//!
//! PENDING until the pinned engine advertises `imports-batch-v1` (U-261)
//! and `imports-transactions-v1` (U-268).

use std::time::Duration;

use cl_api::import_transactions::{
    BatchRunRequest, BatchState, BatchStatus, ImportBatchId, TxnImportRefusal, txn_import_refusal,
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
        if !b.status.running() {
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

/// An unbalanced entry: the upload names its row, or the commit fails
/// with nothing posted; the batch is `failed`, never `committed`.
pub fn bad_row_posts_nothing(h: &Harness) {
    if !caps(h) {
        return;
    }
    let s = fresh_company(h, "Txn Import Bad");
    let n = names(h, &s);
    let it = s.import_transactions();
    let up = h
        .run(it.upload(&file(&n, "499.99")))
        .unwrap_or_else(|e| panic!("upload: {e}"));
    if up.problems.is_empty() {
        let id = h
            .run(it.create_batch(up.session))
            .unwrap_or_else(|e| panic!("batch: {e}"));
        h.run(it.commit(id, &run_req(), key()))
            .unwrap_or_else(|e| panic!("commit: {e}"));
        let b = settle(h, &s, id);
        assert_eq!(b.status, BatchStatus::Failed);
        assert_eq!(
            b.error.map(|e| e.code).as_deref(),
            Some("IMPORT_UNBALANCED")
        );
    } else {
        assert!(up.problems.iter().any(|p| p.code == "IMPORT_UNBALANCED"));
    }
    // The good file still imports: nothing of the bad one is in the way.
    let (_, b) = import(h, &s, &file(&n, "500.00"));
    assert_eq!(b.status, BatchStatus::Committed);
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
        txn_import_refusal(&e),
        Some(TxnImportRefusal::AlreadyCommitted { batch: Some(b), .. }) if b == id
    ));
    h.run(s.import_transactions().undo(id, &run_req(), key()))
        .unwrap_or_else(|e| panic!("undo: {e}"));
    assert_eq!(settle(h, &s, id).status, BatchStatus::Undone);
    let (_, again) = import(h, &s, &csv);
    assert_eq!(again.status, BatchStatus::Committed);
}
