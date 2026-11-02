//! Sales Tax Owed (P23 plan §3 WP-3, `vendors.sales_tax_owed`; U-149,
//! U-152): the liability report by agency and item for a period, on the
//! basis chosen in Preferences (the other basis can be viewed). Columns:
//! taxable and non-taxable sales, collected, discounts, bad debts and
//! recovered (accrual only, Q-SPF3), adjustments, paid and owed at the end.
//! Footer rows: unassigned (legacy) tax, other entries, the cash rounding
//! and the balance check. Clicking an item shows its non-taxable sales by
//! code and the agency's payments in the period; "Pay sales tax" opens Pay
//! Sales Tax on the agency.
//!
//! Every figure is the engine's string; the screen adds nothing up.

use std::sync::Arc;

use cl_api::{
    ApiError, Capabilities, LiabilityAgency, LiabilityColumns, LiabilityItem, LiabilityQuery,
    RequestGroup, SalesTaxBasis, SalesTaxLiability, SalesTaxPaymentRecord,
};
use cl_core::ids::{SalesTaxAgencyId, SalesTaxItemId};
use cl_core::{CalendarDate, Money, MoneyStyle};
use cl_model::{CommandId, Scope};
use eframe::egui::{self, RichText, Ui};
use serde_json::{Value, json};

use crate::company::CompanyLink;
use crate::i18n::{t, t_with};
use crate::screens::company::form::{self, DateInput};
use crate::screens::invoice::engine::{self, Job};
use crate::theme::Tokens;
use crate::windows::{OpenPayload, Screen};

/// The command and window kind.
pub const KIND: &str = "vendors.sales_tax_owed";

/// The first and last day of `d`'s month.
#[must_use]
pub fn month_of(d: CalendarDate) -> (CalendarDate, CalendarDate) {
    let first = CalendarDate::from_ymd(d.year(), d.month(), 1).unwrap_or(d);
    let next = if d.month() == 12 {
        CalendarDate::from_ymd(d.year() + 1, 1, 1)
    } else {
        CalendarDate::from_ymd(d.year(), d.month() + 1, 1)
    };
    let last = next.ok().and_then(|n| n.add_days(-1).ok()).unwrap_or(d);
    (first, last)
}

/// The Sales Tax Owed window.
pub struct OwedScreen {
    link: Arc<CompanyLink>,
    group: Option<RequestGroup>,
    available: bool,
    today: CalendarDate,
    pub(crate) from: DateInput,
    pub(crate) to: DateInput,
    /// `None`: the basis in Preferences.
    pub(crate) basis: Option<SalesTaxBasis>,
    report: Option<SalesTaxLiability>,
    job: Option<Job<SalesTaxLiability>>,
    payments: Vec<SalesTaxPaymentRecord>,
    payments_job: Option<Job<Vec<SalesTaxPaymentRecord>>>,
    /// The item opened for its detail.
    pub(crate) open: Option<SalesTaxItemId>,
    error: Option<ApiError>,
    problem: Option<&'static str>,
    loads: u32,
}

fn fmt(m: Money) -> String {
    m.format(MoneyStyle::default())
}

impl OwedScreen {
    /// The window over the open company, on this month.
    #[must_use]
    pub fn new(link: Arc<CompanyLink>, today: CalendarDate) -> OwedScreen {
        let group = link.group().and_then(Result::ok);
        let available =
            link.is_demo() || link.capabilities().has(Capabilities::SALES_TAX_LIABILITY);
        let (first, last) = month_of(today);
        let mut s = OwedScreen {
            link,
            group,
            available,
            today,
            from: DateInput::new(first),
            to: DateInput::new(last),
            basis: None,
            report: None,
            job: None,
            payments: Vec::new(),
            payments_job: None,
            open: None,
            error: None,
            problem: None,
            loads: 0,
        };
        s.refresh();
        s
    }

    /// Runs the report again for the dates and basis on screen.
    pub(crate) fn refresh(&mut self) {
        if !self.available {
            return;
        }
        let (Some(start), Some(end)) = (self.from.value, self.to.value) else {
            self.problem = Some("sales_tax.owed.bad_dates");
            return;
        };
        if end < start {
            self.problem = Some("sales_tax.owed.bad_range");
            return;
        }
        self.problem = None;
        self.error = None;
        let basis = self.basis;
        let q = LiabilityQuery {
            start_date: start,
            end_date: end,
            basis,
            agency_id: None,
        };
        self.job = Some(engine::read(
            &self.link,
            self.group.as_ref(),
            move |d| crate::company::demo::sales_tax_owed::liability(d, q),
            move |s| async move { s.reports().sales_tax_liability(q).await },
        ));
        self.payments_job = Some(engine::read(
            &self.link,
            self.group.as_ref(),
            move |d| {
                Ok(crate::company::demo::sales_tax_owed::list(d, None)
                    .into_iter()
                    .filter(|p| p.date >= start && p.date <= end)
                    .collect())
            },
            move |s| async move { s.reports().sales_tax_payments(start, end, None).await },
        ));
        self.poll();
    }

