//! Add Billable Costs (P20 WP-B step 3, Ctrl+Shift+B; Q-P20b/c/p/w/y/z).
//!
//! The client's unbilled costs come from the engine (`GET /billable-costs`,
//! U-128). Nothing is added by itself (Q-P20c): the side panel says "N
//! unbilled costs" with an Add button; the dialog lists each cost with its
//! source, a check box and one markup percent defaulted from the client
//! (blank: none). A cost from an expense-only line takes an income account
//! the owner picks (remembered per expense account while the window is
//! open, Q-P20p). A cost is billed whole (Q-P20y); a negative cost gets no
//! markup (Q-P20z). The amounts — markup included — are the engine's
//! (Q-P20w): the line shows them from the preview.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use cl_api::{AccountType, ApiError, BillableCost, Page, PageRequest, RequestGroup, SignedPercent};
use cl_core::ids::{AccountId, ContactId, JournalLineId};
use cl_core::{MoneyStyle, Quantity};
use cl_model::forms::invoice::{BillableSeed, InvoiceForm};
use eframe::egui::{self, RichText};
use serde_json::{Value, json};

use super::InvoiceScreen;
use super::engine::{self, Job};
use crate::company::CompanyLink;
use crate::i18n::{t, t_count, t_with};
use crate::theme::Tokens;

/// The unbilled costs and the dialog.
#[derive(Default)]
pub(crate) struct Billable {
    client: Option<ContactId>,
    job: Option<Job<Page<BillableCost>>>,
    /// The client's unbilled costs (the engine's).
    pub costs: Vec<BillableCost>,
    open: bool,
    picks: HashSet<JournalLineId>,
    /// Markup percent as typed.
    pub markup: String,
    /// The income account chosen for a cost without an item.
    accounts: HashMap<JournalLineId, AccountId>,
    /// The last income account chosen per expense account.
    remembered: HashMap<AccountId, AccountId>,
    problem: Option<&'static str>,
    /// Costs added by the dialog so far (control channel).
    added: usize,
}

impl Billable {
    /// Reads the client's unbilled costs.
    pub(crate) fn fetch(
        &mut self,
        link: &Arc<CompanyLink>,
        group: Option<&RequestGroup>,
        c: ContactId,
    ) {
        if self.client != Some(c) {
            self.costs.clear();
            self.picks.clear();
        }
        self.client = Some(c);
        self.job = Some(engine::read(
            link,
            group,
            |d| {
                Ok(crate::company::demo::billable::costs(
                    d,
                    c,
                    PageRequest::first_max(),
                ))
            },
            move |s| async move {
                s.invoices()
                    .billable_costs(c, PageRequest::first_max())
                    .await
            },
        ));
        self.poll();
    }

    pub(crate) fn poll(&mut self) {
        if let Some(r) = engine::poll(&mut self.job) {
            match r {
                Ok(p) => self.costs = p.data,
                Err(ApiError::Local(_)) => self.costs.clear(),
                Err(_) => self.problem = Some("invoice.billable.load_failed"),
            }
        }
    }

    pub(crate) fn busy(&self) -> bool {
        self.job.is_some()
    }

    /// The income account for a cost without an item (and remembered for
    /// its expense account).
    pub(crate) fn choose_income(&mut self, line: JournalLineId, income: AccountId) {
        self.accounts.insert(line, income);
        if let Some(exp) = self
            .costs
            .iter()
            .find(|c| c.line_id == line)
            .and_then(|c| c.account_id)
        {
            self.remembered.insert(exp, income);
        }
    }

    pub(crate) fn forget(&mut self) {
        self.client = None;
        self.costs.clear();
        self.picks.clear();
        self.open = false;
    }

    pub(crate) fn is_open(&self) -> bool {
        self.open
    }

    pub(crate) fn close(&mut self) {
        self.open = false;
        self.problem = None;
    }

