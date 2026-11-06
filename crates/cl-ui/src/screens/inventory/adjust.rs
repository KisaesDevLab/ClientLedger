//! Adjust Quantity/Value on Hand (P24 plan §3, `vendors.adjust_inventory`;
//! engine U-159): a date, the account the other side posts to, a mode
//! (quantity, value, or both) and one line per part. A quantity change is
//! valued by the engine at average cost; a value change is the owner's
//! whole cents. Anything on or before the closing date is refused, even
//! with the closing-date password (Q-P24b). NEGATIVE_STOCK warns, never
//! blocks.

use std::sync::Arc;

use cl_api::{
    AccountType, ApiError, Capabilities, IdempotencyKey, NewAdjustment, OrphanLabel,
    PostingOptions, RequestGroup, SavedAdjustment,
};
use cl_core::CalendarDate;
use cl_core::ids::AccountId;
use cl_model::forms::SaveIntent;
use cl_model::inventory::{
    AdjustMode, AdjustProblem, AdjustRow, AdjustmentDraft, check_adjustment,
};
use cl_model::{CommandId, Scope};
use eframe::egui::{self, RichText, Ui};
use serde_json::{Value, json};

use super::Books;
use crate::company::CompanyLink;
use crate::i18n::t;
use crate::screens::company::form::{self, DateInput};
use crate::screens::invoice::engine::{self, Job};
use crate::theme::Tokens;
use crate::windows::Screen;

/// The command and window kind.
pub const KIND: &str = "vendors.adjust_inventory";

/// Blank lines a fresh form shows.
const BLANK_ROWS: usize = 4;

/// The Adjust Quantity/Value on Hand window.
pub struct AdjustScreen {
    link: Arc<CompanyLink>,
    group: Option<RequestGroup>,
    available: bool,
    today: CalendarDate,
    books: Books,
    books_job: Option<Job<Books>>,
    pub(crate) date: DateInput,
    pub(crate) draft: AdjustmentDraft,
    save_job: Option<(SaveIntent, Job<SavedAdjustment>)>,
    problems: Vec<AdjustProblem>,
    refusal: Option<String>,
    error: Option<ApiError>,
    warnings: Vec<String>,
    saved: u32,
    dirty: bool,
}

fn blank(today: CalendarDate) -> AdjustmentDraft {
    AdjustmentDraft {
        date: Some(today),
        rows: vec![AdjustRow::default(); BLANK_ROWS],
        ..AdjustmentDraft::default()
    }
}

impl AdjustScreen {
    /// The window over the open company.
    #[must_use]
    pub fn new(link: Arc<CompanyLink>, today: CalendarDate) -> AdjustScreen {
        let group = link.group().and_then(Result::ok);
        let available = link.is_demo() || link.capabilities().has(Capabilities::INVENTORY_ADJUST);
        let books_job = Some(super::load_books(&link, group.as_ref()));
        let mut s = AdjustScreen {
            link,
            group,
            available,
            today,
            books: Books::default(),
            books_job,
            date: DateInput::new(today),
            draft: blank(today),
            save_job: None,
            problems: Vec::new(),
            refusal: None,
            error: None,
            warnings: Vec::new(),
            saved: 0,
            dirty: false,
        };
        s.poll();
        s
    }

    fn poll(&mut self) {
        if let Some(r) = engine::poll(&mut self.books_job) {
            match r {
                Ok(b) => {
                    if self.draft.account.is_none() {
                        self.draft.account = default_account(&b);
                    }
                    self.books = b;
                    self.refresh_on_hand();
                }
                Err(e) => self.error = Some(e),
            }
        }
        let done = match self.save_job.as_mut() {
            Some((intent, job)) => job.take().map(|r| (*intent, r)),
            None => None,
        };
        if let Some((intent, r)) = done {
            self.save_job = None;
            match r {
                Ok(saved) => self.after_save(intent, &saved),
                Err(e) => self.failed(e),
            }
        }
    }

