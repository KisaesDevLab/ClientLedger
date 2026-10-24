//! Home's figures in `--demo` (P16): the mockup's story on 11/04/2026
//! (`docs/design/mockups/Home.html`, `docs/design/README.md` "Sample
//! figures"). The demo stands in for the engine with both P16
//! capabilities (`dashboard-exact-v1` simulated): balances are the demo
//! engine's own (writes made in the demo move them), open A/R and the bills
//! due come from the sample company, and the counts the sample has no
//! record of (payroll imports, overdue invoices) are the mockup's. All of it
//! is synthetic.

use cl_api::{
    Account, ActionItems, CashAccount, CashPosition, ContactType, DashboardSummary, LastReconciled,
    Payables, Receivables, ReconcileState,
};
use cl_core::ids::AccountId;
use cl_core::{CalendarDate, Money, Timestamp};
use cl_model::register::AccountClass;

use super::DemoState;

/// Payroll imports not posted yet ("Payroll for the 11/02 pay date not
/// imported yet").
pub const PAYROLL_UNPOSTED: u64 = 1;

/// Invoices past due ("2 invoices more than 60 days past due").
pub const OVERDUE_INVOICES: u64 = 2;

/// The demo's clock on Home: the story date, after the morning backup.
#[must_use]
pub fn now() -> Timestamp {
    stamp("2026-11-04T09:30:00.000Z")
}

/// The demo company's last backup ("Backed up today 8:12 AM"; the demo
/// shows times in UTC so screenshots do not depend on the machine).
#[must_use]
pub fn last_backup() -> Timestamp {
    stamp("2026-11-04T08:12:00.000Z")
}

fn stamp(s: &str) -> Timestamp {
    Timestamp::parse_wire(s).unwrap_or_else(|_| DemoState::now())
}

/// The demo chart (built on first use).
fn chart(state: &mut DemoState) -> Vec<Account> {
    state.chart.get_or_insert_with(super::chart).clone()
}

/// The chart as the engine lists it.
#[must_use]
pub fn accounts(state: &mut DemoState) -> Vec<Account> {
    chart(state)
}

/// The active bank and card accounts with the demo engine's current
/// balance (what the engine stores: debits minus credits), banks first.
fn money_accounts(state: &mut DemoState) -> Vec<(Account, AccountClass, Option<Money>)> {
    let mut out = Vec::new();
    for a in chart(state) {
        let Some(class) = crate::screens::register::class_of(&a) else {
            continue;
        };
        if !a.is_active || !class.is_money_account() {
            continue;
        }
        let balance = super::checks::balance(state, a.id);
        out.push((a, class, balance));
    }
    out.sort_by_key(|(a, class, _)| {
        (
            u8::from(*class != AccountClass::Bank),
            a.fields.account_number.clone().unwrap_or_default(),
        )
    });
    out
}

/// Open bills due on or before `through`: (count, open amount). A bill is
/// paid when a later bill payment of the same amount to the same vendor
/// exists (the sample pays bills in full; `Sample::bills_due`).
#[must_use]
pub fn bills_due(through: CalendarDate) -> (u64, Money) {
    let sample = cl_company::Sample::load();
    let ap = sample.company.system_accounts.accounts_payable.clone();
    let mut count: u64 = 0;
    for b in sample.transactions.iter().filter(|t| t.kind == "bill") {
        let Some(due) = b.due else { continue };
        if due > through {
            continue;
        }
        let amount = Money::sum(b.lines.iter().filter(|l| l.account == ap).map(|l| l.credit))
            .unwrap_or(Money::ZERO);
        let settled = sample.transactions.iter().any(|p| {
            p.kind == "bill_payment"
                && p.contact == b.contact
                && p.date >= b.date
                && p.lines.first().is_some_and(|l| l.debit == amount)
        });
        if !settled {
            count += 1;
        }
    }
    (count, sample.bills_due(through))
}

/// Bank-feed items waiting (ready, to review and matches: 37).
#[must_use]
pub fn feed_waiting() -> u64 {
    let (ready, review, matched) = cl_company::Sample::load().feed_counts();
    u64::try_from(ready + review + matched).unwrap_or(u64::MAX)
}

/// Active clients and vendors (a contact that is both counts in each).
#[must_use]
pub fn contact_counts(state: &mut DemoState) -> (u64, u64) {
    let (mut clients, mut vendors) = (0u64, 0u64);
    for c in state.contacts_mut().iter().filter(|c| c.is_active) {
        match c.fields.contact_type {
            ContactType::Customer => clients += 1,
            ContactType::Vendor => vendors += 1,
            ContactType::Both => {
                clients += 1;
                vendors += 1;
            }
        }
    }
    (clients, vendors)
}

/// The operating account (1010).
fn operating() -> AccountId {
    super::account_id(1010)
}

