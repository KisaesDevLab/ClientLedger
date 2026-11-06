//! Return of (P24 WP-3; Q-P24e, Q-P24u, Q-P24x): on a credit memo, each
//! inventory-part line may name the invoice or sales receipt it returns.
//! A linked line comes back at that sale's cost (the engine's cumulative
//! difference); an unlinked one at the current average cost.
//!
//! The choices are the client's sales of that part: in the demo its
//! invoices and sales receipts with the item; on a live engine the
//! client's invoices (the engine has no sales receipt list and no line
//! search yet, so the engine refuses a sale without the item with
//! `RETURN_EXCEEDS_SALE`). Quantities left to return are the engine's to
//! check; the form shows none it would have to work out itself.

use std::collections::BTreeMap;

use cl_api::{InvoiceListQuery, PageRequest};
use cl_core::CalendarDate;
use cl_core::ids::{ContactId, ItemId, TransactionId};
use cl_model::forms::invoice::SalesDocKind;
use eframe::egui::{self, RichText, Ui};

use super::InvoiceScreen;
use crate::company::Task;
use crate::i18n::{t, t_with};
use crate::theme::Tokens;

/// One sale a line can return: its id and how the picker names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SaleOption {
    pub id: TransactionId,
    pub label: String,
}

/// The sales read per (client, part).
#[derive(Default)]
pub(crate) struct Returns {
    options: BTreeMap<(ContactId, ItemId), Vec<SaleOption>>,
    task: Option<(ContactId, ItemId, Task<Vec<SaleOption>>)>,
}

fn label(receipt: bool, number: Option<&str>, date: CalendarDate) -> String {
    let key = if receipt {
        "credit_memo.return_of.receipt"
    } else {
        "credit_memo.return_of.invoice"
    };
    t_with(
        key,
        &[
            ("number", number.unwrap_or("")),
            ("date", &crate::shell::us_date(date)),
        ],
    )
}

impl InvoiceScreen {
    /// The part lines of a credit memo: row, part and its link.
    fn return_lines(&self) -> Vec<(cl_model::rowkey::RowKey, ItemId, Option<TransactionId>)> {
        if self.kind != SalesDocKind::CreditMemo {
            return Vec::new();
        }
        self.grid
            .value()
            .lines
            .iter()
            .filter_map(|l| {
                let item = l.item?;
                self.items
                    .iter()
                    .any(|i| i.id == item && i.fields.item_type == cl_api::ItemType::InventoryPart)
                    .then_some((l.key, item, l.return_of))
            })
            .collect()
    }

    /// Reads the sales of `item` to `client` (the demo at once).
    fn load_sales(&mut self, client: ContactId, item: ItemId) {
        if let Some(d) = self.link.demo_state() {
            let mut out: Vec<SaleOption> =
                crate::company::demo::invoices::invoices_with_item(&d, client, item)
                    .into_iter()
                    .map(|(id, n, date)| SaleOption {
                        id,
                        label: label(false, n.as_deref(), date),
                    })
                    .collect();
            out.extend(
                crate::company::demo::credit_memos::receipts_with_item(&d, client, item)
                    .into_iter()
                    .map(|(id, n, date)| SaleOption {
                        id,
                        label: label(true, n.as_deref(), date),
                    }),
            );
            // Only sales the costing ledger holds (as the engine's
            // movements: a sale of a part always has one).
            out.retain(|o| {
                let key = o.id.to_string();
                d.inventory
                    .moves()
                    .iter()
                    .any(|m| m.doc == key && m.item == item)
            });
            drop(d);
            self.returns.options.insert((client, item), out);
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        if let Ok(p) = group.read(&session, move |s| async move {
            let q = InvoiceListQuery {
                contact: Some(client),
                ..InvoiceListQuery::default()
            };
            let page = s.invoices().list(&q, PageRequest::first_max()).await?;
            Ok(page
                .data
                .into_iter()
                .map(|i| SaleOption {
                    id: i.id,
                    label: label(false, i.txn_number.as_deref(), i.txn_date),
                })
                .collect())
        }) {
            self.returns.task = Some((client, item, Task::Pending(p)));
        }
    }

    /// Polls the read; starts the next one a part line needs.
    pub(crate) fn poll_returns(&mut self) {
        if let Some((c, i, task)) = self.returns.task.as_mut()
            && let Some(r) = task.try_take()
        {
            let key = (*c, *i);
            self.returns.task = None;
            self.returns.options.insert(key, r.unwrap_or_default());
        }
        let Some(client) = self.header.current().client else {
            return;
        };
        if self.returns.task.is_some() {
            return;
        }
        let missing = self
            .return_lines()
            .into_iter()
            .map(|(_, item, _)| item)
            .find(|i| !self.returns.options.contains_key(&(client, *i)));
        if let Some(item) = missing {
            self.load_sales(client, item);
        }
    }

    /// The Return of pickers under the grid (credit memos with parts).
    pub(crate) fn returns_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let lines = self.return_lines();
        if lines.is_empty() {
            return;
        }
        let client = self.header.current().client;
        ui.add_space(6.0);
        ui.label(RichText::new(t("credit_memo.return_of.title")).strong());
        ui.label(
            RichText::new(t("credit_memo.return_of.hint"))
                .size(12.0)
                .color(tokens.text_secondary),
        );
        let none = t("credit_memo.return_of.none");
        let mut picked: Option<(cl_model::rowkey::RowKey, Option<TransactionId>)> = None;
        for (n, (key, item, link)) in lines.iter().enumerate() {
            let options = client
                .and_then(|c| self.returns.options.get(&(c, *item)))
                .cloned()
                .unwrap_or_default();
            let name = self
                .items
                .iter()
                .find(|i| i.id == *item)
                .map(|i| i.fields.name.clone())
                .unwrap_or_default();
            let shown = link
                .and_then(|l| options.iter().find(|o| o.id == l).map(|o| o.label.clone()))
                .unwrap_or_else(|| none.clone());
            ui.horizontal(|ui| {
                ui.label(name);
                let label = t_with("credit_memo.return_of.line", &[("n", &(n + 1).to_string())]);
                egui::ComboBox::from_label(label)
                    .selected_text(shown)
                    .show_ui(ui, |ui| {
                        if ui.selectable_label(link.is_none(), &none).clicked() {
                            picked = Some((*key, None));
                        }
                        for o in &options {
                            if ui.selectable_label(*link == Some(o.id), &o.label).clicked() {
                                picked = Some((*key, Some(o.id)));
                            }
                        }
                    });
            });
        }
        if let Some((key, sale)) = picked {
            // The open cell first, so its edit can't replay over this one.
            let _ = self.grid.commit();
            self.grid.edit(|f, _| {
                if let Some(l) = f.lines.iter_mut().find(|l| l.key == key) {
                    l.return_of = sale;
                }
            });
        }
    }

    /// The control channel's view: part lines and how many are linked.
    pub(crate) fn returns_json(&self) -> serde_json::Value {
        let lines = self.return_lines();
        serde_json::json!({
            "part_lines": lines.len(),
            "linked": lines.iter().filter(|(_, _, l)| l.is_some()).count(),
        })
    }
}
