//! Begin (step 1): the statement's figures, the unfinished one, Undo and
//! the discrepancy report.

use cl_core::ids::AccountId;
use cl_model::reconcile::{ReconKind, display};
use eframe::egui::{self, RichText, Ui};

use super::{Dialog, ReconcileScreen};
use crate::i18n::{t, t_with};
use crate::screens::company::form;
use crate::theme::Tokens;

/// A combo box of accounts, labelled for the control channel.
fn account_box(
    ui: &mut Ui,
    label: &str,
    choices: &[(AccountId, String)],
    value: &mut Option<AccountId>,
    enabled: bool,
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        let l = ui.add(egui::Label::new(label).selectable(false));
        let shown = value
            .and_then(|v| choices.iter().find(|(id, _)| *id == v))
            .map_or_else(|| t("reconcile.begin.pick"), |(_, n)| n.clone());
        ui.add_enabled_ui(enabled, |ui| {
            egui::ComboBox::from_id_salt(label)
                .selected_text(shown)
                .width(300.0)
                .show_ui(ui, |ui| {
                    for (id, name) in choices {
                        if ui.selectable_label(*value == Some(*id), name).clicked() {
                            *value = Some(*id);
                            changed = true;
                        }
                    }
                })
                .response
                .labelled_by(l.id);
        });
    });
    changed
}

fn money_text(ui: &mut Ui, label: &str, text: &mut String, enabled: bool) {
    ui.horizontal(|ui| {
        let l = ui.add(egui::Label::new(label).selectable(false));
        let r = ui
            .add_enabled(
                enabled,
                egui::TextEdit::singleline(text)
                    .desired_width(140.0)
                    .font(egui::TextStyle::Monospace)
                    .align(egui::Align2::RIGHT_CENTER),
            )
            .labelled_by(l.id);
        if r.lost_focus()
            && let Some(settled) = form::settled_money_text(text, cl_core::MoneyStyle::default())
        {
            *text = settled;
        }
    });
}

/// Draws Begin.
pub(crate) fn draw(s: &mut ReconcileScreen, ui: &mut Ui) {
    let tokens = Tokens::of(ui.ctx());
    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.add_space(8.0);
        let title = if s.editing {
            t("reconcile.begin.change_title")
        } else {
            t("reconcile.begin.title")
        };
        ui.heading(RichText::new(title).strong());
        ui.add_space(6.0);
        if !s.supported() {
            form::inline_warning(ui, &tokens, &t("reconcile.unsupported"));
            return;
        }
        if s.accounts_loaded && s.accounts.is_empty() {
            form::hint(ui, &tokens, &t("reconcile.begin.no_accounts"));
            if ui.link(t("reconcile.begin.chart_link")).clicked() {
                s.link.push(crate::company::CompanyEvent::OpenWindow(
                    cl_model::CommandId::new("lists.chart_of_accounts"),
                ));
            }
            return;
        }
        if let Some(n) = s.notice {
            form::hint(ui, &tokens, &t(n));
        }
        if let Some(e) = &s.error {
            form::inline_error(ui, &tokens, e);
        }
        form::card(ui, &tokens, |ui| fields(s, ui, &tokens));
        ui.add_space(10.0);
        buttons(s, ui, &tokens);
    });
}

