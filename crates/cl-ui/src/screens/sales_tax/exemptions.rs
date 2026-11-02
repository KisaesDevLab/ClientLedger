//! Tax Exemption Certificates (P23 plan §2.7, §3 WP-3,
//! `clients.tax_exemptions`; U-151, Q-89, Q-P23n): each client's
//! certificates (number, state, expiry, note, the scanned file when one is
//! attached), new and edit, deactivate (never delete), and the report of
//! certificates expired or expiring within 60 days. Invoices warn about a
//! missing or expired certificate; they never refuse a save.
//!
//! Certificate numbers are owner data: never logged, not in the control
//! channel state.

use std::sync::Arc;

use cl_api::{
    ApiError, Capabilities, Contact, ContactFilter, ContactType, ExemptionStatus,
    ExpiringExemption, IdempotencyKey, OrphanLabel, RequestGroup, TaxExemption, TaxExemptionDraft,
};
use cl_core::CalendarDate;
use cl_core::Timestamp;
use cl_core::ids::{ContactId, TaxExemptionId};
use cl_model::forms::sales_tax::exemption::ExemptionForm;
use cl_model::{CommandId, Scope};
use eframe::egui::{self, RichText, Ui};
use serde_json::{Value, json};

use crate::company::CompanyLink;
use crate::i18n::t;
use crate::screens::company::form::{self, DateInput};
use crate::screens::invoice::engine::{self, Job};
use crate::theme::Tokens;
use crate::windows::{OpenPayload, Screen};

/// The command and window kind.
pub const KIND: &str = "clients.tax_exemptions";

/// Days ahead the expiring report looks.
pub const EXPIRING_WITHIN: u32 = 60;

/// The window.
pub struct ExemptionsScreen {
    link: Arc<CompanyLink>,
    group: Option<RequestGroup>,
    available: bool,
    today: CalendarDate,
    clients: Vec<Contact>,
    /// The client shown (`None`: every client).
    pub(crate) client: Option<ContactId>,
    list: Vec<TaxExemption>,
    expiring: Vec<ExpiringExemption>,
    clients_job: Option<Job<Vec<Contact>>>,
    list_job: Option<Job<Vec<TaxExemption>>>,
    expiring_job: Option<Job<Vec<ExpiringExemption>>>,
    save_job: Option<Job<TaxExemption>>,
    key: Option<IdempotencyKey>,
    /// The open editor and the saved version it edits.
    pub(crate) editor: Option<(ExemptionForm, Option<Timestamp>)>,
    pub(crate) expires: DateInput,
    pub(crate) show_inactive: bool,
    problem: Option<&'static str>,
    error: Option<ApiError>,
    saved: u32,
}

impl ExemptionsScreen {
    /// The window over the open company.
    #[must_use]
    pub fn new(link: Arc<CompanyLink>, today: CalendarDate) -> ExemptionsScreen {
        let group = link.group().and_then(Result::ok);
        let available = link.is_demo() || link.capabilities().has(Capabilities::TAX_EXEMPTIONS);
        let mut s = ExemptionsScreen {
            link,
            group,
            available,
            today,
            clients: Vec::new(),
            client: None,
            list: Vec::new(),
            expiring: Vec::new(),
            clients_job: None,
            list_job: None,
            expiring_job: None,
            save_job: None,
            key: None,
            editor: None,
            expires: DateInput {
                text: String::new(),
                value: None,
            },
            show_inactive: false,
            problem: None,
            error: None,
            saved: 0,
        };
        if s.available {
            s.clients_job = Some(engine::read(
                &s.link,
                s.group.as_ref(),
                |d| {
                    Ok(d.contacts_mut()
                        .iter()
                        .filter(|c| {
                            c.is_active
                                && matches!(
                                    c.fields.contact_type,
                                    ContactType::Customer | ContactType::Both
                                )
                        })
                        .cloned()
                        .collect())
                },
                |s| async move {
                    let f = ContactFilter {
                        contact_type: Some(ContactType::Customer),
                        is_active: Some(true),
                        ..ContactFilter::default()
                    };
                    s.contacts().list_all(&f).await
                },
            ));
            s.refresh();
        }
        s
    }

