//! Templates (P15 lane T, `lists.templates`, Lists menu; Q-256 names).
//!
//! The list groups the templates under collapsible group headers (Q-P15a:
//! a group is a folder only) with Name · Type · How often · Next date ·
//! Mode · Amount · Account, and the toolbar's Use (Enter), Enter now, Edit,
//! Delete and New group.
//!
//! - **Use** reads the template's draft for today and opens its form
//!   filled in (`CompanyEvent::OpenTemplate`); the engine's warnings show
//!   as the form's banner.
//! - **Enter now** posts one occurrence at once; a closed period asks for
//!   the closing-date password (the shared prompt), and "already entered"
//!   (a lost reply's retry) counts as done. Every post publishes
//!   `Ledger(All)` on the data bus (Q-262).
//! - **Edit** changes the name, group and schedule in the Template dialog
//!   (version-checked: 409 `STALE` → "changed in another window").
//! - Templates made online (legacy, Q-P15m) are read-only: Delete only.
//! - Amounts are the engine's list figures, display only (never summed).
//! - Without `templates-v1` the window says templates need an engine
//!   update.

mod dialog;
pub(crate) mod engine;
mod save_flow;

use std::collections::BTreeSet;
use std::sync::Arc;

use cl_api::{
    Account, AccountFilter, ApiError, ClosedPeriodAuthorization, ConflictReason, Entered,
    IdempotencyKey, PostingOptions, RequestGroup, ScheduleMode, Template, TemplateChange,
    TemplateGroup, TemplateKind, client_form,
};
use cl_core::ids::{AccountId, TemplateGroupId, TemplateId, TenantId, TransactionId};
use cl_core::{CalendarDate, Secret};
use cl_model::templates::{CheckedDialog, GroupChoice};
use cl_model::{CommandId, Scope};
use eframe::egui::{self, RichText, Ui};
use serde_json::{Value, json};

use crate::company::demo::template_store;
use crate::company::{CompanyEvent, CompanyLink};
use crate::forms::closing_prompt::{ClosingClick, ClosingPrompt};
use crate::forms::template::TemplateOpen;
use crate::i18n::{t, t_with};
use crate::screens::company::form;
use crate::theme::Tokens;
use crate::windows::Screen;
use dialog::{DialogClick, DialogUi, api_schedule, how_often_text, model_schedule};
use engine::{Job, finished, run, templates_on};

pub use save_flow::SaveFlow;

/// The command and window kind.
pub const KIND: &str = "lists.templates";

/// The Templates window for `kind`, if it is one.
#[must_use]
pub fn make(
    kind: CommandId,
    link: &Arc<CompanyLink>,
    open: Option<(TenantId, String)>,
    today: CalendarDate,
) -> Option<Box<dyn Screen>> {
    (kind.as_str() == KIND)
        .then(|| Box::new(TemplatesScreen::new(Arc::clone(link), open, today)) as Box<_>)
}

/// The form a template opens in (`clientForm`, else its kind).
#[must_use]
pub fn form_kind(client: Option<&str>, kind: &TemplateKind) -> Option<&'static str> {
    match client {
        Some(client_form::WRITE_CHECK) => return Some(crate::screens::write_check::KIND),
        Some(client_form::CARD_CHARGE | client_form::CARD_CREDIT) => {
            return Some(crate::screens::card_charge::KIND);
        }
        Some(client_form::DEPOSIT) => return Some(crate::screens::deposit::KIND),
        Some(client_form::TRANSFER) => return Some(crate::screens::transfer::KIND),
        Some(client_form::JOURNAL_ENTRY) => return Some(crate::screens::journal_entry::KIND),
        Some(client_form::DAILY_SALES) => return Some(crate::screens::daily_sales::KIND),
        _ => {}
    }
    Some(match kind {
        TemplateKind::Check => crate::screens::write_check::KIND,
        TemplateKind::Expense => crate::screens::card_charge::KIND,
        TemplateKind::Deposit => crate::screens::deposit::KIND,
        TemplateKind::Transfer => crate::screens::transfer::KIND,
        TemplateKind::JournalEntry => crate::screens::journal_entry::KIND,
        TemplateKind::DailySales => crate::screens::daily_sales::KIND,
        TemplateKind::Legacy | TemplateKind::Other(_) => return None,
    })
}

