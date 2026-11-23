//! Scheduled backups in the desktop app (P34 B1, Q-P34g).
//!
//! One background thread per app run (live, never on an office-network
//! workstation) runs `cl_company::schedule::run_loop` on the computer's
//! clock and time zone. It reads the app's state from [`RunnerInputs`],
//! which the shell keeps current each frame (idle lock) and the maintenance
//! hold sets ([`set_hold`]). The backup itself goes through the request
//! queue as a write, like Back Up Company, so a company switch waits for
//! it and the engine client stays on the queue's runtime.
//!
//! Also here: the backup taken when a company is closed or switched
//! (`backup_on_close`, [`on_close_backup`]).

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::Duration;

use cl_company::schedule::{Job, JobResult, RunnerEnv, RunnerHost};
use cl_core::Timestamp;
use cl_core::ids::TenantId;
use cl_settings::{BackupKind, BackupTrigger, RecentStore, ScheduleRun, SecretStore};

use crate::company::{CompanyEvent, CompanyLink};

static HOLD: AtomicBool = AtomicBool::new(false);

/// Automatic test restores stopped after a passphrase mismatch (review M5).
static AUTO_TESTS_OFF: AtomicBool = AtomicBool::new(false);

/// "Check My Books" in the crash recovery window: a quick check runs
/// after the next company opens, whatever the weekly rule says.
static CHECK_AFTER_OPEN: AtomicBool = AtomicBool::new(false);

/// Ask for a quick check after the next company opens.
pub fn request_check_after_open() {
    CHECK_AFTER_OPEN.store(true, Ordering::SeqCst);
}

/// Takes the request (once).
#[must_use]
pub fn take_check_after_open() -> bool {
    CHECK_AFTER_OPEN.swap(false, Ordering::SeqCst)
}

/// A maintenance hold starts or ends on this computer: scheduled backups
/// wait meanwhile (`with_maintenance` calls this).
pub fn set_hold(on: bool) {
    HOLD.store(on, Ordering::SeqCst);
}

/// True while a maintenance hold is on.
#[must_use]
pub fn hold_on() -> bool {
    HOLD.load(Ordering::SeqCst)
}

/// What the shell tells the scheduler.
#[derive(Debug, Default)]
pub struct RunnerInputs {
    locked: AtomicBool,
    stop: AtomicBool,
    signed_in_since: Mutex<Option<Timestamp>>,
    /// A scheduled backup is being made now (status line).
    running: AtomicBool,
}

impl RunnerInputs {
    /// The idle lock is on or off.
    pub fn set_locked(&self, on: bool) {
        self.locked.store(on, Ordering::Relaxed);
    }

    /// Ends the thread at its next tick (quit).
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    /// True while a scheduled backup runs.
    #[must_use]
    pub fn running(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }

    /// The sign-in the scheduler counts from: set the first time it sees
    /// someone signed in, cleared when nobody is.
    fn signed_in_since(&self, signed_in: bool, now: Timestamp) -> Option<Timestamp> {
        let mut g = self
            .signed_in_since
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        match (signed_in, *g) {
            (false, _) => *g = None,
            (true, None) => *g = Some(now),
            (true, Some(_)) => {}
        }
        *g
    }
}

struct Host {
    link: Weak<CompanyLink>,
    store: RecentStore,
    secrets: Arc<dyn SecretStore>,
    inputs: Arc<RunnerInputs>,
}

impl RunnerHost for Host {
    fn env(&self) -> RunnerEnv {
        let Some(link) = self.link.upgrade() else {
            return RunnerEnv {
                workstation: true,
                ..RunnerEnv::default()
            };
        };
        let now = cl_settings::recent::now();
        RunnerEnv {
            signed_in_since: self.inputs.signed_in_since(link.signed_in(), now),
            workstation: link.is_workstation() || link.is_demo(),
            hold: hold_on(),
            locked: self.inputs.locked.load(Ordering::Relaxed),
        }
    }

    fn session(&self) -> Option<cl_api::Session> {
        self.link.upgrade().and_then(|l| l.session())
    }

