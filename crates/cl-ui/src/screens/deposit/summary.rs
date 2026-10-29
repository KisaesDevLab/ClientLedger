//! The deposit summary (Q-203, Q-P14d): the saved deposit as the bank slip
//! would list it. The model is `cl_model::forms::deposit::DepositSummary`
//! (the P19 print model); every figure is the saved record's and the total
//! is the engine's bank amount. Printing comes in P19.

use cl_model::forms::deposit::{DepositSummary, SummaryRow};
use eframe::egui::{RichText, Ui};

use super::DepositScreen;
use crate::forms::condensed;
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
                .map(|p| {
                    let (check_no, method) =
                        self.panel.seen.get(&p.source).cloned().unwrap_or_default();
                    SummaryRow {
                        label: name(p.contact),
                        detail: p.memo.clone().unwrap_or_default(),
                        check_no,
                        method,
                        amount: p.amount,
                    }
                })
                .collect(),
            other: r
                .lines
                .iter()
                // Received from is the payer, or blank (never the account).
                .map(|l| SummaryRow {
                    label: name(l.contact),
                    detail: l.memo.clone().unwrap_or_default(),
                    check_no: String::new(),
                    method: String::new(),
                    amount: l.amount,
                })
                .collect(),
            cash_back: r.cash_back.as_ref().map(|c| c.amount),
            total: r.total,
        })
    }

    /// Draws the summary; returns where its rows went (sizes only).
    pub(crate) fn summary_ui(
        &self,
        ui: &mut Ui,
        tokens: &Tokens,
    ) -> Option<condensed::CondensedReport> {
        let style = self.money_style;
        let Some(s) = self.summary() else {
            form::hint(ui, tokens, &t("deposit.summary.saved_only"));
            return None;
        };
        let mut out = None;
        // The condensed table (Q-267): Name · Memo · Amount, striped, the
        // amounts and the total right-aligned in one column.
        condensed::capped(ui, |ui| {
            form::card(ui, tokens, |ui| {
                form::heading(ui, tokens, &t("deposit.summary.title"));
                ui.label(format!("{} \u{b7} {}", Self::row_date(s.date), s.account));
                if !s.memo.is_empty() {
                    ui.label(RichText::new(&s.memo).color(tokens.text_secondary));
                }
                let fig = |r: &SummaryRow| r.amount.format(style);
                let ((), report) = condensed::condensed_rows(ui, "deposit/summary", |c| {
                    for (key, rows) in [
                        ("deposit.summary.payments", &s.items),
                        ("deposit.summary.other", &s.other),
                    ] {
                        if rows.is_empty() {
                            continue;
                        }
                        c.section(&t(key));
                        for r in rows {
                            c.figure(&r.label, &r.detail, &fig(r));
                        }
                    }
                    if let Some(cb) = s.cash_back {
                        c.figure(&t("deposit.summary.cash_back"), "", &cb.format(style));
                    }
                    c.total(&t("deposit.summary.total"), &s.total.format(style));
                });
                out = Some(report);
                if !s.parts_agree() {
                    form::hint(ui, tokens, &t("deposit.summary.parts_differ"));
                }
            });
        });
        out
    }
}
