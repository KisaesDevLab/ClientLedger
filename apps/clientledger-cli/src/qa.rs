//! `clientledger-cli qa seed --data-dir <dir> --credentials <file>` (P29
//! WP9, Q-PRA2i): a seeded QA company for the phase QA reruns, built only
//! through real engine routes.
//!
//! - The host runs with **file-based secret stores** beside the database
//!   folder (the same encrypted files the app falls back to and then keeps
//!   using), so the owner's OS keychain is never read or written.
//! - The Ozark Ridge practice load (closed through 09/30/2026 with the
//!   practice password), then a tag group and tags, two budgets, a memorized
//!   Builder report, memorized standard reports filtered by a tag, a
//!   mergeable customer pair with an invoice each, a deleted unused tag a
//!   memorized report names, a payroll expense account with activity, and
//!   one user per role, each signed in once to change the temporary
//!   password to a deterministic synthetic one.
//! - Every name, email and password is synthetic. Passwords go only to the
//!   credentials file, never to stdout or a log. Amounts are integer cents
//!   in code and decimal strings on the wire.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cl_api::resources::reports::budgets::{BudgetLineWrite, NewBudget, PERIODS};
use cl_api::resources::reports::definitions::DefinitionWrite;
use cl_api::{
    AccountFields, AccountFilter, AccountType, Basis, Client, ClientConfig, ContactFields,
    ContactType, EndpointSource, EndpointState, EngineEndpoint, IdempotencyKey, InvoiceLine,
    JeLine, LoginOutcome, NewContact, NewInvoice, NewJournalEntry, NewTeamUser, PostingOptions,
    RolePlan, Session, Side, TagFields, TagGroupFields,
};
use cl_core::ids::{AccountId, ContactId, TagId};
use cl_core::{CalendarDate, Money, Quantity, Secret};
use cl_host::{AdminAccount, AppPaths, HostConfig, HostState, Supervisor};
use cl_model::permissions::BuiltinRole;
use cl_model::roles::{EngineKeys, builtin_wire};

use crate::host::EXIT_USAGE;

const HELP: &str = "\
Usage: clientledger-cli qa seed --data-dir <dir> --credentials <file> [--engine-exe <path>]

Builds a synthetic QA company in a new app data folder (P29, Q-PRA2i). The
folder must not exist yet. Sign-in details go only to the credentials file.";

/// The main Admin (the engine's first user, the super admin).
const OWNER_EMAIL: &str = "qa-owner@example.com";
const OWNER_PASSWORD: &str = "QA-owner-pass-2026";

/// The practice company's closing-date password (`sample.rs`).
const CLOSING_PASSWORD: &str = cl_company::sample::SAMPLE_CLOSING_PASSWORD;

/// One person per role. The custom role is made here.
#[derive(Clone, Copy, Debug)]
enum Who {
    Admin,
    Bookkeeper,
    Receivables,
    Payables,
    ViewOnly,
    Custom,
}

const PEOPLE: [(Who, &str, &str, &str); 6] = [
    (
        Who::Admin,
        "qa-admin2@example.com",
        "QA Second Admin",
        "QA-admin2-pass-2026",
    ),
    (
        Who::Bookkeeper,
        "qa-bookkeeper@example.com",
        "QA Bookkeeper",
        "QA-bookkeeper-pass-2026",
    ),
    (
        Who::Receivables,
        "qa-ar@example.com",
        "QA Receivables",
        "QA-receivables-pass-2026",
    ),
    (
        Who::Payables,
        "qa-ap@example.com",
        "QA Payables",
        "QA-payables-pass-2026",
    ),
    (
        Who::ViewOnly,
        "qa-viewonly@example.com",
        "QA View Only",
        "QA-viewonly-pass-2026",
    ),
    (
        Who::Custom,
        "qa-custom@example.com",
        "QA Custom Role",
        "QA-custom-pass-2026",
    ),
];

/// The custom role's template name.
const CUSTOM_ROLE: &str = "QA Custom (bills, no reports)";

#[derive(Debug, PartialEq, Eq)]
struct Options {
    data_dir: PathBuf,
    credentials: PathBuf,
    engine_exe: Option<PathBuf>,
}

