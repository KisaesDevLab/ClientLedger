//! Payroll import against the live engine (P30 plan §5): every provider
//! fixture's posted entries equal the independent `rust_decimal`
//! expectation (`support/payroll_expect.rs`), a generic layout is reused
//! for the second period, net-pay withdrawals match in the Bank Feed, and
//! the safety cases (duplicate file and run, overlap, concurrent posts, a
//! post that fails part way, the closing date, Reverse, no employee names
//! after posting, a restricted viewer).
//!
//! Every test except the restricted-viewer one makes its own company
//! (`fresh_company`, opened on the switcher session S3), so the file-hash
//! and run-key duplicate checks never trip on a rerun and a closing date
//! can't leak. The restricted viewer needs the shared Bookkeeper, who is a
//! member of Contract Co only; that test uses its own accounts there.
//!
//! PENDING until the pinned engine advertises `payroll-import-exact-v1`,
//! `payroll-import-post-v1` and `payroll-import-sensitive-v1`.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use cl_api::payroll::{
    ColumnMap, ColumnSource, DescriptionMapping, ImportMode, PayrollRefusal, PayrollSessionId,
    PostOutcome, PostRequest, SessionStatus, SkipRule, UploadRequest, payroll_refusal,
};
use cl_api::{
    AccountType, ApiError, AuditAction, AuditEntity, BankFileFormat, BankImportRequest,
    BankImportRow, Capabilities, ClosedPeriodAuthorization, IdempotencyKey, Method, NewCompany,
    Session, TagFields, TxnStatus,
};
use cl_core::ids::{AccountId, TagId, TransactionId};
use cl_core::{CalendarDate, Secret};
use rust_decimal::Decimal;
use serde_json::json;

use crate::common::{account_fields, key};
use crate::payroll_expect::{self, Side, Spec};
use crate::support::{Harness, unique};

fn caps(h: &Harness) -> bool {
    h.require_cap(Capabilities::PAYROLL_IMPORT_EXACT, "U-250")
        && h.require_cap(Capabilities::PAYROLL_IMPORT_POST, "U-244")
        && h.require_cap(Capabilities::PAYROLL_IMPORT_SENSITIVE, "U-247")
}

fn day(s: &str) -> CalendarDate {
    CalendarDate::parse_wire(s).expect("date")
}

/// A new company (its own tenant) opened on S3. Later `h.s3()` calls
/// reopen tenant B.
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

fn account(
    h: &Harness,
    s: &Session,
    t: AccountType,
    detail: Option<&str>,
    name: &str,
) -> AccountId {
    let mut f = account_fields(t, name);
    f.detail_type = detail.map(str::to_string);
    h.run(s.accounts().create(&f, key()))
        .expect("account")
        .record
        .id
}

/// The accounts a test posts to: net pay to a bank (Q-P30f), every debit
/// to wages, every other credit to one liability.
#[derive(Clone, Copy)]
struct Accts {
    bank: AccountId,
    wages: AccountId,
    owed: AccountId,
}

impl Accts {
    /// The account a line of the expectation lands on (Mode A by line
    /// type; Mode B by description, "net" = the bank).
    fn of(&self, line_type: &str, side: Side) -> AccountId {
        if line_type == "payroll_clearing" || line_type.to_ascii_lowercase().contains("net") {
            self.bank
        } else if side == Side::Debit {
            self.wages
        } else {
            self.owed
        }
    }
}

/// Accounts for every Mode A line type, saved as the company's mapping.
fn map_accounts(h: &Harness, s: &Session) -> Accts {
    let a = Accts {
        bank: account(h, s, AccountType::Asset, Some("bank"), "Payroll Bank"),
        wages: account(h, s, AccountType::Expense, None, "Payroll Wages"),
        owed: account(h, s, AccountType::Liability, None, "Payroll Taxes Owed"),
    };
    let mut m = BTreeMap::new();
    for (lt, debit) in line_types() {
        let side = if debit { Side::Debit } else { Side::Credit };
        m.insert(lt.to_string(), a.of(lt, side));
    }
    h.run(s.payroll().save_account_mappings(&m))
        .expect("account map");
    a
}

/// The engine's Mode A line types (cl-model is not a dependency of cl-api).
fn line_types() -> Vec<(&'static str, bool)> {
    vec![
        ("gross_wages_expense", true),
        ("employer_tax_expense", true),
        ("employer_benefits_expense", true),
        ("reimbursement_expense", true),
        ("fit_payable", false),
        ("sit_payable", false),
        ("ss_payable", false),
        ("medicare_payable", false),
        ("futa_payable", false),
        ("suta_payable", false),
        ("health_ins_payable", false),
        ("retirement_payable", false),
        ("other_deduction_payable", false),
        ("other_tax_payable", false),
        ("payroll_clearing", false),
    ]
}

/// The column map for a Mode A fixture, from the expectation's own spec.
fn column_map(stem: &str) -> Option<ColumnMap> {
    let Spec::A(s) = payroll_expect::spec(stem) else {
        return None;
    };
    let mut mappings = BTreeMap::new();
    let mut put = |f: &str, h: &str| {
        mappings.insert(
            f.to_string(),
            ColumnSource {
                source: h.to_string(),
                format: None,
            },
        );
    };
    put("employee_name", s.employee);
    if let Some(c) = s.check_date {
        put("check_date", c);
    }
    if let Some(d) = s.department {
        put("department", d);
    }
    for (f, h) in s.money {
        put(f, h);
    }
    let fmt = if stem.starts_with("generic") {
        "MM/DD/YYYY"
    } else {
        "YYYY-MM-DD"
    };
    Some(ColumnMap {
        header_row: u32::try_from(s.header_row).expect("row"),
        data_start_row: u32::try_from(s.header_row + 1).expect("row"),
        skip_footer_rows: u32::try_from(s.skip_footer).expect("row"),
        date_format: Some(fmt.into()),
        mappings,
        skip_rules: vec![
            SkipRule {
                field: "employee_name".into(),
                values: None,
            },
            SkipRule {
                field: "employee_name".into(),
                values: Some(vec!["Total".into()]),
            },
        ],
        department_tags: BTreeMap::new(),
        left_out: None,
    })
}

/// The worker names in a Mode A fixture (none in Mode B summaries).
fn worker_names(stem: &str) -> Vec<String> {
    let Spec::A(s) = payroll_expect::spec(stem) else {
        return Vec::new();
    };
    let rows = payroll_expect::read_csv(&payroll_expect::fixture_bytes(&format!("{stem}.csv")));
    let header = &rows[s.header_row];
    let c = header
        .iter()
        .position(|x| x.trim() == s.employee)
        .expect("employee column");
    rows[s.header_row + 1..rows.len() - s.skip_footer]
        .iter()
        .filter_map(|r| r.get(c))
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty() && n != "Total")
        .collect()
}

