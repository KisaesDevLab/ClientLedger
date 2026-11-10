//! Golden reports on the golden company (P28 WP2, AC1/AC2, plan §6): the
//! practice company plus `report_story.json` in a fresh test tenant, each
//! catalog report run through `cl-model` (options to request) and `cl-api`
//! (request to table), written as canonical CSV under
//! `fixtures/golden/reports/`.
//!
//! The cross-checks run first, on every run: October's P&L equals the
//! practice company's figures (13,865.14; Jan–Oct 125,985.00; each
//! account's October sum from the fixture), every statement ties to the
//! Trial Balance, every account balance ties to the General Ledger, and
//! every P&L amount drills to ledger lines that sum to it exactly. Only
//! then are the goldens compared, or written with `CL_GOLDEN_BLESS=1`
//! (`cargo xtask golden-reports --bless`).
//!
//! Ids differ on every run, so the CSVs name accounts by number, clients,
//! vendors and tags by name, and any other id by its order of first
//! appearance in the file.
//!
//! Without the engine executable or the `PostgreSQL` binaries the test
//! prints `SKIPPED` and passes, unless `CL_API_REQUIRE=1`. Options the
//! pinned engine can't run yet (U-201 columns and tag split, U-202 drill
//! filters, U-203 budgets) print `PENDING` with their capability.
#![allow(
    clippy::too_many_lines,
    reason = "one linear scenario: load, post, cross-check, compare"
)]

use std::collections::{BTreeMap, HashMap};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use cl_api::{
    AccountFilter, Client, ClientConfig, ContactFilter, EndpointSource, EndpointState,
    EngineEndpoint, LoginOutcome, Session,
};
use cl_company::report_story::{Posted, ReportStory};
use cl_company::sample::Sample;
use cl_core::fiscal::FiscalYearSpec;
use cl_core::report::{Basis, Cell, Drill, ReportRequest, ReportTable, RowKind, TagScope};
use cl_core::{CalendarDate, Secret};
use cl_host::{AdminAccount, HostConfig, HostState, Supervisor};
use cl_model::reports::{DateChoice, ReportOptions, find, resolve};
use cl_settings::RecentStore;
use rust_decimal::Decimal;

const OWNER: &str = "owner@example.com";

fn require() -> bool {
    std::env::var("CL_API_REQUIRE").as_deref() == Ok("1")
}

fn bless() -> bool {
    std::env::var("CL_GOLDEN_BLESS").as_deref() == Ok("1")
}

fn skip(why: &str) {
    assert!(!require(), "CL_API_REQUIRE=1 but {why}");
    eprintln!("SKIPPED: {why} (CI's engine job sets CL_API_REQUIRE=1)");
}

fn pending(item: &str, why: &str) {
    eprintln!("PENDING({item}): {why}");
}

fn engine_exe() -> Option<PathBuf> {
    if let Some(p) = cl_host::find_engine_exe(None) {
        return Some(p);
    }
    // The test binary's own `target`, else the workspace's (a separate
    // build directory keeps test binaries outside it).
    let exe = std::env::current_exe().ok()?;
    let target = exe
        .ancestors()
        .find(|d| d.file_name().is_some_and(|n| n == "target"))
        .map_or_else(
            || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target"),
            Path::to_path_buf,
        );
    std::fs::read_dir(target.join("engine"))
        .ok()?
        .flatten()
        .map(|e| e.path().join(cl_host::engine_exe_name()))
        .find(|p| p.is_file())
}

struct Source(Arc<Supervisor>);

impl EndpointSource for Source {
    fn current(&self) -> EndpointState {
        let snap = self.0.snapshot();
        match (snap.status.state, snap.endpoint) {
            (HostState::Running, Some(ep)) => EndpointState::Ready(EngineEndpoint {
                port: ep.port,
                token: Secret::new(ep.token.expose().to_string()),
                generation: ep.generation,
            }),
            (HostState::Running | HostState::Restarting { .. }, _) => EndpointState::Restarting,
            (HostState::Starting, _) => EndpointState::Starting,
            (HostState::Fault(_) | HostState::Stopping | HostState::Stopped, _) => {
                EndpointState::Down { permanent: true }
            }
        }
    }
}

