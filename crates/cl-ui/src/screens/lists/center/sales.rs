//! The client form's Sales section (P20 WP-B step 2): the client's price
//! level (U-40, `price-levels-v1`, Q-P20l) and default markup on billable
//! costs (U-128, `billable-costs-v1`, Q-P20b; contract only until E3).
//! Read when the form opens, written after the contact saves (`PUT
//! /contacts/{id}` with only these fields).

use std::sync::Arc;

use cl_api::{
    ApiError, Capabilities, ClientSales, OrphanLabel, PriceLevel, PriceLevelId, RequestGroup,
    SignedPercent,
};
use cl_core::ids::{ContactId, SalesTaxItemId, TaxCodeId};
use eframe::egui::{self, RichText, Ui};
use serde_json::{Value, json};

use crate::company::CompanyLink;
use crate::i18n::t;
use crate::screens::invoice::engine::{self, Job};
use crate::screens::lists::detail_pane as pane;
use crate::theme::Tokens;

/// The section's state.
#[derive(Default)]
pub(crate) struct SalesEdit {
    /// The engine can keep a level / a markup.
    levels_on: bool,
    markup_on: bool,
    /// The engine keeps a client's default invoice layout (U-130).
    layouts_on: bool,
    layouts: Vec<cl_api::InvoiceLayout>,
    pub(crate) layout: Option<cl_api::InvoiceLayoutId>,
    layout_before: Option<cl_api::InvoiceLayoutId>,
    layouts_read: Option<Job<Vec<cl_api::InvoiceLayout>>>,
    layout_write: Option<Job<()>>,
    levels: Vec<PriceLevel>,
    pub(crate) level: Option<PriceLevelId>,
    pub(crate) markup: String,
    before: (Option<PriceLevelId>, String),
    read: Option<Job<(Vec<PriceLevel>, ClientSales)>>,
    write: Option<Job<()>>,
    pub(crate) problem: Option<&'static str>,
    /// Where a failed write is reported once the form has closed (S9).
    link: Option<Arc<CompanyLink>>,
    /// P23 (sales-tax-setup-v1): the client's default sales tax and tax
    /// code, with the company's taxes and codes to pick from.
    tax_on: bool,
    tax_items: Vec<cl_api::SalesTaxItem>,
    tax_codes: Vec<cl_api::TaxCode>,
    pub(crate) sales_tax: Option<SalesTaxItemId>,
    pub(crate) tax_code: Option<TaxCodeId>,
    tax_before: (Option<SalesTaxItemId>, Option<TaxCodeId>),
    tax_read: Option<Job<TaxRead>>,
    tax_write: Option<Job<()>>,
}

/// The lists and the client's saved defaults.
type TaxRead = (
    Vec<cl_api::SalesTaxItem>,
    Vec<cl_api::TaxCode>,
    (Option<SalesTaxItemId>, Option<TaxCodeId>),
);

impl SalesEdit {
    /// The section of `client` (`None`: a new client).
    pub(crate) fn open(
        link: &Arc<CompanyLink>,
        group: Option<&RequestGroup>,
        client: Option<ContactId>,
    ) -> SalesEdit {
        let caps = link.capabilities();
        let mut s = SalesEdit {
            levels_on: link.is_demo() || caps.has(Capabilities::PRICE_LEVELS),
            markup_on: link.is_demo() || caps.has(Capabilities::BILLABLE_COSTS),
            layouts_on: link.is_demo() || caps.has(Capabilities::INVOICE_LAYOUTS),
            tax_on: link.is_demo() || caps.has(Capabilities::SALES_TAX_SETUP),
            ..SalesEdit::default()
        };
        if s.tax_on {
            s.tax_read = Some(engine::read(
                link,
                group,
                move |d| {
                    let saved = client
                        .and_then(|c| d.contacts_mut().iter().find(|x| x.id == c).cloned())
                        .map_or((None, None), |c| {
                            (c.fields.default_sales_tax_id, c.fields.tax_code_id)
                        });
                    Ok((
                        crate::company::demo::sales_tax::items(d, false),
                        crate::company::demo::sales_tax::codes(d, false),
                        saved,
                    ))
                },
                move |s| async move {
                    let st = s.sales_tax();
                    let items = st.items(false).await?;
                    let codes = st.codes(false).await?;
                    let saved = match client {
                        Some(c) => {
                            let v = s.contacts().get(c).await?;
                            (
                                v.record.fields.default_sales_tax_id,
                                v.record.fields.tax_code_id,
                            )
                        }
                        None => (None, None),
                    };
                    Ok((items, codes, saved))
                },
            ));
        }
        if s.layouts_on {
            s.layouts_read = Some(engine::read(
                link,
                group,
                |d| Ok(crate::company::demo::layouts::list(d, true)),
                |s| async move { s.invoice_layouts().list(true).await },
            ));
        }
        if !s.levels_on && !s.markup_on {
            s.poll();
            return s;
        }
        let levels_on = s.levels_on;
        let has_balance = link.is_demo() || caps.has(Capabilities::CONTACT_OPEN_BALANCE);
        s.read = Some(engine::read(
            link,
            group,
            |d| {
                let sales = client.map_or_else(ClientSales::default, |c| {
                    crate::company::demo::invoices::client_sales(d, c)
                });
                Ok((d.price_levels.levels.clone(), sales))
            },
            move |s| async move {
                let levels = if levels_on {
                    s.invoices().price_levels().list().await?
                } else {
                    Vec::new()
                };
                let sales = match client {
                    Some(c) if has_balance => s.invoices().client_sales(c).await?,
                    _ => ClientSales::default(),
                };
                Ok((levels, sales))
            },
        ));
        s.poll();
        s
    }

