//! The demo's sign-in, password and recovery rules (P29 WP2, WP4), checked
//! the way the engine does (`auth.service.ts`): a wrong password always
//! reads as "doesn't match" and counts toward the lock; "locked" and
//! "turned off" are told only after the right password; five wrong tries
//! lock a user until an Admin unlocks them. Every value here is synthetic.

use cl_api::{ApiError, AuthFailure, password_fits};
use cl_core::Secret;

/// Wrong tries before a user is locked (the engine's `MAX_LOGIN_ATTEMPTS`).
pub const MAX_TRIES: u8 = 5;

/// The demo owner's sign-in (synthetic).
pub const OWNER_EMAIL: &str = "owner@example.com";
/// The demo owner's password (synthetic).
pub const OWNER_PASSWORD: &str = "demo-password-1";
/// The demo recovery key at start (synthetic).
pub const RECOVERY_KEY: &str = super::DEMO_RECOVERY_KEY;
/// The key a demo recovery hands out next (synthetic).
pub const NEXT_RECOVERY_KEY: &str = "NEW2K-EY567-ABCDE-FGHJK-LMNPQ";

/// One demo user.
#[derive(Clone)]
pub struct DemoUser {
    /// Sign-in email.
    pub email: String,
    /// Name shown in the app.
    pub display_name: String,
    /// The engine's role word.
    pub role: String,
    /// Turned on.
    pub active: bool,
    /// Wrong tries since the last good sign-in.
    pub failed: u8,
    /// Locked until an Admin unlocks.
    pub locked: bool,
    /// Signed in with a temporary password: must choose a new one.
    pub must_change: bool,
    /// The main Admin (the desktop's first user).
    pub super_admin: bool,
    /// The user's id (synthetic).
    pub id: cl_core::ids::UserId,
    /// The last sign-in (ISO 8601).
    pub last_sign_in: Option<String>,
    /// The permission row: None without one, else its template.
    pub row: Option<Option<uuid::Uuid>>,
    password: String,
}

impl std::fmt::Debug for DemoUser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DemoUser")
            .field("role", &self.role)
            .field("active", &self.active)
            .field("locked", &self.locked)
            .finish_non_exhaustive()
    }
}

impl DemoUser {
    fn new(email: &str, name: &str, role: &str, password: &str) -> DemoUser {
        DemoUser {
            email: email.into(),
            display_name: name.into(),
            role: role.into(),
            active: true,
            failed: 0,
            locked: false,
            must_change: false,
            super_admin: false,
            id: demo_user_id(0),
            last_sign_in: None,
            row: None,
            password: password.into(),
        }
    }
}

impl DemoUser {
    /// A user an Admin just added: view-only with a temporary password.
    pub(super) fn invited(email: &str, name: &str, temporary: &str) -> DemoUser {
        let mut u = DemoUser::new(email, name, "readonly", temporary);
        u.must_change = true;
        u
    }

    /// Sets the password (a temporary one from an Admin).
    pub(super) fn set_password(&mut self, password: &str) {
        password.clone_into(&mut self.password);
    }
}

/// What a good demo sign-in found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DemoSignedIn {
    /// Name shown in the status bar.
    pub display_name: String,
    /// Must choose a new password first.
    pub must_change: bool,
}

/// The demo's users and its recovery key.
#[derive(Clone, Debug)]
pub struct DemoAuth {
    /// Everyone on the demo team.
    pub users: Vec<DemoUser>,
    /// Who is signed in (`None` after Sign out).
    pub signed_in: Option<String>,
    /// The company's role templates (none at start: the Users window
    /// creates the built-ins, Q-P29v).
    pub templates: Vec<cl_api::RoleTemplate>,
    /// Tests and scenarios: the team step that fails next (once).
    pub fail_at: Option<cl_api::TeamStep>,
    recovery_key: String,
}

impl Default for DemoAuth {
    fn default() -> Self {
        let mut owner = DemoUser::new(OWNER_EMAIL, "Pat Owner", "owner", OWNER_PASSWORD);
        owner.super_admin = true;
        let mut temp = DemoUser::new(
            "lee@example.com",
            "Lee Bookkeeper",
            "bookkeeper",
            "TEMP-0000-DEMO-1",
        );
        temp.must_change = true;
        let mut locked = DemoUser::new(
            "kim@example.com",
            "Kim Payables",
            "bookkeeper",
            "kim-password-1",
        );
        locked.locked = true;
        locked.failed = MAX_TRIES;
        let mut off = DemoUser::new(
            "alex@example.com",
            "Alex Viewer",
            "readonly",
            "alex-password-1",
        );
        off.active = false;
        owner.last_sign_in = Some("2026-11-04T14:05:00Z".into());
        temp.last_sign_in = Some("2026-10-28T16:40:00Z".into());
        let mut users = vec![owner, temp, locked, off];
        for (i, u) in users.iter_mut().enumerate() {
            u.id = demo_user_id(i);
        }
        DemoAuth {
            users,
            signed_in: Some(OWNER_EMAIL.into()),
            templates: Vec::new(),
            fail_at: None,
            recovery_key: RECOVERY_KEY.into(),
        }
    }
}

