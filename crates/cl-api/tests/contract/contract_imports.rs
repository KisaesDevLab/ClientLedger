//! QBD conversion and other-system imports against the live engine (P31
//! plan §5): the synthetic sets tie out to 0.00 (trial balance, both agings,
//! the history accounts and the valuation), the altered sets differ on the
//! right rows, undo leaves the books exactly as before, a killed commit ends
//! `failed` and a retry posts once, and a re-import or an overlapping
//! journal is refused.
//!
//! Every test makes its own company (`fresh_company`, opened on S3), so the
//! re-import checks never trip on a rerun. Expected values come from the
//! fixtures' `EXPECTED.toml` (worked out by hand; `tests/imports_fixtures.rs`
//! re-sums them offline), never from the engine.
//!
//! PENDING until the pinned engine advertises the P31 capabilities
//! (`imports-amounts-v1`, `imports-batch-v1`, `qbd-open-items-v1`,
//! `qbd-mapping-v1`, `qbd-dates-v1`, `imports-tieout-v1`).

use std::time::{Duration, Instant};

use cl_api::imports::{
    BalanceKey, BatchKind, BatchOptions, BatchState, BatchStatus, CommitRequest, ContactKind,
    FileRole, Flow, ImportBatchId, ImportKind, ImportRefusal, ImportSessionId, NewBatch,
    SourceSystem, SpotCheckItem, TbColumn, TieOut, UploadRequest, import_refusal,
};
use cl_api::{AccountFilter, Capabilities, IdempotencyKey, NewCompany, ReportBasis, Session};
use cl_core::CalendarDate;
use rust_decimal::Decimal;

use crate::imports_expect::{self as fx, Expected};
use crate::support::{Harness, unique};

const QBD: &str = "qbd/shoal-creek";

/// The capability each P31 engine PR adds, with its U-row.
fn caps(h: &Harness, flow: Flow) -> bool {
    flow.capabilities().iter().all(|c| {
        let u = match *c {
            Capabilities::IMPORTS_AMOUNTS => "U-260",
            Capabilities::IMPORTS_BATCH => "U-261",
            Capabilities::QBD_OPEN_ITEMS => "U-264",
            Capabilities::QBD_MAPPING => "U-265",
            Capabilities::QBD_DATES => "U-266",
            Capabilities::IMPORTS_TIEOUT => "U-267",
            _ => "U-268",
        };
        h.require_cap(c, u)
    })
}

fn day(s: &str) -> CalendarDate {
    CalendarDate::parse_wire(s).expect("date")
}

/// A new company (its own tenant) opened on S3.
fn fresh_company(h: &Harness, name: &str) -> (Session, cl_core::ids::TenantId) {
    h.auth_slots(1);
    let tenant = h
        .run(h.s1.create_company(&NewCompany {
            company_name: unique(name),
            business_type: Some("default".into()),
            entity_type: None,
            system_accounts_only: Some(true),
        }))
        .unwrap_or_else(|e| panic!("create {name}: {e}"))
        .tenant_id;
    let s = h.s3();
    h.run(s.open_company(tenant))
        .unwrap_or_else(|e| panic!("open {name}: {e}"));
    (s, tenant)
}

/// One file of a set, as its `EXPECTED.toml` `[files]` names it.
struct FileSpec {
    path: std::path::PathBuf,
    kind: ImportKind,
    role: FileRole,
    contact: Option<ContactKind>,
    report_date: Option<CalendarDate>,
}

fn source_of(e: &Expected) -> SourceSystem {
    let s = e
        .get("company")
        .and_then(|c| c.get("source"))
        .map_or("quickbooks_desktop", String::as_str);
    SourceSystem::from_wire(s).expect("source")
}

