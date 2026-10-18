//! New Company interview (P08 step 2): six short pages in plain language,
//! then Create. Form scope: Alt+S (Save & Close) is "Next" or "Create",
//! Esc reverts the page, then closes (asking first when something was
//! typed). On success the window opens the Recovery key screen (step 3)
//! and offers Opening balances (step 4).

use std::sync::Arc;

use cl_api::{AccountingMethod, Capabilities, EntityType};
use cl_company::{CompanyError, CompanySetup, Created, SetupIssue, StarterChart};
use cl_core::Secret;
use cl_model::{CommandId, Scope};
use cl_settings::{ACCENT_PALETTE, FiscalCalendar};
use eframe::egui::{self, RichText, Ui};
use serde_json::{Value, json};

use super::form;
use crate::company::{CompanyEvent, CompanyLink, Task, spawn_lifecycle};
use crate::i18n::{t, t_with};
use crate::theme::Tokens;
use crate::windows::Screen;

/// Pages of the interview.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    /// Names and legal form.
    Names,
    /// Fiscal year and calendar.
    Year,
    /// Starter chart.
    Chart,
    /// Sales tax, inventory, basis.
    Features,
    /// Closing-date password and colour.
    Security,
    /// Review and create.
    Review,
}

impl Page {
    const ALL: [Page; 6] = [
        Page::Names,
        Page::Year,
        Page::Chart,
        Page::Features,
        Page::Security,
        Page::Review,
    ];

    fn index(self) -> usize {
        Page::ALL.iter().position(|p| *p == self).unwrap_or(0)
    }

    fn next(self) -> Option<Page> {
        Page::ALL.get(self.index() + 1).copied()
    }

    fn prev(self) -> Option<Page> {
        self.index()
            .checked_sub(1)
            .and_then(|i| Page::ALL.get(i).copied())
    }

    fn key(self) -> &'static str {
        match self {
            Page::Names => "interview.page.names",
            Page::Year => "interview.page.year",
            Page::Chart => "interview.page.chart",
            Page::Features => "interview.page.features",
            Page::Security => "interview.page.security",
            Page::Review => "interview.page.review",
        }
    }
}

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// The interview window.
pub struct NewCompanyScreen {
    link: Arc<CompanyLink>,
    group: Option<cl_api::RequestGroup>,
    page: Page,
    setup: CompanySetup,
    tax_rate_text: String,
    closing_password: String,
    closing_confirm: String,
    legal_name: String,
    dirty: bool,
    issues: Vec<SetupIssue>,
    task: Option<Task<Result<Created, CompanyError>>>,
    created: Option<Created>,
    error: Option<CompanyError>,
    closing_supported: bool,
    /// The page's fields as they were when the page was entered (Esc
    /// reverts to this before it asks to close).
    committed: Committed,
}

/// The editable text of a page, for Esc.
#[derive(Clone, Default, PartialEq, Eq)]
struct Committed {
    name: String,
    legal_name: String,
    tax_rate_text: String,
    closing_password: String,
    closing_confirm: String,
}

impl NewCompanyScreen {
    fn snapshot(&self) -> Committed {
        Committed {
            name: self.setup.name.clone(),
            legal_name: self.legal_name.clone(),
            tax_rate_text: self.tax_rate_text.clone(),
            closing_password: self.closing_password.clone(),
            closing_confirm: self.closing_confirm.clone(),
        }
    }

    /// Esc: put the page's fields back as they were on entry. Returns
    /// false when nothing differed (the shell then closes the window).
    fn revert_fields(&mut self) -> bool {
        if self.snapshot() == self.committed {
            return false;
        }
        let c = self.committed.clone();
        self.setup.name = c.name;
        self.legal_name = c.legal_name;
        self.tax_rate_text = c.tax_rate_text;
        self.closing_password = c.closing_password;
        self.closing_confirm = c.closing_confirm;
        self.issues.clear();
        true
    }
    /// A fresh interview.
    #[must_use]
    pub fn new(link: Arc<CompanyLink>) -> NewCompanyScreen {
        let group = link.group().and_then(Result::ok);
        let closing_supported = link.capabilities().has(Capabilities::CLOSING_PASSWORD);
        let mut setup = CompanySetup::new("");
        if let Ok(list) = link.store().load() {
            let used: Vec<_> = list.companies.iter().map(|c| c.accent).collect();
            setup.accent = cl_settings::recent::next_accent(&used);
        }
        NewCompanyScreen {
            link,
            group,
            page: Page::Names,
            setup,
            tax_rate_text: String::new(),
            closing_password: String::new(),
            closing_confirm: String::new(),
            legal_name: String::new(),
            dirty: false,
            issues: Vec::new(),
            task: None,
            created: None,
            error: None,
            closing_supported,
            committed: Committed::default(),
        }
    }

