//! The engine's account types as the list rules see them.

use cl_api::{Account, AccountType};
use cl_model::lists::accounts::AccountKind;
use cl_model::register::AccountClass;

/// The register class of an account (`None` for income and expense
/// accounts, which have no register).
#[must_use]
pub fn class_of(a: &Account) -> Option<AccountClass> {
    let c = AccountClass::of(
        kind_of(a.fields.account_type),
        a.fields.detail_type.as_deref(),
    );
    c.has_register().then_some(c)
}

/// The list kind of an engine type.
#[must_use]
pub fn kind_of(t: AccountType) -> AccountKind {
    match t {
        AccountType::Asset => AccountKind::Asset,
        AccountType::Liability => AccountKind::Liability,
        AccountType::Equity => AccountKind::Equity,
        AccountType::Revenue => AccountKind::Income,
        AccountType::Cogs => AccountKind::Cogs,
        AccountType::Expense => AccountKind::Expense,
        AccountType::OtherRevenue => AccountKind::OtherIncome,
        AccountType::OtherExpense => AccountKind::OtherExpense,
    }
}
