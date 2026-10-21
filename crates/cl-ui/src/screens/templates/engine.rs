//! Who answers the Templates list and Save as Template: the `--demo`
//! engine (`company::demo::template_store`, at once) or the live engine
//! over the request queue (polled each frame; the egui thread never
//! blocks).

use std::future::Future;

use cl_api::{ApiError, OrphanLabel, Pending, RequestGroup, Session};

use crate::company::CompanyLink;
use crate::company::demo::DemoState;

/// A templates call: answered (demo, or refused) or running.
pub(crate) enum Job<T> {
    /// On the queue.
    Running(Pending<T>),
    /// The answer, until taken.
    Done(Option<Result<T, ApiError>>),
}

impl<T> std::fmt::Debug for Job<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Job").finish_non_exhaustive()
    }
}

impl<T> Job<T> {
    /// The answer once; `None` while it runs.
    pub(crate) fn take(&mut self) -> Option<Result<T, ApiError>> {
        match self {
            Job::Running(p) => p.try_take(),
            Job::Done(v) => v.take(),
        }
    }
}

/// Takes a finished job's answer and clears the slot.
pub(crate) fn finished<T>(slot: &mut Option<Job<T>>) -> Option<Result<T, ApiError>> {
    let r = slot.as_mut()?.take()?;
    *slot = None;
    Some(r)
}

/// Runs a call: on the demo engine at once, else on the queue (a write
/// when `write` names its orphan label key).
pub(crate) fn run<T, F, Fut>(
    link: &CompanyLink,
    group: Option<&RequestGroup>,
    write: Option<&'static str>,
    demo: impl FnOnce(&mut DemoState) -> Result<T, ApiError>,
    live: F,
) -> Job<T>
where
    T: Send + 'static,
    F: FnOnce(Session) -> Fut + Send + 'static,
    Fut: Future<Output = Result<T, ApiError>> + Send + 'static,
{
    if let Some(mut state) = link.demo_state() {
        return Job::Done(Some(demo(&mut state)));
    }
    let (Some(session), Some(group)) = (link.session(), group) else {
        return Job::Done(Some(Err(ApiError::NoCompanyOpen)));
    };
    let started = match write {
        None => group.read(&session, live),
        Some(what) => group.write(&session, OrphanLabel { what, key: None }, live),
    };
    match started {
        Ok(p) => Job::Running(p),
        Err(_) => Job::Done(Some(Err(ApiError::Internal))),
    }
}

/// The engine takes templates: the demo always, else `templates-v1`.
pub(crate) fn templates_on(link: &CompanyLink) -> bool {
    link.is_demo() || link.capabilities().has(cl_api::Capabilities::TEMPLATES)
}
