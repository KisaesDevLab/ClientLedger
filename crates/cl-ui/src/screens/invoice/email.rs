//! Email an invoice (P20 WP-B step 4, Ctrl+Shift+M; Q-P20d/e/j/q/aa).
//!
//! The dialog fills To with the client's email (one address until U-36,
//! Q-P20r), BCC with the From address when the company keeps BCC-self on
//! (Q-P20j), and the subject and body from the company's template
//! (`cl_settings::fill_template`; `{total}` copies the engine's figure).
//! Send makes the PDF (`cl_print`, `Invoice-<number>.pdf`, at most 10 MB)
//! and posts it through the engine, which sends with the company SMTP and
//! records the "Email" send. If it can't be sent the invoice stays saved
//! and the dialog says "Saved; not sent" and offers the Outbox or a PDF
//! (Save PDF counts as sent only with Mark as sent, Q-P20e). Logs name
//! neither recipients nor text.

use std::sync::Arc;

use cl_api::{
    EmailMessage, IdempotencyKey, InvoiceRecord, InvoiceRefusal, OutboxItem, SendMethod,
    SendRecord, invoice_pdf_name, invoice_refusal,
};
use cl_core::MoneyStyle;
use cl_core::ids::TransactionId;
use cl_model::forms::SaveIntent;
use cl_settings::{FieldValues, TemplateDoc, TemplateSource, fill_template};
use eframe::egui::{self, RichText};
use rust_decimal::Decimal;
use serde_json::{Value, json};

use super::InvoiceScreen;
use super::engine::{self, Job, label};
use crate::company::CompanyEvent;
use crate::i18n::t;
use crate::theme::Tokens;

/// What a finished email flow did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// Sent now.
    Sent,
    /// Waiting in the Outbox.
    Queued,
    /// Opened as a PDF (marked sent or not).
    Pdf,
    /// Closed without sending.
    Cancelled,
}

/// The email dialog.
#[derive(Default)]
pub(crate) struct EmailFlow {
    open: bool,
    invoice: Option<InvoiceRecord>,
    pub to: String,
    pub cc: String,
    pub bcc: String,
    pub subject: String,
    pub body: String,
    mark_sent: bool,
    /// What the save asked for, run when the dialog closes.
    then: Option<SaveIntent>,
    job: Option<Job<Outcome>>,
    from_job: Option<Job<String>>,
    /// Sending failed: offer the Outbox or a PDF.
    failed: bool,
    problem: Option<&'static str>,
    unknown_fields: usize,
    done: Option<(Outcome, Option<SaveIntent>)>,
    last: Option<Outcome>,
}

fn list(s: &str) -> Vec<String> {
    s.split([',', ';'])
        .map(str::trim)
        .filter(|a| !a.is_empty())
        .map(str::to_string)
        .collect()
}

impl EmailFlow {
    pub(crate) fn is_open(&self) -> bool {
        self.open
    }

    pub(crate) fn busy(&self) -> bool {
        self.job.is_some() || self.from_job.is_some()
    }

    pub(crate) fn cancel(&mut self) {
        if self.open {
            self.open = false;
            self.done = Some((Outcome::Cancelled, self.then.take()));
        }
    }

    pub(crate) fn poll(&mut self) {
        if let Some(Ok(from)) = engine::poll(&mut self.from_job)
            && self.bcc.trim().is_empty()
            && from.contains('@')
        {
            self.bcc = from;
        }
        if let Some(r) = engine::poll(&mut self.job) {
            match r {
                Ok(o) => {
                    self.open = false;
                    self.failed = false;
                    self.last = Some(o);
                    self.done = Some((o, self.then.take()));
                }
                Err(e) => {
                    self.failed = true;
                    self.problem = Some(match invoice_refusal(&e) {
                        Some(InvoiceRefusal::SmtpNotConfigured) => "invoice.email.no_smtp",
                        _ => "invoice.email.not_sent",
                    });
                }
            }
        }
    }

    pub(crate) fn take_done(&mut self) -> Option<(Outcome, Option<SaveIntent>)> {
        self.done.take()
    }

