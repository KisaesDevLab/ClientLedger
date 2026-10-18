//! SMTP settings kept on this computer (P09 step 4, Q-34). Host, port,
//! user name and sender live in the engine (`/company/smtp`); the password
//! lives only in the OS keychain under [`SMTP_PASSWORD`] and is handed to
//! the engine per session once U-13 lands (Q-P09b). The connection
//! security choice is local (the engine derives TLS from the port).

use serde::{Deserialize, Serialize};

/// Keychain entry for the SMTP password.
pub const SMTP_PASSWORD: &str = "smtp-password";

/// How the connection is secured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SmtpSecurity {
    /// STARTTLS on port 587 (the usual choice).
    #[default]
    StartTls,
    /// Implicit TLS on port 465.
    Tls,
    /// Plain text (only for a mail server on this computer or the office network).
    None,
}

impl SmtpSecurity {
    /// Every choice.
    pub const ALL: [SmtpSecurity; 3] = [
        SmtpSecurity::StartTls,
        SmtpSecurity::Tls,
        SmtpSecurity::None,
    ];

    /// The port most servers use with this security.
    #[must_use]
    pub fn default_port(self) -> u16 {
        match self {
            SmtpSecurity::StartTls => 587,
            SmtpSecurity::Tls => 465,
            SmtpSecurity::None => 25,
        }
    }

    /// i18n key of the name.
    #[must_use]
    pub fn label_key(self) -> &'static str {
        match self {
            SmtpSecurity::StartTls => "prefs.email.starttls",
            SmtpSecurity::Tls => "prefs.email.tls",
            SmtpSecurity::None => "prefs.email.plain",
        }
    }
}

/// The non-secret SMTP fields as the Email panel edits them.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SmtpForm {
    /// Server host name.
    pub host: String,
    /// Port text as typed.
    pub port: String,
    /// Connection security.
    pub security: SmtpSecurity,
    /// Sign-in user name (may be empty).
    pub user: String,
    /// Sender address.
    pub from: String,
    /// Sender display name.
    pub from_name: String,
}

/// Why an SMTP form cannot be saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SmtpFieldError {
    /// Host is empty or too long.
    #[error("server address")]
    Host,
    /// Port is not 1-65535.
    #[error("port")]
    Port,
    /// Sender is not an email address.
    #[error("sender address")]
    From,
    /// Plain (no TLS) is only allowed for a server on this computer or the
    /// office network.
    #[error("plain connection to a public server")]
    PlainNotLocal,
    /// Implicit TLS needs port 465 and STARTTLS another port.
    #[error("security does not match the port")]
    SecurityPort,
}

impl SmtpFieldError {
    /// i18n key of the owner message.
    #[must_use]
    pub fn message_key(self) -> &'static str {
        match self {
            SmtpFieldError::Host => "prefs.email.err_host",
            SmtpFieldError::Port => "prefs.email.err_port",
            SmtpFieldError::From => "prefs.email.err_from",
            SmtpFieldError::PlainNotLocal => "prefs.email.err_plain",
            SmtpFieldError::SecurityPort => "prefs.email.err_security_port",
        }
    }
}

/// True when `s` holds a control character (CR, LF, NUL, …).
#[must_use]
pub fn has_control(s: &str) -> bool {
    s.chars().any(char::is_control)
}

/// A plain email-address shape check (one `@`, a dot after it, no spaces).
#[must_use]
pub fn looks_like_email(s: &str) -> bool {
    let s = s.trim();
    let Some((local, domain)) = s.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && !domain.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !s.contains(' ')
        && !domain.contains('@')
        && s.len() <= 255
}

impl SmtpForm {
    /// The port as a number, when valid.
    #[must_use]
    pub fn port_number(&self) -> Option<u16> {
        self.port.trim().parse::<u16>().ok().filter(|p| *p >= 1)
    }

