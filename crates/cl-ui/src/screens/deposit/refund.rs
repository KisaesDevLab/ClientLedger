//! Refund from a vendor (Q-87, Q-P14f): a guided choice.
//!
//! 1. "Something I already paid for": an Other deposits line from the
//!    vendor, on the vendor's default expense account when it has one (the
//!    owner can change it). Always available.
//! 2. "A vendor credit or overpayment": a deposit whose source line takes
//!    the refund out of the vendor credit on Accounts Payable
//!    (`vendor-refund-v1`; the choice is off without it). The engine
//!    refuses more than the credit has left (409).
//!
//! The amount is always the owner's typed amount (cents, Q-232).

use std::sync::Arc;

use cl_api::{
    ApiError, ContactType, IdempotencyKey, LocalValidation, OrphanLabel, PageRequest,
    PostingOptions, TransactionFilter, TxnType, VendorRefundDraft, is_vendor_credit_exceeded,
};
use cl_core::ids::{ContactId, TransactionId};
use cl_core::{Money, Secret};
use cl_model::amount_entry::{self, AmountRules};
use cl_model::closing::needs_password;
use cl_model::forms::lines::FormLine;
use eframe::egui::{self, RichText, Ui};
use serde_json::{Value, json};

use super::{DepositScreen, Pending, Saved, VendorCredit};
use crate::company::Task;
use crate::i18n::t;
use crate::screens::company::form;
use crate::theme::Tokens;

/// Which kind of refund.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RefundKind {
    /// Something already paid for: an expense line.
    AlreadyPaid,
    /// Against a vendor credit (`vendor-refund-v1`).
    VendorCredit,
}

/// The refund card.
#[derive(Default)]
pub(crate) struct RefundState {
    pub open: bool,
    pub vendor: Option<ContactId>,
    pub kind: Option<RefundKind>,
    pub amount: String,
    pub amount_problem: Option<&'static str>,
    pub credits: Vec<VendorCredit>,
    pub credits_task: Option<Task<Vec<VendorCredit>>>,
    pub credit: Option<TransactionId>,
    pub task: Option<Task<Saved>>,
    /// Why the refund can't go yet (i18n key).
    pub problem: Option<&'static str>,
    /// The idempotency key of the refund being retried: the draft and
    /// whether a password went with it (never the password itself). A
    /// lost reply resent unchanged reuses it. The pinned engine ignores
    /// the key on `POST /transactions` (U-18 is not built), so the key is
    /// no guard against a second posting: after an unknown outcome the
    /// form blocks sending until the recheck or the owner answers (B2).
    pub key: Option<(VendorRefundDraft, bool, IdempotencyKey)>,
    /// The save whose outcome is unknown is this refund (not the deposit):
    /// when Check again finds it in the books, the refund card is done.
    pub lost: bool,
}

impl RefundState {
    pub(crate) fn busy(&self) -> bool {
        self.credits_task.is_some() || self.task.is_some()
    }

    /// The control-channel state.
    pub(crate) fn state_json(&self) -> Value {
        json!({
            "open": self.open,
            "kind": self.kind.map(|k| match k {
                RefundKind::AlreadyPaid => "already_paid",
                RefundKind::VendorCredit => "vendor_credit",
            }),
            "has_vendor": self.vendor.is_some(),
            "credits": self.credits.len(),
            "problem": self.problem,
            "saving": self.task.is_some(),
        })
    }
}

impl DepositScreen {
    /// The typed refund amount (cents, Q-232); the problem key otherwise.
    fn refund_amount(&mut self) -> Option<Money> {
        let text = self.refund.amount.trim().to_string();
        if text.is_empty() {
            self.refund.amount_problem = Some("deposit.refund.amount_needed");
            return None;
        }
        match amount_entry::parse(&text, AmountRules::MONEY) {
            Ok(p) => match p.value.as_money() {
                Some(m) if m.is_positive() => {
                    self.refund.amount_problem = None;
                    self.refund.amount = m.format(self.money_style);
                    Some(m)
                }
                _ => {
                    self.refund.amount_problem = Some("deposit.refund.amount_positive");
                    None
                }
            },
            Err(e) => {
                self.refund.amount_problem = Some(e.key().0);
                None
            }
        }
    }

