//! P22 D4: what the Credit Memo and Sales Receipt windows add to the shared
//! sales form.
//!
//! - **Saving** a credit memo or sales receipt goes to the kind's own
//!   routes (`/credit-memos`, `/sales-receipts`) with one idempotency key
//!   per save attempt, reused through the closing-date prompt.
//! - **Apply or keep** (Q-P22b): a credit memo saved while the client has
//!   open invoices opens the apply panel first: "Apply to an invoice"
//!   (oldest first, the engine's suggestion, amounts editable) or "Keep as
//!   credit". The applications go in the same create call (§2.4). Every
//!   figure is the preview's (`cl_model::forms::credit_memo`); the panel
//!   adds nothing up.
//! - A saved memo shows Applied to / Refunded / Remaining (the engine's
//!   strings); Refund opens Refund a Credit with the memo as the source.
//! - Print and Save PDF lay the saved document out with
//!   `cl_print::forms::{credit_memo, sales_receipt}`.

use std::sync::Arc;

use cl_api::{
    ApiError, CreditMemoRecord, CreditRefusal, CreditWriteOptions, SalesReceiptRecord,
    credit_refusal,
};
use cl_core::ids::TransactionId;
use cl_core::{Money, Quantity, Secret, UserInput};
use cl_model::closing::needs_password;
use cl_model::forms::SaveIntent;
use cl_model::forms::credit_memo::{AfterSave, ApplyAmount, CreditMemoProblem};
use cl_model::forms::invoice::{InvoiceForm, InvoiceLine, InvoiceRequest, SalesDocKind};
use eframe::egui::{self, RichText, Ui};
use serde_json::{Value, json};

use super::engine::{self, label};
use super::{InvoiceScreen, Pending, SavedDoc};
use crate::company::CompanyEvent;
use crate::i18n::{t, t_with};
use crate::screens::card_charge::kit;
use crate::theme::Tokens;

/// The saved lines of a memo or receipt as the grid's lines (the stored
/// rate and percent come back so an edit resends them).
fn lines_of(lines: &[cl_api::SalesDocLine], keys: &mut cl_grid::RowKeyGen) -> Vec<InvoiceLine> {
    let pct =
        |p: Option<cl_api::SignedPercent>| p.and_then(|p| Quantity::parse_wire(&p.to_wire()).ok());
    lines
        .iter()
        .map(|l| InvoiceLine {
            item: l.item_id,
            account: if l.item_id.is_some() {
                None
            } else {
                l.account_id
            },
            description: l.description.clone().unwrap_or_default(),
            service_date: l.service_date,
            quantity: l.quantity,
            rate: l.unit_price,
            discount_percent: pct(l.discount_percent),
            taxable: Some(l.is_taxable),
            tag: l.tag_id,
            ..InvoiceLine::blank(keys.next_key())
        })
        .collect()
}

/// A saved credit memo as the form.
pub(crate) fn memo_form_of(
    r: &CreditMemoRecord,
    keys: &mut cl_grid::RowKeyGen,
) -> (InvoiceForm, Vec<InvoiceLine>) {
    let form = InvoiceForm {
        kind: SalesDocKind::CreditMemo,
        client: Some(r.customer_id),
        date: Some(r.date),
        number: r.number.clone().unwrap_or_default(),
        memo: r.memo.clone().unwrap_or_default(),
        customer_message: r.customer_message.clone().unwrap_or_default(),
        lines: Vec::new(),
        ..InvoiceForm::default()
    };
    (form, lines_of(&r.lines, keys))
}

/// The engine's payment method as the form's wire string.
fn method_wire(m: cl_api::PaymentMethod) -> &'static str {
    match m {
        cl_api::PaymentMethod::Check => "check",
        cl_api::PaymentMethod::Cash => "cash",
        cl_api::PaymentMethod::CreditCard => "credit_card",
        cl_api::PaymentMethod::Ach => "ach",
        cl_api::PaymentMethod::Other => "other",
    }
}

