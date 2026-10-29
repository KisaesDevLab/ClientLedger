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
const SECRET_WORDS: [&str; 12] = [
    "bearer",
    "session",
    "credential",
    "refresh",
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

/// Keys whose numbers are not money (ports, counts, statuses): the number
/// after one of these survives.
const NUMBER_KEYS: [&str; 13] = [
    "port",
    "status",
    "attempt",
    "attempts",
    "version",
    "pid",
    "generation",
    "restarts",
    "code",
    "schema_version",
    "schemaversion",
    "max",
    "exit",
];

/// Characters that end a token (whitespace, `=`, `:`, quotes, `,`, `;`,
/// braces and square brackets).
fn is_separator(c: char) -> bool {
    c.is_whitespace()
        || matches!(
            c,
            '=' | ':' | '"' | '\'' | ',' | ';' | '{' | '}' | '[' | ']'
        )
}

/// `[$(-]?\d[\d,]*(\.\d{1,2})?` as a whole token, with a closing `)` or a
/// trailing period tolerated. Whole dollars (`1500`) and short decimals
/// (`12.5`) count; `1.2.3` and `3f2a9c` do not.
fn is_amount(token: &str) -> bool {
    let t = token.trim_end_matches([')', '.']);
    let t = t.strip_prefix(['$', '(', '-', '+']).unwrap_or(t);
    let t = t.strip_prefix(['$', '(', '-']).unwrap_or(t);
    let (whole, cents) = match t.split_once('.') {
        Some((w, c)) => (w, Some(c)),
        None => (t, None),
    };
    let whole_ok = whole.bytes().next().is_some_and(|b| b.is_ascii_digit())
        && whole.bytes().all(|b| b.is_ascii_digit() || b == b',');
    let cents_ok =
        cents.is_none_or(|c| (1..=2).contains(&c.len()) && c.bytes().all(|b| b.is_ascii_digit()));
    whole_ok && cents_ok
}

/// A token that looks like a credential by its shape.
fn is_credential(token: &str) -> bool {
    (token.starts_with("eyJ") && token.len() > 20)
        || ((token.starts_with("sk-") || token.starts_with("sk_")) && token.len() > 10)
        || (token.starts_with("ghp_") && token.len() > 10)
        || (token.starts_with("github_pat_") && token.len() > 15)
}

/// Replace the user-name segment of a home-folder path with `<user>`. The
/// whole segment goes, up to the next path separator, so names with spaces
/// ("Mary Ann") are covered.
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
        let end = after.find(['\\', '/']).unwrap_or(after.len());
        if end > 0 {
            out.push_str("<user>");
        }
        rest = &after[end..];
    }
}

fn flush(token: &mut String, prev_key: &mut String, out: &mut String) {
    if token.is_empty() {
        return;
    }
    let key_ok = NUMBER_KEYS.contains(&prev_key.to_ascii_lowercase().as_str());
    if token.contains('@') {
        out.push_str("[email]");
    } else if is_credential(token) {
        out.push_str("[credential]");
    } else if is_amount(token) && !key_ok {
        out.push_str("[amount]");
    } else {
        out.push_str(token);
    }
    prev_key.clone_from(token);
    token.clear();
}

