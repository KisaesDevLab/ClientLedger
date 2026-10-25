//! English text for the Enter Bill vendor panel and duplicate warning (P17 WP8). Keys:
//! `bill.panel.*`.
//! Registered in `i18n.rs` by WP0; only its work package adds rows.

/// The vendor panel's part of the English catalog.
#[rustfmt::skip]
pub const ENGLISH: &[(&str, &str)] = &[
    // P17 WP8
    ("bill.panel.no_vendor", "Choose a vendor to see their balance and bills."),
    ("bill.panel.open_balance", "Open balance"),
    ("bill.panel.default_account", "Default account"),
    ("bill.panel.is_1099", "1099 vendor"),
    ("bill.panel.yes", "Yes"),
    ("bill.panel.no", "No"),
    ("bill.panel.none", "None"),
    ("bill.panel.loading", "Loading\u{2026}"),
    ("bill.panel.failed", "Couldn't load this."),
    ("bill.panel.try_again", "Try again"),
    ("bill.panel.dup.checking", "Checking ref. no. {ref}\u{2026}"),
    ("bill.panel.dup.none", "No other bill uses ref. no. {ref} \u{2713}"),
    ("bill.panel.dup.none_credit", "No other credit uses ref. no. {ref} \u{2713}"),
    ("bill.panel.dup.found", "Bill {number} on {date} already uses ref. no. {ref}."),
    ("bill.panel.dup.found_no_number", "A bill on {date} already uses ref. no. {ref}."),
    ("bill.panel.dup.found_credit", "Credit {number} on {date} already uses ref. no. {ref}."),
    ("bill.panel.dup.found_credit_no_number", "A credit on {date} already uses ref. no. {ref}."),
    ("bill.panel.dup.open", "Open that bill"),
    ("bill.panel.dup.open_credit", "Open that credit"),
    ("bill.panel.dup.unchecked", "Couldn't check every bill for ref. no. {ref}. Look in the vendor's bills before saving."),
    ("bill.panel.dup.unchecked_credit", "Couldn't check every credit for ref. no. {ref}. Look in the vendor's credits before saving."),
    ("bill.panel.prompt.title", "Ref. no. already used"),
    ("bill.panel.prompt.body", "Another bill from this vendor already uses ref. no. {ref} ({date}). You may be entering the same bill twice."),
    ("bill.panel.prompt.body_credit", "Another credit from this vendor already uses ref. no. {ref} ({date}). You may be entering the same credit twice."),
    ("bill.panel.prompt.save_anyway", "Save anyway"),
    ("bill.panel.prompt.go_back", "Go back"),
    ("bill.panel.open_bills", "Open bills"),
    ("bill.panel.count_total", "{count} \u{b7} {amount}"),
    ("bill.panel.open_bills.none", "No open bills."),
    ("bill.panel.open_bills.more", "Showing {shown} of {count}."),
    ("bill.panel.due", "due {date}"),
    ("bill.panel.credits", "Unused credits"),
    ("bill.panel.recent", "Recent bills"),
    ("bill.panel.recent.none", "No bills from this vendor yet."),
    ("bill.panel.bill", "Bill"),
];
