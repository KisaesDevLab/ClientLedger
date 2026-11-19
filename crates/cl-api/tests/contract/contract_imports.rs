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
//! Upload, batch, commit and undo run once the pin has the merged P31
//! capabilities (`imports-amounts-v1`, `imports-batch-v1`,
//! `qbd-open-items-v1`, `qbd-mapping-v1`, `qbd-dates-v1`); the tie-out,
//! verify, spot-check and batch transactions checks with `imports-tieout-v1`
//! (pin d9b2e037, U-267, U-271).

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

/// The capability each P31 engine PR adds, with its U-row: everything the
/// flow needs except the tie-out (E3), which [`tieout`] gates.
fn caps(h: &Harness, flow: Flow) -> bool {
    flow.capabilities().iter().all(|c| {
        if *c == Capabilities::IMPORTS_TIEOUT {
            return true;
        }
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

/// The tie-out and verify (E3, U-267).
fn tieout(h: &Harness) -> bool {
    h.require_cap(Capabilities::IMPORTS_TIEOUT, "U-267")
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

/// A new company for the QBD set: inventory on (the set has inventory
/// parts and a valuation; the engine refuses them otherwise,
/// `IMPORT_INVENTORY_OFF`).
fn qbd_company(h: &Harness, name: &str) -> (Session, cl_core::ids::TenantId) {
    let (s, tenant) = fresh_company(h, name);
    h.run(s.set_inventory_enabled(true, IdempotencyKey::new()))
        .unwrap_or_else(|e| panic!("inventory on {name}: {e}"));
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
    let rows = fx::read_csv(csv);
    // A QBO Journal shows Date and Transaction Type on an entry's first line
    // only (the engine starts an entry on a row with both); the CSV fixture
    // repeats them on every line.
    let header = rows
        .iter()
        .position(|r| r.iter().any(|c| c == "Transaction Type"));
    // Amount columns (by their header): every amount there is a number
    // cell, whole ones too (review M3: a raw integer value, not text).
    let amount_header = rows
        .iter()
        .position(|r| r.iter().any(|c| c == "Debit" || c == "Credit"));
    let amount_cols: Vec<usize> = amount_header
        .map(|h| &rows[h])
        .map(|r| {
            r.iter()
                .enumerate()
                .filter(|(_, c)| matches!(c.as_str(), "Debit" | "Credit" | "Amount"))
                .map(|(i, _)| i)
                .collect()
        })
        .unwrap_or_default();
    let mut last_entry: Option<(String, String, String)> = None;
    for (r, row) in rows.iter().enumerate() {
        let mut row = row.clone();
        if header.is_some_and(|h| r > h) && row.len() >= 3 {
            let entry = (row[0].clone(), row[1].clone(), row[2].clone());
            if last_entry.as_ref() == Some(&entry) {
                row[0].clear();
                row[1].clear();
            }
            last_entry = Some(entry);
        }
        let r = u32::try_from(r).expect("row");
        for (i, cell) in row.iter().enumerate() {
            let c = u16::try_from(i).expect("col");
            let plain = cell.replace([',', '$'], "");
            let amount_col = amount_header.is_some_and(|h| u32::try_from(h).is_ok_and(|h| r > h))
                && amount_cols.contains(&i);
            // The only float in the import tests: a QBO workbook stores
            // amounts as IEEE doubles, which is what E0 must read exactly.
            match plain.parse::<f64>() {
                Ok(n) if !plain.is_empty() && (plain.contains('.') || amount_col) => {
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
    upload_outcome(h, s, source, f).session.id
}

fn upload_outcome(
    h: &Harness,
    s: &Session,
    source: SourceSystem,
    f: &FileSpec,
) -> cl_api::imports::UploadOutcome {
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
        // As the app sends it (Q-P31ac): a QBD account list takes over a
        // starter account with its number.
        update_existing_coa: source == SourceSystem::QuickBooksDesktop && f.kind == ImportKind::Coa,
    };
    let out = h
        .run(s.imports().upload(&req))
        .unwrap_or_else(|e| panic!("upload {}: {e}", f.path.display()));
    assert!(out.is_clean(), "{}: {:?}", f.path.display(), out.problems);
    out
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
    let outcomes: Vec<_> = files(set, replace)
        .iter()
        .map(|f| upload_outcome(h, s, source, f))
        .collect();
    let sessions: Vec<ImportSessionId> = outcomes.iter().map(|o| o.session.id).collect();
    let qbd = set.starts_with("qbd/");
    // U-272: a QBD or QBO account list's Retained Earnings row carries the
    // role (it still says `create`); other programs' files never do.
    if h.require_cap(Capabilities::QBD_RETAINED_EARNINGS, "U-272") {
        let re: Vec<_> = outcomes
            .iter()
            .filter(|o| o.session.kind == ImportKind::Coa)
            .flat_map(|o| o.preview.account_matches.iter())
            .filter(|m| m.system_role == Some(cl_api::imports::SystemRole::RetainedEarnings))
            .collect();
        let expect = matches!(
            source,
            SourceSystem::QuickBooksDesktop | SourceSystem::QuickBooksOnline
        );
        assert_eq!(re.len(), usize::from(expect), "{set}: {re:?}");
    }
    let new = NewBatch {
        source,
        kind: BatchKind::Conversion,
        sessions,
        options: BatchOptions {
            fiscal_year_start_month: Some(1),
            history_from,
            // E2c: the QBD reference files must be dated it.
            conversion_date: qbd.then(|| day(&fx::section(&e, "company")["conversion_date"])),
            require_empty_company: qbd,
            ..BatchOptions::default()
        },
    };
    h.run(s.imports().create_batch(&new))
        .unwrap_or_else(|e| panic!("batch {set}: {e}"))
        .id
}

/// P31 QA-13: a `QuickBooks` Online report uploaded as CSV (the fixture as
/// it is, not converted to xlsx) on each report card. The engine's QBO
/// adapter reads the chart from CSV or xlsx but the reports from xlsx only:
/// each is refused with a validation error the desktop names in owner words
/// ("export it to Excel"). If the engine ever reads them, this says so.
pub fn qbo_csv_reports_are_refused(h: &Harness) {
    if !caps(h, Flow::OtherSystem) {
        return;
    }
    let (s, _) = fresh_company(h, "qbo-csv");
    let mut refused = Vec::new();
    for f in files("imports/quickbooks-online", &[]) {
        let raw = std::fs::read(&f.path).expect("fixture");
        let req = UploadRequest {
            kind: f.kind,
            source: SourceSystem::QuickBooksOnline,
            role: f.role,
            bytes: raw,
            file_name: "report.csv".into(),
            contact_kind: f.contact,
            tb_column: None,
            tb_report_date: None,
            update_existing_coa: false,
        };
        match h.run(s.imports().upload(&req)) {
            Ok(o) if o.is_clean() => {}
            Ok(o) => refused.push((f.kind, format!("problems {:?}", o.problems))),
            Err(e) => refused.push((f.kind, format!("{e:?}"))),
        }
    }
    eprintln!("QA-13 qbo csv: {refused:?}");
    // The chart reads from CSV; every report does not (QA-13).
    assert!(
        refused.iter().all(|(k, _)| *k != ImportKind::Coa),
        "{refused:?}"
    );
    assert!(
        refused.iter().any(|(k, _)| *k == ImportKind::TrialBalance),
        "the engine now reads QBO reports from CSV: drop the Excel note ({refused:?})"
    );
}

/// P31 QA-1 (Q-P31ac): a QBD conversion into a company made with a starter
/// chart. Every number the starter chart shares with the QuickBooks list
/// goes to `"create"` (the QuickBooks account, not the starter one). Prints
/// what the engine did; asserts the commit ties out.
pub fn qbd_into_a_starter_chart(h: &Harness) {
    if !caps(h, Flow::QbdConversion) {
        return;
    }
    h.auth_slots(1);
    let tenant = h
        .run(h.s1.create_company(&NewCompany {
            company_name: unique("starter-chart"),
            business_type: Some("default".into()),
            entity_type: None,
            system_accounts_only: Some(true),
        }))
        .expect("create")
        .tenant_id;
    let s = h.s3();
    h.run(s.open_company(tenant)).expect("open");
    h.run(s.set_inventory_enabled(true, IdempotencyKey::new()))
        .expect("inventory on");
    // The app's General service starter chart, as setup imports it.
    let tpl: serde_json::Value =
        serde_json::from_str(include_str!("../../../../assets/coa/general_service.json"))
            .expect("template");
    let system = [
        "10100", "10150", "10200", "20100", "20900", "30000", "30120",
    ];
    let rows: Vec<cl_api::AccountImportRow> = tpl["accounts"]
        .as_array()
        .expect("accounts")
        .iter()
        .filter(|a| a["requires"].as_str() != Some("sales_tax"))
        .filter(|a| !system.contains(&a["number"].as_str().unwrap_or_default()))
        .map(|a| cl_api::AccountImportRow {
            account_number: a["number"].as_str().map(str::to_string),
            name: a["name"].as_str().unwrap_or_default().to_string(),
            account_type: serde_json::from_value(a["type"].clone()).expect("type"),
            detail_type: a["detail"].as_str().map(str::to_string),
        })
        .collect();
    for chunk in rows.chunks(cl_api::IMPORT_MAX_ROWS) {
        h.run(s.accounts().import(chunk, false, IdempotencyKey::new()))
            .expect("starter chart");
    }
    let set = "qbd/shoal-creek";
    let e = fx::expected(set);
    let source = source_of(&e);
    let outcomes: Vec<_> = files(set, &[])
        .iter()
        .map(|f| upload_outcome(h, &s, source, f))
        .collect();
    // The account list says which starter numbers it takes over.
    let taken: Vec<String> = outcomes
        .iter()
        .filter(|o| o.session.kind == ImportKind::Coa)
        .flat_map(|o| o.preview.account_matches.iter())
        .filter(|m| {
            m.status == cl_api::imports::MatchStatus::Existing
                && m.matched_by == Some(cl_api::imports::MatchedBy::Number)
        })
        .map(|m| m.source_key.clone())
        .collect();
    assert!(taken.contains(&"2200".to_string()), "{taken:?}");
    let name_of = |n: &str| {
        h.run(s.accounts().list_all(&cl_api::AccountFilter::default()))
            .expect("chart")
            .into_iter()
            .filter(|a| a.fields.account_number.as_deref() == Some(n))
            .map(|a| a.fields.name)
            .collect::<Vec<_>>()
    };
    let before = name_of("2200");
    let new = NewBatch {
        source,
        kind: BatchKind::Conversion,
        sessions: outcomes.iter().map(|o| o.session.id).collect(),
        options: BatchOptions {
            fiscal_year_start_month: Some(1),
            conversion_date: Some(day(&fx::section(&e, "company")["conversion_date"])),
            require_empty_company: true,
            ..BatchOptions::default()
        },
    };
    let id = h.run(s.imports().create_batch(&new)).expect("batch").id;
    commit(h, &s, id);
    assert_eq!(
        name_of("2200"),
        vec!["Sales Tax Payable".to_string()],
        "the QuickBooks account takes the starter number"
    );
    assert_eq!(name_of("6300"), vec!["Payroll Expenses".to_string()]);
    if tieout(h) {
        let t = h.run(s.imports().tieout(id)).expect("tieout");
        assert!(t.all_zero, "QA-1: the QuickBooks chart ties out");
    }
    undo(h, &s, id);
    assert_eq!(
        name_of("2200"),
        before,
        "undo puts the starter account back"
    );
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

/// The nonzero account differences, keyed by the source account key. Since
/// U-272 (`qbd-retained-earnings-v1`) a numbered QBD Retained Earnings row
/// merges into the system account, so nothing is set aside.
fn account_diffs(t: &TieOut) -> fx::Balances {
    let mut d = fx::Balances::new();
    for a in &t.accounts {
        // Keyed as EXPECTED.toml has it: the source key, else the number.
        let key = a
            .source_keys
            .first()
            .or(a.number.as_ref())
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
    assert_eq!(t.difference_count, 0, "{what}");
    assert_eq!(account_diffs(t), fx::Balances::new(), "{what}");
    for i in t.ar_items.iter().chain(&t.ap_items) {
        assert!(i.diff.is_zero(), "{what}: row {:?}", i.source_row);
        assert!(i.source_row.is_some(), "{what}: books-only open item");
    }
    for i in &t.inventory_items {
        assert!(i.ties(), "{what}: item row {:?}", i.source_row);
    }
    for total in [t.totals.ar, t.totals.ap, t.totals.inventory]
        .into_iter()
        .flatten()
    {
        assert!(total.diff.is_zero(), "{what}: a side's total");
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
        let (s, _) = qbd_company(h, "Shoal Creek");
        let id = batch_of(h, &s, QBD, &[], history_from);
        let created = h.run(s.imports().batch(id)).expect("draft");
        let dc = created.batch.date_check.expect("date check (E2c)");
        assert_eq!(dc.opening_balance_date, Some(day("2023-12-31")));
        assert_eq!(dc.history_from, history_from);
        assert_eq!(dc.history_skipped, 0, "the journals start in 2024");
        let st = commit(h, &s, id);
        let conversion = day(&fx::section(&e, "company")["conversion_date"]);
        assert_eq!(st.batch.conversion_date, Some(conversion));
        assert_conversion_results(&st, &e, history_from);
        if !tieout(h) {
            continue;
        }
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
        // The tiles' totals: each aging's own total, as the file has it.
        let totals = fx::section(&e, "totals");
        assert_eq!(
            t.totals.ar.map(|x| x.source.as_decimal()),
            totals.get("open_invoices").map(|v| fx::dec(v)),
            "A/R aging total"
        );
        assert_eq!(
            t.totals.ap.map(|x| x.source.as_decimal()),
            totals.get("unpaid_bills").map(|v| fx::dec(v)),
            "A/P aging total"
        );
        assert_eq!(
            t.totals.accounts.map(|x| x.source_debit.as_decimal()),
            totals.get("reference_tb_debits").map(|v| fx::dec(v)),
            "the reference trial balance's debits"
        );
        assert!(t.reference.is_some(), "compared with the reference TB");
        // U-272: the numbered Retained Earnings merged, so verify succeeds.
        if !h.require_cap(Capabilities::QBD_RETAINED_EARNINGS, "U-272") {
            continue;
        }
        let v = h.run(s.imports().verify(id)).expect("verify");
        assert!(v.batch.is_verified());
        assert!(v.tieout.all_zero && v.tieout.verified_at.is_some());
        // Idempotent: again answers with what it kept.
        let again = h.run(s.imports().verify(id)).expect("verify again");
        assert_eq!(again.batch.verified_at, v.batch.verified_at);
        let kept = h.run(s.imports().tieout(id)).expect("kept tie-out");
        assert_eq!(kept.verified_at, v.tieout.verified_at);
        // No second Retained Earnings account was made from the chart.
        let charts = h
            .run(s.accounts().list_all(&AccountFilter::default()))
            .expect("accounts");
        let re = charts
            .iter()
            .filter(|a| {
                a.fields
                    .name
                    .to_ascii_lowercase()
                    .contains("retained earnings")
            })
            .count();
        assert_eq!(re, 1, "the source's Retained Earnings merged (U-272)");
        // The spot-check sample: the batch's own transactions, paged.
        let page = h
            .run(s.imports().transactions(id, None, 200))
            .expect("transactions");
        assert!(page.total > 0 && !page.transactions.is_empty());
        assert!(
            page.transactions
                .iter()
                .any(|x| x.txn_type == "invoice" || x.txn_type == "journal_entry"),
            "{:?}",
            page.transactions.first()
        );
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

/// The committed QBD batch's per-file results (E2b, E2c): names, items, the
/// valuation total exactly as `EXPECTED.toml` has it, the journals' history
/// skip count. Counts only; the screen never adds amounts across files.
fn assert_conversion_results(st: &BatchState, e: &Expected, history_from: Option<CalendarDate>) {
    let of = |kind: ImportKind| {
        st.batch
            .files
            .iter()
            .filter(|f| f.kind == Some(kind) && f.role == Some(FileRole::Post))
            .filter_map(|f| f.result.clone())
            .collect::<Vec<_>>()
    };
    let valuation = of(ImportKind::InventoryValuation);
    assert_eq!(valuation.len(), 1, "one valuation result");
    assert_eq!(
        valuation[0].valuation_total.map(cl_core::Money::as_decimal),
        Some(fx::dec(&fx::section(e, "totals")["inventory_valuation"]))
    );
    assert_eq!(
        valuation[0].items_valued,
        u64::try_from(fx::section(e, "inventory_valuation").len()).expect("count")
    );
    let items = of(ImportKind::Items);
    assert_eq!(items.len(), 1);
    assert!(items[0].created > 0, "items made");
    // One name is on both lists (Q-77).
    let contacts = of(ImportKind::Contacts);
    assert!(
        contacts.iter().map(|r| r.merged_to_both).sum::<u64>() >= 1,
        "{contacts:?}"
    );
    for j in of(ImportKind::GlTransactions) {
        assert_eq!(
            j.skipped_before_history.is_some(),
            history_from.is_some(),
            "{j:?}"
        );
    }
    for f in &st.batch.files {
        assert!(
            f.status
                .is_some_and(|s| s == cl_api::imports::SessionStatus::Committed),
            "{:?}",
            f.kind
        );
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
    let (s, _) = qbd_company(h, "Shoal Creek repointed");
    let id = batch_of(h, &s, QBD, &altered(set, "journal-2025-2026"), None);
    commit(h, &s, id);
    if !tieout(h) {
        return;
    }
    let t = h.run(s.imports().tieout(id)).expect("tieout");
    assert!(!t.all_zero);
    assert_eq!(account_diffs(&t), fx::amounts(&fx::expected(set), "diffs"));
    let e = h.run(s.imports().verify(id)).expect_err("not zero");
    let Some(ImportRefusal::TieOutNotZero {
        difference_count,
        first,
    }) = import_refusal(&e)
    else {
        panic!("not zero: {e:?}")
    };
    assert_eq!(difference_count, t.difference_count);
    assert!(difference_count > 0 && !first.is_empty() && first.len() <= 20);
    // The first differences point at rows the tie-out has (ids only).
    for d in &first {
        if d.section == Some(cl_api::imports::DifferenceSection::Accounts) {
            assert!(
                t.accounts.iter().any(|a| a.account == d.account
                    && a.source_row == d.source_row
                    && !a.diff.is_zero()),
                "{d:?}"
            );
        }
    }
}

/// Acceptance 2: an open invoice raised by 10.00 shows on A/R and the
/// history account; its own row stays 0.00.
pub fn altered_open_item_ar_and_history(h: &Harness) {
    if !caps(h, Flow::QbdConversion) {
        return;
    }
    let set = "qbd/shoal-creek-altered/open-item";
    let e = fx::expected(set);
    let (s, _) = qbd_company(h, "Shoal Creek open item");
    let id = batch_of(h, &s, QBD, &altered(set, "open-invoices"), None);
    commit(h, &s, id);
    if !tieout(h) {
        return;
    }
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
    let (s, _) = qbd_company(h, "Shoal Creek undo");
    let before = snapshot(h, &s);
    let id = batch_of(h, &s, QBD, &[], None);
    commit(h, &s, id);
    assert_ne!(snapshot(h, &s), before, "the import changed the books");
    undo(h, &s, id);
    assert_eq!(snapshot(h, &s), before);
    if !tieout(h) {
        return;
    }
    let e = h.run(s.imports().tieout(id)).expect_err("undone");
    assert_eq!(import_refusal(&e), Some(ImportRefusal::BatchUndone));
}

/// Acceptance 3: a re-commit after undo creates everything again.
pub fn recommit_after_undo(h: &Harness) {
    if !caps(h, Flow::QbdConversion) {
        return;
    }
    let (s, _) = qbd_company(h, "Shoal Creek recommit");
    let first = batch_of(h, &s, QBD, &[], None);
    commit(h, &s, first);
    let posted = snapshot(h, &s);
    undo(h, &s, first);
    let second = batch_of(h, &s, QBD, &[], None);
    commit(h, &s, second);
    assert_eq!(snapshot(h, &s), posted, "all of it again");
    if !tieout(h) {
        return;
    }
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
    let (s, tenant) = qbd_company(h, "Shoal Creek kill");
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
    if !tieout(h) {
        return;
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
    if !tieout(h) {
        return;
    }
    let t = h.run(s.imports().tieout(id)).expect("tieout");
    assert_ties_out(&t, set);
    // Verify (E3, live): verified, the tie-out kept, again answers the same.
    let v = h.run(s.imports().verify(id)).expect("verify");
    assert!(v.batch.is_verified(), "{set}");
    assert!(v.tieout.all_zero && v.tieout.verified_at.is_some(), "{set}");
    let again = h.run(s.imports().verify(id)).expect("verify again");
    assert_eq!(again.batch.verified_at, v.batch.verified_at, "{set}");
    let kept = h.run(s.imports().tieout(id)).expect("kept tie-out");
    assert_eq!(kept.verified_at, v.tieout.verified_at, "{set}");
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
    if !tieout(h) {
        return;
    }
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
        update_existing_coa: false,
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
    let (s, _) = qbd_company(h, "Overlap");
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
            update_existing_coa: false,
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
/// Uploads the QBD set's files `keep` picks and groups them (no fiscal
/// year, no empty-company check).
fn qbd_subset(
    h: &Harness,
    s: &Session,
    keep: impl Fn(&FileSpec) -> bool,
    options: BatchOptions,
) -> Result<cl_api::imports::ImportBatch, cl_api::ApiError> {
    let sessions = files(QBD, &[])
        .iter()
        .filter(|f| keep(f))
        .map(|f| upload(h, s, SourceSystem::QuickBooksDesktop, f))
        .collect();
    h.run(s.imports().create_batch(&NewBatch {
        source: SourceSystem::QuickBooksDesktop,
        kind: BatchKind::Conversion,
        sessions,
        options,
    }))
}

/// E2c: with a conversion date, every reference and open-item file of
/// another date is named (and nothing is grouped); a history start after
/// the conversion date is refused; the opening balances and the first
/// journal row are reported, with no warning for this set.
pub fn dates_are_checked_before_any_commit(h: &Harness) {
    if !caps(h, Flow::QbdConversion) {
        return;
    }
    let (s, _) = qbd_company(h, "Shoal Creek dates");
    let dated = |f: &FileSpec| {
        matches!(
            f.kind,
            ImportKind::OpenInvoices | ImportKind::UnpaidBills | ImportKind::InventoryValuation
        ) || (f.kind == ImportKind::TrialBalance && f.role == FileRole::Reference)
    };
    let wrong = BatchOptions {
        conversion_date: Some(day("2026-10-26")),
        ..BatchOptions::default()
    };
    let e = qbd_subset(h, &s, dated, wrong).expect_err("not the conversion date");
    let Some(ImportRefusal::DateMismatch {
        conversion_date,
        files,
    }) = import_refusal(&e)
    else {
        panic!("date mismatch: {e:?}")
    };
    assert_eq!(conversion_date, Some(day("2026-10-26")));
    assert_eq!(files.len(), 4, "every file named: {files:?}");
    assert!(files.iter().all(|f| f.session.is_some()
        && f.reason == Some(cl_api::imports::DateMismatchReason::NotConversionDate)));
    let (s, _) = qbd_company(h, "Shoal Creek history");
    let late = BatchOptions {
        conversion_date: Some(day("2026-10-27")),
        history_from: Some(day("2026-11-01")),
        ..BatchOptions::default()
    };
    let e = qbd_subset(h, &s, |f| f.kind == ImportKind::GlTransactions, late)
        .expect_err("history after conversion");
    assert_eq!(import_refusal(&e), Some(ImportRefusal::BadDate), "{e:?}");
}

/// E2b: a trial balance and journals with no chart file name accounts the
/// company lacks. The upload says so (`unmatched`, with row counts); the
/// commit fails with `IMPORT_UNMAPPED_ACCOUNT` and nothing posts. The retry
/// is a new batch: the failed batch's files are deleted, uploaded again,
/// and grouped with an `accountMap`; it then commits.
pub fn unmapped_accounts_fail_then_a_mapped_retry_posts(h: &Harness) {
    if !caps(h, Flow::QbdConversion) {
        return;
    }
    let (s, _) = fresh_company(h, "Shoal Creek unmapped");
    let before = snapshot(h, &s);
    let history = |f: &FileSpec| {
        f.kind == ImportKind::GlTransactions
            || (f.kind == ImportKind::TrialBalance && f.role == FileRole::Post)
    };
    let specs: Vec<FileSpec> = files(QBD, &[]).into_iter().filter(history).collect();
    let mut unmatched = std::collections::BTreeSet::new();
    let mut sessions = Vec::new();
    for f in &specs {
        let raw = std::fs::read(&f.path).expect("file");
        let out = h
            .run(s.imports().upload(&UploadRequest {
                kind: f.kind,
                source: SourceSystem::QuickBooksDesktop,
                role: f.role,
                bytes: raw,
                file_name: "report.csv".into(),
                contact_kind: None,
                tb_column: None,
                tb_report_date: None,
                update_existing_coa: false,
            }))
            .expect("upload");
        for m in &out.preview.account_matches {
            assert!(m.rows > 0, "{m:?}");
            if m.status == cl_api::imports::MatchStatus::Unmatched {
                unmatched.insert(m.source_key.clone());
            }
        }
        sessions.push(out.session.id);
    }
    assert!(
        !unmatched.is_empty(),
        "no chart file: accounts are unmatched"
    );
    let first = h
        .run(s.imports().create_batch(&NewBatch {
            source: SourceSystem::QuickBooksDesktop,
            kind: BatchKind::Conversion,
            sessions: sessions.clone(),
            options: BatchOptions::default(),
        }))
        .expect("batch");
    let req = CommitRequest {
        backup_ref: unique("contract-before-import"),
        ..CommitRequest::default()
    };
    h.run(s.imports().commit(first.id, &req, IdempotencyKey::new()))
        .expect("accepted");
    let st = wait(h, &s, first.id);
    assert_eq!(st.batch.status, BatchStatus::Failed);
    let Some(ImportRefusal::UnmappedAccount { keys, count }) = st
        .error
        .as_ref()
        .and_then(cl_api::imports::BatchFailure::refusal)
    else {
        panic!("unmapped: {:?}", st.error)
    };
    assert!(count > 0 && !keys.0.is_empty());
    assert!(
        keys.0.iter().all(|k| unmatched.contains(k)),
        "the commit names only keys the uploads called unmatched"
    );
    assert_eq!(snapshot(h, &s), before, "nothing posted");
    // Retry: options can't change, so the files go in a new batch.
    for id in &sessions {
        h.run(s.imports().discard(*id))
            .expect("discard a failed batch's file");
    }
    // Two plain accounts of the company take every key, alternately.
    let targets: Vec<cl_core::ids::AccountId> = [
        (cl_api::AccountType::Expense, "Mapped costs"),
        (cl_api::AccountType::Revenue, "Mapped sales"),
    ]
    .into_iter()
    .map(|(t, n)| {
        h.run(
            s.accounts()
                .create(&crate::common::account_fields(t, n), IdempotencyKey::new()),
        )
        .unwrap_or_else(|e| panic!("account: {e}"))
        .record
        .id
    })
    .collect();
    let map = unmatched
        .iter()
        .enumerate()
        .map(|(i, k)| {
            (
                k.clone(),
                cl_api::imports::AccountTarget::Account(targets[i % 2]),
            )
        })
        .collect();
    let again = qbd_subset(
        h,
        &s,
        history,
        BatchOptions {
            account_map: map,
            ..BatchOptions::default()
        },
    )
    .expect("new batch");
    let st = commit(h, &s, again.id);
    assert!(
        st.batch
            .files
            .iter()
            .filter_map(|f| f.result.as_ref())
            .all(|r| r.created > 0),
        "every file posted: {:?}",
        st.batch.files
    );
}

/// E2b: inventory parts while the company's inventory is off are refused
/// before the job starts (the desktop turns inventory on first).
pub fn inventory_off_is_refused(h: &Harness) {
    if !caps(h, Flow::QbdConversion) {
        return;
    }
    let (s, _) = fresh_company(h, "Shoal Creek inventory off");
    let b = qbd_subset(
        h,
        &s,
        |f| matches!(f.kind, ImportKind::Coa | ImportKind::Items),
        BatchOptions::default(),
    )
    .expect("batch");
    let req = CommitRequest {
        backup_ref: unique("contract-before-import"),
        ..CommitRequest::default()
    };
    let e = h
        .run(s.imports().commit(b.id, &req, IdempotencyKey::new()))
        .expect_err("inventory off");
    assert_eq!(
        import_refusal(&e),
        Some(ImportRefusal::InventoryOff),
        "{e:?}"
    );
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

/// Review M3: a QBO workbook's raw number cells read exactly: a whole
/// number (an integer cell, no "."), and values of 1e11 and more (which a
/// spreadsheet shows in exponent form). The engine's sums are exact.
pub fn qbo_xlsx_raw_values_read_exactly(h: &Harness) {
    if !caps(h, Flow::OtherSystem) {
        return;
    }
    let (s, _) = fresh_company(h, "qbo-raw-values");
    let mut wb = rust_xlsxwriter::Workbook::new();
    let ws = wb.add_worksheet();
    for (r, title) in ["Bluebonnet Bakery", "Trial Balance", "As of June 30, 2026"]
        .iter()
        .enumerate()
    {
        ws.write_string(u32::try_from(r).expect("row"), 0, *title)
            .expect("title");
    }
    ws.write_string(3, 1, "Debit").expect("h");
    ws.write_string(3, 2, "Credit").expect("h");
    // (account, debit, credit): whole numbers as integer cells; the big
    // ones above 1e11. The only floats here are the workbook's own cells.
    let lines: [(&str, Option<f64>, Option<f64>); 4] = [
        ("Operating Cash", Some(12_500.0), None),
        ("Land", Some(123_456_789_012.34), None),
        ("Long-term Loan", None, Some(123_456_789_012.0)),
        ("Owner's Equity", None, Some(12_500.34)),
    ];
    for (i, (name, dr, cr)) in lines.iter().enumerate() {
        let r = u32::try_from(i + 4).expect("row");
        ws.write_string(r, 0, *name).expect("name");
        if let Some(d) = dr {
            ws.write_number(r, 1, *d).expect("debit");
        }
        if let Some(c) = cr {
            ws.write_number(r, 2, *c).expect("credit");
        }
    }
    let req = UploadRequest {
        kind: ImportKind::TrialBalance,
        source: SourceSystem::QuickBooksOnline,
        role: FileRole::Reference,
        bytes: wb.save_to_buffer().expect("xlsx"),
        file_name: "tb.xlsx".into(),
        contact_kind: None,
        tb_column: None,
        tb_report_date: None,
        update_existing_coa: false,
    };
    let out = h.run(s.imports().upload(&req)).expect("upload");
    assert!(out.is_clean(), "{:?}", out.problems);
    let want: Decimal = "123456801512.34".parse().expect("decimal");
    assert_eq!(
        out.preview.total_debit.map(cl_core::Money::as_decimal),
        Some(want)
    );
    assert_eq!(
        out.preview.total_credit.map(cl_core::Money::as_decimal),
        Some(want)
    );
    let _ = h.run(s.imports().discard(out.session.id));
}
