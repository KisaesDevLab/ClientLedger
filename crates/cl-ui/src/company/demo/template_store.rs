//! The demo engine's stored templates (P15 lane T, U-62): the twin of
//! `/templates` CRUD, groups, `GET /{id}/draft` and `enter`, so `--demo`
//! runs the Templates list and Save as Template. Synthetic.
//!
//! - The draft is kept as the form's checked draft and checked again
//!   through [`TemplateDraft::to_value`], as the engine runs each kind's
//!   builder: a grouped deposit or an unbalanced entry is refused.
//! - Names are unique per company among live templates (409
//!   `TEMPLATE_NAME_TAKEN`); a write naming an old version is 409 `STALE`.
//! - `draft` nulls an account the chart no longer has active and lists it
//!   (`ACCOUNT_INACTIVE` / `NOT_FOUND`).
//! - `enter` posts through the same demo savers the forms use, under the
//!   occurrence key in [`DemoState::occurrences`] (409 `OCCURRENCE_TAKEN`
//!   for a date already handled), with the closing-date password rule.
//!   Lane C's run-due twin reuses [`post_draft`] and [`next_after`].
//! - The list total is the demo engine's header amount: display only.

use cl_api::{
    ApiError, Capabilities, ConflictReason, DraftWarning, DraftWarningCode, Entered,
    LocalValidation, MISSING_ACCOUNT, NewTemplate, ScheduleEnd, Template, TemplateChange,
    TemplateDraft, TemplateGroup, TemplateKind, TemplateSchedule, TemplateUse, UserText,
};
use cl_core::ids::{AccountId, TemplateGroupId, TemplateId, TransactionId};
use cl_core::{CalendarDate, Money, Secret};
use serde_json::Value;

use super::DemoState;
use super::register::{DemoJournal, DemoLine, DemoPosting, check_closing};
use super::templates::{DemoTemplate, DemoTemplateGroup, Occ, OccStatus};

/// What the demo engine advertises for template drafts (all of it).
#[must_use]
pub fn demo_caps() -> Capabilities {
    serde_json::from_value(serde_json::json!([
        Capabilities::DEPOSIT_CASHBACK,
        Capabilities::LINE_BILLABLE,
        Capabilities::TEMPLATES,
        // The demo journal entry has names and Adjusting (part A).
        Capabilities::JE_NAMES,
        Capabilities::JE_ADJUSTING,
    ]))
    .unwrap_or_default()
}

pub(crate) fn conflict(reason: ConflictReason, code: &str) -> ApiError {
    ApiError::Conflict {
        reason,
        current_version: None,
        code: cl_api::ErrorCode::parse(code),
        message: UserText::default(),
    }
}

fn not_found() -> ApiError {
    ApiError::NotFound { code: None }
}

fn invalid() -> ApiError {
    ApiError::Local(LocalValidation::InvalidValue)
}

pub(crate) fn closing(state: &DemoState) -> Option<CalendarDate> {
    let open = state.open?;
    state
        .companies
        .iter()
        .find(|c| c.tenant_id == open)
        .and_then(|c| c.closed_through)
}

fn chart(state: &mut DemoState) -> Vec<cl_api::Account> {
    state.chart.get_or_insert_with(super::chart).clone()
}

/// The schedule in the dialog's terms (the shared clamped date rule).
#[must_use]
pub fn to_model(s: &TemplateSchedule) -> cl_model::templates::Schedule {
    use cl_api::Frequency as F;
    use cl_model::templates::{Ends, Frequency as M};
    cl_model::templates::Schedule {
        frequency: match s.frequency {
            F::Daily => M::Daily,
            F::Weekly => M::Weekly,
            F::Biweekly => M::Biweekly,
            F::Semimonthly => M::Semimonthly,
            F::Monthly => M::Monthly,
            F::Quarterly => M::Quarterly,
            F::Annually => M::Annually,
        },
        interval: s.interval,
        next_date: s.next_date,
        ends: match s.end {
            ScheduleEnd::Never => Ends::Never,
            ScheduleEnd::On(d) => Ends::On(Some(d)),
            ScheduleEnd::After(n) => Ends::After(Some(n)),
        },
    }
}

