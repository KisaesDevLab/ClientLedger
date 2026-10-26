//! Intake folders against the real engine (A01 WP-E E4, engine U-93,
//! `target/plans/A01.md` D1 and the section 5 contract rows). One binary,
//! one engine, one fresh cluster; tests run in the order listed in `main`.
//!
//! Plain engine HTTP only (the typed `cl-api` calls plus `raw` requests):
//! no `cl-company` backup flow, no files on disk besides the archives the
//! engine writes itself.
//!
//! Every test is PENDING by `intake-settings-v1` on an engine without it
//! and then pins today's behaviour instead: the typed calls send nothing
//! (`Local(Unsupported)`) and the route is the engine's JSON 404.
//!
//! Run: `cargo xtask engine && cargo xtask pg-fetch`, then
//! `cargo test -p cl-api --test intake_settings` (`CL_API_REQUIRE=1` in CI).

#![allow(clippy::too_many_lines)]

#[path = "support/mod.rs"]
mod support;

use std::time::{Duration, Instant};

use cl_api::{
    ApiError, ArchiveRequest, Capabilities, ConflictReason, ImportRequest, IntakeSettings,
    JobState, LocalValidation, LoginOutcome, Method, Session,
};
use cl_core::Secret;
use cl_core::ids::{IntakeFolderId, TenantId};
use cl_core::intake::{IntakeFolder, IntakeKind};
use serde_json::{Value, json};

use support::{EngineMode, Harness, TestFn, run_suite, unique};

const ROUTE: &str = "/api/v1/company/intake-settings";
/// Host part of every test path: it must never show up in an audit row,
/// a log line or a Debug string.
const CANARY_HOST: &str = "canary-intake-host";
const U93: &str = "U-93";

fn folder_id() -> IntakeFolderId {
    IntakeFolderId::parse(&cl_api::IdempotencyKey::new().as_header()).expect("uuid")
}

fn folder(name: &str, path: &str, kind: IntakeKind) -> IntakeFolder {
    IntakeFolder {
        id: folder_id(),
        name: name.to_string(),
        path: path.to_string(),
        default_kind: kind,
        auto_read: false,
        keep_originals: false,
    }
}

/// Two folders: a UNC share and a mapped drive, both naming the canary.
fn two_folders() -> Vec<IntakeFolder> {
    let mut receipts = folder(
        &unique("Receipts"),
        &format!(r"R:\{CANARY_HOST}\Receipts"),
        IntakeKind::Receipts,
    );
    receipts.keep_originals = true;
    receipts.auto_read = true;
    vec![
        folder(
            &unique("Bills"),
            &format!(r"\\{CANARY_HOST}\Scans\Bills"),
            IntakeKind::Bills,
        ),
        receipts,
    ]
}

/// `true` with `intake-settings-v1`. Without it, records PENDING(U-93) and
/// checks today's behaviour: the typed calls send nothing and the route is
/// the engine's JSON 404.
fn live(h: &Harness, s: &Session) -> bool {
    if h.require_cap(Capabilities::INTAKE_SETTINGS, U93) {
        return true;
    }
    let before = h.observer.sent(Method::Get, ROUTE) + h.observer.sent(Method::Put, ROUTE);
    assert_eq!(
        h.run(s.intake().get()),
        Err(ApiError::Local(LocalValidation::Unsupported))
    );
    assert_eq!(
        h.run(s.intake().put(&two_folders(), None)),
        Err(ApiError::Local(LocalValidation::Unsupported))
    );
    assert_eq!(
        h.observer.sent(Method::Get, ROUTE) + h.observer.sent(Method::Put, ROUTE),
        before,
        "nothing sent without the capability"
    );
    let raw = h.run(s.raw(Method::Get, ROUTE, &[], None)).expect("raw");
    assert_eq!(raw.status, 404, "{}", raw.body);
    assert_eq!(raw.body["error"]["code"], "NOT_FOUND", "{}", raw.body);
    false
}

