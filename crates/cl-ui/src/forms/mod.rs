//! Components that more than one transaction form shares (P13 WP5): the
//! lines grid (register Splits, Write Check Expenses) and the closing-date
//! password prompt. P14 adds [`publish_saved`], the one way a form tells
//! the other windows what its save changed (Q-262).

pub mod closing_prompt;
pub mod lines_grid;
pub mod nav_prompt;

use std::collections::BTreeSet;

use cl_core::ids::AccountId;

use crate::company::CompanyLink;
use crate::data_bus::{Change, Subscription};

/// Tells the other windows that a save, void or delete `from` this form
/// changed transactions in `accounts` (Q-262). Open registers of those
/// accounts, the Chart of Accounts, Make Deposit's waiting payments and the
/// forms' balance panels read them again from the engine; the form itself
/// does not hear its own notice. Repeats are dropped; an empty list means
/// any account may have changed. Returns the notice's generation.
pub fn publish_saved(link: &CompanyLink, from: &Subscription, accounts: &[AccountId]) -> u64 {
    let unique: BTreeSet<AccountId> = accounts.iter().copied().collect();
    link.bus().publish_from(from, Change::ledger(unique))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data_bus::LedgerScope;

    fn account(n: u32) -> AccountId {
        crate::company::demo::account_id(n)
    }

    #[test]
    fn a_save_reaches_every_other_window_once_per_account() {
        let dir = tempfile::tempdir().expect("tmp");
        let link = CompanyLink::demo(dir.path());
        let form = link.bus().subscribe();
        let mut register = link.bus().subscribe();
        let mut form_view = link.bus().subscribe();
        // A grouped deposit: bank, Payments Clearing, a fee account.
        publish_saved(
            &link,
            &form,
            &[account(1010), account(10150), account(6250), account(1010)],
        );
        let heard = link.bus().take(&mut register);
        assert_eq!(
            heard.ledger,
            LedgerScope::Accounts([account(1010), account(10150), account(6250)].into())
        );
        assert!(!heard.ledger_touches(account(2050)));
        assert!(!heard.lists(), "a save is not a list change");
        // Nobody else hears it twice; an empty list reaches every register.
        assert!(link.bus().take(&mut register).is_empty());
        publish_saved(&link, &form, &[]);
        assert_eq!(link.bus().take(&mut form_view).ledger, LedgerScope::All);
    }

    #[test]
    fn the_saving_form_does_not_hear_itself() {
        let dir = tempfile::tempdir().expect("tmp");
        let link = CompanyLink::demo(dir.path());
        let mut form = link.bus().subscribe();
        let before = link.bus().generation();
        let g = publish_saved(&link, &form, &[account(1010)]);
        assert_eq!(g, before + 1);
        assert!(link.bus().take(&mut form).is_empty());
    }
}
