//! Drawing Enter Bill (mockup EnterBill.html, plan §3 WP4): the toolbar
//! (Prev/Next, Find, Attach, Save as Template disabled with a tooltip (D8),
//! History; the Bill / Vendor credit radio; Bill received), the
//! source-document banner (WP7), the header card in the mockup's 6-column
//! order (Vendor, Date, Ref. no., Address read-only from the vendor, Terms,
//! Amount due, Discount date, Bill due, Memo) plus the header Tags field
//! (Q-P13g), the Expenses / Items tabs with totals and "Lines total = amount
//! due ✓", Clear / Save & New / Save & Close, and the side panel (the
//! source-document card; the vendor panel, WP8). And the form's keymap
//! commands.
//!
//! Every input carries its visible label as its accessible name. Amounts
//! are shown with `Money::format` in mono, right-aligned; the figures the
//! form shows besides the owner's typed ones are the lines totals of the
//! owner's typed line amounts (`BillForm::lines_total`, exact).

use cl_core::Money;
use cl_core::ids::{ContactId, TagId};
use cl_grid::{LookupResult, Lookups};
use cl_model::CommandId;
use cl_model::amount_entry::{self, AmountRules};
use cl_model::forms::SaveIntent;
use cl_model::forms::bill::{BillKind, CUSTOM_TERMS};
use cl_model::forms::doc_read::ReadTarget;
use cl_model::forms::item_lines::items_total;
use cl_model::forms::lines::{TotalsStatus, lines_total};
use eframe::egui::{self, Align, CornerRadius, Layout, Margin, RichText, Stroke, Ui, Vec2};

use super::{EnterBillScreen, Field, Tab, field};
use crate::company::CompanyEvent;
use crate::forms::closing_prompt::ClosingClick;
use crate::forms::header;
use crate::forms::template::SaveAsTemplate;
use crate::i18n::{t, t_with};
use crate::layout::{Body, FixedFrame, FrameOpts};
use crate::screens::company::form;
use crate::screens::register::lists::NAMES;
use crate::theme::Tokens;

/// Below this width the side panel goes under the form (README §6).
const SIDE_BY_SIDE: f32 = 1100.0;
/// The side panel's width beside the form.
const PANEL_WIDTH: f32 = 300.0;
/// How many names the vendor list shows while typing.
const VENDOR_SUGGESTIONS: usize = 8;
/// The height of Clear / Save & New / Save & Close (DESIGN.md: 44 px).
const ACTION_HEIGHT: f32 = 44.0;
/// The gaps of the header card's grid (mockup: 12 px rows, 16 px columns).
const COL_GAP: f32 = 16.0;
/// The narrowest a date or terms column gets (a date in mono fits).
const DATE_WIDTH: f32 = 124.0;
/// Below this width (200% zoom on the 960 x 640 window) the Bill / Vendor
/// credit choice sits under the title, not at the end of a toolbar that
/// scrolls, and the header card's fields stack.
const COMPACT: f32 = 600.0;

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

/// The label of a payment term (`Net 15`, `2% 10 Net 30`).
pub(crate) fn term_label(code: &str) -> String {
    if let Some(term) = cl_model::lists::terms::term(code) {
        return t(term.label.0);
    }
    let key = format!("bill.terms.{code}");
    if crate::i18n::has(&key) {
        t(&key)
    } else {
        code.to_string()
    }
}