    /// The costs not on the form yet.
    pub(crate) fn unbilled<'a>(&'a self, form: &InvoiceForm) -> Vec<&'a BillableCost> {
        let on = form.billable_sources();
        self.costs
            .iter()
            .filter(|c| !on.contains(&c.line_id))
            .collect()
    }

    pub(crate) fn unbilled_json(&self, form: &InvoiceForm) -> Value {
        json!(self.unbilled(form).len())
    }

    pub(crate) fn dialog_json(&self) -> Value {
        json!({
            "open": self.open,
            "costs": self.costs.len(),
            "picked": self.picks.len(),
            "markup": self.markup,
            "added": self.added,
            "problem": self.problem,
        })
    }

    /// The chosen costs as lines (nothing computed: the engine prices them,
    /// markup included). `None` when the markup is not a percent.
    fn seeds(&self, form: &InvoiceForm) -> Option<Vec<BillableSeed>> {
        let markup = self.markup.trim();
        let markup = if markup.is_empty() {
            None
        } else {
            let p = SignedPercent::parse(markup).ok()?;
            if p.is_negative() {
                return None;
            }
            Some(Quantity::parse_wire(&p.to_wire()).ok()?)
        };
        let one = Quantity::parse_wire("1").ok()?;
        Some(
            self.unbilled(form)
                .into_iter()
                .filter(|c| self.picks.contains(&c.line_id))
                .map(|c| BillableSeed {
                    source_line: c.line_id,
                    item: c.item_id,
                    account: if c.item_id.is_some() {
                        None
                    } else {
                        self.accounts.get(&c.line_id).copied()
                    },
                    description: c.description.clone().unwrap_or_default(),
                    quantity: c.quantity.unwrap_or(one),
                    unit_cost: if c.quantity.is_some() {
                        c.unit_price.unwrap_or(c.amount)
                    } else {
                        c.amount
                    },
                    // A negative cost passes through at cost (Q-P20z).
                    markup_percent: if c.amount.is_negative() { None } else { markup },
                })
                .collect(),
        )
    }
}

impl InvoiceScreen {
    /// Add Billable Costs (Ctrl+Shift+B).
    pub(crate) fn open_billable(&mut self) {
        if self.read_only().is_some() {
            return;
        }
        if !self.caps().billable {
            self.notice = Some("invoice.billable.unavailable");
            return;
        }
        let Some(c) = self.header.current().client else {
            self.notice = Some("invoice.billable.client_first");
            return;
        };
        let link = Arc::clone(&self.link);
        self.billable.fetch(&link, self.group.as_ref(), c);
        let form = self.compose();
        let markup = self
            .sales
            .as_ref()
            .filter(|(id, _)| *id == c)
            .and_then(|(_, s)| s.default_markup_percent)
            .map(SignedPercent::to_wire)
            .unwrap_or_default();
        let b = &mut self.billable;
        b.open = true;
        b.problem = None;
        b.markup = markup;
        let ids: Vec<JournalLineId> = b.unbilled(&form).iter().map(|c| c.line_id).collect();
        b.picks = ids.into_iter().collect();
        for c in &b.costs {
            if c.item_id.is_none()
                && let Some(exp) = c.account_id
                && let Some(inc) = b.remembered.get(&exp)
            {
                b.accounts.entry(c.line_id).or_insert(*inc);
            }
        }
    }

    /// Adds the chosen costs as lines before the entry line (one undo step).
    pub(crate) fn add_billable(&mut self) {
        let form = self.compose();
        let Some(seeds) = self.billable.seeds(&form) else {
            self.billable.problem = Some("invoice.billable.markup_invalid");
            return;
        };
        if seeds.is_empty() {
            self.billable.problem = Some("invoice.billable.none_picked");
            return;
        }
        let mut added = 0;
        self.grid
            .edit(|v, keys| added = v.add_billable(&seeds, keys));
        self.billable.added += added;
        self.billable.close();
        self.last_action = Some("billable_added");
    }

