//! "ClientLedger didn't close normally" (P34 B4, Q-P34o): shown at start
//! when the session marker says the last run ended without its own
//! shutdown, or when the books engine could not start. Local only: nothing
//! here needs the engine, so it works while the engine is down.
//!
//! The journal sentence shows only when the database really replayed its
//! write-ahead log at this start (plan §10.7). "Copy details" copies a
//! scrubbed note (Q-119): an error id, the app version and what happened,
//! never an amount, a name, a memo or a path.

use std::path::PathBuf;
use std::sync::Arc;

use cl_model::{CommandId, Scope};
use eframe::egui::Ui;
use serde_json::{Value, json};

use super::form;
use crate::company::{CompanyEvent, CompanyLink};
use crate::i18n::t;
use crate::theme::Tokens;
use crate::windows::Screen;

/// The window's kind (not in a menu: the shell opens it at start).
pub const KIND: &str = "app.crash_recovery";

static INFO: std::sync::Mutex<Option<CrashInfo>> = std::sync::Mutex::new(None);

/// What the app found at start (the binary sets it before the window
/// opens; a developer command sets it in the demo).
pub fn set_info(info: CrashInfo) {
    *INFO
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(info);
}

/// What was found at start (nothing: a clean start).
#[must_use]
pub fn info() -> CrashInfo {
    INFO.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
        .unwrap_or_default()
}

/// What the shell knows at start.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CrashInfo {
    /// The last run ended without its own shutdown.
    pub unclean: bool,
    /// The database replayed its journal at this start.
    pub recovered: bool,
    /// The books engine could not start.
    pub engine_failed: bool,
    /// The folder with ClientLedger's logs.
    pub log_dir: Option<PathBuf>,
}

impl CrashInfo {
    /// True when the window should open.
    #[must_use]
    pub fn needs_window(&self) -> bool {
        self.unclean || self.engine_failed
    }

    /// The scrubbed text "Copy details" copies (no amounts, names, memos
    /// or paths).
    #[must_use]
    pub fn details(&self) -> String {
        format!(
            "ClientLedger {} — unclean exit: {}; database recovered: {}; engine started: {}; error id: CL-CRASH-{}",
            env!("CARGO_PKG_VERSION"),
            self.unclean,
            self.recovered,
            !self.engine_failed,
            u8::from(self.unclean)
                + 2 * u8::from(self.recovered)
                + 4 * u8::from(self.engine_failed),
        )
    }
}

/// What the owner chose last (for the control channel).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Choice {
    Check,
    Restore,
    RestoreFull,
    Logs,
    Copy,
    Continue,
}

/// The crash recovery window.
pub struct CrashRecoveryScreen {
    link: Arc<CompanyLink>,
    info: CrashInfo,
    chosen: Option<Choice>,
    /// A full database backup is in this computer's records.
    has_full_backup: bool,
}

impl CrashRecoveryScreen {
    /// The window for what the shell found at start.
    #[must_use]
    pub fn new(link: Arc<CompanyLink>, info: CrashInfo) -> CrashRecoveryScreen {
        CrashRecoveryScreen {
            link,
            info,
            chosen: None,
            // The full database backup window arrives in P37 (Q-P34y);
            // until then no full backup is ever recorded.
            has_full_backup: false,
        }
    }

