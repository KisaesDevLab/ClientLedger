//! Puts the built-in roles' "See sensitive accounts" right back after an
//! Admin signs in (owner 2026-11-17, Q-P29ac).
//!
//! An engine with `sensitive-accounts-grant-v1` lets the built-in
//! Bookkeeper carry `sensitive_accounts: view`. Companies made before keep
//! the old template until it is sent again. Users and Roles does that when
//! an Admin opens it (Q-P29v); this does the same once per company open,
//! so the Bookkeeper sees the accounts without anyone opening that window:
//!
//! - only for an Admin, on an engine with the capability, never in the
//!   demo (nothing is read otherwise, so a non-Admin draws no refusal);
//! - only a template with a built-in's exact name whose map is the
//!   built-in table's apart from the `sensitive_accounts` key
//!   ([`cl_model::roles::sensitive_putback`]): a custom role, a copy or a
//!   hand-edited map is never touched, and a missing built-in is left for
//!   Users and Roles to create;
//! - idempotent: once sent the shape no longer matches, so later opens do
//!   nothing; a failure is logged (counts only, no engine text) and the
//!   next Admin sign-in tries again. Never an error dialog.

use std::sync::Arc;

use cl_api::{ApiError, OrphanLabel, PermissionMap, RoleTemplate, Session};
use cl_model::permissions::BuiltinRole;
use cl_model::roles::{EngineKeys, sensitive_putback};
use uuid::Uuid;

use super::CompanyLink;
use crate::screens::users::{REPORT_AREAS, SENSITIVE_ACCOUNTS, SENSITIVE_GRANT};

/// Which optional role keys the open company's engine knows (the one
/// source for Users and Roles and this put-back, so they never disagree).
#[must_use]
pub fn engine_keys(link: &CompanyLink) -> EngineKeys {
    let caps = link.capabilities();
    EngineKeys {
        report_areas: link.is_demo() || caps.has(REPORT_AREAS),
        sensitive: caps.has(SENSITIVE_ACCOUNTS),
        sensitive_grant: caps.has(SENSITIVE_GRANT),
    }
}

/// The role templates (the session; a fake in tests).
pub trait Templates {
    /// Every template.
    fn list(&self)
    -> impl std::future::Future<Output = Result<Vec<RoleTemplate>, ApiError>> + Send;
    /// Replaces one template's map (name unchanged), read at `stamp` (its
    /// `updatedAt`, sent back as `expectedUpdatedAt`, U-291).
    fn put(
        &self,
        id: Uuid,
        stamp: Option<cl_api::RecordStamp>,
        name: &str,
        map: &PermissionMap,
    ) -> impl std::future::Future<Output = Result<(), ApiError>> + Send;
}

impl Templates for Session {
    async fn list(&self) -> Result<Vec<RoleTemplate>, ApiError> {
        self.team().templates().await
    }

    async fn put(
        &self,
        id: Uuid,
        stamp: Option<cl_api::RecordStamp>,
        name: &str,
        map: &PermissionMap,
    ) -> Result<(), ApiError> {
        self.team()
            .update_template(id, stamp, name, map)
            .await
            .map(drop)
    }
}

/// True when this open should look at the templates at all.
#[must_use]
pub fn should_run(demo: bool, admin: bool, keys: EngineKeys) -> bool {
    !demo && admin && keys.sensitive_grant
}

/// Reads the templates and sends each old-shape built-in back with the
/// right. Returns how many were sent.
///
/// # Errors
///
/// The first read or write the engine did not take (the rest wait for the
/// next sign-in).
pub async fn put_back(t: &impl Templates, keys: EngineKeys) -> Result<usize, ApiError> {
    let have = t.list().await?;
    let mut sent = 0;
    for b in BuiltinRole::ALL {
        let Some(tpl) = have.iter().find(|x| x.name == b.template_name()) else {
            continue;
        };
        if let Some(map) = sensitive_putback(*b, &tpl.permissions, keys) {
            t.put(tpl.id, tpl.stamp, &tpl.name, &map).await?;
            sent += 1;
        }
    }
    Ok(sent)
}