/// The demo team's ids (synthetic): user `n` of the list, from 1.
#[must_use]
pub fn demo_user_id(n: usize) -> cl_core::ids::UserId {
    let n = u128::try_from(n).unwrap_or(0) + 1;
    cl_core::ids::UserId::from_uuid(uuid::Uuid::from_u128(
        0x5a1e_0000_0000_4000_8000_0000_0000_0100 + n,
    ))
}

fn same_email(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

impl DemoAuth {
    fn find(&mut self, email: &str) -> Option<&mut DemoUser> {
        self.users.iter_mut().find(|u| same_email(&u.email, email))
    }

    /// The signed-in user.
    #[must_use]
    pub fn current(&self) -> Option<&DemoUser> {
        let email = self.signed_in.as_deref()?;
        self.users.iter().find(|u| same_email(&u.email, email))
    }

    /// Checks a password the engine's way, without signing in (also the
    /// idle unlock).
    ///
    /// # Errors
    /// `Unauthorized(InvalidCredentials)` for an unknown email or a wrong
    /// password (counted; the fifth locks); `Unauthorized(Deactivated)` or
    /// `Unauthorized(AccountLocked)` only after the right password.
    pub fn check(&mut self, email: &str, password: &Secret) -> Result<DemoSignedIn, ApiError> {
        let bad = ApiError::Unauthorized(AuthFailure::InvalidCredentials);
        let Some(user) = self.find(email) else {
            return Err(bad);
        };
        if user.password != password.expose() {
            user.failed = user.failed.saturating_add(1);
            if user.failed >= MAX_TRIES {
                user.locked = true;
            }
            return Err(bad);
        }
        if !user.active {
            return Err(ApiError::Unauthorized(AuthFailure::Deactivated));
        }
        if user.locked {
            return Err(ApiError::Unauthorized(AuthFailure::AccountLocked {
                until: None,
            }));
        }
        user.failed = 0;
        Ok(DemoSignedIn {
            display_name: user.display_name.clone(),
            must_change: user.must_change,
        })
    }

    /// Signs in.
    ///
    /// # Errors
    /// As [`DemoAuth::check`].
    pub fn sign_in(&mut self, email: &str, password: &Secret) -> Result<DemoSignedIn, ApiError> {
        let ok = self.check(email, password)?;
        self.signed_in = Some(email.trim().to_string());
        Ok(ok)
    }

    /// The signed-in user changes their own password (clears must-change).
    ///
    /// # Errors
    /// `Local(InvalidValue)` for a password that doesn't fit;
    /// `Unauthorized(SessionExpired)` when nobody is signed in; `Validation`
    /// shaped like the engine's `INVALID_CURRENT_PASSWORD` and
    /// `PASSWORD_UNCHANGED`.
    pub fn change_password(&mut self, current: &Secret, new: &Secret) -> Result<(), ApiError> {
        if current.is_empty() || !password_fits(new) {
            return Err(ApiError::Local(cl_api::LocalValidation::InvalidValue));
        }
        let email = self
            .signed_in
            .clone()
            .ok_or(ApiError::Unauthorized(AuthFailure::SessionExpired))?;
        let user = self
            .find(&email)
            .ok_or(ApiError::Unauthorized(AuthFailure::SessionExpired))?;
        if user.password != current.expose() {
            return Err(validation("INVALID_CURRENT_PASSWORD"));
        }
        if user.password == new.expose() {
            return Err(validation("PASSWORD_UNCHANGED"));
        }
        user.password = new.expose().to_string();
        user.must_change = false;
        Ok(())
    }

    /// Recovery with the key (Q-P29m): the key is used up and a fresh one
    /// comes back; the lock and must-change are cleared.
    ///
    /// # Errors
    /// `Unauthorized(InvalidCredentials)` for a wrong key or unknown email
    /// (one message for both); `Local(InvalidValue)` for a password that
    /// doesn't fit.
    pub fn recover(
        &mut self,
        email: &str,
        key: &Secret,
        new: &Secret,
    ) -> Result<Option<Secret>, ApiError> {
        if !password_fits(new) || key.is_empty() || email.trim().is_empty() {
            return Err(ApiError::Local(cl_api::LocalValidation::InvalidValue));
        }
        let expected = cl_api::normalize_recovery_key(&self.recovery_key);
        let bad = ApiError::Unauthorized(AuthFailure::InvalidCredentials);
        if key.expose() != expected.expose() {
            return Err(bad);
        }
        let Some(user) = self.find(email).filter(|u| u.super_admin) else {
            return Err(bad);
        };
        user.password = new.expose().to_string();
        user.failed = 0;
        user.locked = false;
        user.must_change = false;
        let fresh = if self.recovery_key == NEXT_RECOVERY_KEY {
            RECOVERY_KEY
        } else {
            NEXT_RECOVERY_KEY
        };
        self.recovery_key = fresh.to_string();
        Ok(Some(Secret::new(fresh)))
    }

    /// "Make a new recovery key": the old one stops working.
    pub fn rotate(&mut self) -> Secret {
        let fresh = if self.recovery_key == NEXT_RECOVERY_KEY {
            RECOVERY_KEY
        } else {
            NEXT_RECOVERY_KEY
        };
        self.recovery_key = fresh.to_string();
        Secret::new(fresh)
    }

    /// How many users are turned on (auto sign-in needs exactly one).
    #[must_use]
    pub fn active_users(&self) -> usize {
        self.users.iter().filter(|u| u.active).count()
    }
}

/// A 400 refusal with an engine code, as cl-api maps one.
fn validation(code: &str) -> ApiError {
    cl_api::engine_refusal(
        400,
        &serde_json::json!({"error": {"code": code, "message": "refused"}}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(x: &str) -> Secret {
        Secret::new(x)
    }

    #[test]
    fn wrong_passwords_lock_on_the_fifth_and_lock_shows_only_after_the_right_one() {
        let mut a = DemoAuth::default();
        for _ in 0..4 {
            assert_eq!(
                a.check(OWNER_EMAIL, &s("nope")),
                Err(ApiError::Unauthorized(AuthFailure::InvalidCredentials))
            );
        }
        // A good password before the fifth wrong one resets the count.
        assert!(a.check(OWNER_EMAIL, &s(OWNER_PASSWORD)).is_ok());
        for _ in 0..5 {
            assert_eq!(
                a.check(OWNER_EMAIL, &s("nope")),
                Err(ApiError::Unauthorized(AuthFailure::InvalidCredentials)),
                "the locking try still reads as doesn't match"
            );
        }
        assert_eq!(
            a.check(OWNER_EMAIL, &s(OWNER_PASSWORD)),
            Err(ApiError::Unauthorized(AuthFailure::AccountLocked {
                until: None
            }))
        );
        assert_eq!(
            a.check("alex@example.com", &s("alex-password-1")),
            Err(ApiError::Unauthorized(AuthFailure::Deactivated))
        );
        assert_eq!(
            a.check("nobody@example.com", &s("x")),
            Err(ApiError::Unauthorized(AuthFailure::InvalidCredentials))
        );
    }

    #[test]
    fn a_temporary_password_must_be_changed_and_recovery_clears_the_lock() {
        let mut a = DemoAuth::default();
        let lee = a
            .sign_in("lee@example.com", &s("TEMP-0000-DEMO-1"))
            .expect("in");
        assert!(lee.must_change);
        assert!(
            a.change_password(&s("TEMP-0000-DEMO-1"), &s("short"))
                .is_err()
        );
        assert!(
            a.change_password(&s("wrong"), &s("a-long-new-password"))
                .is_err()
        );
        a.change_password(&s("TEMP-0000-DEMO-1"), &s("a-long-new-password"))
            .expect("changed");
        assert!(
            !a.check("lee@example.com", &s("a-long-new-password"))
                .expect("ok")
                .must_change
        );

        for _ in 0..5 {
            let _ = a.check(OWNER_EMAIL, &s("nope"));
        }
        assert!(
            a.recover(OWNER_EMAIL, &s("WRONG"), &s("recovered-password-1"))
                .is_err()
        );
        assert!(
            a.recover(
                "lee@example.com",
                &cl_api::normalize_recovery_key(RECOVERY_KEY),
                &s("recovered-password-1")
            )
            .is_err(),
            "only the main Admin has the key"
        );
        let fresh = a
            .recover(
                OWNER_EMAIL,
                &cl_api::normalize_recovery_key(RECOVERY_KEY),
                &s("recovered-password-1"),
            )
            .expect("recovered")
            .expect("fresh key");
        assert_eq!(fresh.expose(), NEXT_RECOVERY_KEY);
        assert!(a.check(OWNER_EMAIL, &s("recovered-password-1")).is_ok());
        assert!(
            a.recover(
                OWNER_EMAIL,
                &cl_api::normalize_recovery_key(RECOVERY_KEY),
                &s("again-password-12")
            )
            .is_err(),
            "a key works once"
        );
    }
}
