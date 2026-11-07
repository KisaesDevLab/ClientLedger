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

use cl_core::ids::{AccountId, ContactId, ItemId, TagId, UnitId};
use cl_core::{Money, MoneyError, Quantity};

use crate::rowkey::{RowKey, RowKeyGen};
use crate::text::TextKey;

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

/// QA2-10 (PR-4): an item's purchase cost per base unit in a unit holding
/// `factor` base units: `round4(cost × factor)`, half away from zero (the
/// rule the engine uses for a unit's sale price). `None` when it leaves
/// the money range. Exact integer arithmetic, no floats.
#[must_use]
pub fn cost_in_unit(cost: Money, factor: cl_core::UnitFactor) -> Option<Money> {
    // Ten-thousandths × millionths: scale 10, back to scale 4.
    let prod = cost
        .to_ten_thousandths()
        .checked_mul(factor.to_millionths())?;
    let div: i128 = 1_000_000;
    let mut q = prod / div;
    if (prod % div).unsigned_abs() * 2 >= div.unsigned_abs() {
        q += prod.signum();
    }
    Money::from_ten_thousandths(q).ok()
}

/// QA2-17 (PR-4): the cost per unit a typed amount implies for `qty`
/// (`amount ÷ qty`), only when it is exact to four decimals and `qty ×
/// cost` gives the amount back to the cent; otherwise `None` and the
/// cost stays blank, so the owner's amount never changes.
#[must_use]
pub fn rate_from_amount(qty: Quantity, amount: Money) -> Option<Money> {
    if !qty.is_positive() || amount.is_negative() {
        return None;
    }
    let num = amount.to_ten_thousandths().checked_mul(10_000)?;
    let q = qty.to_ten_thousandths();
    if num % q != 0 {
        return None;
    }
    let rate = Money::from_ten_thousandths(num / q).ok()?;
    (item_amount(qty, rate).ok()? == amount).then_some(rate)
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
    /// Quantity (in `unit`).
    pub qty: Option<Quantity>,
    /// The unit of the item's unit set the quantity and cost are in
    /// (`None`: the base unit; U-161).
    pub unit: Option<UnitId>,
    /// Cost per unit (the rate), up to four decimals.
    pub rate: Option<Money>,
    /// The amount: typed, or `rate × qty` when both are set (kept in step by
    /// [`ItemLine::set_qty`] and [`ItemLine::set_rate`]).
    pub amount: Option<Money>,
    /// The account the line posts to (required, D5).
    pub account: Option<AccountId>,
    /// Customer (or customer project) the line is for.
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
            .field("unit", &self.unit)
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
            unit: None,
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

    /// QA2-17: a line with a quantity and a typed amount but no cost
    /// shows the cost the amount implies, when [`rate_from_amount`] finds
    /// an exact one (the amount stays exactly as typed).
    pub fn fill_rate_from_amount(&mut self) {
        if self.rate.is_some() {
            return;
        }
        if let (Some(q), Some(a)) = (self.qty, self.amount)
            && let Some(r) = rate_from_amount(q, a)
        {
            self.set_rate(Some(r));
        }
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

/// One checked item line. `Debug` is redacted.
#[derive(Clone, PartialEq, Eq)]
pub struct CheckedItemLine {
    /// The item.
    pub item: Option<ItemId>,
    /// Description, when typed (trimmed).
    pub description: Option<String>,
    /// Quantity (more than zero).
    pub qty: Option<Quantity>,
    /// The unit (`None`: the base unit).
    pub unit: Option<UnitId>,
    /// Rate (zero or more); set only together with `qty`.
    pub rate: Option<Money>,
    /// Amount, more than zero: `rate × qty` rounded to cents when both are
    /// set, otherwise as typed.
    pub amount: Money,
    /// Account.
    pub account: AccountId,
    /// Customer.
    pub contact: Option<ContactId>,
    /// Billable.
    pub billable: bool,
    /// Tag.
    pub tag: Option<TagId>,
}

impl fmt::Debug for CheckedItemLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CheckedItemLine")
            .field("item", &self.item.is_some())
            .field("qty", &self.qty.is_some())
            .field("billable", &self.billable)
            .finish_non_exhaustive()
    }
}

/// Why one item line of a check or card charge can't be posted (the bill's
/// item rules, D6; U-158 on checks and card charges).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ItemIssue {
    /// A cost without a quantity.
    RateNeedsQty,
    /// The quantity is zero or less.
    QtyNotPositive,
    /// The cost is below zero.
    RateNegative,
    /// `qty × cost` leaves the money range.
    AmountTooLarge,
    /// The amount is blank, zero or less.
    AmountNotPositive,
    /// No account.
    AccountMissing,
    /// Billable without a customer.
    BillableNeedsCustomer,
}

impl ItemIssue {
    /// Plain-language message (strings live in `cl-ui::i18n`).
    #[must_use]
    pub const fn key(self) -> TextKey {
        match self {
            ItemIssue::RateNeedsQty => TextKey("bill.problem.item_rate_needs_qty"),
            ItemIssue::QtyNotPositive => TextKey("bill.problem.item_qty_not_positive"),
            ItemIssue::RateNegative => TextKey("bill.problem.item_rate_negative"),
            ItemIssue::AmountTooLarge => TextKey("bill.problem.item_amount_too_large"),
            ItemIssue::AmountNotPositive => TextKey("check.problem.line_amount_not_positive"),
            ItemIssue::AccountMissing => TextKey("check.problem.line_account_missing"),
            ItemIssue::BillableNeedsCustomer => TextKey("check.problem.billable_needs_customer"),
        }
    }

