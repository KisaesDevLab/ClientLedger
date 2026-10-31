//! List import and export (P11 step 6): the engine's per-list CSV exports,
//! the account and contact imports the desktop's column-mapping step sends,
//! and the gaps U-43 (editable terms and payment methods) and U-46 (the
//! item import) as capability probes.

use cl_api::{
    AccountImportRow, AccountType, Capabilities, ContactImportRow, ContactType, ItemFields, Method,
};
use serde_json::json;

use crate::common::{account, key, m};
use crate::support::{Harness, unique};

/// An account number nobody else uses (digits, ≤ 20).
fn fresh_number() -> String {
    let hex = unique("");
    let digits: String = hex
        .trim()
        .bytes()
        .map(|b| char::from(b'0' + b % 10))
        .collect();
    format!("8{}", &digits[..9])
}

/// Accounts import in one go (a number already used is skipped, not an
/// error); contacts import with the chosen role; every list exports as CSV
/// text with the engine's exact decimal strings.
pub fn exports_and_imports(h: &Harness) {
    let s = &h.s1;
    let number = fresh_number();
    let rows = vec![
        AccountImportRow {
            account_number: Some(number.clone()),
            name: unique("Imported fuel"),
            account_type: AccountType::Expense,
            detail_type: Some("utilities".into()),
        },
        AccountImportRow {
            account_number: None,
            name: unique("Imported misc"),
            account_type: AccountType::OtherExpense,
            detail_type: None,
        },
    ];
    let r = h
        .run(s.accounts().import(&rows, false, key()))
        .expect("import");
    assert_eq!((r.imported, r.updated, r.skipped.len()), (2, 0, 0));
    let again = h
        .run(s.accounts().import(&rows[..1], false, key()))
        .expect("again");
    assert_eq!((again.imported, again.skipped.len()), (0, 1));
    let text = h.run(s.accounts().export_csv()).expect("export");
    assert!(text.starts_with("Account Number,Name,Type"), "{text:.80}");
    assert!(text.contains(&number));

    let vendor = unique("Imported vendor");
    let imported = h
        .run(s.contacts().import(
            &[ContactImportRow {
                display_name: vendor.clone(),
                company_name: Some("Supply Co".into()),
                email: Some("orders@example.com".into()),
                phone: Some("555-0100".into()),
            }],
            ContactType::Vendor,
            key(),
        ))
        .expect("contacts");
    assert_eq!(imported.imported, 1);
    let vendors = h
        .run(s.contacts().export_csv(Some(ContactType::Vendor)))
        .expect("vendors");
    assert!(vendors.contains(&vendor));
    assert!(vendors.contains("orders@example.com"));
    let clients = h
        .run(s.contacts().export_csv(Some(ContactType::Customer)))
        .expect("clients");
    assert!(!clients.contains(&vendor));

    let income = account(h, s, AccountType::Revenue, "Imported sales");
    let name = unique("Exported item");
    h.run(s.items().create(
        &ItemFields {
            name: name.clone(),
            description: None,
            unit_price: Some(m("45.0450")),
            income_account_id: income.id,
            is_taxable: true,
            item_type: cl_api::ItemType::Service,
            discount_percent: None,
        },
        key(),
    ))
    .expect("item");
    let items = h.run(s.items().export_csv()).expect("items");
    let line = items
        .lines()
        .find(|l| l.contains(&name))
        .expect("the item is exported");
    assert!(line.contains("45.0450"), "price exact: {line}");
}

/// U-46: the item import takes the income account by name but stores an
/// id, so today it fails; the desktop creates items one at a time.
pub fn items_import_u46(h: &Harness) {
    let s = &h.s1;
    let income = account(h, s, AccountType::Revenue, "Import target");
    let raw = h
        .run(s.raw(
            Method::Post,
            "/api/v1/items/import",
            &[],
            Some(&json!({
                "items": [{
                    "name": unique("Imported item"),
                    "unitPrice": "12.5000",
                    "incomeAccountName": income.fields.name,
                }]
            })),
        ))
        .expect("raw");
    if h.require_cap(Capabilities::ITEMS_IMPORT, "U-46") {
        assert_eq!(raw.status, 201, "{}", raw.body);
    } else {
        assert!(
            raw.status >= 400,
            "fails today: {} {}",
            raw.status,
            raw.body
        );
    }
}

/// U-43: terms and payment methods are fixed lists today (no route).
pub fn terms_lists_u43(h: &Harness) {
    let s = &h.s1;
    let raw = h
        .run(s.raw(Method::Get, "/api/v1/payment-methods", &[], None))
        .expect("raw");
    if h.require_cap(Capabilities::TERMS_LISTS, "U-43") {
        assert_eq!(raw.status, 200, "{}", raw.body);
    } else {
        assert_eq!(raw.status, 404, "no such route today: {}", raw.body);
    }
}
