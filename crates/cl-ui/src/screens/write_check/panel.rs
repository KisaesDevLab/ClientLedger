//! The payee panel beside the form (under it when narrow, README §6):
//! the payee with their open balance and the open-bill note, the recent
//! checks to them, and the check's attachments (mockup WriteCheck.html).
//!
//! Every figure here is the engine's, shown as sent: the open balance is
//! the vendor summary's `totalOwed`, a bill's amount its `balanceDue`, a
//! recent check its total. Nothing is added up (non-negotiable 7).
//!
//! The open-bill note only explains (Q-248): it never stops a save, and it
//! names Pay Bills without a link until that window exists (P18).
//!
//! Each card loads on its own; one that fails says so with Try again.

use cl_api::{OpenBill, TransactionSummary, TxnStatus, TxnType, VendorSummary};
use cl_core::ids::ContactId;
use cl_core::{CalendarDate, Money};
use eframe::egui::{self, RichText, Ui};

use super::WriteCheckScreen;
use crate::company::Task;
use crate::i18n::{t, t_with};
use crate::screens::company::form;
use crate::theme::Tokens;

/// The newest checks shown.
pub(crate) const RECENT_SHOWN: usize = 3;
/// How many of the payee's transactions are read for them.
const RECENT_READ: u16 = 10;

/// One recent check: its number, date and total as the engine sent them.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct RecentCheck {
    pub number: u64,
    pub date: CalendarDate,
    pub amount: Option<Money>,
}

/// Numbers and dates only.
impl std::fmt::Debug for RecentCheck {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RecentCheck")
            .field("number", &self.number)
            .finish_non_exhaustive()
    }
}

/// A card's data: loading, loaded, or failed.
pub(crate) enum Card<T> {
    Loading(Task<T>),
    Ready(T),
    Failed,
}

impl<T> Card<T> {
    fn poll(&mut self) {
        if let Card::Loading(task) = self
            && let Some(r) = task.try_take()
        {
            *self = match r {
                Ok(v) => Card::Ready(v),
                Err(_) => Card::Failed,
            };
        }
    }

    pub(crate) fn ready(&self) -> Option<&T> {
        match self {
            Card::Ready(v) => Some(v),
            _ => None,
        }
    }

    fn loading(&self) -> bool {
        matches!(self, Card::Loading(_))
    }
}

/// The panel's data for one payee.
#[derive(Default)]
pub(crate) struct PanelState {
    /// The payee the cards are for.
    pub payee: Option<ContactId>,
    pub summary: Option<Card<VendorSummary>>,
    pub bills: Option<Card<Vec<OpenBill>>>,
    pub recent: Option<Card<Vec<RecentCheck>>>,
}

impl PanelState {
    pub(crate) fn busy(&self) -> bool {
        self.summary.as_ref().is_some_and(Card::loading)
            || self.bills.as_ref().is_some_and(Card::loading)
            || self.recent.as_ref().is_some_and(Card::loading)
    }

    fn poll(&mut self) {
        if let Some(c) = self.summary.as_mut() {
            c.poll();
        }
        if let Some(c) = self.bills.as_mut() {
            c.poll();
        }
        if let Some(c) = self.recent.as_mut() {
            c.poll();
        }
    }
}

/// The newest numbered, non-void checks among a payee's transactions.
#[must_use]
pub(crate) fn recent_checks(rows: &[TransactionSummary]) -> Vec<RecentCheck> {
    rows.iter()
        .filter(|r| r.txn_type == TxnType::Expense && r.status != TxnStatus::Void)
        .filter_map(|r| {
            Some(RecentCheck {
                number: r.check_number?,
                date: r.txn_date,
                amount: r.display_total.or(r.total),
            })
        })
        .take(RECENT_SHOWN)
        .collect()
}

/// The open-bill note: one bill with its amount and due date, or the count
/// and the engine's total. `None` without unpaid bills.
#[must_use]
pub(crate) fn bill_note(
    summary: Option<&VendorSummary>,
    bills: Option<&[OpenBill]>,
    style: cl_core::MoneyStyle,
) -> Option<String> {
    match (bills, summary) {
        (Some([one]), _) => Some(match one.due {
            Some(d) => t_with(
                "check.panel.one_bill",
                &[
                    ("amount", &one.amount_due.format(style)),
                    ("due", &month_day(d)),
                ],
            ),
            None => t_with(
                "check.panel.one_bill_no_due",
                &[("amount", &one.amount_due.format(style))],
            ),
        }),
        (Some(many), Some(s)) if many.len() > 1 => Some(t_with(
            "check.panel.many_bills",
            &[
                ("count", &many.len().to_string()),
                ("amount", &s.total_owed.format(style)),
            ],
        )),
        (None | Some([]), Some(s)) if s.unpaid_count > 1 => Some(t_with(
            "check.panel.many_bills",
            &[
                ("count", &s.unpaid_count.to_string()),
                ("amount", &s.total_owed.format(style)),
            ],
        )),
        _ => None,
    }
}

/// MM/DD.
fn month_day(d: CalendarDate) -> String {
    format!("{:02}/{:02}", d.month(), d.day())
}

impl WriteCheckScreen {
    /// Loads the cards when the payee changed; polls them.
    pub(crate) fn poll_panel(&mut self) {
        let payee = self.header.current().payee;
        if payee != self.panel.payee {
            self.load_panel(payee);
        }
        self.panel.poll();
    }

