//! Save as Template and Use (P15): the one hook every form that can be a
//! template implements, so the lanes add theirs without touching each
//! other's files.
//!
//! - **Save as Template** runs the form's own `check()` → draft path
//!   ([`TemplateSource::template_draft`]): a template can never hold what
//!   Save would refuse. The date is the only problem a template ignores.
//! - **Use** opens a fresh form of the template's kind
//!   ([`crate::company::CompanyEvent::OpenTemplate`]) and fills it with the
//!   reverse mapping Prev/Next already uses for posted records
//!   ([`TemplateSource::fill_from_template`]).
//!
//! No amount arithmetic happens here; amounts move inside the checked
//! drafts as exact `Money`.

use cl_api::{DraftWarning, TemplateDraft};
use cl_core::CalendarDate;
use cl_core::ids::TemplateId;
use cl_model::CommandId;

/// A form that can be saved as a template and filled from one.
pub trait TemplateSource {
    /// The form's own problem type (what its Save shows).
    type Problem;

    /// The form as a template draft, through the same check as Save (the
    /// date aside). `Err` holds the problems to show, as on Save.
    ///
    /// # Errors
    /// The form's problems; nothing is sent.
    fn template_draft(&self) -> Result<TemplateDraft, Vec<Self::Problem>>;

    /// Fills a fresh form from a template's draft. `warnings` name the
    /// references the engine dropped or found inactive; the form shows them
    /// as a banner.
    fn fill_from_template(&mut self, draft: TemplateDraft, warnings: &[DraftWarning]);
}

/// A template to open in its form (Templates list "Use", Templates Due
/// "Open in form").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemplateOpen {
    /// The form's window kind (the card form for a card charge or credit).
    pub kind: CommandId,
    /// The template, so the form can offer "Replace" on Save as Template.
    pub template: TemplateId,
    /// Its version when it was read (Replace sends it; 409 `STALE` else).
    pub version: u32,
    /// The occurrence this fills in (Templates Due "Open in form"): after
    /// the form saves, the occurrence is resolved with the new transaction.
    pub occurrence: Option<CalendarDate>,
    /// The draft, dated by the form.
    pub draft: TemplateDraft,
    /// What the engine flagged in the draft.
    pub warnings: Vec<DraftWarning>,
}

/// How a form's Save as Template button shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SaveAsTemplate {
    /// Not shown (the form has no template kind).
    Hidden,
    /// Shown but off; the text says why (no `templates-v1`, a grouped
    /// deposit).
    Disabled(String),
    /// Shown and on.
    Enabled,
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Form {
        filled: Option<(TemplateDraft, usize)>,
        ok: bool,
    }

    impl TemplateSource for Form {
        type Problem = &'static str;

        fn template_draft(&self) -> Result<TemplateDraft, Vec<&'static str>> {
            if !self.ok {
                return Err(vec!["needs an account"]);
            }
            let a = crate::company::demo::account_id(1010);
            let b = crate::company::demo::account_id(1020);
            let date = CalendarDate::from_ymd(2026, 11, 4).expect("date");
            Ok(TemplateDraft::Transfer(cl_api::TransferDraft {
                from: a,
                to: b,
                date,
                amount: cl_core::Money::parse_wire("25.00").expect("m"),
                memo: None,
            }))
        }

        fn fill_from_template(&mut self, draft: TemplateDraft, warnings: &[DraftWarning]) {
            self.filled = Some((draft, warnings.len()));
        }
    }

    #[test]
    fn save_and_use_go_through_the_form() {
        let mut f = Form {
            filled: None,
            ok: false,
        };
        assert_eq!(f.template_draft(), Err(vec!["needs an account"]));
        f.ok = true;
        let d = f.template_draft().expect("draft");
        let warn = DraftWarning {
            field: "fromAccountId".into(),
            code: cl_api::DraftWarningCode::AccountInactive,
        };
        f.fill_from_template(d.clone(), std::slice::from_ref(&warn));
        assert_eq!(f.filled, Some((d, 1)));
    }
}
