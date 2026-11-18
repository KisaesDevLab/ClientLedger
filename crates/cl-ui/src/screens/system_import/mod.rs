//! Import from another accounting program (P31 step 7, Q-275): Accounting
//! Power, QuickBooks Online, QuickBooks Desktop (chart and general ledger
//! only) or the Generic CSV layout, on the engine's Bulk Import pipeline.
//! The window is the QuickBooks Desktop wizard's ([`super::qbd_import`])
//! with a source picker on its first step; its mapping and tie-out views
//! are the same ones, re-exported here.

use std::sync::Arc;

use cl_core::CalendarDate;
use cl_model::CommandId;
use cl_model::imports::wizard::Source;

use crate::company::CompanyLink;
use crate::screens::qbd_import::ImportScreen;
use crate::windows::Screen;

pub mod mapping;
pub mod tieout;
pub mod wizard;

/// The command and window kind.
pub const KIND: &str = "file.import_system";

/// The sources this window offers, in picker order.
pub const SOURCES: [Source; 4] = [
    Source::AccountingPower,
    Source::Qbo,
    Source::QbdGl,
    Source::Generic,
];

/// The other-programs window for `kind`, if it is one.
#[must_use]
pub fn make(
    kind: CommandId,
    link: &Arc<CompanyLink>,
    _open: Option<(cl_core::ids::TenantId, String)>,
    today: CalendarDate,
) -> Option<Box<dyn Screen>> {
    (kind.as_str() == KIND).then(|| {
        Box::new(ImportScreen::new(Arc::clone(link), today, SOURCES[0])) as Box<_>
    })
}

#[cfg(test)]
mod tests;
