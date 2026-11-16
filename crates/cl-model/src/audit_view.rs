//! The Audit Trail viewer's rules (P29 WP6), without egui or the engine
//! types: the field-by-field before/after list, the derived source column
//! (Q-P29s), and the formula-safe CSV export (Q-156).
//!
//! - Values are shown **verbatim**: text as the engine sent it, numbers as
//!   serde prints them, never added up or rounded (plan §4).
//! - Secrets, taxpayer ids and bookkeeping fields never show, at any depth
//!   (`cl_core::audit::hidden_field`, NN11).
//! - Every CSV cell is text and goes through `csv_formula_safe`, so no cell
//!   a spreadsheet opens starts with `=`, `+`, `-` or `@`.

use std::collections::BTreeSet;

use cl_core::Timestamp;
use cl_core::audit::hidden_field;
use cl_core::report::{csv_formula_safe, push_csv_line};
use serde_json::Value;

/// Rows read per page on screen.
pub const SCREEN_PAGE: u32 = 100;
/// Rows read per page by an export (plan WP6).
pub const EXPORT_PAGE: u32 = 500;

/// One value of an image, as given.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FieldValue {
    /// Text, as sent.
    Text(String),
    /// A JSON number in serde's own text (no arithmetic).
    Number(String),
    /// Yes or no.
    Bool(bool),
    /// An empty list or object.
    Empty,
}

impl FieldValue {
    fn of(v: &Value) -> Option<FieldValue> {
        match v {
            Value::String(s) => Some(FieldValue::Text(s.clone())),
            Value::Number(n) => Some(FieldValue::Number(n.to_string())),
            Value::Bool(b) => Some(FieldValue::Bool(*b)),
            Value::Array(a) if a.is_empty() => Some(FieldValue::Empty),
            Value::Object(o) if o.is_empty() => Some(FieldValue::Empty),
            // Lists and objects are walked, never shown whole.
            Value::Null | Value::Array(_) | Value::Object(_) => None,
        }
    }

    /// The value in words: `yes`/`no` are the caller's translations.
    #[must_use]
    pub fn text(&self, yes: &str, no: &str) -> String {
        match self {
            FieldValue::Text(s) | FieldValue::Number(s) => s.clone(),
            FieldValue::Bool(true) => yes.to_string(),
            FieldValue::Bool(false) => no.to_string(),
            FieldValue::Empty => String::new(),
        }
    }
}

/// One changed (or, for a create or delete, present) field.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldChange {
    /// Where in the record: `memo`, `lines[2].description`, …
    pub path: String,
    /// The value before (`None`: not there).
    pub before: Option<FieldValue>,
    /// The value after.
    pub after: Option<FieldValue>,
}

fn walk(path: &str, v: Option<&Value>, out: &mut Vec<(String, Option<FieldValue>)>) {
    match v {
        Some(Value::Object(o)) if !o.is_empty() => {
            for (k, child) in o {
                if hidden_field(k) {
                    continue;
                }
                let p = if path.is_empty() {
                    k.clone()
                } else {
                    format!("{path}.{k}")
                };
                walk(&p, Some(child), out);
            }
        }
        Some(Value::Array(a)) if !a.is_empty() => {
            for (i, child) in a.iter().enumerate() {
                walk(&format!("{path}[{}]", i + 1), Some(child), out);
            }
        }
        other => out.push((path.to_string(), other.and_then(FieldValue::of))),
    }
}

/// The fields of a before/after pair, in path order. When both images are
/// there (a change), only fields whose values differ are listed; a create
/// or delete lists every field it has. Secrets never show (any depth).
#[must_use]
pub fn diff(before: Option<&Value>, after: Option<&Value>) -> Vec<FieldChange> {
    let mut b = Vec::new();
    let mut a = Vec::new();
    if before.is_some() {
        walk("", before, &mut b);
    }
    if after.is_some() {
        walk("", after, &mut a);
    }
    let both = before.is_some_and(|v| !v.is_null()) && after.is_some_and(|v| !v.is_null());
    let paths: BTreeSet<&String> = b.iter().chain(&a).map(|(p, _)| p).collect();
    let find = |side: &[(String, Option<FieldValue>)], p: &str| {
        side.iter()
            .find(|(q, _)| q == p)
            .and_then(|(_, v)| v.clone())
    };
    paths
        .into_iter()
        .filter_map(|p| {
            let (bv, av) = (find(&b, p), find(&a, p));
            if (both && bv == av) || (bv.is_none() && av.is_none()) {
                return None;
            }
            Some(FieldChange {
                path: p.clone(),
                before: bv,
                after: av,
            })
        })
        .collect()
}

/// Who or what made a change (Q-P29s).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuditSource {
    /// No user: the engine did it (an automatic template, a merge job).
    Automatic,
    /// The record's own `source` field: `bank_feed`, `import`, `mcp`, …
    Channel(String),
    /// A person, by display name (else email).
    EnteredBy(String),
}

