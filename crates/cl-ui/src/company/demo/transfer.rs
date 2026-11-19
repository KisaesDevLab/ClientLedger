//! The demo engine's Send to My Accountant and IIF export (P32 D6): fixed
//! check totals, the waiting payroll drafts of the demo, a transfer file
//! written as placeholder bytes (the demo has no archive), and an IIF file
//! made by the real writer from a small synthetic company. Scenarios turn
//! failures on for the next call through `dev.*` commands.

use cl_api::VerificationTotals;
use cl_company::iif::{self, Books, IifError};
use cl_company::transfer::{
    MissingAttachment, Preflight, TransferError, TransferOutcome, TransferRequest, history_totals,
};
use cl_core::{CalendarDate, Money};
use cl_settings::TransferRecord;

/// What the next demo call does instead of succeeding.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DemoTransferFailure {
    /// It succeeds.
    #[default]
    None,
    /// Another archive job is running (409 `TENANT_BUSY`).
    Busy,
    /// Three attachment files are missing or changed.
    MissingFiles,
}

/// Demo transfer state.
#[derive(Clone, Debug, Default)]
pub struct DemoTransfer {
    /// The next transfer's failure (taken once).
    pub fail_next: DemoTransferFailure,
    /// The next IIF export includes a transaction that doesn't foot.
    pub iif_unbalanced: bool,
    /// Documents waiting in Review Documents.
    pub waiting_documents: u64,
    /// "Sent before".
    pub history: Vec<TransferRecord>,
    /// Unposted payroll drafts the scenarios add (on top of the demo's).
    pub payroll_drafts: u64,
    /// The books changed since the preview (T11).
    pub posted_since_preview: bool,
}

fn money(s: &str) -> Money {
    Money::parse_wire(s).unwrap_or(Money::ZERO)
}

/// The demo's check totals as of `as_of` (the mockup's figures).
#[must_use]
pub fn preview(d: &super::DemoState, as_of: CalendarDate) -> VerificationTotals {
    let posted = d.transfer.posted_since_preview;
    let from = as_of.start_of_year();
    let counts = serde_json::json!({"accounts": 64, "clients": 64, "vendors": 38, "items": 22,
        "transactions": if posted { 1513 } else { 1512 }, "postedTransactions": 1498,
        "attachments": 214, "attachmentBytes": 48_000_000, "auditEntries": 9120});
    serde_json::from_value(serde_json::json!({
        "definition": "vt1",
        "basis": "accrual",
        "asOf": as_of.to_wire(),
        "companies": [{
            "companyId": "demo-company",
            "name": "Ozark Ridge Landscaping LLC",
            "fiscalYearStart": from.to_wire(),
            "totalAssets": if posted { "413006.3300" } else { "412906.3300" },
            "totalLiabilities": "98214.6000",
            "totalEquity": if posted { "314791.7300" } else { "314691.7300" },
            "netIncome": {"from": from.to_wire(), "to": as_of.to_wire(),
                          "amount": if posted { "126085.0000" } else { "125985.0000" }},
            "accountsReceivable": "48912.4000",
            "accountsPayable": "9286.9000",
            "trialBalance": {"debits": "640210.5500", "credits": "640210.5500"},
            "counts": counts.clone(),
        }],
        "counts": counts,
    }))
    .unwrap_or_else(|_| fallback(as_of))
}

fn fallback(as_of: CalendarDate) -> VerificationTotals {
    VerificationTotals {
        definition: "vt1".into(),
        basis: None,
        companies: Vec::new(),
        as_of,
        fiscal_year_start: None,
        total_assets: Money::ZERO,
        total_liabilities: Money::ZERO,
        total_equity: Money::ZERO,
        net_income: cl_api::NetIncome {
            from: as_of,
            to: as_of,
            amount: Money::ZERO,
        },
        accounts_receivable: Money::ZERO,
        accounts_payable: Money::ZERO,
        trial_balance: cl_api::TrialBalanceTotals {
            debits: Money::ZERO,
            credits: Money::ZERO,
        },
        counts: cl_api::TotalsCounts::default(),
    }
}

/// What the Send screen needs first.
#[must_use]
pub fn preflight(d: &super::DemoState) -> Preflight {
    Preflight {
        unposted_payroll: d.payroll.unposted() + d.transfer.payroll_drafts,
        waiting_documents: d.transfer.waiting_documents,
        capability_ok: true,
    }
}