    pub(crate) fn poll(&mut self) {
        if let Some(r) = engine::poll(&mut self.tax_read) {
            match r {
                Ok((items, codes, saved)) => {
                    self.tax_items = items;
                    self.tax_codes = codes;
                    (self.sales_tax, self.tax_code) = saved;
                    self.tax_before = saved;
                }
                Err(_) => self.problem = Some("center.sales.load_failed"),
            }
        }
        match engine::poll(&mut self.tax_write) {
            Some(Ok(())) => self.tax_before = (self.sales_tax, self.tax_code),
            Some(Err(_)) => {
                self.problem = Some("center.sales.not_saved");
                if let Some(l) = &self.link {
                    l.push(crate::company::CompanyEvent::Toast(
                        "center.sales.not_saved",
                    ));
                }
            }
            None => {}
        }
        if let Some(r) = engine::poll(&mut self.read) {
            match r {
                Ok((levels, sales)) => {
                    self.levels = levels;
                    self.level = sales.price_level_id;
                    self.markup = sales
                        .default_markup_percent
                        .map(SignedPercent::to_wire)
                        .unwrap_or_default();
                    self.before = (self.level, self.markup.clone());
                    self.layout = sales.default_invoice_layout_id;
                    self.layout_before = self.layout;
                }
                Err(_) => self.problem = Some("center.sales.load_failed"),
            }
        }
        if let Some(Ok(l)) = engine::poll(&mut self.layouts_read) {
            self.layouts = l;
        }
        match engine::poll(&mut self.layout_write) {
            Some(Ok(())) => self.layout_before = self.layout,
            Some(Err(_)) => {
                self.problem = Some("center.sales.not_saved");
                if let Some(l) = &self.link {
                    l.push(crate::company::CompanyEvent::Toast(
                        "center.sales.not_saved",
                    ));
                }
            }
            None => {}
        }
        if let Some(r) = engine::poll(&mut self.write) {
            if r.is_ok() {
                self.before = (self.level, self.markup.clone());
            } else {
                self.problem = Some("center.sales.not_saved");
                // The form may have closed (Save & Close): say so.
                if let Some(l) = &self.link {
                    l.push(crate::company::CompanyEvent::Toast(
                        "center.sales.not_saved",
                    ));
                }
            }
        }
    }

    pub(crate) fn busy(&self) -> bool {
        self.read.is_some()
            || self.write.is_some()
            || self.tax_read.is_some()
            || self.tax_write.is_some()
            || self.layouts_read.is_some()
            || self.layout_write.is_some()
    }

    /// The owner changed the level, the markup or the layout.
    pub(crate) fn dirty(&self) -> bool {
        self.levels_dirty()
            || self.layout != self.layout_before
            || (self.sales_tax, self.tax_code) != self.tax_before
    }

    fn levels_dirty(&self) -> bool {
        (self.level, self.markup.trim().to_string())
            != (self.before.0, self.before.1.trim().to_string())
    }

