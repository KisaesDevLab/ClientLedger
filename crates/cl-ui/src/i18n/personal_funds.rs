//! English text for Paid with Personal Funds (P14 lane C).

/// Paid with Personal Funds's part of the English catalog.
#[rustfmt::skip]
pub const ENGLISH: &[(&str, &str)] = &[
    ("personal_funds.intro", "You paid a business expense with your own money."),
    ("personal_funds.who", "Who paid"),
    ("personal_funds.payee", "Paid to (optional)"),
    ("personal_funds.how", "How should we record it?"),
    ("personal_funds.put_in", "Money I put into the business"),
    ("personal_funds.pay_back", "The business will pay me back"),
    ("personal_funds.offset", "Recorded in"),
    ("personal_funds.quick_add", "Add \u{201c}Due to owner\u{201d}"),
    ("personal_funds.due_to_owner", "Due to owner"),
    ("personal_funds.default.sole_prop", "Default for a sole proprietor"),
    ("personal_funds.default.smllc", "Default for a single-member LLC"),
    ("personal_funds.default.partnership", "Default for a partnership"),
    ("personal_funds.default.s_corp", "Default for an S corporation"),
    ("personal_funds.default.c_corp", "Default for a C corporation"),
    ("personal_funds.default.unknown", "Default"),
    ("personal_funds.lines.label", "What it was for"),
    ("personal_funds.lines.heading", "What it was for {total}"),
    ("personal_funds.panel.title", "About this"),
    ("personal_funds.panel.explain", "Money you put in becomes your contribution to the business. Money the business will pay back is kept as an amount it owes you until it is repaid."),
    ("personal_funds.problem.offset_missing", "Choose the account in Recorded in."),
    ("orphan.helper_account", "adding an account"),
];
