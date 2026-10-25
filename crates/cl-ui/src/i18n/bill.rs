//! English text for Enter Bill (P17 WP4). Keys: `bill.*` (not `bill.panel.*` or `bill.read.*`).
//! Registered in `i18n.rs` by WP0; only its work package adds rows.
//! WP3 added the `bill.problem.*` rows for the cl-model checks.

/// Enter Bill's part of the English catalog.
#[rustfmt::skip]
pub const ENGLISH: &[(&str, &str)] = &[
    // WP3: cl-model `forms::bill::BillProblem`.
    ("bill.problem.vendor_missing", "Choose the vendor this bill is from."),
    ("bill.problem.date_missing", "Enter the bill date."),
    ("bill.problem.closed_needs_update", "This date is in a closed period. Saving a bill in a closed period needs a books update."),
    ("bill.problem.ref_too_long", "The Ref. no. is too long. Use 100 characters or fewer."),
    ("bill.problem.terms_unknown", "Choose payment terms from the list."),
    ("bill.problem.terms_days_invalid", "Enter how many days the custom terms give, from 0 to 999."),
    ("bill.problem.discount_invalid", "An early-payment discount needs both a percentage and a number of days, and the days must be fewer than the days until the bill is due (at most 365)."),
    ("bill.problem.discount_needs_update", "Early-payment discounts need a books update."),
    ("bill.problem.due_before_date", "The due date is before the bill date. Change one of them."),
    ("bill.problem.discount_date_invalid", "The discount date must be between the bill date and the due date."),
    ("bill.problem.received_needs_update", "Marking a bill as not received yet needs a books update. Tick Bill received."),
    ("bill.problem.amount_missing", "Enter the amount due, or enter the lines."),
    ("bill.problem.amount_not_positive", "The amount due must be more than zero."),
    ("bill.problem.no_lines", "Enter at least one line on the Expenses or Items tab."),
    ("bill.problem.too_many_lines", "A bill can have up to 500 lines. Split it into more than one bill."),
    ("bill.problem.lines_total_too_large", "The lines add up to more than the books can hold. Check the amounts."),
    ("bill.problem.lines_disagree", "The lines don't add up to the amount due. Make the lines total equal the amount due."),
    ("bill.problem.line_account_missing", "Choose an account for each line."),
    ("bill.problem.line_amount_not_positive", "Enter an amount more than zero on each line."),
    ("bill.problem.billable_needs_customer", "A billable line needs a customer. Choose one, or untick Billable."),
    ("bill.problem.line_detail_needs_update", "Customers, Billable and tags on these lines need a books update. Clear them for now."),
    ("bill.problem.items_need_update", "Item lines need a books update. Enter the amounts on the Expenses tab for now."),
    ("bill.problem.item_rate_needs_qty", "Enter a quantity for this cost, or clear the cost and enter the amount."),
    ("bill.problem.item_qty_not_positive", "The quantity must be more than zero."),
    ("bill.problem.item_rate_negative", "The cost can't be below zero."),
    ("bill.problem.item_amount_too_large", "Quantity times cost is too large. Check both."),
    ("bill.problem.read_not_accepted", "Some values were read from the document. Check them, then choose Accept these values."),
];
