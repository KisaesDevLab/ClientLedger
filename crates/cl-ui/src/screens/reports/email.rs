//! Email a report (U-207, `reports-email-v1`, Q-P28i): the report on
//! screen as PDF, Excel or CSV, sent through the company's mail server as
//! invoices are. The button shows only when the engine sends report email.
//! One Idempotency-Key per Send click; no address, subject or file goes to
//! the log.

use cl_api::resources::reports::email::{EmailRefusal, EmailSent, ReportEmail, email_refusal};
use cl_api::{ApiError, IdempotencyKey, LocalValidation, OrphanLabel};
use eframe::egui::{self, RichText, Ui};

use super::export::{Kind, bytes};
use super::window::ReportWindow;
use crate::company::CompanyEvent;
use crate::i18n::{t, t_with};
use crate::screens::company::form;
use crate::screens::invoice::engine::{self, Job};
use crate::theme::Tokens;

/// The email form's state.
pub(crate) struct Mailer {
    /// Shown.
    pub open: bool,
    pub to: String,
    pub cc: String,
    pub subject: String,
    pub body: String,
    pub kind: Kind,
    job: Option<Job<EmailSent>>,
    /// A plain message (i18n key) after a send or a refusal.
    pub message: Option<&'static str>,
    pub error: Option<ApiError>,
}

impl Default for Mailer {
    fn default() -> Mailer {
        Mailer {
            open: false,
            to: String::new(),
            cc: String::new(),
            subject: String::new(),
            body: String::new(),
            kind: Kind::Pdf,
            job: None,
            message: None,
            error: None,
        }
    }
}

impl Mailer {
    pub(crate) fn busy(&self) -> bool {
        self.job.is_some()
    }

    pub(crate) fn state_json(&self) -> serde_json::Value {
        serde_json::json!({
            "open": self.open,
            "sending": self.busy(),
            "message": self.message.map(|m| m.rsplit('.').next().unwrap_or_default()),
            "error": self.error.is_some(),
        })
    }
}

/// True when the engine sends report email (the Email button shows).
pub(crate) fn available(w: &ReportWindow) -> bool {
    w.link.is_demo()
        || w.link
            .capabilities()
            .has(cl_api::Capabilities::REPORTS_EMAIL)
}

/// Addresses typed with commas or semicolons.
pub(crate) fn addresses(s: &str) -> Vec<String> {
    s.split([',', ';'])
        .map(str::trim)
        .filter(|a| !a.is_empty())
        .map(str::to_string)
        .collect()
}

/// Opens the form with the report's title as the subject.
pub(super) fn open(w: &mut ReportWindow) {
    let title = w.title_text();
    let m = &mut w.mailer;
    m.open = true;
    m.message = None;
    m.error = None;
    if m.subject.is_empty() {
        m.subject = title;
    }
}

fn send(w: &mut ReportWindow) {
    let Some(file) = bytes(w, w.mailer.kind) else {
        w.mailer.message = Some("reports.export.failed");
        return;
    };
    let ext = match w.mailer.kind {
        Kind::Csv => "csv",
        Kind::Xlsx => "xlsx",
        Kind::Pdf => "pdf",
    };
    let mail = ReportEmail {
        to: addresses(&w.mailer.to),
        cc: addresses(&w.mailer.cc),
        subject: w.mailer.subject.clone(),
        body: w.mailer.body.clone(),
        filename: format!("{}.{ext}", cl_print::files::safe_component(&w.title_text())),
        file,
        report_key: Some(w.spec.key.to_string()),
    };
    // One key per click: a retried send is replayed, never sent twice.
    let key = IdempotencyKey::new();
    w.mailer.message = None;
    w.mailer.error = None;
    w.mailer.job = Some(engine::write(
        &w.link,
        w.group.as_ref(),
        OrphanLabel {
            what: "orphan.report_email",
            key: None,
        },
        {
            let mail = mail.clone();
            move |d| crate::company::demo::report_store::email(d, &mail)
        },
        move |s| async move { s.email_report(&mail, key).await },
    ));
}