    fn refresh_on_hand(&mut self) {
        for r in &mut self.draft.rows {
            r.on_hand = r
                .item
                .and_then(|i| self.books.part(i))
                .and_then(|p| p.stock.quantity_on_hand);
        }
    }

    fn failed(&mut self, e: ApiError) {
        let lock = super::lock_of(&self.link, &e);
        match super::refusal_text(&e, lock) {
            Some(text) => self.refusal = Some(text),
            None => self.error = Some(e),
        }
    }

    fn after_save(&mut self, intent: SaveIntent, saved: &SavedAdjustment) {
        self.saved += 1;
        self.warnings = super::negative_texts(&saved.warnings, |id| self.books.name(id));
        super::announce(&self.link, &saved.warnings, &[]);
        self.link.bus().publish(crate::data_bus::Change::Items);
        self.link
            .bus()
            .publish(crate::data_bus::Change::Ledger(None));
        self.link.push(crate::company::CompanyEvent::Toast(
            "inventory.adjust.saved",
        ));
        self.dirty = false;
        let account = self.draft.account;
        self.draft = blank(self.today);
        self.draft.account = account;
        self.date = DateInput::new(self.today);
        self.books_job = Some(super::load_books(&self.link, self.group.as_ref()));
        if intent == SaveIntent::SaveAndClose {
            self.link
                .push(crate::company::CompanyEvent::CloseWindow(CommandId::new(
                    KIND,
                )));
        }
    }

    /// Alt+N / Alt+S.
    pub(crate) fn save(&mut self, intent: SaveIntent) -> bool {
        if self.save_job.is_some() || !self.available {
            return false;
        }
        self.refusal = None;
        self.error = None;
        self.draft.date = self.date.value;
        let lock = super::demo_lock(&self.link);
        let checked = match check_adjustment(&self.draft, lock) {
            Ok(c) => c,
            Err(p) => {
                self.problems = p;
                return false;
            }
        };
        self.problems.clear();
        let a = NewAdjustment {
            date: checked.date,
            adjustment_account_id: checked.account,
            memo: checked.memo,
            lines: checked
                .lines
                .iter()
                .map(|l| cl_api::AdjustmentLine {
                    item_id: l.item,
                    quantity_change: l.quantity_change,
                    value_change: l.value_change,
                    unit_id: l.unit,
                })
                .collect(),
        };
        let key = IdempotencyKey::new();
        let label = OrphanLabel {
            what: "orphan.inventory_adjustment",
            key: Some(key.clone()),
        };
        let a2 = a.clone();
        let job = engine::write(
            &self.link,
            self.group.as_ref(),
            label,
            move |d| crate::company::demo::stock::create_adjustment(d, &a2),
            move |s| async move {
                s.inventory()
                    .create_adjustment(&a, &PostingOptions::default(), key)
                    .await
            },
        );
        self.save_job = Some((intent, job));
        self.poll();
        true
    }

    fn clear(&mut self) {
        let account = self.draft.account;
        self.draft = blank(self.today);
        self.draft.account = account;
        self.date = DateInput::new(self.today);
        self.problems.clear();
        self.refusal = None;
        self.dirty = false;
    }

    // ---- Drawing ----------------------------------------------------------------

