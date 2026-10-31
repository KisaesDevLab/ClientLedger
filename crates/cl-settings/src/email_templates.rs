//! Email templates and the BCC-self choice (P20 plan §3 WP-A step 4,
//! Q-P20j), kept with the company's local preferences
//! ([`crate::CompanyPrefs::email`]) so they travel in its backups.
//!
//! - One template per document type ([`TemplateDoc`]; statements arrive in
//!   P22). A type without a template of its own uses the default texts,
//!   named by i18n key ([`DEFAULT_INVOICE_SUBJECT_KEY`],
//!   [`DEFAULT_INVOICE_BODY_KEY`]) and resolved by the UI, so they can be
//!   translated.
//! - Fields are written `{client_name}`, `{invoice_number}`,
//!   `{invoice_date}`, `{due_date}`, `{total}`, `{balance_due}` and
//!   `{company_name}` ([`FIELDS`]). `{{` and `}}` write a single brace. A
//!   field this version does not know is kept as typed and reported
//!   ([`Filled::unknown`]), so the editor can say so.
//! - [`fill`] substitutes in one pass: a value that itself contains
//!   `{total}` is never expanded again. Values are the caller's display
//!   strings (`{total}` copies the engine's figure as formatted); nothing
//!   here computes.
//! - Template text is owner data: never logged.

use std::fmt;

use serde::{Deserialize, Serialize};

/// The i18n key of the default invoice email subject.
pub const DEFAULT_INVOICE_SUBJECT_KEY: &str = "email.template.invoice.subject";
/// The i18n key of the default invoice email body.
pub const DEFAULT_INVOICE_BODY_KEY: &str = "email.template.invoice.body";
/// The longest subject kept (characters).
pub const SUBJECT_MAX: usize = 255;
/// The longest body kept (characters).
pub const BODY_MAX: usize = 10_000;

/// The document types that have an email template.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TemplateDoc {
    /// Invoices.
    Invoice,
}

/// One field a template can use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Field {
    /// `{client_name}`.
    ClientName,
    /// `{invoice_number}`.
    InvoiceNumber,
    /// `{invoice_date}`.
    InvoiceDate,
    /// `{due_date}`.
    DueDate,
    /// `{total}`.
    Total,
    /// `{balance_due}`.
    BalanceDue,
    /// `{company_name}`.
    CompanyName,
}

/// Every field with the name written between braces, in the editor's
/// order.
pub const FIELDS: [(Field, &str); 7] = [
    (Field::ClientName, "client_name"),
    (Field::InvoiceNumber, "invoice_number"),
    (Field::InvoiceDate, "invoice_date"),
    (Field::DueDate, "due_date"),
    (Field::Total, "total"),
    (Field::BalanceDue, "balance_due"),
    (Field::CompanyName, "company_name"),
];

impl Field {
    /// The name written between braces.
    #[must_use]
    pub fn name(self) -> &'static str {
        FIELDS
            .iter()
            .find(|(f, _)| *f == self)
            .map_or("", |(_, n)| n)
    }

    /// The field written as `name`.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Field> {
        FIELDS.iter().find(|(_, n)| *n == name).map(|(f, _)| *f)
    }
}

/// A template the owner wrote.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct EmailTemplate {
    /// Subject.
    pub subject: String,
    /// Body.
    pub body: String,
}

/// Lengths only: template text is owner data.
impl fmt::Debug for EmailTemplate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EmailTemplate")
            .field("subject", &self.subject.chars().count())
            .field("body", &self.body.chars().count())
            .finish()
    }
}

/// Where a document type's template comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemplateSource<'a> {
    /// The default texts: resolve these i18n keys.
    Default {
        /// Subject key.
        subject_key: &'static str,
        /// Body key.
        body_key: &'static str,
    },
    /// The owner's own template.
    Custom(&'a EmailTemplate),
}

/// The company's email choices.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EmailPrefs {
    /// Send a blind copy to the From address (Q-P20j; on by default). The
    /// sent history is kept in the books either way.
    pub bcc_self: bool,
    /// The owner's invoice template; `None` uses the default texts.
    pub invoice: Option<EmailTemplate>,
}

impl Default for EmailPrefs {
    fn default() -> Self {
        EmailPrefs {
            bcc_self: true,
            invoice: None,
        }
    }
}

