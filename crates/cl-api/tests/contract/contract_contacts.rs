//! Contacts and the client and vendor centers (P11 steps 2-3): one contact
//! in both roles (Q-77), merge, a contact's transaction history, open
//! balances from the engine's balance reports (never summed by the app),
//! the vendor 1099 profile (Q-53) and the default expense account (Q-192).
//! The engine gaps U-35..U-38 and U-44 are capability probes: each asserts
//! what the engine does today and prints `PENDING(U-xx)` until the
//! capability is advertised.

use cl_api::{
    AccountType, ApiError, BillLine, Capabilities, Contact, ContactFields, ContactFilter,
    ContactType, InvoiceLine, Method, NewBill, NewContact, NewInvoice, PageRequest, Session,
    TinType, TxnType, Vendor1099Update,
};
use cl_core::{Money, Quantity, Rate};
use serde_json::json;

use crate::common::{account, date, key, m};
use crate::support::{Harness, unique};

fn contact(h: &Harness, s: &Session, kind: ContactType, prefix: &str) -> Contact {
    h.run(s.contacts().create(
        &NewContact {
            fields: ContactFields::named(kind, unique(prefix)),
            opening_balance: Money::ZERO,
            opening_balance_date: None,
        },
        key(),
    ))
    .unwrap_or_else(|e| panic!("create contact: {e}"))
    .record
}

fn bill(
    h: &Harness,
    s: &Session,
    vendor: &Contact,
    expense: cl_core::ids::AccountId,
    amount: &str,
) {
    h.run(s.create_bill(
        &NewBill {
            contact_id: vendor.id,
            txn_date: date(2026, 7, 1),
            due_date: Some(date(2026, 7, 31)),
            vendor_invoice_number: Some(unique("B")),
            memo: None,
            lines: vec![BillLine {
                account_id: expense,
                description: None,
                amount: m(amount),
            }],
        },
        key(),
    ))
    .expect("bill");
}

/// Q-77: a contact of type "both" is listed among clients and vendors.
pub fn both_roles(h: &Harness) {
    let s = &h.s1;
    let both = contact(h, s, ContactType::Both, "Both ways");
    for kind in [ContactType::Customer, ContactType::Vendor] {
        let filter = ContactFilter {
            contact_type: Some(kind),
            search: Some(both.fields.display_name.clone()),
            ..ContactFilter::default()
        };
        let page = h
            .run(s.contacts().list(&filter, PageRequest::first_max()))
            .expect("list");
        assert_eq!(page.data.len(), 1, "{kind:?}");
        assert_eq!(page.data[0].id, both.id);
    }
}

/// Q-192: the vendor's default expense account is kept.
pub fn default_expense_account(h: &Harness) {
    let s = &h.s1;
    let expense = account(h, s, AccountType::Expense, "Fuel");
    let mut fields = ContactFields::named(ContactType::Vendor, unique("Fuel stop"));
    fields.default_expense_account_id = Some(expense.id);
    let made = h
        .run(s.contacts().create(
            &NewContact {
                fields,
                opening_balance: Money::ZERO,
                opening_balance_date: None,
            },
            key(),
        ))
        .expect("vendor");
    let got = h.run(s.contacts().get(made.record.id)).expect("get");
    assert_eq!(
        got.record.fields.default_expense_account_id,
        Some(expense.id)
    );
}

