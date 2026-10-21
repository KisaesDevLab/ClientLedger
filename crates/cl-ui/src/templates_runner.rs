//! The catch-up runner (P15 C1, U-63, Q-P15h): asks the engine to post the
//! automatic templates that are due, and shows what happened.
//!
//! - **One per open company**, owned by the shell, single-flight.
//! - **Triggers:** the company opened (after sign-in); the local date
//!   changed (midnight; checked every frame, the shell repaints at least
//!   once a minute); every hour; "Check now" in Templates Due.
//! - **Run:** `POST /recurring/run-due {asOf: local today, runId, limit}`,
//!   repeated with the same run id while `more`. A transport error retries
//!   once with the same run id, so a lost reply comes back with the posts
//!   it made: exactly-once is the engine's (one database transaction per
//!   occurrence under its unique `(template, date)` key). The app sends a
//!   date and a run id, never amounts; the body never carries the
//!   closing-date password, so nothing posts into a closed period. While
//!   capped schedules (`CATCH_UP_CAPPED`, 31 dates per call) come back the
//!   run asks again under the same id, at most 12 extra times; the rest is
//!   left for the next run. A round that fails after others answered keeps
//!   what they posted (shown and published) and the run id, which the next
//!   run that day reuses.
//! - **Errors:** no permission (403) is silent; an engine without
//!   `recurring-run-due-v1` turns the runner off; anything else is logged
//!   (no names or amounts) and retried at the next trigger. A company switch
//!   mid-run drops the answer (the engine stays consistent).
//! - **After a run:** the answer goes on the company's [`DueBoard`]; the
//!   shell publishes `Ledger(All)` when anything posted (Q-262) and opens
//!   Templates Due when something posted, a reminder date is new this
//!   session, or a date was skipped by this run. Older open skips only show
//!   the status-bar chip.

use std::collections::BTreeSet;
use std::sync::Arc;

use cl_api::{ApiError, LocalValidation, RUN_LIMIT_DEFAULT, RequestGroup, RunDue};
use cl_core::CalendarDate;
use cl_core::ids::{RunId, TemplateId, TenantId};

use crate::company::CompanyLink;
use crate::company::demo::templates as demo;
use crate::screens::templates::engine::{Job, finished, run};

/// Seconds between the hourly runs.
pub const HOURLY_SECS: f64 = 3600.0;
/// The most `more` rounds one run makes (200 occurrences each).
const MAX_ROUNDS: usize = 1000;
/// The most extra rounds one run makes only because capped schedules
/// (`CATCH_UP_CAPPED`, 31 dates per schedule per call) came back; what is
/// still due after that is left for the next run.
const MAX_CAPPED_ROUNDS: usize = 12;

/// The last catch-up answer of the open company, shared by the runner and
/// the Templates Due window (on the [`CompanyLink`]).
#[derive(Debug, Default)]
pub struct DueBoard {
    generation: u64,
    run: Option<RunDue>,
    check_now: bool,
}

impl DueBoard {
    /// A new answer.
    pub fn set(&mut self, run: RunDue) {
        self.run = Some(run);
        self.generation += 1;
    }

    /// The last answer.
    #[must_use]
    pub fn run(&self) -> Option<&RunDue> {
        self.run.as_ref()
    }

    /// Changes with every new answer.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// "Check now": the runner runs at its next frame.
    pub fn request_check(&mut self) {
        self.check_now = true;
    }

    fn take_check(&mut self) -> bool {
        std::mem::take(&mut self.check_now)
    }

    /// The owner handled `(template, date)` from Templates Due: it leaves
    /// the reminders and the skipped list (the chip counts down).
    pub fn handled(&mut self, template: TemplateId, date: CalendarDate) {
        let Some(run) = self.run.as_mut() else {
            return;
        };
        run.skipped
            .retain(|k| !(k.template == template && k.date == date));
        for r in run.reminders.iter_mut().filter(|r| r.template == template) {
            if let Some(at) = r.dates.iter().position(|d| *d == date) {
                r.dates.remove(at);
                r.count = r.count.saturating_sub(1);
            }
        }
        run.reminders.retain(|r| r.count > 0 && !r.dates.is_empty());
        self.generation += 1;
    }

    /// How many templates wait for the owner (due reminders and open
    /// skipped dates): the status-bar chip.
    #[must_use]
    pub fn needs_you(&self) -> usize {
        let Some(run) = &self.run else {
            return 0;
        };
        run.reminders
            .iter()
            .map(|r| r.template)
            // A capped catch-up continues on its own: not a chip.
            .chain(
                run.skipped
                    .iter()
                    .filter(|k| !k.reason.is_capped())
                    .map(|k| k.template),
            )
            .collect::<BTreeSet<_>>()
            .len()
    }

    /// Forget everything (the company closed).
    pub fn clear(&mut self) {
        self.run = None;
        self.check_now = false;
        self.generation += 1;
    }
}

/// What a finished run asks of the shell.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Finished {
    /// Something posted: publish `Ledger(All)`.
    pub posted: bool,
    /// Open Templates Due.
    pub open_window: bool,
}