/// A prepared import: session, preview token, department map and the
/// expectation it was checked against.
struct Prepared {
    id: PayrollSessionId,
    token: Option<String>,
    departments: BTreeMap<String, Option<cl_core::ids::TagId>>,
    want: Vec<payroll_expect::Entry>,
}

impl Prepared {
    fn request(&self) -> PostRequest {
        PostRequest {
            preview_token: self.token.clone(),
            department_tags: self.departments.clone(),
            ..PostRequest::default()
        }
    }
}

/// Uploads `bytes` as `stem`, maps it (with `map` when given, else the
/// fixture's own map), validates and previews it; the preview must equal
/// the independent expectation.
fn prepare_with(
    h: &Harness,
    s: &Session,
    stem: &str,
    bytes: Vec<u8>,
    a: Accts,
    map: Option<&ColumnMap>,
) -> Prepared {
    let g = payroll_expect::golden(stem);
    let mode = if g.mode == "B" {
        ImportMode::PrebuiltJe
    } else {
        ImportMode::EmployeeLevel
    };
    let up = h
        .run(s.payroll().upload(&UploadRequest {
            mode,
            bytes,
            file_name: format!("{stem}.csv"),
            period_start: day(&g.period_start),
            period_end: day(&g.period_end),
            check_date: day(&g.check_date),
        }))
        .expect("upload");
    let want = payroll_expect::expect(stem, &g.check_date);
    let mut departments = BTreeMap::new();
    if let Some(map) = map.cloned().or_else(|| column_map(stem)) {
        h.run(s.payroll().apply_mapping(up.session, &map))
            .expect("mapping");
        if stem == "gusto" {
            departments.insert("Kitchen".to_string(), None);
            departments.insert("Front".to_string(), None);
        }
    } else {
        let sides: BTreeMap<String, Side> = want
            .iter()
            .flat_map(|e| e.lines.iter())
            .map(|l| (l.line_type.clone(), l.side))
            .collect();
        let descs: Vec<DescriptionMapping> = h
            .run(s.payroll().description_map(up.session))
            .expect("descriptions")
            .into_iter()
            .map(|d| {
                let side = sides.get(&d.description).copied().unwrap_or(Side::Credit);
                DescriptionMapping {
                    account: Some(a.of(&d.description, side)),
                    ..d
                }
            })
            .collect();
        h.run(
            s.payroll()
                .save_description_map(up.session, &g.provider, &descs),
        )
        .expect("description map");
    }
    let v = h.run(s.payroll().validate(up.session)).expect("validate");
    assert!(v.can_continue(), "{stem}: {v:?}");
    let p = h
        .run(s.payroll().preview(up.session, &departments))
        .expect("preview");
    assert_eq!(
        p.entries.len(),
        want.len(),
        "{stem}: one entry per pay date"
    );
    for (e, w) in p.entries.iter().zip(&want) {
        assert_eq!(e.date.to_wire(), w.date, "{stem}");
        assert!(e.is_balanced, "{stem}");
        assert_eq!(e.total_debits.as_decimal(), w.total_debits, "{stem}");
        let mut got: Vec<(String, Option<String>, Side, Decimal)> = e
            .lines
            .iter()
            .map(|l| {
                let (side, amt) = if l.debit.is_zero() {
                    (Side::Credit, l.credit)
                } else {
                    (Side::Debit, l.debit)
                };
                (
                    l.line_type.clone(),
                    l.department.clone(),
                    side,
                    amt.as_decimal(),
                )
            })
            .collect();
        let mut exp: Vec<(String, Option<String>, Side, Decimal)> = w
            .lines
            .iter()
            .map(|l| (l.line_type.clone(), l.department.clone(), l.side, l.amount))
            .collect();
        got.sort();
        exp.sort();
        assert_eq!(got, exp, "{stem} {}", w.date);
    }
    Prepared {
        id: up.session,
        token: p.token,
        departments,
        want,
    }
}

fn prepare(h: &Harness, s: &Session, stem: &str, a: Accts) -> Prepared {
    prepare_with(
        h,
        s,
        stem,
        payroll_expect::fixture_bytes(&format!("{stem}.csv")),
        a,
        None,
    )
}

/// Posted payroll entries dated `on` in the open company, read from the
/// ledger (`source = payroll_import`); fails closed on an unread list.
fn ledger_payroll_entries(h: &Harness, s: &Session, on: &str) -> usize {
    let r = h
        .run(s.raw(
            Method::Get,
            "/api/v1/transactions",
            &[
                ("source", "payroll_import"),
                ("status", "posted"),
                ("startDate", on),
                ("endDate", on),
            ],
            None,
        ))
        .expect("transactions");
    assert_eq!(r.status, 200, "{}", r.body);
    r.body["data"].as_array().expect("a transaction list").len()
}

fn posted(out: PostOutcome, what: &str) -> Vec<TransactionId> {
    match out {
        PostOutcome::Posted { entries, .. } => entries,
        other @ PostOutcome::NeedsConfirmation { .. } => panic!("{what}: {other:?}"),
    }
}

/// A1: the posted entries, read back from the ledger, equal the
/// expectation summed per (account, side); debits equal credits; no
/// worker name in a memo or line description.
fn check_posted(
    h: &Harness,
    s: &Session,
    stem: &str,
    a: Accts,
    want: &[payroll_expect::Entry],
    entries: &[TransactionId],
) {
    assert_eq!(entries.len(), want.len(), "{stem}: entries");
    let names = worker_names(stem);
    for id in entries {
        let t = h.run(s.transactions().get(*id)).expect("entry").record;
        assert_eq!(t.status, TxnStatus::Posted, "{stem}");
        let w = want
            .iter()
            .find(|w| w.date == t.txn_date.to_wire())
            .unwrap_or_else(|| panic!("{stem}: no expected entry on {}", t.txn_date.to_wire()));
        let mut exp: BTreeMap<(AccountId, Side), Decimal> = BTreeMap::new();
        for l in &w.lines {
            *exp.entry((a.of(&l.line_type, l.side), l.side)).or_default() += l.amount;
        }
        let mut got: BTreeMap<(AccountId, Side), Decimal> = BTreeMap::new();
        let (mut dr, mut cr) = (Decimal::ZERO, Decimal::ZERO);
        for l in &t.lines {
            dr += l.debit.as_decimal();
            cr += l.credit.as_decimal();
            if !l.debit.is_zero() {
                *got.entry((l.account_id, Side::Debit)).or_default() += l.debit.as_decimal();
            }
            if !l.credit.is_zero() {
                *got.entry((l.account_id, Side::Credit)).or_default() += l.credit.as_decimal();
            }
            let text = l.description.clone().unwrap_or_default();
            assert!(
                !names.iter().any(|n| text.contains(n.as_str())),
                "{stem}: a name in a line"
            );
        }
        assert_eq!(dr, cr, "{stem} {}: balanced", w.date);
        assert_eq!(dr, w.total_debits, "{stem} {}", w.date);
        assert_eq!(got, exp, "{stem} {}: lines by account", w.date);
        let memo = t.memo.unwrap_or_default();
        assert!(
            !names.iter().any(|n| memo.contains(n.as_str())),
            "{stem}: a name in the memo"
        );
    }
}

