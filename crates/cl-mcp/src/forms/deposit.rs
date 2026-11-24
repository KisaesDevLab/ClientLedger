//! Make Deposits (P15): a deposit, checked by
//! `cl_model::forms::deposit::check`, as the engine's draft, and the
//! control accounts its check runs against.
//!
//! Money (P15 plan §4): `check()` is the exact gate; [`to_draft`] copies
//! the checked values onto the engine draft one field at a time, with no
//! arithmetic; `BankPosting::deposit_with` refuses once more before
//! anything leaves.

use cl_api::{
    Account, ApiError, CashBack, DepositDraft, DepositSource, GroupedPayment, LocalValidation,
    SplitLine,
};
use cl_core::ids::AccountId;
use cl_model::forms::deposit::{CheckedDeposit, DepositForm};
use cl_model::forms::lines::PostingLine;
use cl_model::forms::tag_rule::TagRefusal;
use cl_model::lists::tags::TagList;
use cl_model::register::AccountClass;

use super::accounts::class_of;

fn split(l: &PostingLine) -> SplitLine {
    SplitLine {
        account: l.account,
        amount: l.amount,
        memo: l.memo.clone(),
        contact: l.contact,
        billable: false,
        tag: l.tag,
        item: None,
    }
}

/// The checked deposit as the engine's draft, field by field (no
/// arithmetic): each picked payment is a whole source line on Payments
/// Clearing at the engine's amount, the lines and fees keep the owner's
/// typed amounts (fees positive; cl-api sends them negative).
///
/// # Errors
/// `Local(InvalidValue)` when payments are picked and the tenant has no
/// Payments Clearing account.
pub fn to_draft(c: &CheckedDeposit, clearing: Option<AccountId>) -> Result<DepositDraft, ApiError> {
    let payments = if c.payments.is_empty() {
        Vec::new()
    } else {
        let clearing = clearing.ok_or(ApiError::Local(LocalValidation::InvalidValue))?;
        c.payments
            .iter()
            .map(|p| GroupedPayment {
                source: p.source,
                kind: DepositSource::Payment,
                account: clearing,
                amount: p.amount,
                contact: p.contact,
                memo: None,
            })
            .collect()
    };
    Ok(DepositDraft {
        account: c.account,
        date: c.date,
        memo: c.memo.clone(),
        payments,
        lines: c.lines.iter().map(split).collect(),
        deductions: c.deductions.iter().map(split).collect(),
        cash_back: c.cash_back.as_ref().map(|b| CashBack {
            account: b.account,
            amount: b.amount,
            memo: b.memo.clone(),
        }),
    })
}

/// The accounts a deposit touches (Q-262): the bank, Payments Clearing
/// when payments were taken, every line, fee and cash-back account.
#[must_use]
pub fn touched(d: &DepositDraft) -> Vec<AccountId> {
    let mut v = vec![d.account];
    v.extend(d.payments.iter().map(|p| p.account));
    v.extend(d.lines.iter().chain(&d.deductions).map(|l| l.account));
    v.extend(d.cash_back.as_ref().map(|c| c.account));
    v.sort();
    v.dedup();
    v
}

/// The tenant's Payments Clearing account.
#[must_use]
pub fn clearing(accounts: &[Account]) -> Option<AccountId> {
    accounts
        .iter()
        .find(|a| a.system_tag.as_deref() == Some("payments_clearing"))
        .map(|a| a.id)
}

/// The tenant's Accounts Payable account.
#[must_use]
pub fn payables(accounts: &[Account]) -> Option<AccountId> {
    accounts
        .iter()
        .find(|a| {
            a.system_tag.as_deref() == Some("accounts_payable")
                || class_of(a) == Some(AccountClass::Payable)
        })
        .map(|a| a.id)
}

/// Payments Clearing, A/R, A/P and Sales Tax Payable: cash back can't go
/// there (PR #159).
#[must_use]
pub fn control_accounts(accounts: &[Account]) -> Vec<AccountId> {
    accounts
        .iter()
        .filter(|a| {
            matches!(
                a.system_tag.as_deref(),
                Some(
                    "payments_clearing"
                        | "accounts_receivable"
                        | "accounts_payable"
                        | "sales_tax_payable"
                )
            ) || matches!(
                class_of(a),
                Some(AccountClass::Receivable | AccountClass::Payable)
            )
        })
        .map(|a| a.id)
        .collect()
}

/// The tag rules on the deposit's lines (P29 WP7, Q-276). A deposit has no
/// header tag.
///
/// # Errors
/// The refused lines (nothing is sent).
pub fn tag_gate(
    form: &DepositForm,
    list: &TagList,
    accounts: &[Account],
) -> Result<(), TagRefusal> {
    let lines = cl_model::forms::tag_rule::expense_lines(&[], &form.lines, list, false);
    super::tag_gate::check(list, &lines, accounts)
}