impl EnterBillScreen {
    /// Draws the whole window.
    pub(crate) fn draw(&mut self, ui: &mut Ui) {
        let tokens = Tokens::of(ui.ctx());
        if !self.accounts_loaded {
            form::progress_line(ui, &tokens, &t("bill.loading"));
            return;
        }
        let wide = ui.available_width() >= SIDE_BY_SIDE;
        self.compact = ui.available_width() < COMPACT;
        let mut frame = FixedFrame::begin(ui, "enter_bill", FrameOpts::default());
        frame.top(|ui| {
            self.toolbar(ui, &tokens);
            self.messages(ui, &tokens);
        });
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
                        self.side_ui(ui, &tokens);
                    });
                });
            } else {
                self.form_ui(ui, &tokens);
                ui.add_space(12.0);
                self.side_ui(ui, &tokens);
            }
        });
        frame.bottom(|ui| {
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
        let caps = self.caps();
        header::toolbar(ui, tokens, dirty, &t("bill.unsaved"), |ui| {
            if let Some(nav) = header::nav_buttons(
                ui,
                !working,
                (&t("bill.prev"), &t("bill.prev.tip")),
                (&t("bill.next"), &t("bill.next.tip")),
            ) {
                self.navigate(nav.older());
            }
            ui.separator();
            if ui
                .button(t("bill.find"))
                .on_hover_text(t("bill.find.tip"))
                .clicked()
            {
                self.find();
            }
            if ui
                .add_enabled(!working, egui::Button::new(t("bill.attach")))
                .on_hover_text(t("bill.attach.tip"))
                .clicked()
            {
                self.attach.focus = true;
            }
            // Bills have no templates in the engine yet (D8, U-91).
            let _ = header::save_as_template_button(
                ui,
                &SaveAsTemplate::Disabled(t("bill.template.unavailable")),
            );
            if ui
                .add_enabled(saved, egui::Button::new(t("bill.history")))
                .on_disabled_hover_text(t("bill.history.saved_only"))
                .clicked()
            {
                self.toggle_history();
            }
            // The toolbar overflow: Vendor statement match (WP6) opens in
            // its own tab (also in the Vendors menu). Kept off the row so
            // Bill / Vendor credit stays on it at 960 x 640.
            let more = ui.menu_button(t("bill.more"), |ui| {
                if ui
                    .button(t("cmd.nav.statement_match"))
                    .on_hover_text(t("bill.statement_match.tip"))
                    .clicked()
                {
                    self.link.push(CompanyEvent::OpenWindow(CommandId::new(
                        crate::screens::bill::statement_match::KIND,
                    )));
                    ui.close();
                }
            });
            more.response.on_hover_text(t("bill.more.tip"));
            if !self.compact {
                ui.separator();
                self.kind_row(ui, working, caps.received);
            }
        });
    }

    /// Bill / Vendor credit and Bill received: in the toolbar, or under the
    /// title in a narrow window.
    fn kind_row(&mut self, ui: &mut Ui, working: bool, received_cap: bool) {
        let kind = self.kind();
        // A04: a draft converts as a bill only.
        let can_switch = !working && self.editing.is_none() && self.draft.is_none();
        for (k, key) in [
            (BillKind::Bill, "bill.kind.bill"),
            (BillKind::Credit, "bill.kind.credit"),
        ] {
            if ui
                .add_enabled(can_switch, egui::RadioButton::new(kind == k, t(key)))
                .on_disabled_hover_text(t("bill.kind.locked"))
                .clicked()
            {
                self.set_kind(k);
            }
        }
        if kind == BillKind::Bill {
            let mut received = self.header.current().bill_received;
            let enabled = !working && received_cap;
            let r = ui
                .add_enabled(
                    enabled,
                    egui::Checkbox::new(&mut received, t("bill.received")),
                )
                .on_hover_text(t("bill.received.tip"))
                .on_disabled_hover_text(t("bill.received.needs_update"));
            if r.changed() {
                self.header.edit_step(|h| h.bill_received = received);
            }
        }
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
        if self.void.open {
            self.void_ui(ui, tokens);
        }
        if self.history.open {
            self.history_ui(ui, tokens);
        }
        if self.prefill.dropped {
            form::hint(ui, tokens, &t("bill.prefill.dropped"));
        }
        // A04 draft mode: where the values came from, Discard document.
        let vendor_name = self.draft_vendor_name();
        // One "Filled from" line: the reading banner below names the
        // source while it shows (UI check item 3).
        let reading_shown = self
            .read
            .filled
            .as_ref()
            .is_some_and(|f| self.read.unaccepted > 0 || f.nothing_new);
        if let Some(d) = self.draft.as_mut() {
            d.hide_source_line = reading_shown;
        }
        if let Some(d) = self.draft.as_mut()
            && let crate::screens::doc_review::review::BannerClick::EnterAs(to) =
                d.banner_ui(ui, tokens, None, vendor_name.as_deref())
        {
            self.enter_as(ui.ctx(), to);
        }
        // The source-document banner: "Attached from <file> in your receipt
        // inbox" (Q-P17j, WP5's text), then the reading's values to accept
        // (WP7).
        let name = self.attach.viewer.source().map(|s| s.name.clone());
        // In draft mode the draft banner names the document already.
        let attached_line = self.attach.from_inbox && self.draft.is_none();
        if let Some(n) = name.as_deref().filter(|_| attached_line) {
            egui::Frame::new()
                .fill(tokens.info_tint)
                .stroke(Stroke::new(1.0, tokens.border_default))
                .corner_radius(CornerRadius::same(6))
                .inner_margin(Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.add(
                        egui::Label::new(
                            RichText::new(t_with("inbox.attached_from", &[("name", n)]))
                                .color(tokens.text_primary),
                        )
                        .wrap(),
                    );
                });
        }
        let input = super::doc_read::ReadInput {
            link: &self.link,
            file_name: name.as_deref(),
            attachment: self.attach.shown,
            from_inbox: self.attach.from_inbox,
            tenant: self.tenant.as_ref().map(|(t, _)| *t),
            money_style: self.money_style,
        };
        match super::doc_read::banner(ui, &mut self.read, &input, tokens) {
            super::doc_read::BannerEvent::None => {}
            super::doc_read::BannerEvent::Accepted => self.accept_read(),
            super::doc_read::BannerEvent::Cleared => self.clear_read(),
            super::doc_read::BannerEvent::AskAi => self.start_assist(),
            super::doc_read::BannerEvent::AddVendor => {
                self.link
                    .push(CompanyEvent::OpenWindow(CommandId::new("vendors.center")));
            }
        }
        self.nav_prompt_ui(ui);
    }

    /// Accept these values (Alt+A): the read fields become the owner's.
    pub(crate) fn accept_read(&mut self) {
        let mut form = self.compose();
        self.read.accept(&mut form);
        self.take_form(form);
    }

    fn nav_prompt_ui(&mut self, ui: &mut Ui) {
        if self.nav_prompt.is_none() {
            return;
        }
        let title = self.title_text();
        if let Some(a) = crate::forms::nav_prompt::ui(ui, "enter_bill/nav_prompt", &title) {
            self.answer_nav(a);
        }
    }

    fn void_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let mut send = false;
        let mut cancel = false;
        let delete = self.void.delete;
        form::card(ui, tokens, |ui| {
            let (title, explain, confirm) = if delete {
                (
                    "bill.delete.title",
                    "bill.delete.explain",
                    "bill.delete.confirm",
                )
            } else {
                ("bill.void.title", "bill.void.explain", "bill.void.confirm")
            };
            form::heading(ui, tokens, &t(title));
            form::hint(ui, tokens, &t(explain));
            let r = form::field(ui, &t("bill.void.reason"), &mut self.void.reason);
            send |= crate::forms::enter::submitted(&r);
            ui.horizontal(|ui| {
                send |= ui.button(t(confirm)).clicked();
                cancel = ui.button(t("bill.void.cancel")).clicked();
            });
        });
        if cancel {
            self.void = super::VoidPrompt::default();
        } else if send {
            let reason = self.void.reason.clone();
            self.send_void(&reason, delete, None);
        }
    }

    fn history_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        form::card(ui, tokens, |ui| {
            form::heading(ui, tokens, &t("bill.history.title"));
            if self.history.task.is_some() {
                form::progress_line(ui, tokens, &t("bill.loading"));
            } else if self.history.error {
                form::hint(ui, tokens, &t("bill.history.failed"));
            } else if self.history.rows.is_empty() {
                form::hint(ui, tokens, &t("bill.history.empty"));
            }
            for row in &self.history.rows {
                ui.label(row);
            }
            if ui.button(t("bill.history.close")).clicked() {
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
        let title = match self.kind() {
            BillKind::Bill => t("cmd.vendors.enter_bill"),
            BillKind::Credit => t("bill.title.credit"),
        };
        header::title_left(ui, tokens, &title);
        if self.compact {
            let working = self.save.task.is_some();
            let received = self.caps().received;
            ui.horizontal_wrapped(|ui| self.kind_row(ui, working, received));
        }
        ui.add_space(6.0);
        if let Some(e) = self.editing.as_ref().filter(|e| e.read_only) {
            form::hint(ui, tokens, &t(e.read_only_note));
        } else if self.editing.as_ref().is_some_and(|e| e.has_payments) {
            form::hint(ui, tokens, &t("bill.paid.note"));
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

    /// Vendor, date and amount stay as they are once payments are applied.
    fn paid_locked(&self) -> bool {
        self.editing.as_ref().is_some_and(|e| e.has_payments)
    }

    /// The header card: the mockup's 6-column grid.
    fn card_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        egui::Frame::new()
            .fill(tokens.surface_form)
            .stroke(Stroke::new(1.0, tokens.accent_border))
            .corner_radius(CornerRadius::same(8))
            .inner_margin(Margin::symmetric(18, 16))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                let width = ui.available_width();
                let sp = ui.spacing().item_spacing.x;
                if self.compact {
                    self.card_stacked(ui, tokens, width);
                    return;
                }
                // Six columns with five gaps (each gap is the space plus
                // egui's item spacing).
                let col = ((width - 5.0 * (COL_GAP + sp)) / 6.0).max(60.0);
                let span = |n: f32| col * n + (n - 1.0) * (COL_GAP + sp);
                // A date column never narrower than a date (960 x 640): the
                // wide left column gives up the difference.
                let one = span(1.0).max(DATE_WIDTH);
                let wide = span(3.0) - (one - span(1.0));
                let right = span(3.0) + (one - span(1.0));
                // Row 1: Vendor (3) · Date (1) · Ref. no. (2).
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(wide);
                        self.vendor_ui(ui, tokens, wide);
                    });
                    ui.add_space(COL_GAP);
                    ui.vertical(|ui| {
                        ui.set_width(one);
                        self.date_ui(ui, tokens, one);
                    });
                    ui.add_space(COL_GAP);
                    ui.vertical(|ui| {
                        ui.set_width(span(2.0));
                        self.reference_ui(ui, tokens, span(2.0));
                    });
                });
                ui.add_space(10.0);
                // Rows 2-3: Address (3, two rows) · Terms (1) · Amount due
                // (2) / Discount date (1) · Bill due (2).
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(wide);
                        self.address_ui(ui, tokens, wide);
                    });
                    ui.add_space(COL_GAP);
                    ui.vertical(|ui| {
                        ui.set_width(right);
                        ui.horizontal_top(|ui| {
                            ui.vertical(|ui| {
                                ui.set_width(one);
                                self.terms_ui(ui, tokens, one);
                            });
                            ui.add_space(COL_GAP);
                            ui.vertical(|ui| {
                                ui.set_width(span(2.0));
                                self.amount_ui(ui, tokens, span(2.0));
                            });
                        });
                        ui.add_space(10.0);
                        ui.horizontal_top(|ui| {
                            ui.vertical(|ui| {
                                ui.set_width(one);
                                self.discount_ui(ui, tokens, one);
                            });
                            ui.add_space(COL_GAP);
                            ui.vertical(|ui| {
                                ui.set_width(span(2.0));
                                self.due_ui(ui, tokens, span(2.0));
                            });
                        });
                    });
                });
                ui.add_space(10.0);
                self.memo_ui(ui, tokens, width);
                ui.add_space(6.0);
                self.tags_ui(ui, tokens);
            });
    }

    /// The header card in a narrow window: Vendor, Address and Memo across
    /// the card; Date and Ref. no., Terms and Amount due, Discount date and
    /// Bill due in pairs (the mockup's order), each pair stacking when even
    /// that is too wide.
    fn card_stacked(&mut self, ui: &mut Ui, tokens: &Tokens, width: f32) {
        let sp = ui.spacing().item_spacing.x;
        let rest = width - DATE_WIDTH - COL_GAP - sp;
        let pairs = rest >= DATE_WIDTH;
        let pair = |ui: &mut Ui,
                    s: &mut Self,
                    left: &dyn Fn(&mut Self, &mut Ui, f32),
                    right: &dyn Fn(&mut Self, &mut Ui, f32)| {
            if pairs {
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(DATE_WIDTH);
                        left(s, ui, DATE_WIDTH);
                    });
                    ui.add_space(COL_GAP);
                    ui.vertical(|ui| {
                        ui.set_width(rest);
                        right(s, ui, rest);
                    });
                });
            } else {
                left(s, ui, width);
                ui.add_space(10.0);
                right(s, ui, width);
            }
            ui.add_space(10.0);
        };
        self.vendor_ui(ui, tokens, width);
        ui.add_space(10.0);
        pair(
            ui,
            self,
            &|s, ui, w| s.date_ui(ui, tokens, w),
            &|s, ui, w| s.reference_ui(ui, tokens, w),
        );
        self.address_ui(ui, tokens, width);
        ui.add_space(10.0);
        pair(
            ui,
            self,
            &|s, ui, w| s.terms_ui(ui, tokens, w),
            &|s, ui, w| s.amount_ui(ui, tokens, w),
        );
        pair(
            ui,
            self,
            &|s, ui, w| s.discount_ui(ui, tokens, w),
            &|s, ui, w| s.due_ui(ui, tokens, w),
        );
        self.memo_ui(ui, tokens, width);
        ui.add_space(6.0);
        self.tags_ui(ui, tokens);
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

    fn vendor_ui(&mut self, ui: &mut Ui, tokens: &Tokens, width: f32) {
        let l = above(ui, tokens, &t("bill.vendor"));
        let locked = self.paid_locked();
        let slot = super::doc_read::mark_slot(ui);
        let r = ui
            .add_enabled(
                !locked,
                egui::TextEdit::singleline(&mut self.inputs.vendor)
                    .desired_width(width - 8.0)
                    .margin(crate::list_field::margin())
                    .font(egui::FontId::proportional(15.0)),
            )
            .labelled_by(l.id)
            .on_disabled_hover_text(t("bill.paid.locked"));
        self.take_focus(Field::Vendor, &r);
        let read = self.read_state_of(ReadTarget::Vendor);
        super::doc_read::field_mark(ui, tokens, read, slot, &r);
        if r.changed() {
            self.vendor_list = !self.inputs.vendor.trim().is_empty();
            self.inputs.vendor_problem = false;
        }
        if let Some(open) = crate::list_field::mouse(ui, &r, self.vendor_list) {
            self.vendor_list = open;
        }
        let mut picked = None;
        if self.vendor_list && (r.has_focus() || r.lost_focus()) {
            picked = self.vendor_popup(ui, &r);
        }
        // Tab out of the vendor (list open or not) goes on to the Date, as
        // the next field in the card's order.
        let tabbed =
            r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Tab) && !i.modifiers.shift);
        if tabbed && !self.paid_locked() {
            self.focus = Some(Field::Date);
        }
        if let Some(id) = picked {
            self.pick_vendor(id);
        } else if r.lost_focus() {
            self.resolve_vendor();
            if crate::forms::enter::entered(&r).saves() {
                self.after_frame = Some(CommandId::new("form.save_new"));
            }
        }
        if self.inputs.vendor_problem {
            ui.label(
                RichText::new(t("bill.vendor.pick"))
                    .size(12.0)
                    .color(tokens.warn_text),
            );
        }
    }

    /// A contact the bill can be from: a vendor (or a client that is also
    /// a vendor).
    fn is_vendor(&self, id: ContactId) -> bool {
        self.contacts.iter().any(|c| {
            c.id == id
                && matches!(
                    c.fields.contact_type,
                    cl_api::ContactType::Vendor | cl_api::ContactType::Both
                )
        })
    }

    /// The vendors that start with (or contain) the typed text, under the
    /// field (every vendor when a click opened it, Q-250).
    fn vendor_popup(&mut self, ui: &mut Ui, r: &egui::Response) -> Option<ContactId> {
        let query = crate::list_field::query(ui.ctx(), r.id, &self.inputs.vendor);
        let limit = if query.is_empty() { 400 } else { 40 };
        let LookupResult::Ready(found) = self.lists.suggest(NAMES, &query, limit) else {
            return None;
        };
        let found: Vec<_> = found
            .into_iter()
            .filter(|s| self.lists.name(s.id).is_some_and(|c| self.is_vendor(c)))
            .take(if query.is_empty() {
                200
            } else {
                VENDOR_SUGGESTIONS
            })
            .collect();
        let current = self
            .vendor()
            .and_then(|p| found.iter().position(|s| self.lists.name(s.id) == Some(p)));
        let i = crate::list_field::popup(ui, r, found.iter().map(|s| s.label.as_str()), current)?;
        found.get(i).and_then(|s| self.lists.name(s.id))
    }

    /// Autofill when the owner leaves the vendor: the exact name, else the
    /// first vendor that starts with the typed text. No free-text vendors:
    /// anything else asks for a name from the list.
    pub(crate) fn resolve_vendor(&mut self) {
        self.vendor_list = false;
        let typed = self.inputs.vendor.trim().to_string();
        if typed.is_empty() {
            self.header.edit(field::VENDOR, |h| h.vendor = None);
            self.inputs.rev = self.header.revision();
            return;
        }
        let exact = self
            .lists
            .exact(NAMES, &typed)
            .and_then(|s| self.lists.name(s.id))
            .filter(|c| self.is_vendor(*c));
        let found = exact.or_else(|| match self.lists.suggest(NAMES, &typed, 40) {
            LookupResult::Ready(s) => s
                .into_iter()
                .filter(|s| s.label.to_lowercase().starts_with(&typed.to_lowercase()))
                .filter_map(|s| self.lists.name(s.id))
                .find(|c| self.is_vendor(*c)),
            LookupResult::Loading => None,
        });
        if let Some(id) = found {
            self.pick_vendor(id);
        } else {
            self.inputs.vendor_problem = true;
            self.header.edit(field::VENDOR, |h| h.vendor = None);
            self.inputs.rev = self.header.revision();
        }
    }

    fn date_ui(&mut self, ui: &mut Ui, tokens: &Tokens, width: f32) {
        let l = above(ui, tokens, &t("bill.date"));
        let before = self.inputs.date.text.clone();
        let slot = super::doc_read::mark_slot(ui);
        let r = ui
            .add_enabled(
                !self.paid_locked(),
                egui::TextEdit::singleline(&mut self.inputs.date.text)
                    .desired_width(width - 8.0)
                    .font(egui::TextStyle::Monospace),
            )
            .labelled_by(l.id)
            .on_disabled_hover_text(t("bill.paid.locked"));
        self.take_focus(Field::Date, &r);
        let read = self.read_state_of(ReadTarget::BillDate);
        super::doc_read::field_mark(ui, tokens, read, slot, &r);
        if r.changed() {
            self.inputs.date.typed(&before, self.today);
            let value = self.inputs.date.value;
            if value.is_some() && value != self.header.current().date {
                self.header.edit(field::DATE, |h| h.date = value);
                self.inputs.rev = self.header.revision();
                self.refresh_terms_dates();
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

    fn reference_ui(&mut self, ui: &mut Ui, tokens: &Tokens, width: f32) {
        let l = above(ui, tokens, &t("bill.ref"));
        let mut text = self.header.current().reference.clone();
        let slot = super::doc_read::mark_slot(ui);
        let r = ui
            .add(
                egui::TextEdit::singleline(&mut text)
                    .desired_width(width - 8.0)
                    .char_limit(cl_model::forms::bill::REF_MAX)
                    .font(egui::TextStyle::Monospace),
            )
            .labelled_by(l.id);
        self.take_focus(Field::Reference, &r);
        let read = self.read_state_of(ReadTarget::Reference);
        super::doc_read::field_mark(ui, tokens, read, slot, &r);
        if r.changed() {
            self.header.edit(field::REFERENCE, |h| h.reference = text);
            self.inputs.rev = self.header.revision();
        }
        if crate::forms::enter::entered(&r).saves() {
            self.after_frame = Some(CommandId::new("form.save_new"));
        }
    }

    fn address_ui(&mut self, ui: &mut Ui, tokens: &Tokens, width: f32) {
        let l = above(ui, tokens, &t("bill.address"));
        // Read-only, from the vendor record (bills store no address,
        // Q-P17f).
        // Read-only text keeps its normal colour (contrast >= 4.5:1).
        let mut text = self.vendor_address();
        ui.add(
            egui::TextEdit::multiline(&mut text)
                .interactive(false)
                .desired_rows(4)
                .desired_width(width - 8.0),
        )
        .labelled_by(l.id)
        .on_hover_text(t("bill.address.tip"));
    }

    fn terms_ui(&mut self, ui: &mut Ui, tokens: &Tokens, width: f32) {
        let l = above(ui, tokens, &t("bill.terms"));
        if self.kind() == BillKind::Credit {
            let mut none = "\u{2014}".to_string();
            ui.add_enabled(
                false,
                egui::TextEdit::singleline(&mut none).desired_width(width - 8.0),
            )
            .labelled_by(l.id)
            .on_disabled_hover_text(t("bill.credit.no_terms"));
            return;
        }
        let current = self.effective_terms();
        let shown = current.as_deref().map(term_label).unwrap_or_default();
        let codes: Vec<String> = self.terms.iter().map(|t| t.code.clone()).collect();
        let mut picked = None;
        let r = egui::ComboBox::from_id_salt("enter_bill/terms")
            .width(width - 8.0)
            .selected_text(shown)
            .show_ui(ui, |ui| {
                for code in &codes {
                    if ui
                        .selectable_label(current.as_deref() == Some(code), term_label(code))
                        .clicked()
                    {
                        picked = Some(code.clone());
                    }
                }
            })
            .response
            .labelled_by(l.id);
        self.take_focus(Field::Terms, &r);
        if let Some(code) = picked {
            self.prefill.terms_chosen = true;
            self.header.edit(field::TERMS, |h| h.terms = Some(code));
            self.inputs.rev = self.header.revision();
            self.refresh_terms_dates();
        }
        if current.as_deref() == Some(CUSTOM_TERMS) {
            let d = above(ui, tokens, &t("bill.terms.days"));
            let r = ui
                .add(
                    egui::TextEdit::singleline(&mut self.inputs.terms_days)
                        .desired_width(width - 8.0)
                        .char_limit(3)
                        .font(egui::TextStyle::Monospace),
                )
                .labelled_by(d.id);
            self.take_focus(Field::TermsDays, &r);
            if r.changed() {
                let days = self.inputs.terms_days.trim().parse::<u16>().ok();
                self.header.edit(field::TERMS, |h| h.terms_days = days);
                self.inputs.rev = self.header.revision();
                self.refresh_terms_dates();
            }
            // P17 WP8: the custom term's early-payment discount
            // (`bill-terms-v1`); cl-model checks both-or-neither and the days.
            if self.caps().terms {
                self.custom_discount_ui(ui, tokens, width);
            }
        }
    }

    /// Discount % and Discount days of a custom term (WP8).
    fn custom_discount_ui(&mut self, ui: &mut Ui, tokens: &Tokens, width: f32) {
        let l = above(ui, tokens, &t("bill.discount_percent"));
        let r = ui
            .add(
                egui::TextEdit::singleline(&mut self.inputs.discount_percent)
                    .desired_width(width - 8.0)
                    .char_limit(9)
                    .font(egui::TextStyle::Monospace),
            )
            .labelled_by(l.id)
            .on_hover_text(t("bill.discount_percent.tip"));
        self.take_focus(Field::Discount, &r);
        if r.changed() {
            let p = cl_model::forms::bill::Percent::parse_user(&self.inputs.discount_percent);
            self.header
                .edit(field::DISCOUNT, |h| h.discount_percent = p);
            self.inputs.rev = self.header.revision();
            self.refresh_terms_dates();
        }
        let text = self.inputs.discount_percent.trim();
        if !text.is_empty() && cl_model::forms::bill::Percent::parse_user(text).is_none() {
            ui.label(
                RichText::new(t("bill.discount_percent.invalid"))
                    .size(12.0)
                    .color(tokens.warn_text),
            );
        }
        let d = above(ui, tokens, &t("bill.discount_days"));
        let r = ui
            .add(
                egui::TextEdit::singleline(&mut self.inputs.discount_days)
                    .desired_width(width - 8.0)
                    .char_limit(3)
                    .font(egui::TextStyle::Monospace),
            )
            .labelled_by(d.id);
        self.take_focus(Field::DiscountDays, &r);
        if r.changed() {
            let days = self.inputs.discount_days.trim().parse::<u16>().ok();
            self.header
                .edit(field::DISCOUNT, |h| h.discount_days = days);
            self.inputs.rev = self.header.revision();
            self.refresh_terms_dates();
        }
    }

    fn amount_ui(&mut self, ui: &mut Ui, tokens: &Tokens, width: f32) {
        let key = match self.kind() {
            BillKind::Bill => "bill.amount_due",
            BillKind::Credit => "bill.credit_amount",
        };
        let l = above(ui, tokens, &t(key));
        let slot = super::doc_read::mark_slot(ui);
        let r = ui
            .add_enabled(
                !self.paid_locked(),
                egui::TextEdit::singleline(&mut self.inputs.amount)
                    .desired_width(width - 8.0)
                    .font(egui::FontId::monospace(15.0))
                    .align(egui::Align2::RIGHT_CENTER),
            )
            .labelled_by(l.id)
            .on_disabled_hover_text(t("bill.paid.locked"));
        self.take_focus(Field::Amount, &r);
        let read = self.read_state_of(ReadTarget::AmountDue);
        super::doc_read::field_mark(ui, tokens, read, slot, &r);
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

    /// The owner left the amount due: the text (a plain amount or a
    /// calculation, rounded to the cent, Q-232) becomes the amount due.
    pub(crate) fn amount_left(&mut self) {
        let text = self.inputs.amount.trim().to_string();
        if text.is_empty() {
            if self.header.current().amount_due.is_some() {
                self.amount_typed(None);
            }
            return;
        }
        match amount_entry::parse(&text, AmountRules::MONEY) {
            Ok(p) => {
                let Some(m) = p.value.as_money() else { return };
                let h = self.header.current();
                if h.amount_due != Some(m) || h.amount_from_lines {
                    self.amount_typed(Some(m));
                }
                self.inputs.amount = m.format(self.money_style);
                self.inputs.amount_problem = None;
            }
            Err(e) => self.inputs.amount_problem = Some(e.key().0),
        }
    }

    fn discount_ui(&mut self, ui: &mut Ui, tokens: &Tokens, width: f32) {
        let l = above(ui, tokens, &t("bill.discount_date"));
        // With bill-terms-v1 and a discount in the terms the owner may type
        // it (S3: sent only then); cleared, the terms decide again (WP8).
        // Terms without a discount show "—" (Q-P17g).
        let discount = self.terms_have_discount();
        if self.kind() == BillKind::Bill && self.caps().terms && discount {
            let before = self.inputs.discount.text.clone();
            let r = ui
                .add(
                    egui::TextEdit::singleline(&mut self.inputs.discount.text)
                        .desired_width(width - 8.0)
                        .font(egui::TextStyle::Monospace),
                )
                .labelled_by(l.id)
                .on_hover_text(t("bill.discount_date.owner_tip"));
            self.take_focus(Field::DiscountDate, &r);
            if r.changed() {
                self.inputs.discount.typed(&before, self.today);
                let value = self.inputs.discount.value;
                if value.is_some() && value != self.header.current().discount_date {
                    self.header
                        .edit(field::DISCOUNT_DATE, |h| h.set_discount_by_owner(value));
                    self.inputs.rev = self.header.revision();
                }
            }
            if r.lost_focus() {
                self.inputs.discount.settle();
                if self.inputs.discount.text.trim().is_empty()
                    && self.header.current().discount_overridden
                {
                    self.header
                        .edit(field::DISCOUNT_DATE, |h| h.set_discount_by_owner(None));
                    self.inputs.rev = self.header.revision();
                    self.terms_dates_for = None;
                    self.refresh_terms_dates();
                }
            }
            return;
        }
        // From the terms (the local preview without bill-terms-v1); "—"
        // without a discount.
        let mut text = self
            .header
            .current()
            .discount_date
            .filter(|_| self.kind() == BillKind::Bill && discount)
            .map_or_else(|| "\u{2014}".to_string(), crate::shell::us_date);
        let tip = if self.caps().terms {
            "bill.discount_date.none"
        } else {
            "bill.discount_date.tip"
        };
        ui.add_enabled(
            false,
            egui::TextEdit::singleline(&mut text)
                .desired_width(width - 8.0)
                .font(egui::TextStyle::Monospace),
        )
        .labelled_by(l.id)
        .on_disabled_hover_text(t(tip));
    }

    /// The terms in effect give an early-payment discount (a fixed discount
    /// term, or a custom term with its percentage and days), or the owner
    /// typed a discount date: only then is the discount date a field.
    pub(crate) fn terms_have_discount(&self) -> bool {
        let f = self.header.current();
        if f.discount_overridden {
            return true;
        }
        let Some(code) = self.effective_terms() else {
            return false;
        };
        if code == CUSTOM_TERMS {
            return f.discount_percent.is_some() && f.discount_days.is_some();
        }
        self.terms
            .iter()
            .find(|t| t.code == code)
            .is_some_and(|t| t.discount_percent.is_some() && t.discount_days.is_some())
    }

    fn due_ui(&mut self, ui: &mut Ui, tokens: &Tokens, width: f32) {
        let l = above(ui, tokens, &t("bill.due"));
        if self.kind() == BillKind::Credit {
            let mut none = "\u{2014}".to_string();
            ui.add_enabled(
                false,
                egui::TextEdit::singleline(&mut none).desired_width(width - 8.0),
            )
            .labelled_by(l.id)
            .on_disabled_hover_text(t("bill.credit.no_terms"));
            return;
        }
        let before = self.inputs.due.text.clone();
        let slot = super::doc_read::mark_slot(ui);
        let r = ui
            .add(
                egui::TextEdit::singleline(&mut self.inputs.due.text)
                    .desired_width(width - 8.0)
                    .font(egui::TextStyle::Monospace),
            )
            .labelled_by(l.id)
            .on_hover_text(t("bill.due.tip"));
        self.take_focus(Field::DueDate, &r);
        let read = self.read_state_of(ReadTarget::DueDate);
        super::doc_read::field_mark(ui, tokens, read, slot, &r);
        if r.changed() {
            self.inputs.due.typed(&before, self.today);
            let value = self.inputs.due.value;
            if value.is_some() && value != self.header.current().due_date {
                self.header.edit(field::DUE, |h| h.set_due_by_owner(value));
                self.inputs.rev = self.header.revision();
            }
        }
        if r.lost_focus() {
            self.inputs.due.settle();
            if self.inputs.due.text.trim().is_empty() {
                // Cleared: the terms decide again.
                self.header.edit(field::DUE, |h| h.set_due_by_owner(None));
                self.inputs.rev = self.header.revision();
                self.refresh_terms_dates();
            }
        }
        if !self.caps().terms && !self.header.current().due_overridden {
            form::hint(ui, tokens, &t("bill.due.preview"));
        }
        if crate::forms::enter::entered(&r).saves() {
            self.after_frame = Some(CommandId::new("form.save_new"));
        }
    }

    fn memo_ui(&mut self, ui: &mut Ui, tokens: &Tokens, width: f32) {
        let l = above(ui, tokens, &t("bill.memo"));
        let mut text = self.header.current().memo.clone();
        let r = ui
            .add(
                egui::TextEdit::singleline(&mut text)
                    .desired_width(width - 8.0)
                    .char_limit(500),
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

    /// Header tags (Q-178, Q-P13g): they default onto lines without their
    /// own tag. One tag per group: a pick replaces the group's other tag.
    fn tags_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let header = self.header.current().header_tags.clone();
        let mut remove = None;
        let mut add = None;
        ui.horizontal_wrapped(|ui| {
            let l = ui.add(
                egui::Label::new(
                    RichText::new(t("bill.tags"))
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
            egui::ComboBox::from_id_salt("enter_bill/tags")
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

    /// One lines tab: its name and its exact total, underlined when shown.
    fn tab_button(&mut self, ui: &mut Ui, tokens: &Tokens, tab: Tab, total: Money) {
        let style = self.money_style;
        let key = match tab {
            Tab::Expenses => "bill.tab.expenses",
            Tab::Items => "bill.tab.items",
        };
        let on = self.tab == tab;
        let text = t_with(key, &[("total", &total.format(style))]);
        let rich = if on {
            RichText::new(text).strong().color(tokens.text_primary)
        } else {
            RichText::new(text).color(tokens.text_secondary)
        };
        let r = ui.selectable_label(on, rich);
        if on {
            ui.painter().hline(
                r.rect.x_range(),
                r.rect.bottom() + 2.0,
                Stroke::new(2.0, tokens.accent),
            );
        }
        if r.clicked() {
            self.tab = tab;
        }
    }

    fn lines_ui(&mut self, ui: &mut Ui, tokens: &Tokens, enabled: bool) {
        let caps = self.caps();
        if !caps.lines {
            self.tab = Tab::Expenses;
        }
        let expenses = lines_total(self.expenses.value()).unwrap_or(Money::ZERO);
        let items = items_total(self.items.value()).unwrap_or(Money::ZERO);
        ui.horizontal(|ui| {
            self.tab_button(ui, tokens, Tab::Expenses, expenses);
            // Items only with `bill-lines-v1` (Q-P17c).
            if caps.lines {
                self.tab_button(ui, tokens, Tab::Items, items);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if self.compose().totals_status() == Some(TotalsStatus::Balanced) {
                    crate::widgets::label_with_check(
                        ui,
                        RichText::new(t("bill.totals.balanced"))
                            .size(12.0)
                            .color(tokens.accent),
                        false,
                    );
                }
            });
        });
        ui.add_space(4.0);
        ui.add_enabled_ui(enabled, |ui| match self.tab {
            Tab::Expenses => {
                let _ = self.expenses.ui(ui);
            }
            Tab::Items => {
                let _ = self.items.ui(ui);
            }
        });
        if !caps.lines {
            form::hint(ui, tokens, &t("bill.lines.detail_later"));
        }
        // Lines read from the document, and ghost accounts (WP7).
        if self.tab == Tab::Expenses {
            self.read_lines_ui(ui, tokens);
            self.line_suggestions_ui(ui, tokens);
        }
    }

    fn totals_ui(&self, ui: &mut Ui, tokens: &Tokens) {
        let style = self.money_style;
        let form = self.compose();
        let total = form.lines_total().unwrap_or(Money::ZERO);
        let (key, var, figure, warn) = match form.totals_status() {
            // The tab row says "Lines total = amount due ✓" (mockup).
            Some(TotalsStatus::Balanced) => return,
            Some(TotalsStatus::Remaining(m)) => ("bill.totals.remaining", "amount", m, true),
            Some(TotalsStatus::Over(m)) => ("bill.totals.over", "amount", m, true),
            None => ("bill.totals.lines", "total", total, false),
        };
        let color = if warn {
            tokens.warn_text
        } else {
            tokens.text_secondary
        };
        ui.label(
            RichText::new(t_with(key, &[(var, &figure.format(style))]))
                .size(12.0)
                .color(color),
        );
    }

    fn problems_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        for p in &self.save.problems {
            ui.add(egui::Label::new(RichText::new(t(p.key().0)).color(tokens.warn_text)).wrap());
        }
        if let Some(key) = self.save.refusal {
            ui.add(egui::Label::new(RichText::new(t(key)).color(tokens.warn_text)).wrap());
        }
        if self.save.stale {
            let mut reload = false;
            let mut keep = false;
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(t("bill.changed_elsewhere")).color(tokens.warn_text));
                reload = ui.button(t("bill.reload")).clicked();
                keep = ui.button(t("bill.keep_copy")).clicked();
            });
            if reload {
                self.reload();
            } else if keep {
                self.save.stale = false;
            }
        }
        if self.save.attach_failed.is_some() {
            let mut retry = false;
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(t("bill.attach.not_moved")).color(tokens.warn_text));
                retry = ui
                    .add_enabled(
                        self.save.attach_retry.is_none(),
                        egui::Button::new(t("bill.attach.try_again")),
                    )
                    .clicked();
            });
            if retry {
                self.retry_attach();
            }
        }
        if let Some(e) = &self.save.error {
            form::inline_error(ui, tokens, e);
        }
        // The PR-1 block: shared with every Enter Bill window.
        if let Some(ev) = self.save.lost.ui(
            ui,
            &mut self.header,
            &self.link,
            self.group.as_ref(),
            super::LOST_SCOPE,
        ) {
            self.lost_event(ev);
        }
        if self.save.task.is_some() {
            form::progress_line(ui, tokens, &t("bill.saving"));
        }
    }

    fn side_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        self.source_card_ui(ui, tokens);
        ui.add_space(12.0);
        match self.with_panel(|panel, input| super::vendor_panel::show(ui, panel, input, tokens)) {
            super::vendor_panel::PanelEvent::OpenRecord(kind, id) => self.open_from_panel(kind, id),
            super::vendor_panel::PanelEvent::GoBack => self.focus = Some(Field::Reference),
            super::vendor_panel::PanelEvent::None => {}
        }
    }

    /// The vendor panel's link: the bill or credit in this window (asking
    /// first when the form has unsaved edits).
    fn open_from_panel(&mut self, kind: BillKind, id: cl_core::ids::TransactionId) {
        if self.is_dirty() {
            self.link
                .push(CompanyEvent::OpenRecord(CommandId::new(super::KIND), id));
            return;
        }
        self.read_bill(id, Some(kind));
    }

    fn actions_ui(&mut self, ui: &mut Ui) {
        let idle = self.save.task.is_none() && !self.closing_prompt.dialog_open();
        let read_only = self.editing.as_ref().is_some_and(|e| e.read_only);
        let mut clicked = None;
        // At the right, in the order shown (and so for Tab): Clear, Save &
        // New, Save & Close (mockup).
        header::right_row(ui, "enter_bill/actions", ACTION_HEIGHT, |ui| {
            let can_save = idle && !read_only;
            if ui
                .add_enabled(
                    idle,
                    egui::Button::new(t("bill.clear")).min_size(Vec2::new(0.0, ACTION_HEIGHT)),
                )
                .clicked()
            {
                self.clear();
            }
            let new = ui
                .add_enabled_ui(can_save, |ui| {
                    crate::widgets::action_button_with_chip(
                        ui,
                        &t("bill.save_new"),
                        &chip("form.save_new"),
                        false,
                        ACTION_HEIGHT,
                    )
                })
                .inner;
            if new.clicked() {
                clicked = Some("form.save_new");
            }
            let close = ui
                .add_enabled_ui(can_save, |ui| {
                    crate::widgets::action_button_with_chip(
                        ui,
                        &t("bill.save_close"),
                        &chip("form.save_close"),
                        true,
                        ACTION_HEIGHT,
                    )
                })
                .inner;
            if close.clicked() {
                clicked = Some("form.save_close");
            }
        });
        if let Some(cmd) = clicked {
            self.form_command(CommandId::new(cmd));
        }
    }

    /// Find (Ctrl+F): the Accounts Payable register, where Go To finds a
    /// bill.
    pub(crate) fn find(&mut self) {
        let ap = self
            .accounts
            .iter()
            .find(|a| a.system_tag.as_deref() == Some("accounts_payable"))
            .map(|a| a.id);
        if let Some(ap) = ap {
            self.link.push(CompanyEvent::OpenRegister(ap));
        }
    }

    /// Keymap commands the grids, the viewer and the closing dialog did
    /// not take.
    pub(crate) fn form_command(&mut self, id: CommandId) -> bool {
        match id.as_str() {
            // Ctrl+B in the window: stays.
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
            "form.history" => {
                self.toggle_history();
                true
            }
            "form.save_as_template" => {
                self.save.notice = Some("bill.template.unavailable");
                true
            }
            "form.attach" => {
                self.attach.focus = true;
                true
            }
            "edit.void_transaction" => self.start_void(false),
            "form.delete_item" => self.start_void(true),
            "bill.kind_toggle" => {
                let other = match self.kind() {
                    BillKind::Bill => BillKind::Credit,
                    BillKind::Credit => BillKind::Bill,
                };
                self.set_kind(other);
                true
            }
            // With the engine's document reading assist there is one Read
            // document entry (plan D7): the P17 command reads here too.
            "bill.read_document" if self.read.assist_capable() => {
                self.start_local_read();
                true
            }
            "bill.read_document" => {
                let name = self.attach.viewer.source().map(|s| s.name.clone());
                let input = super::doc_read::ReadInput {
                    link: &self.link,
                    file_name: name.as_deref(),
                    attachment: self.attach.shown,
                    from_inbox: self.attach.from_inbox,
                    tenant: self.tenant.as_ref().map(|(t, _)| *t),
                    money_style: self.money_style,
                };
                self.read.start(&input);
                true
            }
            "bill.read_local" => {
                self.start_local_read();
                true
            }
            "form.accept_read" => {
                self.accept_read();
                true
            }
            "edit.open_list" if self.focused == Some(Field::Vendor) => {
                self.vendor_list = true;
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

    /// Esc: an open question closes, then the vendor list, then the focused
    /// field goes back to its saved value; with nothing to revert the shell
    /// closes the window (asking first when the form is dirty).
    fn revert_field(&mut self) -> bool {
        if self.nav_prompt.is_some() {
            self.answer_nav(crate::windows::PromptAnswer::Keep);
            return true;
        }
        if self.void.open {
            self.void = super::VoidPrompt::default();
            return true;
        }
        // The duplicate-ref question closes as Go back (WP8).
        if self.panel.go_back() {
            self.focus = Some(Field::Reference);
            return true;
        }
        if self.vendor_list {
            self.vendor_list = false;
            return true;
        }
        // Esc stops a reading on this computer, quietly, as Cancel does.
        if self.read.local.job.is_some() {
            self.read.local.cancel_job();
            // Not from a field (from Cancel reading, whose focus egui drops
            // on Esc): the keyboard goes back to Read document.
            if self.focused.is_none() {
                self.read.local.focus_read = true;
            }
            return true;
        }
        let Some(f) = self.focused else { return false };
        let saved = self.header.original().clone();
        let now = self.header.current();
        let differs = match f {
            Field::Vendor => now.vendor != saved.vendor,
            Field::Date => now.date != saved.date,
            Field::Reference => now.reference != saved.reference,
            Field::Terms | Field::TermsDays => {
                now.terms != saved.terms || now.terms_days != saved.terms_days
            }
            Field::Discount | Field::DiscountDays => {
                now.discount_percent != saved.discount_percent
                    || now.discount_days != saved.discount_days
            }
            Field::DiscountDate => now.discount_date != saved.discount_date,
            Field::Amount => now.amount_due != saved.amount_due,
            Field::DueDate => now.due_date != saved.due_date,
            Field::Memo => now.memo != saved.memo,
        };
        if !differs {
            return false;
        }
        self.header.edit_step(|h| match f {
            Field::Vendor => h.vendor = saved.vendor,
            Field::Date => h.date = saved.date,
            Field::Reference => h.reference.clone_from(&saved.reference),
            Field::Terms | Field::TermsDays => {
                h.terms.clone_from(&saved.terms);
                h.terms_days = saved.terms_days;
            }
            Field::Discount | Field::DiscountDays => {
                h.discount_percent = saved.discount_percent;
                h.discount_days = saved.discount_days;
            }
            Field::DiscountDate => {
                h.discount_date = saved.discount_date;
                h.discount_overridden = saved.discount_overridden;
            }
            Field::Amount => {
                h.amount_due = saved.amount_due;
                h.amount_from_lines = saved.amount_from_lines;
            }
            Field::DueDate => {
                h.due_date = saved.due_date;
                h.due_overridden = saved.due_overridden;
            }
            Field::Memo => h.memo.clone_from(&saved.memo),
        });
        self.sync_inputs();
        // The keyboard stays in the field that went back (egui lets go of
        // a text field on Esc).
        self.focus = Some(f);
        true
    }
}