    /// The dialog over the form.
    pub(crate) fn billable_window(&mut self, ctx: &egui::Context, tokens: &Tokens) {
        if !self.billable.open {
            return;
        }
        let style = self.money_style;
        let form = self.compose();
        let incomes: Vec<(AccountId, String)> = self
            .books
            .accounts
            .iter()
            .filter(|a| {
                matches!(
                    a.fields.account_type,
                    AccountType::Revenue | AccountType::OtherRevenue
                ) && a.is_active
            })
            .map(|a| (a.id, a.fields.name.clone()))
            .collect();
        let mut add = false;
        let mut cancel = false;
        let width = (ctx.content_rect().width() - 48.0).clamp(300.0, 640.0);
        egui::Window::new(t("invoice.billable.title"))
            .id(egui::Id::unique("invoice/billable"))
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.set_width(width);
                let b = &mut self.billable;
                let rows: Vec<BillableCost> = b.unbilled(&form).into_iter().cloned().collect();
                if rows.is_empty() {
                    ui.label(t("invoice.billable.none"));
                }
                for c in &rows {
                    ui.horizontal_wrapped(|ui| {
                        let mut on = b.picks.contains(&c.line_id);
                        let source = t_with(
                            "invoice.billable.source",
                            &[
                                ("vendor", c.vendor_name.as_deref().unwrap_or_default()),
                                ("number", c.txn_number.as_deref().unwrap_or_default()),
                                ("date", &crate::shell::us_date(c.txn_date)),
                            ],
                        );
                        let label = format!(
                            "{} \u{b7} {} \u{b7} {}",
                            c.description.as_deref().unwrap_or_default(),
                            source,
                            c.amount.format(style)
                        );
                        if ui.checkbox(&mut on, label).changed() {
                            if on {
                                b.picks.insert(c.line_id);
                            } else {
                                b.picks.remove(&c.line_id);
                            }
                        }
                        if c.item_id.is_none() {
                            let chosen = b.accounts.get(&c.line_id).copied();
                            let text = chosen
                                .and_then(|a| incomes.iter().find(|(id, _)| *id == a))
                                .map_or_else(
                                    || t("invoice.billable.pick_income"),
                                    |(_, n)| n.clone(),
                                );
                            let l = ui.label(t("invoice.billable.income_account"));
                            egui::ComboBox::from_id_salt(("invoice/billable/income", c.line_id))
                                .selected_text(text)
                                .show_ui(ui, |ui| {
                                    for (id, name) in &incomes {
                                        if ui.selectable_label(chosen == Some(*id), name).clicked()
                                        {
                                            b.choose_income(c.line_id, *id);
                                        }
                                    }
                                })
                                .response
                                .labelled_by(l.id);
                        }
                    });
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let l = ui.label(t("invoice.billable.markup"));
                    ui.add(egui::TextEdit::singleline(&mut b.markup).desired_width(60.0))
                        .labelled_by(l.id)
                        .on_hover_text(t("invoice.billable.markup.tip"));
                    ui.label("%");
                });
                if let Some(p) = b.problem {
                    ui.label(RichText::new(t(p)).color(tokens.warn_text));
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    add = ui.button(t("invoice.billable.add")).clicked();
                    cancel = ui.button(t("invoice.billable.cancel")).clicked();
                });
            });
        if cancel {
            self.billable.close();
        } else if add {
            self.add_billable();
        }
    }

    /// The side panel's note: "N unbilled costs, $X" and Add (Q-P20c);
    /// hidden when there are none.
    pub(crate) fn billable_note(&mut self, ui: &mut egui::Ui, tokens: &Tokens) {
        let form = self.compose();
        let rows = self.billable.unbilled(&form);
        if rows.is_empty() {
            return;
        }
        let n = rows.len();
        let style = MoneyStyle::default();
        // One cost shows its own amount; more show the count (the UI adds
        // nothing up, Q-P20n).
        let text = if n == 1 {
            t_with(
                "invoice.billable.note.one",
                &[("amount", &rows[0].amount.format(style))],
            )
        } else {
            t_count("invoice.billable.note", n, &[])
        };
        let mut open = false;
        crate::screens::card_charge::kit::panel_card(
            ui,
            tokens,
            &t("invoice.billable.heading"),
            |ui| {
                ui.label(RichText::new(text).color(tokens.text_secondary));
                open = ui
                    .button(t("invoice.billable.open"))
                    .on_hover_text(crate::screens::card_charge::kit::chip(
                        "invoice.add_billable",
                    ))
                    .clicked();
            },
        );
        if open {
            self.after_frame = Some(cl_model::CommandId::new("invoice.add_billable"));
        }
    }
}
