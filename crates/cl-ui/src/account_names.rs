//! The account names owners see (Q-202).
//!
//! The engine seeds the account that holds received-but-not-deposited
//! payments as "Payments Clearing" (`system_tag payments_clearing`). Owners
//! know it as **Undeposited Funds**, and the forms call it that, so every
//! place that shows an account name shows it that way too. The account is
//! found by its system tag, never by its name; the engine's data is not
//! changed. An owner who renamed the account in the chart sees their name.

use cl_api::Account;

use crate::i18n::t;

/// The engine's role tag for the payments-waiting-for-deposit account.
pub const PAYMENTS_CLEARING_TAG: &str = "payments_clearing";

/// The name the engine seeds that account with (its chart templates).
const ENGINE_NAME: &str = "Payments Clearing";

/// True when `a` still carries the engine's own name for its role, so the
/// owner-facing name replaces it.
fn shows_alias(a: &Account) -> bool {
    a.system_tag.as_deref() == Some(PAYMENTS_CLEARING_TAG) && a.fields.name.trim() == ENGINE_NAME
}

/// The account's name as owners see it.
#[must_use]
pub fn shown_name(a: &Account) -> String {
    if shows_alias(a) {
        t("account.undeposited_funds")
    } else {
        a.fields.name.clone()
    }
}

/// The name to send when the owner saves `a` with `typed` in the name
/// field: the stored name when they left the shown name as it was, so
/// opening and saving the account never renames it in the books.
#[must_use]
pub fn stored_name(a: &Account, typed: &str) -> String {
    if shows_alias(a) && typed == shown_name(a) {
        a.fields.name.clone()
    } else {
        typed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cl_api::{AccountFields, AccountType};
    use cl_core::ids::AccountId;

    fn account(name: &str, tag: Option<&str>) -> Account {
        Account {
            id: AccountId::parse("d0000000-0000-4000-8000-a00000010150").expect("id"),
            fields: AccountFields {
                name: name.to_string(),
                account_number: Some("10150".to_string()),
                account_type: AccountType::Asset,
                detail_type: None,
                description: None,
                parent_id: None,
            },
            is_active: true,
            is_system: tag.is_some(),
            system_tag: tag.map(str::to_string),
            balance: None,
            created_at: None,
            updated_at: None,
            card_last4: None,
        }
    }

    #[test]
    fn payments_clearing_shows_as_undeposited_funds() {
        let a = account("Payments Clearing", Some(PAYMENTS_CLEARING_TAG));
        assert_eq!(shown_name(&a), "Undeposited Funds");
        // Saving it unchanged keeps the engine's name.
        assert_eq!(stored_name(&a, "Undeposited Funds"), "Payments Clearing");
        assert_eq!(stored_name(&a, "Deposits waiting"), "Deposits waiting");
    }

    #[test]
    fn found_by_tag_never_by_name() {
        // Another account that happens to have the name keeps it.
        let other = account("Payments Clearing", None);
        assert_eq!(shown_name(&other), "Payments Clearing");
        assert_eq!(
            stored_name(&other, "Undeposited Funds"),
            "Undeposited Funds"
        );
        // The owner renamed the tagged account: their name is shown.
        let renamed = account("Customer payments", Some(PAYMENTS_CLEARING_TAG));
        assert_eq!(shown_name(&renamed), "Customer payments");
        assert_eq!(
            stored_name(&renamed, "Customer payments"),
            "Customer payments"
        );
    }
}