fn parse(args: &[String]) -> Result<Option<Options>, String> {
    let mut it = args.iter();
    match it.next().map(String::as_str) {
        Some("seed") => {}
        Some("--help" | "-h" | "help") => return Ok(None),
        Some(other) => return Err(format!("unknown qa command `{other}`")),
        None => return Err("missing qa command".into()),
    }
    let (mut data, mut creds, mut exe) = (None, None, None);
    while let Some(arg) = it.next() {
        let mut value = |name: &str| {
            it.next()
                .map(PathBuf::from)
                .ok_or_else(|| format!("{name} needs a value"))
        };
        match arg.as_str() {
            "--data-dir" => data = Some(value("--data-dir")?),
            "--credentials" => creds = Some(value("--credentials")?),
            "--engine-exe" => exe = Some(value("--engine-exe")?),
            other => return Err(format!("unknown option `{other}`")),
        }
    }
    Ok(Some(Options {
        data_dir: data.ok_or("--data-dir is required")?,
        credentials: creds.ok_or("--credentials is required")?,
        engine_exe: exe,
    }))
}

/// Entry point for `clientledger-cli qa ...`.
pub(crate) fn run(args: &[String]) -> ExitCode {
    let opts = match parse(args) {
        Ok(Some(o)) => o,
        Ok(None) => {
            println!("{HELP}");
            return ExitCode::SUCCESS;
        }
        Err(msg) => {
            eprintln!("error: {msg}\n\n{HELP}");
            return ExitCode::from(EXIT_USAGE);
        }
    };
    match seed(&opts) {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("error: {msg}");
            ExitCode::FAILURE
        }
    }
}

/// cl-api's view of the supervisor (as the app's adapter maps it).
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

fn seed(opts: &Options) -> Result<(), String> {
    let started = Instant::now();
    let data = std::path::absolute(&opts.data_dir).map_err(|e| format!("bad --data-dir: {e}"))?;
    if data.exists() {
        return Err(format!(
            "{} already exists; the QA company is only built in a new folder",
            data.display()
        ));
    }
    let engine = cl_host::find_engine_exe(opts.engine_exe.as_deref()).ok_or_else(|| {
        format!(
            "the engine program ({}) was not found; run `cargo xtask engine` or pass --engine-exe",
            cl_host::engine_exe_name()
        )
    })?;
    let paths = AppPaths::at(data.clone()).map_err(|e| e.to_string())?;
    paths.ensure_dirs().map_err(|e| e.to_string())?;
    let pg_data = paths.pg_data();
    let mut cfg = HostConfig::new(paths.clone(), engine);
    // File stores beside the database folder: the app finds them on its
    // next start and keeps using them; the keychain is never touched.
    cfg.pg_password_store = Some(Arc::new(cl_pg::EncryptedFileStore::beside(&pg_data)));
    cfg.secrets_store = Some(Arc::new(cl_pg::EncryptedFileStore::beside_for_purpose(
        &pg_data,
        "host-master",
    )));
    cfg.policy.ready_timeout = Duration::from_mins(3);
    cfg.policy.health_ready_timeout = Duration::from_mins(2);
    let guard = match cl_host::acquire(&cfg.paths).map_err(|e| e.to_string())? {
        cl_host::Acquired::First(g) => g,
        cl_host::Acquired::Second => return Err("that data folder is in use".into()),
    };
    println!("qa: starting the database and the engine");
    let sup = Arc::new(Supervisor::start(cfg, guard).map_err(|e| format!("start: {e}"))?);
    let outcome = (|| {
        sup.initialize_engine(
            &AdminAccount {
                email: OWNER_EMAIL.into(),
                display_name: "QA Owner".into(),
                password: cl_host::Secret::new(OWNER_PASSWORD.to_string()),
            },
            None,
        )
        .map_err(|e| format!("initialize: {e}"))?;
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        let source: Arc<dyn EndpointSource> = Arc::new(Source(Arc::clone(&sup)));
        let client = Client::new(source, ClientConfig::default()).map_err(|e| e.to_string())?;
        let made = rt.block_on(async {
            tokio::time::timeout(Duration::from_mins(20), build(&client, &paths))
                .await
                .map_err(|_| "the seed ran over 20 minutes".to_string())?
        });
        rt.shutdown_timeout(Duration::from_secs(5));
        made
    })();
    println!("qa: stopping the database and the engine");
    match Arc::try_unwrap(sup) {
        Ok(s) => s.stop(),
        Err(shared) => drop(shared),
    }
    let made = outcome?;
    write_credentials(&opts.credentials, &made)?;
    println!("qa: done in {:.0?}", started.elapsed());
    for line in &made.summary {
        println!("qa:   {line}");
    }
    println!(
        "qa: sign-in details are in {} (git-ignored)",
        opts.credentials.display()
    );
    Ok(())
}

