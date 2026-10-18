//! Recovery key (P08 step 3, Q-210): the key from first-run setup is
//! shown once with Copy and Save to a file, and the owner confirms they
//! kept it before the window closes. The engine stores only a hash
//! (U-23). The window never makes a new key by itself: "Make a new
//! recovery key" is an explicit button that says the old key stops
//! working. Until the engine advertises `recovery-key-v1` the screen says
//! the key is not available yet.

use std::sync::Arc;

use cl_api::Capabilities;
use cl_company::CompanyError;
use cl_core::Secret;
use cl_model::{CommandId, Scope};
use eframe::egui::{self, RichText, Ui};
use serde_json::{Value, json};

use super::form;
use crate::company::{CompanyEvent, CompanyLink, Task, spawn_lifecycle};
use crate::i18n::t;
use crate::theme::Tokens;
use crate::windows::Screen;

/// The recovery-key window.
pub struct RecoveryKeyScreen {
    link: Arc<CompanyLink>,
    group: Option<cl_api::RequestGroup>,
    key: Option<Secret>,
    task: Option<Task<Result<Secret, CompanyError>>>,
    revealed: bool,
    confirmed: bool,
    saved_to: Option<String>,
    save_path: String,
    copied: bool,
    error: Option<CompanyError>,
    unavailable: bool,
    done: bool,
}

impl RecoveryKeyScreen {
    /// A window that fetches (rotates) a key, or shows the demo key.
    #[must_use]
    pub fn new(link: Arc<CompanyLink>) -> RecoveryKeyScreen {
        let group = link.group().and_then(Result::ok);
        let stashed = link.take_recovery_key();
        let unavailable = stashed.is_none()
            && !link.is_demo()
            && !link.capabilities().has(Capabilities::RECOVERY_KEY);
        let key = stashed.or_else(|| {
            link.is_demo()
                .then(|| Secret::new(crate::company::demo::DEMO_RECOVERY_KEY))
        });
        // Outside the app data folder (a backup or a copied data folder
        // must not carry the key): the user's Documents, else home. The
        // demo (scripted runs) stays inside its own temporary folder. A
        // time stamp in the name keeps every save a new file.
        let folder = if link.is_demo() {
            link.backup_dir.clone()
        } else {
            default_key_folder()
        };
        let stamp = cl_settings::recent::now().as_utc().format("%Y%m%d-%H%M%S");
        let save_path = folder
            .join(format!("ClientLedger-recovery-key-{stamp}.txt"))
            .display()
            .to_string();
        RecoveryKeyScreen {
            link,
            group,
            key,
            task: None,
            revealed: false,
            confirmed: false,
            saved_to: None,
            save_path,
            copied: false,
            error: None,
            unavailable,
            done: false,
        }
    }

    /// Show a key the engine returned at first-run setup (U-23).
    #[must_use]
    pub fn with_key(mut self, key: Secret) -> RecoveryKeyScreen {
        self.key = Some(key);
        self.unavailable = false;
        self
    }

    /// Makes a new key on the owner's explicit request; the old key stops
    /// working the moment the engine answers.
    fn rotate(&mut self) {
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        self.revealed = false;
        self.confirmed = false;
        self.saved_to = None;
        self.copied = false;
        self.task = spawn_lifecycle(&self.link, group, move || async move {
            session
                .rotate_recovery_key()
                .await
                .map_err(CompanyError::from)
        })
        .ok();
    }

    fn groups(key: &str) -> String {
        let flat: String = key.chars().filter(char::is_ascii_alphanumeric).collect();
        if flat.is_empty() {
            return key.to_string();
        }
        flat.as_bytes()
            .chunks(4)
            .map(|c| String::from_utf8_lossy(c).into_owned())
            .collect::<Vec<_>>()
            .join("-")
    }

    fn save_file(&mut self) {
        let Some(key) = &self.key else { return };
        let path = std::path::PathBuf::from(self.save_path.trim());
        let text = format!(
            "ClientLedger recovery key\n\nThis key resets the Admin password. Keep it somewhere safe and private.\n\n{}\n",
            Self::groups(key.expose())
        );
        let result = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| write_private_new(&path, text.as_bytes()));
        match result {
            Ok(()) => self.saved_to = Some(path.display().to_string()),
            Err(source) => {
                self.error = Some(CompanyError::Io {
                    what: "save recovery key",
                    source,
                });
            }
        }
    }
}

impl Screen for RecoveryKeyScreen {
    fn title(&self) -> String {
        t("recovery.tab")
    }

    fn scope(&self) -> Scope {
        Scope::Window
    }

    fn dirty(&self) -> bool {
        self.key.is_some() && !self.confirmed && !self.done
    }

