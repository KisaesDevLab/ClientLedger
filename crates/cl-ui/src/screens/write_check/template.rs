//! Save as Template and Use on Write Check (P15 T4). The template is the
//! form's own checked check (`check` then `to_draft`, as on Save) without
//! its number or receipt: a template never keeps a check number (Q-P15f).
//! Use fills a fresh, unsaved check; its number comes from the bank
//! account's next number, or "To print".

use std::sync::Arc;

use cl_api::{CheckDraft, DraftWarning, TemplateDraft, client_form};
use cl_model::forms::check::{CheckContext, CheckProblem, check};
use eframe::egui::Ui;

use super::WriteCheckScreen;
use super::save::to_draft;
use crate::forms::template::{TemplateOpen, TemplateSource};
use crate::screens::templates::{form_lines, real_account};

impl TemplateSource for WriteCheckScreen {
    type Problem = CheckProblem;

    fn template_draft(&self) -> Result<TemplateDraft, Vec<CheckProblem>> {
        let form = self.compose();
        let ctx = CheckContext {
            tags: &self.tag_list,
            editing: self.editing.is_some(),
        };
        let mut c = check(&form, &ctx)?;
        // A template keeps no receipt and no number.
        c.draft_attachment = None;
        c.number = None;
        let display = self
            .lists
            .name_text(c.payee)
            .unwrap_or_default()
            .to_string();
        to_draft(&c, &display)
            .map(TemplateDraft::Check)
            .map_err(|_| Vec::new())
    }

    fn fill_from_template(&mut self, draft: TemplateDraft, _warnings: &[DraftWarning]) {
        let TemplateDraft::Check(d) = draft else {
            return;
        };
        self.new_check(false);
        self.fill_check(&d);
    }
}

impl WriteCheckScreen {
    fn fill_check(&mut self, d: &CheckDraft) {
        let lines = form_lines(&d.lines, &mut self.lines.form_mut().adapter_mut().keys);
        let bank = real_account(d.bank).or(self.bank());
        self.header.edit_step(|h| {
            h.bank = bank;
            h.date = Some(d.date);
            h.set_print_later(d.print_later);
            h.payee = d.payee;
            h.payee_name.clone_from(&d.payee_name);
            h.address = d.payee_address.clone().unwrap_or_default();
            h.amount = Some(d.amount);
            h.amount_from_lines = false;
            h.memo = d.memo.clone().unwrap_or_default();
            h.header_tags.clone_from(&d.tag_ids);
        });
        self.lines.form_mut().edit_step(|v| *v = lines);
        self.sync_inputs();
        // The bank's balance and next number (a template keeps none).
        self.fetch_bank_facts();
    }

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
        if !self.commit_header() || !self.lines.commit() {
            return;
        }
        self.left_lines();
        match self.template_draft() {
            Err(p) => self.save.problems = p,
            Ok(d) => {
                self.save.problems.clear();
                let name = match &d {
                    TemplateDraft::Check(c) => c.payee_name.clone(),
                    _ => String::new(),
                };
                let link = Arc::clone(&self.link);
                self.template.start(
                    &link,
                    self.group.as_ref(),
                    d,
                    client_form::WRITE_CHECK,
                    name,
                    self.today,
                );
            }
        }
    }

    /// Use: a fresh check filled from the template.
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
    use cl_model::forms::check::NumberField;

    use super::*;
    use crate::company::CompanyLink;
    use crate::company::demo::template_store;
    use crate::windows::Screen;

    #[test]
    fn a_check_template_keeps_no_number_and_use_fills_a_new_check() {
        let dir = tempfile::tempdir().expect("tmp");
        let link = Arc::new(CompanyLink::demo(dir.path()));
        if let Some(mut s) = link.demo_state() {
            s.open = s.companies.first().map(|c| c.tenant_id);
        }
        let today = CalendarDate::from_ymd(2026, 11, 4).expect("date");
        let mut s = WriteCheckScreen::demo_draft(Arc::clone(&link), None, today);
        s.poll();
        assert!(s.command(cl_model::CommandId::new("form.save_as_template")));
        assert!(s.save.problems.is_empty(), "{:?}", s.save.problems);
        assert!(s.state_json()["template"]["dialog"].is_object());
        assert!(s.template.dialog_save(&link, None), "the payee names it");
        let rows = link
            .demo_state()
            .map(|st| template_store::list(&st))
            .expect("demo");
        assert_eq!(
            rows[0].total,
            Some(Money::parse_wire("1286.40").expect("m"))
        );
        let TemplateDraft::Check(stored) = rows[0].draft_for(today).expect("draft") else {
            panic!("a check");
        };
        assert_eq!(stored.check_number, None, "Q-P15f");
        assert_eq!(stored.lines.len(), 2);
        let used = link
            .demo_state()
            .map(|mut st| template_store::draft(&mut st, rows[0].id, today))
            .expect("demo")
            .expect("use");
        let mut fresh = WriteCheckScreen::new(Arc::clone(&link), None, today);
        fresh.poll();
        assert!(fresh.open_template(TemplateOpen {
            kind: cl_model::CommandId::new(super::super::KIND),
            template: rows[0].id,
            version: rows[0].version,
            occurrence: None,
            draft: used.draft,
            warnings: used.warnings,
        }));
        assert!(fresh.is_dirty());
        let f = fresh.compose();
        assert_eq!(f.amount, Some(Money::parse_wire("1286.40").expect("m")));
        assert_eq!(f.lines.used().count(), 2);
        assert!(!matches!(f.number, NumberField::Typed(ref n) if n == "4418"));
        assert!(
            fresh.start_save(SaveIntent::Save, None),
            "{:?}",
            fresh.save.error
        );
        assert_eq!(fresh.save.count, 1);
    }
}