fn d(y: i32, m: u32, day: u32) -> CalendarDate {
    CalendarDate::from_ymd(y, m, day).expect("date")
}

fn dec(c: &Cell) -> Decimal {
    match c {
        Cell::Amount(x) => *x,
        other => panic!("not an amount: {other:?}"),
    }
}

/// The story's fiscal calendar: the practice company's year starts in
/// January.
const FISCAL: FiscalYearSpec = FiscalYearSpec::CALENDAR_YEAR;
/// "Today" for presets (never used: every golden has explicit dates).
fn today() -> CalendarDate {
    d(2026, 12, 31)
}

/// One golden: a catalog report on fixed options.
struct Golden {
    file: String,
    key: &'static str,
    options: ReportOptions,
}

fn golden(file: &str, key: &'static str, f: impl FnOnce(&mut ReportOptions)) -> Golden {
    let spec = find(key).unwrap_or_else(|| panic!("catalog has {key}"));
    let mut options = ReportOptions::for_spec(spec);
    f(&mut options);
    Golden {
        file: file.to_string(),
        key,
        options,
    }
}

fn range(o: &mut ReportOptions, from: CalendarDate, to: CalendarDate) {
    o.dates = DateChoice::Fixed { from, to };
}

/// The goldens this engine runs (PR-A, `reports-exact-v1`).
fn goldens(posted: &Posted) -> Vec<Golden> {
    let north = posted.tags["North lawn"];
    let mut v = Vec::new();
    for (name, from, to) in [
        ("2026-10", d(2026, 10, 1), d(2026, 10, 31)),
        ("2026-11", d(2026, 11, 1), d(2026, 11, 30)),
        ("2026-12", d(2026, 12, 1), d(2026, 12, 31)),
        ("2026", d(2026, 1, 1), d(2026, 12, 31)),
    ] {
        for basis in [Basis::Accrual, Basis::Cash] {
            let b = basis.as_str();
            v.push(golden(
                &format!("profit_loss__{name}_{b}"),
                "profit_loss",
                |o| {
                    range(o, from, to);
                    o.basis = Some(basis);
                },
            ));
        }
    }
    v.push(golden(
        "profit_loss__2026-12_accrual_without_adjusting",
        "profit_loss",
        |o| {
            range(o, d(2026, 12, 1), d(2026, 12, 31));
            o.filters.adjusting = Some(cl_core::report::Adjusting::Exclude);
        },
    ));
    v.push(golden(
        "profit_loss__2026-q4_tag_north_lawn",
        "profit_loss",
        |o| {
            range(o, d(2026, 10, 1), d(2026, 12, 31));
            o.filters.tag_ids = vec![north];
        },
    ));
    for (name, at) in [
        ("2026-10-31", d(2026, 10, 31)),
        ("2026-11-30", d(2026, 11, 30)),
        ("2026-12-31", d(2026, 12, 31)),
    ] {
        for basis in [Basis::Accrual, Basis::Cash] {
            let b = basis.as_str();
            v.push(golden(
                &format!("balance_sheet__{name}_{b}"),
                "balance_sheet",
                |o| {
                    o.dates = DateChoice::AsOf(at);
                    o.basis = Some(basis);
                },
            ));
            v.push(golden(
                &format!("trial_balance__{name}_{b}"),
                "trial_balance",
                |o| {
                    o.dates = DateChoice::AsOf(at);
                    o.basis = Some(basis);
                },
            ));
        }
    }
    v.push(golden("general_ledger__2026-11", "general_ledger", |o| {
        range(o, d(2026, 11, 1), d(2026, 11, 30));
    }));
    // No cash-basis ledger golden: the engine orders a day's cash-basis
    // lines differently from run to run (and numbers them by row), so the
    // running balances move; its totals are pinned by the cross-checks
    // (Q-P28x).
    for (key, file) in [
        ("revenue_by_category", "revenue_by_category"),
        ("expenses_by_category", "expenses_by_category"),
    ] {
        for basis in [Basis::Accrual, Basis::Cash] {
            let b = basis.as_str();
            v.push(golden(&format!("{file}__2026-q4_{b}"), key, |o| {
                range(o, d(2026, 10, 1), d(2026, 12, 31));
                o.basis = Some(basis);
            }));
        }
    }
    v
}

