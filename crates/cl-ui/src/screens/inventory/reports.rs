//! The inventory reports (P24 plan §3, Q-P24n; engine U-160):
//!
//! - **Inventory Value** as of a date: on hand, average cost and value per
//!   part, the engine's total, and per asset account the check against the
//!   books (accrual GL, Q-P24m) with the cash-basis note when the engine
//!   sends one.
//! - **Inventory Value Detail** over dates, for every part or one: each
//!   movement with the running quantity, value and average; a row opens
//!   its document.
//! - **Stock Status** as of a date: on hand against the reorder point.
//!
//! Read-only grids of the engine's strings; the screen adds nothing up.
//! Print and export come with P28. Detail pages at 500 rows (plan §5).

use std::sync::Arc;

use cl_api::{
    ApiError, Capabilities, RequestGroup, StockStatusRow, TxnType, ValuationDetailRow,
    ValuationSummary,
};
use cl_core::CalendarDate;
use cl_core::ids::{ItemId, TransactionId};
use cl_model::{CommandId, Scope};
use eframe::egui::{self, RichText, Ui};
use serde_json::{Value, json};

use super::Books;
use crate::company::{CompanyEvent, CompanyLink};
use crate::i18n::{t, t_with};
use crate::screens::company::form::{self, DateInput};
use crate::screens::invoice::engine::{self, Job};
use crate::theme::Tokens;
use crate::windows::Screen;

/// Inventory Value.
pub const VALUE_KIND: &str = "reports.inventory_value";
/// Inventory Value Detail.
pub const DETAIL_KIND: &str = "reports.inventory_value_detail";
/// Stock Status.
pub const STOCK_KIND: &str = "reports.stock_status";

/// Detail rows shown per page (plan §5).
pub const PAGE: usize = 500;

/// Which report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Report {
    /// Inventory Value (summary with the GL check).
    Value,
    /// Inventory Value Detail.
    Detail,
    /// Stock Status.
    Stock,
}

impl Report {
    fn kind(self) -> &'static str {
        match self {
            Report::Value => VALUE_KIND,
            Report::Detail => DETAIL_KIND,
            Report::Stock => STOCK_KIND,
        }
    }

    fn title(self) -> &'static str {
        match self {
            Report::Value => "inventory.report.value.title",
            Report::Detail => "inventory.report.detail.title",
            Report::Stock => "inventory.report.stock.title",
        }
    }
}

enum Data {
    Value(ValuationSummary),
    Detail(Vec<ValuationDetailRow>),
    Stock(Vec<StockStatusRow>),
}

/// An inventory report window.
pub struct ReportScreen {
    link: Arc<CompanyLink>,
    group: Option<RequestGroup>,
    report: Report,
    available: bool,
    today: CalendarDate,
    books: Books,
    books_job: Option<Job<Books>>,
    /// As of (Value, Stock) or the end of the range (Detail).
    pub(crate) to: DateInput,
    /// The start of the range (Detail).
    pub(crate) from: DateInput,
    /// One part (Detail; `None` for every part).
    pub(crate) item: Option<ItemId>,
    data: Option<Data>,
    job: Option<Job<Data>>,
    shown: usize,
    error: Option<ApiError>,
    problem: Option<&'static str>,
    note: Option<&'static str>,
    /// QA-10 (PR-4): other windows' saves run the report again.
    bus: crate::data_bus::Subscription,
}

impl ReportScreen {
    /// The report over the open company, as of `today` (Detail: this
    /// year to `today`).
    #[must_use]
    pub fn new(link: Arc<CompanyLink>, report: Report, today: CalendarDate) -> ReportScreen {
        let group = link.group().and_then(Result::ok);
        let available = link.is_demo() || link.capabilities().has(Capabilities::INVENTORY_REPORTS);
        let start = CalendarDate::from_ymd(today.year(), 1, 1).unwrap_or(today);
        let mut s = ReportScreen {
            books_job: Some(super::load_books(&link, group.as_ref())),
            link,
            group,
            report,
            available,
            today,
            books: Books::default(),
            to: DateInput::new(today),
            from: DateInput::new(start),
            item: None,
            data: None,
            job: None,
            shown: PAGE,
            error: None,
            problem: None,
            note: None,
            bus: crate::data_bus::Subscription::detached(),
        };
        s.bus = s.link.bus().subscribe();
        s.refresh();
        s
    }

