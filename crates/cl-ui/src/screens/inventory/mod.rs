//! Inventory screens (P24 plan §3, WP-2): the inventory panel of Lists ›
//! Items, Adjust Quantity/Value on Hand, the physical count worksheet and
//! its printable sheet, Items to Reorder, and the three inventory reports
//! (Inventory Value with the GL check, Inventory Value Detail, Stock
//! Status).
//!
//! Every figure is the engine's (`Money`, `Quantity`, `UnitCost` parsed
//! from its strings); the screens add nothing up (plan §4). The one
//! subtraction is the count worksheet's difference, a quantity the owner
//! reviews before posting (cl-model).

use std::sync::Arc;

use cl_api::{ApiError, InventoryRefusal, NegativeStock};
use cl_core::ids::{ItemId, TenantId};
use cl_core::{CalendarDate, Money, MoneyStyle, Quantity, UnitCost};
use cl_model::CommandId;

use crate::company::CompanyLink;
use crate::i18n::{t, t_with};
use crate::windows::Screen;

pub mod adjust;
pub mod count;
pub mod items;
pub mod reorder;
pub mod reports;

/// The developer command that puts the demo's inventory back to its
/// switched-on seed (the scenarios run light and dark in one app).
pub const DEV_RESET: &str = "dev.demo_inventory_reset";

/// The inventory windows.
pub fn make(
    kind: CommandId,
    link: &Arc<CompanyLink>,
    _open: Option<(TenantId, String)>,
    today: CalendarDate,
) -> Option<Box<dyn Screen>> {
    let link = Arc::clone(link);
    match kind.as_str() {
        adjust::KIND => Some(Box::new(adjust::AdjustScreen::new(link, today))),
        count::KIND => Some(Box::new(count::CountScreen::new(link, today))),
        reorder::KIND => Some(Box::new(reorder::ReorderScreen::new(link))),
        reports::VALUE_KIND => Some(Box::new(reports::ReportScreen::new(
            link,
            reports::Report::Value,
            today,
        ))),
        reports::DETAIL_KIND => Some(Box::new(reports::ReportScreen::new(
            link,
            reports::Report::Detail,
            today,
        ))),
        reports::STOCK_KIND => Some(Box::new(reports::ReportScreen::new(
            link,
            reports::Report::Stock,
            today,
        ))),
        _ => None,
    }
}

/// A quantity as the screens show it: no trailing zeros (`3.5`, `120`).
#[must_use]
pub fn qty(q: Quantity) -> String {
    cl_model::inventory::quantity_text(q)
}

/// An amount in the display style.
#[must_use]
pub fn money(m: Money) -> String {
    m.format(MoneyStyle::default())
}

/// A cost per unit (six places as the engine keeps it).
#[must_use]
pub fn cost(c: UnitCost) -> String {
    c.format(MoneyStyle::default())
}

/// The plan's NEGATIVE_STOCK warning (§3), with the item's name.
#[must_use]
pub fn negative_text(name: &str, w: &NegativeStock) -> String {
    t_with(
        "inventory.negative_stock",
        &[
            ("item", name),
            ("date", &crate::shell::us_date(w.date)),
            ("on_hand", &qty(w.on_hand)),
        ],
    )
}

/// Every warning as text, the item named from `name_of`.
#[must_use]
pub fn negative_texts(
    warnings: &[NegativeStock],
    name_of: impl Fn(ItemId) -> Option<String>,
) -> Vec<String> {
    warnings
        .iter()
        .map(|w| {
            let name = name_of(w.item_id).unwrap_or_else(|| t("inventory.an_item"));
            negative_text(&name, w)
        })
        .collect()
}

/// The plan's closed-period message (§3): never offers the password.
#[must_use]
pub fn closed_text(lock: Option<CalendarDate>) -> String {
    let date = lock.map_or_else(|| t("inventory.closing_date"), crate::shell::us_date);
    t_with("inventory.period_closed", &[("date", &date)])
}

/// The plain message of an inventory refusal (`None` for anything else).
#[must_use]
pub fn refusal_text(e: &ApiError, lock: Option<CalendarDate>) -> Option<String> {
    let key = match cl_api::inventory_refusal(e)? {
        InventoryRefusal::PeriodClosed => return Some(closed_text(lock)),
        InventoryRefusal::Off => "inventory.refusal.off",
        InventoryRefusal::InUse => "inventory.refusal.in_use",
        InventoryRefusal::NoStock => "inventory.refusal.no_stock",
        InventoryRefusal::ItemInUse => "inventory.refusal.item_in_use",
        InventoryRefusal::ItemHasInventory => "inventory.refusal.item_has_inventory",
        InventoryRefusal::Document => "inventory.refusal.document",
        InventoryRefusal::AccountDirect => "inventory.refusal.account_direct",
        InventoryRefusal::AccountInUse => "inventory.refusal.account_in_use",
    };
    Some(t(key))
}

/// The demo's closing date, else `None` (live screens read it from the
/// engine's error when it refuses).
#[must_use]
pub fn demo_lock(link: &CompanyLink) -> Option<CalendarDate> {
    link.demo_state()
        .and_then(|d| crate::company::demo::stock::closing(&d))
}

/// The closing date a refusal names: the demo's, else the engine's
/// `details.lockDate`.
#[must_use]
pub fn lock_of(link: &CompanyLink, e: &ApiError) -> Option<CalendarDate> {
    demo_lock(link).or_else(|| match e {
        ApiError::Conflict { details, .. } => details
            .text("lockDate")
            .and_then(|d| CalendarDate::parse_wire(d).ok()),
        _ => None,
    })
}

