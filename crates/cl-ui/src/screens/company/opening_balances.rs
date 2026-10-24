//! Guided opening balances (P08 step 4, Q-45): a start date, bank and
//! card balances, open invoices, unpaid bills and other balances; post;
//! then review the balance sheet the engine reports. Amounts are exact
//! (`Money`), typed by the owner, never computed here.

use std::sync::Arc;

use cl_api::{
    AccountFilter, AccountRef, BalanceSheet, BillLine, ContactFields, ContactType, IdempotencyKey,
    InvoiceLine, NewBill, NewContact, NewInvoice, OpeningBalance, ReportBasis,
};
use cl_company::CompanyError;
use cl_core::{CalendarDate, Money, Quantity, Rate};
use cl_model::{CommandId, Scope};
use eframe::egui::{self, RichText, Ui};
use serde_json::{Value, json};

use super::form::{self, DateInput};
use crate::company::{CompanyEvent, CompanyLink, Task, spawn_lifecycle};
use crate::forms::enter::HeaderEnter;
use crate::i18n::{t, t_with};
use crate::layout::{Body, FixedFrame, FrameOpts, FrameReport};
use crate::theme::Tokens;
use crate::windows::Screen;

/// A pickable account.
#[derive(Clone, Debug, PartialEq, Eq)]
struct AccountChoice {
    id: Option<cl_core::ids::AccountId>,
    number: String,
    name: String,
    kind: String,
}

/// One balance row.
#[derive(Clone, Debug, Default)]
struct BalanceRow {
    /// Picked account, or none yet ("Pick an account").
    account: Option<usize>,
    amount: String,
}

/// One open invoice or unpaid bill row.
#[derive(Clone, Debug, Default)]
struct DocRow {
    who: String,
    date: Option<DateInput>,
    number: String,
    amount: String,
}

/// What was posted (for the review panel).
#[derive(Clone, Debug)]
pub struct PostedSummary {
    /// Lines in the opening entry.
    pub lines: u64,
    /// Invoices created.
    pub invoices: usize,
    /// Bills created.
    pub bills: usize,
    /// The balance sheet as of the start date.
    pub balance_sheet: Option<BalanceSheet>,
}

/// The Opening balances window.
pub struct OpeningBalancesScreen {
    link: Arc<CompanyLink>,
    group: Option<cl_api::RequestGroup>,
    today: CalendarDate,
    start: DateInput,
    accounts: Vec<AccountChoice>,
    accounts_task: Option<Task<Result<Vec<AccountChoice>, CompanyError>>>,
    bank_rows: Vec<BalanceRow>,
    other_rows: Vec<BalanceRow>,
    invoices: Vec<DocRow>,
    bills: Vec<DocRow>,
    issues: Vec<String>,
    dirty: bool,
    task: Option<Task<Result<PostedSummary, CompanyError>>>,
    posted: Option<PostedSummary>,
    error: Option<CompanyError>,
    /// What the last Enter in a field did (control channel).
    last_enter: Option<&'static str>,
    /// Where the frame's bars went (control channel).
    layout: FrameReport,
    /// The start date takes the keyboard on the first frame.
    focus_start: bool,
}

impl OpeningBalancesScreen {
    /// A fresh window; `today` seeds the start date (first of the year).
    #[must_use]
    pub fn new(link: Arc<CompanyLink>, today: CalendarDate) -> OpeningBalancesScreen {
        let group = link.group().and_then(Result::ok);
        let accounts = if link.is_demo() {
            crate::company::demo::DEMO_ACCOUNTS
                .iter()
                .map(|(n, name, kind)| AccountChoice {
                    id: None,
                    number: (*n).into(),
                    name: (*name).into(),
                    kind: (*kind).into(),
                })
                .collect()
        } else {
            Vec::new()
        };
        OpeningBalancesScreen {
            link,
            group,
            today,
            start: DateInput::new(today.start_of_year()),
            accounts,
            accounts_task: None,
            bank_rows: vec![BalanceRow {
                account: Some(0),
                amount: String::new(),
            }],
            other_rows: Vec::new(),
            invoices: Vec::new(),
            bills: Vec::new(),
            issues: Vec::new(),
            dirty: false,
            task: None,
            posted: None,
            error: None,
            last_enter: None,
            layout: FrameReport::default(),
            focus_start: true,
        }
    }

