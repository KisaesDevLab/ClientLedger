//! English text for the register (P12). The keys named by `cl-model`'s
//! register rules live here; the register screens add theirs below them.

/// The register's part of the English catalog.
#[rustfmt::skip]
pub const ENGLISH: &[(&str, &str)] = &[
    // Show range (cl-model register::RangePreset)
    ("register.range.last_90_days", "Last 90 days"),
    ("register.range.this_month", "This month"),
    ("register.range.last_month", "Last month"),
    ("register.range.this_quarter", "This quarter"),
    ("register.range.this_year", "This year"),
    ("register.range.last_year", "Last year"),
    ("register.range.all_dates", "All dates"),
    ("register.range.custom", "Custom dates\u{2026}"),
    ("register.range.problem.from_after_to", "The From date is after the To date. Change one of them."),
    // Sort by
    ("register.sort.date_oldest", "Date, oldest first"),
    ("register.sort.date_newest", "Date, newest first"),
    ("register.sort.number", "Number"),
    ("register.sort.amount", "Amount"),
    ("register.sort.type", "Type"),
    // Filters and balances
    ("register.filter.problem.min_above_max", "The lowest amount is more than the highest amount. Change one of them."),
    ("register.balance.hidden_filtered", "Balances show when no filter is on"),
    // Amount columns by kind of account
    ("register.col.payment", "Payment"),
    ("register.col.deposit", "Deposit"),
    ("register.col.charge", "Charge"),
    ("register.col.card_payment", "Payment"),
    ("register.col.increase", "Increase"),
    ("register.col.decrease", "Decrease"),
    // What an entry-row line becomes
    ("register.kind.check", "Check"),
    ("register.kind.deposit", "Deposit"),
    ("register.kind.transfer", "Transfer"),
    ("register.kind.card_charge", "Card charge"),
    ("register.kind.journal_entry", "Journal entry"),
    // Entry-row problems
    ("register.entry.problem.date_missing", "Enter a date."),
    ("register.entry.problem.amount_missing", "Enter an amount in one of the two amount columns."),
    ("register.entry.problem.both_sides", "Enter an amount in only one of the two amount columns."),
    ("register.entry.problem.amount_not_positive", "Enter an amount more than zero."),
    ("register.entry.problem.account_missing", "Choose an account."),
    ("register.entry.problem.account_gone", "That account is no longer in your chart of accounts. Choose another account."),
    ("register.entry.problem.same_account", "Choose a different account from the one this register is for."),
    ("register.entry.problem.name_required", "Choose a customer or vendor name. Lines on accounts receivable or accounts payable need one."),
    ("register.entry.problem.two_named_accounts", "Accounts receivable and accounts payable can't both be on one entry. Use one of them."),
    ("register.entry.problem.too_many_tags", "A line can have only one tag for now. Keep one tag on this line."),
    ("register.entry.problem.kind_not_allowed", "This kind of transaction can't be entered in this register. Use its own form instead."),
];
