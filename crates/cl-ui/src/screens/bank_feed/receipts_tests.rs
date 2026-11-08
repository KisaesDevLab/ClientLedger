//! A05: the receipt's coding (Q-A05b) and the list's offers.

#![allow(
    clippy::needless_pass_by_value,
    clippy::assert_is_empty,
    reason = "test builders and readable checks"
)]

use super::*;
use cl_docread::model::{Bbox, Confidence, Found, Header, Line, RuleId, Spot};

fn money(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}

fn found<T>(value: T) -> Found<T> {
    Found {
        value,
        spot: Spot {
            page: 0,
            bbox: Bbox::PAGE,
        },
        rule: RuleId::new("test"),
        confidence: Confidence::HIGH,
    }
}

fn line(desc: &str, amount: &str) -> serde_json::Value {
    serde_json::to_value(Line {
        description: Some(found(desc.to_string())),
        quantity: None,
        unit_price: None,
        amount: found(money(amount)),
    })
    .expect("line")
}

fn draft(lines: Vec<serde_json::Value>, reference: Option<&str>) -> DocumentDraft {
    let header = Header {
        vendor: Some(found("Synthetic Hardware Co.".to_string())),
        ..Header::default()
    };
    serde_json::from_value(serde_json::json!({
        "id": "d0000000-0000-4000-8000-dd0000000001",
        "kind": "cash_expense",
        "status": "ready",
        "fields": serde_json::to_value(header).expect("header"),
        "lines": lines,
        "docReference": reference,
        "docTotal": "84.1700",
        "docDate": "2026-11-03",
        "suggestedContactId": "c0000000-0000-4000-8000-000000000001",
        "updatedAt": "2026-11-08T10:00:00.000Z",
    }))
    .expect("draft")
}

#[test]
fn lines_become_splits_only_when_they_add_up_exactly_in_cents() {
    let d = draft(
        vec![line("Shop supplies", "60.00"), line("Hardware", "24.17")],
        Some("R-1042"),
    );
    let c = coding(&d, money("84.17"));
    assert_eq!(c.lines.len(), 2);
    assert_eq!(c.lines[1].0, money("24.17"));
    assert_eq!(c.lines[0].1.as_deref(), Some("Shop supplies"));
    assert_eq!(
        c.memo.as_deref(),
        Some("Synthetic Hardware Co. \u{b7} R-1042")
    );
    assert!(c.contact.is_some());
    // Either sign of the bank amount.
    assert_eq!(coding(&d, money("-84.17")).lines.len(), 2);

    // One cent off (tax not on a line): one account.
    let off = draft(
        vec![line("Shop supplies", "60.00"), line("Hardware", "24.16")],
        None,
    );
    let c = coding(&off, money("84.17"));
    assert!(c.lines.is_empty());
    assert_eq!(c.memo.as_deref(), Some("Synthetic Hardware Co."));
    // A line with a fourth decimal can't be a split.
    let fine = draft(vec![line("A", "60.0050"), line("B", "24.1650")], None);
    assert!(coding(&fine, money("84.17")).lines.is_empty());
    // One line: no split.
    let one = draft(vec![line("All", "84.17")], None);
    assert!(coding(&one, money("84.17")).lines.is_empty());
}

#[test]
fn the_choice_puts_every_line_on_the_panels_account() {
    let d = draft(
        vec![line("Shop supplies", "60.00"), line("Hardware", "24.17")],
        Some("R-1042"),
    );
    let account: AccountId = "a0000000-0000-4000-8000-000000000001".parse().expect("a");
    let c = choice_of(&coding(&d, money("84.17")), account, None);
    assert_eq!(c.account, Some(account));
    assert_eq!(c.splits.len(), 2);
    assert!(c.splits.iter().all(|l| l.account == account));
    assert_eq!(
        Money::sum(c.splits.iter().map(|l| l.amount)).expect("sum"),
        money("84.17")
    );
    // No vendor matched: the line's payee stays.
    let mut plain = coding(&d, money("84.17"));
    plain.contact = None;
    let payee: ContactId = "c0000000-0000-4000-8000-000000000009".parse().expect("c");
    assert_eq!(choice_of(&plain, account, Some(payee)).contact, Some(payee));
}

#[test]
fn statements_and_closed_drafts_are_never_offered() {
    let mut d = draft(Vec::new(), None);
    assert!(!facts(&d).statement);
    d.kind = DraftKind::Statement;
    assert!(facts(&d).statement);
    d.kind = DraftKind::CashExpense;
    d.status = DraftStatus::Converted;
    assert!(facts(&d).statement, "not live");
}

#[test]
fn coding_debug_has_no_owner_data() {
    let d = draft(
        vec![line("Secret item", "60.00"), line("B", "24.17")],
        Some("R-1042"),
    );
    let shown = format!("{:?}", coding(&d, money("84.17")));
    assert!(!shown.contains("Secret"));
    assert!(!shown.contains("60"));
    assert!(!shown.contains("R-1042"));
}