    fn store(&self) -> RecentStore {
        self.store.clone()
    }

    fn secrets(&self) -> Arc<dyn SecretStore> {
        Arc::clone(&self.secrets)
    }

    fn recorded(&self, _tenant: TenantId, _run: &ScheduleRun) {
        // Home reads the store when it refreshes; nothing to push.
    }

    fn running(&self, job: Option<&Job>) {
        self.inputs.running.store(job.is_some(), Ordering::Relaxed);
    }

    fn after_backup<'a>(
        &'a self,
        job: &'a Job,
        sha256: &'a str,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            // "Test each backup": the file just written, through the queue.
            let path = self.store.load().ok().and_then(|l| {
                l.get(job.tenant_id)
                    .and_then(|c| c.backup_by_sha256(sha256))
                    .map(|b| b.path.clone())
            });
            let Some(path) = path else {
                return;
            };
            // Review M5: after a passphrase mismatch, automatic tests stop
            // for this run of the app (they would feed the engine's
            // passphrase limiter and could lock out a real restore).
            if AUTO_TESTS_OFF.load(Ordering::SeqCst) {
                return;
            }
            let Some(pending) = self.send_test(path) else {
                return;
            };
            let mut pending = pending;
            loop {
                if let Some(r) = pending.try_take() {
                    match r {
                        Ok(Ok(out))
                            if out.test.reason.as_deref()
                                == Some(cl_company::verify::reason::PASSPHRASE) =>
                        {
                            AUTO_TESTS_OFF.store(true, Ordering::SeqCst);
                            tracing::warn!("automatic test restores stopped: passphrase mismatch");
                        }
                        Ok(Ok(_)) => {}
                        _ => tracing::warn!("scheduled test restore could not run"),
                    }
                    return;
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        })
    }

    fn execute<'a>(
        &'a self,
        job: &'a Job,
        now: Timestamp,
    ) -> Pin<Box<dyn Future<Output = JobResult> + Send + 'a>> {
        Box::pin(async move { send_result(self.send(job, now)).await })
    }
}

/// A test restore's answer through the queue.
type TestAnswer = Result<cl_company::verify::TestOutcome, cl_company::CompanyError>;

impl Host {
    fn send_test(&self, path: std::path::PathBuf) -> Option<cl_api::Pending<TestAnswer>> {
        let link = self.link.upgrade()?;
        let session = link.session()?;
        let client = link.client()?.clone();
        let group = link.group()?.ok()?;
        let store = self.store.clone();
        let secrets = Arc::clone(&self.secrets);
        let label = cl_api::OrphanLabel {
            what: "orphan.lifecycle",
            key: None,
        };
        group
            .write(&client, label, move |_c| async move {
                Ok(cl_company::verify::test_restore(
                    &session,
                    &store,
                    secrets.as_ref(),
                    &path,
                    &cl_company::backup::silent(),
                )
                .await)
            })
            .ok()
    }

    /// `None`: no session or link (signed out since the plan); `Some(Err)`:
    /// the queue refused it.
    fn send(
        &self,
        job: &Job,
        now: Timestamp,
    ) -> Option<Result<cl_api::Pending<JobResult>, cl_api::QueueError>> {
        let link = self.link.upgrade()?;
        let session = link.session()?;
        let client = link.client()?.clone();
        let group = match link.group()? {
            Ok(g) => g,
            Err(e) => return Some(Err(e)),
        };
        let store = self.store.clone();
        let secrets = Arc::clone(&self.secrets);
        let job = job.clone();
        let label = cl_api::OrphanLabel {
            what: "orphan.lifecycle",
            key: None,
        };
        group
            .write(&client, label, move |_c| async move {
                Ok(cl_company::schedule::run_job(
                    &session,
                    &store,
                    secrets.as_ref(),
                    &job,
                    now,
                    &cl_company::backup::silent(),
                )
                .await)
            })
            .map_or_else(|e| Some(Err(e)), |p| Some(Ok(p)))
    }
}

