//! English text for the compare tools (A04 D4): highlights, click to fill,
//! drag a box, the check list. A06 reuses them. WP-C adds every key in its
//! own block below.

/// The compare tools' part of the English catalog.
#[rustfmt::skip]
pub const ENGLISH: &[(&str, &str)] = &[
    // A04 WP-C.
    ("compare.highlights", "Highlights"),
    ("compare.highlights.tip", "Show or hide where each field was read on the page"),
    ("compare.checklist", "Check list"),
    ("compare.checklist.tip", "Every field beside the part of the page it was read from"),
    ("compare.checklist.none", "Nothing was read from this document."),
    ("compare.checklist.empty_value", "(empty)"),
    ("compare.box", "{field} on the page"),
    ("compare.snippet", "Where {field} was read"),
    ("compare.click_to_fill", "Click a word to fill: {field}. Drag a box around it on a scan."),
    ("compare.level.sure", "Sure"),
    ("compare.level.check_this", "Check this"),
    ("compare.level.not_found", "Not found on the page"),
    ("compare.looks_right", "Looks right"),
    ("compare.looks_right.named", "Looks right: {field}"),
    ("compare.looks_right.tip", "You checked this value against the document"),
    ("compare.find", "Find on page"),
    ("compare.find.named", "Find on page: {field}"),
    ("compare.find.tip", "Show where this was read and go to the field"),
    ("compare.note.words_loading", "Getting the page ready…"),
    ("compare.note.no_field", "Click a field first, then the word on the page that belongs in it."),
    ("compare.note.box_too_small", "Drag a larger box around the words."),
    ("compare.note.reading_box", "Reading that part of the page…"),
    ("compare.note.box_not_read", "Couldn't read that part of the page. Type the value instead."),
    ("compare.note.box_unavailable", "That part of the page can't be read on this computer. Type the value instead."),
    ("compare.refused.no_words", "There are no words there."),
    ("compare.refused.not_one_amount", "That isn't one amount. Click the amount itself, or type it."),
    ("compare.refused.not_a_date", "That isn't a date. Click the date's words, or type it."),
    ("compare.refused.too_long", "That's more text than this field takes."),
];
