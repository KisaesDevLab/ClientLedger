//! Record versions for optimistic concurrency (P05 plan §2.5).
//!
//! - With U-17 (`record-version-v1`) the engine returns a `version` per
//!   record; an update sends `If-Match: "<n>"` and a mismatch is a 409
//!   `STALE` (`Conflict { Stale, Server(n) }`).
//! - Until then the version is a **fingerprint**: SHA-256 over the canonical
//!   JSON of the fields the record's edit form writes. An update first reads
//!   the record again; a changed fingerprint is a conflict and the update is
//!   not sent. Server-maintained values (balances, `updatedAt`), joined
//!   display names and journal-line ids are never part of a fingerprint, so
//!   a posting or an engine-side line rewrite does not raise a false conflict
//!   (S7). The residual race between the re-read and the update is
//!   milliseconds; LAN (P33) waits for U-17.

use serde::de::DeserializeOwned;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::client::{Call, Method};
use crate::errors::{ApiError, ConflictReason, UserText};
use crate::idempotency::IdempotencyKey;
use crate::resources::operations::Operation;
use crate::session::Session;

/// The version a record was read at.
///
/// `Server` comes from the engine's `version` column (U-17). Until the engine
/// has it, `Fingerprint` is the SHA-256 of the record's editable fields, used
/// by the pre-save check.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum RecordVersion {
    /// The engine's record version (U-17, `record-version-v1`).
    Server(u64),
    /// SHA-256 over the canonical JSON of the editable fields.
    Fingerprint([u8; 32]),
}

impl std::fmt::Debug for RecordVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RecordVersion::Server(v) => write!(f, "Server({v})"),
            RecordVersion::Fingerprint(h) => {
                write!(f, "Fingerprint(")?;
                for b in &h[..4] {
                    write!(f, "{b:02x}")?;
                }
                write!(f, "…)")
            }
        }
    }
}

/// A record together with the version it was read at. Forms keep this and
/// hand the version back on save.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Versioned<T> {
    /// The record.
    pub record: T,
    /// Its version.
    pub version: RecordVersion,
}

/// The fields of a record that its edit form writes.
pub trait EditableFields {
    /// Wire names of the hashed fields, listed next to the model and
    /// unit-tested (for journal entries: header fields plus each line's
    /// account, debit, credit, description, tag, contact and `lineOrder`;
    /// never line ids).
    const EDITABLE: &'static [&'static str];

    /// The editable fields as JSON (money as canonical wire strings; lines
    /// in `lineOrder` order).
    fn editable(&self) -> Value;

    /// SHA-256 over the canonical JSON of [`EditableFields::editable`].
    fn fingerprint(&self) -> [u8; 32] {
        fingerprint_of(&self.editable())
    }
}

/// Canonical JSON: object keys sorted, no whitespace. Independent of
/// `serde_json`'s map ordering features.
#[must_use]
pub fn canonical_json(v: &Value) -> String {
    let mut out = String::new();
    write_canonical(v, &mut out);
    out
}

fn write_canonical(v: &Value, out: &mut String) {
    match v {
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {
            out.push_str(&v.to_string());
        }
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canonical(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (i, k) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(k.clone()).to_string());
                out.push(':');
                write_canonical(&map[k.as_str()], out);
            }
            out.push('}');
        }
    }
}

/// SHA-256 of the canonical JSON of `v`.
#[must_use]
pub fn fingerprint_of(v: &Value) -> [u8; 32] {
    Sha256::digest(canonical_json(v).as_bytes()).into()
}

/// Picks `fields` out of a JSON object (missing fields become `null`, so a
/// field that disappears changes the fingerprint).
#[must_use]
pub fn pick(object: &Value, fields: &[&str]) -> Value {
    let map = fields
        .iter()
        .map(|f| {
            (
                (*f).to_string(),
                object.get(*f).cloned().unwrap_or(Value::Null),
            )
        })
        .collect();
    Value::Object(map)
}