/// After posting, no payroll route answers with a worker name (U-251).
fn no_names_left(h: &Harness, s: &Session, stem: &str, id: PayrollSessionId) {
    let names = worker_names(stem);
    let after = h.run(s.payroll().session(id)).expect("session");
    assert!(after.detail_discarded, "{stem}: detail discarded");
    for route in [
        format!("/api/v1/payroll-import/sessions/{id}"),
        format!("/api/v1/payroll-import/sessions/{id}/preview"),
        "/api/v1/payroll-import/sessions".to_string(),
    ] {
        let r = h.run(s.raw(Method::Get, &route, &[], None)).expect("raw");
        assert_eq!(r.status, 200, "{route}: {}", r.body);
        let text = r.body.to_string();
        for n in &names {
            assert!(!text.contains(n.as_str()), "{stem}: {route} names a worker");
        }
    }
    let r = h
        .run(s.raw(
            Method::Get,
            &format!("/api/v1/payroll-import/sessions/{id}/preview"),
            &[],
            None,
        ))
        .expect("raw");
    assert_eq!(r.body["detailDiscarded"], json!(true), "{stem}");
}

/// A1: every provider fixture, each in a fresh company, posts balanced
/// entries equal to the expectation computed from its CSV.
pub fn providers_post_their_expected_entries(h: &Harness) {
    if !caps(h) {
        return;
    }
    let mut failed = Vec::new();
    for stem in [
        "gusto",
        "adp_run_gli",
        "quickbooks_payroll",
        "paychex_flex_gls",
        "square",
        "onpay_gl_summary",
        "generic_period1",
    ] {
        // Every provider runs, so one failure doesn't hide another's.
        let one = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let s = fresh_company(h, &format!("Payroll {stem}"));
            let a = map_accounts(h, &s);
            // U-252 (engine PR #70, merged): a Total row dropped at
            // apply-mapping is no longer validated, so QuickBooks runs in full.
            let p = prepare(h, &s, stem, a);
            let entries = posted(
                h.run(s.payroll().post(p.id, &p.request(), IdempotencyKey::new()))
                    .expect("post"),
                stem,
            );
            check_posted(h, &s, stem, a, &p.want, &entries);
            no_names_left(h, &s, stem, p.id);
        }));
        if one.is_err() {
            failed.push(stem);
        }
    }
    assert!(failed.is_empty(), "providers that failed: {failed:?}");
}

/// A2: the generic layout saved for period 1 maps period 2 with no
/// mapping step: the saved template whose columns are all in the new
/// file's headers is applied as stored.
pub fn generic_layout_is_reused(h: &Harness) {
    if !caps(h) {
        return;
    }
    let s = fresh_company(h, "Payroll generic");
    let a = map_accounts(h, &s);
    let map = column_map("generic_period1").expect("map");
    let t = h
        .run(
            s.payroll()
                .save_template(None, "Contract generic", "custom", &map),
        )
        .expect("template");
    let p1 = prepare(h, &s, "generic_period1", a);
    let e1 = posted(
        h.run(
            s.payroll()
                .post(p1.id, &p1.request(), IdempotencyKey::new()),
        )
        .expect("post 1"),
        "period 1",
    );
    check_posted(h, &s, "generic_period1", a, &p1.want, &e1);

    // Period 2: pick the template by the upload's headers (what
    // `cl_model::payroll_import::template_for_headers` does) and apply it.
    let bytes = payroll_expect::fixture_bytes("generic_period2.csv");
    let headers: Vec<String> = payroll_expect::read_csv(&bytes)[0]
        .iter()
        .map(|x| x.trim().to_string())
        .collect();
    let templates = h.run(s.payroll().templates()).expect("templates");
    let saved = templates
        .iter()
        .filter(|x| !x.is_system)
        .find_map(|x| {
            let m = x.column_map.clone()?;
            m.sources()
                .iter()
                .all(|c| headers.contains(c))
                .then_some((x.id, m))
        })
        .expect("a saved layout matches period 2");
    assert_eq!(saved.0, t.id);
    let p2 = prepare_with(h, &s, "generic_period2", bytes, a, Some(&saved.1));
    let e2 = posted(
        h.run(
            s.payroll()
                .post(p2.id, &p2.request(), IdempotencyKey::new()),
        )
        .expect("post 2"),
        "period 2",
    );
    check_posted(h, &s, "generic_period2", a, &p2.want, &e2);
}

/// A bank-file row in the feed sign (positive = money out).
fn feed_row(line: u32, on: &str, out: Decimal, text: &str) -> BankImportRow {
    BankImportRow {
        line,
        date: day(on),
        amount: out,
        description: text.into(),
        memo: None,
        check_number: None,
        fitid: None,
        include: None,
        changed_action: None,
    }
}

fn bank_file(account: AccountId, rows: Vec<BankImportRow>) -> BankImportRequest {
    BankImportRequest {
        account,
        format: BankFileFormat::Csv,
        file_name: "bank.csv".into(),
        source_file_sha256: "cd".repeat(32),
        section_index: 0,
        section_start: rows.iter().map(|r| r.date).min(),
        section_end: rows.iter().map(|r| r.date).max(),
        amounts_flipped: true,
        csv_mapping: None,
        relink_identity: false,
        bank_identity: None,
        file_ending_balance: None,
        file_ending_date: None,
        rows,
    }
}

/// `fixtures/payroll/bank_payroll.csv` as feed rows (its withdrawals are
/// negative; the feed's money out is positive).
fn bank_payroll_rows() -> Vec<(String, Decimal, String)> {
    let rows = payroll_expect::read_csv(&payroll_expect::fixture_bytes("bank_payroll.csv"));
    rows[1..]
        .iter()
        .map(|r| {
            let amount: Decimal = r[2].trim().parse().expect("amount");
            (r[0].trim().to_string(), -amount, r[1].trim().to_string())
        })
        .collect()
}

