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
#[rustfmt::skip]
pub const ENGLISH: &[(&str, &str)] = &[
    // App
    ("app.name", "ClientLedger"),
    ("app.tagline", "powered by Vibe MyBooks"),
    // Theme
    ("theme.light", "Light"),
    ("theme.dark", "Dark"),
    ("theme.auto", "Automatic"),
    // Menus
    ("menu.menu", "Menu"),
    ("menu.file", "File"),
    ("menu.edit", "Edit"),
    ("menu.lists", "Lists"),
    ("menu.clients", "Clients"),
    ("menu.vendors", "Vendors"),
    ("menu.banking", "Banking"),
    ("menu.payroll", "Payroll"),
    ("menu.reports", "Reports"),
    ("menu.company", "Company"),
    ("menu.window", "Window"),
    ("menu.help", "Help"),
    // Shell
    ("shell.no_company", "No company open"),
    ("shell.closed_through", "Books closed through {date}"),
    ("shell.closed_short", "Closed through {date}"),
    ("shell.go_to", "Go to"),
    ("shell.go_to_hint", "Go to a screen or command"),
    ("shell.go_to_label", "Go to a screen or command"),
    ("shell.rail.workspace", "Workspace"),
    ("shell.rail.tools", "Tools"),
    ("shell.rail.open_windows", "Open windows"),
    ("shell.rail.toggle", "Navigation"),
    ("shell.empty", "Nothing is open. Press Alt+H for Home, or Ctrl+K to go to anything."),
    ("shell.popped_out", "This window is open on its own."),
    ("shell.bring_back", "Bring it back here"),
    // Tabs
    ("tabs.unsaved", "unsaved changes"),
    ("tabs.label_dirty", "{title}, unsaved changes"),
    ("tabs.close", "Close {title}"),
    ("tabs.close_menu", "Close"),
    ("tabs.pop_out", "Move to its own window"),
    ("tabs.dock", "Bring back as a tab"),
    // Windows
    ("window.placeholder.coming", "This screen is not available yet. It arrives in a later release."),
    ("window.placeholder.home", "Home arrives in a later release."),
    ("window.prompt.title", "Unsaved changes"),
    ("window.prompt.message", "\"{title}\" has changes that are not saved. What would you like to do?"),
    ("window.prompt.save", "Save"),
    ("window.prompt.discard", "Discard changes"),
    ("window.prompt.keep", "Keep editing"),
    // Status bar
    ("status.not_signed_in", "Not signed in"),
    ("status.engine.starting", "Books engine: starting"),
    ("status.engine.running", "Books engine: running"),
    ("status.engine.restarting", "Books engine: restarting"),
    ("status.engine.failed", "Books engine stopped:"),
    ("status.engine.stopped", "Books engine: stopped"),
    ("status.hint.go_to", "go to"),
    ("status.hint.next_window", "next window"),
    ("status.hint.shortcuts", "shortcuts"),
    ("status.hint.help", "help"),
    ("toast.not_here", "That does not apply to this screen."),
    ("toast.help_later", "Help for this screen is not available yet."),
    // Palette and overlay
    ("palette.hint", "Go to a screen or command"),
    ("palette.none", "Nothing matches. Try fewer letters."),
    ("shortcuts.title", "Keyboard shortcuts"),
    ("shortcuts.intro", "The keys that work right here. Press Esc to close."),
    ("shortcuts.scope.form", "In this form or grid"),
    ("shortcuts.scope.window", "In this window"),
    ("shortcuts.scope.global", "Anywhere"),
    ("shortcuts.fixed", "Always the same"),
    ("shortcuts.dates", "In date fields"),
    ("shortcuts.bank_feed", "In the Bank Feed"),
    ("shortcuts.close", "Close"),
    // About and privacy
    ("about.title", "About ClientLedger"),
    ("about.version", "Version"),
    ("about.close", "Close"),
    ("about.h.license", "License"),
    ("about.license.body", "ClientLedger is free, open-source software under the MIT license. The source code is public. There are no license keys, activation, or seat limits."),
    ("about.h.engine", "The accounting engine"),
    ("about.engine.body", "ClientLedger keeps your books with the Vibe MyBooks engine, which comes inside official ClientLedger releases from Kisaes LLC. You may use it for the internal business operations of you and your company, including keeping your own books. You may not copy or pass the engine on to others. Keeping books for other people or organizations as a service is not internal use and needs a separate license from Kisaes LLC. Write to licensing@kisaes.com with questions."),
    ("about.h.components", "Bundled components"),
    ("about.components.body", "PostgreSQL (PostgreSQL License) stores your books on this computer. IBM Plex Sans and IBM Plex Mono (SIL Open Font License 1.1) draw the text. Other libraries keep their own licenses, listed in ATTRIBUTION.md with the app."),
    ("about.font_license.show", "Show the font license"),
    ("about.h.privacy", "Privacy"),
    ("about.h.affiliation", "Not affiliated with Intuit"),
    ("about.affiliation.body", "ClientLedger is an independent project. It is not affiliated with, endorsed by, or sponsored by Intuit Inc. QuickBooks is a trademark of Intuit Inc., mentioned only to describe import and migration features."),
    ("privacy.title", "Privacy notice"),
    ("privacy.body", "Your books stay on this computer. ClientLedger collects no analytics and sends no crash reports. Logs stay on this computer."),
    ("privacy.h.online", "When it goes online"),
    ("privacy.online.body", "ClientLedger connects to the internet only when you turn on a feature that needs it: AI suggestions with your own key (or a local model that never leaves your computer), emailing invoices through your own mail server, or when you choose Check for updates."),
    ("privacy.h.secrets", "Keys and passwords"),
    ("privacy.secrets.body", "AI keys, mail passwords, and the database password are kept in your operating system keychain. They never appear in logs, backups, files you send, or error reports."),
    // Errors
    ("error.dialog.title", "Something went wrong"),
    ("error.dialog.id", "Error ID:"),
    ("error.dialog.copy", "Copy details"),
    ("error.dialog.show_details", "Show what will be copied"),
    ("error.dialog.details_label", "These details contain no amounts, names, or passwords."),
    ("error.dialog.copied", "Copied"),
    ("error.dialog.close", "Close"),
    ("error.internal", "Something went wrong inside ClientLedger."),
    ("fix.internal.report", "Try again. If it keeps happening, copy the details and report it on GitHub."),
    ("error.money.format", "That is not an amount ClientLedger can read."),
    ("fix.money.format", "Use digits, for example 1,250.00."),
    ("error.money.too_precise", "That amount has more than two decimal places."),
    ("fix.money.too_precise", "Round it to cents."),
    ("error.money.too_large", "That amount is too large."),
    ("fix.money.too_large", "Check for an extra digit."),
    ("error.money.bad_weights", "These amounts cannot be split that way."),
    ("fix.money.bad_weights", "Check the split percentages."),
    ("error.money.not_whole_cents", "That amount is not in whole cents."),
    ("fix.money.not_whole_cents", "Round it to cents."),
    ("error.amount.empty", "Enter an amount."),
    ("fix.amount.empty", "Type the amount, for example 125.00."),
    ("error.amount.invalid", "That is not an amount ClientLedger can read."),
    ("fix.amount.invalid", "Use digits, for example 1,250.00."),
    ("error.amount.ambiguous", "It is not clear which mark is the decimal point."),
    ("fix.amount.ambiguous", "Write the amount with a period for cents, for example 1250.00."),
    ("error.amount.too_many_decimals", "That amount has more than two decimal places."),
    ("fix.amount.too_many_decimals", "Round it to cents."),
    ("error.amount.too_large", "That amount is too large."),
    ("fix.amount.too_large", "Check for an extra digit."),
    ("error.amount.sign_conflict", "The minus sign and the parentheses disagree."),
    ("fix.amount.sign_conflict", "Use one or the other."),
    ("error.date.format", "That is not a date ClientLedger can read."),
    ("fix.date.format", "Type it as month/day/year, for example 11/04/2026."),
    ("error.date.no_such_day", "There is no such day on the calendar."),
    ("fix.date.no_such_day", "Check the day and month."),
    ("error.date.out_of_range", "That date is too far in the past or future."),
    ("fix.date.out_of_range", "Use a date between 1900 and 2199."),
    ("error.fiscal.bad_month", "That is not a month of the year."),
    ("fix.fiscal.bad_month", "Pick a month from 1 to 12."),
    ("error.api.validation", "The books engine did not accept that entry."),
    ("fix.api.validation", "Check the highlighted fields and try again."),
    ("error.api.local", "Something in this entry needs fixing before it can be saved."),
    ("error.api.conflict_stale", "Someone or something else changed this record while you were editing it."),
    ("fix.api.reload", "Reload it to see the latest version, then make your change again."),
    ("error.api.conflict_in_use", "This record is in use, so it cannot be changed that way."),
    ("error.api.conflict_exists", "A record with that name already exists."),
    ("error.api.busy", "ClientLedger is busy right now."),
    ("fix.api.try_again_soon", "Wait a moment and try again."),
    ("error.api.conflict", "This change conflicts with something already in your books."),
    ("error.api.closed_period", "That date falls in a period you have closed."),
    ("fix.api.closed_period", "Pick a date after the closing date, or ask an administrator to reopen the period."),
    ("error.api.offline", "That needs the internet, and ClientLedger is working offline."),
    ("fix.api.offline", "Turn off Work offline in Preferences if you want to use it."),
    ("error.api.feature_off", "That feature is not turned on."),
    ("fix.api.feature_off", "Turn it on in Preferences."),
    ("error.api.bad_credentials", "The email or password is not right."),
    ("error.api.session_expired", "Your sign-in has timed out."),
    ("fix.api.sign_in", "Sign in again."),
    ("error.api.tfa_invalid", "That code is not right."),
    ("error.api.locked", "This account is locked for a little while after too many tries."),
    ("fix.api.wait", "Wait a few minutes, then try again."),
    ("error.api.deactivated", "This account has been turned off."),
    ("error.api.forbidden", "You do not have permission to do that."),
    ("error.api.not_found", "That record could not be found. It may have been deleted."),
    ("error.api.unsupported", "That is not available in ClientLedger."),
    ("error.api.no_company", "No company is open."),
    ("fix.api.open_company", "Open a company first (File, Open or Switch Company)."),
    ("error.api.too_large", "That file or entry is too large."),
    ("error.api.engine_unavailable", "The books engine is not answering."),
    ("error.api.outcome_unknown", "ClientLedger could not tell whether that was saved."),
    ("fix.api.check_again", "Look for it in the register before entering it again, so it is not entered twice."),
    ("error.api.internal", "The books engine had a problem."),
    ("fix.api.report", "Try again. If it keeps happening, copy the details and report it on GitHub."),
    ("error.api.saved_unreadable", "Saved. ClientLedger could not show the result."),
    ("fix.api.reload_to_see", "Reload to see it. Do not enter it again."),
    ("error.api.stale_scope", "That result belonged to a company you have since left."),
    ("error.api.cancelled", "That was cancelled."),
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

    fn source_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                source_files(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }

    #[test]
    fn every_key_used_in_the_source_is_in_the_catalog() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        source_files(&root, &mut files);
        let mut missing = Vec::new();
        for f in files {
            let text = std::fs::read_to_string(&f).unwrap();
            // Only code before the tests module counts.
            let code = text.split("#[cfg(test)]").next().unwrap_or("");
            for call in ["t(\"", "t_with(\""] {
                let mut rest = code;
                while let Some(at) = rest.find(call) {
                    let before = rest[..at].chars().last();
                    rest = &rest[at + call.len()..];
                    if before.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                        continue; // part of a longer name such as `assert(`
                    }
                    let key = rest.split('"').next().unwrap_or("");
                    if !key.is_empty() && !has(key) {
                        missing.push(format!("{}: {key}", f.display()));
                    }
                }
            }
        }
        assert!(missing.is_empty(), "{missing:#?}");
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
