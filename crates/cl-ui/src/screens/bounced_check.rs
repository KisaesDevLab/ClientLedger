//! Record bounced check (P21 D8, Q-82, Q-P21d/i/p/u): a guided dialog on
//! Receive Payment's saved payment. Step 1 says what happens in plain
//! words; step 2 takes the bank's fee and an optional fee invoice to the
//! client; the result lists the reopened invoices with the engine's
//! balances. The bounce (one dated "Bounced payment" entry with the bank
//! fee) and the fee invoice are two calls with their own keys; a retry
//! after the bounce sends only the fee invoice
//! (`cl_model::forms::bounced_check`).

use cl_api::{BounceRequest, IdempotencyKey, NewInvoice};
use cl_core::Secret;
use cl_model::closing::needs_password;
use cl_model::forms::bounced_check::{BounceCall, BounceOutcome, BounceProblem, BounceStep};
use eframe::egui::{self, RichText, Ui};

use crate::forms::closing_prompt::Verb;
use crate::i18n::{t, t_with};
use crate::screens::receive_payment::engine;
use crate::screens::receive_payment::{Pending, ReceivePaymentScreen, shown};
use crate::theme::Tokens;

fn problem_key(p: BounceProblem) -> &'static str {
    match p {
        BounceProblem::NoDate => "bounce.problem.date",
        BounceProblem::BankFee => "bounce.problem.bank_fee",
        BounceProblem::CustomerFee => "bounce.problem.customer_fee",
        BounceProblem::FeeBank => "bounce.problem.fee_bank",
        BounceProblem::FeeIncomeAccount => "bounce.problem.fee_income",
    }
}

/// Sends the flow's next call.
pub(crate) fn send(s: &mut ReceivePaymentScreen, password: Option<Secret>) -> bool {
    let (Some(flow), Some(rec)) = (s.bounce.as_ref(), s.editing.clone()) else {
        return false;
    };
    if s.bounce_job.is_some() || s.fee_job.is_some() {
        return false;
    }
    let call = match flow.next_call() {
        Ok(c) => c,
        Err(p) => {
            s.notice = Some(problem_key(p));
            return false;
        }
    };
    s.notice = None;
    match call {
        BounceCall::Bounce(b) => {
            if password.is_none() && needs_password(None, b.date, s.books.closing) {
                s.closing_prompt.set_verb(Verb::Change);
                s.closing_prompt.ask(Pending::Bounce, true, s.books.closing);
                return false;
            }
            let req = BounceRequest {
                date: b.date,
                bank_fee_amount: b.bank_fee,
                bank_fee_account_id: b.bank_fee_account,
                bank_fee_bank_account_id: b.fee_bank_account,
                memo: None,
                expected_updated_at: rec.updated_at,
            };
            let key = s.bounce_keys.0.clone();
            let demo_key = key.to_string();
            let demo_req = req.clone();
            let pw = password.clone();
            let id = b.payment;
            s.bounce_job = Some(engine::write(
                &s.link,
                s.group.as_ref(),
                engine::label(&key),
                move |d| {
                    crate::company::demo::receivables::bounce(
                        d,
                        id,
                        &demo_req,
                        pw.as_ref(),
                        Some(&demo_key),
                    )
                },
                move |x| async move {
                    x.customer_payments()
                        .bounce(id, &req, &engine::options(password), key)
                        .await
                },
            ));
        }
        BounceCall::FeeInvoice(f) => {
            // Q-P21p: one non-taxable line to the fee income account.
            let one = cl_core::Quantity::parse_wire("1").unwrap_or_default();
            let mut line = cl_api::InvoiceLine::account(f.income_account, one, f.amount);
            line.is_taxable = Some(false);
            line.description = Some(t("bounce.fee_invoice.line"));
            let new = NewInvoice::new(rec.customer_id, f.date, vec![line]);
            let key: IdempotencyKey = s.bounce_keys.1.clone();
            let demo_new = new.clone();
            s.fee_job = Some(engine::write(
                &s.link,
                s.group.as_ref(),
                engine::label(&key),
                move |d| {
                    crate::company::demo::invoices::create(d, &demo_new, None)
                        .map(|v| v.record.txn.id)
                },
                move |x| async move {
                    x.invoices()
                        .create(&new, &engine::options(None), key)
                        .await
                        .map(|v| v.invoice.record.txn.id)
                },
            ));
        }
        BounceCall::Nothing => return false,
    }
    poll(s);
    true
}