    fn sync(&mut self) {
        self.setup.legal_name =
            (!self.legal_name.trim().is_empty()).then(|| self.legal_name.trim().to_string());
        self.setup.sales_tax_rate = if self.setup.sales_tax {
            parse_rate(&self.tax_rate_text)
        } else {
            None
        };
        self.setup.closing_password =
            (!self.closing_password.is_empty()).then(|| Secret::new(self.closing_password.clone()));
    }

    fn page_issues(&self) -> Vec<SetupIssue> {
        let all = self.setup.validate(self.closing_supported);
        all.into_iter()
            .filter(|i| match self.page {
                Page::Names => matches!(i, SetupIssue::NameRequired | SetupIssue::NameTooLong),
                Page::Year => matches!(i, SetupIssue::FiscalMonth),
                Page::Features => matches!(i, SetupIssue::TaxRateInvalid),
                Page::Security => matches!(
                    i,
                    SetupIssue::ClosingPasswordTooShort | SetupIssue::ClosingPasswordUnavailable
                ),
                Page::Chart | Page::Review => true,
            })
            .collect()
    }

    fn next(&mut self) {
        self.sync();
        self.issues = self.page_issues();
        if self.page == Page::Security && self.closing_password != self.closing_confirm {
            self.issues.push(SetupIssue::ClosingPasswordTooShort);
            return;
        }
        if !self.issues.is_empty() {
            return;
        }
        if let Some(p) = self.page.next() {
            self.page = p;
            self.committed = self.snapshot();
        } else {
            self.create();
        }
    }

    fn create(&mut self) {
        self.sync();
        self.issues = self.setup.validate(self.closing_supported);
        if !self.issues.is_empty() {
            return;
        }
        if let Some(mut demo) = self.link.demo_state() {
            let id = demo.fresh_id();
            demo.companies.push(crate::company::demo::DemoCompany {
                tenant_id: id,
                name: self.setup.name.trim().to_string(),
                accent: self.setup.accent,
                meta: t("open.meta.new"),
                is_sample: false,
                closed_through: None,
            });
            demo.open = Some(id);
            drop(demo);
            self.task = Some(Task::ready(Ok(Created {
                tenant_id: id,
                company_id: cl_core::ids::CompanyId::parse("d0000000-0000-4000-8000-00000000c0c0")
                    .expect("demo id"),
                accounts_imported: u64::try_from(
                    self.setup
                        .chart
                        .template()
                        .import_rows(self.setup.sales_tax, self.setup.inventory)
                        .len(),
                )
                .unwrap_or(0),
                accounts_skipped: 0,
                closing_password_set: self.setup.closing_password.is_some(),
            })));
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            self.error = Some(CompanyError::Unavailable("session"));
            return;
        };
        let store = self.link.store().clone();
        let setup = self.setup.clone();
        self.task = spawn_lifecycle(&self.link, group, move || async move {
            cl_company::create_company(&session, &store, &setup).await
        })
        .ok();
    }

    fn action_row(&mut self, ui: &mut Ui, salt: &str) {
        let primary = if self.page == Page::Review {
            t("interview.create")
        } else {
            t("interview.next")
        };
        let back = t("interview.back");
        let others: Vec<&str> = if self.page.prev().is_some() {
            vec![&back]
        } else {
            vec![]
        };
        let clicked = ui
            .push_id(salt, |ui| form::actions(ui, &primary, &others))
            .inner;
        match clicked {
            Some(0) => self.next(),
            Some(_) => {
                if let Some(p) = self.page.prev() {
                    self.page = p;
                    self.issues.clear();
                    self.committed = self.snapshot();
                }
            }
            None => {}
        }
    }

