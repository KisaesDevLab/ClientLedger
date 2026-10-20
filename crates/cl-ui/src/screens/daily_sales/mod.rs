//! Daily Sales Summary (P14 lane B, `clients.daily_sales`).
//!
//! Scaffold (WP-S): the command opens this window kind; WP-B3 builds the
//! form on the Write Check pattern (Q-261).

use std::sync::Arc;

use cl_core::CalendarDate;
use cl_core::ids::TenantId;
use cl_model::CommandId;

use crate::company::CompanyLink;
use crate::windows::Screen;

/// The command and window kind.
pub const KIND: &str = "clients.daily_sales";

/// The Daily Sales Summary window for `kind`, if it is one.
#[must_use]
pub fn make(
    kind: CommandId,
    _link: &Arc<CompanyLink>,
    _open: Option<(TenantId, String)>,
    _today: CalendarDate,
) -> Option<Box<dyn Screen>> {
    super::stub(kind, KIND)
}