/// Takes the bounce's and the fee invoice's answers.
pub(crate) fn poll(s: &mut ReceivePaymentScreen) {
    if let Some(r) = engine::poll(&mut s.bounce_job) {
        match r {
            Ok(out) => {
                s.closing_prompt.saved();
                let clearing = s.clearing();
                let outcome = BounceOutcome {
                    return_id: out.return_id,
                    fee_id: out.fee_id,
                    held_in: out.held_in.account_id,
                    undeposited: Some(out.held_in.account_id) == clearing,
                    reopened: out
                        .reopened
                        .iter()
                        .map(|r| (r.invoice_id, r.balance_due))
                        .collect(),
                };
                let accounts = [Some(out.held_in.account_id)]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>();
                crate::forms::publish_saved(&s.link, &s.bus, &accounts);
                if let Some(f) = s.bounce.as_mut() {
                    f.bounced(outcome);
                }
                // The fee invoice, if one was asked for, goes next.
                send(s, None);
            }
            Err(e) => {
                if !s
                    .closing_prompt
                    .failed(&e, Pending::Bounce, true, s.books.closing)
                {
                    s.notice = Some(match cl_api::payment_refusal(&e) {
                        Some(cl_api::PaymentRefusal::Bounced) => "payment.refusal.bounced",
                        Some(cl_api::PaymentRefusal::Refunded) => "payment.refusal.refunded",
                        Some(cl_api::PaymentRefusal::BounceDateInvalid) => {
                            "bounce.problem.date_early"
                        }
                        Some(
                            cl_api::PaymentRefusal::FeeAccountRequired
                            | cl_api::PaymentRefusal::FeeAccountInvalid,
                        ) => "bounce.problem.fee_account",
                        Some(cl_api::PaymentRefusal::FeeBankRequired) => "bounce.problem.fee_bank",
                        _ => "payment.refusal.other",
                    });
                }
            }
        }
    }
    if let Some(r) = engine::poll(&mut s.fee_job) {
        match r {
            Ok(id) => {
                if let Some(f) = s.bounce.as_mut() {
                    f.fee_invoiced(id);
                }
            }
            Err(_) => {
                if let Some(f) = s.bounce.as_mut() {
                    f.fee_failed();
                }
            }
        }
    }
    // The payment, read again, is bounced and read-only.
    if s.bounce
        .as_ref()
        .is_some_and(|f| f.step == BounceStep::Done && f.bounce.is_some())
        && s.editing.as_ref().is_some_and(|e| !e.is_bounced())
        && s.open_job.is_none()
        && let Some(id) = s.editing.as_ref().map(|e| e.id)
    {
        s.open_id(id);
    }
}

