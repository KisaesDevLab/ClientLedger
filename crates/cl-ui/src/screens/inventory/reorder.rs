//! Items to Reorder (P24 plan §3, `vendors.items_to_reorder`; engine U-78):
//! the active parts at or below their reorder point, grouped by preferred
//! vendor, with how many to order. Home's Needs attention line opens it.
//! Everything shown is the engine's.

use std::sync::Arc;

use cl_api::{ApiError, Capabilities, ReorderAlert, RequestGroup};
use cl_model::inventory::{ReorderList, ReorderRow};
use cl_model::{CommandId, Scope};
use eframe::egui::{self, RichText, Ui};
use serde_json::{Value, json};

use super::Books;
use crate::company::CompanyLink;
use crate::i18n::{t, t_with};
use crate::screens::company::form;
use crate::screens::invoice::engine::{self, Job};
use crate::theme::Tokens;
use crate::windows::Screen;

/// The command and window kind.
pub const KIND: &str = "vendors.items_to_reorder";

/// The Items to Reorder window.
pub struct ReorderScreen {
    link: Arc<CompanyLink>,
    group: Option<RequestGroup>,
    available: bool,
    books: Books,
    books_job: Option<Job<Books>>,
    alerts: Option<Vec<ReorderAlert>>,
    alerts_job: Option<Job<Vec<ReorderAlert>>>,
    list: ReorderList,
    error: Option<ApiError>,
}

impl ReorderScreen {
    /// The window over the open company.
    #[must_use]
    pub fn new(link: Arc<CompanyLink>) -> ReorderScreen {
        let group = link.group().and_then(Result::ok);
        let available = link.is_demo() || link.capabilities().has(Capabilities::REORDER_ALERTS);
        let mut s = ReorderScreen {
            books_job: Some(super::load_books(&link, group.as_ref())),
            link,
            group,
            available,
            books: Books::default(),
            alerts: None,
            alerts_job: None,
            list: ReorderList::default(),
            error: None,
        };
        s.refresh();
        s
    }

    fn refresh(&mut self) {
        if !self.available {
            return;
        }
        self.error = None;
        self.alerts_job = Some(engine::read(
            &self.link,
            self.group.as_ref(),
            crate::company::demo::stock::reorder_report,
            // The report form: the alerts with each part's purchase unit.
            |s| async move { s.inventory().reorder_report().await },
        ));
        self.poll();
    }

    fn poll(&mut self) {
        let mut rebuild = false;
        if let Some(r) = engine::poll(&mut self.books_job) {
            match r {
                Ok(b) => self.books = b,
                Err(e) => self.error = Some(e),
            }
            rebuild = true;
        }
        if let Some(r) = engine::poll(&mut self.alerts_job) {
            match r {
                Ok(a) => self.alerts = Some(a),
                Err(e) => self.error = Some(e),
            }
            rebuild = true;
        }
        if rebuild {
            self.build();
        }
    }

    fn build(&mut self) {
        let rows = self
            .alerts
            .iter()
            .flatten()
            .filter_map(|a| {
                Some(ReorderRow {
                    item: a.item_id,
                    name: a
                        .name
                        .clone()
                        .or_else(|| self.books.name(a.item_id))
                        .unwrap_or_default(),
                    on_hand: a.on_hand,
                    reorder_point: a.reorder_point?,
                    reorder_quantity: a.reorder_quantity,
                    in_purchase_unit: a
                        .purchase_unit
                        .as_ref()
                        .zip(a.reorder_quantity_in_purchase_unit)
                        .map(|(u, q)| (u.name.clone(), q)),
                    preferred_vendor: a.preferred_vendor_id,
                    average_cost: self
                        .books
                        .part(a.item_id)
                        .and_then(|p| p.stock.average_cost),
                })
            })
            .collect();
        self.list = ReorderList::new(rows);
    }

