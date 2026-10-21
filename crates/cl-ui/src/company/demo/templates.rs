//! The demo engine's templates and catch-up run (P15 lane C, U-62/U-63).
//! Synthetic.
//!
//! The twin of `POST /recurring/run-due`, `GET /recurring/attention`,
//! `skip` and `resolve` (`enter` is in [`super::template_store`]), mirroring
//! `P15-engine-shapes-b.md`:
//!
//! - each `(template, date)` is handled once, under its unique key in
//!   [`DemoState::occurrences`]; a date already posted, entered or
//!   dismissed is never posted again;
//! - automatic schedules post every due date up to `asOf` on the clamped
//!   month rule ([`super::template_store::next_after`]), at most `limit`
//!   per call (`more` when something is still due);
//! - a date on or before the closing date is never posted (skipped
//!   `CLOSED_PERIOD`, even though the demo knows the password), an
//!   inactive account skips `INVALID_ACCOUNT`; a skipped date still moves
//!   the schedule on, and stays open until it is entered or dismissed;
//! - `posted` repeats every earlier post of the same `runId` (a lost reply
//!   is recovered by asking again);
//! - reminders are listed, never posted or advanced.
//!
//! Amounts come only from the stored drafts (the app sends a date and a
//! run id). Also the practice company's three templates
//! ([`seed_sample`], P15 WP-D).

use std::collections::BTreeSet;
use std::fmt;

use cl_api::{
    ApiError, Attention, CardChargeDraft, CheckDraft, ConflictReason, DueReminder, FailedTemplate,
    Frequency, JeLine, LocalValidation, NewJournalEntry, PostedOccurrence, RunDue, ScheduleEnd,
    ScheduleMode, SkipReason, SkippedOccurrence, SplitLine, TemplateDraft, TemplateSchedule,
};
use cl_core::ids::{AccountId, RunId, TemplateGroupId, TemplateId, TransactionId};
use cl_core::{CalendarDate, Money};

use super::DemoState;
use super::template_store::{closing, conflict, live, next_after, post_draft, total};

/// A stored template as the demo engine keeps it.
#[derive(Clone, PartialEq, Eq)]
pub struct DemoTemplate {
    /// Its id.
    pub id: TemplateId,
    /// The owner's name for it.
    pub name: String,
    /// Its group, if any.
    pub group: Option<TemplateGroupId>,
    /// The stored draft (a form's create body minus the date and numbers).
    pub draft: TemplateDraft,
    /// The desktop form it opens in (`clientForm`, P15 lane T).
    pub client_form: String,
    /// Its schedule; `None` is "Don't remind me" (lane T).
    pub schedule: Option<cl_api::TemplateSchedule>,
    /// The day-of-month anchor the clamped month rule counts from (the
    /// next date the owner last set; lane T).
    pub anchor: Option<cl_core::CalendarDate>,
    /// The last date it was entered (lane T).
    pub last_entered: Option<cl_core::CalendarDate>,
    /// The engine's row version (409 `STALE` when a write names another).
    pub version: u32,
    /// Soft-deleted (gone from the list; its occurrences stay).
    pub deleted: bool,
}

/// Ids and the version only: names and amounts are owner data.
impl fmt::Debug for DemoTemplate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DemoTemplate")
            .field("id", &self.id)
            .field("group", &self.group)
            .field("version", &self.version)
            .field("deleted", &self.deleted)
            .finish_non_exhaustive()
    }
}

/// A template group.
#[derive(Clone, PartialEq, Eq)]
pub struct DemoTemplateGroup {
    /// Its id.
    pub id: TemplateGroupId,
    /// Its name.
    pub name: String,
}

/// The id only.
impl fmt::Debug for DemoTemplateGroup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DemoTemplateGroup")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

/// What happened to one occurrence (`template_occurrences.status`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OccStatus {
    /// Posted by the catch-up run.
    Posted,
    /// Entered by the owner (Enter now, or a form linked with `resolve`).
    Entered,
    /// Could not be posted (a closed period, an inactive account).
    Skipped,
    /// The owner chose Skip this one or Dismiss.
    Dismissed,
}

