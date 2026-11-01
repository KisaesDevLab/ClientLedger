//! Price Levels (P20 WP-B step 2, `lists.price_levels`; U-40, Q-P20l): a
//! name and a percent up or down, with fixed prices for chosen items; one
//! level per client (set in the client form). The engine prices the
//! invoice lines (round2(item price × (1 + % ÷ 100)), a fixed price wins);
//! this window only edits the levels. Synthetic demo data in `--demo`.

use std::sync::Arc;

use cl_api::{
    ApiError, Capabilities, Item, ItemFilter, ItemPrice, ItemType, OrphanLabel, PriceLevel,
    PriceLevelDraft, PriceLevelId, RequestGroup, SignedPercent,
};
use cl_core::CalendarDate;
use cl_core::ids::TenantId;
use cl_model::{CommandId, Scope};
use eframe::egui::{self, RichText, Ui};
use serde_json::{Value, json};

use crate::company::CompanyLink;
use crate::i18n::{t, t_with};
use crate::screens::company::form;
use crate::screens::invoice::engine::{self, Job};
use crate::theme::Tokens;
use crate::windows::Screen;

/// The command and window kind.
pub const KIND: &str = "lists.price_levels";

/// The Price Levels window for `kind`, if it is one.
#[must_use]
pub fn make(
    kind: CommandId,
    link: &Arc<CompanyLink>,
    _open: Option<(TenantId, String)>,
    _today: CalendarDate,
) -> Option<Box<dyn Screen>> {
    (kind.as_str() == KIND).then(|| Box::new(PriceLevelsScreen::new(Arc::clone(link))) as Box<_>)
}

/// The level being edited, as typed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Editor {
    pub id: Option<PriceLevelId>,
    pub name: String,
    pub percent: String,
    pub active: bool,
    /// Item and price as typed.
    pub prices: Vec<(Option<cl_core::ids::ItemId>, String)>,
}

impl Editor {
    fn of(l: &PriceLevel) -> Editor {
        Editor {
            id: Some(l.id),
            name: l.name.clone(),
            percent: l.percent.to_wire(),
            active: l.is_active,
            prices: l
                .item_prices
                .iter()
                .map(|p| (Some(p.item_id), cl_model::lists::items::price_text(p.price)))
                .collect(),
        }
    }

    /// The draft the engine takes, or the problem (nothing sent).
    pub(crate) fn draft(&self) -> Result<PriceLevelDraft, &'static str> {
        let name = self.name.trim();
        if name.is_empty() {
            return Err("price_levels.problem.name");
        }
        let pct = if self.percent.trim().is_empty() {
            "0"
        } else {
            self.percent.trim()
        };
        let percent = SignedPercent::parse(pct).map_err(|_| "price_levels.problem.percent")?;
        let mut item_prices = Vec::new();
        for (item, price) in &self.prices {
            let Some(item) = item else { continue };
            let Ok(Some(price)) = cl_model::lists::items::parse_price(price) else {
                return Err("price_levels.problem.price");
            };
            if item_prices.iter().any(|p: &ItemPrice| p.item_id == *item) {
                return Err("price_levels.problem.twice");
            }
            item_prices.push(ItemPrice {
                item_id: *item,
                price,
            });
        }
        Ok(PriceLevelDraft {
            name: name.to_string(),
            percent,
            is_active: self.active,
            item_prices,
        })
    }
}

/// The Price Levels window.
pub struct PriceLevelsScreen {
    link: Arc<CompanyLink>,
    group: Option<RequestGroup>,
    available: bool,
    levels: Vec<PriceLevel>,
    items: Vec<Item>,
    list_job: Option<Job<Vec<PriceLevel>>>,
    items_job: Option<Job<Vec<Item>>>,
    get_job: Option<Job<PriceLevel>>,
    save_job: Option<Job<PriceLevel>>,
    delete_job: Option<Job<()>>,
    pub(crate) editor: Option<Editor>,
    problem: Option<&'static str>,
    error: Option<ApiError>,
    saved: u32,
}

/// A demo id for the `n`th level.
fn demo_id(n: usize) -> PriceLevelId {
    PriceLevelId::parse(&format!("d6000000-0000-4000-8000-{n:012}"))
        .unwrap_or_else(|_| unreachable!("a well-formed demo id"))
}

