//! Drawing Write Check (mockup WriteCheck.html): the toolbar, the bank
//! account line, the check card, the Expenses lines, the totals line and
//! the action row; and the keymap commands of the form.
//!
//! Every input carries its visible label as its accessible name. Amounts
//! are shown with `Money::format` in mono, right-aligned; the only figures
//! the form shows besides the owner's typed ones are the engine's ending
//! balance and `lines_total` of the owner's typed line amounts.

use cl_core::Money;
use cl_core::ids::TagId;
use cl_grid::{LookupResult, Lookups};
use cl_model::CommandId;
use cl_model::amount_entry::{self, AmountRules};
use cl_model::forms::SaveIntent;
use cl_model::forms::amount_words::amount_in_words;
use cl_model::forms::check::{NumberField, TotalsStatus};
use cl_model::forms::lines::lines_total;
use eframe::egui::{self, Align, CornerRadius, Layout, Margin, RichText, Stroke, Ui, Vec2};

use super::{Field, WriteCheckScreen, field};
use crate::company::CompanyEvent;
use crate::forms::closing_prompt::ClosingClick;
use crate::forms::header;
use crate::i18n::{t, t_with};
use crate::layout::{Body, FixedFrame, FrameOpts};
use crate::screens::company::form;
use crate::screens::register::account_text;
use crate::screens::register::lists::NAMES;
use crate::theme::Tokens;
use crate::windows::OpenPayload;
use cl_api::PrintStatus;

/// Below this width the side panel goes under the form (README §6).
const SIDE_BY_SIDE: f32 = 1100.0;
/// The side panel's width beside the form.
const PANEL_WIDTH: f32 = 270.0;
/// How many names the payee list shows while typing.
const PAYEE_SUGGESTIONS: usize = 8;
/// The height of Clear / Save & New / Save & Close (DESIGN.md: 44 px for
/// Save buttons).
const ACTION_HEIGHT: f32 = 44.0;
/// The printed-check question's width.
const PRINTED_PROMPT_WIDTH: f32 = 440.0;

/// The shortcut chip text of a command ("Alt N"), from the keymap.
fn chip(id: &str) -> String {
    cl_model::commands::find(id)
        .and_then(|c| c.shortcut)
        .map(crate::input::shortcut_text)
        .unwrap_or_default()
        .replace('+', " ")
}

/// A label above its input; the label is the input's accessible name.
fn above(ui: &mut Ui, tokens: &Tokens, label: &str) -> egui::Response {
    ui.add(
        egui::Label::new(RichText::new(label).size(12.0).color(tokens.text_secondary))
            .selectable(false),
    )
}

impl WriteCheckScreen {
    pub(crate) fn style(&self, _ui: &Ui) -> cl_core::MoneyStyle {
        self.money_style
    }

    /// Draws the whole window.
    pub(crate) fn draw(&mut self, ui: &mut Ui) {
        let tokens = Tokens::of(ui.ctx());
        if !self.accounts_loaded {
            form::progress_line(ui, &tokens, &t("check.loading"));
            return;
        }
        let wide = ui.available_width() >= SIDE_BY_SIDE;
        // The toolbar and its messages stay at the top, the actions at the
        // bottom; the check, its lines and the side panel scroll (Q-265).
        let mut frame = FixedFrame::begin(ui, "write_check", FrameOpts::default());
        frame.top(|ui| {
            self.toolbar(ui, &tokens);
            self.messages(ui, &tokens);
        });
        if self.bank_accounts().is_empty() {
            frame.body(Body::Scroll, |ui| {
                ui.add_space(8.0);
                ui.label(RichText::new(t("check.no_bank")).color(tokens.text_secondary));
                if ui.button(t("check.open_chart")).clicked() {
                    self.link.push(CompanyEvent::OpenWindow(CommandId::new(
                        crate::screens::lists::accounts::KIND,
                    )));
                }
            });
            self.layout = frame.end();
            return;
        }
        // The gap, and the spacing egui puts between the three items, so
        // the panel's right edge stays on screen.
        let form_width = |ui: &Ui| {
            let spacing = 2.0 * ui.spacing().item_spacing.x;
            ui.available_width() - PANEL_WIDTH - 20.0 - spacing
        };
        frame.body(Body::Scroll, |ui| {
            if wide {
                ui.horizontal_top(|ui| {
                    let form_width = form_width(ui);
                    ui.vertical(|ui| {
                        ui.set_width(form_width);
                        self.form_ui(ui, &tokens);
                    });
                    ui.add_space(20.0);
                    ui.vertical(|ui| {
                        ui.set_width(PANEL_WIDTH);
                        self.panel_ui(ui, &tokens);
                    });
                });
            } else {
                self.form_ui(ui, &tokens);
                ui.add_space(12.0);
                self.panel_ui(ui, &tokens);
            }
        });
        frame.bottom(|ui| {
            // Under the check's column, as in the mockup.
            if wide {
                let w = form_width(ui);
                ui.allocate_ui(Vec2::new(w, ACTION_HEIGHT), |ui| self.actions_ui(ui));
            } else {
                self.actions_ui(ui);
            }
        });
        self.layout = frame.end();
    }

