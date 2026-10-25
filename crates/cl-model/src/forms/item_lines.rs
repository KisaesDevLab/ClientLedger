//! Item lines of a bill or vendor credit (P17, plan §3 WP3, D5, D6; process
//! review S2).
//!
//! An item line is what the owner typed: an item, a description, a quantity,
//! a cost (the rate) and an amount, plus the account it posts to (D5: items
//! have no purchase account yet, U-39), the customer, billable and one tag.
//!
//! Money rules (plan §4, D6):
//! - With both a quantity and a rate, the amount is `rate × qty` rounded
//!   **half away from zero to cents** ([`item_amount`]); the Amount cell is
//!   then read-only. The engine recomputes the same value and refuses a
//!   mismatch (U-85).
//! - A quantity with a typed amount and no rate is allowed (S2); so is an
//!   amount alone. A rate without a quantity is not.
//! - Quantity: more than zero and below 100,000,000, at most four decimals
//!   (the [`Quantity`] type holds exactly that range). Rate: zero or more,
//!   at most four decimals.
//! - No floats: every figure is a `cl_core` fixed decimal.
//!
//! The golden cases in `fixtures/item-lines/golden.json` are shared with the
//! engine's tests for PR-A, so both sides round the same way.

use std::fmt;

use cl_core::ids::{AccountId, ContactId, ItemId, TagId};
use cl_core::{Money, MoneyError, Quantity};

use crate::rowkey::{RowKey, RowKeyGen};

/// Why a quantity and a rate do not give an item amount.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ItemAmountError {
    /// The quantity is zero or less.
    QuantityNotPositive,
    /// The rate (cost) is below zero.
    RateNegative,
    /// `qty × rate` leaves the money range.
    TooLarge,
}

/// The one item-amount rule (D6): `rate × qty`, rounded half away from zero
/// to cents. `qty` must be more than zero and `rate` zero or more.
///
/// # Errors
/// See [`ItemAmountError`].
pub fn item_amount(qty: Quantity, rate: Money) -> Result<Money, ItemAmountError> {
    if !qty.is_positive() {
        return Err(ItemAmountError::QuantityNotPositive);
    }
    if rate.is_negative() {
        return Err(ItemAmountError::RateNegative);
    }
    // Scale 2 is always accepted, so any error is the money range.
    rate.mul_round(qty, 2)
        .map_err(|_: MoneyError| ItemAmountError::TooLarge)
}

/// One item line. `Debug` is redacted (amounts and descriptions are owner
/// data).
#[derive(Clone, PartialEq, Eq)]
pub struct ItemLine {
    /// Stable row key (never an engine id).
    pub key: RowKey,
    /// The item.
    pub item: Option<ItemId>,
    /// Description (sent as the line memo).
    pub description: String,
    /// Quantity.
    pub qty: Option<Quantity>,
    /// Cost per unit (the rate), up to four decimals.
    pub rate: Option<Money>,
    /// The amount: typed, or `rate × qty` when both are set (kept in step by
    /// [`ItemLine::set_qty`] and [`ItemLine::set_rate`]).
    pub amount: Option<Money>,
    /// The account the line posts to (required, D5).
    pub account: Option<AccountId>,
    /// Customer (or customer job) the line is for.
    pub customer: Option<ContactId>,
    /// Billable to the customer.
    pub billable: bool,
    /// Tag (one per line, Q-233).
    pub tag: Option<TagId>,
}

impl fmt::Debug for ItemLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ItemLine")
            .field("key", &self.key)
            .field("item", &self.item.is_some())
            .field("description", &!self.description.is_empty())
            .field("qty", &self.qty.is_some())
            .field("rate", &self.rate.is_some())
            .field("amount", &self.amount.is_some())
            .field("account", &self.account.is_some())
            .field("customer", &self.customer.is_some())
            .field("billable", &self.billable)
            .field("tag", &self.tag.is_some())
            .finish()
    }
}

impl ItemLine {
    /// A blank line.
    #[must_use]
    pub fn blank(key: RowKey) -> ItemLine {
        ItemLine {
            key,
            item: None,
            description: String::new(),
            qty: None,
            rate: None,
            amount: None,
            account: None,
            customer: None,
            billable: false,
            tag: None,
        }
    }

