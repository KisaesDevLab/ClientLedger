//! Item rules (P11 step 4): the item kinds the owner may pick, the order
//! and filter of the Item List, and the add/edit form's checks.
//!
//! The engine keeps one kind of item today: a service with a name, a
//! description, a sales price, the income account a sale posts to and a
//! taxable mark. Non-inventory parts, other charges, discounts (percent or
//! amount), subtotals, the purchase side of two-sided items (Q-99) and the
//! unit-of-measure label (Q-96) wait for U-39; price levels (Q-58) wait for
//! U-40. Until then [`ItemKind::available`] offers services only.
//!
//! Prices are exact [`Money`] parsed from what the owner typed (up to four
//! decimals, as the engine keeps them); never a float.

use std::cmp::Ordering;

use cl_core::Money;
use cl_core::ids::{AccountId, ItemId};

use crate::amount_entry::{self, Amount, AmountKind, AmountRules, Sign};
use crate::lists::engine_len;
use crate::text::TextKey;

/// Longest item name the engine takes.
pub const NAME_MAX: usize = 255;
/// Longest description kept (the engine's import limit).
pub const DESCRIPTION_MAX: usize = 2000;

/// How a price is typed: money, up to four decimals (a per-unit price such
/// as 0.0450), never negative.
pub const PRICE_RULES: AmountRules = AmountRules {
    kind: AmountKind::Money,
    input: cl_core::UserInput::FULL,
    sign: Sign::NonNegative,
};

/// The kinds of item of the Item List (DESIGN.md lists; Q-58, Q-99).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ItemKind {
    /// A service sold (and, two-sided, bought).
    Service,
    /// A part bought and sold that is not tracked in inventory.
    NonInventory,
    /// A charge such as delivery or a fee.
    OtherCharge,
    /// A discount, as a percent or an amount.
    Discount,
    /// A subtotal of the lines above it.
    Subtotal,
}

impl ItemKind {
    /// Every kind, in list order.
    pub const ALL: [ItemKind; 5] = [
        ItemKind::Service,
        ItemKind::NonInventory,
        ItemKind::OtherCharge,
        ItemKind::Discount,
        ItemKind::Subtotal,
    ];

    /// Owner-facing name.
    #[must_use]
    pub const fn label(self) -> TextKey {
        TextKey(match self {
            ItemKind::Service => "items.kind.service",
            ItemKind::NonInventory => "items.kind.non_inventory",
            ItemKind::OtherCharge => "items.kind.other_charge",
            ItemKind::Discount => "items.kind.discount",
            ItemKind::Subtotal => "items.kind.subtotal",
        })
    }

    /// The kinds the engine can keep: services only until it stores an
    /// item type (U-39, capability `item-types-v1`).
    #[must_use]
    pub fn available(item_types: bool) -> Vec<ItemKind> {
        if item_types {
            ItemKind::ALL.to_vec()
        } else {
            vec![ItemKind::Service]
        }
    }
}

/// One item as the list shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemEntry {
    /// Id.
    pub id: ItemId,
    /// Name.
    pub name: String,
    /// Description.
    pub description: Option<String>,
    /// Sales price (the engine's value).
    pub price: Option<Money>,
    /// Income account.
    pub income_account: AccountId,
    /// Subject to sales tax.
    pub taxable: bool,
    /// Active.
    pub active: bool,
}

/// What the Item List shows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ItemFilter {
    /// Also list inactive items.
    pub show_inactive: bool,
    /// Name or description contains (case-insensitive).
    pub search: String,
}

/// Every loaded item, sorted by name, case-insensitive.
#[derive(Debug, Clone, Default)]
pub struct ItemList {
    entries: Vec<ItemEntry>,
    /// Lowercased name and description per entry.
    haystacks: Vec<String>,
}

fn name_order(a: &ItemEntry, b: &ItemEntry) -> Ordering {
    a.name
        .to_lowercase()
        .cmp(&b.name.to_lowercase())
        .then_with(|| a.name.cmp(&b.name))
        .then_with(|| a.id.to_string().cmp(&b.id.to_string()))
}

