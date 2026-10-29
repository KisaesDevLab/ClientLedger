//! Drawing Pay Bills (mockup `PayBills.html`): the title row, the filter
//! row, the bill grid, the totals, the selected-bill and payment panels
//! side by side (one under the other when narrow), Set Discount / Set
//! Credits, and Clear selections · Pay N bills at the bottom.

use cl_core::ids::AccountId;
use cl_grid::{GridHeight, ModelRef, NoLookups};
use cl_model::forms::bill_payment::SortBy;
use cl_model::register::AccountClass;
use eframe::egui::{self, RichText, Ui};

use super::grid::Rows;
use super::{Dialog, Method, PayBillsScreen};
use crate::forms::closing_prompt::ClosingClick;
use crate::i18n::{t, t_with};
use crate::layout::{Body, FixedFrame, FrameOpts};
use crate::screens::card_charge::kit;
use crate::screens::company::form;
use crate::screens::register::account_text;
use crate::theme::Tokens;

/// Side by side from this width.
const SIDE_BY_SIDE: f32 = 760.0;

fn mono(text: String) -> RichText {
    RichText::new(text).text_style(egui::TextStyle::Monospace)
}

/// A "label … figure" row of a panel.
fn fact(ui: &mut Ui, tokens: &Tokens, label: &str, figure: RichText) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).color(tokens.text_secondary));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(figure);
        });
    });
}

impl PayBillsScreen {
    pub(super) fn draw(&mut self, ui: &mut Ui) {
        let tokens = Tokens::of(ui.ctx());
        let mut frame = FixedFrame::begin(ui, "pay_bills", FrameOpts::default());
        frame.top(|ui| self.top_ui(ui, &tokens));
        frame.body(Body::Scroll, |ui| self.body_ui(ui, &tokens));
        frame.bottom(|ui| self.actions_ui(ui, &tokens));
        self.layout = frame.end();
    }

