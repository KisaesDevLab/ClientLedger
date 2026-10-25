//! English text for Match Vendor Statement (P17 WP6). Keys: `statement.*`.
//! Registered in `i18n.rs` by WP0; only its work package adds rows.
//! WP3 added the `statement.paste.*` rows for the cl-model paste reader.

/// The statement match's part of the English catalog.
#[rustfmt::skip]
pub const ENGLISH: &[(&str, &str)] = &[
    // WP3: cl-model `statement_match::PasteProblem`.
    ("statement.paste.columns", "Each line needs a date, a ref. no. and an amount, and can add a type."),
    ("statement.paste.date", "The date isn't one we can read. Use 10/31/2026 or 2026-10-31."),
    ("statement.paste.amount", "The amount isn't one we can read. Use 1,330.00, -75.00 or (75.00)."),
    ("statement.paste.zero", "The amount is zero. Leave this line out."),
    ("statement.paste.kind", "The type must be invoice, credit or payment, or left blank."),
];
