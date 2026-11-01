//! Assess Finance Charges (P22 D7, `clients.finance_charges`; Q-174,
//! Q-P22a, Q-P22g, Q-P22p, Q-P22s).
//!
//! - The assessment date (today by default) and the month it charges for.
//! - The engine's preview (`POST /finance-charges/preview`): per client
//!   the overdue balance, the charge and the invoices it covers; clients
//!   already charged this month, skipped clients with the reason, and
//!   clients with open credits (unticked, with the hint) are listed too.
//!   Every figure is the engine's string; the UI computes nothing.
//! - With the rate or the income account blank (finance charges off,
//!   Q-P22a) the setup fields come first; saving them writes the company
//!   settings and previews.
//! - Assess sends the ticked clients with the preview's charge strings as
//!   `expectedCharge` (at most 500 per call, one Idempotency-Key per call
//!   of an attempt, reused through the closing-date prompt and a lost
//!   reply). A 409 `FINANCE_CHARGE_CHANGED` previews again and names the
//!   clients whose charge moved; `FINANCE_CHARGE_ASSESSED` says the month
//!   is charged (void that invoice to charge again, Q-P22p).
//! - The result lists the finance-charge invoices with Open.

mod draw;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use cl_api::{
    ApiError, AssessContact, AssessRequest, AssessResult, AssessedInvoice, CompanySettings,
    FinanceChargePreview, FinanceChargeRefusal, IdempotencyKey, RequestGroup,
    finance_charge_refusal,
};
use cl_core::ids::{AccountId, ContactId, TenantId};
use cl_core::{CalendarDate, Money, MoneyStyle, Secret};
use cl_model::closing::needs_password;
use cl_model::forms::finance_charge::{
    ChargeFigures, ChargeRowFigure, FinanceChargeForm, FinanceChargeProblem,
};
use cl_model::{CommandId, Scope};
use eframe::egui::Ui;
use serde_json::{Value, json};

use crate::company::{CompanyEvent, CompanyLink};
use crate::data_bus::Subscription;
use crate::forms::closing_prompt::{ClosingPrompt, Verb};
use crate::screens::card_charge::kit::Books;
use crate::screens::company::form::DateInput;
use crate::screens::invoice::engine::{self, Job};
use crate::screens::preferences::company::FinanceChargeEdits;
use crate::windows::{OpenPayload, Screen};

/// The command and window kind.
pub const KIND: &str = "clients.finance_charges";

/// The Assess Finance Charges window for `kind`, if it is one.
#[must_use]
pub fn make(
    kind: CommandId,
    link: &Arc<CompanyLink>,
    _open: Option<(TenantId, String)>,
    today: CalendarDate,
) -> Option<Box<dyn Screen>> {
    (kind.as_str() == KIND)
        .then(|| Box::new(FinanceChargesScreen::new(Arc::clone(link), today)) as Box<_>)
}

/// What the closing-date password is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Pending {
    Assess,
}

/// What an assess job answered: the calls that posted, and the error that
/// stopped the rest (calls are all or nothing each).
type AssessOutcome = (Vec<AssessResult>, Option<ApiError>);

/// "October 2026" for `2026-10` (the period as sent when it isn't one).
pub(crate) fn month_label(period: &str) -> String {
    let mut parts = period.split('-');
    let year = parts.next().unwrap_or_default();
    let month = parts
        .next()
        .and_then(|m| m.parse::<usize>().ok())
        .filter(|m| (1..=12).contains(m));
    match month {
        Some(m) => format!(
            "{} {year}",
            crate::screens::preferences::company::MONTHS[m - 1]
        ),
        None => period.to_string(),
    }
}

