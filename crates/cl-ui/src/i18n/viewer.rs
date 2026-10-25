//! English text for the document viewer and its full-size tab (P17 WP1). Keys:
//! `viewer.*`.
//! Registered in `i18n.rs` by WP0; only its work package adds rows.

/// The document viewer's part of the English catalog.
#[rustfmt::skip]
pub const ENGLISH: &[(&str, &str)] = &[
    ("viewer.page_of", "Page {page} of {pages}"),
    ("viewer.prev", "Previous page"),
    ("viewer.prev.tip", "Previous page ({keys})"),
    ("viewer.next", "Next page"),
    ("viewer.next.tip", "Next page ({keys})"),
    ("viewer.zoom_in", "Zoom in"),
    ("viewer.zoom_in.tip", "Zoom in ({keys})"),
    ("viewer.zoom_out", "Zoom out"),
    ("viewer.zoom_out.tip", "Zoom out ({keys})"),
    ("viewer.fit", "Fit"),
    ("viewer.fit.tip", "Fit the page to the width, or the whole page in view ({keys})"),
    ("viewer.zoom.fit_width", "Fit width"),
    ("viewer.zoom.fit_page", "Fit page"),
    ("viewer.zoom.percent", "{percent}%"),
    ("viewer.zoom.label", "Zoom"),
    ("viewer.full_size", "Open full size"),
    ("viewer.full_size.tip", "Open the document in its own tab"),
    ("viewer.image", "Preview of {name}"),
    ("viewer.loading", "Opening the document\u{2026}"),
    ("viewer.unavailable", "Preview isn't available on this computer"),
    ("viewer.unavailable.hint", "Save a copy to open the PDF in another program. Pictures still preview here."),
    ("viewer.protected", "This PDF is protected with a password, so it can't be previewed here."),
    ("viewer.protected.hint", "Save a copy and open it in a program that asks for the password."),
    ("viewer.unreadable", "This file can't be previewed. It may be damaged, or too large to show."),
    ("viewer.save_copy", "Save a copy"),
    ("viewer.save_copy.tip", "Save the file to your Downloads folder"),
    ("viewer.truncated", "Showing the first {n} pages of this document."),
    ("viewer.empty", "No document is open here. Use Preview on an attachment, or Open full size on a preview."),
    ("viewer.preview", "Preview"),
    ("viewer.preview.tip", "Show this file in its own tab"),
];
