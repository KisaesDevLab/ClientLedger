//! Drawing Daily Sales Summary on the Write Check pattern (Q-261): the
//! toolbar, the header card (Layout Â· Date Â· Tag Â· Notes), the lines by
//! section (Line Â· Account Â· Amount), the summary with the cash
//! over/short, the action row and the side panel; and the keymap commands.
//!
//! Every input carries its visible label as its accessible name. Amounts
//! are shown with `Money::format` in mono, right-aligned; the only figures
//! besides the owner's typed amounts are the exact totals of
//! `cl_model::forms::daily_sales::totals`.

use cl_core::Money;
use cl_core::ids::DailyLineId;
use cl_model::CommandId;
use cl_model::amount_entry::{self, AmountRules};
use cl_model::forms::SaveIntent;
use cl_model::forms::daily_sales::{DailySection, OverShort};
use eframe::egui::{self, Align, CornerRadius, Layout, Margin, RichText, Stroke, Ui, Vec2};

use super::{DailySalesScreen, Field, field};
use crate::company::CompanyEvent;
use crate::forms::closing_prompt::ClosingClick;
use crate::forms::condensed;
use crate::forms::header;
use crate::i18n::{t, t_with};
use crate::layout::{Body, FixedFrame, FrameOpts};
use crate::screens::company::form;
use crate::screens::register::account_text;
use crate::theme::Tokens;

/// Below this width the side panel goes under the form.
const SIDE_BY_SIDE: f32 = 1100.0;
/// The side panel's width beside the form.
const PANEL_WIDTH: f32 = 270.0;
/// The height of Clear / Save & New / Save & Close (DESIGN.md).
const ACTION_HEIGHT: f32 = 44.0;

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

/// One line on the form: its index, id, label and account text.
type Row = (usize, DailyLineId, String, String);

/// The heading of a section on the form.
pub(crate) const fn section_key(s: DailySection) -> &'static str {
    match s {
        DailySection::Sales => "daily.section.sales",
        DailySection::Discount => "daily.section.discount",
        DailySection::Tax => "daily.section.tax",
        DailySection::Tips => "daily.section.tips",
        DailySection::Payment => "daily.section.payment",
        DailySection::Payout => "daily.section.payout",
        DailySection::Other => "daily.section.other",
    }
}

/// "2.35 short â€” posts to Cash Over/Short" and the like (Q-204).
pub(crate) fn over_short_text(o: OverShort, style: cl_core::MoneyStyle) -> String {
    match o {
        OverShort::Even => t("daily.over_short.even"),
        OverShort::Short(m) => t_with("daily.over_short.short", &[("amount", &m.format(style))]),
        OverShort::Over(m) => t_with("daily.over_short.over", &[("amount", &m.format(style))]),
    }
}

impl DailySalesScreen {
    /// Draws the whole window.
    pub(crate) fn draw(&mut self, ui: &mut Ui) {
        let tokens = Tokens::of(ui.ctx());
        if !self.templates_loaded {
            form::progress_line(ui, &tokens, &t("daily.loading"));
            return;
        }
        let wide = ui.available_width() >= SIDE_BY_SIDE;
        // Toolbar and messages on top, the actions at the bottom; the rest
        // scrolls (Q-265).
        let mut frame = FixedFrame::begin(ui, "daily_sales", FrameOpts::default());
        frame.top(|ui| {
            self.toolbar(ui, &tokens);
            self.messages(ui, &tokens);
            if self.nav_prompt.is_some()
                && let Some(a) =
                    crate::forms::nav_prompt::ui(ui, "daily_sales", &t("cmd.clients.daily_sales"))
            {
                self.answer_nav(a);
            }
        });
        if self.books.is_none() && self.link.session().is_none() {
            frame.body(Body::Scroll, |ui| {
                form::hint(ui, &tokens, &t("daily.unavailable"));
            });
            self.frame_report = frame.end();
            return;
        }
        if self.setup.is_some() {
            frame.body(Body::Scroll, |ui| self.setup_ui(ui, &tokens));
            self.frame_report = frame.end();
            return;
        }
        if self.templates.is_empty() {
            frame.body(Body::Scroll, |ui| self.starter_ui(ui, &tokens));
            self.frame_report = frame.end();
            return;
        }
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
        // The actions share the form's condensed width (Q-267).
        frame.bottom(|ui| {
            if wide {
                let w = form_width(ui).min(condensed::MAX_WIDTH);
                ui.allocate_ui(Vec2::new(w, ACTION_HEIGHT), |ui| self.actions_ui(ui));
            } else {
                condensed::capped(ui, |ui| self.actions_ui(ui));
            }
        });
        self.frame_report = frame.end();
    }

