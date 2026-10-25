//! Templates Due (P15 C2, `lists.templates_due`): what the catch-up runner
//! found. No menu entry: the runner opens it after a run with something
//! new, and the status bar's "templates need you" chip opens it.
//!
//! - **Entered for you**: date · template · amount · account, and Open
//!   (the posted transaction in its form).
//! - **Waiting for you** (reminders, one row per due date): Enter now
//!   (posts that date; a closed period asks for the closing-date
//!   password), Open in form (the form filled for that date; after it
//!   saves, the date is marked entered, Q-P15p) and Skip this one.
//! - **Couldn't be entered** (skipped dates, with the plain reason): Enter
//!   it now opens the form filled for that date, so a closed period asks
//!   for the password through the form's own save; Dismiss.
//! - **Failed**: "we'll try again" and Copy details (Q-119; ids and the
//!   engine's code only).
//!
//! The amounts are the engine's figures, display only: never summed. The
//! keyboard: arrows move, Enter runs the row's default action, Alt+O (or
//! Ctrl+E) opens it.

use std::collections::BTreeMap;
use std::sync::Arc;

use cl_api::{
    Account, AccountFilter, ApiError, ClosedPeriodAuthorization, Entered, IdempotencyKey,
    PostingOptions, RequestGroup, RunDue, SkipReason, Template, TemplateKind, TemplateUse,
};
use cl_core::ids::{AccountId, TemplateId, TenantId, TransactionId};
use cl_core::{CalendarDate, Money, Secret};
use cl_model::{CommandId, Scope};
use eframe::egui::{self, RichText, Ui};
use serde_json::{Value, json};

use crate::company::demo::{template_store, templates as demo};
use crate::company::{CompanyEvent, CompanyLink};
use crate::forms::closing_prompt::{ClosingClick, ClosingPrompt};
use crate::forms::lost_save::LostScope;
use crate::forms::template::TemplateOpen;
use crate::i18n::{t, t_with};
use crate::layout::{Body, FixedFrame, FrameOpts};
use crate::screens::company::form;
use crate::screens::templates::engine::{Job, finished, run};
use crate::screens::templates::form_kind;
use crate::templates_runner::runs_on;
use crate::theme::Tokens;
use crate::windows::Screen;

/// The window kind.
pub const KIND: &str = "lists.templates_due";

/// The Templates Due window for `kind`, if it is one.
#[must_use]
pub fn make(
    kind: CommandId,
    link: &Arc<CompanyLink>,
    open: Option<(TenantId, String)>,
    today: CalendarDate,
) -> Option<Box<dyn Screen>> {
    (kind.as_str() == KIND)
        .then(|| Box::new(TemplatesDueScreen::new(Arc::clone(link), open, today)) as Box<_>)
}

/// Which list a row is in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Section {
    /// Posted by the run.
    Entered(TransactionId),
    /// A due reminder date.
    Waiting,
    /// A date that couldn't be entered.
    Skipped(SkipReason),
    /// A template the run could not check.
    Failed(String),
    /// A form saved this date and its link to the date is still being
    /// made (S2): already entered, nothing to do.
    Linking,
    /// A form saved this date but the engine refused its link for good
    /// (or every try this session failed): the transaction stands on its
    /// own. Dismiss only, never Enter it now (review fix-PR-1 should-fix 5).
    SavedSeparately,
}

impl Section {
    fn word(&self) -> &'static str {
        match self {
            Section::Entered(_) | Section::Linking | Section::SavedSeparately => "entered",
            Section::Waiting => "waiting",
            Section::Skipped(_) => "skipped",
            Section::Failed(_) => "failed",
        }
    }
}

/// One row (names are owner data: never logged or put in the state).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Row {
    pub section: Section,
    pub template: TemplateId,
    pub name: String,
    pub date: Option<CalendarDate>,
    pub amount: Option<Money>,
    pub kind: TemplateKind,
    pub client_form: Option<String>,
}

impl Row {
    /// A capped catch-up (`CATCH_UP_CAPPED`): no Skip or Dismiss.
    pub(crate) fn capped(&self) -> bool {
        matches!(&self.section, Section::Skipped(why) if why.is_capped())
    }
}

/// One occurrence: what the closing-date password is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Occurrence(TemplateId, CalendarDate);

