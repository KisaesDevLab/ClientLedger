//! Delete Company (P08 step 6, Q-185): Admin only, the name typed exactly,
//! a verified backup taken first. Needs the engine's safe delete (E5b,
//! `tenant-delete-v1`); until then the window explains that and the
//! button stays off.

use std::sync::Arc;

use cl_api::Capabilities;
use cl_company::{CompanyError, DeleteOutcome, DeleteRequest, delete::name_matches};
use cl_core::Secret;
use cl_core::ids::TenantId;
use cl_model::{CommandId, Scope};
use eframe::egui::{RichText, Ui};
use serde_json::{Value, json};

use super::form;
use crate::company::{CompanyEvent, CompanyLink, Task, spawn_lifecycle};
use crate::i18n::{t, t_with};
use crate::theme::Tokens;
use crate::windows::Screen;

/// The Delete Company window.
pub struct DeleteCompanyScreen {
    link: Arc<CompanyLink>,
    group: Option<cl_api::RequestGroup>,
    tenant: Option<(TenantId, String)>,
    typed: String,
    closing_password: String,
    available: bool,
    task: Option<Task<Result<DeleteOutcome, CompanyError>>>,
    done: Option<String>,
    error: Option<CompanyError>,
}

impl DeleteCompanyScreen {
    /// A window for the open company.
    #[must_use]
    pub fn new(link: Arc<CompanyLink>, tenant: Option<(TenantId, String)>) -> DeleteCompanyScreen {
        let group = link.group().and_then(Result::ok);
        let available = link.is_demo() || link.capabilities().has(Capabilities::TENANT_DELETE);
        DeleteCompanyScreen {
            link,
            group,
            tenant,
            typed: String::new(),
            closing_password: String::new(),
            available,
            task: None,
            done: None,
            error: None,
        }
    }

    fn matches(&self) -> bool {
        self.tenant
            .as_ref()
            .is_some_and(|(_, name)| name_matches(&self.typed, name))
    }

    fn start(&mut self) {
        let Some((tenant_id, name)) = self.tenant.clone() else {
            return;
        };
        if !self.matches() {
            self.error = Some(CompanyError::NameMismatch);
            return;
        }
        if let Some(mut demo) = self.link.demo_state() {
            demo.companies.retain(|c| c.tenant_id != tenant_id);
            demo.open = None;
            drop(demo);
            self.done = Some(t_with("delete.done", &[("name", &name)]));
            self.link.push(CompanyEvent::Closed);
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        let store = self.link.store().clone();
        let secrets = self.link.secrets();
        let req = DeleteRequest {
            tenant_id,
            typed_name: self.typed.trim().to_string(),
            backup_dir: self.link.backup_dir.clone(),
            closing_password: (!self.closing_password.is_empty())
                .then(|| Secret::new(std::mem::take(&mut self.closing_password))),
        };
        self.task = spawn_lifecycle(&self.link, group, move || async move {
            cl_company::delete_company(
                &session,
                &store,
                secrets.as_ref(),
                &req,
                &cl_company::backup::silent(),
            )
            .await
        })
        .ok();
    }
}

impl Screen for DeleteCompanyScreen {
    fn title(&self) -> String {
        t("delete.tab")
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
                Ok(out) => {
                    self.done = Some(t_with(
                        "delete.done_with_backup",
                        &[("path", &out.backup.path.display().to_string())],
                    ));
                    self.link.push(CompanyEvent::Closed);
                }
                Err(e) => self.error = Some(e),
            }
        }
        ui.add_space(8.0);
        ui.heading(t("delete.title"));
        let Some((_, name)) = self.tenant.clone() else {
            form::hint(ui, &tokens, &t("backup.no_company"));
            return;
        };
        form::hint(ui, &tokens, &t_with("delete.intro", &[("name", &name)]));
        ui.add_space(8.0);
        if let Some(d) = &self.done {
            ui.label(d);
            return;
        }
        form::card(ui, &tokens, |ui| {
            ui.label(RichText::new(t("delete.warning")).color(tokens.warn_text));
            ui.add_space(6.0);
            form::field(ui, &t("delete.type_name"), &mut self.typed);
            form::password(
                ui,
                &t("delete.closing_password"),
                &mut self.closing_password,
            );
            form::hint(ui, &tokens, &t("delete.closing_hint"));
            if !self.available {
                ui.add_space(6.0);
                form::hint(ui, &tokens, &t("delete.unavailable"));
            }
        });
        if let Some(e) = &self.error {
            form::inline_error(ui, &tokens, e);
        }
        ui.add_space(10.0);
        if self.task.is_some() {
            form::progress_line(ui, &tokens, &t("delete.working"));
            return;
        }
        let enabled = self.available && self.matches();
        ui.add_enabled_ui(enabled, |ui| {
            if form::actions(ui, &t("delete.go"), &[]).is_some() {
                self.start();
            }
        });
    }

    fn command(&mut self, _id: CommandId) -> bool {
        false
    }

    fn state(&self) -> Value {
        json!({
            "screen": "delete_company",
            "available": self.available,
            "name_matches": self.matches(),
            "running": self.task.is_some(),
            "done": self.done.is_some(),
        })
    }
}