    pub(crate) fn state_json(&self) -> Value {
        json!({
            "open": self.open,
            "recipients": list(&self.to).len(),
            "subject_filled": !self.subject.contains('{'),
            "unknown_fields": self.unknown_fields,
            "failed": self.failed,
            "problem": self.problem,
            "sending": self.job.is_some(),
            "last": self.last.map(|o| format!("{o:?}").to_lowercase()),
        })
    }

    fn message(&self) -> EmailMessage {
        EmailMessage {
            to: list(&self.to),
            cc: list(&self.cc),
            bcc: list(&self.bcc),
            subject: self.subject.clone(),
            body: self.body.clone(),
        }
    }
}

/// The saved invoice as the printed form (every figure the engine's).
pub(crate) fn print_data(
    r: &InvoiceRecord,
    client: Option<&cl_api::Contact>,
    terms: &str,
) -> cl_print::forms::invoice::InvoiceData {
    let dec = |m: Option<cl_core::Money>| m.map(cl_core::Money::as_decimal);
    let bill_to = client.map_or_else(Vec::new, |c| {
        let f = &c.fields;
        let city = [
            f.billing_city.as_deref(),
            f.billing_state.as_deref(),
            f.billing_zip.as_deref(),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ");
        [
            Some(f.display_name.clone()),
            f.billing_line1.clone(),
            f.billing_line2.clone(),
            Some(city),
        ]
        .into_iter()
        .flatten()
        .filter(|l| !l.trim().is_empty())
        .collect()
    });
    let ship_to = client.map_or_else(Vec::new, |c| {
        let f = &c.fields;
        [
            f.shipping_line1.clone(),
            f.shipping_line2.clone(),
            f.shipping_city.clone(),
        ]
        .into_iter()
        .flatten()
        .collect()
    });
    let total = r
        .txn
        .total
        .map_or(Decimal::ZERO, cl_core::Money::as_decimal);
    cl_print::forms::invoice::InvoiceData {
        number: r.txn.txn_number.clone().unwrap_or_default(),
        date: crate::shell::us_date(r.txn.txn_date),
        due_date: r.due_date.map(crate::shell::us_date).unwrap_or_default(),
        terms: terms.to_string(),
        po_number: r.po_number.clone().unwrap_or_default(),
        bill_to,
        ship_to,
        lines: r
            .lines
            .iter()
            .map(|l| cl_print::forms::ItemLine {
                description: l.description.clone().unwrap_or_default(),
                quantity: Some(l.quantity.as_decimal()),
                rate: dec(l.unit_price),
                amount: Some(l.amount.as_decimal()),
                service_date: l
                    .service_date
                    .map(crate::shell::us_date)
                    .unwrap_or_default(),
            })
            .collect(),
        hide_zero_lines: r.hide_zero_lines,
        subtotal: dec(r.subtotal).unwrap_or(total),
        tax: dec(r.tax_amount),
        total,
        payments: dec(r.amount_paid).filter(|p| !p.is_zero()),
        balance_due: dec(r.balance_due).unwrap_or(total),
        message: r.customer_message.clone().unwrap_or_default(),
    }
}

impl InvoiceScreen {
    fn company_name(&self) -> String {
        self.tenant
            .as_ref()
            .map(|(_, n)| n.clone())
            .unwrap_or_default()
    }

    /// The PDF of the open saved invoice.
    pub(crate) fn pdf(&self) -> Option<(String, Vec<u8>)> {
        let e = self.editing.as_ref()?;
        let client = e
            .record
            .txn
            .contact_id
            .and_then(|c| self.books.contacts.iter().find(|x| x.id == c));
        let terms = e
            .record
            .payment_terms
            .as_deref()
            .map(|c| t(&format!("invoice.terms.{c}")))
            .unwrap_or_default();
        let data = print_data(&e.record, client, &terms);
        let style = cl_print::FormStyle {
            company: crate::screens::print_forms::job::company(&self.company_name(), Vec::new()),
            logo: None,
            footer: String::new(),
            labels: crate::screens::print_forms::job::form_labels(),
        };
        let bytes = cl_print::forms::invoice::pdf_bytes(&data, &style).ok()?;
        Some((invoice_pdf_name(&data.number), bytes))
    }

    /// Email… (Ctrl+Shift+M): the saved invoice only; `then` runs when the
    /// dialog closes (Email a PDF after saving).
    pub(crate) fn open_email(&mut self, then: Option<SaveIntent>) {
        let Some(e) = self.editing.clone() else {
            self.notice = Some("invoice.email.save_first");
            if let Some(i) = then {
                self.follow_up(i);
            }
            return;
        };
        if self.is_dirty() {
            self.notice = Some("invoice.email.save_first");
            return;
        }
        if e.void() {
            self.notice = Some("invoice.read_only.void");
            return;
        }
        let prefs = self
            .tenant
            .as_ref()
            .and_then(|(id, _)| {
                self.link
                    .store()
                    .load()
                    .ok()
                    .and_then(|l| l.get(*id).map(|r| r.prefs.email.clone()))
            })
            .unwrap_or_default();
        let client = e
            .record
            .txn
            .contact_id
            .and_then(|c| self.books.contacts.iter().find(|x| x.id == c));
        let style = MoneyStyle::default();
        let values = FieldValues {
            client_name: client
                .map(|c| c.fields.display_name.clone())
                .unwrap_or_default(),
            invoice_number: e.record.txn.txn_number.clone().unwrap_or_default(),
            invoice_date: crate::shell::us_date(e.record.txn.txn_date),
            due_date: e
                .record
                .due_date
                .map(crate::shell::us_date)
                .unwrap_or_default(),
            total: e
                .record
                .txn
                .total
                .map(|m| m.format(style))
                .unwrap_or_default(),
            balance_due: e
                .record
                .balance_due
                .map(|m| m.format(style))
                .unwrap_or_default(),
            company_name: self.company_name(),
        };
        let (subject, body) = match prefs.template(TemplateDoc::Invoice) {
            TemplateSource::Default {
                subject_key,
                body_key,
            } => (t(subject_key), t(body_key)),
            TemplateSource::Custom(c) => (c.subject.clone(), c.body.clone()),
        };
        let s = fill_template(&subject, &values);
        let b = fill_template(&body, &values);
        let f = &mut self.email;
        f.unknown_fields = s.unknown.len() + b.unknown.len();
        f.subject = s.text;
        f.body = b.text;
        f.to = client
            .and_then(|c| c.fields.email.clone())
            .unwrap_or_default();
        f.cc.clear();
        f.bcc.clear();
        f.mark_sent = false;
        f.failed = false;
        f.problem = None;
        f.invoice = Some(e.record.clone());
        f.then = then;
        f.open = true;
        if prefs.bcc_self && !self.link.is_demo() {
            f.from_job = Some(engine::read(
                &self.link,
                self.group.as_ref(),
                |_| Ok(String::new()),
                |s| async move { Ok(s.smtp_settings().await?.smtp_from) },
            ));
        }
        self.offer_send = false;
    }

    fn email_target(&self) -> Option<TransactionId> {
        self.email.invoice.as_ref().map(|r| r.txn.id)
    }

    /// Send now.
    pub(crate) fn send_email(&mut self) {
        let Some(id) = self.email_target() else {
            return;
        };
        let msg = self.email.message();
        if msg.to.is_empty() {
            self.email.problem = Some("invoice.email.to_needed");
            return;
        }
        let Some((name, pdf)) = self.pdf() else {
            self.email.problem = Some("invoice.email.pdf_failed");
            return;
        };
        if pdf.len() > cl_api::EMAIL_PDF_MAX_BYTES {
            self.email.problem = Some("invoice.email.too_large");
            return;
        }
        self.email.problem = None;
        let key = IdempotencyKey::new();
        let to = msg.to.clone();
        self.email.job = Some(engine::write(
            &self.link,
            self.group.as_ref(),
            label("orphan.invoice_email", Some(key.clone())),
            |d| crate::company::demo::invoices::email(d, id, &to).map(|_| Outcome::Sent),
            move |s| async move {
                s.invoices().email(id, &msg, &name, &pdf, key).await?;
                Ok(Outcome::Sent)
            },
        ));
        self.poll();
    }

    /// Put it in the Outbox (sent when the owner clicks Send all, Q-P20d).
    pub(crate) fn queue_email(&mut self) {
        let Some(id) = self.email_target() else {
            return;
        };
        let msg = self.email.message();
        if msg.to.is_empty() {
            self.email.problem = Some("invoice.email.to_needed");
            return;
        }
        let key = IdempotencyKey::new();
        let m2 = msg.clone();
        self.email.job = Some(engine::write(
            &self.link,
            self.group.as_ref(),
            label("orphan.invoice_email_later", Some(key.clone())),
            |d| {
                crate::company::demo::invoices::email_later(d, id, &m2)
                    .map(|_: OutboxItem| Outcome::Queued)
            },
            move |s| async move {
                s.invoices().email_later(id, &msg, key).await?;
                Ok(Outcome::Queued)
            },
        ));
        self.poll();
    }

    /// Save PDF: opens it full size; counts as sent only with Mark as sent.
    fn pdf_email(&mut self) {
        let Some(id) = self.email_target() else {
            return;
        };
        let Some((name, bytes)) = self.pdf() else {
            self.email.problem = Some("invoice.email.pdf_failed");
            return;
        };
        self.link.push(CompanyEvent::OpenWith(
            cl_model::CommandId::new(crate::screens::document_view::KIND),
            Box::new(crate::windows::OpenPayload::Document(
                crate::widgets::pdf_viewer::DocSource {
                    name,
                    bytes: Arc::from(bytes),
                },
            )),
        ));
        if self.email.mark_sent {
            let key = IdempotencyKey::new();
            let sent = SendRecord {
                method: SendMethod::Pdf,
                recipients: Vec::new(),
                sent_at: None,
            };
            let s2 = sent.clone();
            self.email.job = Some(engine::write(
                &self.link,
                self.group.as_ref(),
                label("orphan.invoice_sent", Some(key.clone())),
                |d| crate::company::demo::invoices::record_sent(d, id, &s2).map(|_| Outcome::Pdf),
                move |s| async move {
                    s.invoices().record_sent(id, &sent, key).await?;
                    Ok(Outcome::Pdf)
                },
            ));
            self.poll();
        } else {
            self.email.open = false;
            self.email.done = Some((Outcome::Pdf, self.email.then.take()));
        }
    }

    /// The flow ended: read the invoice again (its send history) and run
    /// what the save asked for.
    pub(crate) fn email_done(&mut self, done: (Outcome, Option<SaveIntent>)) {
        let (outcome, then) = done;
        self.last_action = Some(match outcome {
            Outcome::Sent => "emailed",
            Outcome::Queued => "queued",
            Outcome::Pdf => "pdf",
            Outcome::Cancelled => "email_cancelled",
        });
        if outcome != Outcome::Cancelled {
            crate::forms::publish_saved(&self.link, &self.bus, &[]);
            crate::screens::outbox::refresh_count(&self.link, self.group.as_ref());
        }
        match then {
            Some(i) if i != SaveIntent::Save => self.follow_up(i),
            _ => {
                if outcome != Outcome::Cancelled
                    && let Some(id) = self.editing.as_ref().map(|e| e.id)
                {
                    self.load(id);
                    self.last_action = Some(match outcome {
                        Outcome::Sent => "emailed",
                        Outcome::Queued => "queued",
                        _ => "pdf",
                    });
                }
            }
        }
    }

    /// The dialog over the form.
    pub(crate) fn email_window(&mut self, ctx: &egui::Context, tokens: &Tokens) {
        if !self.email.open {
            return;
        }
        let mut click = None;
        let width = (ctx.content_rect().width() - 48.0).clamp(300.0, 560.0);
        egui::Window::new(t("invoice.email.title"))
            .id(egui::Id::unique("invoice/email"))
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.set_width(width);
                let f = &mut self.email;
                for (key, text) in [
                    ("invoice.email.to", &mut f.to),
                    ("invoice.email.cc", &mut f.cc),
                    ("invoice.email.bcc", &mut f.bcc),
                    ("invoice.email.subject", &mut f.subject),
                ] {
                    let l = ui.label(t(key));
                    ui.add(egui::TextEdit::singleline(text).desired_width(width - 16.0))
                        .labelled_by(l.id);
                }
                let l = ui.label(t("invoice.email.body"));
                ui.add(
                    egui::TextEdit::multiline(&mut f.body)
                        .desired_rows(6)
                        .desired_width(width - 16.0),
                )
                .labelled_by(l.id);
                ui.label(
                    RichText::new(t("invoice.email.attachment"))
                        .size(12.0)
                        .color(tokens.text_secondary),
                );
                if f.unknown_fields > 0 {
                    ui.label(
                        RichText::new(t("invoice.email.unknown_fields")).color(tokens.warn_text),
                    );
                }
                if let Some(p) = f.problem {
                    ui.label(RichText::new(t(p)).color(tokens.warn_text));
                }
                ui.add_space(8.0);
                ui.horizontal_wrapped(|ui| {
                    let sending = f.job.is_some();
                    if f.failed {
                        if ui
                            .add_enabled(!sending, egui::Button::new(t("invoice.email.to_outbox")))
                            .clicked()
                        {
                            click = Some("outbox");
                        }
                        if f.problem == Some("invoice.email.no_smtp")
                            && ui.button(t("invoice.email.open_prefs")).clicked()
                        {
                            click = Some("prefs");
                        }
                    } else if ui
                        .add_enabled(!sending, egui::Button::new(t("invoice.email.send")))
                        .clicked()
                    {
                        click = Some("send");
                    }
                    if ui
                        .add_enabled(!sending, egui::Button::new(t("invoice.email.later")))
                        .clicked()
                        && !f.failed
                    {
                        click = Some("outbox");
                    }
                    if ui
                        .add_enabled(!sending, egui::Button::new(t("invoice.email.save_pdf")))
                        .clicked()
                    {
                        click = Some("pdf");
                    }
                    ui.checkbox(&mut f.mark_sent, t("invoice.email.mark_sent"));
                    if ui.button(t("invoice.email.cancel")).clicked() {
                        click = Some("cancel");
                    }
                });
            });
        match click {
            Some("send") => self.send_email(),
            Some("outbox") => self.queue_email(),
            Some("pdf") => self.pdf_email(),
            Some("prefs") => self
                .link
                .push(CompanyEvent::OpenWindow(cl_model::CommandId::new(
                    "file.preferences",
                ))),
            Some(_) => self.email.cancel(),
            None => {}
        }
    }

    /// Send the open invoice's update (the banner's button after an edit).
    pub(crate) fn resend(&mut self) {
        self.offer_send = false;
        self.open_email(None);
    }
}

/// "Sent on 11/04/2026 by email to a@b.example" (Q-P20f), the latest send.
pub(crate) fn sent_text(r: &InvoiceRecord) -> Option<String> {
    let last = r.send_history.last()?;
    let when = last
        .sent_at
        .and_then(|ts| cl_core::CalendarDate::from_naive(ts.as_utc().date_naive()).ok())
        .map(crate::shell::us_date)
        .unwrap_or_default();
    let how = t(match last.method {
        SendMethod::Printed => "invoice.sent.printed",
        SendMethod::Email => "invoice.sent.email",
        SendMethod::Pdf | SendMethod::Other => "invoice.sent.pdf",
    });
    let to = last.recipients.join(", ");
    Some(if to.is_empty() {
        crate::i18n::t_with("invoice.sent.banner", &[("date", &when), ("how", &how)])
    } else {
        crate::i18n::t_with(
            "invoice.sent.banner_to",
            &[("date", &when), ("how", &how), ("to", &to)],
        )
    })
}
