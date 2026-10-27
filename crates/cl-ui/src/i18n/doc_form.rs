//! English text for documents entered in the real forms (A04 D2, D3, D5):
//! draft mode in Enter Bill, Record Card Charge and Write Check, Accept,
//! attach to an existing transaction, learning. WP-M adds the keys the
//! model names; WP-F2 adds the rest in its own blocks below.

/// Draft mode's part of the English catalog.
#[rustfmt::skip]
pub const ENGLISH: &[(&str, &str)] = &[
    // A04 WP-M: Save is blocked until read values are accepted (card and
    // check forms in draft mode; `ReadNotAccepted`).
    ("card.problem.read_not_accepted", "Some values were read from the document and aren't accepted yet. Choose Accept (Alt+A) or change them."),
    ("check.problem.read_not_accepted", "Some values were read from the document and aren't accepted yet. Choose Accept (Alt+A) or change them."),
    // A04 WP-F2: the draft banner.
    ("doc_form.loading", "Getting the document\u{2026}"),
    ("doc_form.load.failed", "Couldn't open this document. Close the window and open it again from Review Documents."),
    ("doc_form.load.gone", "This document isn't waiting any more. It may have been entered or discarded on another computer."),
    ("doc_form.load.waiting", "The document is still opening. Try again in a moment."),
    ("doc_form.banner.file", "Filled from {file} in your receipt inbox. Check each value against the document, then save."),
    ("doc_form.banner.layout", "Filled from {vendor}'s saved layout. Check each value against the document, then save."),
    ("doc_form.banner.unread", "This document hasn't been read yet. Read it, or enter it by hand."),
    ("doc_form.banner.unreadable", "Couldn't read this document, enter it by hand."),
    ("doc_form.banner.unaccepted", "{n} values were read from the document. Check them, then Accept (Alt+A)."),
    ("doc_form.remember_card", "Remember card ending {digits}"),
    ("doc_form.read.unavailable", "Reading documents isn't available on this computer. Enter it by hand."),
    ("doc_form.read.failed", "Couldn't read this document, enter it by hand."),
    ("doc_form.matching", "Looking for this purchase in your books\u{2026}"),
    ("doc_form.entering", "Entering the document\u{2026}"),
    ("doc_form.try_again", "Try again"),
    ("doc_form.unknown", "We couldn't tell whether the document was entered. Choose Try again: it won't be entered twice."),
    ("doc_form.forbidden", "You don't have permission to enter this kind of document."),
    ("doc_form.already_done", "This document has already been entered or discarded."),
    ("doc_form.discarded_elsewhere", "This document was discarded on another computer. Nothing was entered."),
    ("doc_form.queue.end", "There are no more documents that way."),
    // A04 WP-F2: Discard document.
    ("doc_form.discard", "Discard document\u{2026}"),
    ("doc_form.discard.tip", "Take this document out of Review Documents without entering it. The file moves to the Discarded folder."),
    ("doc_form.discard.title", "Discard this document?"),
    ("doc_form.discard.body", "It won't be entered. The file moves to the Discarded folder in your intake folder; nothing is deleted from your computer."),
    ("doc_form.discard.yes", "Discard document"),
    ("doc_form.discard.failed", "Couldn't discard the document. Try again."),
    ("doc_form.discarded", "Document discarded"),
    // A04 WP-F2: questions over the form.
    ("doc_form.stale.title", "This document changed"),
    ("doc_form.stale.body", "This document was changed on another computer since you opened it. Nothing was entered. Keep your values and enter it, or reload what was saved there."),
    ("doc_form.stale.keep", "Keep my values and enter"),
    ("doc_form.stale.reload", "Reload"),
    ("doc_form.entered.title", "Already entered"),
    ("doc_form.entered.body", "This document was already entered, on this or another computer. Nothing more was entered."),
    ("doc_form.entered.open", "Open it"),
    ("doc_form.discarded_elsewhere.title", "Discarded on another computer"),
    ("doc_form.leave.title", "Leave this document?"),
    ("doc_form.leave.body", "Your changes to this document aren't saved. The document stays in Review Documents."),
    ("doc_form.leave.yes", "Discard my changes"),
    ("doc_form.close", "Close"),
    ("doc_form.ok", "OK"),
    ("doc_form.cancel", "Cancel"),
    // A04 WP-F2: "What is this document?" (Q-A04n) and "Enter this as".
    ("doc_form.chooser.title", "What is this document?"),
    ("doc_form.as.bill", "A bill to pay"),
    ("doc_form.as.card_charge", "A card charge"),
    ("doc_form.as.cash_expense", "An expense paid from a bank or cash account"),
    ("doc_form.as.check", "A check I wrote"),
    ("doc_form.expense_paid_from", "Expense paid from"),
    ("doc_form.expense.title", "Record Expense"),
    ("doc_form.line_amount", "Line amount"),
    ("doc_form.bill_only", "A document from Review Documents is entered as a bill here. To enter a vendor credit, discard it and enter the credit by hand."),
    // A04 WP-F2: writes the request queue may finish after the window
    // closed (the unconfirmed-writes list names them).
    ("orphan.draft_convert", "entering a document"),
    ("orphan.draft_attach", "attaching a document"),
    ("orphan.draft_discard", "discarding a document"),
    ("orphan.layout_learn", "saving a vendor's document layout"),
    ("orphan.card_digits", "saving a card's last four digits"),
    // A04 WP-kind-ai: "Enter this as" (Q-A04n) and Ask AI to help in draft
    // mode (Q-A03j).
    ("doc_form.enter_as", "Enter this as:"),
    ("doc_form.enter_as.tip", "Opens this document in that form instead. Values you accepted or changed go with it; the rest still need Accept."),
    ("doc_form.assist.waiting", "Available once the page has been read."),
    // A04 fix pass: a failed register search never posts (review B2); card
    // credit in draft mode (L4).
    ("doc_form.match.failed", "We couldn't look for this purchase in your books, so nothing was entered. Choose Try again. If you're sure it isn't entered yet, choose Enter as new."),
    ("doc_form.match.enter_new", "Enter as new"),
    ("doc_form.match.enter_new.tip", "Enter this document as a new expense without looking for it in your books first."),
    ("doc_form.match.back", "Back to the form"),
    // A04 fix pass: UI check item 10 (a sure value's caption).
    ("bill.read.mark.sure", "Read from the document"),
    // A04 fix pass: UI check item 11 (a draft check's tab).
    ("check.title.document", "Write Check \u{b7} from Review Documents"),
    // A04 fix pass: UI check item 3 (the reading banner's source).
    ("doc_form.banner.layout_name", "{vendor}'s saved layout"),
    // A04 fix pass: UI check item 12.
    ("doc_form.banner.unaccepted.one", "1 value was read from the document. Check it, then Accept (Alt+A)."),
    ("doc_form.credit_unavailable", "A document from Review Documents is entered as a charge here. To enter a refund or credit, discard it and enter the credit by hand."),
];