/// Makes a demo transfer: the same checks and order as the real one
/// (passphrase, payroll consent, backup, discard, export), writing
/// placeholder bytes.
///
/// # Errors
/// As [`cl_company::transfer::make_transfer`].
pub fn make(
    d: &mut super::DemoState,
    req: &TransferRequest,
    backup_passphrase: Option<&cl_core::Secret>,
) -> Result<TransferOutcome, TransferError> {
    if req.passphrase.expose().chars().count() < cl_company::transfer::MIN_PASSPHRASE_CHARS {
        return Err(TransferError::PassphraseTooShort);
    }
    if req.passphrase.expose() != req.passphrase_again.expose() {
        return Err(TransferError::PassphraseMismatch);
    }
    if backup_passphrase.is_some_and(|b| b.expose() == req.passphrase.expose()) {
        return Err(TransferError::SameAsBackupPassphrase);
    }
    let unposted = d.payroll.unposted() + d.transfer.payroll_drafts;
    if unposted > 0 && !req.discard_payroll {
        return Err(TransferError::PayrollDraftsWaiting(unposted));
    }
    match std::mem::take(&mut d.transfer.fail_next) {
        DemoTransferFailure::Busy => return Err(TransferError::Busy),
        DemoTransferFailure::MissingFiles => {
            return Err(TransferError::MissingFiles(missing_files()));
        }
        DemoTransferFailure::None => {}
    }
    let ids: Vec<_> = d
        .payroll
        .sessions(None)
        .sessions
        .into_iter()
        .filter(|s| s.status.is_unposted_draft())
        .map(|s| s.id)
        .collect();
    for id in &ids {
        let _ = d.payroll.discard(*id);
    }
    let totals = preview(d, req.as_of);
    let bytes = b"ClientLedger demo transfer file (not an archive)".to_vec();
    if let Some(dir) = req.dest.parent() {
        std::fs::create_dir_all(dir).map_err(|e| {
            TransferError::Company(cl_company::CompanyError::Io {
                what: "save transfer file",
                source: e,
            })
        })?;
    }
    std::fs::write(&req.dest, &bytes).map_err(|e| {
        TransferError::Company(cl_company::CompanyError::Io {
            what: "save transfer file",
            source: e,
        })
    })?;
    let record = TransferRecord {
        path: req.dest.clone(),
        sha256: cl_company::backup::sha256_file(&req.dest).unwrap_or_default(),
        archive_id: format!("demo-transfer-{}", d.transfer.history.len() + 1),
        created_at: super::DemoState::now(),
        totals: Some(history_totals(&totals)),
    };
    d.transfer.history.push(record.clone());
    d.transfer.posted_since_preview = false;
    Ok(TransferOutcome {
        record,
        totals: Some(totals),
        bytes: bytes.len() as u64,
        discarded_payroll: ids.len() as u64 + std::mem::take(&mut d.transfer.payroll_drafts),
    })
}

fn missing_files() -> Vec<MissingAttachment> {
    vec![
        MissingAttachment {
            attachment_id: "demo-att-1".into(),
            file_name: Some("Receipt-Fuel-0412.pdf".into()),
            record_type: Some("expense".into()),
            record_id: Some("demo-expense-0412".into()),
            reason: "FILE_MISSING".into(),
        },
        MissingAttachment {
            attachment_id: "demo-att-2".into(),
            file_name: Some("Mulch-invoice.jpg".into()),
            record_type: Some("bill".into()),
            record_id: Some("demo-bill-1003".into()),
            reason: "FILE_SIZE_MISMATCH".into(),
        },
        MissingAttachment {
            attachment_id: "demo-att-3".into(),
            file_name: None,
            record_type: None,
            record_id: None,
            reason: "EXTERNAL_FILE".into(),
        },
    ]
}