    fn refresh(&mut self) {
        let client = self.client;
        let all = self.show_inactive;
        self.list_job = Some(engine::read(
            &self.link,
            self.group.as_ref(),
            move |d| {
                Ok(crate::company::demo::sales_tax_owed::exemptions(
                    d, client, all,
                ))
            },
            move |s| async move {
                let v = s.sales_tax().exemptions(client).await?;
                Ok(v.into_iter().filter(|e| all || e.is_active).collect())
            },
        ));
        let today = self.today;
        self.expiring_job = Some(engine::read(
            &self.link,
            self.group.as_ref(),
            move |d| {
                Ok(crate::company::demo::sales_tax_owed::expiring(
                    d,
                    today,
                    EXPIRING_WITHIN,
                ))
            },
            move |s| async move { s.sales_tax().exemptions_expiring(today, Some(60)).await },
        ));
        self.poll();
    }

    fn busy(&self) -> bool {
        self.clients_job.is_some()
            || self.list_job.is_some()
            || self.expiring_job.is_some()
            || self.save_job.is_some()
    }

    pub(crate) fn poll(&mut self) {
        if let Some(r) = engine::poll(&mut self.clients_job) {
            self.clients = r.unwrap_or_default();
        }
        if let Some(r) = engine::poll(&mut self.list_job) {
            match r {
                Ok(v) => self.list = v,
                Err(e) => self.error = Some(e),
            }
        }
        if let Some(r) = engine::poll(&mut self.expiring_job) {
            self.expiring = r.unwrap_or_default();
        }
        if let Some(r) = engine::poll(&mut self.save_job) {
            self.key = None;
            match r {
                Ok(_) => {
                    self.saved += 1;
                    self.editor = None;
                    self.problem = None;
                    self.refresh();
                }
                Err(e) => match cl_api::sales_tax_refusal(&e) {
                    Some(cl_api::SalesTaxRefusal::Stale) => {
                        self.problem = Some("sales_tax.refusal.stale");
                    }
                    _ => self.error = Some(e),
                },
            }
        }
    }

    fn name_of(&self, id: ContactId) -> String {
        self.clients
            .iter()
            .find(|c| c.id == id)
            .map(|c| c.fields.display_name.clone())
            .unwrap_or_default()
    }

    /// Shows one client's certificates (`None`: all).
    pub(crate) fn set_client(&mut self, client: Option<ContactId>) {
        self.client = client;
        self.editor = None;
        self.refresh();
    }

    /// Ctrl+N: a new certificate for the client shown.
    pub(crate) fn new_entry(&mut self) {
        self.problem = None;
        self.expires = DateInput {
            text: String::new(),
            value: None,
        };
        self.editor = Some((
            ExemptionForm {
                client: self.client,
                ..ExemptionForm::default()
            },
            None,
        ));
    }

    fn open(&mut self, id: TaxExemptionId) {
        let Some(e) = self.list.iter().find(|e| e.id == id) else {
            return;
        };
        self.problem = None;
        self.expires = e.expires_on.map_or(
            DateInput {
                text: String::new(),
                value: None,
            },
            DateInput::new,
        );
        self.editor = Some((
            ExemptionForm {
                id: Some(e.id),
                client: Some(e.contact_id),
                number: e.number.clone(),
                state: e.state.clone().unwrap_or_default(),
                expires_on: e.expires_on,
                note: e.note.clone().unwrap_or_default(),
            },
            e.updated_at,
        ));
    }

    pub(crate) fn save(&mut self) {
        let Some((f, version)) = self.editor.clone() else {
            return;
        };
        let plan = match f.plan() {
            Ok(p) => p,
            Err(p) => {
                self.problem = Some(p.key().0);
                return;
            }
        };
        let attachment =
            f.id.and_then(|id| self.list.iter().find(|e| e.id == id))
                .and_then(|e| e.attachment_id);
        let draft = TaxExemptionDraft {
            contact_id: plan.client,
            number: plan.number,
            state: plan.state,
            expires_on: plan.expires_on,
            attachment_id: attachment,
            note: plan.note,
        };
        let d2 = draft.clone();
        let id = f.id;
        let key = self.key.get_or_insert_with(IdempotencyKey::new).clone();
        let label = OrphanLabel {
            what: "orphan.sales_tax_exemption",
            key: Some(key.clone()),
        };
        self.save_job = Some(engine::write(
            &self.link,
            self.group.as_ref(),
            label,
            move |d| crate::company::demo::sales_tax_owed::save_exemption(d, id, &d2, version),
            move |s| async move {
                let st = s.sales_tax();
                match id {
                    Some(id) => st.update_exemption(id, &draft, version, key).await,
                    None => st.create_exemption(&draft, key).await,
                }
            },
        ));
        self.problem = None;
        self.poll();
    }