    /// The control channel's name (no amounts).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            ItemIssue::RateNeedsQty => "item_rate_needs_qty",
            ItemIssue::QtyNotPositive => "item_qty_not_positive",
            ItemIssue::RateNegative => "item_rate_negative",
            ItemIssue::AmountTooLarge => "item_amount_too_large",
            ItemIssue::AmountNotPositive => "line_amount_not_positive",
            ItemIssue::AccountMissing => "line_account_missing",
            ItemIssue::BillableNeedsCustomer => "billable_needs_customer",
        }
    }
}

/// The used item lines checked (each with the tag `tag_of` gives it), and
/// every issue in line order. `billable` is whether billable lines are
/// allowed (not on a card credit).
#[must_use]
pub fn check_item_lines(
    lines: &ItemLines,
    billable: bool,
    tag_of: impl Fn(Option<TagId>) -> Option<TagId>,
) -> (Vec<CheckedItemLine>, Vec<(RowKey, ItemIssue)>) {
    let mut out = Vec::new();
    let mut issues = Vec::new();
    for l in lines.used() {
        let k = l.key;
        let amount = match (l.qty, l.rate) {
            (None, Some(_)) => Err(ItemIssue::RateNeedsQty),
            (Some(q), _) if !q.is_positive() => Err(ItemIssue::QtyNotPositive),
            _ => match l.line_amount() {
                Ok(Some(a)) if a.is_positive() => Ok(a),
                Ok(_) => Err(ItemIssue::AmountNotPositive),
                Err(ItemAmountError::QuantityNotPositive) => Err(ItemIssue::QtyNotPositive),
                Err(ItemAmountError::RateNegative) => Err(ItemIssue::RateNegative),
                Err(ItemAmountError::TooLarge) => Err(ItemIssue::AmountTooLarge),
            },
        };
        let amount = match amount {
            Ok(a) => Some(a),
            Err(i) => {
                issues.push((k, i));
                None
            }
        };
        if l.account.is_none() {
            issues.push((k, ItemIssue::AccountMissing));
        }
        let billable = l.billable && billable;
        if billable && l.customer.is_none() {
            issues.push((k, ItemIssue::BillableNeedsCustomer));
        }
        if let (Some(account), Some(amount)) = (l.account, amount) {
            out.push(CheckedItemLine {
                item: l.item,
                description: crate::forms::lines::trimmed(&l.description),
                qty: l.qty,
                unit: l.unit.filter(|_| l.item.is_some()),
                rate: l.rate.filter(|_| l.qty.is_some()),
                amount,
                account,
                contact: l.customer,
                billable,
                tag: tag_of(l.tag),
            });
        }
    }
    (out, issues)
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

    fn factor(s: &str) -> cl_core::UnitFactor {
        cl_core::UnitFactor::parse_wire(s).expect("factor")
    }

    /// QA2-10: 1.10 per each, a box of 12 is 13.20; round4, half away.
    #[test]
    fn qa2_10_cost_in_a_unit_is_round4_of_cost_times_factor() {
        assert_eq!(
            cost_in_unit(money("1.10"), factor("12")),
            Some(money("13.20"))
        );
        assert_eq!(
            cost_in_unit(money("1.10"), factor("1")),
            Some(money("1.10"))
        );
        // 0.3333 × 1.5 = 0.49995 → 0.5000 (half away from zero).
        assert_eq!(
            cost_in_unit(money("0.3333"), factor("1.5")),
            Some(money("0.5"))
        );
        // 2.0001 × 1.333333 = 2.66679999… → 2.6668.
        assert_eq!(
            cost_in_unit(money("2.0001"), factor("1.333333")),
            Some(money("2.6668"))
        );
        assert_eq!(cost_in_unit(Money::ZERO, factor("12")), Some(Money::ZERO));
        assert_eq!(cost_in_unit(Money::MAX, factor("1000000")), None);
    }

    /// QA2-17: 1 box for 12.00 costs 12.00; a cost that isn't exact stays
    /// blank and the typed amount never changes.
    #[test]
    fn qa2_17_cost_from_a_typed_amount_only_when_exact() {
        assert_eq!(
            rate_from_amount(qty("1"), money("12.00")),
            Some(money("12"))
        );
        assert_eq!(
            rate_from_amount(qty("5"), money("66.00")),
            Some(money("13.2"))
        );
        assert_eq!(
            rate_from_amount(qty("8"), money("1.00")),
            Some(money("0.125"))
        );
        // 10.00 ÷ 3 = 3.3333… is not exact: blank.
        assert_eq!(rate_from_amount(qty("3"), money("10.00")), None);
        // 1.00 ÷ 0.0003 = 3333.33… is not exact either.
        assert_eq!(rate_from_amount(qty("0.0003"), money("1.00")), None);
        assert_eq!(rate_from_amount(Quantity::ZERO, money("1")), None);
        assert_eq!(rate_from_amount(qty("2"), money("-4")), None);

        let mut l = ItemLine::blank(crate::rowkey::RowKeyGen::new().next_key());
        l.set_qty(Some(qty("1")));
        assert!(l.set_amount(Some(money("12.00"))));
        l.fill_rate_from_amount();
        assert_eq!(l.rate, Some(money("12")));
        assert_eq!(l.amount, Some(money("12.00")), "the amount as typed");
        let mut odd = ItemLine::blank(crate::rowkey::RowKeyGen::new().next_key());
        odd.set_qty(Some(qty("3")));
        assert!(odd.set_amount(Some(money("10.00"))));
        odd.fill_rate_from_amount();
        assert_eq!(odd.rate, None);
        assert_eq!(odd.amount, Some(money("10.00")));
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
