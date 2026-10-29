//! The demo "engine" for Write Check (P13 WP6): creating, editing, listing
//! and voiding checks over the demo ledger, the next check numbers and the
//! bank account's balance.
//!
//! It plays the engine's part for `--demo` only, including its rules: a
//! Print later check has no number and waits to be printed (`queue`); a
//! number already used by a non-void check on the bank account is refused
//! (409); voiding a check frees its number; a change dated on or before
//! the closing date needs the closing-date password ("demo-close"). The
//! check amount must equal its lines exactly, as with `check-edit-v1`.
//!
//! The screen never adds anything up: the balance comes from here (the
//! demo register summary), as it comes from the engine in a live company.
//! All data is synthetic.

use std::collections::BTreeMap;
use std::fmt;

use cl_api::{
    ApiError, CheckDraft, CheckRecord, CheckSummary, ConflictReason, EditableFields,
    LocalValidation, PrintStatus, RecordVersion, RegisterAmount, UserText, Versioned,
};
use cl_core::ids::{AccountId, ContactId, TagId, TransactionId};
use cl_core::{CalendarDate, Money, Secret};

use super::DemoState;
use super::register::{DemoLedger, DemoPosting, check_closing, summary};

/// The checks written in the demo: what the ledger itself does not keep.
#[derive(Clone, Default)]
pub struct DemoChecks {
    info: BTreeMap<TransactionId, CheckInfo>,
    history: Vec<(TransactionId, &'static str)>,
    /// Write requests that reached the demo engine (creates, edits, voids).
    /// Tests use it to prove that a refused save sent nothing.
    pub writes: u32,
    /// Files attached in the demo (kept in memory only).
    files: Vec<DemoFile>,
    /// Files deleted so far (ids are never reused).
    deleted: usize,
    /// The owner's rules (none match the story by default).
    pub rules: Vec<DemoRule>,
    /// AI preview requests that reached the demo engine (tests prove that
    /// nothing is asked with AI off).
    pub ai_requests: u32,
    /// Rule evaluations that reached the demo engine.
    pub rule_requests: u32,
    /// Behave like an engine without `check-edit-v1` (tests): a check is
    /// changed through [`update_expense`].
    pub check_edit_off: bool,
    /// Behave like an engine without `line-billable-v1` (tests).
    pub line_billable_off: bool,
}

impl DemoChecks {
    /// Write Check keeps the details of check `id` (the demo's stand-in
    /// for a print status).
    #[must_use]
    pub fn knows(&self, id: TransactionId) -> bool {
        self.info.contains_key(&id)
    }
}

/// A demo rule: a memo containing `contains` suggests `account`.
#[derive(Clone, Debug)]
pub struct DemoRule {
    /// Its name (shown in the suggestion caption).
    pub name: String,
    /// Text the description must contain (case-insensitive).
    pub contains: String,
    /// The account it sets.
    pub account: AccountId,
}

/// A file in the demo attachment store.
#[derive(Clone)]
struct DemoFile {
    id: cl_api::AttachmentId,
    /// `draft` or `expense`.
    kind: &'static str,
    /// The draft key or the check id.
    to: String,
    name: String,
    bytes: Vec<u8>,
}

impl fmt::Debug for DemoChecks {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DemoChecks")
            .field("checks", &self.info.len())
            .field("writes", &self.writes)
            .finish_non_exhaustive()
    }
}

/// A check's own fields (the ledger keeps the posting).
#[derive(Clone)]
struct CheckInfo {
    print_status: PrintStatus,
    payee_name: String,
    address: Option<String>,
    printed_memo: Option<String>,
    tag_ids: Vec<TagId>,
    /// Per expense line, in entry order: its customer and billable mark.
    lines: Vec<(Option<ContactId>, bool)>,
}

/// One entry of a check's history (newest last).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HistoryEntry {
    /// `create`, `update` or `void`.
    pub action: &'static str,
}