/// The engine runs `options` of `key`.
async fn run(s: &Session, key: &str, options: &ReportOptions) -> (ReportRequest, ReportTable) {
    let spec = find(key).unwrap_or_else(|| panic!("catalog has {key}"));
    let req = resolve(spec, options, &FISCAL, today()).expect("resolve");
    // The engine allows 30 report reads a minute (`expensiveOpLimiter`);
    // the harness waits out the window rather than failing.
    for _ in 0..10 {
        match s.reports().run(&req).await {
            Ok(table) => return (req, table),
            Err(cl_api::ApiError::RateLimited { retry_after }) => {
                tokio::time::sleep(retry_after.unwrap_or(Duration::from_secs(15))).await;
            }
            Err(e) => panic!("{key}: {e}"),
        }
    }
    panic!("{key}: still rate limited")
}

/// Every id in `text` named stably: accounts by number, contacts and tags
/// by name, any other id by order of first appearance.
fn stable_ids(text: &str, names: &HashMap<String, String>) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut others: HashMap<String, usize> = HashMap::new();
    let mut i = 0;
    while i < bytes.len() {
        if let Some(id) = text.get(i..i + 36).filter(|c| is_uuid(c)) {
            if let Some(n) = names.get(id) {
                out.push_str(n);
            } else {
                let next = others.len() + 1;
                let k = *others.entry(id.to_string()).or_insert(next);
                out.push_str(&format!("#{k}"));
            }
            i += 36;
        } else {
            let ch = text[i..].chars().next().expect("char");
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}

fn is_uuid(s: &str) -> bool {
    s.bytes().enumerate().all(|(i, b)| match i {
        8 | 13 | 18 | 23 => b == b'-',
        _ => b.is_ascii_hexdigit(),
    })
}

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/golden/reports")
}

/// The ledger's net (debits − credits) behind a drill.
async fn drill_net(s: &Session, drill: &Drill) -> Decimal {
    let Drill::Lines {
        account_ids,
        from,
        to,
        tag,
        basis,
        adjusting,
        ..
    } = drill
    else {
        panic!("not a lines drill: {drill:?}")
    };
    // One ledger of every account per dates, basis, lens and tag (the
    // engine rate-limits a run of hundreds of drills); its per-account
    // totals cover every line even past the screen limit. The drill's own
    // account filter is what the window sends; here the same accounts'
    // totals are read from the whole ledger.
    let spec = find("general_ledger").expect("gl");
    let mut o = ReportOptions::for_spec(spec);
    o.dates = DateChoice::Fixed {
        from: from.unwrap_or_else(|| d(1900, 1, 1)),
        to: *to,
    };
    o.basis = *basis;
    o.filters.adjusting = *adjusting;
    if let TagScope::Tag(t) = tag {
        o.filters.tag_ids = vec![*t];
    }
    let key = format!(
        "{:?}",
        (o.dates, o.basis, o.filters.adjusting, &o.filters.tag_ids)
    );
    let cached = LEDGERS.lock().expect("cache").get(&key).cloned();
    let nets = if let Some(n) = cached {
        n
    } else {
        let (_, gl) = run(s, spec.key, &o).await;
        let n: HashMap<String, Decimal> = gl
            .rows
            .iter()
            .filter(|r| r.kind == RowKind::AccountTotal)
            .filter_map(|r| {
                let id = r.path.0.first()?.strip_suffix("~total")?.to_string();
                Some((id, dec(&r.cells[5]) - dec(&r.cells[6])))
            })
            .collect();
        LEDGERS.lock().expect("cache").insert(key, n.clone());
        n
    };
    account_ids
        .iter()
        .map(|a| nets.get(&a.to_string()).copied().unwrap_or_default())
        .sum()
}

