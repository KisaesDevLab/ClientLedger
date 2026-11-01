//! Drawing Receive Payment (mockup `ReceivePayment.html`): the toolbar and
//! banners, the header card, the open-invoice grid with its totals, the
//! callouts, the amounts summary, the client and next-step panels, and
//! Clear · Save & New · Save & Close. Every figure is the engine's preview
//! (`FiguresView`), "Updating…" while a newer one is pending.

use cl_core::ids::{AccountId, TransactionId};
use cl_model::forms::payment::{FiguresView, NothingApplied, Over, RowChoice, Under};
use eframe::egui::{self, RichText, Ui};

use super::{After, Dialog, METHODS, ReceivePaymentScreen, shown};
use crate::forms::closing_prompt::ClosingClick;
use crate::i18n::{t, t_with};
use crate::layout::{Body, FixedFrame, FrameOpts};
use crate::screens::card_charge::kit::{self, NameOutcome};
use crate::screens::company::form;
use crate::theme::Tokens;

/// The side panels' width.
const SIDE: f32 = 250.0;
/// One grid row's height.
const ROW_H: f32 = 26.0;

fn mono(text: String) -> RichText {
    RichText::new(text).text_style(egui::TextStyle::Monospace)
}

fn fact(ui: &mut Ui, tokens: &Tokens, label: &str, figure: RichText) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).color(tokens.text_secondary));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(figure);
        });
    });
}

fn mmdd(d: cl_core::CalendarDate) -> String {
    crate::shell::us_date(d)
}

fn account_combo(
    ui: &mut Ui,
    label: &str,
    salt: &str,
    value: &mut Option<AccountId>,
    list: &[(AccountId, String)],
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        let l = ui.add(egui::Label::new(label).selectable(false));
        let current = value
            .and_then(|id| list.iter().find(|(a, _)| *a == id))
            .map_or_else(|| t("payment.account.choose"), |(_, n)| n.clone());
        egui::ComboBox::from_id_salt(salt)
            .selected_text(current)
            .width(260.0)
            .show_ui(ui, |ui| {
                for (id, name) in list {
                    changed |= ui.selectable_value(value, Some(*id), name).changed();
                }
            })
            .response
            .labelled_by(l.id);
    });
    changed
}

impl ReceivePaymentScreen {
    pub(super) fn draw(&mut self, ui: &mut Ui) {
        let tokens = Tokens::of(ui.ctx());
        let mut frame = FixedFrame::begin(ui, "receive_payment", FrameOpts::default());
        frame.top(|ui| self.top_ui(ui, &tokens));
        frame.body(Body::Scroll, |ui| self.body_ui(ui, &tokens));
        frame.bottom(|ui| self.actions_ui(ui));
        self.layout = frame.end();
    }

