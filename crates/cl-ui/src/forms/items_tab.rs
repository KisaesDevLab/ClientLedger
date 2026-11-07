//! The Items tab of Write Checks and Record Card Charge (P24 plan §3,
//! U-158): parts and other items bought by check or card, beside the
//! Expenses lines, on the bill's shared [`ItemsGrid`].
//!
//! The tab shows only while the company tracks inventory (the switch in
//! Preferences); without it both forms keep their Expenses-only layout
//! (Q-247). The item list and the switch are read with the form's other
//! lists; the screen owns the grid's value like the bill does.

use std::sync::Arc;

use cl_api::{Item, ItemFilter, RequestGroup};
use cl_core::{Money, MoneyStyle};
use eframe::egui::{RichText, Stroke, Ui};

use crate::company::{CompanyLink, Task};
use crate::forms::item_lines_grid::{ItemLists, ItemsGrid};
use crate::i18n::t_with;
use crate::screens::register::lists::RegisterLists;
use crate::theme::Tokens;

/// Which lines tab is shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum LinesTab {
    /// The Expenses lines.
    #[default]
    Expenses,
    /// The Items lines.
    Items,
}

impl LinesTab {
    /// The control channel's name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            LinesTab::Expenses => "expenses",
            LinesTab::Items => "items",
        }
    }
}

/// The Items tab: its grid, which tab is shown, the items and the switch.
pub struct ItemsTab {
    /// The item lines.
    pub grid: ItemsGrid,
    /// The tab shown.
    pub tab: LinesTab,
    /// The company's items (active and inactive).
    pub items: Vec<Item>,
    /// The company tracks inventory: the tab shows.
    pub inventory_on: bool,
    task: Option<Task<(Vec<Item>, bool)>>,
}

impl ItemsTab {
    /// An empty tab whose grid has its own id and accessible label.
    #[must_use]
    pub fn new(id: &'static str, label: &str) -> ItemsTab {
        let lists = Arc::new(ItemLists::new(Arc::new(RegisterLists::default()), &[]));
        ItemsTab {
            grid: ItemsGrid::named(lists, id, label),
            tab: LinesTab::Expenses,
            items: Vec::new(),
            inventory_on: false,
            task: None,
        }
    }

    /// The Items tab can be shown (inventory is on, or lines are already
    /// there: a saved check with items opens with them).
    #[must_use]
    pub fn shown(&self) -> bool {
        self.inventory_on || self.grid.value().used().next().is_some()
    }

    /// Reads the items and the inventory switch (the demo at once).
    pub fn start_load(&mut self, link: &CompanyLink, group: Option<&RequestGroup>) {
        if let Some(mut demo) = link.demo_state() {
            let items = demo.items_mut().clone();
            let on = demo.inventory.enabled;
            drop(demo);
            self.task = Some(Task::ready((items, on)));
            return;
        }
        let (Some(session), Some(group)) = (link.session(), group) else {
            return;
        };
        if let Ok(p) = group.read(&session, |s| async move {
            let items = s.items().list_all(&ItemFilter::default()).await?;
            let on = s.company_settings().await.is_ok_and(|c| c.inventory_on());
            Ok((items, on))
        }) {
            self.task = Some(Task::Pending(p));
        }
    }

    /// Still reading.
    #[must_use]
    pub fn loading(&self) -> bool {
        self.task.is_some()
    }

    /// The read arrived: true when the items changed (the screen then
    /// calls [`ItemsTab::set_base`]).
    pub fn poll(&mut self) -> bool {
        let Some(r) = self.task.as_mut().and_then(Task::try_take) else {
            return false;
        };
        self.task = None;
        match r {
            Ok((items, on)) => {
                self.items = items;
                self.inventory_on = on;
                true
            }
            Err(_) => false,
        }
    }

    /// The register lists changed (accounts, names, tags) or the items did.
    pub fn set_base(&mut self, base: &Arc<RegisterLists>) {
        self.grid
            .set_lists(Arc::new(ItemLists::new(Arc::clone(base), &self.items)));
    }

    /// Every item's id and name (the NEGATIVE_STOCK warning names it).
    #[must_use]
    pub fn names(&self) -> Vec<(cl_core::ids::ItemId, String)> {
        self.grid.lists().names()
    }

    /// The tab bar (Expenses {total} · Items {total}); nothing while the
    /// Items tab is hidden (the Expenses tab is then shown).
    pub fn tab_bar(
        &mut self,
        ui: &mut Ui,
        tokens: &Tokens,
        expenses: Money,
        style: MoneyStyle,
    ) -> bool {
        if !self.shown() {
            self.tab = LinesTab::Expenses;
            return false;
        }
        let items =
            cl_model::forms::item_lines::items_total(self.grid.value()).unwrap_or(Money::ZERO);
        ui.horizontal(|ui| {
            for (tab, key, total) in [
                (LinesTab::Expenses, "bill.tab.expenses", expenses),
                (LinesTab::Items, "bill.tab.items", items),
            ] {
                let on = self.tab == tab;
                let text = t_with(key, &[("total", &total.format(style))]);
                let rich = if on {
                    RichText::new(text).strong().color(tokens.text_primary)
                } else {
                    RichText::new(text).color(tokens.text_secondary)
                };
                let r = ui.selectable_label(on, rich);
                if on {
                    ui.painter().hline(
                        r.rect.x_range(),
                        r.rect.bottom() + 2.0,
                        Stroke::new(2.0, tokens.accent),
                    );
                }
                if r.clicked() {
                    self.tab = tab;
                }
            }
        });
        ui.add_space(4.0);
        true
    }
}
