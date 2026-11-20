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

use crate::client::{Call, EngineReply, Method};
use crate::errors::{ApiError, ConflictReason, UserText};
use crate::idempotency::IdempotencyKey;
use crate::resources::operations::Operation;
use crate::resources::reconcile::{OutcomeMatcher, Probe, RowFp};
use crate::session::Session;

/// The version a record was read at.
///
/// `Server` comes from the engine's `version` column (U-17). Until the engine
/// has it, `Fingerprint` is the SHA-256 of the record's editable fields, used
/// by the pre-save check.
///
/// Equality and hashing go by content: a [`RecordVersion::Stamped`] version
/// equals the [`RecordVersion::Fingerprint`] of the same fingerprint whatever
/// its stamp (the stamp is carried for the engine to check, never compared
/// here), so a form's `saved == read` checks mean what they meant before
/// stamps.
#[derive(Clone, Copy)]
pub enum RecordVersion {
    /// The engine's record version (U-17, `record-version-v1`).
    Server(u64),
    /// SHA-256 over the canonical JSON of the editable fields.
    Fingerprint([u8; 32]),
    /// A record read with its `updatedAt` stamp (P33, U-291). With
    /// `stale-all-v1` an update sends the stamp as `expectedUpdatedAt` and
    /// the engine checks it; without it the fingerprint check runs as for
    /// [`RecordVersion::Fingerprint`].
    Stamped {
        /// The editable fields' fingerprint.
        fingerprint: [u8; 32],
        /// The stamp, echoed exactly.
        stamp: RecordStamp,
    },
    /// Only the stamp is known (a 409 `STALE` answer's `details.updatedAt`).
    Stamp(RecordStamp),
}

impl RecordVersion {
    /// The content key equality and hashing use.
    fn key(&self) -> (u8, [u8; 32], u64, Option<RecordStamp>) {
        match self {
            RecordVersion::Server(v) => (0, [0; 32], *v, None),
            RecordVersion::Fingerprint(f) | RecordVersion::Stamped { fingerprint: f, .. } => {
                (1, *f, 0, None)
            }
            RecordVersion::Stamp(s) => (2, [0; 32], 0, Some(*s)),
        }
    }
}

impl PartialEq for RecordVersion {
    fn eq(&self, other: &Self) -> bool {
        self.key() == other.key()
    }
}

impl Eq for RecordVersion {}

impl std::hash::Hash for RecordVersion {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.key().hash(state);
    }
}

impl RecordVersion {
    /// The record stamp, when the engine sent one.
    #[must_use]
    pub fn stamp(&self) -> Option<RecordStamp> {
        match self {
            RecordVersion::Stamped { stamp, .. } | RecordVersion::Stamp(stamp) => Some(*stamp),
            RecordVersion::Server(_) | RecordVersion::Fingerprint(_) => None,
        }
    }

    /// The same version with the stamp a save answered with (a stamp-only
    /// version takes the fingerprint `fallback` for engines without
    /// `stale-all-v1`). Forms carry the stamp of their last save this way
    /// (review S2), so their next save is never a self-conflict.
    #[must_use]
    pub fn with_stamp(self, stamp: RecordStamp, fallback: [u8; 32]) -> RecordVersion {
        match self {
            RecordVersion::Stamped { fingerprint, .. } => {
                RecordVersion::Stamped { fingerprint, stamp }
            }
            RecordVersion::Fingerprint(f) => RecordVersion::Stamped {
                fingerprint: f,
                stamp,
            },
            RecordVersion::Stamp(_) => RecordVersion::Stamped {
                fingerprint: fallback,
                stamp,
            },
            RecordVersion::Server(v) => RecordVersion::Server(v),
        }
    }

    /// The version a list row was read at: its editable fields'
    /// `fingerprint`, with the row's `updatedAt` as the stamp when it is
    /// one (so a save sends `expectedUpdatedAt` under `stale-all-v1`).
    #[must_use]
    pub fn read_at(fingerprint: [u8; 32], updated_at: Option<cl_core::Timestamp>) -> RecordVersion {
        match updated_at.and_then(RecordStamp::of_timestamp) {
            Some(stamp) => RecordVersion::Stamped { fingerprint, stamp },
            None => RecordVersion::Fingerprint(fingerprint),
        }
    }