/// Reads `{<key>: {...}}` into the record and its version: `Server` when the
/// engine sent a `version` (U-17), otherwise the record's fingerprint.
pub(crate) fn versioned_from<T>(body: &[u8], key: &'static str) -> Result<Versioned<T>, ApiError>
where
    T: DeserializeOwned + EditableFields,
{
    let mut root: Value =
        serde_json::from_slice(body).map_err(|_| ApiError::Decode { what: key })?;
    let inner = root
        .get_mut(key)
        .map(Value::take)
        .ok_or(ApiError::Decode { what: key })?;
    let server = inner.get("version").and_then(Value::as_u64);
    let record: T = serde_json::from_value(inner).map_err(|_| ApiError::Decode { what: key })?;
    let version = server.map_or_else(
        || RecordVersion::Fingerprint(record.fingerprint()),
        RecordVersion::Server,
    );
    Ok(Versioned { record, version })
}

/// `GET` one record with its version.
pub(crate) async fn get_versioned<T>(
    session: &Session,
    op: &Operation,
    path: String,
    key: &'static str,
) -> Result<Versioned<T>, ApiError>
where
    T: DeserializeOwned + EditableFields,
{
    let reply = session
        .send(op, Call::new(Method::Get, op.route(), path), None)
        .await?;
    versioned_from(&reply.body, key)
}

/// One guarded update (P05 plan §2.5).
pub(crate) struct GuardedUpdate<'a> {
    pub get: &'a Operation,
    pub put: &'a Operation,
    pub path: String,
    pub key: &'static str,
    pub body: Vec<u8>,
    pub idempotency_key: IdempotencyKey,
}

