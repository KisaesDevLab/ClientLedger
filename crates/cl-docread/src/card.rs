//! Card account pick (WP-A, step 11; plan D3 "Card account", M4, Q-A03q).
//! Runs in the desktop against the company's card accounts with
//! `accounts.card_last4` (U-92). None, or two or more → no account and a
//! `PickTheCard` check.
//!
//! **Consumer:** the desktop reading service (WP-G); the signature of
//! [`card_account`] is fixed.

use crate::model::{Check, CheckCode, FieldName, Last4, Reading, RuleId};
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

/// Pick the card account for `last4`. The same account listed twice
/// counts once.
#[must_use]
pub fn card_account(last4: Last4, accounts: &[(AccountId, Last4)]) -> CardPick {
    let mut found: Option<AccountId> = None;
    for (id, digits) in accounts {
        if *digits != last4 {
            continue;
        }
        match found {
            None => found = Some(*id),
            Some(f) if f == *id => {}
            Some(_) => return CardPick::Several,
        }
    }
    found.map_or(CardPick::None, CardPick::One)
}

/// The `PickTheCard` check for a reading whose card digits matched no
/// account or several, pointing at the digits. `None` when the pick is
/// [`CardPick::One`] or the reading has no card digits.
#[must_use]
pub fn pick_the_card_check(reading: &Reading, pick: CardPick) -> Option<Check> {
    let payment = reading.header.payment.as_ref()?;
    if !matches!(payment.value, crate::model::Payment::Card { .. })
        || matches!(pick, CardPick::One(_))
    {
        return None;
    }
    Some(Check {
        code: CheckCode::PickTheCard,
        rule: RuleId::new("card.pick"),
        field: Some(FieldName::Payment),
        line: None,
        expected: None,
        found: None,
        difference: None,
        spot: Some(payment.spot),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Bbox, Confidence, Found, Payment, Reader, Spot};
    use proptest::prelude::*;

    fn id(n: u8) -> AccountId {
        AccountId::parse(&format!("00000000-0000-4000-8000-0000000000{n:02x}")).unwrap()
    }

    fn l4(s: &str) -> Last4 {
        Last4::parse(s).unwrap()
    }

    #[test]
    fn none_one_several() {
        let accounts = [
            (id(1), l4("4821")),
            (id(2), l4("1234")),
            (id(3), l4("1234")),
        ];
        assert_eq!(card_account(l4("4821"), &accounts), CardPick::One(id(1)));
        assert_eq!(card_account(l4("9999"), &accounts), CardPick::None);
        assert_eq!(card_account(l4("1234"), &accounts), CardPick::Several);
        assert_eq!(card_account(l4("1234"), &[]), CardPick::None);
        let dup = [(id(1), l4("4821")), (id(1), l4("4821"))];
        assert_eq!(card_account(l4("4821"), &dup), CardPick::One(id(1)));
    }

    #[test]
    fn check_only_when_the_owner_must_pick() {
        let mut r = Reading::empty(Reader::TextPdf, 1);
        assert!(
            pick_the_card_check(&r, CardPick::None).is_none(),
            "no card digits"
        );
        let spot = Spot {
            page: 0,
            bbox: Bbox::PAGE,
        };
        r.header.payment = Some(Found {
            value: Payment::Card { last4: l4("4821") },
            spot,
            rule: RuleId::new("t"),
            confidence: Confidence::HIGH,
        });
        assert!(pick_the_card_check(&r, CardPick::One(id(1))).is_none());
        let c = pick_the_card_check(&r, CardPick::Several).unwrap();
        assert_eq!((c.code, c.spot), (CheckCode::PickTheCard, Some(spot)));
        assert!(pick_the_card_check(&r, CardPick::None).is_some());
    }

    proptest! {
        #[test]
        fn one_exactly_when_one_distinct_account_matches(
            accounts in prop::collection::vec((0_u8..6, 0_u16..4), 0..10),
            want in 0_u16..4,
        ) {
            let list: Vec<(AccountId, Last4)> = accounts
                .iter()
                .map(|(a, d)| (id(*a), l4(&format!("{d:04}"))))
                .collect();
            let mut ids: Vec<u8> = accounts.iter().filter(|(_, d)| *d == want).map(|(a, _)| *a).collect();
            ids.sort_unstable();
            ids.dedup();
            let pick = card_account(l4(&format!("{want:04}")), &list);
            let expected = match ids.as_slice() {
                [] => CardPick::None,
                [one] => CardPick::One(id(*one)),
                _ => CardPick::Several,
            };
            prop_assert_eq!(pick, expected);
        }
    }
}