    /// Option 1: an Other deposits line from the vendor.
    pub(crate) fn add_refund_line(&mut self) {
        let Some(vendor) = self.refund.vendor else {
            self.refund.problem = Some("deposit.refund.vendor_needed");
            return;
        };
        let Some(amount) = self.refund_amount() else {
            return;
        };
        let account = self
            .contacts
            .iter()
            .find(|c| c.id == vendor)
            .and_then(|c| c.fields.default_expense_account_id)
            .filter(|a| self.accounts.iter().any(|x| x.id == *a && x.is_active));
        let (key, entry) = {
            let keys = &mut self.lines.form_mut().adapter_mut().keys;
            (keys.next_key(), keys.next_key())
        };
        let line = FormLine {
            account,
            amount: Some(amount),
            customer: Some(vendor),
            ..FormLine::blank(key)
        };
        self.lines.form_mut().edit_step(|v| {
            // Before the blank entry line, which stays last.
            let at = match v.lines.last() {
                Some(l) if l.is_blank() => v.lines.len() - 1,
                _ => v.lines.len(),
            };
            v.lines.insert(at, line);
            if v.lines.last().is_none_or(|l| !l.is_blank()) {
                v.lines.push(FormLine::blank(entry));
            }
        });
        self.refund = RefundState::default();
        self.save.last_action = Some("refund_line");
    }

