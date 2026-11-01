//! Drawing Assess Finance Charges: the date and month, the setup fields
//! (rate blank), the engine's rows with their invoices, the result and the
//! actions. Figures are the engine's strings.

use cl_api::{FinanceChargePreview, FinanceChargeRow, FinanceChargeSkip};
use eframe::egui::{self, RichText, Ui};

use super::{FinanceChargesScreen, month_label};
use crate::forms::closing_prompt::ClosingClick;
use crate::i18n::{t, t_with};
use crate::layout::{Body, FixedFrame, FrameOpts};
use crate::screens::card_charge::kit;
use crate::screens::company::form;
use crate::theme::Tokens;

fn mono(text: String) -> RichText {
    RichText::new(text).text_style(egui::TextStyle::Monospace)
}

fn skip_key(s: FinanceChargeSkip) -> &'static str {
    match s {
        FinanceChargeSkip::InCredit => "finance_charges.skipped.in_credit",
        FinanceChargeSkip::Inactive => "finance_charges.skipped.inactive",
        FinanceChargeSkip::Other => "finance_charges.skipped.other",
    }
}

impl FinanceChargesScreen {
    pub(super) fn draw(&mut self, ui: &mut Ui) {
        let tokens = Tokens::of(ui.ctx());
        let mut frame = FixedFrame::begin(ui, "finance_charges", FrameOpts::default());
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
        match self.closing_prompt.ui(ui, tokens) {
            ClosingClick::Submit => self.submit_closing(),
            ClosingClick::Cancel => self.closing_prompt.cancel(),
            ClosingClick::None => {}
        }
        if let Some(k) = self.notice.filter(|k| *k != "finance_charges.done") {
            let month = self
                .refused_period
                .as_deref()
                .map(month_label)
                .unwrap_or_default();
            form::inline_warning(ui, tokens, &t_with(k, &[("month", &month)]));
        }
        if !self.changed_names.is_empty() {
            form::inline_warning(
                ui,
                tokens,
                &t_with(
                    "finance_charges.refusal.changed_clients",
                    &[("clients", &self.changed_names.join(", "))],
                ),
            );
        }
        if self.partial {
            form::inline_warning(ui, tokens, &t("finance_charges.done.partial"));
        }
        if let Some(e) = &self.error {
            form::inline_error(ui, tokens, e);
        }
        let keys: Vec<&'static str> = self.problems.iter().map(|p| p.key().0).collect();
        kit::problems(ui, tokens, &keys);
    }

