//! Who answers Pay Bills when it doesn't go to the engine over the queue:
//! the `--demo` engine (`company::demo::pay_bills`) or a test double. Each
//! answers at once (the screen wraps it in `Task::ready`).

use cl_api::{
    Account, ApiError, BillPaymentRecord, CreatedPayment, IdempotencyKey, LocalValidation,
    PayBillsRequest, PayableFilter, PayableList,
};
use cl_core::ids::{AccountId, TransactionId};
use cl_core::{CalendarDate, Money, Secret};

use crate::company::CompanyLink;
use crate::company::demo::DemoState;

/// What the screen reads once: the chart, the closing date and the
/// company's default discount account.
#[derive(Clone, Debug, Default)]
pub(crate) struct Setup {
    pub accounts: Vec<Account>,
    pub closing: Option<CalendarDate>,
    pub discount_account: Option<AccountId>,
}

/// The parts of the pay this engine takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // independent capabilities
pub(crate) struct PayCaps {
    /// `bill-discounts-v1`.
    pub discounts: bool,
    /// `bill-payment-check-number-v1`.
    pub numbers: bool,
    /// `ap-closing-v1` (a closing-date password on the pay).
    pub closing: bool,
}

/// Pay Bills' engine when it is not the live one.
pub(crate) trait LocalPayBills: Send {
    fn caps(&self) -> PayCaps;
    fn setup(&mut self, link: &CompanyLink) -> Result<Setup, ApiError>;
    fn payable(&mut self, link: &CompanyLink, f: &PayableFilter) -> Result<PayableList, ApiError>;
    fn balance(&mut self, link: &CompanyLink, account: AccountId) -> Option<Money>;
    fn numbers(&mut self, link: &CompanyLink, account: AccountId) -> (Option<u32>, Option<u32>);
    /// `key`: the pay's idempotency key (the same on an identical retry).
    fn pay(
        &mut self,
        link: &CompanyLink,
        req: &PayBillsRequest,
        password: Option<&Secret>,
        key: &IdempotencyKey,
    ) -> Result<Vec<CreatedPayment>, ApiError>;
    fn set_discount_account(
        &mut self,
        link: &CompanyLink,
        account: AccountId,
    ) -> Result<(), ApiError>;
    fn get(&mut self, link: &CompanyLink, id: TransactionId) -> Result<Viewed, ApiError>;
    fn void(
        &mut self,
        link: &CompanyLink,
        id: TransactionId,
        password: Option<&Secret>,
    ) -> Result<(), ApiError>;
}

/// A bill payment as the read-only view shows it.
#[derive(Clone)]
pub(crate) struct Viewed {
    pub record: BillPaymentRecord,
    /// The bank or card account it was paid from.
    pub account: Option<AccountId>,
}

impl std::fmt::Debug for Viewed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Viewed")
            .field("record", &self.record)
            .finish_non_exhaustive()
    }
}

/// The `--demo` engine.
#[derive(Debug, Default)]
pub(crate) struct DemoPayBills;

fn demo(link: &CompanyLink) -> Result<std::sync::MutexGuard<'_, DemoState>, ApiError> {
    link.demo_state()
        .ok_or(ApiError::Local(LocalValidation::InvalidValue))
}

impl LocalPayBills for DemoPayBills {
    fn caps(&self) -> PayCaps {
        PayCaps {
            discounts: true,
            numbers: true,
            closing: true,
        }
    }

    fn setup(&mut self, link: &CompanyLink) -> Result<Setup, ApiError> {
        let mut s = demo(link)?;
        let accounts = s
            .chart
            .get_or_insert_with(crate::company::demo::chart)
            .clone();
        let closing = s.open.and_then(|open| {
            s.companies
                .iter()
                .find(|c| c.tenant_id == open)
                .and_then(|c| c.closed_through)
        });
        Ok(Setup {
            accounts,
            closing,
            discount_account: s.pay_bills.discount_account,
        })
    }

    fn payable(&mut self, link: &CompanyLink, f: &PayableFilter) -> Result<PayableList, ApiError> {
        let mut s = demo(link)?;
        Ok(crate::company::demo::pay_bills::payable(&mut s, f))
    }

    fn balance(&mut self, link: &CompanyLink, account: AccountId) -> Option<Money> {
        let mut s = demo(link).ok()?;
        crate::company::demo::checks::balance(&mut s, account)
    }

    fn numbers(&mut self, link: &CompanyLink, account: AccountId) -> (Option<u32>, Option<u32>) {
        demo(link).map_or((None, None), |mut s| {
            crate::company::demo::checks::numbers(&mut s, account)
        })
    }

    fn pay(
        &mut self,
        link: &CompanyLink,
        req: &PayBillsRequest,
        password: Option<&Secret>,
        _key: &IdempotencyKey,
    ) -> Result<Vec<CreatedPayment>, ApiError> {
        let mut s = demo(link)?;
        crate::company::demo::pay_bills::pay(&mut s, req, password)
    }

    fn set_discount_account(
        &mut self,
        link: &CompanyLink,
        account: AccountId,
    ) -> Result<(), ApiError> {
        demo(link)?.pay_bills.discount_account = Some(account);
        Ok(())
    }

    fn get(&mut self, link: &CompanyLink, id: TransactionId) -> Result<Viewed, ApiError> {
        let mut s = demo(link)?;
        let record = crate::company::demo::pay_bills::get(&s, id)
            .ok_or(ApiError::NotFound { code: None })?;
        let account = crate::company::demo::pay_bills::account_of(&mut s, id);
        Ok(Viewed { record, account })
    }

    fn void(
        &mut self,
        link: &CompanyLink,
        id: TransactionId,
        password: Option<&Secret>,
    ) -> Result<(), ApiError> {
        let mut s = demo(link)?;
        crate::company::demo::pay_bills::void(&mut s, id, password)
    }
}