/// The files of `set` in upload order, with `replace` (stem → path) swapped
/// in for altered sets.
fn files(set: &str, replace: &[(&str, std::path::PathBuf)]) -> Vec<FileSpec> {
    let e = fx::expected(set);
    let company = fx::section(&e, "company");
    let mut out = Vec::new();
    for (stem, what) in fx::section(&e, "files") {
        let mut parts = what.split(':');
        let kind = ImportKind::from_wire(parts.next().expect("kind")).expect("kind name");
        let rest = parts.next();
        let role = if rest == Some("reference") {
            FileRole::Reference
        } else {
            FileRole::Post
        };
        let contact = (kind == ImportKind::Contacts)
            .then(|| ContactKind::from_wire(rest.unwrap_or("customer")).expect("list"));
        // Files that carry no date of their own get the set's dates.
        let report_date = (kind == ImportKind::TrialBalance).then(|| {
            day(if role == FileRole::Reference {
                &company["conversion_date"]
            } else {
                &company["opening_tb_date"]
            })
        });
        let path = replace.iter().find(|(s, _)| s == stem).map_or_else(
            || fx::set_dir(set).join(format!("{stem}.csv")),
            |(_, p)| p.clone(),
        );
        out.push(FileSpec {
            path,
            kind,
            role,
            contact,
            report_date,
        });
    }
    // Lists before balances, balances before history (the engine's order).
    out.sort_by_key(|f| match f.kind {
        ImportKind::Coa => 0,
        ImportKind::Contacts => 1,
        ImportKind::Items => 2,
        ImportKind::TrialBalance => 3,
        ImportKind::GlTransactions => 4,
        _ => 5,
    });
    out
}

/// A `QuickBooks` Online CSV fixture as the xlsx QBO exports: numbers in
/// numeric cells (E0 reads their shortest round-trip text).
fn qbo_xlsx(csv: &str) -> Vec<u8> {
    let mut wb = rust_xlsxwriter::Workbook::new();
    let ws = wb.add_worksheet();
    for (r, row) in fx::read_csv(csv).iter().enumerate() {
        let r = u32::try_from(r).expect("row");
        for (c, cell) in row.iter().enumerate() {
            let c = u16::try_from(c).expect("col");
            let plain = cell.replace([',', '$'], "");
            // The only float in the import tests: a QBO workbook stores
            // amounts as IEEE doubles, which is what E0 must read exactly.
            match plain.parse::<f64>() {
                Ok(n) if !plain.is_empty() && plain.contains('.') => {
                    ws.write_number(r, c, n).expect("number");
                }
                _ => {
                    ws.write_string(r, c, cell).expect("text");
                }
            }
        }
    }
    wb.save_to_buffer().expect("xlsx")
}

fn upload(h: &Harness, s: &Session, source: SourceSystem, f: &FileSpec) -> ImportSessionId {
    let raw = std::fs::read(&f.path).unwrap_or_else(|e| panic!("{}: {e}", f.path.display()));
    let (bytes, file_name) = if source == SourceSystem::QuickBooksOnline {
        (
            qbo_xlsx(&String::from_utf8(raw).expect("utf-8")),
            "report.xlsx",
        )
    } else {
        (raw, "report.csv")
    };
    let needs_date = matches!(
        source,
        SourceSystem::AccountingPower | SourceSystem::Generic
    );
    let req = UploadRequest {
        kind: f.kind,
        source,
        role: f.role,
        bytes,
        file_name: file_name.into(),
        contact_kind: f.contact,
        tb_column: (source == SourceSystem::AccountingPower && f.kind == ImportKind::TrialBalance)
            .then_some(TbColumn::Adjusted),
        tb_report_date: if needs_date { f.report_date } else { None },
    };
    let out = h
        .run(s.imports().upload(&req))
        .unwrap_or_else(|e| panic!("upload {}: {e}", f.path.display()));
    assert!(out.is_clean(), "{}: {:?}", f.path.display(), out.problems);
    out.session.id
}