/// Whole ledgers already read, by their options.
#[allow(clippy::type_complexity, reason = "a test cache")]
static LEDGERS: std::sync::LazyLock<std::sync::Mutex<HashMap<String, HashMap<String, Decimal>>>> =
    std::sync::LazyLock::new(Default::default);

/// Each account row's first amount by account number (`4000 · Name`).
fn by_number(t: &ReportTable, col: usize) -> BTreeMap<String, Decimal> {
    t.rows
        .iter()
        .filter(|r| r.kind == RowKind::Account)
        .filter_map(|r| {
            let n = r.caption.split(" \u{b7} ").next()?.to_string();
            Some((n, dec(&r.cells[col])))
        })
        .collect()
}

/// Cross-check 1: October equals the practice company's figures.
async fn october_matches_the_practice_company(s: &Session, sample: &Sample) {
    let mut o = ReportOptions::for_spec(find("profit_loss").expect("p&l"));
    o.dates = DateChoice::Fixed {
        from: d(2026, 10, 1),
        to: d(2026, 10, 31),
    };
    let (_, oct) = run(s, "profit_loss", &o).await;
    let net = dec(&oct.rows.last().expect("net").cells[0]);
    assert_eq!(
        net,
        Decimal::new(1_386_514, 2),
        "October net income (mockup)"
    );
    o.dates = DateChoice::Fixed {
        from: d(2026, 1, 1),
        to: d(2026, 10, 31),
    };
    let (_, ytd) = run(s, "profit_loss", &o).await;
    let net = dec(&ytd.rows.last().expect("net").cells[0]);
    assert_eq!(
        net,
        Decimal::new(12_598_500, 2),
        "Jan-Oct net income (mockup)"
    );
    // Each account's October amount is the fixture's October sum, signed
    // as the P&L shows it.
    let types: HashMap<&str, cl_api::AccountType> = sample
        .company
        .accounts
        .iter()
        .map(|a| (a.number.as_str(), a.account_type))
        .collect();
    let mut want: BTreeMap<String, Decimal> = BTreeMap::new();
    for t in sample
        .transactions
        .iter()
        .filter(|t| t.date >= d(2026, 10, 1) && t.date <= d(2026, 10, 31))
    {
        for l in &t.lines {
            let net = l.debit.as_decimal() - l.credit.as_decimal();
            let signed = match types.get(l.account.as_str()) {
                Some(cl_api::AccountType::Revenue | cl_api::AccountType::OtherRevenue) => -net,
                Some(
                    cl_api::AccountType::Cogs
                    | cl_api::AccountType::Expense
                    | cl_api::AccountType::OtherExpense,
                ) => net,
                _ => continue,
            };
            *want.entry(l.account.clone()).or_default() += signed;
        }
    }
    want.retain(|_, v| !v.is_zero());
    let got: BTreeMap<String, Decimal> = by_number(&oct, 0)
        .into_iter()
        .filter(|(_, v)| !v.is_zero())
        .collect();
    assert_eq!(got, want, "October by account = the practice fixture");
}

