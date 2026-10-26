//! Moving P17's single receipt-inbox folder into the company's intake
//! folders (A01 plan D5, Q-A01q): the glue that carries out
//! `cl_settings::migrate_inbox::plan`.
//!
//! Order (each step crash-safe; a crash between steps repeats harmlessly,
//! because the next plan finds the path already in the engine and only
//! confirms it):
//! 1. PUT the engine's list with the new folder (a 409: read it again and
//!    re-plan once);
//! 2. confirm the folder on this computer (auto-confirmed: the owner chose
//!    it on this computer), pinned by its [`super::canonical_key`];
//! 3. move the P17 inbox entries (`folder: None`) to that folder;
//! 4. clear the legacy field.
//!
//! A 403, an engine without `intake-settings-v1`, any engine error, or a
//! folder that can't be reached to pin it (step 2) keeps the legacy field, so the P17 inbox keeps working and the caller
//! shows a Needs attention line. **Nothing in the folder moves**; `Filed/`
//! is untouched.
//!
//! The local side (the company's preferences: the legacy field and this
//! computer's confirmations) is behind [`LocalIntake`], so this crate
//! doesn't name the preference types; the screen wires it to
//! `RecentStore` and `migrate_inbox::plan` (A01 WP-C).

use std::fmt;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use cl_api::{ApiError, ConflictReason, IntakeSettings, LocalValidation, Session};
use cl_core::ids::IntakeFolderId;
use cl_core::intake::IntakeFolder;
use cl_settings::inbox_state::InboxStateStore;

use super::path::canonical_key;
use super::probe::PROBE_DEADLINE;

/// The engine side: the company's intake settings (`Session::intake`).
pub trait IntakeEngine {
    /// The folders and their change token.
    fn get(&self) -> impl Future<Output = Result<IntakeSettings, ApiError>> + Send;

    /// Replaces the list (`expected`: the token last read).
    fn put(
        &self,
        folders: &[IntakeFolder],
        expected: Option<&str>,
    ) -> impl Future<Output = Result<IntakeSettings, ApiError>> + Send;
}

impl IntakeEngine for Session {
    async fn get(&self) -> Result<IntakeSettings, ApiError> {
        self.intake().get().await
    }

    async fn put(
        &self,
        folders: &[IntakeFolder],
        expected: Option<&str>,
    ) -> Result<IntakeSettings, ApiError> {
        self.intake().put(folders, expected).await
    }
}

/// What to do, from `cl_settings::migrate_inbox::plan` (the caller maps
/// its `MigrationPlan` field for field; `blocked` is `plan.blocked.is_some()`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PlannedMigration {
    /// A folder to append to the engine's list (step 1).
    pub add_folder: Option<IntakeFolder>,
    /// The folder to confirm on this computer (step 2).
    pub confirm: Option<IntakeFolderId>,
    /// Clear the legacy field (step 4).
    pub clear_legacy: bool,
    /// Nothing can be done yet (too many folders, a non-UTF-8 path).
    pub blocked: bool,
}

/// The local side: the open company's preferences on this computer.
pub trait LocalIntake {
    /// The P17 folder still set (`receipt_inbox_dir`), if any.
    ///
    /// # Errors
    /// When the preferences can't be read.
    fn legacy_dir(&self) -> Result<Option<PathBuf>, MigrateError>;

    /// The plan for `legacy` against the engine's `engine` folders, with
    /// this computer's confirmations (`migrate_inbox::plan`, a fresh id
    /// minted by the caller).
    fn plan(&self, legacy: &Path, engine: &[IntakeFolder]) -> PlannedMigration;

    /// Saves this computer's confirmation of `folder` (its path as the
    /// engine stores it), pinned by `canonical_key`.
    ///
    /// # Errors
    /// When the preferences can't be written.
    fn confirm(&self, folder: &IntakeFolder, canonical_key: &str) -> Result<(), MigrateError>;

    /// Clears the legacy field.
    ///
    /// # Errors
    /// When the preferences can't be written.
    fn clear_legacy(&self) -> Result<(), MigrateError>;
}

