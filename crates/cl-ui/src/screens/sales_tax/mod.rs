//! Sales tax screens (P23): Sales Tax Setup (WP-2); Sales Tax Owed, Pay
//! Sales Tax, exemption certificates and the bad-debt recovery dialog
//! (WP-3).

pub mod exemptions;
pub mod liability;
pub mod pay;
pub mod recovery;
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
    today: CalendarDate,
) -> Option<Box<dyn Screen>> {
    match kind.as_str() {
        setup::KIND => Some(Box::new(setup::SetupScreen::new(Arc::clone(link)))),
        liability::KIND => Some(Box::new(liability::OwedScreen::new(
            Arc::clone(link),
            today,
        ))),
        pay::KIND => Some(Box::new(pay::PayScreen::new(Arc::clone(link), today))),
        exemptions::KIND => Some(Box::new(exemptions::ExemptionsScreen::new(
            Arc::clone(link),
            today,
        ))),
        recovery::KIND => Some(Box::new(recovery::RecoveryScreen::new(
            Arc::clone(link),
            today,
        ))),
        _ => None,
    }
}
