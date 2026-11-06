//! Physical Inventory Count (P24 plan §3, `vendors.physical_count`; engine
//! U-159, Q-P24g): the count sheet as of a date, a Counted column the
//! owner fills in, the difference each row would post, and Post. The
//! engine recomputes on hand at post time; when it moved since the sheet
//! was read (409 `STALE`), the sheet is read again, the typed counts stay
//! and the changed rows are highlighted. Print makes the paper count sheet
//! (on hand optional, a blank Counted column).

use std::sync::Arc;

use cl_api::{
    AccountType, ApiError, Capabilities, CountResult, CountSheet, IdempotencyKey, NewCount,
    OrphanLabel, PostingOptions, RequestGroup,
};
use cl_core::CalendarDate;
use cl_core::ids::{AccountId, ItemId};
use cl_model::inventory::{CountProblem, CountWorksheet};
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

/// The command and window kind.
pub const KIND: &str = "vendors.physical_count";

/// The count worksheet window.
pub struct CountScreen {
    link: Arc<CompanyLink>,
    group: Option<RequestGroup>,
    available: bool,
    today: CalendarDate,
    books: Books,
    books_job: Option<Job<Books>>,
    pub(crate) date: DateInput,
    pub(crate) sheet: CountWorksheet,
    descriptions: Vec<(ItemId, String)>,
    sheet_job: Option<Job<CountSheet>>,
    /// The items the engine named on a 409 `STALE` (the reload marks them).
    stale: Option<Vec<ItemId>>,
    post_job: Option<Job<CountResult>>,
    pdf_job: Option<Job<(String, Vec<u8>)>>,
    /// Print on hand on the paper sheet.
    pub(crate) print_on_hand: bool,
    problems: Vec<CountProblem>,
    refusal: Option<String>,
    error: Option<ApiError>,
    notice: Option<String>,
    posted: u32,
}

impl CountScreen {
    /// The window over the open company, counting as of today.
    #[must_use]
    pub fn new(link: Arc<CompanyLink>, today: CalendarDate) -> CountScreen {
        let group = link.group().and_then(Result::ok);
        let available = link.is_demo() || link.capabilities().has(Capabilities::INVENTORY_ADJUST);
        let books_job = Some(super::load_books(&link, group.as_ref()));
        let mut s = CountScreen {
            link,
            group,
            available,
            today,
            books: Books::default(),
            books_job,
            date: DateInput::new(today),
            sheet: CountWorksheet::default(),
            descriptions: Vec::new(),
            sheet_job: None,
            stale: None,
            post_job: None,
            pdf_job: None,
            print_on_hand: false,
            problems: Vec::new(),
            refusal: None,
            error: None,
            notice: None,
            posted: 0,
        };
        s.read_sheet();
        s
    }

    /// Reads the sheet as of the date on screen (a fresh worksheet, or
    /// after `STALE` the same one with on hand replaced).
    pub(crate) fn read_sheet(&mut self) {
        if !self.available {
            return;
        }
        let Some(as_of) = self.date.value else {
            self.problems = vec![CountProblem::DateMissing];
            return;
        };
        self.sheet_job = Some(engine::read(
            &self.link,
            self.group.as_ref(),
            move |d| crate::company::demo::stock::count_sheet(d, as_of),
            move |s| async move { s.inventory().count_sheet(as_of).await },
        ));
        self.poll();
    }

