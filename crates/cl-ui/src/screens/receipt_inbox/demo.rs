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

// ---------------------------------------------------------------------------
// Intake folders in `--demo` (A01 C5).

/// A synthetic one-page invoice.
const INVOICE_PDF: &[u8] =
    include_bytes!("../../../../../assets/sample/docs/midwest_inv_55871.pdf");
/// A synthetic TIFF (the little-endian signature and an empty directory).
const FAX_TIFF: &[u8] = b"II*\0\x08\0\0\0\0\0\0\0synthetic fax";
/// A synthetic iPhone photo header (never read: HEIC isn't taken).
const PHOTO_HEIC: &[u8] = b"\0\0\0\x18ftypheic\0\0\0\0mif1heicsynthetic";

/// The demo's intake files: folder (0 Bills, 1 Receipts), name, bytes,
/// hours old.
pub(crate) const INTAKE_SEED: [(usize, &str, &[u8], u64); 5] = [
    (0, "Midwest Equipment INV-55871.pdf", INVOICE_PDF, 5),
    (0, "Galena Gravel fax.tif", FAX_TIFF, 4),
    (0, "Hollister Fuel photo.heic", PHOTO_HEIC, 4),
    (1, "Ozark Feed receipt.png", RECEIPT_PNG, 2),
    // The same bytes again, an hour later: listed once as a duplicate.
    (1, "Ozark Feed receipt (copy).png", RECEIPT_PNG, 1),
];

/// An 11 MB scan in Bills (listed, never read).
pub(crate) const INTAKE_LARGE: &str = "Scanned statement.pdf";
/// The late file in Receipts, written in two halves 3 s apart, so the inbox
/// shows it settling before it is ready.
pub(crate) const INTAKE_LATE: &str = "Crane Creek delivery.pdf";
/// When the late file's halves are written, after the folders are laid out.
const LATE_FIRST: std::time::Duration = std::time::Duration::from_secs(3);
const LATE_SECOND: std::time::Duration = std::time::Duration::from_secs(3);

/// Where the demo lays out its intake folders: beside its settings.
#[must_use]
pub(crate) fn intake_base(link: &CompanyLink) -> Option<PathBuf> {
    let settings = link.store().path().parent()?;
    Some(settings.parent()?.join("Intake demo"))
}

/// The demo's intake folders, laid out.
pub(crate) struct Seeded {
    /// Confirmed on this computer; leaves originals where they are.
    pub bills: PathBuf,
    /// P17's folder, migrated to the intake folder "Receipts".
    pub receipts: PathBuf,
    /// A folder not confirmed on this computer ("Set up…").
    pub scanner: PathBuf,
    /// The pin of `bills`.
    pub bills_key: Option<String>,
}

fn write_aged(path: &Path, bytes: &[u8], hours: u64) {
    if std::fs::write(path, bytes).is_err() {
        return;
    }
    let at = std::time::SystemTime::now() - std::time::Duration::from_secs(hours * 3600);
    if let Ok(f) = std::fs::File::options().write(true).open(path) {
        let _ = f.set_modified(at);
    }
}

/// Lays the demo's intake folders out under a fresh numbered folder of
/// `base` (so no earlier run's files are touched), on a background thread;
/// the receiver gets the folders once the files are there. The late file
/// is written afterwards, in two halves.
pub(crate) fn seed_intake(base: PathBuf) -> std::sync::mpsc::Receiver<Seeded> {
    let (tx, rx) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("cl-demo-intake".into())
        .spawn(move || {
            let Some(root) = (1..10_000)
                .map(|n| base.join(n.to_string()))
                .find(|p| !p.exists())
            else {
                return;
            };
            let bills = root.join("Bills");
            let receipts = root.join("Receipts");
            let scanner = root.join("Scanner");
            if std::fs::create_dir_all(&bills).is_err()
                || std::fs::create_dir_all(&receipts).is_err()
            {
                return;
            }
            let dirs = [&bills, &receipts];
            for (folder, name, bytes, hours) in INTAKE_SEED {
                write_aged(&dirs[folder].join(name), bytes, hours);
            }
            if let Ok(f) = std::fs::File::create(bills.join(INTAKE_LARGE)) {
                let _ = f.set_len(11 * 1024 * 1024);
            }
            let _ = std::fs::write(receipts.join(OTHER), b"synthetic notes");
            let bills_key = cl_company::intake::canonical_key(&bills).ok();
            let late = receipts.join(INTAKE_LATE);
            if tx
                .send(Seeded {
                    bills,
                    receipts,
                    scanner,
                    bills_key,
                })
                .is_err()
            {
                return;
            }
            let (first, second) = BILL_PDF.split_at(BILL_PDF.len() / 2);
            std::thread::sleep(LATE_FIRST);
            let _ = std::fs::write(&late, first);
            std::thread::sleep(LATE_SECOND);
            if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(&late) {
                use std::io::Write as _;
                let _ = f.write_all(second);
            }
        });
    if spawned.is_err() {
        tracing::warn!("could not lay out the demo intake folders");
    }
    rx
}

/// Puts the laid-out folders in the demo engine and on the company record:
/// "Bills" (confirmed here, originals left in place) and "Scanner" (not
/// confirmed here) in the engine, and P17's folder set to `receipts`, which
/// the migration turns into "Receipts".
pub(crate) fn apply_seed(link: &CompanyLink, tenant: cl_core::ids::TenantId, s: &Seeded) {
    use cl_core::intake::{IntakeFolder, IntakeKind};
    let text = |p: &Path| p.to_string_lossy().into_owned();
    let bills = IntakeFolder {
        id: crate::screens::preferences::intake_folders::new_folder_id(),
        name: "Bills".into(),
        path: text(&s.bills),
        default_kind: IntakeKind::Bills,
        auto_read: false,
        keep_originals: true,
    };
    let scanner = IntakeFolder {
        id: crate::screens::preferences::intake_folders::new_folder_id(),
        name: "Scanner".into(),
        path: text(&s.scanner),
        default_kind: IntakeKind::Auto,
        auto_read: false,
        keep_originals: false,
    };
    let name = {
        let Some(mut demo) = link.demo_state() else {
            return;
        };
        demo.intake
            .save_elsewhere(tenant, vec![bills.clone(), scanner]);
        demo.companies
            .iter()
            .find(|c| c.tenant_id == tenant)
            .map(|c| c.name.clone())
            .unwrap_or_default()
    };
    let receipts = s.receipts.clone();
    let note = s.bills_key.clone().map(|key| cl_settings::HereNote {
        path_hash: cl_settings::path_hash(&bills.path),
        canonical_key: key,
        since_ms: None,
        confirmed_at: Some(cl_settings::recent::now()),
    });
    let r = link.store().update(|l| {
        if l.get(tenant).is_none() {
            l.upsert(tenant, &name);
        }
        if let Some(rec) = l.get_mut(tenant) {
            rec.prefs.intake_here.clear();
            if let Some(n) = note {
                rec.prefs.intake_here.insert(bills.id, n);
            }
            rec.prefs.receipt_inbox_dir = Some(receipts);
        }
    });
    if let Err(e) = r {
        tracing::warn!(error = %e, "could not set up the demo intake folders");
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
