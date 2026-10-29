//! The read-only Bill Payment view (Q-P18f, `vendors.bill_payment`): what
//! `OpenRecord` on a bill-payment row opens. Vendor, date, check number,
//! the account it was paid from, the bills it paid with their discount and
//! credit, the total, and Void (the engine can't change a bill payment; to
//! change one, void it and pay again). A void dated in a closed period
//! takes the closing-date password.

use std::sync::Arc;

use cl_api::{
    Account, AccountFilter, ApWriteOptions, ApiError, ClosedPeriodAuthorization, IdempotencyKey,
    OrphanLabel, PostingOptions, RequestGroup, TxnStatus,
};
use cl_core::ids::{AccountId, TenantId, TransactionId};
use cl_core::{CalendarDate, Money, Secret};
use cl_model::CommandId;
use eframe::egui::{self, RichText, Ui};
use serde_json::{Value, json};

use super::engine::{DemoPayBills, LocalPayBills, Viewed};
use crate::company::{CompanyEvent, CompanyLink, Task};
use crate::forms::closing_prompt::{ClosingClick, ClosingPrompt, Verb};
use crate::i18n::t;
use crate::screens::card_charge::kit;
use crate::screens::company::form;
use crate::screens::register::account_text;
use crate::theme::Tokens;
use crate::windows::Screen;

/// The command and window kind.
pub const KIND: &str = "vendors.bill_payment";

/// The view for `kind`, if it is one.
#[must_use]
pub fn make(
    kind: CommandId,
    link: &Arc<CompanyLink>,
    _open: Option<(TenantId, String)>,
    _today: CalendarDate,
) -> Option<Box<dyn Screen>> {
    (kind.as_str() == KIND).then(|| Box::new(BillPaymentView::new(Arc::clone(link))) as Box<_>)
}

/// One bill the payment paid, as shown.
#[derive(Clone)]
struct PaidRow {
    label: String,
    cash: Money,
    discount: Money,
    credit: Money,
}

/// What the view shows.
#[derive(Clone)]
struct Shown {
    viewed: Viewed,
    vendor: String,
    account: String,
    rows: Vec<PaidRow>,
    closing: Option<CalendarDate>,
    /// Accounts Payable.
    ap: Option<AccountId>,
}

/// The read-only Bill Payment window.
pub struct BillPaymentView {
    link: Arc<CompanyLink>,
    group: Option<RequestGroup>,
    local: Option<Box<dyn LocalPayBills>>,
    id: Option<TransactionId>,
    task: Option<Task<Shown>>,
    shown: Option<Shown>,
    error: Option<ApiError>,
    void_open: bool,
    void_reason: String,
    void_task: Option<Task<()>>,
    voided: bool,
    closing_prompt: ClosingPrompt<String>,
}

/// The account's name, or a dash when there is none (a payment made only
/// from credits and a discount).
fn names(accounts: &[Account], id: Option<AccountId>) -> String {
    id.and_then(|a| accounts.iter().find(|x| x.id == a))
        .map_or_else(|| "\u{2014}".to_string(), account_text)
}

/// Accounts Payable in the chart.
fn ap_of(accounts: &[Account]) -> Option<AccountId> {
    accounts
        .iter()
        .find(|a| a.system_tag.as_deref() == Some("accounts_payable"))
        .map(|a| a.id)
}

/// Every bill the payment touched, in order: the bills it paid cash or a
/// discount on, then those settled by credits alone (the engine writes no
/// bill application for them).
fn bills_of(v: &Viewed) -> Vec<TransactionId> {
    let mut out: Vec<TransactionId> = Vec::new();
    let all = v
        .record
        .bill_applications
        .iter()
        .map(|a| a.bill_id)
        .chain(v.record.credit_applications.iter().map(|c| c.bill_id));
    for b in all {
        if !out.contains(&b) {
            out.push(b);
        }
    }
    out
}

/// The rows of a payment: cash and discount per bill (zero for a bill
/// settled by credits alone), its credits beside (review 2).
fn rows_of(v: &Viewed, label: &dyn Fn(TransactionId) -> String) -> Vec<PaidRow> {
    bills_of(v)
        .into_iter()
        .map(|bill| {
            let a = v
                .record
                .bill_applications
                .iter()
                .find(|a| a.bill_id == bill);
            PaidRow {
                label: label(bill),
                cash: a.map_or(Money::ZERO, |a| a.amount),
                discount: a.and_then(|a| a.discount_amount).unwrap_or(Money::ZERO),
                credit: Money::sum(
                    v.record
                        .credit_applications
                        .iter()
                        .filter(|c| c.bill_id == bill)
                        .map(|c| c.amount),
                )
                .unwrap_or(Money::ZERO),
            }
        })
        .collect()
}

