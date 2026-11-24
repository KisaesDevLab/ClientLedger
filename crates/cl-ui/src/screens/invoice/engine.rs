//! Who answers Create Invoice: the live engine over the window's request
//! group (reads and writes on the queue, never on the egui thread) or the
//! `--demo` engine (`company::demo::invoices`, answered at once). Each call
//! returns a [`Job`] the screen polls each frame.
//!
//! The checked `InvoiceRequest` becomes a `cl_api::NewInvoice` field by
//! field, and the engine's preview the model's `PreviewFigures`, in
//! `cl_mcp::forms::invoice` (A11 D0), re-exported here. Nothing is
//! computed here.

use std::future::Future;

use cl_api::{ApiError, LocalValidation, OrphanLabel, RequestGroup, Session};

use crate::company::demo::DemoState;
use crate::company::{CompanyLink, Task};

/// A call in flight (or answered).
pub(crate) enum Job<T> {
    /// On the queue (or a demo answer).
    Task(Task<Result<T, ApiError>>),
    /// Answered at once.
    Done(Option<Result<T, ApiError>>),
}

impl<T> Job<T> {
    /// The answer once; `None` while running.
    pub(crate) fn take(&mut self) -> Option<Result<T, ApiError>> {
        match self {
            Job::Task(t) => t.poll_result(),
            Job::Done(v) => v.take(),
        }
    }
}

impl<T> std::fmt::Debug for Job<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Job")
    }
}

/// Polls `slot`: the answer once, the slot emptied.
pub(crate) fn poll<T>(slot: &mut Option<Job<T>>) -> Option<Result<T, ApiError>> {
    let r = slot.as_mut()?.take()?;
    *slot = None;
    Some(r)
}

/// A read: the demo answers at once, the live engine on the queue.
pub(crate) fn read<T, F, Fut>(
    link: &CompanyLink,
    group: Option<&RequestGroup>,
    demo: impl FnOnce(&mut DemoState) -> Result<T, ApiError>,
    live: F,
) -> Job<T>
where
    T: Send + 'static,
    F: FnOnce(Session) -> Fut + Send + 'static,
    Fut: Future<Output = Result<T, ApiError>> + Send + 'static,
{
    if let Some(mut d) = link.demo_state() {
        return Job::Done(Some(demo(&mut d)));
    }
    let (Some(session), Some(group)) = (link.session(), group) else {
        return Job::Done(Some(Err(ApiError::NoCompanyOpen)));
    };
    match group.read(&session, move |s| async move { live(s).await.map(Ok) }) {
        Ok(p) => Job::Task(Task::Pending(p)),
        Err(_) => Job::Done(Some(Err(ApiError::Local(LocalValidation::InvalidValue)))),
    }
}

/// A write (labelled for the orphan log; never names a value).
pub(crate) fn write<T, F, Fut>(
    link: &CompanyLink,
    group: Option<&RequestGroup>,
    label: OrphanLabel,
    demo: impl FnOnce(&mut DemoState) -> Result<T, ApiError>,
    live: F,
) -> Job<T>
where
    T: Send + 'static,
    F: FnOnce(Session) -> Fut + Send + 'static,
    Fut: Future<Output = Result<T, ApiError>> + Send + 'static,
{
    if let Some(mut d) = link.demo_state() {
        return Job::Done(Some(demo(&mut d)));
    }
    let (Some(session), Some(group)) = (link.session(), group) else {
        return Job::Done(Some(Err(ApiError::NoCompanyOpen)));
    };
    match group.write(
        &session,
        label,
        move |s| async move { live(s).await.map(Ok) },
    ) {
        Ok(p) => Job::Task(Task::Pending(p)),
        Err(_) => Job::Done(Some(Err(ApiError::Local(LocalValidation::InvalidValue)))),
    }
}

/// The checked request as the engine's invoice, credit memo or sales
/// receipt, and the engine's previews as the model's figures (A11 D0:
/// shared with the MCP tools).
pub(crate) use cl_mcp::forms::invoice::{
    credit_figures_of, figures_of, new_credit_memo, new_invoice, new_sales_receipt,
};

/// The orphan-log label of a write.
pub(crate) fn label(what: &'static str, key: Option<cl_api::IdempotencyKey>) -> OrphanLabel {
    OrphanLabel { what, key }
}