    fn draw(&mut self, ui: &mut Ui) {
        let tokens = Tokens::of(ui.ctx());
        ui.label(
            RichText::new(t("inventory.reorder.title"))
                .size(22.0)
                .strong(),
        );
        if !self.available {
            form::hint(ui, &tokens, &t("inventory.needs_update"));
            return;
        }
        form::hint(ui, &tokens, &t("inventory.reorder.intro"));
        if let Some(e) = &self.error {
            if let Some(text) = super::refusal_text(e, None) {
                form::inline_warning(ui, &tokens, &text);
            } else {
                form::inline_error(ui, &tokens, e);
            }
        }
        ui.horizontal(|ui| {
            if ui.button(t("inventory.refresh")).clicked() {
                self.refresh();
            }
            if ui.button(t("inventory.reorder.open_items")).clicked() {
                self.link
                    .push(crate::company::CompanyEvent::OpenWindow(CommandId::new(
                        crate::screens::lists::items::KIND,
                    )));
            }
        });
        ui.add_space(6.0);
        if self.alerts.is_none() {
            form::progress_line(ui, &tokens, &t("inventory.loading"));
            return;
        }
        if self.list.count() == 0 {
            form::hint(ui, &tokens, &t("inventory.reorder.empty"));
            return;
        }
        ui.label(
            RichText::new(t_with(
                "inventory.reorder.count",
                &[("n", &self.list.count().to_string())],
            ))
            .color(tokens.text_secondary),
        );
        egui::Grid::new("inventory_reorder")
            .striped(true)
            .spacing([16.0, 4.0])
            .show(ui, |ui| {
                for h in [
                    "inventory.reorder.col.vendor",
                    "inventory.reorder.col.item",
                    "inventory.reorder.col.on_hand",
                    "inventory.reorder.col.point",
                    "inventory.reorder.col.order",
                    "inventory.reorder.col.cost",
                ] {
                    super::head(ui, h);
                }
                ui.end_row();
                let mut last: Option<Option<cl_core::ids::ContactId>> = None;
                for r in self.list.rows() {
                    let vendor = if last == Some(r.preferred_vendor) {
                        String::new()
                    } else if r.preferred_vendor.is_none() {
                        t("inventory.reorder.no_vendor")
                    } else {
                        self.books.vendor_name(r.preferred_vendor)
                    };
                    last = Some(r.preferred_vendor);
                    ui.label(RichText::new(vendor).strong());
                    ui.label(&r.name);
                    super::num(
                        ui,
                        RichText::new(super::qty(r.on_hand))
                            .monospace()
                            .color(tokens.warn_text),
                    );
                    super::num(ui, RichText::new(super::qty(r.reorder_point)).monospace());
                    // U-161: "48 (4 case)" for a part bought by the case.
                    let mut order = r.reorder_quantity.map(super::qty).unwrap_or_default();
                    if let Some((unit, q)) = &r.in_purchase_unit {
                        order = t_with(
                            "inventory.reorder.in_unit",
                            &[("qty", &order), ("n", &super::qty(*q)), ("unit", unit)],
                        );
                    }
                    super::num(ui, RichText::new(order).monospace());
                    super::num(
                        ui,
                        RichText::new(r.average_cost.map(super::cost).unwrap_or_default())
                            .monospace(),
                    );
                    ui.end_row();
                }
            });
    }
}

impl Screen for ReorderScreen {
    fn title(&self) -> String {
        t("inventory.reorder.title")
    }

    fn ui(&mut self, ui: &mut Ui) {
        self.poll();
        egui::ScrollArea::both().show(ui, |ui| {
            self.draw(ui);
            crate::layout::keep_focus_in_view(ui);
        });
        if self.alerts_job.is_some() || self.books_job.is_some() {
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
        json!({
            "screen": "inventory_reorder",
            "available": self.available,
            "loaded": self.alerts.is_some(),
            "rows": self.list.count(),
            "error": self.error.is_some(),
        })
    }
}
