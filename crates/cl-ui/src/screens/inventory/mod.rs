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
pub mod units;

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
        units::KIND => Some(Box::new(units::UnitsScreen::new(link))),
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

/// An amount or quantity cell, right-aligned (DESIGN.md: numbers end on
/// the right of their column).
pub(crate) fn num(ui: &mut eframe::egui::Ui, text: eframe::egui::RichText) {
    use eframe::egui;
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.label(text);
    });
}

/// A grid header; the number columns' headers end on the right too.
pub(crate) fn head(ui: &mut eframe::egui::Ui, key: &str) {
    use eframe::egui;
    const NUMBERS: [&str; 10] = [
        "col.on_hand",
        "col.value",
        "col.new_qty",
        "col.average_cost",
        "col.percent",
        "col.quantity",
        "col.running_quantity",
        "col.running_value",
        "col.reorder_point",
        "col.reorder_quantity",
    ];
    let text = egui::RichText::new(t(key)).strong().size(12.0);
    let right = NUMBERS.iter().any(|n| key.ends_with(n))
        || key == "inventory.reorder.col.point"
        || key == "inventory.reorder.col.order"
        || key == "inventory.reorder.col.cost";
    if right {
        num(ui, text);
    } else {
        ui.label(text);
    }
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

/// The same warning after a void or delete, worded for what the owner did.
#[must_use]
pub fn negative_void_text(name: &str, w: &NegativeStock) -> String {
    t_with(
        "inventory.negative_stock.void",
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
    let Some(refusal) = cl_api::inventory_refusal(e) else {
        return unit_refusal_text(e);
    };
    let key = match refusal {
        InventoryRefusal::PeriodClosed => return Some(closed_text(lock)),
        InventoryRefusal::Off => "inventory.refusal.off",
        InventoryRefusal::InUse => "inventory.refusal.in_use",
        InventoryRefusal::NoStock => "inventory.refusal.no_stock",
        InventoryRefusal::ItemInUse => "inventory.refusal.item_in_use",
        InventoryRefusal::ItemHasInventory => "inventory.refusal.item_has_inventory",
        InventoryRefusal::Document => "inventory.refusal.document",
        InventoryRefusal::AccountDirect => "inventory.refusal.account_direct",
        InventoryRefusal::AccountInUse => "inventory.refusal.account_in_use",
        InventoryRefusal::ReturnBeforeSale => "inventory.refusal.return_before_sale",
        InventoryRefusal::ReturnExceedsSale => "inventory.refusal.return_exceeds_sale",
        InventoryRefusal::SaleHasReturns => "inventory.refusal.sale_has_returns",
    };
    Some(t(key))
}

/// The words of a line unit the engine refused (U-161): a unit outside
/// the item's set, a quantity too small for its unit, a unit in use.
#[must_use]
pub fn unit_refusal_text(e: &ApiError) -> Option<String> {
    let key = match cl_api::unit_refusal(e)? {
        cl_api::UnitRefusal::NotInSet => "units.refusal.not_in_set",
        cl_api::UnitRefusal::QuantityTooSmall => "units.refusal.too_small",
        cl_api::UnitRefusal::InUse => "units.refusal.in_use",
        cl_api::UnitRefusal::DuplicateName => "units.refusal.duplicate",
    };
    Some(t(key))
}

/// The Unit picker of an adjustment or count row whose part has a unit
/// set (`units`: its `unitPrices[]`, the base unit first; U-161). Nothing
/// is drawn without units. Returns true when the unit changed.
pub(crate) fn unit_picker(
    ui: &mut eframe::egui::Ui,
    salt: &'static str,
    row: usize,
    units: &[cl_api::UnitPrice],
    unit: &mut Option<cl_core::ids::UnitId>,
) -> bool {
    use eframe::egui;
    if units.is_empty() {
        return false;
    }
    let current = *unit;
    let shown = units
        .iter()
        .find(|u| u.unit_id == current)
        .or_else(|| units.first())
        .map(|u| u.name.clone())
        .unwrap_or_default();
    let label = format!("{} {}", t("lines.col.unit"), row + 1);
    let mut pick = None;
    egui::ComboBox::from_id_salt((salt, row))
        .selected_text(shown)
        .width(90.0)
        .show_ui(ui, |ui| {
            for u in units {
                if ui.selectable_label(u.unit_id == current, &u.name).clicked() {
                    pick = Some(u.unit_id);
                }
            }
        })
        .response
        .on_hover_text(t("lines.col.unit.help"))
        .widget_info(|| egui::WidgetInfo::labeled(egui::Role::ComboBox, true, &label));
    match pick {
        Some(p) if p != current => {
            *unit = p;
            true
        }
        _ => false,
    }
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

/// The company's cost of goods sold account for inventory (QA-2/QA-3,
/// PR-4): an active cost-of-goods-sold account whose detail type is
/// "Cost of goods sold" (what turning inventory on creates), else `None`
/// so the owner must choose. Never just the first COGS-type account: in a
/// service chart that is "Subcontractors", which isn't an inventory cost.
#[must_use]
pub fn inventory_cogs_account(accounts: &[cl_api::Account]) -> Option<cl_core::ids::AccountId> {
    let active_cogs = || {
        accounts
            .iter()
            .filter(|a| a.is_active && a.fields.account_type == cl_api::AccountType::Cogs)
    };
    active_cogs()
        .find(|a| a.system_tag.as_deref() == Some("cost_of_goods_sold"))
        .or_else(|| {
            active_cogs().find(|a| a.fields.detail_type.as_deref() == Some("cost_of_goods_sold"))
        })
        .map(|a| a.id)
}

#[cfg(test)]
mod default_account_tests {
    use cl_api::{Account, AccountType};

    fn acct(n: u32, name: &str, ty: AccountType, detail: &str) -> Account {
        let mut a: Account = serde_json::from_value(serde_json::json!({
            "id": format!("a0000000-0000-4000-8000-{n:012}"),
            "name": name,
            "accountType": "asset",
            "detailType": detail,
        }))
        .expect("account");
        a.fields.account_type = ty;
        a
    }

    #[test]
    fn qa2_service_chart_has_no_inventory_cogs_default() {
        let a = [acct(
            1,
            "Subcontractors",
            AccountType::Cogs,
            "other_cost_of_service",
        )];
        assert_eq!(super::inventory_cogs_account(&a), None);
    }

    #[test]
    fn qa2_cogs_default_is_the_cost_of_goods_sold_account() {
        let a = [
            acct(
                1,
                "Subcontractors",
                AccountType::Cogs,
                "other_cost_of_service",
            ),
            acct(
                2,
                "Cost of Goods Sold",
                AccountType::Cogs,
                "cost_of_goods_sold",
            ),
        ];
        assert_eq!(super::inventory_cogs_account(&a), Some(a[1].id));
    }

    #[test]
    fn qa3_asset_default_by_tag_or_name() {
        let mut a = [acct(
            1,
            "Inventory Asset",
            AccountType::Asset,
            "other_current_asset",
        )];
        assert_eq!(super::inventory_asset_account(&a), Some(a[0].id));
        a[0].fields.name = "Stock".into();
        assert_eq!(super::inventory_asset_account(&a), None);
        a[0].system_tag = Some("inventory_asset".into());
        assert_eq!(super::inventory_asset_account(&a), Some(a[0].id));
    }
}

/// The company's inventory asset account (the one tagged
/// `inventory_asset`, else an active asset account named "Inventory
/// Asset", the name the engine adopts when inventory is turned on).
#[must_use]
pub fn inventory_asset_account(accounts: &[cl_api::Account]) -> Option<cl_core::ids::AccountId> {
    accounts
        .iter()
        .find(|a| a.is_active && a.system_tag.as_deref() == Some("inventory_asset"))
        .or_else(|| {
            accounts.iter().find(|a| {
                a.is_active
                    && a.fields.account_type == cl_api::AccountType::Asset
                    && a.fields.name.trim().eq_ignore_ascii_case("Inventory Asset")
            })
        })
        .map(|a| a.id)
}

/// Shows the NEGATIVE_STOCK warnings of a save that went through (Q-148:
/// never blocking): one toast line per item, item names from the demo
/// or `names`.
pub fn announce(link: &CompanyLink, warnings: &[NegativeStock], names: &[(ItemId, String)]) {
    announce_as(link, warnings, names, negative_text);
}

fn announce_as(
    link: &CompanyLink,
    warnings: &[NegativeStock],
    names: &[(ItemId, String)],
    text_of: fn(&str, &NegativeStock) -> String,
) {
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
    let name_of = |id: ItemId| {
        names
            .iter()
            .chain(&demo_names)
            .find(|(i, _)| *i == id)
            .map(|(_, n)| n.clone())
    };
    let texts: Vec<String> = warnings
        .iter()
        .map(|w| {
            text_of(
                &name_of(w.item_id).unwrap_or_else(|| t("inventory.an_item")),
                w,
            )
        })
        .collect();
    link.push(crate::company::CompanyEvent::ToastText(texts.join(" ")));
}

/// The plan's closed-period message when a document with parts is dated
/// (or moved from a date) on or before the closing date: the forms show
/// it instead of asking for the closing-date password (Q-P24b).
#[must_use]
pub fn closed_for_parts(
    has_parts: bool,
    dates: &[CalendarDate],
    closing: Option<CalendarDate>,
) -> Option<String> {
    let c = closing?;
    (has_parts && dates.iter().any(|d| *d <= c)).then(|| closed_text(Some(c)))
}

/// After a demo save, void or delete: the warnings it found, announced.
/// (A live engine's warnings come in the save reply.)
pub fn announce_demo(link: &CompanyLink, names: &[(ItemId, String)]) {
    let w = demo_warnings(link);
    announce(link, &w, names);
}

/// A void or delete that went through (PR-4 S4): the live engine's
/// NEGATIVE_STOCK warnings from its reply, in the void's words; with the
/// demo and no warnings given, the warnings the demo found.
pub fn announce_void(link: &CompanyLink, warnings: &[NegativeStock], names: &[(ItemId, String)]) {
    if warnings.is_empty() && link.demo_state().is_some() {
        announce_demo_void(link, names);
    } else {
        announce_as(link, warnings, names, negative_void_text);
    }
}

/// After a demo void or delete: the warnings it found, in the void's words.
pub fn announce_demo_void(link: &CompanyLink, names: &[(ItemId, String)]) {
    let w = demo_warnings(link);
    announce_as(link, &w, names, negative_void_text);
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
    // Every part goes (the seeded ones and any made since).
    d.items_mut().retain(|i| {
        !parts.contains(&i.id) && i.fields.item_type != cl_api::ItemType::InventoryPart
    });
    d.inventory = crate::company::demo::inventory::DemoInventory::default();
    d.units = crate::company::demo::units::DemoUnits::default();
    let ok = crate::company::demo::stock::set_enabled(&mut d, true).is_ok();
    drop(d);
    link.bus().publish(crate::data_bus::Change::Items);
    ok
}

#[cfg(test)]
mod tests;
