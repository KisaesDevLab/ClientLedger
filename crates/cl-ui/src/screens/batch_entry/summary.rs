//! After Save all (Q-P15Al): the rows saved per type, the totals per
//! account (sums of the owner's own amounts, exact), and "Open register"
//! per bank or card account. The registers show the engine's balances; no
//! balance is worked out here (non-negotiable 7).

use cl_core::ids::AccountId;
use cl_core::{Money, MoneyStyle};
use cl_model::forms::batch::{BulkBatch, SectionKind};
use eframe::egui::{self, RichText, Ui};
use serde_json::{Value, json};

use super::BulkEntryScreen;
use super::sections::kind_key;
use crate::company::CompanyEvent;
use crate::i18n::{t, t_count, t_with};
use crate::screens::card_charge::kit;
use crate::screens::company::form;
use crate::theme::Tokens;

/// What one Save all posted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Summary {
    /// Rows saved.
    pub rows: usize,
    /// Rows per type.
    pub counts: Vec<(SectionKind, usize)>,
    /// The owner's amounts per account and type.
    pub totals: Vec<(AccountId, SectionKind, Money)>,
    /// Journal entry lines and their debits (= credits).
    pub journal: Option<(usize, Money)>,
    /// Bank and card accounts with an Open register link.
    pub registers: Vec<AccountId>,
    /// Names added to the lists.
    pub created_contacts: u32,
}

impl Summary {
    /// The summary of `batch` as it was saved.
    #[must_use]
    pub(crate) fn of(batch: &BulkBatch, created_contacts: u32) -> Summary {
        let counts: Vec<(SectionKind, usize)> = batch.count_by_kind().into_iter().collect();
        let totals: Vec<(AccountId, SectionKind, Money)> = batch
            .totals()
            .map(|m| m.into_iter().map(|((a, k), m)| (a, k, m)).collect())
            .unwrap_or_default();
        let mut je_rows = 0;
        let mut je_units: i128 = 0;
        for (i, s) in batch.sections.iter().enumerate() {
            if s.kind.is_journal()
                && let Ok(t) = batch.section_totals(i)
            {
                je_rows += t.rows;
                je_units += t.debits.to_ten_thousandths();
            }
        }
        let journal = (je_rows > 0)
            .then(|| Money::from_ten_thousandths(je_units).ok())
            .flatten()
            .map(|m| (je_rows, m));
        let mut registers: Vec<AccountId> = Vec::new();
        for (a, _, _) in &totals {
            if !registers.contains(a) {
                registers.push(*a);
            }
        }
        Summary {
            rows: batch.filled_count(),
            counts,
            totals,
            journal,
            registers,
            created_contacts,
        }
    }

    /// The control-channel state (counts and the owner's totals).
    #[must_use]
    pub(crate) fn state_json(&self) -> Value {
        let fmt = |m: Money| m.format(MoneyStyle::default());
        json!({
            "rows": self.rows,
            "counts": self.counts.iter().map(|(k, n)| json!({"kind": k, "rows": n})).collect::<Vec<_>>(),
            "totals": self.totals.iter().map(|(_, k, m)| json!({"kind": k, "total": fmt(*m)})).collect::<Vec<_>>(),
            "journal": self.journal.map(|(n, m)| json!({"rows": n, "debits": fmt(m)})),
            "registers": self.registers.len(),
            "created_contacts": self.created_contacts,
        })
    }
}