    fn poll(&mut self) {
        if let Some(r) = engine::poll(&mut self.books_job) {
            match r {
                Ok(b) => {
                    self.books = b;
                    if self.sheet.account.is_none() {
                        self.sheet.account = default_account(&self.books);
                    }
                }
                Err(e) => self.error = Some(e),
            }
        }
        if let Some(r) = engine::poll(&mut self.sheet_job) {
            match r {
                Ok(sheet) => self.take_sheet(sheet),
                Err(e) => self.failed(e),
            }
        }
        if let Some(r) = engine::poll(&mut self.post_job) {
            match r {
                Ok(done) => self.after_post(&done),
                Err(e) => self.failed(e),
            }
        }
        match engine::poll(&mut self.pdf_job) {
            Some(Ok((name, bytes))) => self.link.push(CompanyEvent::OpenWith(
                CommandId::new(crate::screens::document_view::KIND),
                Box::new(crate::windows::OpenPayload::Document(
                    crate::widgets::pdf_viewer::DocSource {
                        name,
                        bytes: Arc::from(bytes),
                    },
                )),
            )),
            Some(Err(_)) => self.notice = Some(t("inventory.count.print_failed")),
            None => {}
        }
    }

    fn take_sheet(&mut self, sheet: CountSheet) {
        self.descriptions = sheet
            .rows
            .iter()
            .map(|r| (r.item_id, r.description.clone().unwrap_or_default()))
            .collect();
        if let Some(stale) = self.stale.take() {
            self.sheet
                .reload(sheet.rows.iter().map(|r| (r.item_id, r.on_hand)), &stale);
            let n = self.sheet.rows.iter().filter(|r| r.changed).count();
            self.notice = Some(t_with(
                "inventory.count.stale",
                &[("n", &n.max(stale.len()).to_string())],
            ));
        } else {
            let account = self.sheet.account;
            let memo = std::mem::take(&mut self.sheet.memo);
            self.sheet = CountWorksheet::new(
                sheet.as_of,
                sheet.rows.into_iter().map(|r| {
                    (
                        r.item_id,
                        r.name.unwrap_or_default(),
                        r.unit_label,
                        r.on_hand,
                    )
                }),
            );
            self.sheet.account = account.or_else(|| default_account(&self.books));
            self.sheet.memo = memo;
        }
    }

    fn failed(&mut self, e: ApiError) {
        if let Some(items) = cl_api::count_stale_items(&e) {
            // Q-P24g: read the sheet again; the typed counts stay.
            self.stale = Some(items);
            self.read_sheet();
            return;
        }
        let lock = super::lock_of(&self.link, &e);
        match super::refusal_text(&e, lock) {
            Some(text) => self.refusal = Some(text),
            None => self.error = Some(e),
        }
    }

    fn after_post(&mut self, done: &CountResult) {
        self.posted += 1;
        super::announce(&self.link, &done.warnings, &[]);
        self.link.bus().publish(crate::data_bus::Change::Items);
        self.link
            .bus()
            .publish(crate::data_bus::Change::Ledger(None));
        self.notice = Some(t(if done.adjustment.is_some() {
            "inventory.count.posted"
        } else {
            "inventory.count.nothing_changed"
        }));
        self.link
            .push(CompanyEvent::Toast("inventory.count.posted_toast"));
        self.books_job = Some(super::load_books(&self.link, self.group.as_ref()));
        self.read_sheet();
    }

    /// Post the count (Post is disabled until a row is counted).
    pub(crate) fn post(&mut self) -> bool {
        if self.post_job.is_some() || self.sheet_job.is_some() || !self.available {
            return false;
        }
        self.refusal = None;
        self.error = None;
        self.notice = None;
        self.sheet.as_of = self.date.value;
        for r in &mut self.sheet.rows {
            r.changed = false;
        }
        let lock = super::demo_lock(&self.link);
        let lines = match self.sheet.check(lock) {
            Ok(l) => l,
            Err(p) => {
                self.problems = p;
                return false;
            }
        };
        self.problems.clear();
        let (Some(as_of), Some(account)) = (self.sheet.as_of, self.sheet.account) else {
            return false;
        };
        let c = NewCount {
            as_of,
            adjustment_account_id: account,
            memo: (!self.sheet.memo.trim().is_empty()).then(|| self.sheet.memo.trim().to_string()),
            lines: lines
                .iter()
                .map(|l| cl_api::CountLine {
                    item_id: l.item,
                    sheet_on_hand: l.sheet_on_hand,
                    counted_quantity: l.counted,
                    unit_id: None,
                })
                .collect(),
        };
        let key = IdempotencyKey::new();
        let label = OrphanLabel {
            what: "orphan.inventory_count",
            key: Some(key.clone()),
        };
        let c2 = c.clone();
        self.post_job = Some(engine::write(
            &self.link,
            self.group.as_ref(),
            label,
            move |d| crate::company::demo::stock::post_count(d, &c2),
            move |s| async move {
                s.inventory()
                    .post_count(&c, &PostingOptions::default(), key)
                    .await
            },
        ));
        self.poll();
        true
    }

