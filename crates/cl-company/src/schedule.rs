//! Scheduled backups (P34, Q-42, Q-P34g, Q-P34h).
//!
//! Three parts, so the rules run in simulated time:
//! - **Pure rules:** [`latest_slot`], [`next_slot`] and [`due`] work on a
//!   [`LocalZone`] (wall-clock times; a DST gap moves a slot to the next
//!   valid minute, an overlap takes the earlier instant).
//! - **[`Runner`]:** a state machine fed the time and the app's state
//!   ([`RunnerEnv`]); it decides which company to back up now, which slot
//!   to record as skipped, and when to wait. It never writes the store on
//!   an idle tick.
//! - **[`run_job`] and [`run_loop`]:** the I/O. The loop ticks every
//!   [`TICK`] on an injected [`Clock`]; a job takes [`crate::backup::GATE`]
//!   with `try_lock` (busy means "next tick") and writes a verified backup
//!   with the schedule's own retention.
//!
//! Logs carry codes and counts only, never a folder or a company name.

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use chrono::{Datelike, NaiveDate, NaiveDateTime, NaiveTime, TimeZone};
use cl_api::Session;
use cl_core::Timestamp;
use cl_core::ids::TenantId;
use cl_settings::{
    BackupKind, BackupSchedule, BackupTrigger, Frequency, RecentCompanies, RecentStore, RunOutcome,
    ScheduleRun, SecretStore,
};

use crate::backup::{BackupOptions, BackupRequest, GATE, ProgressSink};
use crate::error::CompanyError;

/// How often the runner looks.
pub const TICK: Duration = Duration::from_secs(30);
/// A missed slot runs this long after sign-in (Q-P34h).
pub const CATCH_UP_DELAY_MS: i64 = 2 * 60 * 1000;
/// A busy engine (`TENANT_BUSY`) is tried this many times, one tick apart,
/// before the slot is recorded as failed.
pub const BUSY_TRIES: u8 = 3;
/// A last run further than this in the future (the clock went back) counts
/// as never (Q-P34h).
const FUTURE_SLACK_MS: i64 = 24 * 60 * 60 * 1000;
/// How far back a slot is looked for (a monthly slot is at most 31 days
/// back; a margin for DST).
const LOOK_BACK_DAYS: i64 = 40;

// ---------------------------------------------------------------- time

/// The time source (system or simulated).
pub trait Clock: Send + Sync {
    /// Now.
    fn now(&self) -> Timestamp;
    /// Waits `d` (a simulated clock moves its time instead).
    fn sleep(&self, d: Duration) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;
}

/// The computer's clock.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        cl_settings::recent::now()
    }

    fn sleep(&self, d: Duration) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(tokio::time::sleep(d))
    }
}

/// A wall-clock time resolved to instants.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resolved {
    /// The time does not exist (a DST gap).
    Gap,
    /// One instant.
    One(Timestamp),
    /// Two instants (a DST overlap), earlier first.
    Two(Timestamp, Timestamp),
}

/// The local time zone.
pub trait LocalZone: Send + Sync {
    /// The local wall-clock time of an instant.
    fn to_local(&self, t: Timestamp) -> NaiveDateTime;
    /// The instants a local wall-clock time names.
    fn from_local(&self, local: NaiveDateTime) -> Resolved;
}

/// The computer's time zone.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemZone;

fn ts(dt: chrono::DateTime<chrono::Utc>) -> Option<Timestamp> {
    Timestamp::from_unix_millis(dt.timestamp_millis()).ok()
}

impl LocalZone for SystemZone {
    fn to_local(&self, t: Timestamp) -> NaiveDateTime {
        t.as_utc().with_timezone(&chrono::Local).naive_local()
    }

    fn from_local(&self, local: NaiveDateTime) -> Resolved {
        let utc = |d: chrono::DateTime<chrono::Local>| ts(d.with_timezone(&chrono::Utc));
        match chrono::Local.from_local_datetime(&local) {
            chrono::LocalResult::None => Resolved::Gap,
            chrono::LocalResult::Single(d) => utc(d).map_or(Resolved::Gap, Resolved::One),
            chrono::LocalResult::Ambiguous(a, b) => match (utc(a), utc(b)) {
                (Some(a), Some(b)) => Resolved::Two(a.min(b), a.max(b)),
                _ => Resolved::Gap,
            },
        }
    }
}

