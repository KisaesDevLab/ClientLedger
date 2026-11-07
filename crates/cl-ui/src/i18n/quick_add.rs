//! English text for Quick Add (Q-PR4d, QA-13).

/// Quick Add's part of the English catalog.
#[rustfmt::skip]
pub const ENGLISH: &[(&str, &str)] = &[
    ("quick_add.not_on_list", "\u{201c}{name}\u{201d} isn't on your list yet."),
    ("quick_add.add_as", "Add as"),
    ("quick_add.kind.client", "Customer"),
    ("quick_add.kind.vendor", "Vendor"),
    ("quick_add.quick_add", "Quick Add"),
    ("quick_add.set_up", "Set Up\u{2026}"),
    ("quick_add.set_up.tip", "Open the full form with this name filled in; you come back here when it's saved."),
    ("quick_add.cancel", "Cancel"),
    ("quick_add.keys", "Enter adds the name as it is \u{b7} Esc cancels"),
    ("quick_add.name_taken", "That name is already on your list (maybe inactive). Pick it from the list instead."),
    ("quick_add.failed", "The name couldn't be added. Try again, or use Set Up."),
    ("orphan.quick_add", "adding a name"),
];
