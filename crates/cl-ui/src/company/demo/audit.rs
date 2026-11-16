//! The demo's audit trail (P29 WP6): a fixed, synthetic list of changes,
//! newest first, filtered and paged as the engine does. It has more than
//! one screen page, a change made automatically (no user), a bank-feed row
//! with a `source` field, a nested change (lines and an address), a text a
//! spreadsheet would read as a formula, and a field that must never show.

use cl_api::{AuditFilter, AuditImage, AuditPage, AuditRow};
use cl_core::ids::UserId;
use cl_core::{CalendarDate, Timestamp};
use serde_json::{Value, json};

/// Changes in the demo trail.
pub const ROWS: u64 = 130;

/// The demo's people (synthetic).
pub const USERS: [(&str, &str); 2] = [
    ("5a1e0000-0000-4000-8000-000000000001", "Dana Rivera"),
    ("5a1e0000-0000-4000-8000-000000000002", "Sam Okafor"),
];

fn user(i: usize) -> Option<(UserId, String)> {
    let (id, name) = USERS.get(i)?;
    Some((id.parse().ok()?, (*name).to_string()))
}

fn record_id(n: u64) -> String {
    format!("7a0c0000-0000-4000-8000-{n:012}")
}

/// Row `n` (1 = oldest). Every tenth minute back from 11/14/2026 5 PM UTC.
fn row(n: u64) -> Option<AuditRow> {
    let minutes = i64::try_from((ROWS - n) * 10).ok()?;
    let at = Timestamp::from_unix_millis(1_794_675_600_000 - minutes * 60_000).ok()?;
    let who = match n % 7 {
        0 => None,
        k => user(usize::from(k % 2 == 0)),
    };
    let (action, entity, before, after): (&str, &str, Value, Value) = match n {
        // The newest: a nested change to a transaction.
        ROWS => (
            "update",
            "transaction",
            json!({"txnType": "expense", "memo": "Office chairs", "total": "418.0000",
                   "lines": [{"accountId": "a1", "amount": "400.0000", "description": "Chairs"},
                             {"accountId": "a2", "amount": "18.0000", "description": "Delivery"}]}),
            json!({"txnType": "expense", "memo": "=Office chairs", "total": "418.0000",
                   "lines": [{"accountId": "a1", "amount": "400.0000", "description": "Chairs (2)"},
                             {"accountId": "a2", "amount": "18.0000", "description": "Delivery"}]}),
        ),
        // A bank-feed posting with its own source.
        n if n == ROWS - 1 => (
            "create",
            "transaction",
            Value::Null,
            json!({"txnType": "deposit", "source": "bank_feed", "total": "1250.0000",
                   "memo": "Mobile deposit"}),
        ),
        // A vendor's address; its taxpayer id never shows.
        n if n == ROWS - 2 => (
            "update",
            "contact",
            json!({"displayName": "Pine Supply", "taxId": "12-3456789",
                   "address": {"city": "Springfield", "zip": "65801"}}),
            json!({"displayName": "Pine Supply Co.", "taxId": "98-7654321",
                   "address": {"city": "Springfield", "zip": "65802"}}),
        ),
        n if n % 5 == 0 => (
            "update",
            "account",
            json!({"name": format!("Account {n}"), "isActive": true}),
            json!({"name": format!("Account {n}"), "isActive": false}),
        ),
        n if n % 3 == 0 => (
            "void",
            "transaction",
            json!({"txnType": "check", "status": "posted", "total": format!("{n}.2500")}),
            json!({"txnType": "check", "status": "void", "total": format!("{n}.2500")}),
        ),
        n if n % 2 == 0 => (
            "create",
            "tag",
            Value::Null,
            json!({"name": format!("Job {n}"), "isActive": true}),
        ),
        _ => (
            "create",
            "transaction",
            Value::Null,
            json!({"txnType": "invoice", "txnNumber": format!("{}", 1000 + n),
                   "total": format!("{n}.00")}),
        ),
    };
    let image = |v: Value| (!v.is_null()).then(|| AuditImage::from_value(v));
    Some(AuditRow {
        id: n,
        action: action.into(),
        entity_type: entity.into(),
        entity_id: Some(record_id(n)),
        user: who.as_ref().map(|(u, _)| *u),
        user_name: who.map(|(_, name)| name),
        at,
        before: image(before),
        after: image(after),
    })
}

fn day(t: Timestamp) -> Option<CalendarDate> {
    CalendarDate::parse_wire(t.to_wire().get(..10)?).ok()
}

fn text(s: Option<&String>) -> Option<&str> {
    s.map(String::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn keeps(r: &AuditRow, f: &AuditFilter) -> bool {
    if text(f.entity_type.as_ref()).is_some_and(|e| e != r.entity_type)
        || text(f.action.as_ref()).is_some_and(|a| a != r.action)
        || f.user.is_some_and(|u| r.user != Some(u))
        || f.entity_id
            .is_some_and(|id| r.entity_id.as_deref() != Some(id.to_string().as_str()))
    {
        return false;
    }
    let d = day(r.at);
    if f.from.is_some_and(|from| d.is_some_and(|d| d < from))
        || f.to.is_some_and(|to| d.is_some_and(|d| d > to))
    {
        return false;
    }
    if let Some(q) = text(f.search.as_ref()) {
        let q = q.to_lowercase();
        let images = [&r.before, &r.after]
            .iter()
            .filter_map(|i| i.as_ref().map(|i| i.raw().to_string().to_lowercase()))
            .collect::<String>();
        if !images.contains(&q) {
            return false;
        }
    }
    true
}

/// One page of the demo trail, newest first.
#[must_use]
pub fn page(f: &AuditFilter, limit: u32, offset: u64) -> AuditPage {
    let all: Vec<AuditRow> = (1..=ROWS)
        .rev()
        .filter_map(row)
        .filter(|r| keeps(r, f))
        .collect();
    let total = all.len() as u64;
    let rows = all
        .into_iter()
        .skip(usize::try_from(offset).unwrap_or(usize::MAX))
        .take(usize::try_from(limit.clamp(1, 500)).unwrap_or(500))
        .collect();
    AuditPage { rows, total }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_demo_trail_pages_and_filters() {
        let all = page(&AuditFilter::default(), 100, 0);
        assert_eq!(all.total, ROWS);
        assert_eq!(all.rows.len(), 100);
        assert_eq!(all.rows[0].id, ROWS, "newest first");
        assert_eq!(page(&AuditFilter::default(), 100, 100).rows.len(), 30);
        let voids = page(
            &AuditFilter {
                action: Some("void".into()),
                ..AuditFilter::default()
            },
            500,
            0,
        );
        assert!(voids.total > 0 && voids.rows.iter().all(|r| r.action == "void"));
        let found = page(
            &AuditFilter {
                search: Some("pine supply".into()),
                ..AuditFilter::default()
            },
            500,
            0,
        );
        assert_eq!(found.total, 1);
        assert!(
            all.rows.iter().any(|r| r.user.is_none()),
            "an automatic row"
        );
    }
}
