//! The demo "engine" for clients and vendors (P11): the sample company's
//! client and vendor names as engine-shaped contacts, the open balances
//! the engine's balance reports would give, each contact's transaction
//! history and vendors' 1099 profiles. Synthetic data only; the figures
//! are the fixture's own, worked out here because there is no engine in
//! the demo (the real screen reads them from the engine and never adds
//! amounts up).

use std::collections::BTreeMap;

use cl_api::{
    ApAgingSummary, Contact, ContactFields, ContactType, CustomerBalanceSummary,
    TransactionSummary, TxnStatus, TxnType, Vendor1099Profile,
};
use cl_core::Money;
use cl_core::ids::{ContactId, TransactionId};
use serde_json::json;

use super::{DemoState, account_id};

/// Receivables and payables accounts of the sample fixture.
const AR: &str = "10200";
const AP: &str = "20100";

/// The client that is also a vendor in the demo (Q-77).
pub const BOTH: &str = "Reeds Spring Hardware";
/// The inactive client of the demo.
pub const INACTIVE: &str = "Powersite Dam Tours";

/// A demo contact id from its position (stable across runs).
#[must_use]
pub fn contact_id(n: u32) -> ContactId {
    ContactId::parse(&format!("d0000000-0000-4000-8000-c{n:011}"))
        .unwrap_or_else(|_| unreachable!("a well-formed demo id"))
}

fn txn_id(n: u32) -> TransactionId {
    TransactionId::parse(&format!("d0000000-0000-4000-8000-b{n:011}"))
        .unwrap_or_else(|_| unreachable!("a well-formed demo id"))
}

fn named(kind: ContactType, name: &str) -> ContactFields {
    let mut f = ContactFields::named(kind, name);
    f.billing_country = Some("US".into());
    f.shipping_country = Some("US".into());
    f
}

/// The demo contacts: the sample's clients (one also a vendor, one
/// inactive) and vendors, a few with contact details filled in.
#[must_use]
pub fn contacts() -> Vec<Contact> {
    let sample = cl_company::Sample::load();
    let mut out = Vec::new();
    let mut n = 0u32;
    let mut push = |fields: ContactFields, active: bool| {
        n += 1;
        out.push(Contact {
            id: contact_id(n),
            fields,
            is_active: active,
            opening_balance: Some(Money::ZERO),
            opening_balance_date: None,
            created_at: Some(DemoState::now()),
            updated_at: Some(DemoState::now()),
        });
    };
    for name in &sample.company.clients {
        let kind = if name == BOTH {
            ContactType::Both
        } else {
            ContactType::Customer
        };
        let mut f = named(kind, name);
        if name == "Riverbend HOA" {
            f.company_name = Some("Riverbend Homeowners Association".into());
            f.first_name = Some("Dana".into());
            f.last_name = Some("Whitfield".into());
            f.email = Some("board@riverbend-hoa.example".into());
            f.phone = Some("417-555-0142".into());
            f.billing_line1 = Some("200 Riverbend Pkwy".into());
            f.billing_city = Some("Hollister".into());
            f.billing_state = Some("MO".into());
            f.billing_zip = Some("65672".into());
            f.shipping_line1 = Some("Clubhouse, 14 Heron Ct".into());
            f.shipping_city = Some("Hollister".into());
            f.shipping_state = Some("MO".into());
            f.shipping_zip = Some("65672".into());
            f.default_payment_terms = Some("net_30".into());
            f.notes = Some(
                "Mow every Tuesday from April through October. Gate code at the clubhouse.".into(),
            );
        }
        if name == BOTH {
            f.phone = Some("417-555-0187".into());
            f.default_payment_terms = Some("net_15".into());
            f.default_expense_account_id = Some(account_id(5200));
        }
        push(f, name != INACTIVE);
    }
    for name in &sample.company.vendors {
        let mut f = named(ContactType::Vendor, name);
        match name.as_str() {
            "Hollister Fuel Stop" => {
                f.default_expense_account_id = Some(account_id(6100));
                f.phone = Some("417-555-0110".into());
            }
            "Crane Creek Tree Service" => {
                f.default_expense_account_id = Some(account_id(5000));
                f.is1099_eligible = true;
                f.tax_id = Some("00-0004521".into());
                f.email = Some("office@cranecreektree.example".into());
                f.default_payment_terms = Some("net_30".into());
            }
            "Galena Gravel" => {
                f.default_expense_account_id = Some(account_id(5200));
                f.default_payment_terms = Some("due_on_receipt".into());
            }
            _ => {}
        }
        push(f, true);
    }
    out
}