/// Starts the put-back for the company just opened (the shell calls it on
/// `Opened`). Fire and forget: a write on the queue that always ends Ok, so
/// nothing reaches the owner as an error.
pub fn start(link: &Arc<CompanyLink>) {
    let keys = engine_keys(link);
    if !should_run(link.is_demo(), link.rights().is_admin(), keys) {
        return;
    }
    let (Some(session), Some(Ok(group))) = (link.session(), link.group()) else {
        return;
    };
    let label = OrphanLabel {
        what: "orphan.team",
        key: None,
    };
    let started = group.write(&session, label, move |s| async move {
        match put_back(&s, keys).await {
            Ok(0) => {}
            Ok(n) => tracing::info!(templates = n, "built-in roles given the sensitive right"),
            // No engine text in the log (it may name a role or person).
            Err(_) => {
                tracing::warn!(
                    "built-in roles not updated now; the next Admin sign-in tries again"
                );
            }
        }
        Ok(())
    });
    if started.is_err() {
        tracing::warn!("built-in roles not updated now; the next Admin sign-in tries again");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cl_model::roles::builtin_wire;
    use std::sync::Mutex;

    const KEYS: EngineKeys = EngineKeys {
        report_areas: true,
        sensitive: true,
        sensitive_grant: true,
    };
    const OLD: EngineKeys = EngineKeys {
        report_areas: true,
        sensitive: true,
        sensitive_grant: false,
    };

    /// Answers with `have`; records every call.
    struct Fake {
        have: Vec<RoleTemplate>,
        lists: Mutex<usize>,
        puts: Mutex<Vec<(Uuid, String, PermissionMap)>>,
    }

    impl Fake {
        fn new(have: Vec<RoleTemplate>) -> Fake {
            Fake {
                have,
                lists: Mutex::new(0),
                puts: Mutex::new(Vec::new()),
            }
        }
        fn puts(&self) -> Vec<(Uuid, String, PermissionMap)> {
            self.puts.lock().map(|p| p.clone()).unwrap_or_default()
        }
    }

    impl Templates for Fake {
        fn list(
            &self,
        ) -> impl std::future::Future<Output = Result<Vec<RoleTemplate>, ApiError>> + Send {
            if let Ok(mut n) = self.lists.lock() {
                *n += 1;
            }
            let have = self.have.clone();
            async move { Ok(have) }
        }
        fn put(
            &self,
            id: Uuid,
            _stamp: Option<cl_api::RecordStamp>,
            name: &str,
            map: &PermissionMap,
        ) -> impl std::future::Future<Output = Result<(), ApiError>> + Send {
            if let Ok(mut p) = self.puts.lock() {
                p.push((id, name.to_owned(), map.clone()));
            }
            async { Ok(()) }
        }
    }

    fn tpl(n: u128, name: &str, map: PermissionMap) -> RoleTemplate {
        RoleTemplate {
            id: Uuid::from_u128(n),
            name: name.to_owned(),
            description: None,
            permissions: map,
            stamp: None,
        }
    }

    fn run(f: &Fake, keys: EngineKeys) -> usize {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime")
            .block_on(put_back(f, keys))
            .expect("put back")
    }

    fn old_bookkeeper() -> RoleTemplate {
        tpl(
            1,
            BuiltinRole::Bookkeeper.template_name(),
            builtin_wire(BuiltinRole::Bookkeeper, OLD),
        )
    }

    #[test]
    fn an_old_shape_bookkeeper_gets_the_right_once() {
        let f = Fake::new(vec![
            old_bookkeeper(),
            tpl(
                2,
                BuiltinRole::Payables.template_name(),
                builtin_wire(BuiltinRole::Payables, KEYS),
            ),
        ]);
        assert_eq!(run(&f, KEYS), 1);
        let want = builtin_wire(BuiltinRole::Bookkeeper, KEYS);
        assert_eq!(
            f.puts(),
            vec![(
                Uuid::from_u128(1),
                BuiltinRole::Bookkeeper.template_name().to_owned(),
                want.clone()
            )]
        );
        // The next sign-in reads the updated template: nothing to send.
        let again = Fake::new(vec![tpl(1, BuiltinRole::Bookkeeper.template_name(), want)]);
        assert_eq!(run(&again, KEYS), 0);
        assert_eq!(again.puts().len(), 0, "nothing sent");
    }

    #[test]
    fn a_differing_template_and_a_copy_are_left_alone() {
        let mut edited = builtin_wire(BuiltinRole::Bookkeeper, OLD);
        edited.insert("banking".into(), "view".into());
        let f = Fake::new(vec![
            tpl(1, BuiltinRole::Bookkeeper.template_name(), edited),
            // A copy the owner made of the old Bookkeeper.
            tpl(
                3,
                "Bookkeeper (copy)",
                builtin_wire(BuiltinRole::Bookkeeper, OLD),
            ),
        ]);
        assert_eq!(run(&f, KEYS), 0);
        assert_eq!(f.puts().len(), 0, "nothing sent");
    }

    #[test]
    fn only_an_admin_on_an_engine_with_the_grant_runs_it() {
        assert!(should_run(false, true, KEYS));
        assert!(!should_run(false, false, KEYS), "a non-Admin");
        assert!(!should_run(false, true, OLD), "an engine without the grant");
        assert!(!should_run(true, true, KEYS), "the demo");
        // Without the capability nothing is sent even if called.
        let f = Fake::new(vec![old_bookkeeper()]);
        assert_eq!(run(&f, OLD), 0);
        assert_eq!(f.puts().len(), 0, "nothing sent");
    }
}