    fn top_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        ui.horizontal_wrapped(|ui| {
            if ui
                .button(t("payment.prev"))
                .on_hover_text(t("payment.prev.tip"))
                .clicked()
            {
                self.nav(-1);
            }
            if ui
                .button(t("payment.next"))
                .on_hover_text(t("payment.next.tip"))
                .clicked()
            {
                self.nav(1);
            }
            ui.separator();
            if ui
                .button(t("payment.discounts_credits"))
                .on_hover_text(kit::chip("payment.discounts_credits"))
                .clicked()
            {
                self.dialog = Some(Dialog::Credits);
            }
            let can_bounce = self
                .editing
                .as_ref()
                .is_some_and(|e| !e.is_bounced() && !e.is_void() && e.deposit_to.is_some());
            if ui
                .add_enabled(can_bounce, egui::Button::new(t("payment.bounced_check")))
                .on_hover_text(t("payment.bounced_check.tip"))
                .clicked()
            {
                self.open_bounce();
            }
            if self
                .editing
                .as_ref()
                .is_some_and(|e| !e.is_void() && !e.is_bounced())
                && ui.button(t("payment.void")).clicked()
            {
                self.dialog = Some(Dialog::Void);
            }
        });
        ui.label(
            RichText::new(crate::i18n::t(&format!("cmd.{}", super::KIND)))
                .size(22.0)
                .strong()
                .color(tokens.text_primary),
        );
        match self.closing_prompt.ui(ui, tokens) {
            ClosingClick::Submit => self.submit_closing(),
            ClosingClick::Cancel => self.closing_prompt.cancel(),
            ClosingClick::None => {}
        }
        if let Some(e) = &self.editing {
            if let Some(b) = e.bounce {
                form::inline_warning(
                    ui,
                    tokens,
                    &t_with("payment.banner.bounced", &[("date", &mmdd(b.date))]),
                );
            } else if e.is_void() {
                form::inline_warning(ui, tokens, &t("payment.banner.void"));
            } else if e.deposited_in.is_some() {
                form::hint(ui, tokens, &t("payment.banner.deposited"));
            }
        }
        if let Some(k) = self.notice {
            form::inline_warning(ui, tokens, &t(k));
        }
        if let Some(e) = &self.error {
            form::inline_error(ui, tokens, e);
        }
        self.banner_ui(ui, tokens);
    }

    /// The bank-file deposit banner (U-136).
    fn banner_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let Some(m) = self.feed_match().cloned() else {
            return;
        };
        let linked = self.form.feed_item() == Some(m.id.as_str());
        let mut link = false;
        let mut dismiss = false;
        form::card(ui, tokens, |ui| {
            ui.label(t_with(
                "payment.banner.feed",
                &[
                    ("amount", &shown(m.amount)),
                    ("date", &mmdd(m.date)),
                    ("bank", &m.bank_name),
                ],
            ));
            if linked {
                ui.label(
                    RichText::new(t("payment.banner.feed.linked")).color(tokens.text_secondary),
                );
            } else if Some(self.form.deposit_to()).flatten() == self.clearing() {
                ui.label(
                    RichText::new(t("payment.banner.feed.make_deposit"))
                        .color(tokens.text_secondary),
                );
            }
            ui.horizontal(|ui| {
                if !linked && m.bank.is_some() {
                    link = ui
                        .button(t_with(
                            "payment.banner.feed.link",
                            &[("bank", &m.bank_name)],
                        ))
                        .clicked();
                }
                dismiss = ui.button(t("payment.banner.feed.dismiss")).clicked();
            });
        });
        if link {
            self.form.link_feed(Some(m.id.clone()), m.bank);
            self.dirty = true;
        }
        if dismiss {
            if linked {
                self.form.link_feed(None, None);
            }
            self.feed_dismissed.push(m.id);
        }
    }

    fn body_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        self.dialog_ui(ui, tokens);
        let wide = ui.available_width() > 760.0;
        if wide {
            ui.horizontal_top(|ui| {
                let w = ui.available_width() - SIDE - 16.0;
                ui.vertical(|ui| {
                    ui.set_width(w);
                    self.main_ui(ui, tokens);
                });
                ui.vertical(|ui| {
                    ui.set_width(SIDE);
                    self.side_ui(ui, tokens);
                });
            });
        } else {
            self.main_ui(ui, tokens);
            self.side_ui(ui, tokens);
        }
    }

    fn main_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let read_only = self.is_read_only();
        ui.add_enabled_ui(!read_only, |ui| {
            form::card(ui, tokens, |ui| self.header_ui(ui, tokens));
            ui.add_space(8.0);
            self.grid_ui(ui, tokens);
            if !read_only {
                self.callouts_ui(ui, tokens);
            }
        });
        self.summary_ui(ui, tokens);
    }

    fn header_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let lists = std::sync::Arc::clone(&self.books.lists);
        ui.horizontal_wrapped(|ui| {
            let (out, _) = kit::name_field(
                ui,
                tokens,
                &t("payment.received_from"),
                &mut self.client_input,
                &lists,
                self.form.client,
                260.0,
            );
            match out {
                NameOutcome::Picked(c) => self.set_client(Some(c)),
                NameOutcome::Cleared => self.set_client(None),
                NameOutcome::None => {}
            }
            let r = form::date_field(ui, &t("payment.date"), &mut self.date, self.today);
            if r.changed() || r.lost_focus() {
                self.form.set_date(self.date.value);
                self.dirty = true;
            }
            let r = ui
                .horizontal(|ui| {
                    let l = ui.add(egui::Label::new(t("payment.amount")).selectable(false));
                    ui.add_sized(
                        [140.0, 24.0],
                        egui::TextEdit::singleline(&mut self.amount_text)
                            .font(egui::TextStyle::Monospace)
                            .align(egui::Align2::RIGHT_CENTER),
                    )
                    .labelled_by(l.id)
                })
                .inner;
            if r.lost_focus() || (r.changed() && self.amount_text.trim().is_empty()) {
                self.amount_typed();
                if let Some(s) = form::settled_money_text(&self.amount_text, self.money_style) {
                    self.amount_text = s;
                }
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.horizontal(|ui| {
                let l = ui.add(egui::Label::new(t("payment.method")).selectable(false));
                egui::ComboBox::from_id_salt("payment-method")
                    .selected_text(t(&format!("payment.method.{}", self.form.method)))
                    .show_ui(ui, |ui| {
                        for m in METHODS {
                            ui.selectable_value(
                                &mut self.form.method,
                                m.to_string(),
                                t(&format!("payment.method.{m}")),
                            );
                        }
                    })
                    .response
                    .labelled_by(l.id);
            });
            let ref_label = if self.form.method == "check" {
                t("payment.check_no")
            } else {
                t("payment.ref_no")
            };
            if form::field(ui, &ref_label, &mut self.form.ref_no).changed() {
                self.dirty = true;
            }
        });
        let deposit: Vec<(AccountId, String)> = self
            .deposit_accounts()
            .iter()
            .map(|a| {
                let name = if a.system_tag.as_deref() == Some("payments_clearing") {
                    t("payment.deposit_to.clearing")
                } else {
                    crate::screens::register::account_text(a)
                };
                (a.id, name)
            })
            .collect();
        let mut to = self.form.deposit_to();
        if account_combo(
            ui,
            &t("payment.deposit_to"),
            "payment-deposit-to",
            &mut to,
            &deposit,
        ) {
            self.form.set_deposit_to(to);
            self.dirty = true;
        }
        if form::field(ui, &t("payment.memo"), &mut self.form.memo).changed() {
            self.dirty = true;
        }
    }

    fn grid_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        ui.label(RichText::new(t("payment.grid")).strong());
        let rows = match self.form.figures() {
            FiguresView::Nothing => {
                form::hint(ui, tokens, &t("payment.grid.choose"));
                return;
            }
            FiguresView::Updating => {
                form::hint(ui, tokens, &t("payment.updating"));
                return;
            }
            FiguresView::CantCalculate => {
                form::inline_warning(ui, tokens, &t("payment.cant_calculate"));
                return;
            }
            FiguresView::Ready(f) => f.clone(),
        };
        if rows.rows.is_empty() {
            let name = self.client_input.text.clone();
            form::hint(
                ui,
                tokens,
                &t_with("payment.grid.none", &[("client", &name)]),
            );
            return;
        }
        let widths = [44.0, 84.0, 70.0, 84.0, 96.0, 96.0, 80.0, 110.0];
        let heads = [
            "payment.col.apply",
            "payment.col.date",
            "payment.col.number",
            "payment.col.due",
            "payment.col.original",
            "payment.col.amount_due",
            "payment.col.discount",
            "payment.col.payment",
        ];
        ui.horizontal(|ui| {
            for (w, h) in widths.iter().zip(heads) {
                ui.add_sized(
                    [*w, 20.0],
                    egui::Label::new(RichText::new(t(h)).strong().size(12.0)),
                );
            }
        });
        let mut toggles: Vec<TransactionId> = Vec::new();
        let mut typed: Vec<TransactionId> = Vec::new();
        let mut discounts: Vec<TransactionId> = Vec::new();
        egui::ScrollArea::vertical()
            .id_salt("payment-rows")
            .max_height(ROW_H * 12.0)
            .auto_shrink([false, true])
            .show_rows(ui, ROW_H, rows.rows.len(), |ui, range| {
                for r in &rows.rows[range] {
                    ui.horizontal(|ui| {
                        let mut on = r.payment.is_positive()
                            || self.form.row_choice(r.invoice) != RowChoice::Off;
                        let label = t_with("payment.apply_row", &[("number", &r.number)]);
                        let c =
                            ui.add_sized([widths[0], 22.0], egui::Checkbox::without_text(&mut on));
                        c.widget_info(|| {
                            egui::WidgetInfo::selected(egui::Role::CheckBox, true, on, &label)
                        });
                        if c.clicked() {
                            toggles.push(r.invoice);
                        }
                        ui.add_sized([widths[1], 22.0], egui::Label::new(mono(mmdd(r.date))));
                        ui.add_sized([widths[2], 22.0], egui::Label::new(mono(r.number.clone())));
                        ui.add_sized(
                            [widths[3], 22.0],
                            egui::Label::new(mono(r.due.map(mmdd).unwrap_or_default())),
                        );
                        ui.add_sized([widths[4], 22.0], egui::Label::new(mono(shown(r.original))));
                        ui.add_sized([widths[5], 22.0], egui::Label::new(mono(shown(r.balance))));
                        ui.add_sized([widths[6], 22.0], egui::Label::new(mono(shown(r.discount))));
                        let text = self
                            .cells
                            .entry(r.invoice)
                            .or_insert_with(|| shown(r.payment));
                        let cell_label = t_with("payment.payment_row", &[("number", &r.number)]);
                        let e = ui.add_sized(
                            [widths[7], 22.0],
                            egui::TextEdit::singleline(text)
                                .font(egui::TextStyle::Monospace)
                                .align(egui::Align2::RIGHT_CENTER),
                        );
                        e.widget_info(|| {
                            egui::WidgetInfo::labeled(egui::Role::TextInput, true, &cell_label)
                        });
                        if e.lost_focus() && *text != shown(r.payment) {
                            typed.push(r.invoice);
                        }
                    });
                    if r.suggest_discount {
                        ui.horizontal(|ui| {
                            ui.add_space(widths[0]);
                            let chip = t_with(
                                "payment.discount.chip",
                                &[
                                    ("date", &r.discount_date.map(mmdd).unwrap_or_default()),
                                    ("percent", r.discount_percent.as_deref().unwrap_or("")),
                                    ("amount", &shown(r.discount_available)),
                                ],
                            );
                            ui.label(RichText::new(chip).color(tokens.accent));
                            if ui
                                .button(t_with("payment.discount.apply", &[("number", &r.number)]))
                                .clicked()
                            {
                                discounts.push(r.invoice);
                            }
                        });
                    }
                }
            });
        ui.horizontal(|ui| {
            ui.add_sized(
                [widths[0] + widths[1] + widths[2] + widths[3], 22.0],
                egui::Label::new(RichText::new(t("payment.totals")).strong()),
            );
            let total = |m| mono(shown(m)).strong();
            ui.add_sized([widths[4], 22.0], egui::Label::new(total(rows.amount_due)));
            ui.add_sized([widths[5], 22.0], egui::Label::new(total(rows.amount_due)));
            ui.add_sized(
                [widths[6], 22.0],
                egui::Label::new(total(rows.discounts_and_credits)),
            );
            ui.add_sized([widths[7], 22.0], egui::Label::new(total(rows.applied)));
        });
        for i in toggles {
            self.toggle(i);
        }
        for i in typed {
            self.cell_typed(i);
        }
        for i in discounts {
            if self.form.apply_discount(i) {
                self.cells.clear();
                self.dirty = true;
            }
        }
    }

    fn callouts_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let c = self.form.callouts();
        let names: Vec<(TransactionId, String)> = match self.form.figures() {
            FiguresView::Ready(f) => f
                .rows
                .iter()
                .map(|r| (r.invoice, r.number.clone()))
                .collect(),
            _ => Vec::new(),
        };
        let number = |i: TransactionId| {
            names
                .iter()
                .find(|(x, _)| *x == i)
                .map(|(_, n)| n.clone())
                .unwrap_or_default()
        };
        if !c.underpaid.is_empty() || c.writing_off {
            let mut choose: Option<bool> = None;
            egui::Frame::new()
                .fill(tokens.warn_surface)
                .stroke(egui::Stroke::new(1.0, tokens.warn))
                .corner_radius(6.0)
                .inner_margin(12.0)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    for s in &c.underpaid {
                        ui.label(
                            RichText::new(t_with(
                                "payment.under.title",
                                &[("amount", &shown(s.amount)), ("number", &number(s.invoice))],
                            ))
                            .strong(),
                        );
                    }
                    if c.writing_off && c.underpaid.is_empty() {
                        ui.label(RichText::new(t("payment.under.writing_off")).strong());
                    }
                    let writing = matches!(self.form.under(), Under::WriteOff { .. });
                    if ui
                        .radio(!writing, t("payment.under.leave"))
                        .on_hover_text(t("payment.under.leave.tip"))
                        .clicked()
                    {
                        choose = Some(false);
                    }
                    if let Some(s) = c.underpaid.first() {
                        ui.label(
                            RichText::new(t_with(
                                "payment.under.leave.note",
                                &[("number", &number(s.invoice)), ("amount", &shown(s.amount))],
                            ))
                            .color(tokens.text_secondary),
                        );
                    }
                    let suggested = c.underpaid.iter().any(|s| s.within_limit);
                    let mut wo = RichText::new(t("payment.under.write_off"));
                    if suggested {
                        wo = wo.strong().color(tokens.accent);
                    }
                    if ui.radio(writing, wo).clicked() {
                        choose = Some(true);
                    }
                    ui.label(
                        RichText::new(t("payment.under.write_off.note"))
                            .color(tokens.text_secondary),
                    );
                    if suggested && let Some(limit) = self.defaults.write_off_limit {
                        ui.label(
                            RichText::new(t_with(
                                "payment.under.hint",
                                &[("limit", &shown(limit))],
                            ))
                            .color(tokens.accent),
                        );
                    }
                });
            match choose {
                Some(true) => {
                    if self.form.write_off(self.write_off_pick) {
                        self.cells.clear();
                        self.dirty = true;
                    }
                }
                Some(false) => {
                    self.form.leave_open();
                    self.cells.clear();
                }
                None => {}
            }
        }
        if let Some(over) = c.overpayment
            && !c.ask_deposit
        {
            let client = self.client_input.text.clone();
            form::card(ui, tokens, |ui| {
                ui.label(
                    RichText::new(t_with("payment.over.title", &[("amount", &shown(over))]))
                        .strong(),
                );
                let keep = self.form.over() == Over::KeepCredit;
                if ui
                    .radio(
                        keep,
                        t_with(
                            "payment.over.keep",
                            &[("amount", &shown(over)), ("client", &client)],
                        ),
                    )
                    .clicked()
                {
                    self.form.set_over(Over::KeepCredit);
                }
                if ui.radio(!keep, t("payment.over.refund")).clicked() {
                    self.form.set_over(Over::Refund);
                }
            });
        }
        if c.ask_deposit {
            form::card(ui, tokens, |ui| {
                ui.label(RichText::new(t("payment.deposit.ask")).strong());
                ui.label(RichText::new(t("payment.deposit.ask.note")).color(tokens.text_secondary));
                let a = self.form.nothing_applied();
                if ui
                    .radio(a == Some(NothingApplied::Hold), t("payment.deposit.hold"))
                    .clicked()
                {
                    self.form.answer_deposit(NothingApplied::Hold);
                    self.dirty = true;
                }
                if ui
                    .radio(
                        a == Some(NothingApplied::KeepCredit),
                        t("payment.deposit.credit"),
                    )
                    .clicked()
                {
                    self.form.answer_deposit(NothingApplied::KeepCredit);
                    self.dirty = true;
                }
            });
        }
    }

    fn summary_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let figure = |m: Option<cl_core::Money>| match m {
            Some(m) => mono(shown(m)),
            None => RichText::new(t("payment.updating")).color(tokens.text_secondary),
        };
        let f = match self.form.figures() {
            FiguresView::Ready(f) => Some(f.clone()),
            _ => None,
        };
        form::card(ui, tokens, |ui| {
            ui.set_max_width(340.0);
            fact(
                ui,
                tokens,
                &t("payment.sum.amount_due"),
                figure(f.as_ref().map(|f| f.amount_due)),
            );
            fact(
                ui,
                tokens,
                &t("payment.sum.applied"),
                figure(f.as_ref().map(|f| f.applied)),
            );
            fact(
                ui,
                tokens,
                &t("payment.sum.discounts_credits"),
                figure(f.as_ref().map(|f| f.discounts_and_credits)),
            );
            fact(
                ui,
                tokens,
                &t("payment.sum.unapplied"),
                figure(f.as_ref().map(|f| f.unapplied)),
            );
        });
        for p in &self.problems {
            ui.label(RichText::new(t(super::problem_key(p))).color(tokens.warn_text));
        }
    }

    fn side_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let f = match self.form.figures() {
            FiguresView::Ready(f) => Some(f.clone()),
            _ => None,
        };
        if self.form.client.is_some() {
            form::card(ui, tokens, |ui| {
                ui.label(RichText::new(self.client_input.text.clone()).strong());
                let figure = |m: Option<cl_core::Money>| match m {
                    Some(m) => mono(shown(m)),
                    None => RichText::new(t("payment.updating")).color(tokens.text_secondary),
                };
                fact(
                    ui,
                    tokens,
                    &t("payment.side.before"),
                    figure(f.as_ref().map(|f| f.balance_before)),
                );
                fact(
                    ui,
                    tokens,
                    &t("payment.side.after"),
                    figure(f.as_ref().map(|f| f.balance_after)),
                );
                if let Some(d) = f.as_ref().and_then(|f| f.average_days) {
                    ui.label(
                        RichText::new(t_with("payment.side.usually", &[("days", &d.to_string())]))
                            .color(tokens.text_secondary),
                    );
                }
            });
        }
        let pending = self
            .saved_next
            .or_else(|| f.as_ref().and_then(|f| f.pending));
        form::card(ui, tokens, |ui| {
            ui.label(RichText::new(t("payment.side.next")).strong());
            if let Some((n, total)) = pending {
                ui.label(t_with(
                    if self.saved_next.is_some() {
                        "payment.side.waiting"
                    } else {
                        "payment.side.will_wait"
                    },
                    &[("n", &n.to_string()), ("total", &shown(total))],
                ));
            }
            if ui
                .button(t("payment.side.make_deposit"))
                .on_hover_text(kit::chip("banking.deposit"))
                .clicked()
            {
                self.link.push(crate::company::CompanyEvent::OpenWindow(
                    cl_model::CommandId::new("banking.deposit"),
                ));
            }
            ui.label(
                RichText::new(t("payment.side.feed_note"))
                    .color(tokens.text_secondary)
                    .size(12.0),
            );
        });
    }

    fn actions_ui(&mut self, ui: &mut Ui) {
        let busy = self.save.is_some() || self.form.in_flight();
        let ready = matches!(self.form.figures(), FiguresView::Ready(_)) && !self.is_read_only();
        ui.horizontal(|ui| {
            if ui.button(t("payment.clear")).clicked() {
                self.clear();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let primary = ui
                    .add_enabled_ui(ready && !busy, |ui| {
                        crate::widgets::primary_button(ui, &t("payment.save_close"))
                    })
                    .inner
                    .on_hover_text(kit::chip("form.save_close"));
                if primary.clicked() {
                    self.start_save(After::Close, None);
                }
                if ui
                    .add_enabled(ready && !busy, egui::Button::new(t("payment.save_new")))
                    .on_hover_text(kit::chip("form.save_new"))
                    .clicked()
                {
                    self.start_save(After::New, None);
                }
            });
        });
    }

    #[allow(clippy::too_many_lines)]
    fn dialog_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let Some(d) = self.dialog.clone() else {
            return;
        };
        match d {
            Dialog::Bounce => crate::screens::bounced_check::draw(self, ui, tokens),
            Dialog::Credits => {
                let (credits, first_open) = match self.form.figures() {
                    FiguresView::Ready(f) => (
                        f.credits.clone(),
                        f.rows
                            .iter()
                            .find(|r| r.remaining.is_positive())
                            .map(|r| r.invoice),
                    ),
                    _ => (Vec::new(), None),
                };
                let mut close = false;
                let mut uses = Vec::new();
                form::card(ui, tokens, |ui| {
                    ui.label(RichText::new(t("payment.credits.title")).strong());
                    if credits.is_empty() {
                        ui.label(t("payment.credits.none"));
                    }
                    for c in &credits {
                        ui.horizontal(|ui| {
                            let kind = if c.deposit {
                                t("payment.credits.deposit")
                            } else {
                                t("payment.credits.credit")
                            };
                            ui.label(format!("{kind} \u{b7} {}", mmdd(c.date)));
                            ui.label(mono(shown(c.remaining)));
                            let in_use = self.form.credits().iter().any(|x| x.source == c.source);
                            if let Some(inv) = first_open
                                && !in_use
                                && ui.button(t("payment.credits.use")).clicked()
                            {
                                uses.push((c.source, inv));
                            }
                        });
                    }
                    close = ui.button(t("payment.credits.close")).clicked();
                });
                for (s, i) in uses {
                    self.form.use_credit(s, i, None);
                    self.dirty = true;
                }
                if close {
                    self.dialog = None;
                }
            }
            Dialog::WriteOffAccount | Dialog::DiscountAccount => {
                let write_off = d == Dialog::WriteOffAccount;
                let types = if write_off {
                    vec![cl_api::AccountType::Expense]
                } else {
                    vec![cl_api::AccountType::Revenue]
                };
                let list: Vec<(AccountId, String)> = self
                    .accounts_of(&types)
                    .iter()
                    .map(|a| (a.id, crate::screens::register::account_text(a)))
                    .collect();
                let mut pick = if write_off {
                    self.write_off_pick
                } else {
                    self.form.discount_account
                };
                let mut go = false;
                let mut cancel = false;
                form::card(ui, tokens, |ui| {
                    ui.label(
                        RichText::new(t(if write_off {
                            "payment.account.write_off"
                        } else {
                            "payment.account.discount"
                        }))
                        .strong(),
                    );
                    // A plain list (one click picks), not a drop-down.
                    ui.label(t("payment.account.label"));
                    ui.horizontal_wrapped(|ui| {
                        for (id, name) in &list {
                            if ui.selectable_label(pick == Some(*id), name).clicked() {
                                pick = Some(*id);
                            }
                        }
                    });
                    ui.horizontal(|ui| {
                        go = ui
                            .add_enabled(
                                pick.is_some(),
                                egui::Button::new(t("payment.account.save")),
                            )
                            .clicked();
                        cancel = ui.button(t("payment.account.cancel")).clicked();
                    });
                });
                if write_off {
                    self.write_off_pick = pick;
                } else {
                    self.form.discount_account = pick;
                }
                if go {
                    self.dialog = None;
                    if write_off {
                        self.form.write_off(pick);
                    }
                    self.start_save(After::New, None);
                }
                if cancel {
                    self.dialog = None;
                }
            }
            Dialog::Refund(mut r) => {
                let banks: Vec<(AccountId, String)> = self
                    .deposit_accounts()
                    .iter()
                    .filter(|a| a.fields.detail_type.as_deref() == Some("bank"))
                    .map(|a| (a.id, crate::screens::register::account_text(a)))
                    .collect();
                let mut send = false;
                let mut close = false;
                form::card(ui, tokens, |ui| {
                    ui.label(RichText::new(t("payment.refund.title")).strong());
                    form::field(ui, &t("payment.refund.amount"), &mut r.amount);
                    account_combo(
                        ui,
                        &t("payment.refund.from"),
                        "payment-refund-from",
                        &mut r.from,
                        &banks,
                    );
                    form::field(ui, &t("payment.check_no"), &mut r.ref_no);
                    if let Some(e) = r.error {
                        ui.label(RichText::new(t(e)).color(tokens.warn_text));
                    }
                    ui.horizontal(|ui| {
                        send = ui.button(t("payment.refund.send")).clicked();
                        close = ui.button(t("payment.refund.later")).clicked();
                    });
                });
                if close {
                    self.dialog = None;
                } else {
                    self.dialog = Some(Dialog::Refund(r));
                    if send {
                        self.send_refund();
                    }
                }
            }
            Dialog::Void => {
                let mut go = false;
                let mut cancel = false;
                form::card(ui, tokens, |ui| {
                    ui.label(t("payment.void.ask"));
                    ui.horizontal(|ui| {
                        go = ui.button(t("payment.void.confirm")).clicked();
                        cancel = ui.button(t("payment.void.cancel")).clicked();
                    });
                });
                if go {
                    self.start_void(None);
                }
                if cancel {
                    self.dialog = None;
                }
            }
        }
    }
}
