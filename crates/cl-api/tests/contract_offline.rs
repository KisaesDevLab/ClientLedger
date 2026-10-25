//! Offline and engine-restart contract (P05 plan §2.10, §6, §6.1, AC3, AC5):
//! a second engine and cluster, started with `CL_OFFLINE=on` ("Work
//! offline", E4). Tests run serially in the order below.

#![allow(clippy::too_many_lines)]

#[path = "support/mod.rs"]
mod support;

#[path = "contract/common.rs"]
#[allow(dead_code)]
mod common;

use std::time::{Duration, Instant};

use cl_api::{
    AccountFilter, AccountType, AiConfigUpdate, ApiError, Capabilities, Method, OllamaTarget,
    PageRequest, PostingOptions, SessionSecrets, SmtpTest, TransactionFilter, TxnStatus, TxnType,
};
use cl_core::Secret;

use common::{account_fields, date, je2, key, pair};
use support::{EngineMode, Harness, OllamaStub, TestFn, kill_pid, run_suite, unique};

const TEST_SMTP: &str = "/api/v1/admin/test-smtp";

/// An online-only route answers 503 `OFFLINE_UNAVAILABLE` at once, is mapped
/// to `OfflineUnavailable{feature: "smtp"}`, and is never retried. The SMTP
/// test is super-admin only and needs a company session (the workspace fence
/// blocks `/api/v1/admin`).
fn online_route_is_offline_unavailable_and_not_retried(h: &Harness) {
    let before = h.observer.sent(Method::Post, TEST_SMTP);
    let test = SmtpTest {
        host: "smtp.example.com".into(),
        port: 587,
        username: "mailer".into(),
        password: Some(Secret::new("smtp test password")),
        from: "books@example.com".into(),
        from_name: None,
        test_email: Some("owner@example.com".into()),
    };
    let t0 = Instant::now();
    let e = h.run(h.s1.test_smtp(&test)).expect_err("offline");
    let took = t0.elapsed();
    match &e {
        ApiError::OfflineUnavailable { feature } => assert_eq!(feature.as_deref(), Some("smtp")),
        other => panic!("expected OfflineUnavailable, got {other:?}"),
    }
    assert!(took < Duration::from_secs(2), "took {took:?}");
    assert_eq!(
        h.observer.sent(Method::Post, TEST_SMTP) - before,
        1,
        "OFFLINE_UNAVAILABLE must not be retried"
    );
}

/// Core bookkeeping needs no network: a posting works under `CL_OFFLINE=on`,
/// and `/health` reports the engine offline.
fn core_resources_work_offline(h: &Harness) {
    let s = &h.s1;
    let (cash, supplies) = pair(h, s);
    common::post(
        h,
        s,
        &je2(
            date(2026, 9, 2),
            &unique("offline"),
            supplies.id,
            cash.id,
            "9.9900",
        ),
    );
    let health = h.run(h.client.health()).expect("health");
    assert_eq!(health.online, Some(false), "{health:?}");
}

/// The engine is killed while idle; the supervisor starts generation n+1 on
/// a new port. The next read re-reads the endpoint and succeeds without a
/// new login (sessions are in the database, the JWT secret is stable).
fn reads_resume_after_engine_restart_without_login(h: &Harness) {
    let logins = h.observer.sent(Method::Post, "/api/v1/auth/login");
    let g0 = h.sup.status().generation;
    let g1 = h.kill_engine_and_wait();
    assert!(g1 > g0);
    h.run_for(
        Duration::from_mins(1),
        h.s1.accounts()
            .list(&AccountFilter::default(), PageRequest::first_max()),
    )
    .expect("read after restart");
    assert_eq!(
        h.observer.sent(Method::Post, "/api/v1/auth/login"),
        logins,
        "a restart must not need a new login"
    );
}

/// A write issued while the engine is down waits for the next generation and
/// is then sent once (connect refused proves nothing reached a handler).
fn write_during_restart_waits_and_is_sent_to_the_new_engine(h: &Harness) {
    let pid = h.sup.status().engine_pid.expect("engine pid");
    let g0 = h.sup.status().generation;
    kill_pid(pid);
    let fields = account_fields(AccountType::Expense, "After restart");
    let made = h
        .run_for(
            Duration::from_mins(2),
            h.s1.accounts().create(&fields, key()),
        )
        .expect("write after restart");
    assert_eq!(made.record.fields.name, fields.name);
    assert!(h.sup.status().generation > g0);
    let found = h
        .run(h.s1.accounts().list_all(&AccountFilter {
            search: Some(fields.name.clone()),
            ..AccountFilter::default()
        }))
        .expect("list");
    assert_eq!(found.len(), 1, "exactly one account");
}