    /// The markup as the engine takes it; `Err` when it isn't a percent of
    /// 0 or more.
    fn markup_value(&self) -> Result<Option<SignedPercent>, ()> {
        let m = self.markup.trim();
        if m.is_empty() {
            return Ok(None);
        }
        match SignedPercent::parse(m) {
            Ok(p) if !p.is_negative() => Ok(Some(p)),
            _ => Err(()),
        }
    }

    /// The save gate (S9): false, with the problem shown, when the markup
    /// is not a percent from 0 to 1000; nothing is saved then.
    pub(crate) fn check(&mut self) -> bool {
        if self.markup_on && self.markup_value().is_err() {
            self.problem = Some("center.sales.markup_invalid");
            return false;
        }
        true
    }

    /// Writes the section for `client` after its contact saved.
    pub(crate) fn save(
        &mut self,
        link: &Arc<CompanyLink>,
        group: Option<&RequestGroup>,
        client: ContactId,
    ) {
        if !self.dirty() {
            return;
        }
        self.link = Some(Arc::clone(link));
        if self.tax_on && (self.sales_tax, self.tax_code) != self.tax_before {
            self.save_tax(link, group, client);
        }
        if self.layouts_on && self.layout != self.layout_before {
            let layout = self.layout;
            self.layout_write = Some(engine::write(
                link,
                group,
                OrphanLabel {
                    what: "orphan.client_sales",
                    key: None,
                },
                move |d| {
                    match layout {
                        Some(l) => {
                            d.layouts.clients.insert(client, l);
                        }
                        None => {
                            d.layouts.clients.remove(&client);
                        }
                    }
                    Ok::<(), ApiError>(())
                },
                move |s| async move { s.invoices().set_client_layout(client, layout).await },
            ));
        }
        if !self.levels_dirty() {
            self.poll();
            return;
        }
        let Ok(markup) = self.markup_value() else {
            self.problem = Some("center.sales.markup_invalid");
            return;
        };
        self.problem = None;
        self.link = Some(Arc::clone(link));
        let level = self.level;
        self.write = Some(engine::write(
            link,
            group,
            OrphanLabel {
                what: "orphan.client_sales",
                key: None,
            },
            |d| {
                match level {
                    Some(l) => {
                        d.price_levels.clients.insert(client, l);
                    }
                    None => {
                        d.price_levels.clients.remove(&client);
                    }
                }
                match markup {
                    Some(m) => {
                        d.invoices.markups.insert(client, m);
                    }
                    None => {
                        d.invoices.markups.remove(&client);
                    }
                }
                Ok::<(), ApiError>(())
            },
            move |s| async move { s.invoices().set_client_sales(client, level, markup).await },
        ));
        self.poll();
    }

    /// P23: writes the client's default sales tax and tax code (the
    /// contact read again and saved with only those changed). A cleared
    /// default reaches the demo engine; the live contact PUT sends only set
    /// values, so clearing waits for the WP-4 contract.
    fn save_tax(
        &mut self,
        link: &Arc<CompanyLink>,
        group: Option<&RequestGroup>,
        client: ContactId,
    ) {
        let (tax, code) = (self.sales_tax, self.tax_code);
        self.tax_write = Some(engine::write(
            link,
            group,
            OrphanLabel {
                what: "orphan.client_sales",
                key: None,
            },
            move |d| {
                let c = d
                    .contacts_mut()
                    .iter_mut()
                    .find(|c| c.id == client)
                    .ok_or(ApiError::NotFound { code: None })?;
                c.fields.default_sales_tax_id = tax;
                c.fields.tax_code_id = code;
                Ok(())
            },
            move |s| async move {
                let v = s.contacts().get(client).await?;
                let mut fields = v.record.fields.clone();
                fields.default_sales_tax_id = tax;
                fields.tax_code_id = code;
                let update = cl_api::ContactUpdate {
                    fields,
                    is_active: v.record.is_active,
                };
                s.contacts()
                    .update(client, &v.version, &update, cl_api::IdempotencyKey::new())
                    .await
                    .map(|_| ())
            },
        ));
    }

