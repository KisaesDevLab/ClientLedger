//! Who answers Transfer Funds when it is not the live engine over the
//! queue: the `--demo` engine ([`DemoTransfers`]) or a test double.

use cl_api::{ApiError, LocalValidation, RecordVersion, Transaction, TransferDraft, Versioned};
use cl_core::ids::{AccountId, TransactionId};
use cl_core::{CalendarDate, Money, Secret};

use crate::company::CompanyLink;
use crate::company::demo::DemoState;
use crate::company::demo::register::{DemoLedger, DemoPosting, check_closing};

/// One saved transfer touching the account (Prev/Next).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TransferRow {
    pub id: TransactionId,
    pub date: CalendarDate,
}

/// Transfer Funds' engine when it is not the live one.
pub(crate) trait LocalTransfers: Send {
    /// Creates (or with `edit`, replaces) a transfer.
    fn save(
        &mut self,
        link: &CompanyLink,
        draft: &TransferDraft,
        edit: Option<(TransactionId, RecordVersion)>,
        password: Option<&Secret>,
    ) -> Result<(TransactionId, RecordVersion), ApiError>;
    /// A saved transaction.
    fn get(
        &mut self,
        link: &CompanyLink,
        id: TransactionId,
    ) -> Result<Versioned<Transaction>, ApiError>;
    /// Transfers touching `account`, newest first.
    fn list(&mut self, link: &CompanyLink, account: AccountId) -> Vec<TransferRow>;
    /// Voids one.
    fn void(
        &mut self,
        link: &CompanyLink,
        id: TransactionId,
        password: Option<&Secret>,
    ) -> Result<(), ApiError>;
    /// An account's balance as the engine stores it.
    fn balance(&mut self, link: &CompanyLink, account: AccountId) -> Option<Money>;
}

/// The `--demo` engine's transfers.
#[derive(Debug, Default)]
pub(crate) struct DemoTransfers;

fn demo(link: &CompanyLink) -> Result<std::sync::MutexGuard<'_, DemoState>, ApiError> {
    link.demo_state()
        .ok_or(ApiError::Local(LocalValidation::InvalidValue))
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
    state
        .chart
        .get_or_insert_with(crate::company::demo::chart)
        .clone()
}

impl LocalTransfers for DemoTransfers {
    fn save(
        &mut self,
        link: &CompanyLink,
        draft: &TransferDraft,
        edit: Option<(TransactionId, RecordVersion)>,
        password: Option<&Secret>,
    ) -> Result<(TransactionId, RecordVersion), ApiError> {
        let mut state = demo(link)?;
        let chart = chart(&mut state);
        let mut dates = vec![draft.date];
        if let Some((id, _)) = edit {
            dates.extend(state.ledger.date_of(id));
        }
        check_closing(closing(&state), &dates, password)?;
        // From is credited, To debited, by exactly the amount.
        let p = DemoPosting {
            kind: "transfer",
            date: draft.date,
            number: String::new(),
            contact: None,
            memo: draft.memo.clone().unwrap_or_default(),
            own: draft.from,
            own_debit: false,
            lines: vec![(draft.to, draft.amount)],
        };
        let id = state
            .ledger
            .save(&chart, &p, edit.map(|(id, _)| id))
            .ok_or(ApiError::Local(LocalValidation::InvalidValue))?;
        let contacts = state.contacts_mut().clone();
        let got = state
            .ledger
            .get(&chart, &contacts, id)
            .ok_or(ApiError::NotFound { code: None })?;
        Ok((id, got.version))
    }

    fn get(
        &mut self,
        link: &CompanyLink,
        id: TransactionId,
    ) -> Result<Versioned<Transaction>, ApiError> {
        let mut state = demo(link)?;
        let chart = chart(&mut state);
        let contacts = state.contacts_mut().clone();
        state
            .ledger
            .get(&chart, &contacts, id)
            .ok_or(ApiError::NotFound { code: None })
    }

    fn list(&mut self, link: &CompanyLink, account: AccountId) -> Vec<TransferRow> {
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
        let mut rows: Vec<TransferRow> = state
            .ledger
            .transactions()
            .into_iter()
            .filter(|t| t.kind == "transfer" && t.lines.iter().any(|l| l.account == number))
            .map(|t| TransferRow {
                id: DemoLedger::id_of(&t),
                date: t.date,
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
        let old = state
            .ledger
            .date_of(id)
            .ok_or(ApiError::NotFound { code: None })?;
        check_closing(closing(&state), &[old], password)?;
        state.ledger.void(id)
    }

    fn balance(&mut self, link: &CompanyLink, account: AccountId) -> Option<Money> {
        let mut state = demo(link).ok()?;
        crate::company::demo::checks::balance(&mut state, account)
    }
}