    /// QA-10 (PR-4): a posting elsewhere (a data-bus notice) runs the
    /// open report again; a notice that comes while it runs waits.
    pub(crate) fn follow_bus(&mut self) {
        if self.job.is_none() && !self.link.bus().take(&mut self.bus).is_empty() {
            self.refresh();
        }
    }

    /// Runs the report for the dates on screen.
    pub(crate) fn refresh(&mut self) {
        if !self.available {
            return;
        }
        self.error = None;
        self.problem = None;
        self.shown = PAGE;
        let Some(to) = self.to.value else {
            self.problem = Some("inventory.report.bad_date");
            return;
        };
        self.job = Some(match self.report {
            Report::Value => engine::read(
                &self.link,
                self.group.as_ref(),
                move |d| crate::company::demo::stock::valuation(d, to).map(Data::Value),
                move |s| async move { s.inventory().valuation(to).await.map(Data::Value) },
            ),
            Report::Stock => engine::read(
                &self.link,
                self.group.as_ref(),
                move |d| crate::company::demo::stock::stock_status(d, to).map(Data::Stock),
                move |s| async move { s.inventory().stock_status(to).await.map(Data::Stock) },
            ),
            Report::Detail => {
                let Some(from) = self.from.value else {
                    self.problem = Some("inventory.report.bad_date");
                    return;
                };
                if to < from {
                    self.problem = Some("inventory.report.bad_range");
                    return;
                }
                let item = self.item;
                engine::read(
                    &self.link,
                    self.group.as_ref(),
                    move |d| {
                        crate::company::demo::stock::valuation_detail(d, from, to, item)
                            .map(Data::Detail)
                    },
                    move |s| async move {
                        s.inventory()
                            .valuation_detail(from, to, item)
                            .await
                            .map(Data::Detail)
                    },
                )
            }
        });
        self.poll();
    }

    fn poll(&mut self) {
        if let Some(r) = engine::poll(&mut self.books_job) {
            match r {
                Ok(b) => self.books = b,
                Err(e) => self.error = Some(e),
            }
        }
        if let Some(r) = engine::poll(&mut self.job) {
            match r {
                Ok(d) => self.data = Some(d),
                Err(e) => self.error = Some(e),
            }
        }
    }

    /// Opens a detail row's document in its form (adjustments have none
    /// yet: a note says so).
    pub(crate) fn open(&mut self, txn: TransactionId, kind: Option<TxnType>) {
        let form = match kind {
            Some(TxnType::Bill | TxnType::VendorCredit) => Some("vendors.enter_bill"),
            Some(TxnType::Invoice) => Some("clients.create_invoice"),
            Some(TxnType::CreditMemo) => Some("clients.credit_memo"),
            Some(TxnType::CashSale) => Some("clients.sales_receipt"),
            Some(TxnType::Expense) => Some("vendors.write_check"),
            _ => None,
        };
        match form {
            Some(f) => self
                .link
                .push(CompanyEvent::OpenRecord(CommandId::new(f), txn)),
            None => self.note = Some("inventory.report.detail.no_form"),
        }
    }

    // ---- Drawing ----------------------------------------------------------------

