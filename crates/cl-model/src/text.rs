//! i18n keys for owner-facing text produced below the UI.
//!
//! cl-model and cl-grid cannot depend on `cl-ui::i18n`, so they name text by
//! key and the UI resolves it. Keys are written as `TextKey("…")` literals so
//! the cl-ui i18n coverage test can find every one in the source (P10 plan
//! F13, step 12).

/// The key of one catalog string (`grid.calc.not_exact`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TextKey(pub &'static str);

impl TextKey {
    /// The key text.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.0
    }
}