    /// Takes the record's new stamp from a write on something it owns (an
    /// attachment or tag write's `parentUpdatedAt`, U-291 S2), so the
    /// form's next save is not a conflict with itself. Only a newer stamp
    /// is taken: an answer that lands after a later save never rolls the
    /// version back. The fingerprint is kept (the editable fields did not
    /// change); an engine version (`Server`) is left as it is.
    pub fn adopt_stamp(&mut self, stamp: Option<RecordStamp>) {
        let Some(stamp) = stamp else { return };
        if self.stamp().is_some_and(|held| held >= stamp) {
            return;
        }
        *self = match *self {
            RecordVersion::Stamp(_) => RecordVersion::Stamp(stamp),
            v => v.with_stamp(stamp, [0; 32]),
        };
    }
}

/// An engine record stamp (`updatedAt`, U-291): a millisecond UTC
/// timestamp as JavaScript writes it (`2026-11-19T10:00:00.123Z`), echoed
/// back exactly. Kept as its 17 digits so [`RecordVersion`] stays small and
/// `Copy`; text in any other shape is not taken as a stamp (the version
/// then falls back to the fingerprint check), so what is sent back is
/// always byte for byte what was read.
///
/// Ordered by time (the digits run from the year down to the millisecond).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RecordStamp(u64);

impl RecordStamp {
    /// Separators after each digit group of `YYYY-MM-DDTHH:MM:SS.mmmZ`.
    const SHAPE: &'static [u8; 24] = b"0000-00-00T00:00:00.000Z";

    /// A stamp from the engine's text, or `None` when it is not a
    /// millisecond UTC timestamp in the engine's shape.
    #[must_use]
    pub fn parse(s: &str) -> Option<RecordStamp> {
        let b = s.as_bytes();
        if b.len() != Self::SHAPE.len() {
            return None;
        }
        let mut n: u64 = 0;
        for (c, shape) in b.iter().zip(Self::SHAPE) {
            if *shape == b'0' {
                if !c.is_ascii_digit() {
                    return None;
                }
                n = n * 10 + u64::from(c - b'0');
            } else if c != shape {
                return None;
            }
        }
        Some(RecordStamp(n))
    }

    /// The text, exactly as received.
    #[must_use]
    pub fn text(&self) -> String {
        let digits = format!("{:017}", self.0);
        let mut d = digits.bytes();
        Self::SHAPE
            .iter()
            .map(|s| {
                if *s == b'0' {
                    char::from(d.next().unwrap_or(b'0'))
                } else {
                    char::from(*s)
                }
            })
            .collect()
    }

    /// A parsed `updatedAt` as a stamp (the engine's millisecond shape;
    /// `None` for a value with sub-millisecond digits, which the engine
    /// never writes).
    #[must_use]
    pub fn of_timestamp(t: cl_core::Timestamp) -> Option<RecordStamp> {
        RecordStamp::parse(&t.to_wire())
    }

    /// The stamp as a timestamp, for records that keep their `updatedAt`
    /// parsed (payments, invoices).
    #[must_use]
    pub fn timestamp(self) -> Option<cl_core::Timestamp> {
        cl_core::Timestamp::parse_wire(&self.text()).ok()
    }

    /// `updatedAt` of a record object, when it is a stamp.
    #[must_use]
    pub fn of_record(v: &Value) -> Option<RecordStamp> {
        v.get("updatedAt")
            .and_then(Value::as_str)
            .and_then(RecordStamp::parse)
    }
}

impl std::fmt::Debug for RecordStamp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RecordStamp({})", self.text())
    }
}
/// Adds `expectedUpdatedAt` to a JSON object body (U-291). A body that is
/// not an object is returned unchanged.
#[must_use]
pub fn with_expected_stamp(body: &[u8], stamp: RecordStamp) -> Vec<u8> {
    match serde_json::from_slice::<Value>(body) {
        Ok(Value::Object(mut map)) => {
            map.insert("expectedUpdatedAt".to_string(), Value::String(stamp.text()));
            serde_json::to_vec(&Value::Object(map)).unwrap_or_else(|_| body.to_vec())
        }
        _ => body.to_vec(),
    }
}

/// Whether the engine checks record stamps itself (`stale-all-v1`, U-291).
/// Every stale-checked write sends its stamp then (LAN mode refuses one
/// without it, 428 `STALE_REQUIRED`, U-292).
pub(crate) fn engine_checks_stamps(session: &Session) -> bool {
    session
        .client()
        .capabilities()
        .has(crate::wire::Capabilities::STALE_ALL)
}