    fn ui(&mut self, ui: &mut Ui) {
        let tokens = Tokens::of(ui.ctx());
        if let Some(t) = self.task.as_mut()
            && let Some(r) = t.poll_result()
        {
            self.task = None;
            match r {
                Ok(k) => self.key = Some(k),
                Err(e) => self.error = Some(e),
            }
        }
        ui.add_space(8.0);
        ui.heading(t("recovery.title"));
        form::hint(ui, &tokens, &t("recovery.intro"));
        ui.add_space(8.0);
        if self.unavailable {
            form::card(ui, &tokens, |ui| {
                form::hint(ui, &tokens, &t("recovery.unavailable"));
                if form::actions(ui, &t("recovery.close"), &[]).is_some() {
                    self.done = true;
                    self.link
                        .push(CompanyEvent::OpenWindow(CommandId::new("nav.home")));
                    self.link.push(CompanyEvent::CloseWindow(CommandId::new(
                        "company.recovery_key",
                    )));
                }
            });
            return;
        }
        if let Some(e) = &self.error {
            form::inline_error(ui, &tokens, e);
        }
        let Some(key) = self.key.clone() else {
            if self.task.is_some() {
                form::progress_line(ui, &tokens, &t("recovery.working"));
                return;
            }
            form::card(ui, &tokens, |ui| {
                form::heading(ui, &tokens, &t("recovery.none_title"));
                form::hint(ui, &tokens, &t("recovery.none_body"));
                ui.add_space(6.0);
                if let Some(i) = form::actions(ui, &t("recovery.rotate"), &[&t("recovery.close")]) {
                    if i == 0 {
                        self.rotate();
                    } else {
                        self.done = true;
                        self.link.push(CompanyEvent::CloseWindow(CommandId::new(
                            "company.recovery_key",
                        )));
                    }
                }
            });
            return;
        };
        // A control-channel screenshot is being taken: show the masked form.
        let masked_for_screenshot = ui.ctx().data(|d| {
            d.get_temp::<std::time::Instant>(crate::automation::hide_secrets_id())
                .is_some_and(|until| std::time::Instant::now() < until)
        });
        form::card(ui, &tokens, |ui| {
            form::heading(ui, &tokens, &t("recovery.your_key"));
            let shown = if self.revealed && !masked_for_screenshot {
                Self::groups(key.expose())
            } else {
                "\u{2022}\u{2022}\u{2022}\u{2022}-\u{2022}\u{2022}\u{2022}\u{2022}-\u{2022}\u{2022}\u{2022}\u{2022}-\u{2022}\u{2022}\u{2022}\u{2022}".to_string()
            };
            let r = ui.add(
                egui::Label::new(
                    RichText::new(shown)
                        .monospace()
                        .size(20.0)
                        .color(tokens.text_primary),
                )
                .selectable(self.revealed),
            );
            // The label names the secret so `inspect` hides its text.
            r.widget_info(|| {
                egui::WidgetInfo::labeled(egui::Role::Label, true, t("recovery.key_label"))
            });
            ui.horizontal(|ui| {
                if ui
                    .button(if self.revealed {
                        t("recovery.hide")
                    } else {
                        t("recovery.show")
                    })
                    .clicked()
                {
                    self.revealed = !self.revealed;
                }
                if ui.button(t("recovery.copy")).clicked() {
                    ui.ctx().copy_text(Self::groups(key.expose()));
                    self.copied = true;
                }
            });
            if self.copied {
                form::hint(ui, &tokens, &t("recovery.copied"));
            }
            ui.add_space(8.0);
            form::field(ui, &t("recovery.save_path"), &mut self.save_path);
            if ui.button(t("recovery.save")).clicked() {
                self.save_file();
            }
            if let Some(p) = &self.saved_to {
                form::hint(
                    ui,
                    &tokens,
                    &crate::i18n::t_with("recovery.saved", &[("path", p)]),
                );
            }
            form::hint(ui, &tokens, &t("recovery.print_later"));
            form::hint(ui, &tokens, &t("recovery.save_where"));
        });
        ui.add_space(10.0);
        ui.checkbox(&mut self.confirmed, t("recovery.confirm"));
        ui.add_space(6.0);
        ui.add_enabled_ui(self.confirmed, |ui| {
            if form::actions(ui, &t("recovery.done"), &[]).is_some() {
                self.done = true;
                self.key = None;
                self.link.push(CompanyEvent::OpenWindow(CommandId::new(
                    "company.opening_balances",
                )));
                self.link.push(CompanyEvent::CloseWindow(CommandId::new(
                    "company.recovery_key",
                )));
            }
        });
    }

    fn state(&self) -> Value {
        json!({
            "screen": "recovery_key",
            "has_key": self.key.is_some(),
            "revealed": self.revealed,
            "confirmed": self.confirmed,
            "saved": self.saved_to.is_some(),
            "copied": self.copied,
            "unavailable": self.unavailable,
            "done": self.done,
        })
    }
}

/// The user's Documents folder when it exists, else the home folder, else
/// the current folder: never the app data folder.
fn default_key_folder() -> std::path::PathBuf {
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(std::path::PathBuf::from);
    if let Some(h) = &home {
        let docs = h.join("Documents");
        if docs.is_dir() {
            return docs;
        }
        return h.clone();
    }
    std::path::PathBuf::from(".")
}

/// Writes a new file only (never over an existing one), readable by the
/// owner alone on Unix.
fn write_private_new(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut f = options.open(path)?;
    f.write_all(bytes)?;
    f.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saving_never_overwrites_and_defaults_outside_the_app_folder() {
        let dir = tempfile::tempdir().expect("tmp");
        let p = dir.path().join("key.txt");
        write_private_new(&p, b"one").expect("first write");
        let e = write_private_new(&p, b"two").expect_err("second write refused");
        assert_eq!(e.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&p).expect("read"), b"one");
        let folder = default_key_folder();
        assert!(!folder.ends_with("backups"));
    }

    #[test]
    fn keys_are_grouped_in_fours() {
        assert_eq!(RecoveryKeyScreen::groups("K7F29QMD3XTB"), "K7F2-9QMD-3XTB");
        assert_eq!(RecoveryKeyScreen::groups("k7f2-9qmd"), "k7f2-9qmd");
        assert_eq!(RecoveryKeyScreen::groups(""), "");
    }
}
