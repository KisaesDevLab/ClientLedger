//! Transfer Funds (P14 lane A, `banking.transfer`, Ctrl+T).
//!
//! Scaffold (WP-S): the command opens this window kind; WP-A4 builds the
//! form on the Write Check pattern (Q-261).

use std::sync::Arc;

use cl_core::CalendarDate;
use cl_core::ids::TenantId;
use cl_model::CommandId;

use crate::company::CompanyLink;
use crate::windows::Screen;

/// The command and window kind.
pub const KIND: &str = "banking.transfer";

/// The Transfer Funds window for `kind`, if it is one.
#[must_use]
pub fn make(
    kind: CommandId,
    _link: &Arc<CompanyLink>,
    _open: Option<(TenantId, String)>,
    _today: CalendarDate,
) -> Option<Box<dyn Screen>> {
    super::stub(kind, KIND)
}
