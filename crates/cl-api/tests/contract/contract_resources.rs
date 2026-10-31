//! Accounts, contacts, items and tags: create, get, update, list with
//! filters, deactivate/delete (AC1). Names carry a random suffix and every
//! list is filtered, so nothing depends on what other tests created.

use cl_api::{
    AccountFilter, AccountType, AccountUpdate, ApiError, ContactFields, ContactFilter, ContactType,
    ContactUpdate, ItemFields, ItemFilter, ItemUpdate, NewContact, PageRequest, TagFields,
    TagFilter, TagUpdate,
};
use cl_core::Money;

use crate::common::{account, account_fields, key, m};
use crate::support::{Harness, unique};

pub fn accounts(h: &Harness) {
    let s = &h.s1;
    let mut fields = account_fields(AccountType::Expense, "Postage");
    fields.account_number = Some(format!("6{}", &fields.name[fields.name.len() - 4..]));
    fields.description = Some("contract".into());
    let made = h.run(s.accounts().create(&fields, key())).expect("create");
    assert_eq!(made.record.fields, fields);
    assert!(made.record.is_active);

    let got = h.run(s.accounts().get(made.record.id)).expect("get");
    assert_eq!(got.record.fields, fields);
    assert_eq!(got.version, made.version);

    let mut renamed = fields.clone();
    renamed.name = format!("{} renamed", fields.name);
    let upd = AccountUpdate {
        fields: renamed.clone(),
        is_active: true,
    };
    let saved = h
        .run(
            s.accounts()
                .update(made.record.id, &got.version, &upd, key()),
        )
        .expect("update");
    assert_eq!(saved.record.fields.name, renamed.name);

    let filter = AccountFilter {
        types: vec![AccountType::Expense],
        search: Some(renamed.name.clone()),
        ..AccountFilter::default()
    };
    let page = h
        .run(s.accounts().list(&filter, PageRequest::first_max()))
        .expect("list");
    assert_eq!(
        page.data.len(),
        1,
        "search must find exactly the new account"
    );
    assert_eq!(page.data[0].id, made.record.id);
    let other_type = AccountFilter {
        types: vec![AccountType::Asset],
        ..filter.clone()
    };
    let none = h
        .run(s.accounts().list(&other_type, PageRequest::first_max()))
        .expect("list");
    assert_eq!(none.data.len(), 0);

    let off = h
        .run(s.accounts().deactivate(made.record.id, key()))
        .expect("deactivate");
    assert!(!off.record.is_active);
    let inactive = AccountFilter {
        is_active: Some(false),
        ..filter
    };
    let all = h.run(s.accounts().list_all(&inactive)).expect("list all");
    assert!(all.iter().any(|a| a.id == made.record.id));
}

pub fn contacts(h: &Harness) {
    let s = &h.s1;
    let mut fields = ContactFields::named(ContactType::Vendor, unique("Acme Supply"));
    fields.email = Some("billing@example.com".into());
    fields.billing_city = Some("Springfield".into());
    // The engine fills both countries with "US" when they are missing.
    fields.billing_country = Some("US".into());
    fields.shipping_country = Some("US".into());
    let new = NewContact {
        fields: fields.clone(),
        opening_balance: Money::ZERO,
        opening_balance_date: None,
    };
    let made = h.run(s.contacts().create(&new, key())).expect("create");
    assert_eq!(made.record.fields, fields);
    let got = h.run(s.contacts().get(made.record.id)).expect("get");
    assert_eq!(got.record.fields, fields);
    assert_eq!(got.version, made.version);

    let mut changed = fields.clone();
    changed.phone = Some("555-0101".into());
    let saved = h
        .run(s.contacts().update(
            made.record.id,
            &got.version,
            &ContactUpdate {
                fields: changed.clone(),
                is_active: true,
            },
            key(),
        ))
        .expect("update");
    assert_eq!(saved.record.fields, changed);

    let filter = ContactFilter {
        contact_type: Some(ContactType::Vendor),
        search: Some(fields.display_name.clone()),
        ..ContactFilter::default()
    };
    let page = h
        .run(s.contacts().list(&filter, PageRequest::first_max()))
        .expect("list");
    assert_eq!(page.data.len(), 1);
    assert_eq!(page.data[0].id, made.record.id);
    let customers = ContactFilter {
        contact_type: Some(ContactType::Customer),
        ..filter.clone()
    };
    let none = h
        .run(s.contacts().list(&customers, PageRequest::first_max()))
        .expect("list");
    assert_eq!(none.data.len(), 0);

    let off = h
        .run(s.contacts().deactivate(made.record.id, key()))
        .expect("deactivate");
    assert!(!off.record.is_active);
}