/// A saved sales receipt as the form.
pub(crate) fn receipt_form_of(
    r: &SalesReceiptRecord,
    keys: &mut cl_grid::RowKeyGen,
) -> (InvoiceForm, Vec<InvoiceLine>) {
    let form = InvoiceForm {
        kind: SalesDocKind::SalesReceipt,
        client: r.customer_id,
        date: Some(r.date),
        number: r.number.clone().unwrap_or_default(),
        memo: r.memo.clone().unwrap_or_default(),
        customer_message: r.customer_message.clone().unwrap_or_default(),
        deposit_to: r.deposit_to_account_id,
        payment_method: r.payment_method.map(|m| method_wire(m).to_string()),
        ref_no: r.ref_no.clone().unwrap_or_default(),
        lines: Vec::new(),
        ..InvoiceForm::default()
    };
    (form, lines_of(&r.lines, keys))
}

/// The sales receipt's payment methods (the engine's wire values).
pub(crate) const RECEIPT_METHODS: [&str; 5] = ["check", "cash", "credit_card", "ach", "other"];

impl InvoiceScreen {
    /// Saves a credit memo or sales receipt (the checked request `req`).
    /// A credit memo asks Apply or Keep first when the client has open
    /// invoices (the panel's Save comes back here). False: nothing sent.
    #[allow(clippy::needless_pass_by_value, clippy::too_many_lines)] // the secret moves into the queued write
    pub(crate) fn start_doc_save(
        &mut self,
        intent: SaveIntent,
        password: Option<Secret>,
        req: InvoiceRequest,
    ) -> bool {
        // The engine's preview answers a sales receipt that needs a bank.
        if req.kind == SalesDocKind::SalesReceipt
            && self
                .doc_problems
                .iter()
                .any(|c| cl_model::forms::invoice::InvoiceProblem::from_preview_code(c).is_some())
        {
            let ps = self
                .doc_problems
                .iter()
                .filter_map(|c| cl_model::forms::invoice::InvoiceProblem::from_preview_code(c))
                .collect();
            self.refuse(ps);
            return false;
        }
        let ctx = self.context();
        let memo = if req.kind == SalesDocKind::CreditMemo {
            self.credit.doc = self.compose();
            match self.credit.save_plan(&ctx) {
                Ok(plan) => Some(plan),
                Err(ps) => {
                    self.credit_problems(intent, &ps);
                    return false;
                }
            }
        } else {
            None
        };
        let closing = self.books.closing;
        let old = self.editing.as_ref().map(|e| e.old_date);
        if password.is_none() && needs_password(old, req.date, closing) {
            let supported = self.closing_supported();
            if supported {
                self.closing_prompt
                    .ask(Pending::Save(intent), supported, closing);
            } else {
                self.closing_prompt.unavailable(supported, closing);
            }
            return false;
        }
        let edit = self.editing.as_ref().map(|e| (e.id, e.updated_at()));
        let pw = password.clone();
        let job = if let Some(plan) = memo {
            let mut new = match engine::new_credit_memo(&plan.request) {
                Ok(n) => n,
                Err(e) => {
                    self.error = Some(e);
                    return false;
                }
            };
            new.warn_only = new.txn_number.is_some();
            // Applications go only with a new memo: the engine refuses them
            // on an edit (400), so a saved memo uses Apply credit instead.
            new.applications = if edit.is_some() {
                Vec::new()
            } else {
                plan.applications
                    .iter()
                    .map(|a| cl_api::CreditMemoApplication {
                        invoice_id: a.invoice,
                        amount: a.amount,
                        expected_balance_due: a.token.clone(),
                    })
                    .collect()
            };
            let identity = format!(
                "memo|{}|{:?}|{}",
                serde_json::to_string(&new).unwrap_or_default(),
                edit.map(|(id, at)| (id.to_string(), at)),
                password.is_some()
            );
            let key = self.key_for(identity);
            let live = new.clone();
            engine::write(
                &self.link,
                self.group.as_ref(),
                label("orphan.credit_memo_save", Some(key.clone())),
                |d| {
                    match edit {
                        None => crate::company::demo::credit_memos::create_memo(
                            d,
                            &new,
                            password.as_ref(),
                        ),
                        Some((id, at)) => crate::company::demo::credit_memos::update_memo(
                            d,
                            id,
                            &new,
                            at,
                            password.as_ref(),
                        ),
                    }
                    .map(|r| (SavedDoc::CreditMemo(r), false))
                },
                move |s| async move {
                    let posting = super::save::options(pw);
                    let r = match edit {
                        None => s.credit_memos().create(&live, &posting, key).await?,
                        Some((id, at)) => {
                            let opts = CreditWriteOptions {
                                posting,
                                expected_updated_at: at,
                            };
                            s.credit_memos().update(id, &live, &opts, key).await?
                        }
                    };
                    Ok((SavedDoc::CreditMemo(r), false))
                },
            )
        } else {
            let mut new = match engine::new_sales_receipt(&req) {
                Ok(n) => n,
                Err(e) => {
                    self.error = Some(e);
                    return false;
                }
            };
            new.warn_only = new.txn_number.is_some();
            let identity = format!(
                "receipt|{}|{:?}|{}",
                serde_json::to_string(&new).unwrap_or_default(),
                edit.map(|(id, at)| (id.to_string(), at)),
                password.is_some()
            );
            let key = self.key_for(identity);
            let live = new.clone();
            engine::write(
                &self.link,
                self.group.as_ref(),
                label("orphan.sales_receipt_save", Some(key.clone())),
                |d| {
                    crate::company::demo::credit_memos::save_receipt(
                        d,
                        &new,
                        edit,
                        password.as_ref(),
                    )
                    .map(|r| (SavedDoc::SalesReceipt(r), false))
                },
                move |s| async move {
                    let posting = super::save::options(pw);
                    let r = match edit {
                        None => s.sales_receipts().create(&live, &posting, key).await?,
                        Some((id, at)) => {
                            let opts = CreditWriteOptions {
                                posting,
                                expected_updated_at: at,
                            };
                            s.sales_receipts().update(id, &live, &opts, key).await?
                        }
                    };
                    Ok((SavedDoc::SalesReceipt(r), false))
                },
            )
        };
        let _ = self.grid.begin_save(intent);
        let _ = self.header.begin_save(intent);
        self.intent = Some(intent);
        self.apply_open = None;
        self.save_job = Some(job);
        if self.link.is_demo() {
            self.poll();
        }
        false
    }