impl BillPaymentView {
    /// An empty view (it shows a payment once `open_record` names one).
    #[must_use]
    pub fn new(link: Arc<CompanyLink>) -> BillPaymentView {
        let local: Option<Box<dyn LocalPayBills>> = link
            .is_demo()
            .then(|| Box::new(DemoPayBills) as Box<dyn LocalPayBills>);
        let mut closing_prompt = ClosingPrompt::default();
        closing_prompt.set_verb(Verb::Void);
        BillPaymentView {
            group: link.group().and_then(Result::ok),
            link,
            local,
            id: None,
            task: None,
            shown: None,
            error: None,
            void_open: false,
            void_reason: String::new(),
            void_task: None,
            voided: false,
            closing_prompt,
        }
    }

    fn load(&mut self, id: TransactionId) {
        self.id = Some(id);
        let link = Arc::clone(&self.link);
        if let Some(local) = self.local.as_mut() {
            let setup = local.setup(&link).unwrap_or_default();
            match local.get(&link, id) {
                Ok(v) => {
                    let label = |b: TransactionId| {
                        link.demo_state()
                            .and_then(|mut s| crate::company::demo::bills::get(&mut s, b))
                            .and_then(|v| {
                                v.record.vendor_invoice_number.or(v.record.txn.txn_number)
                            })
                            .unwrap_or_default()
                    };
                    let rows = rows_of(&v, &label);
                    let vendor = v
                        .record
                        .contact_id
                        .and_then(|c| {
                            link.demo_state().and_then(|mut s| {
                                s.contacts_mut()
                                    .iter()
                                    .find(|x| x.id == c)
                                    .map(|x| x.fields.display_name.clone())
                            })
                        })
                        .unwrap_or_default();
                    let account = names(&setup.accounts, v.account);
                    self.task = Some(Task::ready(Shown {
                        viewed: v,
                        vendor,
                        account,
                        rows,
                        closing: setup.closing,
                        ap: ap_of(&setup.accounts),
                    }));
                }
                Err(e) => self.error = Some(e),
            }
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        if let Ok(p) = group.read(&session, move |s| async move {
            let record = s.bill_payments().get(id).await?;
            let txn = s.transactions().get(id).await?.record;
            let accounts = s.accounts().list_all(&AccountFilter::default()).await?;
            let closing = s.company_settings().await.ok().and_then(|c| c.lock_date);
            let line_accounts: Vec<AccountId> = txn.lines.iter().map(|l| l.account_id).collect();
            // The bank or card line (not the discount line).
            let account = super::engine::paid_from(&accounts, &line_accounts);
            let viewed = Viewed {
                record,
                account,
                line_accounts,
            };
            let mut labels = Vec::new();
            for bill in bills_of(&viewed) {
                let label = s
                    .bills()
                    .get(bill)
                    .await
                    .ok()
                    .and_then(|b| {
                        b.record
                            .vendor_invoice_number
                            .clone()
                            .or_else(|| b.record.txn.txn_number.clone())
                    })
                    .unwrap_or_default();
                labels.push((bill, label));
            }
            let rows = rows_of(&viewed, &|b| {
                labels
                    .iter()
                    .find(|(x, _)| *x == b)
                    .map(|(_, l)| l.clone())
                    .unwrap_or_default()
            });
            Ok(Shown {
                vendor: txn.contact_name.clone().unwrap_or_default(),
                account: names(&accounts, viewed.account),
                ap: ap_of(&accounts),
                viewed,
                rows,
                closing,
            })
        }) {
            self.task = Some(Task::Pending(p));
        }
    }

    /// Tests: the void reason as typed.
    #[cfg(test)]
    pub(crate) fn set_void_reason(&mut self, reason: &str) {
        reason.clone_into(&mut self.void_reason);
    }

    /// Tests: "Void payment" clicked.
    #[cfg(test)]
    pub(crate) fn confirm_void(&mut self) {
        self.send_void(None);
    }

    fn send_void(&mut self, password: Option<Secret>) {
        let Some(id) = self.id else { return };
        let reason = self.void_reason.trim().to_string();
        if reason.is_empty() {
            return;
        }
        let link = Arc::clone(&self.link);
        if let Some(local) = self.local.as_mut() {
            let r = local.void(&link, id, password.as_ref());
            self.void_done(r);
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        let expected = self.shown.as_ref().and_then(|s| s.viewed.record.updated_at);
        let label = OrphanLabel {
            what: "orphan.pay_bills",
            key: None,
        };
        if let Ok(p) = group.write(&session, label, move |s| async move {
            let opts = ApWriteOptions {
                posting: PostingOptions {
                    closed_period: password.map(ClosedPeriodAuthorization::with_password),
                },
                expected_updated_at: expected,
            };
            s.bill_payments()
                .void(id, &reason, &opts, IdempotencyKey::new())
                .await
        }) {
            self.void_task = Some(Task::Pending(p));
        }
    }

    fn poll(&mut self) {
        if let Some(r) = self.task.as_mut().and_then(Task::try_take) {
            self.task = None;
            match r {
                Ok(s) => self.shown = Some(s),
                Err(e) => self.error = Some(e),
            }
        }
        if let Some(r) = self.void_task.as_mut().and_then(Task::try_take) {
            self.void_task = None;
            self.void_done(r);
        }
    }

    fn void_done(&mut self, r: Result<(), ApiError>) {
        {
            match r {
                Ok(()) => {
                    self.closing_prompt.saved();
                    self.void_open = false;
                    self.voided = true;
                    // Every account the payment's lines touched (A/P, the
                    // bank or card, the discount account): an open Pay
                    // Bills and the registers read again (review 3).
                    let mut accounts: Vec<AccountId> = self
                        .shown
                        .as_ref()
                        .map(|s| s.viewed.line_accounts.clone())
                        .unwrap_or_default();
                    accounts.extend(self.shown.as_ref().and_then(|s| s.ap));
                    accounts.sort_unstable();
                    accounts.dedup();
                    let sub = self.link.bus().subscribe();
                    crate::forms::publish_saved(&self.link, &sub, &accounts);
                    self.link.push(CompanyEvent::Toast("billpay.voided"));
                    if let Some(id) = self.id {
                        self.load(id);
                        self.poll();
                    }
                }
                Err(e) => {
                    let closing = self.shown.as_ref().and_then(|s| s.closing);
                    let supported = self.local.is_some()
                        || self
                            .link
                            .capabilities()
                            .has(cl_api::Capabilities::AP_CLOSING);
                    if !self
                        .closing_prompt
                        .failed(&e, self.void_reason.clone(), supported, closing)
                    {
                        self.error = Some(e);
                    }
                }
            }
        }
    }

    fn draw(&mut self, ui: &mut Ui) {
        let tokens = Tokens::of(ui.ctx());
        let style = crate::grid_env::shell_frame(ui.ctx()).money_style;
        ui.label(
            RichText::new(t("cmd.vendors.bill_payment"))
                .size(22.0)
                .strong()
                .color(tokens.text_primary),
        );
        form::hint(ui, &tokens, &t("billpay.read_only"));
        match self.closing_prompt.ui(ui, &tokens) {
            ClosingClick::Submit => {
                if let Some((_, secret)) = self.closing_prompt.submit() {
                    self.send_void(Some(secret));
                }
            }
            ClosingClick::Cancel => self.closing_prompt.cancel(),
            ClosingClick::None => {}
        }
        if let Some(e) = &self.error {
            form::inline_error(ui, &tokens, e);
        }
        let Some(s) = self.shown.clone() else {
            if self.task.is_some() {
                form::progress_line(ui, &tokens, &t("billpay.loading"));
            } else if self.id.is_none() {
                form::hint(ui, &tokens, &t("billpay.none"));
            }
            return;
        };
        let r = &s.viewed.record;
        let void = r.status == TxnStatus::Void;
        form::card(ui, &tokens, |ui| {
            let number = match (r.check_number, r.print_status.as_deref()) {
                (Some(n), _) => n.to_string(),
                (None, Some("queue")) => t("billpay.number.to_print"),
                _ => "\u{2014}".to_string(),
            };
            for (k, v) in [
                ("billpay.vendor", s.vendor.clone()),
                ("billpay.date", crate::shell::us_date(r.txn_date)),
                ("billpay.number", number),
                ("billpay.account", s.account.clone()),
                (
                    "billpay.total",
                    r.total.map(|m| m.format(style)).unwrap_or_default(),
                ),
            ] {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(t(k)).color(tokens.text_secondary));
                    ui.label(RichText::new(v).color(tokens.text_primary));
                });
            }
            if void {
                ui.label(RichText::new(t("billpay.void_status")).color(tokens.warn_text));
            }
        });
        ui.add_space(8.0);
        kit::panel_card(ui, &tokens, &t("billpay.bills"), |ui| {
            egui::Grid::new("billpay/bills")
                .num_columns(4)
                .striped(true)
                .show(ui, |ui| {
                    for k in [
                        "billpay.col.bill",
                        "billpay.col.paid",
                        "billpay.col.discount",
                        "billpay.col.credit",
                    ] {
                        ui.label(RichText::new(t(k)).color(tokens.text_secondary));
                    }
                    ui.end_row();
                    for row in &s.rows {
                        ui.label(&row.label);
                        for m in [row.cash, row.discount, row.credit] {
                            ui.label(
                                RichText::new(m.format(style))
                                    .text_style(egui::TextStyle::Monospace),
                            );
                        }
                        ui.end_row();
                    }
                });
        });
        ui.add_space(8.0);
        if !void {
            if self.void_open {
                let mut send = false;
                let mut cancel = false;
                form::card(ui, &tokens, |ui| {
                    form::heading(ui, &tokens, &t("billpay.void.title"));
                    form::field(ui, &t("billpay.void.reason"), &mut self.void_reason);
                    ui.horizontal(|ui| {
                        send = crate::widgets::destructive_button(ui, &t("billpay.void.confirm"))
                            .clicked();
                        cancel = ui.button(t("billpay.void.cancel")).clicked();
                    });
                });
                if cancel {
                    self.void_open = false;
                } else if send {
                    self.send_void(None);
                }
            } else if ui.button(t("billpay.void")).clicked() {
                self.void_open = true;
            }
        }
    }
}

