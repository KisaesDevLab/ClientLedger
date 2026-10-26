//! Card account pick (WP-A, step 11; plan D3 "Card account", M4, Q-A03q).
//! Runs in the desktop against the company's card accounts with
//! `accounts.card_last4` (U-92). None, or two or more → no account and a
//! `PickTheCard` check.
//!
//! **Consumer:** the desktop reading service (WP-G); the signature of
//! [`card_account`] is fixed.

use crate::model::Last4;
use cl_core::ids::AccountId;

/// The outcome of matching printed card digits to card accounts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardPick {
    /// Exactly one card account ends in the digits.
    One(AccountId),
    /// No card account ends in the digits.
    None,
    /// Several do: the owner picks.
    Several,
}

/// Pick the card account for `last4`. Stub (step 2): always
/// [`CardPick::None`].
#[must_use]
pub fn card_account(last4: Last4, accounts: &[(AccountId, Last4)]) -> CardPick {
    let _ = (last4, accounts);
    CardPick::None
}