    fn draw(&mut self, ui: &mut Ui) {
        let tokens = Tokens::of(ui.ctx());
        ui.label(
            RichText::new(t("inventory.adjust.title"))
                .size(22.0)
                .strong(),
        );
        if !self.available {
            form::hint(ui, &tokens, &t("inventory.needs_update"));
            return;
        }
        form::hint(ui, &tokens, &t("inventory.adjust.intro"));
        ui.add_space(6.0);
        self.header(ui);
        ui.add_space(8.0);
        if self.books.parts.is_empty() && self.books_job.is_none() {
            form::hint(ui, &tokens, &t("inventory.no_parts"));
        } else {
            self.lines(ui, &tokens);
        }
        ui.add_space(8.0);
        for p in &self.problems {
            ui.label(RichText::new(problem_text(*p, &self.link)).color(tokens.warn_text));
        }
        if let Some(r) = &self.refusal {
            form::inline_warning(ui, &tokens, r);
        }
        if let Some(e) = &self.error {
            form::inline_error(ui, &tokens, e);
        }
        for w in &self.warnings {
            form::inline_warning(ui, &tokens, w);
        }
        if self.save_job.is_some() {
            form::progress_line(ui, &tokens, &t("inventory.saving"));
        }
        ui.add_space(6.0);
        let mut intent = None;
        let mut clear = false;
        ui.horizontal(|ui| {
            if crate::widgets::primary_button(ui, &t("inventory.save_close"))
                .on_hover_text(t("inventory.save_close.tip"))
                .clicked()
            {
                intent = Some(SaveIntent::SaveAndClose);
            }
            if ui
                .button(t("inventory.save_new"))
                .on_hover_text(t("inventory.save_new.tip"))
                .clicked()
            {
                intent = Some(SaveIntent::SaveAndNew);
            }
            if ui.button(t("inventory.clear")).clicked() {
                clear = true;
            }
        });
        if let Some(i) = intent {
            self.save(i);
        }
        if clear {
            self.clear();
        }
    }

    fn header(&mut self, ui: &mut Ui) {
        ui.horizontal_wrapped(|ui| {
            let before = self.date.value;
            form::date_field(ui, &t("inventory.adjust.date"), &mut self.date, self.today);
            if self.date.value != before {
                self.dirty = true;
            }
            ui.separator();
            let choices = adjustment_accounts(&self.books);
            let shown = self
                .draft
                .account
                .map_or_else(|| t("inventory.choose"), |a| self.books.account_name(a));
            let l = ui.label(t("inventory.adjust.account"));
            let mut pick = None;
            egui::ComboBox::from_id_salt("inventory_adjust_account")
                .selected_text(shown)
                .width(240.0)
                .show_ui(ui, |ui| {
                    for (id, n) in &choices {
                        if ui
                            .selectable_label(self.draft.account == Some(*id), n)
                            .clicked()
                        {
                            pick = Some(*id);
                        }
                    }
                })
                .response
                .labelled_by(l.id)
                .on_hover_text(t("inventory.adjust.account.tip"));
            if let Some(a) = pick {
                self.draft.account = Some(a);
                self.dirty = true;
            }
        });
        ui.horizontal(|ui| {
            let l = ui.label(t("inventory.adjust.mode"));
            for (m, key) in [
                (AdjustMode::Quantity, "inventory.adjust.mode.quantity"),
                (AdjustMode::Value, "inventory.adjust.mode.value"),
                (AdjustMode::Both, "inventory.adjust.mode.both"),
            ] {
                if ui
                    .radio(self.draft.mode == m, t(key))
                    .labelled_by(l.id)
                    .clicked()
                {
                    self.draft.mode = m;
                }
            }
        });
        ui.horizontal(|ui| {
            let l = ui.label(t("inventory.adjust.memo"));
            if ui
                .add_sized(
                    [420.0, 24.0],
                    egui::TextEdit::singleline(&mut self.draft.memo),
                )
                .labelled_by(l.id)
                .changed()
            {
                self.dirty = true;
            }
        });
    }

