//! The demo "engine" for the account register (P12): pages and the summary
//! of a register, built from the synthetic Ozark Ridge sample.
//!
//! This stands in for the engine in `--demo` only, so it plays the engine's
//! part, including the running balance in date order (filter-independent,
//! as with the engine's `register-exact-v1`). The register screen never
//! adds anything up: it shows what this module (or the real engine) sends.
//! All figures are synthetic.
//!
//! Signs follow the engine (Q-P12h): the running, forward, ending and
//! cleared balances are in the account's natural sign (credit − debit on a
//! credit-normal account), `payment` is the decrease and `deposit` the
//! increase, while the summary's `currentBalance` (the stored account
//! balance) is debits minus credits for every type.

use std::cmp::Ordering;

use cl_api::{
    Account, AccountType, Category, ClearState, EntryType, RegisterAccount, RegisterAmount,
    RegisterLine, RegisterPage, RegisterQuery, RegisterSort, RegisterSummary, SortDir, TxnStatus,
    TxnType,
};
use cl_company::Sample;
use cl_company::sample::SampleTransaction;
use cl_core::ids::{AccountId, JournalLineId, TransactionId};
use cl_core::{CalendarDate, Money};

/// The statement date of the demo's last finished reconciliation (the
/// mockup's "Last reconciled 09/30/2026"); cleared lines on or before it
/// show as reconciled.
#[must_use]
pub fn last_reconciled() -> CalendarDate {
    CalendarDate::from_ymd(2026, 9, 30).unwrap_or_else(|_| CalendarDate::min())
}

fn txn_id(t: &SampleTransaction) -> TransactionId {
    TransactionId::parse(&format!("d0000000-0000-4000-8000-b{:011}", t.id))
        .unwrap_or_else(|_| unreachable!("a well-formed demo id"))
}

fn line_id(t: &SampleTransaction, index: usize) -> JournalLineId {
    JournalLineId::parse(&format!("d0000000-0000-4000-8000-c{:07}{index:04}", t.id))
        .unwrap_or_else(|_| unreachable!("a well-formed demo id"))
}

fn txn_type(kind: &str) -> TxnType {
    match kind {
        "invoice" => TxnType::Invoice,
        "payment" => TxnType::CustomerPayment,
        "bill" => TxnType::Bill,
        "bill_payment" => TxnType::BillPayment,
        "deposit" => TxnType::Deposit,
        "journal_entry" => TxnType::JournalEntry,
        "transfer" => TxnType::Transfer,
        // `check` and `card_charge` are expenses paid from the account.
        _ => TxnType::Expense,
    }
}

/// One line of the account, before filtering, with the engine-style
/// running balance after it.
struct Built {
    line: RegisterLine,
    amount: Money,
}

fn number_of(a: &Account) -> Option<&str> {
    a.fields.account_number.as_deref()
}

/// The engine's `isDebitNormal`: asset, COGS, expense, other expense.
fn debit_normal(t: AccountType) -> bool {
    matches!(
        t,
        AccountType::Asset | AccountType::Cogs | AccountType::Expense | AccountType::OtherExpense
    )
}

