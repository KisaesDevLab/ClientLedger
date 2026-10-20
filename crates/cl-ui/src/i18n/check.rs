//! English text for Write Check (P13). WP3 adds the save problems from
//! `cl_model::forms::check`; the screen's own text follows in WP6.

/// Write Check's part of the English catalog.
#[rustfmt::skip]
pub const ENGLISH: &[(&str, &str)] = &[
    ("check.problem.bank_missing", "Choose the bank account this check is written on."),
    ("check.problem.date_missing", "Enter the date of the check."),
    ("check.problem.payee_missing", "Choose who the check is to in Pay to the order of."),
    ("check.problem.amount_missing", "Enter the check amount."),
    ("check.problem.amount_not_positive", "The check amount must be more than zero."),
    ("check.problem.lines_disagree", "The lines don't add up to the check amount. Make the lines total equal the check amount."),
    ("check.problem.line_account_missing", "Choose an account for each line."),
    ("check.problem.line_amount_not_positive", "Enter an amount more than zero on each line."),
    ("check.problem.billable_needs_customer", "A billable line needs a customer. Choose one, or untick Billable."),
    ("check.problem.number_invalid", "Use a whole number from 1 to 2,147,483,647 for the check number, or leave it blank."),
    ("check.problem.number_with_print_later", "A check marked Print later gets its number when it is printed. Clear the number or untick Print later."),
    ("check.problem.address_too_long", "The address is too long. Use 500 characters or fewer."),
    ("check.problem.amount_too_large", "The check amount is too large. Checks can be up to 999,999,999,999.99."),
    ("check.problem.too_many_lines", "A check can have up to 500 lines. Split it into more than one check."),
];
