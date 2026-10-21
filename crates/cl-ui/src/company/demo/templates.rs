//! The demo engine's templates and catch-up run (P15 lane C, U-62/U-63).
//! Synthetic.
//!
//! WP-S scaffold: the state the demo twin keeps. Lane C adds the twin of
//! `run-due`, `enter`, `skip` and `resolve` (the engine's clamped dates,
//! the unique `(template, date)` key in [`DemoState::occurrences`],
//! closed-period and inactive-account skips, `more`/limit and `runId`
//! replay); WP-D adds the sample's three templates.
//!
//! [`DemoState::occurrences`]: super::DemoState::occurrences

use std::fmt;

use cl_api::TemplateDraft;
use cl_core::ids::{RunId, TemplateGroupId, TemplateId, TransactionId};

/// A stored template as the demo engine keeps it.
#[derive(Clone, PartialEq, Eq)]
pub struct DemoTemplate {
    /// Its id.
    pub id: TemplateId,
    /// The owner's name for it.
    pub name: String,
    /// Its group, if any.
    pub group: Option<TemplateGroupId>,
    /// The stored draft (a form's create body minus the date and numbers).
    pub draft: TemplateDraft,
    /// The desktop form it opens in (`clientForm`, P15 lane T).
    pub client_form: String,
    /// Its schedule; `None` is "Don't remind me" (lane T).
    pub schedule: Option<cl_api::TemplateSchedule>,
    /// The day-of-month anchor the clamped month rule counts from (the
    /// next date the owner last set; lane T).
    pub anchor: Option<cl_core::CalendarDate>,
    /// The last date it was entered (lane T).
    pub last_entered: Option<cl_core::CalendarDate>,
    /// The engine's row version (409 `STALE` when a write names another).
    pub version: u32,
    /// Soft-deleted (gone from the list; its occurrences stay).
    pub deleted: bool,
}

/// Ids and the version only: names and amounts are owner data.
impl fmt::Debug for DemoTemplate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DemoTemplate")
            .field("id", &self.id)
            .field("group", &self.group)
            .field("version", &self.version)
            .field("deleted", &self.deleted)
            .finish_non_exhaustive()
    }
}

/// A template group.
#[derive(Clone, PartialEq, Eq)]
pub struct DemoTemplateGroup {
    /// Its id.
    pub id: TemplateGroupId,
    /// Its name.
    pub name: String,
}

/// The id only.
impl fmt::Debug for DemoTemplateGroup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DemoTemplateGroup")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

/// What happened to one occurrence (`template_occurrences.status`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OccStatus {
    /// Posted by the catch-up run.
    Posted,
    /// Entered by the owner (Enter now, or a form linked with `resolve`).
    Entered,
    /// Could not be posted (a closed period, an inactive account).
    Skipped,
    /// The owner chose Skip this one or Dismiss.
    Dismissed,
}

/// One occurrence, under its unique `(template, date)` key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Occ {
    /// What happened.
    pub status: OccStatus,
    /// The transaction it posted or was linked to.
    pub transaction: Option<TransactionId>,
    /// Why it was skipped (the engine's reason code).
    pub reason: Option<&'static str>,
    /// The run that handled it (replayed for the same `runId`).
    pub run: Option<RunId>,
}