fn demo_save(
    d: &mut crate::company::demo::DemoState,
    id: Option<PriceLevelId>,
    draft: &PriceLevelDraft,
) -> Result<PriceLevel, ApiError> {
    let levels = &mut d.price_levels.levels;
    if levels
        .iter()
        .any(|l| Some(l.id) != id && l.name.to_lowercase() == draft.name.trim().to_lowercase())
    {
        return Err(ApiError::Conflict {
            reason: cl_api::ConflictReason::Unspecified,
            current_version: None,
            code: cl_api::ErrorCode::parse("PRICE_LEVEL_NAME_IN_USE"),
            message: cl_api::UserText::default(),
            details: cl_api::ConflictDetails::default(),
        });
    }
    let level = PriceLevel {
        id: id.unwrap_or_else(|| demo_id(levels.len() + 1)),
        name: draft.name.trim().to_string(),
        percent: draft.percent,
        is_active: draft.is_active,
        item_prices: draft.item_prices.clone(),
        updated_at: Some(crate::company::demo::DemoState::now()),
    };
    match levels.iter_mut().find(|l| l.id == level.id) {
        Some(l) => *l = level.clone(),
        None => levels.push(level.clone()),
    }
    Ok(level)
}

impl PriceLevelsScreen {
    /// The window over the open company.
    #[must_use]
    pub fn new(link: Arc<CompanyLink>) -> PriceLevelsScreen {
        let group = link.group().and_then(Result::ok);
        let available = link.is_demo() || link.capabilities().has(Capabilities::PRICE_LEVELS);
        let mut s = PriceLevelsScreen {
            link,
            group,
            available,
            levels: Vec::new(),
            items: Vec::new(),
            list_job: None,
            items_job: None,
            get_job: None,
            save_job: None,
            delete_job: None,
            editor: None,
            problem: None,
            error: None,
            saved: 0,
        };
        s.refresh();
        s
    }

    fn refresh(&mut self) {
        if !self.available {
            return;
        }
        self.list_job = Some(engine::read(
            &self.link,
            self.group.as_ref(),
            |d| Ok(d.price_levels.levels.clone()),
            |s| async move { s.invoices().price_levels().list().await },
        ));
        self.items_job = Some(engine::read(
            &self.link,
            self.group.as_ref(),
            |d| Ok(d.items_mut().clone()),
            |s| async move { s.items().list_all(&ItemFilter::default()).await },
        ));
        self.poll();
    }

    fn busy(&self) -> bool {
        self.list_job.is_some()
            || self.items_job.is_some()
            || self.get_job.is_some()
            || self.save_job.is_some()
            || self.delete_job.is_some()
    }

    pub(crate) fn poll(&mut self) {
        if let Some(r) = engine::poll(&mut self.list_job) {
            match r {
                Ok(v) => self.levels = v,
                Err(e) => self.error = Some(e),
            }
        }
        if let Some(r) = engine::poll(&mut self.items_job) {
            self.items = r.unwrap_or_default();
        }
        if let Some(r) = engine::poll(&mut self.get_job) {
            match r {
                Ok(l) => self.editor = Some(Editor::of(&l)),
                Err(e) => self.error = Some(e),
            }
        }
        if let Some(r) = engine::poll(&mut self.save_job) {
            match r {
                Ok(l) => {
                    self.saved += 1;
                    self.editor = Some(Editor::of(&l));
                    crate::forms::publish_saved(&self.link, &self.link.bus().subscribe(), &[]);
                    self.refresh();
                }
                Err(e) => self.refused(e),
            }
        }
        if let Some(r) = engine::poll(&mut self.delete_job) {
            match r {
                Ok(()) => {
                    self.editor = None;
                    self.refresh();
                }
                Err(e) => self.refused(e),
            }
        }
    }

    fn refused(&mut self, e: ApiError) {
        let code = match &e {
            ApiError::Conflict { code: Some(c), .. } => Some(c.as_str().to_string()),
            _ => None,
        };
        match code.as_deref() {
            Some("PRICE_LEVEL_NAME_IN_USE") => {
                self.problem = Some("price_levels.problem.name_taken");
            }
            Some("PRICE_LEVEL_IN_USE") => self.problem = Some("price_levels.problem.in_use"),
            _ => self.error = Some(e),
        }
    }

    pub(crate) fn new_level(&mut self) {
        self.problem = None;
        self.editor = Some(Editor {
            active: true,
            percent: "0".into(),
            ..Editor::default()
        });
    }

