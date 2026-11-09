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

/// Which line a seeded receipt goes with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SeedLine {
    /// A line with an account proposed.
    Proposed,
    /// A line nothing was suggested for (A05 QA-1: in a new company, every
    /// line).
    Bare,
    /// A money-in line not matched to an entry (A05 QA-7: a refund).
    MoneyIn,
}

/// A waiting money-out line of the sample, whole cents, no receipt yet,
/// with an account proposed or with nothing suggested: the first one in
/// the list's order.
fn pick_line(state: &DemoState, which: SeedLine) -> Option<Row> {
    let mut rows: Vec<&Row> = state
        .feed_review
        .rows
        .iter()
        .filter(|r| {
            r.waiting()
                && (r.amount.is_positive() != (which == SeedLine::MoneyIn))
                && match which {
                    SeedLine::Proposed => r.kind == "ready" && r.suggested.is_some(),
                    SeedLine::Bare => r.suggested.is_none(),
                    SeedLine::MoneyIn => r.kind != "match",
                }
                && !r.transfer
                && !r.possible_duplicate
                && r.kind != "match"
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
    seed_receipt_named(state, id, lines, "receipt.pdf")
}

/// [`seed_receipt_for`] with the file named `file_name`.
pub fn seed_receipt_named(
    state: &mut DemoState,
    id: FeedItemId,
    lines: bool,
    file_name: &str,
) -> Option<DocumentDraftId> {
    seed_receipt_as(state, id, lines, file_name, false, None)
}

/// A credit memo for money-in line `id` (`dev.demo_feed_receipt_credit`,
/// A05 QA-7): read as a card charge whose total is the bank amount below
/// zero, as a refund reads. The total is stored without its minus as the
/// engine does (none at all), so only the reading carries the sign.
pub fn seed_credit(state: &mut DemoState) -> Option<(FeedItemId, DocumentDraftId)> {
    super::refresh(state);
    discard_seeded(state);
    let r = pick_line(state, SeedLine::MoneyIn)?;
    seed_receipt_as(state, r.id, false, "credit.pdf", true, None).map(|d| (r.id, d))
}

fn seed_receipt_as(
    state: &mut DemoState,
    id: FeedItemId,
    lines: bool,
    file_name: &str,
    credit: bool,
    vendor: Option<cl_core::ids::ContactId>,
) -> Option<DocumentDraftId> {
    let r = state.feed_review.rows.iter().find(|r| r.id == id)?.clone();
    let tenant = state.open?;
    // Each receipt's bytes differ (a draft claims its file's hash).
    let key = DraftKey::new();
    let bytes = format!("%PDF-1.4\n% synthetic demo receipt {key}\n%%EOF\n");
    let file =
        super::super::checks::upload(state, AttachTo::Draft(key), file_name, bytes.as_bytes())
            .ok()?;
    let d = super::super::drafts::create(
        state,
        tenant,
        file.id,
        if credit {
            DraftKind::CardCharge
        } else {
            DraftKind::CashExpense
        },
        IdempotencyKey::new(),
    )
    .ok()?;
    let mut total = Money::parse_wire(&r.amount.abs().to_wire()).ok()?;
    if credit {
        total = total.checked_neg().ok()?;
    }
    let kind = if credit {
        DraftKind::CardCharge
    } else {
        DraftKind::CashExpense
    };
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
        kind,
        reader: DraftReader::TextPdf,
        fields,
        lines: read_lines,
        checks: Vec::new(),
        suggested_contact: vendor,
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
    seed_receipt_on(state, SeedLine::Proposed, lines)
}

/// [`seed_receipt`] on a line of kind `which`
/// (`dev.demo_feed_receipt_bare` takes a line with nothing suggested).
pub fn seed_receipt_on(
    state: &mut DemoState,
    which: SeedLine,
    lines: bool,
) -> Option<(FeedItemId, DocumentDraftId)> {
    super::refresh(state);
    discard_seeded(state);
    let r = pick_line(state, which)?;
    seed_receipt_for(state, r.id, lines).map(|d| (r.id, d))
}

/// Two copies of one receipt for the same line, files `receipt-a.pdf` and
/// `receipt-b.pdf` (`dev.demo_feed_receipt_two`, QA-5: the offers must be
/// told apart).
pub fn seed_two_receipts(state: &mut DemoState) -> Option<(FeedItemId, [DocumentDraftId; 2])> {
    super::refresh(state);
    discard_seeded(state);
    let r = pick_line(state, SeedLine::Proposed)?;
    let a = seed_receipt_named(state, r.id, false, "receipt-a.pdf")?;
    let b = seed_receipt_named(state, r.id, false, "receipt-b.pdf")?;
    Some((r.id, [a, b]))
}

/// PR-A1 B1 (`dev.demo_feed_receipt_dup`): the owner entered the bill by
/// hand (the story vendor, reference R-1042, the bank amount, three days
/// before the line) and the vendor's receipt of it is waiting: the line
/// and the receipt. Synthetic only.
pub fn seed_duplicate(state: &mut DemoState) -> Option<(FeedItemId, DocumentDraftId)> {
    seed_duplicate_as(state, true)
}

/// PR-A1 QA-1 (`dev.demo_feed_receipt_dup_no_vendor`): the same, but the
/// receipt's vendor was not matched when it was read (the owner added the
/// vendor afterwards): the check still finds the bill (Q-PRA1d).
pub fn seed_duplicate_no_vendor(state: &mut DemoState) -> Option<(FeedItemId, DocumentDraftId)> {
    seed_duplicate_as(state, false)
}

fn seed_duplicate_as(
    state: &mut DemoState,
    with_vendor: bool,
) -> Option<(FeedItemId, DocumentDraftId)> {
    if !with_vendor {
        // An engine with `document-duplicates-no-vendor-v1`.
        state.drafts.no_vendor_check = true;
    }
    super::refresh(state);
    discard_seeded(state);
    let r = pick_line(state, SeedLine::Proposed)?;
    let vendor = state
        .contacts_mut()
        .iter()
        .find(|c| c.fields.display_name == super::super::bills::STORY_VENDOR)
        .map(|c| c.id)?;
    let total = r.amount.abs();
    let bill = cl_api::BillDraft {
        vendor,
        date: r.date.add_days(-3).ok()?,
        vendor_invoice_number: Some("R-1042".into()),
        terms: None,
        due_date: None,
        memo: None,
        amount_due: total,
        lines: vec![cl_api::BillLineDraft::expense(
            super::super::account_id(6210),
            total,
        )],
        received: true,
        draft_attachment: None,
    };
    super::super::bills::create_bill(state, &bill, None).ok()?;
    super::refresh(state);
    let read_vendor = with_vendor.then_some(vendor);
    seed_receipt_as(state, r.id, false, "invoice.pdf", false, read_vendor).map(|d| (r.id, d))
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
