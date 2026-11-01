//! The demo engine's journal entries (P15 lane J5): the A/R-A/P name rule,
//! numbering, reversal creation and the exact balanced check, mirroring
//! the part A contract (`target/plans/P15-engine-shapes-a.md`). Synthetic.
//!
//! An entry's lines live in the demo ledger like every other posting (so
//! the registers show it); the fields the ledger has no room for (each
//! line's name and tag, the Adjusting flag and the reversal link) live
//! here, keyed by the entry's id, and are laid over the ledger's record
//! when it is read. The version is the fingerprint of that full record,
//! so a change made in another window is caught as on the engine.

use std::collections::{BTreeMap, BTreeSet};

use cl_api::{
    Account, ApiError, ConflictReason, EditableFields, LocalValidation, NewJournalEntry,
    RecordVersion, Side, Transaction, TxnStatus, Versioned,
};
use cl_core::ids::{AccountId, ContactId, TagId, TransactionId};
use cl_core::{CalendarDate, Money, Secret};

use super::DemoState;
use super::register::{DemoJournal, DemoLedger, DemoLine, check_closing};

/// What the ledger can't hold of one entry.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct JeMeta {
    /// An adjusting entry.
    pub adjusting: bool,
    /// Each line's name, in line order.
    pub contacts: Vec<Option<ContactId>>,
    /// Each line's tag, in line order.
    pub tags: Vec<Option<TagId>>,
    /// The entry that reverses this one.
    pub reversal: Option<TransactionId>,
    /// The entry this one reverses.
    pub reverses: Option<TransactionId>,
}

/// The demo's journal entry fields, by entry.
#[derive(Clone, Debug, Default)]
pub struct DemoJournals {
    meta: BTreeMap<TransactionId, JeMeta>,
}

/// One entry in Prev/Next order: its id, date, number and whether it is
/// void (never an amount).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JeRow {
    /// The entry.
    pub id: TransactionId,
    /// Its date.
    pub date: CalendarDate,
    /// Its number.
    pub number: Option<String>,
    /// Voided.
    pub void: bool,
}

fn chart(state: &mut DemoState) -> Vec<Account> {
    state.chart.get_or_insert_with(super::chart).clone()
}

/// The open company's closing date.
#[must_use]
pub fn closing(state: &DemoState) -> Option<CalendarDate> {
    let open = state.open?;
    state
        .companies
        .iter()
        .find(|c| c.tenant_id == open)
        .and_then(|c| c.closed_through)
}

fn has_role(a: &Account, role: &str) -> bool {
    a.fields.detail_type.as_deref() == Some(role) || a.system_tag.as_deref() == Some(role)
}

/// The chart's Accounts Receivable and Accounts Payable accounts (detail
/// type or system tag, as the engine finds them).
#[must_use]
pub fn ar_ap(chart: &[Account]) -> (BTreeSet<AccountId>, BTreeSet<AccountId>) {
    let pick = |role| {
        chart
            .iter()
            .filter(|a| has_role(a, role))
            .map(|a| a.id)
            .collect()
    };
    (pick("accounts_receivable"), pick("accounts_payable"))
}

fn stale() -> ApiError {
    ApiError::Conflict {
        reason: ConflictReason::Stale,
        current_version: None,
        code: None,
        message: cl_api::UserText::default(),
        details: cl_api::ConflictDetails::default(),
    }
}

fn invalid() -> ApiError {
    ApiError::Local(LocalValidation::InvalidValue)
}

/// The contract's own checks, again (the builder refused them already):
/// a name on every A/R-or-A/P line (any number of them, Q-206), Reverse on
/// after the date, debits equal credits exactly.
fn contract_checks(chart: &[Account], e: &NewJournalEntry) -> Result<(), ApiError> {
    let (ar, ap) = ar_ap(chart);
    if e.lines()
        .iter()
        .any(|l| (ar.contains(&l.account) || ap.contains(&l.account)) && l.contact.is_none())
    {
        return Err(ApiError::Local(LocalValidation::ArApNameMissing));
    }
    if e.reverse_on().is_some_and(|r| r <= e.txn_date()) {
        return Err(ApiError::Local(LocalValidation::ReverseNotAfter));
    }
    let side = |debit: bool| {
        Money::sum(e.lines().iter().filter_map(|l| match (l.side, debit) {
            (Side::Debit(m), true) | (Side::Credit(m), false) => Some(m),
            _ => None,
        }))
        .map_err(|_| invalid())
    };
    if side(true)? != side(false)? {
        return Err(ApiError::Local(LocalValidation::Unbalanced));
    }
    Ok(())
}

fn demo_lines(e: &NewJournalEntry, swap: bool) -> Vec<DemoLine> {
    e.lines()
        .iter()
        .map(|l| {
            let (debit, credit) = match (l.side, swap) {
                (Side::Debit(m), false) | (Side::Credit(m), true) => (m, Money::ZERO),
                (Side::Credit(m), false) | (Side::Debit(m), true) => (Money::ZERO, m),
            };
            DemoLine {
                account: l.account,
                debit,
                credit,
                memo: l.description.clone(),
            }
        })
        .collect()
}