/// The first date of `t`'s series after `d` (the engine's `nextAfter`,
/// counted from the anchor and clamped to month ends). `None` when the
/// schedule has ended.
#[must_use]
#[allow(
    clippy::many_single_char_names,
    reason = "the engine's nextAfter, as written there"
)]
pub fn next_after(t: &DemoTemplate, d: CalendarDate) -> Option<CalendarDate> {
    let s = t.schedule?;
    let mut m = to_model(&s);
    m.next_date = t.anchor.unwrap_or(s.next_date);
    m.ends = match m.ends {
        // The count is spent per occurrence; here only the date matters.
        cl_model::templates::Ends::After(_) => cl_model::templates::Ends::Never,
        e => e,
    };
    // Enough steps for 30 years of daily dates.
    cl_model::templates::next_dates(&m, 11_000)
        .into_iter()
        .find(|x| *x > d)
}

/// The demo engine's header total of a draft (display only).
pub(crate) fn total(d: &TemplateDraft) -> Option<Money> {
    match d {
        TemplateDraft::Check(c) => Some(c.amount),
        TemplateDraft::Transfer(t) => Some(t.amount),
        TemplateDraft::CardCharge(c) => Money::sum(c.lines.iter().map(|l| l.amount)).ok(),
        TemplateDraft::Deposit(d) => d.total(),
        TemplateDraft::Journal(j) => Money::sum(j.lines().iter().filter_map(|l| match l.side {
            cl_api::Side::Debit(m) => Some(m),
            cl_api::Side::Credit(_) => None,
        }))
        .ok(),
        TemplateDraft::DailySales(_) => None,
    }
}

fn main_account(d: &TemplateDraft) -> Option<AccountId> {
    match d {
        TemplateDraft::Check(c) => Some(c.bank),
        TemplateDraft::Transfer(t) => Some(t.from),
        TemplateDraft::CardCharge(c) => Some(c.pay_from),
        TemplateDraft::Deposit(d) => Some(d.account),
        TemplateDraft::Journal(j) => j.lines().first().map(|l| l.account),
        TemplateDraft::DailySales(_) => None,
    }
}

fn contact(d: &TemplateDraft) -> Option<cl_core::ids::ContactId> {
    match d {
        TemplateDraft::Check(c) => c.payee,
        TemplateDraft::CardCharge(c) => c.payee,
        _ => None,
    }
}

fn row(state: &DemoState, t: &DemoTemplate) -> Template {
    let group = t
        .group
        .and_then(|g| state.template_groups.iter().find(|x| x.id == g));
    let open_skipped = state
        .occurrences
        .iter()
        .filter(|((id, _), o)| *id == t.id && o.status == OccStatus::Skipped)
        .count();
    let caps = demo_caps();
    Template {
        id: t.id,
        name: t.name.clone(),
        kind: t.draft.kind(),
        client_form: Some(t.client_form.clone()),
        group: group.map(|g| g.id),
        group_name: group.map(|g| g.name.clone()),
        account: main_account(&t.draft),
        contact: contact(&t.draft),
        total: total(&t.draft),
        schedule: t.schedule,
        last_entered: t.last_entered,
        open_skipped: u32::try_from(open_skipped).unwrap_or(u32::MAX),
        legacy: false,
        version: t.version,
        draft: t.draft.to_value(&caps).ok(),
    }
}

/// `GET /templates`: live templates by name.
#[must_use]
pub fn list(state: &DemoState) -> Vec<Template> {
    let mut rows: Vec<Template> = state
        .templates
        .iter()
        .filter(|t| !t.deleted)
        .map(|t| row(state, t))
        .collect();
    rows.sort_by_key(|r| r.name.to_lowercase());
    rows
}