    fn draw(&mut self, ui: &mut Ui) {
        let tokens = Tokens::of(ui.ctx());
        ui.label(RichText::new(t(self.report.title())).size(22.0).strong());
        if !self.available {
            form::hint(ui, &tokens, &t("inventory.needs_update"));
            return;
        }
        self.controls(ui);
        if let Some(p) = self.problem {
            form::inline_warning(ui, &tokens, &t(p));
        }
        if let Some(n) = self.note {
            form::hint(ui, &tokens, &t(n));
        }
        if let Some(e) = &self.error {
            match super::refusal_text(e, None) {
                Some(text) => form::inline_warning(ui, &tokens, &text),
                None => form::inline_error(ui, &tokens, e),
            }
        }
        ui.add_space(6.0);
        let Some(data) = self.data.take() else {
            if self.error.is_none() {
                form::progress_line(ui, &tokens, &t("inventory.loading"));
            }
            return;
        };
        match &data {
            Data::Value(v) => self.value(ui, &tokens, v),
            Data::Detail(rows) => self.detail(ui, &tokens, rows),
            Data::Stock(rows) => self.stock(ui, &tokens, rows),
        }
        self.data = Some(data);
    }

    fn controls(&mut self, ui: &mut Ui) {
        let mut run = false;
        ui.horizontal_wrapped(|ui| {
            if self.report == Report::Detail {
                let a =
                    form::date_field(ui, &t("inventory.report.from"), &mut self.from, self.today);
                let b = form::date_field(ui, &t("inventory.report.to"), &mut self.to, self.today);
                run |= a.lost_focus() || b.lost_focus();
                let parts = self.books.parts.clone();
                let shown = self
                    .item
                    .and_then(|i| self.books.name(i))
                    .unwrap_or_else(|| t("inventory.report.all_items"));
                let l = ui.label(t("inventory.report.item"));
                let mut pick = None;
                egui::ComboBox::from_id_salt("inventory_detail_item")
                    .selected_text(shown)
                    .width(220.0)
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(self.item.is_none(), t("inventory.report.all_items"))
                            .clicked()
                        {
                            pick = Some(None);
                        }
                        for p in &parts {
                            if ui
                                .selectable_label(self.item == Some(p.id), &p.fields.name)
                                .clicked()
                            {
                                pick = Some(Some(p.id));
                            }
                        }
                    })
                    .response
                    .labelled_by(l.id);
                if let Some(p) = pick {
                    self.item = p;
                    run = true;
                }
            } else {
                let r =
                    form::date_field(ui, &t("inventory.report.as_of"), &mut self.to, self.today);
                run |= r.lost_focus();
            }
            if ui.button(t("inventory.refresh")).clicked() {
                run = true;
            }
        });
        if run {
            self.refresh();
        }
    }

    fn value(&mut self, ui: &mut Ui, tokens: &Tokens, v: &ValuationSummary) {
        if let Some(note) = &v.basis_note {
            form::hint(ui, tokens, &t("inventory.report.value.cash_note"));
            if !note.is_empty() {
                ui.label(RichText::new(note).color(tokens.text_secondary).size(12.0));
            }
        }
        if v.rows.is_empty() {
            form::hint(ui, tokens, &t("inventory.report.empty"));
            return;
        }
        egui::Grid::new("inventory_value")
            .striped(true)
            .spacing([18.0, 4.0])
            .show(ui, |ui| {
                for h in [
                    "inventory.report.col.item",
                    "inventory.report.col.on_hand",
                    "inventory.report.col.average_cost",
                    "inventory.report.col.value",
                    "inventory.report.col.percent",
                ] {
                    super::head(ui, h);
                }
                ui.end_row();
                for r in &v.rows {
                    ui.label(
                        r.name
                            .clone()
                            .or_else(|| self.books.name(r.item_id))
                            .unwrap_or_default(),
                    );
                    super::num(ui, RichText::new(super::qty(r.on_hand)).monospace());
                    super::num(
                        ui,
                        RichText::new(r.average_cost.map(super::cost).unwrap_or_default())
                            .monospace(),
                    );
                    super::num(ui, RichText::new(super::money(r.value)).monospace());
                    super::num(
                        ui,
                        RichText::new(r.percent_of_total.clone().unwrap_or_default()).monospace(),
                    );
                    ui.end_row();
                }
                if let Some(total) = v.total_value {
                    ui.label(RichText::new(t("inventory.report.total")).strong());
                    ui.label("");
                    ui.label("");
                    super::num(ui, RichText::new(super::money(total)).monospace().strong());
                    ui.label("");
                    ui.end_row();
                }
            });
        ui.add_space(8.0);
        ui.label(RichText::new(t("inventory.report.value.check")).strong());
        for a in &v.accounts {
            let name = self.books.account_name(a.account_id);
            let line = t_with(
                "inventory.report.value.check_line",
                &[
                    ("account", &name),
                    ("valuation", &super::money(a.valuation)),
                    ("books", &super::money(a.gl_balance)),
                ],
            );
            ui.label(line);
            if a.difference.is_zero() {
                ui.label(
                    RichText::new(t("inventory.report.value.matches")).color(tokens.text_secondary),
                );
            } else {
                form::inline_warning(
                    ui,
                    tokens,
                    &t_with(
                        "inventory.report.value.differs",
                        &[("difference", &super::money(a.difference))],
                    ),
                );
            }
        }
    }

    fn detail(&mut self, ui: &mut Ui, tokens: &Tokens, rows: &[ValuationDetailRow]) {
        if rows.is_empty() {
            form::hint(ui, tokens, &t("inventory.report.empty"));
            return;
        }
        let mut open = None;
        egui::Grid::new("inventory_value_detail")
            .striped(true)
            .spacing([14.0, 4.0])
            .show(ui, |ui| {
                for h in [
                    "inventory.report.col.date",
                    "inventory.report.col.type",
                    "inventory.report.col.item",
                    "inventory.report.col.quantity",
                    "inventory.report.col.value",
                    "inventory.report.col.running_quantity",
                    "inventory.report.col.running_value",
                    "inventory.report.col.average_cost",
                ] {
                    super::head(ui, h);
                }
                ui.end_row();
                for r in rows.iter().take(self.shown) {
                    let date = crate::shell::us_date(r.date);
                    if ui
                        .link(&date)
                        .on_hover_text(t("inventory.report.detail.open"))
                        .clicked()
                    {
                        open = Some((r.transaction_id, r.txn_type));
                    }
                    ui.label(type_text(r.txn_type));
                    ui.label(self.books.name(r.item_id).unwrap_or_default());
                    super::num(ui, RichText::new(super::qty(r.quantity)).monospace());
                    super::num(ui, RichText::new(super::money(r.value)).monospace());
                    let q = RichText::new(super::qty(r.running_quantity)).monospace();
                    super::num(
                        ui,
                        if r.running_quantity.is_negative() {
                            q.color(tokens.warn_text)
                        } else {
                            q
                        },
                    );
                    super::num(ui, RichText::new(super::money(r.running_value)).monospace());
                    super::num(
                        ui,
                        RichText::new(r.average_cost.map(super::cost).unwrap_or_default())
                            .monospace(),
                    );
                    ui.end_row();
                }
            });
        if rows.len() > self.shown {
            ui.label(
                RichText::new(t_with(
                    "inventory.report.more",
                    &[
                        ("shown", &self.shown.to_string()),
                        ("total", &rows.len().to_string()),
                    ],
                ))
                .color(tokens.text_secondary),
            );
            if ui.button(t("inventory.report.show_more")).clicked() {
                self.shown += PAGE;
            }
        }
        if let Some((txn, kind)) = open {
            self.open(txn, kind);
        }
    }

    fn stock(&mut self, ui: &mut Ui, tokens: &Tokens, rows: &[StockStatusRow]) {
        if rows.is_empty() {
            form::hint(ui, tokens, &t("inventory.report.empty"));
            return;
        }
        egui::Grid::new("inventory_stock_status")
            .striped(true)
            .spacing([16.0, 4.0])
            .show(ui, |ui| {
                for h in [
                    "inventory.report.col.item",
                    "inventory.report.col.description",
                    "inventory.report.col.vendor",
                    "inventory.report.col.unit",
                    "inventory.report.col.reorder_point",
                    "inventory.report.col.on_hand",
                    "inventory.report.col.order",
                    "inventory.report.col.reorder_quantity",
                ] {
                    super::head(ui, h);
                }
                ui.end_row();
                for r in rows {
                    ui.label(
                        r.name
                            .clone()
                            .or_else(|| self.books.name(r.item_id))
                            .unwrap_or_default(),
                    );
                    ui.label(r.description.clone().unwrap_or_default());
                    ui.label(self.books.vendor_name(r.preferred_vendor_id));
                    ui.label(r.unit_label.clone().unwrap_or_default());
                    super::num(
                        ui,
                        RichText::new(r.reorder_point.map(super::qty).unwrap_or_default())
                            .monospace(),
                    );
                    super::num(ui, RichText::new(super::qty(r.on_hand)).monospace());
                    if r.reorder == Some(true) {
                        ui.label(
                            RichText::new(t("inventory.report.order_now"))
                                .color(tokens.warn_text)
                                .strong(),
                        );
                    } else {
                        ui.label("");
                    }
                    super::num(
                        ui,
                        RichText::new(r.reorder_quantity.map(super::qty).unwrap_or_default())
                            .monospace(),
                    );
                    ui.end_row();
                }
            });
    }

    fn state_json(&self) -> Value {
        let (rows, matches, basis_note) = match &self.data {
            Some(Data::Value(v)) => (
                v.rows.len(),
                Some(v.accounts.iter().all(|a| a.difference.is_zero())),
                v.basis_note.is_some(),
            ),
            Some(Data::Detail(r)) => (r.len(), None, false),
            Some(Data::Stock(r)) => (r.len(), None, false),
            None => (0, None, false),
        };
        let total = match &self.data {
            Some(Data::Value(v)) => v.total_value.map(cl_core::Money::to_wire),
            _ => None,
        };
        json!({
            "screen": match self.report {
                Report::Value => "inventory_value",
                Report::Detail => "inventory_value_detail",
                Report::Stock => "stock_status",
            },
            "available": self.available,
            "loaded": self.data.is_some(),
            "rows": rows,
            "total": total,
            "matches_books": matches,
            "basis_note": basis_note,
            "item_filter": self.item.is_some(),
            "error": self.error.is_some(),
            "problem": self.problem.map(|p| p.rsplit('.').next().unwrap_or_default()),
            "note": self.note.map(|p| p.rsplit('.').next().unwrap_or_default()),
        })
    }
}