fn meta_of(e: &NewJournalEntry) -> JeMeta {
    JeMeta {
        adjusting: e.adjusting(),
        contacts: e.lines().iter().map(|l| l.contact).collect(),
        tags: e.lines().iter().map(|l| l.tag).collect(),
        reversal: None,
        reverses: None,
    }
}

/// The live reversal of `id` (not void, not deleted).
fn live_reversal(state: &DemoState, id: TransactionId) -> Option<(TransactionId, CalendarDate)> {
    let r = state.journal.meta.get(&id)?.reversal?;
    let date = state.ledger.date_of(r)?;
    (!state.ledger.is_void(r)).then_some((r, date))
}

/// What a body says about Reverse on — the engine's three `PUT` states.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReverseOnField {
    /// The key is absent: a create has no reversal; an edit keeps the
    /// stored reversal on its stored date (re-posted from the edited lines).
    Omitted,
    /// The key is sent: a date adds or moves the reversal; `null` removes it.
    Sent(Option<CalendarDate>),
}

/// Posts (or, with `edit`, replaces) an entry as `cl-api` sends it — with
/// the capability every update carries `reverseOn` explicitly (the date,
/// or `null` to remove the reversal). See [`save_with`].
///
/// # Errors
/// As [`save_with`].
pub fn save(
    state: &mut DemoState,
    e: &NewJournalEntry,
    edit: Option<(TransactionId, RecordVersion)>,
    password: Option<&Secret>,
) -> Result<(TransactionId, RecordVersion), ApiError> {
    save_with(
        state,
        e,
        edit,
        password,
        ReverseOnField::Sent(e.reverse_on()),
    )
}

/// Posts (or, with `edit`, replaces) an entry; with Reverse on, also its
/// reversal: same accounts, debits and credits swapped, dated Reverse on,
/// linked both ways. An edit replaces the old reversal; `reverse` says
/// whether the body named Reverse on at all (when omitted, `e`'s own value
/// is not on the wire and the stored reversal date is kept).
///
/// # Errors
/// `Stale` when `edit`'s version is not the entry's now; the closed-period
/// rule over every date touched (the entry's old and new dates, the old
/// and new reversal's); the contract's refusals as `Local`; `NotFound`.
pub fn save_with(
    state: &mut DemoState,
    e: &NewJournalEntry,
    edit: Option<(TransactionId, RecordVersion)>,
    password: Option<&Secret>,
    reverse: ReverseOnField,
) -> Result<(TransactionId, RecordVersion), ApiError> {
    let chart = chart(state);
    contract_checks(&chart, e)?;
    let mut dates = vec![e.txn_date()];
    let mut old_reversal = None;
    let mut stored_on = None;
    if let Some((id, version)) = &edit {
        let now = get(state, *id)?;
        if now.version != *version {
            return Err(stale());
        }
        dates.push(now.record.txn_date);
        if let Some((r, d)) = live_reversal(state, *id) {
            dates.push(d);
            old_reversal = Some(r);
            stored_on = Some(d);
        }
    }
    let reverse_on = match reverse {
        ReverseOnField::Sent(on) => on,
        ReverseOnField::Omitted => stored_on,
    };
    // The kept date must still be after the (possibly new) entry date.
    if reverse_on.is_some_and(|r| r <= e.txn_date()) {
        return Err(ApiError::Local(LocalValidation::ReverseNotAfter));
    }
    dates.extend(reverse_on);
    check_closing(closing(state), &dates, password)?;
    if let Some(r) = old_reversal {
        state.ledger.delete(r)?;
        state.journal.meta.remove(&r);
    }
    let journal = DemoJournal {
        kind: "journal_entry",
        date: e.txn_date(),
        number: e.number().unwrap_or_default().to_string(),
        contact: None,
        memo: e.memo().unwrap_or_default().to_string(),
        lines: demo_lines(e, false),
    };
    let id = state
        .ledger
        .save_lines(&chart, &journal, edit.as_ref().map(|(id, _)| *id))
        .ok_or_else(invalid)?;
    let mut meta = meta_of(e);
    if let Some(on) = reverse_on {
        let memo = match e.number() {
            Some(n) => format!("Reverses JE {n}"),
            None => format!(
                "Reverses JE of {:02}/{:02}/{}",
                e.txn_date().month(),
                e.txn_date().day(),
                e.txn_date().year()
            ),
        };
        let reversal = DemoJournal {
            kind: "journal_entry",
            date: on,
            number: String::new(),
            contact: None,
            memo,
            lines: demo_lines(e, true),
        };
        let r = state
            .ledger
            .save_lines(&chart, &reversal, None)
            .ok_or_else(invalid)?;
        state.journal.meta.insert(
            r,
            JeMeta {
                reverses: Some(id),
                ..meta_of(e)
            },
        );
        meta.reversal = Some(r);
    }
    // An edited reversal keeps its link to the entry it reverses.
    meta.reverses = state.journal.meta.get(&id).and_then(|m| m.reverses);
    state.journal.meta.insert(id, meta);
    let v = get(state, id)?;
    Ok((id, v.version))
}

