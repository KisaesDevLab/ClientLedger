//! The Controls reports (P28 WP3): Closing Date Exceptions (U-204,
//! `closing-exceptions-v1`, Q-P28q) and the Audit Trail (`audit-filters-v1`)
//! in plain words. Before and after values are the engine's, shown as
//! given and never computed.

use cl_core::ids::TransactionId;
use cl_core::report::{
    Cell, ColumnKind, Drill, ReportMeta, ReportRequest, ReportTable, RowKind, RowPath, Truncated,
};
use serde_json::Value;

use super::exact::{decimal, list, shape, text};
use super::lists::{F, read};
use super::{col, row};
use crate::errors::ApiError;

fn meta(req: &ReportRequest) -> ReportMeta {
    ReportMeta {
        dates: Some(req.dates.clone()),
        ..ReportMeta::default()
    }
}

fn opt_amount(e: &Value, k: &'static str) -> Result<Cell, ApiError> {
    match e.get(k) {
        None | Some(Value::Null) => Ok(Cell::Empty),
        Some(Value::String(s)) => decimal(s, k).map(Cell::Amount),
        _ => Err(shape(k)),
    }
}

/// Closing Date Exceptions: each change in a closed period, oldest first,
/// with the closing date in effect then, how it was allowed, and the
/// engine's before and after amounts.
///
/// # Errors
/// [`ApiError::ReportShape`] when an amount is not exact text.
pub fn closing_exceptions(v: &Value, req: &ReportRequest) -> Result<ReportTable, ApiError> {
    let columns = vec![
        col("changed", "@reports.col.changed_on", ColumnKind::Date),
        col("user", "@reports.col.user", ColumnKind::Text),
        col("action", "@reports.col.action", ColumnKind::Text),
        col("type", "@reports.col.type", ColumnKind::Text),
        col("number", "@reports.col.number", ColumnKind::Text),
        col("date_before", "@reports.col.date_before", ColumnKind::Date),
        col("date_after", "@reports.col.date_after", ColumnKind::Date),
        col(
            "closing_date",
            "@reports.col.closing_date",
            ColumnKind::Date,
        ),
        col("method", "@reports.col.method", ColumnKind::Text),
        col(
            "amount_before",
            "@reports.col.amount_before",
            ColumnKind::Amount,
        ),
        col(
            "amount_after",
            "@reports.col.amount_after",
            ColumnKind::Amount,
        ),
    ];
    let mut rows = Vec::new();
    for (i, e) in list(v, "data")?.iter().enumerate() {
        let mut r = row(
            RowKind::Detail,
            1,
            RowPath::of(["rows", i.to_string().as_str()]),
            String::new(),
            vec![
                read(e, F::Date("at"))?,
                read(e, F::Text("userName"))?,
                read(e, F::Code("reports.audit.action", "action"))?,
                read(e, F::TxnType("txnType"))?,
                read(e, F::Text("txnNumber"))?,
                read(e, F::Date("txnDateBefore"))?,
                read(e, F::Date("txnDateAfter"))?,
                read(e, F::Date("closingDate"))?,
                read(e, F::Code("reports.closing.method", "method"))?,
                opt_amount(e, "amountBeforeExact")?,
                opt_amount(e, "amountAfterExact")?,
            ],
        );
        r.drill = super::exact::id(e, "transactionId", TransactionId::parse)?.map(|id| {
            Drill::Transaction {
                id,
                txn_type: text(e, "txnType").unwrap_or("").to_string(),
            }
        });
        rows.push(r);
    }
    let mut m = meta(req);
    if v.get("truncated") == Some(&Value::Bool(true)) {
        m.truncated = Some(Truncated {
            shown: rows.len() as u64,
            total: v.get("total").and_then(Value::as_u64).unwrap_or(0),
        });
    }
    Ok(ReportTable {
        columns,
        rows,
        meta: m,
    })
}

