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
use cl_core::report::{Basis, Cell, Drill, ReportRequest, ReportTable, RowKind};
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

/// A capability the pinned engine lacks: under `CL_API_REQUIRE=1` that is
/// a failure, never a quiet skip of AC4/AC5 (review S11).
fn pending(item: &str, why: &str) {
    assert!(!require(), "CL_API_REQUIRE=1 but {item} is pending: {why}");
    eprintln!("PENDING({item}): {why}");
}

/// How many drills were also run as the window sends them.
static OWN_DRILLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

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
/// The U-201 goldens (`reports-columns-v1`): Q4 2026 by month, with YTD
/// and % of income, by client, by tag group and with the tag split.
fn column_options(posted: &Posted) -> Vec<(String, &'static str, ReportOptions)> {
    use cl_core::report::Extra;
    use cl_model::reports::{ColumnBy, TagLabel, TagSplit};
    let job = posted.tag_groups["Job site"];
    let q4 = |o: &mut ReportOptions| {
        o.dates = DateChoice::Fixed {
            from: d(2026, 10, 1),
            to: d(2026, 12, 31),
        };
    };
    let mut v = Vec::new();
    let mut add = |file: &str, key: &'static str, f: &dyn Fn(&mut ReportOptions)| {
        let mut o = ReportOptions::for_spec(find(key).expect("spec"));
        f(&mut o);
        v.push((file.to_string(), key, o));
    };
    add(
        "profit_loss__2026-q4_by_month_ytd_pct",
        "profit_loss_by_month",
        &|o| {
            q4(o);
            o.columns = ColumnBy::Month;
            o.extras.insert(Extra::Ytd);
            o.extras.insert(Extra::PctOfIncome);
        },
    );
    add("profit_loss__2026-q4_prior_period", "profit_loss", &|o| {
        q4(o);
        o.extras.insert(Extra::PriorPeriod);
    });
    // B3/T4: November compares with all of October; a one-date sheet with
    // the previous month end.
    add("profit_loss__2026-11_prior_period", "profit_loss", &|o| {
        o.dates = DateChoice::Fixed {
            from: d(2026, 11, 1),
            to: d(2026, 11, 30),
        };
        o.extras.insert(Extra::PriorPeriod);
    });
    add(
        "balance_sheet__2026-11-30_prior_period",
        "balance_sheet",
        &|o| {
            o.dates = DateChoice::AsOf(d(2026, 11, 30));
            o.extras.insert(Extra::PriorPeriod);
        },
    );
    add(
        "profit_loss__2026-q4_by_client",
        "profit_loss_by_client",
        &|o| {
            q4(o);
            o.columns = ColumnBy::Client;
        },
    );
    add(
        "profit_loss__2026-q4_by_tag_job_site",
        "profit_loss_by_tag_group",
        &|o| {
            q4(o);
            o.columns = ColumnBy::TagGroup;
            o.column_tag_group_id = Some(job);
        },
    );
    add(
        "profit_loss__2026-q4_tag_split_job_site",
        "profit_loss",
        &|o| {
            q4(o);
            o.tag_split = Some(TagSplit {
                tag_group_id: job,
                show_account_total: true,
                label: TagLabel::default(),
                separator: "-".into(),
            });
        },
    );
    // AC5: the label styles, with and without the account total (the CSV
    // is the presented table, `cl_model::reports::present`).
    for (name, label, total) in [
        ("append", TagLabel::Append, false),
        ("prepend", TagLabel::Prepend, false),
        ("none", TagLabel::None, false),
        ("total_append", TagLabel::Append, true),
    ] {
        let file = format!("profit_loss__2026-q4_tag_split_{name}");
        add(&file, "profit_loss", &|o| {
            q4(o);
            o.tag_split = Some(TagSplit {
                tag_group_id: job,
                show_account_total: total,
                label,
                separator: "-".into(),
            });
        });
    }
    // The Trial Balance's and the ledger's tag split (U-201).
    let split = |o: &mut ReportOptions| {
        o.tag_split = Some(TagSplit {
            tag_group_id: job,
            show_account_total: true,
            label: TagLabel::default(),
            separator: "-".into(),
        });
    };
    add(
        "trial_balance__2026-12-31_tag_split_job_site",
        "trial_balance",
        &|o| {
            o.dates = DateChoice::AsOf(d(2026, 12, 31));
            split(o);
        },
    );
    add(
        "general_ledger__2026-11_tag_split_job_site",
        "general_ledger",
        &|o| {
            o.dates = DateChoice::Fixed {
                from: d(2026, 11, 1),
                to: d(2026, 11, 30),
            };
            split(o);
        },
    );
    add("balance_sheet__2026-q4_by_month", "balance_sheet", &|o| {
        o.dates = DateChoice::AsOf(d(2026, 12, 31));
        o.columns = ColumnBy::Month;
    });
    v
}

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
                out.push('#');
                out.push_str(&k.to_string());
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
        contact,
        tag,
        basis,
        adjusting,
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
    o.filters.set_tag_scope(*tag);
    o.filters.set_contact_scope(contact);
    let key = format!(
        "{:?}",
        (
            o.dates,
            o.basis,
            o.filters.adjusting,
            &o.filters.tag_ids,
            o.filters.untagged_in_group,
            o.filters.untagged,
            &o.filters.contact_ids,
            o.filters.no_contact,
            &o.filters.other_contacts_than
        )
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
    let total: Decimal = account_ids
        .iter()
        .map(|a| nets.get(&a.to_string()).copied().unwrap_or_default())
        .sum();
    // S11: a few drills also run exactly as the window sends them (the
    // drill's own account filter), and must agree with the whole ledger.
    if OWN_DRILLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 6 {
        let mut own = o.clone();
        own.filters.account_ids.clone_from(account_ids);
        let (_, gl) = run(s, spec.key, &own).await;
        let sum: Decimal = gl
            .rows
            .iter()
            .filter(|r| r.kind == RowKind::AccountTotal)
            .map(|r| dec(&r.cells[5]) - dec(&r.cells[6]))
            .sum();
        assert_eq!(sum, total, "the drill's own request sums to the same lines");
    }
    total
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

/// U-213 (PR-6 engine fixes): the cash Trial Balance's prior years'
/// retained earnings on the cash basis (review B1). Provisional name until
/// the engine PR merges (Q-PR6e).
const CASH_TB_FIX: &str = "report-fixes-v2";

/// The P&L's net income over `from..=to` on `basis`.
async fn net_income(s: &Session, from: CalendarDate, to: CalendarDate, basis: Basis) -> Decimal {
    let mut o = ReportOptions::for_spec(find("profit_loss").expect("p&l"));
    o.dates = DateChoice::Fixed { from, to };
    o.basis = Some(basis);
    let (_, pl) = run(s, "profit_loss", &o).await;
    dec(&pl.rows.last().expect("net").cells[0])
}

/// Cross-check 2: the Balance Sheet and the P&L tie to the Trial Balance,
/// and every Trial Balance account to the General Ledger; the sheet's
/// equity rows tie to the P&L across the year boundary (T2) and every
/// sheet amount drills to what sums to it, or has no drill (T3).
async fn statements_tie_to_the_trial_balance(
    s: &Session,
    at: CalendarDate,
    basis: Basis,
    cash_tb_fixed: bool,
) {
    let mut tb_o = ReportOptions::for_spec(find("trial_balance").expect("tb"));
    tb_o.dates = DateChoice::AsOf(at);
    tb_o.basis = Some(basis);
    let (_, tb) = run(s, "trial_balance", &tb_o).await;
    let grand = tb.rows.last().expect("total");
    // B1: before U-213 the cash TB's prior years' row is accrual income.
    let b1 = basis == Basis::Cash && !cash_tb_fixed;
    if b1 && dec(&grand.cells[0]) != dec(&grand.cells[1]) {
        pending(
            "U-213",
            "the cash Trial Balance balances after the first fiscal year (B1)",
        );
    } else {
        assert_eq!(
            dec(&grand.cells[0]),
            dec(&grand.cells[1]),
            "{at} {basis:?} TB balances"
        );
    }
    // S5/T10: the practice history's journal entries keep A/R and A/P on
    // cash basis: the note says so.
    if basis == Basis::Cash {
        assert_eq!(
            tb.meta.note.as_deref(),
            Some(cl_api::resources::reports::adapters::statements::CASH_AR_AP_NOTE),
            "{at} cash TB note"
        );
    }
    // Net debit per account number; the computed prior years' row and the
    // Retained Earnings account share 30120 and add up (N7).
    let mut tb_net: BTreeMap<String, Decimal> = BTreeMap::new();
    let mut posted_re = Decimal::ZERO;
    for r in tb.rows.iter().filter(|r| r.kind == RowKind::Account) {
        let Some(n) = r.caption.split(" \u{b7} ").next() else {
            continue;
        };
        let net = dec(&r.cells[0]) - dec(&r.cells[1]);
        *tb_net.entry(n.to_string()).or_default() += net;
        let account_row = r
            .path
            .0
            .last()
            .is_some_and(|k| cl_core::ids::AccountId::parse(k).is_ok());
        if n == "30120" && account_row {
            posted_re += net;
        }
    }

    let mut bs_o = ReportOptions::for_spec(find("balance_sheet").expect("bs"));
    bs_o.dates = DateChoice::AsOf(at);
    bs_o.basis = Some(basis);
    let (_, bs) = run(s, "balance_sheet", &bs_o).await;
    if basis == Basis::Cash {
        assert_eq!(
            bs.meta.note.as_deref(),
            Some(cl_api::resources::reports::adapters::statements::CASH_AR_AP_NOTE),
            "{at} cash sheet note"
        );
    }
    // T2: the sheet's equity across the year boundary. Net income (current
    // year) is the fiscal year-to-date P&L; the Retained Earnings line is
    // the prior years' P&L plus what was posted to it.
    let year_ni = net_income(s, d(at.year(), 1, 1), at, basis).await;
    let prior_ni = net_income(s, d(1900, 1, 1), d(at.year() - 1, 12, 31), basis).await;
    assert!(!prior_ni.is_zero(), "the 2025 slice has income");
    let equity_row = |pred: &dyn Fn(&cl_core::report::ReportRow) -> bool| {
        bs.rows
            .iter()
            .find(|r| r.path.0.first().is_some_and(|p| p == "equity") && pred(r))
            .map(|r| dec(&r.cells[0]))
    };
    assert_eq!(
        equity_row(&|r| r.caption == "@reports.bs.net_income").unwrap_or_default(),
        year_ni,
        "{at} {basis:?} net income = the year-to-date P&L"
    );
    assert_eq!(
        equity_row(&|r| r.caption.starts_with("30120 ")).unwrap_or_default(),
        prior_ni - posted_re,
        "{at} {basis:?} Retained Earnings = prior years' income + posted"
    );
    every_bs_cell_drills_to_its_amount(s, &bs).await;
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
        if b1 && n == "30120" {
            // B1: the cash TB's computed row is accrual until U-213.
            continue;
        }
        assert_eq!(tb_value, want, "{at} {basis:?} sheet {n} ties to the TB");
        // And to the ledger, unless the line holds what no ledger does
        // (Retained Earnings with prior years' income, B2).
        match r.drill.as_ref() {
            Some(drill) => assert_eq!(
                drill_net(s, &drill.at(&bs.columns[0])).await,
                tb_value,
                "{at} {n} ties to the GL"
            ),
            None => assert_eq!(n, "30120", "{at} only Retained Earnings has no drill"),
        }
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

/// B3/T4: November's "Prior period" column is all of October (13,865.14,
/// the mockup's net income, with its October 1 lines), not the 30 days
/// before November; the one-date sheet's prior column is October 31's.
async fn prior_period_is_the_previous_month(s: &Session) {
    use cl_core::report::Extra;
    let mut o = ReportOptions::for_spec(find("profit_loss").expect("p&l"));
    o.dates = DateChoice::Fixed {
        from: d(2026, 11, 1),
        to: d(2026, 11, 30),
    };
    o.extras.insert(Extra::PriorPeriod);
    let (req, t) = run(s, "profit_loss", &o).await;
    assert_eq!(
        req.prior_ranges
            .as_deref()
            .and_then(<[_]>::first)
            .map(|r| (r.from, r.to)),
        Some((d(2026, 10, 1), d(2026, 10, 31)))
    );
    let prior = t
        .columns
        .iter()
        .position(|c| c.key == "total_prior")
        .expect("prior column");
    assert_eq!(
        dec(&t.rows.last().expect("net").cells[prior]),
        Decimal::new(1_386_514, 2),
        "November's prior period is October"
    );
    let mut b = ReportOptions::for_spec(find("balance_sheet").expect("bs"));
    b.dates = DateChoice::AsOf(d(2026, 11, 30));
    b.extras.insert(Extra::PriorPeriod);
    let (_, sheet) = run(s, "balance_sheet", &b).await;
    let mut oct = ReportOptions::for_spec(find("balance_sheet").expect("bs"));
    oct.dates = DateChoice::AsOf(d(2026, 10, 31));
    let (_, oct) = run(s, "balance_sheet", &oct).await;
    let prior = sheet
        .columns
        .iter()
        .position(|c| c.key.ends_with("_prior"))
        .expect("prior column");
    assert_eq!(
        sheet.rows.last().map(|r| dec(&r.cells[prior])),
        oct.rows.last().map(|r| dec(&r.cells[0])),
        "the sheet's prior period is October 31"
    );
}

/// T3 (B2): every Balance Sheet amount drills to what sums to it (ledger
/// lines, or the P&L for net income), or has no drill: the calculated
/// rows, Retained Earnings carrying prior years' income, Total equity with
/// either, and the grand total.
async fn every_bs_cell_drills_to_its_amount(s: &Session, bs: &ReportTable) {
    for r in &bs.rows {
        let Cell::Amount(cell) = r.cells[0] else {
            continue;
        };
        let section = r.path.0.first().map(String::as_str).unwrap_or_default();
        match &r.drill {
            Some(d @ Drill::Lines { .. }) => {
                let net = drill_net(s, &d.at(&bs.columns[0])).await;
                let want = if section.starts_with("assets") {
                    cell
                } else {
                    -cell
                };
                assert_eq!(net, want, "drill of {} sums to it", r.caption);
            }
            Some(Drill::ProfitLoss {
                from, to, basis, ..
            }) => {
                let ni = net_income(s, *from, *to, basis.unwrap_or(Basis::Accrual)).await;
                assert_eq!(ni, cell, "{} opens the P&L that equals it", r.caption);
            }
            Some(other) => panic!("{}: unexpected drill {other:?}", r.caption),
            None => assert!(
                r.kind == RowKind::Grand || section.starts_with("equity"),
                "{} has no drill",
                r.caption
            ),
        }
    }
}

/// Cross-check 3 (AC2): every P&L amount drills to lines that sum to it.
async fn every_pl_cell_drills_to_its_amount(s: &Session, t: &ReportTable) -> usize {
    let mut n = 0;
    for r in &t.rows {
        let Some(drill) = &r.drill else { continue };
        for (i, c) in t.columns.iter().enumerate() {
            let Cell::Amount(cell) = r.cells[i] else {
                continue;
            };
            let Some(d) = drill.for_cell(c) else { continue };
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
    let fixture = ReportStory::load();
    let posted = cl_company::report_story::post(&s, &fixture)
        .await
        .unwrap_or_else(|e| panic!("the report story posts: {e}"));
    let sample = Sample::load();

    // ---------------------------------------------------- cross-checks
    october_matches_the_practice_company(&s, &sample).await;
    for at in [d(2026, 10, 31), d(2026, 11, 30), d(2026, 12, 31)] {
        for basis in [Basis::Accrual, Basis::Cash] {
            statements_tie_to_the_trial_balance(&s, at, basis, caps.has(CASH_TB_FIX)).await;
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
    if caps.has("reports-columns-v1") && caps.has("reports-drill-v1") {
        let mut cells = 0;
        for (file, key, o) in column_options(&posted) {
            let (_, t) = run(&s, key, &o).await;
            if key.starts_with("balance_sheet") {
                continue;
            }
            if key == "trial_balance" || key == "general_ledger" {
                split_identity(&file, &t);
                continue;
            }
            cells += every_pl_cell_drills_to_its_amount(&s, &t).await;
            // AC5: untagged + tags = the account total.
            for (i, r) in t.rows.iter().enumerate() {
                if r.kind != RowKind::Account
                    || t.rows.get(i + 1).is_none_or(|n| {
                        n.kind != RowKind::TagRow && n.kind != RowKind::AccountTotal
                    })
                {
                    continue;
                }
                let mut sum = dec(&r.cells[0]);
                let mut j = i + 1;
                while t.rows[j].kind == RowKind::TagRow {
                    sum += dec(&t.rows[j].cells[0]);
                    j += 1;
                }
                assert_eq!(
                    sum,
                    dec(&t.rows[j].cells[0]),
                    "{file}: {} untagged + tags",
                    r.caption
                );
            }
        }
        assert!(cells > 100, "column cells drill");
        prior_period_is_the_previous_month(&s).await;
    }
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
    let mut all = goldens(&posted);
    let id_named = |want: &str| {
        names
            .iter()
            .find(|(_, n)| n.as_str() == want)
            .map(|(id, _)| id.clone())
    };
    let vendor = id_named("contact:Table Rock Nursery")
        .and_then(|i| cl_core::ids::ContactId::parse(&i).ok())
        .expect("vendor");
    let bank = id_named("acct:1010")
        .and_then(|i| cl_core::ids::AccountId::parse(&i).ok())
        .expect("bank");
    if caps.has("open-balances-asof-v1") && caps.has("ap-aging-exact-v1") {
        who_owes_what_ties(&s).await;
        all.extend(list_goldens(vendor, bank));
    } else {
        pending(
            "U-121",
            "A/P aging, Unpaid Bills, open balances and Open Invoices as of a date",
        );
    }
    if caps.has("closing-exceptions-v1") {
        closing_exceptions_list_the_closed_period_entry(&s).await;
    } else {
        pending("U-204", "Closing Date Exceptions");
    }
    if caps.has("reports-columns-v1") {
        all.extend(
            column_options(&posted)
                .into_iter()
                .map(|(file, key, options)| Golden { file, key, options }),
        );
    }
    if let Some(budget) = &posted.budget_id {
        let year = |o: &mut ReportOptions| {
            o.dates = DateChoice::Fixed {
                from: d(2026, 1, 1),
                to: d(2026, 12, 31),
            };
        };
        for basis in [Basis::Accrual, Basis::Cash] {
            let mut o = ReportOptions::for_spec(find("budget_vs_actual").expect("bva"));
            year(&mut o);
            o.budget_id = Some(budget.clone());
            o.basis = Some(basis);
            let (_, t) = run(&s, "budget_vs_actual", &o).await;
            budget_actuals_tie_to_the_pl(&s, &t, basis).await;
            all.push(Golden {
                file: format!("budget_vs_actual__2026_{}", basis.as_str()),
                key: "budget_vs_actual",
                options: o,
            });
        }
    }
    for g in all {
        // B1: a cash Trial Balance golden would pin the accrual prior years'
        // row until U-213; those files stay as they are (Q-PR6e).
        if g.key == "trial_balance" && g.file.ends_with("_cash") && !caps.has(CASH_TB_FIX) {
            pending(
                "U-213",
                &format!("{} (cash prior years' retained earnings)", g.file),
            );
            continue;
        }
        let (_, t) = run(&s, g.key, &g.options).await;
        // A tag split is written as presented: the owner's label style and
        // account-total choice (AC5).
        let t = if g.options.tag_split.is_some() {
            let all: Vec<usize> = (0..t.rows.len()).collect();
            cl_model::reports::present(&t, &all, &g.options)
        } else {
            t
        };
        let mut csv = stable_ids(&t.to_canonical_csv(), &names);
        if matches!(
            g.key,
            "ar_aging_detail" | "ap_aging_detail" | "check_register"
        ) {
            csv = stable_order(&csv);
        }
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

/// Budget vs. Actual's actual cells are the P&L by fiscal period's, account
/// by account and period by period (each table column group is actual,
/// budget, over, %).
async fn budget_actuals_tie_to_the_pl(s: &Session, t: &ReportTable, basis: Basis) {
    let mut p = ReportOptions::for_spec(find("profit_loss_by_fiscal_period").expect("p&l"));
    p.dates = DateChoice::Fixed {
        from: d(2026, 1, 1),
        to: d(2026, 12, 31),
    };
    p.basis = Some(basis);
    let (_, pl) = run(s, "profit_loss_by_fiscal_period", &p).await;
    let mut tied = 0;
    for r in t.rows.iter().filter(|r| r.kind == RowKind::Account) {
        let id = r.path.0.last().cloned().unwrap_or_default();
        let row = pl
            .rows
            .iter()
            .find(|x| x.kind == RowKind::Account && x.path.0.last() == Some(&id));
        for i in 0..12 {
            let want = row.map_or(Decimal::ZERO, |x| dec(&x.cells[i]));
            assert_eq!(
                dec(&r.cells[i * 4]),
                want,
                "BvA {} period {} ({basis:?}) = the P&L",
                r.caption,
                i + 1
            );
            tied += 1;
        }
    }
    assert!(tied >= 36, "the budgeted accounts tie");
}

/// The aging details' lines in a stable order: the engine breaks
/// same-day ties by transaction id, which differs from run to run
/// (Q-P28x), so the lines are sorted and their paths dropped. The balance
/// details carry a running balance through those ties and have no golden;
/// their totals are tied by [`who_owes_what_ties`].
fn stable_order(csv: &str) -> String {
    let mut lines = csv.lines();
    let head = lines.next().unwrap_or("").to_string();
    let mut detail: Vec<String> = Vec::new();
    let mut rest: Vec<String> = Vec::new();
    for l in lines {
        let mut parts: Vec<&str> = l.splitn(4, ',').collect();
        if parts.first() == Some(&"detail") && parts.len() == 4 {
            parts[2] = "";
            detail.push(parts.join(","));
        } else {
            rest.push(l.to_string());
        }
    }
    detail.sort();
    let mut out = head;
    out.push('\n');
    for l in detail.iter().chain(rest.iter()) {
        out.push_str(l);
        out.push('\n');
    }
    out
}

/// The Trial Balance's and the ledger's tag split partition each account
/// (AC5): untagged + tags = the account's total, on debits and credits
/// (the TB shows a net on one side, so it compares debit − credit).
fn split_identity(file: &str, t: &ReportTable) {
    let mut checked = 0;
    for (i, r) in t.rows.iter().enumerate() {
        let (start, cols): (bool, &[usize]) = match (file.starts_with("trial"), r.kind) {
            (true, RowKind::Account) => (true, &[0, 1]),
            (false, RowKind::Subtotal) if r.path.0.last().is_some_and(|p| p == "untagged") => {
                (true, &[5, 6])
            }
            _ => (false, &[]),
        };
        if !start {
            continue;
        }
        let net = |row: &cl_core::report::ReportRow| -> Decimal {
            if cols[0] == 0 {
                dec(&row.cells[0]) - dec(&row.cells[1])
            } else {
                dec(&row.cells[cols[0]]) - dec(&row.cells[cols[1]])
            }
        };
        let mut sum = net(r);
        let mut j = i + 1;
        while t.rows.get(j).is_some_and(|x| x.kind == RowKind::TagRow) {
            sum += net(&t.rows[j]);
            j += 1;
        }
        let Some(total) = t.rows.get(j).filter(|x| x.kind == RowKind::AccountTotal) else {
            panic!("{file}: {} has no split total", r.caption);
        };
        assert_eq!(sum, net(total), "{file}: {} untagged + tags", r.caption);
        checked += 1;
    }
    assert!(checked > 0, "{file}: split rows");
}

/// The WP3 goldens (U-121, U-14 lists): agings, open items and balances
/// as of 11/30 (P2 is dated after it, VC1 still open) and 12/31, and the
/// Q4 lists. The audit trail and Closing Date Exceptions carry the run's
/// own time, so they are cross-checked, not pinned.
fn list_goldens(vendor: cl_core::ids::ContactId, bank: cl_core::ids::AccountId) -> Vec<Golden> {
    let mut v = Vec::new();
    for (name, at) in [
        ("2026-11-30", d(2026, 11, 30)),
        ("2026-12-31", d(2026, 12, 31)),
    ] {
        for key in [
            "ar_aging_summary",
            "ar_aging_detail",
            "ap_aging_summary",
            "ap_aging_detail",
            "open_invoices",
            "unpaid_bills",
            "customer_balance_summary",
            "vendor_balance_summary",
            "bank_balances",
        ] {
            v.push(golden(&format!("{key}__{name}"), key, |o| {
                o.dates = DateChoice::AsOf(at);
            }));
        }
    }
    let q4 = |o: &mut ReportOptions| range(o, d(2026, 10, 1), d(2026, 12, 31));
    for key in [
        "invoice_list",
        "sales_by_client",
        "sales_by_item",
        "expenses_by_vendor",
        "bill_payment_history",
        "deposit_detail",
        "account_activity_summary",
        "duplicate_finder",
    ] {
        v.push(golden(&format!("{key}__2026-q4"), key, q4));
    }
    for key in ["transactions_by_vendor", "vendor_statement"] {
        v.push(golden(
            &format!("{key}__2026-q4_table_rock_nursery"),
            key,
            |o| {
                q4(o);
                o.filters.contact_ids = vec![vendor];
            },
        ));
    }
    v.push(golden(
        "check_register__2026-q4_1010",
        "check_register",
        |o| {
            q4(o);
            o.filters.account_ids = vec![bank];
        },
    ));
    for key in ["vendor_1099_summary", "ap_1099_prep"] {
        v.push(golden(&format!("{key}__2026"), key, |o| {
            range(o, d(2026, 1, 1), d(2026, 12, 31));
        }));
    }
    v
}

/// The grand line's cell in column `key`.
fn grand(t: &ReportTable, key: &str) -> Decimal {
    let i = t
        .columns
        .iter()
        .position(|c| c.key == key)
        .unwrap_or_else(|| panic!("column {key}"));
    dec(&t.rows.last().expect("grand").cells[i])
}

/// Who owes what ties (U-121): on each date the A/R aging, its detail and
/// the client balances give the same A/R balance, and the A/P aging, its
/// detail and the vendor balances the same A/P balance; at 11/30 the open
/// vendor credit VC1 is a negative row.
async fn who_owes_what_ties(s: &Session) {
    for at in [d(2026, 11, 30), d(2026, 12, 31)] {
        let as_of = |key: &'static str| {
            let mut o = ReportOptions::for_spec(find(key).expect("spec"));
            o.dates = DateChoice::AsOf(at);
            (key, o)
        };
        let (k, o) = as_of("ar_aging_summary");
        let ar = grand(&run(s, k, &o).await.1, "total");
        let (k, o) = as_of("ar_aging_detail");
        assert_eq!(grand(&run(s, k, &o).await.1, "open"), ar, "{at} A/R detail");
        let (k, o) = as_of("customer_balance_summary");
        assert_eq!(
            grand(&run(s, k, &o).await.1, "balance"),
            ar,
            "{at} client balances"
        );
        let (k, o) = as_of("customer_balance_detail");
        assert_eq!(
            grand(&run(s, k, &o).await.1, "balance"),
            ar,
            "{at} client balance detail"
        );
        let (k, o) = as_of("ap_aging_summary");
        let ap = grand(&run(s, k, &o).await.1, "total");
        let (k, o) = as_of("vendor_balance_detail");
        assert_eq!(
            grand(&run(s, k, &o).await.1, "balance"),
            ap,
            "{at} vendor balance detail"
        );
        // An independent figure: the A/R and A/P accounts on the Trial
        // Balance (10200 and 20100 in the practice chart).
        let (k, o) = as_of("trial_balance");
        let (_, tb) = run(s, k, &o).await;
        let net = |n: &str| {
            tb.rows
                .iter()
                .find(|r| r.caption.starts_with(&format!("{n} ·")))
                .map(|r| dec(&r.cells[0]) - dec(&r.cells[1]))
                .unwrap_or_default()
        };
        assert_eq!(
            net("10200"),
            ar,
            "{at} A/R aging = A/R on the Trial Balance"
        );
        assert_eq!(
            -net("20100"),
            ap,
            "{at} A/P aging = A/P on the Trial Balance"
        );
        let (k, o) = as_of("ap_aging_detail");
        let (_, detail) = run(s, k, &o).await;
        assert_eq!(grand(&detail, "open"), ap, "{at} A/P detail");
        let (k, o) = as_of("vendor_balance_summary");
        assert_eq!(
            grand(&run(s, k, &o).await.1, "balance"),
            ap,
            "{at} vendor balances"
        );
        if at == d(2026, 11, 30) {
            let open = detail
                .columns
                .iter()
                .position(|c| c.key == "open")
                .expect("open");
            let ty = detail
                .columns
                .iter()
                .position(|c| c.key == "type")
                .expect("type");
            assert!(
                detail.rows.iter().any(|r| {
                    r.cells[ty] == Cell::Text("@reports.txn.vendor_credit".into())
                        && r.cells[open] == Cell::Amount(Decimal::new(-11_560, 2))
                }),
                "VC1 is an open credit on 11/30"
            );
        }
    }
}

/// Closing Date Exceptions (U-204) lists the reclass posted in the closed
/// period with the practice password.
async fn closing_exceptions_list_the_closed_period_entry(s: &Session) {
    let mut o = ReportOptions::for_spec(find("closing_date_exceptions").expect("spec"));
    range(&mut o, d(2000, 1, 1), d(2099, 12, 31));
    let (_, t) = run(s, "closing_date_exceptions", &o).await;
    let after = t
        .columns
        .iter()
        .position(|c| c.key == "date_after")
        .expect("after");
    assert!(
        t.rows
            .iter()
            .any(|r| r.cells[after] == Cell::Date(d(2026, 9, 15))),
        "the closed-period reclass is listed: {:?}",
        t.rows
    );
    // The Audit Trail (audit-filters-v1) reads, lists the story's
    // transactions in plain words, and shows no taxpayer id.
    let mut o = ReportOptions::for_spec(find("audit_trail").expect("spec"));
    range(&mut o, d(2000, 1, 1), d(2099, 12, 31));
    let (_, t) = run(s, "audit_trail", &o).await;
    let record = t
        .columns
        .iter()
        .position(|c| c.key == "record")
        .expect("record");
    assert!(
        t.rows
            .iter()
            .any(|r| r.cells[record] == Cell::Text("@reports.audit.entity.transaction".into())),
        "the audit trail lists transactions"
    );
    assert!(
        !t.rows
            .iter()
            .any(|r| r.caption.to_lowercase().contains("tax_id")
                || r.caption.to_lowercase().contains("taxid")),
        "no taxpayer id in the trail"
    );
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