fn get(h: &Harness, s: &Session) -> IntakeSettings {
    h.run(s.intake().get())
        .unwrap_or_else(|e| panic!("get intake settings: {e:?}"))
}

/// The engine's own folder list in a raw reply, as JSON.
fn raw_folders(h: &Harness, s: &Session) -> (Value, Value) {
    let raw = h
        .run(s.raw(Method::Get, ROUTE, &[], None))
        .expect("raw get");
    assert_eq!(raw.status, 200, "{}", raw.body);
    (raw.body["folders"].clone(), raw.body["updatedAt"].clone())
}

/// Replaces the list, whatever is stored (reads the token first).
fn replace(h: &Harness, s: &Session, folders: &[IntakeFolder]) -> IntakeSettings {
    let now = get(h, s);
    h.run(s.intake().put(folders, now.updated_at.as_deref()))
        .unwrap_or_else(|e| panic!("put intake settings: {e:?}"))
}

/// The issue paths of a 400 (`folders.0.path`, …).
fn issue_paths(body: &Value) -> Vec<String> {
    body["error"]["details"]
        .as_array()
        .map(|issues| {
            issues
                .iter()
                .map(|i| {
                    i["path"]
                        .as_array()
                        .map(|p| {
                            p.iter()
                                .map(|seg| match seg {
                                    Value::String(s) => s.clone(),
                                    other => other.to_string(),
                                })
                                .collect::<Vec<_>>()
                                .join(".")
                        })
                        .unwrap_or_default()
                })
                .collect()
        })
        .unwrap_or_default()
}

// ------------------------------------------------------------------ tests

/// A new company has no folders and no token; a save returns the stored
/// list with a token; the token reads back byte for byte and is accepted
/// back exactly as read; each save moves it.
fn get_put_and_the_token(h: &Harness) {
    let s = &h.s1;
    if !live(h, s) {
        return;
    }
    let fresh = get(h, s);
    assert!(fresh.folders.is_empty(), "{fresh:?}");
    assert_eq!(fresh.updated_at, None, "never saved");
    let (raw, token) = raw_folders(h, s);
    assert_eq!(raw, json!([]));
    assert_eq!(token, Value::Null);

    let folders = two_folders();
    let saved = h
        .run(s.intake().put(&folders, None))
        .expect("first save with a null token");
    assert_eq!(saved.folders, folders, "the stored list comes back");
    let t1 = saved
        .updated_at
        .clone()
        .expect("a token after the first save");

    let read = get(h, s);
    assert_eq!(read.folders, folders);
    assert_eq!(read.updated_at.as_deref(), Some(t1.as_str()));
    let (raw, raw_token) = raw_folders(h, s);
    assert_eq!(
        raw_token.as_str(),
        Some(t1.as_str()),
        "typed and raw reads give the same text"
    );
    assert_eq!(raw[0]["path"], folders[0].path, "path stored as entered");
    assert_eq!(raw[1]["keepOriginals"], true);
    assert_eq!(raw[1]["defaultKind"], "receipts");

    // The opaque token goes back exactly as read.
    let mut changed = folders.clone();
    changed[0].default_kind = IntakeKind::Auto;
    let again = h
        .run(s.intake().put(&changed, Some(&t1)))
        .expect("save with the token just read");
    let t2 = again.updated_at.clone().expect("token");
    assert_ne!(t2, t1, "each save moves the token");
    assert_eq!(get(h, s).folders, changed);

    // Saving the same list again with the newest token is fine too.
    h.run(s.intake().put(&changed, Some(&t2)))
        .expect("an unchanged list saves");
}

