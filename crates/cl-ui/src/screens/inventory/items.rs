//! The inventory panel of Lists › Items (P24 plan §3): purchase
//! description and cost, the asset and cost of goods sold accounts,
//! preferred vendor, reorder point and quantity, unit, the engine's on
//! hand, average cost and value, and a new part's opening stock (Q-P24h).
//!
//! The panel edits a copy of the form's [`PartDraft`]; the Item List
//! records the change in its undo history.

use cl_api::{Account, AccountType, Contact, ContactType, Item, ItemPurchase, ItemStock, UnitSet};
use cl_core::CalendarDate;
use cl_core::ids::{AccountId, ContactId};
use cl_model::inventory::{PartDraft, PartField, PartProblem, quantity_text};
use cl_model::lists::items::price_text;
use eframe::egui::{self, RichText, Ui};

use crate::i18n::t;
use crate::screens::company::form::{self, DateInput};
use crate::screens::lists::detail_pane as pane;
use crate::theme::Tokens;

/// What the panel offers.
#[derive(Debug, Clone, Default)]
pub struct PanelLists {
    /// Asset accounts a part may be kept in.
    pub assets: Vec<(AccountId, String)>,
    /// Cost of goods sold accounts.
    pub cogs: Vec<(AccountId, String)>,
    /// Vendors.
    pub vendors: Vec<(ContactId, String)>,
    /// Unit sets (active ones, plus the part's own).
    pub unit_sets: Vec<UnitSet>,
}

impl PanelLists {
    /// The choices from the chart and the contacts (active ones, plus the
    /// part's own when it is no longer active).
    #[must_use]
    pub fn new(
        accounts: &[Account],
        contacts: &[Contact],
        unit_sets: &[UnitSet],
        keep: &PartDraft,
    ) -> PanelLists {
        let name = |a: &Account| match &a.fields.account_number {
            Some(n) if !n.is_empty() => format!("{n} {}", crate::account_names::shown_name(a)),
            _ => crate::account_names::shown_name(a),
        };
        let pick = |want: fn(&Account) -> bool, keep: Option<AccountId>| {
            accounts
                .iter()
                .filter(|a| (a.is_active && want(a)) || Some(a.id) == keep)
                .map(|a| (a.id, name(a)))
                .collect::<Vec<_>>()
        };
        let mut vendors: Vec<(ContactId, String)> = contacts
            .iter()
            .filter(|c| {
                matches!(
                    c.fields.contact_type,
                    ContactType::Vendor | ContactType::Both
                ) && (c.is_active || Some(c.id) == keep.preferred_vendor)
            })
            .map(|c| (c.id, c.fields.display_name.clone()))
            .collect();
        vendors.sort_by_key(|(_, n)| n.to_lowercase());
        PanelLists {
            assets: pick(
                |a| a.fields.account_type == AccountType::Asset,
                keep.asset_account,
            ),
            cogs: pick(
                |a| a.fields.account_type == AccountType::Cogs,
                keep.cogs_account,
            ),
            vendors,
            unit_sets: unit_sets
                .iter()
                .filter(|s| s.is_active || Some(s.id) == keep.unit_set)
                .cloned()
                .collect(),
        }
    }
}

/// The form's inventory fields of a loaded item.
#[must_use]
pub fn draft_of(p: &ItemPurchase) -> PartDraft {
    PartDraft {
        purchase_description: p.purchase_description.clone().unwrap_or_default(),
        cost: p.purchase_cost.map(price_text).unwrap_or_default(),
        expense_account: p.expense_account_id,
        asset_account: p.asset_account_id,
        cogs_account: p.cogs_account_id,
        preferred_vendor: p.preferred_vendor_id,
        reorder_point: p.reorder_point.map(quantity_text).unwrap_or_default(),
        reorder_quantity: p.reorder_quantity.map(quantity_text).unwrap_or_default(),
        unit_label: p.unit_label.clone().unwrap_or_default(),
        unit_set: p.unit_set_id,
        purchase_unit: p.default_purchase_unit_id,
        sale_unit: p.default_sale_unit_id,
        opening_quantity: String::new(),
        opening_value: String::new(),
        opening_date: None,
    }
}