    fn choose(&mut self, ui: &Ui, c: Choice) {
        self.chosen = Some(c);
        match c {
            Choice::Check => {
                // The quick check runs once the company is open (Home's
                // weekly check honours this flag).
                crate::backup_runner::request_check_after_open();
                self.link.push(CompanyEvent::Toast("crash.check.hint"));
                self.link
                    .push(CompanyEvent::CloseWindow(CommandId::new(KIND)));
            }
            Choice::Restore => {
                self.link.push(CompanyEvent::OpenWindow(CommandId::new(
                    "file.open_company",
                )));
                self.link
                    .push(CompanyEvent::CloseWindow(CommandId::new(KIND)));
            }
            Choice::RestoreFull => self
                .link
                .push(CompanyEvent::OpenWindow(CommandId::new("file.full_backup"))),
            Choice::Logs => {
                if let Some(dir) = &self.info.log_dir {
                    ui.ctx().open_url(eframe::egui::OpenUrl::new_tab(format!(
                        "file:///{}",
                        dir.display().to_string().replace('\\', "/")
                    )));
                }
            }
            Choice::Copy => {
                ui.ctx().copy_text(self.info.details());
                self.link.push(CompanyEvent::Toast("crash.copied"));
            }
            Choice::Continue => self
                .link
                .push(CompanyEvent::CloseWindow(CommandId::new(KIND))),
        }
    }
}

impl Screen for CrashRecoveryScreen {
    fn title(&self) -> String {
        t("crash.title")
    }

    fn scope(&self) -> Scope {
        Scope::Window
    }

    fn ui(&mut self, ui: &mut Ui) {
        // The database starts after the window: its replay shows when known.
        if let Some(s) = self.link.supervisor() {
            self.info.recovered |= s.database_recovered();
        }
        let tokens = Tokens::of(ui.ctx());
        ui.add_space(8.0);
        ui.heading(t("crash.title"));
        ui.add_space(4.0);
        if self.info.unclean {
            form::hint(ui, &tokens, &t("crash.body"));
        }
        if self.info.recovered {
            form::inline_success(ui, &tokens, &t("crash.journal"));
        }
        if self.info.engine_failed {
            form::inline_warning(ui, &tokens, &t("crash.engine_failed"));
        }
        ui.add_space(8.0);
        let mut pick = None;
        form::card(ui, &tokens, |ui| {
            if ui.button(t("crash.check")).clicked() {
                pick = Some(Choice::Check);
            }
            form::hint(ui, &tokens, &t("crash.check.hint"));
            ui.add_space(4.0);
            if ui.button(t("crash.restore")).clicked() {
                pick = Some(Choice::Restore);
            }
            if self.has_full_backup && ui.button(t("crash.restore_full")).clicked() {
                pick = Some(Choice::RestoreFull);
            }
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if self.info.log_dir.is_some() && ui.button(t("crash.logs")).clicked() {
                    pick = Some(Choice::Logs);
                }
                if ui.button(t("crash.copy")).clicked() {
                    pick = Some(Choice::Copy);
                }
            });
        });
        form::hint(ui, &tokens, &t("crash.secrets"));
        ui.add_space(8.0);
        if ui.button(t("crash.continue")).clicked() {
            pick = Some(Choice::Continue);
        }
        if let Some(c) = pick {
            self.choose(ui, c);
        }
    }

    fn command(&mut self, _id: CommandId) -> bool {
        false
    }

    fn state(&self) -> Value {
        json!({
            "screen": "crash_recovery",
            "unclean": self.info.unclean,
            "journal_replayed": self.info.recovered,
            "engine_failed": self.info.engine_failed,
            "full_backup_offered": self.has_full_backup,
            "chosen": self.chosen.map(|c| format!("{c:?}").to_lowercase()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn details_are_scrubbed_and_the_journal_line_needs_a_replay() {
        let i = CrashInfo {
            unclean: true,
            recovered: false,
            engine_failed: false,
            log_dir: Some(PathBuf::from("C:/Users/Owner Name/logs")),
        };
        assert!(i.needs_window());
        let d = i.details();
        assert!(!d.contains("Owner"), "{d}");
        assert!(d.contains("CL-CRASH-1"));
        assert!(!CrashInfo::default().needs_window());
        let dir = tempfile::tempdir().expect("tmp");
        let link = Arc::new(CompanyLink::demo(dir.path()));
        let s = CrashRecoveryScreen::new(link, i);
        assert_eq!(s.state()["journal_replayed"], false);
        assert_eq!(s.state()["full_backup_offered"], false);
    }
}
