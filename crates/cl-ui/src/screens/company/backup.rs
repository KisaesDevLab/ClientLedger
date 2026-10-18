//! Back Up, Portable copy and Export All Data (P08 steps 5 and 6): one
//! window with the destination folder, the kind, progress, and the
//! result. The backup passphrase comes from the keychain; the first time,
//! the owner sets it here (shown once, Q-P02i).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use cl_company::{BackupOutcome, BackupRequest, CompanyError, ExportAllOutcome, Progress};
use cl_core::Secret;
use cl_core::ids::TenantId;
use cl_model::{CommandId, Scope};
use cl_settings::BackupKind;
use eframe::egui::{RichText, Ui};
use serde_json::{Value, json};

use super::form;
use crate::company::{CompanyEvent, CompanyLink, Task, spawn_lifecycle};
use crate::i18n::{t, t_with};
use crate::theme::Tokens;
use crate::windows::Screen;

/// Which job the window runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Job {
    /// `.clbackup` with retention.
    Backup,
    /// `.clbackup` flagged portable, never pruned.
    Portable,
    /// CSV zip (Q-56).
    ExportAll,
}

impl Job {
    fn kind(self) -> BackupKind {
        if self == Job::Portable {
            BackupKind::Portable
        } else {
            BackupKind::Backup
        }
    }

    fn title_key(self) -> &'static str {
        match self {
            Job::Backup => "backup.title",
            Job::Portable => "backup.portable_title",
            Job::ExportAll => "backup.export_title",
        }
    }

    fn intro_key(self) -> &'static str {
        match self {
            Job::Backup => "backup.intro",
            Job::Portable => "backup.portable_intro",
            Job::ExportAll => "backup.export_intro",
        }
    }
}

enum Outcome {
    Backup(BackupOutcome),
    Export(ExportAllOutcome),
}

/// The Back Up / Portable copy / Export window.
pub struct BackupScreen {
    link: Arc<CompanyLink>,
    group: Option<cl_api::RequestGroup>,
    job: Job,
    tenant: Option<(TenantId, String)>,
    dest_dir: String,
    passphrase_needed: bool,
    passphrase: String,
    passphrase_confirm: String,
    progress: Arc<Mutex<Option<Progress>>>,
    task: Option<Task<Result<Outcome, CompanyError>>>,
    outcome: Option<String>,
    error: Option<CompanyError>,
}

impl BackupScreen {
    /// A window for `job` over the open company.
    #[must_use]
    pub fn new(
        link: Arc<CompanyLink>,
        job: Job,
        tenant: Option<(TenantId, String)>,
    ) -> BackupScreen {
        let group = link.group().and_then(Result::ok);
        let passphrase_needed = job != Job::ExportAll
            && !link.is_demo()
            && link
                .secrets()
                .get(cl_settings::secrets::BACKUP_PASSPHRASE)
                .ok()
                .flatten()
                .is_none_or(|p| p.is_empty());
        BackupScreen {
            dest_dir: link.backup_dir.display().to_string(),
            link,
            group,
            job,
            tenant,
            passphrase_needed,
            passphrase: String::new(),
            passphrase_confirm: String::new(),
            progress: Arc::new(Mutex::new(None)),
            task: None,
            outcome: None,
            error: None,
        }
    }