/// A small synthetic company for the demo IIF export.
fn demo_books(unbalanced: bool) -> Books {
    use iif::{ContactKind, SourceAccount, SourceContact, SourceLine, SourceTxn};
    let acct = |id: &str, name: &str, ty: &str, detail: &str, tag: Option<&str>| SourceAccount {
        id: id.into(),
        name: name.into(),
        number: None,
        parent_id: None,
        account_type: ty.into(),
        detail_type: Some(detail.into()),
        system_tag: tag.map(str::to_string),
        description: None,
    };
    let line = |id: &str, a: &str, amt: &str| SourceLine {
        line_id: id.into(),
        account_id: a.into(),
        contact_id: None,
        amount: money(amt),
        memo: None,
    };
    let date = |s: &str| CalendarDate::parse_wire(s).unwrap_or_else(|_| CalendarDate::min());
    let mut txns = vec![
        SourceTxn {
            id: "demo-t1".into(),
            date: date("2026-10-02"),
            txn_type: "invoice".into(),
            number: Some("1042".into()),
            memo: Some("October mowing".into()),
            contact_id: Some("c1".into()),
            lines: vec![line("1", "ar", "1250.00"), line("2", "inc", "-1250.00")],
        },
        SourceTxn {
            id: "demo-t2".into(),
            date: date("2026-10-09"),
            txn_type: "bill".into(),
            number: Some("B-77".into()),
            memo: Some("Mulch".into()),
            contact_id: Some("v1".into()),
            lines: vec![line("3", "exp", "400.00"), line("4", "ap", "-400.00")],
        },
    ];
    if unbalanced {
        txns.push(SourceTxn {
            id: "demo-t3".into(),
            date: date("2026-10-15"),
            txn_type: "journal_entry".into(),
            number: Some("JE-12".into()),
            memo: Some("Split fuel".into()),
            contact_id: None,
            lines: vec![
                line("5", "exp", "0.005"),
                line("6", "exp", "0.005"),
                line("7", "bank", "-0.01"),
            ],
        });
    }
    Books {
        accounts: vec![
            acct("bank", "Operating Checking", "asset", "bank", None),
            acct(
                "ar",
                "Accounts Receivable",
                "asset",
                "accounts_receivable",
                Some("accounts_receivable"),
            ),
            acct(
                "ap",
                "Accounts Payable",
                "liability",
                "accounts_payable",
                Some("accounts_payable"),
            ),
            acct("inc", "Lawn Care", "revenue", "service", None),
            acct("exp", "Supplies", "expense", "office_supplies", None),
        ],
        contacts: vec![
            SourceContact {
                id: "c1".into(),
                name: "Maple Street HOA".into(),
                kind: ContactKind::Client,
            },
            SourceContact {
                id: "v1".into(),
                name: "Green Supply".into(),
                kind: ContactKind::Vendor,
            },
        ],
        items: Vec::new(),
        txns,
    }
}

/// A demo IIF export, written by the real writer.
///
/// # Errors
/// As [`cl_company::iif::fetch::export_iif`] (`Stopped` for the demo's
/// unbalanced entry, `HiddenAccounts`, `Io`).
pub fn iif_export(
    d: &mut super::DemoState,
    req: &iif::fetch::IifRequest,
) -> Result<iif::fetch::IifOutcome, IifError> {
    if !req.sees_hidden {
        return Err(IifError::HiddenAccounts);
    }
    let unbalanced = std::mem::take(&mut d.transfer.iif_unbalanced);
    let plan = iif::plan(&demo_books(unbalanced), req.options)?;
    let bytes = iif::fetch::write_file(&plan, &req.dest)?;
    Ok(iif::fetch::IifOutcome {
        path: req.dest.clone(),
        counts: plan.counts,
        limitations: plan.limitations,
        unmappable_chars: plan.unmappable_chars,
        bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cl_company::iif::Options;

    #[test]
    fn the_demo_transfer_follows_the_real_rules() {
        let dir = tempfile::tempdir().expect("tmp");
        let mut d = super::super::DemoState::default();
        let as_of = CalendarDate::from_ymd(2026, 11, 4).expect("d");
        let mut req = TransferRequest {
            tenant_id: cl_core::ids::TenantId::parse("1b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01")
                .expect("id"),
            dest: dir.path().join("x.clxfer"),
            passphrase: cl_core::Secret::new("short"),
            passphrase_again: cl_core::Secret::new("short"),
            discard_payroll: true,
            backup_dir: dir.path().to_path_buf(),
            as_of,
        };
        assert!(matches!(
            make(&mut d, &req, None),
            Err(TransferError::PassphraseTooShort)
        ));
        req.passphrase = cl_core::Secret::new("ABCD-EFGH-JKLM-NPQR-STUV");
        req.passphrase_again = req.passphrase.clone();
        d.transfer.fail_next = DemoTransferFailure::MissingFiles;
        assert!(matches!(
            make(&mut d, &req, None),
            Err(TransferError::MissingFiles(f)) if f.len() == 3
        ));
        let out = make(&mut d, &req, None).expect("made");
        assert!(req.dest.is_file());
        assert_eq!(d.transfer.history.len(), 1);
        assert_eq!(
            out.totals.expect("totals").total_assets.to_wire(),
            "412906.3300"
        );
        d.transfer.iif_unbalanced = true;
        let iif_req = iif::fetch::IifRequest {
            dest: dir.path().join("x.iif"),
            start: None,
            end: None,
            options: Options::default(),
            sees_hidden: true,
        };
        assert!(matches!(
            iif_export(&mut d, &iif_req),
            Err(IifError::Stopped(s)) if s.len() == 1
        ));
        assert!(!iif_req.dest.exists());
        let ok = iif_export(&mut d, &iif_req).expect("iif");
        assert_eq!(ok.counts.transactions, 2);
        assert!(iif_req.dest.is_file());
    }
}
