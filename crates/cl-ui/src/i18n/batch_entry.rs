//! English text for Bulk Entry (P15A). Lane D adds the screen's keys here.

/// Bulk Entry's part of the English catalog.
#[rustfmt::skip]
pub const ENGLISH: &[(&str, &str)] = &[
    ("batch_entry.intro", "Enter many checks, expenses, card charges, deposits and journal entries in one grid, then save them all at once."),
    ("batch_entry.unavailable", "Bulk Entry needs an engine update."),
    // Section types (Q-P15Ab).
    ("batch_entry.kind.checks", "Checks"),
    ("batch_entry.kind.expenses", "Expenses"),
    ("batch_entry.kind.card_charges", "Card charges"),
    ("batch_entry.kind.card_credits", "Card credits"),
    ("batch_entry.kind.deposits", "Deposits"),
    ("batch_entry.kind.journal_entries", "Journal entries"),
];