/// One occurrence, under its unique `(template, date)` key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Occ {
    /// What happened.
    pub status: OccStatus,
    /// The transaction it posted or was linked to.
    pub transaction: Option<TransactionId>,
    /// Why it was skipped (the engine's reason code).
    pub reason: Option<&'static str>,
    /// The run that handled it (replayed for the same `runId`).
    pub run: Option<RunId>,
}

/// The most occurrences one run handles (the engine's cap).
const RUN_LIMIT_MAX: u32 = 500;
/// Due dates listed per reminder.
const DUE_DATES_MAX: usize = 24;
/// The highest due count reported.
const DUE_COUNT_MAX: u32 = 9999;

/// Moves `t`'s schedule past `on` when `on` is its next date: the next
/// date on the clamped rule; the schedule ends when its count is spent or
/// the next date passes its end date. Other dates leave it alone.
pub fn advance(t: &mut DemoTemplate, on: CalendarDate) {
    let Some(mut sched) = t.schedule else {
        return;
    };
    if sched.next_date != on {
        return;
    }
    let next = next_after(t, on);
    let spent = match &mut sched.end {
        ScheduleEnd::After(n) => {
            *n = n.saturating_sub(1);
            *n == 0
        }
        _ => false,
    };
    let past_end = matches!((sched.end, next), (ScheduleEnd::On(end), Some(n)) if n > end);
    match next {
        Some(n) if !spent && !past_end => {
            sched.next_date = n;
            t.schedule = Some(sched);
        }
        _ => t.schedule = None,
    }
    t.version += 1;
}

fn not_due() -> ApiError {
    // 400 OCCURRENCE_NOT_DUE / RUN_DUE_FUTURE / OCCURRENCE_RESOLVE_INVALID.
    ApiError::Local(LocalValidation::InvalidValue)
}

fn taken() -> ApiError {
    conflict(ConflictReason::Unspecified, cl_api::OCCURRENCE_TAKEN)
}

/// The auto template due first (earliest next date, then id), leaving out
/// the ones that failed in this call.
fn first_due(
    state: &DemoState,
    as_of: CalendarDate,
    failed: &BTreeSet<TemplateId>,
) -> Option<(TemplateId, CalendarDate)> {
    state
        .templates
        .iter()
        .filter(|t| !t.deleted && t.draft.allows_auto() && !failed.contains(&t.id))
        .filter_map(|t| {
            t.schedule
                .filter(|s| s.mode == ScheduleMode::Auto && s.next_date <= as_of)
                .map(|s| (t.id, s.next_date))
        })
        .min_by_key(|(id, d)| (*d, *id))
}

/// Why posting failed, as a skip reason; `None` for an unexpected error
/// (reported under `failed`: nothing claimed or advanced).
fn skip_reason(e: &ApiError) -> Option<&'static str> {
    match e {
        ApiError::ClosedPeriod(_)
        | ApiError::ClosingPasswordRequired
        | ApiError::ClosingPasswordRejected => Some("CLOSED_PERIOD"),
        ApiError::Local(LocalValidation::InvalidValue) => Some("INVALID_ACCOUNT"),
        _ => None,
    }
}

fn template(state: &DemoState, id: TemplateId) -> Option<&DemoTemplate> {
    state.templates.iter().find(|t| t.id == id)
}

/// The dates of a remind schedule due through `as_of` (at most
/// [`DUE_DATES_MAX`] listed) and how many there are.
fn due_dates(t: &DemoTemplate, as_of: CalendarDate) -> (Vec<CalendarDate>, u32) {
    let Some(s) = t.schedule else {
        return (Vec::new(), 0);
    };
    let mut left = match s.end {
        ScheduleEnd::After(n) => n,
        _ => u32::MAX,
    };
    let mut dates = Vec::new();
    let mut count = 0u32;
    let mut at = Some(s.next_date);
    while let Some(d) = at {
        let past_end = matches!(s.end, ScheduleEnd::On(end) if d > end);
        if d > as_of || past_end || left == 0 || count >= DUE_COUNT_MAX {
            break;
        }
        if dates.len() < DUE_DATES_MAX {
            dates.push(d);
        }
        count += 1;
        left = left.saturating_sub(1);
        at = next_after(t, d);
    }
    (dates, count)
}

