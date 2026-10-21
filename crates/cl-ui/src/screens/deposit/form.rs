//! Drawing Make Deposit (the Write Check pattern, Q-261): the toolbar, the
//! deposit card, Payments to deposit, Other deposits, the cash back row,
//! the totals line and the action row; and the keymap commands.
//!
//! Every input carries its visible label as its accessible name. Amounts
//! show with `Money::format` in mono; the only figures besides the owner's
//! typed ones are the engine's (payment amounts, balances, totals of
//! those exact amounts from `DepositForm::totals`).

use cl_core::ids::AccountId;
use cl_model::CommandId;
use cl_model::amount_entry::{self, AmountRules};
use cl_model::forms::SaveIntent;
use cl_model::forms::deposit::BankMatch;
use cl_model::register::AccountClass;
use eframe::egui::{self, Align, CornerRadius, Layout, Margin, RichText, Stroke, Ui, Vec2};

use super::{DepositScreen, Field, VoidPrompt, field};
use crate::company::CompanyEvent;
use crate::forms::closing_prompt::ClosingClick;
use crate::forms::header;
use crate::i18n::{t, t_with};
use crate::screens::company::form;
use crate::screens::register::account_text;
use crate::theme::Tokens;

/// Below this width the side panel goes under the form.
const SIDE_BY_SIDE: f32 = 1100.0;
/// The side panel's width beside the form.
const PANEL_WIDTH: f32 = 270.0;
/// The height of Clear / Save & New / Save & Close.
const ACTION_HEIGHT: f32 = 44.0;

/// The shortcut chip text of a command ("Alt N").
fn chip(id: &str) -> String {
    cl_model::commands::find(id)
        .and_then(|c| c.shortcut)
        .map(crate::input::shortcut_text)
        .unwrap_or_default()
        .replace('+', " ")
}

fn above(ui: &mut Ui, tokens: &Tokens, label: &str) -> egui::Response {
    ui.add(
        egui::Label::new(RichText::new(label).size(12.0).color(tokens.text_secondary))
            .selectable(false),
    )
}

/// Enter left a single-line header field: Save & New (Q-190).
fn entered(r: &egui::Response) -> bool {
    r.lost_focus() && r.ctx.input(|i| i.key_pressed(egui::Key::Enter))
}

/// The deposit Memo box's width (it moves under Date when it doesn't fit).
const MEMO_WIDTH: f32 = 260.0;

