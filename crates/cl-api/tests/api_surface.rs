//! The typed API can never send `overrideConfirmed` or `lockDate`
//! (NN8; P05 plan §2.7, S3, missing test 10).
//!
//! 1. A source scan: no code line in `src/` names `overrideConfirmed`, and
//!    `lockDate` appears only in read models (the 423 decoder and the
//!    read-only company settings) and their tests.
//! 2. A compile-time check: the read models that hold a lock date do not
//!    implement `Serialize`.
//! 3. Every public request type, serialized, has neither key at any depth.

use std::path::Path;

use cl_api::{
    AccountFields, AccountType, AccountUpdate, ClosedPeriod, CompanyProfileUpdate, CompanySettings,
    ContactFields, ContactType, ContactUpdate, ItemFields, ItemUpdate, NewCompany, NewContact,
    SmtpTest, TagFields, TagUpdate,
};
use cl_core::ids::AccountId;
use cl_core::{Money, Secret};
use serde::Serialize;
use serde_json::Value;

const FORBIDDEN: [&str; 2] = ["overrideConfirmed", "lockDate"];

fn rs_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("src dir") {
        let p = entry.expect("entry").path();
        if p.is_dir() {
            rs_files(&p, out);
        } else if p.extension().is_some_and(|e| e == "rs") {
            out.push(p);
        }
    }
}

#[test]
fn no_code_line_names_the_override_and_lock_date_is_read_only() {
    let mut files = Vec::new();
    rs_files(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut files,
    );
    assert!(files.len() > 10, "scanned {}", files.len());
    let read_models = ["errors.rs", "company.rs"];
    for f in files {
        let name = f
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        // Test-only modules assert the keys' absence and so name them.
        if name.ends_with("_tests.rs") || name == "testkit.rs" {
            continue;
        }
        let text = std::fs::read_to_string(&f).expect("read");
        for (i, line) in text.lines().enumerate() {
            let code = line.trim_start();
            if code.starts_with("//") {
                continue;
            }
            for bad in [
                "overrideConfirmed",
                "override_confirmed",
                "overrideconfirmed",
            ] {
                assert!(!code.contains(bad), "{name}:{} names {bad}", i + 1);
            }
            if code.contains("lockDate") || code.contains("lock_date") {
                assert!(
                    read_models.contains(&name.as_str()),
                    "{name}:{} mentions the closing date outside a read model",
                    i + 1
                );
            }
        }
    }
}

/// `assert_not_impl!(T: Serialize)`: if `T` implemented `Serialize`, the
/// `_` below would be ambiguous and this file would not compile.
trait AmbiguousIfSerialize<A> {
    fn check() {}
}
impl<T: ?Sized> AmbiguousIfSerialize<()> for T {}
struct Marker;
impl<T: ?Sized + Serialize> AmbiguousIfSerialize<Marker> for T {}

#[test]
fn lock_date_read_models_cannot_be_serialized() {
    <CompanySettings as AmbiguousIfSerialize<_>>::check();
    <ClosedPeriod as AmbiguousIfSerialize<_>>::check();
}

fn keys(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::Object(m) => {
            for (k, v) in m {
                out.push(k.clone());
                keys(v, out);
            }
        }
        Value::Array(a) => a.iter().for_each(|v| keys(v, out)),
        _ => {}
    }
}

fn assert_clean<T: Serialize>(what: &str, v: &T) {
    let mut ks = Vec::new();
    keys(&serde_json::to_value(v).expect("serializes"), &mut ks);
    assert!(!ks.is_empty(), "{what} has fields");
    for bad in FORBIDDEN {
        assert!(!ks.iter().any(|k| k == bad), "{what} serializes {bad}");
    }
}

#[test]
fn every_request_type_omits_both_keys() {
    let account = AccountFields {
        name: "n".into(),
        account_number: Some("1".into()),
        account_type: AccountType::Asset,
        detail_type: None,
        description: None,
        parent_id: Some(AccountId::parse("2a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01").expect("id")),
    };
    assert_clean("AccountFields", &account);
    assert_clean(
        "AccountUpdate",
        &AccountUpdate {
            fields: account,
            is_active: false,
        },
    );
    let contact = ContactFields::named(ContactType::Both, "c");
    assert_clean(
        "NewContact",
        &NewContact {
            fields: contact.clone(),
            opening_balance: Money::ZERO,
            opening_balance_date: None,
        },
    );
    assert_clean(
        "ContactUpdate",
        &ContactUpdate {
            fields: contact,
            is_active: true,
        },
    );
    let item = ItemFields {
        name: "i".into(),
        description: None,
        unit_price: Some(Money::ZERO),
        income_account_id: AccountId::parse("2a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01").expect("id"),
        is_taxable: true,
    };
    assert_clean("ItemFields", &item);
    assert_clean(
        "ItemUpdate",
        &ItemUpdate {
            fields: item,
            is_active: true,
        },
    );
    let tag = TagFields {
        name: "t".into(),
        color: None,
        group_id: None,
        description: None,
    };
    assert_clean("TagFields", &tag);
    assert_clean(
        "TagUpdate",
        &TagUpdate {
            fields: tag,
            is_active: true,
        },
    );
    assert_clean(
        "CompanyProfileUpdate",
        &CompanyProfileUpdate {
            business_name: "b".into(),
            ..CompanyProfileUpdate::default()
        },
    );
    assert_clean(
        "SmtpTest",
        &SmtpTest {
            host: "h".into(),
            port: 25,
            username: String::new(),
            password: Some(Secret::new("p")),
            from: "f@example.test".into(),
            from_name: Some("n".into()),
            test_email: Some("t@example.test".into()),
        },
    );
    assert_clean(
        "NewCompany",
        &NewCompany {
            company_name: "c".into(),
            business_type: Some("default".into()),
            entity_type: Some("sole_prop".into()),
            system_accounts_only: None,
        },
    );
}