    fn start(&mut self) {
        let Some((tenant_id, name)) = self.tenant.clone() else {
            self.error = Some(CompanyError::UnknownCompany);
            return;
        };
        if self.passphrase_needed {
            if self.passphrase.chars().count() < 12 {
                self.error = Some(CompanyError::PassphraseTooShort);
                return;
            }
            if self.passphrase != self.passphrase_confirm {
                self.error = Some(CompanyError::PassphraseMismatch);
                return;
            }
            let secret = Secret::new(std::mem::take(&mut self.passphrase));
            self.passphrase_confirm.clear();
            if let Err(e) = self
                .link
                .secrets()
                .set(cl_settings::secrets::BACKUP_PASSPHRASE, &secret)
            {
                self.error = Some(e.into());
                return;
            }
            self.passphrase_needed = false;
        }
        let dest_dir = PathBuf::from(self.dest_dir.trim());
        if self.link.is_demo() {
            let file = dest_dir.join(cl_company::backup::backup_file_name(
                &name,
                crate::company::demo::DemoState::now(),
                self.job.kind(),
            ));
            self.outcome = Some(match self.job {
                Job::ExportAll => t_with(
                    "backup.export_done",
                    &[("path", &file.with_extension("zip").display().to_string())],
                ),
                _ => t_with("backup.done", &[("path", &file.display().to_string())]),
            });
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        let store = self.link.store().clone();
        let secrets = self.link.secrets();
        let job = self.job;
        let slot = Arc::clone(&self.progress);
        let sink: cl_company::ProgressSink = Arc::new(move |p| {
            if let Ok(mut s) = slot.lock() {
                *s = Some(p);
            }
        });
        self.task = spawn_lifecycle(&self.link, group, move || async move {
            match job {
                Job::ExportAll => {
                    let file = dest_dir.join(format!(
                        "{}-export-{}.zip",
                        name.chars()
                            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
                            .collect::<String>(),
                        cl_settings::recent::now().as_utc().format("%Y%m%d-%H%M%S")
                    ));
                    cl_company::export_all(&session, &file)
                        .await
                        .map(Outcome::Export)
                }
                Job::Backup | Job::Portable => cl_company::backup(
                    &session,
                    &store,
                    secrets.as_ref(),
                    &BackupRequest {
                        tenant_id,
                        kind: job.kind(),
                        dest_dir,
                        apply_retention: job == Job::Backup,
                    },
                    &sink,
                )
                .await
                .map(Outcome::Backup),
            }
        })
        .ok();
    }
}

impl Screen for BackupScreen {
    fn title(&self) -> String {
        t(self.job.title_key())
    }

    fn scope(&self) -> Scope {
        Scope::Window
    }

    fn ui(&mut self, ui: &mut Ui) {
        let tokens = Tokens::of(ui.ctx());
        if let Some(t) = self.task.as_mut()
            && let Some(r) = t.poll_result()
        {
            self.task = None;
            match r {
                Ok(Outcome::Backup(b)) => {
                    self.outcome = Some(t_with(
                        "backup.done",
                        &[("path", &b.record.path.display().to_string())],
                    ));
                    self.link.push(CompanyEvent::Toast("backup.toast"));
                }
                Ok(Outcome::Export(e)) => {
                    self.outcome = Some(t_with(
                        "backup.export_done",
                        &[("path", &e.path.display().to_string())],
                    ));
                }
                Err(e) => self.error = Some(e),
            }
        }
        ui.add_space(8.0);
        ui.heading(t(self.job.title_key()));
        form::hint(ui, &tokens, &t(self.job.intro_key()));
        ui.add_space(8.0);
        let Some((_, name)) = self.tenant.clone() else {
            form::hint(ui, &tokens, &t("backup.no_company"));
            return;
        };
        form::card(ui, &tokens, |ui| {
            ui.label(RichText::new(t_with("backup.company", &[("name", &name)])).strong());
            form::field(ui, &t("backup.folder"), &mut self.dest_dir);
            if self.passphrase_needed {
                ui.add_space(6.0);
                form::heading(ui, &tokens, &t("backup.passphrase_title"));
                form::hint(ui, &tokens, &t("backup.passphrase_intro"));
                form::password(ui, &t("backup.passphrase"), &mut self.passphrase);
                form::password(
                    ui,
                    &t("backup.passphrase_confirm"),
                    &mut self.passphrase_confirm,
                );
            }
        });
        if let Some(e) = &self.error {
            form::inline_error(ui, &tokens, e);
        }
        if let Some(o) = &self.outcome {
            ui.add_space(6.0);
            ui.label(o);
        }
        ui.add_space(10.0);
        if self.task.is_some() {
            let text = match self.progress.lock().ok().and_then(|p| p.clone()) {
                Some(Progress::Working {
                    percent: Some(p), ..
                }) => t_with("backup.progress_percent", &[("p", &p.to_string())]),
                Some(Progress::Copying) => t("backup.progress_copy"),
                Some(Progress::Verifying) => t("backup.progress_verify"),
                _ => t("backup.progress"),
            };
            form::progress_line(ui, &tokens, &text);
        } else if form::actions(ui, &t("backup.go"), &[]).is_some() {
            self.error = None;
            self.outcome = None;
            self.start();
        }
    }

    fn command(&mut self, _id: CommandId) -> bool {
        false
    }

    fn state(&self) -> Value {
        json!({
            "screen": match self.job {
                Job::Backup => "backup",
                Job::Portable => "portable_copy",
                Job::ExportAll => "export_all",
            },
            "running": self.task.is_some(),
            "done": self.outcome.is_some(),
            "error": self.error.is_some(),
            "passphrase_needed": self.passphrase_needed,
        })
    }
}