fn fields(s: &mut ReconcileScreen, ui: &mut Ui, tokens: &Tokens) {
    let banks: Vec<(AccountId, String)> = s
        .accounts
        .iter()
        .map(|a| (a.id, crate::screens::register::account_text(a)))
        .collect();
    let mut pick = s.form.account;
    if account_box(
        ui,
        &t("reconcile.begin.account"),
        &banks,
        &mut pick,
        !s.editing,
    ) && let Some(a) = pick
    {
        s.pick_account(a);
    }
    let card = s.kind() == ReconKind::Card;
    let today = s.today;
    form::date_field(
        ui,
        &t("reconcile.begin.statement_date"),
        &mut s.date_in,
        today,
    );
    // The beginning balance is the engine's (read-only).
    let beginning = match s.last_finished() {
        Some(r) => r.statement_ending_balance.map_or_else(String::new, |m| {
            t_with(
                "reconcile.begin.beginning_from",
                &[
                    ("amount", &display(s.kind().shown(m))),
                    ("date", &crate::shell::us_date(r.statement_date)),
                ],
            )
        }),
        None if s.form.account.is_some() => t("reconcile.begin.first"),
        None => String::new(),
    };
    ui.horizontal(|ui| {
        ui.label(t("reconcile.begin.beginning"));
        ui.label(
            RichText::new(beginning)
                .monospace()
                .color(tokens.text_secondary),
        );
    });
    let ending = if card {
        t("reconcile.begin.ending_card")
    } else {
        t("reconcile.begin.ending")
    };
    money_text(ui, &ending, &mut s.form.ending, true);
    ui.add_space(6.0);
    let charges: Vec<(AccountId, String)> = s
        .chart
        .iter()
        .filter(|a| Some(a.id) != s.form.account)
        .map(|a| (a.id, crate::screens::register::account_text(a)))
        .collect();
    let posted_note = t("reconcile.begin.charge_posted");
    // Service charge (a card's finance charge).
    let (svc_label, svc_acct) = if card {
        (
            t("reconcile.begin.finance_charge"),
            t("reconcile.begin.finance_account"),
        )
    } else {
        (
            t("reconcile.begin.service_charge"),
            t("reconcile.begin.service_account"),
        )
    };
    let posted = s.form.service_charge.posted;
    ui.horizontal_wrapped(|ui| {
        money_text(ui, &svc_label, &mut s.form.service_charge.amount, !posted);
        form::date_field(
            ui,
            &t("reconcile.begin.service_date"),
            &mut s.svc_date,
            today,
        );
        account_box(
            ui,
            &svc_acct,
            &charges,
            &mut s.form.service_charge.account,
            !posted,
        );
    });
    if posted {
        form::hint(ui, tokens, &posted_note);
    }
    if !card {
        let posted = s.form.interest.posted;
        ui.horizontal_wrapped(|ui| {
            money_text(
                ui,
                &t("reconcile.begin.interest"),
                &mut s.form.interest.amount,
                !posted,
            );
            form::date_field(
                ui,
                &t("reconcile.begin.interest_date"),
                &mut s.int_date,
                today,
            );
            account_box(
                ui,
                &t("reconcile.begin.interest_account"),
                &charges,
                &mut s.form.interest.account,
                !posted,
            );
        });
        if posted {
            form::hint(ui, tokens, &posted_note);
        }
    }
    form::hint(ui, tokens, &t("reconcile.begin.charges_note"));
    for p in &s.problems {
        form::inline_warning(ui, tokens, &t(p.key()));
    }
}

fn buttons(s: &mut ReconcileScreen, ui: &mut Ui, tokens: &Tokens) {
    if let Some(open) = s.unfinished().copied()
        && !s.editing
    {
        form::hint(
            ui,
            tokens,
            &t_with(
                "reconcile.begin.unfinished",
                &[("date", &crate::shell::us_date(open.statement_date))],
            ),
        );
        ui.horizontal(|ui| {
            let started = open.started.map(super::day_text).unwrap_or_default();
            if ui
                .button(t_with("reconcile.begin.continue", &[("date", &started)]))
                .clicked()
            {
                s.continue_open();
            }
            if ui.button(t("reconcile.begin.start_over")).clicked() {
                s.dialog = Some(Dialog::StartOver);
            }
        });
    } else {
        ui.horizontal(|ui| {
            let primary = if s.editing {
                t("reconcile.begin.save_info")
            } else {
                t("reconcile.begin.continue_new")
            };
            if ui
                .add(
                    egui::Button::new(RichText::new(primary).color(tokens.on_accent))
                        .fill(tokens.accent),
                )
                .clicked()
            {
                s.begin();
            }
            if s.editing && ui.button(t("reconcile.begin.cancel_edit")).clicked() {
                s.editing = false;
                s.step = super::Step::Window;
            }
        });
    }
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        let has_last = s.last_finished().is_some();
        let undo = ui.add_enabled(
            has_last && !s.not_admin && !s.editing,
            egui::Button::new(t("reconcile.undo.button")),
        );
        let undo = if s.not_admin {
            undo.on_disabled_hover_text(t("reconcile.undo.admin_only"))
        } else {
            undo
        };
        if undo.clicked() {
            s.undo_mode = cl_api::UndoMode::KeepChecks;
            s.password.clear();
            s.dialog = Some(Dialog::Undo);
        }
        let disc = ui.add_enabled(
            s.discrepancy_supported(),
            egui::Button::new(t("reconcile.reports.discrepancy")),
        );
        let disc = disc.on_disabled_hover_text(t("reconcile.reports.needs_update"));
        if disc.clicked() {
            s.open_discrepancy();
        }
        if ui
            .add_enabled(has_last, egui::Button::new(t("reconcile.reports.previous")))
            .clicked()
        {
            s.dialog = Some(Dialog::Previous);
        }
    });
}
