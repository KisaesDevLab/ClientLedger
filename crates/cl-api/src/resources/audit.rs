//! The audit trail of one record (`GET /api/v1/audit-log`, P12 plan WP2).
//!
//! - The engine filters by entity type and by text in the before/after
//!   images; it has no record-id filter, so this reader asks for the
//!   record's id as text and keeps only the rows whose `entity_id` is the
//!   record. A row whose images do not mention the id (the engine's create
//!   row for a transaction) is not found this way.
//! - Before and after images are owner data: [`AuditImage`] keeps the raw
//!   JSON with a redacting `Debug`. [`AuditImage::lines`] reads the lines of
//!   a transaction image (P12 R6) with exact money; a JSON number is refused
//!   (`InexactMoney`).

use std::fmt;

use cl_core::ids::{AccountId, ContactId, TagId, UserId};
use cl_core::{Money, Timestamp};
use serde::Deserialize;
use serde_json::Value;

use crate::client::{Call, Method};
use crate::errors::ApiError;
use crate::resources::operations::AUDIT_LOG;
use crate::session::{Session, decode};

/// Rows read per audit page.
const AUDIT_PAGE: u32 = 200;
/// Pages read at most for one record (2,000 rows).
const AUDIT_PAGES: u32 = 10;

/// Which kind of record.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AuditEntity {
    /// A transaction (create, update, void, delete).
    Transaction,
    /// A permitted change in a closed period (U-15: who, which transaction,
    /// old and new date, how).
    ClosingDateOverride,
}

impl AuditEntity {
    const fn as_str(self) -> &'static str {
        match self {
            AuditEntity::Transaction => "transaction",
            AuditEntity::ClosingDateOverride => "closing_date_override",
        }
    }
}

/// What happened.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum AuditAction {
    /// Created.
    Create,
    /// Changed.
    Update,
    /// Voided.
    Void,
    /// Deleted.
    Delete,
    /// A closed-period change was allowed.
    Override,
    /// Anything else (the engine's word, kept for display).
    Other(String),
}

impl AuditAction {
    fn parse(s: &str) -> AuditAction {
        match s {
            "create" => AuditAction::Create,
            "update" => AuditAction::Update,
            "void" => AuditAction::Void,
            "delete" => AuditAction::Delete,
            "override" => AuditAction::Override,
            other => AuditAction::Other(other.to_string()),
        }
    }
}

/// A before or after image: owner data, never logged.
#[derive(Clone, PartialEq, Eq)]
pub struct AuditImage(Value);

impl fmt::Debug for AuditImage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AuditImage(..)")
    }
}

/// One line of a transaction image.
#[derive(Clone, PartialEq, Eq)]
pub struct AuditLine {
    /// Account.
    pub account: AccountId,
    /// Debit.
    pub debit: Money,
    /// Credit.
    pub credit: Money,
    /// Description (owner data).
    pub description: Option<String>,
    /// Tag.
    pub tag: Option<TagId>,
    /// Line contact.
    pub contact: Option<ContactId>,
    /// Position.
    pub line_order: Option<i64>,
}

impl fmt::Debug for AuditLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AuditLine")
            .field("account", &self.account)
            .field("line_order", &self.line_order)
            .finish_non_exhaustive()
    }
}

fn money(v: &Value, what: &'static str) -> Result<Money, ApiError> {
    match v {
        Value::String(s) => Money::parse_wire(s).map_err(|_| ApiError::Decode { what }),
        Value::Number(_) => Err(ApiError::InexactMoney { what }),
        _ => Err(ApiError::Decode { what }),
    }
}

fn opt_id<T: std::str::FromStr>(v: &Value, key: &str) -> Result<Option<T>, ApiError> {
    match v.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => s
            .parse()
            .map(Some)
            .map_err(|_| ApiError::Decode { what: "audit line" }),
        Some(_) => Err(ApiError::Decode { what: "audit line" }),
    }
}

impl AuditImage {
    /// The raw image (owner data: never log it).
    #[must_use]
    pub fn raw(&self) -> &Value {
        &self.0
    }

    /// The lines of a transaction image, in `lineOrder` order. `None` when
    /// the image has no lines (engines before P12 R6 kept only the header).
    ///
    /// # Errors
    /// `InexactMoney` for a JSON-number amount; `Decode` for any other
    /// shape problem.
    pub fn lines(&self) -> Result<Option<Vec<AuditLine>>, ApiError> {
        let Some(raw) = self.0.get("lines") else {
            return Ok(None);
        };
        let rows = raw.as_array().ok_or(ApiError::Decode {
            what: "audit lines",
        })?;
        let mut out = rows
            .iter()
            .map(|l| {
                Ok(AuditLine {
                    account: opt_id(l, "accountId")?
                        .ok_or(ApiError::Decode { what: "audit line" })?,
                    debit: money(l.get("debit").unwrap_or(&Value::Null), "audit debit")?,
                    credit: money(l.get("credit").unwrap_or(&Value::Null), "audit credit")?,
                    description: l
                        .get("description")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    tag: opt_id(l, "tagId")?,
                    contact: opt_id(l, "contactId")?,
                    line_order: l.get("lineOrder").and_then(Value::as_i64),
                })
            })
            .collect::<Result<Vec<_>, ApiError>>()?;
        out.sort_by_key(|l| l.line_order.unwrap_or(0));
        Ok(Some(out))
    }
}

