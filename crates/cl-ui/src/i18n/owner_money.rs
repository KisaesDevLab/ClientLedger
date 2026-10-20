//! English text for Pay Yourself or Put Money In (P14 lane C). Neutral
//! wording only: no tax advice (Q-61).

/// Pay Yourself or Put Money In's part of the English catalog.
#[rustfmt::skip]
pub const ENGLISH: &[(&str, &str)] = &[
    ("owner_money.pay_yourself", "Pay yourself"),
    ("owner_money.put_in", "Put money in"),
    ("owner_money.who", "Who"),
    ("owner_money.bank", "Bank account"),
    ("owner_money.paid_by", "Paid by"),
    ("owner_money.paid_by.check", "Check"),
    ("owner_money.paid_by.transfer", "Transfer or online"),
    ("owner_money.check_no", "Check No."),
    ("owner_money.recorded_in", "Recorded in"),
    ("owner_money.recorded_line", "Recorded in: {account}"),
    ("owner_money.source.engine", "Set up for your type of business. You can change it."),
    ("owner_money.source.remembered", "Your earlier choice for this company. You can change it."),
    ("owner_money.panel.title", "About this"),
    ("owner_money.bank_balance", "Bank balance"),
    ("owner_money.note.owner", "For a sole proprietor or single-member LLC, money you take out is recorded as an owner draw, and money you put in as an owner contribution."),
    ("owner_money.note.partnership", "For a partnership, money a partner takes out is recorded as a partner draw, and money a partner puts in as a partner contribution."),
    ("owner_money.note.s_corp", "For an S corporation, money paid to shareholders is recorded as a shareholder distribution, and money put in as paid-in capital."),
    ("owner_money.note.c_corp", "For a C corporation, money paid to shareholders is recorded as dividends paid, and money put in as paid-in capital."),
    ("owner_money.note.unknown", "Choose the equity account this is recorded in. ClientLedger remembers your choice for this company."),
    ("owner_money.ask_accountant", "Ask your accountant if you're not sure which to use."),
    ("owner_money.problem.who_missing", "Choose who the check is to."),
    ("owner_money.problem.equity_missing", "Choose the equity account in Recorded in."),
    ("owner_money.problem.same_account", "Recorded in can't be the bank account. Choose an equity account."),
];