pub fn items(h: &Harness) {
    let s = &h.s1;
    let income = account(h, s, AccountType::Revenue, "Sales");
    let fields = ItemFields {
        name: unique("Widget"),
        description: Some("contract item".into()),
        unit_price: Some(m("19.9999")),
        income_account_id: income.id,
        is_taxable: true,
        item_type: cl_api::ItemType::Service,
        discount_percent: None,
    };
    let made = h.run(s.items().create(&fields, key())).expect("create");
    assert_eq!(made.record.fields, fields);
    let got = h.run(s.items().get(made.record.id)).expect("get");
    assert_eq!(got.record.fields, fields);
    assert_eq!(
        got.record.fields.unit_price.map(Money::to_wire).as_deref(),
        Some("19.9999")
    );

    let mut changed = fields.clone();
    changed.unit_price = Some(m("0.0001"));
    changed.is_taxable = false;
    let saved = h
        .run(s.items().update(
            made.record.id,
            &got.version,
            &ItemUpdate {
                fields: changed.clone(),
                is_active: true,
            },
            key(),
        ))
        .expect("update");
    assert_eq!(saved.record.fields, changed);

    let filter = ItemFilter {
        search: Some(fields.name.clone()),
        is_taxable: Some(false),
        ..ItemFilter::default()
    };
    let page = h
        .run(s.items().list(&filter, PageRequest::first_max()))
        .expect("list");
    assert_eq!(page.data.len(), 1);
    let taxable = ItemFilter {
        is_taxable: Some(true),
        ..filter.clone()
    };
    let none = h
        .run(s.items().list(&taxable, PageRequest::first_max()))
        .expect("list");
    assert_eq!(none.data.len(), 0);

    h.run(s.items().deactivate(made.record.id, key()))
        .expect("deactivate");
    let after = h.run(s.items().get(made.record.id)).expect("get");
    assert!(!after.record.is_active);
}

pub fn tags(h: &Harness) {
    let s = &h.s1;
    let fields = TagFields {
        name: unique("Project"),
        color: Some("#336699".into()),
        group_id: None,
        description: Some("contract".into()),
        tag_number: None,
    };
    let made = h.run(s.tags().create(&fields, key())).expect("create");
    assert_eq!(made.record.fields, fields);
    let got = h.run(s.tags().get(made.record.id)).expect("get");
    assert_eq!(got.record.fields, fields);

    let mut changed = fields.clone();
    changed.description = Some("changed".into());
    let saved = h
        .run(s.tags().update(
            made.record.id,
            &got.version,
            &TagUpdate {
                fields: changed.clone(),
                is_active: true,
            },
            key(),
        ))
        .expect("update");
    assert_eq!(saved.record.fields, changed);

    let filter = TagFilter {
        search: Some(fields.name.clone()),
        is_active: Some(true),
        ..TagFilter::default()
    };
    let page = h
        .run(s.tags().list(&filter, PageRequest::first_max()))
        .expect("list");
    assert_eq!(page.data.len(), 1);
    assert_eq!(page.data[0].id, made.record.id);

    h.run(s.tags().delete(made.record.id, key()))
        .expect("delete");
    let gone = h.run(s.tags().get(made.record.id)).expect_err("deleted");
    assert!(matches!(gone, ApiError::NotFound { .. }), "{gone:?}");
}
