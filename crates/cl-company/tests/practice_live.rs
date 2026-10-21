//! The practice company against the real engine and the bundled
//! `PostgreSQL` (P15 Acceptance 2, Q-P15r): `load_into` seeds the sample's
//! templates (`templates.json`) through `templates-v1`, and the first
//! catch-up run posts the automatic card charge's missed dates (09/15 and
//! 10/15, no closing date) exactly once each, as the demo does.
//!
//! Without the engine executable or the `PostgreSQL` binaries the test
//! prints `SKIPPED` and passes, unless `CL_API_REQUIRE=1` (CI's engine job).
//! Only loopback traffic; a scratch data folder under the temp dir; the
//! supervisor (and so the engine and `PostgreSQL` it started) is stopped at
//! the end, also when an assertion fails.
#![allow(
    clippy::too_many_lines,
    clippy::similar_names,
    reason = "one linear scenario; `as_of` and `after` read as the demo's words"
)]

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use cl_api::{
    AccountFilter, Client, ClientConfig, EndpointSource, EndpointState, EngineEndpoint,
    LoginOutcome, Session, new_run_id,
};
use cl_core::ids::AccountId;
use cl_core::{CalendarDate, Money, Secret};
use cl_host::{AdminAccount, HostConfig, HostState, Supervisor};
use cl_settings::RecentStore;

const OWNER: &str = "owner@example.com";

fn require() -> bool {
    std::env::var("CL_API_REQUIRE").as_deref() == Ok("1")
}

fn skip(why: &str) {
    assert!(!require(), "CL_API_REQUIRE=1 but {why}");
    eprintln!("SKIPPED: {why} (CI's engine job sets CL_API_REQUIRE=1)");
}

fn engine_exe() -> Option<PathBuf> {
    if let Some(p) = cl_host::find_engine_exe(None) {
        return Some(p);
    }
    let exe = std::env::current_exe().ok()?;
    let target = exe
        .ancestors()
        .find(|d| d.file_name().is_some_and(|n| n == "target"))?
        .to_path_buf();
    std::fs::read_dir(target.join("engine"))
        .ok()?
        .flatten()
        .map(|e| e.path().join(cl_host::engine_exe_name()))
        .find(|p| p.is_file())
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

/// Today (UTC): never after the engine's `CURRENT_DATE` + 1.
fn today() -> CalendarDate {
    let days = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() / 86_400);
    CalendarDate::from_ymd(1970, 1, 1)
        .and_then(|d| d.add_days(i64::try_from(days).expect("days")))
        .expect("today")
}

fn d(y: i32, m: u32, day: u32) -> CalendarDate {
    CalendarDate::from_ymd(y, m, day).expect("date")
}

fn m(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}

async fn account(s: &Session, number: &str) -> AccountId {
    s.accounts()
        .list_all(&AccountFilter::default())
        .await
        .expect("accounts")
        .into_iter()
        .find(|a| a.fields.account_number.as_deref() == Some(number))
        .map_or_else(|| panic!("account {number}"), |a| a.id)
}

async fn balance(s: &Session, id: AccountId) -> Money {
    s.accounts()
        .get(id)
        .await
        .expect("account")
        .record
        .balance
        .unwrap_or_default()
}

/// Run-due to the end of `more`, one run id; the posts of the whole run.
async fn run_due(s: &Session, as_of: CalendarDate) -> cl_api::RunDue {
    let id = new_run_id();
    let mut last = s.recurring().run_due(as_of, id, 200).await.expect("run");
    while last.more {
        last = s.recurring().run_due(as_of, id, 200).await.expect("more");
    }
    last
}