/// A new part's starting accounts: the company's Inventory Asset and its
/// first cost of goods sold account.
#[must_use]
pub fn defaults(accounts: &[Account], today: CalendarDate) -> PartDraft {
    let tagged = |tag: &str| {
        accounts
            .iter()
            .find(|a| a.is_active && a.system_tag.as_deref() == Some(tag))
            .map(|a| a.id)
    };
    PartDraft {
        asset_account: tagged("inventory_asset"),
        cogs_account: tagged("cost_of_goods_sold").or_else(|| {
            accounts
                .iter()
                .find(|a| a.is_active && a.fields.account_type == AccountType::Cogs)
                .map(|a| a.id)
        }),
        opening_date: Some(today),
        ..PartDraft::default()
    }
}

/// The engine's stock of `item` (read-only lines).
#[must_use]
pub fn stock_of(item: Option<&Item>) -> Option<&ItemStock> {
    item.map(|i| &i.stock)
        .filter(|s| s.quantity_on_hand.is_some() || s.asset_value.is_some())
}

/// The panel. `opening_date` is the opening date's text field; returns
/// the edited draft when a field changed.
#[allow(clippy::too_many_arguments)]
pub fn panel(
    ui: &mut Ui,
    tokens: &Tokens,
    lists: &PanelLists,
    d: &PartDraft,
    stock: Option<&ItemStock>,
    new_item: bool,
    opening_date: &mut DateInput,
    today: CalendarDate,
    focus: Option<PartField>,
) -> Option<PartDraft> {
    let mut out = d.clone();
    let mut changed = false;
    let want = |f: PartField| focus == Some(f);
    pane::section(ui, tokens, &t("inventory.panel.title"));
    let mut text = |ui: &mut Ui, field: PartField, key: &str, v: &mut String| {
        let r = pane::text_row(
            ui,
            egui::Id::unique(("items/part", key)),
            &t(key),
            v,
            want(field),
        );
        changed |= r.changed();
    };
    text(
        ui,
        PartField::PurchaseDescription,
        "inventory.panel.purchase_description",
        &mut out.purchase_description,
    );
    text(ui, PartField::Cost, "inventory.panel.cost", &mut out.cost);
    let mut pick_account = |ui: &mut Ui,
                            field: PartField,
                            key: &str,
                            choices: &[(AccountId, String)],
                            cur: &mut Option<AccountId>,
                            locked: bool| {
        let mut labels: Vec<String> = vec![t("inventory.panel.choose")];
        labels.extend(choices.iter().map(|(_, n)| n.clone()));
        let at = cur
            .and_then(|a| choices.iter().position(|(x, _)| *x == a))
            .map_or(0, |i| i + 1);
        ui.add_enabled_ui(!locked, |ui| {
            if let Some(i) = pane::picker(
                ui,
                egui::Id::unique(("items/part", key)),
                &t(key),
                &labels,
                at,
                want(field),
            ) {
                *cur = i
                    .checked_sub(1)
                    .and_then(|i| choices.get(i))
                    .map(|(a, _)| *a);
                changed = true;
            }
        });
    };
    // Q-P24k: the asset account is fixed once the part has stock.
    let has_stock = stock.is_some_and(|s| {
        s.quantity_on_hand.is_some_and(|q| !q.is_zero())
            || s.asset_value.is_some_and(|v| !v.is_zero())
    });
    pick_account(
        ui,
        PartField::AssetAccount,
        "inventory.panel.asset_account",
        &lists.assets,
        &mut out.asset_account,
        has_stock && !new_item,
    );
    if has_stock && !new_item {
        form::hint(ui, tokens, &t("inventory.panel.asset_fixed"));
    }
    pick_account(
        ui,
        PartField::CogsAccount,
        "inventory.panel.cogs_account",
        &lists.cogs,
        &mut out.cogs_account,
        false,
    );
    let mut vendor_labels = vec![t("inventory.panel.no_vendor")];
    vendor_labels.extend(lists.vendors.iter().map(|(_, n)| n.clone()));
    let at = out
        .preferred_vendor
        .and_then(|v| lists.vendors.iter().position(|(x, _)| *x == v))
        .map_or(0, |i| i + 1);
    if let Some(i) = pane::picker(
        ui,
        egui::Id::unique("items/part/vendor"),
        &t("inventory.panel.vendor"),
        &vendor_labels,
        at,
        false,
    ) {
        out.preferred_vendor = i
            .checked_sub(1)
            .and_then(|i| lists.vendors.get(i))
            .map(|(v, _)| *v);
        changed = true;
    }
    let mut text = |ui: &mut Ui, field: PartField, key: &str, v: &mut String| {
        let r = pane::text_row(
            ui,
            egui::Id::unique(("items/part", key)),
            &t(key),
            v,
            want(field),
        );
        changed |= r.changed();
    };
    text(
        ui,
        PartField::ReorderPoint,
        "inventory.panel.reorder_point",
        &mut out.reorder_point,
    );
    text(
        ui,
        PartField::ReorderQuantity,
        "inventory.panel.reorder_quantity",
        &mut out.reorder_quantity,
    );
    let units_changed = units_rows(ui, tokens, lists, &mut out, has_stock && !new_item);
    if out.unit_set.is_none() {
        text(
            ui,
            PartField::UnitLabel,
            "inventory.panel.unit",
            &mut out.unit_label,
        );
    }
    if let Some(s) = stock.filter(|_| !new_item) {
        ui.add_space(4.0);
        let dash = || "\u{2014}".to_string();
        pane::row(
            ui,
            tokens,
            &t("inventory.panel.on_hand"),
            &s.quantity_on_hand.map_or_else(dash, super::qty),
        );
        pane::row(
            ui,
            tokens,
            &t("inventory.panel.average_cost"),
            &s.average_cost.map_or_else(dash, super::cost),
        );
        pane::row(
            ui,
            tokens,
            &t("inventory.panel.value"),
            &s.asset_value.map_or_else(dash, super::money),
        );
    }
    if new_item {
        ui.add_space(4.0);
        ui.label(RichText::new(t("inventory.panel.opening")).strong());
        form::hint(ui, tokens, &t("inventory.panel.opening_hint"));
        text(
            ui,
            PartField::OpeningQuantity,
            "inventory.panel.opening_quantity",
            &mut out.opening_quantity,
        );
        text(
            ui,
            PartField::OpeningValue,
            "inventory.panel.opening_value",
            &mut out.opening_value,
        );
        let r = form::date_field(ui, &t("inventory.panel.opening_date"), opening_date, today);
        if want(PartField::OpeningDate) {
            r.request_focus();
        }
        if opening_date.value != out.opening_date {
            out.opening_date = opening_date.value;
            changed = true;
        }
    }
    (changed || units_changed).then_some(out)
}