/// The engine's preview as the form model's figures (byte copies).
pub(crate) fn figures_of(p: &FinanceChargePreview) -> ChargeFigures {
    ChargeFigures {
        period: p.period.clone(),
        rows: p
            .rows
            .iter()
            .map(|r| ChargeRowFigure {
                client: r.contact_id,
                charge: r.charge,
                chargeable: r.chargeable(),
                has_open_credits: r.has_open_credits.is_some(),
                ticked: r.ticked,
            })
            .collect(),
        not_set_up: p.not_set_up(),
    }
}

/// The empty company settings the demo fills.
fn demo_settings(d: &crate::company::demo::DemoState) -> Result<CompanySettings, ApiError> {
    let mut s: CompanySettings = serde_json::from_value(json!({}))
        .map_err(|_| ApiError::Local(cl_api::LocalValidation::InvalidValue))?;
    crate::company::demo::finance_charges::fill_settings(&d.finance, &mut s);
    Ok(s)
}

/// The Assess Finance Charges window.
pub struct FinanceChargesScreen {
    pub(crate) link: Arc<CompanyLink>,
    pub(crate) group: Option<RequestGroup>,
    pub(crate) bus: Subscription,
    pub(crate) today: CalendarDate,
    pub(crate) books: Books,
    pub(crate) form: FinanceChargeForm,
    pub(crate) date: DateInput,
    /// Only this client (Client Center's entry).
    pub(crate) only: Option<ContactId>,
    pub(crate) preview: Option<FinanceChargePreview>,
    pub(crate) preview_job: Option<Job<FinanceChargePreview>>,
    /// What the shown preview (or the one on its way) is for.
    pub(crate) previewed: Option<(CalendarDate, Option<ContactId>, u64)>,
    /// Bumped to preview again (after a save or a refusal).
    pub(crate) generation: u64,
    /// The engine's company settings (the finance-charge fields).
    pub(crate) settings: Option<CompanySettings>,
    pub(crate) settings_job: Option<Job<CompanySettings>>,
    /// The setup fields, open (rate blank, or "Change settings").
    pub(crate) setup: Option<FinanceChargeEdits>,
    pub(crate) setup_job: Option<Job<CompanySettings>>,
    pub(crate) setup_problem: Option<&'static str>,
    /// Clients whose invoices are shown.
    pub(crate) expanded: BTreeSet<ContactId>,
    /// One key per call of an attempt, kept until the call is answered.
    pub(crate) keys: BTreeMap<(u64, usize), IdempotencyKey>,
    pub(crate) assess_job: Option<Job<AssessOutcome>>,
    /// The finance-charge invoices posted in this window.
    pub(crate) results: Vec<AssessedInvoice>,
    /// Client names for the results (from the preview rows).
    pub(crate) names: BTreeMap<ContactId, String>,
    /// The ticked charges a 409 CHANGED refused, until the new preview.
    pub(crate) changed_before: Option<Vec<(ContactId, Money)>>,
    /// The client a 409 CHANGED named (`details.contactId`).
    pub(crate) changed_named: Option<ContactId>,
    /// The clients whose charge moved.
    pub(crate) changed_names: Vec<String>,
    /// A refusal's month (`YYYY-MM`) for its message.
    pub(crate) refused_period: Option<String>,
    pub(crate) closing_prompt: ClosingPrompt<Pending>,
    pub(crate) problems: Vec<FinanceChargeProblem>,
    pub(crate) notice: Option<&'static str>,
    pub(crate) error: Option<ApiError>,
    /// The last batch posted only part of the ticked clients.
    pub(crate) partial: bool,
    /// Alt+S (Save & Close): close the window once everything posted.
    pub(crate) close_after: bool,
    pub(crate) assessed_count: usize,
    pub(crate) money_style: MoneyStyle,
    pub(crate) layout: crate::layout::FrameReport,
}

impl std::fmt::Debug for FinanceChargesScreen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FinanceChargesScreen")
            .field("form", &self.form)
            .field("results", &self.results.len())
            .finish_non_exhaustive()
    }
}