    fn open(&mut self, id: PriceLevelId) {
        self.problem = None;
        self.get_job = Some(engine::read(
            &self.link,
            self.group.as_ref(),
            |d| {
                d.price_levels
                    .levels
                    .iter()
                    .find(|l| l.id == id)
                    .cloned()
                    .ok_or(ApiError::NotFound { code: None })
            },
            move |s| async move { s.invoices().price_levels().get(id).await },
        ));
        self.poll();
    }

    pub(crate) fn save(&mut self) {
        let Some(ed) = self.editor.clone() else {
            return;
        };
        let draft = match ed.draft() {
            Ok(d) => d,
            Err(p) => {
                self.problem = Some(p);
                return;
            }
        };
        self.problem = None;
        let id = ed.id;
        let d2 = draft.clone();
        self.save_job = Some(engine::write(
            &self.link,
            self.group.as_ref(),
            OrphanLabel {
                what: "orphan.price_level_save",
                key: None,
            },
            |d| demo_save(d, id, &d2),
            move |s| async move {
                let levels = s.invoices();
                let levels = levels.price_levels();
                match id {
                    Some(id) => levels.update(id, &draft).await,
                    None => levels.create(&draft).await,
                }
            },
        ));
        self.poll();
    }

    fn delete(&mut self, id: PriceLevelId) {
        self.delete_job = Some(engine::write(
            &self.link,
            self.group.as_ref(),
            OrphanLabel {
                what: "orphan.price_level_delete",
                key: None,
            },
            |d| {
                if d.price_levels.clients.values().any(|l| *l == id) {
                    return Err(ApiError::Conflict {
                        reason: cl_api::ConflictReason::Unspecified,
                        current_version: None,
                        code: cl_api::ErrorCode::parse("PRICE_LEVEL_IN_USE"),
                        message: cl_api::UserText::default(),
                        details: cl_api::ConflictDetails::default(),
                    });
                }
                d.price_levels.levels.retain(|l| l.id != id);
                Ok(())
            },
            move |s| async move { s.invoices().price_levels().delete(id).await },
        ));
        self.poll();
    }

    fn draw(&mut self, ui: &mut Ui) {
        let tokens = Tokens::of(ui.ctx());
        ui.label(
            RichText::new(t("cmd.lists.price_levels"))
                .size(22.0)
                .strong(),
        );
        if !self.available {
            form::hint(ui, &tokens, &t("price_levels.unavailable"));
            return;
        }
        form::hint(ui, &tokens, &t("price_levels.intro"));
        if let Some(e) = &self.error {
            form::inline_error(ui, &tokens, e);
        }
        if ui.button(t("price_levels.new")).clicked() {
            self.new_level();
        }
        let mut open = None;
        egui::Grid::new("price_levels/list")
            .striped(true)
            .show(ui, |ui| {
                ui.strong(t("price_levels.col.name"));
                ui.strong(t("price_levels.col.percent"));
                ui.strong(t("price_levels.col.status"));
                ui.end_row();
                for l in &self.levels {
                    if ui.link(&l.name).clicked() {
                        open = Some(l.id);
                    }
                    ui.label(RichText::new(format!("{}%", l.percent.to_wire())).monospace());
                    ui.label(if l.is_active {
                        t("price_levels.active")
                    } else {
                        t("price_levels.inactive")
                    });
                    ui.end_row();
                }
            });
        if self.levels.is_empty() {
            ui.label(RichText::new(t("price_levels.empty")).color(tokens.text_secondary));
        }
        if let Some(id) = open {
            self.open(id);
        }
        ui.add_space(10.0);
        self.editor_ui(ui, &tokens);
    }

