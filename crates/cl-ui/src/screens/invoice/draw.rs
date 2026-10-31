//! Drawing Create Invoice: the toolbar, the messages, the header card, the
//! lines grid, the message and totals, the side panel and the actions.
//! Every figure shown is the engine's (Q-P20n).

use cl_core::{CalendarDate, Money};
use cl_model::CommandId;
use cl_model::forms::invoice::{CUSTOM_TERMS, TotalsView};
use eframe::egui::{self, CornerRadius, Margin, RichText, Stroke, Ui};

use super::{InvoiceScreen, TERMS, field};
use crate::forms::closing_prompt::ClosingClick;
use crate::forms::header;
use crate::i18n::{t, t_with};
use crate::layout::{Body, FixedFrame, FrameOpts};
use crate::screens::card_charge::kit::{self, NameOutcome};
use crate::screens::company::form;
use crate::theme::Tokens;

/// A figure as the totals show it.
enum Figure {
    Dash,
    Updating,
    Cant,
    Value(Money),
}

impl Figure {
    fn text(&self, style: cl_core::MoneyStyle) -> String {
        match self {
            Figure::Dash => "\u{2014}".into(),
            Figure::Updating => t("invoice.updating"),
            Figure::Cant => t("invoice.cant_calculate"),
            Figure::Value(m) => m.format(style),
        }
    }
}