/// The rows of a run, in the window's order.
pub(crate) fn rows_of(run: &RunDue, info: &BTreeMap<TemplateId, Template>) -> Vec<Row> {
    let name_of = |id: TemplateId, given: &str| {
        if given.is_empty() {
            info.get(&id).map(|t| t.name.clone()).unwrap_or_default()
        } else {
            given.to_string()
        }
    };
    let total_of = |id: TemplateId| info.get(&id).and_then(|t| t.total);
    let mut out = Vec::new();
    for p in &run.posted {
        out.push(Row {
            section: Section::Entered(p.transaction),
            template: p.template,
            name: name_of(p.template, &p.name),
            date: Some(p.date),
            amount: p.total,
            kind: p.kind.clone(),
            client_form: p.client_form.clone(),
        });
    }
    for r in &run.reminders {
        for d in &r.dates {
            out.push(Row {
                section: Section::Waiting,
                template: r.template,
                name: name_of(r.template, &r.name),
                date: Some(*d),
                amount: r.total.or_else(|| total_of(r.template)),
                kind: r.kind.clone(),
                client_form: r.client_form.clone(),
            });
        }
    }
    for k in &run.skipped {
        let t = info.get(&k.template);
        out.push(Row {
            section: Section::Skipped(k.reason.clone()),
            template: k.template,
            name: name_of(k.template, &k.name),
            date: Some(k.date),
            amount: total_of(k.template),
            kind: t.map_or(TemplateKind::Legacy, |t| t.kind.clone()),
            client_form: t.and_then(|t| t.client_form.clone()),
        });
    }
    for f in &run.failed {
        let t = info.get(&f.template);
        out.push(Row {
            section: Section::Failed(f.reason.clone()),
            template: f.template,
            name: name_of(f.template, ""),
            date: None,
            amount: None,
            kind: t.map_or(TemplateKind::Legacy, |t| t.kind.clone()),
            client_form: t.and_then(|t| t.client_form.clone()),
        });
    }
    out
}

/// What the templates list and the chart give the rows.
type Loaded = (Vec<Template>, Vec<Account>);

/// The Templates Due window.
pub struct TemplatesDueScreen {
    link: Arc<CompanyLink>,
    tenant: Option<(TenantId, String)>,
    group: Option<RequestGroup>,
    today: CalendarDate,
    /// The board generation the rows were built from.
    generation: u64,
    pub(crate) as_of: Option<CalendarDate>,
    pub(crate) rows: Vec<Row>,
    pub(crate) selected: Option<usize>,
    info: BTreeMap<TemplateId, Template>,
    accounts: Vec<Account>,
    load: Option<Job<Loaded>>,
    enter_job: Option<(Occurrence, Job<Entered>)>,
    skip_job: Option<(Occurrence, Job<()>)>,
    open_job: Option<Job<TemplateOpen>>,
    pub(crate) closing_prompt: ClosingPrompt<Occurrence>,
    closing: Option<CalendarDate>,
    /// The last entry ("Entered. Open" link).
    pub(crate) entered: Option<(TransactionId, &'static str)>,
    pub(crate) notice: Option<&'static str>,
    pub(crate) error: Option<ApiError>,
    pub(crate) last_action: Option<&'static str>,
    pub(crate) checking: bool,
    /// Automatic templates whose catch-up asks first (Q-PR1d).
    pub(crate) questions: Vec<crate::templates_runner::CatchUpQuestion>,
    /// "Skip these" on its way: the template and its skips.
    catch_up_job: Option<(TemplateId, Job<()>)>,
    /// An Enter now whose outcome is unknown (B2).
    pub(crate) lost: crate::forms::lost_save::LostSave,
    lost_for: Option<Occurrence>,
    lost_gate: cl_model::forms::FormState<()>,
    /// The fixed bars' sizes last frame (Q-265).
    layout: crate::layout::FrameReport,
}

impl TemplatesDueScreen {
    /// The window, filled from the company's last catch-up answer.
    #[must_use]
    pub fn new(
        link: Arc<CompanyLink>,
        tenant: Option<(TenantId, String)>,
        today: CalendarDate,
    ) -> TemplatesDueScreen {
        let group = link.group().and_then(Result::ok);
        let closing = link.demo_state().and_then(|d| {
            let open = d.open?;
            d.companies
                .iter()
                .find(|c| c.tenant_id == open)
                .and_then(|c| c.closed_through)
        });
        let mut s = TemplatesDueScreen {
            link,
            tenant,
            group,
            today,
            generation: u64::MAX,
            as_of: None,
            rows: Vec::new(),
            selected: None,
            info: BTreeMap::new(),
            accounts: Vec::new(),
            load: None,
            enter_job: None,
            skip_job: None,
            open_job: None,
            closing_prompt: ClosingPrompt::default(),
            closing,
            entered: None,
            notice: None,
            error: None,
            last_action: None,
            checking: false,
            questions: Vec::new(),
            catch_up_job: None,
            lost: crate::forms::lost_save::LostSave::for_templates(),
            lost_for: None,
            lost_gate: cl_model::forms::FormState::new(()),
            layout: crate::layout::FrameReport::default(),
        };
        // An "Enter now" still unknown from earlier this session shows its
        // banner again (review fix-PR-1 blocking 2).
        let open = s.link.lost_saves().open_templates();
        if let Some(id) = open.first() {
            s.adopt_lost(*id);
        }
        s.load_info();
        s.poll();
        s
    }