fn original_name(state: &DemoState, id: ContactId) -> Option<String> {
    state.contact_names.get(&id).cloned()
}

fn kind(t: &str) -> TxnType {
    match t {
        "invoice" => TxnType::Invoice,
        "payment" => TxnType::CustomerPayment,
        "bill" => TxnType::Bill,
        "bill_payment" => TxnType::BillPayment,
        "deposit" => TxnType::Deposit,
        "journal_entry" => TxnType::JournalEntry,
        _ => TxnType::Expense,
    }
}

/// The fixture names that mean contact `id` now (renames keep the
/// fixture's original name; merges add the merged contacts' names).
fn fixture_names(state: &DemoState, contacts: &[Contact], id: ContactId) -> Vec<String> {
    let mut ids = vec![id];
    let mut frontier = vec![id];
    while let Some(t) = frontier.pop() {
        for (s, into) in &state.merged {
            if *into == t && !ids.contains(s) {
                ids.push(*s);
                frontier.push(*s);
            }
        }
    }
    let mut names = Vec::new();
    for i in ids {
        names.extend(original_name(state, i));
    }
    if names.is_empty() {
        names.extend(
            contacts
                .iter()
                .find(|c| c.id == id)
                .map(|c| c.fields.display_name.clone()),
        );
    }
    names
}

/// The contact's transactions, newest first (at most `limit`), as the
/// engine's contact history would list them.
#[must_use]
pub fn history(state: &DemoState, id: ContactId, limit: usize) -> (Vec<TransactionSummary>, u64) {
    let contacts = state.contacts.clone().unwrap_or_default();
    let names = fixture_names(state, &contacts, id);
    let sample = cl_company::Sample::load();
    let mut rows = Vec::new();
    let mut total = 0u64;
    for t in sample.transactions.iter().rev() {
        if t.date > DemoState::today() {
            continue;
        }
        let Some(c) = &t.contact else { continue };
        if !names.iter().any(|n| n == c) {
            continue;
        }
        total += 1;
        if rows.len() >= limit {
            continue;
        }
        let mut amount = Money::ZERO;
        for l in &t.lines {
            amount = amount.checked_add(l.debit).unwrap_or(amount);
        }
        rows.push(TransactionSummary {
            id: txn_id(t.id),
            txn_type: kind(&t.kind),
            txn_number: Some(t.number.clone()),
            txn_date: t.date,
            status: TxnStatus::Posted,
            contact_id: Some(id),
            contact_name: Some(c.clone()),
            memo: Some(t.memo.clone()),
            total: Some(amount),
            display_total: Some(amount),
            created_at: None,
        });
    }
    (rows, total)
}

/// What one contact owes (client, on account `AR`) or is owed (vendor, on
/// `AP`) at the story date, from the fixture.
fn open(state: &DemoState, contacts: &[Contact], id: ContactId, account: &str) -> Money {
    let names = fixture_names(state, contacts, id);
    let sample = cl_company::Sample::load();
    let mut net = Money::ZERO;
    for t in &sample.transactions {
        if t.date > DemoState::today() {
            continue;
        }
        if !t.contact.as_ref().is_some_and(|c| names.contains(c)) {
            continue;
        }
        for l in t.lines.iter().filter(|l| l.account == account) {
            let side = if account == AR {
                l.debit.checked_sub(l.credit)
            } else {
                l.credit.checked_sub(l.debit)
            };
            net = side.and_then(|s| net.checked_add(s)).unwrap_or(net);
        }
    }
    net
}

/// The client balance summary the engine would send: active clients with
/// something open.
#[must_use]
pub fn customer_balances(state: &DemoState) -> CustomerBalanceSummary {
    let contacts = state.contacts.clone().unwrap_or_default();
    let mut data = Vec::new();
    for c in contacts.iter().filter(|c| {
        c.is_active
            && matches!(
                c.fields.contact_type,
                ContactType::Customer | ContactType::Both
            )
    }) {
        let b = open(state, &contacts, c.id, AR);
        if !b.is_zero() {
            data.push(
                json!({"id": c.id, "display_name": c.fields.display_name, "balance": b.to_wire()}),
            );
        }
    }
    serde_json::from_value(json!({"title": "Customer Balance Summary", "data": data}))
        .unwrap_or_else(|_| unreachable!("the demo summary is well formed"))
}