/// `GET /templates/{id}`.
///
/// # Errors
/// `NotFound` for an unknown or deleted template.
pub fn get(state: &DemoState, id: TemplateId) -> Result<Template, ApiError> {
    live(state, id).map(|t| row(state, t))
}

pub(crate) fn live(state: &DemoState, id: TemplateId) -> Result<&DemoTemplate, ApiError> {
    state
        .templates
        .iter()
        .find(|t| t.id == id && !t.deleted)
        .ok_or_else(not_found)
}

fn name_taken(state: &DemoState, name: &str, except: Option<TemplateId>) -> bool {
    let n = name.trim().to_lowercase();
    state
        .templates
        .iter()
        .any(|t| !t.deleted && Some(t.id) != except && t.name.trim().to_lowercase() == n)
}

fn check_schedule(d: &TemplateDraft, s: Option<&TemplateSchedule>) -> Result<(), ApiError> {
    if let Some(s) = s {
        s.to_value().map_err(ApiError::Local)?;
        if s.mode == cl_api::ScheduleMode::Auto && !d.allows_auto() {
            // 400 TEMPLATE_AUTO_UNSUPPORTED.
            return Err(ApiError::Local(LocalValidation::InvalidValue));
        }
    }
    Ok(())
}

/// `POST /templates`.
///
/// # Errors
/// The draft's own refusals (`TemplateGrouping`, `TotalMismatch`, …),
/// `InvalidValue` (name, schedule, auto on daily sales), 409
/// `TEMPLATE_NAME_TAKEN`.
pub fn create(state: &mut DemoState, t: &NewTemplate) -> Result<Template, ApiError> {
    t.draft.to_value(&demo_caps()).map_err(ApiError::Local)?;
    let name = t.name.trim();
    if name.is_empty() || name.chars().count() > cl_api::TEMPLATE_NAME_MAX {
        return Err(invalid());
    }
    check_schedule(&t.draft, t.schedule.as_ref())?;
    if name_taken(state, name, None) {
        return Err(conflict(
            ConflictReason::AlreadyExists,
            cl_api::TEMPLATE_NAME_TAKEN,
        ));
    }
    if let Some(g) = t.group
        && !state.template_groups.iter().any(|x| x.id == g)
    {
        return Err(not_found());
    }
    let id = TemplateId::parse(&format!(
        "d0000000-0000-4000-8000-e{:011}",
        state.templates.len() + 1
    ))
    .map_err(|_| invalid())?;
    state.templates.push(DemoTemplate {
        id,
        name: name.to_string(),
        group: t.group,
        draft: t.draft.clone(),
        client_form: t
            .client_form
            .clone()
            .unwrap_or_else(|| t.draft.client_form().to_string()),
        schedule: t.schedule,
        anchor: t.schedule.map(|s| s.next_date),
        last_entered: None,
        version: 1,
        deleted: false,
    });
    get(state, id)
}