    /// A credit memo's save problems: the unanswered question opens the
    /// apply panel; the rest say what to fix.
    fn credit_problems(&mut self, intent: SaveIntent, ps: &[CreditMemoProblem]) {
        let mut form = Vec::new();
        let mut notice = None;
        for p in ps {
            match p {
                CreditMemoProblem::Form(f) => form.push(*f),
                CreditMemoProblem::Unanswered => {
                    self.open_apply_panel(intent);
                    return;
                }
                other => notice = notice.or(Some(other.key().0)),
            }
        }
        if !form.is_empty() {
            self.refuse(form);
        }
        if let Some(n) = notice {
            self.notice = Some(n);
        }
    }

    /// Opens "Apply to an invoice or keep as credit?" with the engine's
    /// oldest-first suggestion ticked.
    pub(crate) fn open_apply_panel(&mut self, intent: SaveIntent) {
        let caps = self.caps();
        self.credit.doc = self.compose();
        self.credit.choose_apply(caps);
        self.apply_cells.clear();
        self.apply_open = Some(intent);
        self.last_action = Some("apply_panel");
    }

    /// Back: the question is asked again on the next save.
    pub(crate) fn close_apply_panel(&mut self) {
        self.apply_open = None;
        self.apply_cells.clear();
        self.credit.reset_answer();
    }

    /// The panel's Save: "Apply" with the rows as ticked, or "Keep".
    pub(crate) fn submit_apply_panel(&mut self, keep: bool) {
        let Some(intent) = self.apply_open else {
            return;
        };
        if keep {
            self.credit.choose_keep();
        } else {
            self.commit_apply_cells();
        }
        self.start_save(intent, None);
    }