/// A save naming an old token is 409 `STALE` with the current token in
/// `details.updatedAt`, and changes nothing.
fn stale_save_changes_nothing(h: &Harness) {
    let s = &h.s1;
    if !live(h, s) {
        return;
    }
    let first = replace(h, s, &two_folders());
    let old = first.updated_at.clone().expect("token");
    let second = h
        .run(s.intake().put(&two_folders(), Some(&old)))
        .expect("save");
    let current = second.updated_at.clone().expect("token");

    let e = h
        .run(s.intake().put(&[], Some(&old)))
        .expect_err("a token from before the last save");
    assert!(
        matches!(
            e,
            ApiError::Conflict {
                reason: ConflictReason::Stale,
                ..
            }
        ),
        "{e:?}"
    );
    let e = h
        .run(s.intake().put(&[], None))
        .expect_err("a null token once the list was saved");
    assert!(
        matches!(
            e,
            ApiError::Conflict {
                reason: ConflictReason::Stale,
                ..
            }
        ),
        "{e:?}"
    );
    let raw = h
        .run(s.raw(
            Method::Put,
            ROUTE,
            &[],
            Some(&json!({"folders": [], "expectedUpdatedAt": old})),
        ))
        .expect("raw");
    assert_eq!(raw.status, 409, "{}", raw.body);
    assert_eq!(raw.body["error"]["code"], "STALE", "{}", raw.body);
    assert_eq!(
        raw.body["error"]["details"]["updatedAt"].as_str(),
        Some(current.as_str()),
        "{}",
        raw.body
    );

    let after = get(h, s);
    assert_eq!(after, second, "nothing changed");
}

/// The engine's folder rules: each refusal is 400 with the field path, and
/// nothing is stored.
fn validation_refuses_and_keeps_the_list(h: &Harness) {
    let s = &h.s1;
    if !live(h, s) {
        return;
    }
    let kept = replace(h, s, &two_folders());
    let token = kept.updated_at.clone().expect("token");
    let ok = |name: &str, path: &str| {
        json!({
            "id": folder_id().to_string(), "name": name, "path": path,
            "defaultKind": "bills", "autoRead": false, "keepOriginals": false,
        })
    };
    let one = |f: Value| json!({"folders": [f], "expectedUpdatedAt": token});
    let two = |a: Value, b: Value| json!({"folders": [a, b], "expectedUpdatedAt": token});
    let mut bad_kind = ok("Bills", r"C:\Scans");
    bad_kind["defaultKind"] = json!("invoices");
    let mut extra_key = ok("Bills", r"C:\Scans");
    extra_key["hash"] = json!("abc");
    let mut bad_id = ok("Bills", r"C:\Scans");
    bad_id["id"] = json!("folder-1");
    let too_many: Vec<Value> = (0..21)
        .map(|i| ok(&format!("Folder {i}"), &format!(r"C:\Scans\{i}")))
        .collect();
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "relative path",
            one(ok("Bills", r"Scans\Bills")),
            "folders.0.path",
        ),
        (
            "device path",
            one(ok("Bills", r"\\?\C:\Scans")),
            "folders.0.path",
        ),
        (
            "device path, dot",
            one(ok("Bills", r"\\.\pipe\x")),
            "folders.0.path",
        ),
        (
            "url",
            one(ok("Bills", "https://example.test/scans")),
            "folders.0.path",
        ),
        (
            "file url",
            one(ok("Bills", "file:///C:/Scans")),
            "folders.0.path",
        ),
        (
            "webdav",
            one(ok("Bills", r"\\server@SSL\DavWWWRoot\scans")),
            "folders.0.path",
        ),
        (
            "dot-dot",
            one(ok("Bills", r"C:\Scans\..\Windows")),
            "folders.0.path",
        ),
        (
            "control char",
            one(ok("Bills", "C:\\Scans\u{7}")),
            "folders.0.path",
        ),
        ("empty path", one(ok("Bills", "")), "folders.0.path"),
        ("empty name", one(ok("   ", r"C:\Scans")), "folders.0.name"),
        (
            "long name",
            one(ok(&"n".repeat(101), r"C:\Scans")),
            "folders.0.name",
        ),
        (
            "long path",
            one(ok("Bills", &format!(r"C:\{}", "p".repeat(1024)))),
            "folders.0.path",
        ),
        (
            "same name ignoring case",
            two(ok("Bills", r"C:\Scans\A"), ok("BILLS", r"C:\Scans\B")),
            "folders.1.name",
        ),
        (
            "same folder, other slashes and case",
            two(ok("A", r"C:\Scans\Bills"), ok("B", "c:/scans/bills")),
            "folders.1.path",
        ),
        ("unknown kind", one(bad_kind), "folders.0.defaultKind"),
        ("not a uuid", one(bad_id), "folders.0.id"),
        ("unknown key", one(extra_key), "folders.0"),
        (
            "21 folders",
            json!({"folders": too_many, "expectedUpdatedAt": token}),
            "folders",
        ),
        ("no token key", json!({"folders": []}), "expectedUpdatedAt"),
    ];
    for (what, body, field) in cases {
        let raw = h
            .run(s.raw(Method::Put, ROUTE, &[], Some(&body)))
            .expect("raw");
        assert_eq!(raw.status, 400, "{what}: {}", raw.body);
        let paths = issue_paths(&raw.body);
        assert!(
            paths
                .iter()
                .any(|p| p == field || p.starts_with(&format!("{field}."))),
            "{what}: expected an issue at {field}, got {paths:?}"
        );
    }
    // The typed call maps a refused folder to Validation.
    let mut bad = two_folders();
    bad[0].path = r"\\?\C:\Scans".into();
    let e = h
        .run(s.intake().put(&bad, Some(&token)))
        .expect_err("device path");
    assert!(matches!(e, ApiError::Validation(_)), "{e:?}");
    // Accepted forms: drive letter with either slash, UNC, Unix.
    let accepted = vec![
        folder("Drive", r"D:\Scans", IntakeKind::Auto),
        folder("Drive slash", "E:/Scans", IntakeKind::Auto),
        folder("Share", r"\\nas\Scans", IntakeKind::Auto),
        folder("Unix", "/srv/scans", IntakeKind::Auto),
    ];
    assert_eq!(get(h, s), kept, "no refused body stored anything");
    let saved = h
        .run(s.intake().put(&accepted, Some(&token)))
        .expect("every accepted form saves");
    assert_eq!(saved.folders, accepted);
}

