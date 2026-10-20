//! English text for the components transaction forms share (P13 WP5): the
//! lines grid's column headers and problems.

/// The shared form components' part of the English catalog.
#[rustfmt::skip]
pub const ENGLISH: &[(&str, &str)] = &[
    ("lines.label", "Lines"),
    ("lines.col.account", "Account"),
    ("lines.col.amount", "Amount"),
    ("lines.col.memo", "Memo"),
    ("lines.col.customer_job", "Customer:Job"),
    ("lines.empty", "\u{2014}"),
    ("lines.col.billable", "Billable"),
    ("lines.col.tags", "Tags"),
    ("lines.need_account", "Choose an account for each line."),
];