/// Adversarial scenario 1 (§6.1): the engine commits a JE and dies before
/// the client sees the answer. The generation moves (the old request is
/// settled), the ledger stamp moved, and the fingerprint search finds the
/// entry: the save is reported as done, with exactly one posting and no
/// second POST. With `idempotency-v1` (U-18) the create is re-sent once
/// with the same key and the engine replays it: still one posting.
fn engine_dies_after_commit_is_reconciled_as_committed(h: &Harness) {
    let s = &h.s1;
    let (cash, supplies) = pair(h, s);
    let memo = unique("died after commit");
    let on = date(2026, 9, 3);
    let entry = je2(on, &memo, supplies.id, cash.id, "31.4159");
    let pid = h.sup.status().engine_pid.expect("engine pid");
    let g0 = h.sup.status().generation;
    let posts = h.observer.sent(Method::Post, "/api/v1/transactions");
    h.observer
        .drop_next_reply_then(Method::Post, "/api/v1/transactions", move || kill_pid(pid));
    let made = h
        .run_for(
            Duration::from_mins(2),
            s.transactions()
                .create_journal_entry(&entry, &PostingOptions::default(), key()),
        )
        .expect("reconciled as committed");
    assert!(
        h.sup.status().generation > g0,
        "the engine was not restarted"
    );
    // With U-18 (`idempotency-v1`) cl-api sends it again with the same key
    // (once more per try that finds the engine still restarting) and the
    // new engine replays the stored answer; without it, never.
    let sent = h.observer.sent(Method::Post, "/api/v1/transactions") - posts;
    if h.caps.has(Capabilities::IDEMPOTENCY) {
        assert!(sent >= 2, "the keyed create is re-sent: {sent}");
    } else {
        assert_eq!(sent, 1, "the create must not be re-sent");
    }
    assert_eq!(made.record.status, TxnStatus::Posted);
    let page = h
        .run(s.transactions().list(
            &TransactionFilter {
                txn_type: Some(TxnType::JournalEntry),
                start_date: Some(on),
                end_date: Some(on),
                search: Some(memo),
                ..TransactionFilter::default()
            },
            PageRequest::first_max(),
        ))
        .expect("list");
    assert_eq!(page.data.len(), 1, "exactly one posting");
}

/// P09: with "Work offline" on, the Ollama test still reaches a loopback
/// stub (nothing external is contacted) and a cloud provider test is
/// `OfflineUnavailable` at once, never retried.
fn ollama_works_offline_and_cloud_is_unavailable(h: &Harness) {
    let s = &h.s1;
    h.run(s.accept_ai_disclosure(key())).expect("disclosure");
    let stub = OllamaStub::start(h, &["llama3.1:8b"]);
    h.run(s.update_ai_config(
        &AiConfigUpdate {
            ollama_base_url: Some(stub.base_url.clone()),
            categorization_model: Some("llama3.1:8b".into()),
            ..AiConfigUpdate::for_provider("ollama", true)
        },
        key(),
    ))
    .expect("update");
    if h.require_cap(Capabilities::AI_DESKTOP, "U-09") {
        // Desktop mode takes the Ollama address from this session only
        // (`ai-providers/index.ts` `resolveCredential`), as the app does.
        h.run(s.put_session_secrets(&SessionSecrets {
            ollama: Some(OllamaTarget {
                base_url: stub.base_url.clone(),
                model: None,
            }),
            ..SessionSecrets::default()
        }))
        .expect("session ollama");
    }
    let r = h.run(s.test_ai_provider("ollama")).expect("ollama test");
    assert!(r.success, "{:?}", r.error);
    assert_eq!(stub.hits(h), 1, "exactly the loopback stub was contacted");

    let before = h
        .observer
        .sent(Method::Post, "/api/v1/ai/admin/test/{provider}");
    let t0 = Instant::now();
    let e = h.run(s.test_ai_provider("anthropic"));
    let took = t0.elapsed();
    match e {
        Err(ApiError::OfflineUnavailable { .. } | ApiError::FeatureNotEnabled { .. }) => {}
        Ok(r) => assert!(!r.success, "offline, a cloud test cannot succeed"),
        Err(other) => panic!("unexpected {other:?}"),
    }
    assert!(took < Duration::from_secs(5), "took {took:?}");
    assert_eq!(
        h.observer
            .sent(Method::Post, "/api/v1/ai/admin/test/{provider}")
            - before,
        1,
        "never retried"
    );
    assert_eq!(stub.hits(h), 1);
    h.run(s.update_ai_config(
        &AiConfigUpdate {
            is_enabled: Some(false),
            ..AiConfigUpdate::default()
        },
        key(),
    ))
    .expect("off");
}

fn main() -> std::process::ExitCode {
    let tests: &[(&str, TestFn)] = &[
        (
            "offline::ollama_works_offline_and_cloud_is_unavailable",
            ollama_works_offline_and_cloud_is_unavailable,
        ),
        (
            "offline::online_route_is_offline_unavailable_and_not_retried",
            online_route_is_offline_unavailable_and_not_retried,
        ),
        (
            "offline::core_resources_work_offline",
            core_resources_work_offline,
        ),
        (
            "restart::reads_resume_after_engine_restart_without_login",
            reads_resume_after_engine_restart_without_login,
        ),
        (
            "restart::write_during_restart_waits_and_is_sent_to_the_new_engine",
            write_during_restart_waits_and_is_sent_to_the_new_engine,
        ),
        (
            "restart::engine_dies_after_commit_is_reconciled_as_committed",
            engine_dies_after_commit_is_reconciled_as_committed,
        ),
    ];
    run_suite(EngineMode::Offline, tests)
}