/// Folders belong to one company: tenant B reads its own (empty) list and
/// its save does not touch Contract Co's.
fn company_scope(h: &Harness) {
    let a = &h.s1;
    if !live(h, a) {
        return;
    }
    let a_list = replace(h, a, &two_folders());
    let b = h.s3();
    let b_before = get(h, &b);
    assert!(
        b_before
            .folders
            .iter()
            .all(|f| a_list.folders.iter().all(|g| g.id != f.id)),
        "tenant B never sees Contract Co's folders"
    );
    let b_list = vec![folder("B only", r"C:\Tenant B\Scans", IntakeKind::Bills)];
    let b_saved = h
        .run(b.intake().put(&b_list, b_before.updated_at.as_deref()))
        .expect("tenant B saves");
    assert_eq!(b_saved.folders, b_list);
    assert_eq!(get(h, a), a_list, "Contract Co unchanged");
    assert_eq!(get(h, &b).folders, b_list);
    // Contract Co's token is not tenant B's.
    let e = h
        .run(a.intake().put(&[], b_saved.updated_at.as_deref()))
        .expect_err("another company's token");
    assert!(
        matches!(
            e,
            ApiError::Conflict {
                reason: ConflictReason::Stale,
                ..
            }
        ),
        "{e:?}"
    );
}