/// A template's split lines as a form's lines (Use). An account the
/// engine dropped (`MISSING_ACCOUNT`) shows empty; amounts are copied
/// exactly.
pub(crate) fn form_lines(
    lines: &[cl_api::SplitLine],
    keys: &mut cl_model::rowkey::RowKeyGen,
) -> cl_model::forms::lines::FormLines {
    use cl_model::forms::lines::{FormLine, FormLines};
    let seeds = lines
        .iter()
        .map(|l| FormLine {
            account: (l.account != cl_api::MISSING_ACCOUNT).then_some(l.account),
            amount: Some(l.amount),
            memo: l.memo.clone().unwrap_or_default(),
            customer: l.contact,
            billable: l.billable,
            tag: l.tag,
            ..FormLine::blank(keys.next_key())
        })
        .collect();
    FormLines::from_lines(seeds, keys)
}

/// The plain words for "this automatic schedule starts too far back",
/// naming the earliest date allowed (today − 366 days); `None` for any
/// other error.
pub(crate) fn auto_start_text(e: &ApiError, today: CalendarDate) -> Option<String> {
    if !cl_api::is_auto_start_too_early(e) {
        return None;
    }
    let earliest = cl_model::templates::earliest_auto_start(today)?;
    Some(t_with(
        "templates.problem.auto_start_too_early",
        &[("date", crate::shell::us_date(earliest).as_str())],
    ))
}

/// An account the engine dropped from a template shows empty.
pub(crate) fn real_account(a: AccountId) -> Option<AccountId> {
    (a != cl_api::MISSING_ACCOUNT).then_some(a)
}

/// The Type column's words.
/// A group's heading row: a painted chevron (the UI fonts have no
/// triangles; P14 paints Prev / Next the same way), pointing down when the
/// group is open, then its name.
fn group_toggle(ui: &mut Ui, open: bool, name: &str) -> egui::Response {
    let id = egui::IdSalt::new(("templates-group-chevron", name));
    let size = egui::Vec2::new(10.0, 10.0);
    let r = egui::Button::new((egui::Atom::custom(id, size), RichText::new(name).strong()))
        .frame(false)
        .atom_ui(ui);
    if let Some(rect) = r.rect(id) {
        let color = ui.style().interact(&r.response).text_color();
        let c = rect.center();
        let h = rect.height() / 2.0 - 1.0;
        let points = if open {
            vec![
                egui::pos2(c.x - h, c.y - h / 2.0),
                egui::pos2(c.x + h, c.y - h / 2.0),
                egui::pos2(c.x, c.y + h / 2.0 + 1.0),
            ]
        } else {
            vec![
                egui::pos2(c.x - h / 2.0, c.y - h),
                egui::pos2(c.x - h / 2.0, c.y + h),
                egui::pos2(c.x + h / 2.0 + 1.0, c.y),
            ]
        };
        ui.painter().add(egui::Shape::convex_polygon(
            points,
            color,
            egui::Stroke::NONE,
        ));
    }
    r.response
}

fn kind_key(t: &Template) -> &'static str {
    match (t.client_form.as_deref(), &t.kind) {
        (Some(client_form::CARD_CREDIT), _) => "templates.kind.card_credit",
        (_, TemplateKind::Check) => "templates.kind.check",
        (_, TemplateKind::Expense) => "templates.kind.card_charge",
        (_, TemplateKind::Deposit) => "templates.kind.deposit",
        (_, TemplateKind::Transfer) => "templates.kind.transfer",
        (_, TemplateKind::JournalEntry) => "templates.kind.journal_entry",
        (_, TemplateKind::DailySales) => "templates.kind.daily_sales",
        _ => "templates.kind.online",
    }
}

/// The Mode column's words (Q-P15b).
fn mode_key(t: &Template) -> &'static str {
    match t.schedule.map(|s| s.mode) {
        None => "templates.mode.never",
        Some(ScheduleMode::Remind) => "templates.mode.remind",
        Some(ScheduleMode::Auto) => "templates.mode.auto",
    }
}

/// What the list reads: the templates, the groups and the chart.
type Loaded = (Vec<Template>, Vec<TemplateGroup>, Vec<Account>);

/// What the closing-date password is for: one template's one date.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct EnterNow(TemplateId, CalendarDate);

/// One Enter now: the template and the date it posts on. The idempotency
/// key is kept per occurrence until a definite answer, so a lost reply's
/// retry is the same post (a template without a schedule writes no
/// occurrence row: the key is its only guard).
type Occurrence = (TemplateId, CalendarDate);