    /// The typed amounts become the owner's rows (a blank cell goes back to
    /// the engine's suggestion).
    fn commit_apply_cells(&mut self) {
        let cells = std::mem::take(&mut self.apply_cells);
        let caps = self.caps();
        self.credit.doc = self.compose();
        let suggested: Vec<(TransactionId, String)> = self
            .credit
            .figures(caps)
            .map(|f| {
                f.suggested
                    .iter()
                    .map(|s| (s.invoice, s.amount.format(self.money_style)))
                    .collect()
            })
            .unwrap_or_default();
        let ticked: Vec<TransactionId> = match self.credit.after_save() {
            AfterSave::Apply(rows) => rows.iter().map(|r| r.invoice).collect(),
            _ => Vec::new(),
        };
        for (invoice, text) in cells {
            // Only ticked rows: an unticked invoice's cell is ignored.
            if !ticked.contains(&invoice) {
                continue;
            }
            let text = text.trim();
            let same = suggested.iter().any(|(i, s)| *i == invoice && s == text);
            if text.is_empty() || same {
                self.credit.set_row(invoice, Some(ApplyAmount::Fill));
            } else if let Ok(m) = Money::parse_user(text, UserInput::MONEY) {
                self.credit.set_row(invoice, Some(ApplyAmount::Typed(m)));
            }
        }
    }

    /// Ticks or unticks an invoice in the apply panel.
    pub(crate) fn toggle_apply_row(&mut self, invoice: TransactionId, on: bool) {
        self.apply_cells.remove(&invoice);
        self.credit
            .set_row(invoice, on.then_some(ApplyAmount::Fill));
    }