/// `PUT /templates/{id}`.
///
/// # Errors
/// `NotFound`; 409 `STALE` for another version; as [`create`].
pub fn update(
    state: &mut DemoState,
    id: TemplateId,
    version: u32,
    c: &TemplateChange,
) -> Result<Template, ApiError> {
    let now = live(state, id)?;
    if now.version != version {
        return Err(ApiError::Conflict {
            reason: ConflictReason::Stale,
            current_version: None,
            code: cl_api::ErrorCode::parse("STALE"),
            message: UserText::default(),
        });
    }
    let draft = c.draft.clone().unwrap_or_else(|| now.draft.clone());
    if c.draft.is_some() {
        draft.to_value(&demo_caps()).map_err(ApiError::Local)?;
        if draft.kind() != now.draft.kind() {
            return Err(invalid());
        }
    }
    let schedule = c.schedule.unwrap_or(now.schedule);
    check_schedule(&draft, schedule.as_ref())?;
    if let Some(n) = &c.name {
        let n = n.trim();
        if n.is_empty() || n.chars().count() > cl_api::TEMPLATE_NAME_MAX {
            return Err(invalid());
        }
        if name_taken(state, n, Some(id)) {
            return Err(conflict(
                ConflictReason::AlreadyExists,
                cl_api::TEMPLATE_NAME_TAKEN,
            ));
        }
    }
    let t = state
        .templates
        .iter_mut()
        .find(|t| t.id == id)
        .ok_or_else(not_found)?;
    if let Some(n) = &c.name {
        t.name = n.trim().to_string();
    }
    if let Some(g) = c.group {
        t.group = g;
    }
    if let Some(f) = &c.client_form {
        t.client_form.clone_from(f);
    } else if c.draft.is_some() {
        t.client_form = draft.client_form().to_string();
    }
    if c.schedule.is_some() {
        // Setting the next date also sets the anchor.
        t.anchor = schedule.map(|s| s.next_date);
    }
    t.schedule = schedule;
    t.draft = draft;
    t.version += 1;
    get(state, id)
}

/// `DELETE /templates/{id}?version=` (soft delete; its posted transactions
/// and occurrences stay).
///
/// # Errors
/// `NotFound`; 409 `STALE`.
pub fn delete(state: &mut DemoState, id: TemplateId, version: u32) -> Result<(), ApiError> {
    let now = live(state, id)?;
    if now.version != version {
        return Err(conflict(ConflictReason::Stale, "STALE"));
    }
    if let Some(t) = state.templates.iter_mut().find(|t| t.id == id) {
        t.deleted = true;
        t.schedule = None;
        t.version += 1;
    }
    Ok(())
}

/// `GET /templates/groups`, by name.
#[must_use]
pub fn groups(state: &DemoState) -> Vec<TemplateGroup> {
    let mut g: Vec<TemplateGroup> = state
        .template_groups
        .iter()
        .map(|g| TemplateGroup {
            id: g.id,
            name: g.name.clone(),
        })
        .collect();
    g.sort_by_key(|g| g.name.to_lowercase());
    g
}

/// `POST /templates/groups`.
///
/// # Errors
/// `InvalidValue` (empty or over 100 characters); 409 for a name in use.
pub fn create_group(state: &mut DemoState, name: &str) -> Result<TemplateGroup, ApiError> {
    let n = name.trim();
    if n.is_empty() || n.chars().count() > cl_api::TEMPLATE_GROUP_NAME_MAX {
        return Err(invalid());
    }
    if state
        .template_groups
        .iter()
        .any(|g| g.name.to_lowercase() == n.to_lowercase())
    {
        return Err(conflict(
            ConflictReason::AlreadyExists,
            "TEMPLATE_GROUP_TAKEN",
        ));
    }
    let id = TemplateGroupId::parse(&format!(
        "d0000000-0000-4000-8000-f{:011}",
        state.template_groups.len() + 1
    ))
    .map_err(|_| invalid())?;
    state.template_groups.push(DemoTemplateGroup {
        id,
        name: n.to_string(),
    });
    Ok(TemplateGroup {
        id,
        name: n.to_string(),
    })
}

/// `DELETE /templates/groups/{id}`: its templates move to no group.
pub fn delete_group(state: &mut DemoState, id: TemplateGroupId) {
    state.template_groups.retain(|g| g.id != id);
    for t in &mut state.templates {
        if t.group == Some(id) {
            t.group = None;
        }
    }
}

