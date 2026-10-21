//! Templates Due (P15 lane C, `lists.templates_due`): the summary the
//! catch-up runner opens after a run; no menu entry.
//!
//! Scaffold (WP-S): the kind opens this window; C2 builds it.

use std::sync::Arc;

use cl_core::CalendarDate;
use cl_core::ids::TenantId;
use cl_model::CommandId;

use crate::company::CompanyLink;
use crate::windows::Screen;

/// The window kind.
pub const KIND: &str = "lists.templates_due";

/// The Templates Due window for `kind`, if it is one.
#[must_use]
pub fn make(
    kind: CommandId,
    _link: &Arc<CompanyLink>,
    _open: Option<(TenantId, String)>,
    _today: CalendarDate,
) -> Option<Box<dyn Screen>> {
    super::stub(kind, KIND)
}