impl InvoiceScreen {
    pub(crate) fn draw(&mut self, ui: &mut Ui) {
        let tokens = Tokens::of(ui.ctx());
        if !self.books.loaded {
            form::progress_line(ui, &tokens, &t("invoice.loading"));
            return;
        }
        let mut frame = FixedFrame::begin(ui, "create_invoice", FrameOpts::default());
        frame.top(|ui| {
            self.toolbar(ui, &tokens);
            self.messages(ui, &tokens);
        });
        frame.body(Body::Scroll, |ui| {
            let wide = ui.available_width() >= 860.0;
            if wide {
                let side = 270.0;
                let main = ui.available_width() - side - 20.0;
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(main);
                        self.main_ui(ui, &tokens);
                    });
                    ui.add_space(16.0);
                    ui.vertical(|ui| {
                        ui.set_width(side);
                        self.side_ui(ui, &tokens);
                    });
                });
            } else {
                self.main_ui(ui, &tokens);
                ui.add_space(12.0);
                self.side_ui(ui, &tokens);
            }
        });
        let read_only = self.read_only().is_some();
        let free = self.idle() && !self.dialog_open();
        frame.bottom(|ui| {
            if let Some(cmd) = kit::actions_row(ui, free && !read_only, free) {
                self.after_frame = Some(CommandId::new(cmd));
            }
        });
        self.layout = frame.end();
        self.billable_window(ui.ctx(), &tokens);
        self.email_window(ui.ctx(), &tokens);
    }

    fn toolbar(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let idle = self.idle() && !self.dialog_open();
        let saved = self.editing.is_some();
        let dirty = self.is_dirty();
        let open = self.read_only().is_none();
        let mut clicked: Option<&'static str> = None;
        let tip = |cmd: &str| kit::chip(cmd).replace(' ', "+");
        let nav = kit::toolbar(ui, tokens, idle, dirty, |ui| {
            ui.separator();
            ui.add_enabled_ui(idle, |ui| {
                if ui
                    .button(t("invoice.find"))
                    .on_hover_text(t("invoice.find.tip"))
                    .clicked()
                {
                    clicked = Some("edit.find");
                }
                if ui
                    .add_enabled(saved && !dirty, egui::Button::new(t("invoice.preview")))
                    .on_hover_text(t("invoice.preview.tip"))
                    .on_disabled_hover_text(t("invoice.print.save_first"))
                    .clicked()
                {
                    clicked = Some("invoice.preview");
                }
                if ui
                    .add_enabled(
                        saved && !dirty && open,
                        egui::Button::new(t("invoice.print")),
                    )
                    .on_hover_text(t("invoice.print.tip"))
                    .on_disabled_hover_text(t("invoice.print.save_first"))
                    .clicked()
                {
                    clicked = Some("form.print");
                }
                if ui
                    .add_enabled(
                        saved && !dirty && open,
                        egui::Button::new(t("invoice.email")),
                    )
                    .on_hover_text(tip("invoice.email"))
                    .on_disabled_hover_text(t("invoice.email.save_first"))
                    .clicked()
                {
                    clicked = Some("invoice.email");
                }
                if ui
                    .add_enabled(saved, egui::Button::new(t("invoice.attach")))
                    .on_hover_text(t("invoice.attach.tip"))
                    .on_disabled_hover_text(t("invoice.attach.save_first"))
                    .clicked()
                {
                    clicked = Some("invoice.attach");
                }
                ui.separator();
                if ui
                    .add_enabled(open, egui::Button::new(t("invoice.add_billable")))
                    .on_hover_text(tip("invoice.add_billable"))
                    .clicked()
                {
                    clicked = Some("invoice.add_billable");
                }
                if ui
                    .add_enabled(saved && open, egui::Button::new(t("invoice.void")))
                    .on_hover_text(tip("edit.void_transaction"))
                    .clicked()
                {
                    clicked = Some("edit.void_transaction");
                }
            });
        });
        match nav.or(clicked) {
            Some("invoice.preview") => self.preview_pdf(),
            Some("invoice.attach") => self.start_attach(ui.ctx()),
            Some(cmd) => self.after_frame = Some(CommandId::new(cmd)),
            None => {}
        }
    }

    /// Preview: the saved invoice's PDF, full size (made on the queue).
    fn preview_pdf(&mut self) {
        self.open_pdf();
    }

    fn messages(&mut self, ui: &mut Ui, tokens: &Tokens) {
        match self
            .closing_prompt
            .window(ui.ctx(), tokens, "create_invoice")
        {
            ClosingClick::Submit => self.submit_closing(),
            ClosingClick::Cancel => {
                self.closing_prompt.cancel();
                self.void_open = false;
            }
            ClosingClick::None => {}
        }
        let asking = self.closing_prompt.dialog_open();
        if let Some(k) = self.notice {
            form::hint(ui, tokens, &t(k));
        }
        if let Some(k) = self.read_only() {
            form::hint(ui, tokens, &t(k));
        } else if self
            .editing
            .as_ref()
            .is_some_and(|e| cl_model::closing::is_locked(e.old_date, self.books.closing))
        {
            form::hint(ui, tokens, &t("closing.edit_note"));
        }
        // Q-P20f: when and to whom it was sent; editing keeps that history.
        if let Some(text) = self
            .editing
            .as_ref()
            .and_then(|e| super::email::sent_text(&e.record, self.email.own.as_deref()))
        {
            banner(ui, tokens, |ui| {
                ui.label(text);
                if self.is_dirty() {
                    ui.label(RichText::new(t("invoice.sent.editing")).color(tokens.text_secondary));
                }
            });
        }
        if self.offer_send {
            let mut send = false;
            let mut no = false;
            banner(ui, tokens, |ui| {
                ui.label(t("invoice.sent.offer"));
                send = ui.button(t("invoice.sent.offer.send")).clicked();
                no = ui.button(t("invoice.sent.offer.no")).clicked();
            });
            if send {
                self.resend();
            } else if no {
                self.offer_send = false;
            }
        }
        if self.stale {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(t("invoice.changed_elsewhere")).color(tokens.warn_text));
                if ui.button(t("invoice.reload")).clicked() {
                    self.reload();
                }
            });
        }
        if self.void_open && !asking {
            let mut send = false;
            let mut cancel = false;
            form::card(ui, tokens, |ui| {
                form::heading(ui, tokens, &t("invoice.void.title"));
                form::hint(ui, tokens, &t("invoice.void.explain"));
                let r = form::field(ui, &t("invoice.void.reason"), &mut self.void_reason);
                send |= crate::forms::enter::submitted(&r);
                ui.horizontal(|ui| {
                    send |= ui.button(t("invoice.void.confirm")).clicked();
                    cancel = ui.button(t("invoice.void.cancel")).clicked();
                });
            });
            if cancel {
                self.void_open = false;
                self.void_reason.clear();
            } else if send {
                self.send_void(None);
            }
        }
    }

    fn main_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        ui.label(RichText::new(t("invoice.heading")).size(22.0).strong());
        ui.add_space(6.0);
        let read_only = self.read_only().is_some();
        // A dialog over the form is modal: nothing behind it takes input.
        let dialog = self.dialog_open();
        let enabled = self.idle() && !read_only && !dialog;
        header::locked_scope(ui, enabled, read_only, |ui| self.header_ui(ui, tokens));
        ui.add_space(10.0);
        self.grid.set_suspended(dialog);
        header::locked_scope(ui, enabled, read_only, |ui| {
            let _ = self.grid.ui(ui);
            self.item_set_up_ui(ui);
        });
        ui.add_space(8.0);
        header::locked_scope(ui, enabled, read_only, |ui| self.footer_ui(ui, tokens));
        let keys: Vec<&'static str> = self.problems.iter().map(|p| p.key().0).collect();
        kit::problems(ui, tokens, &keys);
        if let Some(e) = &self.error {
            form::inline_error(ui, tokens, e);
        }
        if let Some(ev) = self.lost.ui(
            ui,
            &mut self.header,
            &self.link,
            self.group.as_ref(),
            "create_invoice",
        ) {
            self.lost_event(ev);
        }
        if self.busy() {
            form::progress_line(ui, tokens, &t("helper.saving"));
        }
    }

    fn header_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        egui::Frame::new()
            .fill(tokens.surface_form)
            .stroke(Stroke::new(1.0, tokens.accent_border))
            .corner_radius(CornerRadius::same(8))
            .inner_margin(Margin::symmetric(18, 16))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                header::wrap_row(ui, |ui| {
                    ui.spacing_mut().item_spacing.x = 12.0;
                    kit::wrap_item(ui, "invoice/client", |ui| self.client_ui(ui, tokens));
                    kit::wrap_item(ui, "invoice/date", |ui| {
                        let today = self.today;
                        let (r, d) = kit::date_field(
                            ui,
                            tokens,
                            &t("invoice.date"),
                            &mut self.inputs.date,
                            today,
                            120.0,
                        );
                        if let kit::DateChange::Set(d) = d {
                            self.header.edit(field::DATE, |h| h.date = d);
                            self.inputs.rev = self.header.revision();
                        }
                        kit::track(&mut self.focused, &r, field::DATE);
                    });
                    kit::wrap_item(ui, "invoice/number", |ui| {
                        let mut text = self.header.current().number.clone();
                        let r = kit::text_field(
                            ui,
                            tokens,
                            &t("invoice.number"),
                            &mut text,
                            100.0,
                            cl_model::forms::invoice::NUMBER_MAX,
                        )
                        .on_hover_text(t("invoice.number.tip"));
                        if r.changed() {
                            self.header.edit(field::NUMBER, |h| h.number = text);
                            self.inputs.rev = self.header.revision();
                        }
                        kit::track(&mut self.focused, &r, field::NUMBER);
                        // Q-P20ad: the counter's next number while blank;
                        // the engine confirms it on save.
                        if self.editing.is_none()
                            && self.header.current().number.trim().is_empty()
                            && let Some(n) = &self.next_number
                        {
                            ui.label(
                                RichText::new(t_with("invoice.number.next", &[("number", n)]))
                                    .size(12.0)
                                    .color(tokens.text_secondary),
                            );
                        }
                    });
                });
                ui.add_space(8.0);
                header::wrap_row(ui, |ui| {
                    ui.spacing_mut().item_spacing.x = 12.0;
                    kit::wrap_item(ui, "invoice/terms", |ui| self.terms_ui(ui, tokens));
                    if !self.print_books.layouts.is_empty() {
                        kit::wrap_item(ui, "invoice/template", |ui| self.template_ui(ui, tokens));
                    }
                    kit::wrap_item(ui, "invoice/due", |ui| {
                        let today = self.today;
                        if self.inputs.due.text.is_empty()
                            && let Some(d) = self.tracker.shown_due_date(self.header.current())
                            && self.focused.is_none_or(|f| f != field::DUE)
                        {
                            // The engine's due date from the terms (Q-P20g),
                            // shown until the owner types one.
                            let mut shown = super::super::company::form::DateInput::new(d);
                            let (r, _) = kit::date_field(
                                ui,
                                tokens,
                                &t("invoice.due"),
                                &mut shown,
                                today,
                                120.0,
                            );
                            if r.has_focus() {
                                self.inputs.due = shown;
                                self.focused = Some(field::DUE);
                            }
                            return;
                        }
                        let (r, d) = kit::date_field(
                            ui,
                            tokens,
                            &t("invoice.due"),
                            &mut self.inputs.due,
                            today,
                            120.0,
                        );
                        if let kit::DateChange::Set(d) = d {
                            self.header.edit(field::DUE, |h| h.due_date = d);
                            self.inputs.rev = self.header.revision();
                        }
                        kit::track(&mut self.focused, &r, field::DUE);
                    });
                    kit::wrap_item(ui, "invoice/po", |ui| {
                        let mut text = self.header.current().po_number.clone();
                        let r = kit::text_field(
                            ui,
                            tokens,
                            &t("invoice.po"),
                            &mut text,
                            140.0,
                            cl_model::forms::invoice::PO_MAX,
                        );
                        if r.changed() {
                            self.header.edit(field::PO, |h| h.po_number = text);
                            self.inputs.rev = self.header.revision();
                        }
                        kit::track(&mut self.focused, &r, field::PO);
                    });
                });
                ui.add_space(6.0);
                let tags = self.books.tags.clone();
                let on = self.header.current().header_tags.clone();
                if let Some(edit) = kit::tags_row(ui, tokens, "invoice/tags", &tags, &on) {
                    self.header
                        .edit_step(|h| kit::apply_tag_edit(&tags, &mut h.header_tags, edit));
                }
            });
    }

    fn client_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let lists = std::sync::Arc::clone(&self.clients);
        let current = self.header.current().client;
        let (outcome, r) = kit::name_field(
            ui,
            tokens,
            &t("invoice.client"),
            &mut self.inputs.client,
            &lists,
            current,
            260.0,
        );
        kit::track(&mut self.focused, &r, field::CLIENT);
        match outcome {
            NameOutcome::Picked(id) => {
                if current != Some(id) {
                    self.pick_client(id);
                }
            }
            NameOutcome::Cleared => {
                let typed = self.inputs.client.text.trim().to_string();
                if self.inputs.client.problem && !typed.is_empty() {
                    self.unknown_client = Some(typed);
                } else if current.is_some() {
                    self.header.edit(field::CLIENT, |h| h.client = None);
                    self.client_changed();
                }
            }
            NameOutcome::None => {}
        }
        // Q-P20i: a name not in the list offers Set Up (no Quick Add).
        if let Some(name) = self.unknown_client.clone() {
            let label = t_with("invoice.client.set_up", &[("name", &name)]);
            if ui
                .button(label)
                .on_hover_text(t("invoice.client.set_up.tip"))
                .clicked()
            {
                self.setting_up = Some(name);
                self.link
                    .push(crate::company::CompanyEvent::OpenWindow(CommandId::new(
                        "clients.center",
                    )));
            }
        }
    }

    /// Q-P20i: an item name not in the list offers Set Up (the full item
    /// form); the new item fills the line when it is saved.
    fn item_set_up_ui(&mut self, ui: &mut Ui) {
        let Some((row, name)) = self.grid.unknown_item().cloned() else {
            return;
        };
        let label = t_with("invoice.item.set_up", &[("name", &name)]);
        if ui
            .button(label)
            .on_hover_text(t("invoice.item.set_up.tip"))
            .clicked()
        {
            self.start_item_set_up(row, name);
        }
    }

    /// Template (Q-P20ac, U-130): the invoice's print layout. Archived
    /// layouts are not offered; an invoice that has one keeps showing it.
    fn template_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        use cl_model::forms::invoice::LayoutPick;
        let l = kit::above(ui, tokens, &t("invoice.template"));
        let current = self.header.current().layout.clone();
        let name_of = |id: &str| {
            self.print_books
                .layouts
                .iter()
                .find(|x| x.id.to_string() == id)
                .map(|x| x.name.clone())
        };
        let shown = match &current {
            LayoutPick::Unset => t("invoice.template.client"),
            LayoutPick::CompanyDefault => t("invoice.template.default"),
            LayoutPick::Layout(id) => name_of(id).unwrap_or_else(|| t("invoice.template.default")),
        };
        let offered: Vec<(String, String)> = self
            .print_books
            .layouts
            .iter()
            .filter(|x| !x.is_archived)
            .map(|x| (x.id.to_string(), x.name.clone()))
            .collect();
        let mut pick: Option<LayoutPick> = None;
        ui.add_enabled_ui(self.read_only().is_none(), |ui| {
            egui::ComboBox::from_id_salt("invoice/template")
                .selected_text(shown)
                .width(150.0)
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(
                            current == LayoutPick::CompanyDefault,
                            t("invoice.template.default"),
                        )
                        .clicked()
                    {
                        pick = Some(LayoutPick::CompanyDefault);
                    }
                    for (id, name) in &offered {
                        let on = current == LayoutPick::Layout(id.clone());
                        if ui.selectable_label(on, name).clicked() {
                            pick = Some(LayoutPick::Layout(id.clone()));
                        }
                    }
                })
                .response
                .labelled_by(l.id)
                .on_hover_text(t("invoice.template.tip"));
        });
        if let Some(p) = pick {
            self.header.edit_step(|h| h.layout = p);
        }
    }

    fn terms_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let l = kit::above(ui, tokens, &t("invoice.terms"));
        let current = self.header.current().terms.clone();
        let shown = current.as_deref().map_or_else(
            || t("invoice.terms.default"),
            |c| t(&format!("invoice.terms.{c}")),
        );
        let mut pick: Option<Option<String>> = None;
        egui::ComboBox::from_id_salt("invoice/terms")
            .selected_text(shown)
            .width(150.0)
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(current.is_none(), t("invoice.terms.default"))
                    .clicked()
                {
                    pick = Some(None);
                }
                for code in TERMS {
                    if ui
                        .selectable_label(
                            current.as_deref() == Some(code),
                            t(&format!("invoice.terms.{code}")),
                        )
                        .clicked()
                    {
                        pick = Some(Some(code.to_string()));
                    }
                }
            })
            .response
            .labelled_by(l.id);
        if let Some(p) = pick {
            // Terms change the engine's due date; a typed one goes.
            self.header.edit_step(|h| {
                h.terms = p;
                h.due_date = None;
            });
            self.inputs.rev = u64::MAX;
        }
        if self.header.current().terms.as_deref() == Some(CUSTOM_TERMS) {
            let r = kit::text_field(
                ui,
                tokens,
                &t("invoice.terms.days"),
                &mut self.inputs.terms_days,
                70.0,
                4,
            );
            if r.changed() {
                let days = self.inputs.terms_days.trim().parse::<u16>().ok();
                self.header.edit(field::TERMS_DAYS, |h| h.terms_days = days);
            }
        }
    }

    fn footer_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let wide = ui.available_width() >= 620.0;
        let left = |s: &mut InvoiceScreen, ui: &mut Ui| {
            let l = kit::above(ui, tokens, &t("invoice.message"));
            let mut text = s.header.current().customer_message.clone();
            let r = ui
                .add(
                    egui::TextEdit::multiline(&mut text)
                        .desired_rows(2)
                        .desired_width(ui.available_width().min(420.0))
                        .char_limit(cl_model::forms::invoice::TEXT_MAX),
                )
                .labelled_by(l.id)
                .on_hover_text(t("invoice.message.tip"));
            if r.changed() {
                s.header.edit(field::MESSAGE, |h| h.customer_message = text);
            }
            kit::track(&mut s.focused, &r, field::MESSAGE);
            let mut memo = s.header.current().memo.clone();
            let r = kit::text_field(ui, tokens, &t("invoice.memo"), &mut memo, 420.0, 4000)
                .on_hover_text(t("invoice.memo.tip"));
            if r.changed() {
                s.header.edit(field::MEMO, |h| h.memo = memo);
            }
            kit::track(&mut s.focused, &r, field::MEMO);
            ui.checkbox(&mut s.email_after_save, t("invoice.email_after_save"));
            let mut hide = s.header.current().hide_zero_lines;
            if ui.checkbox(&mut hide, t("invoice.hide_zero")).changed() {
                s.header.edit_step(|h| h.hide_zero_lines = hide);
            }
        };
        if wide {
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_width((ui.available_width() - 320.0).max(260.0));
                    left(self, ui);
                });
                ui.add_space(12.0);
                ui.vertical(|ui| {
                    ui.set_width(300.0);
                    self.totals_ui(ui, tokens);
                });
            });
        } else {
            left(self, ui);
            ui.add_space(8.0);
            self.totals_ui(ui, tokens);
        }
    }

    fn totals_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let style = self.money_style;
        let view = self.tracker.totals();
        let pricing = self.caps().pricing;
        let fig = |f: fn(&cl_model::forms::invoice::PreviewTotals) -> Money| match view {
            TotalsView::Nothing => Figure::Dash,
            TotalsView::Updating => Figure::Updating,
            TotalsView::CantCalculate => Figure::Cant,
            TotalsView::Ready(t) => Figure::Value(f(t)),
        };
        let rate = match view {
            TotalsView::Ready(t) => t.tax_rate.map(|r| {
                format!(
                    "{}%",
                    (r.as_decimal() * rust_decimal::Decimal::ONE_HUNDRED).normalize()
                )
            }),
            _ => None,
        };
        let tax_label = rate.map_or_else(
            || t("invoice.totals.tax"),
            |r| t_with("invoice.totals.tax_rate", &[("rate", &r)]),
        );
        // Payments are the record's while editing (S7); the balance due is
        // the engine's: the record's when nothing changed, a dash until
        // the edit is saved (the UI subtracts nothing).
        let paid = self.editing.as_ref().and_then(|e| e.record.amount_paid);
        let has_paid = paid.is_some_and(|p| !p.is_zero());
        let balance = match self.editing.as_ref() {
            Some(e) if !self.is_dirty() => e.record.balance_due.map_or(Figure::Dash, Figure::Value),
            Some(_) if has_paid => Figure::Dash,
            _ => fig(|t| t.total),
        };
        egui::Frame::new()
            .fill(tokens.surface_subtle)
            .stroke(Stroke::new(1.0, tokens.border_default))
            .corner_radius(CornerRadius::same(6))
            .inner_margin(Margin::symmetric(14, 12))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                let row = |ui: &mut Ui, label: &str, value: String, strong: bool| {
                    ui.horizontal(|ui| {
                        let l = if strong {
                            RichText::new(label).strong()
                        } else {
                            RichText::new(label).color(tokens.text_secondary)
                        };
                        ui.label(l);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let v = RichText::new(value).monospace();
                            ui.label(if strong { v.strong() } else { v });
                        });
                    });
                };
                if !pricing {
                    ui.label(
                        RichText::new(t("invoice.totals.after_save")).color(tokens.text_secondary),
                    );
                    return;
                }
                row(
                    ui,
                    &t("invoice.totals.subtotal"),
                    fig(|t| t.subtotal).text(style),
                    false,
                );
                row(ui, &tax_label, fig(|t| t.tax_total).text(style), false);
                ui.separator();
                row(
                    ui,
                    &t("invoice.totals.total"),
                    fig(|t| t.total).text(style),
                    true,
                );
                row(
                    ui,
                    &t("invoice.totals.payments"),
                    paid.unwrap_or(Money::ZERO).format(style),
                    false,
                );
                row(
                    ui,
                    &t("invoice.totals.balance_due"),
                    balance.text(style),
                    true,
                );
                if let Some(p) = self.preview_problem
                    && matches!(view, TotalsView::CantCalculate)
                {
                    ui.label(RichText::new(t(p)).size(12.0).color(tokens.warn_text));
                }
            });
    }

    fn side_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let style = self.money_style;
        let client = self.header.current().client;
        let contact = client.and_then(|c| self.books.contacts.iter().find(|x| x.id == c).cloned());
        let Some(contact) = contact else {
            kit::panel_card(ui, tokens, &t("invoice.side.client"), |ui| {
                ui.label(RichText::new(t("invoice.side.pick_client")).color(tokens.text_secondary));
            });
            return;
        };
        let sales = self
            .sales
            .as_ref()
            .filter(|(c, _)| Some(*c) == client)
            .map(|(_, s)| *s);
        let after = match self.tracker.totals() {
            TotalsView::Ready(t) => t.balance_after.map(|m| m.format(style)),
            TotalsView::Updating => Some(t("invoice.updating")),
            _ => None,
        };
        let today = self.today;
        kit::panel_card(ui, tokens, &contact.fields.display_name, |ui| {
            kit::balance_line(
                ui,
                tokens,
                &t("invoice.side.open_balance"),
                sales
                    .as_ref()
                    .and_then(|s| s.open_balance)
                    .map(|m| m.format(style)),
            );
            kit::balance_line(ui, tokens, &t("invoice.side.after"), after);
            let oldest = sales
                .as_ref()
                .and_then(|s| s.oldest_open_date)
                .map(|d| days_text(d, today));
            kit::balance_line(ui, tokens, &t("invoice.side.oldest"), oldest);
            if ui.link(t("invoice.side.open_client")).clicked() {
                self.link
                    .push(crate::company::CompanyEvent::OpenWindow(CommandId::new(
                        "clients.center",
                    )));
            }
        });
        ui.add_space(10.0);
        // Ship to (Q-P20m): beside the header, outside the envelope windows
        // (Q-P20v); printed only when it differs from Bill to.
        let f = &contact.fields;
        let ship: Vec<String> = [
            f.shipping_line1.clone(),
            f.shipping_line2.clone(),
            f.shipping_city.clone(),
        ]
        .into_iter()
        .flatten()
        .filter(|l| !l.trim().is_empty())
        .collect();
        if !ship.is_empty() {
            kit::panel_card(ui, tokens, &t("invoice.side.ship_to"), |ui| {
                for l in &ship {
                    ui.label(l);
                }
            });
            ui.add_space(10.0);
        }
        self.billable_note(ui, tokens);
        self.attach_card(ui, tokens);
        if let Some(notes) = f.notes.clone().filter(|n| !n.trim().is_empty()) {
            ui.add_space(10.0);
            kit::panel_card(ui, tokens, &t("invoice.side.notes"), |ui| {
                ui.label(RichText::new(notes).color(tokens.text_secondary));
            });
        }
    }
}

/// "18 days".
fn days_text(d: CalendarDate, today: CalendarDate) -> String {
    let n = d.days_until(today).max(0);
    crate::i18n::t_count("invoice.side.days", usize::try_from(n).unwrap_or(0), &[])
}

fn banner(ui: &mut Ui, tokens: &Tokens, add: impl FnOnce(&mut Ui)) {
    egui::Frame::new()
        .fill(tokens.info_tint)
        .stroke(Stroke::new(1.0, tokens.accent_border))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(add);
        });
}