/// A value of an audit image in plain words: text and numbers as given
/// (amounts are the engine's text), yes or no, and "see the record" for
/// lists and objects.
fn audit_value(v: Option<&Value>) -> Cell {
    match v {
        None | Some(Value::Null) => Cell::Empty,
        Some(Value::String(s)) if s.is_empty() => Cell::Empty,
        Some(Value::String(s)) => Cell::Text(s.clone()),
        Some(Value::Number(n)) => Cell::Text(n.to_string()),
        Some(Value::Bool(b)) => Cell::Text(
            if *b {
                "@reports.audit.yes"
            } else {
                "@reports.audit.no"
            }
            .into(),
        ),
        Some(Value::Array(_) | Value::Object(_)) => Cell::Text("@reports.audit.details".into()),
    }
}

/// Fields the audit trail names in plain words (`@reports.audit.field.*`,
/// camelCase); any other field shows its own name.
pub const AUDIT_FIELDS: [&str; 16] = [
    "txnDate",
    "txnNumber",
    "total",
    "memo",
    "status",
    "contactId",
    "dueDate",
    "lines",
    "name",
    "accountNumber",
    "displayName",
    "closingDate",
    "isActive",
    "balanceDue",
    "amountPaid",
    "voidReason",
];

/// Bookkeeping fields the trail leaves out, and taxpayer ids and secrets
/// (Q-P28p, non-negotiable 11), which never show.
const QUIET: [&str; 14] = [
    "taxId",
    "tax_id",
    "tin",
    "password",
    "passwordHash",
    "password_hash",
    "id",
    "tenantId",
    "tenant_id",
    "createdAt",
    "updatedAt",
    "created_at",
    "updated_at",
    "companyId",
];

/// Name tokens that mark a taxpayer id or a secret (S2): a field whose
/// name has one of them as a whole word (`ein`, `companyEin`, `smtp_pass`),
/// or runs them together (`apiKey`, `taxId`), never shows. Whole words are
/// compared, so `settings` or `reportKey` still show.
const SECRET_WORDS: [&str; 12] = [
    "ein", "tin", "ssn", "taxid", "pass", "password", "secret", "token", "apikey", "hash", "pin",
    "iban",
];