    /// Print the count sheet (a PDF made off the frame, opened full size
    /// with the system's Print).
    pub(crate) fn print(&mut self) {
        let data = cl_print::forms::count_sheet::CountSheetData {
            date: self
                .sheet
                .as_of
                .map(crate::shell::us_date)
                .unwrap_or_default(),
            show_on_hand: self.print_on_hand,
            lines: self
                .sheet
                .rows
                .iter()
                .map(|r| cl_print::forms::count_sheet::CountSheetLine {
                    item: r.name.clone(),
                    description: self
                        .descriptions
                        .iter()
                        .find(|(i, _)| *i == r.item)
                        .map(|(_, d)| d.clone())
                        .unwrap_or_default(),
                    unit: r.unit_label.clone().unwrap_or_default(),
                    on_hand: super::qty(r.on_hand),
                })
                .collect(),
            labels: cl_print::forms::count_sheet::CountSheetLabels {
                title: t("inventory.count.title"),
                date: t("inventory.count.date"),
                item: t("inventory.count.col.item"),
                description: t("inventory.count.col.description"),
                unit: t("inventory.count.col.unit"),
                on_hand: t("inventory.count.col.on_hand"),
                counted: t("inventory.count.col.counted"),
                counted_by: t("inventory.count.counted_by"),
            },
        };
        let style = cl_print::FormStyle {
            company: cl_print::forms::CompanyInfo {
                name: self.company_name(),
                lines: Vec::new(),
            },
            logo: None,
            footer: String::new(),
            labels: crate::screens::print_forms::job::form_labels(),
        };
        let name = cl_print::pdf_file_name(&t("inventory.count.file"), &data.date, "");
        let (d2, s2, n2) = (data.clone(), style.clone(), name.clone());
        self.pdf_job = Some(engine::read(
            &self.link,
            self.group.as_ref(),
            move |_| render(&d2, &s2, n2),
            move |_| async move { render(&data, &style, name) },
        ));
        self.poll();
    }

    fn company_name(&self) -> String {
        self.link
            .demo_state()
            .and_then(|d| {
                let open = d.open?;
                d.companies
                    .iter()
                    .find(|c| c.tenant_id == open)
                    .map(|c| c.name.clone())
            })
            .unwrap_or_default()
    }

    // ---- Drawing ----------------------------------------------------------------