/// What the seed made (passwords only for the credentials file).
struct Made {
    company: String,
    users: Vec<(String, String, &'static str)>,
    summary: Vec<String>,
    notes: Vec<String>,
}

fn write_credentials(file: &Path, made: &Made) -> Result<(), String> {
    let mut text = String::new();
    let _ = writeln!(
        text,
        "# ClientLedger P29 QA company (synthetic). Do not commit.\n\
         # Company: {}\n\
         # Closing date: 09/30/2026, closing-date password: {CLOSING_PASSWORD}\n\
         # Main Admin (recovery key not kept): {OWNER_EMAIL} / {OWNER_PASSWORD}\n",
        made.company
    );
    for (email, password, role) in &made.users {
        let _ = writeln!(text, "{role}: {email} / {password}");
    }
    for n in &made.notes {
        let _ = writeln!(text, "# note: {n}");
    }
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("credentials folder: {e}"))?;
    }
    std::fs::write(file, text).map_err(|e| format!("credentials file: {e}"))
}

/// At most 8 sign-ins in any 62 seconds (the engine allows 10 a minute
/// per address on `/auth/login`).
#[derive(Default)]
struct Pace(VecDeque<Instant>);

impl Pace {
    async fn slot(&mut self) {
        let window = Duration::from_secs(62);
        loop {
            while self.0.front().is_some_and(|t| t.elapsed() >= window) {
                self.0.pop_front();
            }
            if self.0.len() < 8 {
                self.0.push_back(Instant::now());
                return;
            }
            let wait = self
                .0
                .front()
                .map_or(Duration::ZERO, |t| window.saturating_sub(t.elapsed()));
            println!("qa: waiting {wait:.0?} for the engine's sign-in limit");
            tokio::time::sleep(wait + Duration::from_millis(200)).await;
        }
    }
}

fn d(y: i32, m: u32, day: u32) -> Result<CalendarDate, String> {
    CalendarDate::from_ymd(y, m, day).map_err(|e| format!("date: {e:?}"))
}

fn cents(c: i64) -> Result<Money, String> {
    Money::from_cents(c).map_err(|e| format!("amount: {e:?}"))
}

fn key() -> IdempotencyKey {
    IdempotencyKey::new()
}

fn api(what: &'static str) -> impl Fn(cl_api::ApiError) -> String {
    move |e| format!("{what}: {e}")
}

async fn sign_in(
    client: &Client,
    pace: &mut Pace,
    email: &str,
    pw: &str,
) -> Result<Session, String> {
    pace.slot().await;
    match client.login(email, &Secret::new(pw)).await {
        Ok(LoginOutcome::Session(s)) => Ok(s),
        Ok(LoginOutcome::TfaRequired(_)) => Err("sign-in asked for a second step".into()),
        Err(e) => Err(format!("sign-in: {e}")),
    }
}

async fn account_by_number(s: &Session, number: &str) -> Result<Option<AccountId>, String> {
    Ok(s.accounts()
        .list_all(&AccountFilter::default())
        .await
        .map_err(api("accounts"))?
        .into_iter()
        .find(|a| a.fields.account_number.as_deref() == Some(number))
        .map(|a| a.id))
}

async fn first_of_type(s: &Session, t: AccountType) -> Result<AccountId, String> {
    s.accounts()
        .list_all(&AccountFilter::default())
        .await
        .map_err(api("accounts"))?
        .into_iter()
        .find(|a| a.fields.account_type == t && a.is_active)
        .map(|a| a.id)
        .ok_or_else(|| format!("no {t:?} account in the sample"))
}

