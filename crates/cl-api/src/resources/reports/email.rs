//! Report email (U-207, `reports-email-v1`): the exported PDF, xlsx or CSV
//! goes through the company's mail server as invoices do. One
//! Idempotency-Key per Send click: a retried send is replayed, never sent
//! twice. The engine writes one audit row; nothing here logs the
//! addresses, the subject or the file.

use base64::Engine as _;
use serde::Deserialize;

use crate::IdempotencyKey;
use crate::client::{Call, Method};
use crate::errors::{ApiError, LocalValidation};
use crate::resources::invoices::{SMTP_NOT_CONFIGURED, SMTP_SEND_FAILED};
use crate::resources::operations::REPORT_EMAIL;
use crate::session::{Session, decode};
use crate::wire::Capabilities;

/// The engine's cap on the decoded file.
pub const MAX_FILE_BYTES: usize = 10 * 1024 * 1024;

/// A report file to send.
#[derive(Clone, PartialEq, Eq)]
pub struct ReportEmail {
    /// Recipients (at least one, at most 50).
    pub to: Vec<String>,
    /// Copies.
    pub cc: Vec<String>,
    /// The subject.
    pub subject: String,
    /// The message.
    pub body: String,
    /// The file name (`.pdf`, `.xlsx` or `.csv`).
    pub filename: String,
    /// The file.
    pub file: Vec<u8>,
    /// The catalog key (the audit row only).
    pub report_key: Option<String>,
}

impl std::fmt::Debug for ReportEmail {
    // Owner data stays out of logs.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReportEmail")
            .field("to", &self.to.len())
            .field("bytes", &self.file.len())
            .finish_non_exhaustive()
    }
}

/// The engine's answer.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailSent {
    /// Sent.
    #[serde(default)]
    pub sent: bool,
    /// Recipients counted by the engine.
    #[serde(default)]
    pub recipient_count: u32,
}

/// Why a report email was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EmailRefusal {
    /// No mail server set up (400 `SMTP_NOT_CONFIGURED`): Settings › Email.
    SmtpNotConfigured,
    /// The mail server refused it (`SMTP_SEND_FAILED`).
    SendFailed,
    /// A recipient is not an email address (`INVALID_RECIPIENT`).
    InvalidRecipient,
    /// Over 10 MB.
    TooLarge,
    /// The engine can't reach the mail server (`OFFLINE_UNAVAILABLE`).
    Offline,
}

/// The email refusal in `e`, if it is one.
#[must_use]
pub fn email_refusal(e: &ApiError) -> Option<EmailRefusal> {
    match e {
        ApiError::OfflineUnavailable { .. } => Some(EmailRefusal::Offline),
        ApiError::PayloadTooLarge | ApiError::Local(LocalValidation::AttachmentTooLarge) => {
            Some(EmailRefusal::TooLarge)
        }
        ApiError::Validation(v) => match v.code().map(crate::errors::ErrorCode::as_str) {
            Some(SMTP_NOT_CONFIGURED) => Some(EmailRefusal::SmtpNotConfigured),
            Some(SMTP_SEND_FAILED) => Some(EmailRefusal::SendFailed),
            Some("INVALID_RECIPIENT") => Some(EmailRefusal::InvalidRecipient),
            Some("ATTACHMENT_TOO_LARGE") => Some(EmailRefusal::TooLarge),
            _ => None,
        },
        ApiError::Server { code: Some(c), .. } if c.as_str() == SMTP_NOT_CONFIGURED => {
            Some(EmailRefusal::SmtpNotConfigured)
        }
        _ => None,
    }
}

/// True for a file name the engine takes.
#[must_use]
pub fn filename_ok(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    !name.is_empty()
        && name.len() <= 200
        && !name.contains(['/', '\\'])
        && !name.chars().any(char::is_control)
        && [".pdf", ".xlsx", ".csv"].iter().any(|e| lower.ends_with(e))
}