/// Shows the NEGATIVE_STOCK warnings of a save that went through (Q-148:
/// never blocking): one toast line per item, item names from the demo
/// or `names`.
pub fn announce(link: &CompanyLink, warnings: &[NegativeStock], names: &[(ItemId, String)]) {
    if warnings.is_empty() {
        return;
    }
    let demo_names: Vec<(ItemId, String)> = link
        .demo_state()
        .map(|mut d| {
            d.items_mut()
                .iter()
                .map(|i| (i.id, i.fields.name.clone()))
                .collect()
        })
        .unwrap_or_default();
    let texts = negative_texts(warnings, |id| {
        names
            .iter()
            .chain(&demo_names)
            .find(|(i, _)| *i == id)
            .map(|(_, n)| n.clone())
    });
    link.push(crate::company::CompanyEvent::ToastText(texts.join(" ")));
}

/// The demo answers: the warnings its last write kept (read once).
#[must_use]
pub fn demo_warnings(link: &CompanyLink) -> Vec<NegativeStock> {
    link.demo_state()
        .map(|mut d| crate::company::demo::stock::take_warnings(&mut d))
        .unwrap_or_default()
}

/// What the inventory windows read besides their own figures: the parts,
/// the chart (names and pickers) and the vendors.
#[derive(Debug, Clone, Default)]
pub struct Books {
    /// Inventory parts (active and inactive), in name order.
    pub parts: Vec<cl_api::Item>,
    /// The chart.
    pub accounts: Vec<cl_api::Account>,
    /// Vendors.
    pub vendors: Vec<cl_api::Contact>,
}

impl Books {
    /// A part's name.
    #[must_use]
    pub fn name(&self, id: ItemId) -> Option<String> {
        self.parts
            .iter()
            .find(|i| i.id == id)
            .map(|i| i.fields.name.clone())
    }

    /// A part.
    #[must_use]
    pub fn part(&self, id: ItemId) -> Option<&cl_api::Item> {
        self.parts.iter().find(|i| i.id == id)
    }

    /// An account as the pickers show it (number and name).
    #[must_use]
    pub fn account_name(&self, id: cl_core::ids::AccountId) -> String {
        self.accounts
            .iter()
            .find(|a| a.id == id)
            .map(|a| match &a.fields.account_number {
                Some(n) if !n.is_empty() => {
                    format!("{n} {}", crate::account_names::shown_name(a))
                }
                _ => crate::account_names::shown_name(a),
            })
            .unwrap_or_default()
    }

    /// A vendor's name.
    #[must_use]
    pub fn vendor_name(&self, id: Option<cl_core::ids::ContactId>) -> String {
        id.and_then(|v| self.vendors.iter().find(|c| c.id == v))
            .map(|c| c.fields.display_name.clone())
            .unwrap_or_default()
    }

    /// The active parts' names, for a picker.
    #[must_use]
    pub fn active_parts(&self) -> Vec<(ItemId, String)> {
        self.parts
            .iter()
            .filter(|i| i.is_active)
            .map(|i| (i.id, i.fields.name.clone()))
            .collect()
    }
}

fn books_of(
    items: Vec<cl_api::Item>,
    accounts: Vec<cl_api::Account>,
    contacts: Vec<cl_api::Contact>,
) -> Books {
    let mut parts: Vec<cl_api::Item> = items
        .into_iter()
        .filter(|i| i.fields.item_type == cl_api::ItemType::InventoryPart)
        .collect();
    parts.sort_by_key(|i| i.fields.name.to_lowercase());
    let vendors = contacts
        .into_iter()
        .filter(|c| {
            matches!(
                c.fields.contact_type,
                cl_api::ContactType::Vendor | cl_api::ContactType::Both
            )
        })
        .collect();
    Books {
        parts,
        accounts,
        vendors,
    }
}

/// Reads the [`Books`].
pub(crate) fn load_books(
    link: &CompanyLink,
    group: Option<&cl_api::RequestGroup>,
) -> crate::screens::invoice::engine::Job<Books> {
    crate::screens::invoice::engine::read(
        link,
        group,
        |d| {
            crate::company::demo::stock::refresh(d);
            let items = d.items_mut().clone();
            let accounts = d
                .chart
                .get_or_insert_with(crate::company::demo::chart)
                .clone();
            let contacts = d.contacts_mut().clone();
            Ok(books_of(items, accounts, contacts))
        },
        |s| async move {
            let items = s.items().list_all(&cl_api::ItemFilter::default()).await?;
            let accounts = s
                .accounts()
                .list_all(&cl_api::AccountFilter::default())
                .await?;
            let contacts = s
                .contacts()
                .list_all(&cl_api::ContactFilter {
                    contact_type: Some(cl_api::ContactType::Vendor),
                    ..cl_api::ContactFilter::default()
                })
                .await
                .unwrap_or_default();
            Ok(books_of(items, accounts, contacts))
        },
    )
}

/// The developer reset: the demo's inventory as on its first switch-on.
pub fn demo_reset(link: &CompanyLink) -> bool {
    let Some(mut d) = link.demo_state() else {
        return false;
    };
    let first = crate::company::demo::stock::FIRST_PART;
    let parts: Vec<ItemId> = (0..3)
        .map(|n| crate::company::demo::lists::item_id(first + n))
        .collect();
    d.items_mut().retain(|i| !parts.contains(&i.id));
    d.inventory = crate::company::demo::inventory::DemoInventory::default();
    let ok = crate::company::demo::stock::set_enabled(&mut d, true).is_ok();
    drop(d);
    link.bus().publish(crate::data_bus::Change::Items);
    ok
}

#[cfg(test)]
mod tests;
