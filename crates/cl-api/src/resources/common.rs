//! Shared plumbing of the list/get/create/update/delete resources.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::client::{Call, EngineReply, Method};
use crate::errors::ApiError;
use crate::idempotency::{IdempotencyKey, Reconciler};
use crate::resources::operations::Operation;
use crate::resources::reconcile::{OutcomeMatcher, Probe, Search, row_fp};
use crate::resources::versioning::{
    EditableFields, GuardedUpdate, RecordVersion, Versioned, fingerprint_of, get_versioned, pick,
    saved_from, update_guarded,
};
use crate::session::Session;
use crate::wire::{Page, PageRequest};

/// `list_all` stops here (P05 plan §2.8).
pub(crate) const LIST_ALL_CAP: u64 = 100_000;

/// Sort direction of a list.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum SortDir {
    /// Ascending.
    #[default]
    Asc,
    /// Descending.
    Desc,
}

impl SortDir {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            SortDir::Asc => "asc",
            SortDir::Desc => "desc",
        }
    }
}

/// Query pairs in the engine's exact casing.
#[derive(Default)]
pub(crate) struct Query(pub Vec<(String, String)>);

impl Query {
    pub(crate) fn put(&mut self, key: &str, value: impl Into<String>) {
        self.0.push((key.to_string(), value.into()));
    }

    pub(crate) fn opt_bool(&mut self, key: &str, value: Option<bool>) {
        if let Some(v) = value {
            self.put(key, if v { "true" } else { "false" });
        }
    }

    pub(crate) fn opt_str(&mut self, key: &str, value: Option<&str>) {
        if let Some(v) = value.filter(|v| !v.is_empty()) {
            self.put(key, v);
        }
    }

    pub(crate) fn sort(&mut self, sort: Option<(&'static str, SortDir)>) {
        if let Some((by, dir)) = sort {
            self.put("sortBy", by);
            self.put("sortDir", dir.as_str());
        }
    }
}

/// Where one master-data resource lives on the engine.
pub(crate) struct Spec {
    pub list: &'static Operation,
    pub create: &'static Operation,
    pub get: &'static Operation,
    pub put: &'static Operation,
    pub delete: &'static Operation,
    /// `/api/v1/accounts`
    pub base: &'static str,
    /// Key of one record in a response (`account`).
    pub item_key: &'static str,
    /// Key of the rows in a list response (`data`, `tags`).
    pub list_key: &'static str,
}

impl Spec {
    pub(crate) fn path(&self, id: &str) -> String {
        format!("{}/{id}", self.base)
    }
}

pub(crate) async fn list_page<T: DeserializeOwned>(
    s: &Session,
    spec: &Spec,
    mut q: Query,
    page: PageRequest,
) -> Result<Page<T>, ApiError> {
    q.0.extend(page.query());
    let mut call = Call::at(Method::Get, spec.list.route());
    call.query = q.0;
    let reply = s.send(spec.list, call, None).await?;
    Page::from_json(&reply.body, spec.list_key)
}

pub(crate) async fn list_all<T: DeserializeOwned>(
    s: &Session,
    spec: &Spec,
    q: &Query,
) -> Result<Vec<T>, ApiError> {
    let mut out = Vec::new();
    let mut page = PageRequest::first_max();
    loop {
        let p: Page<T> = list_page(s, spec, Query(q.0.clone()), page).await?;
        let got = p.data.len();
        out.extend(p.data);
        let have = u64::try_from(out.len()).unwrap_or(u64::MAX);
        if have > LIST_ALL_CAP {
            return Err(ApiError::Decode {
                what: "list too large",
            });
        }
        if got == 0 || have >= p.total {
            return Ok(out);
        }
        page = page.next();
    }
}

pub(crate) async fn get<T: DeserializeOwned + EditableFields>(
    s: &Session,
    spec: &Spec,
    id: &str,
) -> Result<Versioned<T>, ApiError> {
    get_versioned(s, spec.get, spec.path(id), spec.item_key).await
}

pub(crate) async fn create<T: DeserializeOwned + EditableFields>(
    s: &Session,
    spec: &Spec,
    body: Vec<u8>,
    key: IdempotencyKey,
    rec: Option<&dyn Reconciler>,
) -> Result<Versioned<T>, ApiError> {
    let mut call = Call::at(Method::Post, spec.create.route()).json_body(body);
    call.idempotency_key = Some(key);
    let reply = s.send(spec.create, call, rec).await?;
    saved_from(&reply.body, spec.item_key)
}

/// A guarded master-data update; the requested fingerprint is the update
/// body's editable fields.
pub(crate) async fn update<T: DeserializeOwned + EditableFields>(
    s: &Session,
    spec: &'static Spec,
    id: &str,
    expected: &RecordVersion,
    body: Vec<u8>,
    key: IdempotencyKey,
) -> Result<Versioned<T>, ApiError> {
    let requested = requested_fp::<T>(&body);
    let u = GuardedUpdate {
        requested,
        fp: row_fp::<T>,
        stamp_covered: false,
        get: spec.get,
        put: spec.put,
        path: spec.path(id),
        key: spec.item_key,
        body,
        idempotency_key: key,
    };
    update_guarded(s, expected, u).await
}

pub(crate) async fn delete(
    s: &Session,
    spec: &Spec,
    id: &str,
    key: IdempotencyKey,
    rec: Option<&dyn Reconciler>,
) -> Result<EngineReply, ApiError> {
    let mut call = Call::new(Method::Delete, spec.delete.route(), spec.path(id));
    call.idempotency_key = Some(key);
    s.send(spec.delete, call, rec).await
}

/// The editable JSON of a record: its form fields plus `isActive`.
pub(crate) fn editable_with_active<F: Serialize>(
    fields: &F,
    is_active: bool,
    names: &[&str],
) -> Value {
    let mut v = serde_json::to_value(fields).unwrap_or(Value::Null);
    if let Value::Object(map) = &mut v {
        map.insert("isActive".into(), Value::Bool(is_active));
    }
    pick(&v, names)
}

/// A nullable boolean column that defaults to `true`.
pub(crate) fn null_true<'de, D: Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    Ok(Option::<bool>::deserialize(d)?.unwrap_or(true))
}