impl AuditSource {
    /// The i18n key for a known channel (`audit.source.<channel>`), if any.
    #[must_use]
    pub fn channel_key(channel: &str) -> Option<&'static str> {
        Some(match channel {
            "bank_feed" | "bank-feed" | "feed" => "audit.source.bank_feed",
            "import" | "csv_import" | "bank_import" => "audit.source.import",
            "mcp" => "audit.source.mcp",
            "rule" | "bank_rule" => "audit.source.rule",
            "recurring" | "template" => "audit.source.recurring",
            _ => return None,
        })
    }
}

fn source_field(v: Option<&Value>) -> Option<String> {
    v?.get("source")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty() && s.len() <= 40)
        .map(str::to_string)
}

/// The source column: "Automatic" without a user; else the record's
/// `source` field when it has one; else "Entered by <name>".
#[must_use]
pub fn source(
    has_user: bool,
    user_name: Option<&str>,
    before: Option<&Value>,
    after: Option<&Value>,
) -> AuditSource {
    if !has_user {
        return AuditSource::Automatic;
    }
    if let Some(s) = source_field(after).or_else(|| source_field(before)) {
        return AuditSource::Channel(s);
    }
    AuditSource::EnteredBy(user_name.unwrap_or_default().to_string())
}

/// One audit row ready for the CSV: every text already in words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportRow {
    /// When, as the app shows it.
    pub when: String,
    /// Who.
    pub user: String,
    /// What happened, in words.
    pub action: String,
    /// The record kind, in words.
    pub record: String,
    /// The record id, as sent.
    pub record_id: String,
    /// The source column, in words.
    pub source: String,
    /// The fields with their values already in words.
    pub fields: Vec<(String, String, String)>,
}

/// The CSV heading line (captions already translated).
#[must_use]
pub fn csv_heading(captions: &[String]) -> String {
    let mut out = String::new();
    let cells: Vec<String> = captions.iter().cloned().map(csv_formula_safe).collect();
    push_csv_line(&mut out, &cells);
    out
}

/// The CSV lines of one row: one line per field (a row without fields
/// still gets a line). Every cell is formula-safe text.
#[must_use]
pub fn csv_lines(r: &ExportRow) -> String {
    let mut out = String::new();
    let head = [
        r.when.clone(),
        r.user.clone(),
        r.action.clone(),
        r.record.clone(),
        r.record_id.clone(),
        r.source.clone(),
    ];
    let empty = [(String::new(), String::new(), String::new())];
    let fields: &[(String, String, String)] = if r.fields.is_empty() {
        &empty
    } else {
        &r.fields
    };
    for (f, b, a) in fields {
        let cells: Vec<String> = head
            .iter()
            .cloned()
            .chain([f.clone(), b.clone(), a.clone()])
            .map(csv_formula_safe)
            .collect();
        push_csv_line(&mut out, &cells);
    }
    out
}

/// An export in progress: rows are read newest first in pages that can
/// shift while new changes arrive, so rows already written (by `id`) and
/// rows made after the export started are skipped.
#[derive(Clone, Debug)]
pub struct ExportCursor {
    started: Timestamp,
    seen: BTreeSet<u64>,
    /// The next page's offset.
    pub offset: u64,
}

impl ExportCursor {
    /// An export that covers rows up to `started`.
    #[must_use]
    pub fn new(started: Timestamp) -> ExportCursor {
        ExportCursor {
            started,
            seen: BTreeSet::new(),
            offset: 0,
        }
    }

    /// Whether to write this row (first time seen, made before the start).
    pub fn take(&mut self, id: u64, at: Timestamp) -> bool {
        at <= self.started && self.seen.insert(id)
    }

    /// Rows written so far.
    #[must_use]
    pub fn written(&self) -> usize {
        self.seen.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use serde_json::json;

    #[test]
    fn a_nested_change_lists_only_the_changed_fields_verbatim() {
        let before = json!({
            "id": "x", "memo": "Old", "total": "12.3400", "count": 3,
            "isActive": true, "taxId": "12-3456789",
            "lines": [{"amount": "10.00", "memo": "a"}, {"amount": "2.3400", "apiKey": "k"}],
            "address": {"city": "Springfield", "smtpPass": "p"}
        });
        let after = json!({
            "id": "x", "memo": "New", "total": "12.3400", "count": 3.50,
            "isActive": false, "taxId": "98-7654321",
            "lines": [{"amount": "10.00", "memo": "b"}],
            "address": {"city": "Shelbyville", "smtpPass": "q"}
        });
        let d = diff(Some(&before), Some(&after));
        let paths: Vec<&str> = d.iter().map(|c| c.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "address.city",
                "count",
                "isActive",
                "lines[1].memo",
                "lines[2].amount",
                "memo"
            ]
        );
        let count = &d[1];
        assert_eq!(count.before, Some(FieldValue::Number("3".into())));
        assert_eq!(count.after, Some(FieldValue::Number("3.5".into())));
        let gone = &d[4];
        assert_eq!(gone.before, Some(FieldValue::Text("2.3400".into())));
        assert_eq!(gone.after, None);
        let shown = format!("{d:?}");
        for secret in ["12-3456789", "98-7654321", "\"k\"", "\"p\"", "\"q\""] {
            assert!(!shown.contains(secret), "{secret} in {shown}");
        }
    }

