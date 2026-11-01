//! Drawing Statements: the choices and the batch list on the left, the
//! selected client's statement on the right, the actions at the bottom,
//! and the email dialog. Figures are the engine's strings.

use cl_model::forms::statements::StatementFormatChoice;
use eframe::egui::{self, RichText, Ui};

use super::{Action, EmailWay, StatementsScreen};
use crate::i18n::{t, t_with};
use crate::layout::{Body, FixedFrame, FrameOpts};
use crate::screens::card_charge::kit;
use crate::screens::company::form;
use crate::theme::Tokens;

/// The left column's width.
const LEFT: f32 = 560.0;

fn mono(text: String) -> RichText {
    RichText::new(text).text_style(egui::TextStyle::Monospace)
}

impl StatementsScreen {
    pub(super) fn draw(&mut self, ui: &mut Ui) {
        let tokens = Tokens::of(ui.ctx());
        let mut frame = FixedFrame::begin(ui, "statements", FrameOpts::default());
        frame.top(|ui| self.top_ui(ui, &tokens));
        frame.body(Body::Scroll, |ui| self.body_ui(ui, &tokens));
        frame.bottom(|ui| self.actions_ui(ui, &tokens));
        self.layout = frame.end();
    }

    fn top_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        ui.label(
            RichText::new(t(&format!("cmd.{}", super::KIND)))
                .size(22.0)
                .strong()
                .color(tokens.text_primary),
        );
        if let Some(k) = self.notice {
            form::hint(ui, tokens, &t(k));
        }
        if let Some(e) = &self.error {
            form::inline_error(ui, tokens, e);
        }
        let keys: Vec<&'static str> = self.problems.iter().map(|p| p.key().0).collect();
        kit::problems(ui, tokens, &keys);
        if let Some(o) = self.email_outcome {
            form::hint(
                ui,
                tokens,
                &t_with(
                    "statements.email.result",
                    &[
                        ("sent", &o.sent.to_string()),
                        ("queued", &o.queued.to_string()),
                        ("failed", &o.failed.to_string()),
                    ],
                ),
            );
            if let Some(p) = o.problem {
                form::inline_warning(ui, tokens, &t(p));
            }
        }
    }

    fn body_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        if self.email.is_some() {
            self.email_ui(ui, tokens);
            ui.add_space(8.0);
        }
        if ui.available_width() > LEFT + 360.0 {
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(LEFT);
                    self.left_ui(ui, tokens);
                });
                ui.add_space(12.0);
                ui.vertical(|ui| {
                    ui.set_width(ui.available_width() - 30.0);
                    self.preview_ui(ui, tokens);
                });
            });
        } else {
            self.left_ui(ui, tokens);
            self.preview_ui(ui, tokens);
        }
    }

    fn left_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let today = self.today;
        form::card(ui, tokens, |ui| {
            crate::forms::header::wrap_row(ui, |ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                kit::wrap_item(ui, "statements/as_of", |ui| {
                    let (_, d) = kit::date_field(
                        ui,
                        tokens,
                        &t("statements.as_of"),
                        &mut self.as_of,
                        today,
                        120.0,
                    );
                    if let kit::DateChange::Set(d) = d {
                        self.set_as_of(d);
                    }
                });
                kit::wrap_item(ui, "statements/format", |ui| {
                    let l = kit::above(ui, tokens, &t("statements.format"));
                    let now = self.form.format;
                    let mut pick = None;
                    egui::ComboBox::from_id_salt("statements/format")
                        .width(150.0)
                        .selected_text(t(now.key().0))
                        .show_ui(ui, |ui| {
                            for f in [
                                StatementFormatChoice::OpenItem,
                                StatementFormatChoice::BalanceForward,
                                StatementFormatChoice::Transaction,
                            ] {
                                if ui.selectable_label(f == now, t(f.key().0)).clicked() {
                                    pick = Some(f);
                                }
                            }
                        })
                        .response
                        .labelled_by(l.id)
                        .on_hover_text(t("statements.format.tip"));
                    if let Some(f) = pick {
                        self.set_format(f);
                    }
                });
                if self.form.shows_start() {
                    kit::wrap_item(ui, "statements/start", |ui| {
                        let (_, d) = kit::date_field(
                            ui,
                            tokens,
                            &t("statements.start"),
                            &mut self.start,
                            today,
                            120.0,
                        );
                        if let kit::DateChange::Set(d) = d {
                            self.set_start(d);
                        }
                    });
                }
                kit::wrap_item(ui, "statements/min_days", |ui| {
                    let r = kit::text_field(
                        ui,
                        tokens,
                        &t("statements.min_days"),
                        &mut self.min_days,
                        110.0,
                        5,
                    );
                    if r.lost_focus() {
                        self.min_days_typed();
                    }
                });
            });
            let mut open = self.form.open_only;
            if ui
                .checkbox(&mut open, t("statements.open_only"))
                .on_hover_text(t("statements.open_only.tip"))
                .changed()
            {
                self.form.open_only = open;
                self.form.touched();
            }
        });
        ui.add_space(8.0);
        self.list_ui(ui, tokens);
    }

    fn list_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        if !self.loaded {
            form::progress_line(ui, tokens, &t("statements.loading"));
            return;
        }
        if self.rows.is_empty() {
            let date = self
                .form
                .as_of
                .map(crate::shell::us_date)
                .unwrap_or_default();
            form::hint(ui, tokens, &t_with("statements.none", &[("date", &date)]));
            return;
        }
        let style = self.money_style;
        ui.horizontal(|ui| {
            if ui.button(t("statements.select_all")).clicked() {
                self.select_all(true);
            }
            if ui.button(t("statements.select_none")).clicked() {
                self.select_all(false);
            }
            ui.label(
                RichText::new(t_with(
                    "statements.selected",
                    &[("n", &self.form.selected_count().to_string())],
                ))
                .color(tokens.text_secondary),
            );
        });
        let mut toggles = Vec::new();
        let mut show = None;
        egui::ScrollArea::vertical()
            .id_salt("statements/list")
            .max_height(420.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                egui::Grid::new("statements/rows")
                    .striped(true)
                    .num_columns(6)
                    .show(ui, |ui| {
                        ui.label("");
                        ui.strong(t("statements.col.client"));
                        ui.strong(t("statements.col.amount_due"));
                        ui.strong(t("statements.col.overdue"));
                        ui.strong(t("statements.col.email"));
                        ui.strong(t("statements.col.last"));
                        ui.end_row();
                        for r in &self.rows {
                            let mut on = self.form.is_selected(r.contact_id);
                            let label = t_with("statements.pick", &[("client", &r.name)]);
                            let c = ui.add(egui::Checkbox::without_text(&mut on));
                            c.widget_info(|| {
                                egui::WidgetInfo::selected(egui::Role::CheckBox, true, on, &label)
                            });
                            if c.clicked() {
                                toggles.push(r.contact_id);
                            }
                            if ui
                                .selectable_label(self.cursor == Some(r.contact_id), &r.name)
                                .on_hover_text(t("statements.show.tip"))
                                .clicked()
                            {
                                show = Some(r.contact_id);
                            }
                            ui.label(mono(r.amount_due.format(style)));
                            ui.label(
                                r.oldest_days_overdue
                                    .map(|d| t_with("statements.days", &[("n", &d.to_string())]))
                                    .unwrap_or_default(),
                            );
                            ui.label(if r.has_email {
                                t("statements.email.yes")
                            } else {
                                t("statements.email.print_instead")
                            });
                            ui.label(
                                r.last_statement
                                    .as_ref()
                                    .map(|l| {
                                        format!(
                                            "{} {}",
                                            l.number.clone().unwrap_or_default(),
                                            crate::shell::us_date(l.date)
                                        )
                                    })
                                    .unwrap_or_default(),
                            );
                            ui.end_row();
                        }
                    });
            });
        for c in toggles {
            self.form.toggle(c);
        }
        if let Some(c) = show {
            self.show_client(c);
        }
    }

    fn preview_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let style = self.money_style;
        let Some(p) = self.preview.clone() else {
            form::card(ui, tokens, |ui| {
                form::hint(ui, tokens, &t("statements.preview.none"));
            });
            return;
        };
        form::card(ui, tokens, |ui| {
            ui.label(RichText::new(t("statements.preview")).strong());
            ui.label(RichText::new(p.contact.name.clone()).strong().size(15.0));
            for l in &p.contact.address {
                ui.label(RichText::new(l).color(tokens.text_secondary));
            }
            ui.label(t_with(
                "statements.preview.as_of",
                &[("date", &crate::shell::us_date(p.as_of))],
            ));
            if p.has_difference() {
                form::inline_warning(ui, tokens, &t("statements.difference"));
            }
            ui.add_space(6.0);
            egui::Grid::new("statements/preview")
                .striped(true)
                .num_columns(5)
                .show(ui, |ui| {
                    let open_item = p.format == cl_api::StatementFormat::OpenItem;
                    ui.strong(t("statements.col.date"));
                    ui.strong(t("statements.col.transaction"));
                    ui.strong(t("statements.col.due"));
                    ui.strong(t("statements.col.amount"));
                    ui.strong(if open_item {
                        t("statements.col.open")
                    } else {
                        t("statements.col.balance")
                    });
                    ui.end_row();
                    if let Some(o) = p.opening_balance {
                        ui.label(p.start.map(crate::shell::us_date).unwrap_or_default());
                        ui.label(t(if p.format == cl_api::StatementFormat::Transaction {
                            "statements.previous_balance"
                        } else {
                            "statements.balance_forward"
                        }));
                        ui.label("");
                        ui.label("");
                        ui.label(mono(o.format(style)));
                        ui.end_row();
                    }
                    for l in &p.lines {
                        let word = super::print::txn_word(&l.txn_type);
                        let what = match &l.number {
                            Some(n) if !n.is_empty() => format!("{word} {n}"),
                            _ => word,
                        };
                        ui.label(mono(crate::shell::us_date(l.date)));
                        ui.label(what);
                        ui.label(mono(
                            l.due_date.map(crate::shell::us_date).unwrap_or_default(),
                        ));
                        ui.label(mono(l.amount.format(style)));
                        let last = if open_item {
                            l.open.unwrap_or(l.amount)
                        } else {
                            l.balance
                        };
                        ui.label(mono(last.format(style)));
                        ui.end_row();
                    }
                });
            ui.add_space(8.0);
            // The aging row (Q-177).
            let b = &p.aging.buckets;
            egui::Grid::new("statements/aging")
                .num_columns(6)
                .show(ui, |ui| {
                    for k in [
                        "statements.aging.current",
                        "statements.aging.1_30",
                        "statements.aging.31_60",
                        "statements.aging.61_90",
                        "statements.aging.over_90",
                        "statements.aging.total",
                    ] {
                        ui.label(RichText::new(t(k)).color(tokens.text_secondary).size(12.0));
                    }
                    ui.end_row();
                    for m in [
                        b.current,
                        b.days_1_30,
                        b.days_31_60,
                        b.days_61_90,
                        b.over_90,
                        p.aging.total,
                    ] {
                        ui.label(mono(m.format(style)));
                    }
                    ui.end_row();
                });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(t("statements.amount_due")).strong());
                ui.label(mono(p.amount_due.format(style)).strong());
            });
        });
        ui.add_space(8.0);
        form::card(ui, tokens, |ui| {
            let l = ui.label(t("statements.message"));
            ui.add(
                egui::TextEdit::multiline(&mut self.form.message)
                    .desired_rows(2)
                    .desired_width(f32::INFINITY)
                    .char_limit(1000),
            )
            .labelled_by(l.id);
        });
    }

    fn actions_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let idle = !self.busy() && self.email.is_none();
        let any = self.form.selected_count() > 0;
        let mut go = None;
        ui.horizontal_wrapped(|ui| {
            if self.busy() {
                form::progress_line(ui, tokens, &t("statements.working"));
            }
            ui.checkbox(&mut self.pdf_marks_sent, t("statements.pdf_marks_sent"))
                .on_hover_text(t("statements.pdf_marks_sent.tip"));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let primary = ui
                    .add_enabled_ui(idle && any, |ui| {
                        crate::widgets::primary_button(ui, &t("statements.print"))
                    })
                    .inner
                    .on_hover_text(t("statements.print.tip"));
                if primary.clicked() {
                    go = Some(Action::Print);
                }
                for (k, tip, a) in [
                    ("statements.email", "statements.email.tip", Action::Email),
                    ("statements.pdf", "statements.pdf.tip", Action::Pdf),
                    (
                        "statements.mark_sent",
                        "statements.mark_sent.tip",
                        Action::MarkSent,
                    ),
                ] {
                    if ui
                        .add_enabled(idle && any, egui::Button::new(t(k)))
                        .on_hover_text(t(tip))
                        .clicked()
                    {
                        go = Some(a);
                    }
                }
            });
        });
        if let Some(a) = go {
            self.start(a);
        }
    }

    fn email_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let Some(mut d) = self.email.clone() else {
            return;
        };
        let with: Vec<String> = self
            .issued
            .iter()
            .filter(|r| r.snapshot.contact.email.is_some())
            .map(|r| r.snapshot.contact.name.clone())
            .collect();
        let without: Vec<String> = self
            .rows
            .iter()
            .filter(|r| self.form.is_selected(r.contact_id) && !r.has_email)
            .map(|r| r.name.clone())
            .collect();
        let mut way = None;
        let mut cancel = false;
        form::card(ui, tokens, |ui| {
            ui.label(RichText::new(t("statements.email.title")).strong());
            ui.label(t_with(
                "statements.email.to",
                &[("clients", &with.join(", "))],
            ));
            if !without.is_empty() {
                form::inline_warning(
                    ui,
                    tokens,
                    &t_with(
                        "statements.email.print_these",
                        &[("clients", &without.join(", "))],
                    ),
                );
            }
            form::field_wide(ui, &t("statements.email.cc"), &mut d.cc);
            form::field_wide(ui, &t("statements.email.subject"), &mut d.subject);
            let l = ui.label(t("statements.email.body"));
            ui.add(
                egui::TextEdit::multiline(&mut d.body)
                    .desired_rows(6)
                    .desired_width(f32::INFINITY),
            )
            .labelled_by(l.id);
            form::hint(ui, tokens, &t("statements.email.fields"));
            let busy = self.email_job.is_some();
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(!busy, egui::Button::new(t("statements.email.send")))
                    .clicked()
                {
                    way = Some(EmailWay::Now);
                }
                if ui
                    .add_enabled(!busy, egui::Button::new(t("statements.email.outbox")))
                    .on_hover_text(t("statements.email.outbox.tip"))
                    .clicked()
                {
                    way = Some(EmailWay::Outbox);
                }
                cancel = ui.button(t("statements.email.cancel")).clicked();
            });
        });
        self.email = Some(d);
        if let Some(w) = way {
            self.send_email(w);
        }
        if cancel {
            self.email = None;
        }
    }
}