/// Cross-check 2: the Balance Sheet and the P&L tie to the Trial Balance,
/// and every Trial Balance account to the General Ledger.
async fn statements_tie_to_the_trial_balance(s: &Session, at: CalendarDate, basis: Basis) {
    let mut tb_o = ReportOptions::for_spec(find("trial_balance").expect("tb"));
    tb_o.dates = DateChoice::AsOf(at);
    tb_o.basis = Some(basis);
    let (_, tb) = run(s, "trial_balance", &tb_o).await;
    let grand = tb.rows.last().expect("total");
    assert_eq!(
        dec(&grand.cells[0]),
        dec(&grand.cells[1]),
        "{at} TB balances"
    );
    // Net debit per account number.
    let tb_net: BTreeMap<String, Decimal> = tb
        .rows
        .iter()
        .filter(|r| r.kind == RowKind::Account)
        .filter_map(|r| {
            let n = r.caption.split(" \u{b7} ").next()?.to_string();
            Some((n, dec(&r.cells[0]) - dec(&r.cells[1])))
        })
        .collect();

    let mut bs_o = ReportOptions::for_spec(find("balance_sheet").expect("bs"));
    bs_o.dates = DateChoice::AsOf(at);
    bs_o.basis = Some(basis);
    let (_, bs) = run(s, "balance_sheet", &bs_o).await;
    for r in bs.rows.iter().filter(|r| r.kind == RowKind::Account) {
        let Some(n) = r.caption.split(" \u{b7} ").next() else {
            continue;
        };
        if r.caption.starts_with('@') {
            continue;
        }
        let asset = r.path.0.first().is_some_and(|p| p == "assets");
        let sheet = dec(&r.cells[0]);
        let want = if asset { sheet } else { -sheet };
        // A parent added at zero by the engine's tree has no TB row.
        let tb_value = tb_net.get(n).copied().unwrap_or_default();
        if r.expandable && sheet.is_zero() {
            continue;
        }
        assert_eq!(tb_value, want, "{at} {basis:?} sheet {n} ties to the TB");
        // And to the ledger.
        let drill = r.drill.as_ref().expect("drill").at(&bs.columns[0]);
        assert_eq!(
            drill_net(s, &drill).await,
            tb_value,
            "{at} {n} ties to the GL"
        );
    }
    let assets = bs
        .rows
        .iter()
        .find(|r| r.path.0 == ["assets~total"])
        .expect("assets");
    assert_eq!(
        dec(&assets.cells[0]),
        dec(&bs.rows.last().expect("L&E").cells[0]),
        "{at} sheet balances"
    );

    let mut pl_o = ReportOptions::for_spec(find("profit_loss").expect("p&l"));
    pl_o.dates = DateChoice::Fixed {
        from: d(at.year(), 1, 1),
        to: at,
    };
    pl_o.basis = Some(basis);
    let (_, pl) = run(s, "profit_loss", &pl_o).await;
    for r in pl.rows.iter().filter(|r| r.kind == RowKind::Account) {
        let Some(n) = r.caption.split(" \u{b7} ").next() else {
            continue;
        };
        let amount = dec(&r.cells[0]);
        if r.expandable && amount.is_zero() {
            continue;
        }
        let income = r
            .path
            .0
            .first()
            .is_some_and(|p| p == "income" || p == "other_income");
        let want = if income { -amount } else { amount };
        assert_eq!(
            tb_net.get(n).copied().unwrap_or_default(),
            want,
            "{at} {basis:?} P&L {n} ties to the TB"
        );
    }
}

/// Cross-check 3 (AC2): every P&L amount drills to lines that sum to it.
async fn every_pl_cell_drills_to_its_amount(s: &Session, t: &ReportTable) -> usize {
    let mut n = 0;
    for r in &t.rows {
        let Some(drill) = &r.drill else { continue };
        for (i, c) in t.columns.iter().enumerate() {
            let cell = dec(&r.cells[i]);
            let d = drill.at(c);
            let net = drill_net(s, &d).await;
            let income = r
                .path
                .0
                .first()
                .is_some_and(|p| p.starts_with("income") || p.starts_with("other_income"));
            let want = if income { -cell } else { cell };
            assert_eq!(net, want, "drill of {} ({})", r.caption, c.key);
            n += 1;
        }
    }
    n
}