fn reminders(state: &DemoState, as_of: CalendarDate) -> Vec<DueReminder> {
    let mut out: Vec<DueReminder> = state
        .templates
        .iter()
        .filter(|t| !t.deleted)
        .filter(|t| t.schedule.is_some_and(|s| s.mode == ScheduleMode::Remind))
        .filter_map(|t| {
            let (dates, count) = due_dates(t, as_of);
            (count > 0).then(|| DueReminder {
                template: t.id,
                name: t.name.clone(),
                kind: t.draft.kind(),
                client_form: Some(t.client_form.clone()),
                dates,
                count,
                total: total(&t.draft),
            })
        })
        .collect();
    out.sort_by_key(|r| (r.dates.first().copied(), r.name.to_lowercase()));
    out
}

fn open_skips(
    state: &DemoState,
    new: &BTreeSet<(TemplateId, CalendarDate)>,
) -> Vec<SkippedOccurrence> {
    state
        .occurrences
        .iter()
        .filter(|(_, o)| o.status == OccStatus::Skipped)
        .filter_map(|((id, date), o)| {
            let t = template(state, *id).filter(|t| !t.deleted)?;
            Some(SkippedOccurrence {
                template: *id,
                name: t.name.clone(),
                date: *date,
                reason: SkipReason::from_wire(o.reason.unwrap_or("VALIDATION")),
                new: new.contains(&(*id, *date)),
            })
        })
        .collect()
}

/// `POST /recurring/run-due {asOf, runId, limit}`: posts the due automatic
/// occurrences through `as_of`; also the accounts the posts touched (for
/// the data bus). `today` is the demo's clock (the engine's
/// `CURRENT_DATE`).
///
/// # Errors
/// `Local(InvalidValue)` for an `as_of` after tomorrow (`RUN_DUE_FUTURE`);
/// `Local(PageLimit)` for a limit outside 1–500.
pub fn run_due(
    state: &mut DemoState,
    as_of: CalendarDate,
    run_id: RunId,
    limit: u32,
    today: CalendarDate,
) -> Result<(RunDue, Vec<AccountId>), ApiError> {
    if today.add_days(1).is_ok_and(|tomorrow| as_of > tomorrow) {
        return Err(not_due());
    }
    if !(1..=RUN_LIMIT_MAX).contains(&limit) {
        return Err(ApiError::Local(LocalValidation::PageLimit));
    }
    let lock = closing(state);
    let mut handled = 0u32;
    let mut more = false;
    let mut failed_ids: BTreeSet<TemplateId> = BTreeSet::new();
    let mut failed = Vec::new();
    let mut new_skips = BTreeSet::new();
    let mut touched: Vec<AccountId> = Vec::new();
    while let Some((id, on)) = first_due(state, as_of, &failed_ids) {
        if handled == limit {
            more = true;
            break;
        }
        let Some(t) = template(state, id).cloned() else {
            break;
        };
        // Entered early, dismissed or posted already: never twice.
        let done = state
            .occurrences
            .get(&(id, on))
            .is_some_and(|o| o.status != OccStatus::Skipped);
        if !done {
            // Auto never posts on or before the closing date.
            let posted = if lock.is_some_and(|c| on <= c) {
                Err(Some("CLOSED_PERIOD"))
            } else {
                post_draft(state, &t.draft, on, None).map_err(|e| skip_reason(&e))
            };
            let occ = match posted {
                Ok((txn, accounts)) => {
                    touched.extend(accounts);
                    Occ {
                        status: OccStatus::Posted,
                        transaction: Some(txn),
                        reason: None,
                        run: Some(run_id),
                    }
                }
                Err(Some(reason)) => {
                    new_skips.insert((id, on));
                    Occ {
                        status: OccStatus::Skipped,
                        transaction: None,
                        reason: Some(reason),
                        run: Some(run_id),
                    }
                }
                Err(None) => {
                    // Nothing claimed or advanced; the next run retries.
                    failed_ids.insert(id);
                    failed.push(FailedTemplate {
                        template: id,
                        reason: "INTERNAL".into(),
                    });
                    continue;
                }
            };
            state.occurrences.insert((id, on), occ);
            handled += 1;
        }
        if let Some(t) = state.templates.iter_mut().find(|x| x.id == id) {
            if !done {
                t.last_entered = Some(on);
            }
            advance(t, on);
        }
    }
    let mut posted: Vec<PostedOccurrence> = state
        .occurrences
        .iter()
        .filter(|(_, o)| o.status == OccStatus::Posted && o.run == Some(run_id))
        .filter_map(|((id, date), o)| {
            let t = template(state, *id)?;
            Some(PostedOccurrence {
                template: *id,
                name: t.name.clone(),
                kind: t.draft.kind(),
                client_form: Some(t.client_form.clone()),
                date: *date,
                transaction: o.transaction?,
                total: total(&t.draft),
            })
        })
        .collect();
    posted.sort_by_key(|p| (p.date, p.name.to_lowercase()));
    touched.sort();
    touched.dedup();
    let answer = RunDue {
        as_of,
        run_id,
        more,
        posted,
        reminders: reminders(state, as_of),
        skipped: open_skips(state, &new_skips),
        failed,
    };
    Ok((answer, touched))
}