impl ItemList {
    /// The list of `entries`, in name order.
    #[must_use]
    pub fn new(mut entries: Vec<ItemEntry>) -> ItemList {
        entries.sort_by(name_order);
        let haystacks = entries
            .iter()
            .map(|e| {
                format!(
                    "{}\n{}",
                    e.name,
                    e.description.as_deref().unwrap_or_default()
                )
                .to_lowercase()
            })
            .collect();
        ItemList { entries, haystacks }
    }

    /// Every entry, in name order.
    #[must_use]
    pub fn entries(&self) -> &[ItemEntry] {
        &self.entries
    }

    /// Number of items loaded.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Nothing loaded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The item `id`.
    #[must_use]
    pub fn entry(&self, id: ItemId) -> Option<&ItemEntry> {
        self.entries.iter().find(|e| e.id == id)
    }

    /// Indexes of the entries shown under `filter`.
    #[must_use]
    pub fn visible(&self, filter: &ItemFilter) -> Vec<usize> {
        let q = filter.search.trim().to_lowercase();
        self.entries
            .iter()
            .enumerate()
            .filter(|(i, e)| {
                (e.active || filter.show_inactive)
                    && (q.is_empty() || self.haystacks[*i].contains(&q))
            })
            .map(|(i, _)| i)
            .collect()
    }

    /// Another item already called `name`, ignoring case and outer spaces.
    /// Inactive items keep their name: the engine's names are unique over
    /// every item of the company.
    #[must_use]
    pub fn name_owner(&self, name: &str, except: Option<ItemId>) -> Option<ItemId> {
        let want = name.trim().to_lowercase();
        if want.is_empty() {
            return None;
        }
        self.entries
            .iter()
            .find(|e| Some(e.id) != except && e.name.trim().to_lowercase() == want)
            .map(|e| e.id)
    }
}

/// The add/edit form's values as typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemDraft {
    /// Kind (services only until U-39).
    pub kind: ItemKind,
    /// Name.
    pub name: String,
    /// Description.
    pub description: String,
    /// Sales price as typed.
    pub price: String,
    /// Income account.
    pub income_account: Option<AccountId>,
    /// Subject to sales tax.
    pub taxable: bool,
}

impl ItemDraft {
    /// A blank service item, taxable, posting to `income_account`.
    #[must_use]
    pub fn new(income_account: Option<AccountId>) -> ItemDraft {
        ItemDraft {
            kind: ItemKind::Service,
            name: String::new(),
            description: String::new(),
            price: String::new(),
            income_account,
            taxable: true,
        }
    }

    /// The form over a loaded item. The price is shown in the engine's
    /// exact value without trailing zeros past the cents (`12.50`,
    /// `0.0450`).
    #[must_use]
    pub fn from_entry(e: &ItemEntry) -> ItemDraft {
        ItemDraft {
            kind: ItemKind::Service,
            name: e.name.clone(),
            description: e.description.clone().unwrap_or_default(),
            price: e.price.map(price_text).unwrap_or_default(),
            income_account: Some(e.income_account),
            taxable: e.taxable,
        }
    }
}

/// A price as the form shows it: at least two decimals, the rest only when
/// they are not zero.
#[must_use]
pub fn price_text(m: Money) -> String {
    let wire = m.to_wire();
    match wire.split_once('.') {
        Some((whole, frac)) => {
            let mut frac = frac.trim_end_matches('0').to_string();
            while frac.len() < 2 {
                frac.push('0');
            }
            format!("{whole}.{frac}")
        }
        None => format!("{wire}.00"),
    }
}

/// A draft that passed [`check`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedItem {
    /// Name, trimmed.
    pub name: String,
    /// Description, trimmed; `None` when blank.
    pub description: Option<String>,
    /// Price; `None` when blank.
    pub price: Option<Money>,
    /// Income account.
    pub income_account: AccountId,
    /// Subject to sales tax.
    pub taxable: bool,
}

/// The form field a problem belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ItemField {
    /// Kind.
    Kind,
    /// Name.
    Name,
    /// Description.
    Description,
    /// Sales price.
    Price,
    /// Income account.
    IncomeAccount,
    /// Taxable.
    Taxable,
}

