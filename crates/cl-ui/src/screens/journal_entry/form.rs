//! Drawing Make Journal Entry: the toolbar, the messages, the header card,
//! the lines grid, the totals line and the action row.

use cl_model::CommandId;
use cl_model::forms::journal::{JournalProblem, save_blocker, totals};
use eframe::egui::{self, Align, CornerRadius, Layout, Margin, RichText, Stroke, Ui, Vec2};

use super::{JournalScreen, field};
use crate::forms::closing_prompt::ClosingClick;
use crate::forms::header;
use crate::i18n::{t, t_with};
use crate::screens::card_charge::kit::{self, ACTION_HEIGHT};
use crate::screens::company::form;
use crate::theme::Tokens;

impl JournalScreen {
    pub(crate) fn draw(&mut self, ui: &mut Ui) {
        let tokens = Tokens::of(ui.ctx());
        if !self.books.loaded {
            form::progress_line(ui, &tokens, &t("journal.loading"));
            return;
        }
        self.toolbar(ui, &tokens);
        self.messages(ui, &tokens);
        if self.nav_prompt.is_some()
            && let Some(a) =
                crate::forms::nav_prompt::ui(ui, "journal_entry", &t("cmd.company.journal_entry"))
        {
            self.answer_nav(a);
        }
        if self.books.accounts.is_empty() {
            form::hint(ui, &tokens, &t("journal.no_accounts"));
            return;
        }
        egui::ScrollArea::vertical()
            .id_salt("journal_entry/scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| self.form_ui(ui, &tokens));
    }

    fn toolbar(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let idle = self.idle();
        let template = self.template_state();
        let saved = self.editing.is_some();
        let mut clicked: Option<&'static str> = None;
        let dirty = self.is_dirty();
        let link = std::sync::Arc::clone(&self.link);
        let (flow, group, today) = (&mut self.template, self.group.as_ref(), self.today);
        let nav = kit::toolbar(ui, tokens, idle, dirty, |ui| {
            ui.separator();
            ui.add_enabled_ui(idle, |ui| {
                // The shared Save as Template: the button, the template's
                // warnings and the Template dialog (P15).
                if flow.slot(ui, &link, group, &template, today) {
                    clicked = Some("form.save_as_template");
                }
                if ui
                    .add_enabled(saved, egui::Button::new(t("journal.void")))
                    .on_hover_text(kit::chip("edit.void_transaction").replace(' ', "+"))
                    .clicked()
                {
                    clicked = Some("edit.void_transaction");
                }
                if ui
                    .add_enabled(saved, egui::Button::new(t("journal.delete")))
                    .on_hover_text(kit::chip("form.delete_item").replace(' ', "+"))
                    .clicked()
                {
                    clicked = Some("form.delete_item");
                }
            });
        });
        if let Some(cmd) = nav.or(clicked) {
            self.after_frame = Some(CommandId::new(cmd));
        }
    }

    fn messages(&mut self, ui: &mut Ui, tokens: &Tokens) {
        match self.closing_prompt.ui(ui, tokens) {
            ClosingClick::Submit => self.submit_closing(),
            ClosingClick::Cancel => self.closing_prompt.cancel(),
            ClosingClick::None => {}
        }
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
        if self.stale {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(t("journal.changed_elsewhere")).color(tokens.warn_text));
                if ui.button(t("journal.reload")).clicked() {
                    self.reload();
                }
            });
        }
        if self.void_open {
            let mut send = false;
            let mut cancel = false;
            form::card(ui, tokens, |ui| {
                form::heading(ui, tokens, &t("journal.void.title"));
                form::hint(ui, tokens, &t("journal.void.explain"));
                let r = form::field(ui, &t("journal.void.reason"), &mut self.void_reason);
                send |= kit::entered(&r);
                ui.horizontal(|ui| {
                    send |= ui.button(t("journal.void.confirm")).clicked();
                    cancel = ui.button(t("journal.void.cancel")).clicked();
                });
            });
            if cancel {
                self.void_open = false;
                self.void_reason.clear();
            } else if send {
                self.send_void(None);
            }
        }
        if self.delete_open {
            let mut send = false;
            let mut cancel = false;
            form::card(ui, tokens, |ui| {
                form::heading(ui, tokens, &t("journal.delete.title"));
                form::hint(ui, tokens, &t("journal.delete.explain"));
                ui.horizontal(|ui| {
                    send = ui.button(t("journal.delete.confirm")).clicked();
                    cancel = ui.button(t("journal.delete.cancel")).clicked();
                });
            });
            if cancel {
                self.delete_open = false;
            } else if send {
                self.send_delete(None);
            }
        }
    }

    fn form_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        kit::title_only(ui, tokens, &t("cmd.company.journal_entry"));
        ui.add_space(6.0);
        let read_only = self.read_only().is_some();
        let enabled = self.idle() && !read_only;
        header::locked_scope(ui, enabled, read_only, |ui| self.header_ui(ui, tokens));
        self.link_ui(ui, tokens);
        ui.add_space(10.0);
        header::locked_scope(ui, enabled, read_only, |ui| {
            let _ = self.lines.ui(ui);
        });
        ui.add_space(6.0);
        self.totals_ui(ui, tokens, enabled);
        let keys: Vec<&'static str> = self.problems.iter().map(|p| p.key().0).collect();
        kit::problems(ui, tokens, &keys);
        if let Some(e) = &self.error {
            form::inline_error(ui, tokens, e);
        }
        if self.busy() {
            form::progress_line(ui, tokens, &t("helper.saving"));
        }
        ui.add_space(12.0);
        self.actions_ui(ui, read_only);
    }

    fn header_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        egui::Frame::new()
            .fill(tokens.surface_form)
            .stroke(Stroke::new(1.0, tokens.accent_border))
            .corner_radius(CornerRadius::same(8))
            .inner_margin(Margin::symmetric(20, 16))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                header::wrap_row(ui, |ui| {
                    kit::wrap_item(ui, "journal/date", |ui| {
                        let today = self.today;
                        let (r, d) = kit::date_field(
                            ui,
                            tokens,
                            &t("journal.date"),
                            &mut self.inputs.date,
                            today,
                            130.0,
                        );
                        if let kit::DateChange::Set(d) = d {
                            self.header.edit(field::DATE, |h| h.date = d);
                            self.inputs.rev = self.header.revision();
                        }
                        kit::track(&mut self.focused, &r, field::DATE);
                        if kit::entered(&r) {
                            self.after_frame = Some(CommandId::new("form.save_new"));
                        }
                    });
                    ui.add_space(12.0);
                    kit::wrap_item(ui, "journal/number", |ui| {
                        let mut text = self.header.current().number.clone();
                        let r = kit::text_field(
                            ui,
                            tokens,
                            &t("journal.number"),
                            &mut text,
                            120.0,
                            cl_model::forms::journal::NUMBER_MAX_CHARS,
                        )
                        .on_hover_text(t("journal.number.tip"));
                        if r.changed() {
                            self.header.edit(field::NUMBER, |h| h.number = text);
                            self.inputs.rev = self.header.revision();
                        }
                        kit::track(&mut self.focused, &r, field::NUMBER);
                    });
                    ui.add_space(12.0);
                    kit::wrap_item(ui, "journal/reverse", |ui| {
                        let today = self.today;
                        let (r, d) = kit::date_field(
                            ui,
                            tokens,
                            &t("journal.reverse_on"),
                            &mut self.inputs.reverse,
                            today,
                            130.0,
                        );
                        let r = r.on_hover_text(format!(
                            "{} {}",
                            t("journal.reverse_on.tip"),
                            t("jargon.reversing")
                        ));
                        if let kit::DateChange::Set(d) = d {
                            self.header.edit(field::REVERSE, |h| h.reverse_on = d);
                            self.inputs.rev = self.header.revision();
                        }
                        kit::track(&mut self.focused, &r, field::REVERSE);
                    });
                    ui.add_space(12.0);
                    kit::wrap_item(ui, "journal/adjusting", |ui| {
                        ui.add_space(20.0);
                        let mut on = self.header.current().adjusting;
                        let r = ui
                            .checkbox(&mut on, t("journal.adjusting"))
                            .on_hover_text(t("jargon.adjusting"));
                        if r.changed() {
                            self.header.edit_step(|h| h.adjusting = on);
                            self.inputs.rev = self.header.revision();
                        }
                        kit::track(&mut self.focused, &r, field::ADJUSTING);
                    });
                });
                ui.add_space(8.0);
                let mut memo = self.header.current().memo.clone();
                let width = ui.available_width().clamp(200.0, 560.0);
                let r = kit::text_field(ui, tokens, &t("journal.memo"), &mut memo, width, 4000);
                if r.changed() {
                    self.header.edit(field::MEMO, |h| h.memo = memo);
                    self.inputs.rev = self.header.revision();
                }
                kit::track(&mut self.focused, &r, field::MEMO);
            });
    }

    /// "Reversed by the entry of DATE" (a link) or "This entry reverses an
    /// earlier entry" (Open it).
    fn link_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let Some(e) = self.editing.clone() else {
            return;
        };
        let mut open = None;
        if let Some((r, d)) = e.reversal {
            ui.add_space(6.0);
            let text = t_with(
                "journal.reversed_by",
                &[("date", &crate::shell::us_date(d))],
            );
            if ui.link(RichText::new(text).color(tokens.accent)).clicked() {
                open = Some(r);
            }
        }
        if let Some(r) = e.reverses {
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(t("journal.reverses")).color(tokens.text_secondary));
                if ui.link(t("journal.open_reversal")).clicked() {
                    open = Some(r);
                }
            });
        }
        if let Some(id) = open {
            self.open_linked(id);
        }
    }

    /// "Debits $X · Credits $Y · Difference $Z" (warn colour when they
    /// differ) and "Balance on this line".
    fn totals_ui(&mut self, ui: &mut Ui, tokens: &Tokens, enabled: bool) {
        let t_ = totals(self.lines.value());
        let style = self.money_style;
        let text = match t_ {
            Some(x) => t_with(
                "journal.totals",
                &[
                    ("debits", &x.debits.format(style)),
                    ("credits", &x.credits.format(style)),
                    ("difference", &x.difference.abs().format(style)),
                ],
            ),
            None => t("journal.problem.too_large"),
        };
        let off = t_.is_none_or(|x| !x.balanced());
        let color = if off {
            tokens.warn_text
        } else {
            tokens.text_secondary
        };
        ui.horizontal_wrapped(|ui| {
            ui.add(egui::Label::new(RichText::new(text).size(13.0).color(color)).selectable(false));
            ui.add_space(12.0);
            if ui
                .add_enabled(enabled && off, egui::Button::new(t("journal.balance_here")))
                .on_hover_text(t("journal.balance_here.tip"))
                .clicked()
            {
                self.balance_here();
            }
        });
    }

    /// Clear · Save & New · Save & Close. The save buttons are off while
    /// the entry can't save (unbalanced, fewer than two lines), with the
    /// reason in their tooltip.
    fn actions_ui(&mut self, ui: &mut Ui, read_only: bool) {
        let idle = self.idle();
        let blocker = save_blocker(&self.compose());
        let why = match (read_only, blocker) {
            (true, _) => self.read_only().map(t),
            (false, Some(JournalProblem::Unbalanced { .. })) => Some(t("journal.save_blocked")),
            (false, Some(p)) => Some(t(p.key().0)),
            (false, None) => None,
        };
        let can_save = idle && why.is_none();
        let mut clicked = None;
        let row = Vec2::new(ui.available_width(), ACTION_HEIGHT);
        ui.allocate_ui_with_layout(row, Layout::right_to_left(Align::Center), |ui| {
            let tip = why.clone().unwrap_or_default();
            for (cmd, label, primary) in [
                ("form.save_close", "helper.save_close", true),
                ("form.save_new", "helper.save_new", false),
            ] {
                let r = ui
                    .add_enabled_ui(can_save, |ui| {
                        crate::widgets::action_button_with_chip(
                            ui,
                            &t(label),
                            &kit::chip(cmd),
                            primary,
                            ACTION_HEIGHT,
                        )
                    })
                    .inner;
                let r = if can_save {
                    r
                } else {
                    r.on_disabled_hover_text(&tip)
                };
                if r.clicked() {
                    clicked = Some(cmd);
                }
            }
            if ui
                .add_enabled(
                    idle,
                    egui::Button::new(t("helper.clear")).min_size(Vec2::new(0.0, ACTION_HEIGHT)),
                )
                .clicked()
            {
                clicked = Some("form.clear");
            }
        });
        if let Some(cmd) = clicked {
            self.after_frame = Some(CommandId::new(cmd));
        }
    }
}