    /// The templates (names, amounts, accounts) and the chart.
    fn load_info(&mut self) {
        if !runs_on(&self.link) {
            return;
        }
        self.load = Some(run(
            &self.link,
            self.group.as_ref(),
            None,
            |s| {
                let chart = s
                    .chart
                    .get_or_insert_with(crate::company::demo::chart)
                    .clone();
                Ok((template_store::list(s), chart))
            },
            |s| async move {
                let rows = s.templates().list_all().await?;
                let accounts = s
                    .accounts()
                    .list_all(&AccountFilter::default())
                    .await
                    .unwrap_or_default();
                Ok((rows, accounts))
            },
        ));
    }

    /// Rebuilds the rows when the board has a new answer.
    fn refresh(&mut self) {
        let (generation, run, questions) = {
            let board = self.link.due();
            (
                board.generation(),
                board.run().cloned(),
                board.questions().to_vec(),
            )
        };
        if generation == self.generation {
            return;
        }
        self.questions = questions;
        self.generation = generation;
        self.checking = false;
        let selected = self.selected_row().map(|r| (r.template, r.date));
        self.as_of = run.as_ref().map(|r| r.as_of);
        self.rows = run.map(|r| rows_of(&r, &self.info)).unwrap_or_default();
        {
            // A saved form's date whose link is still being made: already
            // entered (S2), never "Enter it now".
            let board = self.link.due();
            for r in &mut self.rows {
                if !matches!(r.section, Section::Waiting | Section::Skipped(_)) {
                    continue;
                }
                if r.date.is_some_and(|d| board.is_linking(r.template, d)) {
                    r.section = Section::Linking;
                } else if r
                    .date
                    .is_some_and(|d| board.is_saved_separately(r.template, d))
                {
                    r.section = Section::SavedSeparately;
                }
            }
        }
        self.selected = selected
            .and_then(|k| self.rows.iter().position(|r| (r.template, r.date) == k))
            .or_else(|| (!self.rows.is_empty()).then_some(0))
            .map(|i| i.min(self.rows.len().saturating_sub(1)));
    }

    fn selected_row(&self) -> Option<&Row> {
        self.selected.and_then(|i| self.rows.get(i))
    }

    fn account_text(&self, template: TemplateId) -> String {
        let id: Option<AccountId> = self.info.get(&template).and_then(|t| t.account);
        id.and_then(|a| self.accounts.iter().find(|x| x.id == a))
            .map(crate::screens::register::account_text)
            .unwrap_or_default()
    }

    fn busy(&self) -> bool {
        self.enter_job.is_some()
            || self.skip_job.is_some()
            || self.open_job.is_some()
            || self.catch_up_job.is_some()
    }

    pub(crate) fn poll(&mut self) {
        if let Some(r) = finished(&mut self.load) {
            if let Ok((rows, accounts)) = r {
                self.info = rows.into_iter().map(|t| (t.id, t)).collect();
                self.accounts = accounts;
            }
            // The names and amounts may fill in now.
            self.generation = u64::MAX;
        }
        self.refresh();
        if let Some((occ, job)) = self.enter_job.as_mut()
            && let Some(r) = job.take()
        {
            let occ = *occ;
            self.enter_job = None;
            match r {
                Ok(e) => {
                    // Another template's unknown Enter now stays unknown.
                    if self.lost_for.is_some_and(|o| o.0 == occ.0) {
                        self.lost.saved();
                        self.lost_for = None;
                    }
                    self.entered_ok(occ, Some(e.transaction));
                }
                // Could not tell: this template waits for the recheck (B2),
                // here, in Templates and after this window closes (the
                // link's book, review fix-PR-1 blocking 2).
                Err(e)
                    if self
                        .lost
                        .lost_as(&e, &self.link, LostScope::Template(occ.0)) =>
                {
                    self.lost_gate.mark_unconfirmed();
                    self.lost_for = Some(occ);
                }
                // A retry of a lost reply: the date is already entered.
                Err(e) if cl_api::is_occurrence_taken(&e) => self.entered_ok(occ, None),
                Err(e) => {
                    let supported = self.link.is_demo()
                        || self
                            .link
                            .capabilities()
                            .has(cl_api::Capabilities::CLOSING_PASSWORD);
                    if !self.closing_prompt.failed(&e, occ, supported, self.closing) {
                        self.error = Some(e);
                    }
                }
            }
        }
        if let Some((occ, job)) = self.skip_job.as_mut()
            && let Some(r) = job.take()
        {
            let Occurrence(id, date) = *occ;
            self.skip_job = None;
            match r {
                Ok(()) => {
                    self.notice = Some("templates_due.done.skipped");
                    self.last_action = Some("skip");
                    let mut board = self.link.due();
                    board.link_dismissed(id, date);
                    board.handled(id, date);
                }
                Err(e) if cl_api::is_occurrence_taken(&e) => {
                    self.notice = Some("templates_due.done.already");
                    let mut board = self.link.due();
                    board.link_dismissed(id, date);
                    board.handled(id, date);
                }
                Err(e) => self.error = Some(e),
            }
        }
        if let Some((id, job)) = self.catch_up_job.as_mut()
            && let Some(r) = job.take()
        {
            let id = *id;
            self.catch_up_job = None;
            self.link.due().skip_ended(id);
            match r {
                Ok(()) => {
                    self.notice = Some("templates_due.catch_up.skipped");
                    self.last_action = Some("catch_up_skip");
                    self.link.due().skipped_all(id);
                }
                Err(e) => self.error = Some(e),
            }
        }
        if let Some(ev) = self.lost.poll(&mut self.lost_gate) {
            self.lost_event(ev);
        }
        if let Some(r) = finished(&mut self.open_job) {
            match r {
                Ok(open) => {
                    self.last_action = Some("open_in_form");
                    self.link.push(CompanyEvent::OpenTemplate(Box::new(open)));
                }
                Err(e) => self.error = Some(e),
            }
        }
        self.refresh();
    }

