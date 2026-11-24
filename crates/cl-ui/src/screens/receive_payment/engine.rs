//! Who answers Receive Payment: the live engine on the window's request
//! group or the `--demo` engine (`company::demo::receivables`), through the
//! invoice screen's [`Job`] helpers. The model's request becomes the API's,
//! and the engine's preview the model's figures (copied, never computed),
//! in `cl_mcp::forms::payment` (A11 D0), re-exported here.

use cl_api::OrphanLabel;

pub(crate) use crate::screens::invoice::engine::{Job, poll, read, write};
pub(crate) use cl_mcp::forms::payment::{
    apply_request, figures_of, new_payment, options, preview_request,
};

/// The orphan-log label of a payment write (never a value).
#[must_use]
pub(crate) fn label(key: &cl_api::IdempotencyKey) -> OrphanLabel {
    OrphanLabel {
        what: "orphan.receive_payment",
        key: Some(key.clone()),
    }
}
