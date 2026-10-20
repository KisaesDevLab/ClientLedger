//! Who answers Make Deposit's reads and writes when they don't go to the
//! engine over the queue: the `--demo` engine ([`DemoDeposits`]) or a test
//! double. Each answers at once (the screen wraps it in `Task::ready`).
//!
//! The demo posts deposits through `DemoLedger::save_lines` exactly as the
//! engine journals them (bank debit of the total, payments and lines as
//! credits, fees and cash back as debits). Its pending payments and
//! bank-file groupings come from the WP-D.3 twins
//! (`company::demo::deposits`) once they land; until then the demo has
//! none waiting and the screen shows its empty state.

use cl_api::{
    ApiError, DepositCaps, DepositDraft, FeedDeposit, FeedGrouping, FeedGroupings, FeedItemId,
    LocalValidation, PendingDeposits, PendingPayment, RecordVersion, RegisterAmount, Transaction,
    TxnType, Versioned,
};
use cl_core::ids::{AccountId, ContactId, TransactionId};
use cl_core::{CalendarDate, Money, Secret};

use crate::company::CompanyLink;
use crate::company::demo::DemoState;
use crate::company::demo::deposits as twins;
use crate::company::demo::register::{DemoJournal, DemoLedger, DemoLine, check_closing};

/// A save the engine took.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Saved {
    pub id: TransactionId,
    pub version: RecordVersion,
    pub date: CalendarDate,
    /// The engine warned that Payments Clearing now holds a net credit.
    pub clearing_negative: bool,
}

/// One saved deposit in the account (Prev/Next, recent deposits).
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct DepositRow {
    pub id: TransactionId,
    pub date: CalendarDate,
    /// The engine's total (the bank amount).
    pub total: Option<Money>,
    pub void: bool,
}

impl std::fmt::Debug for DepositRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DepositRow")
            .field("id", &self.id)
            .field("date", &self.date)
            .finish_non_exhaustive()
    }
}

/// A vendor credit a refund can apply to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct VendorCredit {
    pub id: TransactionId,
    pub date: CalendarDate,
    /// Its number, if any.
    pub number: Option<String>,
}

/// Make Deposit's engine when it is not the live one.
pub(crate) trait LocalDeposits: Send {
    /// The deposit parts this engine takes.
    fn caps(&self) -> DepositCaps;
    /// It suggests groupings for bank-file deposits.
    fn feed_groupings(&self) -> bool {
        false
    }
    /// Payments waiting in Payments Clearing.
    fn pending(&mut self, link: &CompanyLink) -> Result<PendingDeposits, ApiError>;
    /// Bank-file deposits waiting.
    fn waiting(&mut self, _link: &CompanyLink) -> Vec<FeedDeposit> {
        Vec::new()
    }
    /// Suggested groupings for a bank-file deposit.
    fn groupings(
        &mut self,
        _link: &CompanyLink,
        _id: FeedItemId,
    ) -> Result<FeedGroupings, ApiError> {
        Err(ApiError::Local(LocalValidation::FeedGroupingsUnavailable))
    }
    /// Creates (or with `edit`, replaces) a deposit.
    fn save(
        &mut self,
        link: &CompanyLink,
        draft: &DepositDraft,
        edit: Option<(TransactionId, RecordVersion)>,
        password: Option<&Secret>,
    ) -> Result<Saved, ApiError>;
    /// A saved transaction (`GET /transactions/{id}`).
    fn get(
        &mut self,
        link: &CompanyLink,
        id: TransactionId,
    ) -> Result<Versioned<Transaction>, ApiError>;
    /// The deposits into `account`, newest first.
    fn list(&mut self, link: &CompanyLink, account: AccountId) -> Vec<DepositRow>;
    /// Voids a deposit.
    fn void(
        &mut self,
        link: &CompanyLink,
        id: TransactionId,
        password: Option<&Secret>,
    ) -> Result<(), ApiError>;
    /// The account's balance as the engine reports it.
    fn balance(&mut self, link: &CompanyLink, account: AccountId) -> Option<Money>;
    /// A vendor's credits a refund can apply to.
    fn vendor_credits(&mut self, _link: &CompanyLink, _vendor: ContactId) -> Vec<VendorCredit> {
        Vec::new()
    }
}

/// The `--demo` engine's deposits.
#[derive(Debug, Default)]
pub(crate) struct DemoDeposits;

fn demo(link: &CompanyLink) -> Result<std::sync::MutexGuard<'_, DemoState>, ApiError> {
    link.demo_state()
        .ok_or(ApiError::Local(LocalValidation::InvalidValue))
}