    fn load_accounts(&mut self) {
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        self.accounts_task = spawn_lifecycle(&self.link, group, move || async move {
            let list = session
                .accounts()
                .list_all(&AccountFilter::default())
                .await?;
            let mut out: Vec<AccountChoice> = list
                .into_iter()
                .filter(|a| a.is_active)
                .map(|a| AccountChoice {
                    id: Some(a.id),
                    number: a.fields.account_number.clone().unwrap_or_default(),
                    name: crate::account_names::shown_name(&a),
                    kind: a.fields.account_type.as_str().to_string(),
                })
                .collect();
            out.sort_by(|a, b| a.number.cmp(&b.number).then_with(|| a.name.cmp(&b.name)));
            Ok(out)
        })
        .ok();
    }

    fn parse_rows(&self) -> Result<Plan, Vec<String>> {
        let mut issues = Vec::new();
        let Some(start) = self.start.value else {
            issues.push(t("opening.issue.start_date"));
            return Err(issues);
        };
        let mut balances = Vec::new();
        for (section, rows) in [("bank", &self.bank_rows), ("other", &self.other_rows)] {
            for (i, r) in rows.iter().enumerate() {
                if r.amount.trim().is_empty() {
                    continue;
                }
                let Some(acct) = r.account.and_then(|i| self.accounts.get(i)) else {
                    issues.push(t_with(
                        "opening.issue.account",
                        &[("row", &(i + 1).to_string())],
                    ));
                    continue;
                };
                match Money::parse_user(&r.amount, cl_core::UserInput::MONEY) {
                    Ok(m) if !m.is_zero() => balances.push((section, acct.clone(), m)),
                    Ok(_) => {}
                    Err(_) => issues.push(t_with(
                        "opening.issue.amount",
                        &[("row", &(i + 1).to_string())],
                    )),
                }
            }
        }
        let mut docs = |rows: &[DocRow], what: &str| -> Vec<Doc> {
            let mut out = Vec::new();
            for (i, r) in rows.iter().enumerate() {
                if r.who.trim().is_empty() && r.amount.trim().is_empty() {
                    continue;
                }
                let amount = match Money::parse_user(&r.amount, cl_core::UserInput::MONEY) {
                    Ok(m) if m.is_positive() => m,
                    _ => {
                        issues.push(t_with(
                            "opening.issue.doc_amount",
                            &[("what", what), ("row", &(i + 1).to_string())],
                        ));
                        continue;
                    }
                };
                if r.who.trim().is_empty() {
                    issues.push(t_with(
                        "opening.issue.doc_name",
                        &[("what", what), ("row", &(i + 1).to_string())],
                    ));
                    continue;
                }
                out.push(Doc {
                    who: r.who.trim().to_string(),
                    date: r.date.as_ref().and_then(|d| d.value).unwrap_or(start),
                    number: r.number.trim().to_string(),
                    amount,
                });
            }
            out
        };
        let invoices = docs(&self.invoices, &t("opening.invoices"));
        let bills = docs(&self.bills, &t("opening.bills"));
        if issues.is_empty() {
            Ok(Plan {
                start,
                balances,
                invoices,
                bills,
            })
        } else {
            Err(issues)
        }
    }