/// Why the migration stopped. The legacy field is kept in every case.
#[derive(Debug, thiserror::Error)]
pub enum MigrateError {
    /// The engine has no `intake-settings-v1`: keep the P17 folder.
    #[error("the engine has no intake folders yet")]
    Unsupported,
    /// The user may not change company settings (403).
    #[error("not allowed to change intake folders")]
    Forbidden,
    /// The list changed again while the migration ran (409 twice).
    #[error("intake folders were changed elsewhere")]
    Stale,
    /// Any other engine error.
    #[error("the engine refused the intake folders")]
    Engine(#[source] ApiError),
    /// The local preferences couldn't be read or written.
    #[error("the company's preferences could not be {0}")]
    Local(&'static str),
}

fn engine_error(e: ApiError) -> MigrateError {
    match e {
        ApiError::Local(LocalValidation::Unsupported) => MigrateError::Unsupported,
        ApiError::Forbidden { .. } => MigrateError::Forbidden,
        ApiError::Conflict {
            reason: ConflictReason::Stale,
            ..
        } => MigrateError::Stale,
        e => MigrateError::Engine(e),
    }
}

/// What the migration did.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MigrationOutcome {
    /// No P17 folder (or nothing left to do).
    Nothing,
    /// The P17 folder can't be moved over yet (the plan said why); the
    /// legacy field is kept.
    Blocked,
    /// Intake folder `folder` holds the P17 folder's path, but the folder
    /// couldn't be reached to pin it: the legacy field and the P17 entries
    /// are kept (the P17 folder keeps working) and the next run tries again.
    Unpinned {
        /// The folder.
        folder: IntakeFolderId,
    },
    /// The P17 folder is now intake folder `folder`.
    Migrated {
        /// The folder.
        folder: IntakeFolderId,
        /// It was added to the engine now (not found there already).
        added: bool,
        /// It was confirmed on this computer now (false when it already
        /// was).
        confirmed: bool,
        /// P17 inbox entries moved to the folder.
        entries: usize,
    },
}

impl fmt::Debug for MigrationOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MigrationOutcome::Nothing => f.write_str("Nothing"),
            MigrationOutcome::Blocked => f.write_str("Blocked"),
            MigrationOutcome::Unpinned { folder } => {
                f.debug_struct("Unpinned").field("folder", folder).finish()
            }
            MigrationOutcome::Migrated {
                folder,
                added,
                confirmed,
                entries,
            } => f
                .debug_struct("Migrated")
                .field("folder", folder)
                .field("added", added)
                .field("confirmed", confirmed)
                .field("entries", entries)
                .finish(),
        }
    }
}

/// The engine's comparison of two paths (case ignored, `/` as `\`).
fn same_path(a: &str, b: &str) -> bool {
    a.replace('/', "\\").to_lowercase() == b.replace('/', "\\").to_lowercase()
}

/// The pin of `root`, computed on its own thread so a dead share never
/// holds the caller (or the runtime's shutdown); `None` past the deadline
/// or when the folder can't be resolved.
async fn pin_off_thread(root: PathBuf, deadline: Duration) -> Option<String> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("cl-intake-pin".into())
        .spawn(move || {
            let _ = tx.send(canonical_key(&root).ok());
        })
        .ok()?;
    let until = Instant::now() + deadline;
    loop {
        match rx.try_recv() {
            Ok(key) => return key,
            Err(mpsc::TryRecvError::Disconnected) => return None,
            Err(mpsc::TryRecvError::Empty) if Instant::now() >= until => return None,
            Err(mpsc::TryRecvError::Empty) => {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }
    }
}

/// Runs the migration of the open company's P17 folder (D5): `engine` is
/// the session, `local` its preferences on this computer, `state` the
/// inbox state and the company's key in it (entries move to the folder).
/// Run it once the session is up and before the first scan.
///
/// # Errors
/// [`MigrateError`]; the legacy field is kept.
pub async fn run_migration(
    engine: &impl IntakeEngine,
    local: &impl LocalIntake,
    state: Option<(&InboxStateStore, &str)>,
) -> Result<MigrationOutcome, MigrateError> {
    run_migration_with(engine, local, state, PROBE_DEADLINE).await
}