    fn toolbar(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let saved = self.editing.is_some();
        let working = self.save.task.is_some();
        let dirty = self.is_dirty();
        header::toolbar(ui, tokens, dirty, &t("check.unsaved"), |ui| {
            if let Some(nav) = header::nav_buttons(
                ui,
                !working,
                (&t("check.prev"), &t("check.prev.tip")),
                (&t("check.next"), &t("check.next.tip")),
            ) {
                self.navigate(nav.older());
            }
            ui.separator();
            if ui.button(t("check.find")).clicked() {
                self.find();
            }
            // P19 (Q-P19s): print this check now, preview first; a new or
            // changed check is saved as Print later on the way.
            if ui
                .add_enabled(self.can_print_now(), egui::Button::new(t("check.print")))
                .on_hover_text(t("print.check.print_now.tip"))
                .on_disabled_hover_text(t("check.print.unavailable"))
                .clicked()
            {
                self.print_now();
            }
            if ui
                .add_enabled(!working, egui::Button::new(t("check.attach")))
                .on_hover_text(t("check.attach.tip"))
                .clicked()
            {
                self.attach.focus = true;
            }
            if ui
                .add_enabled(saved, egui::Button::new(t("check.history")))
                .on_disabled_hover_text(t("check.history.saved_only"))
                .clicked()
            {
                self.toggle_history();
            }
            self.template_slot(ui);
            ui.separator();
            let mut later = self.header.current().print_later;
            // A04 draft mode: a check already written is never printed later.
            let can_print_later =
                !working && (self.editing.is_none() || self.check_edit()) && !self.draft.on();
            let r = ui
                .add_enabled(
                    can_print_later,
                    egui::Checkbox::new(&mut later, t("check.print_later")),
                )
                .on_hover_text(t("check.print_later.tip"));
            if r.changed() {
                self.set_print_later(later);
            }
            // P19 (Q-P19ab): reprint a printed check with the next number,
            // preview first.
            if let Some(e) = self
                .editing
                .as_ref()
                .filter(|e| !e.read_only && e.print_status == PrintStatus::Printed)
            {
                let clean = !self.header.is_dirty();
                if ui
                    .add_enabled(
                        clean && !working,
                        egui::Button::new(t("print.check.reprint")),
                    )
                    .on_hover_text(t("print.check.reprint.tip"))
                    .clicked()
                {
                    let p = OpenPayload::ReprintCheck(e.id, e.old_date);
                    self.open_print_forms(p);
                }
            }
        });
    }

    fn open_print_forms(&self, p: OpenPayload) {
        self.link.push(crate::company::CompanyEvent::OpenWith(
            CommandId::new(crate::screens::print_forms::KIND),
            Box::new(p),
        ));
    }

    /// Print can run: a saved check waiting to print, or a new or changed
    /// check that can be saved as Print later (never a blank one, a
    /// printed one — that is Reprint — or one in draft mode).
    pub(crate) fn can_print_now(&self) -> bool {
        if self.save.task.is_some() || self.draft.on() {
            return false;
        }
        let dirty = self.header.is_dirty();
        match self.editing.as_ref() {
            Some(e) if e.read_only || e.print_status == PrintStatus::Printed => false,
            Some(e) if e.print_status == PrintStatus::Queue && !dirty => true,
            Some(_) => self.check_edit(),
            None => dirty,
        }
    }

    /// Print (Q-P19s): a saved check waiting to print opens on its preview
    /// in File > Print Forms; otherwise the check is saved as Print later
    /// first (a new one with Save & New, an edited one staying in front)
    /// and the preview opens once the engine took it. The number is the
    /// bank account's next check number, set in the preview.
    pub(crate) fn print_now(&mut self) -> bool {
        if !self.can_print_now() {
            return false;
        }
        if let Some(e) = self
            .editing
            .as_ref()
            .filter(|e| e.print_status == PrintStatus::Queue && !self.header.is_dirty())
        {
            let p = OpenPayload::PrintChecks(vec![e.id]);
            self.open_print_forms(p);
            return true;
        }
        if !self.header.current().print_later {
            self.set_print_later(true);
        }
        let intent = if self.editing.is_some() {
            SaveIntent::Save
        } else {
            SaveIntent::SaveAndNew
        };
        self.print_after_save = true;
        let started = self.start_save(intent, None);
        if !started && self.save.task.is_none() {
            self.print_after_save = false;
        }
        started
    }

    pub(crate) fn set_print_later(&mut self, on: bool) {
        self.header.edit_step(|h| h.set_print_later(on));
        self.sync_inputs();
    }

    fn messages(&mut self, ui: &mut Ui, tokens: &Tokens) {
        match self.closing_prompt.ui(ui, tokens) {
            ClosingClick::Submit => self.submit_closing(),
            ClosingClick::Cancel => self.closing_prompt.cancel(),
            ClosingClick::None => {}
        }
        // A04 draft mode: where the values came from, Accept, Discard.
        if self.draft.on() {
            self.draft_banner(ui, tokens);
        }
        if let Some(key) = self.save.notice {
            form::hint(ui, tokens, &t(key));
        }
        if self.void.open {
            self.void_ui(ui, tokens);
        }
        if self.history.open {
            self.history_ui(ui, tokens);
        }
        if let Some(key) = self.suggest.hint {
            form::hint(ui, tokens, &t(key));
        }
        if self.prefill.dropped {
            form::hint(ui, tokens, &t("check.prefill.dropped"));
        }
        self.nav_prompt_ui(ui);
        self.printed_prompt_ui(ui);
    }