fn closing(state: &DemoState) -> Option<CalendarDate> {
    state.open.and_then(|open| {
        state
            .companies
            .iter()
            .find(|c| c.tenant_id == open)
            .and_then(|c| c.closed_through)
    })
}

fn chart(state: &mut DemoState) -> Vec<cl_api::Account> {
    state
        .chart
        .get_or_insert_with(crate::company::demo::chart)
        .clone()
}

/// The deposit's journal as the engine writes it (bank debit of the exact
/// total first, then each part in body order).
pub(crate) fn journal(d: &DepositDraft) -> Option<DemoJournal> {
    let side = |account: AccountId, amount: Money, debit: bool, memo: Option<String>| DemoLine {
        account,
        debit: if debit { amount } else { Money::ZERO },
        credit: if debit { Money::ZERO } else { amount },
        memo,
    };
    let mut lines = vec![side(d.account, d.total()?, true, None)];
    lines.extend(
        d.payments
            .iter()
            .map(|p| side(p.account, p.amount, false, p.memo.clone())),
    );
    lines.extend(
        d.lines
            .iter()
            .map(|l| side(l.account, l.amount, false, l.memo.clone())),
    );
    lines.extend(
        d.deductions
            .iter()
            .map(|l| side(l.account, l.amount, true, l.memo.clone())),
    );
    if let Some(c) = &d.cash_back {
        lines.push(side(c.account, c.amount, true, c.memo.clone()));
    }
    Some(DemoJournal {
        kind: "deposit",
        date: d.date,
        number: String::new(),
        contact: None,
        memo: d.memo.clone().unwrap_or_default(),
        lines,
    })
}

impl LocalDeposits for DemoDeposits {
    fn caps(&self) -> DepositCaps {
        DepositCaps {
            grouping: true,
            cashback: true,
            vendor_refund: true,
        }
    }

    fn feed_groupings(&self) -> bool {
        true
    }

    fn pending(&mut self, link: &CompanyLink) -> Result<PendingDeposits, ApiError> {
        let mut state = demo(link)?;
        let balance = twins::clearing_balance(&mut state);
        let items = twins::pending(&mut state).iter().map(pending_of).collect();
        Ok(PendingDeposits {
            exact: true,
            clearing_balance: RegisterAmount::Exact(balance),
            items,
        })
    }

    fn waiting(&mut self, link: &CompanyLink) -> Vec<FeedDeposit> {
        let Ok(mut state) = demo(link) else {
            return Vec::new();
        };
        let all = twins::feed_deposits();
        twins::bank_deposits_waiting(&mut state)
            .into_iter()
            .filter_map(|(f, _)| {
                let i = all
                    .iter()
                    .position(|x| x.date == f.date && x.amount == f.amount)?;
                Some(FeedDeposit {
                    id: feed_id(i)?,
                    date: f.date,
                    amount: f.amount.abs(),
                    description: Some(f.description.clone()),
                    account_name: None,
                })
            })
            .collect()
    }

    fn groupings(&mut self, link: &CompanyLink, id: FeedItemId) -> Result<FeedGroupings, ApiError> {
        let mut state = demo(link)?;
        let all = twins::feed_deposits();
        let f = (0..all.len())
            .find(|i| feed_id(*i) == Some(id))
            .and_then(|i| all.get(i))
            .ok_or(ApiError::NotFound { code: None })?;
        let g = twins::deposit_groupings(&mut state, f, usize::from(cl_api::GROUPINGS_LIMIT));
        Ok(FeedGroupings {
            target: g.target,
            candidates: g
                .candidates
                .iter()
                .map(|c| FeedGrouping {
                    items: c.iter().map(pending_of).collect(),
                })
                .collect(),
            truncated: g.truncated,
        })
    }

    fn vendor_credits(&mut self, link: &CompanyLink, vendor: ContactId) -> Vec<VendorCredit> {
        let Ok(mut state) = demo(link) else {
            return Vec::new();
        };
        crate::company::demo::money::vendor_credits(&mut state, vendor)
            .into_iter()
            .filter(|c| c.open.is_positive())
            .map(|c| VendorCredit {
                id: c.id,
                date: c.date,
                number: None,
            })
            .collect()
    }

