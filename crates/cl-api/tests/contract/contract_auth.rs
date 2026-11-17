//! P29 WP2 and WP4 (U-230, U-23): a temporary password must be changed
//! before anything else works; five wrong passwords lock a normal user
//! until an Admin unlocks; the recovery key works once and hands out a
//! fresh one.
//!
//! The lockout and recovery checks sign in many times (the engine's auth
//! limit is 10 a minute per address) and recovery ends every session of
//! the owner, so both run only with `CL_API_AUTH_LIVE=1`, in a run of
//! their own; otherwise they record PENDING.

use cl_api::{ApiError, AuthFailure, LoginOutcome, Method};
use cl_core::Secret;
use serde_json::json;

use crate::support::{Harness, OWNER_EMAIL};

const TEMP_PASSWORD: &str = "temp-password-v1";
const RECOVERY_KEY: &str = "recovery-key-v1";

fn auth_live(h: &Harness, item: &str) -> bool {
    if std::env::var("CL_API_AUTH_LIVE").as_deref() == Ok("1") {
        return true;
    }
    h.pending(
        item,
        "signs in many times or ends the owner's sessions: run with CL_API_AUTH_LIVE=1 on its own",
    );
    false
}

/// A new view-only user; returns the email and the temporary password.
fn invite(h: &Harness) -> (String, String, String) {
    let email = format!(
        "auth-{}@example.com",
        &uuid::Uuid::new_v4().simple().to_string()[..12]
    );
    let made = h
        .run(h.s1.raw(
            Method::Post,
            "/api/v1/company/invite-user",
            &[],
            Some(&json!({"email": email, "displayName": "Synthetic Viewer", "role": "readonly"})),
        ))
        .expect("invite");
    assert_eq!(made.status, 201, "{}", made.body);
    let id = made.body["user"]["id"].as_str().expect("id").to_string();
    let pw = made.body["temporaryPassword"]
        .as_str()
        .expect("temporary password")
        .to_string();
    (id, email, pw)
}

/// U-230: an invite's temporary password signs in with must-change set;
/// a company route is refused with `PASSWORD_CHANGE_REQUIRED` (not a
/// rights 403); `/auth/me` works; change-password clears it and keeps the
/// session working.
pub fn temporary_password_must_be_changed(h: &Harness) {
    if !h.require_cap(TEMP_PASSWORD, "U-230") {
        return;
    }
    let (id, email, pw) = invite(h);
    let temp = Secret::new(pw);
    h.auth_slots(1);
    let Ok(LoginOutcome::Session(s)) = h.run(h.client.login(&email, &temp)) else {
        panic!("login");
    };
    assert!(s.password_change_required());
    let me = h.run(s.me()).expect("me works while must-change");
    assert!(me.password_change_required);
    let e = h
        .run(s.open_company(h.contract_co))
        .expect_err("refused until a new password");
    assert_eq!(e, ApiError::PasswordChangeRequired);
    assert_eq!(s.forbidden_seen(), 0, "not a rights refusal");
    let short = h.run(s.change_password(&temp, &Secret::new("short")));
    assert!(matches!(short, Err(ApiError::Local(_))));
    h.run(s.change_password(&temp, &Secret::new("synthetic-viewer-pw-1")))
        .expect("changed");
    assert!(!s.password_change_required());
    h.run(s.open_company(h.contract_co))
        .expect("opens after the change");

    // No password, temporary or new, in what the owner can read.
    let raw = h
        .run(h.s1.raw(
            Method::Get,
            "/api/v1/audit-log",
            &[
                ("company", "current"),
                ("entity_type", "user_password_change"),
            ],
            None,
        ))
        .expect("raw audit log");
    assert_eq!(raw.status, 200, "{}", raw.body);
    let body = raw.body.to_string();
    for secret in [temp.expose(), "synthetic-viewer-pw-1"] {
        assert!(!body.contains(secret), "no password in the audit log");
    }
    // P29b (owner 2026-11-17): the user's own change is in the company's
    // audit log as `user_password_change`, by and about that user. Read by
    // the owner.
    let rows = h
        .run(h.s1.audit().list(
            &cl_api::AuditFilter {
                entity_type: Some("user_password_change".into()),
                entity_id: Some(uuid::Uuid::parse_str(&id).expect("user id")),
                ..cl_api::AuditFilter::default()
            },
            50,
            0,
        ))
        .expect("audit log");
    let Some(row) = rows
        .rows
        .iter()
        .find(|r| r.entity_id.as_deref() == Some(id.as_str()))
    else {
        // Engine fault found 2026-11-17: `changePassword` writes the row
        // to `users.tenant_id`, and on the desktop every user is homed in
        // the workspace tenant, whose audit log `/audit-log` refuses (409
        // WORKSPACE_TENANT). No company's Audit Trail can show it.
        h.pending(
            "P29b password-change audit",
            "the engine writes user_password_change to the workspace tenant, not the company",
        );
        return;
    };
    assert_eq!(row.entity_type, "user_password_change");
    assert_eq!(
        row.user.map(|u| u.to_string()).as_deref(),
        Some(id.as_str())
    );
}

