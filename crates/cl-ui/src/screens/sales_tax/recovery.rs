//! Recover a Bad Debt (P23 plan §2.8, §3 WP-3, `clients.recover_bad_debt`;
//! U-154, Q-P23q): part of a written-off invoice comes back. The owner
//! picks the customer and the invoice, the date and the amount; the engine
//! reopens that much of the invoice and works out the sales tax owed again
//! (SSUTA, accrual). "Receive the payment now" then opens Receive Payment
//! for the customer. A 409 `RECOVERY_EXCEEDS_WRITE_OFF` offers what is left
//! (the engine's figure, copied). One key per save attempt, reused through
//! the closing-date password.

use std::sync::Arc;

use cl_api::{
    ApiError, Capabilities, ClosedPeriodAuthorization, Contact, ContactFilter, ContactType,
    IdempotencyKey, NewRecovery, OrphanLabel, PostingOptions, RecoveryRefusal, RecoveryResult,
    RequestGroup,
};
use cl_core::ids::{ContactId, TransactionId};
use cl_core::{CalendarDate, Money, MoneyStyle, Secret};
use cl_model::forms::sales_tax::recovery::{RecoveryForm, RecoveryPlan};
use cl_model::{CommandId, Scope};
use eframe::egui::{self, RichText, Ui};
use serde_json::{Value, json};

use crate::company::CompanyLink;
use crate::forms::closing_prompt::{ClosingClick, ClosingPrompt};
use crate::i18n::{t, t_with};
use crate::screens::company::form::{self, DateInput};
use crate::screens::invoice::engine::{self, Job};
use crate::theme::Tokens;
use crate::windows::{OpenPayload, Screen};

/// The command and window kind.
pub const KIND: &str = "clients.recover_bad_debt";

/// A written-off invoice: (invoice, number, written off, left to recover).
type Recoverable = (TransactionId, String, Money, Money);

fn fmt(m: Money) -> String {
    m.format(MoneyStyle::default())
}

/// The Recover a Bad Debt window.
pub struct RecoveryScreen {
    link: Arc<CompanyLink>,
    group: Option<RequestGroup>,
    available: bool,
    today: CalendarDate,
    clients: Vec<Contact>,
    pub(crate) client: Option<ContactId>,
    invoices: Vec<Recoverable>,
    pub(crate) form: RecoveryForm,
    pub(crate) date: DateInput,
    amount: String,
    clients_job: Option<Job<Vec<Contact>>>,
    invoices_job: Option<Job<Vec<Recoverable>>>,
    save_job: Option<Job<RecoveryResult>>,
    key: Option<(u64, IdempotencyKey)>,
    pending: Option<RecoveryPlan>,
    pub(crate) closing_prompt: ClosingPrompt<RecoveryPlan>,
    pub(crate) done: Option<RecoveryResult>,
    problem: Option<String>,
    error: Option<ApiError>,
}

