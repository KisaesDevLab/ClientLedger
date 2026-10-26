//! English text for the native Browse dialogs (A01 WP-D, Q-A01m, Q-P13k):
//! `picker.*`, the Document intake Browse button and the attachments'
//! Browse button.

/// The Browse dialogs' part of the English catalog.
#[rustfmt::skip]
pub const ENGLISH: &[(&str, &str)] = &[
    ("picker.title.intake_folder", "Choose an intake folder"),
    ("picker.title.attachment", "Choose a file to attach"),
    ("picker.filter.documents", "PDF and images (PDF, JPEG, PNG, TIFF)"),
    ("picker.browse", "Browse\u{2026}"),
    ("picker.waiting", "Waiting for the Browse window\u{2026}"),
    ("prefs.intake.browse.tip", "Choose the folder in a window instead of typing its path"),
    ("prefs.intake.a11y.browse_new", "Browse for the new folder"),
    ("check.attach.browse.tip", "Choose a PDF or image file to attach"),
];