    fn save(
        &mut self,
        link: &CompanyLink,
        draft: &DepositDraft,
        edit: Option<(TransactionId, RecordVersion)>,
        password: Option<&Secret>,
    ) -> Result<Saved, ApiError> {
        let mut state = demo(link)?;
        let chart = chart(&mut state);
        let mut dates = vec![draft.date];
        if let Some((id, _)) = edit {
            dates.extend(state.ledger.date_of(id));
        }
        let id = if edit.is_none() {
            // A new deposit goes through the twin (links, 409, cash back).
            twins::create(&mut state, draft, password)?
        } else {
            check_closing(closing(&state), &dates, password)?;
            let j = journal(draft).ok_or(ApiError::Local(LocalValidation::InvalidValue))?;
            state
                .ledger
                .save_lines(&chart, &j, edit.map(|(id, _)| id))
                .ok_or(ApiError::Local(LocalValidation::InvalidValue))?
        };
        let contacts = state.contacts_mut().clone();
        let got = state
            .ledger
            .get(&chart, &contacts, id)
            .ok_or(ApiError::NotFound { code: None })?;
        Ok(Saved {
            id,
            version: got.version,
            date: draft.date,
            clearing_negative: false,
        })
    }

    fn get(
        &mut self,
        link: &CompanyLink,
        id: TransactionId,
    ) -> Result<Versioned<Transaction>, ApiError> {
        let mut state = demo(link)?;
        let chart = chart(&mut state);
        let contacts = state.contacts_mut().clone();
        let mut got = state
            .ledger
            .get(&chart, &contacts, id)
            .ok_or(ApiError::NotFound { code: None })?;
        // The twin keeps the links and the cash back the ledger doesn't.
        if let Some(r) = twins::record(&mut state, id) {
            let mut sources = r.sources.clone();
            for l in &mut got.record.lines {
                if let Some(i) = sources
                    .iter()
                    .position(|(_, m)| l.credit == *m && l.debit.is_zero())
                    && Some(l.account_id) == twins::clearing_account(&mut state)
                {
                    l.source_transaction_id = Some(sources.remove(i).0);
                } else if r
                    .cash_back
                    .is_some_and(|(a, m)| a == l.account_id && l.debit == m)
                {
                    l.line_role = Some(cl_api::LINE_ROLE_CASH_BACK.to_string());
                }
            }
        }
        Ok(got)
    }

    fn list(&mut self, link: &CompanyLink, account: AccountId) -> Vec<DepositRow> {
        let Ok(mut state) = demo(link) else {
            return Vec::new();
        };
        let chart = chart(&mut state);
        let Some(number) = chart
            .iter()
            .find(|a| a.id == account)
            .and_then(|a| a.fields.account_number.clone())
        else {
            return Vec::new();
        };
        let mut rows: Vec<DepositRow> = state
            .ledger
            .transactions()
            .into_iter()
            .filter(|t| t.kind == "deposit" && t.lines.iter().any(|l| l.account == number))
            .map(|t| {
                let id = DemoLedger::id_of(&t);
                DepositRow {
                    id,
                    date: t.date,
                    total: t
                        .lines
                        .iter()
                        .find(|l| l.account == number && l.debit.is_positive())
                        .map(|l| l.debit),
                    void: state.ledger.is_void(id),
                }
            })
            .collect();
        rows.sort_by(|a, b| b.date.cmp(&a.date).then(b.id.cmp(&a.id)));
        rows
    }

    fn void(
        &mut self,
        link: &CompanyLink,
        id: TransactionId,
        password: Option<&Secret>,
    ) -> Result<(), ApiError> {
        let mut state = demo(link)?;
        twins::void(&mut state, id, password)
    }

    fn balance(&mut self, link: &CompanyLink, account: AccountId) -> Option<Money> {
        let mut state = demo(link).ok()?;
        crate::company::demo::checks::balance(&mut state, account)
    }
}

/// A demo bank-file deposit's id (the sample feed has none: its place).
fn feed_id(i: usize) -> Option<FeedItemId> {
    FeedItemId::parse(&format!("f0000000-0000-4000-8000-{i:012}")).ok()
}

fn type_name(t: TxnType) -> &'static str {
    match t {
        TxnType::CustomerPayment => "customer_payment",
        TxnType::CashSale => "cash_sale",
        TxnType::DailySales => "daily_sales",
        TxnType::JournalEntry => "journal_entry",
        _ => "other",
    }
}

fn pending_of(p: &twins::DemoPending) -> PendingPayment {
    PendingPayment {
        source: p.source,
        txn_type: type_name(p.txn_type).to_string(),
        date: p.date,
        received_from: p.received_from.clone(),
        contact: p.contact,
        ref_no: p.ref_no.clone(),
        method: None,
        memo: p.memo.clone(),
        amount: RegisterAmount::Exact(p.amount),
    }
}