    /// Nothing entered.
    #[must_use]
    pub fn is_blank(&self) -> bool {
        *self == ItemLine::blank(self.key)
    }

    /// True when the Amount cell is computed (both a quantity and a rate are
    /// set) and therefore read-only.
    #[must_use]
    pub fn amount_is_computed(&self) -> bool {
        self.qty.is_some() && self.rate.is_some()
    }

    /// The line's amount: `rate × qty` (rounded to cents, half away from
    /// zero) when both are set, otherwise the typed amount.
    ///
    /// # Errors
    /// [`ItemAmountError`] when the quantity and rate are both set but do
    /// not give an amount.
    pub fn line_amount(&self) -> Result<Option<Money>, ItemAmountError> {
        match (self.qty, self.rate) {
            (Some(q), Some(r)) => item_amount(q, r).map(Some),
            _ => Ok(self.amount),
        }
    }

    /// Recomputes the stored amount when both a quantity and a rate are set
    /// (an amount that cannot be computed is cleared, so a stale figure is
    /// never shown).
    fn recompute(&mut self) {
        if let (Some(q), Some(r)) = (self.qty, self.rate) {
            self.amount = item_amount(q, r).ok();
        }
    }

    /// The owner typed the quantity.
    pub fn set_qty(&mut self, qty: Option<Quantity>) {
        self.qty = qty;
        self.recompute();
    }

    /// The owner typed the rate (cost).
    pub fn set_rate(&mut self, rate: Option<Money>) {
        self.rate = rate;
        self.recompute();
    }

    /// The owner typed the amount. Ignored (false) while it is computed.
    pub fn set_amount(&mut self, amount: Option<Money>) -> bool {
        if self.amount_is_computed() {
            return false;
        }
        self.amount = amount;
        true
    }
}

/// A form's item lines, normally with one blank entry line last.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ItemLines {
    /// Lines.
    pub lines: Vec<ItemLine>,
}

impl ItemLines {
    /// Lines from `seeds`, each with a fresh key, plus the blank entry line.
    #[must_use]
    pub fn from_lines(seeds: Vec<ItemLine>, keys: &mut RowKeyGen) -> ItemLines {
        let mut v = ItemLines {
            lines: seeds
                .into_iter()
                .map(|l| ItemLine {
                    key: keys.next_key(),
                    ..l
                })
                .collect(),
        };
        v.ensure_entry(keys);
        v
    }

    /// The lines with something on them.
    pub fn used(&self) -> impl Iterator<Item = &ItemLine> {
        self.lines.iter().filter(|l| !l.is_blank())
    }

    /// Mutable access to the lines with something on them.
    pub fn used_mut(&mut self) -> impl Iterator<Item = &mut ItemLine> {
        self.lines.iter_mut().filter(|l| !l.is_blank())
    }

    /// The line with `key`.
    pub fn get_mut(&mut self, key: RowKey) -> Option<&mut ItemLine> {
        self.lines.iter_mut().find(|l| l.key == key)
    }

    /// Exactly one blank entry line at the end: trailing blank lines
    /// collapse to one, and a blank line is added when the last one is used.
    pub fn ensure_entry(&mut self, keys: &mut RowKeyGen) {
        let lines = &mut self.lines;
        while lines.len() >= 2 && lines[lines.len() - 2..].iter().all(ItemLine::is_blank) {
            lines.pop();
        }
        if lines.last().is_none_or(|l| !l.is_blank()) {
            lines.push(ItemLine::blank(keys.next_key()));
        }
    }
}

