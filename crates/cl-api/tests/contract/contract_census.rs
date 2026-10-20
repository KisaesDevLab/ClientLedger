//! Spec and shape drift guards against the live engine (P05 plan §2.8, B6,
//! AC8).
//!
//! - `live_spec_matches_fixtures`: the engine's `GET /openapi.json`, as
//!   canonical JSON, hashes to `fixtures/openapi.sha256`, and every covered
//!   operation is in it (except the two the engine serves outside the spec),
//!   with the recorded fragment hash.
//! - `field_census`: the engine's responses carry every key the typed models
//!   declare, money/quantity/rate keys are JSON strings, and the secret
//!   columns are not modelled. Keys the models do not declare are listed in
//!   the output (forward compatible, not a failure).
//!
//! The model key sets are read from the models themselves: a capturing
//! `Deserializer` records the field list serde derives for each struct. The
//! wrappers that use `#[serde(flatten)]` (serde gives no field list for
//! those) add their own few keys by hand below, next to the struct they
//! wrap.

use std::collections::BTreeSet;

use cl_api::{
    AccountFields, CompanyProfileUpdate, CompanySettings, CompanySummary, ContactFields,
    ContactType, DesktopInfo, Health, ItemFields, Method, NewContact, TagFields, Transaction,
    TransactionLine, TransactionSummary,
};
use cl_core::Money;
use serde::de::{self, DeserializeOwned, Deserializer, Visitor};
use serde_json::Value;

use crate::common::{account, date, je2, key, pair, post};
use crate::support::{Harness, unique};

// ---------------------------------------------------------- field capture

struct Captured(Vec<&'static str>);

#[derive(Debug)]
struct Stop;
impl std::fmt::Display for Stop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("field capture stopped")
    }
}
impl std::error::Error for Stop {}
impl de::Error for Stop {
    fn custom<T: std::fmt::Display>(_: T) -> Self {
        Stop
    }
}

struct Capture<'a>(&'a mut Captured);

impl<'de> Deserializer<'de> for Capture<'_> {
    type Error = Stop;

    fn deserialize_any<V: Visitor<'de>>(self, _: V) -> Result<V::Value, Stop> {
        Err(Stop)
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        fields: &'static [&'static str],
        _visitor: V,
    ) -> Result<V::Value, Stop> {
        self.0.0 = fields.to_vec();
        Err(Stop)
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string bytes
        byte_buf option unit unit_struct newtype_struct seq tuple tuple_struct map
        enum identifier ignored_any
    }
}

/// The wire names serde derived for `T`'s own fields.
fn fields_of<T: DeserializeOwned>() -> BTreeSet<String> {
    let mut captured = Captured(Vec::new());
    let _ = T::deserialize(Capture(&mut captured));
    assert!(
        !captured.0.is_empty(),
        "no field list for {} (a flatten wrapper? list its keys by hand)",
        std::any::type_name::<T>()
    );
    captured.0.into_iter().map(str::to_string).collect()
}

fn with(mut base: BTreeSet<String>, extra: &[&str]) -> BTreeSet<String> {
    base.extend(extra.iter().map(|s| (*s).to_string()));
    base
}

// ------------------------------------------------------------------ shape

/// Substrings that must never appear in a modelled key (non-negotiable 11,
/// plan section 2.8: the public invoice token, the Stripe intent id and the
/// SMTP password).
const NEVER_MODELLED: [&str; 4] = ["smtp", "publictoken", "stripe", "password"];

struct Shape {
    name: &'static str,
    keys: BTreeSet<String>,
    /// Keys that must be JSON strings when not null.
    strings: &'static [&'static str],
    /// Modelled keys the engine may leave out (documented per shape).
    optional: &'static [&'static str],
}

#[derive(Default)]
struct Report {
    failures: Vec<String>,
    unmodelled: Vec<String>,
    checked: usize,
}