/// The engine runs catch-up: the demo always, else `recurring-run-due-v1`.
#[must_use]
pub fn runs_on(link: &CompanyLink) -> bool {
    link.is_demo()
        || link
            .capabilities()
            .has(cl_api::Capabilities::RECURRING_RUN_DUE)
}

/// A reply that may have been lost on the way (asked again once with the
/// same run id).
fn transient(e: &ApiError) -> bool {
    matches!(
        e,
        ApiError::Timeout | ApiError::EngineUnavailable { .. } | ApiError::OutcomeUnknown { .. }
    )
}

/// Folds one round into the run's answer: the posts are the run's (the
/// engine repeats every post of the run id), the reminders and skips the
/// latest; a date skipped by any round stays new.
fn fold(
    acc: Option<RunDue>,
    mut next: RunDue,
    new: &mut BTreeSet<(TemplateId, CalendarDate)>,
) -> RunDue {
    new.extend(
        next.skipped
            .iter()
            .filter(|k| k.new)
            .map(|k| (k.template, k.date)),
    );
    for k in &mut next.skipped {
        k.new = new.contains(&(k.template, k.date));
    }
    if let Some(prev) = acc {
        next.failed.splice(0..0, prev.failed);
    }
    next
}

/// What one run gathered: the folded answer of the rounds that answered
/// (their posts are real and are shown), and the error that stopped a
/// later round (the run id is kept so the retry gets those posts back).
#[derive(Debug, Default)]
pub(crate) struct Gathered {
    /// The folded answer, when at least one round answered.
    pub run: Option<RunDue>,
    /// Why the run stopped early.
    pub error: Option<ApiError>,
}

/// The rounds of one run (one run id).
#[derive(Default)]
struct Rounds {
    new: BTreeSet<(TemplateId, CalendarDate)>,
    acc: Option<RunDue>,
    calls: usize,
    capped_calls: usize,
}

impl Rounds {
    /// Folds one answer in; true when the run asks again (same run id):
    /// while `more`, and while capped schedules come back (bounded).
    fn take(&mut self, r: RunDue) -> bool {
        let more = r.more;
        let capped = r.skipped.iter().any(|k| k.reason.is_capped());
        self.acc = Some(fold(self.acc.take(), r, &mut self.new));
        self.calls += 1;
        if self.calls >= MAX_ROUNDS {
            return false;
        }
        if more {
            return true;
        }
        if capped && self.capped_calls < MAX_CAPPED_ROUNDS {
            self.capped_calls += 1;
            return true;
        }
        false
    }

    fn end(self, error: Option<ApiError>) -> Gathered {
        Gathered {
            run: self.acc,
            error,
        }
    }
}

/// Every round of one run: `ask` calls run-due with the run's id. A
/// transport error asks once more (the same run id gives the posts back,
/// never twice); any other error ends the run with what was gathered.
pub(crate) async fn gather<F, Fut>(mut ask: F) -> Gathered
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<RunDue, ApiError>>,
{
    let mut rounds = Rounds::default();
    loop {
        let r = match ask().await {
            Err(e) if transient(&e) => ask().await,
            r => r,
        };
        match r {
            Ok(r) => {
                if !rounds.take(r) {
                    return rounds.end(None);
                }
            }
            Err(e) => return rounds.end(Some(e)),
        }
    }
}

/// One whole run on the demo engine (every round, as [`gather`]).
fn demo_run(
    state: &mut crate::company::demo::DemoState,
    as_of: CalendarDate,
    run_id: RunId,
    today: CalendarDate,
) -> Gathered {
    // The practice company's templates arrive with its first open (WP-D).
    let first = state.companies.first().map(|c| c.tenant_id);
    if first.is_some() && state.open == first {
        demo::seed_sample(state);
    }
    let mut rounds = Rounds::default();
    loop {
        match demo::run_due(state, as_of, run_id, RUN_LIMIT_DEFAULT, today) {
            Ok((r, _)) => {
                if !rounds.take(r) {
                    return rounds.end(None);
                }
            }
            Err(e) => return rounds.end(Some(e)),
        }
    }
}

/// The catch-up runner of the open company.
#[derive(Default)]
pub struct TemplatesRunner {
    company: Option<TenantId>,
    group: Option<RequestGroup>,
    job: Option<Job<Gathered>>,
    /// The day and run id of the run on its way.
    current: Option<(CalendarDate, RunId)>,
    /// A run that stopped early: the next run that day reuses its id.
    pub(crate) retry: Option<(CalendarDate, RunId)>,
    /// A trigger waits for the running call.
    pending: bool,
    /// The day of the last run (a new local date runs again).
    last_day: Option<CalendarDate>,
    /// When the next hourly run is due (`egui` seconds).
    next_hour: Option<f64>,
    /// The engine has no catch-up: the runner stays off.
    off: bool,
    /// Reminder dates already shown this session.
    seen: BTreeSet<(TemplateId, CalendarDate)>,
    /// Runs answered since the company opened.
    pub(crate) runs: u32,
}

impl std::fmt::Debug for TemplatesRunner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TemplatesRunner")
            .field("company", &self.company)
            .field("busy", &self.job.is_some())
            .field("pending", &self.pending)
            .field("off", &self.off)
            .field("runs", &self.runs)
            .finish_non_exhaustive()
    }
}