/// Every line of `account` in date order with its running balance (in the
/// account's natural sign, like the engine's register). `None` when a
/// figure leaves the supported money range.
fn build(ledger: &DemoLedger, chart: &[Account], account: &Account) -> Option<Vec<Built>> {
    let Some(number) = number_of(account) else {
        return Some(Vec::new());
    };
    let debit_normal = debit_normal(account.fields.account_type);
    let all = ledger.transactions();
    let reconciled_through = last_reconciled();
    let mut txns: Vec<&SampleTransaction> = all
        .iter()
        .filter(|t| t.lines.iter().any(|l| l.account == number))
        .collect();
    let voided = |t: &SampleTransaction| ledger.voided.contains(&t.id);
    txns.sort_by(|a, b| a.date.cmp(&b.date).then(a.id.cmp(&b.id)));
    let mut running = Money::ZERO;
    let mut out = Vec::new();
    for t in txns {
        let others: Vec<&str> = t
            .lines
            .iter()
            .filter(|l| l.account != number)
            .map(|l| l.account.as_str())
            .collect();
        let category = match others.as_slice() {
            [one] => chart
                .iter()
                .find(|a| number_of(a) == Some(one))
                .map_or(Category::None, |a| Category::Account {
                    id: a.id,
                    name: Some(a.fields.name.clone()),
                }),
            [] => Category::None,
            many if many.iter().all(|n| *n == many[0]) => chart
                .iter()
                .find(|a| number_of(a) == Some(many[0]))
                .map_or(Category::None, |a| Category::Account {
                    id: a.id,
                    name: Some(a.fields.name.clone()),
                }),
            _ => Category::Split,
        };
        for (i, l) in t
            .lines
            .iter()
            .enumerate()
            .filter(|(_, l)| l.account == number)
        {
            let void = voided(t);
            // A void line moves nothing: its row carries the balance before.
            let net = match (void, debit_normal) {
                (true, _) => Money::ZERO,
                (false, true) => l.debit.checked_sub(l.credit).ok()?,
                (false, false) => l.credit.checked_sub(l.debit).ok()?,
            };
            running = running.checked_add(net).ok()?;
            // Payment is the decrease and deposit the increase, relative to
            // the account's normal side (the engine's convention).
            let (decrease, increase) = if debit_normal {
                (l.credit, l.debit)
            } else {
                (l.debit, l.credit)
            };
            let cleared = match (t.cleared, t.date <= reconciled_through) {
                (true, true) => ClearState::Reconciled,
                (true, false) => ClearState::Cleared,
                _ => ClearState::None,
            };
            let kind = txn_type(&t.kind);
            let number = (!t.number.is_empty()).then(|| t.number.clone());
            out.push(Built {
                amount: net.abs(),
                line: RegisterLine {
                    line_id: line_id(t, i),
                    txn_id: txn_id(t),
                    txn_type: kind,
                    check_number: (t.kind == "check").then(|| t.number.parse().ok()).flatten(),
                    number,
                    date: t.date,
                    payee: t.contact.clone(),
                    contact: None,
                    category: category.clone(),
                    memo: (!t.memo.is_empty()).then(|| t.memo.clone()),
                    payment: (!void && decrease.is_positive())
                        .then_some(RegisterAmount::Exact(decrease)),
                    deposit: (!void && increase.is_positive())
                        .then_some(RegisterAmount::Exact(increase)),
                    balance: RegisterAmount::Exact(running),
                    cleared,
                    has_splits: matches!(category, Category::Split),
                    engine_editable: cleared != ClearState::Reconciled && !void,
                    status: if void {
                        TxnStatus::Void
                    } else {
                        TxnStatus::Posted
                    },
                },
            });
        }
    }
    Some(out)
}

fn matches(b: &Built, q: &RegisterQuery) -> bool {
    let l = &b.line;
    if l.date < q.start || l.date > q.end {
        return false;
    }
    if q.txn_type.is_some_and(|k| k != l.txn_type) {
        return false;
    }
    if let Some(p) = q.payee.as_deref().filter(|p| !p.is_empty()) {
        let want = p.to_lowercase();
        if !l
            .payee
            .as_deref()
            .is_some_and(|n| n.to_lowercase().contains(&want))
        {
            return false;
        }
    }
    if let Some(s) = q.search.as_deref().filter(|s| !s.is_empty()) {
        let want = s.to_lowercase();
        let has = |v: Option<&str>| v.is_some_and(|v| v.to_lowercase().contains(&want));
        if !(has(l.number.as_deref()) || has(l.payee.as_deref()) || has(l.memo.as_deref())) {
            return false;
        }
    }
    // The sample lines carry no tags.
    if q.tag.is_some() {
        return false;
    }
    if q.min.is_some_and(|m| b.amount < m) || q.max.is_some_and(|m| b.amount > m) {
        return false;
    }
    match q.cleared {
        Some(cl_api::ClearedFilter::Uncleared) => l.cleared == ClearState::None,
        Some(cl_api::ClearedFilter::Cleared) => l.cleared == ClearState::Cleared,
        Some(cl_api::ClearedFilter::Reconciled) => l.cleared == ClearState::Reconciled,
        None => true,
    }
}

