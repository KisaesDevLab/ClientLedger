//! Company lifecycle screens (P08): Open Company, New Company interview,
//! Recovery key, Opening balances, Back Up / Portable / Export, Delete.

pub mod backup;
pub mod delete;
pub mod form;
pub mod new_interview;
pub mod open;
pub mod opening_balances;
pub mod recovery_key;

use std::sync::Arc;

use cl_core::CalendarDate;
use cl_core::ids::TenantId;
use cl_model::CommandId;

use crate::company::CompanyLink;
use crate::windows::Screen;

/// Command ids of windows that are not in the menu (opened by other
/// screens): the recovery key and opening balances after the interview.
pub const RECOVERY_KEY: &str = "company.recovery_key";
/// See [`RECOVERY_KEY`].
pub const OPENING_BALANCES: &str = "company.opening_balances";

/// Builds the screen for a command, or `None` when another module owns it.
#[must_use]
pub fn make(
    kind: CommandId,
    link: &Arc<CompanyLink>,
    open: Option<(TenantId, String)>,
    today: CalendarDate,
) -> Option<Box<dyn Screen>> {
    let link = Arc::clone(link);
    Some(match kind.as_str() {
        "file.open_company" => Box::new(open::OpenCompanyScreen::new(link)),
        "file.new_company" => Box::new(new_interview::NewCompanyScreen::new(link)),
        "file.backup" => Box::new(backup::BackupScreen::new(link, backup::Job::Backup, open)),
        "file.portable_copy" => {
            Box::new(backup::BackupScreen::new(link, backup::Job::Portable, open))
        }
        "file.export_all" => Box::new(backup::BackupScreen::new(
            link,
            backup::Job::ExportAll,
            open,
        )),
        "file.delete_company" => Box::new(delete::DeleteCompanyScreen::new(link, open)),
        RECOVERY_KEY => Box::new(recovery_key::RecoveryKeyScreen::new(link)),
        OPENING_BALANCES => Box::new(opening_balances::OpeningBalancesScreen::new(link, today)),
        _ => return None,
    })
}

/// Tab titles for the two windows without a command-table entry.
#[must_use]
pub fn extra_title(kind: CommandId) -> Option<String> {
    match kind.as_str() {
        RECOVERY_KEY => Some(crate::i18n::t("recovery.tab")),
        OPENING_BALANCES => Some(crate::i18n::t("opening.tab")),
        _ => None,
    }
}