/// The dialog.
#[allow(clippy::too_many_lines)]
pub(crate) fn draw(s: &mut ReceivePaymentScreen, ui: &mut Ui, tokens: &Tokens) {
    let Some(rec) = s.editing.clone() else {
        return;
    };
    let accounts_expense: Vec<(cl_core::ids::AccountId, String)> = s
        .accounts_of(&[cl_api::AccountType::Expense])
        .iter()
        .map(|a| (a.id, crate::screens::register::account_text(a)))
        .collect();
    let accounts_income: Vec<(cl_core::ids::AccountId, String)> = s
        .accounts_of(&[cl_api::AccountType::Revenue])
        .iter()
        .map(|a| (a.id, crate::screens::register::account_text(a)))
        .collect();
    let banks: Vec<(cl_core::ids::AccountId, String)> = s
        .deposit_accounts()
        .iter()
        .filter(|a| a.fields.detail_type.as_deref() == Some("bank"))
        .map(|a| (a.id, crate::screens::register::account_text(a)))
        .collect();
    let held_name = match rec.deposited_in {
        Some(_) => t("bounce.held.deposit_bank"),
        None => s.account_name(rec.deposit_to),
    };
    let numbers: Vec<String> = rec
        .live_applications()
        .map(|a| a.invoice_number.clone())
        .collect();
    let names: Vec<(cl_core::ids::AccountId, String)> = s
        .books
        .accounts
        .iter()
        .map(|a| (a.id, a.fields.name.clone()))
        .collect();
    let mut send_now = false;
    let mut close = false;
    let Some(flow) = s.bounce.as_mut() else {
        return;
    };
    let pick = |ui: &mut Ui,
                label: &str,
                salt: &str,
                v: &mut Option<cl_core::ids::AccountId>,
                list: &[(cl_core::ids::AccountId, String)]| {
        ui.horizontal(|ui| {
            let l = ui.label(label);
            let current = v
                .and_then(|id| list.iter().find(|(a, _)| *a == id))
                .map_or_else(|| t("payment.account.choose"), |(_, n)| n.clone());
            egui::ComboBox::from_id_salt(salt)
                .selected_text(current)
                .width(260.0)
                .show_ui(ui, |ui| {
                    for (id, name) in list {
                        ui.selectable_value(v, Some(*id), name);
                    }
                })
                .response
                .labelled_by(l.id);
        });
    };
    ui.group(|ui| {
        ui.label(RichText::new(t("bounce.title")).strong().size(16.0));
        match flow.step {
            BounceStep::Confirm => {
                let undeposited = flow.never_deposited;
                let text = t_with(
                    if undeposited {
                        "bounce.explain.undeposited"
                    } else {
                        "bounce.explain"
                    },
                    &[
                        ("amount", &shown(rec.amount)),
                        ("account", &held_name),
                        ("invoices", &numbers.join(", ")),
                    ],
                );
                ui.label(text);
                let mut text = flow
                    .bounce_date
                    .map(crate::shell::us_date)
                    .unwrap_or_default();
                ui.horizontal(|ui| {
                    let l = ui.label(t("bounce.date"));
                    let r = ui
                        .add_sized([120.0, 24.0], egui::TextEdit::singleline(&mut text))
                        .labelled_by(l.id);
                    if r.changed() {
                        flow.bounce_date = cl_model::date_entry::parse(
                            &text,
                            cl_model::date_entry::DateOrder::Mdy,
                            s.today,
                        )
                        .ok();
                    }
                });
                ui.horizontal(|ui| {
                    if ui.button(t("bounce.next")).clicked() && flow.next_step().is_err() {
                        s.notice = Some("bounce.problem.date");
                    }
                    if ui.button(t("bounce.cancel")).clicked() {
                        close = true;
                    }
                });
            }
            BounceStep::Fees => {
                ui.label(RichText::new(t("bounce.fees.bank")).strong());
                ui.horizontal(|ui| {
                    let l = ui.label(t("bounce.bank_fee"));
                    ui.add_sized(
                        [120.0, 24.0],
                        egui::TextEdit::singleline(&mut flow.bank_fee),
                    )
                    .labelled_by(l.id);
                });
                pick(
                    ui,
                    &t("bounce.bank_fee_account"),
                    "bounce-fee-acct",
                    &mut flow.bank_fee_account,
                    &accounts_expense,
                );
                if flow.never_deposited {
                    pick(
                        ui,
                        &t("bounce.fee_bank"),
                        "bounce-fee-bank",
                        &mut flow.fee_bank_account,
                        &banks,
                    );
                }
                ui.separator();
                ui.label(RichText::new(t("bounce.fees.client")).strong());
                ui.horizontal(|ui| {
                    let l = ui.label(t("bounce.customer_fee"));
                    ui.add_sized(
                        [120.0, 24.0],
                        egui::TextEdit::singleline(&mut flow.customer_fee),
                    )
                    .labelled_by(l.id);
                });
                pick(
                    ui,
                    &t("bounce.fee_income"),
                    "bounce-fee-income",
                    &mut flow.fee_income_account,
                    &accounts_income,
                );
                ui.horizontal(|ui| {
                    if ui.button(t("bounce.record")).clicked() {
                        send_now = true;
                    }
                    if flow.bounce.is_none() && ui.button(t("bounce.back")).clicked() {
                        flow.back();
                    }
                });
            }
            BounceStep::Done => {
                if let Some(o) = &flow.bounce {
                    ui.label(t("bounce.done"));
                    for (inv, bal) in &o.reopened {
                        let n = rec
                            .applications
                            .iter()
                            .find(|a| a.invoice_id == *inv)
                            .map(|a| a.invoice_number.clone())
                            .unwrap_or_default();
                        ui.label(t_with(
                            "bounce.reopened",
                            &[("number", &n), ("balance", &shown(*bal))],
                        ));
                    }
                    ui.label(t_with(
                        "bounce.charged",
                        &[(
                            "account",
                            &names
                                .iter()
                                .find(|(a, _)| *a == o.held_in)
                                .map(|(_, n)| n.clone())
                                .unwrap_or_default(),
                        )],
                    ));
                }
                if flow.fee_invoice.is_some() {
                    ui.label(t("bounce.fee_invoice.done"));
                }
                if flow.fee_failed {
                    ui.label(RichText::new(t("bounce.fee_invoice.failed")).color(tokens.warn_text));
                    if ui.button(t("bounce.try_again")).clicked() {
                        send_now = true;
                    }
                }
                if ui.button(t("bounce.close")).clicked() {
                    close = true;
                }
            }
        }
    });
    if send_now {
        s.send_bounce(None);
    }
    if close {
        s.dialog = None;
        if s.bounce.as_ref().is_none_or(|f| f.bounce.is_none()) {
            s.bounce = None;
        }
    }
}
