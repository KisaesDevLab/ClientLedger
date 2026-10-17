//! Error presentation (Q-119): a plain-language message, a suggested fix, and
//! "Copy details", which copies a scrubbed error id and log excerpt the owner
//! can paste into a GitHub issue.
//!
//! The details never contain amounts, names, memo text, email addresses,
//! secrets or the owner's user name: [`scrub_line`] removes what it can
//! recognise and withholds any line that mentions a credential. Error values
//! come in through `cl_core::OwnerFacing`, whose `scrubbed_detail` is already
//! limited to codes, statuses and ids.

use std::collections::hash_map::RandomState;
use std::fmt::Write as _;
use std::hash::{BuildHasher, Hasher};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use cl_core::{ErrorId, OwnerFacing};
use eframe::egui::{self, Align2, Context, RichText};

use crate::i18n::t;
use crate::theme::Tokens;
use crate::widgets::primary_button;

/// How many log lines "Copy details" includes.
pub const EXCERPT_LINES: usize = 20;

/// Words that mean a log line may carry a credential; such lines are replaced
/// whole.
const SECRET_WORDS: [&str; 8] = [
    "password",
    "passwd",
    "secret",
    "token",
    "authorization",
    "cookie",
    "api_key",
    "apikey",
];

/// A fresh error id, from the system clock mixed with process-random state.
#[must_use]
pub fn fresh_id() -> ErrorId {
    let mut h = RandomState::new().build_hasher();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    h.write_u128(nanos);
    ErrorId::new(h.finish())
}

fn is_amount(core: &str) -> bool {
    let core = core.trim_start_matches(['$', '-', '(', '+']);
    let core = core.trim_end_matches([')', '.', ',', ';', ':']);
    let Some((whole, cents)) = core.rsplit_once('.') else {
        return false;
    };
    cents.len() == 2
        && cents.bytes().all(|b| b.is_ascii_digit())
        && !whole.is_empty()
        && whole.bytes().all(|b| b.is_ascii_digit() || b == b',')
        && whole.bytes().any(|b| b.is_ascii_digit())
}

/// Replace the user-name segment of a home-folder path with `<user>`.
fn scrub_user_path(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    loop {
        let lower = rest.to_ascii_lowercase();
        let hit = ["\\users\\", "/users/", "/home/"]
            .iter()
            .filter_map(|m| lower.find(m).map(|i| (i, m.len())))
            .min_by_key(|(i, _)| *i);
        let Some((at, len)) = hit else {
            out.push_str(rest);
            return out;
        };
        out.push_str(&rest[..at + len]);
        let after = &rest[at + len..];
        let end = after
            .find(['\\', '/', ' ', '"', '\'', ':'])
            .unwrap_or(after.len());
        if end > 0 {
            out.push_str("<user>");
        }
        rest = &after[end..];
    }
}

/// Remove from one log line what must not leave the machine.
#[must_use]
pub fn scrub_line(line: &str) -> String {
    let lower = line.to_ascii_lowercase();
    if SECRET_WORDS.iter().any(|w| lower.contains(w)) {
        return "[line withheld: may contain a credential]".to_string();
    }
    let line = scrub_user_path(line);
    line.split(' ')
        .map(|tok| {
            if tok.contains('@') {
                "[email]".to_string()
            } else if is_amount(tok) {
                "[amount]".to_string()
            } else {
                tok.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Scrubbed last lines of a log file.
#[must_use]
pub fn log_excerpt(path: &Path, max_lines: usize) -> Vec<String> {
    cl_host::tail_lines(path, max_lines)
        .iter()
        .map(|l| scrub_line(l))
        .collect()
}

/// An error ready to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorDialog {
    /// Quoted back to the owner and written to the log.
    pub id: ErrorId,
    /// Plain-language message.
    pub message: String,
    /// Suggested fix, when there is one.
    pub fix: Option<String>,
    /// What "Copy details" puts on the clipboard.
    pub details: String,
    /// "Copied" feedback has been shown.
    pub copied: bool,
}

impl ErrorDialog {
    /// Build the dialog for any owner-facing error. `log` is the file whose
    /// tail is attached; `None` attaches no log.
    #[must_use]
    pub fn new(error: &dyn OwnerFacing, id: ErrorId, log: Option<&Path>) -> ErrorDialog {
        let m = error.owner_message();
        let excerpt = log
            .map(|p| log_excerpt(p, EXCERPT_LINES))
            .unwrap_or_default();
        let detail = scrub_line(&error.scrubbed_detail());
        // The id also goes to the log, so a report can be matched to its line.
        tracing::error!(error_id = %id, key = m.key, "error shown to the owner");
        ErrorDialog {
            id,
            message: t(m.key),
            fix: m.suggestion_key.map(t),
            details: details_text(id, m.key, &detail, &excerpt),
            copied: false,
        }
    }
}

/// The text "Copy details" produces.
#[must_use]
pub fn details_text(id: ErrorId, key: &str, detail: &str, excerpt: &[String]) -> String {
    let mut s = String::new();
    s.push_str("ClientLedger error report\n");
    let _ = writeln!(s, "Error ID: {id}");
    let _ = writeln!(
        s,
        "App version: {}\nSystem: {} {}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    let _ = writeln!(s, "Message key: {key}\nDetail: {detail}");
    if excerpt.is_empty() {
        s.push_str("Log: (none)\n");
    } else {
        let _ = writeln!(s, "Log (last {} lines, scrubbed):", excerpt.len());
        for l in excerpt {
            s.push_str("  ");
            s.push_str(l);
            s.push('\n');
        }
    }
    s
}

/// What the owner did in the dialog this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogAction {
    /// Still open.
    None,
    /// Closed.
    Close,
}

/// Draw the dialog. Returns whether it was closed.
pub fn show(ctx: &Context, dialog: &mut ErrorDialog) -> DialogAction {
    let tokens = Tokens::of(ctx);
    let mut action = DialogAction::None;
    egui::Window::new(t("error.dialog.title"))
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .collapsible(false)
        .resizable(false)
        .default_width(420.0)
        .show(ctx, |ui| {
            ui.label(RichText::new(&dialog.message).text_style(crate::theme::styles::section()));
            if let Some(fix) = &dialog.fix {
                ui.add_space(4.0);
                ui.label(RichText::new(fix).color(tokens.text_secondary));
            }
            ui.add_space(8.0);
            ui.label(
                RichText::new(format!("{} {}", t("error.dialog.id"), dialog.id))
                    .text_style(crate::theme::styles::kbd())
                    .color(tokens.text_tertiary),
            );
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if primary_button(ui, &t("error.dialog.close")).clicked() {
                    action = DialogAction::Close;
                }
                let label = if dialog.copied {
                    t("error.dialog.copied")
                } else {
                    t("error.dialog.copy")
                };
                if ui.button(label).clicked() {
                    ui.ctx().copy_text(dialog.details.clone());
                    dialog.copied = true;
                }
            });
        });
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        action = DialogAction::Close;
    }
    action
}