    fn busy(&self) -> bool {
        self.job.is_some() || self.payments_job.is_some()
    }

    pub(crate) fn poll(&mut self) {
        if let Some(r) = engine::poll(&mut self.job) {
            match r {
                Ok(v) => {
                    self.report = Some(v);
                    self.loads += 1;
                }
                Err(e) => self.error = Some(e),
            }
        }
        if let Some(r) = engine::poll(&mut self.payments_job) {
            self.payments = r.unwrap_or_default();
        }
    }

    /// The basis the report ran on.
    fn shown_basis(&self) -> Option<SalesTaxBasis> {
        self.report.as_ref().map(|r| r.basis)
    }

    fn agency_of(&self, item: SalesTaxItemId) -> Option<&LiabilityAgency> {
        self.report
            .as_ref()?
            .agencies
            .iter()
            .find(|a| a.items.iter().any(|i| i.tax_item_id == Some(item)))
    }

    /// Pay Sales Tax on `agency`.
    pub(crate) fn pay(&self, agency: Option<SalesTaxAgencyId>) {
        match agency {
            Some(a) => self.link.push(crate::company::CompanyEvent::OpenWith(
                CommandId::new(super::pay::KIND),
                Box::new(OpenPayload::PaySalesTax(a)),
            )),
            None => self
                .link
                .push(crate::company::CompanyEvent::OpenWindow(CommandId::new(
                    super::pay::KIND,
                ))),
        }
    }

    // ---- Drawing ----------------------------------------------------------------

    fn draw(&mut self, ui: &mut Ui) {
        let tokens = Tokens::of(ui.ctx());
        ui.label(RichText::new(t("sales_tax.owed.title")).size(22.0).strong());
        if !self.available {
            form::hint(ui, &tokens, &t("sales_tax.owed.needs_update"));
            return;
        }
        self.controls(ui);
        if let Some(p) = self.problem {
            form::inline_warning(ui, &tokens, &t(p));
        }
        if let Some(e) = &self.error {
            form::inline_error(ui, &tokens, e);
        }
        let Some(report) = self.report.clone() else {
            if self.error.is_none() {
                form::progress_line(ui, &tokens, &t("sales_tax.owed.loading"));
            }
            return;
        };
        let cash = report.hides_bad_debts();
        form::hint(
            ui,
            &tokens,
            &t(if cash {
                "sales_tax.owed.cash_note"
            } else {
                "sales_tax.owed.accrual_note"
            }),
        );
        if report.agencies.iter().all(|a| a.items.is_empty()) {
            form::hint(ui, &tokens, &t("sales_tax.owed.empty"));
            return;
        }
        ui.add_space(6.0);
        self.table(ui, &tokens, &report, cash);
        ui.add_space(8.0);
        Self::footer(ui, &tokens, &report);
        if let Some(item) = self.open {
            ui.add_space(8.0);
            self.detail(ui, &tokens, &report, item);
        }
    }