/// The instant of a local wall-clock time: the next valid minute in a gap,
/// the earlier instant in an overlap.
#[must_use]
pub fn resolve(zone: &dyn LocalZone, local: NaiveDateTime) -> Option<Timestamp> {
    let mut at = local;
    // A gap is at most a few hours; give up after a day.
    for _ in 0..(24 * 60) {
        match zone.from_local(at) {
            Resolved::One(t) | Resolved::Two(t, _) => return Some(t),
            Resolved::Gap => at += chrono::Duration::minutes(1),
        }
    }
    None
}

fn runs_on(freq: Frequency, date: NaiveDate) -> bool {
    match freq {
        Frequency::Daily => true,
        Frequency::Weekly { weekday } => {
            u8::try_from(date.weekday().num_days_from_monday()).ok() == Some(weekday)
        }
        Frequency::Monthly { day } => u32::from(day) == date.day(),
    }
}

fn slot_on(schedule: &BackupSchedule, zone: &dyn LocalZone, date: NaiveDate) -> Option<Timestamp> {
    if !runs_on(schedule.frequency, date) {
        return None;
    }
    let time = NaiveTime::from_hms_opt(
        u32::from(schedule.at.hour()),
        u32::from(schedule.at.minute()),
        0,
    )?;
    resolve(zone, date.and_time(time))
}

/// The newest slot at or before `now`.
#[must_use]
pub fn latest_slot(
    schedule: &BackupSchedule,
    zone: &dyn LocalZone,
    now: Timestamp,
) -> Option<Timestamp> {
    if !schedule.frequency.is_valid() {
        return None;
    }
    // From tomorrow back: a DST shift can move today's slot either way.
    let today = zone.to_local(now).date();
    (-1..=LOOK_BACK_DAYS)
        .filter_map(|i| today.checked_sub_signed(chrono::Duration::days(i)))
        .filter_map(|d| slot_on(schedule, zone, d))
        .filter(|t| *t <= now)
        .max()
}

/// The first slot after `after`.
#[must_use]
pub fn next_slot(
    schedule: &BackupSchedule,
    zone: &dyn LocalZone,
    after: Timestamp,
) -> Option<Timestamp> {
    if !schedule.frequency.is_valid() {
        return None;
    }
    let day = zone.to_local(after).date();
    (-1..=LOOK_BACK_DAYS)
        .filter_map(|i| day.checked_add_signed(chrono::Duration::days(i)))
        .filter_map(|d| slot_on(schedule, zone, d))
        .filter(|t| *t > after)
        .min()
}

/// The newest slot a run used up (Done or Failed), ignoring one more than
/// a day in the future (the clock went back: it counts as never).
#[must_use]
pub fn last_used_slot(runs: &[ScheduleRun], now: Timestamp) -> Option<Timestamp> {
    let limit = now.unix_millis().saturating_add(FUTURE_SLACK_MS);
    runs.iter()
        .filter(|r| r.outcome.consumes_slot() && r.slot.unix_millis() <= limit)
        .map(|r| r.slot)
        .max()
}

/// The slot due now, if any: the newest slot at or before `now`, when no
/// run used it or a later one and it is not before the schedule was set.
/// Only ever one: slots missed while the app was closed are never stacked
/// (Q-P34h).
#[must_use]
pub fn due(
    schedule: &BackupSchedule,
    runs: &[ScheduleRun],
    zone: &dyn LocalZone,
    now: Timestamp,
) -> Option<Timestamp> {
    if !schedule.is_active() {
        return None;
    }
    let latest = latest_slot(schedule, zone, now)?;
    if schedule.since.is_some_and(|s| latest < s) {
        return None;
    }
    match last_used_slot(runs, now) {
        Some(last) if last >= latest => None,
        _ => Some(latest),
    }
}

// ---------------------------------------------------------------- runner

