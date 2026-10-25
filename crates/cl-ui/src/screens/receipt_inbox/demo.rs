//! The receipt inbox in `--demo`: its own folder next to the demo's
//! settings, filled with synthetic files (the repository's fixtures) each
//! time the inbox window opens, and the demo ledger's records for the
//! "Add to …" pickers. Nothing here is real data.

use std::path::{Path, PathBuf};

use cl_api::TxnType;
use cl_core::Money;

use super::picker::{PickKind, PickRow};
use crate::company::CompanyLink;
use crate::company::demo::DemoState;
use crate::company::demo::register::DemoLedger;

/// A synthetic two-page PDF (the viewer's fixture).
const BILL_PDF: &[u8] = include_bytes!("../../../../../fixtures/docs/two_pages.pdf");
/// A synthetic receipt photo.
const RECEIPT_PNG: &[u8] = include_bytes!("../../../../../fixtures/docs/receipt.png");

/// The demo files: name and bytes. The large one is made by size only.
pub(crate) const SEED: [(&str, &[u8]); 2] = [
    ("Midwest Equipment INV-55871.pdf", BILL_PDF),
    ("Ozark Feed receipt.png", RECEIPT_PNG),
];

/// A scan over the 10 MB limit (listed, never read).
pub(crate) const LARGE: &str = "Scanned statement.pdf";

/// A file the inbox does not take (left out of the list).
pub(crate) const OTHER: &str = "notes.txt";

/// The demo's inbox folder, beside its settings folder.
#[must_use]
pub(crate) fn folder(link: &CompanyLink) -> Option<PathBuf> {
    let settings = link.store().path().parent()?;
    Some(settings.parent()?.join("Receipt inbox"))
}

/// Puts back any demo file that is not in `dir` (a filed one comes back,
/// so every run of the scenario starts alike).
pub(crate) fn seed(dir: &Path) {
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    for (name, bytes) in SEED {
        let p = dir.join(name);
        if !p.exists() {
            let _ = std::fs::write(&p, bytes);
        }
    }
    let large = dir.join(LARGE);
    if !large.exists()
        && let Ok(f) = std::fs::File::create(&large)
    {
        let _ = f.set_len(cl_company::inbox::MAX_FILE_BYTES + 1);
    }
    let other = dir.join(OTHER);
    if !other.exists() {
        let _ = std::fs::write(&other, b"synthetic notes");
    }
}

fn txn_type(kind: &str) -> Option<TxnType> {
    Some(match kind {
        "check" | "expense" | "card_charge" => TxnType::Expense,
        "bill" => TxnType::Bill,
        "bill_payment" => TxnType::BillPayment,
        "deposit" => TxnType::Deposit,
        "invoice" => TxnType::Invoice,
        "payment" => TxnType::CustomerPayment,
        "transfer" => TxnType::Transfer,
        "journal_entry" => TxnType::JournalEntry,
        _ => return None,
    })
}

/// The demo ledger's records for a picker, newest first, at most 50.
#[must_use]
pub(crate) fn rows(
    state: &mut DemoState,
    kind: PickKind,
    account: Option<cl_core::ids::AccountId>,
    search: &str,
) -> Vec<PickRow> {
    let chart = state
        .chart
        .get_or_insert_with(crate::company::demo::chart)
        .clone();
    let number = account.and_then(|a| {
        chart
            .iter()
            .find(|x| x.id == a)
            .and_then(|x| x.fields.account_number.clone())
    });
    let needle = search.trim().to_lowercase();
    let mut txns: Vec<_> = state
        .ledger
        .transactions()
        .into_iter()
        .filter(|t| !state.ledger.is_void(DemoLedger::id_of(t)))
        .filter(|t| match kind {
            PickKind::Bill => t.kind == "bill",
            PickKind::Check => t.kind == "check",
            PickKind::BankTxn => number
                .as_ref()
                .is_some_and(|n| t.lines.iter().any(|l| l.account == *n)),
        })
        .filter(|t| {
            needle.is_empty()
                || t.number.to_lowercase().contains(&needle)
                || t.memo.to_lowercase().contains(&needle)
                || t.contact
                    .as_ref()
                    .is_some_and(|c| c.to_lowercase().contains(&needle))
        })
        .collect();
    txns.sort_by(|a, b| b.date.cmp(&a.date).then(b.id.cmp(&a.id)));
    txns.into_iter()
        .filter_map(|t| {
            Some(PickRow {
                id: DemoLedger::id_of(&t),
                txn_type: txn_type(&t.kind)?,
                date: t.date,
                number: Some(t.number.clone()).filter(|n| !n.is_empty()),
                payee: t.contact.clone(),
                amount: Money::sum(t.lines.iter().map(|l| l.debit)).ok(),
            })
        })
        .take(super::picker::MAX_ROWS)
        .collect()
}