/// A3: Gusto (two pay dates) and ADP posted with net pay to the bank;
/// `bank_payroll.csv` imported into that bank: each net-pay withdrawal
/// has exactly one payroll line of the same amount and matches it; the
/// tax debit has none.
pub fn bank_withdrawals_match(h: &Harness) {
    if !caps(h) {
        return;
    }
    let s = fresh_company(h, "Payroll bank match");
    let a = map_accounts(h, &s);
    for stem in ["gusto", "adp_run_gli"] {
        let p = prepare(h, &s, stem, a);
        posted(
            h.run(s.payroll().post(p.id, &p.request(), IdempotencyKey::new()))
                .expect("post"),
            stem,
        );
    }
    let file = bank_payroll_rows();
    let rows = file
        .iter()
        .zip(2u32..)
        .map(|((on, out, text), line)| feed_row(line, on, *out, text))
        .collect();
    let done = h
        .run(s.bank_import().commit(&bank_file(a.bank, rows), key()))
        .expect("bank import");
    assert_eq!(done.inserted_items.len(), file.len());
    let mut matched = 0;
    for (item, (_, out, text)) in done.inserted_items.iter().zip(&file) {
        let found = h.run(s.feed().payroll_overlap(*item)).expect("overlap");
        if text.contains("TAX") {
            assert!(
                found.is_empty(),
                "the tax debit has no payroll match: {found:?}"
            );
            continue;
        }
        assert_eq!(found.len(), 1, "{text}: exactly one payroll line");
        assert_eq!(found[0].amount.as_decimal().abs(), *out, "{text}");
        h.run(
            s.feed()
                .match_line(*item, found[0].transaction, Some(found[0].line), key()),
        )
        .expect("match");
        matched += 1;
    }
    assert_eq!(matched, 3, "three net-pay withdrawals");
}

/// Safety: same file → 409 `PAYROLL_DUPLICATE_FILE`; same run in other
/// bytes → 409 `PAYROLL_DUPLICATE_RUN`; an overlapping period →
/// `requiresConfirmation`, then `forcePost` with a new key posts; two
/// concurrent posts → one set of entries; a repeated key replays;
/// Reverse voids every entry; reverse twice → 409 `PAYROLL_NOT_POSTED`.
pub fn duplicates_overlap_concurrency_and_reverse(h: &Harness) {
    if !caps(h) {
        return;
    }
    let s = fresh_company(h, "Payroll safety");
    let a = map_accounts(h, &s);
    let first = prepare(h, &s, "square", a);
    let k = IdempotencyKey::new();
    let square = posted(
        h.run(s.payroll().post(first.id, &first.request(), k.clone()))
            .expect("post"),
        "square",
    );
    // The same key replays the answer and posts nothing more.
    let again = h
        .run(s.payroll().post(first.id, &first.request(), k))
        .expect("replay");
    assert!(
        matches!(&again, PostOutcome::Posted { entries, replayed: true } if *entries == square),
        "{again:?}"
    );

    // The same file again.
    let dup = prepare(h, &s, "square", a);
    let mut req = dup.request();
    req.force_post = true;
    let e = h
        .run(s.payroll().post(dup.id, &req, IdempotencyKey::new()))
        .expect_err("same file");
    // Square has a run key too, and the engine checks the run first.
    assert!(
        matches!(
            payroll_refusal(&e),
            Some(PayrollRefusal::DuplicateFile { prior, .. } | PayrollRefusal::DuplicateRun { prior, .. })
                if prior == Some(first.id)
        ),
        "{e:?}"
    );
    h.run(s.payroll().discard(dup.id)).expect("discard");

    // The same run in other bytes (a trailing blank line).
    let mut bytes = payroll_expect::fixture_bytes("square.csv");
    bytes.extend_from_slice(b"\n");
    let run = prepare_with(h, &s, "square", bytes, a, None);
    let mut req = run.request();
    req.force_post = true;
    let e = h
        .run(s.payroll().post(run.id, &req, IdempotencyKey::new()))
        .expect_err("same run");
    assert!(
        matches!(
            payroll_refusal(&e),
            Some(PayrollRefusal::DuplicateRun { .. })
        ),
        "{e:?}"
    );
    h.run(s.payroll().discard(run.id)).expect("discard");

    // Overlap: generic period 2 uploaded for Square's pay period.
    let g = payroll_expect::golden("square");
    let up = h
        .run(s.payroll().upload(&UploadRequest {
            mode: ImportMode::EmployeeLevel,
            bytes: payroll_expect::fixture_bytes("generic_period2.csv"),
            file_name: "generic_period2.csv".into(),
            period_start: day(&g.period_start),
            period_end: day(&g.period_end),
            check_date: day(&g.check_date),
        }))
        .expect("upload");
    h.run(
        s.payroll()
            .apply_mapping(up.session, &column_map("generic_period2").expect("map")),
    )
    .expect("mapping");
    assert!(
        h.run(s.payroll().validate(up.session))
            .expect("validate")
            .can_continue()
    );
    let pv = h
        .run(s.payroll().preview(up.session, &BTreeMap::new()))
        .expect("preview");
    let req = PostRequest {
        preview_token: pv.token.clone(),
        ..PostRequest::default()
    };
    match h
        .run(s.payroll().post(up.session, &req, IdempotencyKey::new()))
        .expect("overlap prompt")
    {
        PostOutcome::NeedsConfirmation { overlaps } => {
            assert!(
                overlaps.iter().any(|o| o.session == Some(first.id)),
                "{overlaps:?}"
            );
        }
        other @ PostOutcome::Posted { .. } => panic!("expected the overlap prompt: {other:?}"),
    }
    let forced = PostRequest {
        force_post: true,
        ..req
    };
    posted(
        h.run(s.payroll().post(up.session, &forced, IdempotencyKey::new()))
            .expect("post anyway"),
        "forcePost",
    );

    // Two posts of one import at once: one posts, the other is refused.
    let c = prepare(h, &s, "generic_period1", a);
    let mut req = c.request();
    req.force_post = true;
    let (p1, p2) = (s.payroll(), s.payroll());
    let (r1, r2) = h.run(async {
        tokio::join!(
            p1.post(c.id, &req, IdempotencyKey::new()),
            p2.post(c.id, &req, IdempotencyKey::new())
        )
    });
    let oks = [&r1, &r2]
        .iter()
        .filter(|r| matches!(r, Ok(PostOutcome::Posted { .. })))
        .count();
    assert_eq!(oks, 1, "{r1:?} {r2:?}");
    let refused = [r1, r2]
        .into_iter()
        .find_map(Result::err)
        .expect("one refused");
    assert_eq!(
        payroll_refusal(&refused),
        Some(PayrollRefusal::AlreadyPosted),
        "{refused:?}"
    );
    let sess = h.run(s.payroll().session(c.id)).expect("session");
    assert_eq!(sess.entries.len(), 1, "one set of entries");
    assert_eq!(
        ledger_payroll_entries(h, &s, &c.want[0].date),
        1,
        "the ledger holds one entry"
    );
    // The same generic file again (no provider run key): the file hash.
    let dup = prepare(h, &s, "generic_period1", a);
    let mut req = dup.request();
    req.force_post = true;
    let e = h
        .run(s.payroll().post(dup.id, &req, IdempotencyKey::new()))
        .expect_err("same file");
    assert!(
        matches!(payroll_refusal(&e), Some(PayrollRefusal::DuplicateFile { prior, .. }) if prior == Some(c.id)),
        "{e:?}"
    );
    h.run(s.payroll().discard(dup.id)).expect("discard");

    // Reverse Square: every entry is void; the same key replays; a new
    // key is refused.
    let k = IdempotencyKey::new();
    let r = h
        .run(
            s.payroll()
                .reverse(first.id, "Contract test", None, k.clone()),
        )
        .expect("reverse");
    assert_eq!(usize::try_from(r.reversed).ok(), Some(square.len()));
    for id in &square {
        let t = h.run(s.transactions().get(*id)).expect("entry").record;
        assert_eq!(t.status, TxnStatus::Void, "every entry voided");
    }
    let replay = h
        .run(s.payroll().reverse(first.id, "Contract test", None, k))
        .expect("replay");
    assert_eq!(replay.entries, r.entries, "the same key replays");
    let e = h
        .run(
            s.payroll()
                .reverse(first.id, "Contract test", None, IdempotencyKey::new()),
        )
        .expect_err("twice");
    assert_eq!(payroll_refusal(&e), Some(PayrollRefusal::NotPosted));
    let after = h.run(s.payroll().session(first.id)).expect("session");
    assert_eq!(after.status, SessionStatus::Cancelled);
}

