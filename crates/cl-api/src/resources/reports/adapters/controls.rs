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
                read(e, F::LocalDate("at"))?,
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
pub const AUDIT_FIELDS: [&str; 17] = [
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
    // A password change ends the user's other sign-ins (P29b).
    "sessionsRevoked",
];

/// Secrets and taxpayer ids never show (`cl_core::audit`, shared with the
/// Audit Trail viewer).
fn secret(name: &str) -> bool {
    cl_core::audit::secret_field(name)
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
pub const AUDIT_ENTITIES: [&str; 20] = [
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
    "report_builder_draft",
    "payment_applications",
    // A10 QA-6: the company's AI switches and consent.
    "company_ai_tasks",
    "company_ai_consent_accepted",
    "company_ai_consent_revoked",
    // P29b: a user changed their own password (no secret in the row).
    "user_password_change",
];

/// Rows whose action reads better as one phrase than as the engine's verb
/// and record kind (P29b: `update` on `user_password_change` reads
/// "Password changed"). The i18n key, without the `@`.
#[must_use]
pub fn plain_action(action: &str, entity: &str) -> Option<&'static str> {
    match (action, entity) {
        ("update" | "create", "user_password_change") => {
            Some("reports.audit.action.password_changed")
        }
        _ => None,
    }
}

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
    // A10 (Q-A10i, Q-A10k): one row per AI draft. `after.outcome` says
    // whether it was used: "refused" reads "Asked AI to draft a report (not
    // used)"; "failed" (the service didn't answer after the names were
    // sent, owner 2026-11-15, Q-A10k) "Asked AI to draft a report (the AI
    // service didn't answer)"; "drafted", or a row without it (an older
    // engine), "Drafted a report with AI".
    if entity == "report_builder_draft" && text(e, "action") == Some("create") {
        return Some(
            match e.pointer("/after_data/outcome").and_then(Value::as_str) {
                Some("refused") => "@reports.audit.action.drafted_ai_refused",
                Some("failed") => "@reports.audit.action.drafted_ai_failed",
                _ => "@reports.audit.action.drafted_ai",
            },
        );
    }
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

/// A service's own name, not the engine's key (A10 QA-6).
fn provider_name(p: &str) -> Option<&'static str> {
    Some(match p {
        "anthropic" => "Anthropic",
        "openai" => "OpenAI",
        "ollama" => "Ollama",
        _ => return None,
    })
}