    fn draw(&mut self, ui: &mut Ui) {
        let tokens = Tokens::of(ui.ctx());
        ui.label(
            RichText::new(t("inventory.count.title"))
                .size(22.0)
                .strong(),
        );
        if !self.available {
            form::hint(ui, &tokens, &t("inventory.needs_update"));
            return;
        }
        form::hint(ui, &tokens, &t("inventory.count.intro"));
        ui.add_space(6.0);
        self.controls(ui);
        ui.add_space(6.0);
        if let Some(n) = &self.notice {
            form::inline_warning(ui, &tokens, n);
        }
        for p in &self.problems {
            let text = match p {
                CountProblem::ClosedPeriod => super::closed_text(super::demo_lock(&self.link)),
                CountProblem::CountInvalid(i) => format!(
                    "{} {}",
                    t_with("inventory.line_n", &[("n", &(i + 1).to_string())]),
                    t(p.key().as_str())
                ),
                other => t(other.key().as_str()),
            };
            ui.label(RichText::new(text).color(tokens.warn_text));
        }
        if let Some(r) = &self.refusal {
            form::inline_warning(ui, &tokens, r);
        }
        if let Some(e) = &self.error {
            form::inline_error(ui, &tokens, e);
        }
        if self.sheet_job.is_some() {
            form::progress_line(ui, &tokens, &t("inventory.loading"));
        } else if self.sheet.rows.is_empty() {
            form::hint(ui, &tokens, &t("inventory.no_parts"));
        } else {
            self.table(ui, &tokens);
        }
        ui.add_space(6.0);
        form::hint(ui, &tokens, &t("inventory.count.help_same_day"));
        ui.add_space(6.0);
        let mut post = false;
        let mut print = false;
        ui.horizontal(|ui| {
            let can = self.sheet.can_post() && self.post_job.is_none();
            if ui
                .add_enabled(can, egui::Button::new(t("inventory.count.post")))
                .on_hover_text(t("inventory.count.post.tip"))
                .on_disabled_hover_text(t("inventory.count.post.disabled"))
                .clicked()
            {
                post = true;
            }
            if ui.button(t("inventory.count.print")).clicked() {
                print = true;
            }
            ui.checkbox(&mut self.print_on_hand, t("inventory.count.print_on_hand"));
        });
        if post {
            self.post();
        }
        if print {
            self.print();
        }
    }

    fn controls(&mut self, ui: &mut Ui) {
        ui.horizontal_wrapped(|ui| {
            let r = form::date_field(ui, &t("inventory.count.date"), &mut self.date, self.today);
            if r.lost_focus() && self.date.value != self.sheet.as_of {
                self.read_sheet();
            }
            ui.separator();
            let choices: Vec<(AccountId, String)> = self
                .books
                .accounts
                .iter()
                .filter(|a| {
                    a.is_active
                        && matches!(
                            a.fields.account_type,
                            AccountType::Cogs | AccountType::Expense | AccountType::OtherExpense
                        )
                })
                .map(|a| (a.id, self.books.account_name(a.id)))
                .collect();
            let shown = self
                .sheet
                .account
                .map_or_else(|| t("inventory.choose"), |a| self.books.account_name(a));
            let l = ui.label(t("inventory.count.account"));
            let mut pick = None;
            egui::ComboBox::from_id_salt("inventory_count_account")
                .selected_text(shown)
                .width(240.0)
                .show_ui(ui, |ui| {
                    for (id, n) in &choices {
                        if ui
                            .selectable_label(self.sheet.account == Some(*id), n)
                            .clicked()
                        {
                            pick = Some(*id);
                        }
                    }
                })
                .response
                .labelled_by(l.id);
            if let Some(a) = pick {
                self.sheet.account = Some(a);
            }
            if ui.button(t("inventory.count.reload")).clicked() {
                self.read_sheet();
            }
        });
    }

    fn table(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let heads = [
            "inventory.count.col.item",
            "inventory.count.col.description",
            "inventory.count.col.unit",
            "inventory.count.col.on_hand",
            "inventory.count.col.counted",
            "inventory.count.col.difference",
        ];
        let diffs: Vec<Option<String>> = (0..self.sheet.rows.len())
            .map(|i| self.sheet.difference(i).map(super::qty))
            .collect();
        let descriptions = &self.descriptions;
        egui::Grid::new("inventory_count_rows")
            .striped(true)
            .spacing([14.0, 4.0])
            .show(ui, |ui| {
                for h in heads {
                    ui.label(RichText::new(t(h)).strong().size(12.0));
                }
                ui.end_row();
                for (i, r) in self.sheet.rows.iter_mut().enumerate() {
                    let color = if r.changed {
                        tokens.warn_text
                    } else {
                        tokens.text_primary
                    };
                    let mut name = RichText::new(&r.name).color(color);
                    if r.changed {
                        name = name.strong();
                    }
                    ui.label(name);
                    ui.label(
                        descriptions
                            .iter()
                            .find(|(x, _)| *x == r.item)
                            .map(|(_, d)| d.clone())
                            .unwrap_or_default(),
                    );
                    ui.label(r.unit_label.clone().unwrap_or_default());
                    let on_hand = ui.label(
                        RichText::new(super::qty(r.on_hand))
                            .monospace()
                            .color(color),
                    );
                    if r.changed {
                        on_hand.on_hover_text(t("inventory.count.changed"));
                    }
                    let label = format!("{} {}", t("inventory.count.col.counted"), r.name);
                    ui.add_sized([90.0, 22.0], egui::TextEdit::singleline(&mut r.counted))
                        .widget_info(|| {
                            egui::WidgetInfo::labeled(egui::Role::TextInput, true, &label)
                        });
                    ui.label(
                        RichText::new(diffs.get(i).cloned().flatten().unwrap_or_default())
                            .monospace(),
                    );
                    ui.end_row();
                }
            });
    }