async fn scenario(client: &Client, password: &Secret, scratch: &Path) {
    let s = match client.login(OWNER, password).await {
        Ok(LoginOutcome::Session(s)) => s,
        other => panic!("login: {other:?}"),
    };
    let caps = client.capabilities();
    if !caps.has(cl_api::Capabilities::REPORTS_EXACT) {
        return skip("the engine has no reports-exact-v1 (U-14)");
    }
    let store = RecentStore::in_dir(&scratch.join("settings"));
    cl_company::sample::load_into(
        &s,
        &store,
        &cl_company::backup::silent(),
        &cl_company::Statistics::none(),
    )
    .await
    .expect("the practice company loads");
    let story = ReportStory::load();
    let posted = cl_company::report_story::post(&s, &story)
        .await
        .unwrap_or_else(|e| panic!("the report story posts: {e}"));
    let sample = Sample::load();

    // ---------------------------------------------------- cross-checks
    october_matches_the_practice_company(&s, &sample).await;
    for at in [d(2026, 10, 31), d(2026, 11, 30), d(2026, 12, 31)] {
        for basis in [Basis::Accrual, Basis::Cash] {
            statements_tie_to_the_trial_balance(&s, at, basis).await;
        }
    }
    let mut drilled = 0;
    for (from, to) in [
        (d(2026, 10, 1), d(2026, 10, 31)),
        (d(2026, 11, 1), d(2026, 11, 30)),
        (d(2026, 10, 1), d(2026, 12, 31)),
    ] {
        for basis in [Basis::Accrual, Basis::Cash] {
            let mut o = ReportOptions::for_spec(find("profit_loss").expect("p&l"));
            o.dates = DateChoice::Fixed { from, to };
            o.basis = Some(basis);
            let (_, t) = run(&s, "profit_loss", &o).await;
            drilled += every_pl_cell_drills_to_its_amount(&s, &t).await;
        }
    }
    assert!(
        drilled > 100,
        "account, parent total and subtotal cells drill"
    );
    for (item, cap, why) in [
        (
            "U-201",
            "reports-columns-v1",
            "month, fiscal period, YTD, prior, by client and by tag columns; tag split (untagged + tags = account total, AC5)",
        ),
        (
            "U-202",
            "reports-drill-v1",
            "the Journal and drills by client, untagged, amount, cleared and type",
        ),
        (
            "U-203",
            "budgets-exact-v1",
            "Budget vs. Actual against fixtures/budgets/bva_hand.json (AC4)",
        ),
    ] {
        if !caps.has(cap) {
            pending(item, why);
        }
    }

    // ------------------------------------------------------- goldens
    let mut names: HashMap<String, String> = HashMap::new();
    for a in s
        .accounts()
        .list_all(&AccountFilter::default())
        .await
        .expect("accounts")
    {
        let n = a
            .fields
            .account_number
            .clone()
            .unwrap_or_else(|| a.fields.name.clone());
        names.insert(a.id.to_string(), format!("acct:{n}"));
    }
    for c in s
        .contacts()
        .list_all(&ContactFilter::default())
        .await
        .expect("contacts")
    {
        names.insert(
            c.id.to_string(),
            format!("contact:{}", c.fields.display_name),
        );
    }
    for (n, id) in &posted.tags {
        names.insert(id.to_string(), format!("tag:{n}"));
    }
    let dir = golden_dir();
    let mut mismatched = Vec::new();
    for g in goldens(&posted) {
        let (_, t) = run(&s, g.key, &g.options).await;
        let csv = stable_ids(&t.to_canonical_csv(), &names);
        let path = dir.join(format!("{}.csv", g.file));
        if bless() {
            std::fs::create_dir_all(&dir).expect("golden dir");
            std::fs::write(&path, &csv).expect("write golden");
            continue;
        }
        match std::fs::read_to_string(&path) {
            Ok(want) if want.replace("\r\n", "\n") == csv => {}
            Ok(_) => {
                // The answer beside the golden for a diff (target/, ignored).
                let out = Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../target/golden-reports-actual");
                let _ = std::fs::create_dir_all(&out);
                let _ = std::fs::write(out.join(format!("{}.csv", g.file)), &csv);
                mismatched.push(format!("{} differs", g.file));
            }
            Err(_) => mismatched.push(format!("{} is missing", g.file)),
        }
    }
    assert!(
        mismatched.is_empty(),
        "golden reports: {mismatched:?} (after a reviewed change: cargo xtask golden-reports --bless)"
    );
    let _ = s.logout().await;
}