impl EmailPrefs {
    /// The template of `doc`.
    #[must_use]
    pub fn template(&self, doc: TemplateDoc) -> TemplateSource<'_> {
        match doc {
            TemplateDoc::Invoice => self.invoice.as_ref().map_or(
                TemplateSource::Default {
                    subject_key: DEFAULT_INVOICE_SUBJECT_KEY,
                    body_key: DEFAULT_INVOICE_BODY_KEY,
                },
                TemplateSource::Custom,
            ),
        }
    }

    /// Saves the owner's template for `doc`, cut to [`SUBJECT_MAX`] and
    /// [`BODY_MAX`] characters (a line break in the subject becomes a
    /// space). An empty subject and body go back to the default texts.
    pub fn set_template(&mut self, doc: TemplateDoc, template: &EmailTemplate) {
        let subject: String = template
            .subject
            .chars()
            .map(|c| if c == '\r' || c == '\n' { ' ' } else { c })
            .take(SUBJECT_MAX)
            .collect();
        let body: String = template.body.chars().take(BODY_MAX).collect();
        let kept = (!subject.trim().is_empty() || !body.trim().is_empty())
            .then_some(EmailTemplate { subject, body });
        match doc {
            TemplateDoc::Invoice => self.invoice = kept,
        }
    }

    /// Drops what a stored file can't hold (overlong text).
    pub fn normalize(&mut self) {
        if let Some(t) = self.invoice.take() {
            self.set_template(TemplateDoc::Invoice, &t);
        }
    }
}

/// The values of one email, as the owner will read them (formatted by the
/// caller; owner data).
#[derive(Clone, PartialEq, Eq, Default)]
pub struct FieldValues {
    /// The client's name.
    pub client_name: String,
    /// The invoice number.
    pub invoice_number: String,
    /// The invoice date.
    pub invoice_date: String,
    /// The due date.
    pub due_date: String,
    /// The engine's total, as formatted.
    pub total: String,
    /// The engine's balance due, as formatted.
    pub balance_due: String,
    /// The company's name.
    pub company_name: String,
}

impl fmt::Debug for FieldValues {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("FieldValues(..)")
    }
}

impl FieldValues {
    /// The value of `field`.
    #[must_use]
    pub fn get(&self, field: Field) -> &str {
        match field {
            Field::ClientName => &self.client_name,
            Field::InvoiceNumber => &self.invoice_number,
            Field::InvoiceDate => &self.invoice_date,
            Field::DueDate => &self.due_date,
            Field::Total => &self.total,
            Field::BalanceDue => &self.balance_due,
            Field::CompanyName => &self.company_name,
        }
    }
}

/// A filled-in template.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct Filled {
    /// The text with every known field replaced.
    pub text: String,
    /// The `{names}` this version does not know, each once, in order;
    /// they stay in [`Filled::text`] as typed.
    pub unknown: Vec<String>,
}

impl fmt::Debug for Filled {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Filled")
            .field("text", &self.text.chars().count())
            .field("unknown", &self.unknown)
            .finish()
    }
}