impl FinanceChargesScreen {
    /// Assess as of today.
    #[must_use]
    pub fn new(link: Arc<CompanyLink>, today: CalendarDate) -> FinanceChargesScreen {
        let group = link.group().and_then(Result::ok);
        let bus = link.bus().subscribe();
        let mut s = FinanceChargesScreen {
            link,
            group,
            bus,
            today,
            books: Books::default(),
            form: FinanceChargeForm::new(Some(today)),
            date: DateInput::new(today),
            only: None,
            preview: None,
            preview_job: None,
            previewed: None,
            generation: 0,
            settings: None,
            settings_job: None,
            setup: None,
            setup_job: None,
            setup_problem: None,
            expanded: BTreeSet::new(),
            keys: BTreeMap::new(),
            assess_job: None,
            results: Vec::new(),
            names: BTreeMap::new(),
            changed_before: None,
            changed_named: None,
            changed_names: Vec::new(),
            refused_period: None,
            closing_prompt: ClosingPrompt::default(),
            problems: Vec::new(),
            notice: None,
            error: None,
            partial: false,
            close_after: false,
            assessed_count: 0,
            money_style: MoneyStyle::default(),
            layout: crate::layout::FrameReport::default(),
        };
        s.books.start(&s.link, s.group.as_ref());
        s.read_settings();
        s.poll();
        s
    }

    /// The active income accounts (the setup's picker).
    pub(crate) fn incomes(&self) -> Vec<(AccountId, String)> {
        self.books
            .accounts
            .iter()
            .filter(|a| {
                a.is_active
                    && matches!(
                        a.fields.account_type,
                        cl_api::AccountType::Revenue | cl_api::AccountType::OtherRevenue
                    )
            })
            .map(|a| (a.id, crate::screens::register::account_text(a)))
            .collect()
    }

    /// Finance-charge invoices wait in the print queue (the setting).
    pub(crate) fn print_later(&self) -> bool {
        self.settings
            .as_ref()
            .and_then(|s| s.finance_charge_print_later)
            .unwrap_or(true)
    }

    fn read_settings(&mut self) {
        self.settings_job = Some(engine::read(
            &self.link,
            self.group.as_ref(),
            |d| demo_settings(d),
            |s| async move { s.company_settings().await },
        ));
    }

    // ------------------------------------------------------------ choices

    /// The assessment date (a change previews again).
    pub(crate) fn set_date(&mut self, d: Option<CalendarDate>) {
        self.form.set_date(d);
        self.changed_before = None;
        self.changed_named = None;
        self.changed_names.clear();
        self.problems.clear();
    }

    /// Ticks or unticks a client.
    pub(crate) fn toggle(&mut self, c: ContactId) {
        self.form.toggle(c);
        self.problems.clear();
    }

    /// Shows or hides a client's invoices.
    pub(crate) fn expand(&mut self, c: ContactId) {
        if !self.expanded.remove(&c) {
            self.expanded.insert(c);
        }
    }

    /// The ticked clients.
    pub(crate) fn ticked_count(&self) -> usize {
        self.form.figures().map_or(0, |f| {
            f.rows
                .iter()
                .filter(|r| self.form.is_ticked(r.client))
                .count()
        })
    }

    /// The preview shown is for the date shown.
    pub(crate) fn current(&self) -> Option<&FinanceChargePreview> {
        self.form.figures()?;
        self.preview.as_ref()
    }

    // ------------------------------------------------------------ reads