    fn body_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let today = self.today;
        form::card(ui, tokens, |ui| {
            crate::forms::header::wrap_row(ui, |ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                kit::wrap_item(ui, "finance_charges/date", |ui| {
                    let (_, d) = kit::date_field(
                        ui,
                        tokens,
                        &t("finance_charges.date"),
                        &mut self.date,
                        today,
                        120.0,
                    );
                    if let kit::DateChange::Set(d) = d {
                        self.set_date(d);
                    }
                });
            });
            if let Some(p) = self.current() {
                let text = t_with(
                    "finance_charges.period",
                    &[("month", &month_label(&p.period))],
                );
                ui.label(RichText::new(text).strong());
            }
            if self.setup.is_none()
                && let Some(s) = self.current().and_then(|p| p.settings.clone())
                && let Some(rate) = s.annual_rate
            {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(t_with(
                            "finance_charges.settings",
                            &[
                                ("rate", &rate.to_wire()),
                                ("grace", &s.grace_days.unwrap_or(0).to_string()),
                                (
                                    "minimum",
                                    &s.minimum
                                        .unwrap_or(cl_core::Money::ZERO)
                                        .format(self.money_style),
                                ),
                            ],
                        ))
                        .color(tokens.text_secondary),
                    );
                    if ui.button(t("finance_charges.setup.change")).clicked() {
                        self.open_setup();
                    }
                });
            }
        });
        ui.add_space(8.0);
        if !self.results.is_empty() {
            self.results_ui(ui, tokens);
            ui.add_space(8.0);
        }
        if self.setup.is_some() {
            self.setup_ui(ui, tokens);
            return;
        }
        self.rows_ui(ui, tokens);
    }

    fn setup_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let incomes = self.incomes();
        let required = self.current().is_some_and(FinanceChargePreview::not_set_up);
        let busy = self.setup_job.is_some();
        let mut save = false;
        let mut cancel = false;
        form::card(ui, tokens, |ui| {
            form::heading(ui, tokens, &t("finance_charges.setup.title"));
            if required {
                form::hint(ui, tokens, &t("finance_charges.setup.intro"));
            }
            if let Some(edits) = self.setup.as_mut() {
                crate::screens::preferences::company::finance_charge_fields(
                    ui, tokens, edits, &incomes,
                );
            }
            if let Some(k) = self.setup_problem {
                form::inline_warning(ui, tokens, &t(k));
            }
            if busy {
                form::progress_line(ui, tokens, &t("finance_charges.setup.saving"));
            }
            ui.horizontal(|ui| {
                save = ui
                    .add_enabled_ui(!busy, |ui| {
                        crate::widgets::primary_button(ui, &t("finance_charges.setup.save"))
                    })
                    .inner
                    .clicked();
                if !required {
                    cancel = ui.button(t("finance_charges.setup.cancel")).clicked();
                }
            });
        });
        if save {
            self.save_setup();
        }
        if cancel {
            self.cancel_setup();
        }
    }

    fn rows_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let Some(p) = self.current().cloned() else {
            form::progress_line(ui, tokens, &t("finance_charges.loading"));
            return;
        };
        let month = month_label(&p.period);
        if p.rows.is_empty() {
            form::hint(
                ui,
                tokens,
                &t_with("finance_charges.none", &[("month", &month)]),
            );
            return;
        }
        let style = self.money_style;
        let mut toggles = Vec::new();
        let mut expand = Vec::new();
        form::card(ui, tokens, |ui| {
            egui::Grid::new("finance_charges/rows")
                .striped(true)
                .num_columns(5)
                .show(ui, |ui| {
                    ui.label("");
                    ui.strong(t("finance_charges.col.client"));
                    ui.strong(t("finance_charges.col.overdue"));
                    ui.strong(t("finance_charges.col.charge"));
                    ui.strong(t("finance_charges.col.note"));
                    ui.end_row();
                    for r in &p.rows {
                        self.row_ui(ui, tokens, r, &month, &mut toggles, &mut expand);
                    }
                });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(t_with(
                        "finance_charges.totals",
                        &[
                            ("count", &p.totals.count.to_string()),
                            ("total", &p.totals.charge.format(style)),
                        ],
                    ))
                    .strong(),
                );
                ui.label(
                    RichText::new(t_with(
                        "finance_charges.selected",
                        &[("n", &self.ticked_count().to_string())],
                    ))
                    .color(tokens.text_secondary),
                );
            });
        });
        for c in toggles {
            self.toggle(c);
        }
        for c in expand {
            self.expand(c);
        }
    }

    fn row_ui(
        &self,
        ui: &mut Ui,
        tokens: &Tokens,
        r: &FinanceChargeRow,
        month: &str,
        toggles: &mut Vec<cl_core::ids::ContactId>,
        expand: &mut Vec<cl_core::ids::ContactId>,
    ) {
        let style = self.money_style;
        let chargeable = r.chargeable();
        let mut on = self.form.is_ticked(r.contact_id);
        let label = t_with("finance_charges.pick", &[("client", &r.name)]);
        let c = ui.add_enabled(chargeable, egui::Checkbox::without_text(&mut on));
        c.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, chargeable, on, &label));
        if c.clicked() {
            toggles.push(r.contact_id);
        }
        let open = self.expanded.contains(&r.contact_id);
        if r.sources.is_empty() {
            ui.label(&r.name);
        } else {
            let tip = t_with(
                if open {
                    "finance_charges.hide"
                } else {
                    "finance_charges.show"
                },
                &[("client", &r.name)],
            );
            let b = ui.selectable_label(open, &r.name).on_hover_text(&tip);
            b.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &tip));
            if b.clicked() {
                expand.push(r.contact_id);
            }
        }
        let dim = |text: String| {
            if chargeable {
                mono(text)
            } else {
                mono(text).color(tokens.text_secondary)
            }
        };
        ui.label(dim(r.overdue_balance.format(style)));
        ui.label(dim(r.charge.format(style)));
        let note = if let Some(a) = &r.already_assessed {
            t_with(
                "finance_charges.already",
                &[
                    ("month", month),
                    ("number", a.number.as_deref().unwrap_or_default()),
                ],
            )
        } else if let Some(s) = r.skipped_reason {
            t(skip_key(s))
        } else if let Some(m) = r.has_open_credits {
            t_with(
                "finance_charges.open_credits",
                &[("amount", &m.format(style))],
            )
        } else {
            String::new()
        };
        ui.label(RichText::new(note).color(tokens.text_secondary));
        ui.end_row();
        if open {
            for s in &r.sources {
                ui.label("");
                ui.label(
                    RichText::new(t_with(
                        "finance_charges.source",
                        &[
                            ("number", s.number.as_deref().unwrap_or_default()),
                            ("due", &crate::shell::us_date(s.due_date)),
                            ("open", &s.open.format(style)),
                            ("days", &s.days.to_string()),
                            ("from", &crate::shell::us_date(s.from_date)),
                            ("amount", &s.amount.format(style)),
                        ],
                    ))
                    .color(tokens.text_secondary)
                    .size(12.0),
                );
                ui.end_row();
            }
        }
    }

    fn results_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let style = self.money_style;
        let mut open = None;
        form::card(ui, tokens, |ui| {
            ui.label(
                RichText::new(t_with(
                    "finance_charges.done",
                    &[("n", &self.results.len().to_string())],
                ))
                .strong(),
            );
            if self.print_later() {
                form::hint(ui, tokens, &t("finance_charges.done.queued"));
            }
            for inv in &self.results {
                let number = inv.number.clone().unwrap_or_default();
                ui.horizontal(|ui| {
                    ui.label(mono(t_with(
                        "finance_charges.result.row",
                        &[
                            ("number", &number),
                            (
                                "client",
                                self.names
                                    .get(&inv.contact_id)
                                    .map(String::as_str)
                                    .unwrap_or_default(),
                            ),
                            ("total", &inv.total.format(style)),
                        ],
                    )));
                    let label = t_with("finance_charges.result.open", &[("number", &number)]);
                    if ui.button(&label).clicked() {
                        open = Some(inv.clone());
                    }
                });
            }
        });
        if let Some(inv) = open {
            self.open_invoice(&inv);
        }
    }

    fn actions_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let idle = !self.busy()
            && self.preview_job.is_none()
            && self.setup.is_none()
            && !self.closing_prompt.dialog_open();
        let any = self.ticked_count() > 0;
        let mut go = false;
        let mut close = false;
        ui.horizontal_wrapped(|ui| {
            if self.assess_job.is_some() {
                form::progress_line(ui, tokens, &t("finance_charges.assessing"));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                go = ui
                    .add_enabled_ui(idle && any, |ui| {
                        crate::widgets::primary_button(ui, &t("finance_charges.assess"))
                    })
                    .inner
                    .on_hover_text(t("finance_charges.assess.tip"))
                    .clicked();
                close = ui.button(t("finance_charges.close")).clicked();
            });
        });
        if go {
            // The Assess button stays with the results (as Alt+N).
            self.close_after = false;
            self.start_assess(None);
        }
        if close {
            self.link.push(crate::company::CompanyEvent::CloseWindow(
                cl_model::CommandId::new(super::KIND),
            ));
        }
    }
}