    fn poll(&mut self) {
        if let Some(t) = self.task.as_mut()
            && let Some(r) = t.poll_result()
        {
            self.task = None;
            match r {
                Ok(c) => {
                    self.link.push(CompanyEvent::Opened {
                        tenant_id: c.tenant_id,
                        name: self.setup.name.trim().to_string(),
                        accent: self.setup.accent,
                        closed_through: None,
                    });
                    self.created = Some(c);
                    self.dirty = false;
                    self.closing_password.clear();
                    self.closing_confirm.clear();
                }
                Err(e) => self.error = Some(e),
            }
        }
    }

    fn page_names(&mut self, ui: &mut Ui, tokens: &Tokens) {
        form::hint(ui, tokens, &t("interview.names.intro"));
        let before = (
            self.setup.name.clone(),
            self.legal_name.clone(),
            self.setup.entity_type,
        );
        form::field(ui, &t("interview.names.name"), &mut self.setup.name);
        form::field(ui, &t("interview.names.legal"), &mut self.legal_name);
        form::hint(ui, tokens, &t("interview.names.legal_hint"));
        ui.add_space(6.0);
        form::heading(ui, tokens, &t("interview.names.entity"));
        let choices: [(Option<EntityType>, &str); 6] = [
            (Some(EntityType::SoleProp), "interview.entity.sole_prop"),
            (Some(EntityType::SingleMemberLlc), "interview.entity.smllc"),
            (
                Some(EntityType::Partnership),
                "interview.entity.partnership",
            ),
            (Some(EntityType::SCorp), "interview.entity.s_corp"),
            (Some(EntityType::CCorp), "interview.entity.c_corp"),
            (None, "interview.entity.unsure"),
        ];
        ui.horizontal_wrapped(|ui| {
            for (value, key) in choices {
                if ui.radio(self.setup.entity_type == value, t(key)).clicked() {
                    self.setup.entity_type = value;
                }
            }
        });
        if before
            != (
                self.setup.name.clone(),
                self.legal_name.clone(),
                self.setup.entity_type,
            )
        {
            self.dirty = true;
            // Typing answers the message, so it goes away.
            self.issues.clear();
        }
    }

    fn page_year(&mut self, ui: &mut Ui, tokens: &Tokens) {
        form::hint(ui, tokens, &t("interview.year.intro"));
        ui.horizontal(|ui| {
            let l = ui.add(egui::Label::new(t("interview.year.start")).selectable(false));
            let current = MONTHS[usize::from(self.setup.fiscal_year_start_month.clamp(1, 12)) - 1];
            egui::ComboBox::from_id_salt("fy_month")
                .selected_text(current)
                .show_ui(ui, |ui| {
                    for (i, m) in MONTHS.iter().enumerate() {
                        let n = u8::try_from(i + 1).unwrap_or(1);
                        if ui
                            .selectable_label(self.setup.fiscal_year_start_month == n, *m)
                            .clicked()
                        {
                            self.setup.fiscal_year_start_month = n;
                            self.dirty = true;
                        }
                    }
                })
                .response
                .labelled_by(l.id);
        });
        ui.add_space(6.0);
        form::heading(ui, tokens, &t("interview.year.calendar"));
        let choices = [
            (
                FiscalCalendar::Standard,
                "interview.calendar.standard",
                "interview.calendar.standard_sub",
            ),
            (
                FiscalCalendar::Retail445,
                "interview.calendar.445",
                "interview.calendar.445_sub",
            ),
            (
                FiscalCalendar::Retail454,
                "interview.calendar.454",
                "interview.calendar.454_sub",
            ),
            (
                FiscalCalendar::Retail544,
                "interview.calendar.544",
                "interview.calendar.544_sub",
            ),
        ];
        for (value, title, sub) in choices {
            if form::choice(
                ui,
                tokens,
                self.setup.fiscal_calendar == value,
                &t(title),
                &t(sub),
            )
            .clicked()
            {
                self.setup.fiscal_calendar = value;
                self.dirty = true;
            }
            ui.add_space(4.0);
        }
        form::hint(ui, tokens, &t("interview.calendar.note"));
    }