    /// Takes every answer that arrived; previews again when the date
    /// changed or a write asked for it.
    pub(crate) fn poll(&mut self) {
        self.books.poll();
        if let Some(r) = engine::poll(&mut self.settings_job) {
            match r {
                Ok(s) => self.settings = Some(s),
                Err(e) => self.error = Some(e),
            }
        }
        if let Some(r) = engine::poll(&mut self.preview_job) {
            match r {
                Ok(p) => self.preview_arrived(p),
                // A date in a later month: the engine refuses the preview
                // as it would the assessment (400, `latestPeriod`).
                Err(e) if finance_charge_refusal(&e) == Some(FinanceChargeRefusal::FutureDate) => {
                    self.preview = None;
                    self.notice = Some("finance_charges.refusal.future_date");
                }
                Err(e) => self.error = Some(e),
            }
        }
        let want = self.form.date().map(|d| (d, self.only, self.generation));
        if self.preview_job.is_none()
            && self.assess_job.is_none()
            && self.setup_job.is_none()
            && want.is_some()
            && want != self.previewed
        {
            self.previewed = want;
            self.start_preview();
        }
        self.poll_setup();
        self.poll_assess();
    }

    fn start_preview(&mut self) {
        let Some((date, only, _)) = self.previewed else {
            return;
        };
        self.preview_job = Some(engine::read(
            &self.link,
            self.group.as_ref(),
            move |d| {
                let one: Vec<ContactId> = only.into_iter().collect();
                crate::company::demo::finance_charges::preview(
                    d,
                    date,
                    only.is_some().then_some(one.as_slice()),
                )
            },
            move |s| async move {
                let one: Vec<ContactId> = only.into_iter().collect();
                s.finance_charges()
                    .preview(date, only.is_some().then_some(one.as_slice()))
                    .await
            },
        ));
    }

    fn preview_arrived(&mut self, p: FinanceChargePreview) {
        let Some((date, _, _)) = self.previewed else {
            return;
        };
        if !self.form.apply_preview(date, figures_of(&p)) {
            return;
        }
        if self.notice == Some("finance_charges.refusal.future_date") {
            self.notice = None;
        }
        for r in &p.rows {
            self.names.insert(r.contact_id, r.name.clone());
        }
        // A 409 CHANGED: name the clients whose charge moved (the engine's
        // strings compared as sent, nothing worked out).
        // The engine names the first client it found changed
        // (`details.contactId`); any other ticked charge that moved is
        // named after it.
        if let Some(before) = self.changed_before.take() {
            let named = self.changed_named.take();
            let moved = before.iter().filter(|(c, charge)| {
                Some(*c) != named
                    && !p
                        .rows
                        .iter()
                        .any(|r| r.contact_id == *c && r.chargeable() && r.charge == *charge)
            });
            self.changed_names = named
                .into_iter()
                .chain(moved.map(|(c, _)| *c))
                .filter_map(|c| self.names.get(&c).cloned())
                .collect();
        }
        // Rate or income account blank: the setup first (Q-P22a).
        if p.not_set_up() && self.setup.is_none() {
            self.open_setup();
        }
        self.preview = Some(p);
    }

    // ------------------------------------------------------------ setup

    /// Shows the setup fields with the engine's settings.
    pub(crate) fn open_setup(&mut self) {
        self.setup = Some(FinanceChargeEdits::from_settings(self.settings.as_ref()));
        self.setup_problem = None;
    }

    /// Closes the setup fields (only once finance charges are set up).
    pub(crate) fn cancel_setup(&mut self) {
        if self.current().is_some_and(FinanceChargePreview::not_set_up) {
            return;
        }
        self.setup = None;
        self.setup_problem = None;
    }