#[test]
fn golden_reports_match_after_the_cross_checks_pass() {
    let Some(engine) = engine_exe() else {
        return skip("the engine executable is missing (run `cargo xtask engine`)");
    };
    let Ok(pg) = cl_pg::PgBinaries::discover(None) else {
        return skip("the PostgreSQL binaries are missing (run `cargo xtask pg-fetch`)");
    };
    let root = std::env::temp_dir().join(format!("cl-company-golden-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("scratch");
    let paths = cl_host::AppPaths::at(root.join("data")).expect("paths");
    let mut cfg = cl_host::in_memory_stores(HostConfig::new(paths, engine));
    cfg.pg_binaries = Some(pg);
    cfg.policy.ready_timeout = Duration::from_mins(3);
    cfg.policy.health_ready_timeout = Duration::from_mins(2);
    cfg.policy.health_interval = Duration::from_secs(1);
    let guard = match cl_host::acquire(&cfg.paths).expect("lock") {
        cl_host::Acquired::First(g) => g,
        cl_host::Acquired::Second => panic!("instance lock held"),
    };
    let sup = Arc::new(Supervisor::start(cfg, guard).expect("start"));
    let password = Secret::new(format!(
        "Cl-{}",
        &cl_api::IdempotencyKey::new().as_header().replace('-', "")[..20]
    ));
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        sup.initialize_engine(
            &AdminAccount {
                email: OWNER.into(),
                display_name: "Golden Owner".into(),
                password: cl_host::Secret::new(password.expose().to_string()),
            },
            None,
        )
        .expect("initialize");
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("runtime");
        {
            let source: Arc<dyn EndpointSource> = Arc::new(Source(Arc::clone(&sup)));
            let client = Client::new(source, ClientConfig::default()).expect("client");
            rt.block_on(async {
                tokio::time::timeout(Duration::from_mins(15), async {
                    client.desktop_info().await.expect("info");
                    scenario(&client, &password, &root).await;
                })
                .await
                .expect("within fifteen minutes");
            });
        }
        rt.shutdown_timeout(Duration::from_secs(5));
    }));
    match Arc::try_unwrap(sup) {
        Ok(s) => s.stop(),
        Err(shared) => drop(shared),
    }
    if std::env::var("CL_API_KEEP").as_deref() != Ok("1") {
        for _ in 0..40 {
            if std::fs::remove_dir_all(&root).is_ok() || !root.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }
    if let Err(p) = outcome {
        std::panic::resume_unwind(p);
    }
}

#[test]
fn ids_are_named_stably() {
    let mut names = HashMap::new();
    names.insert(
        "11111111-1111-4111-8111-111111111111".to_string(),
        "acct:4000".to_string(),
    );
    let text = "a/11111111-1111-4111-8111-111111111111/0:22222222-2222-4222-8222-222222222222,x\n\
                b/0:33333333-3333-4333-8333-333333333333 \u{b7} 0:22222222-2222-4222-8222-222222222222\n";
    assert_eq!(
        stable_ids(text, &names),
        "a/acct:4000/0:#1,x\nb/0:#2 \u{b7} 0:#1\n"
    );
}