/// PR-A2 (U-226): rows whose action needs more than the code.
/// - An AI draft whose names went to more than one service (`after.sentTo`,
///   6-S1, Q-PRA2e): "Drafted a report with AI (Ollama on this computer;
///   names were also sent to Anthropic, which didn't answer)".
/// - A merge that rewrote memorized reports (`after.moved.reportDefinitions`,
///   Q-PRA2b): "Merged and updated 2 memorized reports".
///
/// Only provider keys the app knows and counts are read: no names.
fn extra_action(e: &Value, entity: &str) -> Option<String> {
    if let Some(n) = e
        .pointer("/after_data/moved/reportDefinitions")
        .and_then(Value::as_u64)
        .filter(|n| *n > 0)
    {
        return Some(if n == 1 {
            "@reports.audit.action.merged_report_one".into()
        } else {
            format!("@reports.audit.action.merged_reports {n}")
        });
    }
    if entity != "report_builder_draft" || text(e, "action") != Some("create") {
        return None;
    }
    let outcome = e.pointer("/after_data/outcome").and_then(Value::as_str);
    if outcome == Some("failed") {
        return None;
    }
    let sent = e.pointer("/after_data/sentTo").and_then(Value::as_array)?;
    if sent.len() < 2 {
        return None;
    }
    let field = |s: &Value, k: &str| {
        s.get(k)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let answered = sent.iter().find(|s| field(s, "outcome") != "failed")?;
    let name = provider_name(&field(answered, "provider"))?;
    let local = field(answered, "reader") == "self_hosted" || name == "Ollama";
    let mut others: Vec<&str> = sent
        .iter()
        .filter(|s| field(s, "outcome") == "failed")
        .filter_map(|s| provider_name(&field(s, "provider")))
        .collect();
    others.dedup();
    if others.is_empty() {
        return None;
    }
    let head = if local {
        format!("{name} on this computer")
    } else {
        name.to_string()
    };
    Some(format!(
        "@reports.audit.action.drafted_ai_also {head}; names were also sent to {}",
        others.join(", ")
    ))
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
    // A10 (Q-A10i): which AI service drafted it; the description and the
    // draft are never stored.
    let provider = e
        .pointer("/after_data/provider")
        .and_then(Value::as_str)
        .filter(|p| p.len() <= 20 && p.bytes().all(|c| c.is_ascii_lowercase() || c == b'_'));
    if let ("report_builder_draft", Some(p)) = (entity, provider) {
        // The service by its own name, not the engine's key (A10 QA-6).
        let p = match p {
            "anthropic" => "Anthropic",
            "openai" => "OpenAI",
            "ollama" => "Ollama",
            other => other,
        };
        return format!("@reports.audit.entity.report_builder_draft_by {p}");
    }
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
                read(e, F::LocalDate("created_at"))?,
                read(e, F::Text("userName"))?,
                match (extra_action(e, entity), report_file_action(e, entity)) {
                    (Some(text), _) => Cell::Text(text),
                    (None, Some(key)) => Cell::Text(key.into()),
                    (None, None) => match plain_action(text(e, "action").unwrap_or(""), entity) {
                        Some(key) => Cell::Text(format!("@{key}")),
                        None => read(e, F::Code("reports.audit.action", "action"))?,
                    },
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
        // An AI draft's row says who and which service in its heading; its
        // ids and counts are not field lines (Q-A10i).
        let draft_row = entity == "report_builder_draft";
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
                || cl_core::audit::QUIET.contains(&key.as_str())
                // A memorized report's action already says it came from a
                // file (A09 QA-4: "Imported").
                || (report_row && key == "origin")
                || draft_row
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
        // P29 QA-8: "Changed on" is the date on this computer's clock, not
        // the UTC timestamp's date (an evening edit in the US is "tomorrow"
        // in UTC).
        let at = "2026-11-17T00:53:00.000Z";
        let local = chrono::DateTime::parse_from_rfc3339(at)
            .expect("ts")
            .with_timezone(&chrono::Local)
            .date_naive();
        let evening = json!({"data": [{"at": at, "userName": "QA"}]});
        let t = closing_exceptions(&evening, &req(Source::ClosingDateExceptions)).expect("table");
        assert_eq!(
            t.rows[0].cells[0],
            Cell::Date(cl_core::CalendarDate::from_naive(local).expect("day"))
        );
        assert_eq!(
            super::super::lists::local_date("2026-09-30"),
            Some(cl_core::CalendarDate::from_ymd(2026, 9, 30).expect("day")),
            "a bare date is taken as given"
        );
    }

    /// P29 QA-8: the engine's raw queries hand `timestamptz` over as
    /// PostgreSQL's text (its driver keeps it as stored); those read as the
    /// same instant, so the local date is the same as for the `Z` form.
    #[test]
    fn postgres_text_timestamps_take_the_local_date() {
        use super::super::lists::local_date;
        let local = |rfc: &str| {
            cl_core::CalendarDate::from_naive(
                chrono::DateTime::parse_from_rfc3339(rfc)
                    .expect("ts")
                    .with_timezone(&chrono::Local)
                    .date_naive(),
            )
            .expect("day")
        };
        for (pg, same) in [
            (
                "2026-11-17 00:53:00.123456+00",
                "2026-11-17T00:53:00.123456Z",
            ),
            ("2026-11-17 00:53:00+00", "2026-11-17T00:53:00Z"),
            ("2026-11-16 18:53:00.5-06", "2026-11-17T00:53:00.5Z"),
            ("2026-11-17 06:23:00+05:30", "2026-11-17T00:53:00Z"),
            ("2026-11-17T00:53:00.000Z", "2026-11-17T00:53:00Z"),
        ] {
            assert_eq!(local_date(pg), Some(local(same)), "{pg}");
        }
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

    /// A10 (Q-A10i): an AI draft reads "Drafted a report with AI" with its
    /// service; its ids and counts are not field lines.
    #[test]
    fn an_ai_draft_is_named_in_plain_words() {
        let v = json!({"data": [{"action": "create", "entity_type": "report_builder_draft",
            "entity_id": "7f3a2c10-5b6d-4e8f-9a0b-1c2d3e4f5a60", "userName": "Owner",
            "created_at": "2026-11-15T10:00:00.000Z", "before_data": null,
            "after_data": {"companyId": "7f3a2c10-5b6d-4e8f-9a0b-1c2d3e4f5a61", "provider": "ollama",
                           "model": "llama3", "reader": "self_hosted", "nodes": 2, "unresolved": 1}}],
            "total": 1});
        let t = audit_trail(&v, &req(Source::AuditLog)).expect("table");
        assert_eq!(t.rows.len(), 1, "a heading only");
        assert_eq!(
            (&t.rows[0].cells[2], &t.rows[0].cells[3]),
            (
                &Cell::Text("@reports.audit.action.drafted_ai".into()),
                &Cell::Text("@reports.audit.entity.report_builder_draft_by Ollama".into())
            )
        );
        // Q-A10k: the outcome names the action; it is never a field line.
        let action = |outcome: &str| {
            let mut v = v.clone();
            v["data"][0]["after_data"]["outcome"] = json!(outcome);
            let t = audit_trail(&v, &req(Source::AuditLog)).expect("table");
            assert_eq!(t.rows.len(), 1, "a heading only");
            t.rows[0].cells[2].clone()
        };
        assert_eq!(
            action("drafted"),
            Cell::Text("@reports.audit.action.drafted_ai".into())
        );
        assert_eq!(
            action("refused"),
            Cell::Text("@reports.audit.action.drafted_ai_refused".into())
        );
        // A10 QA-6: the service didn't answer after the names were sent.
        assert_eq!(
            action("failed"),
            Cell::Text("@reports.audit.action.drafted_ai_failed".into())
        );
        // ... and a failed row without a provider still reads in words.
        let mut bare = v.clone();
        bare["data"][0]["after_data"] = json!({"outcome": "failed"});
        let t = audit_trail(&bare, &req(Source::AuditLog)).expect("table");
        assert_eq!(
            t.rows[0].cells[3],
            Cell::Text("@reports.audit.entity.report_builder_draft".into())
        );
    }

    /// PR-A2 6-S1 (Q-PRA2e): a draft whose names went to two services says
    /// so; a merge that rewrote memorized reports counts them (Q-PRA2b).
    #[test]
    fn a_draft_sent_twice_and_a_merge_say_so() {
        let v = json!({"data": [{"action": "create", "entity_type": "report_builder_draft",
            "entity_id": "7f3a2c10-5b6d-4e8f-9a0b-1c2d3e4f5a60", "userName": "Owner",
            "created_at": "2026-11-15T10:00:00.000Z", "before_data": null,
            "after_data": {"provider": "ollama", "reader": "cloud", "outcome": "drafted",
                "sentTo": [
                    {"provider": "anthropic", "model": "m", "reader": "cloud", "outcome": "failed"},
                    {"provider": "ollama", "model": "llama3", "reader": "self_hosted", "outcome": "drafted"}]}}],
            "total": 1});
        let t = audit_trail(&v, &req(Source::AuditLog)).expect("table");
        assert_eq!(t.rows.len(), 1, "a heading only");
        assert_eq!(
            t.rows[0].cells[2],
            Cell::Text(
                "@reports.audit.action.drafted_ai_also Ollama on this computer; names were \
                 also sent to Anthropic"
                    .into()
            )
        );
        // One service: as before.
        let mut one = v.clone();
        one["data"][0]["after_data"]["sentTo"] = json!([{"provider": "ollama", "model": "llama3", "reader": "self_hosted", "outcome": "drafted"}]);
        let t = audit_trail(&one, &req(Source::AuditLog)).expect("table");
        assert_eq!(
            t.rows[0].cells[2],
            Cell::Text("@reports.audit.action.drafted_ai".into())
        );
        let merge = |n: u64| {
            let v = json!({"data": [{"action": "merge", "entity_type": "contact",
                "entity_id": "7f3a2c10-5b6d-4e8f-9a0b-1c2d3e4f5a62", "userName": "Owner",
                "created_at": "2026-11-15T10:00:00.000Z", "before_data": null,
                "after_data": {"moved": {"reportDefinitions": n}}}], "total": 1});
            audit_trail(&v, &req(Source::AuditLog)).expect("table").rows[0].cells[2].clone()
        };
        assert_eq!(
            merge(2),
            Cell::Text("@reports.audit.action.merged_reports 2".into())
        );
        assert_eq!(
            merge(1),
            Cell::Text("@reports.audit.action.merged_report_one".into())
        );
        // No report rewritten: the action as before.
        assert_ne!(merge(0), merge(1));
    }

    /// A10 QA-6: the company's AI settings and consent read in plain words,
    /// not as the engine's table names.
    #[test]
    fn ai_settings_rows_are_named_in_plain_words() {
        for e in [
            "company_ai_tasks",
            "company_ai_consent_accepted",
            "company_ai_consent_revoked",
        ] {
            assert_eq!(entity_label(e), format!("@reports.audit.entity.{e}"));
        }
    }

    /// P29b: the engine's row for a user's own password change reads
    /// "Password changed", with who and when, and no secret.
    #[test]
    fn a_password_change_reads_password_changed() {
        let v = json!({"data": [{"id": "41", "action": "update",
            "entity_type": "user_password_change",
            "entity_id": "7f3a2c10-5b6d-4e8f-9a0b-1c2d3e4f5a62",
            "user_id": "7f3a2c10-5b6d-4e8f-9a0b-1c2d3e4f5a62", "userName": "Lee Bookkeeper",
            "created_at": "2026-11-17T15:00:00.000Z", "before_data": null,
            "after_data": {"sessionsRevoked": true}}], "total": 1});
        let t = audit_trail(&v, &req(Source::AuditLog)).expect("table");
        let head = &t.rows[0];
        assert_eq!(head.cells[1], Cell::Text("Lee Bookkeeper".into()));
        assert_eq!(
            head.cells[2],
            Cell::Text("@reports.audit.action.password_changed".into())
        );
        assert_eq!(
            head.cells[3],
            Cell::Text("@reports.audit.entity.user_password_change".into())
        );
        assert_eq!(t.rows[1].caption, "@reports.audit.field.sessionsRevoked");
        assert_eq!(t.rows[1].cells[5], Cell::Text("@reports.audit.yes".into()));
        assert_eq!(plain_action("delete", "user_password_change"), None);
        assert_eq!(plain_action("update", "contact"), None);
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