/// History, open balance and merge: the vendor's bill shows in its history
/// and in the vendor aging total; merging moves the bill to the kept
/// vendor and makes the duplicate inactive.
pub fn history_balance_and_merge(h: &Harness) {
    let s = &h.s1;
    let expense = account(h, s, AccountType::Expense, "Gravel");
    let dup = contact(h, s, ContactType::Vendor, "Galena Gravel dup");
    let kept = contact(h, s, ContactType::Vendor, "Galena Gravel");
    bill(h, s, &dup, expense.id, "212.3400");

    let history = h
        .run(s.contacts().transactions(dup.id, PageRequest::first_max()))
        .expect("history");
    assert_eq!(history.total, 1);
    assert_eq!(history.data[0].txn_type, TxnType::Bill);
    assert_eq!(
        history.data[0].total.map(Money::to_wire).as_deref(),
        Some("212.3400")
    );

    let aging = h
        .run(s.reports().ap_aging_summary(date(2026, 8, 15)))
        .expect("aging");
    let open = aging.total_of(dup.id).expect("the vendor is listed");
    assert_eq!(open.display(2).as_deref(), Some("212.34"));
    assert!(aging.total_of(kept.id).is_none(), "nothing open yet");

    let merged = h
        .run(s.contacts().merge(dup.id, kept.id, key()))
        .expect("merge");
    assert_eq!(merged.record.id, kept.id);
    let moved = h
        .run(s.contacts().transactions(kept.id, PageRequest::first_max()))
        .expect("kept history");
    assert_eq!(moved.total, 1, "the bill moved");
    let gone = h.run(s.contacts().get(dup.id)).expect("dup");
    assert!(!gone.record.is_active, "the duplicate is inactive");
    let e = h
        .run(s.contacts().merge(kept.id, kept.id, key()))
        .expect_err("self merge");
    assert!(matches!(e, ApiError::Local(_)), "{e:?}");
}

/// A client's open balance comes from the client balance summary, exact.
pub fn client_open_balance(h: &Harness) {
    let s = &h.s1;
    let income = account(h, s, AccountType::Revenue, "Mowing");
    let client = contact(h, s, ContactType::Customer, "Riverbend HOA");
    let idle = contact(h, s, ContactType::Customer, "Idle client");
    h.run(s.create_invoice(
        &NewInvoice {
            txn_date: date(2026, 7, 2),
            due_date: Some(date(2026, 8, 1)),
            contact_id: client.id,
            lines: vec![InvoiceLine {
                account_id: income.id,
                description: None,
                quantity: Quantity::parse_wire("3").expect("q"),
                unit_price: m("41.1100"),
                is_taxable: false,
                tax_rate: Rate::parse_wire("0").expect("r"),
            }],
            memo: None,
            txn_number: None,
        },
        key(),
    ))
    .expect("invoice");
    let summary = h
        .run(s.reports().customer_balance_summary())
        .expect("summary");
    let open = summary.balance_of(client.id).expect("listed");
    assert_eq!(open.display(4).as_deref(), Some("123.3300"));
    assert!(summary.balance_of(idle.id).is_none(), "zero is left out");
}

/// Q-53: the vendor's 1099 profile round-trips tax ID type and backup
/// withholding; the tax ID on the contact never comes back unmasked from
/// the profile.
pub fn vendor_1099_profile(h: &Harness) {
    let s = &h.s1;
    let mut fields = ContactFields::named(ContactType::Vendor, unique("Crane Creek Tree"));
    fields.is1099_eligible = true;
    fields.tax_id = Some("12-3456789".into());
    let v = h
        .run(s.contacts().create(
            &NewContact {
                fields,
                opening_balance: Money::ZERO,
                opening_balance_date: None,
            },
            key(),
        ))
        .expect("vendor")
        .record;
    let before = h.run(s.form1099().profile(v.id)).expect("profile");
    assert!(before.is1099_eligible);
    assert!(!before.w9_on_file);
    assert!(!before.backup_withholding);
    h.run(s.form1099().update_profile(
        v.id,
        &Vendor1099Update {
            tin_type: Some(Some(TinType::Ein)),
            backup_withholding: Some(true),
        },
        key(),
    ))
    .expect("update");
    let after = h.run(s.form1099().profile(v.id)).expect("profile");
    assert_eq!(after.tin_type, Some(TinType::Ein));
    assert!(after.backup_withholding);
    assert!(
        after
            .tin_masked
            .as_deref()
            .is_none_or(|t| !t.contains("3456789")),
        "never a full tax ID"
    );
}