    fn entered_ok(&mut self, Occurrence(id, date): Occurrence, txn: Option<TransactionId>) {
        self.closing_prompt.saved();
        // Any register may show it (Q-262).
        self.link
            .bus()
            .publish(crate::data_bus::Change::Ledger(None));
        let kind = self
            .rows
            .iter()
            .find(|r| r.template == id)
            .and_then(|r| form_kind(r.client_form.as_deref(), &r.kind));
        self.entered = txn.zip(kind);
        self.notice = Some(if txn.is_some() {
            "templates_due.done.entered"
        } else {
            "templates_due.done.already"
        });
        self.last_action = Some("enter");
        self.link.due().handled(id, date);
    }

    // ------------------------------------------------------------ actions

    /// Enter now (a reminder date), with the closing-date password after a
    /// refusal.
    pub(crate) fn enter_now(&mut self, password: Option<Secret>) {
        // One call at a time: a second Enter (keyboard) while one is on its
        // way would drop the first answer.
        if self.busy() {
            return;
        }
        let Some(row) = self.selected_row().cloned() else {
            return;
        };
        let Some(date) = row.date else { return };
        if matches!(row.section, Section::Linking | Section::SavedSeparately) {
            return;
        }
        // A lost Enter now of this date: nothing is sent until the recheck
        // or the owner answers (B2).
        if self.lost_gate.is_unconfirmed() && self.lost_for == Some(Occurrence(row.template, date))
        {
            return;
        }
        // One lost before (here, in Templates, or in a window closed since):
        // the same block, per template (review fix-PR-1 blocking 2).
        if self
            .link
            .lost_saves()
            .is_open(LostScope::Template(row.template))
        {
            self.adopt_lost(row.template);
            return;
        }
        if row.section != Section::Waiting || !enters_directly(&row.kind) {
            self.open_in_form();
            return;
        }
        self.error = None;
        self.notice = None;
        let (id, today) = (row.template, self.today);
        let pw = password.clone();
        let key = IdempotencyKey::new();
        let job = run(
            &self.link,
            self.group.as_ref(),
            Some("orphan.template_enter"),
            move |s| template_store::enter(s, id, Some(date), today, pw.as_ref()).map(|(e, _)| e),
            move |s| async move {
                let opts = PostingOptions {
                    closed_period: password.map(ClosedPeriodAuthorization::with_password),
                };
                s.templates().enter(id, Some(date), &opts, key).await
            },
        );
        self.enter_job = Some((Occurrence(id, date), job));
        self.poll();
    }

    /// Open in form / Enter it now: the template's form, filled for the
    /// row's date; after it saves, the date is marked entered.
    pub(crate) fn open_in_form(&mut self) {
        let Some(row) = self.selected_row().cloned() else {
            return;
        };
        let Some(date) = row.date else { return };
        let Some(kind) = form_kind(row.client_form.as_deref(), &row.kind) else {
            self.notice = Some("templates.online_read_only");
            return;
        };
        // Its Enter now may already be in the books: the form could enter
        // it a second time.
        if self
            .link
            .lost_saves()
            .is_open(LostScope::Template(row.template))
        {
            self.adopt_lost(row.template);
            return;
        }
        self.error = None;
        let id = row.template;
        let version = self.info.get(&id).map_or(0, |t| t.version);
        let cmd = CommandId::new(kind);
        let open = move |u: TemplateUse| TemplateOpen {
            kind: cmd,
            template: id,
            version,
            occurrence: Some(date),
            draft: u.draft,
            warnings: u.warnings,
        };
        self.open_job = Some(run(
            &self.link,
            self.group.as_ref(),
            None,
            move |s| template_store::draft(s, id, date).map(open),
            move |s| async move { s.templates().draft(id, date).await.map(open) },
        ));
        self.poll();
    }

