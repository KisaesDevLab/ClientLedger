//! English text for Review Documents (A04): the queue (`doc_review.*`),
//! its Home line (`home.attention.documents*`) and the attach dialog. WP-M
//! adds the keys the model names; WP-F1 (the queue) and WP-F2 (attach) add
//! theirs in their own blocks below.

/// Review Documents' part of the English catalog.
#[rustfmt::skip]
pub const ENGLISH: &[(&str, &str)] = &[
    // A04 WP-M: the queue's status words (`cl_model::doc_draft::QueueStatus`).
    ("doc_review.status.waiting", "Waiting to be read"),
    ("doc_review.status.ready", "Ready"),
    ("doc_review.status.check_fields", "Check these fields"),
    ("doc_review.status.possible_duplicate", "Possible duplicate"),
    ("doc_review.status.couldnt_read", "Couldn't read"),
    // A04 WP-M: Home › Needs attention (`AttentionKind::DocumentsToReview`).
    ("home.attention.documents", "{n} documents to review"),
    ("home.attention.documents.one", "1 document to review"),
    ("home.attention.documents_to_review.name", "Documents to review"),
    // A04 WP-M: the shortcut overlay's heading for the queue's keys.
    ("shortcuts.scope.list", "In this list"),
    // A04 WP-M: the scaffold window until WP-F1 builds the queue.
    ("doc_review.empty", "No documents waiting. Files saved into your intake folders appear here."),
    // A04 WP-F1: the queue.
    ("doc_review.kind.bill", "Bill"),
    ("doc_review.kind.card_charge", "Card charge"),
    ("doc_review.kind.check", "Check"),
    ("doc_review.kind.cash_expense", "Expense"),
    ("doc_review.kind.unknown", "Not sure yet"),
    ("doc_review.row.label", "{kind}, document {n} of {of}, {status}"),
    ("doc_review.checking", "checking for duplicates"),
    ("doc_review.count", "{n} documents waiting"),
    ("doc_review.count.one", "1 document waiting"),
    ("doc_review.filter", "Filter"),
    ("doc_review.filter.hint", "Kind, status or number"),
    ("doc_review.no_match", "No documents match the filter."),
    ("doc_review.read_all", "Read all"),
    ("doc_review.read_all.tip", "Read every document that hasn't been read yet, on this computer. Nothing goes online."),
    ("doc_review.read_all.none", "There's nothing to read on this computer right now. Documents are read where your intake folders are set up."),
    ("doc_review.reading", "Reading {done} of {total}\u{2026}"),
    ("doc_review.refresh", "Refresh"),
    ("doc_review.load_failed", "The list couldn't be loaded. It will try again in a few seconds."),
    ("doc_review.unsupported", "This company's books can't keep documents for review yet. Update ClientLedger to use Review Documents."),
    ("doc_review.keys", "J and K move through the list. Enter opens the document in its form. Delete discards it."),
    ("doc_review.page.prev", "Previous page"),
    ("doc_review.page.next", "Next page"),
    ("doc_review.page.shown", "{first}\u{2013}{last} of {total}"),
    ("doc_review.discard.ask", "Discard this document? It leaves the review list and nothing is entered. The file isn't deleted: the computer that brought it in moves it to the Discarded folder."),
    ("doc_review.discard.yes", "Discard"),
    ("doc_review.discard.no", "Keep it"),
    ("doc_review.discard.closed", "This document was already entered, so it can't be discarded."),
    ("doc_review.discard.failed", "The document couldn't be discarded. Try again."),
    ("doc_review.orphan.discard", "Discarding a document"),
    ("doc_review.choose.title", "What is this document?"),
    ("doc_review.choose.hint", "Choose the form to enter it on."),
    ("doc_review.choose.bill", "A bill to pay later"),
    ("doc_review.choose.card_charge", "A card charge"),
    ("doc_review.choose.check", "A check already written"),
    ("doc_review.choose.cash_expense", "An expense paid from a bank or cash account"),
    ("doc_review.choose.cancel", "Cancel"),
];