/// The engine's P30 review fixes (`payroll-import-review-v1`): pending
/// until the pin has them.
fn review_caps(h: &Harness) -> bool {
    caps(h)
        && h.require_cap(
            Capabilities::PAYROLL_IMPORT_REVIEW,
            "P30 review B1/S1/S3/S5",
        )
}

/// Engine S1: two drafts of the same file posted at once — exactly one
/// posts; the other is refused as a duplicate; the ledger holds one set.
pub fn two_drafts_of_one_file_post_once(h: &Harness) {
    if !review_caps(h) {
        return;
    }
    let s = fresh_company(h, "Payroll race");
    let a = map_accounts(h, &s);
    let one = prepare(h, &s, "generic_period1", a);
    let two = prepare(h, &s, "generic_period1", a);
    let (mut r1, mut r2) = (one.request(), two.request());
    r1.force_post = true;
    r2.force_post = true;
    let (p1, p2) = (s.payroll(), s.payroll());
    let (x, y) = h.run(async {
        tokio::join!(
            p1.post(one.id, &r1, IdempotencyKey::new()),
            p2.post(two.id, &r2, IdempotencyKey::new())
        )
    });
    let oks = [&x, &y]
        .iter()
        .filter(|r| matches!(r, Ok(PostOutcome::Posted { .. })))
        .count();
    assert_eq!(oks, 1, "{x:?} {y:?}");
    let refused = [x, y]
        .into_iter()
        .find_map(Result::err)
        .expect("one refused");
    assert!(
        matches!(
            payroll_refusal(&refused),
            Some(PayrollRefusal::DuplicateFile { .. } | PayrollRefusal::DuplicateRun { .. })
        ),
        "{refused:?}"
    );
    assert_eq!(ledger_payroll_entries(h, &s, &one.want[0].date), 1);
}

/// Engine B1: a discard racing a post never removes the posted import —
/// afterwards the import is posted, listed, and reversible.
pub fn a_discard_racing_a_post_keeps_the_posted_import(h: &Harness) {
    if !review_caps(h) {
        return;
    }
    let s = fresh_company(h, "Payroll discard race");
    let a = map_accounts(h, &s);
    let p = prepare(h, &s, "generic_period1", a);
    let mut req = p.request();
    req.force_post = true;
    let (p1, p2) = (s.payroll(), s.payroll());
    let (posted_r, discard_r) =
        h.run(async { tokio::join!(p1.post(p.id, &req, IdempotencyKey::new()), p2.discard(p.id)) });
    if let Ok(PostOutcome::Posted { entries, .. }) = &posted_r {
        let sess = h
            .run(s.payroll().session(p.id))
            .expect("the import survives");
        assert_eq!(sess.status, SessionStatus::Posted, "{discard_r:?}");
        assert_eq!(&sess.entries, entries);
        let listed = h
            .run(s.payroll().sessions_all(None))
            .expect("history")
            .sessions
            .iter()
            .any(|x| x.id == p.id);
        assert!(listed, "History still lists it");
        h.run(
            s.payroll()
                .reverse(p.id, "Contract test", None, IdempotencyKey::new()),
        )
        .expect("still reversible");
    } else {
        // The discard won: nothing posted, nothing left behind.
        assert!(discard_r.is_ok(), "{posted_r:?} {discard_r:?}");
        assert_eq!(ledger_payroll_entries(h, &s, &p.want[0].date), 0);
    }
}

/// U-244: a post whose second entry is refused by the ledger (all-zero
/// amounts on the second pay date) leaves nothing posted.
pub fn a_failed_post_leaves_nothing(h: &Harness) {
    if !caps(h) {
        return;
    }
    let s = fresh_company(h, "Payroll torn");
    map_accounts(h, &s);
    let csv = "Worker,Paid On,Wages,Fed WH,State WH,SS EE,Medicare EE,SS ER,Medicare ER,Take Home\n\
               Test Worker 91,08/01/2025,\"$1,000.00\",100.00,30.00,62.00,14.50,62.00,14.50,793.50\n\
               Test Worker 92,08/15/2025,0.00,0.00,0.00,0.00,0.00,0.00,0.00,0.00\n";
    let up = h
        .run(s.payroll().upload(&UploadRequest {
            mode: ImportMode::EmployeeLevel,
            bytes: csv.as_bytes().to_vec(),
            file_name: "torn.csv".into(),
            period_start: day("2025-07-26"),
            period_end: day("2025-08-15"),
            check_date: day("2025-08-15"),
        }))
        .expect("upload");
    h.run(
        s.payroll()
            .apply_mapping(up.session, &column_map("generic_period1").expect("map")),
    )
    .expect("mapping");
    let v = h.run(s.payroll().validate(up.session)).expect("validate");
    assert!(v.can_continue(), "{v:?}");
    let pv = h
        .run(s.payroll().preview(up.session, &BTreeMap::new()))
        .expect("preview");
    assert_eq!(
        pv.entries.len(),
        2,
        "the engine previews the all-zero date too: {pv:?}"
    );
    let req = PostRequest {
        preview_token: pv.token.clone(),
        ..PostRequest::default()
    };
    let e = h
        .run(s.payroll().post(up.session, &req, IdempotencyKey::new()))
        .expect_err("the second entry is refused");
    // The ledger's refusal of entry 2 (400), not one of the up-front
    // payroll refusals and not a lost answer.
    assert_eq!(payroll_refusal(&e), None, "{e:?}");
    assert!(
        !matches!(
            e,
            ApiError::OutcomeUnknown { .. } | ApiError::ClosedPeriod(_) | ApiError::Conflict { .. }
        ),
        "{e:?}"
    );
    let sess = h.run(s.payroll().session(up.session)).expect("session");
    assert!(sess.entries.is_empty(), "no entry recorded on the import");
    assert_ne!(sess.status, SessionStatus::Posted);
    assert_eq!(
        ledger_payroll_entries(h, &s, "2025-08-01"),
        0,
        "nothing posted for the first pay date"
    );
    let all = h
        .run(s.raw(Method::Get, "/api/v1/transactions", &[], None))
        .expect("transactions");
    assert_eq!(
        all.body["data"].as_array().map(Vec::len),
        Some(0),
        "the fresh company has no entry at all"
    );
}

