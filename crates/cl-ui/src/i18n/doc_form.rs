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
];