/// The Templates window.
pub struct TemplatesScreen {
    link: Arc<CompanyLink>,
    tenant: Option<(TenantId, String)>,
    group: Option<RequestGroup>,
    today: CalendarDate,
    pub(crate) rows: Vec<Template>,
    pub(crate) groups: Vec<TemplateGroup>,
    accounts: Vec<Account>,
    pub(crate) loaded: bool,
    load: Option<Job<Loaded>>,
    pub(crate) selected: Option<TemplateId>,
    collapsed: BTreeSet<Option<TemplateGroupId>>,
    pub(crate) dialog: Option<(TemplateId, u32, DialogUi)>,
    edit_job: Option<Job<Template>>,
    pub(crate) confirm_delete: Option<TemplateId>,
    delete_job: Option<Job<()>>,
    use_job: Option<Job<TemplateOpen>>,
    enter_job: Option<Job<Entered>>,
    enter_for: Option<Occurrence>,
    enter_key: Option<(Occurrence, IdempotencyKey)>,
    /// Enter now on a template without a schedule: the date to post on.
    pub(crate) enter_date: Option<(TemplateId, form::DateInput)>,
    pub(crate) closing_prompt: ClosingPrompt<EnterNow>,
    closing: Option<CalendarDate>,
    pub(crate) new_group: Option<String>,
    group_job: Option<Job<TemplateGroup>>,
    /// The last entry ("Entered. Open" link).
    pub(crate) entered: Option<(TransactionId, &'static str)>,
    pub(crate) error: Option<ApiError>,
    pub(crate) notice: Option<&'static str>,
    pub(crate) stale: bool,
    pub(crate) last_action: Option<&'static str>,
    bus: crate::data_bus::Subscription,
}

impl TemplatesScreen {
    /// The Templates list of the open company.
    #[must_use]
    pub fn new(
        link: Arc<CompanyLink>,
        tenant: Option<(TenantId, String)>,
        today: CalendarDate,
    ) -> TemplatesScreen {
        let group = link.group().and_then(Result::ok);
        let mut s = TemplatesScreen {
            bus: link.bus().subscribe(),
            link,
            tenant,
            group,
            today,
            rows: Vec::new(),
            groups: Vec::new(),
            accounts: Vec::new(),
            loaded: false,
            load: None,
            selected: None,
            collapsed: BTreeSet::new(),
            dialog: None,
            edit_job: None,
            confirm_delete: None,
            delete_job: None,
            use_job: None,
            enter_job: None,
            enter_for: None,
            enter_key: None,
            enter_date: None,
            closing_prompt: ClosingPrompt::default(),
            closing: None,
            new_group: None,
            group_job: None,
            entered: None,
            error: None,
            notice: None,
            stale: false,
            last_action: None,
        };
        s.reload();
        s.poll();
        s
    }