    fn controls(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            let a = form::date_field(ui, &t("sales_tax.owed.from"), &mut self.from, self.today);
            let b = form::date_field(ui, &t("sales_tax.owed.to"), &mut self.to, self.today);
            if a.lost_focus() || b.lost_focus() {
                self.refresh();
            }
            ui.separator();
            let label = |b: Option<SalesTaxBasis>| match b {
                None => t("sales_tax.owed.basis.company"),
                Some(SalesTaxBasis::Cash) => t("sales_tax.owed.basis.cash"),
                Some(_) => t("sales_tax.owed.basis.accrual"),
            };
            let mut picked = None;
            let l = ui.label(t("sales_tax.owed.basis"));
            egui::ComboBox::from_id_salt("sales_tax_owed_basis")
                .selected_text(label(self.basis))
                .show_ui(ui, |ui| {
                    for b in [
                        None,
                        Some(SalesTaxBasis::Accrual),
                        Some(SalesTaxBasis::Cash),
                    ] {
                        if ui.selectable_label(self.basis == b, label(b)).clicked() {
                            picked = Some(b);
                        }
                    }
                })
                .response
                .labelled_by(l.id);
            if let Some(b) = picked
                && b != self.basis
            {
                self.basis = b;
                self.refresh();
            }
            if ui.button(t("sales_tax.owed.refresh")).clicked() {
                self.refresh();
            }
            if ui.button(t("sales_tax.owed.pay")).clicked() {
                self.pay(None);
            }
        });
    }

    fn table(&mut self, ui: &mut Ui, tokens: &Tokens, report: &SalesTaxLiability, cash: bool) {
        let mut heads = vec![
            "sales_tax.owed.col.item",
            "sales_tax.owed.col.rate",
            "sales_tax.owed.col.taxable",
            "sales_tax.owed.col.non_taxable",
            "sales_tax.owed.col.collected",
            "sales_tax.owed.col.discounts",
        ];
        if !cash {
            heads.push("sales_tax.owed.col.bad_debts");
            heads.push("sales_tax.owed.col.recovered");
        }
        heads.extend([
            "sales_tax.owed.col.adjustments",
            "sales_tax.owed.col.paid",
            "sales_tax.owed.col.owed",
        ]);
        let mut clicked = None;
        let mut pay = None;
        egui::Grid::new("sales_tax_owed_grid")
            .striped(true)
            .spacing([14.0, 4.0])
            .show(ui, |ui| {
                for h in &heads {
                    ui.label(RichText::new(t(h)).strong().size(12.0));
                }
                ui.end_row();
                for a in &report.agencies {
                    if a.items.is_empty() {
                        continue;
                    }
                    let name = if a.name.is_empty() {
                        t("sales_tax.owed.no_agency")
                    } else {
                        a.name.clone()
                    };
                    ui.label(RichText::new(name).strong());
                    for _ in 1..heads.len() - 1 {
                        ui.label("");
                    }
                    if a.agency_id.is_some()
                        && ui
                            .button(t("sales_tax.owed.pay_agency"))
                            .on_hover_text(t("sales_tax.owed.pay_agency_tip"))
                            .clicked()
                    {
                        pay = Some(a.agency_id);
                    }
                    ui.end_row();
                    for i in &a.items {
                        if Self::item_row(ui, i, cash, self.open == i.tax_item_id) {
                            clicked = i.tax_item_id;
                        }
                        ui.end_row();
                    }
                    ui.label(RichText::new(t("sales_tax.owed.agency_total")).italics());
                    for _ in 0..3 {
                        ui.label("");
                    }
                    Self::columns_cells(ui, &a.totals, cash, true);
                    ui.end_row();
                }
            });
        let _ = tokens;
        if let Some(i) = clicked {
            self.open = if self.open == Some(i) { None } else { Some(i) };
        }
        if let Some(a) = pay {
            self.pay(a);
        }
    }

    /// One item row; true when its name was clicked.
    fn item_row(ui: &mut Ui, i: &LiabilityItem, cash: bool, open: bool) -> bool {
        let clicked = ui
            .selectable_label(open, &i.name)
            .on_hover_text(t("sales_tax.owed.item_tip"))
            .clicked();
        ui.label(
            i.rate
                .map(cl_core::TaxRate::format_percent)
                .unwrap_or_default(),
        );
        money_cell(ui, i.taxable_sales, false);
        let non = i
            .non_taxable
            .iter()
            .fold(None::<Money>, |_, n| Some(n.amount));
        // More than one code: the detail lists them; the cell names the
        // count (no sums here).
        if i.non_taxable.len() > 1 {
            ui.label(t_with(
                "sales_tax.owed.codes",
                &[("count", &i.non_taxable.len().to_string())],
            ));
        } else {
            money_cell(ui, non.unwrap_or(Money::ZERO), false);
        }
        Self::columns_cells(ui, &i.tax, cash, false);
        clicked
    }

    fn columns_cells(ui: &mut Ui, c: &LiabilityColumns, cash: bool, strong: bool) {
        money_cell(ui, c.collected, strong);
        money_cell(ui, c.discounts, strong);
        if !cash {
            money_cell(ui, c.bad_debts, strong);
            money_cell(ui, c.recoveries, strong);
        }
        money_cell(ui, c.adjustments, strong);
        money_cell(ui, c.paid, strong);
        money_cell(ui, c.owed_at_end, true);
    }

    fn footer(ui: &mut Ui, tokens: &Tokens, r: &SalesTaxLiability) {
        egui::Grid::new("sales_tax_owed_footer")
            .spacing([14.0, 2.0])
            .show(ui, |ui| {
                let mut row = |key: &str, m: Money| {
                    ui.label(t(key));
                    money_cell(ui, m, false);
                    ui.end_row();
                };
                if !r.opening_paid.is_zero() {
                    row("sales_tax.owed.opening_paid", r.opening_paid);
                }
                if !r.unassigned.is_zero() {
                    row("sales_tax.owed.unassigned", r.unassigned);
                }
                // The GL rows: absent when the report is filtered by agency.
                if let Some(m) = r.other_entries {
                    row("sales_tax.owed.other_entries", m);
                }
                if let Some(m) = r.gl_movement {
                    row("sales_tax.owed.gl_movement", m);
                }
                if let Some(b) = &r.balance_check {
                    row("sales_tax.owed.stp_balance", b.stp_balance);
                }
                if let Some(x) = r.rounding_difference {
                    ui.label(t("sales_tax.owed.rounding"))
                        .on_hover_text(t("sales_tax.owed.rounding_tip"));
                    ui.label(RichText::new(x.to_text()).monospace());
                    ui.end_row();
                }
            });
        let _ = tokens;
    }

    fn detail(&self, ui: &mut Ui, tokens: &Tokens, r: &SalesTaxLiability, item: SalesTaxItemId) {
        let Some(i) = r
            .agencies
            .iter()
            .flat_map(|a| a.items.iter())
            .find(|i| i.tax_item_id == Some(item))
        else {
            return;
        };
        let agency = self.agency_of(item).and_then(|a| a.agency_id);
        form::card(ui, tokens, |ui| {
            ui.label(RichText::new(&i.name).strong());
            ui.label(t("sales_tax.owed.detail.non_taxable"));
            if i.non_taxable.is_empty() {
                form::hint(ui, tokens, &t("sales_tax.owed.detail.none"));
            }
            for n in &i.non_taxable {
                ui.horizontal(|ui| {
                    ui.label(&n.code);
                    money_cell(ui, n.amount, false);
                });
            }
            ui.add_space(4.0);
            ui.label(t("sales_tax.owed.detail.payments"));
            let mine: Vec<&SalesTaxPaymentRecord> = self
                .payments
                .iter()
                .filter(|p| Some(p.agency_id) == agency)
                .collect();
            if mine.is_empty() {
                form::hint(ui, tokens, &t("sales_tax.owed.detail.no_payments"));
            }
            for p in mine {
                ui.horizontal(|ui| {
                    ui.label(crate::shell::us_date(p.date));
                    money_cell(ui, p.total, false);
                    if p.linked {
                        ui.label(t("sales_tax.owed.detail.linked"));
                    }
                    if p.voided {
                        ui.label(t("sales_tax.owed.detail.void"));
                    }
                });
            }
        });
    }

    fn state_json(&self) -> Value {
        let r = self.report.as_ref();
        let item = |name: &str| -> Value {
            r.and_then(|r| {
                r.agencies
                    .iter()
                    .flat_map(|a| a.items.iter())
                    .find(|i| i.name == name)
            })
            .map_or(Value::Null, |i| {
                json!({
                    "collected": i.tax.collected.to_wire(),
                    "discounts": i.tax.discounts.to_wire(),
                    "bad_debts": i.tax.bad_debts.to_wire(),
                    "paid": i.tax.paid.to_wire(),
                    "owed": i.tax.owed_at_end.to_wire(),
                })
            })
        };
        json!({
            "screen": "sales_tax_owed",
            "available": self.available,
            "basis": self.shown_basis().map(SalesTaxBasis::as_wire),
            "basis_override": self.basis.is_some(),
            "agencies": r.map_or(0, |r| r.agencies.iter().filter(|a| !a.items.is_empty()).count()),
            "items": r.map_or(0, |r| r.agencies.iter().map(|a| a.items.len()).sum::<usize>()),
            "bad_debt_columns": r.is_some_and(|r| !r.hides_bad_debts()),
            "state_item": item("State"),
            "springfield": item("Springfield"),
            "city": item("City"),
            "open": self.open.is_some(),
            "payments": self.payments.len(),
            "loads": self.loads,
            "problem": self.problem,
            "error": self.error.is_some(),
        })
    }
}

/// An amount, right-aligned in mono like every amount.
fn money_cell(ui: &mut Ui, m: Money, strong: bool) {
    let text = RichText::new(fmt(m)).monospace();
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.label(if strong { text.strong() } else { text });
    });
}

impl Screen for OwedScreen {
    fn title(&self) -> String {
        t("sales_tax.owed.title")
    }

    fn ui(&mut self, ui: &mut Ui) {
        self.poll();
        egui::ScrollArea::both().show(ui, |ui| self.draw(ui));
        if self.busy() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    fn scope(&self) -> Scope {
        Scope::Window
    }

    fn command(&mut self, id: CommandId) -> bool {
        match id.as_str() {
            KIND => true,
            "view.refresh" => {
                self.refresh();
                true
            }
            // Demo only: a few taxed documents to report on (the scenario).
            "dev.demo_sales_tax_story" => {
                let Some(mut d) = self.link.demo_state() else {
                    return false;
                };
                crate::company::demo::sales_tax_owed::story(&mut d);
                drop(d);
                self.refresh();
                true
            }
            _ => false,
        }
    }

    fn state(&self) -> Value {
        self.state_json()
    }
}

#[cfg(test)]
#[path = "liability_tests.rs"]
mod tests;