/// The host log the dialog attaches, under the app data root.
#[must_use]
pub fn host_log_path(paths: &cl_host::AppPaths) -> PathBuf {
    paths.host_log()
}

#[cfg(test)]
mod tests {
    use super::*;
    use cl_core::{Money, UserInput};

    #[test]
    fn amounts_emails_and_user_names_are_removed() {
        assert_eq!(
            scrub_line("paid $1,234.56 to bob@example.com"),
            "paid [amount] to [email]"
        );
        assert_eq!(scrub_line("balance -42.10 ok"), "balance [amount] ok");
        assert_eq!(scrub_line("total (1,000.00)"), "total [amount]");
        assert_eq!(
            scrub_line(r"opened C:\Users\Kurt\AppData\Local\ClientLedger\x.log"),
            r"opened C:\Users\<user>\AppData\Local\ClientLedger\x.log"
        );
        assert_eq!(
            scrub_line("db at /home/kurt/.local/share/cl"),
            "db at /home/<user>/.local/share/cl"
        );
        assert_eq!(
            scrub_line("db at /Users/kurt/Library/x"),
            "db at /Users/<user>/Library/x"
        );
    }

    #[test]
    fn ordinary_numbers_and_ids_survive() {
        let line = "request 3f2a9c status 503 attempt 2 version 1.2.3 port 4100";
        assert_eq!(scrub_line(line), line);
    }

    #[test]
    fn credential_lines_are_withheld_whole() {
        for l in [
            "X-CL-Token: abcdef",
            "Authorization=Bearer abc",
            "password reset for user",
            "set-cookie: a=b",
            "ANTHROPIC_API_KEY loaded",
        ] {
            assert!(scrub_line(l).starts_with("[line withheld"), "{l}");
        }
    }

    #[test]
    fn details_have_the_id_and_no_marker_values() {
        let marker_err = Money::parse_user("987,654.555", UserInput::CENTS).unwrap_err();
        let id = ErrorId::new(0x12_3456_789A);
        let d = ErrorDialog::new(&marker_err, id, None);
        assert!(d.details.contains(&id.to_string()));
        assert!(
            d.details
                .contains("Message key: error.amount.too_many_decimals")
        );
        assert!(!d.details.contains("987"), "{}", d.details);
        assert_eq!(d.message, t("error.amount.too_many_decimals"));
        assert!(d.fix.is_some());
    }

    #[test]
    fn log_excerpt_is_scrubbed_and_bounded() {
        let dir = std::env::temp_dir().join(format!("cl-ui-log-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("host.log");
        let mut text = String::new();
        for i in 0..50 {
            let _ = writeln!(text, "line {i} for kurt@example.com amount 12.50");
        }
        std::fs::write(&path, text).unwrap();
        let ex = log_excerpt(&path, EXCERPT_LINES);
        assert_eq!(ex.len(), EXCERPT_LINES);
        assert!(
            ex.iter()
                .all(|l| l.contains("[email]") && l.contains("[amount]"))
        );
        assert!(ex.last().unwrap().starts_with("line 49"));
        assert_eq!(
            log_excerpt(&dir.join("missing.log"), 5),
            Vec::<String>::new()
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn ids_are_well_formed_and_vary() {
        let a = fresh_id();
        let b = fresh_id();
        assert_eq!(a.to_string().len(), 12);
        assert_ne!(a, b);
        assert_eq!(ErrorId::parse(&a.to_string()), Some(a));
    }

    #[test]
    fn every_message_key_the_core_and_api_emit_has_text() {
        // Mirrors cl-core/cl-api: `error.*` and `fix.*` keys.
        for src in [
            include_str!("../../cl-core/src/errors.rs"),
            include_str!("../../cl-api/src/errors.rs"),
        ] {
            for piece in src.split('"') {
                let is_key = (piece.starts_with("error.") || piece.starts_with("fix."))
                    && piece.len() > 6
                    && piece.bytes().all(|b| {
                        b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'_'
                    });
                if is_key && piece != "error.x" && !piece.ends_with('.') {
                    assert!(crate::i18n::has(piece), "missing text for {piece}");
                }
            }
        }
    }
}
