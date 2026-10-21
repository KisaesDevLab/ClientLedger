//! Save as Template and Use on Record Card Charge / Credit (P15 T4). The
//! template is the form's own checked charge or credit (`check`, as on
//! Save) mapped field by field as the save maps it: a charge is an
//! expense paid from the card (`card_charge`), a credit a deposit into the
//! card (`card_credit`). The reference number is never kept (Q-P15f).

use std::sync::Arc;

use cl_api::{CardChargeDraft, DepositDraft, DraftWarning, SplitLine, TemplateDraft, client_form};
use cl_model::forms::card::{CardForm, CardMode, CardProblem, check};
use cl_model::forms::lines::PostingLine;
use eframe::egui::Ui;

use super::CardChargeScreen;
use crate::forms::template::{TemplateOpen, TemplateSource};
use crate::screens::templates::{form_lines, real_account};

fn split(l: &PostingLine) -> SplitLine {
    SplitLine {
        account: l.account,
        amount: l.amount,
        memo: l.memo.clone(),
        contact: l.contact,
        billable: l.billable,
        tag: l.tag,
    }
}

impl TemplateSource for CardChargeScreen {
    type Problem = CardProblem;

    fn template_draft(&self) -> Result<TemplateDraft, Vec<CardProblem>> {
        let c = check(&self.compose(), &self.books.tag_list)?;
        Ok(match c.mode {
            CardMode::Charge => TemplateDraft::CardCharge(CardChargeDraft {
                pay_from: c.card,
                date: c.date,
                number: None,
                payee: c.payee,
                memo: c.memo.clone(),
                lines: c.lines.iter().map(split).collect(),
            }),
            // As the save: a credit's lines name the payee when they have
            // no customer of their own, and none is billable.
            CardMode::Credit => TemplateDraft::Deposit(DepositDraft::plain(
                c.card,
                c.date,
                c.memo.clone(),
                c.lines
                    .iter()
                    .map(|l| SplitLine {
                        contact: l.contact.or(c.payee),
                        billable: false,
                        ..split(l)
                    })
                    .collect(),
            )),
        })
    }

    fn fill_from_template(&mut self, draft: TemplateDraft, _warnings: &[DraftWarning]) {
        let (mode, card, payee, memo, lines, amount, date) = match &draft {
            TemplateDraft::CardCharge(d) => (
                CardMode::Charge,
                d.pay_from,
                d.payee,
                d.memo.clone(),
                d.lines.clone(),
                cl_core::Money::sum(d.lines.iter().map(|l| l.amount)).ok(),
                d.date,
            ),
            TemplateDraft::Deposit(d) => (
                CardMode::Credit,
                d.account,
                None,
                d.memo.clone(),
                d.lines.clone(),
                d.total(),
                d.date,
            ),
            _ => return,
        };
        self.new_form(false);
        let rows = form_lines(&lines, &mut self.lines.form_mut().adapter_mut().keys);
        let card = real_account(card).or(self.card());
        let mut f = CardForm::new(card, Some(date));
        f.mode = mode;
        f.payee = payee;
        f.memo = memo.unwrap_or_default();
        f.amount = amount;
        self.header.edit_step(|h| *h = f);
        self.lines.form_mut().edit_step(|v| *v = rows);
        self.lock_columns();
        self.sync_inputs();
        if let Some(card) = card {
            self.balances.fetch(&self.link, self.group.as_ref(), card);
        }
    }
}

impl CardChargeScreen {
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
            Err(p) => self.refuse(p),
            Ok(d) => {
                self.problems.clear();
                let form = match self.header.current().mode {
                    CardMode::Charge => client_form::CARD_CHARGE,
                    CardMode::Credit => client_form::CARD_CREDIT,
                };
                let name = self
                    .header
                    .current()
                    .payee
                    .and_then(|p| self.books.contact_name(p))
                    .unwrap_or_default();
                let link = Arc::clone(&self.link);
                self.template
                    .start(&link, self.group.as_ref(), d, form, name, self.today);
            }
        }
    }

    /// Use: a fresh charge or credit filled from the template.
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
    use cl_model::forms::lines::{FormLine, FormLines};

    use super::*;
    use crate::company::CompanyLink;
    use crate::company::demo::template_store;
    use crate::windows::Screen;

    fn m(s: &str) -> Money {
        Money::parse_wire(s).expect("m")
    }

    fn use_it(link: &Arc<CompanyLink>, today: CalendarDate, name: &str) -> CardChargeScreen {
        let rows = link
            .demo_state()
            .map(|st| template_store::list(&st))
            .expect("demo");
        let row = rows.iter().find(|r| r.name == name).expect("row");
        let used = link
            .demo_state()
            .map(|mut st| template_store::draft(&mut st, row.id, today))
            .expect("demo")
            .expect("use");
        let mut fresh = CardChargeScreen::new(Arc::clone(link), None, today);
        fresh.poll();
        assert!(fresh.open_template(TemplateOpen {
            kind: cl_model::CommandId::new(super::super::KIND),
            template: row.id,
            version: row.version,
            occurrence: None,
            draft: used.draft,
            warnings: used.warnings,
        }));
        fresh
    }

    #[test]
    fn charge_and_credit_templates_reopen_in_their_mode() {
        let dir = tempfile::tempdir().expect("tmp");
        let link = Arc::new(CompanyLink::demo(dir.path()));
        if let Some(mut s) = link.demo_state() {
            s.open = s.companies.first().map(|c| c.tenant_id);
        }
        let today = CalendarDate::from_ymd(2026, 11, 4).expect("date");
        for (mode, name) in [(CardMode::Charge, "Software"), (CardMode::Credit, "Refund")] {
            let mut s = CardChargeScreen::new(Arc::clone(&link), None, today);
            s.poll();
            s.set_mode(mode);
            let keys = &mut s.lines.form_mut().adapter_mut().keys;
            let lines = FormLines::from_lines(
                vec![FormLine {
                    account: Some(crate::company::demo::account_id(6000)),
                    amount: Some(m("49.00")),
                    ..FormLine::blank(keys.next_key())
                }],
                keys,
            );
            s.lines.form_mut().edit_step(|v| *v = lines);
            s.left_lines();
            assert!(s.command(cl_model::CommandId::new("form.save_as_template")));
            assert!(s.problems.is_empty(), "{:?}", s.problems);
            let dlg = s.template.dialog_mut().expect("dialog");
            assert!(dlg.set("name", name));
            assert!(s.template.dialog_save(&link, None));
            let mut fresh = use_it(&link, today, name);
            assert_eq!(fresh.header.current().mode, mode);
            assert_eq!(fresh.header.current().amount, Some(m("49.00")));
            assert!(fresh.is_dirty());
            assert!(
                fresh.start_save(SaveIntent::Save, None),
                "{:?}",
                fresh.problems
            );
        }
        let rows = link
            .demo_state()
            .map(|st| template_store::list(&st))
            .expect("demo");
        let forms: Vec<_> = rows.iter().map(|r| r.client_form.clone()).collect();
        assert!(forms.contains(&Some("card_credit".into())));
        assert!(forms.contains(&Some("card_charge".into())));
    }
}
