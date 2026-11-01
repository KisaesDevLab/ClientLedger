//! "Deposit on hand: $500.00 — Apply" on Create Invoice (P21 D7, Q-P21f,
//! Q-P21l): the client's held customer deposits; Apply on a saved invoice
//! asks the engine to apply the deposit (`POST /payments/:id/apply` with no
//! amount: the engine takes what is left of the deposit, up to the
//! invoice's balance). On an unsaved invoice it says to save first.
//!
//! - The apply is dated today; on or before the closing date it needs the
//!   closing-date password, asked through Create Invoice's prompt.
//! - One idempotency key per apply, kept until a definite answer: a retry
//!   after a timeout replays instead of drawing twice.
//! - A failed apply leaves the invoice saved and says why here.

use std::sync::Arc;

use cl_api::{
    ApiError, ApplyInvoice, ApplyRequest, DepositOnHand, IdempotencyKey, PaymentRefusal,
    RequestGroup,
};
use cl_core::ids::{ContactId, TransactionId};
use cl_core::{CalendarDate, MoneyStyle, Secret};
use eframe::egui::{RichText, Ui};

use super::engine::{self, Job};
use crate::company::CompanyLink;
use crate::i18n::{t, t_with};
use crate::screens::card_charge::kit;
use crate::theme::Tokens;

/// What the card asks Create Invoice to do.
#[derive(Debug)]
pub(crate) enum CardEvent {
    /// Apply was clicked on this deposit.
    Apply(TransactionId),
    /// The apply failed (the closing prompt may take it).
    Failed(TransactionId, ApiError),
}

/// The card's state.
#[derive(Debug, Default)]
pub(crate) struct DepositCard {
    client: Option<ContactId>,
    deposits: Vec<DepositOnHand>,
    job: Option<Job<Vec<DepositOnHand>>>,
    pub(crate) apply: Option<(TransactionId, Job<cl_api::ApplyResult>)>,
    /// The deposit and invoice the key was made for, and the key: kept
    /// until the engine answers for sure.
    pub(crate) key: Option<(TransactionId, TransactionId, IdempotencyKey)>,
    note: Option<&'static str>,
    /// Applies done (tests and the scenario).
    pub(crate) applied: u32,
}

/// The plain-language note for a failed apply.
fn failed_note(e: &ApiError) -> &'static str {
    match cl_api::payment_refusal(e) {
        Some(PaymentRefusal::DepositNotAvailable | PaymentRefusal::CreditNotAvailable) => {
            "invoice.deposit.not_available"
        }
        Some(PaymentRefusal::Bounced) => "payment.refusal.bounced",
        Some(PaymentRefusal::InvoiceBalanceChanged | PaymentRefusal::Overapplied) => {
            "invoice.deposit.balance_changed"
        }
        Some(PaymentRefusal::Stale) => "payment.refusal.stale",
        _ => "invoice.deposit.failed",
    }
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

    /// True while an apply is in flight.
    pub(crate) fn busy(&self) -> bool {
        self.apply.is_some()
    }

    /// The note shown (tests and the scenario).
    pub(crate) fn note(&self) -> Option<&'static str> {
        self.note
    }

    /// Sends the apply of deposit `source` to `invoice`, dated `date`, with
    /// the closing-date password if one was given.
    pub(crate) fn send(
        &mut self,
        link: &Arc<CompanyLink>,
        group: Option<&RequestGroup>,
        source: TransactionId,
        invoice: TransactionId,
        date: CalendarDate,
        password: Option<Secret>,
    ) {
        if self.apply.is_some() {
            return;
        }
        let req = ApplyRequest {
            date,
            applications: vec![ApplyInvoice {
                invoice_id: invoice,
                amount: None,
                expected_balance_due: None,
            }],
            expected_updated_at: None,
        };
        // The same apply keeps its key until the engine answers for sure.
        let key = match &self.key {
            Some((s, i, k)) if *s == source && *i == invoice => k.clone(),
            _ => {
                let k = IdempotencyKey::new();
                self.key = Some((source, invoice, k.clone()));
                k
            }
        };
        let demo_req = req.clone();
        let demo_pw = password.clone();
        let opts = crate::screens::receive_payment::engine::options(password);
        self.note = None;
        self.apply = Some((
            source,
            engine::write(
                link,
                group,
                crate::screens::receive_payment::engine::label(&key),
                move |d| {
                    crate::company::demo::receivables::apply(d, source, &demo_req, demo_pw.as_ref())
                },
                move |s| async move { s.customer_payments().apply(source, &req, &opts, key).await },
            ),
        ));
    }

    /// Takes the apply's answer: `Some(Failed)` for Create Invoice's
    /// closing prompt, which may take it; otherwise the note says why.
    pub(crate) fn poll_apply(
        &mut self,
        link: &CompanyLink,
        group: Option<&RequestGroup>,
    ) -> Option<CardEvent> {
        let (source, job) = self.apply.as_mut()?;
        let source = *source;
        let r = job.take()?;
        self.apply = None;
        match r {
            Ok(_) => {
                self.key = None;
                self.applied += 1;
                self.note = Some("invoice.deposit.applied");
                if let Some(c) = self.client {
                    self.fetch(link, group, c);
                }
                None
            }
            Err(e) => {
                if !crate::screens::register::keeps_key(&e) {
                    self.key = None;
                }
                self.note = Some(failed_note(&e));
                Some(CardEvent::Failed(source, e))
            }
        }
    }

    /// Draws the card (nothing when the client has no deposit on hand).
    /// Returns what Create Invoice must do.
    pub(crate) fn ui(
        &mut self,
        ui: &mut Ui,
        tokens: &Tokens,
        link: &Arc<CompanyLink>,
        group: Option<&RequestGroup>,
        client: Option<ContactId>,
        saved: Option<TransactionId>,
    ) -> Option<CardEvent> {
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
        let mut event = self.poll_apply(link, group);
        if self.deposits.is_empty() && self.note.is_none() {
            return event;
        }
        let deposits = self.deposits.clone();
        let busy = self.busy();
        kit::panel_card(ui, tokens, &t("invoice.deposit.title"), |ui| {
            for d in &deposits {
                ui.label(t_with(
                    "invoice.deposit.on_hand",
                    &[("amount", &d.remaining.format(MoneyStyle::default()))],
                ));
                if saved.is_some() {
                    if ui
                        .add_enabled(
                            !busy,
                            eframe::egui::Button::new(t_with(
                                "invoice.deposit.apply",
                                &[("amount", &d.remaining.format(MoneyStyle::default()))],
                            )),
                        )
                        .clicked()
                    {
                        event = Some(CardEvent::Apply(d.payment_id));
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
        event
    }
}