/// A field name: letters, digits and `_`, 1 to 40 characters.
fn is_name(s: &str) -> bool {
    !s.is_empty() && s.len() <= 40 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// Substitutes the fields of `template` with `values` in one pass.
///
/// `{{` writes `{` and `}}` writes `}`; `{name}` is replaced when `name` is
/// a known field, kept and reported when it is not; any other brace is kept
/// as typed.
#[must_use]
pub fn fill(template: &str, values: &FieldValues) -> Filled {
    let mut out = String::with_capacity(template.len());
    let mut unknown: Vec<String> = Vec::new();
    let mut rest = template;
    while let Some(i) = rest.find(['{', '}']) {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        if let Some(after) = tail.strip_prefix("{{") {
            out.push('{');
            rest = after;
        } else if let Some(after) = tail.strip_prefix("}}") {
            out.push('}');
            rest = after;
        } else if tail.starts_with('{')
            && let Some(end) = tail[1..].find('}')
            && is_name(&tail[1..=end])
        {
            let name = &tail[1..=end];
            if let Some(f) = Field::from_name(name) {
                out.push_str(values.get(f));
            } else {
                out.push_str(&tail[..end + 2]);
                if !unknown.iter().any(|u| u == name) {
                    unknown.push(name.to_string());
                }
            }
            rest = &tail[end + 2..];
        } else {
            // A lone brace: kept as typed.
            let c = tail.chars().next().unwrap_or('{');
            out.push(c);
            rest = &tail[c.len_utf8()..];
        }
    }
    out.push_str(rest);
    Filled { text: out, unknown }
}

/// The unknown `{names}` of `template` (the editor's warning).
#[must_use]
pub fn unknown_fields(template: &str) -> Vec<String> {
    fill(template, &FieldValues::default()).unknown
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn values() -> FieldValues {
        FieldValues {
            client_name: "Hartley Builders".into(),
            invoice_number: "1042".into(),
            invoice_date: "11/03/2026".into(),
            due_date: "12/03/2026".into(),
            total: "$3,234.87".into(),
            balance_due: "$3,234.87".into(),
            company_name: "Kisaes Garden Supply".into(),
        }
    }

    #[test]
    fn every_field_is_replaced() {
        let t = "{client_name}|{invoice_number}|{invoice_date}|{due_date}|{total}|{balance_due}|{company_name}";
        let f = fill(t, &values());
        assert_eq!(
            f.text,
            "Hartley Builders|1042|11/03/2026|12/03/2026|$3,234.87|$3,234.87|Kisaes Garden Supply"
        );
        assert_eq!(f.unknown, Vec::<String>::new());
        for (field, name) in FIELDS {
            assert_eq!(field.name(), name);
            assert_eq!(Field::from_name(name), Some(field));
        }
    }

    #[test]
    fn braces_escape_and_unknown_fields_are_kept_and_reported() {
        let f = fill(
            "Invoice {{{invoice_number}}} {job} {job} {client name} { } {} }{ {total",
            &values(),
        );
        assert_eq!(
            f.text,
            "Invoice {1042} {job} {job} {client name} { } {} }{ {total"
        );
        assert_eq!(f.unknown, vec!["job".to_string()]);
        assert_eq!(
            unknown_fields("Hi {first_name}, {total}"),
            vec!["first_name"]
        );
    }

    #[test]
    fn values_are_never_expanded_again() {
        let mut v = values();
        v.client_name = "{total} {{x}}".into();
        assert_eq!(fill("{client_name}", &v).text, "{total} {{x}}");
    }

    #[test]
    fn bcc_self_is_on_and_the_default_texts_are_i18n_keys() {
        let p = EmailPrefs::default();
        assert!(p.bcc_self);
        assert_eq!(
            p.template(TemplateDoc::Invoice),
            TemplateSource::Default {
                subject_key: "email.template.invoice.subject",
                body_key: "email.template.invoice.body",
            }
        );
        let old: EmailPrefs = serde_json::from_str("{}").expect("json");
        assert!(old.bcc_self, "a file from before P20 keeps the default");
    }

    #[test]
    fn templates_are_kept_trimmed_to_their_limits() {
        let mut p = EmailPrefs::default();
        p.set_template(
            TemplateDoc::Invoice,
            &EmailTemplate {
                subject: format!("Invoice\n{}", "s".repeat(400)),
                body: "b".repeat(BODY_MAX + 5),
            },
        );
        let TemplateSource::Custom(t) = p.template(TemplateDoc::Invoice) else {
            panic!("custom");
        };
        assert_eq!(t.subject.chars().count(), SUBJECT_MAX);
        assert!(t.subject.starts_with("Invoice s"));
        assert_eq!(t.body.chars().count(), BODY_MAX);
        assert!(!format!("{t:?}").contains("Invoice"));
        p.set_template(TemplateDoc::Invoice, &EmailTemplate::default());
        assert!(matches!(
            p.template(TemplateDoc::Invoice),
            TemplateSource::Default { .. }
        ));
    }

    proptest! {
        #[test]
        fn text_without_braces_is_unchanged(s in "[^{}]{0,200}") {
            let f = fill(&s, &values());
            prop_assert_eq!(f.text, s);
            prop_assert!(f.unknown.is_empty());
        }

        #[test]
        fn doubled_braces_always_read_back_as_the_text(s in "[a-z {}]{0,60}") {
            let escaped = s.replace('{', "{{").replace('}', "}}");
            prop_assert_eq!(fill(&escaped, &values()).text, s);
        }

        #[test]
        fn unknown_fields_stay_in_the_text(s in "\\PC{0,200}") {
            let f = fill(&s, &values());
            for u in &f.unknown {
                let written = format!("{{{u}}}");
                prop_assert!(f.text.contains(&written));
                prop_assert!(Field::from_name(u).is_none());
            }
        }
    }
}
