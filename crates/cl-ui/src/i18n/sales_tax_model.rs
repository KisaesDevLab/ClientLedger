//! English text for the P23 sales tax form models
//! (`cl_model::forms::sales_tax`): editor, Pay Sales Tax, link, exemption
//! and recovery problems, and the exemption strip. The screens' own text
//! arrives with them (WP-2, WP-3).

/// The P23 models' part of the English catalog.
#[rustfmt::skip]
pub const ENGLISH: &[(&str, &str)] = &[
    ("sales_tax.problem.name_required", "Enter a name."),
    ("sales_tax.problem.name_too_long", "Use a name of 100 characters or fewer."),
    ("sales_tax.problem.bad_rate", "Enter the rate as a percent below 100, with up to 4 decimals (for example 8.875)."),
    ("sales_tax.problem.agency_required", "Choose the agency this tax is paid to."),
    ("sales_tax.problem.group_empty", "Add at least one sales tax item to the group."),
    ("sales_tax.problem.group_too_large", "A group can hold up to 10 sales tax items."),
    ("sales_tax.problem.code_required", "Enter a code."),
    ("sales_tax.problem.code_too_long", "Use a code of 8 characters or fewer."),
    ("pay_sales_tax.problem.no_agency", "Choose the agency you are paying."),
    ("pay_sales_tax.problem.no_date", "Enter the payment date and the date to pay tax through."),
    ("pay_sales_tax.problem.no_bank", "Choose the account you are paying from."),
    ("pay_sales_tax.problem.bad_amount", "Enter an amount of zero or more, to the cent."),
    ("pay_sales_tax.problem.bad_adjustment", "Each adjustment needs a tax item, an account, an amount other than zero and a reason."),
    ("pay_sales_tax.problem.check_number", "Enter a check number or tick Print later, not both."),
    ("pay_sales_tax.problem.bad_check_number", "Enter the check number as a whole number, or leave it blank to print the check later."),
    ("pay_sales_tax.problem.updating", "The amounts are updating. Try again in a moment."),
    ("pay_sales_tax.problem.nothing_to_pay", "There is nothing to pay. The total must be more than zero."),
    ("pay_sales_tax.problem.more_than_owed", "You're paying more than is owed on a tax. Confirm that's what you mean before paying."),
    ("pay_sales_tax.link.no_payment", "Choose the payment you already made."),
    ("pay_sales_tax.link.no_lines", "Enter how much of the payment was for each tax item."),
    ("tax_exemptions.problem.no_client", "Choose the customer."),
    ("tax_exemptions.problem.number_required", "Enter the certificate number."),
    ("tax_exemptions.problem.number_too_long", "Use a certificate number of 50 characters or fewer."),
    ("tax_exemptions.problem.bad_state", "Enter the state as two letters (for example MO)."),
    ("tax_exemptions.problem.note_too_long", "Use a note of 1,000 characters or fewer."),
    ("tax_exemptions.strip.missing", "This customer doesn't pay sales tax, but no exemption certificate is on file."),
    ("tax_exemptions.strip.expired", "This customer's exemption certificate has expired. Ask for a new one."),
    ("tax_exemptions.strip.expiring", "This customer's exemption certificate expires within 30 days."),
    ("recover_bad_debt.problem.no_invoice", "Choose the written-off invoice."),
    ("recover_bad_debt.problem.no_date", "Enter the date."),
    ("recover_bad_debt.problem.bad_amount", "Enter an amount more than zero, to the cent."),
];