impl RecoveryScreen {
    /// The window over the open company, dated today.
    #[must_use]
    pub fn new(link: Arc<CompanyLink>, today: CalendarDate) -> RecoveryScreen {
        let group = link.group().and_then(Result::ok);
        let available = link.is_demo() || link.capabilities().has(Capabilities::BAD_DEBT_RECOVERY);
        let mut s = RecoveryScreen {
            link,
            group,
            available,
            today,
            clients: Vec::new(),
            client: None,
            invoices: Vec::new(),
            form: {
                let mut f = RecoveryForm::default();
                f.date = Some(today);
                f
            },
            date: DateInput::new(today),
            amount: String::new(),
            clients_job: None,
            invoices_job: None,
            save_job: None,
            key: None,
            pending: None,
            closing_prompt: ClosingPrompt::default(),
            done: None,
            problem: None,
            error: None,
        };
        if s.available {
            s.clients_job = Some(engine::read(
                &s.link,
                s.group.as_ref(),
                |d| {
                    Ok(d.contacts_mut()
                        .iter()
                        .filter(|c| {
                            matches!(
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
                        ..ContactFilter::default()
                    };
                    s.contacts().list_all(&f).await
                },
            ));
            s.read_invoices();
        }
        s
    }

    /// The written-off invoices with something left to recover (`GET
    /// /payments/write-offs`, engine PR #221; the engine's figures).
    fn read_invoices(&mut self) {
        let client = self.client;
        self.invoices_job = Some(engine::read(
            &self.link,
            self.group.as_ref(),
            move |d| Ok(crate::company::demo::sales_tax_owed::recoverable(d, client)),
            move |s| async move {
                Ok(s.recoveries()
                    .write_offs(client, false)
                    .await?
                    .into_iter()
                    .filter(|w| w.recoverable.is_positive())
                    .map(|w| {
                        (
                            w.invoice_id,
                            w.invoice_number.unwrap_or_default(),
                            w.written_off,
                            w.recoverable,
                        )
                    })
                    .collect())
            },
        ));
        self.poll();
    }

    fn busy(&self) -> bool {
        self.clients_job.is_some() || self.invoices_job.is_some() || self.save_job.is_some()
    }

    pub(crate) fn poll(&mut self) {
        if let Some(r) = engine::poll(&mut self.clients_job) {
            self.clients = r.unwrap_or_default();
        }
        if let Some(r) = engine::poll(&mut self.invoices_job) {
            self.invoices = r.unwrap_or_default();
            if self.form.invoice.is_none()
                && let Some(first) = self.invoices.first()
            {
                self.form.invoice = Some(first.0);
            }
        }
        if let Some(r) = engine::poll(&mut self.save_job) {
            match r {
                Ok(done) => {
                    self.closing_prompt.saved();
                    self.key = None;
                    self.pending = None;
                    self.done = Some(done);
                    self.problem = None;
                    crate::forms::publish_saved(&self.link, &self.link.bus().subscribe(), &[]);
                    self.read_invoices();
                }
                Err(e) => self.failed(e),
            }
        }
    }

    fn failed(&mut self, e: ApiError) {
        let supported =
            self.link.is_demo() || self.link.capabilities().has(Capabilities::CLOSING_PASSWORD);
        if let Some(p) = self.pending.take()
            && self.closing_prompt.failed(&e, p, supported, None)
        {
            return;
        }
        self.key = None;
        match cl_api::recovery_refusal(&e) {
            Some(RecoveryRefusal::ExceedsWriteOff) => {
                if let Some(a) = cl_api::recovery_available(&e) {
                    self.form.apply_available(a);
                    self.amount = self.form.amount_text().to_string();
                    self.problem = Some(t_with(
                        "recover_bad_debt.exceeds",
                        &[("available", &fmt(a))],
                    ));
                }
            }
            Some(RecoveryRefusal::DateInvalid) => {
                self.problem = Some(t("recover_bad_debt.date_invalid"));
            }
            Some(_) => self.problem = Some(t("recover_bad_debt.in_use")),
            None => self.error = Some(e),
        }
    }

    pub(crate) fn set_client(&mut self, client: Option<ContactId>) {
        self.client = client;
        self.form.invoice = None;
        self.done = None;
        self.read_invoices();
    }

    pub(crate) fn set_amount(&mut self, text: &str) {
        self.amount = text.to_string();
        self.form.set_amount(text);
    }

    /// Records the recovery.
    pub(crate) fn save(&mut self) {
        self.form.date = self.date.value;
        match self.form.plan() {
            Ok(p) => self.send(p, None),
            Err(p) => self.problem = Some(t(p.key().0)),
        }
    }

    fn send(&mut self, plan: RecoveryPlan, password: Option<Secret>) {
        if self.save_job.is_some() {
            return;
        }
        let key = match &self.key {
            Some((a, k)) if *a == plan.attempt => k.clone(),
            _ => {
                let k = IdempotencyKey::new();
                self.key = Some((plan.attempt, k.clone()));
                k
            }
        };
        let r = NewRecovery {
            invoice_id: plan.invoice,
            date: plan.date,
            amount: plan.amount,
            memo: plan.memo.clone(),
        };
        let live = r.clone();
        let demo_key = key.as_header();
        let opts = PostingOptions {
            closed_period: password
                .clone()
                .map(ClosedPeriodAuthorization::with_password),
        };
        let label = OrphanLabel {
            what: "orphan.recover_bad_debt",
            key: Some(key.clone()),
        };
        self.pending = Some(plan);
        self.problem = None;
        self.save_job = Some(engine::write(
            &self.link,
            self.group.as_ref(),
            label,
            move |d| {
                crate::company::demo::sales_tax_owed::recover(
                    d,
                    &r,
                    password.as_ref(),
                    Some(&demo_key),
                )
            },
            move |s| async move { s.recoveries().create(&live, &opts, key).await },
        ));
        self.poll();
    }

    fn submit_closing(&mut self) {
        if let Some((p, secret)) = self.closing_prompt.submit() {
            self.send(p, Some(secret));
        }
    }

    /// "Receive the payment now": Receive Payment for the customer.
    pub(crate) fn receive_now(&self) {
        if let Some(c) = self.client {
            self.link.push(crate::company::CompanyEvent::OpenWith(
                CommandId::new("clients.receive_payment"),
                Box::new(OpenPayload::ReceiveFrom(c)),
            ));
        }
    }

    // ---- Drawing ----------------------------------------------------------------

    fn draw(&mut self, ui: &mut Ui) {
        let tokens = Tokens::of(ui.ctx());
        match self
            .closing_prompt
            .window(ui.ctx(), &tokens, "recover_bad_debt")
        {
            ClosingClick::Submit => self.submit_closing(),
            ClosingClick::Cancel => self.closing_prompt.cancel(),
            ClosingClick::None => {}
        }
        ui.label(
            RichText::new(t("recover_bad_debt.title"))
                .size(22.0)
                .strong(),
        );
        if !self.available {
            form::hint(ui, &tokens, &t("recover_bad_debt.needs_update"));
            return;
        }
        form::hint(ui, &tokens, &t("recover_bad_debt.intro"));
        if let Some(e) = &self.error {
            form::inline_error(ui, &tokens, e);
        }
        ui.horizontal(|ui| {
            let l = ui.label(t("recover_bad_debt.client"));
            let shown = self
                .client
                .and_then(|c| self.clients.iter().find(|x| x.id == c))
                .map(|c| c.fields.display_name.clone())
                .unwrap_or_default();
            let mut picked = None;
            egui::ComboBox::from_id_salt("recover_bad_debt_client")
                .selected_text(shown)
                .show_ui(ui, |ui| {
                    for c in &self.clients {
                        if ui
                            .selectable_label(self.client == Some(c.id), &c.fields.display_name)
                            .clicked()
                        {
                            picked = Some(c.id);
                        }
                    }
                })
                .response
                .labelled_by(l.id);
            if let Some(c) = picked {
                self.set_client(Some(c));
            }
        });
        if self.invoices.is_empty() {
            form::hint(ui, &tokens, &t("recover_bad_debt.none"));
        }
        ui.label(t("recover_bad_debt.invoice"));
        let mut picked = None;
        for (id, number, w, left) in &self.invoices {
            let text = format!(
                "{number} \u{b7} {}",
                t_with(
                    "recover_bad_debt.left",
                    &[("written_off", &fmt(*w)), ("left", &fmt(*left))]
                )
            );
            if ui.radio(self.form.invoice == Some(*id), text).clicked() {
                picked = Some(*id);
            }
        }
        if let Some(i) = picked {
            self.form.invoice = Some(i);
        }
        form::date_field(ui, &t("recover_bad_debt.date"), &mut self.date, self.today);
        let mut amount = self.amount.clone();
        let _ = form::money_field(
            ui,
            &t("recover_bad_debt.amount"),
            &mut amount,
            MoneyStyle::default(),
        );
        if amount != self.amount {
            self.set_amount(&amount);
        }
        form::field(ui, &t("recover_bad_debt.memo"), &mut self.form.memo);
        if let Some(p) = &self.problem {
            form::inline_warning(ui, &tokens, p);
        }
        ui.horizontal(|ui| {
            let enabled = self.save_job.is_none() && !self.closing_prompt.dialog_open();
            if ui
                .add_enabled(enabled, egui::Button::new(t("recover_bad_debt.save")))
                .clicked()
            {
                self.save();
            }
        });
        if let Some(d) = &self.done {
            let tax = d
                .recovered_tax
                .filter(|m| !m.is_zero())
                .map(|m| t_with("recover_bad_debt.saved_tax", &[("tax", &fmt(m))]))
                .unwrap_or_default();
            form::hint(
                ui,
                &tokens,
                &t_with(
                    "recover_bad_debt.saved",
                    &[("balance", &fmt(d.invoice_balance_due)), ("tax", &tax)],
                ),
            );
            if ui.button(t("recover_bad_debt.receive")).clicked() {
                self.receive_now();
            }
        }
    }

    fn state_json(&self) -> Value {
        json!({
            "screen": "recover_bad_debt",
            "available": self.available,
            "client": self.client.is_some(),
            "invoices": self.invoices.len(),
            "invoice": self.form.invoice.is_some(),
            "done": self.done.as_ref().map(|d| json!({
                "balance": d.invoice_balance_due.to_wire(),
                "tax": d.recovered_tax.map(Money::to_wire),
            })),
            "closing_dialog": self.closing_prompt.dialog_open(),
            "problem": self.problem.is_some(),
            "error": self.error.is_some(),
        })
    }
}

impl Screen for RecoveryScreen {
    fn title(&self) -> String {
        t("recover_bad_debt.title")
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
        Scope::Form
    }

    fn command(&mut self, id: CommandId) -> bool {
        if let Some(h) = self.closing_prompt.command(id) {
            return h;
        }
        match id.as_str() {
            KIND => true,
            "form.save_close" | "form.save_new" => {
                self.save();
                true
            }
            // Demo only: golden 13's written-off invoice (the scenario).
            "dev.demo_write_off_story" => {
                let Some(mut d) = self.link.demo_state() else {
                    return false;
                };
                // Once per demo session: the dark run reuses the light run's.
                if crate::company::demo::sales_tax_owed::recoverable(&d, None).is_empty() {
                    let _ = crate::company::demo::sales_tax_owed::write_off_story(&mut d);
                }
                drop(d);
                self.set_client(None);
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
#[path = "recovery_tests.rs"]
mod tests;
