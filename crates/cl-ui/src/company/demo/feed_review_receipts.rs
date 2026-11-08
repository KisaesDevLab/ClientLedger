//! A05: receipts for the demo's Bank Feed lines (the control channel's
//! `dev.demo_feed_receipt` and the demo tests): a synthetic receipt read
//! as a cash expense whose total is the bank amount exactly (written with
//! 4 decimals, as the engine stores `docTotal`), dated two days before the
//! bank line, with a vendor and reference read and, optionally, two lines
//! that add up to it. Synthetic only: no real receipt, vendor or amount.

use cl_api::{
    AttachTo, DocumentDraftId, DraftKey, DraftKind, DraftReader, DraftReading, FeedItemId,
    IdempotencyKey,
};
use cl_core::Money;
use cl_docread::model::{Bbox, Confidence, Found, Header, Line, RuleId, Spot};
use serde_json::Value;

use super::{DemoState, Row};

fn found<T>(value: T) -> Found<T> {
    Found {
        value,
        spot: Spot {
            page: 0,
            bbox: Bbox::new(1_000, 1_000, 3_000, 1_400).unwrap_or(Bbox::PAGE),
        },
        rule: RuleId::new("demo.receipt"),
        confidence: Confidence::HIGH,
    }
}

/// A waiting money-out line of the sample with an account proposed, whole
/// cents, no receipt yet: the first one in the list's order.
fn pick_line(state: &DemoState) -> Option<Row> {
    let mut rows: Vec<&Row> = state
        .feed_review
        .rows
        .iter()
        .filter(|r| {
            r.waiting()
                && r.amount.is_positive()
                && r.kind == "ready"
                && !r.transfer
                && !r.possible_duplicate
                && r.suggested.is_some()
                && cl_model::bank_feed::whole_cents(r.amount)
        })
        .collect();
    rows.sort_by(|a, b| b.date.cmp(&a.date).then(a.id.cmp(&b.id)));
    rows.first().map(|r| (*r).clone())
}

/// A ready receipt for line `id` (any status: "bank file first, then the
/// receipt" seeds one for an accepted line). `lines`: two lines that add
/// up to the total. `None` without drafts or when the line is unknown.
pub fn seed_receipt_for(
    state: &mut DemoState,
    id: FeedItemId,
    lines: bool,
) -> Option<DocumentDraftId> {
    let r = state.feed_review.rows.iter().find(|r| r.id == id)?.clone();
    let tenant = state.open?;
    // Each receipt's bytes differ (a draft claims its file's hash).
    let key = DraftKey::new();
    let bytes = format!("%PDF-1.4\n% synthetic demo receipt {key}\n%%EOF\n");
    let file =
        super::super::checks::upload(state, AttachTo::Draft(key), "receipt.pdf", bytes.as_bytes())
            .ok()?;
    let d = super::super::drafts::create(
        state,
        tenant,
        file.id,
        DraftKind::CashExpense,
        IdempotencyKey::new(),
    )
    .ok()?;
    let total = Money::parse_wire(&r.amount.abs().to_wire()).ok()?;
    let mut header = Header {
        vendor: Some(found("Synthetic Hardware Co.".to_string())),
        reference: Some(found("R-1042".to_string())),
        total: Some(found(total)),
        ..Header::default()
    };
    header.date = r.date.add_days(-2).ok().map(found);
    let read_lines: Vec<Value> = if lines {
        split_cents(total)
            .map(|(a, b)| {
                [("Shop supplies", a), ("Hardware", b)]
                    .into_iter()
                    .filter_map(|(what, amount)| {
                        serde_json::to_value(Line {
                            description: Some(found(what.to_string())),
                            quantity: None,
                            unit_price: None,
                            amount: found(amount),
                        })
                        .ok()
                    })
                    .collect()
            })
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let Value::Object(fields) = serde_json::to_value(&header).ok()? else {
        return None;
    };
    let reading = DraftReading {
        kind: DraftKind::CashExpense,
        reader: DraftReader::TextPdf,
        fields,
        lines: read_lines,
        checks: Vec::new(),
        suggested_contact: None,
        doc_reference: Some("R-1042".into()),
        doc_total: Some(total),
        doc_date: r.date.add_days(-2).ok(),
    };
    let stored =
        super::super::drafts::store_reading(state, tenant, d.id, &d.updated_at, &reading).ok()?;
    state.drafts.feed_seeded.push(stored.id);
    Some(stored.id)
}

/// A ready receipt for the first waiting money-out line of the sample
/// with an account proposed (`dev.demo_feed_receipt`): the line and the
/// receipt.
pub fn seed_receipt(state: &mut DemoState, lines: bool) -> Option<(FeedItemId, DocumentDraftId)> {
    super::refresh(state);
    discard_seeded(state);
    let r = pick_line(state)?;
    seed_receipt_for(state, r.id, lines).map(|d| (r.id, d))
}

/// Throws away the receipts [`seed_receipt`] made that are still live
/// (`dev.demo_feed_reset`: every scenario run starts without them).
pub fn discard_seeded(state: &mut DemoState) {
    let Some(tenant) = state.open else {
        return;
    };
    for old in std::mem::take(&mut state.drafts.feed_seeded) {
        let _ = super::super::drafts::discard(state, tenant, old);
    }
}

/// `total` (whole cents) as two positive whole-cent parts that add up to
/// it exactly: the whole dollars' lower half and the rest.
fn split_cents(total: Money) -> Option<(Money, Money)> {
    if !cl_model::bank_feed::whole_cents(total) {
        return None;
    }
    let first = Money::parse_wire("1.00").ok()?;
    let rest = total.checked_sub(first).ok()?;
    rest.is_positive().then_some((first, rest))
}

#[cfg(test)]
#[path = "feed_review_receipts_tests.rs"]
mod tests;