impl DepositScreen {
    /// Draws the whole window.
    pub(crate) fn draw(&mut self, ui: &mut Ui) {
        let tokens = Tokens::of(ui.ctx());
        if !self.loaded {
            form::progress_line(ui, &tokens, &t("deposit.loading"));
            return;
        }
        self.toolbar(ui, &tokens);
        self.messages(ui, &tokens);
        if self.nav_prompt.is_some()
            && let Some(a) = crate::forms::nav_prompt::ui(ui, "deposit", &t("cmd.banking.deposit"))
        {
            self.answer_nav(a);
        }
        if self.deposit_accounts().is_empty() {
            ui.add_space(8.0);
            ui.label(RichText::new(t("deposit.no_bank")).color(tokens.text_secondary));
            if ui.button(t("deposit.open_chart")).clicked() {
                self.link.push(CompanyEvent::OpenWindow(CommandId::new(
                    crate::screens::lists::accounts::KIND,
                )));
            }
            return;
        }
        let wide = ui.available_width() >= SIDE_BY_SIDE;
        egui::ScrollArea::vertical()
            .id_salt("deposit/scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if wide {
                    ui.horizontal_top(|ui| {
                        let spacing = 2.0 * ui.spacing().item_spacing.x;
                        let form_width = ui.available_width() - PANEL_WIDTH - 20.0 - spacing;
                        ui.vertical(|ui| {
                            ui.set_width(form_width);
                            self.body_ui(ui, &tokens);
                        });
                        ui.add_space(20.0);
                        ui.vertical(|ui| {
                            ui.set_width(PANEL_WIDTH);
                            self.panel_ui(ui, &tokens);
                        });
                    });
                } else {
                    self.body_ui(ui, &tokens);
                    ui.add_space(12.0);
                    self.panel_ui(ui, &tokens);
                }
            });
    }

    fn body_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        if self.show_summary {
            self.summary_ui(ui, tokens);
        } else {
            self.form_ui(ui, tokens);
        }
    }

    fn toolbar(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let working = self.save.task.is_some();
        let dirty = self.is_dirty();
        header::toolbar(ui, tokens, dirty, &t("deposit.unsaved"), |ui| {
            if let Some(nav) = header::nav_buttons(
                ui,
                !working,
                (&t("deposit.prev"), &t("deposit.prev.tip")),
                (&t("deposit.next"), &t("deposit.next.tip")),
            ) {
                self.navigate(nav.older());
            }
            ui.separator();
            ui.add_enabled(false, egui::Button::new(t("deposit.print")))
                .on_disabled_hover_text(t("deposit.print.later"));
            let saved = self.record.is_some();
            let label = if self.show_summary {
                t("deposit.form")
            } else {
                t("deposit.summary")
            };
            if ui
                .add_enabled(saved, egui::Button::new(label))
                .on_disabled_hover_text(t("deposit.summary.saved_only"))
                .clicked()
            {
                self.show_summary = !self.show_summary;
            }
            if ui
                .add_enabled(
                    !working && self.editing.is_none(),
                    egui::Button::new(t("deposit.refund")),
                )
                .on_hover_text(t("deposit.refund.tip"))
                .clicked()
            {
                self.refund.open = !self.refund.open;
            }
        });
    }

    fn messages(&mut self, ui: &mut Ui, tokens: &Tokens) {
        match self.closing_prompt.ui(ui, tokens) {
            ClosingClick::Submit => self.submit_closing(),
            ClosingClick::Cancel => self.closing_prompt.cancel(),
            ClosingClick::None => {}
        }
        if let Some(key) = self.save.notice {
            form::hint(ui, tokens, &t(key));
        }
        if self.save.clearing_warning {
            ui.add(
                egui::Label::new(
                    RichText::new(t("deposit.clearing_warning")).color(tokens.warn_text),
                )
                .wrap(),
            );
        }
        if self.void.open {
            self.void_ui(ui, tokens);
        }
        if self.refund.open {
            self.refund_ui(ui, tokens);
        }
    }

    fn void_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let mut send = false;
        let mut cancel = false;
        form::card(ui, tokens, |ui| {
            form::heading(ui, tokens, &t("deposit.void.title"));
            let key = if self.editing.as_ref().is_some_and(|e| e.grouped) {
                "deposit.void.explain_grouped"
            } else {
                "deposit.void.explain"
            };
            form::hint(ui, tokens, &t(key));
            let r = form::field(ui, &t("deposit.void.reason"), &mut self.void.reason);
            send |= entered(&r);
            ui.horizontal(|ui| {
                send |= ui.button(t("deposit.void.confirm")).clicked();
                cancel = ui.button(t("deposit.void.cancel")).clicked();
            });
        });
        if cancel {
            self.void = VoidPrompt::default();
        } else if send {
            let reason = self.void.reason.clone();
            self.send_void(&reason, None);
        }
    }

    fn form_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let read_only = self.editing.as_ref().is_some_and(|e| e.read_only);
        let enabled = self.save.task.is_none() && !self.closing_prompt.dialog_open() && !read_only;
        if let Some(e) = self.editing.as_ref().filter(|e| e.read_only) {
            form::hint(ui, tokens, &t(e.note));
        } else if self.editing_locked() {
            form::hint(ui, tokens, &t("closing.edit_note"));
        }
        header::title_only(ui, tokens, &t("cmd.banking.deposit"));
        ui.add_space(6.0);
        ui.add_enabled_ui(enabled, |ui| self.card_ui(ui, tokens));
        ui.add_space(12.0);
        ui.add_enabled_ui(enabled, |ui| self.payments_ui(ui, tokens, enabled));
        ui.add_space(12.0);
        self.lines_ui(ui, tokens, enabled);
        ui.add_space(6.0);
        ui.add_enabled_ui(enabled, |ui| self.cash_back_ui(ui, tokens));
        ui.add_space(6.0);
        self.totals_ui(ui, tokens);
        self.problems_ui(ui, tokens);
        ui.add_space(12.0);
        self.actions_ui(ui);
    }

    fn take_focus(&mut self, f: Field, r: &egui::Response) {
        if self.focus == Some(f) {
            self.focus = None;
            r.request_focus();
        }
        if r.has_focus() {
            self.focused = Some(f);
        } else if self.focused == Some(f) {
            self.focused = None;
        }
    }

    /// Deposit to (with the engine's balance), Date, Memo.
    fn card_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let banks: Vec<(AccountId, String)> = self
            .deposit_accounts()
            .into_iter()
            .map(|a| (a.id, account_text(a)))
            .collect();
        let current = self.account();
        let shown = banks
            .iter()
            .find(|(id, _)| Some(*id) == current)
            .map(|(_, s)| s.clone())
            .unwrap_or_default();
        let mut picked = None;
        let style = self.money_style;
        egui::Frame::new()
            .fill(tokens.surface_form)
            .stroke(Stroke::new(1.0, tokens.accent_border))
            .corner_radius(CornerRadius::same(8))
            .inner_margin(Margin::symmetric(20, 16))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                header::wrap_row(ui, |ui| {
                    header::wrap_item(ui, "deposit/account", |ui| {
                        let l = above(ui, tokens, &t("deposit.account"));
                        // Q-250: a click opens the list.
                        let r = ui
                            .add_enabled_ui(self.editing.is_none(), |ui| {
                                egui::ComboBox::from_id_salt("deposit/account")
                                    .width(260.0)
                                    .selected_text(shown)
                                    .show_ui(ui, |ui| {
                                        for (id, text) in &banks {
                                            if ui
                                                .selectable_label(Some(*id) == current, text)
                                                .clicked()
                                            {
                                                picked = Some(*id);
                                            }
                                        }
                                    })
                                    .response
                                    .labelled_by(l.id)
                            })
                            .inner;
                        self.take_focus(Field::Account, &r);
                        let text = self
                            .panel
                            .balance
                            .map(|m| m.format(style))
                            .unwrap_or_default();
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(t("deposit.balance"))
                                    .size(12.0)
                                    .color(tokens.text_secondary),
                            );
                            // The engine's balance as sent; never derived.
                            ui.label(RichText::new(text).text_style(egui::TextStyle::Monospace));
                        });
                    });
                    header::wrap_item(ui, "deposit/date", |ui| {
                        let l = above(ui, tokens, &t("deposit.date"));
                        let before = self.inputs.date.text.clone();
                        let r = ui
                            .add(
                                egui::TextEdit::singleline(&mut self.inputs.date.text)
                                    .desired_width(110.0)
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
                        if entered(&r) {
                            self.after_frame = Some(CommandId::new("form.save_new"));
                        }
                    });
                    ui.add_space(16.0);
                    // Memo keeps a usable width; it takes a row of its own
                    // when the row has no room for it (200 % zoom).
                    let memo_width = MEMO_WIDTH;
                    header::wrap_item(ui, "deposit/memo", |ui| {
                        let l = above(ui, tokens, &t("deposit.memo"));
                        let mut text = self.header.current().memo.clone();
                        let r = ui
                            .add(
                                egui::TextEdit::singleline(&mut text)
                                    .desired_width(memo_width)
                                    .char_limit(1000),
                            )
                            .labelled_by(l.id);
                        self.take_focus(Field::Memo, &r);
                        if r.changed() {
                            self.header.edit(field::MEMO, |h| h.memo = text);
                            self.inputs.rev = self.header.revision();
                        }
                        if entered(&r) {
                            self.after_frame = Some(CommandId::new("form.save_new"));
                        }
                    });
                });
            });
        if let Some(id) = picked {
            self.set_account(id);
        }
    }

    fn lines_ui(&mut self, ui: &mut Ui, tokens: &Tokens, enabled: bool) {
        ui.horizontal(|ui| {
            let r = ui.add(
                egui::Label::new(
                    RichText::new(t("deposit.lines.label"))
                        .size(14.0)
                        .color(tokens.text_primary),
                )
                .selectable(false),
            );
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
        let hint = if self.caps().cashback {
            "deposit.lines.fee_hint"
        } else {
            "deposit.lines.fee_later"
        };
        form::hint(ui, tokens, &t(hint));
    }

    /// The accounts cash back can go to: active, not the deposit account,
    /// not a control account, not income/expense.
    fn cash_back_accounts(&self) -> Vec<(AccountId, String)> {
        let control = self.control_accounts();
        let own = self.account();
        self.accounts
            .iter()
            .filter(|a| a.is_active && Some(a.id) != own && !control.contains(&a.id))
            .filter(|a| {
                !matches!(
                    crate::screens::register::class_of(a),
                    Some(AccountClass::IncomeOrExpense) | None
                )
            })
            .map(|a| (a.id, account_text(a)))
            .collect()
    }

    /// Cash back: account · amount · memo (hidden without
    /// `deposit-cashback-v1`, Q-P14c).
    fn cash_back_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        if !self.caps().cashback {
            return;
        }
        let options = self.cash_back_accounts();
        let current = self.header.current().cash_back.account;
        let shown = options
            .iter()
            .find(|(id, _)| Some(*id) == current)
            .map(|(_, s)| s.clone())
            .unwrap_or_default();
        let mut picked: Option<Option<AccountId>> = None;
        ui.horizontal_wrapped(|ui| {
            let l = ui
                .label(RichText::new(t("deposit.cash_back")).color(tokens.text_secondary))
                .on_hover_text(t("deposit.cash_back.tip"));
            let r = egui::ComboBox::from_id_salt("deposit/cash_back/account")
                .width(220.0)
                .selected_text(shown)
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(current.is_none(), t("deposit.cash_back.none"))
                        .clicked()
                    {
                        picked = Some(None);
                    }
                    for (id, text) in &options {
                        if ui.selectable_label(Some(*id) == current, text).clicked() {
                            picked = Some(Some(*id));
                        }
                    }
                })
                .response
                .labelled_by(l.id);
            self.take_focus(Field::CashAccount, &r);
            let la = ui.label(t("deposit.cash_back.amount"));
            let r = ui
                .add(
                    egui::TextEdit::singleline(&mut self.inputs.cash_amount)
                        .desired_width(110.0)
                        .font(egui::TextStyle::Monospace)
                        .align(egui::Align2::RIGHT_CENTER),
                )
                .labelled_by(la.id);
            self.take_focus(Field::CashAmount, &r);
            if r.changed() {
                self.inputs.cash_problem = None;
            }
            if r.lost_focus() {
                self.cash_amount_left();
            }
            let lm = ui.label(t("deposit.cash_back.memo"));
            let mut memo = self.header.current().cash_back.memo.clone();
            let r = ui
                .add(
                    egui::TextEdit::singleline(&mut memo)
                        .desired_width(180.0)
                        .char_limit(cl_api::CASH_BACK_MEMO_MAX),
                )
                .labelled_by(lm.id);
            if r.changed() {
                self.header
                    .edit(field::CASH_MEMO, |h| h.cash_back.memo = memo);
                self.inputs.rev = self.header.revision();
            }
        });
        if let Some(k) = self.inputs.cash_problem {
            ui.label(RichText::new(t(k)).size(12.0).color(tokens.warn_text));
        }
        if let Some(a) = picked {
            self.header
                .edit(field::CASH_ACCOUNT, |h| h.cash_back.account = a);
            self.inputs.rev = self.header.revision();
        }
    }

    /// The owner left the cash back amount: the text (a plain amount or a
    /// calculation, rounded to the cent, Q-232) becomes the amount.
    pub(crate) fn cash_amount_left(&mut self) {
        let text = self.inputs.cash_amount.trim().to_string();
        let current = self.header.current().cash_back.amount;
        if text.is_empty() {
            if current.is_some() {
                self.header
                    .edit(field::CASH_AMOUNT, |h| h.cash_back.amount = None);
                self.inputs.rev = self.header.revision();
            }
            return;
        }
        match amount_entry::parse(&text, AmountRules::MONEY) {
            Ok(p) => {
                let Some(m) = p.value.as_money() else { return };
                if current != Some(m) {
                    self.header
                        .edit(field::CASH_AMOUNT, |h| h.cash_back.amount = Some(m));
                }
                self.inputs.cash_amount = m.format(self.money_style);
                self.inputs.cash_problem = None;
                self.inputs.rev = self.header.revision();
            }
            Err(e) => self.inputs.cash_problem = Some(e.key().0),
        }
    }

    fn totals_ui(&self, ui: &mut Ui, tokens: &Tokens) {
        let style = self.money_style;
        let Some(totals) = self.compose().totals() else {
            return;
        };
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new(t_with(
                    "deposit.totals.total",
                    &[("amount", &totals.total.format(style))],
                ))
                .strong()
                .color(tokens.text_primary),
            );
            if !totals.cash_back.is_zero() {
                ui.label(
                    RichText::new(t_with(
                        "deposit.totals.cash_back",
                        &[("amount", &totals.cash_back.format(style))],
                    ))
                    .size(12.0)
                    .color(tokens.text_secondary),
                );
            }
            match self.bank_match() {
                Some(BankMatch::Matches) => {
                    let bank = self.panel.chosen.as_ref().map(|c| c.amount.format(style));
                    ui.label(
                        RichText::new(t_with(
                            "deposit.totals.bank_matches",
                            &[("amount", &bank.unwrap_or_default())],
                        ))
                        .color(tokens.text_secondary),
                    );
                }
                Some(BankMatch::Differs(d)) => {
                    ui.label(
                        RichText::new(t_with(
                            "deposit.totals.bank_differs",
                            &[("amount", &d.format(style))],
                        ))
                        .color(tokens.warn_text),
                    );
                }
                None => {}
            }
        });
    }

    fn problems_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        for p in &self.save.problems {
            ui.add(egui::Label::new(RichText::new(t(p.key().0)).color(tokens.warn_text)).wrap());
        }
        if self.save.stale {
            let mut reload = false;
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(t("deposit.changed_elsewhere")).color(tokens.warn_text));
                reload = ui.button(t("deposit.reload")).clicked();
            });
            if reload {
                self.reload();
            }
        }
        if let Some(e) = &self.save.error {
            form::inline_error(ui, tokens, e);
        }
        if self.save.task.is_some() {
            form::progress_line(ui, tokens, &t("deposit.saving"));
        }
    }

    fn actions_ui(&mut self, ui: &mut Ui) {
        let idle = self.save.task.is_none() && !self.closing_prompt.dialog_open();
        let read_only = self.editing.as_ref().is_some_and(|e| e.read_only);
        let mut clicked = None;
        let row = Vec2::new(ui.available_width(), ACTION_HEIGHT);
        ui.allocate_ui_with_layout(row, Layout::right_to_left(Align::Center), |ui| {
            let can_save = idle && !read_only;
            let close = ui
                .add_enabled_ui(can_save, |ui| {
                    crate::widgets::action_button_with_chip(
                        ui,
                        &t("deposit.save_close"),
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
                        &t("deposit.save_new"),
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
                    egui::Button::new(t("deposit.clear")).min_size(Vec2::new(0.0, ACTION_HEIGHT)),
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

    /// Keymap commands the grid and the closing dialog did not take.
    pub(crate) fn form_command(&mut self, id: CommandId) -> bool {
        match id.as_str() {
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
            "file.print" => {
                self.save.notice = Some("deposit.print.later");
                true
            }
            "edit.void_transaction" => self.start_void(),
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

    /// Esc: an open card closes, then the focused field goes back to its
    /// saved value; with nothing to revert the shell closes the window.
    fn revert_field(&mut self) -> bool {
        if self.nav_prompt.is_some() {
            self.answer_nav(crate::windows::PromptAnswer::Keep);
            return true;
        }
        if self.void.open {
            self.void = VoidPrompt::default();
            return true;
        }
        if self.refund.open {
            self.refund = super::refund::RefundState::default();
            return true;
        }
        if self.show_summary {
            self.show_summary = false;
            return true;
        }
        let Some(f) = self.focused else { return false };
        let saved = self.header.original().clone();
        let now = self.header.current();
        let differs = match f {
            Field::Account => now.account != saved.account,
            Field::Date => now.date != saved.date,
            Field::Memo => now.memo != saved.memo,
            Field::CashAccount => now.cash_back.account != saved.cash_back.account,
            Field::CashAmount => now.cash_back.amount != saved.cash_back.amount,
            Field::Payments => now.picked != saved.picked,
        };
        if !differs {
            return false;
        }
        self.header.edit_step(|h| match f {
            Field::Account => h.account = saved.account,
            Field::Date => h.date = saved.date,
            Field::Memo => h.memo.clone_from(&saved.memo),
            Field::CashAccount => h.cash_back.account = saved.cash_back.account,
            Field::CashAmount => h.cash_back.amount = saved.cash_back.amount,
            Field::Payments => h.picked.clone_from(&saved.picked),
        });
        self.sync_inputs();
        true
    }
}