    /// Checks the fields; nothing is sent while this fails. Host, user
    /// name, sender and display name may not carry control characters
    /// (header injection); plain connections only reach local or private
    /// servers; implicit TLS is port 465 and STARTTLS any other port.
    ///
    /// # Errors
    /// The first field that is wrong.
    pub fn validate(&self) -> Result<(), SmtpFieldError> {
        let host = self.host.trim();
        if host.is_empty() || host.len() > 255 || host.contains(' ') || has_control(host) {
            return Err(SmtpFieldError::Host);
        }
        let Some(port) = self.port_number() else {
            return Err(SmtpFieldError::Port);
        };
        if !looks_like_email(&self.from) || has_control(&self.from) {
            return Err(SmtpFieldError::From);
        }
        if has_control(&self.user) || has_control(&self.from_name) {
            return Err(SmtpFieldError::Host);
        }
        match self.security {
            SmtpSecurity::None if !crate::ai::is_local_host(host) => {
                return Err(SmtpFieldError::PlainNotLocal);
            }
            SmtpSecurity::Tls if port != 465 => return Err(SmtpFieldError::SecurityPort),
            SmtpSecurity::StartTls if port == 465 => return Err(SmtpFieldError::SecurityPort),
            _ => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn security_ports_and_labels() {
        assert_eq!(SmtpSecurity::StartTls.default_port(), 587);
        assert_eq!(SmtpSecurity::Tls.default_port(), 465);
        assert_eq!(SmtpSecurity::None.default_port(), 25);
        for s in SmtpSecurity::ALL {
            assert!(s.label_key().starts_with("prefs.email."));
        }
    }

    #[test]
    fn form_validation_names_the_first_bad_field() {
        let ok = SmtpForm {
            host: "smtp.example.test".into(),
            port: "587".into(),
            security: SmtpSecurity::StartTls,
            user: "books".into(),
            from: "books@example.test".into(),
            from_name: "Ozark Ridge".into(),
        };
        assert_eq!(ok.validate(), Ok(()));
        assert_eq!(ok.port_number(), Some(587));
        let mut bad = ok.clone();
        bad.host = " ".into();
        assert_eq!(bad.validate(), Err(SmtpFieldError::Host));
        assert_eq!(SmtpFieldError::Host.message_key(), "prefs.email.err_host");
        let mut bad = ok.clone();
        bad.port = "0".into();
        assert_eq!(bad.validate(), Err(SmtpFieldError::Port));
        bad.port = "70000".into();
        assert_eq!(bad.validate(), Err(SmtpFieldError::Port));
        let mut bad = ok;
        bad.from = "not an address".into();
        assert_eq!(bad.validate(), Err(SmtpFieldError::From));
        assert!(looks_like_email("a@b.co"));
        assert!(!looks_like_email("a@b"));
        assert!(!looks_like_email("@b.co"));
        assert!(!looks_like_email("a@.co"));
        assert!(!looks_like_email("a b@c.co"));
    }

    #[test]
    fn security_must_match_the_port_and_plain_stays_local() {
        let base = SmtpForm {
            host: "smtp.example.test".into(),
            port: "587".into(),
            security: SmtpSecurity::StartTls,
            user: "books".into(),
            from: "books@example.test".into(),
            from_name: String::new(),
        };
        let mut f = base.clone();
        f.security = SmtpSecurity::Tls;
        assert_eq!(f.validate(), Err(SmtpFieldError::SecurityPort));
        f.port = "465".into();
        assert_eq!(f.validate(), Ok(()));
        f.security = SmtpSecurity::StartTls;
        assert_eq!(f.validate(), Err(SmtpFieldError::SecurityPort));
        let mut f = base.clone();
        f.security = SmtpSecurity::None;
        f.port = "25".into();
        assert_eq!(f.validate(), Err(SmtpFieldError::PlainNotLocal));
        f.host = "192.168.1.5".into();
        assert_eq!(f.validate(), Ok(()));
        f.host = "mailhub".into();
        assert_eq!(f.validate(), Ok(()));
        let mut f = base.clone();
        f.user = "books\r\nX-Injected: 1".into();
        assert_eq!(f.validate(), Err(SmtpFieldError::Host));
        let mut f = base.clone();
        f.from = "books@example.test\n".into();
        assert_eq!(f.validate(), Err(SmtpFieldError::From));
        let mut f = base;
        f.host = "smtp.exam\rple.test".into();
        assert_eq!(f.validate(), Err(SmtpFieldError::Host));
        assert!(has_control("a\u{0}b"));
    }
}