/// Remove from one log line what must not leave the machine.
#[must_use]
pub fn scrub_line(line: &str) -> String {
    // The host's master-secret messages name a folder and give guidance; they
    // never hold a secret, so the word "secret" alone does not withhold them.
    // The host decides by matching the whole message (fixed opening, folder,
    // fixed ending), so any added text loses the waiver. Every other
    // credential word still withholds them, and the path and token passes
    // below still run.
    let master = cl_host::is_master_secret_message(line);
    let lower = line.to_ascii_lowercase();
    if SECRET_WORDS
        .iter()
        .any(|w| lower.contains(w) && !(master && *w == "secret"))
    {
        return "[line withheld: may contain a credential]".to_string();
    }
    let line = scrub_user_path(line);
    let mut out = String::with_capacity(line.len());
    let mut token = String::new();
    let mut prev_key = String::new();
    let chars: Vec<char> = line.chars().collect();
    for (i, c) in chars.iter().copied().enumerate() {
        // A comma between digits is a thousands separator, part of the number.
        let thousands = c == ','
            && token.ends_with(|d: char| d.is_ascii_digit())
            && chars.get(i + 1).is_some_and(char::is_ascii_digit);
        if is_separator(c) && !thousands {
            flush(&mut token, &mut prev_key, &mut out);
            out.push(c);
        } else {
            token.push(c);
        }
    }
    flush(&mut token, &mut prev_key, &mut out);
    out
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
            ui.add_space(8.0);
            egui::CollapsingHeader::new(t("error.dialog.show_details")).show(ui, |ui| {
                let label = ui.label(
                    RichText::new(t("error.dialog.details_label")).color(tokens.text_secondary),
                );
                // Read-only and selectable: the owner can see exactly what
                // "Copy details" will put on the clipboard.
                let mut shown: &str = &dialog.details;
                let edit = ui.add(
                    egui::TextEdit::multiline(&mut shown)
                        .font(egui::TextStyle::Monospace)
                        .desired_rows(8)
                        .desired_width(f32::INFINITY),
                );
                edit.labelled_by(label.id);
            });
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
    fn amounts_in_every_shape_are_removed() {
        for (input, want) in [
            ("amount=1250.00 ok", "amount=[amount] ok"),
            (
                r#"{"total":"1,250.5","memo":"x"}"#,
                r#"{"total":"[amount]","memo":"x"}"#,
            ),
            ("paid 1500 to vendor", "paid [amount] to vendor"),
            ("balance 12.5 due", "balance [amount] due"),
            ("rows\t100.25\t(3,000.00)", "rows\t[amount]\t[amount]"),
            ("net:-42.10;gross:$9", "net:[amount];gross:[amount]"),
            ("debit,17.5,credit", "debit,[amount],credit"),
        ] {
            assert_eq!(scrub_line(input), want, "{input}");
        }
    }

    #[test]
    fn allow_listed_keys_keep_their_numbers() {
        for line in [
            "port=4100 pid=912",
            "status: 503, attempt: 2, version 1.2.3",
            r#"{"status":409,"restarts":3,"generation":7}"#,
        ] {
            assert_eq!(scrub_line(line), line);
        }
    }

    #[test]
    fn user_names_with_spaces_are_removed_whole() {
        assert_eq!(
            scrub_line(r"opened C:\Users\Mary Ann Smith\AppData\x.log"),
            r"opened C:\Users\<user>\AppData\x.log"
        );
        assert_eq!(
            scrub_line("db /home/mary ann/.local/share"),
            "db /home/<user>/.local/share"
        );
    }

    #[test]
    fn credentials_without_keywords_are_removed() {
        let jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.abcdefghijk";
        assert_eq!(scrub_line(&format!("got {jwt} ok")), "got [credential] ok");
        assert_eq!(
            scrub_line("key sk-ant-api03-abcdefghijklmnop used"),
            "key [credential] used"
        );
        assert_eq!(
            scrub_line("tok ghp_abcdefghijklmnop1234"),
            "tok [credential]"
        );
        for l in [
            "Bearer abc",
            "session id abc",
            "credential store",
            "refresh value",
        ] {
            assert!(scrub_line(l).starts_with("[line withheld"), "{l}");
        }
    }

    fn sample_dir() -> PathBuf {
        PathBuf::from(r"C:\Users\alex.sample\AppData\Local\ClientLedger\pgdata")
    }

    #[test]
    fn master_secret_messages_are_shown_not_withheld() {
        let dir = sample_dir();
        for e in [
            cl_host::HostError::MasterSecretMissing {
                data_dir: dir.clone(),
            },
            cl_host::HostError::MasterSecretMismatch {
                data_dir: dir.clone(),
            },
        ] {
            let shown = scrub_line(&e.to_string());
            assert!(shown.starts_with("the engine's master secret"), "{shown}");
            assert!(
                shown.contains(r"C:\Users\<user>\AppData\Local\ClientLedger\pgdata"),
                "{shown}"
            );
            assert!(!shown.contains("alex.sample"), "{shown}");
        }
        let fault = cl_host::HostFault {
            component: cl_host::Component::Database,
            reason: cl_host::HostError::MasterSecretMissing { data_dir: dir }.to_string(),
            restarts: 0,
            log_paths: vec![],
            last_output: vec![],
        };
        assert!(!crate::engine_link::fault_text(&fault).contains("withheld"));
    }

    #[test]
    fn the_master_secret_waiver_needs_the_whole_message() {
        // Without the exact message, "secret" still withholds the line.
        assert!(scrub_line("master secret sk-abcdefghijklmnop").starts_with("[line withheld"));
        let p = "the engine's master secret ";
        assert!(scrub_line(&format!("{p}for x")).starts_with("[line withheld"));
        let text = cl_host::HostError::MasterSecretMissing {
            data_dir: sample_dir(),
        }
        .to_string();
        // Anything appended or prepended (a hex value, another word, a second
        // line) loses the waiver ...
        for line in [
            format!("{text} 9f3a2b7c4d5e6f708192a3b4c5d6e7f8"),
            format!("{text} password=hunter2"),
            format!("{text}\nsecret=abc"),
            format!("token abc {text}"),
        ] {
            assert!(scrub_line(&line).starts_with("[line withheld"), "{line}");
        }
        // ... and a credential word inside the folder name still withholds it.
        let odd = cl_host::HostError::MasterSecretMissing {
            data_dir: PathBuf::from("/srv/password=hunter2/pgdata"),
        }
        .to_string();
        assert!(scrub_line(&odd).starts_with("[line withheld"));
        // Credential-shaped tokens in the folder are still removed.
        let tok = cl_host::HostError::MasterSecretMissing {
            data_dir: PathBuf::from("/srv/old sk-abcdefghijklmnop"),
        }
        .to_string();
        let shown = scrub_line(&tok);
        assert!(!shown.contains("sk-abcdefghijklmnop"), "{shown}");
        assert!(shown.contains("[credential]"), "{shown}");
    }

    #[test]
    fn details_text_after_scrubbing_has_no_marker_values() {
        let id = ErrorId::new(7);
        let line = scrub_line("save failed amount=987654.32 for bob@example.com");
        let text = details_text(
            id,
            "error.api.validation",
            &line,
            std::slice::from_ref(&line),
        );
        assert!(!text.contains("987654") && !text.contains("bob@"), "{text}");
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
            let _ = writeln!(text, "entry-{i} for kurt@example.com amount 12.50");
        }
        std::fs::write(&path, text).unwrap();
        let ex = log_excerpt(&path, EXCERPT_LINES);
        assert_eq!(ex.len(), EXCERPT_LINES);
        assert!(
            ex.iter()
                .all(|l| l.contains("[email]") && l.contains("[amount]"))
        );
        assert!(ex.last().unwrap().starts_with("entry-49"));
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
