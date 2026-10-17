//! Every user-visible string goes through here (CLAUDE.md coding standards).
//!
//! English only for now; the point is that a later locale replaces one table
//! instead of hunting through widgets. Keys are dotted (`shell.rail.workspace`).
//! Command labels come from the `cl-model` command table (`cmd.<id>` resolves
//! to the registered English label), and owner-facing error keys from `cl-core`
//! and `cl-api` (`error.*`, `fix.*`) live in this catalog.
//!
//! An unknown key returns the key itself so a missing string is visible, never
//! a panic; the `every_key_used_in_the_source_is_in_the_catalog` test makes it
//! a build failure instead.

use std::collections::HashMap;
use std::sync::OnceLock;

use cl_model::commands;

/// Translate `key`. `cmd.<command id>` returns the command's label.
#[must_use]
pub fn t(key: &str) -> String {
    if let Some(id) = key.strip_prefix("cmd.")
        && let Some(c) = commands::find(id)
    {
        return c.label.to_string();
    }
    match catalog().get(key) {
        Some(text) => (*text).to_string(),
        None => key.to_string(),
    }
}

/// Translate `key` and replace each `{name}` with its value.
#[must_use]
pub fn t_with(key: &str, args: &[(&str, &str)]) -> String {
    let mut text = t(key);
    for (name, value) in args {
        text = text.replace(&format!("{{{name}}}"), value);
    }
    text
}

/// True when `key` has an entry (used by tests and by error presentation).
#[must_use]
pub fn has(key: &str) -> bool {
    key.strip_prefix("cmd.")
        .is_some_and(|id| commands::find(id).is_some())
        || catalog().contains_key(key)
}

fn catalog() -> &'static HashMap<&'static str, &'static str> {
    static CATALOG: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    CATALOG.get_or_init(|| ENGLISH.iter().copied().collect())
}

/// The English catalog.
pub const ENGLISH: &[(&str, &str)] = &[
    // App
    ("app.name", "ClientLedger"),
    ("app.tagline", "powered by Vibe MyBooks"),
    // Theme
    ("theme.light", "Light"),
    ("theme.dark", "Dark"),
    ("theme.auto", "Automatic"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_keys_come_back_as_the_key() {
        assert_eq!(t("no.such.key"), "no.such.key");
    }

    #[test]
    fn command_labels_resolve() {
        assert_eq!(t("cmd.vendors.write_check"), "Write Check");
        assert!(has("cmd.help.about"));
        assert!(!has("cmd.not.a.command"));
    }

    #[test]
    fn placeholders_are_replaced() {
        assert_eq!(
            t_with("no.such.{x}", &[("x", "y")]),
            "no.such.{x}".replace("{x}", "y")
        );
    }

    #[test]
    fn catalog_has_no_duplicate_keys_and_no_empty_text() {
        let mut seen = std::collections::HashSet::new();
        for (k, v) in ENGLISH {
            assert!(seen.insert(*k), "duplicate key {k}");
            assert_ne!(*v, "", "{k}");
        }
    }
}