    fn page_chart(&mut self, ui: &mut Ui, tokens: &Tokens) {
        form::hint(ui, tokens, &t("interview.chart.intro"));
        for chart in StarterChart::ALL {
            let tpl = chart.template();
            // The count the owner will get with the features chosen so far.
            let n = tpl
                .import_rows(self.setup.sales_tax, self.setup.inventory)
                .len();
            let title = format!(
                "{} \u{b7} {}",
                tpl.name,
                t_with("interview.chart.count", &[("n", &n.to_string())])
            );
            if form::choice(
                ui,
                tokens,
                self.setup.chart == chart,
                &title,
                &tpl.description,
            )
            .clicked()
            {
                self.setup.chart = chart;
                self.dirty = true;
            }
            ui.add_space(4.0);
        }
    }

    fn page_features(&mut self, ui: &mut Ui, tokens: &Tokens) {
        form::hint(ui, tokens, &t("interview.features.intro"));
        if ui
            .checkbox(&mut self.setup.sales_tax, t("interview.features.sales_tax"))
            .changed()
        {
            self.dirty = true;
        }
        if self.setup.sales_tax {
            form::field(
                ui,
                &t("interview.features.tax_rate"),
                &mut self.tax_rate_text,
            );
            form::hint(ui, tokens, &t("interview.features.tax_rate_hint"));
        }
        if ui
            .checkbox(&mut self.setup.inventory, t("interview.features.inventory"))
            .changed()
        {
            self.dirty = true;
        }
        ui.add_space(6.0);
        form::heading(ui, tokens, &t("interview.features.basis"));
        if form::choice(
            ui,
            tokens,
            self.setup.basis == AccountingMethod::Accrual,
            &t("interview.basis.accrual"),
            &t("interview.basis.accrual_sub"),
        )
        .clicked()
        {
            self.setup.basis = AccountingMethod::Accrual;
            self.dirty = true;
        }
        ui.add_space(4.0);
        if form::choice(
            ui,
            tokens,
            self.setup.basis == AccountingMethod::Cash,
            &t("interview.basis.cash"),
            &t("interview.basis.cash_sub"),
        )
        .clicked()
        {
            self.setup.basis = AccountingMethod::Cash;
            self.dirty = true;
        }
    }

    fn page_security(&mut self, ui: &mut Ui, tokens: &Tokens) {
        form::heading(ui, tokens, &t("interview.security.closing"));
        if self.closing_supported {
            form::hint(ui, tokens, &t("interview.security.closing_intro"));
            // Secret inputs: masked (`password(true)` inside `form::password`).
            if form::password(
                ui,
                &t("interview.security.closing_password"),
                &mut self.closing_password,
            )
            .changed()
            {
                self.dirty = true;
            }
            form::password(
                ui,
                &t("interview.security.closing_confirm"),
                &mut self.closing_confirm,
            );
        } else {
            form::hint(ui, tokens, &t("interview.security.closing_unavailable"));
        }
        ui.add_space(10.0);
        form::heading(ui, tokens, &t("interview.security.colour"));
        form::hint(ui, tokens, &t("interview.security.colour_intro"));
        ui.horizontal(|ui| {
            for (i, a) in ACCENT_PALETTE.iter().enumerate() {
                let name = t_with("interview.colour.swatch", &[("n", &(i + 1).to_string())]);
                if form::swatch(ui, a.0, self.setup.accent == *a, &name).clicked() {
                    self.setup.accent = *a;
                    self.dirty = true;
                }
                ui.add_space(4.0);
            }
        });
    }