    /// Reads the cards for `payee` again.
    pub(crate) fn load_panel(&mut self, payee: Option<ContactId>) {
        self.panel = PanelState {
            payee,
            ..PanelState::default()
        };
        let Some(id) = payee else { return };
        if let Some(mut demo) = self.link.demo_state() {
            use crate::company::demo::checks;
            let summary = checks::vendor_summary(&mut demo, id);
            let bills = checks::payable(&mut demo, id);
            let rows = checks::contact_transactions(&mut demo, id, usize::from(RECENT_READ));
            drop(demo);
            self.panel.summary = Some(Card::Ready(summary));
            self.panel.bills = Some(Card::Ready(bills));
            self.panel.recent = Some(Card::Ready(recent_checks(&rows)));
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        self.panel.summary = group
            .read(&session, move |s| async move {
                s.bills().vendor_summary(id).await
            })
            .ok()
            .map(|p| Card::Loading(Task::Pending(p)));
        self.panel.bills = group
            .read(
                &session,
                move |s| async move { s.bills().payable(id).await },
            )
            .ok()
            .map(|p| Card::Loading(Task::Pending(p)));
        self.panel.recent = group
            .read(&session, move |s| async move {
                let page = cl_api::PageRequest::new(RECENT_READ, 0)?;
                let rows = s.contacts().transactions(id, page).await?;
                Ok(recent_checks(&rows.data))
            })
            .ok()
            .map(|p| Card::Loading(Task::Pending(p)));
    }

    /// Draws the side panel.
    pub(crate) fn panel_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let style = self.money_style;
        let name = self
            .header
            .current()
            .payee
            .and_then(|p| self.lists.name_text(p))
            .map(str::to_string);
        let mut retry = false;
        if let Some(name) = name {
            form::card_tight(ui, tokens, |ui| {
                ui.label(RichText::new(&name).strong().color(tokens.text_primary));
                match self.panel.summary.as_ref() {
                    Some(Card::Ready(s)) => {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(t("check.panel.open_balance"))
                                    .size(12.0)
                                    .color(tokens.text_secondary),
                            );
                            ui.label(
                                RichText::new(s.total_owed.format(style))
                                    .text_style(egui::TextStyle::Monospace)
                                    .color(tokens.text_primary),
                            );
                        });
                    }
                    Some(Card::Failed) => retry |= failed(ui, tokens),
                    Some(Card::Loading(_)) => form::progress_line(ui, tokens, &t("check.loading")),
                    None => {}
                }
                let note = bill_note(
                    self.panel.summary.as_ref().and_then(Card::ready),
                    self.panel
                        .bills
                        .as_ref()
                        .and_then(Card::ready)
                        .map(Vec::as_slice),
                    style,
                );
                if let Some(note) = note {
                    ui.add_space(6.0);
                    egui::Frame::new()
                        .fill(tokens.warn_tint)
                        .corner_radius(egui::CornerRadius::same(6))
                        .inner_margin(egui::Margin::same(8))
                        .show(ui, |ui| {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(note).size(12.0).color(tokens.warn_text),
                                )
                                .wrap(),
                            );
                        });
                } else if matches!(self.panel.bills, Some(Card::Failed)) {
                    retry |= failed(ui, tokens);
                }
            });
            ui.add_space(12.0);
            form::card_tight(ui, tokens, |ui| {
                ui.label(
                    RichText::new(t("check.panel.recent"))
                        .strong()
                        .color(tokens.text_primary),
                );
                match self.panel.recent.as_ref() {
                    Some(Card::Ready(rows)) if rows.is_empty() => {
                        form::hint(ui, tokens, &t("check.panel.recent.none"));
                    }
                    Some(Card::Ready(rows)) => {
                        for r in rows {
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(format!("#{}", r.number))
                                        .text_style(egui::TextStyle::Monospace)
                                        .color(tokens.text_primary),
                                );
                                ui.label(
                                    RichText::new(month_day(r.date))
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
                    Some(Card::Failed) => retry |= failed(ui, tokens),
                    Some(Card::Loading(_)) => form::progress_line(ui, tokens, &t("check.loading")),
                    None => {}
                }
            });
            ui.add_space(12.0);
        }
        if retry {
            let payee = self.panel.payee;
            self.load_panel(payee);
        }
        self.attachments_ui(ui, tokens);
    }

    /// The panel's control-channel state (counts and flags; no amounts or
    /// names).
    pub(crate) fn panel_json(&self) -> serde_json::Value {
        let bills = self.panel.bills.as_ref().and_then(Card::ready);
        let note = bill_note(
            self.panel.summary.as_ref().and_then(Card::ready),
            bills.map(Vec::as_slice),
            self.money_style,
        );
        serde_json::json!({
            "payee": self.panel.payee.is_some(),
            "open_bills": bills.map(Vec::len),
            "bill_warning": note.is_some(),
            "recent_checks": self
                .panel
                .recent
                .as_ref()
                .and_then(Card::ready)
                .map(|r| r.iter().map(|c| c.number).collect::<Vec<_>>()),
            "failed": matches!(self.panel.summary, Some(Card::Failed))
                || matches!(self.panel.bills, Some(Card::Failed))
                || matches!(self.panel.recent, Some(Card::Failed)),
        })
    }
}

/// "Couldn't load" with Try again; true when clicked.
fn failed(ui: &mut Ui, tokens: &Tokens) -> bool {
    let mut again = false;
    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new(t("check.panel.failed"))
                .size(12.0)
                .color(tokens.text_secondary),
        );
        again = ui.button(t("check.panel.try_again")).clicked();
    });
    again
}