impl Report {
    fn check(&mut self, shape: &Shape, raw: &Value) {
        self.checked += 1;
        let Some(obj) = raw.as_object() else {
            self.failures
                .push(format!("{}: reply is not an object", shape.name));
            return;
        };
        for k in &shape.keys {
            let lower = k.to_ascii_lowercase();
            if NEVER_MODELLED.iter().any(|bad| lower.contains(bad)) {
                self.failures
                    .push(format!("{}: `{k}` must not be modelled", shape.name));
            }
        }
        let missing: Vec<&String> = shape
            .keys
            .iter()
            .filter(|k| !obj.contains_key(k.as_str()) && !shape.optional.contains(&k.as_str()))
            .collect();
        if !missing.is_empty() {
            self.failures.push(format!(
                "{}: modelled keys missing from the engine reply (a rename would become a silent None): {missing:?}",
                shape.name
            ));
        }
        for k in shape.strings {
            match obj.get(*k) {
                None | Some(Value::Null | Value::String(_)) => {}
                Some(other) => self.failures.push(format!(
                    "{}: `{k}` must be a JSON string, got {}",
                    shape.name,
                    kind(other)
                )),
            }
        }
        let extra: Vec<&String> = obj.keys().filter(|k| !shape.keys.contains(*k)).collect();
        if !extra.is_empty() {
            self.unmodelled.push(format!("{}: {extra:?}", shape.name));
        }
    }
}

fn kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

// ------------------------------------------------------------------ tests

fn raw(h: &Harness, path: &str, query: &[(&str, &str)]) -> Value {
    let reply = h
        .run(h.s1.raw(Method::Get, path, query, None))
        .unwrap_or_else(|e| panic!("GET {path}: {e}"));
    assert_eq!(reply.status, 200, "GET {path}");
    reply.body
}

fn first<'a>(v: &'a Value, list_key: &str, path: &str) -> &'a Value {
    v[list_key]
        .as_array()
        .and_then(|a| a.first())
        .unwrap_or_else(|| panic!("GET {path}: `{list_key}` is empty or missing"))
}

pub fn live_spec_matches_fixtures(h: &Harness) {
    use crate::support::spec;
    // At the pinned engine the running server does not serve `GET
    // /openapi.json` (the document is a build-time export, `engine/openapi.json`,
    // checked by `tests/spec_fixtures.rs`). When a later engine serves it, the
    // served document must match the fixtures.
    let bytes = match h.run(h.client.openapi_spec()) {
        Ok(b) => b,
        Err(cl_api::ApiError::NotFound { .. }) => {
            eprintln!(
                "    NOTE: the engine does not serve GET /openapi.json at this pin; the spec fixtures are verified against engine/openapi.json (spec_fixtures)"
            );
            return;
        }
        Err(e) => panic!("openapi: {e}"),
    };
    let live: Value = serde_json::from_slice(&bytes).expect("spec is JSON");
    let hint = "the engine changed: run `cargo xtask api-spec --update`, review, and commit";

    let recorded = include_str!("../fixtures/openapi.sha256");
    assert_eq!(
        spec::spec_hash(&live),
        recorded.trim(),
        "live openapi.json differs from fixtures/openapi.sha256: {hint}"
    );

    let table: Value = serde_json::from_str(include_str!("../fixtures/covered_operations.json"))
        .expect("fixture json");
    let ops = spec::operations(include_str!("../../operations.txt"));
    assert_eq!(table.as_array().map(Vec::len), Some(ops.len()));
    for (method, route) in &ops {
        let (in_spec, hash) = spec::covered_entry(&live, method, route);
        let row = table
            .as_array()
            .and_then(|rows| {
                rows.iter()
                    .find(|r| r["method"] == method.as_str() && r["path"] == route.as_str())
            })
            .unwrap_or_else(|| panic!("{method} {route} missing from covered_operations.json"));
        assert_eq!(row["inSpec"], in_spec, "{method} {route} inSpec: {hint}");
        assert_eq!(
            row["fragmentHash"],
            hash.map_or(Value::Null, Value::String),
            "{method} {route} request fragment: {hint}"
        );
    }
}