    #[test]
    fn a_create_lists_every_field_and_a_text_image_is_one_value() {
        let after = json!({"name": "Rent", "number": "6100", "tags": []});
        let d = diff(None, Some(&after));
        assert_eq!(d.len(), 3);
        assert!(d.iter().all(|c| c.before.is_none()));
        let d = diff(Some(&json!("legacy text")), None);
        assert_eq!(
            d,
            [FieldChange {
                path: String::new(),
                before: Some(FieldValue::Text("legacy text".into())),
                after: None
            }]
        );
        assert_eq!(diff(None, None), Vec::new());
    }

    #[test]
    fn the_source_column_follows_q_p29s() {
        let feed = json!({"source": "bank_feed"});
        assert_eq!(
            source(false, Some("Pat"), None, Some(&feed)),
            AuditSource::Automatic
        );
        assert_eq!(
            source(true, Some("Pat"), None, Some(&feed)),
            AuditSource::Channel("bank_feed".into())
        );
        assert_eq!(
            source(true, Some("Pat"), Some(&json!({"memo": "x"})), None),
            AuditSource::EnteredBy("Pat".into())
        );
        assert_eq!(
            AuditSource::channel_key("bank_feed"),
            Some("audit.source.bank_feed")
        );
        assert_eq!(AuditSource::channel_key("smoke_signal"), None);
    }

    #[test]
    fn the_export_skips_rows_seen_or_made_after_it_started() {
        let t = |s: &str| Timestamp::parse_wire(s).expect("t");
        let mut c = ExportCursor::new(t("2026-11-16T12:00:00Z"));
        assert!(c.take(5, t("2026-11-16T11:00:00Z")));
        assert!(
            !c.take(5, t("2026-11-16T11:00:00Z")),
            "a shifted page repeats a row"
        );
        assert!(
            !c.take(9, t("2026-11-16T12:00:01Z")),
            "made after the start"
        );
        assert_eq!(c.written(), 1);
    }

    /// The parsed cells of a CSV (RFC 4180, as `push_csv_line` writes it).
    fn cells(csv: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut cur = String::new();
        let mut quoted = false;
        let mut chars = csv.chars().peekable();
        while let Some(ch) = chars.next() {
            match (quoted, ch) {
                (true, '"') if chars.peek() == Some(&'"') => {
                    cur.push('"');
                    chars.next();
                }
                (true, '"') => quoted = false,
                (false, '"') if cur.is_empty() => quoted = true,
                (false, ',' | '\n') => out.push(std::mem::take(&mut cur)),
                (_, c) => cur.push(c),
            }
        }
        out
    }

    proptest! {
        #[test]
        fn no_csv_cell_begins_with_a_formula_sign(
            texts in proptest::collection::vec("[ \t\r=+@,\"a-z0-9.\\-]{0,12}", 9)
        ) {
            let row = ExportRow {
                when: texts[0].clone(),
                user: texts[1].clone(),
                action: texts[2].clone(),
                record: texts[3].clone(),
                record_id: texts[4].clone(),
                source: texts[5].clone(),
                fields: vec![(texts[6].clone(), texts[7].clone(), texts[8].clone())],
            };
            let csv = csv_lines(&row) + &csv_heading(&texts);
            for cell in cells(&csv) {
                let first = cell.trim_start_matches(' ').chars().next();
                prop_assert!(
                    !matches!(first, Some('=' | '+' | '-' | '@' | '\t' | '\r')),
                    "{cell:?}"
                );
            }
        }
    }

    #[test]
    fn the_export_matches_the_golden_file() {
        let rows = [
            ExportRow {
                when: "11/02/2026 10:15 AM".into(),
                user: "Pat Owner".into(),
                action: "Changed".into(),
                record: "Transaction".into(),
                record_id: "7a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01".into(),
                source: "Entered by Pat Owner".into(),
                fields: vec![
                    (
                        "memo".into(),
                        "=SUM(A1:A9)".into(),
                        "Paid, with thanks".into(),
                    ),
                    ("total".into(), "-12.3400".into(), "12.3400".into()),
                ],
            },
            ExportRow {
                when: "11/01/2026 09:00 AM".into(),
                user: String::new(),
                action: "Added".into(),
                record: "Tag".into(),
                record_id: String::new(),
                source: "Automatic".into(),
                fields: Vec::new(),
            },
        ];
        let heading: Vec<String> = [
            "When",
            "User",
            "Action",
            "Record",
            "Record ID",
            "Source",
            "Field",
            "Before",
            "After",
        ]
        .iter()
        .map(|s| (*s).to_string())
        .collect();
        let mut csv = csv_heading(&heading);
        for r in &rows {
            csv.push_str(&csv_lines(r));
        }
        let golden =
            include_str!("../../../fixtures/golden/audit_export.csv").replace("\r\n", "\n");
        assert_eq!(csv, golden);
    }
}