/// Why an item draft cannot be saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ItemProblem {
    /// The kind needs engine support that is not there yet (U-39).
    KindNotAvailable,
    /// No name.
    NameMissing,
    /// Longer than the engine takes.
    NameTooLong,
    /// Another item (perhaps inactive) has this name.
    NameTaken,
    /// Longer than kept.
    DescriptionTooLong,
    /// The price is not an amount.
    PriceInvalid,
    /// The price is below zero.
    PriceNegative,
    /// No income account, or one that is gone or not an income account.
    IncomeAccountMissing,
}

impl ItemProblem {
    /// Plain-language message.
    #[must_use]
    pub const fn key(self) -> TextKey {
        TextKey(match self {
            ItemProblem::KindNotAvailable => "items.problem.kind_not_available",
            ItemProblem::NameMissing => "items.problem.name_missing",
            ItemProblem::NameTooLong => "items.problem.name_too_long",
            ItemProblem::NameTaken => "items.problem.name_taken",
            ItemProblem::DescriptionTooLong => "items.problem.description_too_long",
            ItemProblem::PriceInvalid => "items.problem.price_invalid",
            ItemProblem::PriceNegative => "items.problem.price_negative",
            ItemProblem::IncomeAccountMissing => "items.problem.income_account_missing",
        })
    }

    /// The field to focus.
    #[must_use]
    pub const fn field(self) -> ItemField {
        match self {
            ItemProblem::KindNotAvailable => ItemField::Kind,
            ItemProblem::NameMissing | ItemProblem::NameTooLong | ItemProblem::NameTaken => {
                ItemField::Name
            }
            ItemProblem::DescriptionTooLong => ItemField::Description,
            ItemProblem::PriceInvalid | ItemProblem::PriceNegative => ItemField::Price,
            ItemProblem::IncomeAccountMissing => ItemField::IncomeAccount,
        }
    }
}

/// Parses a typed price: blank is no price; a calculation is allowed and
/// its result kept exact (to four decimals).
///
/// # Errors
/// [`ItemProblem::PriceNegative`] or [`ItemProblem::PriceInvalid`].
pub fn parse_price(text: &str) -> Result<Option<Money>, ItemProblem> {
    if text.trim().is_empty() {
        return Ok(None);
    }
    match amount_entry::parse(text, PRICE_RULES) {
        Ok(p) => match p.value {
            Amount::Money(m) if m.is_negative() => Err(ItemProblem::PriceNegative),
            Amount::Money(m) => Ok(Some(m)),
            _ => Err(ItemProblem::PriceInvalid),
        },
        Err(amount_entry::AmountEntryError::Negative) => Err(ItemProblem::PriceNegative),
        Err(_) => Err(ItemProblem::PriceInvalid),
    }
}

/// Checks a draft against the loaded list. `income_ok` says whether an
/// account exists and is an income account.
///
/// # Errors
/// Every problem found, in field order.
pub fn check(
    draft: &ItemDraft,
    list: &ItemList,
    editing: Option<ItemId>,
    item_types: bool,
    income_ok: impl Fn(AccountId) -> bool,
) -> Result<CheckedItem, Vec<ItemProblem>> {
    let mut problems = Vec::new();
    if !ItemKind::available(item_types).contains(&draft.kind) {
        problems.push(ItemProblem::KindNotAvailable);
    }
    let name = draft.name.trim();
    if name.is_empty() {
        problems.push(ItemProblem::NameMissing);
    } else if engine_len(name) > NAME_MAX {
        problems.push(ItemProblem::NameTooLong);
    } else if list.name_owner(name, editing).is_some() {
        problems.push(ItemProblem::NameTaken);
    }
    let description = draft.description.trim();
    if engine_len(description) > DESCRIPTION_MAX {
        problems.push(ItemProblem::DescriptionTooLong);
    }
    let price = match parse_price(&draft.price) {
        Ok(p) => p,
        Err(e) => {
            problems.push(e);
            None
        }
    };
    let income = draft.income_account.filter(|a| income_ok(*a));
    if income.is_none() {
        problems.push(ItemProblem::IncomeAccountMissing);
    }
    match (income, problems.is_empty()) {
        (Some(income_account), true) => Ok(CheckedItem {
            name: name.to_string(),
            description: (!description.is_empty()).then(|| description.to_string()),
            price,
            income_account,
            taxable: draft.taxable,
        }),
        _ => Err(problems),
    }
}

#[cfg(test)]
mod tests;
