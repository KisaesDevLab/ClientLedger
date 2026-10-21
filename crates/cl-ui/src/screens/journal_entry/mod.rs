//! Make Journal Entry (P15 lane J, `company.journal_entry`, Ctrl+J).
//!
//! Scaffold (WP-S): the command opens this window kind; J4 builds the
//! debit/credit form on the Write Check pattern (Q-261, Q-P15t).

use std::sync::Arc;

use cl_core::CalendarDate;
use cl_core::ids::TenantId;
use cl_model::CommandId;

use crate::company::CompanyLink;
use crate::windows::Screen;

/// The command and window kind.
pub const KIND: &str = "company.journal_entry";

/// The Make Journal Entry window for `kind`, if it is one.
#[must_use]
pub fn make(
    kind: CommandId,
    _link: &Arc<CompanyLink>,
    _open: Option<(TenantId, String)>,
    _today: CalendarDate,
) -> Option<Box<dyn Screen>> {
    super::stub(kind, KIND)
}
