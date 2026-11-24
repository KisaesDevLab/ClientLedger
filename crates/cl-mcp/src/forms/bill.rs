//! Enter Bills (P17): a bill or vendor credit, checked by
//! `cl_model::forms::bill::check`, as the engine's draft.
//!
//! Money (P17 plan §4): `check()` is the exact gate (the lines equal the
//! amount due to the cent, or nothing is sent); [`to_draft`] then copies the
//! checked values onto the engine draft one field at a time, with no
//! arithmetic. Item amounts are the checked `cost × qty` (D6), which the
//! engine recomputes and refuses on a mismatch.

use cl_api::{
    Account, BillDraft, BillLineDraft, BillTerms, Capabilities, DraftKey, VendorCreditDraft,
};
use cl_model::forms::bill::{BillCaps, BillForm, BillKind, CheckedBill, PaymentTerm};
use cl_model::forms::tag_rule::TagRefusal;
use cl_model::lists::tags::TagList;

/// The engine draft of a bill or a vendor credit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Draft {
    /// `POST /bills`, `PUT /bills/{id}`.
    Bill(BillDraft),
    /// `POST /vendor-credits`, `PUT /vendor-credits/{id}`.
    Credit(VendorCreditDraft),
}

/// The checked bill as the engine's draft, field by field (no arithmetic):
/// the amount due and every line amount are the owner's typed `Money` (an
/// item line's is the checked `cost × qty`). `draft` is sent on the create
/// only with `bill-lines-v1` (`lines`); otherwise the files are moved after
/// the save.
#[must_use]
pub fn to_draft(c: &CheckedBill, draft: Option<DraftKey>, lines: bool) -> Draft {
    let mut out: Vec<BillLineDraft> = c
        .expense_lines
        .iter()
        .map(|l| BillLineDraft {
            account: l.account,
            amount: l.amount,
            description: l.memo.clone(),
            tag: l.tag,
            item: None,
            quantity: None,
            unit_price: None,
            customer: l.contact,
            billable: l.billable,
            unit: None,
        })
        .collect();
    out.extend(c.item_lines.iter().map(|l| BillLineDraft {
        account: l.account,
        amount: l.amount,
        description: l.description.clone(),
        tag: l.tag,
        item: l.item,
        quantity: l.qty,
        unit_price: l.rate,
        customer: l.contact,
        billable: l.billable,
        unit: l.unit,
    }));
    let draft_attachment = draft.filter(|_| lines && !c.attachments.is_empty());
    match c.kind {
        BillKind::Bill => Draft::Bill(BillDraft {
            vendor: c.vendor,
            date: c.date,
            vendor_invoice_number: c.reference.clone(),
            terms: c.terms.as_ref().map(|t| BillTerms {
                code: t.code.clone(),
                days: t.days.map(u32::from),
                discount_percent: t
                    .discount_percent
                    .and_then(|p| cl_api::Percent::parse(&percent_text(p)).ok()),
                discount_days: t.discount_days.map(u32::from),
            }),
            due_date: c.due_date,
            memo: c.memo.clone(),
            amount_due: c.amount,
            lines: out,
            received: c.bill_received.unwrap_or(true),
            draft_attachment,
        }),
        BillKind::Credit => Draft::Credit(VendorCreditDraft {
            vendor: c.vendor,
            date: c.date,
            vendor_invoice_number: c.reference.clone(),
            memo: c.memo.clone(),
            amount: c.amount,
            lines: out,
            draft_attachment,
        }),
    }
}

/// A percentage as the engine's strict text (`2`, `1.5`).
#[must_use]
pub fn percent_text(p: cl_model::forms::bill::Percent) -> String {
    let w = p.to_wire();
    if w.contains('.') {
        w.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        w
    }
}

/// One of the engine's payment terms (`GET /payment-terms`) as the form
/// checks it.
#[must_use]
pub fn term_of(p: cl_api::PaymentTerm) -> Option<PaymentTerm> {
    let discount_percent = match p.discount_percent {
        Some(pct) => Some(cl_model::forms::bill::Percent::parse_user(&pct.to_wire())?),
        None => None,
    };
    Some(PaymentTerm {
        code: p.code,
        net_days: p.net_days.and_then(|d| u16::try_from(d).ok()),
        discount_percent,
        discount_days: p.discount_days.and_then(|d| u16::try_from(d).ok()),
    })
}

/// What a live engine lets a bill carry.
#[must_use]
pub fn caps(c: &Capabilities) -> BillCaps {
    BillCaps {
        closing: c.has(Capabilities::AP_CLOSING),
        lines: c.has(Capabilities::BILL_LINES),
        terms: c.has(Capabilities::BILL_TERMS),
        received: c.has(Capabilities::BILL_RECEIVED),
    }
}

/// The tag rules on the bill as composed (P29 WP7, Q-276): `list` is the
/// company's tag list with its rules as they are now, `accounts` the chart
/// the form read.
///
/// # Errors
/// The refused lines (nothing is sent).
pub fn tag_gate(form: &BillForm, list: &TagList, accounts: &[Account]) -> Result<(), TagRefusal> {
    let lines = cl_model::forms::tag_rule::tabbed_lines(
        &form.header_tags,
        &form.expenses,
        &form.items,
        list,
    );
    super::tag_gate::check(list, &lines, accounts)
}