/// The exact sum of the item lines' amounts (blank amounts count as nothing;
/// no lines is zero). `None` when a line's quantity × rate cannot be
/// computed or the sum leaves the money range.
#[must_use]
pub fn items_total(v: &ItemLines) -> Option<Money> {
    let mut amounts = Vec::with_capacity(v.lines.len());
    for l in &v.lines {
        if let Some(a) = l.line_amount().ok()? {
            amounts.push(a);
        }
    }
    Money::sum(amounts).ok()
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn money(s: &str) -> Money {
        Money::parse_wire(s).expect("money")
    }

    fn qty(s: &str) -> Quantity {
        Quantity::parse_wire(s).expect("qty")
    }

    #[test]
    fn amount_rounds_half_away_from_zero_to_cents() {
        assert_eq!(item_amount(qty("3"), money("0.335")), Ok(money("1.01")));
        assert_eq!(item_amount(qty("1"), money("1.0049")), Ok(money("1.00")));
        assert_eq!(item_amount(qty("0.5"), money("0.01")), Ok(money("0.01")));
        assert_eq!(item_amount(qty("0.4999"), money("0.01")), Ok(money("0")));
        assert_eq!(item_amount(qty("2.5"), money("19.99")), Ok(money("49.98")));
        assert_eq!(item_amount(qty("10"), money("0")), Ok(Money::ZERO));
        assert_eq!(
            item_amount(Quantity::ZERO, money("1")),
            Err(ItemAmountError::QuantityNotPositive)
        );
        assert_eq!(
            item_amount(qty("-1"), money("1")),
            Err(ItemAmountError::QuantityNotPositive)
        );
        assert_eq!(
            item_amount(qty("1"), money("-0.01")),
            Err(ItemAmountError::RateNegative)
        );
        assert_eq!(
            item_amount(Quantity::MAX, Money::MAX),
            Err(ItemAmountError::TooLarge)
        );
    }

    #[test]
    fn setters_keep_a_computed_amount_in_step() {
        let mut l = ItemLine::blank(RowKey(1));
        assert!(l.set_amount(Some(money("5"))));
        l.set_qty(Some(qty("2")));
        assert_eq!(
            l.amount,
            Some(money("5")),
            "qty + amount without a rate (S2)"
        );
        assert!(!l.amount_is_computed());
        l.set_rate(Some(money("665")));
        assert!(l.amount_is_computed());
        assert_eq!(l.amount, Some(money("1330")));
        assert!(!l.set_amount(Some(money("1"))), "read-only while computed");
        assert_eq!(l.line_amount(), Ok(Some(money("1330"))));
        l.set_rate(Some(Money::MAX));
        l.set_qty(Some(Quantity::MAX));
        assert_eq!(
            l.amount, None,
            "an amount that can't be computed is cleared"
        );
        assert_eq!(l.line_amount(), Err(ItemAmountError::TooLarge));
        l.set_rate(None);
        assert!(l.set_amount(Some(money("7"))));
        assert_eq!(l.line_amount(), Ok(Some(money("7"))));
    }

    #[test]
    fn items_total_is_exact_and_entry_line_is_single() {
        let mut keys = RowKeyGen::new();
        let mut a = ItemLine::blank(RowKey(0));
        a.set_qty(Some(qty("3")));
        a.set_rate(Some(money("0.335")));
        let mut b = ItemLine::blank(RowKey(0));
        b.set_amount(Some(money("150")));
        let mut v = ItemLines::from_lines(vec![a, b], &mut keys);
        assert_eq!(v.lines.len(), 3);
        assert_eq!(v.used().count(), 2);
        assert_eq!(items_total(&v), Some(money("151.01")));
        assert_eq!(items_total(&ItemLines::default()), Some(Money::ZERO));
        v.lines.push(ItemLine::blank(keys.next_key()));
        v.ensure_entry(&mut keys);
        assert_eq!(v.lines.len(), 3, "trailing blanks collapse");
        let k = v.lines[0].key;
        if let Some(l) = v.get_mut(k) {
            l.qty = Some(Quantity::MAX);
            l.rate = Some(Money::MAX);
        }
        assert_eq!(items_total(&v), None);
        for l in v.used_mut() {
            l.billable = true;
        }
        assert!(v.lines[1].billable);
    }

    #[test]
    fn debug_hides_owner_data() {
        let mut l = ItemLine::blank(RowKey(1));
        l.description = "secret part".into();
        l.set_amount(Some(money("1286.4")));
        let s = format!("{l:?}");
        assert!(!s.contains("1286") && !s.contains("secret"), "{s}");
    }

    #[derive(serde::Deserialize)]
    struct Golden {
        cases: Vec<GoldenCase>,
    }

    #[derive(serde::Deserialize)]
    struct GoldenCase {
        name: String,
        quantity: Option<String>,
        #[serde(rename = "unitPrice")]
        unit_price: Option<String>,
        amount: Option<String>,
        error: Option<String>,
    }

    /// The client's rules applied to one golden case, as the engine reason
    /// codes.
    fn golden_rule(case: &GoldenCase) -> Result<Money, &'static str> {
        let quantity = match case.quantity.as_deref().map(Quantity::parse_wire) {
            None => None,
            Some(Ok(q)) => Some(q),
            Some(Err(MoneyError::TooPrecise)) => return Err("quantity_too_precise"),
            Some(Err(_)) => return Err("quantity_out_of_range"),
        };
        let rate = match case.unit_price.as_deref().map(Money::parse_wire) {
            None => None,
            Some(Ok(r)) => Some(r),
            Some(Err(MoneyError::TooPrecise)) => return Err("rate_too_precise"),
            Some(Err(_)) => return Err("rate_out_of_range"),
        };
        let typed = case
            .amount
            .as_deref()
            .map(|a| Money::parse_wire(a).expect("amount"));
        if quantity.is_some_and(|q| !q.is_positive()) {
            return Err("quantity_not_positive");
        }
        if quantity.is_none() && rate.is_some() {
            return Err("quantity_required");
        }
        let mut line = ItemLine::blank(RowKey(1));
        line.set_qty(quantity);
        line.set_rate(rate);
        line.set_amount(typed);
        match line.line_amount() {
            Ok(Some(amount)) => Ok(amount),
            Ok(None) => Err("amount_missing"),
            Err(ItemAmountError::QuantityNotPositive) => Err("quantity_not_positive"),
            Err(ItemAmountError::RateNegative) => Err("rate_negative"),
            Err(ItemAmountError::TooLarge) => Err("amount_out_of_range"),
        }
    }

    #[test]
    fn golden_cases_shared_with_the_engine() {
        let raw = include_str!("../../../../fixtures/item-lines/golden.json");
        let golden: Golden = serde_json::from_str(raw).expect("golden json");
        assert!(golden.cases.len() >= 20);
        for c in &golden.cases {
            let got = golden_rule(c);
            match (&c.amount, &c.error) {
                (_, Some(e)) => assert_eq!(got, Err(e.as_str()), "{}", c.name),
                (Some(a), None) => {
                    assert_eq!(got, Ok(money(a)), "{}", c.name);
                    // Two decimals in the fixture: the amount is in cents.
                    assert_eq!(
                        got.map(|m| m.to_ten_thousandths() % 100),
                        Ok(0),
                        "{}",
                        c.name
                    );
                }
                (None, None) => panic!("case {} has neither amount nor error", c.name),
            }
        }
    }

    /// The reference rule, written independently on integers: ten-thousandth
    /// units in, cents out, half away from zero.
    fn reference_cents(q_units: i128, r_units: i128) -> i128 {
        let prod = q_units * r_units; // 8 decimals
        let div = 1_000_000; // to 2 decimals
        let (q, r) = (prod / div, prod % div);
        if r * 2 >= div { q + 1 } else { q }
    }

    proptest! {
        /// Proptest (1): the item amount equals `mul_round` (and an
        /// independent integer rule), and the lines total is the exact sum.
        #[test]
        fn item_amount_and_total_are_exact(
            rows in proptest::collection::vec(
                (1_i128..=999_999_999_999, 0_i128..=9_999_999_999, any::<bool>()),
                0..12,
            )
        ) {
            let mut keys = RowKeyGen::new();
            let mut seeds = Vec::new();
            let mut want: i128 = 0;
            for (q, r, computed) in &rows {
                let qv = Quantity::from_ten_thousandths(*q).expect("q");
                let rv = Money::from_ten_thousandths(*r).expect("r");
                let mut l = ItemLine::blank(RowKey(0));
                l.description = "x".into();
                if *computed {
                    l.set_qty(Some(qv));
                    l.set_rate(Some(rv));
                    let got = l.line_amount().expect("fits").expect("set");
                    prop_assert_eq!(got, rv.mul_round(qv, 2).expect("fits"));
                    prop_assert_eq!(got.to_ten_thousandths(), reference_cents(*q, *r) * 100);
                    want += got.to_ten_thousandths();
                } else {
                    l.set_qty(Some(qv));
                    l.set_amount(Some(rv));
                    want += r;
                }
                seeds.push(l);
            }
            let v = ItemLines::from_lines(seeds, &mut keys);
            // Every amount is zero or more, so the sum leaves the money
            // range exactly when the exact total does.
            prop_assert_eq!(items_total(&v), Money::from_ten_thousandths(want).ok());
        }
    }
}