    fn post(&mut self) {
        let plan = match self.parse_rows() {
            Ok(p) => p,
            Err(issues) => {
                self.issues = issues;
                return;
            }
        };
        self.issues.clear();
        if plan.balances.is_empty() && plan.invoices.is_empty() && plan.bills.is_empty() {
            self.issues.push(t("opening.issue.nothing"));
            return;
        }
        if self.link.is_demo() {
            self.task = Some(Task::ready(Ok(PostedSummary {
                lines: u64::try_from(plan.balances.len() + 1).unwrap_or(0),
                invoices: plan.invoices.len(),
                bills: plan.bills.len(),
                balance_sheet: None,
            })));
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        self.task = spawn_lifecycle(&self.link, group, move || async move {
            let mut lines = 0;
            if !plan.balances.is_empty() {
                let balances: Vec<OpeningBalance> = plan
                    .balances
                    .iter()
                    .map(|(_, a, m)| OpeningBalance {
                        account: a
                            .id
                            .map_or_else(|| AccountRef::Number(a.number.clone()), AccountRef::Id),
                        balance: *m,
                    })
                    .collect();
                lines = session
                    .post_opening_balances(&balances, plan.start, IdempotencyKey::new())
                    .await?
                    .lines_created;
            }
            let accounts = session
                .accounts()
                .list_all(&AccountFilter::default())
                .await?;
            let opening_equity = accounts
                .iter()
                .find(|a| a.system_tag.as_deref() == Some("opening_balances"))
                .map(|a| a.id)
                .ok_or(CompanyError::Unavailable("opening_balances account"))?;
            for inv in &plan.invoices {
                let contact = session
                    .contacts()
                    .create(
                        &NewContact {
                            fields: ContactFields::named(ContactType::Customer, inv.who.clone()),
                            opening_balance: Money::ZERO,
                            opening_balance_date: None,
                        },
                        IdempotencyKey::new(),
                    )
                    .await?
                    .record;
                session
                    .create_invoice(
                        &NewInvoice {
                            txn_date: inv.date,
                            due_date: None,
                            contact_id: contact.id,
                            lines: vec![InvoiceLine {
                                account_id: opening_equity,
                                description: Some(t("opening.doc_line")),
                                quantity: Quantity::parse_wire("1")
                                    .map_err(|_| CompanyError::Metadata)?,
                                unit_price: inv.amount,
                                is_taxable: false,
                                tax_rate: Rate::parse_wire("0")
                                    .map_err(|_| CompanyError::Metadata)?,
                            }],
                            memo: Some(t("opening.doc_memo")),
                            txn_number: (!inv.number.is_empty()).then(|| inv.number.clone()),
                        },
                        IdempotencyKey::new(),
                    )
                    .await?;
            }
            for bill in &plan.bills {
                let contact = session
                    .contacts()
                    .create(
                        &NewContact {
                            fields: ContactFields::named(ContactType::Vendor, bill.who.clone()),
                            opening_balance: Money::ZERO,
                            opening_balance_date: None,
                        },
                        IdempotencyKey::new(),
                    )
                    .await?
                    .record;
                session
                    .create_bill(
                        &NewBill {
                            contact_id: contact.id,
                            txn_date: bill.date,
                            due_date: None,
                            vendor_invoice_number: (!bill.number.is_empty())
                                .then(|| bill.number.clone()),
                            memo: Some(t("opening.doc_memo")),
                            lines: vec![BillLine {
                                account_id: opening_equity,
                                description: Some(t("opening.doc_line")),
                                amount: bill.amount,
                            }],
                        },
                        IdempotencyKey::new(),
                    )
                    .await?;
            }
            let balance_sheet = session
                .reports()
                .balance_sheet(plan.start, ReportBasis::Accrual)
                .await
                .ok();
            Ok(PostedSummary {
                lines,
                invoices: plan.invoices.len(),
                bills: plan.bills.len(),
                balance_sheet,
            })
        })
        .ok();
    }

    fn account_picker(&self, ui: &mut Ui, id: &str, row: &mut BalanceRow) -> bool {
        let mut changed = false;
        let current = row.account.and_then(|i| self.accounts.get(i)).map_or_else(
            || t("opening.pick_account"),
            |a| format!("{} {}", a.number, a.name),
        );
        let label = ui.add(egui::Label::new(t("opening.account")).selectable(false));
        egui::ComboBox::from_id_salt(id)
            .width(260.0)
            .selected_text(current)
            .show_ui(ui, |ui| {
                for (i, a) in self.accounts.iter().enumerate() {
                    if ui
                        .selectable_label(
                            row.account == Some(i),
                            format!("{} {}", a.number, a.name),
                        )
                        .clicked()
                    {
                        row.account = Some(i);
                        changed = true;
                    }
                }
            })
            .response
            .labelled_by(label.id);
        changed
    }

    /// The balance rows; true when Enter in one of them means post.
    fn balance_rows(&mut self, ui: &mut Ui, tokens: &Tokens, which: &str) -> bool {
        let mut save = false;
        let mut rows = if which == "bank" {
            std::mem::take(&mut self.bank_rows)
        } else {
            std::mem::take(&mut self.other_rows)
        };
        let mut remove = None;
        for (i, row) in rows.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                if self.account_picker(ui, &format!("{which}-acct-{i}"), row) {
                    self.dirty = true;
                }
                let l = ui.add(egui::Label::new(t("opening.balance")).selectable(false));
                let r = ui
                    .add_sized([140.0, 24.0], egui::TextEdit::singleline(&mut row.amount))
                    .labelled_by(l.id);
                if r.changed() {
                    self.dirty = true;
                }
                save |= self.entered(&r);
                if ui.small_button(t("opening.remove_row")).clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            rows.remove(i);
        }
        if ui.button(t("opening.add_row")).clicked() {
            rows.push(BalanceRow::default());
        }
        form::hint(ui, tokens, &t("opening.sign_hint"));
        if which == "bank" {
            self.bank_rows = rows;
        } else {
            self.other_rows = rows;
        }
        save
    }

