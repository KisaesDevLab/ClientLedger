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
];