impl BulkEntryScreen {
    /// The summary card: what was saved, with register links and New batch.
    pub(crate) fn summary_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let Some(s) = self.summary.clone() else {
            return;
        };
        let style = self.money_style;
        let mut open = None;
        let mut fresh = false;
        form::card(ui, tokens, |ui| {
            form::heading(
                ui,
                tokens,
                &t_count("batch_entry.summary.title", s.rows, &[]),
            );
            for (k, n) in &s.counts {
                ui.label(t_with(
                    "batch_entry.summary.count",
                    &[("kind", &t(kind_key(*k))), ("n", &n.to_string())],
                ));
            }
            ui.add_space(6.0);
            // The amounts line up on the right in the figure font.
            let mono = egui::TextStyle::Monospace.resolve(ui.style());
            let amounts: Vec<String> = s
                .totals
                .iter()
                .map(|(_, _, m)| m.format(style))
                .chain(s.journal.map(|(_, m)| m.format(style)))
                .collect();
            let width = amounts
                .iter()
                .map(|a| {
                    ui.painter()
                        .layout_no_wrap(a.clone(), mono.clone(), tokens.text_secondary)
                        .size()
                        .x
                })
                .fold(0.0_f32, f32::max);
            let figure = |ui: &mut Ui, text: String| {
                let h = ui.spacing().interact_size.y;
                ui.allocate_ui_with_layout(
                    egui::vec2(width, h),
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| ui.label(RichText::new(text).text_style(egui::TextStyle::Monospace)),
                );
            };
            egui::Grid::new("batch_entry/summary/totals")
                .num_columns(3)
                .spacing([16.0, 4.0])
                .show(ui, |ui| {
                    for (a, k, m) in &s.totals {
                        let name = self
                            .books
                            .lists
                            .account_text(*a)
                            .unwrap_or_default()
                            .to_string();
                        ui.label(RichText::new(t(kind_key(*k))).color(tokens.text_secondary));
                        ui.label(name);
                        figure(ui, m.format(style));
                        ui.end_row();
                    }
                    if let Some((_, m)) = s.journal {
                        ui.label(
                            RichText::new(t(kind_key(SectionKind::JournalEntries)))
                                .color(tokens.text_secondary),
                        );
                        ui.label(t("batch_entry.summary.debits"));
                        figure(ui, m.format(style));
                        ui.end_row();
                    }
                });
            if s.created_contacts > 0 {
                form::hint(
                    ui,
                    tokens,
                    &t_count(
                        "batch_entry.summary.names_added",
                        usize::try_from(s.created_contacts).unwrap_or(usize::MAX),
                        &[],
                    ),
                );
            }
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                for a in &s.registers {
                    let name = self
                        .books
                        .lists
                        .account_text(*a)
                        .unwrap_or_default()
                        .to_string();
                    if ui
                        .link(t_with(
                            "batch_entry.summary.open_register",
                            &[("account", &name)],
                        ))
                        .clicked()
                    {
                        open = Some(*a);
                    }
                }
            });
            ui.add_space(8.0);
            fresh = crate::widgets::action_button_with_chip(
                ui,
                &t("batch_entry.summary.new"),
                &kit::chip("form.save_new"),
                true,
                kit::ACTION_HEIGHT,
            )
            .clicked();
        });
        if let Some(a) = open {
            self.link.push(CompanyEvent::OpenRegister(a));
        }
        if fresh {
            self.summary_open = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cl_model::forms::batch::{RowDraft, RowKeyGen, Section};

    #[test]
    fn totals_are_the_owners_amounts_per_account_and_type() {
        let bank = crate::company::demo::account_id(1010);
        let mut keys = RowKeyGen::new();
        let row = |keys: &mut RowKeyGen, a: &str| {
            let mut r = RowDraft::new(keys.next_key());
            r.amount = Money::parse_wire(a).ok();
            r.memo = "m".into();
            r
        };
        let mut checks = Section::new(SectionKind::Checks, Some(bank));
        checks.rows = vec![
            row(&mut keys, "10.05"),
            row(&mut keys, "0.10"),
            RowDraft::new(keys.next_key()),
        ];
        let mut deposits = Section::new(SectionKind::Deposits, Some(bank));
        deposits.rows = vec![row(&mut keys, "250.00")];
        let mut b = BulkBatch::new(checks);
        b.sections.push(deposits);
        let s = Summary::of(&b, 0);
        assert_eq!(s.rows, 3);
        assert_eq!(s.registers, vec![bank]);
        assert!(s.totals.contains(&(
            bank,
            SectionKind::Checks,
            Money::parse_wire("10.15").expect("m")
        )));
        assert!(s.totals.contains(&(
            bank,
            SectionKind::Deposits,
            Money::parse_wire("250").expect("m")
        )));
        assert_eq!(s.state_json()["totals"].as_array().map(Vec::len), Some(2));
    }
}
