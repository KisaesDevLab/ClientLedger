//! Shared form-to-engine-request mapping used by the desktop forms and the
//! MCP tools (plan D0, Q-A11l), so an MCP save and a human save send the
//! same JSON.
//!
//! The pure checks live in `cl_model::forms`; what is here reads `cl_api`
//! types: the tag gate over the engine's tag and account lists, context
//! built from engine capabilities and lists, and each form's checked state
//! (`Checked*`) as the `cl_api` draft it sends. Every conversion copies the
//! checked values field by field, with no arithmetic: amounts stay the
//! owner's typed `Money`. Plain-language texts and screen state stay in
//! `cl-ui`.

pub mod accounts;
pub mod batch;
pub mod bill;
pub mod card;
pub mod check;
pub mod daily_sales;
pub mod deposit;
pub mod finance_charges;
pub mod inventory;
pub mod invoice;
pub mod journal;
pub mod lists;
pub mod loan_payment;
pub mod owner_money;
pub mod pay_bills;
pub mod payment;
pub mod personal_funds;
pub mod sales_tax;
pub mod statements;
pub mod tag_gate;
pub mod transfer;

use cl_core::ids::TagId;
use cl_model::lists::tags::TagList;

/// Q-276: the header tags a new, untouched form starts with (the company's
/// default tag), or `None` when it keeps what it has (no default, or the
/// header already has tags). Whether the form is new and untouched is the
/// caller's to decide: never on an edit.
#[must_use]
pub fn default_header_tags(list: &TagList, current: &[TagId]) -> Option<Vec<TagId>> {
    let tags = cl_model::forms::tag_rule::new_form_header_tags(list);
    (!tags.is_empty() && current.is_empty()).then_some(tags)
}