    /// The invoice or bill rows; true when Enter in one of them means post.
    fn doc_rows(&mut self, ui: &mut Ui, which: &str) -> bool {
        let mut save = false;
        let mut rows = if which == "invoices" {
            std::mem::take(&mut self.invoices)
        } else {
            std::mem::take(&mut self.bills)
        };
        let today = self.today;
        let mut remove = None;
        let who_label = if which == "invoices" {
            t("opening.client")
        } else {
            t("opening.vendor")
        };
        for (i, row) in rows.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                let l = ui.add(egui::Label::new(who_label.clone()).selectable(false));
                let r = ui
                    .add_sized([200.0, 24.0], egui::TextEdit::singleline(&mut row.who))
                    .labelled_by(l.id);
                if r.changed() {
                    self.dirty = true;
                }
                save |= self.entered(&r);
                let date = row
                    .date
                    .get_or_insert_with(|| DateInput::new(today.start_of_year()));
                let r = form::date_field(ui, &t("opening.date"), date, today);
                save |= self.entered(&r);
                let l = ui.add(egui::Label::new(t("opening.number")).selectable(false));
                let r = ui
                    .add_sized([90.0, 24.0], egui::TextEdit::singleline(&mut row.number))
                    .labelled_by(l.id);
                save |= self.entered(&r);
                let l = ui.add(egui::Label::new(t("opening.amount")).selectable(false));
                let r = ui
                    .add_sized([120.0, 24.0], egui::TextEdit::singleline(&mut row.amount))
                    .labelled_by(l.id);
                if r.changed() {
                    self.dirty = true;
                }
                save |= self.entered(&r);
                if ui.small_button(t("opening.remove_row")).clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            rows.remove(i);
        }
        if ui.button(t("opening.add_row")).clicked() {
            rows.push(DocRow::default());
        }
        if which == "invoices" {
            self.invoices = rows;
        } else {
            self.bills = rows;
        }
        save
    }

    /// Enter in field `r` (Q-190, Q-264): true when it means Save & New
    /// (here: post, like Alt+N); in the Move mode the focus moves on.
    fn entered(&mut self, r: &egui::Response) -> bool {
        let e = crate::forms::enter::entered(r);
        if e != HeaderEnter::None {
            self.last_enter = Some(match e {
                HeaderEnter::SaveAndNew => "save",
                HeaderEnter::MovedNext => "next",
                HeaderEnter::MovedPrev => "prev",
                HeaderEnter::None => "none",
            });
        }
        e.saves()
    }

    /// "Posted: 3 lines in the opening entry, 1 invoice, 2 bills."
    fn summary_text(p: &PostedSummary) -> String {
        let count = |n: usize, one: &str, many: &str| {
            format!("{n} {}", if n == 1 { t(one) } else { t(many) })
        };
        t_with(
            "opening.review.summary",
            &[
                ("lines", &p.lines.to_string()),
                (
                    "invoices",
                    &count(p.invoices, "opening.one_invoice", "opening.many_invoices"),
                ),
                (
                    "bills",
                    &count(p.bills, "opening.one_bill", "opening.many_bills"),
                ),
            ],
        )
    }

    /// Esc: clear what was typed (rows go back to one empty bank line).
    /// Returns false when nothing was typed.
    fn revert_fields(&mut self) -> bool {
        let typed = self.bank_rows.iter().any(|r| !r.amount.trim().is_empty())
            || self.other_rows.iter().any(|r| !r.amount.trim().is_empty())
            || !self.invoices.is_empty()
            || !self.bills.is_empty();
        if !typed {
            return false;
        }
        self.bank_rows = vec![BalanceRow {
            account: Some(0),
            amount: String::new(),
        }];
        self.other_rows.clear();
        self.invoices.clear();
        self.bills.clear();
        self.issues.clear();
        true
    }

    fn review(ui: &mut Ui, tokens: &Tokens, p: &PostedSummary) {
        form::card(ui, tokens, |ui| {
            form::heading(ui, tokens, &t("opening.review.title"));
            form::hint(ui, tokens, &Self::summary_text(p));
            let Some(bs) = &p.balance_sheet else {
                form::hint(ui, tokens, &t("opening.review.no_report"));
                return;
            };
            egui::Grid::new("bs")
                .num_columns(2)
                .spacing([24.0, 4.0])
                .show(ui, |ui| {
                    for (title, rows, total) in [
                        (t("opening.review.assets"), &bs.assets, &bs.total_assets),
                        (
                            t("opening.review.liabilities"),
                            &bs.liabilities,
                            &bs.total_liabilities,
                        ),
                        (t("opening.review.equity"), &bs.equity, &bs.total_equity),
                    ] {
                        ui.label(RichText::new(title).strong());
                        ui.label("");
                        ui.end_row();
                        for e in rows {
                            ui.label(format!(
                                "{} {}",
                                e.account_number.as_deref().unwrap_or(""),
                                e.name
                            ));
                            ui.label(e.balance.display(2).unwrap_or_default());
                            ui.end_row();
                        }
                        ui.label(
                            RichText::new(t("opening.review.total")).color(tokens.text_secondary),
                        );
                        ui.label(total.display(2).unwrap_or_default());
                        ui.end_row();
                    }
                });
        });
    }
}