/// `GET /recurring/attention?asOf=`: due reminders and open skipped dates
/// (changes nothing).
#[must_use]
pub fn attention(state: &DemoState, as_of: CalendarDate) -> Attention {
    Attention {
        reminders: reminders(state, as_of),
        skipped: open_skips(state, &BTreeSet::new()),
    }
}

/// Marks `(id, date)` handled by the owner: an open skipped date, or the
/// schedule's next date (which then moves on).
fn close_occurrence(
    state: &mut DemoState,
    id: TemplateId,
    date: CalendarDate,
    status: OccStatus,
    transaction: Option<TransactionId>,
) -> Result<(), ApiError> {
    let t = live(state, id)?.clone();
    let next = t.schedule.is_some_and(|s| s.next_date == date);
    match state.occurrences.get(&(id, date)) {
        Some(o) if o.status != OccStatus::Skipped => return Err(taken()),
        Some(_) => {}
        None if next => {}
        None => return Err(not_due()),
    }
    state.occurrences.insert(
        (id, date),
        Occ {
            status,
            transaction,
            reason: None,
            run: None,
        },
    );
    if let Some(t) = state.templates.iter_mut().find(|x| x.id == id) {
        if status == OccStatus::Entered {
            t.last_entered = Some(date);
        }
        if next {
            advance(t, date);
        }
    }
    Ok(())
}

/// `POST /templates/{id}/occurrences/{date}/skip`: "Skip this one" or
/// "Dismiss".
///
/// # Errors
/// `NotFound`; 409 `OCCURRENCE_TAKEN` for a date already handled;
/// `Local(InvalidValue)` for a date that is not due (`OCCURRENCE_NOT_DUE`).
pub fn skip(state: &mut DemoState, id: TemplateId, date: CalendarDate) -> Result<(), ApiError> {
    close_occurrence(state, id, date, OccStatus::Dismissed, None)
}

/// `POST /templates/{id}/occurrences/{date}/resolve {transactionId}`: the
/// owner saved the occurrence through its form ("Open in form").
///
/// # Errors
/// As [`skip`]; `Local(InvalidValue)` for a transaction already linked to
/// a date (`OCCURRENCE_RESOLVE_INVALID`).
pub fn resolve(
    state: &mut DemoState,
    id: TemplateId,
    date: CalendarDate,
    txn: TransactionId,
) -> Result<(), ApiError> {
    if state
        .occurrences
        .values()
        .any(|o| o.transaction == Some(txn))
    {
        return Err(not_due());
    }
    close_occurrence(state, id, date, OccStatus::Entered, Some(txn))
}

// ------------------------------------------------------------ sample (WP-D)

fn sample_id<T: std::str::FromStr>(prefix: char, n: u32) -> Option<T> {
    format!("d0000000-0000-4000-8000-{prefix}{n:011}")
        .parse()
        .ok()
}

fn sample_money(s: &str) -> Money {
    Money::parse_wire(s).unwrap_or(Money::ZERO)
}

fn vendor(state: &mut DemoState, name: &str) -> Option<cl_core::ids::ContactId> {
    state
        .contacts_mut()
        .iter()
        .find(|c| c.fields.display_name == name)
        .map(|c| c.id)
}

