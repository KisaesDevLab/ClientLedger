//! Transfer Funds (P15): the checked transfer as the engine's draft, field
//! by field (no arithmetic); `BankPosting::transfer` refuses again before
//! anything is sent.

use cl_api::TransferDraft;
use cl_model::forms::transfer::CheckedTransfer;

/// The checked transfer as the engine's draft.
#[must_use]
pub fn to_draft(c: &CheckedTransfer) -> TransferDraft {
    TransferDraft {
        from: c.from,
        to: c.to,
        date: c.date,
        amount: c.amount,
        memo: c.memo.clone(),
    }
}