    /// Saves the setup fields to the company settings, then previews.
    pub(crate) fn save_setup(&mut self) -> bool {
        let Some(edits) = self.setup.clone() else {
            return false;
        };
        if self.setup_job.is_some() {
            return false;
        }
        self.setup_problem = None;
        if let Err(b) = edits.validate() {
            self.setup_problem = Some(match b {
                crate::screens::preferences::SaveBlock::BadNumber(_) => {
                    "finance_charges.setup.bad_grace"
                }
                _ => "finance_charges.setup.bad_rate",
            });
            return false;
        }
        if edits.is_off() {
            self.setup_problem = Some("finance_charges.setup.needs_rate");
            return false;
        }
        if edits.income_account.is_none() {
            self.setup_problem = Some("finance_charges.setup.needs_account");
            return false;
        }
        let update = match edits.update_for(self.settings.as_ref()) {
            Ok(Some(u)) => u,
            Ok(None) => {
                self.setup = None;
                self.generation += 1;
                return true;
            }
            Err(_) => {
                self.setup_problem = Some("finance_charges.setup.bad_rate");
                return false;
            }
        };
        let demo = update.clone();
        self.setup_job = Some(engine::write(
            &self.link,
            self.group.as_ref(),
            engine::label("orphan.finance_settings", None),
            move |d| {
                crate::company::demo::finance_charges::set_settings(d, &demo);
                demo_settings(d)
            },
            move |s| async move {
                match s
                    .set_finance_charge_settings(&update, IdempotencyKey::new())
                    .await?
                {
                    Some(saved) => Ok(saved),
                    None => s.company_settings().await,
                }
            },
        ));
        self.poll_setup();
        true
    }

    fn poll_setup(&mut self) {
        match engine::poll(&mut self.setup_job) {
            Some(Ok(s)) => {
                self.settings = Some(s);
                self.setup = None;
                self.generation += 1;
                self.link.push(CompanyEvent::Toast("prefs.toast"));
            }
            Some(Err(e)) => self.error = Some(e),
            None => {}
        }
    }

    // ------------------------------------------------------------ assess

    pub(crate) fn busy(&self) -> bool {
        self.assess_job.is_some() || self.setup_job.is_some()
    }

    /// Assesses the ticked clients. False: nothing sent.
    #[allow(clippy::needless_pass_by_value)] // the secret moves into the queued write
    pub(crate) fn start_assess(&mut self, password: Option<Secret>) -> bool {
        if self.busy() || self.preview_job.is_some() {
            return false;
        }
        self.notice = None;
        self.error = None;
        self.partial = false;
        self.changed_names.clear();
        let plan = match self.form.assess_plan() {
            Ok(p) => p,
            Err(p) => {
                if p == FinanceChargeProblem::NotSetUp && self.setup.is_none() {
                    self.open_setup();
                }
                self.problems = vec![p];
                return false;
            }
        };
        self.problems.clear();
        if password.is_none() && needs_password(None, plan.date, self.books.closing) {
            self.closing_prompt.set_verb(Verb::Change);
            self.closing_prompt
                .ask(Pending::Assess, true, self.books.closing);
            return false;
        }
        let calls: Vec<(AssessRequest, IdempotencyKey)> = plan
            .chunks
            .iter()
            .map(|c| {
                let key = self.keys.entry((c.attempt, c.index)).or_default().clone();
                (
                    AssessRequest {
                        assessment_date: plan.date,
                        contacts: c
                            .clients
                            .iter()
                            .map(|(contact_id, expected_charge)| AssessContact {
                                contact_id: *contact_id,
                                expected_charge: *expected_charge,
                            })
                            .collect(),
                    },
                    key,
                )
            })
            .collect();
        let first = calls.first().map(|(_, k)| k.clone());
        let demo_calls = calls.clone();
        let demo_pw = password.clone();
        self.assess_job = Some(engine::write(
            &self.link,
            self.group.as_ref(),
            engine::label("orphan.finance_charges", first),
            move |d| {
                let mut done = Vec::new();
                for (r, k) in &demo_calls {
                    match crate::company::demo::finance_charges::assess(
                        d,
                        r,
                        &k.to_string(),
                        demo_pw.as_ref(),
                    ) {
                        Ok(x) => done.push(x),
                        Err(e) => return Ok((done, Some(e))),
                    }
                }
                Ok((done, None))
            },
            move |s| async move {
                let opts = cl_api::PostingOptions {
                    closed_period: password.map(cl_api::ClosedPeriodAuthorization::with_password),
                };
                let mut done = Vec::new();
                for (r, k) in calls {
                    match s.finance_charges().assess(&r, &opts, k).await {
                        Ok(x) => done.push(x),
                        Err(e) => return Ok((done, Some(e))),
                    }
                }
                Ok((done, None))
            },
        ));
        self.poll_assess();
        true
    }