/// One audit row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuditEntry {
    /// Row id (the engine's row counter).
    pub id: u64,
    /// What happened.
    pub action: AuditAction,
    /// When.
    pub at: Timestamp,
    /// Who (`None` for system changes).
    pub user: Option<UserId>,
    /// The record before.
    pub before: Option<AuditImage>,
    /// The record after.
    pub after: Option<AuditImage>,
}

#[derive(Deserialize)]
struct WireRow {
    /// `bigserial`: a JSON string of digits from the raw query, or a number.
    id: Value,
    action: String,
    entity_id: Option<String>,
    #[serde(default)]
    user_id: Option<UserId>,
    #[serde(default)]
    before_data: Option<Value>,
    #[serde(default)]
    after_data: Option<Value>,
    created_at: String,
}

#[derive(Deserialize)]
struct WirePage {
    data: Vec<WireRow>,
    total: u64,
}

/// The engine stores each image as a JSON string inside the `jsonb`
/// column, so the row usually carries the image as text; both forms are
/// read.
fn image(v: Option<Value>) -> Option<AuditImage> {
    match v? {
        Value::Null => None,
        Value::String(text) => Some(AuditImage(
            serde_json::from_str(&text).unwrap_or(Value::String(text)),
        )),
        other => Some(AuditImage(other)),
    }
}

/// The audit time: RFC 3339, or the PostgreSQL text form the engine's raw
/// query sends (`2026-10-19 17:36:05.208999-05`: a space before the time
/// and an hour-only offset), which is rewritten to RFC 3339 first.
fn audit_time(text: &str) -> Option<Timestamp> {
    if let Ok(t) = Timestamp::parse_wire(text) {
        return Some(t);
    }
    if text.as_bytes().get(10) != Some(&b' ') || !text.is_ascii() {
        return None;
    }
    let mut s = format!("{}T{}", &text[..10], &text[11..]);
    // The offset starts at the last sign after the time.
    let sign = s.rfind(['+', '-']).filter(|&i| i >= 19)?;
    let offset = &s[sign + 1..];
    let fixed = match offset.len() {
        2 => format!("{offset}:00"),
        4 if !offset.contains(':') => format!("{}:{}", &offset[..2], &offset[2..]),
        _ => offset.to_string(),
    };
    s.replace_range(sign + 1.., &fixed);
    Timestamp::parse_wire(&s).ok()
}

/// Audit trail reads of a session (`session.audit()`).
#[derive(Clone, Copy, Debug)]
pub struct Audit<'a>(pub(crate) &'a Session);

impl Session {
    /// The audit trail of the open company.
    #[must_use]
    pub fn audit(&self) -> Audit<'_> {
        Audit(self)
    }
}