/// U-35: a client has no jobs today (`parentId` on a contact is dropped).
pub fn jobs_u35(h: &Harness) {
    let s = &h.s1;
    let client = contact(h, s, ContactType::Customer, "Job parent");
    let raw = h
        .run(s.raw(
            Method::Post,
            "/api/v1/contacts",
            &[],
            Some(&json!({
                "contactType": "customer",
                "displayName": unique("Spring cleanup"),
                "parentId": client.id,
                "jobStatus": "in_progress",
            })),
        ))
        .expect("raw");
    assert_eq!(raw.status, 201, "{}", raw.body);
    let made = &raw.body["contact"];
    if h.require_cap(Capabilities::CONTACT_JOBS, "U-35") {
        assert_eq!(made["parentId"], json!(client.id));
    } else {
        assert!(made["parentId"].is_null(), "not stored today: {made}");
    }
}

/// U-36: one ship-to address only; no address or people lists today.
pub fn addresses_u36(h: &Harness) {
    let s = &h.s1;
    let c = contact(h, s, ContactType::Customer, "Ship-to");
    let raw = h
        .run(s.raw(
            Method::Get,
            &format!("/api/v1/contacts/{}/addresses", c.id),
            &[],
            None,
        ))
        .expect("raw");
    if h.require_cap(Capabilities::CONTACT_ADDRESSES, "U-36") {
        assert_eq!(raw.status, 200, "{}", raw.body);
    } else {
        assert_eq!(raw.status, 404, "no such route today");
    }
}

/// U-37: one free-text notes field only; no dated notes today.
pub fn notes_u37(h: &Harness) {
    let s = &h.s1;
    let c = contact(h, s, ContactType::Customer, "Notes");
    let raw = h
        .run(s.raw(
            Method::Get,
            &format!("/api/v1/contacts/{}/notes", c.id),
            &[],
            None,
        ))
        .expect("raw");
    if h.require_cap(Capabilities::CONTACT_NOTES, "U-37") {
        assert_eq!(raw.status, 200, "{}", raw.body);
    } else {
        assert_eq!(raw.status, 404, "no such route today");
    }
}

/// U-38: a contact read carries no open balance today; the centers read it
/// from the balance reports.
pub fn open_balance_u38(h: &Harness) {
    let s = &h.s1;
    let c = contact(h, s, ContactType::Customer, "Balance field");
    let raw = h
        .run(s.raw(
            Method::Get,
            &format!("/api/v1/contacts/{}", c.id),
            &[],
            None,
        ))
        .expect("raw");
    assert_eq!(raw.status, 200, "{}", raw.body);
    let field = &raw.body["contact"]["openBalance"];
    if h.require_cap(Capabilities::CONTACT_OPEN_BALANCE, "U-38") {
        assert!(field.is_string(), "{field}");
    } else {
        assert!(field.is_null(), "not on the contact today: {field}");
    }
}

/// U-44: the owner cannot record a paper W-9 today (`w9OnFile` is not in
/// the profile update).
pub fn w9_on_file_u44(h: &Harness) {
    let s = &h.s1;
    let v = contact(h, s, ContactType::Vendor, "Paper W-9");
    let raw = h
        .run(s.raw(
            Method::Put,
            &format!("/api/v1/practice/1099/vendors/{}/profile", v.id),
            &[],
            Some(&json!({"w9OnFile": true})),
        ))
        .expect("raw");
    assert_eq!(raw.status, 200, "{}", raw.body);
    let after = h.run(s.form1099().profile(v.id)).expect("profile");
    if h.require_cap(Capabilities::VENDOR_W9_ON_FILE, "U-44") {
        assert!(after.w9_on_file);
    } else {
        assert!(!after.w9_on_file, "dropped today");
    }
}