/// Nulls every account the chart no longer has active, listing it.
fn checked_accounts(
    chart: &[cl_api::Account],
    v: &mut Value,
    at: &str,
    out: &mut Vec<DraftWarning>,
) {
    match v {
        Value::Object(map) => {
            for (k, x) in map.iter_mut() {
                let path = if at.is_empty() {
                    k.clone()
                } else {
                    format!("{at}.{k}")
                };
                let is_account = k == "accountId" || k.ends_with("AccountId");
                if is_account && let Value::String(s) = x {
                    let found = AccountId::parse(s)
                        .ok()
                        .and_then(|id| chart.iter().find(|a| a.id == id));
                    let code = match found {
                        None => Some(DraftWarningCode::NotFound),
                        Some(a) if !a.is_active => Some(DraftWarningCode::AccountInactive),
                        Some(_) => None,
                    };
                    if let Some(code) = code {
                        *x = Value::Null;
                        out.push(DraftWarning { field: path, code });
                    }
                } else {
                    checked_accounts(chart, x, &path, out);
                }
            }
        }
        Value::Array(a) => {
            for (i, x) in a.iter_mut().enumerate() {
                checked_accounts(chart, x, &format!("{at}[{i}]"), out);
            }
        }
        _ => {}
    }
}

/// `GET /templates/{id}/draft?date=` (posts nothing).
///
/// # Errors
/// `NotFound`; `Decode` for a draft that does not read back.
pub fn draft(
    state: &mut DemoState,
    id: TemplateId,
    date: CalendarDate,
) -> Result<TemplateUse, ApiError> {
    let chart = chart(state);
    let t = live(state, id)?;
    let mut v = t.draft.to_value(&demo_caps()).map_err(ApiError::Local)?;
    let mut warnings = Vec::new();
    checked_accounts(&chart, &mut v, "", &mut warnings);
    let draft = TemplateDraft::from_value(&t.draft.kind(), date, &v)?;
    Ok(TemplateUse {
        draft,
        client_form: Some(t.client_form.clone()),
        warnings,
    })
}

fn contact_name(state: &mut DemoState, id: Option<cl_core::ids::ContactId>) -> Option<String> {
    let id = id?;
    state
        .contacts_mut()
        .iter()
        .find(|c| c.id == id)
        .map(|c| c.fields.display_name.clone())
}