/// An entry as `GET /transactions/{id}` answers it, with its names, tags,
/// Adjusting flag and reversal link.
///
/// # Errors
/// `NotFound`.
pub fn get(state: &mut DemoState, id: TransactionId) -> Result<Versioned<Transaction>, ApiError> {
    let chart = chart(state);
    let contacts = state.contacts_mut().clone();
    let tags = state.tags_mut().clone();
    let mut v = state
        .ledger
        .get(&chart, &contacts, id)
        .ok_or(ApiError::NotFound { code: None })?;
    let live = live_reversal(state, id);
    if let Some(m) = state.journal.meta.get(&id) {
        let t = &mut v.record;
        t.is_adjusting = m.adjusting;
        t.reverses_transaction_id = m.reverses;
        t.reversal_transaction_id = live.map(|(r, _)| r);
        t.reverse_on = live.map(|(_, d)| d);
        for (i, l) in t.lines.iter_mut().enumerate() {
            l.contact_id = m.contacts.get(i).copied().flatten();
            l.contact_name = l.contact_id.and_then(|c| {
                contacts
                    .iter()
                    .find(|x| x.id == c)
                    .map(|x| x.fields.display_name.clone())
            });
            l.tag_id = m.tags.get(i).copied().flatten();
            l.tag_name = l.tag_id.and_then(|g| {
                tags.iter()
                    .find(|x| x.id == g)
                    .map(|x| x.fields.name.clone())
            });
        }
    }
    v.version = RecordVersion::Fingerprint(v.record.fingerprint());
    Ok(v)
}

/// Every journal entry, newest first (Prev/Next).
#[must_use]
pub fn list(state: &DemoState) -> Vec<JeRow> {
    let mut rows: Vec<JeRow> = state
        .ledger
        .transactions()
        .into_iter()
        .filter(|t| t.kind == "journal_entry")
        .map(|t| {
            let id = DemoLedger::id_of(&t);
            JeRow {
                id,
                date: t.date,
                number: (!t.number.is_empty()).then(|| t.number.clone()),
                void: state.ledger.is_void(id),
            }
        })
        .collect();
    rows.sort_by(|a, b| b.date.cmp(&a.date).then(b.id.cmp(&a.id)));
    rows
}

/// The suggested next number: the highest all-digit number (up to 19
/// digits) + 1, `"1"` if none. Not reserved.
#[must_use]
pub fn next_number(state: &DemoState) -> String {
    state
        .ledger
        .transactions()
        .iter()
        .filter(|t| t.kind == "journal_entry")
        .filter(|t| {
            !t.number.is_empty()
                && t.number.len() <= 19
                && t.number.bytes().all(|b| b.is_ascii_digit())
        })
        .filter_map(|t| t.number.parse::<u64>().ok())
        .max()
        .map_or_else(|| "1".to_string(), |n| n.saturating_add(1).to_string())
}

/// What a void or delete touches: the entry's date and its live
/// reversal's.
fn targets(
    state: &DemoState,
    id: TransactionId,
) -> Result<(CalendarDate, Option<(TransactionId, CalendarDate)>), ApiError> {
    let date = state
        .ledger
        .date_of(id)
        .ok_or(ApiError::NotFound { code: None })?;
    Ok((date, live_reversal(state, id)))
}

/// Voids an entry and its live reversal (one closing-date password opens
/// both dates).
///
/// # Errors
/// The closed-period rule; `NotFound`; already void.
pub fn void(
    state: &mut DemoState,
    id: TransactionId,
    password: Option<&Secret>,
) -> Result<(), ApiError> {
    let (date, reversal) = targets(state, id)?;
    let mut dates = vec![date];
    dates.extend(reversal.map(|(_, d)| d));
    check_closing(closing(state), &dates, password)?;
    state.ledger.void(id)?;
    if let Some((r, _)) = reversal {
        state.ledger.void(r)?;
    }
    Ok(())
}

/// Deletes an entry and its live reversal.
///
/// # Errors
/// The closed-period rule; `NotFound`.
pub fn delete(
    state: &mut DemoState,
    id: TransactionId,
    password: Option<&Secret>,
) -> Result<(), ApiError> {
    let (date, reversal) = targets(state, id)?;
    let mut dates = vec![date];
    dates.extend(reversal.map(|(_, d)| d));
    check_closing(closing(state), &dates, password)?;
    state.ledger.delete(id)?;
    if let Some((r, _)) = reversal {
        state.ledger.delete(r)?;
        state.journal.meta.remove(&r);
    }
    state.journal.meta.remove(&id);
    Ok(())
}

/// True when the record is void (the form shows it read-only).
#[must_use]
pub fn is_void(v: &Versioned<Transaction>) -> bool {
    v.record.status == TxnStatus::Void
}

#[cfg(test)]
mod tests;
