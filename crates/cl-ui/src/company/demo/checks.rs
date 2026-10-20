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
    let mut tag_ids = except
        .and_then(|id| state.checks.info.get(&id))
        .map(|i| i.tag_ids.clone())
        .unwrap_or_default();
    // Header tags are added, never removed (the engine's rule).
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
) -> Result<TransactionId, ApiError> {
    let old = state
        .ledger
        .date_of(id)
        .ok_or(ApiError::NotFound { code: None })?;
    if state.ledger.is_void(id) {
        state.checks.writes += 1;
        return Err(invalid());
    }
    save(state, d, Some((id, old)), password)
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
            update(&mut s, id, &edit, None),
            Err(ApiError::ClosedPeriod(_))
        ));
    }
}