/// Reconciliations: October's is started and not finished for Operating
/// Checking; the last finished one is September's (09/30/2026).
#[must_use]
pub fn reconcile(accounts: &[AccountId]) -> ReconcileState {
    let op = operating();
    let has = accounts.contains(&op);
    ReconcileState {
        in_progress: if has { vec![op] } else { Vec::new() },
        last_completed: has
            .then(|| CalendarDate::from_ymd(2026, 9, 30).ok())
            .flatten()
            .map(|statement_date| LastReconciled {
                account: op,
                statement_date,
            }),
    }
}

/// The closing date of the open demo company.
#[must_use]
pub fn closing_date(state: &DemoState) -> Option<CalendarDate> {
    let open = state.open?;
    state
        .companies
        .iter()
        .find(|c| c.tenant_id == open)
        .and_then(|c| c.closed_through)
}

/// `/dashboard/summary` as the demo engine answers it on `today` with a
/// bills window of `days` (the engine takes 1 to 90).
#[must_use]
pub fn summary(state: &mut DemoState, today: CalendarDate, days: u16) -> DashboardSummary {
    let window = days.clamp(1, cl_api::DUE_WITHIN_DAYS_MAX);
    let (mut banks, mut cards) = (Vec::new(), Vec::new());
    for (a, class, balance) in money_accounts(state) {
        let Some(balance) = balance else { continue };
        let entry = CashAccount {
            id: a.id,
            name: a.fields.name.clone(),
            account_number: a.fields.account_number.clone(),
            balance,
        };
        if class == AccountClass::Bank {
            banks.push(entry);
        } else {
            cards.push(entry);
        }
    }
    let total =
        |rows: &[CashAccount]| Money::sum(rows.iter().map(|r| r.balance)).unwrap_or(Money::ZERO);
    let total_bank = total(&banks);
    let total_cards = total(&cards).checked_neg().unwrap_or(Money::ZERO);
    let sample = cl_company::Sample::load();
    let ar = sample.company.system_accounts.accounts_receivable.clone();
    let open_ar = sample.balance(&ar, today);
    let through = today.add_days(i64::from(window)).unwrap_or(today);
    let (due_count, due_amount) = bills_due(through);
    DashboardSummary {
        as_of: today,
        cash_position: Some(CashPosition {
            bank_accounts: banks,
            credit_cards: cards,
            total_bank,
            total_cards,
        }),
        receivables: Some(Receivables {
            total_outstanding: open_ar,
            overdue_count: OVERDUE_INVOICES,
            overdue_amount: Money::ZERO,
            invoice_count: OVERDUE_INVOICES,
        }),
        payables: Some(Payables {
            total_owed: due_amount,
            bill_count: due_count,
            overdue_count: 0,
            overdue_amount: Money::ZERO,
            due_soon_count: due_count,
            due_soon_amount: due_amount,
            due_within_days: window,
            credit_count: 0,
            credit_amount: Money::ZERO,
            ap_balance: due_amount,
        }),
        action_items: Some(ActionItems {
            pending_feed_count: feed_waiting(),
            overdue_invoice_count: OVERDUE_INVOICES,
            stale_reconciliations: Vec::new(),
            pending_deposit_count: 0,
            pending_deposit_amount: Money::ZERO,
            print_queue_count: 0,
            print_queue_amount: Money::ZERO,
        }),
        errors: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(s: &str) -> Money {
        Money::parse_wire(s).expect("money")
    }

    /// The mockup's figures (docs/design/README.md).
    #[test]
    fn the_demo_home_figures_are_the_mockups() {
        let mut state = DemoState::new();
        let today = DemoState::today();
        let s = summary(&mut state, today, 7);
        let cash = s.cash_position.expect("cash");
        let op = cash
            .bank_accounts
            .iter()
            .find(|a| a.account_number.as_deref() == Some("1010"))
            .expect("1010");
        assert_eq!(op.balance, m("84210.55"), "1010 in your books");
        assert!(!cash.credit_cards.is_empty(), "the business card");
        assert_eq!(
            s.receivables.expect("ar").total_outstanding,
            m("48912.40"),
            "open A/R"
        );
        let pay = s.payables.expect("ap");
        assert_eq!(pay.due_soon_amount, m("7471.90"), "bills due in 7 days");
        assert_eq!(pay.due_within_days, 7);
        assert!(pay.due_soon_count > 0);
        assert_eq!(s.action_items.expect("actions").pending_feed_count, 37);
        assert_eq!(feed_waiting(), 37);
        assert_eq!(
            summary(&mut state, today, 200)
                .payables
                .map(|p| p.due_within_days),
            Some(90)
        );
        let r = reconcile(&[operating()]);
        assert_eq!(r.in_progress, vec![operating()]);
        assert_eq!(
            r.last_completed.map(|l| l.statement_date.to_wire()),
            Some("2026-09-30".into())
        );
        assert_eq!(reconcile(&[]), ReconcileState::default());
        assert_eq!(
            closing_date(&state).map(CalendarDate::to_wire),
            Some("2026-09-30".into())
        );
        let (clients, vendors) = contact_counts(&mut state);
        assert!(clients > 0 && vendors > 0);
        assert!(last_backup() < now(), "backed up this morning");
    }
}