    fn tax_ui(&mut self, ui: &mut Ui) {
        let tax_name = |id: Option<SalesTaxItemId>| {
            id.and_then(|id| self.tax_items.iter().find(|i| i.id == id))
                .map_or_else(
                    || t("contacts.sales_tax.company_default"),
                    |i| i.name.clone(),
                )
        };
        let shown = tax_name(self.sales_tax);
        let l = ui.label(t("contacts.field.sales_tax"));
        let mut pick = None;
        egui::ComboBox::from_id_salt("center/sales/sales_tax")
            .selected_text(shown)
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(
                        self.sales_tax.is_none(),
                        t("contacts.sales_tax.company_default"),
                    )
                    .clicked()
                {
                    pick = Some(None);
                }
                for i in &self.tax_items {
                    let label = format!("{} ({})", i.name, i.rate.format_percent());
                    if ui
                        .selectable_label(self.sales_tax == Some(i.id), label)
                        .clicked()
                    {
                        pick = Some(Some(i.id));
                    }
                }
            })
            .response
            .labelled_by(l.id)
            .on_hover_text(t("contacts.sales_tax.hint"));
        if let Some(p) = pick {
            self.sales_tax = p;
        }
        let code_name = self
            .tax_code
            .and_then(|id| self.tax_codes.iter().find(|c| c.id == id))
            .map_or_else(
                || t("contacts.tax_code.none"),
                |c| format!("{} \u{b7} {}", c.code, c.name),
            );
        let l = ui.label(t("contacts.field.tax_code"));
        let mut pick = None;
        egui::ComboBox::from_id_salt("center/sales/tax_code")
            .selected_text(code_name)
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(self.tax_code.is_none(), t("contacts.tax_code.none"))
                    .clicked()
                {
                    pick = Some(None);
                }
                for c in &self.tax_codes {
                    let label = format!("{} \u{b7} {}", c.code, c.name);
                    if ui
                        .selectable_label(self.tax_code == Some(c.id), label)
                        .clicked()
                    {
                        pick = Some(Some(c.id));
                    }
                }
            })
            .response
            .labelled_by(l.id)
            .on_hover_text(t("contacts.tax_code.hint"));
        if let Some(p) = pick {
            self.tax_code = p;
        }
    }

    /// Draws the section (clients only).
    pub(crate) fn ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        if self.busy() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
        }
        if !self.levels_on && !self.markup_on && !self.layouts_on && !self.tax_on {
            return;
        }
        pane::section(ui, tokens, &t("center.sales.title"));
        if self.tax_on && !self.tax_items.is_empty() {
            self.tax_ui(ui);
        }
        if self.layouts_on && !self.layouts.is_empty() {
            // Q-P20ae/af: archived layouts are not offered; a client keeps
            // an archived one it has.
            let name = |id: Option<cl_api::InvoiceLayoutId>| match id {
                None => t("center.sales.layout_default"),
                Some(id) => self
                    .layouts
                    .iter()
                    .find(|l| l.id == id)
                    .map_or_else(|| t("center.sales.layout_default"), |l| l.name.clone()),
            };
            let shown = name(self.layout);
            let offered: Vec<(cl_api::InvoiceLayoutId, String)> = self
                .layouts
                .iter()
                .filter(|l| !l.is_archived)
                .map(|l| (l.id, l.name.clone()))
                .collect();
            let l = ui.label(t("center.sales.layout"));
            egui::ComboBox::from_id_salt("center/sales/layout")
                .selected_text(shown)
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(self.layout.is_none(), t("center.sales.layout_default"))
                        .clicked()
                    {
                        self.layout = None;
                    }
                    for (id, n) in &offered {
                        if ui.selectable_label(self.layout == Some(*id), n).clicked() {
                            self.layout = Some(*id);
                        }
                    }
                })
                .response
                .labelled_by(l.id)
                .on_hover_text(t("center.sales.layout.tip"));
        }
        if self.levels_on {
            let mut names = vec![t("center.sales.no_level")];
            let mut ids = vec![None];
            for l in self
                .levels
                .iter()
                .filter(|l| l.is_active || Some(l.id) == self.level)
            {
                names.push(format!("{} ({}%)", l.name, l.percent.to_wire()));
                ids.push(Some(l.id));
            }
            let cur = ids.iter().position(|x| *x == self.level).unwrap_or(0);
            let l = ui.label(t("center.sales.level"));
            egui::ComboBox::from_id_salt("center/sales/level")
                .selected_text(names[cur].clone())
                .show_ui(ui, |ui| {
                    for (i, n) in names.iter().enumerate() {
                        if ui.selectable_label(i == cur, n).clicked() {
                            self.level = ids[i];
                        }
                    }
                })
                .response
                .labelled_by(l.id)
                .on_hover_text(t("center.sales.level.tip"));
        }
        if self.markup_on {
            ui.horizontal(|ui| {
                let l = ui.label(t("center.sales.markup"));
                ui.add(egui::TextEdit::singleline(&mut self.markup).desired_width(70.0))
                    .labelled_by(l.id)
                    .on_hover_text(t("center.sales.markup.tip"));
                ui.label("%");
            });
        }
        if self.markup_value().is_err() {
            ui.label(RichText::new(t("center.sales.markup_invalid")).color(tokens.warn_text));
        }
        if let Some(p) = self.problem {
            ui.label(RichText::new(t(p)).color(tokens.warn_text));
        }
    }

    pub(crate) fn state_json(&self) -> Value {
        json!({
            "level": self.level.is_some(),
            "levels": self.levels.len(),
            "markup": self.markup,
            "sales_tax": self.sales_tax.and_then(|id| self.tax_items.iter().find(|i| i.id == id)).map(|i| i.name.clone()),
            "tax_code": self.tax_code.and_then(|id| self.tax_codes.iter().find(|c| c.id == id)).map(|c| c.code.clone()),
            "dirty": self.dirty(),
            "problem": self.problem,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Q-P20ae/af: a client's default layout is saved and new invoices for
    /// the client start on it.
    #[test]
    fn a_client_default_layout_is_kept() {
        let dir = tempfile::tempdir().expect("tmp");
        let link = Arc::new(CompanyLink::demo(dir.path()));
        let client = {
            let mut d = link.demo_state().expect("demo");
            d.contacts_mut()
                .iter()
                .find(|c| c.fields.display_name == crate::company::demo::bills::STORY_CUSTOMER)
                .map(|c| c.id)
                .expect("story client")
        };
        let mut s = SalesEdit::open(&link, None, Some(client));
        assert_eq!(s.layouts.len(), 2);
        let simple = s.layouts.iter().find(|l| l.name == "Simple").map(|l| l.id);
        s.layout = simple;
        assert!(s.dirty());
        s.save(&link, None, client);
        assert!(!s.dirty());
        let d = link.demo_state().expect("demo");
        assert_eq!(d.layouts.clients.get(&client).copied(), simple);
    }

    /// P23: a client's default sales tax and tax code are kept, and the
    /// demo engine's invoices for the client use them.
    #[test]
    fn a_client_default_sales_tax_and_code_are_kept() {
        let dir = tempfile::tempdir().expect("tmp");
        let link = Arc::new(CompanyLink::demo(dir.path()));
        let client = {
            let mut d = link.demo_state().expect("demo");
            d.contacts_mut()
                .iter()
                .find(|c| c.fields.display_name == crate::company::demo::bills::STORY_CUSTOMER)
                .map(|c| c.id)
                .expect("story client")
        };
        let mut s = SalesEdit::open(&link, None, Some(client));
        assert!(s.tax_on && s.tax_items.len() >= 4 && s.tax_codes.len() >= 3);
        assert_eq!(s.sales_tax, None);
        s.sales_tax = s
            .tax_items
            .iter()
            .find(|i| i.name == "Springfield")
            .map(|i| i.id);
        s.tax_code = s.tax_codes.iter().find(|c| c.code == "Res").map(|c| c.id);
        assert!(s.dirty());
        s.save(&link, None, client);
        assert!(!s.dirty());
        assert_eq!(s.state_json()["sales_tax"], "Springfield");
        let mut d = link.demo_state().expect("demo");
        let r = crate::company::demo::sales_tax::resolve(&mut d, None, client)
            .expect("ok")
            .expect("tax");
        assert_eq!(r.rate.to_string(), "0.088750");
        let code = crate::company::demo::sales_tax::line_code(
            &mut d,
            None,
            client,
            Some((None, true)),
            None,
        );
        assert_eq!((code.code.as_str(), code.source), ("Res", "client"));
    }

    #[test]
    fn a_bad_markup_stops_the_save_and_1000_is_allowed() {
        let mut s = SalesEdit {
            markup_on: true,
            markup: "-5".into(),
            ..SalesEdit::default()
        };
        assert!(!s.check());
        assert_eq!(s.problem, Some("center.sales.markup_invalid"));
        s.markup = "1000".into();
        assert!(s.check());
        s.markup = "1000.5".into();
        assert!(!s.check());
    }
}