/// Uploads a set and groups it into a batch.
fn batch_of(
    h: &Harness,
    s: &Session,
    set: &str,
    replace: &[(&str, std::path::PathBuf)],
    history_from: Option<CalendarDate>,
) -> ImportBatchId {
    let e = fx::expected(set);
    let source = source_of(&e);
    let sessions: Vec<ImportSessionId> = files(set, replace)
        .iter()
        .map(|f| upload(h, s, source, f))
        .collect();
    let qbd = set.starts_with("qbd/");
    let new = NewBatch {
        source,
        kind: BatchKind::Conversion,
        sessions,
        options: BatchOptions {
            fiscal_year_start_month: Some(1),
            history_from,
            require_empty_company: qbd,
            ..BatchOptions::default()
        },
    };
    h.run(s.imports().create_batch(&new))
        .unwrap_or_else(|e| panic!("batch {set}: {e}"))
        .id
}

/// Polls the batch until its job is done.
fn wait(h: &Harness, s: &Session, id: ImportBatchId) -> BatchState {
    let start = Instant::now();
    loop {
        let st = h.run(s.imports().batch(id)).expect("batch state");
        if !st.batch.status.is_running() {
            return st;
        }
        assert!(
            start.elapsed() < Duration::from_mins(3),
            "job still running: {:?}",
            st.progress
        );
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn commit(h: &Harness, s: &Session, id: ImportBatchId) -> BatchState {
    let req = CommitRequest {
        backup_ref: unique("contract-before-import"),
        ..CommitRequest::default()
    };
    h.run(s.imports().commit(id, &req, IdempotencyKey::new()))
        .unwrap_or_else(|e| panic!("commit: {e}"));
    let st = wait(h, s, id);
    assert_eq!(st.batch.status, BatchStatus::Committed, "{:?}", st.error);
    st
}

fn undo(h: &Harness, s: &Session, id: ImportBatchId) {
    h.run(s.imports().undo(
        id,
        &unique("contract-before-undo"),
        None,
        IdempotencyKey::new(),
    ))
    .unwrap_or_else(|e| panic!("undo: {e}"));
    let st = wait(h, s, id);
    assert_eq!(st.batch.status, BatchStatus::Undone, "{:?}", st.error);
}

/// The nonzero account differences, keyed by the source account key.
fn account_diffs(t: &TieOut) -> fx::Balances {
    let mut d = fx::Balances::new();
    for a in &t.accounts {
        let key = a
            .source_keys
            .first()
            .cloned()
            .unwrap_or_else(|| a.name.clone());
        fx::add(&mut d, &key, a.diff.as_decimal());
    }
    d
}

fn balance(t: &TieOut, key: BalanceKey) -> Option<Decimal> {
    t.balance_rows
        .iter()
        .find(|r| r.key == key)
        .map(|r| r.diff.as_decimal())
}

fn assert_ties_out(t: &TieOut, what: &str) {
    assert!(t.all_zero, "{what}: {:?}", account_diffs(t));
    assert_eq!(account_diffs(t), fx::Balances::new(), "{what}");
    for i in t
        .ar_items
        .iter()
        .chain(&t.ap_items)
        .chain(&t.inventory_items)
    {
        assert!(i.diff.is_zero(), "{what}: row {}", i.source_row);
    }
}

/// The books as the owner sees them: the exact trial balance text and the
/// chart of accounts.
fn snapshot(h: &Harness, s: &Session) -> (String, Vec<(Option<String>, String, bool)>) {
    let tb = h
        .run(s.reports().trial_balance_csv(
            day("2000-01-01"),
            day("2030-12-31"),
            ReportBasis::Accrual,
        ))
        .expect("tb");
    let mut accounts: Vec<_> = h
        .run(s.accounts().list_all(&AccountFilter::default()))
        .expect("accounts")
        .into_iter()
        .map(|a| (a.fields.account_number, a.fields.name, a.is_active))
        .collect();
    accounts.sort();
    (tb, accounts)
}

/// Acceptance 1: the QBD set ties out at both history depths, including
/// the history accounts and the valuation row; Mark verified works and the
/// spot-check list is kept.
pub fn qbd_conversion_ties_out(h: &Harness) {
    if !caps(h, Flow::QbdConversion) {
        return;
    }
    let e = fx::expected(QBD);
    let two_years = day(&fx::section(&e, "company")["history_from_two_years"]);
    for history_from in [Some(two_years), None] {
        let (s, _) = fresh_company(h, "Shoal Creek");
        let id = batch_of(h, &s, QBD, &[], history_from);
        let st = commit(h, &s, id);
        assert_eq!(
            st.batch.conversion_date,
            Some(day(&fx::section(&e, "company")["conversion_date"]))
        );
        let t = h.run(s.imports().tieout(id)).expect("tieout");
        assert_ties_out(&t, &format!("history from {history_from:?}"));
        for key in [
            BalanceKey::HistoryAr,
            BalanceKey::HistoryAp,
            BalanceKey::HistoryInventory,
            BalanceKey::ValuationVsAsset,
        ] {
            assert_eq!(balance(&t, key), Some(Decimal::ZERO), "{key:?}");
        }
        assert_eq!(t.ar_items.len(), fx::section(&e, "open_invoices").len());
        assert_eq!(t.ap_items.len(), fx::section(&e, "unpaid_bills").len());
        assert_eq!(
            t.inventory_items.len(),
            fx::section(&e, "inventory_valuation").len()
        );
        let reference = fx::amounts(&e, "reference_tb");
        for a in &t.accounts {
            if let Some(want) = a.source_keys.first().and_then(|k| reference.get(k)) {
                assert_eq!(a.source.as_decimal(), *want, "{:?}", a.source_keys);
            }
        }
        let b = h.run(s.imports().verify(id)).expect("verify");
        assert!(b.is_verified());
        let items = vec![SpotCheckItem {
            key: "bank:1000".into(),
            label: "Checking".into(),
            done: true,
            note: String::new(),
        }];
        h.run(s.imports().save_spot_check(id, &items))
            .expect("spot check");
        let again = h.run(s.imports().batch(id)).expect("batch");
        assert_eq!(again.batch.spot_check.len(), 1);
        assert!(again.batch.spot_check[0].done);
    }
}

fn altered(set: &str, stem: &str) -> Vec<(&'static str, std::path::PathBuf)> {
    let e = fx::expected(set);
    let file = &fx::section(&e, "replaces")[stem];
    let stem: &'static str = match stem {
        "journal-2025-2026" => "journal-2025-2026",
        "open-invoices" => "open-invoices",
        "gl" => "gl",
        other => panic!("stem {other}"),
    };
    vec![(stem, fx::set_dir(set).join(file))]
}

/// Acceptance 2: a re-pointed line shows ±x on those two accounts only,
/// and verify is refused.
pub fn altered_repointed_two_accounts(h: &Harness) {
    if !caps(h, Flow::QbdConversion) {
        return;
    }
    let set = "qbd/shoal-creek-altered/repointed";
    let (s, _) = fresh_company(h, "Shoal Creek repointed");
    let id = batch_of(h, &s, QBD, &altered(set, "journal-2025-2026"), None);
    commit(h, &s, id);
    let t = h.run(s.imports().tieout(id)).expect("tieout");
    assert!(!t.all_zero);
    assert_eq!(account_diffs(&t), fx::amounts(&fx::expected(set), "diffs"));
    let e = h.run(s.imports().verify(id)).expect_err("not zero");
    assert!(matches!(
        import_refusal(&e),
        Some(ImportRefusal::TieOutNotZero { .. })
    ));
}

/// Acceptance 2: an open invoice raised by 10.00 shows on A/R and the
/// history account; its own row stays 0.00.
pub fn altered_open_item_ar_and_history(h: &Harness) {
    if !caps(h, Flow::QbdConversion) {
        return;
    }
    let set = "qbd/shoal-creek-altered/open-item";
    let e = fx::expected(set);
    let (s, _) = fresh_company(h, "Shoal Creek open item");
    let id = batch_of(h, &s, QBD, &altered(set, "open-invoices"), None);
    commit(h, &s, id);
    let t = h.run(s.imports().tieout(id)).expect("tieout");
    assert!(!t.all_zero);
    assert_eq!(account_diffs(&t), fx::amounts(&e, "diffs"));
    assert_eq!(
        balance(&t, BalanceKey::HistoryAr),
        Some(fx::dec(&fx::section(&e, "balance_rows")["history_ar"]))
    );
    assert!(t.ar_items.iter().all(|i| i.diff.is_zero()));
}

/// Acceptance 3: undo leaves the trial balance and the chart exactly as
/// before the import.
pub fn undo_restores_snapshot(h: &Harness) {
    if !caps(h, Flow::QbdConversion) {
        return;
    }
    let (s, _) = fresh_company(h, "Shoal Creek undo");
    let before = snapshot(h, &s);
    let id = batch_of(h, &s, QBD, &[], None);
    commit(h, &s, id);
    assert_ne!(snapshot(h, &s), before, "the import changed the books");
    undo(h, &s, id);
    assert_eq!(snapshot(h, &s), before);
}

/// Acceptance 3: a re-commit after undo creates everything again.
pub fn recommit_after_undo(h: &Harness) {
    if !caps(h, Flow::QbdConversion) {
        return;
    }
    let (s, _) = fresh_company(h, "Shoal Creek recommit");
    let first = batch_of(h, &s, QBD, &[], None);
    commit(h, &s, first);
    let posted = snapshot(h, &s);
    undo(h, &s, first);
    let second = batch_of(h, &s, QBD, &[], None);
    commit(h, &s, second);
    assert_eq!(snapshot(h, &s), posted, "all of it again");
    assert_ties_out(
        &h.run(s.imports().tieout(second)).expect("tieout"),
        "re-commit",
    );
}

/// Acceptance 3: the engine is killed during the commit. After the restart
/// Check status shows `failed` (or `committed` if the job won the race);
/// a retry with a new key posts once.
pub fn kill_mid_commit(h: &Harness) {
    if !caps(h, Flow::QbdConversion) {
        return;
    }
    let (s, tenant) = fresh_company(h, "Shoal Creek kill");
    let before = snapshot(h, &s);
    let id = batch_of(h, &s, QBD, &[], None);
    let req = CommitRequest {
        backup_ref: unique("contract-before-import"),
        ..CommitRequest::default()
    };
    h.run(s.imports().commit(id, &req, IdempotencyKey::new()))
        .expect("accepted");
    h.kill_engine_and_wait();
    let s = h.s3();
    h.run(s.open_company(tenant)).expect("reopen");
    let st = wait(h, &s, id);
    match st.batch.status {
        BatchStatus::Failed => {
            assert_eq!(snapshot(h, &s), before, "nothing posted");
            commit(h, &s, id);
        }
        BatchStatus::Committed => {}
        other => panic!("after restart: {other:?}"),
    }
    assert_ties_out(
        &h.run(s.imports().tieout(id)).expect("tieout"),
        "posted once",
    );
}

fn system_ties_out(h: &Harness, set: &str) {
    if !caps(h, Flow::OtherSystem) {
        return;
    }
    let (s, _) = fresh_company(h, set);
    let id = batch_of(h, &s, set, &[], None);
    commit(h, &s, id);
    assert_ties_out(&h.run(s.imports().tieout(id)).expect("tieout"), set);
}

/// Acceptance 4: Accounting Power ties out.
pub fn system_accounting_power_ties_out(h: &Harness) {
    system_ties_out(h, "imports/accounting-power");
}

/// Acceptance 4: `QuickBooks` Online (xlsx with numeric cells) ties out.
pub fn system_qbo_ties_out(h: &Harness) {
    system_ties_out(h, "imports/quickbooks-online");
}

/// Acceptance 4: a `QuickBooks` Desktop GL-only import ties out.
pub fn system_qbd_gl_ties_out(h: &Harness) {
    system_ties_out(h, "imports/qbd-gl");
}

/// Acceptance 4: Generic CSV ties out.
pub fn system_generic_ties_out(h: &Harness) {
    system_ties_out(h, "imports/generic");
}

/// Acceptance 4: the Generic altered GL differs on its two accounts.
pub fn generic_altered(h: &Harness) {
    if !caps(h, Flow::OtherSystem) {
        return;
    }
    let set = "imports/generic-altered";
    let (s, _) = fresh_company(h, "Generic altered");
    let id = batch_of(h, &s, "imports/generic", &altered(set, "gl"), None);
    commit(h, &s, id);
    let t = h.run(s.imports().tieout(id)).expect("tieout");
    assert!(!t.all_zero);
    assert_eq!(account_diffs(&t), fx::amounts(&fx::expected(set), "diffs"));
}

fn generic_gl_upload(h: &Harness, s: &Session) -> Result<ImportSessionId, cl_api::ApiError> {
    let f = files("imports/generic", &[])
        .into_iter()
        .find(|f| f.kind == ImportKind::GlTransactions)
        .expect("gl");
    let req = UploadRequest {
        kind: f.kind,
        source: SourceSystem::Generic,
        role: f.role,
        bytes: std::fs::read(&f.path).expect("gl"),
        file_name: "gl.csv".into(),
        contact_kind: None,
        tb_column: None,
        tb_report_date: None,
    };
    h.run(s.imports().upload(&req)).map(|o| o.session.id)
}

/// Acceptance 4: the same file again is refused, naming the earlier import.
pub fn reimport_refused_409(h: &Harness) {
    if !caps(h, Flow::OtherSystem) {
        return;
    }
    let (s, _) = fresh_company(h, "Reimport");
    let id = batch_of(h, &s, "imports/generic", &[], None);
    commit(h, &s, id);
    let e = generic_gl_upload(h, &s).expect_err("refused");
    let Some(ImportRefusal::AlreadyCommitted { batch, .. }) = import_refusal(&e) else {
        panic!("not a re-import refusal: {e:?}")
    };
    assert_eq!(batch, Some(id));
}

/// Acceptance 4 (Q-P31o): a journal whose dates overlap an earlier import
/// is refused, at batch or at commit.
pub fn overlap_refused_409(h: &Harness) {
    if !caps(h, Flow::QbdConversion) {
        return;
    }
    let (s, _) = fresh_company(h, "Overlap");
    let id = batch_of(h, &s, QBD, &[], None);
    commit(h, &s, id);
    let attempt = || -> Result<ImportBatchId, cl_api::ApiError> {
        let req = UploadRequest {
            kind: ImportKind::GlTransactions,
            source: SourceSystem::QuickBooksDesktop,
            role: FileRole::Post,
            bytes: fx::bytes("qbd/shoal-creek-altered", "journal-overlap.csv"),
            file_name: "journal.csv".into(),
            contact_kind: None,
            tb_column: None,
            tb_report_date: None,
        };
        let overlap = h.run(s.imports().upload(&req))?.session.id;
        let new = NewBatch {
            source: SourceSystem::QuickBooksDesktop,
            kind: BatchKind::Single,
            sessions: vec![overlap],
            options: BatchOptions::default(),
        };
        let b = h.run(s.imports().create_batch(&new))?;
        let commit = CommitRequest {
            backup_ref: unique("contract-before-import"),
            ..CommitRequest::default()
        };
        h.run(s.imports().commit(b.id, &commit, IdempotencyKey::new()))?;
        Ok(b.id)
    };
    match attempt() {
        Err(e) => assert!(
            matches!(
                import_refusal(&e),
                Some(ImportRefusal::PeriodOverlap { .. })
            ),
            "{e:?}"
        ),
        Ok(b) => {
            // Refused inside the job: the batch fails and posts nothing.
            let st = wait(h, &s, b);
            assert_eq!(st.batch.status, BatchStatus::Failed);
            assert!(matches!(
                st.error.expect("failure").refusal(),
                Some(ImportRefusal::PeriodOverlap { .. })
            ));
        }
    }
}
/// Acceptance 4: after an undo the same file imports again.
pub fn reimport_after_undo_allowed(h: &Harness) {
    if !caps(h, Flow::OtherSystem) {
        return;
    }
    let (s, _) = fresh_company(h, "Reimport after undo");
    let id = batch_of(h, &s, "imports/generic", &[], None);
    commit(h, &s, id);
    undo(h, &s, id);
    generic_gl_upload(h, &s).expect("allowed after undo");
}