/// A read-only member may read the folders and may not change them (403).
fn readonly_cannot_save(h: &Harness) {
    let s = &h.s1;
    if !live(h, s) {
        return;
    }
    let stored = replace(h, s, &two_folders());
    let email = format!(
        "ro-{}@example.com",
        &cl_api::IdempotencyKey::new().as_header()[..12]
    );
    let made = h
        .run(s.raw(
            Method::Post,
            "/api/v1/company/invite-user",
            &[],
            Some(&json!({
                "email": email,
                "displayName": "Synthetic Read-only",
                "role": "readonly",
            })),
        ))
        .expect("invite");
    assert_eq!(made.status, 201, "{}", made.body);
    let pw = made.body["temporaryPassword"]
        .as_str()
        .expect("temporary password")
        .to_string();
    let ro = match h.run(h.client.login(&email, &Secret::new(pw))) {
        Ok(LoginOutcome::Session(r)) => r,
        other => panic!("read-only login: {other:?}"),
    };
    h.run(ro.open_company(h.contract_co))
        .unwrap_or_else(|e| panic!("read-only opens: {e}"));

    let read = h
        .run(ro.intake().get())
        .expect("company_settings view reads");
    assert_eq!(read, stored);
    let e = h
        .run(ro.intake().put(&[], read.updated_at.as_deref()))
        .expect_err("read-only save");
    assert!(matches!(e, ApiError::Forbidden { .. }), "{e:?}");
    let raw = h
        .run(ro.raw(
            Method::Put,
            ROUTE,
            &[],
            Some(&json!({"folders": [], "expectedUpdatedAt": read.updated_at})),
        ))
        .expect("raw");
    assert_eq!(raw.status, 403, "{}", raw.body);
    assert_eq!(get(h, s), stored, "nothing changed");
    let _ = h.run(ro.logout());
}

/// Polls an archive job to its end.
fn finish_job(h: &Harness, s: &Session, job: &cl_api::JobId) -> cl_api::ArchiveJobStatus {
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let st = h.run(s.tenants().archive_job(job)).expect("job");
        match st.status {
            JobState::Succeeded | JobState::Failed => return st,
            JobState::Queued | JobState::Running => {
                assert!(Instant::now() < deadline, "job did not finish");
                std::thread::sleep(Duration::from_millis(300));
            }
        }
    }
}

/// Exports `co` (from the workspace), imports it as a new company and
/// returns the new tenant. Leaves `s` in the workspace.
fn export_import(h: &Harness, s: &Session, co: TenantId, omit: bool) -> TenantId {
    let passphrase = Secret::new(format!("intake-{}", unique("pass")));
    let job = h
        .run(s.tenants().export_archive(
            co,
            &ArchiveRequest {
                include_attachments: false,
                include_audit: true,
                omit_intake_settings: omit,
                omit_document_drafts: false,
                host: json!({"app": "ClientLedger", "appVersion": "test", "kind": "backup"}),
                passphrase: passphrase.clone(),
            },
        ))
        .expect("export job");
    let st = finish_job(h, s, &job.job_id);
    assert_eq!(st.status, JobState::Succeeded, "{:?}", st.error);
    let path = st
        .export_result()
        .expect("export result")
        .path
        .clone()
        .expect("path");
    let job = h
        .run(s.tenants().import_archive(&ImportRequest {
            path,
            passphrase,
            scratch: false,
            reset_document_drafts: false,
        }))
        .expect("import job");
    let st = finish_job(h, s, &job.job_id);
    assert_eq!(st.status, JobState::Succeeded, "{:?}", st.error);
    st.import_result().expect("import result").tenant_id
}

