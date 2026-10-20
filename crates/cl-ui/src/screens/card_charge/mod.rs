//! Record Card Charge (P14 lane C, `vendors.card_charge`, Ctrl+Shift+C).
//!
//! Scaffold (WP-S): the command opens this window kind; WP-C3 builds the
//! form on the Write Check pattern (Q-261).

use std::sync::Arc;

use cl_core::CalendarDate;
use cl_core::ids::TenantId;
use cl_model::CommandId;

use crate::company::CompanyLink;
use crate::windows::Screen;

/// The command and window kind.
pub const KIND: &str = "vendors.card_charge";

/// The Record Card Charge window for `kind`, if it is one.
#[must_use]
pub fn make(
    kind: CommandId,
    _link: &Arc<CompanyLink>,
    _open: Option<(TenantId, String)>,
    _today: CalendarDate,
) -> Option<Box<dyn Screen>> {
    super::stub(kind, KIND)
}