    /// Shows the banner of the still unknown "Enter now" of `template`
    /// (made elsewhere or in a window closed before an answer; its date is
    /// not known here).
    fn adopt_lost(&mut self, template: TemplateId) {
        if self.lost.pending() {
            if self.lost_for.is_none_or(|o| o.0 != template) {
                self.notice = Some("templates.enter.lost_other");
            }
            return;
        }
        self.lost.bind(
            &self.link,
            LostScope::Template(template),
            &mut self.lost_gate,
        );
        self.lost_for = None;
    }

    /// The recheck or the owner answered a lost Enter now (B2).
    fn lost_event(&mut self, ev: crate::forms::lost_save::LostEvent) {
        use crate::forms::lost_save::LostEvent;
        match ev {
            LostEvent::Saved(txn) => {
                if let Some(occ) = self.lost_for.take() {
                    self.entered_ok(occ, txn);
                } else {
                    // Picked up from elsewhere: the next run shows the date
                    // as entered.
                    self.notice = Some("templates_due.done.entered");
                    self.check_now();
                }
            }
            LostEvent::NotSaved | LostEvent::OwnerConfirmed => self.lost_for = None,
            LostEvent::StillUnknown => {}
        }
    }

    /// "Post all" on a catch-up question: the next run posts its dates.
    pub(crate) fn catch_up_post_all(&mut self, template: TemplateId) {
        self.link.due().confirm(template);
        self.notice = Some("templates_due.catch_up.posting");
        self.last_action = Some("catch_up_post");
        self.checking = true;
    }

    /// "Skip these": every missed date goes through the skip path, oldest
    /// first (each is the schedule's next date then), so none is lost
    /// silently; then the next run goes on.
    pub(crate) fn catch_up_skip(&mut self, template: TemplateId) {
        if self.busy() {
            return;
        }
        // A run on its way could post these dates while they are being
        // skipped: wait for it (and no run starts while they are).
        if self.link.due().running() {
            self.notice = Some("templates_due.catch_up.wait");
            return;
        }
        let Some(q) = self.questions.iter().find(|q| q.template == template) else {
            return;
        };
        let dates = q.dates.clone();
        let demo_dates = dates.clone();
        self.error = None;
        let job = run(
            &self.link,
            self.group.as_ref(),
            Some("orphan.template_skip"),
            move |s| {
                for d in demo_dates {
                    demo::skip(s, template, d)?;
                }
                Ok(())
            },
            move |s| async move {
                for d in dates {
                    match s.templates().skip(template, d).await {
                        // Already handled (another window): go on.
                        Err(e) if cl_api::is_occurrence_taken(&e) => {}
                        r => r?,
                    }
                }
                Ok(())
            },
        );
        self.link.due().skip_started(template);
        self.catch_up_job = Some((template, job));
        self.poll();
    }

    /// Skip this one / Dismiss.
    pub(crate) fn skip_selected(&mut self) {
        if self.busy() {
            return;
        }
        let Some(row) = self.selected_row().cloned() else {
            return;
        };
        let Some(date) = row.date else { return };
        if !matches!(
            row.section,
            Section::Waiting | Section::Skipped(_) | Section::SavedSeparately
        ) || row.capped()
        {
            // A capped catch-up is not a stored skip: the next run enters it.
            return;
        }
        self.error = None;
        let id = row.template;
        let job = run(
            &self.link,
            self.group.as_ref(),
            Some("orphan.template_skip"),
            move |s| demo::skip(s, id, date),
            move |s| async move { s.templates().skip(id, date).await },
        );
        self.skip_job = Some((Occurrence(id, date), job));
        self.poll();
    }

    /// Open: the posted transaction in its form.
    pub(crate) fn open_posted(&mut self) {
        let Some(row) = self.selected_row().cloned() else {
            return;
        };
        let Section::Entered(txn) = row.section else {
            return;
        };
        match form_kind(row.client_form.as_deref(), &row.kind) {
            Some(kind) => {
                self.last_action = Some("open");
                self.link
                    .push(CompanyEvent::OpenRecord(CommandId::new(kind), txn));
            }
            None => self.notice = Some("templates.online_read_only"),
        }
    }

    /// Copy details of a failed row (ids and the engine's code only).
    fn copy_details(&mut self, ctx: &egui::Context) {
        let Some(row) = self.selected_row().cloned() else {
            return;
        };
        let Section::Failed(code) = &row.section else {
            return;
        };
        let detail = format!(
            "templates catch-up: template {} \u{b7} {}",
            row.template,
            crate::errors::scrub_line(code)
        );
        let text = crate::errors::details_text(
            crate::errors::fresh_id(),
            "templates_due.failed",
            &detail,
            &[],
        );
        ctx.copy_text(text);
        self.notice = Some("templates_due.copied");
    }