async fn scenario(client: &Client, password: &Secret, scratch: &std::path::Path) {
    let s = match client.login(OWNER, password).await {
        Ok(LoginOutcome::Session(s)) => s,
        other => panic!("login: {other:?}"),
    };
    assert!(
        client.capabilities().has(cl_api::Capabilities::TEMPLATES)
            && client
                .capabilities()
                .has(cl_api::Capabilities::RECURRING_RUN_DUE),
        "the pinned engine has templates-v1 and recurring-run-due-v1"
    );
    let store = RecentStore::in_dir(&scratch.join("settings"));
    let created = cl_company::sample::load_into(&s, &store, &cl_company::backup::silent())
        .await
        .expect("the practice company loads");
    // load_into leaves the new company open in the session.
    let _ = created;

    // The templates and their group, as templates.json has them.
    let mut templates = s.templates().list_all().await.expect("templates");
    templates.sort_by(|a, b| a.name.cmp(&b.name));
    let names: Vec<&str> = templates.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(
        names,
        ["Accounting software", "Owner health insurance", "Shop rent"]
    );
    let groups = s.templates().groups().await.expect("groups");
    assert_eq!(
        groups.iter().map(|g| g.name.as_str()).collect::<Vec<_>>(),
        ["Monthly bills"]
    );
    let in_group: Vec<&str> = templates
        .iter()
        .filter(|t| t.group_name.as_deref() == Some("Monthly bills"))
        .map(|t| t.name.as_str())
        .collect();
    assert_eq!(in_group, ["Accounting software", "Shop rent"]);
    let software_t = templates
        .iter()
        .find(|t| t.name == "Accounting software")
        .expect("card charge");
    let sched = software_t.schedule.as_ref().expect("scheduled");
    assert_eq!(sched.mode, cl_api::ScheduleMode::Auto);
    assert_eq!(sched.next_date, d(2026, 9, 15));
    assert_eq!(
        software_t.total.map(Money::to_wire).as_deref(),
        Some("49.0000")
    );
    let rent_t = templates
        .iter()
        .find(|t| t.name == "Shop rent")
        .expect("rent");
    assert_eq!(
        rent_t.schedule.as_ref().map(|s| s.mode),
        Some(cl_api::ScheduleMode::Remind)
    );
    let je_t = templates
        .iter()
        .find(|t| t.name == "Owner health insurance")
        .expect("je");
    assert!(je_t.schedule.is_none(), "Don't remind me");

    // The first catch-up, as the app runs it on open: the clock is today,
    // or the story date once that has passed (both give 09/15 and 10/15).
    let story = d(2026, 11, 4);
    let as_of = today().min(story);
    assert!(
        as_of >= d(2026, 10, 15),
        "the clock is past the second date"
    );
    let card = account(&s, "2050").await;
    let software = account(&s, "6250").await;
    let before = (balance(&s, card).await, balance(&s, software).await);
    let first = run_due(&s, as_of).await;
    let mut posted: Vec<CalendarDate> = first
        .posted
        .iter()
        .filter(|p| p.template == software_t.id)
        .map(|p| p.date)
        .collect();
    posted.sort();
    assert_eq!(
        posted,
        [d(2026, 9, 15), d(2026, 10, 15)],
        "both missed dates"
    );
    assert!(
        first.posted.iter().all(|p| p.template == software_t.id),
        "only the automatic template posts"
    );
    assert!(
        !first.skipped.iter().any(|k| k.template == software_t.id),
        "no closing date: nothing skipped"
    );
    assert!(
        first.reminders.iter().all(|r| r.template != software_t.id),
        "an automatic template is not a reminder"
    );
    let after = (balance(&s, card).await, balance(&s, software).await);
    assert_eq!(
        after.1.checked_sub(before.1),
        Ok(m("98.00")),
        "software +98.00 exactly"
    );
    assert_eq!(
        before.0.checked_sub(after.0).map(Money::abs),
        Ok(m("98.00")),
        "the card moves by 98.00 exactly"
    );
    // Again (a second open): nothing more.
    let again = run_due(&s, as_of).await;
    assert!(
        again.posted.iter().all(|p| p.template != software_t.id),
        "each date once"
    );
    assert_eq!(
        (balance(&s, card).await, balance(&s, software).await),
        after
    );
    let _ = s.logout().await;
}

#[test]
fn practice_company_templates_load_and_catch_up_posts_each_missed_date_once() {
    let Some(engine) = engine_exe() else {
        return skip("the engine executable is missing (run `cargo xtask engine`)");
    };
    let Ok(pg) = cl_pg::PgBinaries::discover(None) else {
        return skip("the PostgreSQL binaries are missing (run `cargo xtask pg-fetch`)");
    };
    let root = std::env::temp_dir().join(format!("cl-company-practice-{}", std::process::id()));
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
                display_name: "Practice Owner".into(),
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
                tokio::time::timeout(Duration::from_mins(5), async {
                    client.desktop_info().await.expect("info");
                    scenario(&client, &password, &root).await;
                })
                .await
                .expect("within five minutes");
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