/// The unit set and default unit pickers (U-161). The set is fixed once the
/// part has stock (the engine refuses a change while any saved line names
/// the item, 409 `UNIT_IN_USE`). Returns true when a pick changed.
fn units_rows(
    ui: &mut Ui,
    tokens: &Tokens,
    lists: &PanelLists,
    out: &mut PartDraft,
    locked: bool,
) -> bool {
    let mut changed = false;
    let mut set_labels = vec![t("inventory.panel.no_unit_set")];
    set_labels.extend(lists.unit_sets.iter().map(|s| s.name.clone()));
    let at = out
        .unit_set
        .and_then(|id| lists.unit_sets.iter().position(|s| s.id == id))
        .map_or(0, |i| i + 1);
    ui.add_enabled_ui(!locked, |ui| {
        if let Some(i) = pane::picker(
            ui,
            egui::Id::unique("items/part/unit_set"),
            &t("inventory.panel.unit_set"),
            &set_labels,
            at,
            false,
        ) {
            let next = i
                .checked_sub(1)
                .and_then(|i| lists.unit_sets.get(i))
                .map(|s| s.id);
            if next != out.unit_set {
                out.unit_set = next;
                out.purchase_unit = None;
                out.sale_unit = None;
                changed = true;
            }
        }
    });
    if locked && out.unit_set.is_some() {
        form::hint(ui, tokens, &t("inventory.panel.unit_set_fixed"));
    }
    let Some(set) = out
        .unit_set
        .and_then(|id| lists.unit_sets.iter().find(|s| s.id == id))
    else {
        return changed;
    };
    let mut ids = vec![None];
    ids.extend(set.units.iter().map(|u| u.id));
    let names: Vec<String> = ids
        .iter()
        .map(|u| set.name_of(*u).unwrap_or_default().to_string())
        .collect();
    for (key, cur) in [
        ("inventory.panel.purchase_unit", &mut out.purchase_unit),
        ("inventory.panel.sale_unit", &mut out.sale_unit),
    ] {
        let at = ids.iter().position(|u| u == cur).unwrap_or(0);
        if let Some(i) = pane::picker(
            ui,
            egui::Id::unique(("items/part", key)),
            &t(key),
            &names,
            at,
            false,
        ) {
            *cur = ids.get(i).copied().flatten();
            changed = true;
        }
    }
    changed
}

/// The words of an inventory-section problem.
#[must_use]
pub fn problem_text(p: PartProblem, lock: Option<CalendarDate>) -> String {
    match p {
        PartProblem::OpeningInClosedPeriod => super::closed_text(lock),
        other => t(other.key().as_str()),
    }
}
