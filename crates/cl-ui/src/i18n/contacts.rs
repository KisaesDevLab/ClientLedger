//! English text for contacts and the Client and Vendor centers (P11 steps
//! 2-3).

/// The contacts' part of the English catalog.
#[rustfmt::skip]
pub const ENGLISH: &[(&str, &str)] = &[
    // Roles, terms and form problems (cl-model `lists::contacts`)
    ("contacts.roles.client", "Client"),
    ("contacts.roles.vendor", "Vendor"),
    ("contacts.roles.both", "Client and vendor"),
    ("contacts.terms.due_on_receipt", "Due on receipt"),
    ("contacts.terms.net_15", "Net 15"),
    ("contacts.terms.net_30", "Net 30"),
    ("contacts.terms.net_60", "Net 60"),
    ("contacts.terms.net_90", "Net 90"),
    ("contacts.problem.name_missing", "Type a name. It is how this client or vendor appears on forms and reports."),
    ("contacts.problem.name_too_long", "That name is too long. Keep it under 255 characters."),
    ("contacts.problem.name_taken", "Another client or vendor already uses this name. Use a different name, or merge the two."),
    ("contacts.problem.company_too_long", "That company name is too long. Keep it under 255 characters."),
    ("contacts.problem.person_too_long", "A first or last name is too long. Keep each under 100 characters."),
    ("contacts.problem.email_invalid", "That doesn't look like an email address. Check it, or leave it blank."),
    ("contacts.problem.phone_too_long", "That phone number is too long. Keep it under 30 characters."),
    ("contacts.problem.billing_too_long", "Part of the bill-to address is too long. Shorten the street, city, state or ZIP code."),
    ("contacts.problem.shipping_too_long", "Part of the ship-to address is too long. Shorten the street, city, state or ZIP code."),
    ("contacts.problem.terms_too_long", "Those terms are too long. Pick terms from the list."),
    ("contacts.problem.notes_too_long", "The notes are too long. Keep them under 4,000 characters."),
    ("contacts.problem.expense_account_invalid", "That expense account is gone or isn't an expense account. Pick another one."),
    ("contacts.problem.vendor_account_too_long", "That account number is too long. Keep it under 50 characters."),
    ("contacts.problem.tax_id_invalid", "A tax ID has nine digits, like 12-3456789 or 123-45-6789. Check it, or leave it blank."),
];