    fn top_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new(t("cmd.vendors.pay_bills"))
                    .size(22.0)
                    .strong()
                    .color(tokens.text_primary),
            );
            ui.label(RichText::new(t("paybills.subtitle")).color(tokens.text_secondary));
        });
        match self.closing_prompt.ui(ui, tokens) {
            ClosingClick::Submit => self.submit_closing(),
            ClosingClick::Cancel => self.closing_prompt.cancel(),
            ClosingClick::None => {}
        }
        if self.unknown {
            let mut again = false;
            form::card(ui, tokens, |ui| {
                ui.label(RichText::new(t("paybills.unknown")).color(tokens.warn_text));
                again = ui.button(t("paybills.unknown.look_again")).clicked();
            });
            if again {
                self.recheck();
            }
        }
        if let Some(k) = self.notice {
            form::hint(ui, tokens, &t(k));
        }
        if let Some((payments, bills)) = self.last_paid
            && self.notice.is_none()
        {
            form::hint(
                ui,
                tokens,
                &t_with(
                    "paybills.paid",
                    &[
                        ("bills", &bills.to_string()),
                        ("payments", &payments.to_string()),
                    ],
                ),
            );
        }
        let keys: Vec<&'static str> = self.problems.iter().map(|p| p.key().0).collect();
        kit::problems(ui, tokens, &keys);
        if let Some(e) = &self.error {
            form::inline_error(ui, tokens, e);
        }
        // Set Discount / Set Credits stay in view above the list.
        if self.dialog.is_some() {
            self.dialog_ui(ui, tokens);
        }
    }

    fn body_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        if !self.loaded {
            form::progress_line(ui, tokens, &t("paybills.loading"));
            return;
        }
        self.filters_ui(ui, tokens);
        ui.add_space(10.0);
        if self.list_failed {
            ui.horizontal(|ui| {
                form::hint(ui, tokens, &t("paybills.list_failed"));
                if ui.button(t("paybills.try_again")).clicked() {
                    self.list_failed = false;
                    self.load_list();
                }
            });
        } else if !self.list_loaded {
            form::progress_line(ui, tokens, &t("paybills.loading"));
        } else if self.form.visible().is_empty() {
            form::hint(ui, tokens, &t("paybills.none"));
        } else {
            self.grid_ui(ui);
            self.totals_ui(ui, tokens);
        }
        // Selected bills a filter hides stay selected and are not paid
        // (review, Low): say so rather than drop them quietly.
        let hidden = self.form.hidden_selected();
        if hidden > 0 && self.list_loaded {
            form::hint(
                ui,
                tokens,
                &if hidden == 1 {
                    t("paybills.hidden_selected.one")
                } else {
                    t_with("paybills.hidden_selected", &[("n", &hidden.to_string())])
                },
            );
        }
        ui.add_space(12.0);
        let wide = ui.available_width() >= SIDE_BY_SIDE;
        if wide {
            ui.horizontal_top(|ui| {
                let w = (ui.available_width() - 14.0) / 2.0;
                ui.vertical(|ui| {
                    ui.set_width(w);
                    self.selected_ui(ui, tokens);
                });
                ui.add_space(14.0);
                ui.vertical(|ui| {
                    ui.set_width(w);
                    self.payment_ui(ui, tokens);
                });
            });
        } else {
            self.selected_ui(ui, tokens);
            ui.add_space(12.0);
            self.payment_ui(ui, tokens);
        }
    }

    fn filters_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        form::card(ui, tokens, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(t("paybills.show")).color(tokens.text_secondary));
                if ui.radio(!self.show_all, t("paybills.show.due")).clicked() {
                    self.show_all = false;
                    if let Some(d) = self.through.value {
                        self.form.show = cl_model::forms::bill_payment::ShowBills::DueOnOrBefore(d);
                        self.revision += 1;
                    }
                }
                let before = self.through.text.clone();
                let r = ui.add(
                    egui::TextEdit::singleline(&mut self.through.text)
                        .desired_width(96.0)
                        .font(egui::TextStyle::Monospace),
                );
                r.widget_info(|| {
                    egui::WidgetInfo::text_edit(
                        true,
                        before.clone(),
                        t("paybills.show.due_date"),
                        "",
                    )
                });
                if r.changed() {
                    self.through.typed(&before, self.today);
                    if let (false, Some(d)) = (self.show_all, self.through.value) {
                        self.form.show = cl_model::forms::bill_payment::ShowBills::DueOnOrBefore(d);
                        self.revision += 1;
                    }
                }
                if r.lost_focus() {
                    self.through.settle();
                }
                if ui.radio(self.show_all, t("paybills.show.all")).clicked() {
                    self.show_all = true;
                    self.form.show = cl_model::forms::bill_payment::ShowBills::All;
                    self.revision += 1;
                }
                ui.add_space(12.0);
                self.vendor_combo(ui, tokens);
                self.sort_combo(ui, tokens);
                let mut take = self.form.take_discounts();
                if ui
                    .checkbox(&mut take, t("paybills.take_discounts"))
                    .on_hover_text(t("paybills.take_discounts.tip"))
                    .changed()
                {
                    self.form.set_take_discounts(take);
                    self.revision += 1;
                }
            });
        });
    }

    fn vendor_combo(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let vendors = self.form.vendors();
        let shown = self
            .form
            .vendor
            .and_then(|v| vendors.iter().find(|(id, _)| *id == v))
            .map_or_else(|| t("paybills.vendor.all"), |(_, n)| n.clone());
        let l = kit::above(ui, tokens, &t("paybills.vendor"));
        let mut pick = None;
        egui::ComboBox::from_id_salt("pay_bills/vendor")
            .width(180.0)
            .selected_text(shown)
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(self.form.vendor.is_none(), t("paybills.vendor.all"))
                    .clicked()
                {
                    pick = Some(None);
                }
                for (id, name) in &vendors {
                    if ui
                        .selectable_label(self.form.vendor == Some(*id), name)
                        .clicked()
                    {
                        pick = Some(Some(*id));
                    }
                }
            })
            .response
            .labelled_by(l.id);
        if let Some(v) = pick {
            self.form.vendor = v;
            self.revision += 1;
        }
    }

    fn sort_combo(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let key = |s: SortBy| match s {
            SortBy::DueDate => "paybills.sort.due",
            SortBy::Vendor => "paybills.sort.vendor",
            SortBy::AmountDue => "paybills.sort.amount",
        };
        let l = kit::above(ui, tokens, &t("paybills.sort"));
        let mut pick = None;
        egui::ComboBox::from_id_salt("pay_bills/sort")
            .width(120.0)
            .selected_text(t(key(self.form.sort)))
            .show_ui(ui, |ui| {
                for s in [SortBy::DueDate, SortBy::Vendor, SortBy::AmountDue] {
                    if ui
                        .selectable_label(self.form.sort == s, t(key(s)))
                        .clicked()
                    {
                        pick = Some(s);
                    }
                }
            })
            .response
            .labelled_by(l.id);
        if let Some(s) = pick {
            self.form.sort = s;
            self.revision += 1;
        }
    }

    fn grid_ui(&mut self, ui: &mut Ui) {
        let label = t("paybills.grid_label");
        let env = self
            .parts
            .env(&NoLookups, &label, GridHeight::Rows { min: 3, max: 10 });
        let mut rows = Rows::new(&mut self.form, &mut self.revision);
        self.grid.show(ui, ModelRef::Edit(&mut rows), &env);
        self.snap = self.grid.snapshot(&ModelRef::Edit(&mut rows));
    }

    /// Totals · n of m selected, then each column's total of the selected
    /// bills (Q-P18b, owner 2026-10-28).
    fn totals_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let Some(totals) = self.form.totals() else {
            return;
        };
        let style = self.money_style;
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new(t_with(
                    "paybills.totals",
                    &[
                        ("selected", &totals.selected.to_string()),
                        ("listed", &totals.listed.to_string()),
                    ],
                ))
                .strong()
                .color(tokens.text_primary),
            );
            for (key, m) in [
                ("paybills.col.amount_due", totals.amount_due),
                ("paybills.col.discount", totals.discount),
                ("paybills.col.credits", totals.credits),
                ("paybills.col.to_pay", totals.cash),
            ] {
                ui.add_space(12.0);
                ui.label(RichText::new(t(key)).color(tokens.text_secondary));
                ui.label(mono(m.format(style)).color(tokens.text_primary));
            }
        });
    }

    fn selected_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let style = self.money_style;
        let bill = self.cursor_bill();
        let row = bill.and_then(|b| self.form.all_rows().iter().find(|r| r.bill == b).cloned());
        let heading = row.as_ref().map_or_else(
            || t("paybills.selected.none_title"),
            |r| {
                t_with(
                    "paybills.selected.title",
                    &[
                        ("vendor", &r.vendor_name),
                        ("ref", r.reference.as_deref().unwrap_or("")),
                    ],
                )
            },
        );
        let mut action = None;
        kit::panel_card(ui, tokens, &heading, |ui| {
            let Some(r) = &row else {
                form::hint(ui, tokens, &t("paybills.selected.none"));
                return;
            };
            fact(
                ui,
                tokens,
                &t("paybills.selected.terms"),
                RichText::new(r.terms.clone().unwrap_or_else(|| "\u{2014}".into())),
            );
            fact(
                ui,
                tokens,
                &t("paybills.selected.suggested"),
                mono(r.discount_available.format(style)),
            );
            let used = self.form.credits_on(r.bill).to_vec();
            let total = self.form.credit_total(r.bill);
            let labels: Vec<String> = used
                .iter()
                .filter_map(|(c, _)| {
                    self.form
                        .open_credits()
                        .iter()
                        .find(|x| x.credit == *c)
                        .map(|x| x.label.clone())
                })
                .collect();
            let text = if labels.is_empty() {
                total.format(style)
            } else {
                t_with(
                    "paybills.selected.credits_from",
                    &[
                        ("amount", &total.format(style)),
                        ("memos", &labels.join(", ")),
                    ],
                )
            };
            fact(ui, tokens, &t("paybills.selected.credits"), mono(text));
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                let has_credits = !self.form.credits_for(r.vendor).is_empty();
                if ui
                    .add_enabled(
                        self.caps().discounts,
                        egui::Button::new(t("paybills.set_discount")),
                    )
                    .on_disabled_hover_text(t("paybills.discount.needs_update"))
                    .clicked()
                {
                    action = Some(0);
                }
                if ui
                    .add_enabled(has_credits, egui::Button::new(t("paybills.set_credits")))
                    .on_disabled_hover_text(t("paybills.credits.none"))
                    .clicked()
                {
                    action = Some(1);
                }
                if ui.button(t("paybills.go_to_bill")).clicked() {
                    action = Some(2);
                }
            });
        });
        if let (Some(a), Some(b)) = (action, bill) {
            match a {
                0 => self.open_discount(b, false),
                1 => self.open_credits(b),
                _ => self.go_to_bill(b),
            }
        }
    }

    fn dialog_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let accounts: Vec<(AccountId, String)> = self
            .discount_accounts()
            .into_iter()
            .map(|a| (a.id, account_text(a)))
            .collect();
        let credit_labels: Vec<(cl_core::ids::TransactionId, String, String)> = self
            .form
            .open_credits()
            .iter()
            .map(|c| (c.credit, c.label.clone(), c.open.format(self.money_style)))
            .collect();
        let mut ok = false;
        let mut cancel = false;
        let mut engine_discount = false;
        form::card(ui, tokens, |ui| match self.dialog.as_mut() {
            Some(Dialog::Discount {
                text,
                account,
                asked,
                ..
            }) => {
                form::heading(ui, tokens, &t("paybills.discount.title"));
                if *asked {
                    form::hint(ui, tokens, &t("paybills.discount.account_needed"));
                }
                let l = kit::above(ui, tokens, &t("paybills.discount.amount"));
                ui.add(
                    egui::TextEdit::singleline(text)
                        .desired_width(120.0)
                        .font(egui::TextStyle::Monospace)
                        .align(egui::Align2::RIGHT_CENTER),
                )
                .labelled_by(l.id);
                let l = kit::above(ui, tokens, &t("paybills.discount.account"));
                let shown = account
                    .and_then(|a| accounts.iter().find(|(id, _)| *id == a))
                    .map_or_else(|| t("paybills.discount.account.choose"), |(_, s)| s.clone());
                egui::ComboBox::from_id_salt("pay_bills/discount_account")
                    .width(260.0)
                    .selected_text(shown)
                    .show_ui(ui, |ui| {
                        for (id, s) in &accounts {
                            if ui.selectable_label(*account == Some(*id), s).clicked() {
                                *account = Some(*id);
                            }
                        }
                    })
                    .response
                    .labelled_by(l.id);
                form::hint(ui, tokens, &t("paybills.discount.account_hint"));
                ui.horizontal(|ui| {
                    ok = crate::widgets::primary_button(ui, &t("paybills.dialog.ok")).clicked();
                    engine_discount = ui.button(t("paybills.discount.use_suggested")).clicked();
                    cancel = ui.button(t("paybills.dialog.cancel")).clicked();
                });
            }
            Some(Dialog::Credits { texts, .. }) => {
                form::heading(ui, tokens, &t("paybills.credits.title"));
                for (credit, text) in texts.iter_mut() {
                    let (label, open) = credit_labels
                        .iter()
                        .find(|(c, _, _)| c == credit)
                        .map(|(_, l, o)| (l.clone(), o.clone()))
                        .unwrap_or_default();
                    let name = t_with("paybills.credits.row", &[("memo", &label), ("open", &open)]);
                    ui.horizontal(|ui| {
                        let l = ui.label(name);
                        ui.add(
                            egui::TextEdit::singleline(text)
                                .desired_width(110.0)
                                .font(egui::TextStyle::Monospace)
                                .align(egui::Align2::RIGHT_CENTER),
                        )
                        .labelled_by(l.id);
                    });
                }
                ui.horizontal(|ui| {
                    ok = crate::widgets::primary_button(ui, &t("paybills.dialog.ok")).clicked();
                    cancel = ui.button(t("paybills.dialog.cancel")).clicked();
                });
            }
            None => {}
        });
        if cancel {
            self.dialog = None;
        } else if engine_discount {
            if let Some(Dialog::Discount { bill, .. }) = self.dialog.take() {
                self.form.set_discount(bill, None);
                self.revision += 1;
            }
        } else if ok {
            match self.dialog {
                Some(Dialog::Discount { .. }) => {
                    self.apply_discount();
                }
                Some(Dialog::Credits { .. }) => {
                    self.apply_credits();
                }
                None => {}
            }
        }
    }

    fn payment_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let caps = self.caps();
        let enabled = self.pay_task.is_none() && !self.closing_prompt.dialog_open();
        kit::panel_card(ui, tokens, &t("paybills.payment"), |ui| {
            ui.add_enabled_ui(enabled, |ui| {
                ui.horizontal_wrapped(|ui| {
                    kit::wrap_item(ui, "pay_bills/date", |ui| {
                        let l = kit::above(ui, tokens, &t("paybills.date"));
                        let before = self.date.text.clone();
                        let r = ui
                            .add(
                                egui::TextEdit::singleline(&mut self.date.text)
                                    .desired_width(110.0)
                                    .font(egui::TextStyle::Monospace),
                            )
                            .labelled_by(l.id);
                        if r.changed() {
                            self.date.typed(&before, self.today);
                        }
                        if r.lost_focus() {
                            self.date.settle();
                            self.date_changed();
                        }
                    });
                    kit::wrap_item(ui, "pay_bills/method", |ui| {
                        let l = kit::above(ui, tokens, &t("paybills.method"));
                        let key = |m: Method| match m {
                            Method::Check => "paybills.method.check",
                            Method::Card => "paybills.method.card",
                        };
                        let mut pick = None;
                        egui::ComboBox::from_id_salt("pay_bills/method")
                            .width(160.0)
                            .selected_text(t(key(self.method)))
                            .show_ui(ui, |ui| {
                                for m in [Method::Check, Method::Card] {
                                    if ui.selectable_label(self.method == m, t(key(m))).clicked() {
                                        pick = Some(m);
                                    }
                                }
                            })
                            .response
                            .labelled_by(l.id);
                        if let Some(m) = pick
                            && m != self.method
                        {
                            self.method = m;
                            self.account_changed();
                        }
                    });
                });
                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    match self.method {
                        Method::Check => {
                            kit::wrap_item(ui, "pay_bills/numbers", |ui| {
                                ui.label(
                                    RichText::new(t("paybills.numbers"))
                                        .size(12.0)
                                        .color(tokens.text_secondary),
                                );
                                if ui
                                    .radio(self.print_later, t("paybills.numbers.print_later"))
                                    .on_hover_text(t("paybills.numbers.print_later.tip"))
                                    .clicked()
                                {
                                    self.print_later = true;
                                }
                                ui.horizontal(|ui| {
                                    if ui
                                        .add_enabled(
                                            caps.numbers,
                                            egui::RadioButton::new(
                                                !self.print_later,
                                                t("paybills.numbers.assign"),
                                            ),
                                        )
                                        .on_disabled_hover_text(t("paybills.numbers.needs_update"))
                                        .clicked()
                                    {
                                        self.print_later = false;
                                    }
                                    if !self.print_later {
                                        let r = ui.add(
                                            egui::TextEdit::singleline(&mut self.first_number)
                                                .desired_width(70.0)
                                                .font(egui::TextStyle::Monospace),
                                        );
                                        let name = t("paybills.numbers.first");
                                        let text = self.first_number.clone();
                                        r.widget_info(|| {
                                            egui::WidgetInfo::text_edit(
                                                true,
                                                text.clone(),
                                                &name,
                                                "",
                                            )
                                        });
                                    }
                                });
                            });
                        }
                        Method::Card => {
                            kit::wrap_item(ui, "pay_bills/reference", |ui| {
                                let l = kit::above(ui, tokens, &t("paybills.reference"));
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.reference)
                                        .desired_width(160.0),
                                )
                                .labelled_by(l.id)
                                .on_hover_text(t("paybills.reference.tip"));
                            });
                        }
                    }
                    self.account_ui(ui, tokens);
                });
            });
            ui.add_space(6.0);
            let label = match self.method {
                Method::Check => t("paybills.ending_balance"),
                Method::Card => t("paybills.amount_owed"),
            };
            let figure = self.preview().map_or_else(
                || "\u{2014}".to_string(),
                |(b, a)| {
                    format!(
                        "{} \u{2192} {}",
                        b.format(self.money_style),
                        a.format(self.money_style)
                    )
                },
            );
            egui::Frame::new()
                .fill(tokens.accent_tint)
                .corner_radius(egui::CornerRadius::same(4))
                .inner_margin(egui::Margin::symmetric(10, 8))
                .show(ui, |ui| {
                    fact(ui, tokens, &label, mono(figure).strong());
                });
        });
    }

    fn account_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let class = match self.method {
            Method::Check => AccountClass::Bank,
            Method::Card => AccountClass::CreditCard,
        };
        let options: Vec<(AccountId, String)> = self
            .accounts_of(class)
            .into_iter()
            .map(|a| (a.id, account_text(a)))
            .collect();
        let current = self.pay_from();
        let shown = options
            .iter()
            .find(|(id, _)| Some(*id) == current)
            .map_or_else(|| t("paybills.pay_from.none"), |(_, s)| s.clone());
        let mut pick = None;
        kit::wrap_item(ui, "pay_bills/account", |ui| {
            let l = kit::above(ui, tokens, &t("paybills.pay_from"));
            egui::ComboBox::from_id_salt("pay_bills/pay_from")
                .width(240.0)
                .selected_text(shown)
                .show_ui(ui, |ui| {
                    for (id, text) in &options {
                        if ui.selectable_label(Some(*id) == current, text).clicked() {
                            pick = Some(*id);
                        }
                    }
                })
                .response
                .labelled_by(l.id);
        });
        if let Some(a) = pick {
            match self.method {
                Method::Check => self.bank = Some(a),
                Method::Card => self.card = Some(a),
            }
            self.account_changed();
        }
    }

    fn actions_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let totals = self.form.totals();
        let n = totals.map_or(0, |t| t.selected);
        let cash = totals.map_or(cl_core::Money::ZERO, |t| t.cash);
        let idle = self.pay_task.is_none()
            && !self.list_task_running()
            && !self.closing_prompt.dialog_open()
            && !self.unknown;
        let mut pay = false;
        let mut clear = false;
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let text = t_with(
                if n == 1 {
                    "paybills.pay.one"
                } else {
                    "paybills.pay"
                },
                &[
                    ("n", &n.to_string()),
                    ("total", &cash.format(self.money_style)),
                ],
            );
            ui.add_enabled_ui(idle && n > 0, |ui| {
                pay = crate::widgets::primary_button(ui, &text)
                    .on_hover_text(kit::chip("form.save_close"))
                    .clicked();
            });
            clear = ui
                .add_enabled(n > 0, egui::Button::new(t("paybills.clear")))
                .clicked();
            if self.pay_task.is_some() {
                form::progress_line(ui, tokens, &t("paybills.paying"));
            }
        });
        if clear {
            self.form.clear();
            self.revision += 1;
        }
        if pay {
            self.start_pay(None);
        }
    }
}
