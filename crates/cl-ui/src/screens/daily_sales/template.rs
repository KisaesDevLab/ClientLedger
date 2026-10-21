//! Save as Template and Use on the Daily Sales Summary (P15 T4, Q-P15d):
//! the template keeps the layout, the amounts, the day's tag and the notes
//! (the form's own `check`, as on Save). It is never entered
//! automatically: the dialog offers Remind me and Don't remind me only.

use std::sync::Arc;

use cl_api::{DailyEntryDraft, DraftWarning, MISSING_LAYOUT, TemplateDraft, client_form};
use cl_model::forms::daily_sales::{DailyProblem, check};
use eframe::egui::Ui;

use super::DailySalesScreen;
use super::save::to_draft;
use crate::forms::template::{TemplateOpen, TemplateSource};

impl TemplateSource for DailySalesScreen {
    type Problem = DailyProblem;

    fn template_draft(&self) -> Result<TemplateDraft, Vec<DailyProblem>> {
        let c = check(self.header.current(), &self.layout)?;
        Ok(TemplateDraft::DailySales(to_draft(&c)))
    }

    fn fill_from_template(&mut self, draft: TemplateDraft, _warnings: &[DraftWarning]) {
        let TemplateDraft::DailySales(d) = draft else {
            return;
        };
        self.new_day(d.date);
        self.fill_day(&d);
    }
}

impl DailySalesScreen {
    fn fill_day(&mut self, d: &DailyEntryDraft) {
        // The layout must still exist (else the warning banner says so).
        if d.template != MISSING_LAYOUT {
            self.pick_layout(d.template);
        }
        self.header.edit_step(|h| {
            h.date = Some(d.date);
            h.tag = d.tag;
            h.notes = d.notes.clone().unwrap_or_default();
            h.values = d.values.iter().map(|v| (v.line, Some(v.amount))).collect();
        });
        self.inputs.rev = u64::MAX;
    }

    /// The toolbar's Save as Template slot.
    pub(crate) fn template_slot(&mut self, ui: &mut Ui) {
        let state = self.save_template.button(&self.link, None);
        let link = Arc::clone(&self.link);
        if self
            .save_template
            .slot(ui, &link, self.group.as_ref(), &state, self.today)
        {
            self.save_as_template();
        }
    }

    /// Save as Template (Ctrl+M): the form's problems show as on Save.
    pub(crate) fn save_as_template(&mut self) {
        match self.template_draft() {
            Err(p) => self.save.problems = p,
            Ok(d) => {
                self.save.problems.clear();
                let name = self.template_layout_name().unwrap_or_default();
                let link = Arc::clone(&self.link);
                self.save_template.start(
                    &link,
                    self.group.as_ref(),
                    d,
                    client_form::DAILY_SALES,
                    name,
                    self.today,
                );
            }
        }
    }

    fn template_layout_name(&self) -> Option<String> {
        let id = self.header.current().template?;
        self.templates
            .iter()
            .find(|t| t.id == id)
            .map(|t| t.name.clone())
    }

    /// Use: a fresh day filled from the template.
    pub(crate) fn fill_template(&mut self, t: &TemplateOpen) -> bool {
        self.fill_from_template(t.draft.clone(), &t.warnings);
        self.save_template.opened(t);
        true
    }
}

#[cfg(test)]
mod tests {
    use cl_core::CalendarDate;

    use super::*;
    use crate::windows::Screen;

    #[test]
    fn a_daily_sales_template_never_enters_automatically() {
        let dir = tempfile::tempdir().expect("tmp");
        let link = Arc::new(crate::company::CompanyLink::demo(dir.path()));
        if let Some(mut s) = link.demo_state() {
            s.open = s.companies.first().map(|c| c.tenant_id);
        }
        let today = CalendarDate::from_ymd(2026, 11, 4).expect("date");
        let mut s = DailySalesScreen::new(Arc::clone(&link), None, today);
        for _ in 0..3 {
            s.poll();
        }
        // An empty day (no layout yet) is refused as on Save.
        if s.header.current().template.is_none() {
            s.save_as_template();
            assert!(!s.save.problems.is_empty(), "refused as on Save");
            return;
        }
        assert!(s.command(cl_model::CommandId::new("form.save_as_template")));
        let v = s.state_json();
        if v["template"]["dialog"].is_object() {
            assert_eq!(
                v["template"]["dialog"]["modes"],
                serde_json::json!(["remind", "never"]),
                "Q-P15d"
            );
        } else {
            assert!(!s.save.problems.is_empty(), "refused as on Save");
        }
    }
}