async fn tag(s: &Session, name: &str, group: cl_core::ids::TagGroupId) -> Result<TagId, String> {
    Ok(s.tags()
        .create(
            &TagFields {
                name: name.into(),
                color: None,
                group_id: Some(group),
                description: None,
                tag_number: None,
            },
            key(),
        )
        .await
        .map_err(api("tag"))?
        .record
        .id)
}

async fn customer(s: &Session, name: &str) -> Result<ContactId, String> {
    Ok(s.contacts()
        .create(
            &NewContact {
                fields: ContactFields::named(ContactType::Customer, name),
                opening_balance: Money::ZERO,
                opening_balance_date: None,
            },
            key(),
        )
        .await
        .map_err(api("customer"))?
        .record
        .id)
}

async fn standard_report(
    s: &Session,
    name: &str,
    report_key: &str,
    tag: TagId,
) -> Result<(), String> {
    let spec = cl_model::reports::find(report_key).ok_or("unknown report")?;
    let mut options = cl_model::reports::ReportOptions::for_spec(spec);
    options
        .filters
        .set_tag_scope(cl_core::report::TagScope::Tag(tag));
    let definition = serde_json::to_value(cl_model::reports::StandardDefinition::new(
        report_key, options,
    ))
    .map_err(|e| format!("report definition: {e}"))?;
    s.report_definitions()
        .create(
            &DefinitionWrite {
                name: name.into(),
                group_id: None,
                kind: "standard".into(),
                definition,
                origin: None,
            },
            key(),
        )
        .await
        .map_err(api("memorized report"))?;
    Ok(())
}

/// The custom role "QA Custom (bills, no reports)": A/P's map with every
/// full right cut to view, then bills and contacts full and reports none.
/// "See sensitive accounts" stays none (from the built-in).
fn custom_role_map(keys: EngineKeys) -> std::collections::BTreeMap<String, String> {
    let mut custom = builtin_wire(BuiltinRole::Payables, keys);
    for v in custom.values_mut() {
        if v == "full" {
            *v = "view".into();
        }
    }
    for k in ["bills", "contacts"] {
        if custom.contains_key(k) {
            custom.insert(k.into(), "full".into());
        }
    }
    if custom.contains_key("reports") {
        custom.insert("reports".into(), "none".into());
    }
    custom
}