impl Audit<'_> {
    /// The audit rows of one record, newest first.
    ///
    /// # Errors
    /// `Decode` when a row cannot be read; `Forbidden` without audit
    /// permission; any [`ApiError`].
    pub async fn for_record(
        &self,
        entity: AuditEntity,
        id: uuid::Uuid,
    ) -> Result<Vec<AuditEntry>, ApiError> {
        let id_text = id.to_string();
        let mut out = Vec::new();
        let mut offset: u64 = 0;
        for _ in 0..AUDIT_PAGES {
            let mut call = Call::at(Method::Get, AUDIT_LOG.route());
            call.query = vec![
                ("entity_type".into(), entity.as_str().into()),
                ("search".into(), id_text.clone()),
                ("limit".into(), AUDIT_PAGE.to_string()),
                ("offset".into(), offset.to_string()),
            ];
            let reply = self.0.send(&AUDIT_LOG, call, None).await?;
            let page: WirePage = decode(&reply, "audit log")?;
            let got = u64::try_from(page.data.len()).unwrap_or(u64::MAX);
            for row in page.data {
                if row.entity_id.as_deref() != Some(id_text.as_str()) {
                    continue;
                }
                out.push(AuditEntry {
                    id: match &row.id {
                        Value::String(t) => t.parse().ok(),
                        Value::Number(n) => n.as_u64(),
                        _ => None,
                    }
                    .ok_or(ApiError::Decode { what: "audit id" })?,
                    action: AuditAction::parse(&row.action),
                    at: audit_time(&row.created_at)
                        .ok_or(ApiError::Decode { what: "audit time" })?,
                    user: row.user_id,
                    before: image(row.before_data),
                    after: image(row.after_data),
                });
            }
            offset += got;
            if got == 0 || offset >= page.total {
                return Ok(out);
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::testkit::{company_session, json, query, requests};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer};

    const TXN: &str = "7a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
    const OTHER: &str = "7b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
    const ACCT: &str = "2a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";

    fn row(id: &str, action: &str, entity: &str, before: &str) -> String {
        format!(
            r#"{{"id":"{id}","tenant_id":"t","user_id":null,"action":"{action}",
               "entity_type":"transaction","entity_id":"{entity}","before_data":{before},
               "after_data":null,"ip_address":null,"user_agent":null,
               "created_at":"2026-10-19 12:00:00.123456-05"}}"#
        )
    }

    #[tokio::test]
    async fn rows_of_the_record_with_exact_before_lines() {
        let server = MockServer::start().await;
        let before = format!(
            r#"{{"id":"{TXN}","memo":"Secret memo","lines":[
                {{"accountId":"{ACCT}","debit":"0.0000","credit":"12.3456","lineOrder":1,"description":"Owner text"}},
                {{"accountId":"{ACCT}","debit":"12.3456","credit":"0.0000","lineOrder":0,"tagId":null}}]}}"#
        );
        let body = format!(
            r#"{{"data":[{},{},{}],"total":3}}"#,
            row("101", "void", TXN, &before),
            row("102", "update", OTHER, "null"),
            row(
                "103",
                "update",
                TXN,
                // As the engine stores it: the image as a JSON string.
                &serde_json::to_string(&format!(r#"{{"id":"{TXN}"}}"#)).expect("text")
            ),
        );
        Mock::given(method("GET"))
            .and(path("/api/v1/audit-log"))
            .respond_with(json(200, &body))
            .mount(&server)
            .await;
        let s = company_session(&server);
        let rows = s
            .audit()
            .for_record(
                AuditEntity::Transaction,
                uuid::Uuid::parse_str(TXN).expect("id"),
            )
            .await
            .expect("audit");
        assert_eq!(rows.len(), 2, "only this record's rows");
        assert_eq!(rows[0].action, AuditAction::Void);
        assert_eq!((rows[0].id, rows[1].id), (101, 103));
        let lines = rows[0]
            .before
            .as_ref()
            .expect("before")
            .lines()
            .expect("lines")
            .expect("has lines");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].debit.to_wire(), "12.3456");
        assert_eq!(lines[1].credit.to_wire(), "12.3456");
        let b1 = rows[1].before.as_ref().expect("b");
        assert_eq!(b1.raw()["id"], TXN, "string image decoded");
        assert_eq!(b1.lines(), Ok(None));
        let shown = format!("{rows:?} {lines:?}");
        assert!(
            !shown.contains("Secret") && !shown.contains("12.3456") && !shown.contains("Owner")
        );
        let q = query(&requests(&server, "GET").await[0]);
        assert!(q.contains("entity_type=transaction"), "{q}");
        assert!(q.contains(&format!("search={TXN}")), "{q}");
    }

    #[test]
    fn audit_times_in_both_forms() {
        let want = Timestamp::parse_wire("2026-10-19T22:36:05.208999Z").expect("t");
        for text in [
            "2026-10-19 17:36:05.208999-05",
            "2026-10-19 17:36:05.208999-0500",
            "2026-10-19 17:36:05.208999-05:00",
            "2026-10-19T22:36:05.208999Z",
        ] {
            assert_eq!(audit_time(text), Some(want), "{text}");
        }
        assert_eq!(
            audit_time("2026-10-19 22:36:05+00"),
            Timestamp::parse_wire("2026-10-19T22:36:05Z").ok()
        );
        for bad in [
            "2026-10-19",
            "2026-10-19 17:36:05",
            "yesterday",
            "2026-10-19 17:36:05.2-5",
        ] {
            assert_eq!(audit_time(bad), None, "{bad}");
        }
    }

    #[test]
    fn number_money_in_an_image_is_refused() {
        let img = AuditImage(serde_json::json!({
            "lines": [{"accountId": ACCT, "debit": 12, "credit": "0.0000"}]
        }));
        assert!(matches!(img.lines(), Err(ApiError::InexactMoney { .. })));
        let img = AuditImage(serde_json::json!({"lines": "x"}));
        assert!(matches!(img.lines(), Err(ApiError::Decode { .. })));
        assert_eq!(
            AuditAction::parse("restore"),
            AuditAction::Other("restore".into())
        );
        assert_eq!(AuditAction::parse("override"), AuditAction::Override);
    }
}