    fn poll_assess(&mut self) {
        let Some(r) = engine::poll(&mut self.assess_job) else {
            return;
        };
        let (done, err) = match r {
            Ok(x) => x,
            Err(e) => (Vec::new(), Some(e)),
        };
        let posted: Vec<AssessedInvoice> = done.into_iter().flat_map(|r| r.invoices).collect();
        if !posted.is_empty() {
            self.assessed_count += posted.len();
            self.results.extend(posted);
            crate::forms::publish_saved(&self.link, &self.bus, &[]);
            self.link.push(CompanyEvent::Toast("finance_charges.toast"));
        }
        let Some(e) = err else {
            self.closing_prompt.saved();
            self.keys.clear();
            self.notice = Some("finance_charges.done");
            self.generation += 1;
            // Alt+S: Save & Close (DESIGN.md form keys); the toast says
            // what posted. Alt+N stays with the results and a new preview.
            if std::mem::take(&mut self.close_after) {
                self.link
                    .push(CompanyEvent::CloseWindow(CommandId::new(KIND)));
            }
            return;
        };
        self.close_after = false;
        self.partial = !self.results.is_empty();
        if !crate::screens::register::keeps_key(&e) {
            self.keys.clear();
        }
        if self
            .closing_prompt
            .failed(&e, Pending::Assess, true, self.books.closing)
        {
            return;
        }
        self.refused_period = self.form.figures().map(|f| f.period.clone());
        self.notice = match finance_charge_refusal(&e) {
            Some(FinanceChargeRefusal::Changed) => {
                self.changed_named = match &e {
                    ApiError::Conflict { details, .. } => details.id("contactId"),
                    _ => None,
                };
                self.changed_before = self.form.assess_plan().ok().map(|p| {
                    p.chunks
                        .into_iter()
                        .flat_map(|c| c.clients)
                        .collect::<Vec<_>>()
                });
                Some("finance_charges.refusal.changed")
            }
            Some(FinanceChargeRefusal::Assessed) => Some("finance_charges.refusal.assessed"),
            Some(FinanceChargeRefusal::FutureDate) => Some("finance_charges.refusal.future_date"),
            Some(FinanceChargeRefusal::NotSetUp) => {
                self.read_settings();
                self.open_setup();
                Some("finance_charges.refusal.not_set_up")
            }
            None => {
                self.error = Some(e);
                None
            }
        };
        // The engine's figures again.
        self.generation += 1;
    }

    pub(crate) fn submit_closing(&mut self) {
        let Some((Pending::Assess, secret)) = self.closing_prompt.submit() else {
            return;
        };
        let sent = self.start_assess(Some(secret));
        if !sent && self.closing_prompt.dialog_open() {
            self.closing_prompt.nothing_sent();
        }
    }

    /// Opens a posted finance-charge invoice.
    pub(crate) fn open_invoice(&self, inv: &AssessedInvoice) {
        self.link.push(CompanyEvent::OpenRecord(
            CommandId::new(crate::screens::invoice::KIND),
            inv.invoice_id,
        ));
    }