    /// The row's default action (Enter).
    pub(crate) fn default_action(&mut self, ctx: Option<&egui::Context>) {
        match self.selected_row().map(|r| r.section.clone()) {
            Some(Section::Entered(_)) => self.open_posted(),
            Some(Section::Waiting) => self.enter_now(None),
            Some(Section::Skipped(_)) => self.open_in_form(),
            Some(Section::Failed(_)) => {
                if let Some(ctx) = ctx {
                    self.copy_details(ctx);
                }
            }
            Some(Section::SavedSeparately) => self.skip_selected(),
            Some(Section::Linking) | None => {}
        }
    }

    /// Alt+O: open the row (the posted transaction, or the form).
    pub(crate) fn open_action(&mut self) {
        match self.selected_row().map(|r| r.section.clone()) {
            Some(Section::Entered(_)) => self.open_posted(),
            Some(Section::Waiting | Section::Skipped(_)) => self.open_in_form(),
            _ => {}
        }
    }

    /// Check now.
    pub(crate) fn check_now(&mut self) {
        self.checking = true;
        self.notice = None;
        self.link.due().request_check();
    }

    fn submit_closing(&mut self) {
        if let Some((Occurrence(id, date), secret)) = self.closing_prompt.submit() {
            self.selected = self
                .rows
                .iter()
                .position(|r| r.template == id && r.date == Some(date));
            self.enter_now(Some(secret));
        }
    }

    fn step(&mut self, down: bool) {
        if self.rows.is_empty() {
            return;
        }
        let last = self.rows.len() - 1;
        self.selected = Some(match (self.selected, down) {
            (None, _) => 0,
            (Some(i), true) => (i + 1).min(last),
            (Some(i), false) => i.saturating_sub(1),
        });
    }

    // ------------------------------------------------------------ drawing

    fn draw(&mut self, ui: &mut Ui) {
        let tokens = Tokens::of(ui.ctx());
        // The title, Check now and the messages stay put; the lists scroll
        // under them (Q-265).
        let mut frame = FixedFrame::begin(ui, KIND, FrameOpts::default());
        let on = runs_on(&self.link);
        frame.top(|ui| {
            crate::forms::header::title_only(ui, &tokens, &t("cmd.lists.templates_due"));
            ui.add_space(6.0);
            if on {
                self.top_ui(ui, &tokens);
            }
        });
        let click = frame.body(Body::Scroll, |ui| {
            if !on {
                form::hint(ui, &tokens, &t("templates_due.unavailable"));
                return None;
            }
            if self.rows.is_empty() {
                ui.add_space(12.0);
                form::hint(ui, &tokens, &t("templates_due.nothing"));
                return None;
            }
            self.sections(ui, &tokens)
        });
        self.layout = frame.end();
        if let Some((i, action)) = click {
            self.selected = Some(i);
            match action {
                "open" => self.open_posted(),
                "enter_now" => self.enter_now(None),
                "open_in_form" | "enter_it_now" => self.open_in_form(),
                "skip" => self.skip_selected(),
                "copy" => self.copy_details(ui.ctx()),
                _ => {}
            }
        }
    }

    /// The as-of line with Check now, the closing-date prompt and the
    /// messages.
    fn top_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let tokens = *tokens;
        ui.horizontal(|ui| {
            if let Some(d) = self.as_of {
                form::hint(
                    ui,
                    &tokens,
                    &t_with(
                        "templates_due.intro",
                        &[("date", &crate::shell::us_date(d))],
                    ),
                );
            }
            if ui
                .add_enabled(
                    !self.checking && !self.busy(),
                    egui::Button::new(t("templates_due.check_now")),
                )
                .on_hover_text(t("templates_due.check_now.tip"))
                .clicked()
            {
                self.check_now();
            }
            if self.checking {
                form::hint(ui, &tokens, &t("templates_due.checking"));
            }
        });
        match self.closing_prompt.ui(ui, &tokens) {
            ClosingClick::Submit => self.submit_closing(),
            ClosingClick::Cancel => self.closing_prompt.cancel(),
            ClosingClick::None => {}
        }
        self.messages(ui, &tokens);
    }

    fn messages(&mut self, ui: &mut Ui, tokens: &Tokens) {
        if let Some(k) = self.notice {
            ui.horizontal(|ui| {
                form::hint(ui, tokens, &t(k));
                if let Some((txn, kind)) = self.entered
                    && ui.link(t("templates.entered.open")).clicked()
                {
                    self.link
                        .push(CompanyEvent::OpenRecord(CommandId::new(kind), txn));
                }
            });
        }
        if let Some(e) = &self.error {
            form::inline_error(ui, tokens, e);
        }
        if let Some(ev) = self.lost.ui(
            ui,
            &mut self.lost_gate,
            &self.link,
            self.group.as_ref(),
            "templates_due_enter",
        ) {
            self.lost_event(ev);
        }
        self.questions_ui(ui, tokens);
    }