    fn page_review(&mut self, ui: &mut Ui, tokens: &Tokens) {
        self.sync();
        let rows: Vec<(String, String)> = vec![
            (
                t("interview.names.name"),
                self.setup.name.trim().to_string(),
            ),
            (
                t("interview.names.legal"),
                self.setup
                    .legal_name
                    .clone()
                    .unwrap_or_else(|| t("interview.review.same")),
            ),
            (
                t("interview.year.start"),
                MONTHS[usize::from(self.setup.fiscal_year_start_month.clamp(1, 12)) - 1]
                    .to_string(),
            ),
            (
                t("interview.year.calendar"),
                t(match self.setup.fiscal_calendar {
                    FiscalCalendar::Standard => "interview.calendar.standard",
                    FiscalCalendar::Retail445 => "interview.calendar.445",
                    FiscalCalendar::Retail454 => "interview.calendar.454",
                    FiscalCalendar::Retail544 => "interview.calendar.544",
                }),
            ),
            (t("interview.page.chart"), self.setup.chart.template().name),
            (
                t("interview.features.sales_tax"),
                t(if self.setup.sales_tax {
                    "interview.review.yes"
                } else {
                    "interview.review.no"
                }),
            ),
            (
                t("interview.features.inventory"),
                t(if self.setup.inventory {
                    "interview.review.yes"
                } else {
                    "interview.review.no"
                }),
            ),
            (
                t("interview.features.basis"),
                t(match self.setup.basis {
                    AccountingMethod::Accrual => "interview.basis.accrual",
                    AccountingMethod::Cash => "interview.basis.cash",
                }),
            ),
            (
                t("interview.security.closing"),
                t(if self.setup.closing_password.is_some() {
                    "interview.review.set"
                } else {
                    "interview.review.not_set"
                }),
            ),
        ];
        egui::Grid::new("review")
            .num_columns(2)
            .spacing([24.0, 6.0])
            .show(ui, |ui| {
                for (k, v) in rows {
                    ui.label(RichText::new(k).color(tokens.text_secondary));
                    ui.label(v);
                    ui.end_row();
                }
            });
    }

    fn done_panel(&mut self, ui: &mut Ui, tokens: &Tokens, created: &Created) {
        form::card(ui, tokens, |ui| {
            form::heading(ui, tokens, &t("interview.done.title"));
            form::hint(
                ui,
                tokens,
                &t_with(
                    "interview.done.body",
                    &[("n", &created.accounts_imported.to_string())],
                ),
            );
            ui.add_space(8.0);
            // The recovery key is offered only when first-run setup produced
            // one that has not been shown yet (M1); it is never made here.
            let offer_key = self.link.has_recovery_key() || self.link.is_demo();
            let clicked = if offer_key {
                form::actions(
                    ui,
                    &t("interview.done.recovery"),
                    &[&t("interview.done.opening"), &t("interview.done.later")],
                )
            } else {
                form::actions(
                    ui,
                    &t("interview.done.opening"),
                    &[&t("interview.done.later")],
                )
                .map(|i| i + 1)
            };
            if let Some(i) = clicked {
                let next = match i {
                    0 => "company.recovery_key",
                    1 => "company.opening_balances",
                    _ => "nav.home",
                };
                self.link
                    .push(CompanyEvent::OpenWindow(CommandId::new(next)));
                self.link.push(CompanyEvent::CloseWindow(CommandId::new(
                    "file.new_company",
                )));
            }
        });
    }
}

fn parse_rate(text: &str) -> Option<cl_core::Rate> {
    let s = text.trim().trim_end_matches('%').trim();
    if s.is_empty() {
        return None;
    }
    // Percent typed by the owner (8.25) becomes the fraction 0.0825.
    let d = rust_decimal_parse(s)?;
    cl_core::Rate::parse_wire(&d).ok()
}

/// Divides a percent string by 100 exactly, as text (no floats).
fn rust_decimal_parse(s: &str) -> Option<String> {
    let (int, frac) = s.split_once('.').unwrap_or((s, ""));
    if !int.chars().all(|c| c.is_ascii_digit()) || !frac.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let digits = format!("{int:0>3}{frac}");
    let split = digits.len() - frac.len() - 2;
    let (a, b) = digits.split_at(split);
    let a = a.trim_start_matches('0');
    let a = if a.is_empty() { "0" } else { a };
    let b: String = b.chars().take(4).collect();
    Some(format!("{a}.{b:0<4}"))
}

impl Screen for NewCompanyScreen {
    fn title(&self) -> String {
        t("interview.tab")
    }

    fn scope(&self) -> Scope {
        Scope::Form
    }

    fn dirty(&self) -> bool {
        self.dirty && self.created.is_none()
    }

