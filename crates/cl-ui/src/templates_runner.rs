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

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use cl_api::{ApiError, LocalValidation, RUN_LIMIT_DEFAULT, RequestGroup, RunDue};
use cl_core::CalendarDate;
use cl_core::ids::{RunId, TemplateId, TenantId, TransactionId};

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
    /// Automatic templates whose catch-up waits for the owner (more than
    /// three missed dates, Q-PR1d).
    questions: Vec<CatchUpQuestion>,
    /// "Post all" answers: the template and its first missed date then.
    confirmed: BTreeSet<(TemplateId, CalendarDate)>,
    /// Occurrences a form saved whose link to the date is still being
    /// made (S2): the saved transaction and the tries so far. Templates Due
    /// shows them as already entered; the runner tries again.
    linking: BTreeMap<(TemplateId, CalendarDate), Linking>,
    /// Where `linking` is kept between runs (review fix-PR-1 should-fix
    /// 5): the settings folder and the company.
    store: Option<(std::path::PathBuf, String)>,
    /// "Skip these dates" on its way for these templates: no run starts
    /// meanwhile (it could post a date the owner is skipping).
    skipping: BTreeSet<TemplateId>,
    /// A run is on its way: "Skip these dates" waits for it.
    running: bool,
    /// The run waits because an automatic template's date has a saved
    /// form whose link has not taken (it could post the date again).
    links_wait: bool,
}

/// One pending link of a saved form to its date (S2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Linking {
    txn: TransactionId,
    tries: u32,
    /// The last try failed and may go again (none is on its way).
    ready: bool,
    /// The engine refused it for good: "Saved separately", dismissable.
    refused: bool,
}

/// How often a failed link of a saved form to its date is tried (S2).
pub const LINK_TRIES: u32 = 6;
/// Seconds between the tries.
pub const LINK_RETRY_SECS: f64 = 5.0;

/// One automatic template the catch-up asks about before it posts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatchUpQuestion {
    /// The template.
    pub template: TemplateId,
    /// The missed dates the run would post, oldest first.
    pub dates: Vec<CalendarDate>,
}

/// The most missed dates the question lists (the engine posts at most
/// 403 per schedule per run, Q-P15i).
const QUESTION_DATES_MAX: usize = 403;

/// The automatic templates in `rows` whose catch-up as of `today` would
/// post more than three dates and that the owner has not confirmed.
pub(crate) fn questions_of(
    rows: &[cl_api::Template],
    today: CalendarDate,
    confirmed: &BTreeSet<(TemplateId, CalendarDate)>,
) -> Vec<CatchUpQuestion> {
    rows.iter()
        .filter_map(|t| {
            let s = t.schedule.as_ref()?;
            if s.mode != cl_api::ScheduleMode::Auto {
                return None;
            }
            let model = crate::screens::templates::dialog::model_schedule(s);
            let dates = cl_model::templates::missed_dates(&model, today, QUESTION_DATES_MAX);
            let first = *dates.first()?;
            (cl_model::templates::catch_up_asks(dates.len()) && !confirmed.contains(&(t.id, first)))
                .then_some(CatchUpQuestion {
                    template: t.id,
                    dates,
                })
        })
        .collect()
}

impl DueBoard {
    /// The catch-up questions waiting for the owner.
    #[must_use]
    pub fn questions(&self) -> &[CatchUpQuestion] {
        &self.questions
    }

    fn set_questions(&mut self, q: Vec<CatchUpQuestion>) {
        self.questions = q;
        self.generation += 1;
    }

    /// The "Post all" answers so far.
    #[must_use]
    pub fn confirmed(&self) -> BTreeSet<(TemplateId, CalendarDate)> {
        self.confirmed.clone()
    }

    /// "Post all" for `template`: the next run posts its missed dates.
    pub fn confirm(&mut self, template: TemplateId) {
        if let Some(at) = self.questions.iter().position(|q| q.template == template) {
            let q = self.questions.remove(at);
            if let Some(first) = q.dates.first() {
                self.confirmed.insert((template, *first));
            }
            self.generation += 1;
            self.check_now = true;
        }
    }

    /// "Skip these" for `template` finished (each date went through the
    /// skip path): the next run goes on.
    pub fn skipped_all(&mut self, template: TemplateId) {
        self.questions.retain(|q| q.template != template);
        self.generation += 1;
        self.check_now = true;
    }

    /// A form saved `txn` for `(template, date)`; the link to the date is
    /// on its way (and is tried again until it takes, S2).
    pub fn linking(&mut self, template: TemplateId, date: CalendarDate, txn: TransactionId) {
        let l = self.linking.entry((template, date)).or_insert(Linking {
            txn,
            tries: 0,
            ready: false,
            refused: false,
        });
        l.tries += 1;
        l.ready = false;
        l.refused = false;
        self.generation += 1;
        self.persist();
    }