/// Adds `expectedUpdatedAt` to a JSON object body when the engine checks
/// stamps and the stamp is known (U-291; the body for PUT, POST and the
/// transaction DELETE).
pub(crate) fn stamp_body(session: &Session, body: &mut Value, stamp: Option<RecordStamp>) {
    if let (Some(s), true, Value::Object(map)) = (stamp, engine_checks_stamps(session), body) {
        map.insert("expectedUpdatedAt".to_string(), Value::String(s.text()));
    }
}

/// Gives a DELETE the JSON body `{expectedUpdatedAt}` when the engine
/// checks stamps and the stamp is known (U-291). The body, not the query:
/// the engine's `expectStamp` reads the body first and `DELETE
/// /transactions/{id}` reads only the body (review S3), so the body is the
/// one place every `expectStamp` DELETE reads. Routes whose handler parses
/// the query instead (vendor layout templates; `version` on budgets,
/// report definitions and templates) set their own query.
pub(crate) fn stamp_delete(session: &Session, call: &mut Call, stamp: Option<RecordStamp>) {
    if let (Some(s), true) = (stamp, engine_checks_stamps(session)) {
        let body = serde_json::json!({ "expectedUpdatedAt": s.text() });
        // A stamp is no secret, so the call's `sensitive` flag stays.
        call.body = Some(zeroize::Zeroizing::new(
            serde_json::to_vec(&body).unwrap_or_default(),
        ));
    }
}

/// Per-item stamps of a bulk write (`stamps {id: updatedAt}`, U-291): only
/// the known ones, and only when the engine checks stamps. No known stamp
/// sends no map: `{}` would pass LAN mode's 428 while comparing nothing,
/// so a caller that read no stamps is refused there instead of silently
/// unguarded.
pub(crate) fn stamp_items(
    session: &Session,
    body: &mut Value,
    stamps: impl IntoIterator<Item = (String, Option<RecordStamp>)>,
) {
    if !engine_checks_stamps(session) {
        return;
    }
    let map: serde_json::Map<String, Value> = stamps
        .into_iter()
        .filter_map(|(id, s)| s.map(|s| (id, Value::String(s.text()))))
        .collect();
    if let (false, Value::Object(b)) = (map.is_empty(), body) {
        b.insert("stamps".to_string(), Value::Object(map));
    }
}

impl std::fmt::Debug for RecordVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RecordVersion::Server(v) => write!(f, "Server({v})"),
            RecordVersion::Stamped { stamp, .. } => write!(f, "Stamped({})", stamp.text()),
            RecordVersion::Stamp(s) => write!(f, "Stamp({})", s.text()),
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
    /// The `NEGATIVE_STOCK` warnings of the save that answered with this
    /// record (`warnings[]` beside it, `inventory-costing-v1`); empty on a
    /// read. Never blocking (Q-148).
    pub stock_warnings: Vec<crate::resources::inventory::NegativeStock>,
}

impl<T> Versioned<T> {
    /// A record read at `version` (no warnings).
    pub fn new(record: T, version: RecordVersion) -> Versioned<T> {
        Versioned {
            record,
            version,
            stock_warnings: Vec::new(),
        }
    }
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
    let stock_warnings = crate::resources::inventory::stock_warnings(&root);
    let inner = root
        .get_mut(key)
        .map(Value::take)
        .ok_or(ApiError::Decode { what: key })?;
    let server = inner.get("version").and_then(Value::as_u64);
    let stamp = RecordStamp::of_record(&inner);
    let record: T = serde_json::from_value(inner).map_err(|_| ApiError::Decode { what: key })?;
    let version = match (server, stamp) {
        (Some(v), _) => RecordVersion::Server(v),
        (None, Some(stamp)) => RecordVersion::Stamped {
            fingerprint: record.fingerprint(),
            stamp,
        },
        (None, None) => RecordVersion::Fingerprint(record.fingerprint()),
    };
    Ok(Versioned {
        record,
        version,
        stock_warnings,
    })
}