    fn state_json(&self) -> Value {
        json!({
            "screen": "inventory_count",
            "available": self.available,
            "rows": self.sheet.rows.len(),
            "counted": self.sheet.rows.iter().filter(|r| !r.counted.trim().is_empty()).count(),
            "changed": self.sheet.rows.iter().filter(|r| r.changed).count(),
            "can_post": self.sheet.can_post(),
            "loading": self.sheet_job.is_some(),
            "posting": self.post_job.is_some(),
            "posted": self.posted,
            "problems": self
                .problems
                .iter()
                .map(|p| p.key().as_str().rsplit('.').next().unwrap_or_default())
                .collect::<Vec<_>>(),
            "refused": self.refusal.is_some(),
            "error": self.error.is_some(),
            "notice": self.notice.is_some(),
            "printing": self.pdf_job.is_some(),
        })
    }
}

fn render(
    d: &cl_print::forms::count_sheet::CountSheetData,
    style: &cl_print::FormStyle,
    name: String,
) -> Result<(String, Vec<u8>), ApiError> {
    let doc = cl_print::forms::count_sheet::layout(d, style)
        .map_err(|_| ApiError::Local(cl_api::LocalValidation::InvalidValue))?;
    let bytes = cl_print::render(&doc)
        .map_err(|_| ApiError::Local(cl_api::LocalValidation::InvalidValue))?;
    Ok((name, bytes))
}

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

impl Screen for CountScreen {
    fn title(&self) -> String {
        t("inventory.count.title")
    }

    fn ui(&mut self, ui: &mut Ui) {
        self.poll();
        egui::ScrollArea::both().show(ui, |ui| self.draw(ui));
        if self.sheet_job.is_some() || self.post_job.is_some() || self.pdf_job.is_some() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    fn dirty(&self) -> bool {
        self.sheet.can_post()
    }

    fn scope(&self) -> Scope {
        Scope::Form
    }

    fn command(&mut self, id: CommandId) -> bool {
        match id.as_str() {
            KIND => true,
            "view.refresh" => {
                self.read_sheet();
                true
            }
            "file.print" => {
                self.print();
                true
            }
            "form.save_close" | "form.save_new" => self.post(),
            super::DEV_RESET => {
                let ok = super::demo_reset(&self.link);
                self.posted = 0;
                self.notice = None;
                self.stale = None;
                self.sheet = CountWorksheet::default();
                self.books_job = Some(super::load_books(&self.link, self.group.as_ref()));
                self.read_sheet();
                ok
            }
            // Demo only: a sale of 2 of the first part posted in "another
            // window" after the sheet was read (the stale-sheet scenario).
            "dev.demo_inventory_sale" => {
                let Some(mut d) = self.link.demo_state() else {
                    return false;
                };
                let ok = crate::company::demo::stock::story_sale(&mut d).is_ok();
                drop(d);
                ok
            }
            _ => false,
        }
    }

    fn save(&mut self) -> bool {
        self.post()
    }

    fn state(&self) -> Value {
        self.state_json()
    }
}

#[cfg(test)]
#[path = "count_tests.rs"]
mod tests;