/// The JSON body (checked before anything is sent).
///
/// # Errors
/// `Local(AttachmentTooLarge)` over 10 MB; `Local(InvalidValue)` for no
/// recipient, more than 50, or a file name the engine refuses.
pub fn body(m: &ReportEmail) -> Result<serde_json::Value, ApiError> {
    if m.file.len() > MAX_FILE_BYTES {
        return Err(ApiError::Local(LocalValidation::AttachmentTooLarge));
    }
    let to: Vec<&str> =
        m.to.iter()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();
    let cc: Vec<&str> =
        m.cc.iter()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();
    if to.is_empty() || to.len() > 50 || cc.len() > 50 || !filename_ok(&m.filename) {
        return Err(ApiError::Local(LocalValidation::InvalidValue));
    }
    let mut v = serde_json::json!({
        "to": to,
        "cc": cc,
        "subject": m.subject,
        "body": m.body,
        "filename": m.filename,
        "fileBase64": base64::engine::general_purpose::STANDARD.encode(&m.file),
    });
    if let Some(k) = &m.report_key {
        v["reportKey"] = serde_json::Value::from(k.as_str());
    }
    Ok(v)
}

impl Session {
    /// True when the engine sends report email (the Email button shows).
    #[must_use]
    pub fn can_email_reports(&self) -> bool {
        self.client()
            .capabilities()
            .has(Capabilities::REPORTS_EMAIL)
    }

    /// Emails a report file.
    ///
    /// # Errors
    /// `Local(Unsupported)` without `reports-email-v1`; the local checks of
    /// [`body`]; [`email_refusal`] for the engine's; any [`ApiError`].
    pub async fn email_report(
        &self,
        m: &ReportEmail,
        key: IdempotencyKey,
    ) -> Result<EmailSent, ApiError> {
        if !self.can_email_reports() {
            return Err(ApiError::Local(LocalValidation::Unsupported));
        }
        let bytes = serde_json::to_vec(&body(m)?)
            .map_err(|_| ApiError::Local(LocalValidation::InvalidValue))?;
        let mut call = Call::at(Method::Post, REPORT_EMAIL.route()).json_body(bytes);
        call.idempotency_key = Some(key);
        let reply = self.send(&REPORT_EMAIL, call, None).await?;
        decode(&reply, "report email")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mail() -> ReportEmail {
        ReportEmail {
            to: vec!["owner@example.test".into(), " ".into()],
            cc: vec![],
            subject: "P&L".into(),
            body: "Attached.".into(),
            filename: "Profit and Loss.pdf".into(),
            file: b"%PDF-1.7".to_vec(),
            report_key: Some("profit_loss".into()),
        }
    }

    #[test]
    fn the_body_carries_base64_and_drops_blank_recipients() {
        let v = body(&mail()).expect("body");
        assert_eq!(v["to"], serde_json::json!(["owner@example.test"]));
        assert_eq!(v["fileBase64"], "JVBERi0xLjc=");
        assert_eq!(v["reportKey"], "profit_loss");
    }

    #[test]
    fn local_checks_send_nothing() {
        let mut m = mail();
        m.file = vec![0; MAX_FILE_BYTES + 1];
        assert!(matches!(
            body(&m),
            Err(ApiError::Local(LocalValidation::AttachmentTooLarge))
        ));
        let mut m = mail();
        m.to = vec![];
        assert!(body(&m).is_err());
        let mut m = mail();
        m.filename = "report.exe".into();
        assert!(body(&m).is_err());
        assert!(!filename_ok("a/b.pdf"));
        assert!(filename_ok("Q4.XLSX"));
    }

    #[test]
    fn debug_shows_no_owner_data() {
        let s = format!("{:?}", mail());
        assert!(!s.contains("example.test") && !s.contains("P&L"), "{s}");
    }

    #[test]
    fn refusals_are_recognised() {
        let smtp = crate::engine_refusal(
            400,
            &serde_json::json!({"error": {"message": "x", "code": "SMTP_NOT_CONFIGURED"}}),
        );
        assert_eq!(email_refusal(&smtp), Some(EmailRefusal::SmtpNotConfigured));
        let bad = crate::engine_refusal(
            400,
            &serde_json::json!({"error": {"message": "x", "code": "INVALID_RECIPIENT", "details": {"field": "to", "index": 0}}}),
        );
        assert_eq!(email_refusal(&bad), Some(EmailRefusal::InvalidRecipient));
        assert_eq!(email_refusal(&ApiError::Timeout), None);
    }
}