fn line(account: AccountId, amount: Money) -> SplitLine {
    SplitLine {
        account,
        amount,
        memo: None,
        contact: None,
        billable: false,
        tag: None,
    }
}

fn je_line(account: AccountId, side: cl_api::Side) -> JeLine {
    JeLine {
        account,
        side,
        description: None,
        tag: None,
        contact: None,
    }
}

/// The practice company's templates (P15 WP-D, Q-P15r; the same three as
/// `fixtures/sample/ozark_ridge/templates.json`), added once:
///
/// - "Shop rent" (Monthly bills): a check to print, Remind me monthly from
///   12/01/2026;
/// - "Accounting software" (Monthly bills): a 49.00 card charge, Enter
///   automatically monthly from 09/15/2026, 3 times, so the first open
///   catches up (09/15 falls in the demo's closed period and is skipped;
///   10/15 posts);
/// - "Owner health insurance": a 412.50 journal entry, Don't remind me.
///
/// Synthetic.
pub fn seed_sample(state: &mut DemoState) {
    if state.templates_seeded {
        return;
    }
    state.templates_seeded = true;
    let a = super::account_id;
    let story = DemoState::today();
    let Some(group) = sample_id::<TemplateGroupId>('f', 900) else {
        return;
    };
    if !state.template_groups.iter().any(|g| g.id == group) {
        state.template_groups.push(DemoTemplateGroup {
            id: group,
            name: "Monthly bills".into(),
        });
    }
    let landlord = vendor(state, "Pine Ridge Storage Units");
    let software = vendor(state, "Mountain Software Co");
    let rent = sample_money("1450.00");
    let check = TemplateDraft::Check(CheckDraft {
        bank: a(1010),
        date: story,
        payee: landlord,
        payee_name: "Pine Ridge Storage Units".into(),
        check_number: None,
        print_later: true,
        payee_address: None,
        memo: Some("Shop rent".into()),
        printed_memo: None,
        tag_ids: Vec::new(),
        draft_attachment: None,
        amount: rent,
        lines: vec![line(a(6300), rent)],
    });
    let card = TemplateDraft::CardCharge(CardChargeDraft {
        pay_from: a(2050),
        date: story,
        number: None,
        payee: software,
        memo: Some("Accounting software".into()),
        lines: vec![line(a(6250), sample_money("49.00"))],
    });
    let insurance = sample_money("412.50");
    let je = NewJournalEntry::new(
        story,
        Some("Owner health insurance".into()),
        cl_api::Basis::Both,
        vec![
            je_line(a(30170), cl_api::Side::Debit(insurance)),
            je_line(a(1010), cl_api::Side::Credit(insurance)),
        ],
    );
    let monthly = |mode, next, end| TemplateSchedule {
        mode,
        frequency: Frequency::Monthly,
        interval: 1,
        next_date: next,
        end,
    };
    let mut rows = vec![
        (
            1,
            "Shop rent",
            Some(group),
            check,
            cl_api::client_form::WRITE_CHECK,
            CalendarDate::from_ymd(2026, 12, 1)
                .ok()
                .map(|d| monthly(ScheduleMode::Remind, d, ScheduleEnd::Never)),
        ),
        (
            2,
            "Accounting software",
            Some(group),
            card,
            cl_api::client_form::CARD_CHARGE,
            CalendarDate::from_ymd(2026, 9, 15)
                .ok()
                .map(|d| monthly(ScheduleMode::Auto, d, ScheduleEnd::After(3))),
        ),
    ];
    if let Ok(je) = je {
        rows.push((
            3,
            "Owner health insurance",
            None,
            TemplateDraft::Journal(je),
            cl_api::client_form::JOURNAL_ENTRY,
            None,
        ));
    }
    for (n, name, group, draft, form, schedule) in rows {
        let Some(id) = sample_id::<TemplateId>('e', 900 + n) else {
            continue;
        };
        if state.templates.iter().any(|t| t.id == id) {
            continue;
        }
        state.templates.push(DemoTemplate {
            id,
            name: name.into(),
            group,
            draft,
            client_form: form.into(),
            schedule,
            anchor: schedule.map(|s| s.next_date),
            last_entered: None,
            version: 1,
            deleted: false,
        });
    }
}

#[cfg(test)]
#[path = "templates_tests.rs"]
mod tests;