    fn ui(&mut self, ui: &mut Ui) {
        let tokens = Tokens::of(ui.ctx());
        self.poll();
        ui.add_space(8.0);
        ui.heading(t("interview.title"));
        form::hint(ui, &tokens, &t("interview.intro"));
        ui.add_space(8.0);
        if let Some(created) = self.created.clone() {
            self.done_panel(ui, &tokens, &created);
            return;
        }
        ui.horizontal_wrapped(|ui| {
            for (i, p) in Page::ALL.iter().enumerate() {
                let current = *p == self.page;
                let text = format!("{}. {}", i + 1, t(p.key()));
                ui.add(
                    egui::Label::new(if current {
                        RichText::new(text).strong().color(tokens.accent)
                    } else {
                        RichText::new(text).color(tokens.text_tertiary)
                    })
                    .selectable(false),
                );
                if i + 1 < Page::ALL.len() {
                    ui.label(RichText::new("\u{203a}").color(tokens.text_tertiary));
                }
            }
        });
        ui.add_space(8.0);
        // Next and Back stay in view at the minimum window size: once above
        // the page and once below it.
        if self.task.is_none() {
            self.action_row(ui, "top");
            ui.add_space(6.0);
        }
        form::card(ui, &tokens, |ui| {
            form::heading(ui, &tokens, &t(self.page.key()));
            ui.add_space(4.0);
            match self.page {
                Page::Names => self.page_names(ui, &tokens),
                Page::Year => self.page_year(ui, &tokens),
                Page::Chart => self.page_chart(ui, &tokens),
                Page::Features => self.page_features(ui, &tokens),
                Page::Security => self.page_security(ui, &tokens),
                Page::Review => self.page_review(ui, &tokens),
            }
        });
        for issue in &self.issues {
            ui.label(RichText::new(t(issue.key())).color(tokens.warn_text));
        }
        if let Some(e) = &self.error {
            form::inline_error(ui, &tokens, e);
        }
        ui.add_space(10.0);
        if self.task.is_some() {
            form::progress_line(ui, &tokens, &t("interview.creating"));
            return;
        }
        self.action_row(ui, "bottom");
    }

    fn command(&mut self, id: CommandId) -> bool {
        match id.as_str() {
            "form.save_close" | "form.save_new" => {
                if self.task.is_none() && self.created.is_none() {
                    self.next();
                }
                true
            }
            "form.revert" => self.revert_fields(),
            "form.prev_record" => {
                if let Some(p) = self.page.prev() {
                    self.page = p;
                    self.committed = self.snapshot();
                }
                true
            }
            "form.next_record" => {
                self.next();
                true
            }
            _ => false,
        }
    }

    fn save(&mut self) -> bool {
        false
    }

    fn state(&self) -> Value {
        json!({
            "screen": "new_company",
            "page": format!("{:?}", self.page).to_ascii_lowercase(),
            "page_index": self.page.index(),
            "issues": self.issues.len(),
            "created": self.created.is_some(),
            "creating": self.task.is_some(),
            "dirty": self.dirty,
            "chart": self.setup.chart.id(),
            "sales_tax": self.setup.sales_tax,
            "inventory": self.setup.inventory,
            "closing_supported": self.closing_supported,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_text_becomes_an_exact_fraction() {
        assert_eq!(
            parse_rate("8.25").map(cl_core::Rate::to_wire).as_deref(),
            Some("0.0825")
        );
        assert_eq!(
            parse_rate("8.25%").map(cl_core::Rate::to_wire).as_deref(),
            Some("0.0825")
        );
        assert_eq!(
            parse_rate("10").map(cl_core::Rate::to_wire).as_deref(),
            Some("0.1000")
        );
        assert_eq!(
            parse_rate("0.5").map(cl_core::Rate::to_wire).as_deref(),
            Some("0.0050")
        );
        assert_eq!(parse_rate("abc"), None);
        assert_eq!(parse_rate(""), None);
    }

    #[test]
    fn pages_step_in_order() {
        assert_eq!(Page::Names.next(), Some(Page::Year));
        assert_eq!(Page::Review.next(), None);
        assert_eq!(Page::Names.prev(), None);
        assert_eq!(Page::Review.prev(), Some(Page::Security));
    }
}
