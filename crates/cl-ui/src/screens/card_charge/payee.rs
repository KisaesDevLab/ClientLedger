//! Record Card Charge's payee: prefill from the payee's last transaction
//! (Q-191), the vendor's default account and tag when there is none
//! (Q-192, Q-178), and the side panel's recent charges to the payee.
//!
//! Prefill copies accounts, line memos, customers, billable marks and tags,
//! and the memo; **never an amount** (non-negotiable 4: the copy goes
//! through `CardForm::prefill_from_last`, which clears line amounts, and the
//! card amount is never touched). It runs only on a new, untouched form.
//! The recent charges are the engine's figures as sent (nothing is added
//! up here).

use cl_api::{TransactionSummary, TxnStatus, TxnType};
use cl_core::ids::{AccountId, ContactId, TagId};
use cl_core::{CalendarDate, Money};
use cl_model::forms::PayeePrefill;
use cl_model::forms::card::CardForm;
use cl_model::forms::lines::{FormLine, FormLines};
use eframe::egui::{self, RichText, Ui};

use super::{CardChargeScreen, field};
use crate::company::Task;
use crate::i18n::t;
use crate::screens::company::form;
use crate::screens::write_check::prefill::{LOOK_BACK, LastTxn, PrefillData, copyable, last_of};
use crate::theme::Tokens;

/// The newest charges shown.
pub(crate) const RECENT_SHOWN: usize = 3;

/// One recent charge to the payee: its date and total as the engine sent
/// them.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct RecentCharge {
    pub date: CalendarDate,
    pub amount: Option<Money>,
}

/// Dates only.
impl std::fmt::Debug for RecentCharge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RecentCharge")
            .field("date", &self.date)
            .finish_non_exhaustive()
    }
}

/// Prefill and the panel for the picked payee.
#[derive(Default)]
pub(crate) struct PayeeState {
    pub prefill: Option<(ContactId, Task<PrefillData>)>,
    /// Prefill filled the form.
    pub applied: bool,
    /// `last` or `default`.
    pub source: Option<&'static str>,
    /// Some inactive details of the last transaction were left out.
    pub dropped: bool,
    /// The payee the recent charges are for.
    pub recent_for: Option<ContactId>,
    pub recent_task: Option<Task<Vec<RecentCharge>>>,
    pub recent: Option<Vec<RecentCharge>>,
    pub recent_failed: bool,
}

impl PayeeState {
    pub(crate) fn busy(&self) -> bool {
        self.prefill.is_some() || self.recent_task.is_some()
    }
}

/// The newest non-void charges (expenses without a check number) among a
/// payee's transactions.
#[must_use]
pub(crate) fn recent_charges(rows: &[TransactionSummary]) -> Vec<RecentCharge> {
    rows.iter()
        .filter(|r| {
            r.txn_type == TxnType::Expense
                && r.status != TxnStatus::Void
                && r.check_number.is_none()
        })
        .map(|r| RecentCharge {
            date: r.txn_date,
            amount: r.display_total.or(r.total),
        })
        .take(RECENT_SHOWN)
        .collect()
}

impl CardChargeScreen {
    /// A payee was picked: prefill when the form is new and untouched.
    pub(crate) fn payee_picked(&mut self, id: ContactId) {
        if self.flow.editing.is_some() || !self.compose().can_prefill() {
            return;
        }
        self.start_prefill(id);
    }

