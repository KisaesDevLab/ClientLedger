//! Write Checks (P13): a check, checked by `cl_model::forms::check::check`,
//! as the engine's draft.
//!
//! Money (P13 plan §4): `check()` is the exact gate (the lines equal the
//! check amount to the cent, or nothing is sent); [`to_draft`] then copies
//! the checked values onto the engine draft one field at a time, with no
//! arithmetic; `BankPosting::check_with` refuses lines that don't add up
//! once more before anything leaves.

use cl_api::{Account, ApiError, CheckDraft, DraftKey, LocalValidation, SplitLine};
use cl_model::forms::check::{CheckForm, CheckedCheck};
use cl_model::forms::tag_rule::TagRefusal;
use cl_model::lists::tags::TagList;

/// The checked check as the engine's draft, field by field (no
/// arithmetic): the amount and every line amount are the owner's typed
/// `Money`, the memo is also the printed memo (Q-P13n), and an empty name
/// on the check takes the payee's display name.
///
/// # Errors
/// `Local(InvalidValue)` when a draft attachment's id is not one the
/// engine gave.
pub fn to_draft(c: &CheckedCheck, payee_display: &str) -> Result<CheckDraft, ApiError> {
    let draft_attachment = match &c.draft_attachment {
        None => None,
        Some(a) => Some(
            DraftKey::parse(&a.id).map_err(|_| ApiError::Local(LocalValidation::InvalidValue))?,
        ),
    };
    Ok(CheckDraft {
        bank: c.bank,
        date: c.date,
        payee: Some(c.payee),
        payee_name: if c.payee_name.is_empty() {
            payee_display.to_string()
        } else {
            c.payee_name.clone()
        },
        check_number: c.number,
        print_later: c.print_later,
        payee_address: c.address.clone(),
        memo: c.memo.clone(),
        printed_memo: c.memo.clone(),
        tag_ids: c.header_tags.clone(),
        draft_attachment,
        amount: c.amount,
        lines: c
            .lines
            .iter()
            .map(|l| SplitLine {
                account: l.account,
                amount: l.amount,
                memo: l.memo.clone(),
                contact: l.contact,
                billable: l.billable,
                tag: l.tag,
                item: None,
            })
            .chain(c.item_lines.iter().map(item_split))
            .collect(),
    })
}

/// A checked item line as the engine's split line (P24, U-158): the item,
/// its quantity and cost go with the line; the amount is the checked one.
#[must_use]
pub fn item_split(l: &cl_model::forms::item_lines::CheckedItemLine) -> SplitLine {
    SplitLine {
        account: l.account,
        amount: l.amount,
        memo: l.description.clone(),
        contact: l.contact,
        billable: l.billable,
        tag: l.tag,
        item: l.item.map(|item| cl_api::SplitItem {
            item,
            quantity: l.qty,
            unit_price: l.rate,
            unit: l.unit,
        }),
    }
}

/// The tag rules on the check as composed (P29 WP7): `list` is the
/// company's tag list with its rules as they are now, `accounts` the chart
/// the form read.
///
/// # Errors
/// The refused lines (nothing is sent).
pub fn tag_gate(form: &CheckForm, list: &TagList, accounts: &[Account]) -> Result<(), TagRefusal> {
    let lines =
        cl_model::forms::tag_rule::tabbed_lines(&form.header_tags, &form.lines, &form.items, list);
    super::tag_gate::check(list, &lines, accounts)
}