    /// The link of `(template, date)` took: the date is entered. A run
    /// that waited on it goes on.
    pub fn linked(&mut self, template: TemplateId, date: CalendarDate) {
        if self.linking.remove(&(template, date)).is_some() {
            self.generation += 1;
            self.persist();
            if self.links_wait {
                self.check_now = true;
            }
        }
    }

    /// The engine refused the link of `(template, date)` for good: the
    /// saved transaction stands on its own ("Saved separately"); the date
    /// stays off "Enter it now" until the owner dismisses it.
    pub fn link_refused(&mut self, template: TemplateId, date: CalendarDate) {
        if let Some(l) = self.linking.get_mut(&(template, date)) {
            l.refused = true;
            l.ready = false;
            self.generation += 1;
            self.persist();
        }
    }

    /// The owner dismissed a "Saved separately" date (it went through the
    /// skip path, so the engine never posts it).
    pub fn link_dismissed(&mut self, template: TemplateId, date: CalendarDate) {
        self.linked(template, date);
    }

    /// A saved form's link to `(template, date)` is still being made.
    #[must_use]
    pub fn is_linking(&self, template: TemplateId, date: CalendarDate) -> bool {
        self.linking
            .get(&(template, date))
            .is_some_and(|l| !Self::separately(l))
    }

    /// The saved form's link to `(template, date)` was refused, or every
    /// try this session failed: shown as "Saved separately" with Dismiss.
    #[must_use]
    pub fn is_saved_separately(&self, template: TemplateId, date: CalendarDate) -> bool {
        self.linking
            .get(&(template, date))
            .is_some_and(Self::separately)
    }

    fn separately(l: &Linking) -> bool {
        l.refused || (l.ready && l.tries >= LINK_TRIES)
    }

    /// Every date a saved form holds (pending or saved separately).
    #[must_use]
    pub fn held_dates(&self) -> Vec<(TemplateId, CalendarDate)> {
        self.linking.keys().copied().collect()
    }

    /// A try of the link of `(template, date)` failed and may go again.
    pub fn link_failed(&mut self, template: TemplateId, date: CalendarDate) {
        if let Some(l) = self.linking.get_mut(&(template, date)) {
            l.ready = true;
            if l.tries >= LINK_TRIES {
                // Shown as saved separately now.
                self.generation += 1;
            }
        }
    }

    /// The next link to try again (failed, fewer than [`LINK_TRIES`] tries
    /// so far); the try is counted and marked on its way.
    pub(crate) fn next_link(&mut self) -> Option<(TemplateId, CalendarDate, TransactionId)> {
        let (key, l) = self
            .linking
            .iter_mut()
            .find(|(_, l)| l.ready && !l.refused && l.tries < LINK_TRIES)?;
        l.tries += 1;
        l.ready = false;
        Some((key.0, key.1, l.txn))
    }

    /// Keeps the pending links of `company` in `dir` and reads the ones
    /// left from before (a restart tries them again).
    pub fn attach(&mut self, dir: std::path::PathBuf, company: String) {
        let book = crate::prefs::PendingLinksBook::load(&dir);
        for p in book.get(&company) {
            self.linking.insert(
                (p.template, p.date),
                Linking {
                    txn: p.txn,
                    tries: 0,
                    ready: !p.refused,
                    refused: p.refused,
                },
            );
        }
        self.store = Some((dir, company));
        self.generation += 1;
    }

    fn persist(&self) {
        let Some((dir, company)) = &self.store else {
            return;
        };
        let links: Vec<crate::prefs::PendingLink> = self
            .linking
            .iter()
            .map(|((template, date), l)| crate::prefs::PendingLink {
                template: *template,
                date: *date,
                txn: l.txn,
                refused: l.refused,
            })
            .collect();
        let mut book = crate::prefs::PendingLinksBook::load(dir);
        book.set(company, &links);
        if let Err(e) = book.save(dir) {
            tracing::warn!(error = %e, "could not keep the pending template links");
        }
    }

    /// "Skip these dates" for `template` started: no run starts until it
    /// ends ([`Self::skip_ended`]).
    pub fn skip_started(&mut self, template: TemplateId) {
        self.skipping.insert(template);
    }

    /// "Skip these dates" for `template` ended (done or failed).
    pub fn skip_ended(&mut self, template: TemplateId) {
        self.skipping.remove(&template);
    }

    /// A run is on its way ("Skip these dates" waits for it).
    #[must_use]
    pub fn running(&self) -> bool {
        self.running
    }