struct Plan {
    start: CalendarDate,
    balances: Vec<(&'static str, AccountChoice, Money)>,
    invoices: Vec<Doc>,
    bills: Vec<Doc>,
}

struct Doc {
    who: String,
    date: CalendarDate,
    number: String,
    amount: Money,
}

impl Screen for OpeningBalancesScreen {
    fn title(&self) -> String {
        t("opening.tab")
    }

    fn scope(&self) -> Scope {
        Scope::Form
    }

    fn dirty(&self) -> bool {
        self.dirty && self.posted.is_none()
    }

    fn ui(&mut self, ui: &mut Ui) {
        let tokens = Tokens::of(ui.ctx());
        if let Some(t) = self.accounts_task.as_mut()
            && let Some(r) = t.poll_result()
        {
            self.accounts_task = None;
            match r {
                Ok(a) => self.accounts = a,
                Err(e) => self.error = Some(e),
            }
        }
        if let Some(t) = self.task.as_mut()
            && let Some(r) = t.poll_result()
        {
            self.task = None;
            // Open registers, centers and lists read again (Q-262); after
            // a failure too, since part of it may have posted.
            self.link
                .bus()
                .publish(crate::data_bus::Change::Ledger(None));
            match r {
                Ok(p) => {
                    self.posted = Some(p);
                    self.dirty = false;
                    self.link.push(CompanyEvent::Toast("opening.toast.posted"));
                }
                Err(e) => self.error = Some(e),
            }
        }
        if self.accounts.is_empty() && self.accounts_task.is_none() && !self.link.is_demo() {
            self.load_accounts();
        }
        // The heading stays at the top and Post / Skip at the bottom; only
        // the cards scroll (Q-265, review P16-part1 #8).
        let mut frame = FixedFrame::begin(ui, "opening_balances", FrameOpts::default());
        frame.top(|ui| {
            ui.add_space(8.0);
            ui.heading(t("opening.title"));
            form::hint(ui, &tokens, &t("opening.intro"));
            ui.add_space(4.0);
        });
        if let Some(p) = &self.posted {
            frame.body(Body::Scroll, |ui| Self::review(ui, &tokens, p));
            let done = frame.bottom(|ui| {
                ui.add_space(6.0);
                form::actions(ui, &t("opening.review.done"), &[]).is_some()
            });
            self.layout = frame.end();
            if done {
                self.link
                    .push(CompanyEvent::OpenWindow(CommandId::new("nav.home")));
                self.link.push(CompanyEvent::CloseWindow(CommandId::new(
                    "company.opening_balances",
                )));
            }
            return;
        }
        let mut save = false;
        frame.body(Body::Scroll, |ui| {
            form::card(ui, &tokens, |ui| {
                form::heading(ui, &tokens, &t("opening.start"));
                form::hint(ui, &tokens, &t("opening.start_hint"));
                let r = form::date_field(ui, &t("opening.start_date"), &mut self.start, self.today);
                if std::mem::take(&mut self.focus_start) {
                    r.request_focus();
                }
                if r.changed() {
                    self.dirty = true;
                }
                save |= self.entered(&r);
            });
            ui.add_space(8.0);
            form::card(ui, &tokens, |ui| {
                form::heading(ui, &tokens, &t("opening.bank"));
                save |= self.balance_rows(ui, &tokens, "bank");
            });
            ui.add_space(8.0);
            form::card(ui, &tokens, |ui| {
                form::heading(ui, &tokens, &t("opening.invoices"));
                form::hint(ui, &tokens, &t("opening.invoices_hint"));
                save |= self.doc_rows(ui, "invoices");
            });
            ui.add_space(8.0);
            form::card(ui, &tokens, |ui| {
                form::heading(ui, &tokens, &t("opening.bills"));
                form::hint(ui, &tokens, &t("opening.bills_hint"));
                save |= self.doc_rows(ui, "bills");
            });
            ui.add_space(8.0);
            form::card(ui, &tokens, |ui| {
                form::heading(ui, &tokens, &t("opening.other"));
                form::hint(ui, &tokens, &t("opening.other_hint"));
                save |= self.balance_rows(ui, &tokens, "other");
            });
        });
        let picked = frame.bottom(|ui| {
            ui.add_space(4.0);
            for issue in &self.issues {
                ui.label(RichText::new(issue).color(tokens.warn_text));
            }
            if let Some(e) = &self.error {
                form::inline_error(ui, &tokens, e);
            }
            ui.add_space(6.0);
            if self.task.is_some() {
                form::progress_line(ui, &tokens, &t("opening.posting"));
                None
            } else {
                form::actions(ui, &t("opening.post"), &[&t("opening.skip")])
            }
        });
        self.layout = frame.end();
        match picked {
            Some(0) => self.post(),
            Some(_) => {
                self.link
                    .push(CompanyEvent::OpenWindow(CommandId::new("nav.home")));
                self.link.push(CompanyEvent::CloseWindow(CommandId::new(
                    "company.opening_balances",
                )));
            }
            None if save && self.task.is_none() => {
                self.command(CommandId::new("form.save_new"));
            }
            None => {}
        }
    }
    fn command(&mut self, id: CommandId) -> bool {
        match id.as_str() {
            "form.save_close" | "form.save_new" => {
                if self.task.is_none() && self.posted.is_none() {
                    self.post();
                }
                true
            }
            "form.revert" => self.revert_fields(),
            "form.insert_line" => {
                self.bank_rows.push(BalanceRow::default());
                true
            }
            _ => false,
        }
    }