/// Posts `draft` on `date` through the demo savers the forms use; the
/// accounts it touched. Daily sales is entered from its own form.
///
/// # Errors
/// `ClosedPeriod` / `ClosingPasswordRejected`; `Local(InvalidValue)` for a
/// missing account or a draft the demo ledger refuses; `Validation` for
/// a daily-sales template.
pub fn post_draft(
    state: &mut DemoState,
    draft: &TemplateDraft,
    date: CalendarDate,
    password: Option<&Secret>,
) -> Result<(TransactionId, Vec<AccountId>), ApiError> {
    let chart = chart(state);
    let mut touched: Vec<AccountId> = Vec::new();
    let in_chart =
        |a: AccountId| a != MISSING_ACCOUNT && chart.iter().any(|x| x.id == a && x.is_active);
    let id = match draft {
        TemplateDraft::Check(c) => {
            let mut c = c.clone();
            c.date = date;
            if !c.print_later && c.check_number.is_none() {
                c.check_number = state.ledger.next_check_number(&chart, c.bank);
            }
            if !in_chart(c.bank) || !c.lines.iter().all(|l| in_chart(l.account)) {
                return Err(invalid());
            }
            touched.push(c.bank);
            touched.extend(c.lines.iter().map(|l| l.account));
            super::checks::create(state, &c, password)?
        }
        TemplateDraft::Deposit(d) => {
            let mut d = d.clone();
            d.date = date;
            if !in_chart(d.account) {
                return Err(invalid());
            }
            touched.push(d.account);
            touched.extend(d.lines.iter().chain(&d.deductions).map(|l| l.account));
            super::deposits::create(state, &d, password)?
        }
        TemplateDraft::Transfer(t) => {
            check_closing(closing(state), &[date], password)?;
            if !in_chart(t.from) || !in_chart(t.to) {
                return Err(invalid());
            }
            touched.extend([t.from, t.to]);
            let p = DemoPosting {
                kind: "transfer",
                date,
                number: String::new(),
                contact: None,
                memo: t.memo.clone().unwrap_or_default(),
                own: t.from,
                own_debit: false,
                lines: vec![(t.to, t.amount)],
            };
            state.ledger.save(&chart, &p, None).ok_or_else(invalid)?
        }
        TemplateDraft::CardCharge(c) => {
            check_closing(closing(state), &[date], password)?;
            if !in_chart(c.pay_from) || !c.lines.iter().all(|l| in_chart(l.account)) {
                return Err(invalid());
            }
            touched.push(c.pay_from);
            touched.extend(c.lines.iter().map(|l| l.account));
            let p = DemoPosting {
                kind: "card_charge",
                date,
                number: String::new(),
                contact: contact_name(state, c.payee),
                memo: c.memo.clone().unwrap_or_default(),
                own: c.pay_from,
                own_debit: false,
                lines: c.lines.iter().map(|l| (l.account, l.amount)).collect(),
            };
            state.ledger.save(&chart, &p, None).ok_or_else(invalid)?
        }
        TemplateDraft::Journal(j) => {
            check_closing(closing(state), &[date], password)?;
            if !j.lines().iter().all(|l| in_chart(l.account)) {
                return Err(invalid());
            }
            touched.extend(j.lines().iter().map(|l| l.account));
            let lines = j
                .lines()
                .iter()
                .map(|l| {
                    let (debit, credit) = match l.side {
                        cl_api::Side::Debit(m) => (m, Money::ZERO),
                        cl_api::Side::Credit(m) => (Money::ZERO, m),
                    };
                    DemoLine {
                        account: l.account,
                        debit,
                        credit,
                        memo: l.description.clone(),
                    }
                })
                .collect();
            let journal = DemoJournal {
                kind: "journal_entry",
                date,
                number: String::new(),
                contact: None,
                memo: j.memo().unwrap_or_default().to_string(),
                lines,
            };
            state
                .ledger
                .save_lines(&chart, &journal, None)
                .ok_or_else(invalid)?
        }
        TemplateDraft::DailySales(_) => {
            return Err(invalid());
        }
    };
    Ok((id, touched))
}

/// `POST /templates/{id}/enter {occurrenceDate?, closingPassword?}`: posts
/// one occurrence now. The date defaults to the schedule's next date, or
/// `today` without a schedule (then no occurrence is kept).
///
/// # Errors
/// `NotFound`; 409 `OCCURRENCE_TAKEN`; as [`post_draft`].
pub fn enter(
    state: &mut DemoState,
    id: TemplateId,
    date: Option<CalendarDate>,
    today: CalendarDate,
    password: Option<&Secret>,
) -> Result<(Entered, Vec<AccountId>), ApiError> {
    let t = live(state, id)?.clone();
    let on = date.or(t.schedule.map(|s| s.next_date)).unwrap_or(today);
    let keyed = t.schedule.is_some();
    let open_skip = match state.occurrences.get(&(id, on)) {
        // Posted, entered or dismissed: that date is done.
        Some(o) if keyed && o.status != OccStatus::Skipped => {
            return Err(conflict(
                ConflictReason::Unspecified,
                cl_api::OCCURRENCE_TAKEN,
            ));
        }
        Some(_) => true,
        None => false,
    };
    // E2: the next date (even early) or an open skipped date; any other
    // date is 400 OCCURRENCE_NOT_DUE.
    if keyed && !open_skip && t.schedule.is_some_and(|s| s.next_date != on) {
        return Err(invalid());
    }
    let (txn, touched) = post_draft(state, &t.draft, on, password)?;
    if keyed {
        state.occurrences.insert(
            (id, on),
            Occ {
                status: OccStatus::Entered,
                transaction: Some(txn),
                reason: None,
                run: None,
            },
        );
    }
    if let Some(s) = state.templates.iter_mut().find(|x| x.id == id) {
        s.last_entered = Some(on);
        super::templates::advance(s, on);
    }
    Ok((
        Entered {
            transaction: txn,
            occurrence: keyed.then_some(on),
        },
        touched,
    ))
}

