//! "Payments to deposit" (the payments waiting in Payments Clearing,
//! ticked whole) and the side panel: bank-file deposits waiting (a click
//! ticks the engine's suggested grouping; nothing is saved by it, Q-P14e),
//! the engine's Payments Clearing balance and recent deposits.
//!
//! Every figure here is the engine's: a payment's amount as sent (a
//! display-only amount can be seen but not ticked), the Payments Clearing
//! balance as reported (never the sum of the list).

use std::sync::Arc;

use cl_api::{FeedDeposit, PendingPayment};
use cl_core::Money;
use cl_model::forms::deposit::PickedPayment;
use eframe::egui::{self, RichText, Ui};

use super::DepositScreen;
use crate::company::Task;
use crate::i18n::{t, t_with};
use crate::screens::company::form;
use crate::theme::Tokens;

/// The height of one payment row.
const ROW_HEIGHT: f32 = 24.0;
/// The most payment rows shown before the list scrolls.
const VISIBLE_ROWS: usize = 8;
/// Recent deposits in the side panel.
const RECENT: usize = 5;

/// The plain name of a pending item's kind (i18n key).
fn type_key(txn_type: &str) -> &'static str {
    match txn_type {
        "customer_payment" => "deposit.type.payment",
        "cash_sale" => "deposit.type.sales_receipt",
        "daily_sales" => "deposit.type.daily_sales",
        "journal_entry" => "deposit.type.journal_entry",
        _ => "deposit.type.other",
    }
}

/// The plain name of a payment method (i18n key).
fn method_key(method: &str) -> &'static str {
    match method {
        "check" => "deposit.method.check",
        "cash" => "deposit.method.cash",
        "ach" | "bank_transfer" => "deposit.method.bank_transfer",
        "card" | "credit_card" => "deposit.method.card",
        _ => "deposit.method.other",
    }
}

/// A pending payment ticked whole: the engine's exact amount, or `None`
/// for a display-only one.
pub(crate) fn pick_of(p: &PendingPayment) -> Option<PickedPayment> {
    Some(PickedPayment {
        source: p.source,
        amount: p.exact()?,
        contact: p.contact,
    })
}