/// The message for a refusal.
pub(crate) fn refusal_key(e: &ApiError) -> Option<&'static str> {
    Some(match email_refusal(e)? {
        EmailRefusal::SmtpNotConfigured => "reports.email.smtp_not_configured",
        EmailRefusal::SendFailed => "reports.email.send_failed",
        EmailRefusal::InvalidRecipient => "reports.email.invalid_recipient",
        EmailRefusal::TooLarge => "reports.email.too_large",
        EmailRefusal::Offline => "reports.email.offline",
    })
}

pub(super) fn poll(w: &mut ReportWindow) {
    if let Some(r) = engine::poll(&mut w.mailer.job) {
        match r {
            Ok(_) => {
                w.mailer.open = false;
                w.mailer.message = Some("reports.email.sent");
            }
            Err(e) => match refusal_key(&e) {
                Some(k) => w.mailer.message = Some(k),
                None if matches!(e, ApiError::Local(LocalValidation::InvalidValue)) => {
                    w.mailer.message = Some("reports.email.check_fields");
                }
                None => w.mailer.error = Some(e),
            },
        }
    }
}

pub(super) fn ui(w: &mut ReportWindow, ui: &mut Ui, tokens: &Tokens) {
    if let Some(m) = w.mailer.message
        && !w.mailer.open
    {
        form::hint(ui, tokens, &t(m));
    }
    if !w.mailer.open {
        return;
    }
    let mut go = false;
    let mut close = false;
    let mut settings = false;
    egui::Frame::group(ui.style()).show(ui, |ui| {
        let m = &mut w.mailer;
        ui.label(RichText::new(t("reports.email.title")).strong());
        form::field(ui, &t("reports.email.to"), &mut m.to);
        form::field(ui, &t("reports.email.cc"), &mut m.cc);
        form::field(ui, &t("reports.email.subject"), &mut m.subject);
        ui.label(RichText::new(t("reports.email.body")).size(12.0));
        ui.add(egui::TextEdit::multiline(&mut m.body).desired_rows(3));
        ui.horizontal(|ui| {
            for (k, key) in [
                (Kind::Pdf, "reports.email.as_pdf"),
                (Kind::Xlsx, "reports.email.as_xlsx"),
                (Kind::Csv, "reports.email.as_csv"),
            ] {
                ui.radio_value(&mut m.kind, k, t(key));
            }
        });
        if let Some(msg) = m.message {
            ui.horizontal_wrapped(|ui| {
                form::inline_warning(ui, tokens, &t(msg));
                if msg == "reports.email.smtp_not_configured"
                    && ui.link(t("reports.email.open_settings")).clicked()
                {
                    settings = true;
                }
            });
        }
        if let Some(e) = &m.error {
            form::inline_error(ui, tokens, e);
        }
        ui.horizontal(|ui| {
            let ready = !addresses(&m.to).is_empty() && m.job.is_none();
            if ui
                .add_enabled(ready, egui::Button::new(t("reports.email.send")))
                .clicked()
            {
                go = true;
            }
            if ui.button(t("reports.email.cancel")).clicked() {
                close = true;
            }
            if m.job.is_some() {
                ui.spinner();
                ui.label(t_with(
                    "reports.email.sending",
                    &[("n", &addresses(&m.to).len().to_string())],
                ));
            }
        });
    });
    if go {
        send(w);
    }
    if close {
        w.mailer.open = false;
        w.mailer.message = None;
    }
    if settings {
        w.link
            .push(CompanyEvent::OpenWindow(cl_model::CommandId::new(
                "edit.preferences",
            )));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_split_on_commas_and_semicolons() {
        assert_eq!(
            addresses(" a@x.test, b@x.test; ;c@x.test "),
            ["a@x.test", "b@x.test", "c@x.test"]
        );
        assert_eq!(addresses("  "), Vec::<String>::new());
    }

    #[test]
    fn smtp_not_configured_is_a_plain_message() {
        let e = cl_api::engine_refusal(
            400,
            &serde_json::json!({"error": {"message": "x", "code": "SMTP_NOT_CONFIGURED"}}),
        );
        assert_eq!(refusal_key(&e), Some("reports.email.smtp_not_configured"));
    }
}
