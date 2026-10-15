//! `cl-settings` crate (stub; filled in by a later phase).
#![forbid(unsafe_code)]

/// Returns the crate name; placeholder so the skeleton has a testable item.
#[must_use]
pub fn crate_name() -> &'static str {
    "cl-settings"
}

#[cfg(test)]
mod tests {
    #[test]
    fn crate_name_matches() {
        assert_eq!(super::crate_name(), "cl-settings");
    }
}