impl Screen for BillPaymentView {
    fn title(&self) -> String {
        t("cmd.vendors.bill_payment")
    }

    fn ui(&mut self, ui: &mut Ui) {
        self.poll();
        egui::ScrollArea::vertical()
            .id_salt("billpay")
            .show(ui, |ui| self.draw(ui));
        if self.task.is_some() || self.void_task.is_some() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    fn command(&mut self, id: CommandId) -> bool {
        if let Some(h) = self.closing_prompt.command(id) {
            return h;
        }
        match id.as_str() {
            KIND => true,
            "edit.void_transaction" => {
                self.void_open = self.shown.is_some();
                true
            }
            _ => false,
        }
    }

    fn open_record(&mut self, txn: TransactionId) -> bool {
        self.load(txn);
        self.poll();
        true
    }

    fn state(&self) -> Value {
        let r = self.shown.as_ref().map(|s| &s.viewed.record);
        let mut v = json!({
            "screen": "bill_payment",
            "loaded": self.shown.is_some(),
            "id": self.id.map(|i| i.to_string()),
            "status": r.map(|r| if r.status == TxnStatus::Void { "void" } else { "posted" }),
            "check_number": r.and_then(|r| r.check_number),
            "bills": r.map(|r| r.bill_applications.len()),
            "credits": r.map(|r| r.credit_applications.len()),
            "void_open": self.void_open,
            "voided": self.voided,
            "error": self.error.is_some(),
        });
        if let (Value::Object(m), Value::Object(c)) = (&mut v, self.closing_prompt.state_json(true))
        {
            m.extend(c);
        }
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::company::demo::{account_id, chart};
    use cl_api::{BillApplication, BillPaymentRecord, CreditApplication};

    fn id(n: u8) -> TransactionId {
        TransactionId::parse(&format!("d0000000-0000-4000-8000-0000000000{n:02x}")).expect("id")
    }

    fn m(s: &str) -> Money {
        Money::parse_wire(s).expect("money")
    }

    /// Review 2: a bill settled by a credit alone has no bill application
    /// from the engine; the view still lists it (cash and discount zero).
    #[test]
    fn a_bill_settled_by_credits_alone_is_listed() {
        let record = BillPaymentRecord {
            id: id(20),
            txn_date: CalendarDate::from_ymd(2026, 11, 4).expect("d"),
            contact_id: None,
            total: Some(m("100.00")),
            check_number: None,
            print_status: None,
            status: TxnStatus::Posted,
            memo: None,
            updated_at: None,
            bill_applications: vec![BillApplication {
                bill_id: id(1),
                amount: m("100.00"),
                discount_amount: Some(Money::ZERO),
            }],
            credit_applications: vec![CreditApplication {
                credit_id: id(9),
                bill_id: id(2),
                amount: m("80.00"),
            }],
        };
        let v = Viewed {
            record,
            account: None,
            line_accounts: Vec::new(),
        };
        let rows = rows_of(&v, &|b| if b == id(2) { "B2".into() } else { "B1".into() });
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].label, "B2");
        assert_eq!(
            (rows[1].cash, rows[1].discount, rows[1].credit),
            (Money::ZERO, Money::ZERO, m("80.00"))
        );
    }

    /// Review (Low): "Paid from" is the bank or card line, never the
    /// discount account; with neither it shows a dash.
    #[test]
    fn paid_from_is_the_bank_or_card_line() {
        let accounts = chart();
        let ap = ap_of(&accounts).expect("A/P");
        let (bank, discount) = (account_id(1010), account_id(5200));
        assert_eq!(
            super::super::engine::paid_from(&accounts, &[ap, discount, bank]),
            Some(bank)
        );
        let none = super::super::engine::paid_from(&accounts, &[ap, discount]);
        assert_eq!(none, None);
        assert_eq!(names(&accounts, none), "\u{2014}");
    }
}