#[allow(clippy::too_many_lines, reason = "one linear seed, step by step")]
async fn build(client: &Client, paths: &AppPaths) -> Result<Made, String> {
    let mut pace = Pace::default();
    let mut summary = Vec::new();
    let mut notes = Vec::new();
    client.desktop_info().await.map_err(api("engine info"))?;
    let caps = client.capabilities();
    let s = sign_in(client, &mut pace, OWNER_EMAIL, OWNER_PASSWORD).await?;

    // 1. The practice company (leaves it open; closed through 09/30).
    println!("qa: loading the Ozark Ridge practice company (a few minutes)");
    let store = cl_settings::RecentStore::in_dir(&paths.root().join("settings"));
    cl_company::sample::load_into(
        &s,
        &store,
        &cl_company::backup::silent(),
        &cl_company::Statistics::none(),
    )
    .await
    .map_err(|e| format!("practice company: {e}"))?;
    summary.push("Ozark Ridge practice company, closed through 09/30/2026".into());

    // 2. A tag group and tags made through the engine.
    println!("qa: tags, budgets and memorized reports");
    let group = s
        .tags()
        .create_group(
            &TagGroupFields {
                name: "QA Job sites".into(),
                description: Some("P29 QA".into()),
                is_single_select: true,
            },
            key(),
        )
        .await
        .map_err(api("tag group"))?
        .id;
    let north = tag(&s, "QA North yard", group).await?;
    let south = tag(&s, "QA South yard", group).await?;
    let retired = tag(&s, "QA Retired yard", group).await?;
    summary.push("tag group QA Job sites with 3 tags".into());

    // A payroll expense account with activity (sensitive-accounts QA),
    // tagged so the tag-filtered reports show something. Dated after the
    // closing date.
    let payroll = s
        .accounts()
        .create(
            &AccountFields {
                name: "QA Payroll - Wages".into(),
                account_number: None,
                account_type: AccountType::Expense,
                detail_type: None,
                description: Some("P29 QA: mark sensitive".into()),
                parent_id: None,
            },
            key(),
        )
        .await
        .map_err(api("payroll account"))?
        .record
        .id;
    let bank = match account_by_number(&s, "1010").await? {
        Some(a) => a,
        None => first_of_type(&s, AccountType::Asset).await?,
    };
    for (day, amount, t) in [(9, 245_000, north), (23, 198_050, south)] {
        let mut debit = JeLine {
            account: payroll,
            side: Side::Debit(cents(amount)?),
            description: None,
            tag: Some(t),
            contact: None,
        };
        debit.description = Some("QA payroll".into());
        let entry = NewJournalEntry::new(
            d(2026, 10, day)?,
            Some("QA payroll run".into()),
            Basis::Both,
            vec![
                debit,
                JeLine {
                    account: bank,
                    side: Side::Credit(cents(amount)?),
                    description: None,
                    tag: Some(t),
                    contact: None,
                },
            ],
        )
        .map_err(|e| format!("payroll entry: {e:?}"))?;
        s.transactions()
            .create_journal_entry(&entry, &PostingOptions::default(), key())
            .await
            .map_err(api("payroll entry"))?;
    }
    summary.push("expense account QA Payroll - Wages with 2 entries in 10/2026".into());

    // 3. Two budgets.
    let budgets = s.budgets();
    let b27 = budgets
        .create(
            &NewBudget {
                name: "QA Budget 2027 (from 2026 actuals)".into(),
                fiscal_year: 2027,
            },
            key(),
        )
        .await
        .map_err(api("budget 2027"))?;
    budgets
        .fill_from_actuals(&b27.id, b27.version, None, key())
        .await
        .map_err(api("budget 2027 fill"))?;
    let b26 = budgets
        .create(
            &NewBudget {
                name: "QA Budget 2026".into(),
                fiscal_year: 2026,
            },
            key(),
        )
        .await
        .map_err(api("budget 2026"))?;
    let mut periods = [Money::ZERO; PERIODS];
    for p in &mut periods {
        *p = cents(220_000)?;
    }
    budgets
        .save_lines(
            &b26.id,
            b26.version,
            &[BudgetLineWrite {
                account_id: payroll,
                periods,
            }],
            key(),
        )
        .await
        .map_err(api("budget 2026 lines"))?;
    summary.push("budgets for 2026 (payroll line) and 2027 (filled from 2026 actuals)".into());

    // 4. Memorized reports: a Builder report, standard reports filtered
    // by a tag (one names the tag deleted below).
    let builder = serde_json::json!({
        "schemaVersion": 1, "kind": "builder", "dates": {"mode": "range"},
        "columns": {"detailFields": ["date", "amount"], "compare": [], "headers": {}},
        "nodes": [{
            "type": "section", "id": "payroll", "caption": "Payroll by job site",
            "source": "gl", "measure": "amount", "polarity": "debit",
            "filters": [{"field": "account", "op": "in", "ids": [payroll.to_string()]}],
            "groupBy": [{"key": "tag", "tagGroupId": group.to_string()}],
            "display": "summary", "defaultExpanded": false, "showTotal": true, "hideZero": false
        }]
    });
    s.report_definitions()
        .create(
            &DefinitionWrite {
                name: "QA Payroll by job site (Builder)".into(),
                group_id: None,
                kind: "builder".into(),
                definition: builder,
                origin: None,
            },
            key(),
        )
        .await
        .map_err(api("builder report"))?;
    standard_report(&s, "QA P&L - North yard", "profit_loss", north).await?;
    standard_report(&s, "QA P&L - Retired yard", "profit_loss", retired).await?;
    summary.push(
        "memorized: QA Payroll by job site (Builder), QA P&L - North yard, QA P&L - Retired yard"
            .into(),
    );

    // 5. The unused tag the last report names, deleted.
    match s.tags().delete(retired, key()).await {
        Ok(()) => summary.push("tag QA Retired yard deleted (a memorized report names it)".into()),
        Err(e) => notes.push(format!("deleting QA Retired yard was refused: {e}")),
    }

    // The payroll account marked sensitive (U-33) by the main Admin, after
    // its entries, budget line and memorized report exist. The Admins and
    // (owner 2026-11-17, sensitive-accounts-grant-v1) the Bookkeeper see
    // it; A/R, A/P, View-only and Custom have "See sensitive accounts" at
    // none.
    if caps.has("sensitive-accounts-v1") {
        s.accounts()
            .set_sensitive(payroll, true, key())
            .await
            .map_err(api("payroll sensitive"))?;
        // A note, so credentials.txt says which account it is.
        notes.push(if caps.has("sensitive-accounts-grant-v1") {
            "QA Payroll - Wages is marked SENSITIVE: the Admins and the Bookkeeper see it; the A/R, A/P, View-only and Custom users don't"
                .into()
        } else {
            "QA Payroll - Wages is marked SENSITIVE: only the Admins see it (this engine can't grant it to the Bookkeeper); the Bookkeeper, A/R, A/P, View-only and Custom users don't"
                .into()
        });
    } else {
        notes.push("the engine has no sensitive accounts (sensitive-accounts-v1)".into());
    }

    // 6. A mergeable customer pair, one invoice each (after the closing
    // date).
    println!("qa: customers and invoices");
    let income = match account_by_number(&s, "4000").await? {
        Some(a) => a,
        None => first_of_type(&s, AccountType::Revenue).await?,
    };
    let qty = Quantity::parse_wire("1").map_err(|e| format!("qty: {e:?}"))?;
    for (name, day, amount) in [
        ("QA Harbor Supply", 14, 48_500),
        ("QA Harbor Supply Co.", 21, 31_275),
    ] {
        let c = customer(&s, name).await?;
        let invoice = NewInvoice::new(
            c,
            d(2026, 10, day)?,
            vec![InvoiceLine::account(income, qty, cents(amount)?)],
        );
        s.invoices()
            .create(&invoice, &PostingOptions::default(), key())
            .await
            .map_err(api("invoice"))?;
    }
    summary.push("customers QA Harbor Supply and QA Harbor Supply Co., one invoice each".into());

    // 7. One user per role. Built-in templates by name (as the Users
    // window makes them), plus one custom template.
    println!("qa: users");
    let keys = EngineKeys {
        report_areas: caps.has("report-areas-v1"),
        sensitive: caps.has("sensitive-accounts-v1"),
        sensitive_grant: caps.has("sensitive-accounts-grant-v1"),
    };
    let team = s.team();
    let existing = team.templates().await.map_err(api("templates"))?;
    let template = |name: &str| existing.iter().find(|t| t.name == name).map(|t| t.id);
    let mut ids = std::collections::BTreeMap::new();
    for b in BuiltinRole::ALL {
        let id = match template(b.template_name()) {
            Some(id) => id,
            None => {
                team.create_template(b.template_name(), None, &builtin_wire(*b, keys))
                    .await
                    .map_err(api("role template"))?
                    .id
            }
        };
        ids.insert(b.template_name(), id);
    }
    let custom = custom_role_map(keys);
    let custom_id = match template(CUSTOM_ROLE) {
        Some(id) => id,
        None => {
            team.create_template(CUSTOM_ROLE, Some("P29 QA"), &custom)
                .await
                .map_err(api("custom role"))?
                .id
        }
    };
    let mut users = Vec::new();
    let mut temps = Vec::new();
    for (who, email, name, _) in PEOPLE {
        let plan = match who {
            Who::Admin => RolePlan::Admin,
            Who::Bookkeeper => RolePlan::Template(ids[BuiltinRole::Bookkeeper.template_name()]),
            Who::Receivables => RolePlan::Template(ids[BuiltinRole::Receivables.template_name()]),
            Who::Payables => RolePlan::Template(ids[BuiltinRole::Payables.template_name()]),
            Who::ViewOnly => RolePlan::ViewOnly,
            Who::Custom => RolePlan::Template(custom_id),
        };
        let added = team
            .add(
                &NewTeamUser {
                    email: email.into(),
                    display_name: name.into(),
                },
                plan,
            )
            .await
            .map_err(|f| format!("adding {who:?}: {f:?}"))?;
        temps.push((who, email, added.temporary_password));
    }
    summary.push(format!(
        "users: second Admin, Bookkeeper, A/R, A/P, View-only, Custom ({CUSTOM_ROLE})"
    ));

    // 8. Each signs in once with the temporary password and changes it.
    println!("qa: each user signs in once to change the temporary password");
    for (who, email, temp) in temps {
        let new = PEOPLE.iter().find(|p| p.1 == email).map_or("", |p| p.3);
        let role = match who {
            Who::Admin => "Admin (second)",
            Who::Bookkeeper => "Bookkeeper",
            Who::Receivables => "A/R",
            Who::Payables => "A/P",
            Who::ViewOnly => "View-only",
            Who::Custom => "Custom",
        };
        let Some(temp) = temp else {
            notes.push(format!("{role}: the engine returned no temporary password"));
            continue;
        };
        let us = sign_in(client, &mut pace, email, temp.expose()).await?;
        us.change_password(&temp, &Secret::new(new))
            .await
            .map_err(api("change password"))?;
        let _ = us.logout().await;
        users.push((email.to_string(), new.to_string(), role));
    }
    let _ = s.logout().await;
    Ok(Made {
        company: cl_company::SAMPLE_NAME.into(),
        users,
        summary,
        notes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn seed_needs_both_paths() {
        let o = parse(&args(&[
            "seed",
            "--data-dir",
            "d",
            "--credentials",
            "c.txt",
        ]))
        .expect("ok")
        .expect("seed");
        assert_eq!(o.data_dir, PathBuf::from("d"));
        assert_eq!(o.credentials, PathBuf::from("c.txt"));
        assert!(parse(&args(&["seed", "--data-dir", "d"])).is_err());
        assert!(parse(&args(&["seed", "--bogus"])).is_err());
        assert!(parse(&args(&["wipe"])).is_err());
        assert_eq!(parse(&args(&["--help"])), Ok(None));
    }

    /// The Admins and (owner 2026-11-17) the Bookkeeper see the sensitive
    /// payroll account: no other QA role holds "See sensitive accounts".
    #[test]
    fn only_admins_and_the_bookkeeper_see_sensitive_accounts() {
        let keys = EngineKeys {
            report_areas: true,
            sensitive: true,
            sensitive_grant: true,
        };
        let key = cl_model::roles::SENSITIVE_KEY;
        assert_eq!(
            builtin_wire(BuiltinRole::Bookkeeper, keys)
                .get(key)
                .map(String::as_str),
            Some("view")
        );
        for b in [BuiltinRole::Receivables, BuiltinRole::Payables] {
            assert_eq!(
                builtin_wire(b, keys).get(key).map(String::as_str),
                Some("none"),
                "{b:?}"
            );
        }
        assert_eq!(
            custom_role_map(keys).get(key).map(String::as_str),
            Some("none")
        );
    }

    #[test]
    fn passwords_are_long_enough_and_synthetic() {
        assert!(OWNER_PASSWORD.len() >= 12);
        for (_, email, _, pw) in PEOPLE {
            assert!(pw.len() >= 12, "{email}");
            assert!(email.ends_with("@example.com"));
        }
    }

    #[test]
    fn the_credentials_file_lists_every_user() {
        let dir = std::env::temp_dir().join(format!("cl-qa-creds-{}", std::process::id()));
        let file = dir.join("credentials.txt");
        let made = Made {
            company: "C".into(),
            users: vec![("a@example.com".into(), "pw-123456789012".into(), "A/P")],
            summary: vec![],
            notes: vec!["n".into()],
        };
        write_credentials(&file, &made).expect("write");
        let text = std::fs::read_to_string(&file).expect("read");
        assert!(text.contains("A/P: a@example.com / pw-123456789012"));
        assert!(text.contains(OWNER_EMAIL) && text.contains("# note: n"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