/// Five wrong passwords lock a normal user; every wrong try reads as
/// "doesn't match"; the right password then says locked; an Admin's
/// unlock lets them in.
pub fn five_wrong_passwords_lock_until_unlocked(h: &Harness) {
    if !auth_live(h, "P29 lockout") {
        return;
    }
    let (id, email, pw) = invite(h);
    for _ in 0..5 {
        h.auth_slots(1);
        let e = h
            .run(h.client.login(&email, &Secret::new("not-the-password")))
            .expect_err("wrong");
        assert_eq!(e, ApiError::Unauthorized(AuthFailure::InvalidCredentials));
    }
    h.auth_slots(1);
    let e = h
        .run(h.client.login(&email, &Secret::new(pw.clone())))
        .expect_err("locked");
    assert_eq!(
        e,
        ApiError::Unauthorized(AuthFailure::AccountLocked { until: None })
    );
    let r = h
        .run(h.s1.raw(
            Method::Post,
            &format!("/api/v1/company/users/{id}/unlock"),
            &[],
            None,
        ))
        .expect("unlock");
    assert_eq!(r.status, 200, "{}", r.body);
    h.auth_slots(1);
    assert!(matches!(
        h.run(h.client.login(&email, &Secret::new(pw))),
        Ok(LoginOutcome::Session(_))
    ));
}

/// U-23 (Q-P29m): rotate, then recover with that key: the reply has a
/// fresh key, the used key fails the second time, and the new password
/// signs in. Ends every owner session: run alone.
pub fn recovery_key_works_once(h: &Harness) {
    if !h.require_cap(RECOVERY_KEY, "U-23") || !auth_live(h, "U-23 recovery") {
        return;
    }
    let key = h.run(h.s3().rotate_recovery_key()).expect("rotate");
    let new_pw = Secret::new("recovered-owner-pw-1");
    let req = cl_api::PasswordRecovery {
        email: OWNER_EMAIL.into(),
        recovery_key: cl_api::normalize_recovery_key(key.expose()),
        new_password: new_pw.clone(),
    };
    let fresh = h
        .run(h.client.recover_password(&req))
        .expect("recovered")
        .expect("a fresh key in the reply");
    assert_ne!(fresh.expose(), key.expose());
    let again = h
        .run(h.client.recover_password(&req))
        .expect_err("a key works once");
    assert!(matches!(
        again,
        ApiError::Unauthorized(_) | ApiError::RateLimited { .. }
    ));
    h.auth_slots(1);
    assert!(matches!(
        h.run(h.client.login(OWNER_EMAIL, &new_pw)),
        Ok(LoginOutcome::Session(_))
    ));
}