/// What the app tells the runner each tick.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RunnerEnv {
    /// When the current sign-in began (`None`: nobody signed in).
    pub signed_in_since: Option<Timestamp>,
    /// This computer is an office-network workstation (never runs).
    pub workstation: bool,
    /// A maintenance hold is on (waits).
    pub hold: bool,
    /// The app is locked for being idle (skips; catches up later).
    pub locked: bool,
}

/// One backup to make.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Job {
    /// The company.
    pub tenant_id: TenantId,
    /// The slot it is for.
    pub slot: Timestamp,
    /// On time or a catch-up.
    pub trigger: BackupTrigger,
    /// The schedule (folder, retention, test each).
    pub schedule: BackupSchedule,
}

/// What a job came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobResult {
    /// Written and verified.
    Done {
        /// The file's SHA-256.
        sha256: String,
    },
    /// Failed with a code.
    Failed {
        /// A code.
        code: String,
    },
    /// The engine was busy with another archive job (`TENANT_BUSY`).
    Busy,
    /// The engine refused this user (403).
    NotAllowed,
    /// Another archive job of this app holds the gate.
    GateBusy,
}

/// The runner's answer for one tick.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    /// Runs to write to the history now (skips, failures).
    pub record: Vec<(TenantId, ScheduleRun)>,
    /// The backup to make now (one at a time).
    pub run: Option<Job>,
}

/// Skip reasons (codes; the UI has the words).
pub mod reason {
    /// The app was locked.
    pub const LOCKED: &str = "LOCKED";
    /// Nobody was signed in.
    pub const SIGNED_OUT: &str = "SIGNED_OUT";
    /// The signed-in user may not back up this company.
    pub const NOT_ALLOWED: &str = "NOT_ALLOWED";
}

/// The scheduler's memory between ticks (one per app run).
#[derive(Debug, Default)]
pub struct Runner {
    signed_in_since: Option<Timestamp>,
    /// `(company, slot)` skips already written (at most one per slot).
    noted: BTreeSet<(TenantId, i64)>,
    /// `(company, slot)` the engine refused this sign-in: not tried again
    /// until the next sign-in.
    refused: BTreeSet<(TenantId, i64)>,
    /// `TENANT_BUSY` answers so far per `(company, slot)`.
    busy: BTreeMap<(TenantId, i64), u8>,
}

impl Runner {
    /// A fresh runner.
    #[must_use]
    pub fn new() -> Runner {
        Runner::default()
    }

    /// Decides this tick.
    #[must_use]
    pub fn plan(
        &mut self,
        now: Timestamp,
        env: RunnerEnv,
        list: &RecentCompanies,
        zone: &dyn LocalZone,
    ) -> Plan {
        let mut plan = Plan::default();
        if env.workstation {
            return plan;
        }
        if env.signed_in_since != self.signed_in_since {
            // A new sign-in tries refused slots again.
            self.signed_in_since = env.signed_in_since;
            self.refused.clear();
        }

        for c in list.live().filter(|c| !c.is_sample) {
            let Some(schedule) = c.schedule.as_ref() else {
                continue;
            };
            let Some(slot) = due(schedule, &c.schedule_runs, zone, now) else {
                continue;
            };
            let key = (c.tenant_id, slot.unix_millis());
            let skip = match env.signed_in_since {
                None => Some(reason::SIGNED_OUT),
                Some(_) if env.locked => Some(reason::LOCKED),
                Some(_) => None,
            };
            if let Some(why) = skip {
                if !c
                    .schedule_runs
                    .iter()
                    .any(|r| r.slot == slot && !r.outcome.consumes_slot())
                    && self.noted.insert(key)
                {
                    plan.record.push((
                        c.tenant_id,
                        ScheduleRun {
                            slot,
                            at: now,
                            outcome: RunOutcome::Skipped { reason: why.into() },
                        },
                    ));
                }
                continue;
            }
            if env.hold || plan.run.is_some() || self.refused.contains(&key) {
                continue;
            }
            let since = env.signed_in_since.unwrap_or(now);
            let was_skipped = c
                .schedule_runs
                .iter()
                .any(|r| r.slot == slot && !r.outcome.consumes_slot());
            let trigger = if slot < since {
                // Missed while the app was closed or nobody was signed in:
                // one catch-up, 2 minutes after sign-in.
                if now.unix_millis() < since.unix_millis().saturating_add(CATCH_UP_DELAY_MS) {
                    continue;
                }
                BackupTrigger::Missed
            } else if was_skipped {
                // Skipped while locked: a catch-up now that it is open.
                BackupTrigger::Missed
            } else {
                BackupTrigger::Scheduled
            };
            plan.run = Some(Job {
                tenant_id: c.tenant_id,
                slot,
                trigger,
                schedule: schedule.clone(),
            });
        }
        plan
    }