    fn lines(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let parts = self.books.active_parts();
        let show_qty = self.draft.mode != AdjustMode::Value;
        let show_value = self.draft.mode != AdjustMode::Quantity;
        let mut heads = vec![
            "inventory.adjust.col.item",
            "inventory.adjust.col.on_hand",
            "inventory.adjust.col.value",
        ];
        if show_qty {
            heads.push("inventory.adjust.col.qty_change");
            heads.push("inventory.adjust.col.new_qty");
        }
        if show_value {
            heads.push("inventory.adjust.col.value_change");
        }
        let mut changed_item = false;
        egui::Grid::new("inventory_adjust_lines")
            .striped(true)
            .spacing([12.0, 4.0])
            .show(ui, |ui| {
                for h in &heads {
                    ui.label(RichText::new(t(h)).strong().size(12.0));
                }
                ui.end_row();
                for (i, row) in self.draft.rows.iter_mut().enumerate() {
                    let shown = row
                        .item
                        .and_then(|id| parts.iter().find(|(x, _)| *x == id))
                        .map_or_else(|| t("inventory.choose_item"), |(_, n)| n.clone());
                    let mut pick = None;
                    egui::ComboBox::from_id_salt(("inventory_adjust_item", i))
                        .selected_text(shown)
                        .width(220.0)
                        .show_ui(ui, |ui| {
                            for (id, n) in &parts {
                                if ui.selectable_label(row.item == Some(*id), n).clicked() {
                                    pick = Some(*id);
                                }
                            }
                        })
                        .response
                        .on_hover_text(t("inventory.adjust.col.item"))
                        .widget_info(|| {
                            egui::WidgetInfo::labeled(
                                egui::Role::ComboBox,
                                true,
                                format!("{} {}", t("inventory.adjust.col.item"), i + 1),
                            )
                        });
                    if let Some(p) = pick {
                        if row.item != Some(p) {
                            row.unit = None;
                        }
                        row.item = Some(p);
                        changed_item = true;
                    }
                    let part = row.item.and_then(|id| self.books.part(id));
                    let stock = part.map(|p| &p.stock);
                    ui.label(
                        RichText::new(
                            stock
                                .and_then(|s| s.quantity_on_hand)
                                .map(super::qty)
                                .unwrap_or_default(),
                        )
                        .monospace(),
                    );
                    ui.label(
                        RichText::new(
                            stock
                                .and_then(|s| s.asset_value)
                                .map(super::money)
                                .unwrap_or_default(),
                        )
                        .monospace(),
                    );
                    if show_qty {
                        let label = format!("{} {}", t("inventory.adjust.col.qty_change"), i + 1);
                        let units = part.map_or(&[][..], |p| p.stock.unit_prices.as_slice());
                        ui.horizontal(|ui| {
                            let r = ui.add_sized(
                                [90.0, 22.0],
                                egui::TextEdit::singleline(&mut row.quantity),
                            );
                            r.widget_info(|| {
                                egui::WidgetInfo::labeled(egui::Role::TextInput, true, &label)
                            });
                            // U-161: a part with a unit set picks the unit.
                            super::unit_picker(
                                ui,
                                "inventory_adjust_unit",
                                i,
                                units,
                                &mut row.unit,
                            );
                        });
                        let new_q = cl_model::inventory::new_quantity(row);
                        ui.label(
                            RichText::new(new_q.map(super::qty).unwrap_or_default())
                                .monospace()
                                .color(tokens.text_secondary),
                        );
                    }
                    if show_value {
                        let label = format!("{} {}", t("inventory.adjust.col.value_change"), i + 1);
                        let r =
                            ui.add_sized([100.0, 22.0], egui::TextEdit::singleline(&mut row.value));
                        r.widget_info(|| {
                            egui::WidgetInfo::labeled(egui::Role::TextInput, true, &label)
                        });
                    }
                    ui.end_row();
                }
            });
        if changed_item {
            self.dirty = true;
            self.refresh_on_hand();
        }
        if ui.button(t("inventory.adjust.add_line")).clicked() {
            self.draft.rows.push(AdjustRow::default());
        }
        form::hint(
            ui,
            tokens,
            &t(match self.draft.mode {
                AdjustMode::Quantity => "inventory.adjust.hint.quantity",
                AdjustMode::Value => "inventory.adjust.hint.value",
                AdjustMode::Both => "inventory.adjust.hint.both",
            }),
        );
    }

