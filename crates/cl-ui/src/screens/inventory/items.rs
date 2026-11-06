//! The inventory panel of Lists › Items (P24 plan §3): purchase
//! description and cost, the asset and cost of goods sold accounts,
//! preferred vendor, reorder point and quantity, unit, the engine's on
//! hand, average cost and value, and a new part's opening stock (Q-P24h).
//!
//! The panel edits a copy of the form's [`PartDraft`]; the Item List
//! records the change in its undo history.

use cl_api::{Account, AccountType, Contact, ContactType, Item, ItemPurchase, ItemStock};
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
}

impl PanelLists {
    /// The choices from the chart and the contacts (active ones, plus the
    /// part's own when it is no longer active).
    #[must_use]
    pub fn new(accounts: &[Account], contacts: &[Contact], keep: &PartDraft) -> PanelLists {
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
    text(
        ui,
        PartField::UnitLabel,
        "inventory.panel.unit",
        &mut out.unit_label,
    );
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
    changed.then_some(out)
}

/// The words of an inventory-section problem.
#[must_use]
pub fn problem_text(p: PartProblem, lock: Option<CalendarDate>) -> String {
    match p {
        PartProblem::OpeningInClosedPeriod => super::closed_text(lock),
        other => t(other.key().as_str()),
    }
}