/// The field's name split into lower-case words (`smtpPassHash` → smtp,
/// pass, hash; `tax_id` → tax, id).
fn words(name: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut prev_lower = false;
    for ch in name.chars() {
        if ch == '_' || ch == '-' || ch == '.' {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            prev_lower = false;
            continue;
        }
        if ch.is_uppercase() && prev_lower && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
        prev_lower = ch.is_lowercase() || ch.is_ascii_digit();
        cur.extend(ch.to_lowercase());
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// True for a field that must never show in the trail.
fn secret(name: &str) -> bool {
    let w = words(name);
    let joined: String = w.concat();
    w.iter().any(|x| SECRET_WORDS.contains(&x.as_str()))
        || SECRET_WORDS.contains(&joined.as_str())
        || ["taxid", "apikey", "privatekey"]
            .iter()
            .any(|k| joined.contains(k))
}

fn camel(s: &str) -> String {
    let mut out = String::new();
    let mut up = false;
    for ch in s.chars() {
        if ch == '_' {
            up = true;
        } else if up {
            out.extend(ch.to_uppercase());
            up = false;
        } else {
            out.push(ch);
        }
    }
    out
}

/// A field's caption: the plain-words key, else the field's own name.
#[must_use]
pub fn field_label(k: &str) -> String {
    let c = camel(k);
    if AUDIT_FIELDS.contains(&c.as_str()) {
        format!("@reports.audit.field.{c}")
    } else {
        k.to_string()
    }
}

/// Records the trail names in plain words (`@reports.audit.entity.*`).
pub const AUDIT_ENTITIES: [&str; 15] = [
    "transaction",
    "account",
    "contact",
    "company",
    "tag",
    "item",
    "user",
    "bank_feed",
    "reconciliation",
    "budget",
    "closing_date_override",
    "report_email",
    "report_definition",
    "report_definition_group",
    "payment_applications",
];

/// A record kind's caption: the plain-words key, else its own name with
/// spaces.
#[must_use]
pub fn entity_label(e: &str) -> String {
    if AUDIT_ENTITIES.contains(&e) {
        format!("@reports.audit.entity.{e}")
    } else {
        e.replace('_', " ")
    }
}

/// A09 QA-4: a memorized report's trip through a report file in plain
/// words. The engine audits an export as `download` with `format:
/// 'clreport'`, and a save from a file as `create` / `update` with
/// `after.origin = 'import'`.
fn report_file_action(e: &Value, entity: &str) -> Option<&'static str> {
    if !matches!(entity, "report_definition" | "report_definition_group") {
        return None;
    }
    let imported = e.pointer("/after_data/origin").and_then(Value::as_str) == Some("import");
    match text(e, "action").unwrap_or("") {
        "download" => Some("@reports.audit.action.exported"),
        "create" if imported => Some("@reports.audit.action.imported"),
        "update" if imported => Some("@reports.audit.action.imported_replace"),
        _ => None,
    }
}

/// The Record cell: the record kind, with its name when the audit row
/// carries one (`after.name`, else `before.name`) for a memorized report
/// or a report group (A09 QA-4). Engine cf0cb239 doesn't put the name in
/// these rows yet, so today this is the kind alone.
fn record_label(e: &Value, entity: &str) -> String {
    let name = ["/after_data/name", "/before_data/name"]
        .iter()
        .find_map(|p| e.pointer(p).and_then(Value::as_str))
        .map(str::trim)
        .filter(|n| !n.is_empty());
    match (entity, name) {
        ("report_definition" | "report_definition_group", Some(n)) => {
            format!("@reports.audit.entity.{entity}_named {n}")
        }
        _ => entity_label(entity),
    }
}

/// The Audit Trail: each change newest first as a heading (when, who,
/// what, which record) and one line per changed field with its before and
/// after values from the audit images.
///
/// # Errors
/// [`ApiError::ReportShape`] when the rows are not a list.
pub fn audit_trail(v: &Value, req: &ReportRequest) -> Result<ReportTable, ApiError> {
    let columns = vec![
        col("when", "@reports.col.changed_on", ColumnKind::Date),
        col("user", "@reports.col.user", ColumnKind::Text),
        col("action", "@reports.col.action", ColumnKind::Text),
        col("record", "@reports.col.record", ColumnKind::Text),
        col("before", "@reports.col.before", ColumnKind::Text),
        col("after", "@reports.col.after", ColumnKind::Text),
    ];
    let mut rows = Vec::new();
    let data = list(v, "data")?;
    for (i, e) in data.iter().enumerate() {
        let k = i.to_string();
        let entity = text(e, "entity_type").unwrap_or("");
        let mut head = row(
            RowKind::Header,
            0,
            RowPath::of(["rows", k.as_str()]),
            String::new(),
            vec![
                read(e, F::Date("created_at"))?,
                read(e, F::Text("userName"))?,
                match report_file_action(e, entity) {
                    Some(key) => Cell::Text(key.into()),
                    None => read(e, F::Code("reports.audit.action", "action"))?,
                },
                Cell::Text(record_label(e, entity)),
                Cell::Empty,
                Cell::Empty,
            ],
        );
        if entity == "transaction" {
            let txn_type = e
                .pointer("/after_data/txnType")
                .or_else(|| e.pointer("/before_data/txnType"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            head.drill = super::exact::id(e, "entity_id", TransactionId::parse)?
                .map(|id| Drill::Transaction { id, txn_type });
        }
        head.expandable = true;
        let report_row = matches!(entity, "report_definition" | "report_definition_group");
        rows.push(head);
        let before = e.get("before_data").and_then(Value::as_object);
        let after = e.get("after_data").and_then(Value::as_object);
        let mut keys: Vec<&String> = before
            .into_iter()
            .chain(after)
            .flat_map(serde_json::Map::keys)
            .collect();
        keys.sort();
        keys.dedup();
        for key in keys {
            let b = before.and_then(|o| o.get(key.as_str()));
            let a = after.and_then(|o| o.get(key.as_str()));
            // An update lists what changed; a create or delete what it had.
            if (before.is_some() && after.is_some() && a == b)
                || QUIET.contains(&key.as_str())
                // A memorized report's action already says it came from a
                // file (A09 QA-4: "Imported").
                || (report_row && key == "origin")
                || secret(key)
            {
                continue;
            }
            rows.push(row(
                RowKind::Detail,
                1,
                RowPath::of(["rows", k.as_str(), key.as_str()]),
                field_label(key),
                vec![
                    Cell::Empty,
                    Cell::Empty,
                    Cell::Empty,
                    Cell::Empty,
                    audit_value(b),
                    audit_value(a),
                ],
            ));
        }
    }
    let mut m = meta(req);
    let total = v.get("total").and_then(Value::as_u64).unwrap_or(0);
    if total > data.len() as u64 {
        m.truncated = Some(Truncated {
            shown: data.len() as u64,
            total,
        });
    }
    Ok(ReportTable {
        columns,
        rows,
        meta: m,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cl_core::CalendarDate;
    use cl_core::report::{DateRange, ReportDates, Source};
    use serde_json::json;

    fn req(source: Source) -> ReportRequest {
        let d = |m, dd| CalendarDate::from_ymd(2026, m, dd).expect("d");
        ReportRequest::new(
            source,
            ReportDates::Range(DateRange {
                from: d(1, 1),
                to: d(12, 31),
            }),
        )
    }

    /// The plan's contract (PR-E, recorded sample): amounts are the
    /// engine's text; a create has no before amount.
    #[test]
    fn closing_exceptions_show_the_engine_s_before_and_after() {
        let v: Value = serde_json::from_str(include_str!(
            "../../../../tests/fixtures/reports/closing_date_exceptions.json"
        ))
        .expect("fixture");
        let t = closing_exceptions(&v, &req(Source::ClosingDateExceptions)).expect("table");
        assert_eq!(t.rows.len(), 2);
        assert_eq!(t.rows[0].cells[9], Cell::Empty, "a create has no before");
        assert_eq!(
            t.rows[1].cells[9],
            Cell::Amount(rust_decimal::Decimal::new(1_234_000, 4))
        );
        assert!(matches!(t.rows[1].drill, Some(Drill::Transaction { .. })));
        assert!(t.meta.truncated.is_none());
        let bad = json!({"data": [{"amountBeforeExact": 1234}]});
        assert!(closing_exceptions(&bad, &req(Source::ClosingDateExceptions)).is_err());
    }

    #[test]
    fn the_audit_trail_lists_changed_fields_in_plain_words() {
        let v = json!({"data": [{
            "id": "a1", "action": "update", "entity_type": "transaction",
            "entity_id": "7f3a2c10-5b6d-4e8f-9a0b-1c2d3e4f5a60", "userName": "Owner",
            "created_at": "2026-11-05T10:00:00.000Z",
            "before_data": {"txnDate": "2026-09-15", "memo": "x", "total": "12.3400", "updatedAt": "t1"},
            "after_data": {"txnDate": "2026-09-16", "memo": "x", "total": "12.3400", "updatedAt": "t2", "txnType": "journal_entry"}
        }], "total": 3});
        let t = audit_trail(&v, &req(Source::AuditLog)).expect("table");
        let captions: Vec<&str> = t.rows.iter().map(|r| r.caption.as_str()).collect();
        assert_eq!(captions, ["", "@reports.audit.field.txnDate", "txnType"]);
        assert_eq!(t.rows[1].cells[4], Cell::Text("2026-09-15".into()));
        assert_eq!(t.rows[1].cells[5], Cell::Text("2026-09-16".into()));
        assert!(matches!(
            &t.rows[0].drill,
            Some(Drill::Transaction { txn_type, .. }) if txn_type == "journal_entry"
        ));
        assert_eq!(t.meta.truncated.map(|x| x.total), Some(3));
        assert_eq!(field_label("txn_number"), "@reports.audit.field.txnNumber");
    }

    /// A09 QA-4: report-file exports read "Exported", saves from a file
    /// "Imported", groups "Report group", and the report is named when the
    /// row carries a name. The rows are the engine's (cf0cb239) shapes.
    #[test]
    fn report_file_rows_read_in_plain_words() {
        let row = |action: &str, entity: &str, before: Value, after: Value| {
            json!({"id": "a", "action": action, "entity_type": entity,
                   "entity_id": "7f3a2c10-5b6d-4e8f-9a0b-1c2d3e4f5a60", "userName": "Owner",
                   "created_at": "2026-11-13T10:00:00.000Z",
                   "before_data": before, "after_data": after})
        };
        let v = json!({"data": [
            row("download", "report_definition", Value::Null,
                json!({"kind": "builder", "reportKey": null, "format": "clreport"})),
            row("create", "report_definition", Value::Null,
                json!({"kind": "builder", "reportKey": null, "groupId": null, "version": 1, "origin": "import"})),
            row("update", "report_definition", json!({"kind": "builder", "version": 2}),
                json!({"kind": "builder", "version": 3, "origin": "import"})),
            row("create", "report_definition", Value::Null,
                json!({"kind": "standard", "version": 1})),
            row("create", "report_definition_group", Value::Null, json!({"sort": 0, "version": 1})),
            row("update", "report_definition", json!({"name": "Old"}),
                json!({"name": "Bank pack", "version": 2})),
            row("download", "attachment", Value::Null, json!({"fileName": "x"})),
        ], "total": 7});
        let t = audit_trail(&v, &req(Source::AuditLog)).expect("table");
        let heads: Vec<(String, String)> = t
            .rows
            .iter()
            .filter(|r| r.kind == RowKind::Header)
            .map(|r| match (&r.cells[2], &r.cells[3]) {
                (Cell::Text(a), Cell::Text(b)) => (a.clone(), b.clone()),
                other => panic!("{other:?}"),
            })
            .collect();
        let s = |a: &str, b: &str| (a.to_string(), b.to_string());
        assert_eq!(
            heads,
            [
                s(
                    "@reports.audit.action.exported",
                    "@reports.audit.entity.report_definition"
                ),
                s(
                    "@reports.audit.action.imported",
                    "@reports.audit.entity.report_definition"
                ),
                s(
                    "@reports.audit.action.imported_replace",
                    "@reports.audit.entity.report_definition"
                ),
                s(
                    "@reports.audit.action.create",
                    "@reports.audit.entity.report_definition"
                ),
                s(
                    "@reports.audit.action.create",
                    "@reports.audit.entity.report_definition_group"
                ),
                s(
                    "@reports.audit.action.update",
                    "@reports.audit.entity.report_definition_named Bank pack"
                ),
                s("@reports.audit.action.download", "attachment"),
            ]
        );
        // The origin is the action's word, not a field line.
        assert!(t.rows.iter().all(|r| r.caption != "origin"));
    }
}

#[cfg(test)]
mod secret_tests {
    use super::secret;

    /// S2: the company EIN, passwords, keys and tokens never show; plain
    /// fields that merely contain those letters do.
    #[test]
    fn taxpayer_ids_and_secrets_are_matched_by_word() {
        for s in [
            "ein",
            "companyEin",
            "tin",
            "taxId",
            "tax_id",
            "ssn",
            "smtpPass",
            "smtp_password",
            "apiKey",
            "api_key",
            "closingPasswordHash",
            "accessToken",
            "clientSecret",
        ] {
            assert!(secret(s), "{s} is hidden");
        }
        for s in [
            "settings",
            "reportKey",
            "name",
            "einstein",
            "printStatus",
            "keyword",
            "memo",
        ] {
            assert!(!secret(s), "{s} shows");
        }
    }
}
