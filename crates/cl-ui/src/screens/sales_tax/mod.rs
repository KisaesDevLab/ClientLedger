//! Sales tax screens (P23): Sales Tax Setup (WP-2). The liability report,
//! Pay Sales Tax, exemption certificates and the recovery dialog arrive
//! with WP-3.

pub mod setup;

use std::sync::Arc;

use cl_core::CalendarDate;
use cl_core::ids::TenantId;
use cl_model::CommandId;

use crate::company::CompanyLink;
use crate::windows::Screen;

/// The sales tax window for `kind`, if it is one.
#[must_use]
pub fn make(
    kind: CommandId,
    link: &Arc<CompanyLink>,
    _open: Option<(TenantId, String)>,
    _today: CalendarDate,
) -> Option<Box<dyn Screen>> {
    match kind.as_str() {
        setup::KIND => Some(Box::new(setup::SetupScreen::new(Arc::clone(link)))),
        _ => None,
    }
}