    pub(crate) fn deactivate(&mut self) {
        let Some(id) = self.editor.as_ref().and_then(|(f, _)| f.id) else {
            return;
        };
        let key = IdempotencyKey::new();
        let label = OrphanLabel {
            what: "orphan.sales_tax_exemption",
            key: Some(key.clone()),
        };
        self.save_job = Some(engine::write(
            &self.link,
            self.group.as_ref(),
            label,
            move |d| crate::company::demo::sales_tax_owed::deactivate_exemption(d, id),
            move |s| async move { s.sales_tax().deactivate_exemption(id, key).await },
        ));
        self.poll();
    }

    // ---- Drawing ----------------------------------------------------------------

    fn draw(&mut self, ui: &mut Ui) {
        let tokens = Tokens::of(ui.ctx());
        ui.label(RichText::new(t("tax_exemptions.title")).size(22.0).strong());
        if !self.available {
            form::hint(ui, &tokens, &t("tax_exemptions.needs_update"));
            return;
        }
        if let Some(e) = &self.error {
            form::inline_error(ui, &tokens, e);
        }
        form::hint(ui, &tokens, &t("tax_exemptions.intro"));
        ui.horizontal(|ui| {
            let l = ui.label(t("tax_exemptions.client"));
            let shown = self
                .client
                .map_or_else(|| t("tax_exemptions.all_clients"), |c| self.name_of(c));
            let mut picked = None;
            egui::ComboBox::from_id_salt("tax_exemptions_client")
                .selected_text(shown)
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(self.client.is_none(), t("tax_exemptions.all_clients"))
                        .clicked()
                    {
                        picked = Some(None);
                    }
                    for c in &self.clients {
                        if ui
                            .selectable_label(self.client == Some(c.id), &c.fields.display_name)
                            .clicked()
                        {
                            picked = Some(Some(c.id));
                        }
                    }
                })
                .response
                .labelled_by(l.id);
            if let Some(c) = picked {
                self.set_client(c);
            }
            if ui.button(t("tax_exemptions.new")).clicked() {
                self.new_entry();
            }
            let mut all = self.show_inactive;
            if ui
                .checkbox(&mut all, t("tax_exemptions.show_inactive"))
                .changed()
            {
                self.show_inactive = all;
                self.refresh();
            }
        });
        ui.add_space(6.0);
        if self.list.is_empty() {
            form::hint(ui, &tokens, &t("tax_exemptions.empty"));
        }
        let mut open = None;
        egui::Grid::new("tax_exemptions_list")
            .striped(true)
            .spacing([14.0, 4.0])
            .show(ui, |ui| {
                for e in &self.list {
                    if ui.link(&e.number).clicked() {
                        open = Some(e.id);
                    }
                    ui.label(self.name_of(e.contact_id));
                    ui.label(e.state.clone().unwrap_or_default());
                    ui.label(
                        e.expires_on
                            .map_or_else(|| t("tax_exemptions.never"), crate::shell::us_date),
                    );
                    ui.label(if e.attachment_id.is_some() {
                        t("tax_exemptions.attached")
                    } else {
                        String::new()
                    });
                    ui.label(if e.is_active {
                        String::new()
                    } else {
                        t("tax_exemptions.inactive")
                    });
                    ui.end_row();
                }
            });
        if let Some(id) = open {
            self.open(id);
        }
        ui.add_space(8.0);
        self.editor_ui(ui, &tokens);
        ui.add_space(10.0);
        self.expiring_ui(ui, &tokens);
    }

    fn editor_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let names: Vec<(ContactId, String)> = self
            .clients
            .iter()
            .map(|c| (c.id, c.fields.display_name.clone()))
            .collect();
        let today = self.today;
        let problem = self.problem;
        let mut save = false;
        let mut cancel = false;
        let mut deactivate = false;
        let expires = &mut self.expires;
        let Some((f, _)) = self.editor.as_mut() else {
            return;
        };
        form::card(ui, tokens, |ui| {
            ui.label(
                RichText::new(t(if f.id.is_some() {
                    "tax_exemptions.edit"
                } else {
                    "tax_exemptions.new_title"
                }))
                .strong(),
            );
            ui.horizontal(|ui| {
                let l = ui.label(t("tax_exemptions.client"));
                let shown = f
                    .client
                    .and_then(|c| names.iter().find(|n| n.0 == c))
                    .map(|n| n.1.clone())
                    .unwrap_or_default();
                egui::ComboBox::from_id_salt("tax_exemptions_editor_client")
                    .selected_text(shown)
                    .show_ui(ui, |ui| {
                        for (id, n) in &names {
                            ui.selectable_value(&mut f.client, Some(*id), n);
                        }
                    })
                    .response
                    .labelled_by(l.id);
            });
            form::field(ui, &t("tax_exemptions.number"), &mut f.number);
            form::field(ui, &t("tax_exemptions.state"), &mut f.state)
                .on_hover_text(t("tax_exemptions.state_tip"));
            if form::date_field(ui, &t("tax_exemptions.expires"), expires, today).changed() {
                f.expires_on = expires.value;
            }
            form::hint(ui, tokens, &t("tax_exemptions.expires_tip"));
            form::field(ui, &t("tax_exemptions.note"), &mut f.note);
            if let Some(p) = problem {
                form::inline_warning(ui, tokens, &t(p));
            }
            ui.horizontal(|ui| {
                save = ui.button(t("tax_exemptions.save")).clicked();
                if f.id.is_some() {
                    deactivate = ui.button(t("tax_exemptions.deactivate")).clicked();
                }
                cancel = ui.button(t("tax_exemptions.cancel")).clicked();
            });
        });
        if save {
            self.save();
        }
        if deactivate {
            self.deactivate();
        }
        if cancel {
            self.editor = None;
            self.problem = None;
        }
    }

    fn expiring_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        ui.label(RichText::new(t("tax_exemptions.expiring")).strong());
        if self.expiring.is_empty() {
            form::hint(ui, tokens, &t("tax_exemptions.expiring.none"));
            return;
        }
        egui::Grid::new("tax_exemptions_expiring")
            .striped(true)
            .spacing([14.0, 4.0])
            .show(ui, |ui| {
                for e in &self.expiring {
                    ui.label(&e.contact_name);
                    ui.label(&e.number);
                    ui.label(crate::shell::us_date(e.expires_on));
                    ui.label(t(match e.status {
                        ExemptionStatus::Expired => "tax_exemptions.status.expired",
                        _ => "tax_exemptions.status.expiring",
                    }));
                    ui.end_row();
                }
            });
    }

    fn state_json(&self) -> Value {
        json!({
            "screen": "tax_exemptions",
            "available": self.available,
            "client": self.client.is_some(),
            "certificates": self.list.len(),
            "inactive": self.list.iter().filter(|e| !e.is_active).count(),
            "expiring": self.expiring.len(),
            "expired": self.expiring.iter().filter(|e| e.status == ExemptionStatus::Expired).count(),
            "editing": self.editor.as_ref().map(|(f, _)| json!({"new": f.id.is_none()})),
            "problem": self.problem,
            "saved": self.saved,
            "error": self.error.is_some(),
        })
    }
}

impl Screen for ExemptionsScreen {
    fn title(&self) -> String {
        t("tax_exemptions.title")
    }

    fn ui(&mut self, ui: &mut Ui) {
        self.poll();
        egui::ScrollArea::vertical().show(ui, |ui| self.draw(ui));
        if self.busy() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    fn scope(&self) -> Scope {
        Scope::Window
    }

    fn command(&mut self, id: CommandId) -> bool {
        match id.as_str() {
            KIND => true,
            "list.new" => {
                self.new_entry();
                true
            }
            "list.toggle_active" => {
                self.deactivate();
                true
            }
            _ => false,
        }
    }

    fn open_with(&mut self, payload: OpenPayload) -> bool {
        match payload {
            OpenPayload::ForClient(c) => {
                self.set_client(Some(c));
                true
            }
            _ => false,
        }
    }

    fn state(&self) -> Value {
        self.state_json()
    }
}

#[cfg(test)]
#[path = "exemptions_tests.rs"]
mod tests;