    fn state_json(&self) -> Value {
        json!({
            "screen": "inventory_adjust",
            "available": self.available,
            "parts": self.books.parts.len(),
            "mode": match self.draft.mode {
                AdjustMode::Quantity => "quantity",
                AdjustMode::Value => "value",
                AdjustMode::Both => "both",
            },
            "account": self.draft.account.is_some(),
            "rows": self.draft.rows.len(),
            "lines": self
                .draft
                .rows
                .iter()
                .filter(|r| r.item.is_some())
                .count(),
            "problems": self
                .problems
                .iter()
                .map(|p| p.key().as_str().rsplit('.').next().unwrap_or_default())
                .collect::<Vec<_>>(),
            "refused": self.refusal.is_some(),
            "error": self.error.is_some(),
            "warnings": self.warnings.len(),
            "saving": self.save_job.is_some(),
            "saved": self.saved,
            "dirty": self.is_dirty(),
        })
    }

    fn is_dirty(&self) -> bool {
        self.dirty
            || self
                .draft
                .rows
                .iter()
                .any(|r| r.item.is_some() || !r.quantity.is_empty() || !r.value.is_empty())
    }
}

/// The adjustment account the form starts on: cost of goods sold.
fn default_account(b: &Books) -> Option<AccountId> {
    b.accounts
        .iter()
        .find(|a| a.is_active && a.system_tag.as_deref() == Some("cost_of_goods_sold"))
        .or_else(|| {
            b.accounts
                .iter()
                .find(|a| a.is_active && a.fields.account_type == AccountType::Cogs)
        })
        .map(|a| a.id)
}

/// Accounts an adjustment may post to: expense, cost of goods sold and
/// equity accounts (never an inventory asset account itself).
fn adjustment_accounts(b: &Books) -> Vec<(AccountId, String)> {
    b.accounts
        .iter()
        .filter(|a| {
            a.is_active
                && matches!(
                    a.fields.account_type,
                    AccountType::Cogs
                        | AccountType::Expense
                        | AccountType::OtherExpense
                        | AccountType::Equity
                        | AccountType::Revenue
                        | AccountType::OtherRevenue
                )
        })
        .map(|a| (a.id, b.account_name(a.id)))
        .collect()
}

fn problem_text(p: AdjustProblem, link: &CompanyLink) -> String {
    match p {
        AdjustProblem::ClosedPeriod => super::closed_text(super::demo_lock(link)),
        other => {
            let base = t(other.key().as_str());
            match other {
                AdjustProblem::ItemTwice(i)
                | AdjustProblem::QuantityInvalid(i)
                | AdjustProblem::ValueInvalid(i)
                | AdjustProblem::ItemMissing(i) => {
                    format!(
                        "{} {}",
                        crate::i18n::t_with("inventory.line_n", &[("n", &(i + 1).to_string())]),
                        base
                    )
                }
                _ => base,
            }
        }
    }
}

impl Screen for AdjustScreen {
    fn title(&self) -> String {
        t("inventory.adjust.title")
    }

    fn ui(&mut self, ui: &mut Ui) {
        self.poll();
        egui::ScrollArea::both().show(ui, |ui| self.draw(ui));
        if self.books_job.is_some() || self.save_job.is_some() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    fn dirty(&self) -> bool {
        self.is_dirty()
    }

    fn scope(&self) -> Scope {
        Scope::Form
    }

    fn command(&mut self, id: CommandId) -> bool {
        match id.as_str() {
            KIND => true,
            "form.save_new" => self.save(SaveIntent::SaveAndNew),
            "form.save_close" => self.save(SaveIntent::SaveAndClose),
            "form.clear" => {
                self.clear();
                true
            }
            super::DEV_RESET => {
                let ok = super::demo_reset(&self.link);
                self.books_job = Some(super::load_books(&self.link, self.group.as_ref()));
                self.clear();
                self.saved = 0;
                self.warnings.clear();
                self.poll();
                ok
            }
            _ => false,
        }
    }

    fn save(&mut self) -> bool {
        AdjustScreen::save(self, SaveIntent::Save)
    }

    fn state(&self) -> Value {
        self.state_json()
    }
}

#[cfg(test)]
#[path = "adjust_tests.rs"]
mod tests;
