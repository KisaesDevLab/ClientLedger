//! English text for list import and export, terms and payment methods
//! (P11 step 6).

/// The CSV, terms and payment methods part of the English catalog.
#[rustfmt::skip]
pub const ENGLISH: &[(&str, &str)] = &[
    ("csv.list.accounts", "Chart of Accounts"),
    ("csv.list.contacts", "Clients and vendors"),
    ("csv.list.items", "Items"),
    ("csv.list.tags", "Tags"),
    ("csv.field.account_number", "Account number"),
    ("csv.field.name", "Name"),
    ("csv.field.account_type", "Account type"),
    ("csv.field.detail_type", "Detail type"),
    ("csv.field.company", "Company"),
    ("csv.field.email", "Email"),
    ("csv.field.phone", "Phone"),
    ("csv.field.description", "Description"),
    ("csv.field.price", "Sales price"),
    ("csv.field.income_account", "Income account"),
    ("csv.field.taxable", "Taxable"),
    ("csv.field.skip", "Don't import"),
    ("csv.problem.missing", "Choose the column that holds {field}."),
    ("csv.problem.twice", "Two columns are set to {field}. Set one of them to \u{201C}Don't import\u{201D}."),
    ("csv.problem.too_many_rows", "One import takes up to {max} rows. Split the file into smaller files."),
    ("csv.problem.no_rows", "The file has no rows below the header."),
    ("csv.row.name_missing", "The name is blank."),
    ("csv.row.too_long", "{field} is too long."),
    ("csv.row.type_unknown", "The account type isn't one ClientLedger knows. Use Asset, Liability, Equity, Income, Cost of goods sold, Expense, Other income or Other expense."),
    ("csv.row.price_invalid", "The price isn't an amount ClientLedger can read (up to four decimals)."),
    ("csv.row.price_negative", "The price is below zero."),
    ("csv.row.income_unknown", "The income account isn't in your Chart of Accounts."),
    ("csv.row.income_missing", "No income account. Choose one for rows that don't name it."),
    ("csv.row.email_invalid", "The email address isn't valid."),
    ("csv.row.taxable_invalid", "Taxable must be Yes or No."),
    ("csv.row.name_taken", "An item already has this name (perhaps an inactive one, or an earlier row)."),
];