    fn editor_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let items: Vec<(cl_core::ids::ItemId, String)> = self
            .items
            .iter()
            .filter(|i| {
                i.is_active
                    && !matches!(i.fields.item_type, ItemType::Discount | ItemType::Subtotal)
            })
            .map(|i| (i.id, i.fields.name.clone()))
            .collect();
        let Some(ed) = self.editor.as_mut() else {
            return;
        };
        let mut save = false;
        let mut delete = None;
        let mut close = false;
        form::card(ui, tokens, |ui| {
            form::heading(
                ui,
                tokens,
                &t(if ed.id.is_some() {
                    "price_levels.edit"
                } else {
                    "price_levels.new"
                }),
            );
            form::field(ui, &t("price_levels.name"), &mut ed.name);
            form::field(ui, &t("price_levels.percent"), &mut ed.percent);
            form::hint(ui, tokens, &t("price_levels.percent.tip"));
            ui.checkbox(&mut ed.active, t("price_levels.active"));
            ui.add_space(6.0);
            ui.label(t("price_levels.item_prices"));
            let mut remove = None;
            for (i, (item, price)) in ed.prices.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    let shown = item
                        .and_then(|id| items.iter().find(|(x, _)| *x == id))
                        .map_or_else(|| t("price_levels.pick_item"), |(_, n)| n.clone());
                    let l = ui.label(t_with(
                        "price_levels.item_n",
                        &[("n", &(i + 1).to_string())],
                    ));
                    egui::ComboBox::from_id_salt(("price_levels/item", i))
                        .selected_text(shown)
                        .show_ui(ui, |ui| {
                            for (id, name) in &items {
                                if ui.selectable_label(*item == Some(*id), name).clicked() {
                                    *item = Some(*id);
                                }
                            }
                        })
                        .response
                        .labelled_by(l.id);
                    let p = ui.label(t("price_levels.price"));
                    ui.add(egui::TextEdit::singleline(price).desired_width(90.0))
                        .labelled_by(p.id);
                    if ui.button(t("price_levels.remove")).clicked() {
                        remove = Some(i);
                    }
                });
            }
            if let Some(i) = remove {
                ed.prices.remove(i);
            }
            if ui.button(t("price_levels.add_price")).clicked() {
                ed.prices.push((None, String::new()));
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                save = crate::widgets::primary_button(ui, &t("price_levels.save")).clicked();
                if let Some(id) = ed.id
                    // Plainer than Save (UI review): a quiet button with
                    // warning-coloured text.
                    && ui
                        .button(RichText::new(t("price_levels.delete")).color(tokens.warn_text))
                        .clicked()
                {
                    delete = Some(id);
                }
                close = ui.button(t("price_levels.close")).clicked();
            });
        });
        if let Some(p) = self.problem {
            ui.label(RichText::new(t(p)).color(tokens.warn_text));
        }
        if save {
            self.save();
        } else if let Some(id) = delete {
            self.delete(id);
        } else if close {
            self.editor = None;
            self.problem = None;
        }
    }

    fn state_json(&self) -> Value {
        json!({
            "screen": "price_levels",
            "available": self.available,
            "levels": self.levels.len(),
            "editing": self.editor.as_ref().map(|e| e.id.is_some()),
            "problem": self.problem,
            "saved": self.saved,
            "error": self.error.is_some(),
        })
    }
}

impl Screen for PriceLevelsScreen {
    fn title(&self) -> String {
        t("cmd.lists.price_levels")
    }

    fn ui(&mut self, ui: &mut Ui) {
        self.poll();
        egui::ScrollArea::vertical().show(ui, |ui| self.draw(ui));
        if self.busy() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    fn scope(&self) -> Scope {
        Scope::Global
    }

    fn command(&mut self, id: CommandId) -> bool {
        id.as_str() == KIND
    }

    fn state(&self) -> Value {
        self.state_json()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cl_core::Money;

    #[test]
    fn a_level_saves_with_a_fixed_item_price_and_names_stay_unique() {
        let dir = tempfile::tempdir().expect("tmp");
        let link = Arc::new(CompanyLink::demo(dir.path()));
        let mut s = PriceLevelsScreen::new(Arc::clone(&link));
        assert!(s.available);
        let item = s.items[1].id;
        s.new_level();
        if let Some(e) = s.editor.as_mut() {
            e.name = "Contractor".into();
            e.percent = "-7.5".into();
            e.prices.push((Some(item), "30".into()));
        }
        s.save();
        assert_eq!(s.saved, 1, "{:?}", s.problem);
        // The demo's Commercial level and this one.
        assert_eq!(s.levels.len(), 2);
        let made = s
            .levels
            .iter()
            .find(|l| l.name == "Contractor")
            .expect("saved");
        assert_eq!(
            made.item_prices[0].price,
            Money::parse_wire("30").expect("m")
        );
        s.new_level();
        if let Some(e) = s.editor.as_mut() {
            e.name = "contractor".into();
        }
        s.save();
        assert_eq!(s.problem, Some("price_levels.problem.name_taken"));
        s.new_level();
        if let Some(e) = s.editor.as_mut() {
            e.name = "Bad".into();
            e.percent = "lots".into();
        }
        s.save();
        assert_eq!(s.problem, Some("price_levels.problem.percent"));
        for k in [
            "price_levels.intro",
            "price_levels.problem.in_use",
            "price_levels.unavailable",
            "orphan.price_level_save",
        ] {
            assert!(crate::i18n::has(k), "{k}");
        }
    }
}
