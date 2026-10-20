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

#[cfg(test)]
mod tests {
    use super::*;

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
