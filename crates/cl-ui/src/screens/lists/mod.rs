//! The master lists (P11): the Chart of Accounts (step 1), the Client and
//! Vendor centers (steps 2-3), the Item List (step 4), Tags (step 5), and
//! terms, payment methods and CSV import/export (step 6).

pub mod accounts;
pub mod center;
pub mod detail_pane;
pub mod items;
pub mod rows;
pub mod tags;

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
        center::CLIENTS_KIND => Some(Box::new(center::CenterScreen::new(
            cl_model::lists::contacts::Role::Client,
            Arc::clone(link),
            open,
            today,
        ))),
        center::VENDORS_KIND => Some(Box::new(center::CenterScreen::new(
            cl_model::lists::contacts::Role::Vendor,
            Arc::clone(link),
            open,
            today,
        ))),
        items::KIND => Some(Box::new(items::ItemsScreen::new(Arc::clone(link), open))),
        tags::KIND => Some(Box::new(tags::TagsScreen::new(Arc::clone(link), open))),
        _ => None,
    }
}