    /// The control channel's view (counts, flags and the engine's figures
    /// of the demo's synthetic data; no names).
    #[must_use]
    pub fn state_json(&self) -> Value {
        let style = MoneyStyle::default();
        let mut v = json!({
            "screen": "finance_charges",
            "date": self.form.date().map(CalendarDate::to_wire),
            "only": self.only.is_some(),
            "loading": self.preview_job.is_some(),
            "setup": self.setup.is_some(),
            "setup_problem": self.setup_problem,
            "busy": self.busy(),
            "ticked": self.ticked_count(),
            "assessed": self.assessed_count,
            "results": self.results.iter().map(|r| json!({
                "number": r.number,
                "total": r.total.format(style),
            })).collect::<Vec<_>>(),
            "print_later": self.print_later(),
            "partial": self.partial,
            "changed": self.changed_names.len(),
            "notice": self.notice,
            "error": self.error.is_some(),
            "problems": self.problems.iter().map(|p| format!("{p:?}")).collect::<Vec<_>>(),
            "closing": self.closing_prompt.state_json(true),
            "layout": self.layout.json(),
        });
        if let Some(p) = self.current() {
            v["period"] = json!(p.period);
            v["not_set_up"] = json!(p.not_set_up());
            v["total"] = json!(p.totals.charge.format(style));
            v["count"] = json!(p.totals.count);
            v["credit_rows_unticked"] = json!(
                p.rows
                    .iter()
                    .filter(|r| r.has_open_credits.is_some() && !self.form.is_ticked(r.contact_id))
                    .count()
            );
            v["rows"] = json!(
                p.rows
                    .iter()
                    .map(|r| json!({
                        "overdue": r.overdue_balance.format(style),
                        "charge": r.charge.format(style),
                        "chargeable": r.chargeable(),
                        "ticked": self.form.is_ticked(r.contact_id),
                        "already": r.already_assessed.is_some(),
                        "skipped": r.skipped_reason.map(|s| format!("{s:?}").to_lowercase()),
                        "open_credits": r.has_open_credits.map(|m| m.format(style)),
                        "sources": r.sources.iter().map(|s| json!({
                            "days": s.days,
                            "amount": s.amount.format(style),
                        })).collect::<Vec<_>>(),
                    }))
                    .collect::<Vec<_>>()
            );
        }
        v
    }
}

impl Screen for FinanceChargesScreen {
    fn title(&self) -> String {
        crate::i18n::t(&format!("cmd.{KIND}"))
    }

    fn ui(&mut self, ui: &mut Ui) {
        self.money_style = crate::grid_env::shell_frame(ui.ctx()).money_style;
        self.poll();
        self.draw(ui);
        if self.busy()
            || self.preview_job.is_some()
            || self.settings_job.is_some()
            || !self.books.loaded
        {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(60));
        }
    }

    fn dirty(&self) -> bool {
        false
    }

    fn scope(&self) -> Scope {
        Scope::Form
    }

    fn command(&mut self, id: CommandId) -> bool {
        if let Some(h) = self.closing_prompt.command(id) {
            if h && id.as_str() == "form.save_new" {
                self.submit_closing();
            }
            return h;
        }
        match id.as_str() {
            KIND => {}
            // Alt+N assesses and stays (Save & New: the results and a new
            // preview); Alt+S assesses and closes once posted (Save &
            // Close). With the setup fields shown, both save the setup.
            "form.save_close" | "form.save_new" => {
                if self.setup.is_some() {
                    self.save_setup();
                } else {
                    self.close_after = id.as_str() == "form.save_close";
                    if !self.start_assess(None) && !self.closing_prompt.dialog_open() {
                        self.close_after = false;
                    }
                }
            }
            "form.revert" => {
                if self.setup.is_some()
                    && !self.current().is_some_and(FinanceChargePreview::not_set_up)
                {
                    self.cancel_setup();
                } else {
                    self.link
                        .push(CompanyEvent::CloseWindow(CommandId::new(KIND)));
                }
            }
            _ => return false,
        }
        true
    }

    fn open_with(&mut self, payload: OpenPayload) -> bool {
        match payload {
            OpenPayload::ForClient(c) => {
                self.only = Some(c);
                true
            }
            OpenPayload::OnDate(d) => {
                self.only = None;
                self.date = DateInput::new(d);
                self.set_date(Some(d));
                true
            }
            _ => false,
        }
    }

    fn state(&self) -> Value {
        self.state_json()
    }

    fn fixed_layout(&self) -> bool {
        true
    }
}
