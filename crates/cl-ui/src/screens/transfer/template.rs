//! Save as Template and Use on Transfer Funds (P15 T4): the template is
//! the form's own checked transfer (`check`, as on Save); Use fills a
//! fresh, unsaved form from the template's draft.

use std::sync::Arc;

use cl_api::{DraftWarning, MISSING_ACCOUNT, TemplateDraft, TransferDraft, client_form};
use cl_core::ids::AccountId;
use cl_model::forms::transfer::{TransferForm, TransferProblem, check};
use eframe::egui::Ui;

use super::TransferScreen;
use crate::forms::template::{TemplateOpen, TemplateSource};

fn real(a: AccountId) -> Option<AccountId> {
    (a != MISSING_ACCOUNT).then_some(a)
}

impl TemplateSource for TransferScreen {
    type Problem = TransferProblem;

    fn template_draft(&self) -> Result<TemplateDraft, Vec<TransferProblem>> {
        let c = check(self.form.current())?;
        Ok(TemplateDraft::Transfer(TransferDraft {
            from: c.from,
            to: c.to,
            date: c.date,
            amount: c.amount,
            memo: c.memo,
        }))
    }

    fn fill_from_template(&mut self, draft: TemplateDraft, _warnings: &[DraftWarning]) {
        let TemplateDraft::Transfer(d) = draft else {
            return;
        };
        self.new_transfer(false);
        let f = TransferForm {
            from: real(d.from),
            to: real(d.to),
            date: Some(d.date),
            amount: Some(d.amount),
            memo: d.memo.unwrap_or_default(),
        };
        // An unsaved form the owner can change and save.
        self.form.edit_step(|v| *v = f);
        self.sync_inputs();
        self.fetch_balances();
    }
}

impl TransferScreen {
    /// The toolbar's Save as Template slot.
    pub(crate) fn template_slot(&mut self, ui: &mut Ui) {
        let state = self.template.button(&self.link, None);
        let link = Arc::clone(&self.link);
        if self
            .template
            .slot(ui, &link, self.group.as_ref(), &state, self.today)
        {
            self.save_as_template();
        }
    }

    /// Save as Template (Ctrl+M): the form's problems show as on Save.
    pub(crate) fn save_as_template(&mut self) {
        if self.rev == self.form.revision() && !self.amount_left() {
            return;
        }
        match self.template_draft() {
            Err(p) => self.problems = p,
            Ok(d) => {
                self.problems.clear();
                let link = Arc::clone(&self.link);
                self.template.start(
                    &link,
                    self.group.as_ref(),
                    d,
                    client_form::TRANSFER,
                    String::new(),
                    self.today,
                );
            }
        }
    }

    /// Use: a fresh form filled from the template.
    pub(crate) fn fill_template(&mut self, t: &TemplateOpen) -> bool {
        self.fill_from_template(t.draft.clone(), &t.warnings);
        self.template.opened(t);
        true
    }
}

#[cfg(test)]
mod tests {
    use cl_core::{CalendarDate, Money};
    use cl_model::forms::SaveIntent;

    use super::*;
    use crate::company::CompanyLink;
    use crate::company::demo::template_store;
    use crate::windows::Screen;

    fn account(n: u32) -> AccountId {
        crate::company::demo::account_id(n)
    }

    #[test]
    fn save_as_template_then_use_fills_an_unsaved_form() {
        let dir = tempfile::tempdir().expect("tmp");
        let link = Arc::new(CompanyLink::demo(dir.path()));
        if let Some(mut s) = link.demo_state() {
            s.open = s.companies.first().map(|c| c.tenant_id);
        }
        let today = CalendarDate::from_ymd(2026, 11, 4).expect("date");
        let mut s = TransferScreen::new(Arc::clone(&link), None, today);
        // A form Save would refuse can't be a template either.
        s.save_as_template();
        assert!(!s.problems.is_empty(), "refused as on Save");
        s.set_account(true, account(1010));
        s.set_account(false, account(2050));
        s.amount_text = "300.00".into();
        assert!(s.command(cl_model::CommandId::new("form.save_as_template")));
        assert!(s.problems.is_empty(), "{:?}", s.problems);
        let v = s.state_json();
        assert!(v["template"]["dialog"].is_object(), "{v}");
        // The dialog's Save stores it (nothing posts).
        let saved_before = s.saved_count;
        assert!(!s.template.dialog_save(&link, None), "a name is required");
        let dlg = s.template.dialog_mut().expect("dialog");
        assert!(dlg.set("name", "Savings"));
        assert!(s.template.dialog_save(&link, None));
        assert_eq!(s.saved_count, saved_before);
        let rows = link
            .demo_state()
            .map(|st| template_store::list(&st))
            .expect("demo");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].total, Some(Money::parse_wire("300").expect("m")));
        // Use: a fresh form, dirty, offering Replace.
        let open = link
            .demo_state()
            .map(|mut st| template_store::draft(&mut st, rows[0].id, today))
            .expect("demo")
            .expect("draft");
        let mut fresh = TransferScreen::new(Arc::clone(&link), None, today);
        assert!(fresh.open_template(TemplateOpen {
            kind: cl_model::CommandId::new(super::super::KIND),
            template: rows[0].id,
            version: rows[0].version,
            occurrence: None,
            draft: open.draft,
            warnings: open.warnings,
        }));
        assert!(fresh.is_dirty());
        assert_eq!(fresh.form.current().from, Some(account(1010)));
        assert_eq!(fresh.amount_text, "300.00");
        assert_eq!(
            fresh.state_json()["template"]["from_template"],
            rows[0].id.to_string()
        );
        assert!(
            fresh.start_save(SaveIntent::Save, None),
            "{:?}",
            fresh.error
        );
        assert_eq!(fresh.saved_count, 1);
    }
}
