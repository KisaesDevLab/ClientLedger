//! The dialogs: the closing password before Finish, the done dialog, a
//! card's pay step (Q-60, Q-P27g: a check now, a bill for later or a
//! transfer from a bank account), Start over, Undo ([`super::undo`]),
//! previous reports and the discrepancy report ([`super::reports`]).

use cl_model::CommandId;
use eframe::egui::{self, Ui};

use super::{Dialog, ReconcileScreen};
use crate::company::CompanyEvent;
use crate::i18n::t;
use crate::windows::OpenPayload;

fn dialog(ui: &Ui, title: &str, add: impl FnOnce(&mut Ui)) {
    egui::Window::new(title)
        .id(egui::Id::unique("reconcile_dialog").with(title))
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .collapsible(false)
        .resizable(false)
        .show(ui.ctx(), |ui| {
            ui.set_max_width(460.0);
            add(ui);
        });
}

/// Draws the dialog that is open, if any.
pub(crate) fn dialogs(s: &mut ReconcileScreen, ui: &Ui) {
    let Some(d) = s.dialog.clone() else {
        return;
    };
    match d {
        Dialog::ClosingPassword => closing(s, ui),
        Dialog::Done => done(s, ui),
        Dialog::CardPay => card_pay(s, ui),
        Dialog::StartOver => start_over(s, ui),
        Dialog::ChargeExists => charge_exists(s, ui),
        Dialog::Undo => super::undo::draw(s, ui),
        Dialog::Previous => super::reports::previous(s, ui),
        Dialog::Discrepancy => super::reports::discrepancy(s, ui),
    }
}

fn closing(s: &mut ReconcileScreen, ui: &Ui) {
    dialog(ui, &t("reconcile.closing.title"), |ui| {
        ui.add(egui::Label::new(t("reconcile.closing.text")).wrap());
        if s.password_rejected {
            ui.add_space(8.0);
            let tokens = crate::theme::Tokens::of(ui.ctx());
            crate::screens::company::form::inline_warning(
                ui,
                &tokens,
                &t("reconcile.closing.rejected"),
            );
        }
        ui.add_space(8.0);
        crate::screens::company::form::password(
            ui,
            &t("reconcile.closing.password"),
            &mut s.password,
        );
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if crate::widgets::primary_button(ui, &t("reconcile.closing.confirm")).clicked()
                && !s.password.is_empty()
            {
                s.finish();
            }
            if ui.button(t("reconcile.cancel")).clicked() {
                s.password.clear();
                s.password_rejected = false;
                s.dialog = None;
            }
        });
    });
}

fn done(s: &mut ReconcileScreen, ui: &Ui) {
    // Only a card with a balance owed goes on to the pay step.
    let card = s.card_pay.is_some();
    dialog(ui, &t("reconcile.done.title"), |ui| {
        ui.add(egui::Label::new(t("reconcile.done.text")).wrap());
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button(t("reconcile.reports.summary")).clicked() {
                s.reports.open(
                    &s.link,
                    s.group.as_ref(),
                    s.report_target(),
                    super::reports::Kind::Summary,
                );
            }
            if ui.button(t("reconcile.reports.detail")).clicked() {
                s.reports.open(
                    &s.link,
                    s.group.as_ref(),
                    s.report_target(),
                    super::reports::Kind::Detail,
                );
            }
            let next = if card {
                t("reconcile.done.next")
            } else {
                t("reconcile.done.close")
            };
            if crate::widgets::primary_button(ui, &next).clicked() {
                s.dialog = card.then_some(Dialog::CardPay);
            }
        });
    });
}

/// The card pay step's hand-off: the statement balance the engine held
/// when the reconciliation finished (never the Begin text, review D2).
pub(crate) fn pay(s: &mut ReconcileScreen, kind: &'static str) {
    let Some(p) = s.card_pay.clone() else {
        s.dialog = None;
        return;
    };
    s.link.push(CompanyEvent::OpenWith(
        CommandId::new(kind),
        Box::new(OpenPayload::CardPayment(p)),
    ));
    s.last = Some(match kind {
        "vendors.write_check" => "pay_check",
        "vendors.enter_bill" => "pay_bill",
        _ => "pay_transfer",
    });
    s.dialog = None;
}

fn card_pay(s: &mut ReconcileScreen, ui: &Ui) {
    dialog(ui, &t("reconcile.pay.title"), |ui| {
        ui.add(egui::Label::new(t("reconcile.pay.text")).wrap());
        ui.add_space(8.0);
        ui.vertical(|ui| {
            if ui.button(t("reconcile.pay.check")).clicked() {
                pay(s, "vendors.write_check");
            }
            if ui.button(t("reconcile.pay.bill")).clicked() {
                pay(s, "vendors.enter_bill");
            }
            if ui.button(t("reconcile.pay.transfer")).clicked() {
                pay(s, "banking.transfer");
            }
            if ui.button(t("reconcile.pay.not_now")).clicked() {
                s.last = Some("pay_later");
                s.dialog = None;
            }
        });
    });
}

/// Q-P27u: the statement's charge is already posted in the account.
fn charge_exists(s: &mut ReconcileScreen, ui: &Ui) {
    dialog(ui, &t("reconcile.charge_exists.title"), |ui| {
        ui.add(egui::Label::new(t("reconcile.charge_exists.text")).wrap());
        ui.add(egui::Label::new(t("reconcile.charge_exists.detail")).wrap());
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if crate::widgets::primary_button(ui, &t("reconcile.charge_exists.confirm")).clicked() {
                s.add_charge_again();
            }
            if ui.button(t("reconcile.charge_exists.back")).clicked() {
                s.dialog = None;
            }
        });
    });
}

fn start_over(s: &mut ReconcileScreen, ui: &Ui) {
    dialog(ui, &t("reconcile.start_over.title"), |ui| {
        ui.add(egui::Label::new(t("reconcile.start_over.text")).wrap());
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if crate::widgets::primary_button(ui, &t("reconcile.start_over.confirm")).clicked() {
                s.start_over();
            }
            if ui.button(t("reconcile.cancel")).clicked() {
                s.dialog = None;
            }
        });
    });
}