/// U-245: a closed period asks for the password; a wrong one is refused;
/// the right one posts and audits the exception; Reverse asks again.
pub fn closed_period_needs_the_password(h: &Harness) {
    if !caps(h) {
        return;
    }
    let s = fresh_company(h, "Payroll closed");
    let a = map_accounts(h, &s);
    let p = prepare(h, &s, "gusto", a);
    h.run(s.raw_mapped(
        Method::Put,
        "/api/v1/company/settings",
        Some(&json!({"lockDate": "2025-01-31"})),
    ))
    .expect("closing date");
    let pw = Secret::new("payroll-closing-1");
    assert!(
        h.run(s.set_closing_password(None, Some(&pw), key()))
            .expect("password")
    );
    let with = |p: &str| Some(ClosedPeriodAuthorization::with_password(Secret::new(p)));
    let e = h
        .run(s.payroll().post(p.id, &p.request(), IdempotencyKey::new()))
        .expect_err("closed");
    assert!(
        matches!(&e, ApiError::ClosedPeriod(c) if c.password_required == Some(true)),
        "{e:?}"
    );
    let mut req = p.request();
    req.closed_period = with("not-it");
    let e = h
        .run(s.payroll().post(p.id, &req, IdempotencyKey::new()))
        .expect_err("wrong password");
    assert_eq!(e, ApiError::ClosingPasswordRejected);
    req.closed_period = with("payroll-closing-1");
    let entries = posted(
        h.run(s.payroll().post(p.id, &req, IdempotencyKey::new()))
            .expect("right password"),
        "closed post",
    );
    check_posted(h, &s, "gusto", a, &p.want, &entries);
    assert_eq!(entries.len(), 2, "two pay dates, one password");
    let overrides = |id: &TransactionId| {
        h.run(
            s.audit()
                .for_record(AuditEntity::ClosingDateOverride, *id.as_uuid()),
        )
        .expect("audit")
        .iter()
        .filter(|r| r.action == AuditAction::Override)
        .count()
    };
    let after_post: Vec<usize> = entries.iter().map(overrides).collect();
    for n in &after_post {
        assert!(*n >= 1, "the closing-date exception is audited");
    }
    let e = h
        .run(
            s.payroll()
                .reverse(p.id, "Contract test", None, IdempotencyKey::new()),
        )
        .expect_err("reverse closed");
    assert!(matches!(&e, ApiError::ClosedPeriod(_)), "{e:?}");
    let r = h
        .run(s.payroll().reverse(
            p.id,
            "Contract test",
            with("payroll-closing-1"),
            IdempotencyKey::new(),
        ))
        .expect("reverse with the password");
    assert_eq!(usize::try_from(r.reversed).ok(), Some(entries.len()));
    for (id, before) in entries.iter().zip(&after_post) {
        let t = h.run(s.transactions().get(*id)).expect("entry").record;
        assert_eq!(t.status, TxnStatus::Void, "every entry voided");
        // S7: the reverse in the closed period is audited too.
        assert!(
            overrides(id) > *before,
            "the reverse's closing-date exception is audited"
        );
    }
}

/// U-247: with an account hidden from the Bookkeeper, every payroll route
/// refuses them (403 `SENSITIVE_HIDDEN`) and the Bank Feed gives them no
/// payroll match (the owner still gets one), and matching it is refused.
pub fn restricted_viewer_sees_no_payroll(h: &Harness) {
    if !caps(h) || !h.require_cap(Capabilities::ACCOUNT_SENSITIVE, "U-33") {
        return;
    }
    let owner = &h.s1;
    let a = map_accounts(h, owner);
    let stem = "onpay_gl_summary";
    let p = prepare(h, owner, stem, a);
    posted(
        h.run(
            owner
                .payroll()
                .post(p.id, &p.request(), IdempotencyKey::new()),
        )
        .expect("post"),
        stem,
    );
    let net = p.want[0]
        .lines
        .iter()
        .find(|l| l.line_type.to_ascii_lowercase().contains("net"))
        .expect("net pay")
        .amount;
    let done = h
        .run(owner.bank_import().commit(
            &bank_file(
                a.bank,
                vec![feed_row(2, &p.want[0].date, net, &unique("PAYROLL NET"))],
            ),
            key(),
        ))
        .expect("bank import");
    let item = done.inserted_items[0];
    assert_eq!(
        h.run(owner.feed().payroll_overlap(item))
            .expect("owner")
            .len(),
        1,
        "the owner sees the payroll line"
    );

    let bk = crate::contract_sensitive::as_restricted_bookkeeper(h);
    h.run(owner.accounts().set_sensitive(a.wages, true, key()))
        .expect("mark");
    let _unmark = crate::contract_sensitive::Unmark { h, id: a.wages };
    for (m, route) in [
        (Method::Get, "/api/v1/payroll-import/sessions".to_string()),
        (
            Method::Get,
            format!("/api/v1/payroll-import/sessions/{}", p.id),
        ),
        (Method::Get, "/api/v1/payroll-import/templates".to_string()),
        (
            Method::Post,
            format!("/api/v1/payroll-import/sessions/{}/reverse", p.id),
        ),
    ] {
        let body = (m == Method::Post).then(|| json!({"reason": "x"}));
        let r = h.run(bk.raw(m, &route, &[], body.as_ref())).expect("raw");
        assert_eq!(r.status, 403, "{route}: {}", r.body);
        assert_eq!(r.body["error"]["code"], "SENSITIVE_HIDDEN", "{route}");
    }
    let seen = h.run(bk.feed().payroll_overlap(item)).expect("bookkeeper");
    assert!(
        seen.is_empty(),
        "no payroll match for a restricted viewer: {seen:?}"
    );
    let line = h.run(owner.feed().payroll_overlap(item)).expect("owner")[0].clone();
    let e = h
        .run(
            bk.feed()
                .match_line(item, line.transaction, Some(line.line), key()),
        )
        .expect_err("match refused");
    assert!(
        matches!(e, ApiError::Forbidden { .. } | ApiError::Sensitive { .. }),
        "{e:?}"
    );
}

/// A tag in the open company.
fn new_tag(h: &Harness, s: &Session, name: &str) -> TagId {
    h.run(s.tags().create(
        &TagFields {
            name: unique(name),
            color: None,
            group_id: None,
            description: None,
            tag_number: None,
        },
        key(),
    ))
    .expect("tag")
    .record
    .id
}