/// The kind of a stored template (for the shell's form choice).
#[must_use]
pub fn kind_of(state: &DemoState, id: TemplateId) -> Option<TemplateKind> {
    live(state, id).ok().map(|t| t.draft.kind())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cl_api::{Frequency, ScheduleMode, SplitLine, TransferDraft};

    fn state() -> DemoState {
        let mut s = DemoState::new();
        s.open = s.companies.first().map(|c| c.tenant_id);
        s
    }

    fn d(y: i32, m: u32, day: u32) -> CalendarDate {
        CalendarDate::from_ymd(y, m, day).expect("date")
    }

    fn transfer(amount: &str) -> TemplateDraft {
        TemplateDraft::Transfer(TransferDraft {
            from: super::super::account_id(1010),
            to: super::super::account_id(1020),
            date: d(2026, 11, 4),
            amount: Money::parse_wire(amount).expect("m"),
            memo: None,
        })
    }

    fn new(name: &str, draft: TemplateDraft, schedule: Option<TemplateSchedule>) -> NewTemplate {
        NewTemplate {
            name: name.into(),
            group: None,
            client_form: None,
            draft,
            schedule,
        }
    }

    fn monthly(next: CalendarDate) -> TemplateSchedule {
        TemplateSchedule {
            mode: ScheduleMode::Remind,
            frequency: Frequency::Monthly,
            interval: 1,
            next_date: next,
            end: ScheduleEnd::Never,
        }
    }

    #[test]
    fn crud_names_and_versions() {
        let mut s = state();
        let made = create(&mut s, &new("Savings", transfer("250.00"), None)).expect("made");
        assert_eq!(made.version, 1);
        assert_eq!(made.total, Some(Money::parse_wire("250").expect("m")));
        let e = create(&mut s, &new(" savings ", transfer("1.00"), None)).expect_err("taken");
        assert!(cl_api::is_name_taken(&e));
        let up = update(
            &mut s,
            made.id,
            1,
            &TemplateChange {
                schedule: Some(Some(monthly(d(2026, 11, 30)))),
                ..TemplateChange::default()
            },
        )
        .expect("up");
        assert_eq!(up.version, 2);
        let stale = update(&mut s, made.id, 1, &TemplateChange::default()).expect_err("stale");
        assert!(matches!(
            stale,
            ApiError::Conflict {
                reason: ConflictReason::Stale,
                ..
            }
        ));
        let g = create_group(&mut s, "Monthly bills").expect("group");
        update(
            &mut s,
            made.id,
            2,
            &TemplateChange {
                group: Some(Some(g.id)),
                ..TemplateChange::default()
            },
        )
        .expect("grouped");
        assert_eq!(list(&s)[0].group_name.as_deref(), Some("Monthly bills"));
        delete_group(&mut s, g.id);
        assert_eq!(list(&s)[0].group, None, "the template stays, ungrouped");
        delete(&mut s, made.id, 3).expect("deleted");
        assert!(list(&s).is_empty(), "gone from the list");
        assert!(get(&s, made.id).is_err());
    }

    #[test]
    fn refusals_follow_the_forms_builders() {
        let mut s = state();
        let mut dep = cl_api::DepositDraft::plain(
            super::super::account_id(1010),
            d(2026, 11, 4),
            None,
            vec![SplitLine {
                account: super::super::account_id(4000),
                amount: Money::parse_wire("10").expect("m"),
                memo: None,
                contact: None,
                billable: false,
                tag: None,
            }],
        );
        dep.payments.push(cl_api::GroupedPayment {
            source: TransactionId::parse("d0000000-0000-4000-8000-b00000000001").expect("id"),
            kind: cl_api::DepositSource::Payment,
            account: super::super::account_id(1499),
            amount: Money::parse_wire("5").expect("m"),
            contact: None,
            memo: None,
        });
        assert_eq!(
            create(&mut s, &new("Grouped", TemplateDraft::Deposit(dep), None)).expect_err("g"),
            ApiError::Local(LocalValidation::TemplateGrouping)
        );
        let daily = TemplateDraft::DailySales(cl_api::DailyEntryDraft {
            template: cl_api::MISSING_LAYOUT,
            date: d(2026, 11, 4),
            tag: None,
            notes: None,
            values: Vec::new(),
        });
        let mut auto = monthly(d(2026, 11, 5));
        auto.mode = ScheduleMode::Auto;
        assert!(create(&mut s, &new("Counter", daily, Some(auto))).is_err());
        assert!(s.templates.is_empty(), "nothing stored");
    }

    #[test]
    fn enter_posts_once_per_date_and_advances_on_the_clamped_rule() {
        let mut s = state();
        let made = create(
            &mut s,
            &new("Savings", transfer("25.00"), Some(monthly(d(2026, 10, 31)))),
        )
        .expect("made");
        let today = d(2026, 11, 4);
        let (e, touched) = enter(&mut s, made.id, None, today, None).expect("entered");
        assert_eq!(e.occurrence, Some(d(2026, 10, 31)));
        assert_eq!(touched.len(), 2);
        let now = get(&s, made.id).expect("get");
        assert_eq!(now.schedule.expect("s").next_date, d(2026, 11, 30));
        assert_eq!(now.last_entered, Some(d(2026, 10, 31)));
        let taken = enter(&mut s, made.id, Some(d(2026, 10, 31)), today, None).expect_err("again");
        assert!(cl_api::is_occurrence_taken(&taken));
        // The anchor stays the 31st: Nov 30 → Dec 31.
        enter(&mut s, made.id, None, today, None).expect("nov");
        assert_eq!(
            get(&s, made.id).expect("g").schedule.expect("s").next_date,
            d(2026, 12, 31)
        );
        let c = chart(&mut s);
        assert!(s.ledger.get(&c, &[], e.transaction).is_some());
    }

    #[test]
    fn closed_period_needs_the_password() {
        let mut s = state();
        let made = create(
            &mut s,
            &new("Old", transfer("5.00"), Some(monthly(d(2026, 9, 15)))),
        )
        .expect("made");
        let today = d(2026, 11, 4);
        let e = enter(&mut s, made.id, None, today, None).expect_err("closed");
        assert!(matches!(e, ApiError::ClosedPeriod(_)), "{e:?}");
        assert!(s.occurrences.is_empty(), "nothing kept");
        let pw = Secret::new(super::super::register::DEMO_CLOSING_PASSWORD.to_string());
        enter(&mut s, made.id, None, today, Some(&pw)).expect("with password");
    }

    #[test]
    fn draft_flags_inactive_accounts_and_leaves_them_empty() {
        let mut s = state();
        let made = create(&mut s, &new("Savings", transfer("25.00"), None)).expect("made");
        let _ = chart(&mut s);
        if let Some(c) = s.chart.as_mut() {
            for a in c.iter_mut() {
                if a.id == super::super::account_id(1010) {
                    a.is_active = false;
                }
            }
        }
        let u = draft(&mut s, made.id, d(2026, 11, 4)).expect("draft");
        let TemplateDraft::Transfer(t) = u.draft else {
            panic!("transfer");
        };
        assert_eq!(t.from, MISSING_ACCOUNT);
        assert_eq!(u.warnings[0].code, DraftWarningCode::AccountInactive);
        assert_eq!(u.warnings[0].field, "fromAccountId");
        assert!(enter(&mut s, made.id, None, d(2026, 11, 4), None).is_err());
    }
}