/// The vendor aging summary the engine would send: vendors with open
/// bills, everything in the current bucket (the demo does not age bills).
#[must_use]
pub fn ap_aging(state: &DemoState) -> ApAgingSummary {
    let contacts = state.contacts.clone().unwrap_or_default();
    let mut vendors = Vec::new();
    for c in contacts.iter().filter(|c| {
        matches!(
            c.fields.contact_type,
            ContactType::Vendor | ContactType::Both
        )
    }) {
        let b = open(state, &contacts, c.id, AP);
        if !b.is_zero() {
            vendors.push(json!({
                "contact_id": c.id, "vendor_name": c.fields.display_name,
                "current": b.to_wire(), "bucket1to30": "0", "bucket31to60": "0",
                "bucket61to90": "0", "bucketOver90": "0", "total": b.to_wire(),
            }));
        }
    }
    serde_json::from_value(json!({
        "title": "AP Aging Summary",
        "asOfDate": DemoState::today().to_wire(),
        "vendors": vendors,
    }))
    .unwrap_or_else(|_| unreachable!("the demo aging is well formed"))
}

/// A vendor's 1099 profile as the demo keeps it.
#[must_use]
pub fn profile(state: &DemoState, id: ContactId) -> Vendor1099Profile {
    if let Some(p) = state.profiles.get(&id) {
        return p.clone();
    }
    let eligible = state
        .contacts
        .as_ref()
        .and_then(|cs| cs.iter().find(|c| c.id == id))
        .is_some_and(|c| c.fields.is1099_eligible);
    let crane = state
        .contact_names
        .get(&id)
        .is_some_and(|n| n == "Crane Creek Tree Service");
    serde_json::from_value(json!({
        "contactId": id,
        "is1099Eligible": eligible,
        "w9OnFile": crane,
        "w9CapturedAt": if crane { Some("2026-02-03T15:00:00.000Z") } else { None },
        "tinMasked": if crane { Some("***-**-4521") } else { None },
        "tinType": if crane { Some("EIN") } else { None },
        "backupWithholding": false,
    }))
    .unwrap_or_else(|_| unreachable!("the demo profile is well formed"))
}

/// The fixture name of every demo contact, by id (kept when a contact is
/// renamed, so its history stays with it).
#[must_use]
pub fn original_names(contacts: &[Contact]) -> BTreeMap<ContactId, String> {
    contacts
        .iter()
        .map(|c| (c.id, c.fields.display_name.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> DemoState {
        let mut s = DemoState::new();
        let cs = contacts();
        s.contact_names = original_names(&cs);
        s.contacts = Some(cs);
        s
    }

    fn id_of(s: &DemoState, name: &str) -> ContactId {
        s.contacts
            .as_ref()
            .and_then(|cs| cs.iter().find(|c| c.fields.display_name == name))
            .map_or_else(|| panic!("{name}"), |c| c.id)
    }

    #[test]
    fn the_demo_has_both_roles_an_inactive_client_and_open_balances() {
        let s = state();
        let cs = s.contacts.as_ref().expect("contacts");
        assert!(
            cs.iter()
                .any(|c| c.fields.contact_type == ContactType::Both)
        );
        assert!(cs.iter().any(|c| !c.is_active));
        let ar = customer_balances(&s);
        assert!(!ar.data.is_empty());
        let ap = ap_aging(&s);
        assert!(!ap.vendors.is_empty());
        let (rows, total) = history(&s, id_of(&s, "Galena Gravel"), 10);
        assert_eq!(rows.len(), 10);
        assert!(total >= 10);
        assert!(
            rows.windows(2).all(|w| w[0].txn_date >= w[1].txn_date),
            "newest first"
        );
    }

    #[test]
    fn a_merge_moves_history_and_balance() {
        let mut s = state();
        let a = id_of(&s, "Galena Gravel");
        let b = id_of(&s, "Bear Creek Hauling");
        let (_, before_a) = history(&s, a, 0);
        let (_, before_b) = history(&s, b, 0);
        s.merged.push((a, b));
        let (_, after_b) = history(&s, b, 0);
        assert_eq!(after_b, before_a + before_b);
        assert_eq!(profile(&s, a).contact_id, a);
    }
}