    /// The vendor's credits (option 2).
    fn fetch_credits(&mut self, vendor: ContactId) {
        let link = Arc::clone(&self.link);
        if let Some(local) = self.local.as_mut() {
            self.refund.credits_task = Some(Task::ready(local.vendor_credits(&link, vendor)));
            self.poll_refund();
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        if let Ok(p) = group.read(&session, move |s| async move {
            let filter = TransactionFilter {
                txn_type: Some(TxnType::VendorCredit),
                contact_id: Some(vendor),
                ..TransactionFilter::default()
            };
            let page = s
                .transactions()
                .list(&filter, PageRequest::new(50, 0)?)
                .await?;
            Ok(page
                .data
                .into_iter()
                .filter(|r| r.status == cl_api::TxnStatus::Posted)
                .map(|r| VendorCredit {
                    id: r.id,
                    date: r.txn_date,
                    number: r.txn_number,
                })
                .collect())
        }) {
            self.refund.credits_task = Some(Task::Pending(p));
        }
    }

    pub(crate) fn poll_refund(&mut self) {
        if let Some(r) = self.refund.credits_task.as_mut().and_then(Task::try_take) {
            self.refund.credits_task = None;
            self.refund.credits = r.unwrap_or_default();
            self.refund.credit = self.refund.credits.first().map(|c| c.id);
        }
        if let Some(r) = self.refund.task.as_mut().and_then(Task::try_take) {
            self.refund.task = None;
            match r {
                Ok(s) => self.refund_saved(&s),
                Err(e) => self.refund_failed(e),
            }
        }
    }

    /// Option 2: the refund against a vendor credit, saved on its own.
    pub(crate) fn send_refund(&mut self, password: Option<Secret>) {
        // An unknown save in this window blocks every posting from it (B2).
        if self.header.is_unconfirmed() || self.refund.task.is_some() {
            return;
        }
        self.refund.problem = None;
        let (Some(vendor), Some(credit)) = (self.refund.vendor, self.refund.credit) else {
            self.refund.problem = Some("deposit.refund.credit_needed");
            return;
        };
        let Some(amount) = self.refund_amount() else {
            return;
        };
        let (Some(deposit_to), Some(date), Some(payables)) =
            (self.account(), self.header.current().date, self.payables())
        else {
            self.refund.problem = Some("deposit.refund.account_needed");
            return;
        };
        let draft = VendorRefundDraft {
            vendor,
            deposit_to,
            payables,
            date,
            amount,
            applies_to: credit,
            memo: None,
        };
        let posting = match draft.posting(self.caps()) {
            Ok(p) => p,
            Err(LocalValidation::VendorRefundUnavailable) => {
                self.refund.problem = Some("deposit.refund.needs_update");
                return;
            }
            Err(v) => {
                self.save.error = Some(ApiError::Local(v));
                return;
            }
        };
        if password.is_none() && needs_password(None, date, self.closing) {
            let supported = self.closing_supported();
            if supported {
                self.closing_prompt
                    .ask(Pending::Refund, supported, self.closing);
            } else {
                self.closing_prompt.unavailable(supported, self.closing);
            }
            return;
        }
        self.save.published = vec![deposit_to, payables];
        let link = Arc::clone(&self.link);
        let deposit = draft.to_deposit();
        if let Some(local) = self.local.as_mut() {
            match local.save(&link, &deposit, None, password.as_ref()) {
                Ok(s) => self.refund.task = Some(Task::ready(s)),
                Err(e) => {
                    self.refund_failed(e);
                    return;
                }
            }
            self.poll_refund();
            return;
        }
        let key = self.refund_key(&draft, password.is_some());
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        let label = OrphanLabel {
            what: "orphan.deposit_save",
            key: Some(key.clone()),
        };
        if let Ok(p) = group.write(&session, label, move |s| async move {
            let opts = PostingOptions {
                closed_period: password.map(cl_api::ClosedPeriodAuthorization::with_password),
            };
            let d = s.payments().create_deposit(&posting, &opts, key).await?;
            Ok(Saved {
                id: d.saved.record.id,
                version: d.saved.version,
                date,
                clearing_negative: d.clearing_would_go_negative,
            })
        }) {
            self.refund.task = Some(Task::Pending(p));
        }
    }

    /// The key for this refund: the stored one when the same refund (same
    /// draft, same password-or-not) is being sent again, a fresh one
    /// otherwise.
    pub(crate) fn refund_key(
        &mut self,
        draft: &VendorRefundDraft,
        password: bool,
    ) -> IdempotencyKey {
        match &self.refund.key {
            Some((d, p, k)) if d == draft && *p == password => k.clone(),
            _ => {
                let k = IdempotencyKey::new();
                self.refund.key = Some((draft.clone(), password, k.clone()));
                k
            }
        }
    }

    fn refund_saved(&mut self, _s: &Saved) {
        self.save.lost.saved();
        self.refund_done();
    }

    /// Check again found the lost refund in the books (review fix-PR-1
    /// should-fix 3): it is done, exactly as if its reply had come; the
    /// card closes so the same refund is not sent a second time.
    pub(crate) fn refund_found(&mut self) {
        self.refund_done();
    }

    fn refund_done(&mut self) {
        crate::forms::publish_saved(&self.link, &self.bus, &self.save.published);
        self.closing_prompt.saved();
        self.refund = RefundState::default();
        self.save.count += 1;
        self.save.last_action = Some("refund");
        self.save.notice = Some("deposit.refund.saved");
        self.refresh_facts();
    }

    pub(crate) fn refund_failed(&mut self, e: ApiError) {
        if !crate::screens::register::keeps_key(&e) {
            self.refund.key = None;
        }
        if self.save.lost.lost(&e) {
            self.header.mark_unconfirmed();
            self.refund.lost = true;
            return;
        }
        if self.closing_failed(&e, Pending::Refund) {
            return;
        }
        if is_vendor_credit_exceeded(&e) {
            self.refund.problem = Some("deposit.refund.too_much");
            return;
        }
        self.save.error = Some(e);
    }

    /// The refund card.
    pub(crate) fn refund_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let vendor_refund = self.caps().vendor_refund;
        let mut close = false;
        let mut picked_vendor = None;
        form::card(ui, tokens, |ui| {
            form::heading(ui, tokens, &t("deposit.refund.title"));
            let vendors: Vec<(ContactId, String)> = self
                .contacts
                .iter()
                .filter(|c| {
                    c.is_active
                        && matches!(
                            c.fields.contact_type,
                            ContactType::Vendor | ContactType::Both
                        )
                })
                .map(|c| (c.id, c.fields.display_name.clone()))
                .collect();
            let shown = vendors
                .iter()
                .find(|(id, _)| Some(*id) == self.refund.vendor)
                .map(|(_, n)| n.clone())
                .unwrap_or_default();
            ui.horizontal(|ui| {
                let l = ui.label(t("deposit.refund.vendor"));
                egui::ComboBox::from_id_salt("deposit/refund/vendor")
                    .width(240.0)
                    .selected_text(shown)
                    .show_ui(ui, |ui| {
                        for (id, name) in &vendors {
                            if ui
                                .selectable_label(Some(*id) == self.refund.vendor, name)
                                .clicked()
                            {
                                picked_vendor = Some(*id);
                            }
                        }
                    })
                    .response
                    .labelled_by(l.id);
            });
            if form::choice(
                ui,
                tokens,
                self.refund.kind == Some(RefundKind::AlreadyPaid),
                &t("deposit.refund.already_paid"),
                &t("deposit.refund.already_paid.tip"),
            )
            .clicked()
            {
                self.refund.kind = Some(RefundKind::AlreadyPaid);
            }
            let r = ui.add_enabled_ui(vendor_refund, |ui| {
                form::choice(
                    ui,
                    tokens,
                    self.refund.kind == Some(RefundKind::VendorCredit),
                    &t("deposit.refund.vendor_credit"),
                    &t("deposit.refund.vendor_credit.tip"),
                )
            });
            let r = r
                .inner
                .on_disabled_hover_text(t("deposit.refund.needs_update"));
            if r.clicked() {
                self.refund.kind = Some(RefundKind::VendorCredit);
                if let Some(v) = self.refund.vendor {
                    self.fetch_credits(v);
                }
            }
            if self.refund.kind == Some(RefundKind::VendorCredit) {
                if self.refund.credits.is_empty() {
                    form::hint(ui, tokens, &t("deposit.refund.no_credits"));
                }
                let mut credit = self.refund.credit;
                for c in &self.refund.credits {
                    let text = format!(
                        "{} {}",
                        Self::row_date(c.date),
                        c.number.clone().unwrap_or_default()
                    );
                    ui.radio_value(&mut credit, Some(c.id), text);
                }
                self.refund.credit = credit;
            }
            ui.horizontal(|ui| {
                let l = ui.label(t("deposit.refund.amount"));
                ui.add_sized(
                    [140.0, 24.0],
                    egui::TextEdit::singleline(&mut self.refund.amount)
                        .font(egui::TextStyle::Monospace)
                        .align(egui::Align2::RIGHT_CENTER),
                )
                .labelled_by(l.id);
            });
            if let Some(k) = self.refund.amount_problem.or(self.refund.problem) {
                ui.label(RichText::new(t(k)).color(tokens.warn_text));
            }
            ui.horizontal(|ui| {
                let ready = self.refund.kind.is_some() && self.refund.task.is_none();
                if ui
                    .add_enabled(ready, egui::Button::new(t("deposit.refund.add")))
                    .clicked()
                {
                    match self.refund.kind {
                        Some(RefundKind::AlreadyPaid) => self.add_refund_line(),
                        Some(RefundKind::VendorCredit) => self.send_refund(None),
                        None => {}
                    }
                }
                close = ui.button(t("deposit.refund.cancel")).clicked();
            });
        });
        if let Some(v) = picked_vendor {
            self.refund.vendor = Some(v);
            self.refund.problem = None;
            if self.refund.kind == Some(RefundKind::VendorCredit) {
                self.fetch_credits(v);
            }
        }
        if close {
            self.refund = RefundState::default();
        }
    }
}