    fn reload(&mut self) {
        if !templates_on(&self.link) {
            self.loaded = true;
            return;
        }
        if let Some(d) = self.link.demo_state() {
            self.closing = d.open.and_then(|open| {
                d.companies
                    .iter()
                    .find(|c| c.tenant_id == open)
                    .and_then(|c| c.closed_through)
            });
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
                Ok((template_store::list(s), template_store::groups(s), chart))
            },
            |s| async move {
                let rows = s.templates().list_all().await?;
                let groups = s.templates().groups().await?;
                let accounts = s
                    .accounts()
                    .list_all(&AccountFilter::default())
                    .await
                    .unwrap_or_default();
                Ok((rows, groups, accounts))
            },
        ));
    }

    fn row(&self, id: TemplateId) -> Option<&Template> {
        self.rows.iter().find(|r| r.id == id)
    }

    fn account_name(&self, id: Option<AccountId>) -> String {
        id.and_then(|a| self.accounts.iter().find(|x| x.id == a))
            .map(crate::screens::register::account_text)
            .unwrap_or_default()
    }

    pub(crate) fn poll(&mut self) {
        if let Some(r) = finished(&mut self.load) {
            self.loaded = true;
            match r {
                Ok((rows, groups, accounts)) => {
                    self.rows = rows;
                    self.groups = groups;
                    self.accounts = accounts;
                    if self.selected.is_none_or(|s| self.row(s).is_none()) {
                        self.selected = self.ordered().first().copied();
                    }
                }
                Err(e) => self.error = Some(e),
            }
        }
        if let Some(r) = finished(&mut self.edit_job) {
            match r {
                Ok(_) => {
                    self.dialog = None;
                    self.last_action = Some("edit");
                    self.reload();
                }
                Err(e) => self.failed(e),
            }
        }
        if let Some(r) = finished(&mut self.delete_job) {
            match r {
                Ok(()) => {
                    self.last_action = Some("delete");
                    self.selected = None;
                    self.reload();
                }
                Err(e) => self.failed(e),
            }
        }
        if let Some(r) = finished(&mut self.group_job) {
            match r {
                Ok(_) => {
                    self.new_group = None;
                    self.reload();
                }
                Err(e) => self.error = Some(e),
            }
        }
        if let Some(r) = finished(&mut self.use_job) {
            match r {
                Ok(open) => {
                    self.last_action = Some("use");
                    self.link.push(CompanyEvent::OpenTemplate(Box::new(open)));
                }
                Err(e) => self.error = Some(e),
            }
        }
        if let Some(r) = finished(&mut self.enter_job) {
            self.enter_done(r);
        }
    }

    /// Enter now's answer. A definite one (posted, already entered, a
    /// refusal) ends the occurrence's key; an unknown outcome keeps it so
    /// the retry is recognised as the same post.
    pub(crate) fn enter_done(&mut self, r: Result<Entered, ApiError>) {
        let occ = self.enter_for.take();
        match r {
            Ok(e) => {
                self.enter_key = None;
                self.entered_ok(Some(e.transaction), occ.map(|(id, _)| id));
            }
            // A retry of a lost reply: the date is already entered.
            Err(e) if cl_api::is_occurrence_taken(&e) => {
                self.enter_key = None;
                self.entered_ok(None, occ.map(|(id, _)| id));
            }
            Err(e) => {
                if !crate::screens::register::keeps_key(&e) {
                    self.enter_key = None;
                }
                let supported = true;
                let pending = occ.map(|(id, date)| EnterNow(id, date));
                let took = pending
                    .is_some_and(|p| self.closing_prompt.failed(&e, p, supported, self.closing));
                if !took {
                    self.error = Some(e);
                }
            }
        }
    }

    /// The idempotency key of one occurrence: the same until a definite
    /// answer, a new one for another template or date.
    fn enter_key_for(&mut self, occ: Occurrence) -> IdempotencyKey {
        match &self.enter_key {
            Some((o, k)) if *o == occ => k.clone(),
            _ => {
                let k = IdempotencyKey::new();
                self.enter_key = Some((occ, k.clone()));
                k
            }
        }
    }

    fn failed(&mut self, e: ApiError) {
        if let ApiError::Conflict {
            reason: ConflictReason::Stale,
            ..
        } = e
        {
            self.stale = true;
            self.reload();
        } else {
            self.error = Some(e);
        }
        if let Some((_, _, d)) = self.dialog.as_mut() {
            d.busy = false;
        }
    }

    fn entered_ok(&mut self, txn: Option<TransactionId>, id: Option<TemplateId>) {
        self.closing_prompt.saved();
        // Any register may show it (Q-262).
        crate::forms::publish_saved(&self.link, &self.bus, &[]);
        self.last_action = Some("enter");
        let kind = id
            .and_then(|i| self.row(i))
            .and_then(|r| form_kind(r.client_form.as_deref(), &r.kind));
        self.entered = txn.zip(kind);
        self.notice = Some(if txn.is_some() {
            "templates.entered"
        } else {
            "templates.entered_already"
        });
        self.reload();
    }

    /// The template ids in the order shown (groups by name, then no
    /// group), skipping collapsed groups.
    fn ordered(&self) -> Vec<TemplateId> {
        self.sections()
            .into_iter()
            .filter(|(g, _)| !self.collapsed.contains(g))
            .flat_map(|(_, rows)| rows)
            .collect()
    }

    /// The groups in order with their templates (by name).
    fn sections(&self) -> Vec<(Option<TemplateGroupId>, Vec<TemplateId>)> {
        let mut out: Vec<(Option<TemplateGroupId>, Vec<TemplateId>)> = self
            .groups
            .iter()
            .map(|g| {
                let ids = self
                    .rows
                    .iter()
                    .filter(|r| r.group == Some(g.id))
                    .map(|r| r.id)
                    .collect();
                (Some(g.id), ids)
            })
            .filter(|(_, ids): &(_, Vec<_>)| !ids.is_empty())
            .collect();
        let known: BTreeSet<TemplateGroupId> = self.groups.iter().map(|g| g.id).collect();
        let loose: Vec<TemplateId> = self
            .rows
            .iter()
            .filter(|r| r.group.is_none_or(|g| !known.contains(&g)))
            .map(|r| r.id)
            .collect();
        if !loose.is_empty() {
            out.push((None, loose));
        }
        out
    }

    // ------------------------------------------------------------ actions

    /// Use: the draft for today, then its form opens filled in.
    pub(crate) fn use_selected(&mut self) {
        let Some(row) = self.selected.and_then(|s| self.row(s)).cloned() else {
            return;
        };
        let Some(kind) = form_kind(row.client_form.as_deref(), &row.kind) else {
            self.notice = Some("templates.online_read_only");
            return;
        };
        self.error = None;
        let (id, version, today) = (row.id, row.version, self.today);
        let cmd = CommandId::new(kind);
        let open = move |u: cl_api::TemplateUse| TemplateOpen {
            kind: cmd,
            template: id,
            version,
            occurrence: None,
            draft: u.draft,
            warnings: u.warnings,
        };
        let open2 = open;
        self.use_job = Some(run(
            &self.link,
            self.group.as_ref(),
            None,
            move |s| template_store::draft(s, id, today).map(open),
            move |s| async move { s.templates().draft(id, today).await.map(open2) },
        ));
        self.poll();
    }

    /// Enter now: a scheduled template posts on the row's next date; one
    /// without a schedule first asks for the date (today to start).
    pub(crate) fn enter_selected(&mut self) {
        let Some(row) = self.selected.and_then(|s| self.row(s)).cloned() else {
            return;
        };
        if !row.kind.is_editable() || row.kind == TemplateKind::DailySales {
            // A daily-sales day is entered from its form (its amounts
            // differ each day).
            self.notice = Some("templates.enter.use_form");
            return;
        }
        self.error = None;
        self.notice = None;
        match row.schedule {
            Some(s) => self.enter_at(row.id, s.next_date, None),
            None => self.enter_date = Some((row.id, form::DateInput::new(self.today))),
        }
    }

    /// The date asked for a template without a schedule was confirmed.
    pub(crate) fn enter_date_confirmed(&mut self) {
        let Some((id, input)) = self.enter_date.as_ref() else {
            return;
        };
        let Some(date) = input.value else {
            self.notice = Some("templates.enter.date_missing");
            return;
        };
        let id = *id;
        self.enter_date = None;
        self.notice = None;
        self.enter_at(id, date, None);
    }

    /// Posts `id` on `date` (with the closing-date password after a
    /// refusal), under the occurrence's kept idempotency key.
    pub(crate) fn enter_at(
        &mut self,
        id: TemplateId,
        date: CalendarDate,
        password: Option<Secret>,
    ) {
        let today = self.today;
        self.enter_for = Some((id, date));
        let key = self.enter_key_for((id, date));
        let pw = password.clone();
        self.enter_job = Some(run(
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
        ));
        self.poll();
    }

    pub(crate) fn edit_selected(&mut self) {
        let Some(row) = self.selected.and_then(|s| self.row(s)).cloned() else {
            return;
        };
        if row.legacy || !row.kind.is_editable() {
            self.notice = Some("templates.online_read_only");
            return;
        }
        self.stale = false;
        self.dialog = Some((
            row.id,
            row.version,
            DialogUi::edit(&row, self.groups.clone(), self.today),
        ));
    }

    fn save_edit(&mut self, id: TemplateId, version: u32, c: CheckedDialog<TemplateGroupId>) {
        let schedule = api_schedule(&c);
        let name = c.name.clone();
        let choice = c.group;
        let change = TemplateChange {
            name: Some(name),
            group: None,
            client_form: None,
            draft: None,
            schedule: Some(schedule),
        };
        let (change2, choice2) = (change.clone(), choice.clone());
        self.edit_job = Some(run(
            &self.link,
            self.group.as_ref(),
            Some("orphan.template_save"),
            move |s| {
                let mut change = change;
                change.group = Some(match choice {
                    GroupChoice::None => None,
                    GroupChoice::Existing(g) => Some(g),
                    GroupChoice::New(n) => Some(template_store::create_group(s, &n)?.id),
                });
                template_store::update(s, id, version, &change)
            },
            move |s| async move {
                let mut change = change2;
                change.group = Some(match choice2 {
                    GroupChoice::None => None,
                    GroupChoice::Existing(g) => Some(g),
                    GroupChoice::New(n) => Some(s.templates().create_group(&n).await?.id),
                });
                s.templates().update(id, version, &change).await
            },
        ));
        self.poll();
    }

    /// Delete (button or key): asks first about the selected template. An
    /// earlier "entered" note goes, so the question stands alone.
    pub(crate) fn ask_delete(&mut self) {
        if self.selected.and_then(|s| self.row(s)).is_none() {
            return;
        }
        self.confirm_delete = self.selected;
        self.notice = None;
        self.entered = None;
    }

    pub(crate) fn delete_confirmed(&mut self) {
        let Some(id) = self.confirm_delete.take() else {
            return;
        };
        let Some(version) = self.row(id).map(|r| r.version) else {
            return;
        };
        self.delete_job = Some(run(
            &self.link,
            self.group.as_ref(),
            Some("orphan.template_delete"),
            move |s| template_store::delete(s, id, version),
            move |s| async move { s.templates().delete(id, version).await },
        ));
        self.poll();
    }

    pub(crate) fn create_group(&mut self) {
        let Some(name) = self.new_group.clone() else {
            return;
        };
        self.group_job = Some(run(
            &self.link,
            self.group.as_ref(),
            Some("orphan.template_group"),
            {
                let n = name.clone();
                move |s| template_store::create_group(s, &n)
            },
            move |s| async move { s.templates().create_group(&name).await },
        ));
        self.poll();
    }

    fn submit_closing(&mut self) {
        if let Some((EnterNow(id, date), secret)) = self.closing_prompt.submit() {
            self.selected = Some(id);
            self.enter_at(id, date, Some(secret));
        }
    }

    fn step(&mut self, down: bool) {
        let order = self.ordered();
        if order.is_empty() {
            return;
        }
        let at = self
            .selected
            .and_then(|s| order.iter().position(|x| *x == s));
        let next = match (at, down) {
            (None, _) => 0,
            (Some(i), true) => (i + 1).min(order.len() - 1),
            (Some(i), false) => i.saturating_sub(1),
        };
        self.selected = Some(order[next]);
    }

    fn busy(&self) -> bool {
        self.load.is_some()
            || self.edit_job.is_some()
            || self.delete_job.is_some()
            || self.use_job.is_some()
            || self.enter_job.is_some()
            || self.group_job.is_some()
    }

    // ------------------------------------------------------------ drawing

    fn draw(&mut self, ui: &mut Ui) {
        let tokens = Tokens::of(ui.ctx());
        if !templates_on(&self.link) {
            form::hint(ui, &tokens, &t("templates.unavailable"));
            return;
        }
        if !self.loaded {
            form::progress_line(ui, &tokens, &t("templates.loading"));
            return;
        }
        self.toolbar(ui, &tokens);
        match self.closing_prompt.ui(ui, &tokens) {
            ClosingClick::Submit => self.submit_closing(),
            ClosingClick::Cancel => self.closing_prompt.cancel(),
            ClosingClick::None => {}
        }
        self.messages(ui, &tokens);
        if self.rows.is_empty() {
            ui.add_space(12.0);
            form::hint(ui, &tokens, &t("templates.empty"));
            return;
        }
        self.table(ui, &tokens);
        self.windows(ui.ctx());
    }

    fn toolbar(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let idle = !self.busy();
        let row = self.selected.and_then(|s| self.row(s)).cloned();
        let editable = row
            .as_ref()
            .is_some_and(|r| !r.legacy && r.kind.is_editable());
        let mut click: Option<&str> = None;
        crate::forms::header::toolbar(ui, tokens, false, "", |ui| {
            let mut b = |ui: &mut Ui, key: &'static str, on: bool, tip: &str| {
                if ui
                    .add_enabled(on && idle, egui::Button::new(t(key)))
                    .on_hover_text(tip)
                    .clicked()
                {
                    click = Some(key);
                }
            };
            b(ui, "templates.use", editable, &t("templates.use.tip"));
            b(
                ui,
                "templates.enter_now",
                editable,
                &t("templates.enter_now.tip"),
            );
            b(ui, "templates.edit", editable, &t("templates.edit.tip"));
            b(
                ui,
                "templates.delete",
                row.is_some(),
                &t("templates.delete.tip"),
            );
            ui.separator();
            b(
                ui,
                "templates.new_group",
                true,
                &t("templates.new_group.tip"),
            );
            None::<()>
        });
        match click {
            Some("templates.use") => self.use_selected(),
            Some("templates.enter_now") => self.enter_selected(),
            Some("templates.edit") => self.edit_selected(),
            Some("templates.delete") => self.ask_delete(),
            Some("templates.new_group") => self.new_group = Some(String::new()),
            _ => {}
        }
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
        if self.stale {
            ui.label(RichText::new(t("templates.save.stale")).color(tokens.warn_text));
        }
        if let Some(e) = &self.error {
            match auto_start_text(e, self.today) {
                Some(text) => {
                    ui.label(RichText::new(text).color(tokens.warn_text));
                }
                None => form::inline_error(ui, tokens, e),
            }
        }
        if let Some(id) = self.confirm_delete {
            let name = self.row(id).map(|r| r.name.clone()).unwrap_or_default();
            let mut answer = None;
            form::card(ui, tokens, |ui| {
                ui.label(t_with(
                    "templates.delete.confirm",
                    &[("name", name.as_str())],
                ));
                ui.horizontal(|ui| {
                    if crate::widgets::destructive_button(ui, &t("templates.delete")).clicked() {
                        answer = Some(true);
                    }
                    if ui
                        .add(
                            egui::Button::new(t("templates.dialog.cancel"))
                                .min_size(egui::Vec2::new(0.0, 44.0)),
                        )
                        .clicked()
                    {
                        answer = Some(false);
                    }
                });
            });
            match answer {
                Some(true) => self.delete_confirmed(),
                Some(false) => self.confirm_delete = None,
                None => {}
            }
        }
        if let Some((_, input)) = self.enter_date.as_mut() {
            let today = self.today;
            let mut answer = None;
            form::card(ui, tokens, |ui| {
                ui.label(t("templates.enter.date_prompt"));
                ui.horizontal(|ui| {
                    let l = ui.label(t("templates.enter.date"));
                    dialog::date_input(ui, input, today, l.id);
                    if ui.button(t("templates.enter_now")).clicked() {
                        answer = Some(true);
                    }
                    if ui.button(t("templates.dialog.cancel")).clicked() {
                        answer = Some(false);
                    }
                });
            });
            match answer {
                Some(true) => self.enter_date_confirmed(),
                Some(false) => self.enter_date = None,
                None => {}
            }
        }
        if let Some(name) = self.new_group.as_mut() {
            let mut answer = None;
            form::card(ui, tokens, |ui| {
                ui.horizontal(|ui| {
                    let l = ui.label(t("templates.new_group.name"));
                    let r = ui
                        .add(egui::TextEdit::singleline(name).desired_width(220.0))
                        .labelled_by(l.id);
                    if form::activated(&r) {
                        answer = Some(true);
                    }
                    if ui.button(t("templates.new_group.add")).clicked() {
                        answer = Some(true);
                    }
                    if ui.button(t("templates.dialog.cancel")).clicked() {
                        answer = Some(false);
                    }
                });
            });
            match answer {
                Some(true) => self.create_group(),
                Some(false) => self.new_group = None,
                None => {}
            }
        }
    }

    fn table(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let style = crate::grid_env::shell_frame(ui.ctx()).money_style;
        let sections = self.sections();
        let mut picked = None;
        let mut used = None;
        let mut toggled = None;
        egui::ScrollArea::vertical()
            .id_salt("templates/scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Grid::new("templates/table")
                    .num_columns(7)
                    .striped(true)
                    .spacing([16.0, 6.0])
                    .show(ui, |ui| {
                        for key in [
                            "templates.col.name",
                            "templates.col.type",
                            "templates.col.how_often",
                            "templates.col.next_date",
                            "templates.col.mode",
                            "templates.col.amount",
                            "templates.col.account",
                        ] {
                            ui.label(RichText::new(t(key)).strong().color(tokens.text_secondary));
                        }
                        ui.end_row();
                        for (g, ids) in &sections {
                            let open = !self.collapsed.contains(g);
                            let name = g
                                .and_then(|g| self.groups.iter().find(|x| x.id == g))
                                .map_or_else(|| t("templates.group.none"), |x| x.name.clone());
                            if group_toggle(ui, open, &name)
                                .on_hover_text(t("templates.group.toggle"))
                                .clicked()
                            {
                                toggled = Some(*g);
                            }
                            ui.end_row();
                            if !open {
                                continue;
                            }
                            for id in ids {
                                let Some(r) = self.row(*id) else { continue };
                                let sel = self.selected == Some(*id);
                                let resp = ui.selectable_label(sel, format!("    {}", r.name));
                                if resp.clicked() {
                                    picked = Some(*id);
                                }
                                if resp.double_clicked() {
                                    used = Some(*id);
                                }
                                ui.label(t(kind_key(r)));
                                let how = r
                                    .schedule
                                    .map(|s| how_often_text(&model_schedule(&s)))
                                    .unwrap_or_default();
                                ui.label(how);
                                ui.label(
                                    r.schedule
                                        .map(|s| crate::shell::us_date(s.next_date))
                                        .unwrap_or_default(),
                                );
                                ui.label(t(mode_key(r)));
                                ui.label(
                                    RichText::new(
                                        r.total.map(|m| m.format(style)).unwrap_or_default(),
                                    )
                                    .text_style(egui::TextStyle::Monospace),
                                );
                                ui.label(self.account_name(r.account));
                                ui.end_row();
                            }
                        }
                    });
            });
        if let Some(g) = toggled
            && !self.collapsed.remove(&g)
        {
            self.collapsed.insert(g);
        }
        if let Some(id) = picked {
            self.selected = Some(id);
        }
        if let Some(id) = used {
            self.selected = Some(id);
            self.use_selected();
        }
    }

    fn windows(&mut self, ctx: &egui::Context) {
        let Some((id, version, d)) = self.dialog.as_mut() else {
            return;
        };
        let (id, version) = (*id, *version);
        match d.show(ctx) {
            DialogClick::None => {}
            DialogClick::Cancel => self.dialog = None,
            DialogClick::Save(c) => {
                d.busy = true;
                self.save_edit(id, version, c);
            }
        }
    }

    /// Arrow keys move, Enter uses, Esc closes the open question or else
    /// the window (when no text field has the keyboard).
    fn keys(&mut self, ui: &Ui) {
        if self.dialog.is_some()
            || self.enter_date.is_some()
            || self.closing_prompt.dialog_open()
            || ui.ctx().egui_wants_keyboard_input()
        {
            return;
        }
        let (up, down, enter, esc) = ui.input(|i| {
            (
                i.key_pressed(egui::Key::ArrowUp),
                i.key_pressed(egui::Key::ArrowDown),
                i.key_pressed(egui::Key::Enter),
                i.key_pressed(egui::Key::Escape),
            )
        });
        if up || down {
            self.step(down);
        }
        if enter {
            self.use_selected();
        }
        if esc && !self.command(CommandId::new("form.revert")) {
            self.link
                .push(CompanyEvent::CloseWindow(CommandId::new(KIND)));
        }
    }

    fn state_json(&self) -> Value {
        let rows: Vec<Value> = self
            .ordered()
            .iter()
            .filter_map(|id| self.row(*id))
            .map(|r| {
                json!({
                    "id": r.id.to_string(),
                    "kind": r.kind.as_wire(),
                    "group": r.group.map(|g| g.to_string()),
                    "mode": mode_key(r).rsplit('.').next(),
                    "next_date": r.schedule.map(|s| s.next_date.to_wire()),
                    "legacy": r.legacy,
                    "version": r.version,
                })
            })
            .collect();
        let mut v = json!({
            "screen": "templates",
            "company_open": self.tenant.is_some(),
            "available": templates_on(&self.link),
            "loaded": self.loaded,
            "count": self.rows.len(),
            "groups": self.groups.len(),
            "rows": rows,
            "selected": self.selected.map(|s| s.to_string()),
            "empty": self.loaded && self.rows.is_empty(),
            "dialog": self.dialog.as_ref().map(|(_, _, d)| d.state_json()),
            "confirm_delete": self.confirm_delete.is_some(),
            "new_group": self.new_group.is_some(),
            "enter_date": self
                .enter_date
                .as_ref()
                .map(|(_, d)| d.value.map(CalendarDate::to_wire)),
            "last_action": self.last_action,
            "notice": self.notice,
            "entered": self.entered.map(|(t, _)| t.to_string()),
            "stale": self.stale,
            "error": self.error.is_some(),
            "auto_start_too_early": self.error.as_ref().is_some_and(cl_api::is_auto_start_too_early),
            "busy": self.busy(),
        });
        if let (Value::Object(m), Value::Object(c)) = (&mut v, self.closing_prompt.state_json(true))
        {
            m.extend(c);
        }
        v
    }
}

impl Screen for TemplatesScreen {
    fn title(&self) -> String {
        t("cmd.lists.templates")
    }

    fn ui(&mut self, ui: &mut Ui) {
        self.poll();
        let changes = self.link.bus().take(&mut self.bus);
        if !changes.is_empty() && !self.busy() {
            self.reload();
            self.poll();
        }
        self.draw(ui);
        self.keys(ui);
        if self.busy() {
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
            "list.new" => {
                self.new_group = Some(String::new());
                true
            }
            // Delete (the key): the same confirmation as the button.
            "list.delete" => {
                if !self.busy() && self.dialog.is_none() && self.enter_date.is_none() {
                    self.ask_delete();
                }
                true
            }
            "edit.edit_transaction" => {
                if !self.busy() {
                    self.edit_selected();
                }
                true
            }
            "form.revert" => {
                if self.dialog.take().is_some()
                    || self.confirm_delete.take().is_some()
                    || self.new_group.take().is_some()
                    || self.enter_date.take().is_some()
                {
                    return true;
                }
                false
            }
            _ => false,
        }
    }

    fn state(&self) -> Value {
        self.state_json()
    }
}

#[cfg(test)]
mod tests;