/// A raw tenant archive keeps the folders through export and import;
/// with `omit_intake_settings` the imported company has none.
fn archive_keeps_folders_unless_omitted(h: &Harness) {
    let s = h.s3();
    let co = h.tenant_b();
    if !live(h, &s) {
        // Without U-93 the omit flag is refused before anything is sent.
        let before = h.observer.total();
        let e = h
            .run(s.tenants().export_archive(
                co,
                &ArchiveRequest {
                    include_attachments: false,
                    include_audit: true,
                    omit_intake_settings: true,
                    omit_document_drafts: false,
                    host: json!({"app": "ClientLedger", "kind": "transfer"}),
                    passphrase: Secret::new("intake-pending-pass"),
                },
            ))
            .expect_err("omit without the capability");
        assert_eq!(e, ApiError::Local(LocalValidation::Unsupported));
        assert_eq!(h.observer.total(), before, "nothing sent");
        return;
    }
    let folders = two_folders();
    let stored = replace(h, &s, &folders);
    h.run(s.close_company()).expect("workspace");

    let kept = export_import(h, &s, co, false);
    h.run(s.open_company(kept)).expect("open restored");
    let restored = get(h, &s);
    assert_eq!(
        restored.folders, folders,
        "ids, names, paths and switches kept"
    );
    assert_eq!(
        restored.updated_at, stored.updated_at,
        "the archive carries the stored value as written"
    );
    // The restored copy can be saved with the token it carries.
    h.run(s.intake().put(&folders, restored.updated_at.as_deref()))
        .expect("save on the restored company");
    h.run(s.close_company()).expect("workspace");

    let omitted = export_import(h, &s, co, true);
    h.run(s.open_company(omitted)).expect("open omitted");
    let none = get(h, &s);
    assert!(none.folders.is_empty(), "{none:?}");
    assert_eq!(none.updated_at, None, "the column default");
    let (raw, _) = raw_folders(h, &s);
    assert_eq!(raw, json!([]));
    h.run(s.close_company()).expect("workspace");
}

/// Audit rows of a change list each folder's id, name, kind and switches,
/// never its path; nothing the client logged names a path either.
fn audit_rows_hold_no_path(h: &Harness) {
    let s = &h.s1;
    if !live(h, s) {
        return;
    }
    let folders = two_folders();
    replace(h, s, &folders);
    let raw = h
        .run(s.raw(
            Method::Get,
            "/api/v1/audit-log",
            &[("entity_type", "company_intake_settings"), ("limit", "100")],
            None,
        ))
        .expect("audit log");
    assert_eq!(raw.status, 200, "{}", raw.body);
    let rows = raw.body["data"].as_array().cloned().unwrap_or_default();
    assert!(!rows.is_empty(), "every save is audited: {}", raw.body);
    for row in &rows {
        let text = row.to_string();
        assert!(
            !text.contains(CANARY_HOST),
            "a path in an audit row: {text}"
        );
        assert!(!text.contains("Scans"), "a path in an audit row: {text}");
        assert!(!text.to_ascii_lowercase().contains("\"path\""), "{text}");
    }
    let newest = rows[0].to_string();
    for f in &folders {
        assert!(
            newest.contains(&f.id.to_string()),
            "folder id audited: {newest}"
        );
    }
    // A search for the host finds nothing either.
    let hit = h
        .run(s.raw(
            Method::Get,
            "/api/v1/audit-log",
            &[("search", CANARY_HOST), ("limit", "10")],
            None,
        ))
        .expect("audit search");
    assert_eq!(hit.status, 200, "{}", hit.body);
    assert_eq!(hit.body["total"], 0, "{}", hit.body);
    // And the client never logged a path or a folder name.
    let logs = h.logs.text();
    assert!(!logs.contains(CANARY_HOST), "a path in the client log");
    for f in &folders {
        assert!(!logs.contains(&f.name), "a folder name in the client log");
    }
}

fn main() -> std::process::ExitCode {
    let tests: &[(&str, TestFn)] = &[
        ("intake::get_put_and_the_token", get_put_and_the_token),
        (
            "intake::stale_save_changes_nothing",
            stale_save_changes_nothing,
        ),
        (
            "intake::validation_refuses_and_keeps_the_list",
            validation_refuses_and_keeps_the_list,
        ),
        ("intake::company_scope", company_scope),
        ("intake::readonly_cannot_save", readonly_cannot_save),
        (
            "intake::archive_keeps_folders_unless_omitted",
            archive_keeps_folders_unless_omitted,
        ),
        ("intake::audit_rows_hold_no_path", audit_rows_hold_no_path),
    ];
    run_suite(EngineMode::Normal, tests)
}
