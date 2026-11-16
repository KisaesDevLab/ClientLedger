//! English text for sensitive accounts (P29, U-33): the Chart of Accounts
//! switch, the notice on reports, and the refusals a user without the
//! "See sensitive accounts" right reads.

/// The sensitive-accounts part of the English catalog.
#[rustfmt::skip]
pub const ENGLISH: &[(&str, &str)] = &[
    ("error.api.sensitive_transaction", "This transaction uses an account hidden for your role, so you can't open or change it."),
    ("error.api.sensitive_report", "This report needs access to sensitive accounts."),
    ("fix.api.ask_admin", "Ask your Admin if you need it."),
    ("sensitive.notice", "Some amounts are hidden for your role, so totals may not balance."),
    ("sensitive.report_refused", "This report needs access to sensitive accounts. Ask your Admin if you need it."),
    ("sensitive.hidden_account", "Hidden account"),
    ("sensitive.txn_refused.title", "Hidden for your role"),
    ("sensitive.txn_refused", "This transaction uses an account hidden for your role, so you can't open or change it. Ask your Admin if you need it."),
    ("sensitive.register_closed", "This account is now hidden for your role, so its register was closed."),
    ("sensitive.drill_closed", "These lines use an account hidden for your role."),
    ("sensitive.not_allowed", "Bank, credit card and built-in accounts can't be marked sensitive."),
    ("sensitive.not_saved", "The sensitive mark could not be saved. Try again."),
    ("sensitive.admin_only", "Only the Admin can mark accounts sensitive."),
    ("sensitive.kept_local", "{n} accounts can't be marked sensitive (bank, card or built-in accounts)."),
    ("sensitive.kept_local.one", "1 account can't be marked sensitive (bank, card or built-in account)."),
    ("sensitive.definition_read_only", "This report uses an account hidden for your role, so you can't change it. You can still run it."),
    ("sensitive.definition_refused", "This saved report uses an account hidden for your role, so you can't change it. You can still run it."),
];