fn type_text(k: Option<TxnType>) -> String {
    t(match k {
        Some(TxnType::Bill) => "inventory.txn.bill",
        Some(TxnType::VendorCredit) => "inventory.txn.vendor_credit",
        Some(TxnType::Invoice) => "inventory.txn.invoice",
        Some(TxnType::CreditMemo) => "inventory.txn.credit_memo",
        Some(TxnType::CashSale) => "inventory.txn.sales_receipt",
        Some(TxnType::Expense) => "inventory.txn.check",
        Some(TxnType::InventoryAdjustment) => "inventory.txn.adjustment",
        _ => "inventory.txn.other",
    })
}

impl Screen for ReportScreen {
    fn title(&self) -> String {
        t(self.report.title())
    }

    fn ui(&mut self, ui: &mut Ui) {
        self.poll();
        self.follow_bus();
        egui::ScrollArea::both().show(ui, |ui| self.draw(ui));
        if self.job.is_some() || self.books_job.is_some() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    fn scope(&self) -> Scope {
        Scope::Window
    }

    fn command(&mut self, id: CommandId) -> bool {
        match id.as_str() {
            k if k == self.report.kind() => true,
            "view.refresh" => {
                self.refresh();
                true
            }
            super::DEV_RESET => {
                let ok = super::demo_reset(&self.link);
                self.books_job = Some(super::load_books(&self.link, self.group.as_ref()));
                self.refresh();
                ok
            }
            _ => false,
        }
    }

    fn state(&self) -> Value {
        self.state_json()
    }
}

#[cfg(test)]
#[path = "reports_tests.rs"]
mod tests;
