//! Company lifecycle screens (P08): Open Company, New Company interview,
//! Recovery key, Opening balances, Back Up / Export, Delete.

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_company_kind_makes_its_screen_and_others_do_not() {
        let dir = tempfile::tempdir().expect("tmp");
        let link = Arc::new(CompanyLink::demo(dir.path()));
        let today = CalendarDate::from_ymd(2026, 11, 4).expect("date");
        let open = Some((
            TenantId::parse("d0000000-0000-4000-8000-000000000001").expect("id"),
            "Ozark".to_string(),
        ));
        let kinds = [
            ("file.open_company", "Open Company"),
            ("file.new_company", "New Company"),
            ("file.backup", "Back Up Company"),
            ("file.export_all", "Export All Data"),
            ("file.delete_company", "Delete Company"),
            (RECOVERY_KEY, "Recovery Key"),
            (OPENING_BALANCES, "Opening Balances"),
        ];
        for (kind, title) in kinds {
            let screen = make(CommandId::new(kind), &link, open.clone(), today)
                .unwrap_or_else(|| panic!("{kind} makes a screen"));
            assert_eq!(screen.title(), title, "{kind}");
            assert!(screen.state().is_object(), "{kind} reports state");
            // The recovery key counts as unsaved work until confirmed.
            assert_eq!(screen.dirty(), kind == RECOVERY_KEY, "{kind} dirty");
        }
        assert!(make(CommandId::new("nav.home"), &link, None, today).is_none());
        assert_eq!(
            extra_title(CommandId::new(RECOVERY_KEY)).as_deref(),
            Some("Recovery Key")
        );
        assert_eq!(
            extra_title(CommandId::new(OPENING_BALANCES)).as_deref(),
            Some("Opening Balances")
        );
        assert_eq!(extra_title(CommandId::new("nav.home")), None);
    }
}
