//! The dialogs: the closing password before Finish, the done dialog, a
//! card's pay step (Q-60, Q-P27g: a check now, a bill for later or a
//! transfer from a bank account), Start over, Undo ([`super::undo`]),
//! previous reports and the discrepancy report ([`super::reports`]).

use cl_model::CommandId;
use cl_model::reconcile::ReconKind;
use eframe::egui::{self, Ui};

use super::{CardPay, Dialog, ReconcileScreen};
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
        Dialog::Undo => super::undo::draw(s, ui),
        Dialog::Previous => super::reports::previous(s, ui),
        Dialog::Discrepancy => super::reports::discrepancy(s, ui),
    }
}

fn closing(s: &mut ReconcileScreen, ui: &Ui) {
    dialog(ui, &t("reconcile.closing.title"), |ui| {
        ui.add(egui::Label::new(t("reconcile.closing.text")).wrap());
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
                s.dialog = None;
            }
        });
    });
}

fn done(s: &mut ReconcileScreen, ui: &Ui) {
    let card = s.kind() == ReconKind::Card;
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

/// The card pay step's hand-off.
pub(crate) fn pay(s: &mut ReconcileScreen, kind: &'static str) {
    let (Some(card), Some(amount), Some(date)) = (
        s.form.account,
        cl_core::Money::parse_user(&s.form.ending, cl_core::money::UserInput::CENTS).ok(),
        s.form.statement_date,
    ) else {
        s.dialog = None;
        return;
    };
    s.link.push(CompanyEvent::OpenWith(
        CommandId::new(kind),
        Box::new(OpenPayload::CardPayment(CardPay { card, amount, date })),
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