/// [`run_migration`] with its own pin deadline (tests).
///
/// # Errors
/// As [`run_migration`].
pub async fn run_migration_with(
    engine: &impl IntakeEngine,
    local: &impl LocalIntake,
    state: Option<(&InboxStateStore, &str)>,
    pin_deadline: Duration,
) -> Result<MigrationOutcome, MigrateError> {
    let Some(legacy) = local.legacy_dir()? else {
        return Ok(MigrationOutcome::Nothing);
    };
    let mut settings = engine.get().await.map_err(engine_error)?;
    let mut plan = local.plan(&legacy, &settings.folders);
    let mut added = false;
    if let Some(add) = plan.add_folder.clone() {
        let mut folders = settings.folders.clone();
        folders.push(add);
        match engine.put(&folders, settings.updated_at.as_deref()).await {
            Ok(s) => {
                settings = s;
                added = true;
            }
            Err(ApiError::Conflict {
                reason: ConflictReason::Stale,
                ..
            }) => {
                // Changed elsewhere meanwhile: read again and re-plan once.
                settings = engine.get().await.map_err(engine_error)?;
                plan = local.plan(&legacy, &settings.folders);
                if let Some(add) = plan.add_folder.clone() {
                    let mut folders = settings.folders.clone();
                    folders.push(add);
                    settings = engine
                        .put(&folders, settings.updated_at.as_deref())
                        .await
                        .map_err(engine_error)?;
                    added = true;
                }
            }
            Err(e) => return Err(engine_error(e)),
        }
    }
    if plan.blocked {
        return Ok(MigrationOutcome::Blocked);
    }
    let Some(legacy_text) = legacy.to_str() else {
        return Ok(MigrationOutcome::Blocked);
    };
    // The folder the legacy path now is in the engine.
    let id = plan
        .add_folder
        .as_ref()
        .map(|f| f.id)
        .or(plan.confirm)
        .or_else(|| {
            settings
                .folders
                .iter()
                .find(|f| same_path(&f.path, legacy_text))
                .map(|f| f.id)
        });
    let Some(id) = id else {
        // Nothing in the engine to move to (an empty legacy value).
        if plan.clear_legacy {
            local.clear_legacy()?;
        }
        return Ok(MigrationOutcome::Nothing);
    };
    let Some(folder) = settings.folders.iter().find(|f| f.id == id).cloned() else {
        // The engine's answer doesn't hold it: try again next time.
        return Err(MigrateError::Stale);
    };
    let mut confirmed = false;
    if plan.confirm == Some(id) {
        let Some(key) = pin_off_thread(PathBuf::from(&folder.path), pin_deadline).await else {
            // The legacy field and the P17 entries stay, so the P17 folder
            // keeps working; the next run pins it (A01 S6).
            tracing::warn!(
                "the receipt inbox folder could not be pinned; it is tried again next time"
            );
            return Ok(MigrationOutcome::Unpinned { folder: id });
        };
        local.confirm(&folder, &key)?;
        confirmed = true;
    }
    let mut entries = 0;
    if let Some((store, company)) = state {
        match store.update(|s| s.assign_legacy_folder(company, id)) {
            Ok(n) => entries = n,
            // The state is a cache: its entries stay with the legacy folder
            // and are offered again at worst.
            Err(e) => tracing::warn!(error = %e, "could not move the receipt inbox entries"),
        }
    }
    if plan.clear_legacy {
        local.clear_legacy()?;
    }
    Ok(MigrationOutcome::Migrated {
        folder: id,
        added,
        confirmed,
        entries,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cl_api::UserText;
    use cl_core::intake::IntakeKind;
    use cl_settings::inbox_state::{FileIdentity, company_key};
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    /// What the fake engine answers next.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Next {
        Ok,
        Stale,
        Forbidden,
        Unsupported,
    }

    #[derive(Default)]
    struct Engine {
        folders: Mutex<Vec<IntakeFolder>>,
        token: Mutex<u32>,
        put_answers: Mutex<Vec<Next>>,
        get_answer: Mutex<Option<Next>>,
        puts: Mutex<u32>,
    }

    fn err(n: Next) -> ApiError {
        match n {
            Next::Stale => ApiError::Conflict {
                reason: ConflictReason::Stale,
                current_version: None,
                code: None,
                message: UserText::default(),
            },
            Next::Forbidden => ApiError::Forbidden { code: None },
            Next::Unsupported | Next::Ok => ApiError::Local(LocalValidation::Unsupported),
        }
    }

    impl Engine {
        fn settings(&self) -> IntakeSettings {
            IntakeSettings {
                folders: self.folders.lock().expect("lock").clone(),
                updated_at: Some(format!("t{}", self.token.lock().expect("lock"))),
            }
        }
    }

    impl IntakeEngine for Engine {
        fn get(&self) -> impl Future<Output = Result<IntakeSettings, ApiError>> + Send {
            let answer = *self.get_answer.lock().expect("lock");
            let s = self.settings();
            async move {
                match answer {
                    Some(n) if n != Next::Ok => Err(err(n)),
                    _ => Ok(s),
                }
            }
        }

        fn put(
            &self,
            folders: &[IntakeFolder],
            expected: Option<&str>,
        ) -> impl Future<Output = Result<IntakeSettings, ApiError>> + Send {
            *self.puts.lock().expect("lock") += 1;
            let answer = {
                let mut a = self.put_answers.lock().expect("lock");
                if a.is_empty() { Next::Ok } else { a.remove(0) }
            };
            let result = if answer == Next::Stale {
                // Someone else saved: the token moves on.
                *self.token.lock().expect("lock") += 1;
                Err(err(Next::Stale))
            } else if answer != Next::Ok {
                Err(err(answer))
            } else if expected != self.settings().updated_at.as_deref() {
                Err(err(Next::Stale))
            } else {
                *self.folders.lock().expect("lock") = folders.to_vec();
                *self.token.lock().expect("lock") += 1;
                Ok(self.settings())
            };
            async move { result }
        }
    }

    /// The local preferences, with a planner following the D5 rules.
    struct Local {
        legacy: Mutex<Option<PathBuf>>,
        here: Mutex<BTreeMap<IntakeFolderId, String>>,
        fresh: IntakeFolderId,
        fail_confirm: Mutex<bool>,
    }

    impl Local {
        fn new(legacy: Option<&Path>) -> Local {
            Local {
                legacy: Mutex::new(legacy.map(Path::to_path_buf)),
                here: Mutex::new(BTreeMap::new()),
                fresh: IntakeFolderId::parse("7f3a2c10-5b6d-4e8f-9a0b-1c2d3e4f5a00").expect("id"),
                fail_confirm: Mutex::new(false),
            }
        }
    }

    impl LocalIntake for Local {
        fn legacy_dir(&self) -> Result<Option<PathBuf>, MigrateError> {
            Ok(self.legacy.lock().expect("lock").clone())
        }

        fn plan(&self, legacy: &Path, engine: &[IntakeFolder]) -> PlannedMigration {
            let text = legacy.to_str().expect("utf-8");
            if let Some(f) = engine.iter().find(|f| same_path(&f.path, text)) {
                let confirmed = self.here.lock().expect("lock").contains_key(&f.id);
                return PlannedMigration {
                    add_folder: None,
                    confirm: (!confirmed).then_some(f.id),
                    clear_legacy: true,
                    blocked: false,
                };
            }
            let taken = engine
                .iter()
                .any(|f| f.name.eq_ignore_ascii_case("Receipts"));
            PlannedMigration {
                add_folder: Some(IntakeFolder {
                    id: self.fresh,
                    name: if taken {
                        "Receipts (this computer)".into()
                    } else {
                        "Receipts".into()
                    },
                    path: text.into(),
                    default_kind: IntakeKind::Auto,
                    auto_read: false,
                    keep_originals: false,
                }),
                confirm: Some(self.fresh),
                clear_legacy: true,
                blocked: false,
            }
        }

        fn confirm(&self, folder: &IntakeFolder, key: &str) -> Result<(), MigrateError> {
            if *self.fail_confirm.lock().expect("lock") {
                return Err(MigrateError::Local("saved"));
            }
            assert_eq!(key.len(), 64);
            self.here
                .lock()
                .expect("lock")
                .insert(folder.id, key.into());
            Ok(())
        }

        fn clear_legacy(&self) -> Result<(), MigrateError> {
            *self.legacy.lock().expect("lock") = None;
            Ok(())
        }
    }

    fn rt() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("runtime")
    }

    fn other(name: &str, path: &str) -> IntakeFolder {
        IntakeFolder {
            id: IntakeFolderId::parse("7f3a2c10-5b6d-4e8f-9a0b-1c2d3e4f5a99").expect("id"),
            name: name.into(),
            path: path.into(),
            default_kind: IntakeKind::Bills,
            auto_read: false,
            keep_originals: false,
        }
    }

    #[test]
    fn no_legacy_folder_is_nothing() {
        let engine = Engine::default();
        let local = Local::new(None);
        let out = rt()
            .block_on(run_migration(&engine, &local, None))
            .expect("ok");
        assert_eq!(out, MigrationOutcome::Nothing);
        assert_eq!(*engine.puts.lock().expect("lock"), 0);
    }

    #[test]
    fn adds_confirms_moves_entries_and_clears() {
        let dir = tempfile::tempdir().expect("tmp");
        let settings = tempfile::tempdir().expect("tmp");
        let store = InboxStateStore::in_dir(settings.path());
        let c = company_key("tenant");
        let f = FileIdentity {
            path_hash: "p".into(),
            size: 1,
            modified_ms: 1,
            sha256: "s".into(),
        };
        store
            .update(|s| s.record_upload(&c, &f, "d0000000-0000-4000-8000-0000000000a1"))
            .expect("save");
        let engine = Engine::default();
        engine
            .folders
            .lock()
            .expect("lock")
            .push(other("Receipts", "/elsewhere"));
        let local = Local::new(Some(dir.path()));
        let out = rt()
            .block_on(run_migration(&engine, &local, Some((&store, &c))))
            .expect("ok");
        assert_eq!(
            out,
            MigrationOutcome::Migrated {
                folder: local.fresh,
                added: true,
                confirmed: true,
                entries: 1,
            }
        );
        let folders = engine.folders.lock().expect("lock").clone();
        assert_eq!(folders.len(), 2);
        assert_eq!(folders[1].name, "Receipts (this computer)", "name taken");
        assert!(local.here.lock().expect("lock").contains_key(&local.fresh));
        assert!(local.legacy.lock().expect("lock").is_none());
        assert_eq!(
            store.load().expect("load").entries(&c)[0].folder,
            Some(local.fresh)
        );
        // Run again: nothing left to do.
        let again = rt().block_on(run_migration(&engine, &local, Some((&store, &c))));
        assert_eq!(again.expect("ok"), MigrationOutcome::Nothing);
        assert_eq!(*engine.puts.lock().expect("lock"), 1);
        assert!(!format!("{out:?}").contains(&dir.path().to_string_lossy().to_string()));
    }

    #[test]
    fn a_409_re_reads_and_re_plans_once() {
        let dir = tempfile::tempdir().expect("tmp");
        let engine = Engine::default();
        engine.put_answers.lock().expect("lock").push(Next::Stale);
        let local = Local::new(Some(dir.path()));
        let out = rt()
            .block_on(run_migration(&engine, &local, None))
            .expect("ok");
        assert!(matches!(
            out,
            MigrationOutcome::Migrated { added: true, .. }
        ));
        assert_eq!(*engine.puts.lock().expect("lock"), 2);
        assert_eq!(engine.folders.lock().expect("lock").len(), 1);

        // Twice in a row: stop, keep the legacy field.
        let engine = Engine::default();
        engine
            .put_answers
            .lock()
            .expect("lock")
            .extend([Next::Stale, Next::Stale]);
        let local = Local::new(Some(dir.path()));
        let e = rt().block_on(run_migration(&engine, &local, None));
        assert!(matches!(e, Err(MigrateError::Stale)));
        assert!(local.legacy.lock().expect("lock").is_some());
        assert!(local.here.lock().expect("lock").is_empty());
    }

    #[test]
    fn forbidden_or_an_old_engine_keeps_the_legacy_field() {
        let dir = tempfile::tempdir().expect("tmp");
        let engine = Engine::default();
        engine
            .put_answers
            .lock()
            .expect("lock")
            .push(Next::Forbidden);
        let local = Local::new(Some(dir.path()));
        let e = rt().block_on(run_migration(&engine, &local, None));
        assert!(matches!(e, Err(MigrateError::Forbidden)));
        assert!(local.legacy.lock().expect("lock").is_some());
        assert!(local.here.lock().expect("lock").is_empty());

        let engine = Engine::default();
        *engine.get_answer.lock().expect("lock") = Some(Next::Unsupported);
        let e = rt().block_on(run_migration(&engine, &local, None));
        assert!(matches!(e, Err(MigrateError::Unsupported)));
        assert!(local.legacy.lock().expect("lock").is_some());
        assert_eq!(*engine.puts.lock().expect("lock"), 0);
    }

    /// A crash after the PUT (the confirmation wasn't saved): the next run
    /// finds the path in the engine and only confirms it, with no second
    /// folder.
    #[test]
    fn a_crash_between_steps_repeats_harmlessly() {
        let dir = tempfile::tempdir().expect("tmp");
        let engine = Engine::default();
        let local = Local::new(Some(dir.path()));
        *local.fail_confirm.lock().expect("lock") = true;
        let e = rt().block_on(run_migration(&engine, &local, None));
        assert!(matches!(e, Err(MigrateError::Local(_))));
        assert_eq!(engine.folders.lock().expect("lock").len(), 1, "step 1 done");
        assert!(local.legacy.lock().expect("lock").is_some(), "kept");
        *local.fail_confirm.lock().expect("lock") = false;
        let out = rt()
            .block_on(run_migration(&engine, &local, None))
            .expect("ok");
        assert_eq!(
            out,
            MigrationOutcome::Migrated {
                folder: local.fresh,
                added: false,
                confirmed: true,
                entries: 0,
            }
        );
        assert_eq!(
            engine.folders.lock().expect("lock").len(),
            1,
            "no second folder"
        );
        assert_eq!(*engine.puts.lock().expect("lock"), 1);
        assert!(local.legacy.lock().expect("lock").is_none());
    }

    /// The legacy folder is gone (an unplugged drive): it is added but
    /// can't be pinned, so the legacy field **stays** (the P17 folder keeps
    /// working) and nothing moves; the next run, with the drive back,
    /// confirms it and only then clears the field (A01 review S6).
    #[test]
    fn an_unreachable_legacy_folder_keeps_the_old_setting_until_pinned() {
        let dir = tempfile::tempdir().expect("tmp");
        let settings = tempfile::tempdir().expect("tmp");
        let store = InboxStateStore::in_dir(settings.path());
        let c = company_key("tenant");
        let f = FileIdentity {
            path_hash: "p".into(),
            size: 1,
            modified_ms: 1,
            sha256: "s".into(),
        };
        store
            .update(|s| s.record_upload(&c, &f, "d0000000-0000-4000-8000-0000000000a1"))
            .expect("save");
        let gone = dir.path().join("unplugged");
        let engine = Engine::default();
        let local = Local::new(Some(&gone));
        let out = rt()
            .block_on(run_migration_with(
                &engine,
                &local,
                Some((&store, &c)),
                Duration::from_secs(5),
            ))
            .expect("ok");
        assert_eq!(
            out,
            MigrationOutcome::Unpinned {
                folder: local.fresh
            }
        );
        assert_eq!(engine.folders.lock().expect("lock").len(), 1, "added");
        assert!(local.here.lock().expect("lock").is_empty());
        assert!(
            local.legacy.lock().expect("lock").is_some(),
            "the P17 setting is kept"
        );
        assert_eq!(
            store.load().expect("load").entries(&c)[0].folder,
            None,
            "the P17 entries stay with the P17 folder"
        );
        assert!(!gone.exists(), "nothing was created");
        // The drive is back.
        std::fs::create_dir_all(&gone).expect("dir");
        let out = rt()
            .block_on(run_migration_with(
                &engine,
                &local,
                Some((&store, &c)),
                Duration::from_secs(5),
            ))
            .expect("ok");
        assert!(matches!(
            out,
            MigrationOutcome::Migrated {
                added: false,
                confirmed: true,
                entries: 1,
                ..
            }
        ));
        assert!(local.legacy.lock().expect("lock").is_none());
    }

    /// The engine's refusal never reaches a log line with its text: every
    /// error reads as fixed words (A01 review, not covered).
    #[test]
    fn errors_read_as_fixed_words() {
        let all = [
            (
                MigrateError::Unsupported,
                "the engine has no intake folders yet",
            ),
            (
                MigrateError::Forbidden,
                "not allowed to change intake folders",
            ),
            (MigrateError::Stale, "intake folders were changed elsewhere"),
            (
                MigrateError::Engine(ApiError::Timeout),
                "the engine refused the intake folders",
            ),
            (
                MigrateError::Local("written"),
                "the company's preferences could not be written",
            ),
        ];
        for (e, want) in all {
            assert_eq!(e.to_string(), want);
        }
    }
}
