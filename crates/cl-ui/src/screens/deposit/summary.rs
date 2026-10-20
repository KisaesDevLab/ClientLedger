//! The deposit summary (Q-203, Q-P14d): the saved deposit as the bank slip
//! would list it. The model is `cl_model::forms::deposit::DepositSummary`
//! (the P19 print model); every figure is the saved record's and the total
//! is the engine's bank amount. Printing comes in P19.

use cl_model::forms::deposit::{DepositSummary, SummaryRow};
use eframe::egui::{self, RichText, Ui};

use super::DepositScreen;
use crate::i18n::t;
use crate::screens::company::form;
use crate::theme::Tokens;

impl DepositScreen {
    /// The summary of the deposit in front, when it is a saved one.
    pub(crate) fn summary(&self) -> Option<DepositSummary> {
        let r = self.record.as_ref()?;
        let name = |c: Option<cl_core::ids::ContactId>| {
            c.and_then(|c| self.lists.name_text(c))
                .unwrap_or_default()
                .to_string()
        };
        let account = |a| self.lists.account_text(a).unwrap_or_default().to_string();
        Some(DepositSummary {
            date: r.txn.txn_date,
            account: account(r.account),
            memo: r.txn.memo.clone().unwrap_or_default(),
            items: r
                .payments
                .iter()
                .map(|p| SummaryRow {
                    label: name(p.contact),
                    detail: p.memo.clone().unwrap_or_default(),
                    amount: p.amount,
                })
                .collect(),
            other: r
                .lines
                .iter()
                .map(|l| SummaryRow {
                    label: if l.contact.is_some() {
                        name(l.contact)
                    } else {
                        account(l.account)
                    },
                    detail: l.memo.clone().unwrap_or_default(),
                    amount: l.amount,
                })
                .collect(),
            cash_back: r.cash_back.as_ref().map(|c| c.amount),
            total: r.total,
        })
    }

    /// Draws the summary.
    pub(crate) fn summary_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let style = self.money_style;
        let Some(s) = self.summary() else {
            form::hint(ui, tokens, &t("deposit.summary.saved_only"));
            return;
        };
        form::card(ui, tokens, |ui| {
            form::heading(ui, tokens, &t("deposit.summary.title"));
            ui.label(format!("{} \u{b7} {}", Self::row_date(s.date), s.account));
            if !s.memo.is_empty() {
                ui.label(RichText::new(&s.memo).color(tokens.text_secondary));
            }
            let row = |ui: &mut Ui, r: &SummaryRow| {
                ui.label(&r.label);
                ui.label(RichText::new(&r.detail).color(tokens.text_secondary));
                ui.label(
                    RichText::new(r.amount.format(style)).text_style(egui::TextStyle::Monospace),
                );
                ui.end_row();
            };
            egui::Grid::new("deposit/summary")
                .num_columns(3)
                .striped(true)
                .show(ui, |ui| {
                    if !s.items.is_empty() {
                        ui.label(RichText::new(t("deposit.summary.payments")).strong());
                        ui.end_row();
                        for r in &s.items {
                            row(ui, r);
                        }
                    }
                    if !s.other.is_empty() {
                        ui.label(RichText::new(t("deposit.summary.other")).strong());
                        ui.end_row();
                        for r in &s.other {
                            row(ui, r);
                        }
                    }
                    if let Some(c) = s.cash_back {
                        ui.label(t("deposit.summary.cash_back"));
                        ui.label("");
                        ui.label(
                            RichText::new(c.format(style)).text_style(egui::TextStyle::Monospace),
                        );
                        ui.end_row();
                    }
                    ui.label(RichText::new(t("deposit.summary.total")).strong());
                    ui.label("");
                    ui.label(
                        RichText::new(s.total.format(style))
                            .strong()
                            .text_style(egui::TextStyle::Monospace),
                    );
                    ui.end_row();
                });
            if !s.parts_agree() {
                form::hint(ui, tokens, &t("deposit.summary.parts_differ"));
            }
        });
    }
}