    /// The run waits on a saved form's link of an automatic template.
    #[must_use]
    pub fn links_wait(&self) -> bool {
        self.links_wait
    }
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

    /// How many dates wait for the owner (due reminder dates and open
    /// skipped dates, each a row in Templates Due): the status-bar chip
    /// ("4 entries need you").
    /// Also counted: a catch-up question (one per template, until it is
    /// answered: the window opens for it once, then the chip stays) and a
    /// "Saved separately" date to dismiss.
    #[must_use]
    pub fn needs_you(&self) -> usize {
        let mut all: BTreeSet<(TemplateId, CalendarDate)> = BTreeSet::new();
        if let Some(run) = &self.run {
            all.extend(
                run.reminders
                    .iter()
                    .flat_map(|r| r.dates.iter().map(move |d| (r.template, *d)))
                    // A capped catch-up continues on its own: not a chip.
                    .chain(
                        run.skipped
                            .iter()
                            .filter(|k| !k.reason.is_capped())
                            .map(|k| (k.template, k.date)),
                    )
                    // A date a saved form holds is no longer waiting.
                    .filter(|(t, d)| !self.is_linking(*t, *d)),
            );
        }
        all.extend(
            self.linking
                .iter()
                .filter(|(_, l)| Self::separately(l))
                .map(|(k, _)| *k),
        );
        all.extend(
            self.questions
                .iter()
                .filter_map(|q| q.dates.first().map(|d| (q.template, *d))),
        );
        all.len()
    }