/// Posted lines summed by (account, side, tag).
type Tagged = BTreeMap<(AccountId, Side, Option<TagId>), Decimal>;

fn posted_by_tag(h: &Harness, s: &Session, id: TransactionId) -> (String, Tagged) {
    let t = h.run(s.transactions().get(id)).expect("entry").record;
    let mut got = Tagged::new();
    for l in &t.lines {
        let (side, amt) = if l.debit.is_zero() {
            (Side::Credit, l.credit)
        } else {
            (Side::Debit, l.debit)
        };
        *got.entry((l.account_id, side, l.tag_id)).or_default() += amt.as_decimal();
    }
    (t.txn_date.to_wire(), got)
}

/// Review S6: Gusto's departments mapped to two tags. Every split expense
/// line carries its department's tag (the blank department none), every
/// liability is untagged, and net pay is exactly one untagged line per
/// pay date — in the preview and in the posted entries.
pub fn department_tags_reach_the_posted_lines(h: &Harness) {
    if !review_caps(h) {
        return;
    }
    let s = fresh_company(h, "Payroll department tags");
    let a = map_accounts(h, &s);
    let kitchen = new_tag(h, &s, "Kitchen");
    let front = new_tag(h, &s, "Front");
    let tags: BTreeMap<String, Option<TagId>> = [
        ("Kitchen".to_string(), Some(kitchen)),
        ("Front".to_string(), Some(front)),
    ]
    .into();
    let mut p = prepare(h, &s, "gusto", a);
    p.departments = tags.clone();
    let tag_of = |d: &Option<String>| d.as_ref().and_then(|d| tags.get(d).copied().flatten());
    // The preview stamps the same tags.
    let pv = h
        .run(s.payroll().preview(p.id, &tags))
        .expect("preview with tags");
    p.token.clone_from(&pv.token);
    for e in &pv.entries {
        for l in &e.lines {
            let want = if l.debit.is_zero() {
                None
            } else {
                tag_of(&l.department)
            };
            assert_eq!(l.tag, want, "preview {l:?}");
        }
    }
    let entries = posted(
        h.run(s.payroll().post(p.id, &p.request(), IdempotencyKey::new()))
            .expect("post"),
        "gusto tagged",
    );
    assert_eq!(entries.len(), p.want.len());
    let mut split_lines = 0;
    for id in &entries {
        let (date, got) = posted_by_tag(h, &s, *id);
        let w = p
            .want
            .iter()
            .find(|w| w.date == date)
            .expect("expected date");
        let mut exp = Tagged::new();
        for l in &w.lines {
            let tag = if l.side == Side::Debit {
                tag_of(&l.department)
            } else {
                None
            };
            split_lines += usize::from(tag.is_some());
            *exp.entry((a.of(&l.line_type, l.side), l.side, tag))
                .or_default() += l.amount;
        }
        assert_eq!(got, exp, "{date}: lines by account, side and tag");
        let t = h.run(s.transactions().get(*id)).expect("entry").record;
        let net: Vec<_> = t.lines.iter().filter(|l| l.account_id == a.bank).collect();
        assert_eq!(net.len(), 1, "{date}: one net-pay line");
        assert_eq!(net[0].tag_id, None, "{date}: net pay is untagged");
        assert_eq!(
            Some(net[0].credit.as_decimal()),
            w.net_pay(),
            "{date}: net pay"
        );
    }
    assert!(split_lines >= 2, "both tags were used");
}

/// Review S5: a Mode B file with a correction pair (Debit -50.05, Credit
/// -50.05) posts each row on the other side, equal line by line to the
/// independent expectation; the same file with an undated entry row is
/// refused with `PAYROLL_DATE_INVALID` naming that row and posts nothing.
pub fn mode_b_negatives_post_on_the_other_side(h: &Harness) {
    if !review_caps(h) {
        return;
    }
    let stem = "paychex_correction";
    let s = fresh_company(h, "Payroll Mode B correction");
    let want = payroll_expect::expect(stem, "2025-08-22");
    assert_eq!(want.len(), 1);
    // One account per description, so posted lines compare one to one.
    let mut acct: BTreeMap<String, AccountId> = BTreeMap::new();
    for l in &want[0].lines {
        let (t, detail) = if l.line_type.contains("Net") {
            (AccountType::Asset, Some("bank"))
        } else if l.line_type.contains("Wage") || l.line_type == "OASDI/ER" {
            (AccountType::Expense, None)
        } else {
            (AccountType::Liability, None)
        };
        acct.insert(
            l.line_type.clone(),
            account(h, &s, t, detail, &format!("Payroll {}", l.line_type)),
        );
    }
    let upload = |bytes: Vec<u8>| {
        let up = h
            .run(s.payroll().upload(&UploadRequest {
                mode: ImportMode::PrebuiltJe,
                bytes,
                file_name: format!("{stem}.csv"),
                period_start: day("2025-08-09"),
                period_end: day("2025-08-22"),
                check_date: day("2025-08-22"),
            }))
            .expect("upload");
        let descs: Vec<DescriptionMapping> = h
            .run(s.payroll().description_map(up.session))
            .expect("descriptions")
            .into_iter()
            .map(|d| DescriptionMapping {
                account: acct.get(&d.description).copied(),
                ..d
            })
            .collect();
        assert_eq!(descs.len(), acct.len(), "every description is listed");
        h.run(
            s.payroll()
                .save_description_map(up.session, "paychex_flex_gls", &descs),
        )
        .expect("description map");
        up.session
    };

    // The undated row first: nothing may post from it.
    let mut undated = payroll_expect::fixture_bytes(&format!("{stem}.csv"));
    undated.extend_from_slice(b",2120,Fed W/H,0.00,10.00\n");
    let bad = upload(undated);
    let v = h.run(s.payroll().validate(bad)).expect("validate");
    assert!(!v.can_continue(), "{v:?}");
    assert!(
        v.messages
            .iter()
            .any(|m| m.code == "DATE_INVALID" && m.row_number == Some(9) && m.is_error),
        "{v:?}"
    );
    let e = h
        .run(s.payroll().preview(bad, &BTreeMap::new()))
        .expect_err("undated preview");
    assert_eq!(
        payroll_refusal(&e),
        Some(PayrollRefusal::DateInvalid { rows: vec![9] }),
        "{e:?}"
    );
    let req = PostRequest {
        force_post: true,
        ..PostRequest::default()
    };
    let e = h
        .run(s.payroll().post(bad, &req, IdempotencyKey::new()))
        .expect_err("undated post");
    assert_eq!(
        payroll_refusal(&e),
        Some(PayrollRefusal::DateInvalid { rows: vec![9] }),
        "{e:?}"
    );
    assert_eq!(ledger_payroll_entries(h, &s, "2025-08-22"), 0);
    h.run(s.payroll().discard(bad)).expect("discard");
    // A post after the discard answers 404 (what the screen reads as
    // "discarded; nothing was posted").
    let e = h
        .run(s.payroll().post(bad, &req, IdempotencyKey::new()))
        .expect_err("post after discard");
    assert!(matches!(e, ApiError::NotFound { .. }), "{e:?}");

    // The correction pair.
    let id = upload(payroll_expect::fixture_bytes(&format!("{stem}.csv")));
    let v = h.run(s.payroll().validate(id)).expect("validate");
    assert!(v.can_continue(), "{v:?}");
    let pv = h
        .run(s.payroll().preview(id, &BTreeMap::new()))
        .expect("preview");
    assert_eq!(pv.entries.len(), 1);
    let line_of = |side: Side, amt: Decimal| (side, amt);
    let mut got: Vec<(String, Side, Decimal)> = pv.entries[0]
        .lines
        .iter()
        .map(|l| {
            let (side, amt) = if l.debit.is_zero() {
                line_of(Side::Credit, l.credit.as_decimal())
            } else {
                line_of(Side::Debit, l.debit.as_decimal())
            };
            (l.line_type.clone(), side, amt)
        })
        .collect();
    let mut exp: Vec<(String, Side, Decimal)> = want[0]
        .lines
        .iter()
        .map(|l| (l.line_type.clone(), l.side, l.amount))
        .collect();
    got.sort();
    exp.sort();
    assert_eq!(got, exp, "preview line by line");
    let req = PostRequest {
        preview_token: pv.token.clone(),
        ..PostRequest::default()
    };
    let entries = posted(
        h.run(s.payroll().post(id, &req, IdempotencyKey::new()))
            .expect("post"),
        stem,
    );
    assert_eq!(entries.len(), 1);
    let t = h
        .run(s.transactions().get(entries[0]))
        .expect("entry")
        .record;
    assert_eq!(t.txn_date.to_wire(), "2025-08-22");
    let mut got: Vec<(AccountId, Side, Decimal)> = t
        .lines
        .iter()
        .map(|l| {
            if l.debit.is_zero() {
                (l.account_id, Side::Credit, l.credit.as_decimal())
            } else {
                (l.account_id, Side::Debit, l.debit.as_decimal())
            }
        })
        .collect();
    let mut exp: Vec<(AccountId, Side, Decimal)> = want[0]
        .lines
        .iter()
        .map(|l| (acct[&l.line_type], l.side, l.amount))
        .collect();
    got.sort();
    exp.sort();
    assert_eq!(got, exp, "posted line by line");
    let (dr, cr) = t
        .lines
        .iter()
        .fold((Decimal::ZERO, Decimal::ZERO), |(d, c), l| {
            (d + l.debit.as_decimal(), c + l.credit.as_decimal())
        });
    assert_eq!(dr, want[0].total_debits);
    assert_eq!(cr, want[0].total_credits);
}