    fn toolbar(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let working = self.save.task.is_some();
        let posted = self
            .saved
            .as_ref()
            .is_some_and(|s| s.status == cl_api::EntryStatus::Posted);
        let dirty = self.is_dirty();
        header::toolbar(ui, tokens, dirty, &t("daily.unsaved"), |ui| {
            let can_nav = !working && self.setup.is_none() && self.load_task.is_none();
            if let Some(nav) = header::nav_buttons(
                ui,
                can_nav,
                (&t("daily.prev"), &t("daily.prev.tip")),
                (&t("daily.next"), &t("daily.next.tip")),
            ) {
                self.navigate(nav.older());
            }
            ui.separator();
            let can_setup = !working && self.setup.is_none() && !self.templates.is_empty();
            if ui
                .add_enabled(can_setup, egui::Button::new(t("daily.setup")))
                .on_hover_text(t("daily.setup.tip"))
                .clicked()
            {
                self.open_setup();
            }
            if ui
                .add_enabled(!working && posted, egui::Button::new(t("daily.void")))
                .on_disabled_hover_text(t("daily.void.posted_only"))
                .clicked()
            {
                self.start_void();
            }
            self.template_slot(ui);
        });
    }

    fn messages(&mut self, ui: &mut Ui, tokens: &Tokens) {
        match self.closing_prompt.ui(ui, tokens) {
            ClosingClick::Submit => self.submit_closing(),
            ClosingClick::Cancel => self.closing_prompt.cancel(),
            ClosingClick::None => {}
        }
        if let Some(key) = self.save.notice {
            let mut retry = false;
            ui.horizontal_wrapped(|ui| {
                form::hint(ui, tokens, &t(key));
                if key == "daily.not_posted" {
                    retry = ui.button(t("daily.try_again")).clicked();
                }
            });
            if retry {
                self.start_save(SaveIntent::Save, None);
            }
        }
        if self.void.open {
            let mut send = false;
            let mut cancel = false;
            form::card(ui, tokens, |ui| {
                form::heading(ui, tokens, &t("daily.void.title"));
                form::hint(ui, tokens, &t("daily.void.explain"));
                ui.horizontal(|ui| {
                    send = ui.button(t("daily.void.confirm")).clicked();
                    cancel = ui.button(t("daily.void.cancel")).clicked();
                });
            });
            if cancel {
                self.void = super::VoidState::default();
            } else if send {
                self.send_void(None);
            }
        }
    }

