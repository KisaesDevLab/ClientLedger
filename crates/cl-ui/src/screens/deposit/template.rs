//! Save as Template and Use on Make Deposit (P15 T4). The template is the
//! form's own checked deposit (`check` then `to_draft`, as on Save): other
//! lines, fees and cash back are kept; a deposit that takes customer
//! payments in whole can't be a template (Q-P15e: the button is off with
//! its reason, and cl-api refuses it again with nothing sent).

use std::sync::Arc;

use cl_api::{DepositDraft, DraftWarning, TemplateDraft, client_form};
use cl_model::forms::deposit::{CashBackForm, DepositProblem, check};
use cl_model::forms::lines::{FormLine, FormLines};
use eframe::egui::Ui;

use super::DepositScreen;
use super::save::to_draft;
use crate::forms::template::{TemplateOpen, TemplateSource};
use crate::i18n::t;
use crate::screens::templates::real_account;

impl TemplateSource for DepositScreen {
    type Problem = DepositProblem;

    fn template_draft(&self) -> Result<TemplateDraft, Vec<DepositProblem>> {
        let control = self.control_accounts();
        let c = check(&self.compose(), &self.context(&control))?;
        to_draft(&c, self.clearing())
            .map(TemplateDraft::Deposit)
            .map_err(|_| Vec::new())
    }

    fn fill_from_template(&mut self, draft: TemplateDraft, _warnings: &[DraftWarning]) {
        let TemplateDraft::Deposit(d) = draft else {
            return;
        };
        self.new_deposit(false);
        self.fill_deposit(&d);
    }
}

impl DepositScreen {
    fn fill_deposit(&mut self, d: &DepositDraft) {
        let keys = &mut self.lines.form_mut().adapter_mut().keys;
        // Other lines as typed; a fee goes back to its negative line.
        let mut seeds: Vec<FormLine> = Vec::new();
        for (l, fee) in d
            .lines
            .iter()
            .map(|l| (l, false))
            .chain(d.deductions.iter().map(|l| (l, true)))
        {
            let amount = if fee {
                l.amount.checked_neg().ok()
            } else {
                Some(l.amount)
            };
            seeds.push(FormLine {
                account: real_account(l.account),
                amount,
                memo: l.memo.clone().unwrap_or_default(),
                customer: l.contact,
                tag: l.tag,
                ..FormLine::blank(keys.next_key())
            });
        }
        let lines = FormLines::from_lines(seeds, keys);
        let account = real_account(d.account).or(self.account());
        self.header.edit_step(|h| {
            h.account = account;
            h.date = Some(d.date);
            h.memo = d.memo.clone().unwrap_or_default();
            h.cash_back = d
                .cash_back
                .as_ref()
                .map_or_else(CashBackForm::default, |c| CashBackForm {
                    account: real_account(c.account),
                    amount: Some(c.amount),
                    memo: c.memo.clone().unwrap_or_default(),
                });
        });
        self.lines.form_mut().edit_step(|v| *v = lines);
        self.sync_inputs();
    }

    /// Why Save as Template is off: a deposit that takes payments.
    fn template_blocked(&self) -> Option<String> {
        (!self.header.current().picked.is_empty()).then(|| t("template.save.grouped"))
    }

    /// The toolbar's Save as Template slot.
    pub(crate) fn template_slot(&mut self, ui: &mut Ui) {
        let state = self.template.button(&self.link, self.template_blocked());
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
        if self.template_blocked().is_some() {
            self.save.notice = Some("template.save.grouped");
            return;
        }
        if !self.commit_header() || !self.lines.commit() {
            return;
        }
        match self.template_draft() {
            Err(p) => self.save.problems = p,
            Ok(d) => {
                self.save.problems.clear();
                let link = Arc::clone(&self.link);
                self.template.start(
                    &link,
                    self.group.as_ref(),
                    d,
                    client_form::DEPOSIT,
                    String::new(),
                    self.today,
                );
            }
        }
    }

    /// Use: a fresh deposit filled from the template.
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
    use crate::forms::template::SaveAsTemplate;
    use crate::windows::Screen;

    fn m(s: &str) -> Money {
        Money::parse_wire(s).expect("m")
    }

    #[test]
    fn a_deposit_with_a_fee_round_trips_and_grouped_payments_turn_the_button_off() {
        let dir = tempfile::tempdir().expect("tmp");
        let link = Arc::new(CompanyLink::demo(dir.path()));
        if let Some(mut s) = link.demo_state() {
            s.open = s.companies.first().map(|c| c.tenant_id);
        }
        let today = CalendarDate::from_ymd(2026, 11, 4).expect("date");
        let mut s = DepositScreen::new(Arc::clone(&link), None, today);
        s.poll();
        let income = crate::company::demo::account_id(4000);
        let fees = crate::company::demo::account_id(6250);
        let keys = &mut s.lines.form_mut().adapter_mut().keys;
        let lines = FormLines::from_lines(
            vec![
                FormLine {
                    account: Some(income),
                    amount: Some(m("500.00")),
                    ..FormLine::blank(keys.next_key())
                },
                FormLine {
                    account: Some(fees),
                    amount: Some(m("-12.50")),
                    ..FormLine::blank(keys.next_key())
                },
            ],
            keys,
        );
        s.lines.form_mut().edit_step(|v| *v = lines);
        assert!(s.command(cl_model::CommandId::new("form.save_as_template")));
        assert!(
            s.save.problems.is_empty() && s.save.error.is_none(),
            "{:?} {:?}",
            s.save.problems,
            s.save.error
        );
        let dlg = s.template.dialog_mut().expect("dialog");
        assert!(dlg.set("name", "Weekly"));
        assert!(s.template.dialog_save(&link, None));
        let rows = link
            .demo_state()
            .map(|st| template_store::list(&st))
            .expect("demo");
        assert_eq!(rows[0].total, Some(m("487.50")), "exact: 500.00 - 12.50");
        let used = link
            .demo_state()
            .map(|mut st| template_store::draft(&mut st, rows[0].id, today))
            .expect("demo")
            .expect("use");
        let mut fresh = DepositScreen::new(Arc::clone(&link), None, today);
        fresh.poll();
        assert!(fresh.open_template(TemplateOpen {
            kind: cl_model::CommandId::new(super::super::KIND),
            template: rows[0].id,
            version: rows[0].version,
            occurrence: None,
            draft: used.draft,
            warnings: used.warnings,
        }));
        let amounts: Vec<_> = fresh.lines.value().used().map(|l| l.amount).collect();
        assert_eq!(amounts, vec![Some(m("500.00")), Some(m("-12.50"))]);
        assert!(
            fresh.start_save(SaveIntent::Save, None),
            "{:?}",
            fresh.save.error
        );
        // A picked payment turns Save as Template off with its reason.
        let mut grouped = DepositScreen::new(Arc::clone(&link), None, today);
        grouped.poll();
        grouped.header.edit_step(|h| {
            h.picked.push(cl_model::forms::deposit::PickedPayment {
                source: cl_core::ids::TransactionId::parse("d0000000-0000-4000-8000-b00000000001")
                    .expect("id"),
                amount: m("10.00"),
                contact: None,
            });
        });
        assert_eq!(
            grouped.template.button(&link, grouped.template_blocked()),
            SaveAsTemplate::Disabled(t("template.save.grouped"))
        );
        grouped.save_as_template();
        assert_eq!(grouped.save.notice, Some("template.save.grouped"));
        assert!(grouped.state_json()["template"]["dialog"].is_null());
    }
}