    /// Prev/Next on unsaved changes: the standard question.
    fn nav_prompt_ui(&mut self, ui: &mut Ui) {
        if self.nav_prompt.is_none() {
            return;
        }
        let mut answer = None;
        let title = self.title_text();
        egui::Window::new(t("window.prompt.title"))
            .id(egui::Id::unique("write_check/nav_prompt"))
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .collapsible(false)
            .resizable(false)
            .show(ui.ctx(), |ui| {
                ui.label(crate::windows::prompt_message(&title));
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if crate::widgets::primary_button(ui, &t("window.prompt.save")).clicked() {
                        answer = Some(crate::windows::PromptAnswer::Save);
                    }
                    if ui.button(t("window.prompt.discard")).clicked() {
                        answer = Some(crate::windows::PromptAnswer::Discard);
                    }
                    if ui.button(t("window.prompt.keep")).clicked() {
                        answer = Some(crate::windows::PromptAnswer::Keep);
                    }
                });
            });
        if let Some(a) = answer {
            self.answer_nav(a);
        }
    }

    /// A printed check changed: Reprint / Keep number / Cancel.
    fn printed_prompt_ui(&mut self, ui: &mut Ui) {
        let Some(p) = self.printed else { return };
        let mut answer: Option<Option<cl_api::PrintedCheckChange>> = None;
        let text = match p.number {
            Some(n) => t_with("check.printed.question", &[("number", &n.to_string())]),
            None => t("check.printed.question_no_number"),
        };
        egui::Window::new(t("check.printed.title"))
            .id(egui::Id::unique("write_check/printed_prompt"))
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .collapsible(false)
            .resizable(false)
            // Wide enough for the whole title (egui cuts a title that is
            // wider than the window's content).
            .min_width(PRINTED_PROMPT_WIDTH)
            .show(ui.ctx(), |ui| {
                ui.add(egui::Label::new(text).wrap());
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if crate::widgets::primary_button(ui, &t("check.printed.reprint")).clicked() {
                        answer = Some(Some(cl_api::PrintedCheckChange::Reprint));
                    }
                    if ui.button(t("check.printed.keep")).clicked() {
                        answer = Some(Some(cl_api::PrintedCheckChange::KeepNumber));
                    }
                    if ui.button(t("check.printed.cancel")).clicked() {
                        answer = Some(None);
                    }
                });
            });
        if let Some(a) = answer {
            self.answer_printed(a);
        }
    }

    fn void_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let mut send = false;
        let mut cancel = false;
        form::card(ui, tokens, |ui| {
            form::heading(ui, tokens, &t("check.void.title"));
            form::hint(ui, tokens, &t("check.void.explain"));
            let r = form::field(ui, &t("check.void.reason"), &mut self.void.reason);
            send |= crate::forms::enter::submitted(&r);
            ui.horizontal(|ui| {
                send |= ui.button(t("check.void.confirm")).clicked();
                cancel = ui.button(t("check.void.cancel")).clicked();
            });
        });
        if cancel {
            self.void = super::VoidPrompt::default();
        } else if send {
            let reason = self.void.reason.clone();
            self.send_void(&reason, None);
        }
    }

    fn history_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        form::card(ui, tokens, |ui| {
            form::heading(ui, tokens, &t("check.history.title"));
            if self.history.task.is_some() {
                form::progress_line(ui, tokens, &t("check.loading"));
            } else if self.history.error {
                form::hint(ui, tokens, &t("check.history.failed"));
            } else if self.history.rows.is_empty() {
                form::hint(ui, tokens, &t("check.history.empty"));
            }
            for row in &self.history.rows {
                ui.label(row);
            }
            if ui.button(t("check.history.close")).clicked() {
                self.history = super::HistoryView::default();
            }
        });
    }

    fn form_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let read_only = self.editing.as_ref().is_some_and(|e| e.read_only);
        let enabled = self.save.task.is_none()
            && !self.closing_prompt.dialog_open()
            && !read_only
            && !self.draft_locked();
        // A read-only check keeps readable text (contrast >= 4.5:1).
        header::locked_scope(ui, true, read_only, |ui| self.bank_row(ui, tokens, enabled));
        ui.add_space(10.0);
        if let Some(e) = self.editing.as_ref().filter(|e| e.read_only) {
            form::hint(ui, tokens, &t(e.read_only_note));
        } else if self.editing_locked() {
            form::hint(ui, tokens, &t("closing.edit_note"));
        }
        header::locked_scope(ui, enabled, read_only, |ui| self.card_ui(ui, tokens));
        ui.add_space(12.0);
        header::locked_scope(ui, true, read_only, |ui| self.lines_ui(ui, tokens, enabled));
        ui.add_space(6.0);
        self.totals_ui(ui, tokens);
        self.problems_ui(ui, tokens);
    }

    fn bank_row(&mut self, ui: &mut Ui, tokens: &Tokens, enabled: bool) {
        let banks: Vec<(cl_core::ids::AccountId, String)> = self
            .bank_accounts()
            .into_iter()
            .map(|a| (a.id, account_text(a)))
            .collect();
        let current = self.bank();
        let shown = banks
            .iter()
            .find(|(id, _)| Some(*id) == current)
            .map(|(_, s)| s.clone())
            .unwrap_or_default();
        let mut picked = None;
        let style = self.style(ui);
        let title = t("cmd.vendors.write_check");
        header::title_row(ui, tokens, "write_check", &title, |ui| {
            header::wrap_item(ui, "write_check/bank", |ui| {
                let l = above(ui, tokens, &t("check.bank"));
                ui.add_enabled_ui(enabled && self.editing.is_none(), |ui| {
                    let r = egui::ComboBox::from_id_salt("write_check/bank")
                        .width(300.0)
                        .selected_text(shown)
                        .show_ui(ui, |ui| {
                            for (id, text) in &banks {
                                if ui.selectable_label(Some(*id) == current, text).clicked() {
                                    picked = Some(*id);
                                }
                            }
                        })
                        .response
                        .labelled_by(l.id);
                    // Only a Bank request is taken here; a request for a
                    // later field stays for that field.
                    if self.focus == Some(Field::Bank) {
                        self.focus = None;
                        r.request_focus();
                    }
                });
            });
            header::wrap_item(ui, "write_check/balance", |ui| {
                ui.add_space(20.0);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(t("check.ending_balance"))
                            .size(12.0)
                            .color(tokens.text_secondary),
                    );
                    // The engine's balance as sent; never derived here.
                    let text = self.balance.map(|m| m.format(style)).unwrap_or_default();
                    ui.label(
                        RichText::new(text)
                            .text_style(egui::TextStyle::Monospace)
                            .color(tokens.text_primary),
                    );
                });
            });
        });
        if let Some(id) = picked {
            self.set_bank(ui.ctx(), id);
        }
    }

    /// The check card: company, No., Date, Pay to the order of, Amount, the
    /// words, Address, Memo and the header tags.
    fn card_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        egui::Frame::new()
            .fill(tokens.surface_form)
            .stroke(Stroke::new(1.0, tokens.accent_border))
            .corner_radius(CornerRadius::same(8))
            .inner_margin(Margin::symmetric(20, 16))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                let width = ui.available_width();
                // egui puts `item_spacing.x` between the columns of each
                // row as well as the gap: both come off the widths, so the
                // card fits its column down to the 960 x 640 window.
                let sp = ui.spacing().item_spacing.x;
                let right = (width * 0.34).clamp(200.0, 300.0);
                let gap = 16.0;
                let left = (width - right - gap - sp).max(160.0);
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(left);
                        ui.label(
                            RichText::new(&self.company.name)
                                .strong()
                                .color(tokens.text_primary),
                        );
                        ui.label(
                            RichText::new(&self.company.address)
                                .size(12.0)
                                .color(tokens.text_secondary),
                        );
                    });
                    ui.add_space(gap);
                    ui.horizontal_top(|ui| {
                        let half = (right - sp) / 2.0;
                        ui.vertical(|ui| {
                            ui.set_width(half);
                            self.number_ui(ui, tokens, half);
                        });
                        ui.vertical(|ui| {
                            ui.set_width(half);
                            self.date_ui(ui, tokens, half);
                        });
                    });
                });
                ui.add_space(8.0);
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(left);
                        self.payee_ui(ui, tokens, left);
                    });
                    ui.add_space(gap);
                    ui.vertical(|ui| {
                        ui.set_width(right);
                        self.amount_ui(ui, tokens, right);
                    });
                });
                ui.add_space(8.0);
                self.words_ui(ui, tokens);
                ui.add_space(8.0);
                let half = (width - gap - sp) / 2.0;
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(half);
                        self.address_ui(ui, tokens, half);
                    });
                    ui.add_space(gap);
                    ui.vertical(|ui| {
                        ui.set_width(half);
                        self.memo_ui(ui, tokens, half);
                        ui.add_space(6.0);
                        self.tags_ui(ui, tokens);
                    });
                });
            });
    }

    fn take_focus(&mut self, f: Field, r: &egui::Response) {
        if self.focus == Some(f) {
            self.focus = None;
            r.request_focus();
        }
        if r.has_focus() {
            self.focused = Some(f);
        } else if self.focused == Some(f) && !r.has_focus() {
            self.focused = None;
        }
    }

    fn number_ui(&mut self, ui: &mut Ui, tokens: &Tokens, width: f32) {
        let l = above(ui, tokens, &t("check.number"));
        let to_print = self.header.current().print_later;
        let locked_number = self.editing.is_some() && !self.check_edit();
        if to_print {
            let mut text = t("check.number.to_print");
            let r = ui
                .add_enabled(
                    false,
                    egui::TextEdit::singleline(&mut text)
                        .desired_width(width - 8.0)
                        .font(egui::TextStyle::Monospace),
                )
                .labelled_by(l.id);
            r.on_disabled_hover_text(t("check.number.to_print.tip"));
            return;
        }
        let r = ui
            .add_enabled(
                !locked_number,
                egui::TextEdit::singleline(&mut self.inputs.number)
                    .desired_width(width - 8.0)
                    .char_limit(10)
                    .font(egui::TextStyle::Monospace),
            )
            .labelled_by(l.id);
        let r = r.on_disabled_hover_text(t("check.number.locked"));
        self.take_focus(Field::Number, &r);
        if r.changed() {
            let text = self.inputs.number.clone();
            self.header.edit(field::NUMBER, |h| {
                let same =
                    matches!(&h.number, NumberField::Proposed(n) if n.to_string() == text.trim());
                if !same {
                    h.number = NumberField::Typed(text);
                }
            });
            self.inputs.rev = self.header.revision();
            self.save.duplicate = None;
        }
        if crate::forms::enter::entered(&r).saves() {
            self.after_frame = Some(CommandId::new("form.save_new"));
        }
    }

    fn date_ui(&mut self, ui: &mut Ui, tokens: &Tokens, width: f32) {
        let l = above(ui, tokens, &t("check.date"));
        let before = self.inputs.date.text.clone();
        let r = ui
            .add(
                egui::TextEdit::singleline(&mut self.inputs.date.text)
                    .desired_width(width - 8.0)
                    .font(egui::TextStyle::Monospace),
            )
            .labelled_by(l.id);
        self.take_focus(Field::Date, &r);
        if r.changed() {
            self.inputs.date.typed(&before, self.today);
            let value = self.inputs.date.value;
            if value.is_some() {
                self.header.edit(field::DATE, |h| h.date = value);
                self.inputs.rev = self.header.revision();
            }
        }
        if r.lost_focus() {
            self.inputs.date.settle();
            if self.inputs.date.value.is_none() {
                self.header.edit(field::DATE, |h| h.date = None);
                self.inputs.rev = self.header.revision();
            }
        }
        if crate::forms::enter::entered(&r).saves() {
            self.after_frame = Some(CommandId::new("form.save_new"));
        }
    }

    fn payee_ui(&mut self, ui: &mut Ui, tokens: &Tokens, width: f32) {
        let l = above(ui, tokens, &t("check.payee"));
        let r = ui
            .add(
                egui::TextEdit::singleline(&mut self.inputs.payee)
                    .desired_width(width - 8.0)
                    .margin(crate::list_field::margin())
                    .font(egui::FontId::proportional(15.0)),
            )
            .labelled_by(l.id);
        self.take_focus(Field::Payee, &r);
        if r.changed() {
            self.payee_list = !self.inputs.payee.trim().is_empty();
            self.inputs.payee_problem = false;
        }
        // Q-250: a click opens the names list; the chevron toggles it.
        if let Some(open) = crate::list_field::mouse(ui, &r, self.payee_list) {
            self.payee_list = open;
        }
        let mut picked = None;
        if self.payee_list && (r.has_focus() || r.lost_focus()) {
            picked = self.payee_popup(ui, &r);
        }
        if let Some(id) = picked {
            self.pick_payee(id);
        } else if r.lost_focus() {
            self.resolve_payee();
            if crate::forms::enter::entered(&r).saves() {
                self.after_frame = Some(CommandId::new("form.save_new"));
            }
        }
        if self.inputs.payee_problem {
            ui.label(
                RichText::new(t("check.payee.pick"))
                    .size(12.0)
                    .color(tokens.warn_text),
            );
        }
    }

    /// The names that start with (or contain) the typed text, under the
    /// field (every name when a click opened it, Q-250); a click picks one.
    fn payee_popup(&mut self, ui: &mut Ui, r: &egui::Response) -> Option<cl_core::ids::ContactId> {
        let query = crate::list_field::query(ui.ctx(), r.id, &self.inputs.payee);
        let limit = if query.is_empty() {
            200
        } else {
            PAYEE_SUGGESTIONS
        };
        let LookupResult::Ready(found) = self.lists.suggest(NAMES, &query, limit) else {
            return None;
        };
        let current = self
            .header
            .current()
            .payee
            .and_then(|p| found.iter().position(|s| self.lists.name(s.id) == Some(p)));
        let i = crate::list_field::popup(ui, r, found.iter().map(|s| s.label.as_str()), current)?;
        found.get(i).and_then(|s| self.lists.name(s.id))
    }

    /// Autofill when the owner leaves the payee: the exact name, else the
    /// first name that starts with the typed text. No free-text payees
    /// (Q-P13o): anything else asks for a name from the list.
    pub(crate) fn resolve_payee(&mut self) {
        self.payee_list = false;
        let typed = self.inputs.payee.trim().to_string();
        if typed.is_empty() {
            self.header.edit(field::PAYEE, |h| {
                h.payee = None;
                h.payee_name.clear();
            });
            self.inputs.rev = self.header.revision();
            return;
        }
        let found = self.lists.exact(NAMES, &typed).or_else(|| {
            match self.lists.suggest(NAMES, &typed, 1) {
                LookupResult::Ready(s) => s
                    .into_iter()
                    .next()
                    .filter(|s| s.label.to_lowercase().starts_with(&typed.to_lowercase())),
                LookupResult::Loading => None,
            }
        });
        if let Some(id) = found.and_then(|s| self.lists.name(s.id)) {
            self.pick_payee(id);
        } else {
            self.inputs.payee_problem = true;
            self.header.edit(field::PAYEE, |h| {
                h.payee = None;
                h.payee_name.clear();
            });
            self.inputs.rev = self.header.revision();
        }
    }

    /// A name was picked for Pay to the order of: the address, prefill
    /// (Q-191/Q-192) and the payee panel follow.
    pub(crate) fn pick_payee(&mut self, id: cl_core::ids::ContactId) {
        self.payee_list = false;
        self.inputs.payee_problem = false;
        let name = self.lists.name_text(id).unwrap_or_default().to_string();
        self.inputs.payee.clone_from(&name);
        let same = self.header.current().payee == Some(id);
        self.header.edit(field::PAYEE, |h| {
            h.payee = Some(id);
            h.payee_name = name;
        });
        if !same {
            self.payee_picked(id);
        }
        self.inputs.rev = self.header.revision();
    }

    fn amount_ui(&mut self, ui: &mut Ui, tokens: &Tokens, width: f32) {
        let l = above(ui, tokens, &t("check.amount"));
        let r = ui
            .add(
                egui::TextEdit::singleline(&mut self.inputs.amount)
                    .desired_width(width - 8.0)
                    .font(egui::FontId::monospace(15.0))
                    .align(egui::Align2::RIGHT_CENTER),
            )
            .labelled_by(l.id);
        self.take_focus(Field::Amount, &r);
        if r.changed() {
            self.inputs.amount_problem = None;
        }
        if r.lost_focus() {
            self.amount_left();
            if crate::forms::enter::entered(&r).saves() && self.inputs.amount_problem.is_none() {
                self.after_frame = Some(CommandId::new("form.save_new"));
            }
        }
        if let Some(key) = self.inputs.amount_problem {
            ui.label(RichText::new(t(key)).size(12.0).color(tokens.warn_text));
        }
    }

    /// The owner left the amount: the text (a plain amount or a
    /// calculation, rounded to the cent, Q-232) becomes the check amount.
    pub(crate) fn amount_left(&mut self) {
        let text = self.inputs.amount.trim().to_string();
        if text.is_empty() {
            if self.header.current().amount.is_some() {
                self.amount_typed(None);
            }
            return;
        }
        match amount_entry::parse(&text, AmountRules::MONEY) {
            Ok(p) => {
                let Some(m) = p.value.as_money() else { return };
                if self.header.current().amount != Some(m)
                    || self.header.current().amount_from_lines
                {
                    self.amount_typed(Some(m));
                }
                self.inputs.amount = m.format(self.money_style);
                self.inputs.amount_problem = None;
            }
            Err(e) => self.inputs.amount_problem = Some(e.key().0),
        }
    }

    fn words_ui(&self, ui: &mut Ui, tokens: &Tokens) {
        let words = self.header.current().amount.and_then(amount_in_words);
        let rect = ui
            .horizontal(|ui| {
                ui.set_min_height(20.0);
                if let Some(w) = words {
                    ui.label(RichText::new(w).color(tokens.text_primary));
                    ui.label(RichText::new(t("check.words.dollars")).color(tokens.text_secondary));
                }
            })
            .response
            .rect;
        ui.painter().hline(
            rect.x_range(),
            rect.bottom() + 3.0,
            Stroke::new(1.0, tokens.accent_border),
        );
    }

    fn address_ui(&mut self, ui: &mut Ui, tokens: &Tokens, width: f32) {
        let l = above(ui, tokens, &t("check.address"));
        let locked = self.editing.is_some() && !self.check_edit();
        let mut text = self.header.current().address.clone();
        // Enter adds a line here (Q-P13p).
        let r = ui
            .add_enabled(
                !locked,
                egui::TextEdit::multiline(&mut text)
                    .desired_rows(3)
                    .desired_width(width - 8.0)
                    .char_limit(cl_model::forms::check::ADDRESS_MAX),
            )
            .labelled_by(l.id)
            .on_disabled_hover_text(t("check.number.locked"));
        self.take_focus(Field::Address, &r);
        if r.changed() {
            self.header.edit(field::ADDRESS, |h| h.address = text);
            self.inputs.rev = self.header.revision();
        }
    }

    fn memo_ui(&mut self, ui: &mut Ui, tokens: &Tokens, width: f32) {
        let l = above(ui, tokens, &t("check.memo"));
        let mut text = self.header.current().memo.clone();
        let r = ui
            .add(
                egui::TextEdit::singleline(&mut text)
                    .desired_width(width - 8.0)
                    .char_limit(cl_api::PRINTED_MEMO_MAX),
            )
            .labelled_by(l.id);
        self.take_focus(Field::Memo, &r);
        if r.changed() {
            self.header.edit(field::MEMO, |h| h.memo = text);
            self.inputs.rev = self.header.revision();
        }
        if crate::forms::enter::entered(&r).saves() {
            self.after_frame = Some(CommandId::new("form.save_new"));
        }
    }

    /// Header tags (Q-178): they default onto lines without their own tag.
    /// One tag per group: a pick replaces the group's other tag.
    fn tags_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let header = self.header.current().header_tags.clone();
        let mut remove = None;
        let mut add = None;
        ui.horizontal_wrapped(|ui| {
            let l = ui.add(
                egui::Label::new(
                    RichText::new(t("check.tags"))
                        .size(12.0)
                        .color(tokens.text_secondary),
                )
                .selectable(false),
            );
            for id in &header {
                let name = crate::tag_names::label_of(&self.tags, *id);
                let r = ui
                    .add(
                        egui::Button::new(
                            RichText::new(t_with("check.tags.remove", &[("tag", &name)]))
                                .size(12.0),
                        )
                        .fill(tokens.neutral_chip),
                    )
                    .on_hover_text(t("check.tags.remove.tip"));
                if r.clicked() {
                    remove = Some(*id);
                }
            }
            egui::ComboBox::from_id_salt("write_check/tags")
                .selected_text(t("check.tags.add"))
                .show_ui(ui, |ui| {
                    for g in crate::tag_names::sorted(
                        self.tags
                            .iter()
                            .filter(|g| g.is_active && !header.contains(&g.id)),
                    ) {
                        if ui
                            .selectable_label(false, crate::tag_names::label(g))
                            .clicked()
                        {
                            add = Some(g.id);
                        }
                    }
                })
                .response
                .labelled_by(l.id);
        });
        if let Some(id) = remove {
            self.header
                .edit_step(|h| h.header_tags.retain(|g| *g != id));
        }
        if let Some(id) = add {
            let group = self
                .tags
                .iter()
                .find(|g| g.id == id)
                .and_then(|g| g.fields.group_id);
            let same_group: Vec<TagId> = self
                .tags
                .iter()
                .filter(|g| group.is_some() && g.fields.group_id == group)
                .map(|g| g.id)
                .collect();
            self.header.edit_step(|h| {
                h.header_tags.retain(|g| !same_group.contains(g));
                h.header_tags.push(id);
            });
        }
    }

    fn lines_ui(&mut self, ui: &mut Ui, tokens: &Tokens, enabled: bool) {
        let style = self.style(ui);
        let total = lines_total(self.lines.value()).unwrap_or(Money::ZERO);
        // One section, Expenses, drawn as the selected tab of the mockup
        // (Q-247: no Items tab before P20). A label, not a tab: the window
        // tabs are the only tabs.
        ui.horizontal(|ui| {
            let job = with_figure(
                ui,
                "check.expenses",
                "total",
                &total.format(style),
                14.0,
                tokens.text_primary,
            );
            let r = ui.add(egui::Label::new(job).selectable(false));
            ui.painter().hline(
                r.rect.x_range(),
                r.rect.bottom() + 2.0,
                Stroke::new(2.0, tokens.accent),
            );
        });
        ui.add_space(4.0);
        ui.add_enabled_ui(enabled, |ui| {
            let _ = self.lines.ui(ui);
        });
        ui.add_space(4.0);
        self.suggestions_ui(ui, tokens);
        if !self.line_billable() {
            form::hint(ui, tokens, &t("check.lines.billable_later"));
        }
    }

    fn totals_ui(&self, ui: &mut Ui, tokens: &Tokens) {
        let style = self.style(ui);
        let form = self.compose();
        let total = lines_total(&form.lines).unwrap_or(Money::ZERO);
        let (key, var, figure, warn) = match form.totals_status() {
            Some(TotalsStatus::Balanced) => ("check.totals.balanced", "total", total, false),
            Some(TotalsStatus::Remaining(m)) => ("check.totals.remaining", "amount", m, true),
            Some(TotalsStatus::Over(m)) => ("check.totals.over", "amount", m, true),
            None => ("check.totals.lines", "total", total, false),
        };
        ui.horizontal_wrapped(|ui| {
            let color = if warn {
                tokens.warn_text
            } else {
                tokens.text_secondary
            };
            let job = with_figure(ui, key, var, &figure.format(style), 12.0, color);
            crate::widgets::label_with_check(ui, job, false);
            ui.label(
                RichText::new(t("check.totals.ai_note"))
                    .size(12.0)
                    .color(tokens.text_secondary),
            );
        });
    }

    fn problems_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        for p in &self.save.problems {
            ui.add(egui::Label::new(RichText::new(t(p.key().0)).color(tokens.warn_text)).wrap());
        }
        if let Some((refused, offer)) = self.save.duplicate {
            let mut take = false;
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(t_with(
                        "check.number.used",
                        &[("number", &refused.to_string())],
                    ))
                    .color(tokens.warn_text),
                );
                take = ui
                    .button(t_with(
                        "check.number.use",
                        &[("number", &offer.to_string())],
                    ))
                    .clicked();
            });
            if take {
                self.use_offered_number();
            }
        }
        if self.save.stale {
            let mut reload = false;
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(t("check.changed_elsewhere")).color(tokens.warn_text));
                reload = ui.button(t("check.reload")).clicked();
            });
            if reload {
                self.reload();
            }
        }
        if let Some(e) = &self.save.error {
            form::inline_error(ui, tokens, e);
        }
        if let Some(ev) = self.save.lost.ui(
            ui,
            &mut self.header,
            &self.link,
            self.group.as_ref(),
            "write_check",
        ) {
            self.lost_event(ev);
        }
        if self.save.task.is_some() {
            form::progress_line(ui, tokens, &t("check.saving"));
        }
    }

    fn actions_ui(&mut self, ui: &mut Ui) {
        let idle = self.save.task.is_none() && !self.closing_prompt.dialog_open();
        let read_only = self.editing.as_ref().is_some_and(|e| e.read_only);
        let mut clicked = None;
        // One row of fixed height: the shortcut chips sit inside the
        // buttons (mockup), so nothing in the row stretches.
        let row = Vec2::new(ui.available_width(), ACTION_HEIGHT);
        ui.allocate_ui_with_layout(row, Layout::right_to_left(Align::Center), |ui| {
            let can_save = idle && !read_only;
            let close = ui
                .add_enabled_ui(can_save, |ui| {
                    crate::widgets::action_button_with_chip(
                        ui,
                        &t("check.save_close"),
                        &chip("form.save_close"),
                        true,
                        ACTION_HEIGHT,
                    )
                })
                .inner;
            if close.clicked() {
                clicked = Some("form.save_close");
            }
            let new = ui
                .add_enabled_ui(can_save, |ui| {
                    crate::widgets::action_button_with_chip(
                        ui,
                        &t("check.save_new"),
                        &chip("form.save_new"),
                        false,
                        ACTION_HEIGHT,
                    )
                })
                .inner;
            if new.clicked() {
                clicked = Some("form.save_new");
            }
            if ui
                .add_enabled(
                    idle,
                    egui::Button::new(t("check.clear")).min_size(Vec2::new(0.0, ACTION_HEIGHT)),
                )
                .clicked()
            {
                self.clear();
            }
        });
        if let Some(cmd) = clicked {
            self.form_command(CommandId::new(cmd));
        }
    }

    /// Find (Ctrl+F): this bank account's register, where Go To finds a
    /// check (Q-P13v).
    pub(crate) fn find(&mut self) {
        if let Some(bank) = self.bank() {
            self.link.push(CompanyEvent::OpenRegister(bank));
        }
    }

    /// Keymap commands the grid and the closing dialog did not take.
    pub(crate) fn form_command(&mut self, id: CommandId) -> bool {
        match id.as_str() {
            // Ctrl+W in the window: stays.
            super::KIND => true,
            "form.save_new" => {
                self.start_save(SaveIntent::SaveAndNew, None);
                true
            }
            "form.save_close" => {
                self.start_save(SaveIntent::SaveAndClose, None);
                true
            }
            "form.prev_record" => {
                self.navigate(true);
                true
            }
            "form.next_record" => {
                self.navigate(false);
                true
            }
            "form.clear" => {
                self.clear();
                true
            }
            "edit.find" => {
                self.find();
                true
            }
            "file.print" => {
                if !self.print_now() && !self.print_after_save {
                    self.save.notice = Some("check.print.unavailable");
                }
                true
            }
            "form.history" => {
                self.toggle_history();
                true
            }
            "form.save_as_template" => {
                self.save_as_template();
                true
            }
            "form.attach" => {
                self.attach.focus = true;
                true
            }
            "check.print_later" => {
                let on = !self.header.current().print_later;
                if self.editing.is_none() || self.check_edit() {
                    self.set_print_later(on);
                }
                true
            }
            "edit.void_transaction" => self.start_void(),
            // `--demo` only (the command exists only there): the saved
            // check in front was printed.
            "dev.check_mark_printed" => self.demo_mark_printed(),
            "edit.open_list" if self.focused == Some(Field::Payee) => {
                self.payee_list = true;
                true
            }
            "edit.undo" => {
                let done = self.header.undo();
                if done {
                    self.sync_inputs();
                }
                done
            }
            "edit.redo" => {
                let done = self.header.redo();
                if done {
                    self.sync_inputs();
                }
                done
            }
            "form.revert" => self.revert_field(),
            _ => false,
        }
    }

    /// The demo check being edited becomes a printed one (no printing
    /// before P19; the scenario reaches the printed-check question so).
    fn demo_mark_printed(&mut self) -> bool {
        let Some(e) = self.editing.as_mut() else {
            return false;
        };
        let Some(mut demo) = self.link.demo_state() else {
            return false;
        };
        let done = crate::company::demo::checks::mark_printed(&mut demo, e.id);
        if done {
            e.print_status = cl_api::PrintStatus::Printed;
        }
        done
    }

    /// Esc: the payee list closes, then the focused field goes back to
    /// its saved value; with nothing to revert the shell closes the window
    /// (asking first when the form is dirty).
    fn revert_field(&mut self) -> bool {
        if self.nav_prompt.is_some() {
            self.answer_nav(crate::windows::PromptAnswer::Keep);
            return true;
        }
        if self.printed.is_some() {
            self.answer_printed(None);
            return true;
        }
        if self.void.open {
            self.void = super::VoidPrompt::default();
            return true;
        }
        if self.payee_list {
            self.payee_list = false;
            return true;
        }
        let Some(f) = self.focused else { return false };
        let saved = self.header.original().clone();
        let now = self.header.current();
        let differs = match f {
            Field::Bank => now.bank != saved.bank,
            Field::Number => now.number != saved.number,
            Field::Date => now.date != saved.date,
            Field::Payee => now.payee != saved.payee,
            Field::Amount => now.amount != saved.amount,
            Field::Address => now.address != saved.address,
            Field::Memo => now.memo != saved.memo,
        };
        if !differs {
            return false;
        }
        self.header.edit_step(|h| match f {
            Field::Bank => h.bank = saved.bank,
            Field::Number => h.number = saved.number.clone(),
            Field::Date => h.date = saved.date,
            Field::Payee => {
                h.payee = saved.payee;
                h.payee_name.clone_from(&saved.payee_name);
            }
            Field::Amount => {
                h.amount = saved.amount;
                h.amount_from_lines = saved.amount_from_lines;
            }
            Field::Address => h.address.clone_from(&saved.address),
            Field::Memo => h.memo.clone_from(&saved.memo),
        });
        self.sync_inputs();
        true
    }
}

/// The text of `key` with `figure` (an amount) for `{var}` in the
/// monospace numeric font and the rest in the body font (DESIGN.md: amounts
/// are mono); one label, so it reads as one sentence.
fn with_figure(
    ui: &Ui,
    key: &str,
    var: &str,
    figure: &str,
    size: f32,
    color: egui::Color32,
) -> egui::text::LayoutJob {
    const MARK: &str = "\u{1}";
    let text = t_with(key, &[(var, MARK)]);
    let body = egui::TextFormat {
        font_id: egui::FontId::proportional(size),
        color,
        ..Default::default()
    };
    let mono = egui::TextFormat {
        font_id: egui::FontId::new(size, egui::TextStyle::Monospace.resolve(ui.style()).family),
        color,
        ..Default::default()
    };
    let mut job = egui::text::LayoutJob::default();
    for (i, part) in text.split(MARK).enumerate() {
        if i > 0 {
            job.append(figure, 0.0, mono.clone());
        }
        if !part.is_empty() {
            job.append(part, 0.0, body.clone());
        }
    }
    job
}