    /// Folds a job's result in; returns the run to record, if any.
    pub fn finished(
        &mut self,
        job: &Job,
        result: &JobResult,
        now: Timestamp,
    ) -> Option<ScheduleRun> {
        let key = (job.tenant_id, job.slot.unix_millis());
        let outcome = match result {
            JobResult::Done { sha256 } => RunOutcome::Done {
                sha256: sha256.clone(),
            },
            JobResult::Failed { code } => RunOutcome::Failed { code: code.clone() },
            JobResult::GateBusy => return None,
            JobResult::Busy => {
                let n = self.busy.entry(key).or_insert(0);
                *n += 1;
                if *n < BUSY_TRIES {
                    return None;
                }
                RunOutcome::Failed {
                    code: "TENANT_BUSY".into(),
                }
            }
            JobResult::NotAllowed => {
                self.refused.insert(key);
                if !self.noted.insert(key) {
                    return None;
                }
                RunOutcome::Skipped {
                    reason: reason::NOT_ALLOWED.into(),
                }
            }
        };
        self.busy.remove(&key);
        Some(ScheduleRun {
            slot: job.slot,
            at: now,
            outcome,
        })
    }
}

// ---------------------------------------------------------------- I/O

/// Code for a schedule folder that is missing (an unplugged drive).
pub const FOLDER_UNREACHABLE: &str = "FOLDER_UNREACHABLE";

/// The code a failed scheduled backup is recorded with (never a path).
#[must_use]
pub fn failure_code(e: &CompanyError) -> String {
    match e {
        CompanyError::NoPassphrase => "NO_PASSPHRASE".into(),
        CompanyError::JobFailed { code } => code.clone(),
        CompanyError::MissingFiles(_) => "FILE_MISSING".into(),
        CompanyError::CopyMismatch => "COPY_MISMATCH".into(),
        CompanyError::Io { .. } => "WRITE_FAILED".into(),
        CompanyError::UnknownCompany => "UNKNOWN_COMPANY".into(),
        CompanyError::Settings(_) => "SETTINGS".into(),
        CompanyError::Api(_) => "ENGINE".into(),
        _ => "FAILED".into(),
    }
}

fn classify(r: Result<crate::BackupOutcome, CompanyError>) -> JobResult {
    match r {
        Ok(o) => JobResult::Done {
            sha256: o.record.sha256,
        },
        Err(CompanyError::Api(cl_api::ApiError::Forbidden { .. })) => JobResult::NotAllowed,
        Err(CompanyError::Api(cl_api::ApiError::Conflict {
            reason: cl_api::ConflictReason::Busy,
            ..
        })) => JobResult::Busy,
        Err(CompanyError::JobFailed { code }) if code == "TENANT_BUSY" => JobResult::Busy,
        Err(e) => {
            let code = failure_code(&e);
            tracing::warn!(code = %code, "scheduled backup failed");
            JobResult::Failed { code }
        }
    }
}

