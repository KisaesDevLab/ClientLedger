//! Logging canary (AC5): tokens, passwords, amounts, names, emails and memo
//! text never appear in captured `tracing` output, in host log files, or in
//! an error's `Display`/`Debug`. Also drives one write through the UI queue
//! (AC6).

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cl_api::{
    AccountType, ApiQueue, ContactFields, ContactType, Method, NewContact, OrphanLabel, QueueConfig,
};
use serde_json::json;

use crate::common::{account_fields, date, je2, key, m, pair, post};
use crate::support::{Harness, OWNER_EMAIL};

const NAME: &str = "Zebulon Canaryworth";
const MEMO: &str = "quokka canary memo";
const AMOUNT: &str = "8642.1357";
const EMAIL: &str = "zebulon.canary@example.com";

fn log_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            log_files(&p, out);
        } else if p
            .file_name()
            .is_some_and(|n| n.to_string_lossy().contains(".log"))
        {
            out.push(p);
        }
    }
}

pub fn canary_never_reaches_logs_or_error_text(h: &Harness) {
    let s = &h.s1;
    let mut fields = ContactFields::named(ContactType::Customer, NAME);
    fields.email = Some(EMAIL.into());
    h.run(s.contacts().create(
        &NewContact {
            fields,
            opening_balance: m("0.0000"),
            opening_balance_date: None,
        },
        key(),
    ))
    .expect("contact");
    let (cash, supplies) = pair(h, s);
    post(
        h,
        s,
        &je2(date(2026, 9, 1), MEMO, supplies.id, cash.id, AMOUNT),
    );
    // An engine message that carries amounts ("debits (x) != credits (y)").
    let bad = json!({
        "txnType": "journal_entry",
        "txnDate": "2026-09-01",
        "memo": MEMO,
        "lines": [
            {"accountId": supplies.id, "debit": "9753.1100", "credit": "0"},
            {"accountId": cash.id, "debit": "0", "credit": "9752.1100"},
        ],
    });
    let e = h
        .run(s.raw_mapped(Method::Post, "/api/v1/transactions", Some(&bad)))
        .expect_err("unbalanced");
    let shown = format!("{e} {e:?}");
    // A refresh, so rotated tokens pass through the code paths too.
    h.run(s.force_refresh()).expect("refresh");

    let access = s.access_token_for_test();
    let cookie = s.refresh_cookie_for_test().expect("cookie");
    let secrets = [h.password.expose(), access.expose(), cookie.expose()];
    let personal = [NAME, MEMO, EMAIL, OWNER_EMAIL, AMOUNT, "9753.11", "9752.11"];
    for needle in personal.iter().chain(secrets.iter()) {
        assert!(!shown.contains(needle), "error text leaks a canary");
    }
    let logs = h.logs.text();
    assert!(logs.contains("engine attempt"), "nothing was captured");
    for needle in personal.iter().chain(secrets.iter()) {
        assert!(!logs.contains(needle), "tracing output leaks a canary");
    }
    let mut files = Vec::new();
    log_files(&h.data_dir, &mut files);
    for f in files {
        let text = std::fs::read(&f).map(|b| String::from_utf8_lossy(&b).into_owned());
        let Ok(text) = text else { continue };
        for needle in &secrets {
            assert!(!text.contains(needle), "a secret reached {}", f.display());
        }
    }
}

pub fn a_write_through_the_ui_queue(h: &Harness) {
    let queue = ApiQueue::start(Arc::new(|| {}), QueueConfig::default()).expect("queue");
    let group = queue.group().expect("group");
    let fields = account_fields(AccountType::Expense, "Queued");
    let name = fields.name.clone();
    let k = key();
    let mut pending = group
        .write(
            &h.s1,
            OrphanLabel {
                what: "test.save",
                key: Some(k.clone()),
            },
            move |s| async move { s.accounts().create(&fields, k).await },
        )
        .expect("queued");
    let deadline = Instant::now() + Duration::from_secs(20);
    let made = loop {
        if let Some(r) = pending.try_take() {
            break r.expect("created through the queue");
        }
        assert!(Instant::now() < deadline, "queue result did not arrive");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(made.record.fields.name, name);
    let report = queue.shutdown(Duration::from_secs(5), None);
    assert_eq!(report.abandoned.len(), 0);
}