    /// Forget everything (the company closed). The pending links stay in
    /// their file for the next open.
    pub fn clear(&mut self) {
        self.run = None;
        self.check_now = false;
        self.questions.clear();
        self.confirmed.clear();
        self.linking.clear();
        self.store = None;
        self.skipping.clear();
        self.running = false;
        self.links_wait = false;
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
    /// The run did not start: these templates ask first (Q-PR1d).
    pub questions: Vec<CatchUpQuestion>,
    /// The run did not start: an automatic template's date is held by a
    /// saved form whose link has not taken (review fix-PR-1 should-fix 5).
    pub links_wait: bool,
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
            ..Gathered::default()
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

/// An automatic template has a due date held by a saved form whose link to
/// it has not taken: the engine's run (no template filter) could post that
/// date a second time, so the run waits (review fix-PR-1 should-fix 5).
pub(crate) fn waits_on_links(
    rows: &[cl_api::Template],
    today: CalendarDate,
    held: &[(TemplateId, CalendarDate)],
) -> bool {
    held.iter().any(|(id, date)| {
        *date <= today
            && rows.iter().any(|t| {
                t.id == *id
                    && t.schedule
                        .as_ref()
                        .is_some_and(|s| s.mode == cl_api::ScheduleMode::Auto)
            })
    })
}

/// One whole run on the demo engine (every round, as [`gather`]).
fn demo_run(
    state: &mut crate::company::demo::DemoState,
    as_of: CalendarDate,
    run_id: RunId,
    today: CalendarDate,
    confirmed: &BTreeSet<(TemplateId, CalendarDate)>,
    held: &[(TemplateId, CalendarDate)],
) -> Gathered {
    // The practice company's templates arrive with its first open (WP-D).
    let first = state.companies.first().map(|c| c.tenant_id);
    if first.is_some() && state.open == first {
        demo::seed_sample(state);
    }
    let rows = crate::company::demo::template_store::list(state);
    let questions = questions_of(&rows, as_of, confirmed);
    if !questions.is_empty() {
        return Gathered {
            questions,
            ..Gathered::default()
        };
    }
    if waits_on_links(&rows, as_of, held) {
        return Gathered {
            links_wait: true,
            ..Gathered::default()
        };
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
    /// A saved form's link to its date being tried again (S2).
    link_job: Option<((TemplateId, CalendarDate), Job<()>)>,
    /// When the next link try may start (`egui` seconds).
    next_link_at: f64,
    /// Templates whose catch-up question was shown this session: a later
    /// run asking the same opens no window (the chip stays).
    asked: BTreeSet<TemplateId>,
}

/// A refusal that another try of the same link can't change: the date or
/// the transaction no longer fit (`OCCURRENCE_RESOLVE_INVALID`), the date
/// was dismissed, the template is gone.
fn link_final(e: &ApiError) -> bool {
    matches!(
        e,
        ApiError::Validation(_) | ApiError::NotFound { .. } | ApiError::Forbidden { .. }
    ) || matches!(e, ApiError::Conflict { .. } if !cl_api::is_occurrence_taken(e))
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
        let mut board = link.due();
        board.clear();
        // The links saved forms still owe their dates (review fix-PR-1
        // should-fix 5), kept in the settings folder.
        if let Some(dir) = link.store().path().parent() {
            board.attach(dir.to_path_buf(), tenant.to_string());
        }
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
        self.tick_links(link, now);
        let mut out = finished(&mut self.job).and_then(|r| self.finish(link, r));
        // "Skip these dates" on its way: a run now could post a date the
        // owner is skipping, so it waits.
        let skipping = !link.due().skipping.is_empty();
        if self.pending && self.job.is_none() && !skipping {
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

    /// Tries again the links of saved forms to their dates that failed
    /// (S2): one at a time, every [`LINK_RETRY_SECS`], at most
    /// [`LINK_TRIES`] tries each, always with the saved transaction.
    pub(crate) fn tick_links(&mut self, link: &Arc<CompanyLink>, now: f64) {
        if let Some((key, job)) = self.link_job.as_mut()
            && let Some(r) = job.take()
        {
            let (template, date) = *key;
            self.link_job = None;
            let mut board = link.due();
            match r {
                Ok(()) => {
                    board.linked(template, date);
                    board.handled(template, date);
                }
                Err(e) if cl_api::is_occurrence_taken(&e) => {
                    board.linked(template, date);
                    board.handled(template, date);
                }
                // Another try can't change this: the saved transaction
                // stands on its own ("Saved separately", dismissable).
                Err(e) if link_final(&e) => board.link_refused(template, date),
                Err(_) => board.link_failed(template, date),
            }
        }
        if self.link_job.is_some() || now < self.next_link_at {
            return;
        }
        let Some((template, date, txn)) = link.due().next_link() else {
            return;
        };
        self.next_link_at = now + LINK_RETRY_SECS;
        let job = run(
            link,
            self.group.as_ref(),
            Some("orphan.template_resolve"),
            move |s| demo::resolve(s, template, date, txn),
            move |s| async move { s.templates().resolve(template, date, txn).await },
        );
        let done = matches!(job, Job::Done(_));
        self.link_job = Some(((template, date), job));
        // The demo answers at once.
        if done {
            self.tick_links(link, now);
        }
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
        let (confirmed, held) = {
            let mut board = link.due();
            board.running = true;
            (board.confirmed(), board.held_dates())
        };
        let demo_confirmed = confirmed.clone();
        let demo_held = held.clone();
        self.job = Some(run(
            link,
            self.group.as_ref(),
            Some("orphan.templates_run_due"),
            move |s| Ok(demo_run(s, today, run_id, today, &demo_confirmed, &demo_held)),
            move |s| async move {
                // An automatic template with more than three missed dates
                // asks first (Q-PR1d): the run waits for the answer. The
                // engine's run takes no template filter, so nothing is
                // sent until then.
                match s.templates().list_all().await {
                    Ok(rows) => {
                        let questions = questions_of(&rows, today, &confirmed);
                        if !questions.is_empty() {
                            return Ok(Gathered {
                                questions,
                                ..Gathered::default()
                            });
                        }
                        if waits_on_links(&rows, today, &held) {
                            return Ok(Gathered {
                                links_wait: true,
                                ..Gathered::default()
                            });
                        }
                    }
                    // No templates here: nothing to ask about.
                    Err(ApiError::Local(LocalValidation::TemplatesUnavailable)) => {}
                    Err(e) => {
                        return Ok(Gathered {
                            error: Some(e),
                            ..Gathered::default()
                        });
                    }
                }
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
        link.due().running = false;
        let (run, error, questions, links_wait) = match r {
            Ok(g) => (g.run, g.error, g.questions, g.links_wait),
            Err(e) => (None, Some(e), Vec::new(), false),
        };
        if !questions.is_empty() {
            // Nothing was sent; the run id stays for the run after the
            // answer. Templates Due opens with a question asked for the
            // first time this session; after that the chip stays (an
            // hourly run never opens it again for the same question).
            tracing::info!(
                templates = questions.len(),
                "templates catch-up waits for the owner"
            );
            self.retry = current;
            let fresh = questions.iter().any(|q| !self.asked.contains(&q.template));
            self.asked.extend(questions.iter().map(|q| q.template));
            link.due().set_questions(questions);
            return Some(Finished {
                posted: false,
                open_window: fresh,
            });
        }
        if links_wait {
            // Nothing was sent: an automatic template's date waits for a
            // saved form's link (or the owner's Dismiss); the run goes on
            // as soon as it is settled.
            tracing::info!("templates catch-up waits for a saved form's link");
            self.retry = current;
            let mut board = link.due();
            board.links_wait = true;
            board.generation += 1;
            return None;
        }
        {
            let mut board = link.due();
            board.links_wait = false;
            board.set_questions(Vec::new());
        }
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
