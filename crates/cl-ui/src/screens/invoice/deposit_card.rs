//! "Deposit on hand: $500.00 — Apply" on Create Invoice (P21 D7, Q-P21f,
//! Q-P21l): the client's held customer deposits; Apply on a saved invoice
//! asks the engine to apply the deposit (`POST /payments/:id/apply` with no
//! amount: the engine takes what is left of the deposit, up to the
//! invoice's balance). On an unsaved invoice it says to save first. A
//! failed apply leaves the invoice saved and shows the error here.

use std::sync::Arc;

use cl_api::{ApplyInvoice, ApplyRequest, DepositOnHand, IdempotencyKey, RequestGroup};
use cl_core::ids::{ContactId, TransactionId};
use cl_core::{CalendarDate, MoneyStyle};
use eframe::egui::{RichText, Ui};

use super::engine::{self, Job};
use crate::company::CompanyLink;
use crate::i18n::{t, t_with};
use crate::screens::card_charge::kit;
use crate::theme::Tokens;

/// The card's state.
#[derive(Debug, Default)]
pub(crate) struct DepositCard {
    client: Option<ContactId>,
    deposits: Vec<DepositOnHand>,
    job: Option<Job<Vec<DepositOnHand>>>,
    apply: Option<Job<cl_api::ApplyResult>>,
    note: Option<&'static str>,
    /// Applies done (tests and the scenario).
    pub(crate) applied: u32,
}

impl DepositCard {
    fn fetch(&mut self, link: &CompanyLink, group: Option<&RequestGroup>, client: ContactId) {
        self.client = Some(client);
        self.deposits.clear();
        let has = link.is_demo()
            || link
                .capabilities()
                .has(cl_api::Capabilities::CUSTOMER_DEPOSITS);
        if !has {
            return;
        }
        self.job = Some(engine::read(
            link,
            group,
            move |d| {
                Ok(crate::company::demo::receivables::deposits_on_hand(
                    d, client,
                ))
            },
            move |s| async move { s.customer_payments().deposits_on_hand(client).await },
        ));
    }

    /// Deposits on hand for the shown client.
    pub(crate) fn count(&self) -> usize {
        self.deposits.len()
    }

    /// Draws the card (nothing when the client has no deposit on hand).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn ui(
        &mut self,
        ui: &mut Ui,
        tokens: &Tokens,
        link: &Arc<CompanyLink>,
        group: Option<&RequestGroup>,
        client: Option<ContactId>,
        saved: Option<TransactionId>,
        today: CalendarDate,
    ) {
        match client {
            Some(c) if self.client != Some(c) => self.fetch(link, group, c),
            None => {
                self.client = None;
                self.deposits.clear();
            }
            _ => {}
        }
        if let Some(Ok(d)) = engine::poll(&mut self.job) {
            self.deposits = d;
        }
        if let Some(r) = engine::poll(&mut self.apply) {
            match r {
                Ok(_) => {
                    self.applied += 1;
                    self.note = Some("invoice.deposit.applied");
                    if let Some(c) = self.client {
                        self.fetch(link, group, c);
                    }
                }
                Err(_) => self.note = Some("invoice.deposit.failed"),
            }
        }
        if self.deposits.is_empty() && self.note.is_none() {
            return;
        }
        let deposits = self.deposits.clone();
        let mut apply: Option<TransactionId> = None;
        kit::panel_card(ui, tokens, &t("invoice.deposit.title"), |ui| {
            for d in &deposits {
                ui.label(t_with(
                    "invoice.deposit.on_hand",
                    &[("amount", &d.remaining.format(MoneyStyle::default()))],
                ));
                if saved.is_some() {
                    if ui
                        .button(t_with(
                            "invoice.deposit.apply",
                            &[("amount", &d.remaining.format(MoneyStyle::default()))],
                        ))
                        .clicked()
                    {
                        apply = Some(d.payment_id);
                    }
                } else {
                    ui.label(
                        RichText::new(t("invoice.deposit.save_first")).color(tokens.text_secondary),
                    );
                }
            }
            if let Some(n) = self.note {
                ui.label(RichText::new(t(n)).color(tokens.text_secondary));
            }
        });
        if let (Some(source), Some(invoice)) = (apply, saved) {
            let req = ApplyRequest {
                date: today,
                applications: vec![ApplyInvoice {
                    invoice_id: invoice,
                    amount: None,
                    expected_balance_due: None,
                }],
                expected_updated_at: None,
            };
            let key = IdempotencyKey::new();
            let demo_req = req.clone();
            self.note = None;
            self.apply = Some(engine::write(
                link,
                group,
                crate::screens::receive_payment::engine::label(&key),
                move |d| crate::company::demo::receivables::apply(d, source, &demo_req, None),
                move |s| async move {
                    s.customer_payments()
                        .apply(source, &req, &cl_api::PostingOptions::default(), key)
                        .await
                },
            ));
        }
    }
}