/// [`versioned_from`] for the 2xx answer of a mutation: the save is
/// recorded, so an answer that cannot be read is `SavedUnreadable` ("Saved —
/// reload to see it"), never the generic internal error.
pub(crate) fn saved_from<T>(body: &[u8], key: &'static str) -> Result<Versioned<T>, ApiError>
where
    T: DeserializeOwned + EditableFields,
{
    versioned_from(body, key).map_err(saved_unreadable)
}

/// Turns a `Decode` failure of a 2xx save answer into `SavedUnreadable`.
pub(crate) fn saved_unreadable(e: ApiError) -> ApiError {
    match e {
        ApiError::Decode { what } => ApiError::SavedUnreadable { what },
        other => other,
    }
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
pub(crate) struct GuardedUpdate {
    pub get: &'static Operation,
    pub put: &'static Operation,
    pub path: String,
    pub key: &'static str,
    pub body: Vec<u8>,
    pub idempotency_key: IdempotencyKey,
    /// Fingerprint the record has once this update committed.
    pub requested: [u8; 32],
    /// Fingerprint of a raw record (outcome reconciliation).
    pub fp: RowFp,
    /// The update moves the ledger-version stamp (postings).
    pub stamp_covered: bool,
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
    u: GuardedUpdate,
) -> Result<Versioned<T>, ApiError>
where
    T: DeserializeOwned + EditableFields,
{
    let key = u.key;
    let reply = update_guarded_reply::<T>(session, expected, u, None).await?;
    saved_from(&reply.body, key)
}

/// [`update_guarded`] that returns the raw answer, for a write whose route
/// differs from the read's (`put_path`, for example `PUT /checks/{id}`
/// guarded and reconciled by `GET /transactions/{id}`) and whose answer
/// sits under another key. `T` is the model the pre-read and the version
/// are computed from.
pub(crate) async fn update_guarded_reply<T>(
    session: &Session,
    expected: &RecordVersion,
    u: GuardedUpdate,
    put_path: Option<String>,
) -> Result<EngineReply, ApiError>
where
    T: DeserializeOwned + EditableFields,
{
    // The read, the probe and the write all belong to one company.
    let session = &session.pinned();
    let put_path = put_path.unwrap_or_else(|| u.path.clone());
    // U-291: with `stale-all-v1` the engine checks the stamp itself, under
    // the record's lock; without it the fingerprint pre-read below runs.
    let engine_checks = expected.stamp().filter(|_| {
        session
            .client()
            .capabilities()
            .has(crate::wire::Capabilities::STALE_ALL)
    });
    let body = match engine_checks {
        Some(s) => with_expected_stamp(&u.body, s),
        None => u.body,
    };
    let mut call = Call::new(u.put.method(), u.put.route(), put_path).json_body(body);
    call.idempotency_key = Some(u.idempotency_key);
    let pre_image = match expected {
        RecordVersion::Server(v) => {
            call.if_match = Some(format!("\"{v}\""));
            None
        }
        _ if engine_checks.is_some() => None,
        RecordVersion::Stamp(s) => {
            // Only a stamp and an engine that doesn't check it: compare it
            // with a fresh read.
            let current: Versioned<T> =
                get_versioned(session, u.get, u.path.clone(), u.key).await?;
            if current.version.stamp() != Some(*s) {
                return Err(ApiError::Conflict {
                    reason: ConflictReason::Stale,
                    current_version: Some(current.version),
                    code: None,
                    message: UserText::default(),
                    details: crate::ConflictDetails::default(),
                });
            }
            Some(current.record.fingerprint())
        }
        RecordVersion::Fingerprint(f) | RecordVersion::Stamped { fingerprint: f, .. } => {
            let current: Versioned<T> =
                get_versioned(session, u.get, u.path.clone(), u.key).await?;
            let now = current.record.fingerprint();
            if &now != f {
                return Err(ApiError::Conflict {
                    reason: ConflictReason::Stale,
                    current_version: Some(match current.version {
                        RecordVersion::Server(_) => RecordVersion::Fingerprint(now),
                        v => v,
                    }),
                    code: None,
                    message: UserText::default(),
                    details: crate::ConflictDetails::default(),
                });
            }
            Some(now)
        }
    };
    let matcher = OutcomeMatcher::Updated {
        get: u.get,
        path: u.path,
        key: u.key,
        fp: u.fp,
        requested: u.requested,
        pre_image,
    };
    let probe = Probe::new(session, matcher, u.stamp_covered).await;
    session.send(u.put, call, Some(&probe)).await
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

    fn update(body: &str) -> GuardedUpdate {
        GuardedUpdate {
            get: find(Method::Get, ROUTE).expect("get"),
            put: find(Method::Put, ROUTE).expect("put"),
            path: PATH.to_string(),
            key: "transaction",
            body: body.as_bytes().to_vec(),
            idempotency_key: IdempotencyKey::new(),
            requested: [0; 32],
            fp: crate::resources::reconcile::row_fp::<Sample>,
            stamp_covered: false,
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

    /// P33 U-291: a read keeps the record's stamp; with `stale-all-v1` the
    /// update sends it as `expectedUpdatedAt` and skips the pre-read; a 409
    /// `STALE` brings the stamp now; without the capability the
    /// fingerprint check runs as before.
    #[tokio::test]
    async fn stamps_are_carried_sent_under_stale_all_and_compared_by_content() {
        const STAMP: &str = "2026-11-19T10:00:00.123Z";
        let rec = sample_json(("l1", "l2"), (0, 1), "rent", STAMP);
        let v: Versioned<Sample> =
            versioned_from(wrap(&rec).as_bytes(), "transaction").expect("read");
        let stamp = v.version.stamp().expect("stamp");
        assert_eq!(stamp.text(), STAMP);
        assert!(RecordStamp::parse("2026-11-19 10:00:00.123Z").is_none());
        // Equality goes by content, whatever the stamp.
        assert_eq!(v.version, RecordVersion::Fingerprint(fp(&rec)));
        assert!(RecordStamp::parse("").is_none());
        assert!(RecordStamp::parse("a b").is_none());
        assert!(RecordStamp::parse(&"x".repeat(49)).is_none());

        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(wiremock::matchers::body_partial_json(
                serde_json::json!({"expectedUpdatedAt": STAMP}),
            ))
            .respond_with(json(200, &wrap(&rec)))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .respond_with(json(
                409,
                r#"{"error":{"code":"STALE","message":"changed",
                    "details":{"updatedAt":"2026-11-19T10:00:01.000Z"}}}"#,
            ))
            .mount(&server)
            .await;
        let s = crate::resources::testkit::session_with_caps(
            &server,
            &[crate::wire::Capabilities::STALE_ALL],
        )
        .await;
        let ok: Versioned<Sample> = update_guarded(&s, &v.version, update(r#"{"memo":"m"}"#))
            .await
            .expect("saved");
        assert_eq!(ok.version.stamp().map(|s| s.text()), Some(STAMP.into()));
        let e = update_guarded::<Sample>(&s, &v.version, update(r#"{"memo":"n"}"#))
            .await
            .expect_err("stale");
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
            current_version.and_then(|c| c.stamp()).map(|s| s.text()),
            Some("2026-11-19T10:00:01.000Z".to_string())
        );
        let reqs = server.received_requests().await.expect("r");
        // The engine checks: no pre-read.
        assert_eq!(
            reqs.iter()
                .filter(|r| r.method.as_str() == "GET" && r.url.path() != "/api/v1/desktop/info")
                .count(),
            0
        );

        // Without the capability: no stamp in the body; the pre-read runs.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(PATH))
            .respond_with(json(200, &wrap(&rec)))
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .respond_with(json(200, &wrap(&rec)))
            .mount(&server)
            .await;
        let s = company_session(&server);
        let _: Versioned<Sample> = update_guarded(&s, &v.version, update(r#"{"memo":"m"}"#))
            .await
            .expect("saved");
        let reqs = server.received_requests().await.expect("r");
        let put = reqs
            .iter()
            .find(|r| r.method.as_str() == "PUT")
            .expect("put");
        assert!(!String::from_utf8_lossy(&put.body).contains("expectedUpdatedAt"));
        assert_eq!(
            reqs.iter().filter(|r| r.method.as_str() == "GET").count(),
            1
        );
        // A save's stamp replaces the old one.
        let next = RecordStamp::parse("2026-11-19T10:00:02.000Z").expect("s");
        assert_eq!(
            RecordVersion::Fingerprint([1; 32])
                .with_stamp(next, [0; 32])
                .stamp(),
            Some(next)
        );
        assert_eq!(
            with_expected_stamp(b"[1]", next),
            b"[1]".to_vec(),
            "only objects take a stamp"
        );
    }

    #[test]
    fn malformed_records_are_decode_errors() {
        assert!(versioned_from::<Sample>(b"{}", "transaction").is_err());
        assert!(versioned_from::<Sample>(br#"{"transaction":{"id":1}}"#, "transaction").is_err());
        let money_as_number = r#"{"transaction":{"id":"t","memo":null,"lines":[{"id":"l","accountId":"a","debit":10.5,"credit":"0.0000","lineOrder":0}]}}"#;
        assert!(versioned_from::<Sample>(money_as_number.as_bytes(), "transaction").is_err());
    }
}
