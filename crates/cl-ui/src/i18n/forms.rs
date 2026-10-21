//! English text for the components transaction forms share (P13 WP5): the
//! lines grid's column headers and problems.

/// The shared form components' part of the English catalog.
#[rustfmt::skip]
pub const ENGLISH: &[(&str, &str)] = &[
    ("lines.label", "Lines"),
    ("lines.col.account", "Account"),
    ("lines.col.amount", "Amount"),
    ("lines.col.memo", "Memo"),
    ("lines.col.customer_job", "Customer / Job"),
    ("lines.empty", "\u{2014}"),
    ("lines.col.billable", "Billable"),
    ("lines.col.tags", "Tags"),
    ("lines.need_account", "Choose an account for each line."),
    // Save as Template (P15): the toolbar slot every template form shares.
    ("template.save.tip", "Keep this form as a template to reuse or schedule it ({keys})."),
    ("template.save.unavailable", "Templates need an engine update."),
    ("template.save.grouped", "A deposit that includes customer payments can't be a template; remove the payments or enter it each time."),
    // Plain one-sentence explanations of accounting words (CLAUDE.md rule 12).
    // The owner-facing name of the engine's Payments Clearing account (Q-202).
    ("account.undeposited_funds", "Undeposited Funds"),
    ("jargon.undeposited_funds", "Undeposited Funds holds payments you've received but not yet taken to the bank."),
    ("jargon.cash_over_short", "Cash over/short is the difference between the cash counted in the drawer and what the day's sales say should be there."),
    ("jargon.equity", "Equity is the owners' share of the business: what they put in and take out, and the profit kept in the business."),
    ("jargon.principal", "Principal is the part of a loan payment that pays down what you still owe."),
    ("jargon.interest", "Interest is what the lender charges for the loan; it is a business expense."),
    ("jargon.owner_money", "An owner's draw is money an owner takes out of the business for personal use; an owner's contribution is money an owner puts in."),
    ("jargon.partner_money", "A partner draw is money a partner takes out of the business; a partner contribution is money a partner puts in."),
    ("jargon.shareholder_money", "A shareholder distribution is a payment of the corporation's profit to its shareholders; paid-in capital is money shareholders put in."),
];
