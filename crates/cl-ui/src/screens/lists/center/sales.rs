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
use cl_core::ids::ContactId;
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
    levels: Vec<PriceLevel>,
    pub(crate) level: Option<PriceLevelId>,
    pub(crate) markup: String,
    before: (Option<PriceLevelId>, String),
    read: Option<Job<(Vec<PriceLevel>, ClientSales)>>,
    write: Option<Job<()>>,
    pub(crate) problem: Option<&'static str>,
    /// Where a failed write is reported once the form has closed (S9).
    link: Option<Arc<CompanyLink>>,
}

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
            ..SalesEdit::default()
        };
        if !s.levels_on && !s.markup_on {
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
                }
                Err(_) => self.problem = Some("center.sales.load_failed"),
            }
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
        self.read.is_some() || self.write.is_some()
    }

    /// The owner changed the level or the markup.
    pub(crate) fn dirty(&self) -> bool {
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

    /// Draws the section (clients only).
    pub(crate) fn ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        if self.busy() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
        }
        if !self.levels_on && !self.markup_on {
            return;
        }
        pane::section(ui, tokens, &t("center.sales.title"));
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
            "dirty": self.dirty(),
            "problem": self.problem,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