impl TemplatesRunner {
    /// The company opened (or was opened again): run at the next frame. A
    /// run still on its way for the previous company is dropped.
    pub fn opened(&mut self, link: &CompanyLink, tenant: TenantId) {
        *self = TemplatesRunner {
            company: Some(tenant),
            group: link.group().and_then(Result::ok),
            pending: true,
            ..TemplatesRunner::default()
        };
        link.due().clear();
    }

    /// The company closed.
    pub fn closed(&mut self, link: &CompanyLink) {
        *self = TemplatesRunner::default();
        link.due().clear();
    }

    /// A run is on its way.
    #[must_use]
    pub fn busy(&self) -> bool {
        self.job.is_some()
    }

    /// The runner watches a company (the shell repaints each minute so the
    /// date and hour triggers fire).
    #[must_use]
    pub fn armed(&self) -> bool {
        self.company.is_some() && !self.off
    }

    /// One frame: starts a run when a trigger fired and takes a finished
    /// answer. `today` is the local date, `now` the `egui` clock.
    pub fn tick(
        &mut self,
        link: &Arc<CompanyLink>,
        today: CalendarDate,
        now: f64,
    ) -> Option<Finished> {
        if !self.armed() {
            return None;
        }
        if !runs_on(link) {
            self.off = true;
            return None;
        }
        if link.due().take_check() {
            self.pending = true;
        }
        if self.last_day.is_some_and(|d| d != today) || self.next_hour.is_some_and(|h| now >= h) {
            self.pending = true;
        }
        let mut out = finished(&mut self.job).and_then(|r| self.finish(link, r));
        if self.pending && self.job.is_none() {
            self.pending = false;
            self.last_day = Some(today);
            self.next_hour = Some(now + HOURLY_SECS);
            self.start(link, today);
            if let Some(f) = finished(&mut self.job).and_then(|r| self.finish(link, r)) {
                out = Some(f);
            }
        }
        out
    }

    /// The run id for a run today: the stopped run's (same day), else new.
    fn run_id_for(&mut self, today: CalendarDate) -> RunId {
        match self.retry.take() {
            Some((day, id)) if day == today => id,
            _ => cl_api::new_run_id(),
        }
    }

    fn start(&mut self, link: &Arc<CompanyLink>, today: CalendarDate) {
        let run_id = self.run_id_for(today);
        self.current = Some((today, run_id));
        self.job = Some(run(
            link,
            self.group.as_ref(),
            Some("orphan.templates_run_due"),
            move |s| Ok(demo_run(s, today, run_id, today)),
            move |s| async move {
                let s = &s;
                Ok(gather(move || async move {
                    s.recurring()
                        .run_due(today, run_id, RUN_LIMIT_DEFAULT)
                        .await
                })
                .await)
            },
        ));
    }

    pub(crate) fn finish(
        &mut self,
        link: &CompanyLink,
        r: Result<Gathered, ApiError>,
    ) -> Option<Finished> {
        let current = self.current.take();
        let (run, error) = match r {
            Ok(g) => (g.run, g.error),
            Err(e) => (None, Some(e)),
        };
        // A run stopped by anything but "no permission" or "no catch-up"
        // keeps its id: the retry gets its posts back, never twice.
        self.retry = match &error {
            None
            | Some(
                ApiError::Forbidden { .. } | ApiError::Local(LocalValidation::RunDueUnavailable),
            ) => None,
            Some(_) => current,
        };
        let shown = run.map(|run| self.show(link, run));
        match error {
            None | Some(ApiError::Forbidden { .. }) => {}
            Some(ApiError::Local(LocalValidation::RunDueUnavailable)) => self.off = true,
            Some(e) => {
                tracing::warn!(error = %e, "templates catch-up stopped; trying again later");
            }
        }
        shown
    }

    /// Puts an answer on the board (the posts it gathered are real even
    /// when a later round failed).
    fn show(&mut self, link: &CompanyLink, run: RunDue) -> Finished {
        self.runs += 1;
        let reminders: Vec<(TemplateId, CalendarDate)> = run
            .reminders
            .iter()
            .flat_map(|r| r.dates.iter().map(|d| (r.template, *d)))
            .collect();
        let fresh = reminders.iter().filter(|k| !self.seen.contains(k)).count();
        self.seen.extend(reminders);
        // A capped catch-up is not a skip the owner must see.
        let new_skips = run
            .skipped
            .iter()
            .filter(|k| k.new && !k.reason.is_capped())
            .count();
        let posted = !run.posted.is_empty();
        let open_window = cl_model::templates::should_open_due(run.posted.len(), fresh, new_skips);
        tracing::info!(
            posted = run.posted.len(),
            reminders = run.reminders.len(),
            skipped = run.skipped.len(),
            failed = run.failed.len(),
            "templates catch-up ran"
        );
        link.due().set(run);
        Finished {
            posted,
            open_window,
        }
    }
}

#[cfg(test)]
#[path = "templates_runner_tests.rs"]
mod tests;