    fn start_prefill(&mut self, id: ContactId) {
        self.payee.prefill = None;
        let link = std::sync::Arc::clone(&self.link);
        if let Some(mut demo) = link.demo_state() {
            use crate::company::demo::checks;
            let last = checks::contact_transactions(&mut demo, id, usize::from(LOOK_BACK))
                .into_iter()
                .find(|t| copyable(t.txn_type) && t.status != TxnStatus::Void)
                .and_then(|t| checks::transaction(&mut demo, t.id))
                .map(|t| last_of(&t));
            let suggest = if last.is_none() {
                checks::suggest_account(&mut demo, id)
            } else {
                None
            };
            drop(demo);
            self.payee.prefill = Some((id, Task::ready(PrefillData { last, suggest })));
            self.poll_payee();
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        if let Ok(p) = group.read(&session, move |s| async move {
            let page = cl_api::PageRequest::new(LOOK_BACK, 0)?;
            let rows = s.contacts().transactions(id, page).await?;
            let pick = rows
                .data
                .into_iter()
                .find(|t| copyable(t.txn_type) && t.status != TxnStatus::Void);
            let last = match pick {
                Some(t) => Some(last_of(&s.transactions().get(t.id).await?.record)),
                None => None,
            };
            let suggest = if last.is_none() {
                s.contacts().suggest_account(id).await.ok().flatten()
            } else {
                None
            };
            Ok(PrefillData { last, suggest })
        }) {
            self.payee.prefill = Some((id, Task::Pending(p)));
        }
    }

    /// Polls prefill and the recent charges (read again when the payee
    /// changed).
    pub(crate) fn poll_payee(&mut self) {
        let payee = self.header.current().payee;
        if payee != self.payee.recent_for {
            self.load_recent(payee);
        }
        if let Some(r) = self.payee.recent_task.as_mut().and_then(Task::try_take) {
            self.payee.recent_task = None;
            match r {
                Ok(rows) => {
                    self.payee.recent = Some(rows);
                    self.payee.recent_failed = false;
                }
                Err(_) => self.payee.recent_failed = true,
            }
        }
        let Some((id, task)) = self.payee.prefill.as_mut() else {
            return;
        };
        let id = *id;
        let Some(r) = task.try_take() else { return };
        self.payee.prefill = None;
        let Ok(data) = r else { return };
        if self.flow.editing.is_some()
            || self.header.current().payee != Some(id)
            || !self.compose().can_prefill()
        {
            return;
        }
        if let Some(last) = data.last.filter(|l| !l.lines.is_empty()) {
            self.apply_last(&last);
        } else {
            self.apply_default(id, data.suggest);
        }
    }

    /// Reads the recent charges to `payee`.
    pub(crate) fn load_recent(&mut self, payee: Option<ContactId>) {
        self.payee.recent_for = payee;
        self.payee.recent = None;
        self.payee.recent_task = None;
        self.payee.recent_failed = false;
        let Some(id) = payee else { return };
        let link = std::sync::Arc::clone(&self.link);
        if let Some(mut demo) = link.demo_state() {
            let rows = crate::company::demo::checks::contact_transactions(
                &mut demo,
                id,
                usize::from(LOOK_BACK),
            );
            drop(demo);
            self.payee.recent = Some(recent_charges(&rows));
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        if let Ok(p) = group.read(&session, move |s| async move {
            let page = cl_api::PageRequest::new(LOOK_BACK, 0)?;
            let rows = s.contacts().transactions(id, page).await?;
            Ok(recent_charges(&rows.data))
        }) {
            self.payee.recent_task = Some(Task::Pending(p));
        }
    }

    fn account_active(&self, a: AccountId) -> bool {
        self.books.accounts.iter().any(|x| x.id == a && x.is_active)
    }

    fn tag_active(&self, t: TagId) -> bool {
        self.books.tags.iter().any(|x| x.id == t && x.is_active)
    }

    fn contact_active(&self, c: ContactId) -> bool {
        self.books.contacts.iter().any(|x| x.id == c && x.is_active)
    }

    /// Copies the last transaction (Q-191); never an amount. Inactive
    /// details are left out.
    pub(crate) fn apply_last(&mut self, last: &LastTxn) {
        let mut dropped = false;
        let customers = self.line_billable();
        let mut seeds = Vec::new();
        for l in &last.lines {
            if !self.account_active(l.account) {
                dropped = true;
                continue;
            }
            let mut tag = l.tag;
            if tag.is_some_and(|t| !self.tag_active(t)) {
                dropped = true;
                tag = None;
            }
            let mut customer = l.customer.filter(|_| customers);
            if customer.is_some_and(|c| !self.contact_active(c)) {
                dropped = true;
                customer = None;
            }
            let key = self.lines.form_mut().adapter_mut().keys.next_key();
            seeds.push(FormLine {
                account: Some(l.account),
                memo: l.memo.clone(),
                customer,
                billable: l.billable && customer.is_some(),
                tag,
                ..FormLine::blank(key)
            });
        }
        let mut f = self.compose();
        let mut from = CardForm::new(f.card, f.date);
        from.memo.clone_from(&last.memo);
        from.header_tags.clone_from(&f.header_tags);
        from.lines = FormLines { lines: seeds };
        f.prefill_from_last(&from);
        let (memo, mut lines) = (f.memo, f.lines);
        lines.ensure_entry(&mut self.lines.form_mut().adapter_mut().keys);
        self.header.edit(field::PAYEE, |h| h.memo = memo);
        self.lines.form_mut().edit_step(|v| *v = lines);
        self.payee.applied = true;
        self.payee.source = Some("last");
        self.payee.dropped = dropped;
        self.inputs.rev = u64::MAX;
    }

    /// No earlier transaction: the vendor's default account and tag fill an
    /// untouched first line (Q-192, Q-178).
    pub(crate) fn apply_default(
        &mut self,
        id: ContactId,
        suggest: Option<cl_api::SuggestedAccount>,
    ) {
        let contact = self.books.contacts.iter().find(|c| c.id == id);
        let account = suggest
            .map(|s| s.account)
            .or_else(|| contact.and_then(|c| c.fields.default_expense_account_id))
            .filter(|a| self.account_active(*a));
        let tag = contact
            .and_then(|c| c.fields.default_tag_id)
            .filter(|t| self.tag_active(*t));
        let Some(account) = account else { return };
        let mut f = self.compose();
        let keys = &mut self.lines.form_mut().adapter_mut().keys;
        if f.apply_vendor_default(account, tag, keys) {
            let lines = f.lines;
            self.lines.form_mut().edit_step(|v| *v = lines);
            self.payee.applied = true;
            self.payee.source = Some("default");
        }
    }

    /// The payee card of the side panel: the recent charges.
    pub(crate) fn payee_panel_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let Some(name) = self
            .header
            .current()
            .payee
            .and_then(|p| self.books.lists.name_text(p))
            .map(str::to_string)
        else {
            return;
        };
        let style = self.money_style;
        let mut retry = false;
        ui.add_space(12.0);
        form::card_tight(ui, tokens, |ui| {
            ui.label(RichText::new(&name).strong().color(tokens.text_primary));
            ui.label(
                RichText::new(t("card.panel.recent"))
                    .size(12.0)
                    .color(tokens.text_secondary),
            );
            match &self.payee.recent {
                Some(rows) if rows.is_empty() => {
                    form::hint(ui, tokens, &t("card.panel.recent.none"));
                }
                Some(rows) => {
                    for r in rows {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(format!("{:02}/{:02}", r.date.month(), r.date.day()))
                                    .size(12.0)
                                    .color(tokens.text_secondary),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    let amount =
                                        r.amount.map(|m| m.format(style)).unwrap_or_default();
                                    ui.label(
                                        RichText::new(amount)
                                            .text_style(egui::TextStyle::Monospace)
                                            .color(tokens.text_primary),
                                    );
                                },
                            );
                        });
                    }
                }
                None if self.payee.recent_failed => {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            RichText::new(t("check.panel.failed"))
                                .size(12.0)
                                .color(tokens.text_secondary),
                        );
                        retry = ui.button(t("check.panel.try_again")).clicked();
                    });
                }
                None => form::progress_line(ui, tokens, &t("check.loading")),
            }
            if self.payee.dropped {
                form::hint(ui, tokens, &t("check.prefill.dropped"));
            }
        });
        if retry {
            let payee = self.payee.recent_for;
            self.load_recent(payee);
        }
    }
}