/// A nullable boolean column that defaults to `false`.
pub(crate) fn null_false<'de, D: Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    Ok(Option::<bool>::deserialize(d)?.unwrap_or(false))
}

pub(crate) const fn yes() -> bool {
    true
}

/// The fingerprint a record has once `body` (an update) committed.
pub(crate) fn requested_fp<T: EditableFields>(body: &[u8]) -> [u8; 32] {
    let v: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    fingerprint_of(&pick(&v, T::EDITABLE))
}

/// Creates master data with outcome reconciliation by exact name.
pub(crate) async fn create_named<T: DeserializeOwned + EditableFields>(
    s: &Session,
    spec: &'static Spec,
    name_field: &'static str,
    name: &str,
    expected: &Value,
    body: Vec<u8>,
    key: IdempotencyKey,
) -> Result<Versioned<T>, ApiError> {
    // One company for the probe and the write (pinned view).
    let s = &s.pinned();
    let matcher = OutcomeMatcher::Created {
        search: Search::Named {
            spec,
            name_field,
            name: name.to_string(),
        },
        fp: row_fp::<T>,
        fingerprint: fingerprint_of(expected),
    };
    let probe = Probe::new(s, matcher, false).await;
    create(s, spec, body, key, Some(&probe)).await
}

/// Deactivates (`DELETE` that sets `isActive: false`) with reconciliation.
pub(crate) async fn deactivate(
    s: &Session,
    spec: &'static Spec,
    id: &str,
    key: IdempotencyKey,
) -> Result<EngineReply, ApiError> {
    // One company for the probe and the write (pinned view).
    let s = &s.pinned();
    let matcher = OutcomeMatcher::Field {
        get: spec.get,
        path: spec.path(id),
        key: spec.item_key,
        field: "isActive",
        value: Value::Bool(false),
    };
    let probe = Probe::new(s, matcher, false).await;
    delete(s, spec, id, key, Some(&probe)).await
}

/// Deletes for real with reconciliation (gone = committed).
pub(crate) async fn delete_record(
    s: &Session,
    spec: &'static Spec,
    id: &str,
    key: IdempotencyKey,
) -> Result<EngineReply, ApiError> {
    // One company for the probe and the write (pinned view).
    let s = &s.pinned();
    let matcher = OutcomeMatcher::Deleted {
        get: spec.get,
        path: spec.path(id),
    };
    let probe = Probe::new(s, matcher, false).await;
    delete(s, spec, id, key, Some(&probe)).await
}
