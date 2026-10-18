//! The master lists (P11). Step 1 builds the Chart of Accounts; the other
//! lists follow in later steps of the phase.

pub mod accounts;

use std::sync::Arc;

use cl_core::CalendarDate;
use cl_core::ids::TenantId;
use cl_model::CommandId;

use crate::company::CompanyLink;
use crate::windows::Screen;

/// The list window for `kind`, if it is one.
#[must_use]
pub fn make(
    kind: CommandId,
    link: &Arc<CompanyLink>,
    open: Option<(TenantId, String)>,
    today: CalendarDate,
) -> Option<Box<dyn Screen>> {
    match kind.as_str() {
        accounts::KIND => Some(Box::new(accounts::ChartScreen::new(
            Arc::clone(link),
            open,
            today,
        ))),
        _ => None,
    }
}