pub fn field_census(h: &Harness) {
    let s = &h.s1;
    // Sample rows of every kind (names are unique; other tests may add more).
    let (cash, supplies) = pair(h, s);
    let income = account(h, s, cl_api::AccountType::Revenue, "Census sales");
    let txn = post(
        h,
        s,
        &je2(
            date(2026, 6, 5),
            &unique("census"),
            supplies.id,
            cash.id,
            "12.5000",
        ),
    )
    .record;
    let contact = h
        .run(s.contacts().create(
            &NewContact {
                fields: ContactFields::named(ContactType::Customer, unique("Census Cust")),
                opening_balance: Money::ZERO,
                opening_balance_date: None,
            },
            key(),
        ))
        .expect("contact")
        .record;
    let item = h
        .run(s.items().create(
            &ItemFields {
                name: unique("Census item"),
                description: None,
                unit_price: Some(Money::parse_wire("3.2500").expect("money")),
                income_account_id: income.id,
                is_taxable: false,
            },
            key(),
        ))
        .expect("item")
        .record;
    let tag = h
        .run(s.tags().create(
            &TagFields {
                name: unique("Census tag"),
                color: None,
                group_id: None,
                description: None,
            },
            key(),
        ))
        .expect("tag")
        .record;

    let account_keys = with(
        fields_of::<AccountFields>(),
        &[
            "id",
            "isActive",
            "isSystem",
            "systemTag",
            "balance",
            "createdAt",
            "updatedAt",
        ],
    );
    let contact_keys = with(
        fields_of::<ContactFields>(),
        &[
            "id",
            "isActive",
            "openingBalance",
            "openingBalanceDate",
            "createdAt",
            "updatedAt",
        ],
    );
    let item_keys = with(
        fields_of::<ItemFields>(),
        &["id", "isActive", "defaultTagId", "createdAt", "updatedAt"],
    );
    let tag_keys = with(
        fields_of::<TagFields>(),
        &[
            "id",
            "isActive",
            "usageCount",
            "sortOrder",
            "createdAt",
            "updatedAt",
        ],
    );
    let profile_keys = with(
        fields_of::<CompanyProfileUpdate>(),
        &["id", "setupComplete", "createdAt", "updatedAt"],
    );

    let account_shape = |name| Shape {
        name,
        keys: account_keys.clone(),
        strings: &["balance"],
        optional: &[],
    };
    let contact_shape = |name| Shape {
        name,
        keys: contact_keys.clone(),
        strings: &["openingBalance"],
        optional: &[],
    };
    let item_shape = |name| Shape {
        name,
        keys: item_keys.clone(),
        strings: &["unitPrice"],
        optional: &[],
    };
    let tag_shape = |name| Shape {
        name,
        keys: tag_keys.clone(),
        strings: &[],
        optional: &[],
    };

    let mut r = Report::default();

    // Public and session routes.
    r.check(
        &Shape {
            name: "GET /health",
            keys: fields_of::<Health>(),
            strings: &[],
            optional: &[],
        },
        &raw(h, "/health", &[]),
    );
    r.check(
        &Shape {
            name: "GET /api/v1/desktop/info",
            keys: fields_of::<DesktopInfo>(),
            strings: &[],
            // `capabilities` arrives with U-21; cl-api reads a missing array as empty.
            optional: &["capabilities"],
        },
        &raw(h, "/api/v1/desktop/info", &[]),
    );
    let me = raw(h, "/api/v1/auth/me", &[]);
    let tenants = me["accessibleTenants"]
        .as_array()
        .or_else(|| me["user"]["accessibleTenants"].as_array())
        .unwrap_or_else(|| panic!("auth/me: no accessibleTenants"));
    r.check(
        &Shape {
            name: "GET /api/v1/auth/me accessibleTenants[]",
            keys: fields_of::<CompanySummary>(),
            strings: &[],
            optional: &[],
        },
        tenants.first().expect("a tenant"),
    );
    r.check(
        &Shape {
            name: "GET /api/v1/ledger-version",
            keys: ["stamp".to_string()].into_iter().collect(),
            strings: &[],
            optional: &[],
        },
        &raw(h, "/api/v1/ledger-version", &[]),
    );

    // Company.
    let company = raw(h, "/api/v1/company", &[]);
    r.check(
        &Shape {
            name: "GET /api/v1/company company",
            keys: profile_keys,
            strings: &[],
            optional: &[],
        },
        &company["company"],
    );
    r.check(
        &Shape {
            name: "GET /api/v1/company/settings settings",
            keys: fields_of::<CompanySettings>(),
            strings: &["defaultSalesTaxRate"],
            optional: &[],
        },
        &raw(h, "/api/v1/company/settings", &[])["settings"],
    );

    // Lists and single records.
    let q = [("limit", "5")];
    let list = raw(h, "/api/v1/accounts", &q);
    r.check(
        &account_shape("GET /accounts data[]"),
        first(&list, "data", "/accounts"),
    );
    r.check(
        &account_shape("GET /accounts/{id} account"),
        &raw(h, &format!("/api/v1/accounts/{}", cash.id), &[])["account"],
    );

    let list = raw(h, "/api/v1/contacts", &q);
    r.check(
        &contact_shape("GET /contacts data[]"),
        first(&list, "data", "/contacts"),
    );
    r.check(
        &contact_shape("GET /contacts/{id} contact"),
        &raw(h, &format!("/api/v1/contacts/{}", contact.id), &[])["contact"],
    );

    let list = raw(h, "/api/v1/items", &q);
    r.check(
        &item_shape("GET /items data[]"),
        first(&list, "data", "/items"),
    );
    r.check(
        &item_shape("GET /items/{id} item"),
        &raw(h, &format!("/api/v1/items/{}", item.id), &[])["item"],
    );

    let list = raw(h, "/api/v1/tags", &[]);
    r.check(
        &tag_shape("GET /tags tags[]"),
        first(&list, "tags", "/tags"),
    );
    r.check(
        &tag_shape("GET /tags/{id} tag"),
        &raw(h, &format!("/api/v1/tags/{}", tag.id), &[])["tag"],
    );

    let list = raw(h, "/api/v1/transactions", &q);
    r.check(
        &Shape {
            name: "GET /transactions data[]",
            keys: fields_of::<TransactionSummary>(),
            strings: &["total", "displayTotal"],
            // P13 plan E5: the engine list does not carry the check number
            // yet; recent checks show a dash until it does.
            optional: &["checkNumber"],
        },
        first(&list, "data", "/transactions"),
    );
    // P13 WP2: the payee panel's open-bill summary.
    r.check(
        &Shape {
            name: "GET /bills/vendor-summary summary",
            keys: fields_of::<cl_api::VendorSummary>(),
            strings: &["totalOwed", "totalOverdue", "totalAvailableCredits"],
            optional: &[],
        },
        &raw(
            h,
            "/api/v1/bills/vendor-summary",
            &[("contactId", &contact.id.to_string())],
        )["summary"],
    );
    let one = raw(h, &format!("/api/v1/transactions/{}", txn.id), &[]);
    r.check(
        &Shape {
            name: "GET /transactions/{id} transaction",
            keys: fields_of::<Transaction>(),
            strings: &["total"],
            optional: &[],
        },
        &one["transaction"],
    );
    let line = one["transaction"]["lines"]
        .as_array()
        .and_then(|l| l.first())
        .expect("a line");
    r.check(
        &Shape {
            name: "GET /transactions/{id} transaction.lines[]",
            keys: fields_of::<TransactionLine>(),
            strings: &["debit", "credit"],
            // P13: `line-billable-v1` adds these; older engines leave them out.
            optional: &["isBillable", "billedStatus"],
        },
        line,
    );

    for u in &r.unmodelled {
        eprintln!("    census: unmodelled keys, {u}");
    }
    assert!(
        r.failures.is_empty(),
        "field census ({} shapes) found drift between the models and the engine:\n  {}\n\
         Fix the model (or, if the engine changed on purpose, update it with the spec fixtures).",
        r.checked,
        r.failures.join("\n  ")
    );
}
