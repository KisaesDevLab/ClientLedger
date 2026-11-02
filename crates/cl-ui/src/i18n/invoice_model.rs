//! English text for the Create Invoice form model (P20 WP-A): the problems
//! `cl_model::forms::invoice` names, where a rate came from, and the default
//! email template (`cl_settings::email_templates`). Keys: `invoice.problem.*`,
//! `invoice.rate_source.*`, `email.template.*`. WP-B's screen text lives in
//! its own module.

/// The invoice model's part of the English catalog.
#[rustfmt::skip]
pub const ENGLISH: &[(&str, &str)] = &[
    ("invoice.problem.client_missing", "Choose a customer for this invoice."),
    ("invoice.problem.date_missing", "Enter the invoice date."),
    ("invoice.problem.closed_needs_update", "This date is in a closed period. Saving it needs the closing-date password, which this version of the books engine can't take for invoices yet. Choose a later date."),
    ("invoice.problem.number_too_long", "The invoice number can be at most 50 characters."),
    ("invoice.problem.po_too_long", "The P.O. number can be at most 50 characters."),
    ("invoice.problem.message_too_long", "The customer message can be at most 4,000 characters."),
    ("invoice.problem.terms_days_invalid", "Custom terms need a number of days from 0 to 3,650."),
    ("invoice.problem.due_before_date", "The due date can't be before the invoice date."),
    ("invoice.problem.fields_need_update", "Items, service dates, the P.O. number, the customer message and tags need an update of the books engine. Remove them or update ClientLedger."),
    ("invoice.problem.no_lines", "Add at least one line to the invoice."),
    ("invoice.problem.too_many_lines", "An invoice can have at most 500 lines. Split it into two invoices."),
    ("invoice.problem.line_item_missing", "Choose an item or an income account for this line."),
    ("invoice.problem.line_description_too_long", "A line description can be at most 4,000 characters."),
    ("invoice.problem.line_rate_missing", "Enter a rate for this line."),
    ("invoice.problem.billable_needs_update", "Billable costs need an update of the books engine. Remove this line or update ClientLedger."),
    ("invoice.problem.markup_negative", "The markup must be a percent from 0 to 1,000."),
    ("invoice.problem.billable_account_missing", "Choose the income account this billable cost is billed to."),
    ("invoice.rate_source.typed", "Rate you entered"),
    ("invoice.rate_source.item", "Rate from the item"),
    ("invoice.rate_source.price_level", "Price the customer's price level sets for this item"),
    ("invoice.rate_source.price_level_percent", "Rate from the customer's price level (a percent of the item's price)"),
    ("invoice.rate_source.unpriced", "Subtotal of the lines above"),
    ("invoice.rate_source.billable", "Billable cost, with any markup"),
    ("invoice.rate_source.other", "Rate from your books"),
    // cl-print form labels added in P20 (the print screens read them).
    ("print.pdf.ship_to", "Ship to"),
    ("print.pdf.service_date", "Service date"),
    // Default invoice email (cl_settings::email_templates; Preferences > Email edits a copy).
    ("email.template.invoice.subject", "Invoice {invoice_number} from {company_name}"),
    ("email.template.invoice.body", "Dear {client_name},\n\nYour invoice {invoice_number} for {total} is attached. It is due {due_date}.\n\nThank you for your business.\n{company_name}"),
    ("email.template.credit_memo.subject", "Credit memo {number} from {company_name}"),
    ("email.template.credit_memo.body", "Dear {client_name},\n\nYour credit memo {number} dated {date} for {total} is attached.\n\nThank you for your business.\n{company_name}"),
    ("email.template.sales_receipt.subject", "Sales receipt {number} from {company_name}"),
    ("email.template.sales_receipt.body", "Dear {client_name},\n\nYour sales receipt {number} dated {date} for {total} is attached.\n\nThank you for your business.\n{company_name}"),
    ("email.template.statement.subject", "Statement {statement_date} from {company_name}"),
    ("email.template.statement.body", "Dear {client_name},\n\nYour statement as of {statement_date} is attached. The amount due is {amount_due}.\n\nThank you for your business.\n{company_name}"),
];

#[cfg(test)]
mod tests {
    use crate::i18n::{has, t};
    use cl_settings::email_templates::{DEFAULT_INVOICE_BODY_KEY, DEFAULT_INVOICE_SUBJECT_KEY};

    #[test]
    fn the_default_email_texts_exist_and_use_known_fields_only() {
        for key in [DEFAULT_INVOICE_SUBJECT_KEY, DEFAULT_INVOICE_BODY_KEY] {
            assert!(has(key), "{key}");
            assert_eq!(cl_settings::unknown_fields(&t(key)), Vec::<String>::new());
        }
    }
}
