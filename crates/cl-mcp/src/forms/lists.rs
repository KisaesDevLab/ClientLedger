//! The master lists (P08–P09): a checked contact or account as the engine's
//! fields, keeping what the form does not edit.

use cl_api::{
    AccountFields, AccountType, ContactFields, ContactType, ItemFields, TinType, Vendor1099Update,
};
use cl_model::inventory::CheckedPart;
use cl_model::lists::accounts::{AccountKind, CheckedAccount};
use cl_model::lists::contacts::{CheckedContact, Role, Roles, TaxIdKind};
use cl_model::lists::items::{CheckedItem, ItemKind};

/// The engine type of a list kind.
#[must_use]
pub fn api_type(kind: AccountKind) -> AccountType {
    match kind {
        AccountKind::Asset => AccountType::Asset,
        AccountKind::Liability => AccountType::Liability,
        AccountKind::Equity => AccountType::Equity,
        AccountKind::Income => AccountType::Revenue,
        AccountKind::Cogs => AccountType::Cogs,
        AccountKind::Expense => AccountType::Expense,
        AccountKind::OtherIncome => AccountType::OtherRevenue,
        AccountKind::OtherExpense => AccountType::OtherExpense,
    }
}

/// The checked account as the engine's fields. `name` is the name to
/// store (the caller keeps the engine's stored name when the owner left
/// the shown one as it was, Q-202).
#[must_use]
pub fn account_fields(checked: &CheckedAccount, name: String) -> AccountFields {
    AccountFields {
        name,
        account_number: checked.number.clone(),
        account_type: api_type(checked.kind),
        detail_type: checked.detail.clone(),
        description: checked.description.clone(),
        parent_id: checked.parent,
    }
}

/// The engine contact type of the form's roles.
#[must_use]
pub fn api_roles(r: Roles) -> ContactType {
    match r {
        Roles::Client => ContactType::Customer,
        Roles::Vendor => ContactType::Vendor,
        Roles::Both => ContactType::Both,
    }
}

/// The engine's TIN type of the form's tax ID kind.
#[must_use]
pub fn tin_of(k: Option<TaxIdKind>) -> Option<TinType> {
    k.map(|k| match k {
        TaxIdKind::Ssn => TinType::Ssn,
        TaxIdKind::Ein => TinType::Ein,
    })
}

/// Engine fields for a checked contact, keeping what the form does not
/// edit (countries, default tag) and, for a contact without the vendor
/// role, its vendor fields as they were.
#[must_use]
pub fn contact_fields(checked: &CheckedContact, existing: Option<&ContactFields>) -> ContactFields {
    let mut f = existing
        .cloned()
        .unwrap_or_else(|| ContactFields::named(api_roles(checked.roles), checked.name.clone()));
    f.contact_type = api_roles(checked.roles);
    f.display_name.clone_from(&checked.name);
    f.company_name.clone_from(&checked.company);
    f.first_name.clone_from(&checked.first);
    f.last_name.clone_from(&checked.last);
    f.email.clone_from(&checked.email);
    f.phone.clone_from(&checked.phone);
    let [l1, l2, city, state, zip] = checked.billing.clone();
    (
        f.billing_line1,
        f.billing_line2,
        f.billing_city,
        f.billing_state,
        f.billing_zip,
    ) = (l1, l2, city, state, zip);
    let [l1, l2, city, state, zip] = checked.shipping.clone();
    (
        f.shipping_line1,
        f.shipping_line2,
        f.shipping_city,
        f.shipping_state,
        f.shipping_zip,
    ) = (l1, l2, city, state, zip);
    f.default_payment_terms.clone_from(&checked.terms);
    f.notes.clone_from(&checked.notes);
    if checked.roles.has(Role::Vendor) {
        f.default_expense_account_id = checked.expense_account;
        f.vendor_account_number.clone_from(&checked.vendor_account);
        f.is1099_eligible = checked.track_1099;
        // An empty field keeps the tax ID on file unless it was removed.
        if checked.tax_id.is_some() || checked.clear_tax_id || existing.is_none() {
            f.tax_id.clone_from(&checked.tax_id);
        }
    }
    f
}

/// The 1099 profile change a vendor's save sends: only what changed from
/// `before` (the tax ID kind and backup withholding as the form opened).
#[must_use]
pub fn vendor_1099_update(
    checked: &CheckedContact,
    before: (Option<TaxIdKind>, bool),
) -> Vendor1099Update {
    let vendor = checked.roles.has(Role::Vendor);
    let (kind0, backup0) = before;
    Vendor1099Update {
        tin_type: (vendor && checked.tax_id_kind != kind0).then(|| tin_of(checked.tax_id_kind)),
        backup_withholding: (vendor && checked.backup_withholding != backup0)
            .then_some(checked.backup_withholding),
    }
}

/// The engine's item type of a kind.
#[must_use]
pub fn item_type(k: ItemKind) -> cl_api::ItemType {
    match k {
        ItemKind::Service => cl_api::ItemType::Service,
        ItemKind::NonInventory => cl_api::ItemType::NonInventory,
        ItemKind::OtherCharge => cl_api::ItemType::OtherCharge,
        ItemKind::Discount => cl_api::ItemType::Discount,
        ItemKind::Subtotal => cl_api::ItemType::Subtotal,
        ItemKind::InventoryPart => cl_api::ItemType::InventoryPart,
    }
}

/// The opening stock a new part is saved with.
#[must_use]
pub fn opening_stock(part: &CheckedPart) -> Option<cl_api::OpeningStock> {
    part.opening.map(|o| cl_api::OpeningStock {
        quantity: o.quantity,
        value: o.value,
        as_of: o.as_of,
    })
}

/// The checked item and its purchase side as the engine's fields, field by
/// field (the price and cost are the owner's typed values).
#[must_use]
pub fn item_fields(checked: CheckedItem, part: CheckedPart) -> ItemFields {
    ItemFields {
        purchase: cl_api::ItemPurchase {
            purchase_description: part.purchase_description,
            purchase_cost: part.cost,
            expense_account_id: part.expense_account,
            asset_account_id: part.asset_account,
            cogs_account_id: part.cogs_account,
            preferred_vendor_id: part.preferred_vendor,
            reorder_point: part.reorder_point,
            reorder_quantity: part.reorder_quantity,
            unit_label: part.unit_label,
            unit_set_id: part.unit_set,
            default_purchase_unit_id: part.purchase_unit,
            default_sale_unit_id: part.sale_unit,
        },
        name: checked.name,
        description: checked.description,
        unit_price: checked.price,
        income_account_id: checked.income_account,
        is_taxable: checked.taxable,
        item_type: item_type(checked.kind),
        discount_percent: checked
            .discount_percent
            .and_then(|q| cl_api::SignedPercent::parse(&q.to_wire()).ok()),
        tax_code_id: checked.tax_code,
    }
}
