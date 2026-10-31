//! Components that more than one transaction form shares (P13 WP5): the
//! lines grid (register Splits, Write Check Expenses) and the closing-date
//! password prompt. P14 adds [`publish_saved`], the one way a form tells
//! the other windows what its save changed (Q-262), and [`header`], the
//! toolbar and title row every form draws the same way. P15 adds
//! [`template`], the Save as Template and Use hook. P15A lifts Write Check's
//! account suggestions into [`suggest`] for Bulk Entry. P16 adds [`enter`],
//! what Enter does in a header field (Save & New, or the next field, Q-264),
//! and [`condensed`], the compact label · amount table of wide forms (Q-267).

pub mod closing_prompt;
pub mod condensed;
pub mod enter;
pub mod header;
pub mod invoice_lines_grid;
pub mod item_lines_grid;
pub mod lines_grid;
pub mod lost_save;
pub mod nav_prompt;
pub mod suggest;
pub mod template;

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

/// A refused save's problems after the owner's edits: `now` is the form's
/// check as it is now, and only the kinds of problem already shown stay
/// (a fixed one goes away at once; a new one waits for the next save, as
/// in Write Check, P13). Call every frame while `shown` is not empty.
pub fn recheck<P>(shown: &mut Vec<P>, now: Vec<P>) {
    if shown.is_empty() {
        return;
    }
    let kinds: Vec<_> = shown.iter().map(std::mem::discriminant).collect();
    *shown = now
        .into_iter()
        .filter(|p| kinds.contains(&std::mem::discriminant(p)))
        .collect();
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

    #[derive(Debug, PartialEq)]
    enum P {
        A,
        B(u8),
        C,
    }

    #[test]
    fn recheck_drops_fixed_problems_and_keeps_new_ones_for_the_next_save() {
        let mut shown = vec![P::A, P::B(1)];
        recheck(&mut shown, vec![P::B(2), P::C]);
        assert_eq!(shown, vec![P::B(2)], "A fixed, C not shown before");
        recheck(&mut shown, Vec::new());
        assert!(shown.is_empty(), "all fixed");
        recheck(&mut shown, vec![P::A]);
        assert!(shown.is_empty(), "nothing shown, nothing comes back");
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
