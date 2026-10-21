//! Screens (one module per area). P08 adds the company lifecycle screens;
//! every later phase adds its own folder here.

pub mod company;
pub mod dev;
pub mod lists;
pub mod preferences;
pub mod register;
pub mod write_check;

// P14: deposits, transfers, daily sales, card charges, owner and loan
// helpers (one folder per screen; each lane owns its own).
pub mod card_charge;
pub mod daily_sales;
pub mod deposit;
pub mod loan_payment;
pub mod owner_money;
pub mod personal_funds;
pub mod transfer;

// P15: journal entries, templates and the catch-up summary (each lane owns
// its own folder).
pub mod journal_entry;
pub mod templates;
pub mod templates_due;

use cl_model::CommandId;

use crate::windows::{Placeholder, Screen};

/// A scaffolded screen (WP-S): its command opens a window of its own kind
/// until the lane that builds it replaces the stub.
fn stub(kind: CommandId, own: &str) -> Option<Box<dyn Screen>> {
    (kind.as_str() == own).then(|| Box::new(Placeholder::for_command(kind)) as Box<dyn Screen>)
}

/// Every P14 screen's `make`, in menu order (the shell tries each).
pub(crate) const P14_SCREENS: [crate::shell::MakeScreen; 7] = [
    deposit::make,
    transfer::make,
    daily_sales::make,
    card_charge::make,
    owner_money::make,
    personal_funds::make,
    loan_payment::make,
];

/// Every P15 screen's `make` (the shell tries each after P14's).
pub(crate) const P15_SCREENS: [crate::shell::MakeScreen; 3] =
    [journal_entry::make, templates::make, templates_due::make];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_p15_screen_opens_its_own_kind_only() {
        let dir = tempfile::tempdir().expect("tmp");
        let link = std::sync::Arc::new(crate::company::CompanyLink::demo(dir.path()));
        let today = cl_core::CalendarDate::from_ymd(2026, 11, 4).expect("date");
        let kinds = [journal_entry::KIND, templates::KIND, templates_due::KIND];
        for (make, kind) in P15_SCREENS.iter().zip(kinds) {
            let cmd = cl_model::commands::find(kind).expect("a command").id;
            let s = make(cmd, &link, None, today).expect("its screen");
            assert_eq!(s.title(), crate::i18n::t(&format!("cmd.{kind}")));
            let other = cl_model::commands::find("vendors.write_check")
                .expect("write check")
                .id;
            assert!(make(other, &link, None, today).is_none(), "{kind}");
        }
    }

    #[test]
    fn each_p14_screen_opens_its_own_kind_only() {
        let dir = tempfile::tempdir().expect("tmp");
        let link = std::sync::Arc::new(crate::company::CompanyLink::demo(dir.path()));
        let today = cl_core::CalendarDate::from_ymd(2026, 11, 4).expect("date");
        let kinds = [
            deposit::KIND,
            transfer::KIND,
            daily_sales::KIND,
            card_charge::KIND,
            owner_money::KIND,
            personal_funds::KIND,
            loan_payment::KIND,
        ];
        for (make, kind) in P14_SCREENS.iter().zip(kinds) {
            let cmd = cl_model::commands::find(kind).expect("a command").id;
            let s = make(cmd, &link, None, today).expect("its screen");
            assert_eq!(s.title(), crate::i18n::t(&format!("cmd.{kind}")));
            let other = cl_model::commands::find("vendors.write_check")
                .expect("write check")
                .id;
            assert!(make(other, &link, None, today).is_none(), "{kind}");
        }
    }
}