/// Makes one scheduled backup: the folder must exist (never a fallback),
/// [`GATE`] must be free (else [`JobResult::GateBusy`]), then a verified
/// backup with the schedule's retention, stamped with `now`.
pub async fn run_job(
    session: &Session,
    store: &RecentStore,
    secrets: &dyn SecretStore,
    job: &Job,
    now: Timestamp,
    progress: &ProgressSink,
) -> JobResult {
    let folder = job.schedule.folder.clone();
    let reachable = tokio::task::spawn_blocking(move || Path::new(&folder).is_dir())
        .await
        .unwrap_or(false);
    if !reachable {
        tracing::warn!(code = FOLDER_UNREACHABLE, "scheduled backup failed");
        return JobResult::Failed {
            code: FOLDER_UNREACHABLE.into(),
        };
    }
    let Ok(_gate) = GATE.try_lock() else {
        return JobResult::GateBusy;
    };
    let r = crate::backup::backup_ungated(
        session,
        store,
        secrets,
        &BackupRequest {
            tenant_id: job.tenant_id,
            kind: BackupKind::Backup,
            dest_dir: job.schedule.folder.clone(),
            apply_retention: true,
            protected: None,
        },
        &BackupOptions {
            trigger: Some(job.trigger),
            now: Some(now),
            keep: Some((job.schedule.keep_last, job.schedule.keep_months)),
        },
        progress,
    )
    .await;
    classify(r)
}

/// What the loop needs from the app.
pub trait RunnerHost: Send + Sync {
    /// The app's state now.
    fn env(&self) -> RunnerEnv;
    /// The signed-in session, if any.
    fn session(&self) -> Option<Session>;
    /// The companies file.
    fn store(&self) -> RecentStore;
    /// The keychain.
    fn secrets(&self) -> Arc<dyn SecretStore>;
    /// A run was recorded (the UI refreshes Home and History).
    fn recorded(&self, _tenant: TenantId, _run: &ScheduleRun) {}
    /// A job starts or ends (the UI may show a status line).
    fn running(&self, _job: Option<&Job>) {}
    /// After a scheduled backup is written: a test restore when the
    /// schedule asks for it (B2 wires this).
    fn after_backup<'a>(
        &'a self,
        _session: &'a Session,
        _job: &'a Job,
        _sha256: &'a str,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async {})
    }
}

/// One tick: plan, record skips, run at most one job, record its result.
pub async fn tick(
    runner: &mut Runner,
    clock: &dyn Clock,
    zone: &dyn LocalZone,
    host: &dyn RunnerHost,
) {
    let store = host.store();
    let Ok(list) = store.load() else {
        return;
    };
    let now = clock.now();
    let env = host.env();
    let plan = runner.plan(now, env, &list, zone);
    record(&store, host, plan.record);
    let Some(job) = plan.run else {
        return;
    };
    let Some(session) = host.session() else {
        return;
    };
    host.running(Some(&job));
    let secrets = host.secrets();
    let result = run_job(
        &session,
        &store,
        secrets.as_ref(),
        &job,
        now,
        &crate::backup::silent(),
    )
    .await;
    host.running(None);
    let done_sha = match &result {
        JobResult::Done { sha256 } => Some(sha256.clone()),
        _ => None,
    };
    if let Some(run) = runner.finished(&job, &result, clock.now()) {
        record(&store, host, vec![(job.tenant_id, run)]);
    }
    if let Some(sha) = done_sha
        && job.schedule.test_each
    {
        host.after_backup(&session, &job, &sha).await;
    }
}

fn record(store: &RecentStore, host: &dyn RunnerHost, runs: Vec<(TenantId, ScheduleRun)>) {
    if runs.is_empty() {
        return;
    }
    let saved = store.update(|l| {
        for (t, r) in &runs {
            l.record_schedule_run(*t, r.clone());
        }
    });
    if saved.is_err() {
        tracing::warn!("scheduled backup: the history could not be saved");
        return;
    }
    for (t, r) in &runs {
        host.recorded(*t, r);
    }
}

/// Ticks every [`TICK`] until `stop` answers true.
pub async fn run_loop(
    clock: Arc<dyn Clock>,
    zone: Arc<dyn LocalZone>,
    host: Arc<dyn RunnerHost>,
    stop: impl Fn() -> bool + Send,
) {
    let mut runner = Runner::new();
    while !stop() {
        tick(&mut runner, clock.as_ref(), zone.as_ref(), host.as_ref()).await;
        clock.sleep(TICK).await;
    }
}

#[cfg(test)]
#[path = "schedule_tests.rs"]
mod tests;