fn closing(state: &DemoState) -> Option<CalendarDate> {
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

fn invalid() -> ApiError {
    ApiError::Local(LocalValidation::InvalidValue)
}

/// The engine's 409 for a check number already used on the bank account
/// (it sends no code).
fn number_used() -> ApiError {
    ApiError::Conflict {
        reason: ConflictReason::Unspecified,
        current_version: None,
        code: None,
        message: UserText::default(),
    }
}

fn taken(
    ledger: &DemoLedger,
    chart: &[cl_api::Account],
    bank: AccountId,
    n: u32,
    except: Option<TransactionId>,
) -> bool {
    ledger
        .check_numbers(chart, bank)
        .iter()
        .any(|(id, num, void)| *num == n && !*void && Some(*id) != except)
}

/// The next number the demo engine keeps for `bank` (none: the screen
/// proposes from the highest number) and the highest number of a non-void
/// check on it.
#[must_use]
pub fn numbers(state: &mut DemoState, bank: AccountId) -> (Option<u32>, Option<u32>) {
    let chart = chart(state);
    let highest = state
        .ledger
        .check_numbers(&chart, bank)
        .into_iter()
        .filter(|(_, _, void)| !*void)
        .map(|(_, n, _)| n)
        .max();
    (None, highest)
}

/// The bank account's balance as the demo engine reports it (the stored
/// account balance; never computed by the screen).
#[must_use]
pub fn balance(state: &mut DemoState, bank: AccountId) -> Option<Money> {
    let chart = chart(state);
    summary(&state.ledger, &chart, bank, None).and_then(|s| s.current_balance.exact())
}

fn contact_name(state: &mut DemoState, id: Option<ContactId>) -> Option<String> {
    let id = id?;
    state
        .contacts_mut()
        .iter()
        .find(|c| c.id == id)
        .map(|c| c.fields.display_name.clone())
}

fn tag_name(state: &mut DemoState, id: Option<TagId>) -> Option<String> {
    let id = id?;
    state
        .tags_mut()
        .iter()
        .find(|t| t.id == id)
        .map(|t| t.fields.name.clone())
}

/// Saves `d` (new, or over `existing` with its old date). The rules are
/// the engine's; see the module notes.
fn save(
    state: &mut DemoState,
    d: &CheckDraft,
    existing: Option<(TransactionId, CalendarDate)>,
    password: Option<&Secret>,
) -> Result<TransactionId, ApiError> {
    state.checks.writes += 1;
    let dates: Vec<CalendarDate> = existing
        .map(|(_, old)| old)
        .into_iter()
        .chain([d.date])
        .collect();
    check_closing(closing(state), &dates, password)?;
    let total = Money::sum(d.lines.iter().map(|l| l.amount)).map_err(|_| invalid())?;
    if d.lines.is_empty() || total != d.amount {
        return Err(invalid());
    }
    let chart = chart(state);
    let except = existing.map(|(id, _)| id);
    let kept = except
        .and_then(|id| state.checks.info.get(&id))
        .filter(|i| i.print_status != PrintStatus::Queue)
        .and_then(|_| {
            let id = except?;
            state
                .ledger
                .check_numbers(&chart, d.bank)
                .into_iter()
                .find(|(t, _, _)| *t == id)
                .map(|(_, n, _)| n)
        });
    let number = if d.print_later {
        None
    } else {
        d.check_number.or(kept)
    };
    if let Some(n) = number
        && taken(&state.ledger, &chart, d.bank, n, except)
    {
        return Err(number_used());
    }
    let posting = DemoPosting {
        kind: "check",
        date: d.date,
        number: number.map(|n| n.to_string()).unwrap_or_default(),
        contact: contact_name(state, d.payee),
        memo: d.memo.clone().unwrap_or_default(),
        own: d.bank,
        own_debit: false,
        lines: d.lines.iter().map(|l| (l.account, l.amount)).collect(),
    };
    let id = state
        .ledger
        .save(&chart, &posting, except)
        .ok_or_else(invalid)?;
    let details: Vec<(Option<String>, Option<String>)> = d
        .lines
        .iter()
        .map(|l| (l.memo.clone(), tag_name(state, l.tag)))
        .collect();
    state.ledger.set_line_details(&chart, id, d.bank, &details);
    let print_status = if d.print_later {
        PrintStatus::Queue
    } else {
        PrintStatus::HandWritten
    };
    // The header tags become exactly the ones sent (the engine replaces
    // the set on `PUT /checks/{id}`).
    let mut tag_ids: Vec<TagId> = Vec::new();
    for t in &d.tag_ids {
        if !tag_ids.contains(t) {
            tag_ids.push(*t);
        }
    }
    state.checks.info.insert(
        id,
        CheckInfo {
            print_status,
            payee_name: d.payee_name.clone(),
            address: d.payee_address.clone(),
            printed_memo: d.printed_memo.clone(),
            tag_ids,
            lines: d.lines.iter().map(|l| (l.contact, l.billable)).collect(),
        },
    );
    // A receipt uploaded before the first save moves onto the check (not
    // a file a live document draft reads: A04, U-95 guard).
    if existing.is_none()
        && let Some(draft) = d.draft_attachment
    {
        link_draft(state, draft, cl_api::AttachTo::Expense(id));
    }
    let action = if existing.is_some() {
        "update"
    } else {
        "create"
    };
    state.checks.history.push((id, action));
    Ok(id)
}

/// `POST /checks`.
///
/// # Errors
/// `ClosedPeriod` / `ClosingPasswordRejected` (closing date), `Conflict`
/// (the number is used on the bank account), `Local(InvalidValue)` (lines
/// that don't add up exactly, an account outside the chart).
pub fn create(
    state: &mut DemoState,
    d: &CheckDraft,
    password: Option<&Secret>,
) -> Result<TransactionId, ApiError> {
    save(state, d, None, password)
}

/// `PUT /checks/{id}`.
///
/// # Errors
/// As [`create`]; `NotFound` for an unknown check; `Validation`-like
/// refusal for a void one.
pub fn update(
    state: &mut DemoState,
    id: TransactionId,
    d: &CheckDraft,
    password: Option<&Secret>,
    printed: Option<cl_api::PrintedCheckChange>,
) -> Result<TransactionId, ApiError> {
    let old = state
        .ledger
        .date_of(id)
        .ok_or(ApiError::NotFound { code: None })?;
    if state.ledger.is_void(id) {
        state.checks.writes += 1;
        return Err(invalid());
    }
    // A printed check whose printed face changes needs the owner's choice
    // (409 CHECK_PRINTED otherwise; nothing changes).
    let was_printed = state
        .checks
        .info
        .get(&id)
        .is_some_and(|i| i.print_status == PrintStatus::Printed);
    if was_printed {
        let before = get(state, id).map(|v| v.record);
        let face_changed = before.as_ref().is_none_or(|b| {
            let amount = b
                .txn
                .lines
                .iter()
                .find(|l| l.credit.is_positive() && l.debit.is_zero())
                .map(|l| l.credit);
            amount != Some(d.amount)
                || b.txn.contact_id != d.payee
                || b.payee_name != d.payee_name
                || b.payee_address != d.payee_address
                || b.printed_memo != d.printed_memo
                || b.txn.txn_date != d.date
                || (d.check_number.is_some() && d.check_number != b.check_number)
                || d.print_later
        });
        let refused = match printed {
            None => face_changed,
            Some(cl_api::PrintedCheckChange::KeepNumber) => d.print_later,
            Some(cl_api::PrintedCheckChange::Reprint) => false,
        };
        if refused {
            state.checks.writes += 1;
            return Err(ApiError::Conflict {
                reason: ConflictReason::Unspecified,
                current_version: None,
                code: cl_api::ErrorCode::parse(cl_api::CHECK_PRINTED),
                message: UserText::default(),
            });
        }
        let saved = match printed.filter(|_| face_changed) {
            Some(cl_api::PrintedCheckChange::Reprint) => {
                let mut queued = d.clone();
                queued.print_later = true;
                queued.check_number = None;
                save(state, &queued, Some((id, old)), password)?
            }
            _ => save(state, d, Some((id, old)), password)?,
        };
        if !face_changed && let Some(i) = state.checks.info.get_mut(&saved) {
            // No face change: it stays printed.
            i.print_status = PrintStatus::Printed;
        }
        return Ok(saved);
    }
    save(state, d, Some((id, old)), password)
}

/// `PUT /transactions/{id}` for a check (an engine without
/// `check-edit-v1`): guarded by the plain transaction's version (nothing
/// is sent when it changed), and the check's own fields (number, print
/// status, printed name, address, printed memo, header tags) stay as they
/// were.
///
/// # Errors
/// `Conflict { Stale }` (nothing sent); as [`update`].
pub fn update_expense(
    state: &mut DemoState,
    id: TransactionId,
    expected: &RecordVersion,
    d: &CheckDraft,
    password: Option<&Secret>,
) -> Result<TransactionId, ApiError> {
    let current = get(state, id).ok_or(ApiError::NotFound { code: None })?;
    let now = current.transaction_version();
    if &now != expected {
        return Err(ApiError::Conflict {
            reason: ConflictReason::Stale,
            current_version: Some(now),
            code: None,
            message: UserText::default(),
        });
    }
    let old = current.record.txn.txn_date;
    if state.ledger.is_void(id) {
        state.checks.writes += 1;
        return Err(invalid());
    }
    let r = current.record;
    let kept = CheckDraft {
        check_number: r.check_number,
        print_later: r.print_status == PrintStatus::Queue,
        payee_name: r.payee_name.clone(),
        payee_address: r.payee_address.clone(),
        printed_memo: r.printed_memo.clone(),
        tag_ids: r.tag_ids.clone(),
        draft_attachment: None,
        ..d.clone()
    };
    let saved = save(state, &kept, Some((id, old)), password)?;
    if let Some(i) = state.checks.info.get_mut(&saved) {
        i.print_status = r.print_status;
    }
    Ok(saved)
}

/// Marks a check printed (the demo has no printing yet; tests and the
/// control channel use it to reach the printed-check question).
pub fn mark_printed(state: &mut DemoState, id: TransactionId) -> bool {
    match state.checks.info.get_mut(&id) {
        Some(i) if i.print_status != PrintStatus::Queue => {
            i.print_status = PrintStatus::Printed;
            true
        }
        _ => false,
    }
}

/// The print run numbered a written check (`Some`) or put it back in the
/// queue without its number (`None`), as `POST /checks/print` and
/// `/checks/requeue` do (P19). False for an id that is not a demo check.
pub fn set_printed(state: &mut DemoState, id: TransactionId, number: Option<u64>) -> bool {
    let text = number.map(|n| n.to_string()).unwrap_or_default();
    if !state.ledger.set_number(id, &text) {
        return false;
    }
    let status = if number.is_some() {
        PrintStatus::Printed
    } else {
        PrintStatus::Queue
    };
    if let Some(i) = state.checks.info.get_mut(&id) {
        i.print_status = status;
        return true;
    }
    let payee_name = transaction(state, id)
        .and_then(|t| t.contact_name)
        .unwrap_or_default();
    state.checks.info.insert(
        id,
        CheckInfo {
            print_status: status,
            payee_name,
            address: None,
            printed_memo: None,
            tag_ids: Vec::new(),
            lines: Vec::new(),
        },
    );
    true
}

/// Voids a check (its number is free again).
///
/// # Errors
/// As the ledger's void; the closing-date rule.
pub fn void(
    state: &mut DemoState,
    id: TransactionId,
    password: Option<&Secret>,
) -> Result<(), ApiError> {
    state.checks.writes += 1;
    let old = state
        .ledger
        .date_of(id)
        .ok_or(ApiError::NotFound { code: None })?;
    check_closing(closing(state), &[old], password)?;
    state.ledger.void(id)?;
    state.checks.history.push((id, "void"));
    Ok(())
}

/// One check as `GET /transactions/{id}` answers it (with the check's own
/// fields), and its version.
#[must_use]
pub fn get(state: &mut DemoState, id: TransactionId) -> Option<Versioned<CheckRecord>> {
    let chart = chart(state);
    let contacts = state.contacts_mut().clone();
    let txn = state.ledger.get(&chart, &contacts, id)?.record;
    let info = state.checks.info.get(&id).cloned();
    let number = txn.check_number.and_then(|n| u32::try_from(n).ok());
    let mut record = CheckRecord {
        payee_name: info
            .as_ref()
            .map(|i| i.payee_name.clone())
            .or_else(|| txn.contact_name.clone())
            .unwrap_or_default(),
        payee_address: info.as_ref().and_then(|i| i.address.clone()),
        printed_memo: info.as_ref().and_then(|i| i.printed_memo.clone()),
        tag_ids: info.as_ref().map(|i| i.tag_ids.clone()).unwrap_or_default(),
        print_status: match (&info, number) {
            (Some(i), _) => i.print_status.clone(),
            (None, Some(_)) => PrintStatus::HandWritten,
            (None, None) => PrintStatus::Queue,
        },
        check_number: number,
        txn,
    };
    if let Some(i) = &info {
        let bank_line = record
            .txn
            .lines
            .iter()
            .position(|l| l.credit.is_positive() && l.debit.is_zero());
        let others = record
            .txn
            .lines
            .iter_mut()
            .enumerate()
            .filter(|(n, _)| Some(*n) != bank_line)
            .map(|(_, l)| l);
        for (l, (contact, billable)) in others.zip(&i.lines) {
            l.contact_id = *contact;
            l.is_billable = *billable;
            l.billed_status = billable.then_some(cl_api::BilledStatus::Unbilled);
        }
    }
    let version = RecordVersion::Fingerprint(record.fingerprint());
    Some(Versioned { record, version })
}

/// The checks drawn on `bank`, newest first (as `GET /checks` sends them).
#[must_use]
pub fn list(state: &mut DemoState, bank: AccountId) -> Vec<CheckSummary> {
    let chart = chart(state);
    let Some(number) = chart
        .iter()
        .find(|a| a.id == bank)
        .and_then(|a| a.fields.account_number.clone())
    else {
        return Vec::new();
    };
    let mut txns: Vec<_> = state
        .ledger
        .transactions()
        .into_iter()
        .filter(|t| t.kind == "check" && t.lines.iter().any(|l| l.account == number))
        .collect();
    txns.sort_by(|a, b| b.date.cmp(&a.date).then(b.id.cmp(&a.id)));
    txns.into_iter()
        .map(|t| {
            let id = DemoLedger::id_of(&t);
            let amount = t
                .lines
                .iter()
                .find(|l| l.account == number)
                .map(|l| RegisterAmount::Exact(l.credit));
            let info = state.checks.info.get(&id);
            let n = t.number.parse::<u32>().ok();
            CheckSummary {
                id,
                number: n,
                date: Some(t.date),
                payee: info
                    .map(|i| i.payee_name.clone())
                    .filter(|p| !p.is_empty())
                    .or(t.contact.clone()),
                amount,
                print_status: info.map_or(
                    if n.is_some() {
                        PrintStatus::HandWritten
                    } else {
                        PrintStatus::Queue
                    },
                    |i| i.print_status.clone(),
                ),
                status: if state.ledger.is_void(id) {
                    cl_api::TxnStatus::Void
                } else {
                    cl_api::TxnStatus::Posted
                },
            }
        })
        .collect()
}

/// The demo's written checks waiting to print on `bank` (P19); `printed`
/// are the ones the print queue numbered.
pub fn print_queue(
    state: &mut DemoState,
    bank: AccountId,
    printed: &dyn Fn(TransactionId) -> bool,
) -> Vec<cl_api::QueuedCheck> {
    list(state, bank)
        .into_iter()
        .filter(|r| {
            r.print_status == PrintStatus::Queue
                && r.status != cl_api::TxnStatus::Void
                && !printed(r.id)
        })
        .filter_map(|r| {
            let RegisterAmount::Exact(amount) = r.amount? else {
                return None;
            };
            let info = state.checks.info.get(&r.id);
            Some(cl_api::QueuedCheck {
                id: r.id,
                date: r.date?,
                amount,
                payee: r.payee.unwrap_or_default(),
                memo: info
                    .and_then(|i| i.printed_memo.clone())
                    .unwrap_or_default(),
                kind: cl_api::QueuedCheckKind::Check,
                bank_account: Some(bank),
                address: info
                    .and_then(|i| i.address.as_deref())
                    .map(|a| {
                        a.lines()
                            .map(str::to_string)
                            .filter(|l| !l.trim().is_empty())
                            .collect()
                    })
                    .unwrap_or_default(),
            })
        })
        .collect()
}

/// A check's expense lines (account name, amount) for its voucher stub.
pub fn stub_lines(state: &mut DemoState, id: TransactionId) -> Vec<(String, Money)> {
    transaction(state, id)
        .map(|t| {
            t.lines
                .into_iter()
                .filter(|l| l.debit.is_positive())
                .map(|l| (l.account_name.unwrap_or_default(), l.debit))
                .collect()
        })
        .unwrap_or_default()
}

/// A check's history in the demo (what this run did to it).
#[must_use]
pub fn history(state: &DemoState, id: TransactionId) -> Vec<HistoryEntry> {
    state
        .checks
        .history
        .iter()
        .filter(|(t, _)| *t == id)
        .map(|(_, action)| HistoryEntry { action })
        .collect()
}

// ------------------------------------------------------------------------
// The payee panel, prefill and suggestions (P13 WP7).

/// The fixture names that stand for contact `id` (its name now and its
/// sample name).
fn names_of(state: &mut DemoState, id: ContactId) -> Vec<String> {
    let mut names: Vec<String> = state
        .contacts_mut()
        .iter()
        .filter(|c| c.id == id)
        .map(|c| c.fields.display_name.clone())
        .collect();
    if let Some(n) = state.contact_names.get(&id) {
        names.push(n.clone());
    }
    names
}

/// The vendor's bills in brief, as `GET /bills/vendor-summary` sends it.
#[must_use]
pub fn vendor_summary(state: &mut DemoState, vendor: ContactId) -> cl_api::VendorSummary {
    state.contacts_mut();
    let bills = super::contacts::open_bills(state, vendor);
    let today = DemoState::today();
    let owed = Money::sum(bills.iter().map(|b| b.3)).unwrap_or(Money::ZERO);
    let overdue: Vec<Money> = bills
        .iter()
        .filter(|b| b.2.is_some_and(|d| d < today))
        .map(|b| b.3)
        .collect();
    let overdue_total = Money::sum(overdue.iter().copied()).unwrap_or(Money::ZERO);
    serde_json::from_value(serde_json::json!({
        "unpaidCount": bills.len(),
        "totalOwed": owed.to_wire(),
        "overdueCount": overdue.len(),
        "totalOverdue": overdue_total.to_wire(),
    }))
    .unwrap_or_else(|_| unreachable!("the demo summary is well formed"))
}

/// The vendor's unpaid bills, as `GET /bills/payable` sends them.
#[must_use]
pub fn payable(state: &mut DemoState, vendor: ContactId) -> Vec<cl_api::OpenBill> {
    state.contacts_mut();
    super::contacts::open_bills(state, vendor)
        .into_iter()
        .filter_map(|(id, number, due, owed)| {
            serde_json::from_value(serde_json::json!({
                "id": id,
                "contactId": vendor,
                "txnNumber": number,
                "txnDate": DemoState::today().to_wire(),
                "dueDate": due.map(CalendarDate::to_wire),
                "balanceDue": owed.to_wire(),
                "daysOverdue": 0,
            }))
            .ok()
        })
        .collect()
}

/// The contact's checks, expenses and bills, newest first, as
/// `GET /contacts/{id}/transactions` lists them (numbered checks carry
/// their number; void ones are left out).
#[must_use]
pub fn contact_transactions(
    state: &mut DemoState,
    contact: ContactId,
    limit: usize,
) -> Vec<cl_api::TransactionSummary> {
    let names = names_of(state, contact);
    let chart = chart(state);
    let today = DemoState::today();
    let mut txns: Vec<_> = state
        .ledger
        .transactions()
        .into_iter()
        .filter(|t| {
            matches!(
                t.kind.as_str(),
                "check" | "expense" | "card_charge" | "bill"
            ) && t.date <= today
                && t.contact.as_ref().is_some_and(|c| names.contains(c))
                && !state.ledger.is_void(DemoLedger::id_of(t))
        })
        .collect();
    txns.sort_by(|a, b| b.date.cmp(&a.date).then(b.id.cmp(&a.id)));
    txns.into_iter()
        .take(limit)
        .map(|t| {
            let id = DemoLedger::id_of(&t);
            let total = Money::sum(t.lines.iter().map(|l| l.debit)).ok();
            let bank = t.kind == "check"
                && t.lines.iter().any(|l| {
                    chart.iter().any(|a| {
                        a.fields.account_number.as_deref() == Some(l.account.as_str())
                            && a.fields.detail_type.as_deref() == Some("bank")
                    })
                });
            cl_api::TransactionSummary {
                id,
                txn_type: if t.kind == "bill" {
                    cl_api::TxnType::Bill
                } else {
                    cl_api::TxnType::Expense
                },
                txn_number: Some(t.number.clone()),
                txn_date: t.date,
                status: cl_api::TxnStatus::Posted,
                contact_id: Some(contact),
                contact_name: t.contact.clone(),
                memo: Some(t.memo.clone()),
                total,
                display_total: total,
                created_at: None,
                check_number: if bank { t.number.parse().ok() } else { None },
            }
        })
        .collect()
}

/// One transaction as `GET /transactions/{id}` answers it, with a story
/// line's project as its customer (prefill copies it).
#[must_use]
pub fn transaction(state: &mut DemoState, id: TransactionId) -> Option<cl_api::Transaction> {
    let chart = chart(state);
    let contacts = state.contacts_mut().clone();
    let mut txn = state.ledger.get(&chart, &contacts, id)?.record;
    if let Some(info) = state.checks.info.get(&id) {
        let bank_line = txn
            .lines
            .iter()
            .position(|l| l.credit.is_positive() && l.debit.is_zero());
        let others = txn
            .lines
            .iter_mut()
            .enumerate()
            .filter(|(n, _)| Some(*n) != bank_line)
            .map(|(_, l)| l);
        for (l, (contact, billable)) in others.zip(&info.lines) {
            l.contact_id = *contact;
            l.is_billable = *billable;
        }
        return Some(txn);
    }
    // A story line names its project: the project's customer is a demo contact.
    let sample = state
        .ledger
        .transactions()
        .into_iter()
        .find(|t| DemoLedger::id_of(t) == id)?;
    for (l, s) in txn.lines.iter_mut().zip(&sample.lines) {
        if let Some(project) = &s.project {
            let customer = project.split(':').next().unwrap_or(project);
            l.contact_id = contacts
                .iter()
                .find(|c| c.fields.display_name == *project || c.fields.display_name == customer)
                .map(|c| c.id);
        }
    }
    Some(txn)
}

/// The account to prefill for a contact: its default expense account,
/// else the one on its newest check, expense or bill.
#[must_use]
pub fn suggest_account(
    state: &mut DemoState,
    contact: ContactId,
) -> Option<cl_api::SuggestedAccount> {
    let default = state
        .contacts_mut()
        .iter()
        .find(|c| c.id == contact)
        .and_then(|c| c.fields.default_expense_account_id);
    if let Some(account) = default {
        return Some(cl_api::SuggestedAccount {
            account,
            source: cl_api::SuggestSource::Default,
        });
    }
    let last = contact_transactions(state, contact, 1).into_iter().next()?;
    let txn = transaction(state, last.id)?;
    txn.lines
        .iter()
        .find(|l| l.debit.is_positive())
        .map(|l| cl_api::SuggestedAccount {
            account: l.account_id,
            source: cl_api::SuggestSource::Recent,
        })
}

/// `POST /practice/conditional-rules/evaluate`: the first demo rule whose
/// text is in the description.
#[must_use]
pub fn evaluate_rule(
    state: &mut DemoState,
    query: &cl_api::RuleQuery,
) -> Option<cl_api::RuleMatch> {
    state.checks.rule_requests += 1;
    let text = query.description.to_lowercase();
    let rule = state
        .checks
        .rules
        .iter()
        .find(|r| !r.contains.is_empty() && text.contains(&r.contains.to_lowercase()))?;
    serde_json::from_value(serde_json::json!({
        "ruleId": "d0000000-0000-4000-8000-e00000000001",
        "ruleName": rule.name,
        "accountId": rule.account,
    }))
    .ok()
}

/// The demo AI's account suggestions (`POST /ai/categorize/preview`):
/// "mulch" in the text suggests 5110 Materials:Mulch & stone. Nothing is
/// asked unless AI and the account-suggestion task are on.
///
/// # Errors
/// `AiUnavailable` with AI or the task off.
pub fn ai_preview(
    state: &mut DemoState,
    rows: &[cl_api::AiPreviewRow],
) -> Result<Vec<cl_api::AiSuggestion>, ApiError> {
    if !(state.ai.enabled && state.ai.categorization) {
        return Err(ApiError::AiUnavailable);
    }
    state.checks.ai_requests += 1;
    let mulch = chart(state)
        .iter()
        .find(|a| a.fields.account_number.as_deref() == Some("5110"))
        .map(|a| a.id);
    Ok(rows
        .iter()
        .enumerate()
        .filter(|(_, r)| r.description.to_lowercase().contains("mulch"))
        .map(|(index, _)| cl_api::AiSuggestion {
            index,
            account_id: mulch,
            account_name: None,
            tag_id: None,
            basis: Some("from 6 past checks to this payee".into()),
        })
        .collect())
}

fn attachment(f: &DemoFile) -> Option<cl_api::Attachment> {
    serde_json::from_value(serde_json::json!({
        "id": f.id.to_string(),
        "fileName": f.name,
        "fileSize": f.bytes.len(),
        "mimeType": cl_api::FileKind::sniff(&f.bytes).map(cl_api::FileKind::mime),
        "attachableType": f.kind,
        "attachableId": f.to,
    }))
    .ok()
}

/// `POST /attachments`: the file is checked as the client checks it.
///
/// # Errors
/// `Local(AttachmentTooLarge | AttachmentType)`.
pub fn upload(
    state: &mut DemoState,
    to: cl_api::AttachTo,
    name: &str,
    bytes: &[u8],
) -> Result<cl_api::Attachment, ApiError> {
    cl_api::check_file(bytes).map_err(ApiError::Local)?;
    let n = state.checks.files.len() + state.checks.deleted + 1;
    let id = cl_api::AttachmentId::parse(&format!("d0000000-0000-4000-8000-a{n:011}"))
        .map_err(|_| invalid())?;
    let (kind, to) = (to.attachable_type(), to.attachable_id());
    let f = DemoFile {
        id,
        kind,
        to,
        name: name.to_string(),
        bytes: bytes.to_vec(),
    };
    let a = attachment(&f).ok_or_else(invalid)?;
    state.checks.files.push(f);
    Ok(a)
}

/// `GET /attachments` for a check or a draft.
#[must_use]
pub fn attachments(state: &DemoState, of: cl_api::AttachTo) -> Vec<cl_api::Attachment> {
    let (kind, to) = (of.attachable_type(), of.attachable_id());
    state
        .checks
        .files
        .iter()
        .filter(|f| f.kind == kind && f.to == to)
        .filter_map(attachment)
        .collect()
}

/// What a save with `draftAttachmentId` (or `POST /bills/{id}/attach-draft`)
/// does to a draft's files: they move onto the saved record `to`, except
/// files a live document draft reads (A04, the engine's U-95 guard: they
/// stay on the key). Returns how many moved.
pub fn link_draft(state: &mut DemoState, draft: cl_api::DraftKey, to: cl_api::AttachTo) -> usize {
    move_draft_key(state, &draft.to_string(), to, None).len()
}

/// The engine's guarded relink (U-95 b): every file on draft key `key`
/// moves onto `to` (the attachable type `to` names; an adjusting entry's
/// files as `journal_entry`), except files a live document draft other
/// than `except` reads. Returns the moved files.
pub(crate) fn move_draft_key(
    state: &mut DemoState,
    key: &str,
    to: cl_api::AttachTo,
    except: Option<cl_api::DocumentDraftId>,
) -> Vec<cl_api::AttachmentId> {
    let held = state.drafts.live_files(except);
    let (kind, id) = (super::drafts::file_type(to), to.attachable_id());
    let mut moved = Vec::new();
    for f in state
        .checks
        .files
        .iter_mut()
        .filter(|f| f.kind == "draft" && f.to == key && !held.contains(&f.id))
    {
        f.kind = kind;
        f.to.clone_from(&id);
        moved.push(f.id);
    }
    moved
}

/// `POST /attachments/{id}/link`: files one attachment on `to`. A file a
/// live document draft reads is refused (409 `ATTACHMENT_IN_USE`,
/// `details.draftId`; A04, U-95 guard): attach the draft or discard it
/// first.
///
/// # Errors
/// `NotFound`; `Conflict { InUse }`.
pub fn link_attachment(
    state: &mut DemoState,
    id: cl_api::AttachmentId,
    to: cl_api::AttachTo,
) -> Result<cl_api::Attachment, ApiError> {
    if !state.checks.files.iter().any(|f| f.id == id) {
        return Err(ApiError::NotFound { code: None });
    }
    if let Some(draft) = state.drafts.reader_of(id) {
        return Err(cl_api::engine_refusal(
            409,
            &serde_json::json!({"error": {
                "message": "This file is being read as a document.",
                "code": cl_api::ATTACHMENT_IN_USE,
                "details": {"draftId": draft},
            }}),
        ));
    }
    let kind = super::drafts::file_type(to);
    let file = state
        .checks
        .files
        .iter_mut()
        .find(|f| f.id == id)
        .ok_or(ApiError::NotFound { code: None })?;
    file.kind = kind;
    file.to = to.attachable_id();
    attachment(file).ok_or_else(invalid)
}

/// `GET /attachments/{id}`: one file's details (P17 receipt inbox).
#[must_use]
pub fn find_attachment(state: &DemoState, id: cl_api::AttachmentId) -> Option<cl_api::Attachment> {
    state
        .checks
        .files
        .iter()
        .find(|f| f.id == id)
        .and_then(attachment)
}

/// How many files are filed on record `id` (not on a draft key): the
/// control channel's proof that an attach filed the document there.
#[must_use]
pub fn files_on(state: &DemoState, id: TransactionId) -> usize {
    let id = id.to_string();
    state
        .checks
        .files
        .iter()
        .filter(|f| f.kind != "draft" && f.to == id)
        .count()
}

/// Files deleted so far (tests: an orphan upload was cleaned up).
#[cfg(test)]
#[must_use]
pub fn deleted_count(state: &DemoState) -> usize {
    state.checks.deleted
}

/// `DELETE /attachments/{id}` (A01: the engine's copy of a discarded
/// intake file). False when there is no such file.
pub fn delete_attachment(state: &mut DemoState, id: cl_api::AttachmentId) -> bool {
    let before = state.checks.files.len();
    state.checks.files.retain(|f| f.id != id);
    let gone = before - state.checks.files.len();
    state.checks.deleted += gone;
    gone > 0
}

/// `GET /attachments/{id}/download`.
///
/// # Errors
/// `NotFound`.
pub fn download(state: &DemoState, id: cl_api::AttachmentId) -> Result<Vec<u8>, ApiError> {
    state
        .checks
        .files
        .iter()
        .find(|f| f.id == id)
        .map(|f| f.bytes.clone())
        .ok_or(ApiError::NotFound { code: None })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cl_api::SplitLine;

    fn money(s: &str) -> Money {
        Money::parse_wire(s).expect("money")
    }

    fn by_number(state: &mut DemoState, n: &str) -> AccountId {
        chart(state)
            .iter()
            .find(|a| a.fields.account_number.as_deref() == Some(n))
            .map_or_else(|| panic!("{n}"), |a| a.id)
    }

    fn draft(state: &mut DemoState, number: Option<u32>, print_later: bool) -> CheckDraft {
        let bank = by_number(state, "1010");
        let plants = by_number(state, "5100");
        let mulch = by_number(state, "5110");
        let payee = state
            .contacts_mut()
            .iter()
            .find(|c| c.fields.display_name == "Ozark Feed & Seed")
            .map(|c| c.id);
        CheckDraft {
            bank,
            date: CalendarDate::from_ymd(2026, 11, 4).expect("d"),
            payee,
            payee_name: "Ozark Feed & Seed".into(),
            check_number: number,
            print_later,
            payee_address: Some("1200 Commerce Dr".into()),
            memo: Some("Acct 7712".into()),
            printed_memo: Some("Acct 7712".into()),
            tag_ids: Vec::new(),
            draft_attachment: None,
            amount: money("1286.40"),
            lines: vec![
                SplitLine {
                    account: plants,
                    amount: money("842.15"),
                    memo: Some("Perennials".into()),
                    contact: None,
                    billable: false,
                    tag: None,
                },
                SplitLine {
                    account: mulch,
                    amount: money("444.25"),
                    memo: Some("Hardwood mulch".into()),
                    contact: None,
                    billable: false,
                    tag: None,
                },
            ],
        }
    }

    #[test]
    fn a_printed_check_carries_its_number_and_a_requeue_takes_it_off() {
        use super::super::printing;
        let mut s = DemoState::new();
        let bank = by_number(&mut s, "1010");
        let d = draft(&mut s, None, true);
        let id = create(&mut s, &d, None).expect("saved");
        assert!(printing::check_queue(&mut s, bank).iter().any(|c| c.id == id));
        assert_eq!(printing::layout(&mut s, bank).0, Some(4418));
        let req = cl_api::PrintChecks {
            bank_account: bank,
            checks: vec![id],
            starting_number: 4418,
            format: cl_api::CheckFormat::Voucher,
        };
        printing::print(&mut s, &req, None).expect("print");
        let got = get(&mut s, id).expect("get").record;
        assert_eq!(got.print_status, PrintStatus::Printed);
        assert_eq!(got.check_number, Some(4418));
        let row = list(&mut s, bank).into_iter().find(|r| r.id == id);
        assert_eq!(row.and_then(|r| r.number), Some(4418));
        assert_eq!(printing::layout(&mut s, bank).0, Some(4419));
        printing::requeue(&mut s, &[id], &[d.date], None).expect("requeue");
        let got = get(&mut s, id).expect("get").record;
        assert_eq!(got.print_status, PrintStatus::Queue);
        assert_eq!(got.check_number, None);
        assert!(printing::check_queue(&mut s, bank).iter().any(|c| c.id == id));
    }

    #[test]
    fn a_check_saves_numbered_and_its_number_is_then_taken() {
        let mut s = DemoState::new();
        let bank = by_number(&mut s, "1010");
        assert_eq!(numbers(&mut s, bank), (None, Some(4417)));
        let before = balance(&mut s, bank).expect("balance");
        let d = draft(&mut s, Some(4418), false);
        let id = create(&mut s, &d, None).expect("saved");
        let after = balance(&mut s, bank).expect("balance");
        assert_eq!(before.checked_sub(after).expect("m"), money("1286.40"));
        let got = get(&mut s, id).expect("get").record;
        assert_eq!(got.check_number, Some(4418));
        assert_eq!(got.print_status, PrintStatus::HandWritten);
        assert_eq!(got.txn.lines[0].description.as_deref(), Some("Perennials"));
        assert_eq!(numbers(&mut s, bank).1, Some(4418));
        assert!(matches!(
            create(&mut s, &d, None),
            Err(ApiError::Conflict { .. })
        ));
        // Voiding frees the number.
        void(&mut s, id, None).expect("void");
        assert_eq!(numbers(&mut s, bank).1, Some(4417));
        assert!(create(&mut s, &d, None).is_ok());
        assert_eq!(list(&mut s, bank)[0].number, Some(4418));
        assert_eq!(
            history(&s, id),
            [
                HistoryEntry { action: "create" },
                HistoryEntry { action: "void" }
            ]
        );
    }

    #[test]
    fn print_later_waits_unnumbered_and_lines_must_add_up() {
        let mut s = DemoState::new();
        let d = draft(&mut s, None, true);
        let id = create(&mut s, &d, None).expect("saved");
        let got = get(&mut s, id).expect("get").record;
        assert_eq!(got.check_number, None);
        assert_eq!(got.print_status, PrintStatus::Queue);
        let mut off = draft(&mut s, Some(5000), false);
        off.amount = money("1286.39");
        assert_eq!(create(&mut s, &off, None), Err(invalid()));
    }

    #[test]
    fn the_closing_date_needs_the_password() {
        let mut s = DemoState::new();
        let mut d = draft(&mut s, Some(5001), false);
        d.date = CalendarDate::from_ymd(2026, 9, 15).expect("d");
        assert!(matches!(
            create(&mut s, &d, None),
            Err(ApiError::ClosedPeriod(_))
        ));
        assert_eq!(
            create(&mut s, &d, Some(&Secret::new("wrong"))),
            Err(ApiError::ClosingPasswordRejected)
        );
        let id = create(
            &mut s,
            &d,
            Some(&Secret::new(super::super::register::DEMO_CLOSING_PASSWORD)),
        )
        .expect("saved");
        let mut edit = d.clone();
        edit.memo = Some("changed".into());
        assert!(matches!(
            update(&mut s, id, &edit, None, None),
            Err(ApiError::ClosedPeriod(_))
        ));
    }
}