/// Updates a record only if it is still at `expected`.
///
/// - `Server(v)`: sends `If-Match: "v"`; the engine answers 409 `STALE` on a
///   mismatch.
/// - `Fingerprint(f)`: re-reads the record; a different fingerprint is
///   `Conflict { Stale, Fingerprint(new) }` and nothing is sent; otherwise
///   the update goes out without `If-Match`.
pub(crate) async fn update_guarded<T>(
    session: &Session,
    expected: &RecordVersion,
    u: GuardedUpdate<'_>,
) -> Result<Versioned<T>, ApiError>
where
    T: DeserializeOwned + EditableFields,
{
    let mut call = Call::new(u.put.method(), u.put.route(), u.path.clone()).json_body(u.body);
    call.idempotency_key = Some(u.idempotency_key);
    match expected {
        RecordVersion::Server(v) => call.if_match = Some(format!("\"{v}\"")),
        RecordVersion::Fingerprint(f) => {
            let current: Versioned<T> = get_versioned(session, u.get, u.path, u.key).await?;
            let now = current.record.fingerprint();
            if &now != f {
                return Err(ApiError::Conflict {
                    reason: ConflictReason::Stale,
                    current_version: Some(RecordVersion::Fingerprint(now)),
                    code: None,
                    message: UserText::default(),
                });
            }
        }
    }
    let reply = session.send(u.put, call, None).await?;
    versioned_from(&reply.body, u.key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::operations::find;
    use crate::session::Scope;
    use crate::session::tests::session_on;
    use cl_core::Money;
    use cl_core::ids::{CompanyId, TenantId};
    use serde::Deserialize;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// A journal-entry-shaped sample model.
    #[derive(Debug, Deserialize, PartialEq)]
    #[serde(rename_all = "camelCase")]
    struct Sample {
        id: String,
        memo: Option<String>,
        lines: Vec<SampleLine>,
        #[serde(default)]
        contact_name: Option<String>,
        #[serde(default)]
        updated_at: Option<String>,
    }

    #[derive(Debug, Deserialize, PartialEq)]
    #[serde(rename_all = "camelCase")]
    struct SampleLine {
        id: String,
        account_id: String,
        debit: Money,
        credit: Money,
        line_order: i64,
    }

    impl EditableFields for Sample {
        const EDITABLE: &'static [&'static str] = &["memo", "lines"];
        fn editable(&self) -> Value {
            let mut lines: Vec<&SampleLine> = self.lines.iter().collect();
            lines.sort_by_key(|l| l.line_order);
            serde_json::json!({
                "memo": self.memo,
                "lines": lines.iter().map(|l| serde_json::json!({
                    "accountId": l.account_id,
                    "debit": l.debit.to_wire(),
                    "credit": l.credit.to_wire(),
                    "lineOrder": l.line_order,
                })).collect::<Vec<_>>(),
            })
        }
    }

    fn sample_json(line_ids: (&str, &str), orders: (i64, i64), memo: &str, extra: &str) -> String {
        format!(
            r#"{{"id":"t1","memo":"{memo}","contactName":"{extra}","updatedAt":"{extra}",
               "lines":[{{"id":"{}","accountId":"a","debit":"10.0000","credit":"0.0000","lineOrder":{}}},
                        {{"id":"{}","accountId":"b","debit":"0.0000","credit":"10.0000","lineOrder":{}}}]}}"#,
            line_ids.0, orders.0, line_ids.1, orders.1
        )
    }

    fn fp(json: &str) -> [u8; 32] {
        serde_json::from_str::<Sample>(json)
            .expect("sample")
            .fingerprint()
    }

    #[test]
    fn fingerprint_ignores_line_ids_and_server_fields_but_not_line_order() {
        let base = fp(&sample_json(("l1", "l2"), (0, 1), "rent", "x"));
        assert_eq!(
            base,
            fp(&sample_json(("l1", "l2"), (0, 1), "rent", "x")),
            "stable"
        );
        assert_eq!(
            base,
            fp(&sample_json(("n1", "n2"), (0, 1), "rent", "x")),
            "new line ids"
        );
        assert_eq!(
            base,
            fp(&sample_json(("l1", "l2"), (0, 1), "rent", "renamed")),
            "joined names"
        );
        assert_ne!(
            base,
            fp(&sample_json(("l1", "l2"), (1, 0), "rent", "x")),
            "line order"
        );
        assert_ne!(
            base,
            fp(&sample_json(("l1", "l2"), (0, 1), "rent2", "x")),
            "memo"
        );
        assert_eq!(Sample::EDITABLE, &["memo", "lines"]);
    }

    #[test]
    fn canonical_json_sorts_keys_at_every_depth() {
        let a: Value =
            serde_json::from_str(r#"{"b":1,"a":{"d":[1,{"z":2,"y":"é\""}],"c":null}}"#).expect("a");
        assert_eq!(
            canonical_json(&a),
            r#"{"a":{"c":null,"d":[1,{"y":"é\"","z":2}]},"b":1}"#
        );
        let b: Value =
            serde_json::from_str(r#"{"a":{"c":null,"d":[1,{"y":"é\"","z":2}]},"b":1}"#).expect("b");
        assert_eq!(fingerprint_of(&a), fingerprint_of(&b));
        let p = pick(&a, &["b", "missing"]);
        assert_eq!(canonical_json(&p), r#"{"b":1,"missing":null}"#);
        assert!(
            format!("{:?}", RecordVersion::Fingerprint(fingerprint_of(&a)))
                .starts_with("Fingerprint(")
        );
        assert_eq!(format!("{:?}", RecordVersion::Server(3)), "Server(3)");
    }

    fn company_session(server: &MockServer) -> Session {
        session_on(
            server,
            Scope::Company {
                tenant_id: TenantId::parse("1b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01").expect("t"),
                company_id: CompanyId::parse("0b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01").expect("c"),
            },
        )
    }

    const ROUTE: &str = "/api/v1/transactions/{id}";
    const PATH: &str = "/api/v1/transactions/t1";

    fn update(body: &str) -> GuardedUpdate<'static> {
        GuardedUpdate {
            get: find(Method::Get, ROUTE).expect("get"),
            put: find(Method::Put, ROUTE).expect("put"),
            path: PATH.to_string(),
            key: "transaction",
            body: body.as_bytes().to_vec(),
            idempotency_key: IdempotencyKey::new(),
        }
    }

    fn wrap(record: &str) -> String {
        format!(r#"{{"transaction":{record}}}"#)
    }

    fn with_version(record: &str, v: u64) -> String {
        format!(
            "{},\"version\":{v}}}",
            record.trim_end().trim_end_matches('}')
        )
    }

    fn json(status: u16, body: &str) -> ResponseTemplate {
        ResponseTemplate::new(status).set_body_raw(body.to_string(), "application/json")
    }

    #[tokio::test]
    async fn get_returns_server_version_when_present_else_fingerprint() {
        let server = MockServer::start().await;
        let rec = sample_json(("l1", "l2"), (0, 1), "rent", "x");
        Mock::given(method("GET"))
            .respond_with(json(200, &wrap(&rec)))
            .up_to_n_times(1)
            .with_priority(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .respond_with(json(200, &wrap(&with_version(&rec, 4))))
            .mount(&server)
            .await;
        let s = company_session(&server);
        let get = find(Method::Get, ROUTE).expect("get");
        let a: Versioned<Sample> = get_versioned(&s, get, PATH.into(), "transaction")
            .await
            .expect("a");
        assert_eq!(a.version, RecordVersion::Fingerprint(fp(&rec)));
        let b: Versioned<Sample> = get_versioned(&s, get, PATH.into(), "transaction")
            .await
            .expect("b");
        assert_eq!(b.version, RecordVersion::Server(4));
        assert_eq!(a.record, b.record);
    }

    /// U-17 stub contract: `If-Match` with the server version, 409 `STALE`
    /// with `details.currentVersion` → `Conflict { Stale, Server(n) }`.
    #[tokio::test]
    async fn u17_server_version_sends_if_match_and_maps_stale() {
        let server = MockServer::start().await;
        let rec = sample_json(("n1", "n2"), (0, 1), "rent", "x");
        Mock::given(method("PUT"))
            .and(wiremock::matchers::header("if-match", "\"7\""))
            .respond_with(json(200, &wrap(&with_version(&rec, 8))))
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(wiremock::matchers::header("if-match", "\"6\""))
            .respond_with(json(
                409,
                r#"{"error":{"code":"STALE","message":"This record was changed by someone else.",
                    "details":{"entity":"transaction","id":"t1","currentVersion":9,"updatedAt":"2026-10-15T00:00:00.000Z"}}}"#,
            ))
            .mount(&server)
            .await;
        let s = company_session(&server);
        let ok: Versioned<Sample> = update_guarded(&s, &RecordVersion::Server(7), update("{}"))
            .await
            .expect("ok");
        assert_eq!(ok.version, RecordVersion::Server(8));
        let e = update_guarded::<Sample>(&s, &RecordVersion::Server(6), update("{}"))
            .await
            .expect_err("stale");
        assert!(matches!(
            e,
            ApiError::Conflict {
                reason: ConflictReason::Stale,
                current_version: Some(RecordVersion::Server(9)),
                ..
            }
        ));
        let gets = server
            .received_requests()
            .await
            .expect("r")
            .iter()
            .filter(|r| r.method.as_str() == "GET")
            .count();
        assert_eq!(gets, 0, "server versions need no pre-read");
    }

    #[tokio::test]
    async fn interim_fingerprint_check_blocks_a_stale_save_without_sending_it() {
        let server = MockServer::start().await;
        let original = sample_json(("l1", "l2"), (0, 1), "rent", "x");
        let changed = sample_json(("l3", "l4"), (0, 1), "rent (edited elsewhere)", "x");
        Mock::given(method("GET"))
            .and(path(PATH))
            .respond_with(json(200, &wrap(&changed)))
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .respond_with(json(200, &wrap(&changed)))
            .mount(&server)
            .await;
        let s = company_session(&server);
        let e =
            update_guarded::<Sample>(&s, &RecordVersion::Fingerprint(fp(&original)), update("{}"))
                .await
                .expect_err("conflict");
        let ApiError::Conflict {
            reason,
            current_version,
            ..
        } = e
        else {
            panic!("{e:?}")
        };
        assert_eq!(reason, ConflictReason::Stale);
        assert_eq!(
            current_version,
            Some(RecordVersion::Fingerprint(fp(&changed)))
        );
        let reqs = server.received_requests().await.expect("r");
        assert_eq!(
            reqs.iter().filter(|r| r.method.as_str() == "PUT").count(),
            0
        );

        // Same editable content (new line ids only): the save goes out
        // without If-Match and carries the idempotency key.
        let same_content = sample_json(("l9", "l8"), (0, 1), "rent (edited elsewhere)", "y");
        let ok: Versioned<Sample> = update_guarded(
            &s,
            &RecordVersion::Fingerprint(fp(&same_content)),
            update(r#"{"memo":"m"}"#),
        )
        .await
        .expect("saved");
        assert_eq!(ok.version, RecordVersion::Fingerprint(fp(&changed)));
        let reqs = server.received_requests().await.expect("r");
        let put = reqs
            .iter()
            .find(|r| r.method.as_str() == "PUT")
            .expect("put");
        assert!(put.headers.get("if-match").is_none());
        assert!(put.headers.get("idempotency-key").is_some());
        assert!(put.headers.get("x-company-id").is_some());
    }

    #[test]
    fn malformed_records_are_decode_errors() {
        assert!(versioned_from::<Sample>(b"{}", "transaction").is_err());
        assert!(versioned_from::<Sample>(br#"{"transaction":{"id":1}}"#, "transaction").is_err());
        let money_as_number = r#"{"transaction":{"id":"t","memo":null,"lines":[{"id":"l","accountId":"a","debit":10.5,"credit":"0.0000","lineOrder":0}]}}"#;
        assert!(versioned_from::<Sample>(money_as_number.as_bytes(), "transaction").is_err());
    }
}