/// What a send became (review M4). Signed out since the plan, or a full
/// queue: the next tick tries again (a sign-out is recorded as a skip
/// then). A closed queue (the app is stopping or the engine link is gone)
/// fails the slot instead of retrying it every tick for ever.
async fn send_result(
    sent: Option<Result<cl_api::Pending<JobResult>, cl_api::QueueError>>,
) -> JobResult {
    match sent {
        None | Some(Err(cl_api::QueueError::QueueFull)) => JobResult::GateBusy,
        Some(Err(_)) => JobResult::Failed {
            code: ENGINE_UNAVAILABLE.into(),
        },
        Some(Ok(pending)) => wait(pending).await,
    }
}

/// The run's code when the request queue is closed.
const ENGINE_UNAVAILABLE: &str = "ENGINE_UNAVAILABLE";

async fn wait(mut pending: cl_api::Pending<JobResult>) -> JobResult {
    loop {
        if let Some(r) = pending.try_take() {
            return r.unwrap_or(JobResult::GateBusy);
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// The weekly quick check in flight (its group keeps it from being
/// cancelled).
pub type WeeklyCheck = (cl_api::RequestGroup, cl_api::Pending<bool>);

/// Q-171 / Q-P34f: after a company opens, for an owner or Admin on an
/// engine with `integrity-v1`, run a quick check in the background when the
/// company's last quick run is 7 or more days old (or "Check My Books" asked
/// for one). Answers `true` when a check ran.
#[must_use]
pub fn weekly_check(link: &Arc<CompanyLink>) -> Option<WeeklyCheck> {
    let force = take_check_after_open();
    if link.is_demo()
        || !link.rights().is_admin()
        || !link.capabilities().has(cl_api::Capabilities::INTEGRITY)
    {
        return None;
    }
    if force {
        // QA-17: the owner asked to check the books, so the answer is
        // shown: Verify Data opens and runs the quick check itself.
        crate::screens::company::verify_data::request_quick_check_on_open();
        link.push(CompanyEvent::OpenWindow(cl_model::CommandId::new(
            crate::screens::company::verify_data::KIND,
        )));
        return None;
    }
    let session = link.session()?;
    let group = link.group()?.ok()?;
    let pending = group
        .read(&session, move |s| async move {
            let summary = s.integrity().summary().await?;
            if cl_company::health::quick_check_due(&summary, cl_settings::recent::now()) {
                let run = s.integrity().check(cl_api::IntegrityLevel::Quick).await?;
                tracing::info!(findings = run.findings.len(), "weekly quick check done");
                Ok(true)
            } else {
                Ok(false)
            }
        })
        .ok()?;
    Some((group, pending))
}

/// Starts the scheduler thread for a live app on this computer (not the
/// demo, not a workstation). `None` when it does not run.
#[must_use]
pub fn start(link: &Arc<CompanyLink>) -> Option<Arc<RunnerInputs>> {
    if link.is_demo() || link.is_workstation() {
        return None;
    }
    let inputs = Arc::new(RunnerInputs::default());
    let host = Arc::new(Host {
        link: Arc::downgrade(link),
        store: link.store().clone(),
        secrets: link.secrets(),
        inputs: Arc::clone(&inputs),
    });
    let stop_inputs = Arc::clone(&inputs);
    let weak = Arc::downgrade(link);
    let spawned = std::thread::Builder::new()
        .name("cl-backup-schedule".into())
        .spawn(move || {
            let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                .enable_time()
                .build()
            else {
                tracing::warn!("scheduled backups could not start");
                return;
            };
            rt.block_on(cl_company::schedule::run_loop(
                Arc::new(cl_company::schedule::SystemClock),
                Arc::new(cl_company::schedule::SystemZone),
                host,
                move || stop_inputs.stop.load(Ordering::Relaxed) || weak.strong_count() == 0,
            ));
        });
    if spawned.is_ok() {
        Some(inputs)
    } else {
        tracing::warn!("scheduled backups could not start");
        None
    }
}

/// The backup taken when the open company is closed or switched
/// (`backup_on_close`): `None` when it does not apply (off, demo,
/// workstation, the practice company, no company). The future never fails
/// the close: a refusal or error is a toast, and the close goes on.
#[must_use]
pub fn on_close_backup(
    link: &Arc<CompanyLink>,
    tenant: TenantId,
) -> Option<Pin<Box<dyn Future<Output = ()> + Send + 'static>>> {
    if link.is_demo() || link.is_workstation() {
        return None;
    }
    let list = link.store().load().ok()?;
    if !list.backup_on_close {
        return None;
    }
    let rec = list.get(tenant)?;
    if rec.is_sample || rec.is_tombstone() {
        return None;
    }
    let dest_dir = list
        .backup_folder
        .clone()
        .unwrap_or_else(|| link.backup_dir.clone());
    let session = link.session()?;
    let store = link.store().clone();
    let secrets = link.secrets();
    let link = Arc::clone(link);
    Some(Box::pin(async move {
        // Review M1: never wait for a running backup or test restore here
        // (the close would wait, or be cut at quit).
        let Some(gate) = cl_company::backup::try_gate() else {
            link.push(CompanyEvent::Toast("backup.on_close.busy"));
            return;
        };
        let r = cl_company::backup::backup_holding(
            gate,
            &session,
            &store,
            secrets.as_ref(),
            &cl_company::BackupRequest {
                tenant_id: tenant,
                kind: BackupKind::Backup,
                dest_dir,
                apply_retention: true,
                protected: None,
            },
            &cl_company::backup::BackupOptions {
                trigger: Some(BackupTrigger::OnClose),
                ..Default::default()
            },
            &cl_company::backup::silent(),
        )
        .await;
        match r {
            Ok(_) => link.push(CompanyEvent::Toast("backup.on_close.done")),
            Err(e) => {
                tracing::warn!(code = %cl_company::schedule::failure_code(&e), "backup on close failed");
                link.push(CompanyEvent::Toast("backup.on_close.failed"));
            }
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Review M4: a full queue or a sign-out retries; a closed queue uses
    /// the slot.
    #[test]
    fn a_refused_send_retries_unless_the_queue_is_closed() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("rt");
        assert_eq!(rt.block_on(send_result(None)), JobResult::GateBusy);
        assert_eq!(
            rt.block_on(send_result(Some(Err(cl_api::QueueError::QueueFull)))),
            JobResult::GateBusy
        );
        assert_eq!(
            rt.block_on(send_result(Some(Err(cl_api::QueueError::QueueClosed)))),
            JobResult::Failed {
                code: ENGINE_UNAVAILABLE.into()
            }
        );
    }

    #[test]
    fn the_sign_in_time_is_kept_until_sign_out() {
        let i = RunnerInputs::default();
        let t1 = Timestamp::from_unix_millis(1_000).expect("ts");
        let t2 = Timestamp::from_unix_millis(2_000).expect("ts");
        assert_eq!(i.signed_in_since(false, t1), None);
        assert_eq!(i.signed_in_since(true, t1), Some(t1));
        assert_eq!(i.signed_in_since(true, t2), Some(t1));
        assert_eq!(i.signed_in_since(false, t2), None);
        assert_eq!(i.signed_in_since(true, t2), Some(t2));
    }

    #[test]
    fn the_demo_has_no_scheduler_and_no_close_backup() {
        let dir = tempfile::tempdir().expect("tmp");
        let link = Arc::new(CompanyLink::demo(dir.path()));
        assert!(start(&link).is_none());
        link.store()
            .update(|l| {
                l.backup_on_close = true;
                l.upsert(
                    TenantId::parse("1b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01").expect("id"),
                    "Ozark",
                );
            })
            .expect("store");
        let t = TenantId::parse("1b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01").expect("id");
        assert!(on_close_backup(&link, t).is_none());
    }

    #[test]
    fn a_hold_is_seen_by_the_scheduler() {
        set_hold(true);
        assert!(hold_on());
        set_hold(false);
        assert!(!hold_on());
    }
}