fn type_name(t: TxnType) -> String {
    serde_json::to_value(t)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// One page of the demo register, shaped like the engine's answer with
/// `register-exact-v1` (exact amounts, balances that ignore filters and
/// sort). `page` counts from 1.
#[must_use]
pub fn page(
    ledger: &DemoLedger,
    chart: &[Account],
    account: AccountId,
    q: &RegisterQuery,
    page: u32,
    per_page: u16,
) -> Option<RegisterPage> {
    let acct = chart.iter().find(|a| a.id == account)?;
    let all = build(ledger, chart, acct)?;
    let balance_forward = all
        .iter()
        .take_while(|b| b.line.date < q.start)
        .last()
        .and_then(|b| b.line.balance.exact())
        .unwrap_or(Money::ZERO);
    let ending = all
        .iter()
        .take_while(|b| b.line.date <= q.end)
        .last()
        .and_then(|b| b.line.balance.exact())
        .unwrap_or(Money::ZERO);
    let mut rows: Vec<Built> = all.into_iter().filter(|b| matches(b, q)).collect();
    let by_date = |a: &Built, b: &Built| a.line.date.cmp(&b.line.date);
    rows.sort_by(|a, b| {
        let o = match q.sort {
            RegisterSort::Date => Ordering::Equal,
            RegisterSort::Amount => a.amount.cmp(&b.amount),
            RegisterSort::Type => type_name(a.line.txn_type).cmp(&type_name(b.line.txn_type)),
            RegisterSort::Number => a.line.number.cmp(&b.line.number),
        };
        let o = o.then_with(|| by_date(a, b));
        if q.dir == SortDir::Desc {
            o.reverse()
        } else {
            o
        }
    });
    let total_rows = rows.len() as u64;
    let per = u64::from(per_page.max(1));
    let skip = usize::try_from(u64::from(page.saturating_sub(1)) * per).unwrap_or(usize::MAX);
    let lines: Vec<RegisterLine> = rows
        .into_iter()
        .skip(skip)
        .take(usize::from(per_page.max(1)))
        .map(|b| b.line)
        .collect();
    let allowed = match acct.fields.detail_type.as_deref() {
        Some("bank") => vec![
            EntryType::Expense,
            EntryType::Deposit,
            EntryType::Transfer,
            EntryType::JournalEntry,
        ],
        Some("credit_card") => vec![
            EntryType::Expense,
            EntryType::Transfer,
            EntryType::JournalEntry,
        ],
        _ => vec![EntryType::JournalEntry],
    };
    Some(RegisterPage {
        account: RegisterAccount {
            id: acct.id,
            account_type: acct.fields.account_type,
            detail_type: acct.fields.detail_type.clone(),
        },
        exact: true,
        balance_forward: RegisterAmount::Exact(balance_forward),
        ending_balance: Some(RegisterAmount::Exact(ending)),
        page,
        per_page,
        total_rows,
        total_pages: total_rows.div_ceil(per),
        allowed,
        lines,
    })
}

/// The demo register footer through `end` (all dates when `None`).
#[must_use]
pub fn summary(
    ledger: &DemoLedger,
    chart: &[Account],
    account: AccountId,
    end: Option<CalendarDate>,
) -> Option<RegisterSummary> {
    let acct = chart.iter().find(|a| a.id == account)?;
    let all = build(ledger, chart, acct)?;
    let through = |b: &&Built| end.is_none_or(|e| b.line.date <= e);
    let last_balance = |it: &mut dyn Iterator<Item = &Built>| {
        it.last()
            .and_then(|b| b.line.balance.exact())
            .unwrap_or(Money::ZERO)
    };
    // The stored account balance is debits minus credits for every type.
    let natural = last_balance(&mut all.iter());
    let current = if debit_normal(acct.fields.account_type) {
        natural
    } else {
        natural.checked_neg().ok()?
    };
    let ending = last_balance(&mut all.iter().filter(through));
    // The demo engine's own cleared total (not the screen's arithmetic).
    let cleared = all
        .iter()
        .filter(through)
        .filter(|b| b.line.cleared != ClearState::None)
        .try_fold(Money::ZERO, |acc, b| {
            let net = match (&b.line.deposit, &b.line.payment) {
                (Some(d), _) => d.exact(),
                (None, Some(p)) => p.exact().and_then(|m| m.checked_neg().ok()),
                _ => Some(Money::ZERO),
            }?;
            acc.checked_add(net).ok()
        })?;
    let uncleared = all
        .iter()
        .filter(through)
        .filter(|b| b.line.cleared == ClearState::None)
        .count() as u64;
    let reconciled = all
        .iter()
        .any(|b| b.line.cleared == ClearState::Reconciled)
        .then(last_reconciled);
    Some(RegisterSummary {
        exact: true,
        current_balance: RegisterAmount::Exact(current),
        ending_balance: Some(RegisterAmount::Exact(ending)),
        cleared_balance: Some(RegisterAmount::Exact(cleared)),
        as_of: end,
        uncleared_count: uncleared,
        last_reconciled: reconciled,
    })
}

/// Ids of transactions recorded in the demo start here (the fixture's are
/// below).
const NEW_BASE: u32 = 900_000;

/// The demo's own changes to the sample ledger: transactions recorded or
/// changed through a register (P12). Synthetic.
#[derive(Clone, Debug, Default)]
pub struct DemoLedger {
    changed: std::collections::BTreeMap<u32, SampleTransaction>,
    /// Voided in the demo (kept, amounts zero).
    voided: std::collections::BTreeSet<u32>,
    /// Deleted in the demo (gone from the register; R7).
    deleted: std::collections::BTreeSet<u32>,
}

/// The demo company's closing-date password (synthetic; the demo plays an
/// engine with `closing-password-v1`). Documented in the register scenario.
pub const DEMO_CLOSING_PASSWORD: &str = "demo-close";

/// The demo engine's closed-period rule (U-15): a change touching a date on
/// or before `closing` needs the closing-date password.
///
/// # Errors
/// `ClosedPeriod` (password required) without a password,
/// `ClosingPasswordRejected` for a wrong one.
pub fn check_closing(
    closing: Option<CalendarDate>,
    dates: &[CalendarDate],
    password: Option<&cl_core::Secret>,
) -> Result<(), cl_api::ApiError> {
    if !dates.iter().any(|d| closing.is_some_and(|c| *d <= c)) {
        return Ok(());
    }
    match password {
        None => Err(cl_api::ApiError::ClosedPeriod(cl_api::ClosedPeriod {
            lock_date: closing,
            can_override: false,
            password_required: Some(true),
            message: cl_api::UserText::default(),
        })),
        Some(p) if p.expose() == DEMO_CLOSING_PASSWORD => Ok(()),
        Some(_) => Err(cl_api::ApiError::ClosingPasswordRejected),
    }
}

fn refused(reason: cl_api::ConflictReason) -> cl_api::ApiError {
    cl_api::ApiError::Conflict {
        reason,
        current_version: None,
        code: None,
        message: cl_api::UserText::default(),
    }
}

/// A posting as the demo engine stores it (built by the register screen
/// from the owner's typed values; amounts exact).
#[derive(Clone, Debug)]
pub struct DemoPosting {
    /// `check`, `deposit`, `transfer`, `card_charge` or `journal_entry`.
    pub kind: &'static str,
    /// Date.
    pub date: CalendarDate,
    /// Number (empty for none).
    pub number: String,
    /// Payee name.
    pub contact: Option<String>,
    /// Memo.
    pub memo: String,
    /// The register's account.
    pub own: AccountId,
    /// The register's account is debited (money in).
    pub own_debit: bool,
    /// The other side's accounts and amounts.
    pub lines: Vec<(AccountId, Money)>,
}

fn sample_id(id: TransactionId) -> Option<u32> {
    let s = id.to_string();
    let tail = s.rsplit('-').next()?;
    tail.strip_prefix('b')?.parse().ok()
}

impl DemoLedger {
    /// The sample's transactions with the demo's changes, then the ones
    /// recorded in the demo.
    #[must_use]
    pub fn transactions(&self) -> Vec<SampleTransaction> {
        let mut v: Vec<SampleTransaction> = Sample::load()
            .transactions
            .into_iter()
            .map(|t| self.changed.get(&t.id).cloned().unwrap_or(t))
            .collect();
        v.extend(self.changed.range(NEW_BASE..).map(|(_, t)| t.clone()));
        v.retain(|t| !self.deleted.contains(&t.id));
        v
    }

    fn find(&self, id: u32) -> Option<SampleTransaction> {
        if self.deleted.contains(&id) {
            return None;
        }
        self.changed
            .get(&id)
            .cloned()
            .or_else(|| Sample::load().transactions.into_iter().find(|t| t.id == id))
    }

    /// The date of a transaction (for the closed-period rule).
    #[must_use]
    pub fn date_of(&self, id: TransactionId) -> Option<CalendarDate> {
        self.find(sample_id(id)?).map(|t| t.date)
    }

    /// Voids a transaction (amounts become zero; it stays listed).
    ///
    /// # Errors
    /// `NotFound`; `Validation`-like refusal when it is already void.
    pub fn void(&mut self, id: TransactionId) -> Result<(), cl_api::ApiError> {
        let n = sample_id(id)
            .filter(|n| self.find(*n).is_some())
            .ok_or(cl_api::ApiError::NotFound { code: None })?;
        if !self.voided.insert(n) {
            return Err(refused(cl_api::ConflictReason::Unspecified));
        }
        Ok(())
    }

    /// Soft-deletes a transaction (R7): gone from the register; only the
    /// kinds of Q-245, never a reconciled one.
    ///
    /// # Errors
    /// `NotFound`; `Conflict` with `DeleteUnsupported` or `Reconciled`.
    pub fn delete(&mut self, id: TransactionId) -> Result<(), cl_api::ApiError> {
        let t = sample_id(id)
            .and_then(|n| self.find(n))
            .ok_or(cl_api::ApiError::NotFound { code: None })?;
        if !txn_type(&t.kind).is_deletable() {
            return Err(refused(cl_api::ConflictReason::DeleteUnsupported));
        }
        if t.cleared && t.date <= last_reconciled() {
            return Err(refused(cl_api::ConflictReason::Reconciled));
        }
        self.deleted.insert(t.id);
        Ok(())
    }

    /// The next check number in `account` (the highest numeric one + 1).
    #[must_use]
    pub fn next_check_number(&self, chart: &[Account], account: AccountId) -> Option<u32> {
        let number = chart.iter().find(|a| a.id == account).and_then(number_of)?;
        self.transactions()
            .iter()
            .filter(|t| t.kind == "check" && t.lines.iter().any(|l| l.account == number))
            .filter_map(|t| t.number.parse::<u32>().ok())
            .max()
            .map(|n| n.saturating_add(1))
    }

    /// Records (or, with `existing`, replaces) a posting; `None` when an
    /// account is not in the chart or the transaction is unknown.
    pub fn save(
        &mut self,
        chart: &[Account],
        p: &DemoPosting,
        existing: Option<TransactionId>,
    ) -> Option<TransactionId> {
        let num = |id: AccountId| {
            chart
                .iter()
                .find(|a| a.id == id)
                .and_then(number_of)
                .map(str::to_string)
        };
        let side = |account: String, amount: Money, debit: bool| cl_company::sample::SampleLine {
            account,
            debit: if debit { amount } else { Money::ZERO },
            credit: if debit { Money::ZERO } else { amount },
        };
        let mut lines = Vec::new();
        for (a, m) in &p.lines {
            lines.push(side(num(*a)?, *m, !p.own_debit));
        }
        let total = Money::sum(p.lines.iter().map(|(_, m)| *m)).ok()?;
        lines.push(side(num(p.own)?, total, p.own_debit));
        let (id, cleared) = if let Some(t) = existing {
            let id = sample_id(t)?;
            (id, self.find(id)?.cleared)
        } else {
            let last = self.changed.range(NEW_BASE..).next_back().map(|(k, _)| *k);
            (last.map_or(NEW_BASE, |k| k + 1), false)
        };
        let t = SampleTransaction {
            id,
            date: p.date,
            kind: p.kind.to_string(),
            number: p.number.clone(),
            contact: p.contact.clone(),
            memo: p.memo.clone(),
            due: None,
            cleared,
            attachment: false,
            lines,
        };
        let tid = txn_id(&t);
        self.changed.insert(id, t);
        Some(tid)
    }

    /// One transaction as `GET /transactions/{id}` answers it.
    #[must_use]
    pub fn get(
        &self,
        chart: &[Account],
        contacts: &[cl_api::Contact],
        id: TransactionId,
    ) -> Option<cl_api::Versioned<cl_api::Transaction>> {
        use cl_api::EditableFields;
        let t = self.find(sample_id(id)?)?;
        let contact = t.contact.as_deref().and_then(|n| {
            contacts
                .iter()
                .find(|c| c.fields.display_name == n)
                .map(|c| c.id)
        });
        let deposit = t.kind == "deposit";
        let mut lines = Vec::new();
        for (i, l) in t.lines.iter().enumerate() {
            // A fixture account outside the demo chart keeps its number.
            let a = chart
                .iter()
                .find(|a| number_of(a) == Some(l.account.as_str()));
            let own_side = a.is_some_and(|a| {
                matches!(
                    a.fields.detail_type.as_deref(),
                    Some("bank" | "credit_card")
                )
            });
            lines.push(cl_api::TransactionLine {
                id: line_id(&t, i),
                account_id: a.map_or_else(
                    || super::account_id(l.account.parse().unwrap_or(0)),
                    |a| a.id,
                ),
                account_name: a.map(|a| a.fields.name.clone()),
                account_number: Some(l.account.clone()),
                debit: l.debit,
                credit: l.credit,
                description: None,
                line_order: i64::try_from(i).ok(),
                tag_id: None,
                tag_name: None,
                contact_id: if deposit && !own_side { contact } else { None },
            });
        }
        let record = cl_api::Transaction {
            id: txn_id(&t),
            txn_type: txn_type(&t.kind),
            txn_number: (!t.number.is_empty() && t.kind != "check").then(|| t.number.clone()),
            check_number: (t.kind == "check").then(|| t.number.parse().ok()).flatten(),
            txn_date: t.date,
            status: if self.voided.contains(&t.id) {
                TxnStatus::Void
            } else {
                TxnStatus::Posted
            },
            basis: cl_api::Basis::Both,
            contact_id: if deposit { None } else { contact },
            contact_name: if deposit { None } else { t.contact.clone() },
            memo: (!t.memo.is_empty()).then(|| t.memo.clone()),
            total: None,
            void_reason: None,
            created_at: Some(super::DemoState::now()),
            updated_at: Some(super::DemoState::now()),
            lines,
        };
        let version = cl_api::RecordVersion::Fingerprint(record.fingerprint());
        Some(cl_api::Versioned { record, version })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn operating(chart: &[Account]) -> AccountId {
        chart
            .iter()
            .find(|a| a.fields.account_number.as_deref() == Some("1010"))
            .map(|a| a.id)
            .expect("1010")
    }

    #[test]
    fn pages_cover_the_range_once_and_end_on_the_account_balance() {
        let chart = super::super::chart();
        let id = operating(&chart);
        let all = RegisterQuery::new(CalendarDate::min(), CalendarDate::max());
        let first = page(&DemoLedger::default(), &chart, id, &all, 1, 200).expect("page");
        assert!(first.exact);
        assert!(first.total_rows > 200, "{}", first.total_rows);
        let mut seen = std::collections::BTreeSet::new();
        let mut last = None;
        for p in 1..=u32::try_from(first.total_pages).expect("pages") {
            let pg = page(&DemoLedger::default(), &chart, id, &all, p, 200).expect("page");
            for l in &pg.lines {
                assert!(seen.insert(l.line_id), "a line once");
                last = l.balance.exact();
            }
        }
        assert_eq!(seen.len() as u64, first.total_rows);
        let acct = chart.iter().find(|a| a.id == id).expect("acct");
        assert_eq!(last, acct.balance, "the last balance is the account's");
        let s = summary(&DemoLedger::default(), &chart, id, None).expect("summary");
        assert_eq!(s.ending_balance.and_then(|a| a.exact()), acct.balance);
        assert_eq!(s.last_reconciled, Some(last_reconciled()));
        assert!(s.uncleared_count > 0);
    }

    #[test]
    fn void_and_delete_follow_the_engine_rules() {
        let chart = super::super::chart();
        let id = operating(&chart);
        let all = RegisterQuery::new(CalendarDate::min(), CalendarDate::max());
        let mut ledger = DemoLedger::default();
        let first = page(&ledger, &chart, id, &all, 1, 1).expect("page");
        let total = first.total_rows;
        let day = CalendarDate::from_ymd(2026, 11, 2).expect("d");
        let fuel = chart
            .iter()
            .find(|a| a.fields.account_number.as_deref() == Some("6100"))
            .expect("6100")
            .id;
        let posting = DemoPosting {
            kind: "check",
            date: day,
            number: "9001".into(),
            contact: None,
            memo: String::new(),
            own: id,
            own_debit: false,
            lines: vec![(fuel, Money::parse_wire("10.00").expect("m"))],
        };
        let a = ledger.save(&chart, &posting, None).expect("a");
        let b = ledger.save(&chart, &posting, None).expect("b");
        let before = summary(&ledger, &chart, id, None).expect("s");
        ledger.void(a).expect("void");
        assert!(ledger.void(a).is_err(), "already void");
        let after = summary(&ledger, &chart, id, None).expect("s");
        assert_eq!(
            after.current_balance.exact(),
            before
                .current_balance
                .exact()
                .and_then(|m| m.checked_add(Money::parse_wire("10.00").ok()?).ok())
        );
        let listed = page(&ledger, &chart, id, &all, 1, 1).expect("page");
        assert_eq!(listed.total_rows, total + 2, "a void stays listed");
        ledger.delete(b).expect("delete");
        assert_eq!(
            page(&ledger, &chart, id, &all, 1, 1)
                .expect("page")
                .total_rows,
            total + 1
        );
        assert!(ledger.get(&chart, &[], b).is_none(), "gone");
        // Closed period: password required, wrong, right.
        let closing = CalendarDate::from_ymd(2026, 9, 30).ok();
        let locked = CalendarDate::from_ymd(2026, 9, 1).expect("d");
        assert!(matches!(
            check_closing(closing, &[locked], None),
            Err(cl_api::ApiError::ClosedPeriod(c)) if c.password_required == Some(true)
        ));
        assert_eq!(
            check_closing(closing, &[locked], Some(&cl_core::Secret::new("no"))),
            Err(cl_api::ApiError::ClosingPasswordRejected)
        );
        assert!(
            check_closing(
                closing,
                &[locked],
                Some(&cl_core::Secret::new(DEMO_CLOSING_PASSWORD))
            )
            .is_ok()
        );
        assert!(check_closing(closing, &[day], None).is_ok());
    }

    #[test]
    fn balances_ignore_sort_and_filters() {
        let chart = super::super::chart();
        let id = operating(&chart);
        let mut q = RegisterQuery::new(
            CalendarDate::from_ymd(2026, 9, 1).expect("d"),
            CalendarDate::from_ymd(2026, 10, 31).expect("d"),
        );
        let base = page(&DemoLedger::default(), &chart, id, &q, 1, 200).expect("page");
        let by_line = |p: &RegisterPage| -> Vec<(JournalLineId, Option<Money>)> {
            let mut v: Vec<_> = p
                .lines
                .iter()
                .map(|l| (l.line_id, l.balance.exact()))
                .collect();
            v.sort_by_key(|(k, _)| *k);
            v
        };
        q.sort = RegisterSort::Amount;
        q.dir = SortDir::Desc;
        let sorted = page(&DemoLedger::default(), &chart, id, &q, 1, 200).expect("page");
        assert_eq!(by_line(&sorted), by_line(&base));
        q.min = Some(Money::parse_wire("1000").expect("m"));
        let filtered = page(&DemoLedger::default(), &chart, id, &q, 1, 200).expect("page");
        assert!(filtered.lines.len() < base.lines.len());
        let all = by_line(&base);
        for (k, b) in by_line(&filtered) {
            assert!(all.contains(&(k, b)));
        }
        assert!(
            page(
                &DemoLedger::default(),
                &chart,
                AccountId::parse("d0000000-0000-4000-8000-a99999999999").expect("id"),
                &q,
                1,
                1
            )
            .is_none()
        );
    }
}