impl DepositScreen {
    /// Reads the waiting payments again.
    pub(crate) fn refresh_pending(&mut self) {
        let link = Arc::clone(&self.link);
        if let Some(local) = self.local.as_mut() {
            match local.pending(&link) {
                Ok(p) => self.panel.pending_task = Some(Task::ready(p)),
                Err(_) => self.panel.pending_failed = true,
            }
            self.poll_panel();
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        if let Ok(p) = group.read(&session, |s| async move {
            s.payments().pending_deposits().await
        }) {
            self.panel.pending_task = Some(Task::Pending(p));
        }
    }

    /// Reads the account's balance, its recent deposits and the bank-file
    /// deposits waiting.
    pub(crate) fn refresh_facts(&mut self) {
        let Some(account) = self.account() else {
            return;
        };
        self.panel.recent_task = self.list_task(account);
        let link = Arc::clone(&self.link);
        if let Some(local) = self.local.as_mut() {
            let balance = local.balance(&link, account);
            let waiting = local.waiting(&link);
            self.panel.balance_task = Some(Task::ready(balance));
            self.panel.waiting_task = Some(Task::ready(waiting));
            self.poll_panel();
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        if let Ok(p) = group.read(&session, move |s| async move {
            Ok(s.accounts().get(account).await?.record.balance)
        }) {
            self.panel.balance_task = Some(Task::Pending(p));
        }
        if let Ok(p) = group.read(&session, |s| async move {
            // A missing banking permission or feed is no suggestions.
            Ok(s.feed().deposits_waiting().await.unwrap_or_default())
        }) {
            self.panel.waiting_task = Some(Task::Pending(p));
        }
    }

    pub(crate) fn poll_panel(&mut self) {
        if let Some(r) = self.panel.pending_task.as_mut().and_then(Task::try_take) {
            self.panel.pending_task = None;
            match r {
                Ok(p) => {
                    self.panel.pending_failed = false;
                    self.pending_arrived(p);
                }
                Err(_) => self.panel.pending_failed = true,
            }
        }
        if let Some(r) = self.panel.balance_task.as_mut().and_then(Task::try_take) {
            self.panel.balance_task = None;
            self.panel.balance = r.ok().flatten();
        }
        if let Some(r) = self.panel.recent_task.as_mut().and_then(Task::try_take) {
            self.panel.recent_task = None;
            self.panel.recent = r.unwrap_or_default();
        }
        if let Some(r) = self.panel.waiting_task.as_mut().and_then(Task::try_take) {
            self.panel.waiting_task = None;
            self.panel.waiting = r.unwrap_or_default();
        }
        if let Some(r) = self.panel.groupings_task.as_mut().and_then(Task::try_take) {
            self.panel.groupings_task = None;
            match r {
                Ok(g) => self.grouping_arrived(&g),
                // No permission, another company's item, an older engine:
                // quietly no suggestion.
                Err(_) => self.panel.no_grouping = true,
            }
        }
    }

    /// The waiting list arrived: ticked payments no longer waiting
    /// (deposited elsewhere) are dropped from the form.
    fn pending_arrived(&mut self, p: cl_api::PendingDeposits) {
        let waiting: Vec<_> = p.items.iter().map(|i| i.source).collect();
        self.panel.pending = Some(p);
        // Only unsaved picks can go stale; a saved deposit keeps its own.
        if self.editing.is_some() || !self.header.is_dirty() {
            return;
        }
        let stale = self
            .header
            .current()
            .picked
            .iter()
            .any(|x| !waiting.contains(&x.source));
        if stale {
            self.header.edit_step(|h| {
                h.keep_only(&waiting);
            });
            self.save.notice = Some("deposit.already_deposited");
        }
    }

    /// The engine's best grouping for the chosen bank deposit is ticked
    /// (only payments still waiting, at their exact amounts).
    fn grouping_arrived(&mut self, g: &cl_api::FeedGroupings) {
        let Some(best) = g.candidates.first() else {
            self.panel.no_grouping = true;
            return;
        };
        let waiting = self
            .panel
            .pending
            .as_ref()
            .map(|p| p.items.clone())
            .unwrap_or_default();
        let picks: Vec<PickedPayment> = best
            .items
            .iter()
            .filter_map(|i| waiting.iter().find(|w| w.source == i.source))
            .filter_map(pick_of)
            .collect();
        if picks.len() != best.items.len() {
            self.panel.no_grouping = true;
            return;
        }
        self.panel.no_grouping = false;
        self.header.edit_step(|h| {
            for p in picks {
                if !h.picked.iter().any(|x| x.source == p.source) {
                    h.picked.push(p);
                }
            }
        });
    }

    /// The owner clicked a bank-file deposit: its amount is what the
    /// totals compare with, and the engine's grouping is ticked.
    pub(crate) fn choose_feed(&mut self, fd: FeedDeposit) {
        self.panel.no_grouping = false;
        let id = fd.id;
        self.panel.chosen = Some(fd);
        if !self.feed_groupings() || !self.caps().grouping {
            self.panel.no_grouping = true;
            return;
        }
        let link = Arc::clone(&self.link);
        if let Some(local) = self.local.as_mut() {
            match local.groupings(&link, id) {
                Ok(g) => self.panel.groupings_task = Some(Task::ready(g)),
                Err(_) => self.panel.no_grouping = true,
            }
            self.poll_panel();
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        if let Ok(p) = group.read(&session, move |s| async move {
            s.feed().deposit_groupings(id).await
        }) {
            self.panel.groupings_task = Some(Task::Pending(p));
        }
    }

    /// Ticks or unticks one waiting payment (exact amounts only).
    pub(crate) fn toggle_payment(&mut self, p: &PendingPayment) {
        let Some(pick) = pick_of(p) else { return };
        self.header.edit_step(|h| {
            h.toggle(pick);
        });
    }

    /// Select all: every waiting payment ticked (or all unticked when all
    /// already are).
    pub(crate) fn select_all(&mut self) {
        let Some(pending) = self.panel.pending.as_ref() else {
            return;
        };
        let all: Vec<PickedPayment> = pending.items.iter().filter_map(pick_of).collect();
        let every = all.iter().all(|p| {
            self.header
                .current()
                .picked
                .iter()
                .any(|x| x.source == p.source)
        });
        self.header.edit_step(|h| {
            if every {
                h.picked.clear();
            } else {
                for p in all {
                    if !h.picked.iter().any(|x| x.source == p.source) {
                        h.picked.push(p);
                    }
                }
            }
        });
    }

    /// The payments list.
    pub(crate) fn payments_ui(&mut self, ui: &mut Ui, tokens: &Tokens, enabled: bool) {
        let style = self.money_style;
        let picked = &self.header.current().picked;
        let selected = Money::sum(picked.iter().map(|p| p.amount)).unwrap_or(Money::ZERO);
        ui.horizontal(|ui| {
            let r = ui.add(
                egui::Label::new(
                    RichText::new(t("deposit.payments"))
                        .size(14.0)
                        .color(tokens.text_primary),
                )
                .selectable(false),
            );
            r.on_hover_text(t("deposit.payments.tip"));
            ui.label(
                RichText::new(t_with(
                    "deposit.payments.selected",
                    &[
                        ("count", &picked.len().to_string()),
                        ("amount", &selected.format(style)),
                    ],
                ))
                .size(12.0)
                .color(tokens.text_secondary),
            );
        });
        if self.panel.pending_failed {
            ui.horizontal(|ui| {
                form::hint(ui, tokens, &t("deposit.payments.failed"));
                if ui.button(t("deposit.try_again")).clicked() {
                    self.refresh_pending();
                }
            });
            return;
        }
        let Some(pending) = self.panel.pending.clone() else {
            form::progress_line(ui, tokens, &t("deposit.loading"));
            return;
        };
        if pending.items.is_empty() {
            form::hint(ui, tokens, &t("deposit.payments.empty"));
            return;
        }
        let grouping = self.caps().grouping && pending.exact;
        if !grouping {
            form::hint(ui, tokens, &t("deposit.payments.needs_update"));
        }
        let can_tick = enabled && grouping;
        if ui
            .add_enabled(
                can_tick,
                egui::Button::new(t("deposit.payments.select_all")),
            )
            .clicked()
        {
            self.select_all();
        }
        let mut toggled = None;
        let ticked: Vec<_> = self
            .header
            .current()
            .picked
            .iter()
            .map(|p| p.source)
            .collect();
        egui::Grid::new("deposit/payments/head")
            .num_columns(7)
            .min_col_width(60.0)
            .show(ui, |ui| {
                for key in [
                    "deposit.col.pick",
                    "deposit.col.date",
                    "deposit.col.type",
                    "deposit.col.received_from",
                    "deposit.col.ref_no",
                    "deposit.col.method",
                    "deposit.col.amount",
                ] {
                    ui.label(
                        RichText::new(t(key))
                            .size(12.0)
                            .color(tokens.text_secondary),
                    );
                }
                ui.end_row();
            });
        let rows = pending.items.len();
        let height = ROW_HEIGHT * f32::from(u8::try_from(rows.min(VISIBLE_ROWS)).unwrap_or(8));
        egui::ScrollArea::vertical()
            .id_salt("deposit/payments")
            .max_height(height + 4.0)
            .auto_shrink([false, true])
            .show_rows(ui, ROW_HEIGHT, rows, |ui, range| {
                for i in range {
                    let p = &pending.items[i];
                    ui.horizontal(|ui| {
                        let mut on = ticked.contains(&p.source);
                        let label = t_with(
                            "deposit.payments.pick",
                            &[("name", p.received_from.as_deref().unwrap_or(""))],
                        );
                        let r = ui.add_enabled(
                            can_tick && p.exact().is_some(),
                            egui::Checkbox::without_text(&mut on),
                        );
                        let r = r.on_hover_text(&label);
                        r.widget_info(|| {
                            egui::WidgetInfo::selected(
                                egui::Role::CheckBox,
                                true,
                                on,
                                label.clone(),
                            )
                        });
                        if r.clicked() {
                            toggled = Some(i);
                        }
                        ui.add_sized([80.0, ROW_HEIGHT], egui::Label::new(Self::row_date(p.date)));
                        ui.add_sized(
                            [110.0, ROW_HEIGHT],
                            egui::Label::new(t(type_key(&p.txn_type))),
                        );
                        ui.add_sized(
                            [180.0, ROW_HEIGHT],
                            egui::Label::new(p.received_from.clone().unwrap_or_default())
                                .truncate(),
                        );
                        ui.add_sized(
                            [70.0, ROW_HEIGHT],
                            egui::Label::new(p.ref_no.clone().unwrap_or_default()).truncate(),
                        );
                        ui.add_sized(
                            [90.0, ROW_HEIGHT],
                            egui::Label::new(
                                p.method
                                    .as_deref()
                                    .map(method_key)
                                    .map(t)
                                    .unwrap_or_default(),
                            ),
                        );
                        let amount = match p.exact() {
                            Some(m) => m.format(style),
                            None => p.amount.display(false).unwrap_or_default(),
                        };
                        ui.label(RichText::new(amount).text_style(egui::TextStyle::Monospace));
                    });
                }
            });
        if let Some(i) = toggled
            && let Some(p) = pending.items.get(i)
        {
            self.toggle_payment(p);
        }
    }

    /// The side panel.
    pub(crate) fn panel_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let style = self.money_style;
        form::card(ui, tokens, |ui| {
            form::heading(ui, tokens, &t("deposit.panel.waiting"));
            let account_name = self
                .account()
                .and_then(|a| self.accounts.iter().find(|x| x.id == a))
                .map(|a| a.fields.name.clone());
            let shown: Vec<FeedDeposit> = self
                .panel
                .waiting
                .iter()
                .filter(|w| w.account_name.is_none() || w.account_name == account_name)
                .cloned()
                .collect();
            if shown.is_empty() {
                form::hint(ui, tokens, &t("deposit.panel.waiting.none"));
            }
            let mut clicked = None;
            for w in shown {
                let text = t_with(
                    "deposit.panel.waiting.row",
                    &[
                        ("date", &Self::row_date(w.date)),
                        ("amount", &w.amount.format(style)),
                    ],
                );
                let chosen = self.panel.chosen.as_ref().is_some_and(|c| c.id == w.id);
                if ui.selectable_label(chosen, text).clicked() {
                    clicked = Some(w);
                }
            }
            if let Some(w) = clicked {
                self.choose_feed(w);
            }
            if self.panel.no_grouping && self.panel.chosen.is_some() {
                form::hint(ui, tokens, &t("deposit.panel.no_grouping"));
            }
        });
        ui.add_space(8.0);
        form::card(ui, tokens, |ui| {
            form::heading(ui, tokens, &t("deposit.panel.clearing"));
            let text = self
                .panel
                .pending
                .as_ref()
                .and_then(|p| match p.clearing_balance.exact() {
                    Some(m) => Some(m.format(style)),
                    None => p.clearing_balance.display(false),
                })
                .unwrap_or_default();
            ui.label(RichText::new(text).text_style(egui::TextStyle::Monospace))
                .on_hover_text(t("deposit.payments.tip"));
        });
        ui.add_space(8.0);
        form::card(ui, tokens, |ui| {
            form::heading(ui, tokens, &t("deposit.panel.recent"));
            if self.panel.recent.is_empty() {
                form::hint(ui, tokens, &t("deposit.panel.recent.none"));
            }
            let mut open = None;
            for r in self.panel.recent.iter().take(RECENT) {
                let total = r.total.map(|m| m.format(style)).unwrap_or_default();
                let key = if r.void {
                    "deposit.panel.recent.void"
                } else {
                    "deposit.panel.recent.row"
                };
                let text = t_with(
                    key,
                    &[("date", &Self::row_date(r.date)), ("amount", &total)],
                );
                if ui.link(text).clicked() {
                    open = Some(r.id);
                }
            }
            if let Some(id) = open
                && let Some(account) = self.account()
                && !self.is_dirty()
            {
                self.nav.at = None;
                self.load_deposit(id, account);
            }
        });
    }
}