    fn save(&mut self) -> bool {
        false
    }

    fn fixed_layout(&self) -> bool {
        true
    }

    fn state(&self) -> Value {
        json!({
            "screen": "opening_balances",
            "layout": self.layout.json(),
            "last_enter": self.last_enter,
            "start_date": self.start.value.map(CalendarDate::to_wire),
            "accounts": self.accounts.len(),
            "bank_rows": self.bank_rows.len(),
            "invoice_rows": self.invoices.len(),
            "bill_rows": self.bills.len(),
            "issues": self.issues.len(),
            "posted": self.posted.is_some(),
            "posting": self.task.is_some(),
            "dirty": self.dirty,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Review P16-part1 #6: Enter in a field posts by default (Q-190, the
    /// same as Alt+N; here refused because nothing is typed) and moves
    /// between the fields when the option is on (Q-264).
    #[test]
    fn enter_posts_or_moves_between_fields() {
        use crate::forms::enter::testing::{context, enter, frame, settle};
        use cl_model::forms::EnterMode;
        for mode in [EnterMode::SaveAndNew, EnterMode::MoveBetweenFields] {
            let dir = tempfile::tempdir().expect("tmp");
            let link = Arc::new(CompanyLink::demo(dir.path()));
            let today = CalendarDate::from_ymd(2026, 11, 4).expect("date");
            let mut s = OpeningBalancesScreen::new(link, today);
            let ctx = context();
            settle(&ctx, mode, &mut |ui| s.ui(ui));
            assert!(ctx.text_edit_focused(), "the start date has the keyboard");
            frame(&ctx, mode, enter(false), &mut |ui| s.ui(ui));
            settle(&ctx, mode, &mut |ui| s.ui(ui));
            let v = s.state();
            match mode {
                EnterMode::SaveAndNew => {
                    assert_eq!(v["last_enter"], "save");
                    assert_eq!(v["issues"], 1, "posting was asked: nothing to post");
                }
                EnterMode::MoveBetweenFields => {
                    assert_eq!(v["last_enter"], "next");
                    assert_eq!(v["issues"], 0, "Enter never posts here");
                    assert_eq!(v["posted"], false);
                }
            }
            assert!(v["layout"].is_object(), "on the fixed frame");
        }
    }
}