    /// The apply panel (modal over the form).
    #[allow(clippy::too_many_lines)]
    pub(crate) fn apply_window(&mut self, ctx: &egui::Context, tokens: &Tokens) {
        if self.apply_open.is_none() {
            return;
        }
        let style = self.money_style;
        let caps = self.caps();
        self.credit.doc = self.compose();
        let figures = self.credit.figures(caps).cloned();
        let rows = match self.credit.after_save() {
            AfterSave::Apply(r) => r.clone(),
            _ => Vec::new(),
        };
        let keep = matches!(self.credit.after_save(), AfterSave::KeepCredit);
        let mut choose: Option<bool> = None;
        let mut toggles: Vec<(TransactionId, bool)> = Vec::new();
        let mut save = false;
        let mut back = false;
        let width = (ctx.content_rect().width() - 48.0).clamp(320.0, 620.0);
        let focus_first = std::mem::take(&mut self.dialog_focus);
        egui::Window::new(t("credit_memo.apply.title"))
            .id(egui::Id::unique("credit_memo/apply"))
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.set_width(width);
                ui.label(
                    RichText::new(t("credit_memo.apply.explain")).color(tokens.text_secondary),
                );
                ui.add_space(6.0);
                let r = ui.radio(!keep, t("credit_memo.apply.apply"));
                if focus_first {
                    r.request_focus();
                }
                if r.clicked() {
                    choose = Some(false);
                }
                if ui.radio(keep, t("credit_memo.apply.keep")).clicked() {
                    choose = Some(true);
                }
                ui.add_space(6.0);
                let Some(f) = figures.as_ref() else {
                    ui.label(t("credit_memo.problem.updating"));
                    return;
                };
                if keep {
                    ui.label(t_with(
                        "credit_memo.apply.kept",
                        &[("amount", &f.total.format(style))],
                    ));
                } else {
                    egui::Grid::new("credit_memo/apply/rows")
                        .num_columns(4)
                        .spacing([12.0, 6.0])
                        .show(ui, |ui| {
                            ui.label(RichText::new(t("credit_memo.apply.invoice")).strong());
                            ui.label(RichText::new(t("credit_memo.apply.due")).strong());
                            ui.label(RichText::new(t("credit_memo.apply.balance")).strong());
                            ui.label(RichText::new(t("credit_memo.apply.amount")).strong());
                            ui.end_row();
                            for o in &f.open_invoices {
                                let row = rows.iter().find(|r| r.invoice == o.invoice);
                                let mut on = row.is_some();
                                let name =
                                    t_with("credit_memo.apply.row", &[("number", &o.number)]);
                                if ui
                                    .checkbox(&mut on, &o.number)
                                    .on_hover_text(name)
                                    .changed()
                                {
                                    toggles.push((o.invoice, on));
                                }
                                ui.label(o.due.map(crate::shell::us_date).unwrap_or_default());
                                ui.label(RichText::new(o.balance_due.format(style)).monospace());
                                let shown = match row.map(|r| r.amount) {
                                    Some(ApplyAmount::Typed(m)) => m.format(style),
                                    Some(ApplyAmount::Fill) => f
                                        .suggested
                                        .iter()
                                        .find(|s| s.invoice == o.invoice)
                                        .map(|s| s.amount.format(style))
                                        .unwrap_or_default(),
                                    None => String::new(),
                                };
                                let text = self
                                    .apply_cells
                                    .entry(o.invoice)
                                    .or_insert_with(|| shown.clone());
                                let r = ui.add_enabled(
                                    row.is_some(),
                                    egui::TextEdit::singleline(text)
                                        .desired_width(110.0)
                                        .font(egui::TextStyle::Monospace)
                                        .align(egui::Align2::RIGHT_CENTER),
                                );
                                let r = r.on_hover_text(t_with(
                                    "credit_memo.apply.amount_on",
                                    &[("number", &o.number)],
                                ));
                                r.widget_info(|| {
                                    egui::WidgetInfo::labeled(
                                        egui::Role::TextInput,
                                        true,
                                        t_with(
                                            "credit_memo.apply.amount_on",
                                            &[("number", &o.number)],
                                        ),
                                    )
                                });
                                ui.end_row();
                            }
                        });
                    // The engine's remaining credit answers its own
                    // suggestion; once an amount is typed it is the
                    // engine's to work out on save.
                    let all_fill = rows.iter().all(|r| r.amount == ApplyAmount::Fill)
                        && self.apply_cells.iter().all(|(id, text)| {
                            let s = f.suggested.iter().find(|s| s.invoice == *id);
                            s.map(|s| s.amount.format(style)).unwrap_or_default() == *text
                                || !rows.iter().any(|r| r.invoice == *id)
                        });
                    let left = if all_fill {
                        f.remaining_credit.format(style)
                    } else {
                        t("credit_memo.apply.on_save")
                    };
                    ui.add_space(4.0);
                    ui.label(t_with("credit_memo.apply.remaining", &[("amount", &left)]));
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    save = ui.button(t("credit_memo.apply.save")).clicked();
                    back = ui.button(t("credit_memo.apply.back")).clicked();
                });
            });
        match choose {
            Some(true) => self.credit.choose_keep(),
            Some(false) => {
                self.credit.choose_apply(caps);
                self.apply_cells.clear();
            }
            None => {}
        }
        for (invoice, on) in toggles {
            self.toggle_apply_row(invoice, on);
        }
        if back {
            self.close_apply_panel();
        } else if save {
            let keep = matches!(self.credit.after_save(), AfterSave::KeepCredit);
            self.submit_apply_panel(keep);
        }
    }

    /// A sales receipt's Deposit to, method and reference (labels above,
    /// Q-P20h: Payments Clearing unless the owner picks a bank).
    pub(crate) fn payment_fields_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let accounts: Vec<(Option<cl_core::ids::AccountId>, String)> =
            std::iter::once((None, t("payment.deposit_to.clearing")))
                .chain(
                    self.books
                        .accounts
                        .iter()
                        .filter(|a| a.is_active && a.fields.detail_type.as_deref() == Some("bank"))
                        .map(|a| (Some(a.id), crate::screens::register::account_text(a))),
                )
                .collect();
        let current = self.header.current().deposit_to;
        let problem = self
            .problems
            .contains(&cl_model::forms::invoice::InvoiceProblem::NegativeReceiptNeedsBank);
        kit::wrap_item(ui, "sales_receipt/deposit_to", |ui| {
            ui.vertical(|ui| {
                let l = kit::above(ui, tokens, &t("sales_receipt.deposit_to"));
                let shown = accounts
                    .iter()
                    .find(|(id, _)| *id == current)
                    .map(|(_, n)| n.clone())
                    .unwrap_or_default();
                let mut pick = None;
                egui::ComboBox::from_id_salt("sales_receipt/deposit_to")
                    .selected_text(shown)
                    .width(220.0)
                    .show_ui(ui, |ui| {
                        for (id, name) in &accounts {
                            if ui.selectable_label(*id == current, name).clicked() {
                                pick = Some(*id);
                            }
                        }
                    })
                    .response
                    .labelled_by(l.id)
                    .on_hover_text(t("sales_receipt.deposit_to.tip"));
                if problem {
                    ui.label(
                        RichText::new(t("sales_receipt.problem.needs_bank"))
                            .size(12.0)
                            .color(tokens.warn_text),
                    );
                }
                if let Some(p) = pick {
                    self.header.edit_step(|h| h.deposit_to = p);
                }
            });
        });
        let method = self.header.current().payment_method.clone();
        kit::wrap_item(ui, "sales_receipt/method", |ui| {
            ui.vertical(|ui| {
                let l = kit::above(ui, tokens, &t("sales_receipt.method"));
                let shown = method
                    .as_deref()
                    .map_or_else(String::new, |m| t(&format!("sales_receipt.method.{m}")));
                let mut pick = None;
                egui::ComboBox::from_id_salt("sales_receipt/method")
                    .selected_text(shown)
                    .width(140.0)
                    .show_ui(ui, |ui| {
                        for m in RECEIPT_METHODS {
                            let on = method.as_deref() == Some(m);
                            if ui
                                .selectable_label(on, t(&format!("sales_receipt.method.{m}")))
                                .clicked()
                            {
                                pick = Some(m.to_string());
                            }
                        }
                    })
                    .response
                    .labelled_by(l.id);
                if let Some(p) = pick {
                    self.header.edit_step(|h| h.payment_method = Some(p));
                }
            });
        });
        kit::wrap_item(ui, "sales_receipt/ref", |ui| {
            ui.vertical(|ui| {
                let label = if method.as_deref() == Some("check") {
                    t("sales_receipt.check_no")
                } else {
                    t("sales_receipt.ref_no")
                };
                let mut text = self.header.current().ref_no.clone();
                let r = kit::text_field(
                    ui,
                    tokens,
                    &label,
                    &mut text,
                    120.0,
                    cl_model::forms::invoice::REF_MAX,
                );
                if r.changed() {
                    self.header.edit(super::field::REF, |h| h.ref_no = text);
                    self.inputs.rev = self.header.revision();
                }
                kit::track(&mut self.focused, &r, super::field::REF);
            });
        });
    }

    /// The saved memo's Applied to / Refunded / Remaining (engine strings).
    pub(crate) fn memo_history_card(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let Some(m) = self
            .editing
            .as_ref()
            .and_then(super::Editing::memo)
            .cloned()
        else {
            return;
        };
        let style = self.money_style;
        kit::panel_card(ui, tokens, &t("credit_memo.history.title"), |ui| {
            let applied: Vec<_> = m.live_applications().collect();
            if applied.is_empty() {
                ui.label(
                    RichText::new(t("credit_memo.history.none_applied"))
                        .color(tokens.text_secondary),
                );
            }
            for a in applied {
                kit::balance_line(
                    ui,
                    tokens,
                    &t_with(
                        "credit_memo.history.applied_to",
                        &[("number", a.invoice_number.as_deref().unwrap_or_default())],
                    ),
                    Some(a.amount.format(style)),
                );
            }
            for r in m.live_refunds() {
                kit::balance_line(
                    ui,
                    tokens,
                    &t_with(
                        "credit_memo.history.refunded",
                        &[("date", &crate::shell::us_date(r.date))],
                    ),
                    Some(r.amount.format(style)),
                );
            }
            ui.separator();
            kit::balance_line(
                ui,
                tokens,
                &t("credit_memo.history.remaining"),
                Some(m.remaining.format(style)),
            );
        });
    }

    /// Refund (the saved memo's remaining credit): Refund a Credit opens
    /// with this memo as the source (P22 D4/D5).
    pub(crate) fn refund_memo(&mut self) {
        let Some(m) = self
            .editing
            .as_ref()
            .and_then(super::Editing::memo)
            .cloned()
        else {
            self.notice = Some("credit_memo.refund.save_first");
            return;
        };
        if m.is_void() || !m.remaining.is_positive() {
            self.notice = Some("credit_memo.refund.nothing_left");
            return;
        }
        let source = cl_model::forms::refund::RefundSourceChoice {
            id: m.id,
            kind: cl_model::forms::refund::RefundSourceKind::CreditMemo,
            remaining: m.remaining,
        };
        self.link.push(CompanyEvent::OpenWith(
            cl_model::CommandId::new(crate::screens::refund::KIND),
            Box::new(crate::windows::OpenPayload::RefundFrom(
                m.customer_id,
                source,
            )),
        ));
        self.last_action = Some("refund");
    }

    /// Apply credit (a saved memo with credit left): Receive Payment for
    /// the client, where Discounts & credits applies it to invoices.
    pub(crate) fn apply_credit_later(&mut self) {
        let Some(m) = self
            .editing
            .as_ref()
            .and_then(super::Editing::memo)
            .cloned()
        else {
            self.notice = Some("credit_memo.refund.save_first");
            return;
        };
        if m.is_void() || !m.remaining.is_positive() {
            self.notice = Some("credit_memo.refund.nothing_left");
            return;
        }
        self.link.push(CompanyEvent::OpenWith(
            cl_model::CommandId::new(crate::screens::receive_payment::KIND),
            Box::new(crate::windows::OpenPayload::ReceiveFrom(m.customer_id)),
        ));
        self.last_action = Some("apply_credit");
    }

    /// A refusal of a credit memo or sales receipt save in plain words
    /// (`None`: not one of theirs).
    pub(crate) fn doc_refusal(&mut self, e: &ApiError) -> Option<&'static str> {
        Some(match credit_refusal(e)? {
            CreditRefusal::Stale => {
                self.stale = true;
                return Some("credit_memo.refusal.stale");
            }
            CreditRefusal::InvoiceBalanceChanged => {
                // The open invoices are read again with the next preview.
                self.credit.reset_answer();
                self.tracker = cl_model::forms::invoice::PreviewTracker::new();
                "credit_memo.refusal.balance_changed"
            }
            CreditRefusal::Overapplied | CreditRefusal::TooManyApplications => {
                "credit_memo.refusal.overapplied"
            }
            CreditRefusal::InvoiceNotForCustomer => "credit_memo.refusal.not_for_customer",
            CreditRefusal::CreditNotAvailable => "credit_memo.refusal.credit_not_available",
            CreditRefusal::CreditInUse => "credit_memo.refusal.credit_in_use",
            CreditRefusal::CreditRefunded => "credit_memo.refusal.credit_refunded",
            CreditRefusal::ApplyDateBeforeSource => "credit_memo.refusal.apply_date",
            CreditRefusal::SourceDateAfterUse => "credit_memo.refusal.source_date",
            CreditRefusal::TotalNotPositive => "credit_memo.problem.total_not_positive",
            CreditRefusal::NegativeReceiptNeedsBank => "sales_receipt.problem.needs_bank",
            CreditRefusal::Deposited => "sales_receipt.refusal.deposited",
            CreditRefusal::NumberInUse => "credit_memo.refusal.number_in_use",
        })
    }

    /// The control channel's view of the credit memo's state (flags,
    /// counts and the engine's figures as shown; no names).
    pub(crate) fn credit_state_json(&self) -> Value {
        let style = cl_core::MoneyStyle::default();
        let caps = self.caps();
        let mut c = self.credit.clone();
        c.doc = self.compose();
        let figures = c.figures(caps);
        let rows: Vec<Value> = match c.after_save() {
            AfterSave::Apply(rows) => rows
                .iter()
                .map(|r| {
                    let number = figures
                        .and_then(|f| f.open_invoices.iter().find(|o| o.invoice == r.invoice))
                        .map(|o| o.number.clone());
                    let amount = match r.amount {
                        ApplyAmount::Typed(m) => Some(m.format(style)),
                        ApplyAmount::Fill => figures
                            .and_then(|f| f.suggested.iter().find(|s| s.invoice == r.invoice))
                            .map(|s| s.amount.format(style)),
                    };
                    json!({"number": number, "amount": amount})
                })
                .collect(),
            _ => Vec::new(),
        };
        let saved = self.editing.as_ref().and_then(super::Editing::memo);
        json!({
            "panel_open": self.apply_open.is_some(),
            "answer": match c.after_save() {
                AfterSave::Unanswered => "unanswered",
                AfterSave::Apply(_) => "apply",
                AfterSave::KeepCredit => "keep",
            },
            "open_invoices": figures.map(|f| f.open_invoices.len()),
            "suggested": figures.map(|f| f.suggested.iter().map(|s| s.amount.format(style)).collect::<Vec<_>>()),
            "remaining_credit": figures.map(|f| f.remaining_credit.format(style)),
            "apply_rows": rows,
            "saved_applied": saved.map(|m| m.live_applications().map(|a| a.amount.format(style)).collect::<Vec<_>>()),
            "saved_refunded": saved.map(|m| m.live_refunds().map(|r| r.amount.format(style)).collect::<Vec<_>>()),
            "saved_remaining": saved.map(|m| m.remaining.format(style)),
        })
    }

    /// The saved memo or receipt as its printed form (every figure the
    /// engine's, parsed for display only).
    pub(crate) fn doc_print(&self) -> Option<(String, cl_print::layout::Document)> {
        let e = self.editing.as_ref()?;
        let client = e
            .client()
            .and_then(|c| self.books.contacts.iter().find(|x| x.id == c));
        let address = client
            .map(|c| {
                let (bill, _) = super::draw::addresses(&c.fields);
                std::iter::once(c.fields.display_name.clone())
                    .chain(bill)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let item = |l: &cl_api::SalesDocLine| cl_print::forms::ItemLine {
            description: l.description.clone().unwrap_or_default(),
            quantity: l.quantity.map(Quantity::as_decimal),
            rate: l.unit_price.map(Money::as_decimal),
            amount: Some(l.amount.as_decimal()),
            service_date: l
                .service_date
                .map(crate::shell::us_date)
                .unwrap_or_default(),
        };
        let style = super::email::pdf_style(
            &self
                .tenant
                .as_ref()
                .map(|(_, n)| n.clone())
                .unwrap_or_default(),
        );
        let tax = |m: Money| (!m.is_zero()).then(|| m.as_decimal());
        match &e.doc {
            SavedDoc::CreditMemo(m) => {
                let d = cl_print::forms::credit_memo::CreditMemoData {
                    number: m.number.clone().unwrap_or_default(),
                    date: crate::shell::us_date(m.date),
                    customer: address,
                    lines: m.lines.iter().map(item).collect(),
                    hide_zero_lines: false,
                    subtotal: m.subtotal.as_decimal(),
                    tax: tax(m.tax_amount),
                    total: m.total.as_decimal(),
                    remaining: Some(m.remaining.as_decimal()),
                    message: m.customer_message.clone().unwrap_or_default(),
                };
                let doc = cl_print::forms::credit_memo::layout(&d, &style).ok()?;
                Some((format!("CreditMemo-{}.pdf", d.number), doc))
            }
            SavedDoc::SalesReceipt(r) => {
                let d = cl_print::forms::sales_receipt::SalesReceiptData {
                    number: r.number.clone().unwrap_or_default(),
                    date: crate::shell::us_date(r.date),
                    payment_method: r
                        .payment_method
                        .map(|m| t(&format!("sales_receipt.method.{}", method_wire(m))))
                        .unwrap_or_default(),
                    reference: r.ref_no.clone().unwrap_or_default(),
                    sold_to: address,
                    lines: r.lines.iter().map(item).collect(),
                    hide_zero_lines: false,
                    subtotal: r.subtotal.as_decimal(),
                    tax: tax(r.tax_amount),
                    total: r.total.as_decimal(),
                    message: r.customer_message.clone().unwrap_or_default(),
                };
                let doc = cl_print::forms::sales_receipt::layout(&d, &style).ok()?;
                Some((format!("SalesReceipt-{}.pdf", d.number), doc))
            }
            SavedDoc::Invoice(_) => None,
        }
    }

    /// Makes the saved memo's or receipt's PDF on the queue; it opens full
    /// size when done (S10: never in a frame).
    pub(crate) fn open_doc_pdf(&mut self) {
        let Some((name, doc)) = self.doc_print() else {
            self.notice = Some("invoice.email.pdf_failed");
            return;
        };
        let doc = Arc::new(doc);
        let render = move || {
            cl_print::render(&doc)
                .map(|b| (name.clone(), b))
                .map_err(|_| ApiError::Local(cl_api::LocalValidation::InvalidValue))
        };
        let r2 = render.clone();
        self.pdf_job = Some(engine::read(
            &self.link,
            self.group.as_ref(),
            move |_| r2(),
            move |_| async move { render() },
        ));
        self.poll_pdf();
    }
}