/// Review S3: a 60-row employee file whose third department first appears
/// at row 30 (past the 25 sample rows). Validate lists all three, and the
/// post stamps each department's tag on its expense lines with the
/// amounts computed here.
pub fn every_department_is_listed_and_tagged(h: &Harness) {
    if !review_caps(h) {
        return;
    }
    let s = fresh_company(h, "Payroll sixty rows");
    let a = map_accounts(h, &s);
    let d = |x: &str| x.parse::<Decimal>().expect("decimal");
    let dept_of = |i: u32| match i {
        30.. if i.is_multiple_of(3) => "Bar",
        _ if i.is_multiple_of(2) => "Kitchen",
        _ => "Front",
    };
    let mut csv = String::from(
        "Worker,Paid On,Dept,Wages,Fed WH,State WH,SS EE,Medicare EE,SS ER,Medicare ER,Take Home\n",
    );
    // Expected expense per department: wages plus the employer taxes.
    let mut expense: BTreeMap<&str, Decimal> = BTreeMap::new();
    let mut net = Decimal::ZERO;
    let withheld = d("100.00") + d("30.00") + d("62.00") + d("14.50");
    for i in 1..=60u32 {
        let wages = d("1000.00") + Decimal::new(i64::from(i), 2);
        let take_home = wages - withheld;
        net += take_home;
        *expense.entry(dept_of(i)).or_default() += wages + d("62.00") + d("14.50");
        let dept = dept_of(i);
        let _ = writeln!(
            csv,
            "Test Worker {i:02},07/11/2025,{dept},{wages},100.00,30.00,62.00,14.50,62.00,14.50,{take_home}"
        );
    }
    assert!(
        (1..=25).all(|i| dept_of(i) != "Bar"),
        "Bar is past the sample rows"
    );
    let up = h
        .run(s.payroll().upload(&UploadRequest {
            mode: ImportMode::EmployeeLevel,
            bytes: csv.into_bytes(),
            file_name: "sixty.csv".into(),
            period_start: day("2025-06-28"),
            period_end: day("2025-07-11"),
            check_date: day("2025-07-11"),
        }))
        .expect("upload");
    let mut map = column_map("generic_period1").expect("map");
    map.mappings.insert(
        "department".into(),
        ColumnSource {
            source: "Dept".into(),
            format: None,
        },
    );
    h.run(s.payroll().apply_mapping(up.session, &map))
        .expect("mapping");
    let v = h.run(s.payroll().validate(up.session)).expect("validate");
    assert!(v.can_continue(), "{v:?}");
    assert_eq!(v.total_rows, 60);
    assert_eq!(
        v.departments.as_deref(),
        Some(&["Bar".to_string(), "Front".into(), "Kitchen".into()][..]),
        "every department, sorted"
    );
    assert!(!v.departments_truncated);
    let tags: BTreeMap<String, Option<TagId>> = ["Bar", "Front", "Kitchen"]
        .into_iter()
        .map(|n| (n.to_string(), Some(new_tag(h, &s, n))))
        .collect();
    let pv = h
        .run(s.payroll().preview(up.session, &tags))
        .expect("preview");
    let req = PostRequest {
        preview_token: pv.token.clone(),
        department_tags: tags.clone(),
        ..PostRequest::default()
    };
    let entries = posted(
        h.run(s.payroll().post(up.session, &req, IdempotencyKey::new()))
            .expect("post"),
        "sixty rows",
    );
    assert_eq!(entries.len(), 1, "one pay date");
    let (_, got) = posted_by_tag(h, &s, entries[0]);
    for (dept, amount) in &expense {
        let tag = tags[*dept];
        assert_eq!(
            got.get(&(a.wages, Side::Debit, tag)),
            Some(amount),
            "{dept}: its tag and amount"
        );
    }
    assert!(
        !got.keys()
            .any(|(acct, side, tag)| *acct == a.wages && *side == Side::Debit && tag.is_none()),
        "no untagged expense: every row has a department"
    );
    assert_eq!(
        got.get(&(a.bank, Side::Credit, None)),
        Some(&net),
        "net pay: one untagged line"
    );
}