    /// The catch-up questions (Q-PR1d): each template's missed dates with
    /// Post all and Skip these.
    fn questions_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        if self.questions.is_empty() {
            return;
        }
        let idle = !self.busy();
        let mut answer = None;
        form::card(ui, tokens, |ui| {
            ui.label(
                RichText::new(t("templates_due.catch_up.title"))
                    .strong()
                    .color(tokens.text_primary),
            );
            ui.add(egui::Label::new(t("templates_due.catch_up.body")).wrap());
            for q in &self.questions {
                let name = self
                    .info
                    .get(&q.template)
                    .map(|t| t.name.clone())
                    .unwrap_or_default();
                let dates: Vec<String> =
                    q.dates.iter().map(|d| crate::shell::us_date(*d)).collect();
                ui.add_space(4.0);
                ui.label(RichText::new(t_with(
                    "templates_due.catch_up.row",
                    &[("name", &name), ("n", &q.dates.len().to_string())],
                )));
                ui.add(
                    egui::Label::new(RichText::new(dates.join(", ")).color(tokens.text_secondary))
                        .wrap(),
                );
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            idle,
                            egui::Button::new(t("templates_due.catch_up.post_all")),
                        )
                        .clicked()
                    {
                        answer = Some((q.template, true));
                    }
                    if ui
                        .add_enabled(
                            idle,
                            egui::Button::new(t("templates_due.catch_up.skip_all")),
                        )
                        .clicked()
                    {
                        answer = Some((q.template, false));
                    }
                });
            }
        });
        match answer {
            Some((id, true)) => self.catch_up_post_all(id),
            Some((id, false)) => self.catch_up_skip(id),
            None => {}
        }
    }

    /// The four lists; the clicked row and action, if any.
    fn sections(&self, ui: &mut Ui, tokens: &Tokens) -> Option<(usize, &'static str)> {
        let style = crate::grid_env::shell_frame(ui.ctx()).money_style;
        let idle = !self.busy();
        let mut click = None;
        let heading = |ui: &mut Ui, key: &str| {
            ui.add_space(10.0);
            ui.label(RichText::new(t(key)).strong().size(15.0));
        };
        let groups = [
            (
                "templates_due.entered",
                "templates_due.entered.none",
                "entered",
            ),
            (
                "templates_due.waiting",
                "templates_due.waiting.none",
                "waiting",
            ),
            ("templates_due.skipped", "", "skipped"),
            ("templates_due.failed", "", "failed"),
        ];
        for (title, none, word) in groups {
            let rows: Vec<usize> = (0..self.rows.len())
                .filter(|i| self.rows[*i].section.word() == word)
                .collect();
            if rows.is_empty() && none.is_empty() {
                continue;
            }
            heading(ui, title);
            if rows.is_empty() {
                form::hint(ui, tokens, &t(none));
                continue;
            }
            egui::Grid::new(("templates_due/grid", title))
                .num_columns(5)
                .striped(true)
                .spacing([16.0, 6.0])
                .show(ui, |ui| {
                    for i in rows {
                        let r = &self.rows[i];
                        let sel = self.selected == Some(i);
                        ui.label(r.date.map(crate::shell::us_date).unwrap_or_default());
                        if ui.selectable_label(sel, r.name.as_str()).clicked() {
                            click = Some((i, ""));
                        }
                        ui.label(
                            RichText::new(r.amount.map(|m| m.format(style)).unwrap_or_default())
                                .text_style(egui::TextStyle::Monospace),
                        );
                        match &r.section {
                            Section::Skipped(why) => {
                                ui.label(
                                    RichText::new(t(cl_model::templates::reason_key(
                                        why.as_wire(),
                                    )
                                    .as_str()))
                                    .color(tokens.warn_text),
                                );
                            }
                            Section::Failed(_) => {
                                ui.label(t("templates_due.failed"));
                            }
                            Section::SavedSeparately => {
                                ui.label(
                                    RichText::new(t("templates_due.saved_separately"))
                                        .color(tokens.warn_text),
                                );
                            }
                            _ => {
                                ui.label(self.account_text(r.template));
                            }
                        }
                        ui.horizontal(|ui| {
                            let mut b = |key: &'static str, tip: &str, on: bool, action| {
                                if ui
                                    .add_enabled(on && idle, egui::Button::new(t(key)))
                                    .on_hover_text(tip)
                                    .clicked()
                                {
                                    click = Some((i, action));
                                }
                            };
                            match r.section {
                                Section::Linking => {
                                    form::hint(ui, tokens, &t("templates_due.linking"));
                                }
                                Section::SavedSeparately => {
                                    b(
                                        "templates_due.dismiss",
                                        &t("templates_due.saved_separately.tip"),
                                        true,
                                        "skip",
                                    );
                                }
                                Section::Entered(_) => {
                                    b(
                                        "templates_due.open",
                                        &t("templates_due.open.tip"),
                                        true,
                                        "open",
                                    );
                                }
                                Section::Waiting => {
                                    b(
                                        "templates_due.enter_now",
                                        &t("templates_due.enter_now.tip"),
                                        enters_directly(&r.kind),
                                        "enter_now",
                                    );
                                    b(
                                        "templates_due.open_in_form",
                                        &t("templates_due.open_in_form.tip"),
                                        true,
                                        "open_in_form",
                                    );
                                    b(
                                        "templates_due.skip_one",
                                        &t("templates_due.skip_one.tip"),
                                        true,
                                        "skip",
                                    );
                                }
                                Section::Skipped(ref why) => {
                                    b(
                                        "templates_due.enter_it_now",
                                        &t("templates_due.enter_it_now.tip"),
                                        true,
                                        "enter_it_now",
                                    );
                                    // A capped catch-up continues on its own.
                                    if !why.is_capped() {
                                        b(
                                            "templates_due.dismiss",
                                            &t("templates_due.dismiss.tip"),
                                            true,
                                            "skip",
                                        );
                                    }
                                }
                                Section::Failed(_) => {
                                    b(
                                        "templates_due.copy_details",
                                        &t("templates_due.copy_details"),
                                        true,
                                        "copy",
                                    );
                                }
                            }
                        });
                        ui.end_row();
                    }
                });
        }
        click
    }

    /// Arrows move, Enter runs the default action, Alt+O opens (when no
    /// text field has the keyboard).
    fn keys(&mut self, ui: &Ui) {
        if self.closing_prompt.dialog_open() || ui.ctx().egui_wants_keyboard_input() {
            return;
        }
        let (up, down, enter, open, esc) = ui.input_mut(|i| {
            (
                i.key_pressed(egui::Key::ArrowUp),
                i.key_pressed(egui::Key::ArrowDown),
                i.key_pressed(egui::Key::Enter),
                i.consume_key(egui::Modifiers::ALT, egui::Key::O),
                i.key_pressed(egui::Key::Escape),
            )
        });
        if esc {
            // Esc closes the window (the status bar says so).
            self.link
                .push(CompanyEvent::CloseWindow(CommandId::new(KIND)));
            return;
        }
        if up || down {
            self.step(down);
        }
        if enter {
            self.default_action(Some(ui.ctx()));
        }
        if open {
            self.open_action();
        }
    }

    fn state_json(&self) -> Value {
        let count = |w: &str| self.rows.iter().filter(|r| r.section.word() == w).count();
        let rows: Vec<Value> = self
            .rows
            .iter()
            .map(|r| {
                json!({
                    "section": r.section.word(),
                    "template": r.template.to_string(),
                    "date": r.date.map(CalendarDate::to_wire),
                    "reason": match &r.section {
                        Section::Skipped(why) => Some(why.as_wire().to_string()),
                        _ => None,
                    },
                    "can_dismiss": matches!(
                        r.section,
                        Section::Waiting | Section::Skipped(_) | Section::SavedSeparately
                    ) && !r.capped(),
                    "saved_separately": r.section == Section::SavedSeparately,
                })
            })
            .collect();
        let mut v = json!({
            "screen": "templates_due",
            "layout": self.layout.json(),
            "company_open": self.tenant.is_some(),
            "available": runs_on(&self.link),
            "as_of": self.as_of.map(CalendarDate::to_wire),
            "entered": count("entered"),
            "waiting": count("waiting"),
            "skipped": count("skipped"),
            "failed": count("failed"),
            "rows": rows,
            "selected": self.selected,
            "notice": self.notice,
            "last_action": self.last_action,
            "entered_txn": self.entered.map(|(t, _)| t.to_string()),
            "error": self.error.is_some(),
            "busy": self.busy(),
            "checking": self.checking,
        });
        if let (Value::Object(m), Value::Object(c)) = (&mut v, self.closing_prompt.state_json(true))
        {
            m.extend(c);
        }
        v
    }
}

impl Drop for TemplatesDueScreen {
    /// A "Skip these dates" still on its way when the window closes no
    /// longer holds the runner (its answer is not read any more).
    fn drop(&mut self) {
        if let Some((id, _)) = &self.catch_up_job {
            self.link.due().skip_ended(*id);
        }
    }
}

/// The engine enters this kind itself (a daily-sales day is entered from
/// its form: its amounts differ each day; templates made online are
/// read-only).
fn enters_directly(kind: &TemplateKind) -> bool {
    kind.is_editable() && *kind != TemplateKind::DailySales
}

impl Screen for TemplatesDueScreen {
    fn title(&self) -> String {
        t("cmd.lists.templates_due")
    }

    fn ui(&mut self, ui: &mut Ui) {
        self.poll();
        self.draw(ui);
        self.keys(ui);
        if self.busy() || self.checking || self.load.is_some() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    fn scope(&self) -> Scope {
        Scope::Window
    }

    fn command(&mut self, id: CommandId) -> bool {
        if let Some(h) = self.closing_prompt.command(id) {
            return h;
        }
        match id.as_str() {
            KIND => true,
            "edit.edit_transaction" => {
                self.open_action();
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

#[cfg(test)]
mod tests;