    /// No layout yet: start from a preset or blank.
    fn starter_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        form::card(ui, tokens, |ui| {
            form::heading(ui, tokens, &t("daily.starter.title"));
            form::hint(ui, tokens, &t("daily.starter.explain"));
            ui.horizontal(|ui| {
                ui.label(t("daily.starter.from"));
                let idle = self.setup_task.is_none();
                if ui
                    .add_enabled(idle, egui::Button::new(t("daily.starter.retail")))
                    .clicked()
                {
                    self.start_from(Some(cl_api::Preset::Retail));
                }
                if ui
                    .add_enabled(idle, egui::Button::new(t("daily.starter.restaurant")))
                    .clicked()
                {
                    self.start_from(Some(cl_api::Preset::Restaurant));
                }
                if ui
                    .add_enabled(idle, egui::Button::new(t("daily.starter.blank")))
                    .clicked()
                {
                    self.start_from(None);
                }
            });
            if let Some(e) = &self.save.error {
                form::inline_error(ui, tokens, e);
            }
        });
    }

    /// The form, no wider than the condensed table (Q-267): the header
    /// card, the lines and the totals share one width.
    fn form_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        condensed::capped(ui, |ui| self.form_column(ui, tokens));
    }

    fn form_column(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let enabled =
            self.save.task.is_none() && !self.closing_prompt.dialog_open() && !self.read_only();
        if self.read_only() {
            let key = match self.saved.as_ref().map(|s| &s.status) {
                Some(cl_api::EntryStatus::Posted) => "daily.read_only.posted",
                _ => "daily.read_only.void",
            };
            form::hint(ui, tokens, &t(key));
        } else if self.saved.is_some() {
            form::hint(ui, tokens, &t("daily.draft_note"));
        }
        header::title_only(ui, tokens, &t("cmd.clients.daily_sales"));
        ui.add_space(6.0);
        let read_only = self.read_only();
        header::locked_scope(ui, enabled, read_only, |ui| self.header_ui(ui, tokens));
        ui.add_space(12.0);
        header::locked_scope(ui, enabled, read_only, |ui| self.lines_ui(ui, tokens));
        ui.add_space(6.0);
        self.summary_ui(ui, tokens);
        self.problems_ui(ui, tokens);
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

    /// The header card: Layout Â· Date on top, Tag Â· Notes below.
    fn header_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        egui::Frame::new()
            .fill(tokens.surface_form)
            .stroke(Stroke::new(1.0, tokens.accent_border))
            .corner_radius(CornerRadius::same(8))
            .inner_margin(Margin::symmetric(20, 16))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                let width = ui.available_width();
                let sp = ui.spacing().item_spacing.x;
                let right = (width * 0.34).clamp(160.0, 260.0);
                let gap = 16.0;
                let left = (width - right - gap - sp).max(160.0);
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(left);
                        self.layout_field(ui, tokens, left);
                    });
                    ui.add_space(gap);
                    ui.vertical(|ui| {
                        ui.set_width(right);
                        self.date_ui(ui, tokens, right);
                    });
                });
                ui.add_space(8.0);
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(left);
                        self.notes_ui(ui, tokens, left);
                    });
                    ui.add_space(gap);
                    ui.vertical(|ui| {
                        ui.set_width(right);
                        self.tag_ui(ui, tokens, right);
                    });
                });
            });
    }

    /// Layout: a list field (Q-250: a click opens the list, Ctrl+L too).
    fn layout_field(&mut self, ui: &mut Ui, tokens: &Tokens, width: f32) {
        let l = above(ui, tokens, &t("daily.layout"));
        let r = ui
            .add(
                egui::TextEdit::singleline(&mut self.inputs.layout)
                    .desired_width(width - 8.0)
                    .margin(crate::list_field::margin()),
            )
            .labelled_by(l.id);
        self.take_focus(Field::Layout, &r);
        if r.changed() {
            self.layout_list = !self.inputs.layout.trim().is_empty();
            self.inputs.layout_problem = false;
        }
        if let Some(open) = crate::list_field::mouse(ui, &r, self.layout_list) {
            self.layout_list = open;
        }
        let mut picked = None;
        if self.layout_list && (r.has_focus() || r.lost_focus()) {
            let query =
                crate::list_field::query(ui.ctx(), r.id, &self.inputs.layout).to_lowercase();
            let found: Vec<(cl_core::ids::DailyTemplateId, String)> = self
                .templates
                .iter()
                .filter(|t| query.is_empty() || t.name.to_lowercase().contains(&query))
                .map(|t| (t.id, t.name.clone()))
                .collect();
            let current = self
                .header
                .current()
                .template
                .and_then(|id| found.iter().position(|(f, _)| *f == id));
            if let Some(i) =
                crate::list_field::popup(ui, &r, found.iter().map(|(_, n)| n.as_str()), current)
            {
                picked = found.get(i).map(|(id, _)| *id);
            }
        }
        if let Some(id) = picked {
            self.pick_layout(id);
        } else if r.lost_focus() {
            self.resolve_layout();
            if crate::forms::enter::entered(&r).saves() {
                self.after_frame = Some(CommandId::new("form.save_new"));
            }
        }
        if self.inputs.layout_problem {
            ui.label(
                RichText::new(t("daily.layout.pick"))
                    .size(12.0)
                    .color(tokens.warn_text),
            );
        }
    }

    /// Autofill when the owner leaves Layout: the exact name, else the
    /// first that starts with the typed text.
    pub(crate) fn resolve_layout(&mut self) {
        self.layout_list = false;
        let typed = self.inputs.layout.trim().to_lowercase();
        let found = self
            .templates
            .iter()
            .find(|t| t.name.to_lowercase() == typed)
            .or_else(|| {
                self.templates
                    .iter()
                    .find(|t| !typed.is_empty() && t.name.to_lowercase().starts_with(&typed))
            })
            .map(|t| t.id);
        if let Some(id) = found {
            self.pick_layout(id);
        } else {
            self.inputs.layout_problem = true;
            self.inputs.rev = u64::MAX;
        }
    }

    fn date_ui(&mut self, ui: &mut Ui, tokens: &Tokens, width: f32) {
        let l = above(ui, tokens, &t("daily.date"));
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

    fn tag_ui(&mut self, ui: &mut Ui, tokens: &Tokens, width: f32) {
        let l = above(ui, tokens, &t("daily.tag"));
        let current = self.header.current().tag;
        let shown = current
            .and_then(|id| self.tags.iter().find(|g| g.id == id))
            .map_or_else(|| t("daily.tag.none"), |g| g.fields.name.clone());
        let mut picked = None;
        let r = egui::ComboBox::from_id_salt("daily_sales/tag")
            .width(width - 8.0)
            .selected_text(shown)
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(current.is_none(), t("daily.tag.none"))
                    .clicked()
                {
                    picked = Some(None);
                }
                for g in self.tags.iter().filter(|g| g.is_active) {
                    if ui
                        .selectable_label(current == Some(g.id), &g.fields.name)
                        .clicked()
                    {
                        picked = Some(Some(g.id));
                    }
                }
            })
            .response
            .labelled_by(l.id);
        self.take_focus(Field::Tag, &r);
        if let Some(tag) = picked {
            self.header.edit(field::TAG, |h| h.tag = tag);
        }
    }

    fn notes_ui(&mut self, ui: &mut Ui, tokens: &Tokens, width: f32) {
        let l = above(ui, tokens, &t("daily.notes"));
        let mut text = self.header.current().notes.clone();
        let r = ui
            .add(
                egui::TextEdit::singleline(&mut text)
                    .desired_width(width - 8.0)
                    .char_limit(cl_model::forms::daily_sales::NOTES_MAX),
            )
            .labelled_by(l.id);
        self.take_focus(Field::Notes, &r);
        if r.changed() {
            self.header.edit(field::NOTES, |h| h.notes = text);
            self.inputs.rev = self.header.revision();
        }
        if crate::forms::enter::entered(&r).saves() {
            self.after_frame = Some(CommandId::new("form.save_new"));
        }
    }

    /// The lines by section: Line Â· Account (read-only) Â· Amount.
    fn lines_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        if self.template.is_none() {
            form::progress_line(ui, tokens, &t("daily.loading"));
            return;
        }
        let rows: Vec<(DailySection, Vec<Row>)> = DailySection::ALL
            .iter()
            .map(|s| {
                let lines = self
                    .layout
                    .in_section(*s)
                    .filter_map(|(i, l)| {
                        let account = l
                            .account
                            .and_then(|a| self.accounts.iter().find(|x| x.id == a))
                            .map_or_else(|| t("daily.account.unmapped"), account_text);
                        l.id.map(|id| (i, id, l.label.clone(), account))
                    })
                    .collect::<Vec<_>>();
                (*s, lines)
            })
            .filter(|(_, l)| !l.is_empty())
            .collect();
        if rows.is_empty() {
            form::hint(ui, tokens, &t("daily.no_lines"));
            return;
        }
        // Line · Account · Amount in the condensed table (Q-267): striped,
        // the whole row lit on hover and focus, no wider than 760.
        let ((), report) = condensed::condensed_rows(ui, "daily_sales/lines", |c| {
            for (section, lines) in rows {
                c.section(&t(section_key(section)));
                if section == DailySection::Tax {
                    form::hint(c.ui(), tokens, &t("daily.section.tax.hint"));
                }
                for (index, id, label, account) in lines {
                    c.row(&label, &account, |ui, l| {
                        self.amount_ui(ui, tokens, index, id, l)
                    });
                }
            }
        });
        self.condensed = report;
    }

    fn amount_ui(
        &mut self,
        ui: &mut Ui,
        tokens: &Tokens,
        index: usize,
        id: DailyLineId,
        label: &egui::Response,
    ) -> egui::Response {
        let text = self.inputs.amounts.entry(id).or_default();
        let r = ui
            .add(
                egui::TextEdit::singleline(text)
                    .desired_width(condensed::AMOUNT_WIDTH)
                    .font(egui::TextStyle::Monospace)
                    .align(egui::Align2::RIGHT_CENTER),
            )
            .labelled_by(label.id);
        self.take_focus(Field::Amount(id), &r);
        if r.changed() {
            self.inputs.amount_problems.remove(&id);
        }
        if r.lost_focus() {
            self.amount_left(index, id);
            if crate::forms::enter::entered(&r).saves()
                && !self.inputs.amount_problems.contains_key(&id)
            {
                self.after_frame = Some(CommandId::new("form.save_new"));
            }
        }
        if let Some(key) = self.inputs.amount_problems.get(&id) {
            ui.label(RichText::new(t(key)).size(12.0).color(tokens.warn_text));
        }
        r
    }

    /// The owner left an amount: the text (a plain amount or a
    /// calculation, rounded to the cent, Q-232) becomes the line's value.
    /// A negative amount is kept so the save names it (Q-P14t).
    pub(crate) fn amount_left(&mut self, index: usize, id: DailyLineId) {
        let text = self
            .inputs
            .amounts
            .get(&id)
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        let value = if text.is_empty() {
            None
        } else {
            match amount_entry::parse(&text, AmountRules::MONEY) {
                Ok(p) => p.value.as_money(),
                Err(e) => {
                    self.inputs.amount_problems.insert(id, e.key().0);
                    return;
                }
            }
        };
        self.amount_typed(index, id, value);
    }

    /// Sets a line's value (as one undo step per line).
    pub(crate) fn amount_typed(&mut self, index: usize, id: DailyLineId, value: Option<Money>) {
        let now = self.header.current().values.get(&id).copied().flatten();
        if now != value {
            let at = field::AMOUNT + u64::try_from(index).unwrap_or(0);
            self.header.edit(at, |h| {
                if value.is_some() {
                    h.values.insert(id, value);
                } else {
                    h.values.remove(&id);
                }
            });
        }
        match value {
            Some(m) => {
                self.inputs.amounts.insert(id, m.format(self.money_style));
            }
            None => {
                self.inputs.amounts.remove(&id);
            }
        }
        self.inputs.amount_problems.remove(&id);
        self.inputs.rev = self.header.revision();
    }

    /// Totals and the cash over/short (Q-204).
    fn summary_ui(&self, ui: &mut Ui, tokens: &Tokens) {
        let Some(totals) = self.totals() else {
            return;
        };
        let style = self.money_style;
        let fig = |m: Money| m.format(style);
        // The totals right-aligned in the lines' amount column (Q-267).
        let _ = condensed::condensed_rows(ui, "daily_sales/totals", |c| {
            for (key, m) in [
                ("daily.total.sales", totals.sales),
                ("daily.total.tax", totals.tax),
                ("daily.total.payments", totals.payments),
            ] {
                c.total(&t(key), &fig(m));
            }
        });
        let color = match totals.over_short {
            OverShort::Even => tokens.text_secondary,
            _ => tokens.warn_text,
        };
        ui.label(
            RichText::new(over_short_text(totals.over_short, style))
                .strong()
                .color(color),
        )
        .on_hover_text(t("jargon.cash_over_short"));
        if !totals.unmapped.is_empty() {
            ui.label(
                RichText::new(t("daily.unmapped"))
                    .size(12.0)
                    .color(tokens.warn_text),
            );
        }
    }

    fn problems_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        for p in &self.save.problems {
            ui.add(egui::Label::new(RichText::new(t(p.key())).color(tokens.warn_text)).wrap());
        }
        if let Some(e) = &self.save.error {
            form::inline_error(ui, tokens, e);
        }
        if self.save.task.is_some() {
            form::progress_line(ui, tokens, &t("daily.saving"));
        }
    }

    fn actions_ui(&mut self, ui: &mut Ui) {
        let idle = self.save.task.is_none() && !self.closing_prompt.dialog_open();
        let can_save = idle && !self.read_only();
        let mut clicked = None;
        let row = Vec2::new(ui.available_width(), ACTION_HEIGHT);
        ui.allocate_ui_with_layout(row, Layout::right_to_left(Align::Center), |ui| {
            let close = ui
                .add_enabled_ui(can_save, |ui| {
                    crate::widgets::action_button_with_chip(
                        ui,
                        &t("daily.save_close"),
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
                        &t("daily.save_new"),
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
                    egui::Button::new(t("daily.clear")).min_size(Vec2::new(0.0, ACTION_HEIGHT)),
                )
                .clicked()
            {
                clicked = Some("form.clear");
            }
        });
        if let Some(cmd) = clicked {
            self.form_command(CommandId::new(cmd));
        }
    }

    /// The side panel: Payments Clearing, days not posted yet, recent days.
    fn panel_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let style = self.money_style;
        form::card(ui, tokens, |ui| {
            form::heading(ui, tokens, &t("daily.panel.clearing"))
                .on_hover_text(t("jargon.undeposited_funds"));
            ui.add(
                egui::Label::new(
                    RichText::new(t("daily.panel.clearing.explain"))
                        .size(12.0)
                        .color(tokens.text_secondary),
                )
                .wrap(),
            );
            if ui.button(t("cmd.banking.deposit")).clicked() {
                self.link
                    .push(CompanyEvent::OpenWindow(CommandId::new("banking.deposit")));
            }
        });
        let mut open = None;
        if !self.panel.drafts.is_empty() {
            ui.add_space(8.0);
            form::card(ui, tokens, |ui| {
                form::heading(ui, tokens, &t("daily.panel.drafts"));
                for d in &self.panel.drafts {
                    let text = t_with(
                        "daily.panel.draft_row",
                        &[("date", &crate::shell::us_date(d.date))],
                    );
                    if ui.link(text).clicked() {
                        open = Some(d.id);
                    }
                }
            });
        }
        ui.add_space(8.0);
        let mut retry = false;
        form::card(ui, tokens, |ui| {
            form::heading(ui, tokens, &t("daily.panel.recent"));
            if self.panel.recent_failed {
                ui.horizontal_wrapped(|ui| {
                    form::hint(ui, tokens, &t("daily.panel.failed"));
                    retry = ui.button(t("daily.try_again")).clicked();
                });
            } else if self.panel.recent.is_empty() {
                form::hint(ui, tokens, &t("daily.panel.recent.none"));
            }
            for d in &self.panel.recent {
                let status = match d.status {
                    cl_api::EntryStatus::Draft => t("daily.status.draft"),
                    cl_api::EntryStatus::Posted => t("daily.status.posted"),
                    cl_api::EntryStatus::Void => t("daily.status.void"),
                    cl_api::EntryStatus::Unknown(_) => String::new(),
                };
                let os = match d.status {
                    cl_api::EntryStatus::Posted => over_short_short(
                        cl_model::forms::daily_sales::OverShort::of(d.over_short),
                        style,
                    ),
                    _ => String::new(),
                };
                let text = format!("{} \u{b7} {status}{os}", crate::shell::us_date(d.date));
                if ui.link(text).clicked() {
                    open = Some(d.id);
                }
            }
        });
        if retry {
            self.load_panel();
        }
        if let Some(id) = open
            && !self.is_dirty()
        {
            self.open_entry(id);
        }
    }

    /// Keymap commands the closing dialog and the line setup did not take.
    pub(crate) fn form_command(&mut self, id: CommandId) -> bool {
        match id.as_str() {
            super::KIND => true,
            "form.save_as_template" => {
                self.save_as_template();
                true
            }
            "form.save_new" => {
                self.start_save(SaveIntent::SaveAndNew, None);
                true
            }
            "form.save_close" => {
                self.start_save(SaveIntent::SaveAndClose, None);
                true
            }
            "form.clear" => {
                self.clear();
                true
            }
            "edit.void_transaction" => self.start_void(),
            "form.prev_record" => {
                self.navigate(true);
                true
            }
            "form.next_record" => {
                self.navigate(false);
                true
            }
            "edit.open_list" if self.focused == Some(Field::Layout) => {
                self.layout_list = true;
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

    /// Esc: the layout list or Void closes, then the focused field goes
    /// back to its saved value; with nothing to revert the shell closes the
    /// window (asking first when the form is dirty).
    fn revert_field(&mut self) -> bool {
        if self.nav_prompt.is_some() {
            self.answer_nav(crate::windows::PromptAnswer::Keep);
            return true;
        }
        if self.void.open {
            self.void = super::VoidState::default();
            return true;
        }
        if self.layout_list {
            self.layout_list = false;
            return true;
        }
        let Some(f) = self.focused else { return false };
        let saved = self.header.original().clone();
        let now = self.header.current().clone();
        let differs = match f {
            Field::Layout => now.template != saved.template,
            Field::Date => now.date != saved.date,
            Field::Tag => now.tag != saved.tag,
            Field::Notes => now.notes != saved.notes,
            Field::Amount(id) => now.values.get(&id) != saved.values.get(&id),
        };
        if !differs {
            return false;
        }
        if f == Field::Layout {
            if let Some(id) = saved.template {
                self.header.edit_step(|h| {
                    h.template = saved.template;
                    h.values.clone_from(&saved.values);
                });
                self.load_template(id);
            }
        } else {
            self.header.edit_step(|h| match f {
                Field::Date => h.date = saved.date,
                Field::Tag => h.tag = saved.tag,
                Field::Notes => h.notes.clone_from(&saved.notes),
                Field::Amount(id) => match saved.values.get(&id) {
                    Some(v) => {
                        h.values.insert(id, *v);
                    }
                    None => {
                        h.values.remove(&id);
                    }
                },
                Field::Layout => {}
            });
        }
        self.sync_inputs();
        true
    }
}

/// " Â· 2.35 short" for the recent-days list ("" when even).
fn over_short_short(o: OverShort, style: cl_core::MoneyStyle) -> String {
    match o {
        OverShort::Even => String::new(),
        OverShort::Short(m) => t_with("daily.panel.short", &[("amount", &m.format(style))]),
        OverShort::Over(m) => t_with("daily.panel.over", &[("amount", &m.format(style))]),
    }
}
