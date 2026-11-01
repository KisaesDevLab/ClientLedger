//! Statements (P22 D6, `clients.statements`; Q-175-Q-177, Q-P22c,
//! Q-P22h, Q-P22n).
//!
//! - Left: the statement date, layout (open item by default, balance
//!   forward, transaction), the period start (not for open item), days
//!   overdue at least, "Only clients with a balance" (on, Q-176), and the
//!   engine's batch list (`GET /reports/customer-statements`): tick
//!   clients, click a name to preview.
//! - Right: the selected client's statement as the engine computes it
//!   (`GET /reports/customer-statement`): lines, the aging row (Q-177)
//!   and the amount due; a non-zero `difference` shows a plain warning.
//! - Print, Save PDF, Email and Mark as sent each issue the ticked
//!   clients' statements first (`POST /customer-statement-runs`, at most
//!   200 per run, one key per run kept for a retry): each is a numbered
//!   snapshot (Q-P22n) and what goes out is that snapshot. Printing
//!   records "printed"; a PDF records "pdf" only with Mark as sent ticked
//!   (Q-P20e); Email sends now or puts the message in the Outbox with the
//!   statement template (Q-163); clients without an email address are
//!   listed to print instead.
//!
//! No money is computed here: every figure is the engine's string.

mod draw;
pub(crate) mod print;
#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::mpsc::Receiver;

use cl_api::{
    ApiError, CustomerStatement, EmailMessage, IdempotencyKey, IssuedStatementRecord,
    NewStatementRun, OrphanLabel, RequestGroup, SendMethod, SendRecord, StatementFormat,
    StatementListQuery, StatementListRow, StatementQuery,
};
use cl_core::ids::{ContactId, TenantId};
use cl_core::{CalendarDate, MoneyStyle};
use cl_model::forms::statements::{
    RunPlan, StatementFormatChoice, StatementsForm, StatementsProblem,
};
use cl_model::{CommandId, Scope};
use eframe::egui::Ui;
use serde_json::{Value, json};

use crate::company::{CompanyEvent, CompanyLink};
use crate::screens::company::form::DateInput;
use crate::screens::invoice::engine::{self, Job};
use crate::screens::print_forms::job::{self as print_job, Sent};
use crate::windows::Screen;

/// The command and window kind.
pub const KIND: &str = "clients.statements";

/// The Statements window for `kind`, if it is one.
#[must_use]
pub fn make(
    kind: CommandId,
    link: &Arc<CompanyLink>,
    open: Option<(TenantId, String)>,
    today: CalendarDate,
) -> Option<Box<dyn Screen>> {
    (kind.as_str() == KIND)
        .then(|| Box::new(StatementsScreen::new(Arc::clone(link), open, today)) as Box<_>)
}

/// What the issued statements are for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    Print,
    Pdf,
    Email,
    MarkSent,
}

impl Action {
    fn wire(self) -> &'static str {
        match self {
            Action::Print => "print",
            Action::Pdf => "pdf",
            Action::Email => "email",
            Action::MarkSent => "mark_sent",
        }
    }
}

/// The email dialog: the template text (fields filled per client when
/// sent) and the issued statements it goes with.
#[derive(Clone, Default)]
pub(crate) struct EmailDraft {
    pub subject: String,
    pub body: String,
    pub cc: String,
}

/// Where the email goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EmailWay {
    Now,
    Outbox,
}

/// What an email batch did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct EmailOutcome {
    pub sent: u32,
    pub queued: u32,
    pub failed: u32,
    pub problem: Option<&'static str>,
}

/// The Statements window.
pub struct StatementsScreen {
    pub(crate) link: Arc<CompanyLink>,
    pub(crate) group: Option<RequestGroup>,
    pub(crate) tenant: Option<(TenantId, String)>,
    pub(crate) today: CalendarDate,
    pub(crate) form: StatementsForm,
    pub(crate) as_of: DateInput,
    pub(crate) start: DateInput,
    pub(crate) min_days: String,
    pub(crate) rows: Vec<StatementListRow>,
    pub(crate) list_job: Option<Job<Vec<StatementListRow>>>,
    pub(crate) loaded: bool,
    pub(crate) listed_for: Option<(CalendarDate, u32, bool)>,
    /// The client previewed.
    pub(crate) cursor: Option<ContactId>,
    /// A client asked for before the list arrived (Client Center).
    pub(crate) wanted: Option<ContactId>,
    pub(crate) preview: Option<CustomerStatement>,
    pub(crate) preview_job: Option<Job<CustomerStatement>>,
    pub(crate) preview_for: Option<(
        ContactId,
        CalendarDate,
        StatementFormatChoice,
        Option<CalendarDate>,
    )>,
    /// One key per run of an attempt, kept until the run is answered.
    pub(crate) keys: BTreeMap<(u64, usize), IdempotencyKey>,
    pub(crate) issue_job: Option<(Action, Job<Vec<IssuedStatementRecord>>)>,
    /// The statements the last action issued.
    pub(crate) issued: Vec<IssuedStatementRecord>,
    /// Save PDF counts as sent (Q-P20e).
    pub(crate) pdf_marks_sent: bool,
    pub(crate) email: Option<EmailDraft>,
    pub(crate) email_job: Option<Job<EmailOutcome>>,
    pub(crate) mark_job: Option<Job<u32>>,
    pub(crate) print_rx: Option<Receiver<Sent>>,
    pub(crate) pdf_job: Option<Job<(String, Vec<u8>)>>,
    pub(crate) printer: print_job::Printer,
    pub(crate) prefs: Option<cl_settings::EmailPrefs>,
    pub(crate) prefs_rx: Option<Receiver<cl_settings::EmailPrefs>>,
    pub(crate) problems: Vec<StatementsProblem>,
    pub(crate) notice: Option<&'static str>,
    pub(crate) error: Option<ApiError>,
    /// What the last action did (control channel).
    pub(crate) last: Option<&'static str>,
    pub(crate) issued_count: usize,
    pub(crate) marked_count: u32,
    pub(crate) email_outcome: Option<EmailOutcome>,
    pub(crate) money_style: MoneyStyle,
    pub(crate) layout: crate::layout::FrameReport,
    /// The last action started (control channel).
    pub(crate) action: Option<Action>,
}

impl std::fmt::Debug for StatementsScreen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StatementsScreen")
            .field("form", &self.form)
            .field("rows", &self.rows.len())
            .finish_non_exhaustive()
    }
}

fn wire_format(f: StatementFormatChoice) -> StatementFormat {
    match f {
        StatementFormatChoice::OpenItem => StatementFormat::OpenItem,
        StatementFormatChoice::BalanceForward => StatementFormat::BalanceForward,
        StatementFormatChoice::Transaction => StatementFormat::Transaction,
    }
}

fn split(s: &str) -> Vec<String> {
    s.split([',', ';'])
        .map(str::trim)
        .filter(|a| !a.is_empty())
        .map(str::to_string)
        .collect()
}

impl StatementsScreen {
    /// Statements as of today.
    #[must_use]
    pub fn new(
        link: Arc<CompanyLink>,
        tenant: Option<(TenantId, String)>,
        today: CalendarDate,
    ) -> StatementsScreen {
        let group = link.group().and_then(Result::ok);
        let root = link
            .store()
            .path()
            .parent()
            .and_then(std::path::Path::parent)
            .map(std::path::Path::to_path_buf);
        let fake = link.is_demo();
        let mut s = StatementsScreen {
            printer: print_job::Printer { root, fake },
            link,
            group,
            tenant,
            today,
            form: StatementsForm::new(Some(today)),
            as_of: DateInput::new(today),
            start: DateInput {
                text: String::new(),
                value: None,
            },
            min_days: "0".into(),
            rows: Vec::new(),
            list_job: None,
            loaded: false,
            listed_for: None,
            cursor: None,
            wanted: None,
            preview: None,
            preview_job: None,
            preview_for: None,
            keys: BTreeMap::new(),
            issue_job: None,
            issued: Vec::new(),
            pdf_marks_sent: false,
            email: None,
            email_job: None,
            mark_job: None,
            print_rx: None,
            pdf_job: None,
            prefs: None,
            prefs_rx: None,
            problems: Vec::new(),
            notice: None,
            error: None,
            last: None,
            issued_count: 0,
            marked_count: 0,
            email_outcome: None,
            money_style: MoneyStyle::default(),
            layout: crate::layout::FrameReport::default(),
            action: None,
        };
        s.load_prefs();
        s.poll();
        s
    }

    fn company(&self) -> String {
        self.tenant
            .as_ref()
            .map(|(_, n)| n.clone())
            .unwrap_or_default()
    }

    /// Reads the email templates off the frame.
    fn load_prefs(&mut self) {
        let Some((id, _)) = self.tenant.clone() else {
            self.prefs = Some(cl_settings::EmailPrefs::default());
            return;
        };
        let store = self.link.store().clone();
        let (tx, rx) = std::sync::mpsc::channel();
        if std::thread::Builder::new()
            .name("statement-email-prefs".into())
            .spawn(move || {
                let p = store
                    .load()
                    .ok()
                    .and_then(|l| l.get(id).map(|r| r.prefs.email.clone()))
                    .unwrap_or_default();
                let _ = tx.send(p);
            })
            .is_ok()
        {
            self.prefs_rx = Some(rx);
        }
    }

    // ------------------------------------------------------------ choices

    pub(crate) fn set_as_of(&mut self, d: Option<CalendarDate>) {
        self.form.as_of = d;
        self.form.touched();
    }

    pub(crate) fn set_format(&mut self, f: StatementFormatChoice) {
        self.form.format = f;
        self.form.touched();
    }

    pub(crate) fn set_start(&mut self, d: Option<CalendarDate>) {
        self.form.start = d;
        self.form.touched();
    }

    pub(crate) fn min_days_typed(&mut self) {
        if let Ok(n) = self.min_days.trim().parse::<u32>() {
            self.form.min_days_overdue = n.min(3650);
        } else if self.min_days.trim().is_empty() {
            self.form.min_days_overdue = 0;
        }
        self.min_days = self.form.min_days_overdue.to_string();
    }

    /// The client previewed (a name clicked, or Client Center's).
    pub(crate) fn show_client(&mut self, c: ContactId) {
        self.cursor = Some(c);
    }

    /// Client Center's Statement: this client picked and previewed.
    pub(crate) fn for_client(&mut self, c: ContactId) {
        if !self.form.is_selected(c) {
            self.form.toggle(c);
        }
        self.cursor = Some(c);
        self.wanted = Some(c);
    }

    pub(crate) fn select_all(&mut self, on: bool) {
        let ids: Vec<ContactId> = if on {
            self.rows.iter().map(|r| r.contact_id).collect()
        } else {
            Vec::new()
        };
        self.form.select_only(&ids);
    }

    // ------------------------------------------------------------ reads

    /// Takes every answer that arrived; reads the list and the preview
    /// again when their inputs changed.
    pub(crate) fn poll(&mut self) {
        if let Some(rx) = &self.prefs_rx
            && let Ok(p) = rx.try_recv()
        {
            self.prefs = Some(p);
            self.prefs_rx = None;
        }
        if let Some(r) = engine::poll(&mut self.list_job) {
            self.loaded = true;
            match r {
                Ok(rows) => {
                    if self.cursor.is_none() {
                        self.cursor = rows.first().map(|r| r.contact_id);
                    }
                    self.rows = rows;
                }
                Err(e) => self.error = Some(e),
            }
        }
        let want = self
            .form
            .as_of
            .map(|d| (d, self.form.min_days_overdue, self.form.open_only));
        if self.list_job.is_none() && want.is_some() && want != self.listed_for {
            self.listed_for = want;
            self.start_list();
        }
        if let Some(r) = engine::poll(&mut self.preview_job) {
            match r {
                Ok(p) => self.preview = Some(p),
                Err(e) => {
                    self.preview = None;
                    self.error = Some(e);
                }
            }
        }
        let pv = match (self.cursor, self.form.as_of) {
            (Some(c), Some(d)) => Some((c, d, self.form.format, self.form.effective_start())),
            _ => None,
        };
        if self.preview_job.is_none() && pv.is_some() && pv != self.preview_for {
            self.preview_for = pv;
            self.start_preview();
        }
        self.poll_issue();
        self.poll_send();
    }

    fn start_list(&mut self) {
        let Some((as_of, min, open)) = self.listed_for else {
            return;
        };
        let q = StatementListQuery {
            as_of,
            min_days_overdue: min,
            open_only: open,
        };
        self.list_job = Some(engine::read(
            &self.link,
            self.group.as_ref(),
            move |d| Ok(crate::company::demo::statements::statement_list(d, &q)),
            move |s| async move {
                let limit = cl_api::PageRequest::MAX_LIMIT;
                let mut all = Vec::new();
                let mut offset = 0u64;
                loop {
                    let page = s
                        .customer_statements()
                        .statement_list(&q, cl_api::PageRequest::new(limit, offset)?)
                        .await?;
                    let n = page.data.len();
                    all.extend(page.data);
                    offset += u64::try_from(n).unwrap_or(u64::MAX);
                    if n < usize::from(limit) || offset >= page.total {
                        break;
                    }
                }
                Ok(all)
            },
        ));
    }

    fn start_preview(&mut self) {
        let Some((contact, as_of, format, start)) = self.preview_for else {
            return;
        };
        let q = StatementQuery {
            contact,
            as_of,
            format: wire_format(format),
            start,
        };
        self.preview_job = Some(engine::read(
            &self.link,
            self.group.as_ref(),
            move |d| crate::company::demo::statements::statement(d, &q),
            move |s| async move { s.customer_statements().statement(&q).await },
        ));
    }

    // ------------------------------------------------------------ issue

    pub(crate) fn busy(&self) -> bool {
        self.issue_job.is_some()
            || self.email_job.is_some()
            || self.mark_job.is_some()
            || self.print_rx.is_some()
            || self.pdf_job.is_some()
    }

    /// The clients the action goes to: Email only those with an address.
    fn plan_for(&mut self, action: Action) -> Option<RunPlan> {
        self.problems.clear();
        let mut plan = match self.form.run_plan() {
            Ok(p) => p,
            Err(p) => {
                self.problems = p;
                return None;
            }
        };
        if action == Action::Email {
            let with: Vec<ContactId> = self
                .rows
                .iter()
                .filter(|r| r.has_email)
                .map(|r| r.contact_id)
                .collect();
            for c in &mut plan.chunks {
                c.clients.retain(|x| with.contains(x));
            }
            plan.chunks.retain(|c| !c.clients.is_empty());
            if plan.chunks.is_empty() {
                self.notice = Some("statements.email.nobody");
                return None;
            }
        }
        Some(plan)
    }

    /// Issues the ticked clients' statements, then does `action` with
    /// them.
    pub(crate) fn start(&mut self, action: Action) {
        if self.busy() {
            return;
        }
        self.notice = None;
        self.error = None;
        self.email_outcome = None;
        let Some(plan) = self.plan_for(action) else {
            return;
        };
        let runs: Vec<(NewStatementRun, IdempotencyKey)> = plan
            .chunks
            .iter()
            .map(|c| {
                let key = self.keys.entry((c.attempt, c.index)).or_default().clone();
                (
                    NewStatementRun {
                        as_of: plan.as_of,
                        format: wire_format(plan.format),
                        start: plan.start,
                        contact_ids: c.clients.clone(),
                    },
                    key,
                )
            })
            .collect();
        let Some(first) = runs.first().map(|(_, k)| k.clone()) else {
            return;
        };
        let demo_runs = runs.clone();
        let job = engine::write(
            &self.link,
            self.group.as_ref(),
            OrphanLabel {
                what: "orphan.statement_run",
                key: Some(first),
            },
            move |d| {
                let mut out = Vec::new();
                for (r, k) in &demo_runs {
                    let run = crate::company::demo::statements::create_run(d, r, &k.to_string())?;
                    for s in run.statements {
                        out.push(crate::company::demo::statements::get(d, s.statement_id)?);
                    }
                }
                Ok(out)
            },
            move |s| async move {
                let mut out = Vec::new();
                for (r, k) in runs {
                    let run = s.customer_statements().create_run(&r, k).await?;
                    for st in run.statements {
                        out.push(s.customer_statements().get(st.statement_id).await?);
                    }
                }
                Ok(out)
            },
        );
        self.action = Some(action);
        self.issue_job = Some((action, job));
        self.poll_issue();
    }

    fn poll_issue(&mut self) {
        let Some((action, job)) = self.issue_job.as_mut() else {
            return;
        };
        let Some(r) = job.take() else {
            return;
        };
        let action = *action;
        self.issue_job = None;
        match r {
            Ok(records) => {
                // The runs are answered: the next action issues new
                // statements (a retry before this reused the keys).
                self.keys.clear();
                self.form.touched();
                self.issued_count += records.len();
                self.issued = records;
                self.listed_for = None;
                match action {
                    Action::Print => self.print_issued(),
                    Action::Pdf => self.pdf_issued(),
                    Action::Email => self.open_email(),
                    Action::MarkSent => self.mark(SendMethod::Printed, "marked"),
                }
            }
            Err(e) => {
                if !crate::screens::register::keeps_key(&e) {
                    self.keys.clear();
                }
                if cl_api::statement_refusal(&e).is_some() {
                    self.notice = Some("statements.refused");
                } else {
                    self.error = Some(e);
                    self.notice = Some("statements.not_issued");
                }
            }
        }
    }

    fn message(&self) -> String {
        self.form.message.clone()
    }

    /// Print: the issued statements to the printer (the OS hand-off); a
    /// print that worked records "printed" on each.
    fn print_issued(&mut self) {
        let Ok(doc) = print::document(&self.issued, &self.company(), &self.message()) else {
            self.notice = Some("statements.print.failed");
            return;
        };
        let Ok(bytes) = cl_print::render(&doc) else {
            self.notice = Some("statements.print.failed");
            return;
        };
        self.print_rx = Some(print_job::print(
            &self.printer,
            doc,
            Arc::from(bytes),
            "statement",
            None,
            1,
        ));
        self.poll_send();
    }

    /// Save PDF: the issued statements open full size (made on the
    /// queue); "pdf" is recorded only with Mark as sent ticked.
    fn pdf_issued(&mut self) {
        let records = self.issued.clone();
        let (company, message) = (self.company(), self.message());
        let render = move || {
            let doc = print::document(&records, &company, &message)?;
            let bytes = cl_print::render(&doc)
                .map_err(|_| ApiError::Local(cl_api::LocalValidation::InvalidValue))?;
            let name = match records.as_slice() {
                [one] => cl_api::statement_pdf_name(&one.number),
                _ => "Statements.pdf".to_string(),
            };
            Ok((name, bytes))
        };
        let r2 = render.clone();
        self.pdf_job = Some(engine::read(
            &self.link,
            self.group.as_ref(),
            move |_| r2(),
            move |_| async move { render() },
        ));
        self.poll_send();
    }

    /// Records `method` on every issued statement.
    fn mark(&mut self, method: SendMethod, last: &'static str) {
        let ids: Vec<_> = self.issued.iter().map(|r| r.id).collect();
        let rec = SendRecord {
            method,
            recipients: Vec::new(),
            sent_at: None,
        };
        let demo_ids = ids.clone();
        let demo_rec = rec.clone();
        self.mark_job = Some(engine::write(
            &self.link,
            self.group.as_ref(),
            OrphanLabel {
                what: "orphan.statement_sent",
                key: None,
            },
            move |d| {
                for id in &demo_ids {
                    crate::company::demo::statements::mark_sent(d, *id, &demo_rec)?;
                }
                Ok(u32::try_from(demo_ids.len()).unwrap_or(u32::MAX))
            },
            move |s| async move {
                for id in &ids {
                    s.customer_statements().mark_sent(*id, &rec).await?;
                }
                Ok(u32::try_from(ids.len()).unwrap_or(u32::MAX))
            },
        ));
        self.last = Some(last);
        self.poll_send();
    }

    // ------------------------------------------------------------ email

    fn open_email(&mut self) {
        let prefs = self.prefs.clone().unwrap_or_default();
        let (subject, body) = match prefs.template(cl_settings::TemplateDoc::Statement) {
            cl_settings::TemplateSource::Default {
                subject_key,
                body_key,
            } => (crate::i18n::t(subject_key), crate::i18n::t(body_key)),
            cl_settings::TemplateSource::Custom(c) => (c.subject.clone(), c.body.clone()),
        };
        self.email = Some(EmailDraft {
            subject,
            body,
            cc: String::new(),
        });
    }

    /// The message to `r`'s client: the template filled with its values.
    pub(crate) fn message_for(&self, d: &EmailDraft, r: &IssuedStatementRecord) -> EmailMessage {
        let values = cl_settings::FieldValues {
            client_name: r.snapshot.contact.name.clone(),
            statement_date: crate::shell::us_date(r.as_of),
            amount_due: r.amount_due.format(self.money_style),
            company_name: self.company(),
            ..cl_settings::FieldValues::default()
        };
        // BCC-self needs the company's From address, which only the
        // invoice dialog reads today; statements go without it.
        EmailMessage {
            to: r.snapshot.contact.email.iter().cloned().collect(),
            cc: split(&d.cc),
            bcc: Vec::new(),
            subject: cl_settings::fill_template(&d.subject, &values).text,
            body: cl_settings::fill_template(&d.body, &values).text,
        }
    }

    /// Sends the issued statements' emails now, or puts them in the
    /// Outbox. One key per statement and way.
    pub(crate) fn send_email(&mut self, way: EmailWay) {
        let Some(d) = self.email.clone() else {
            return;
        };
        if self.email_job.is_some() {
            return;
        }
        let items: Vec<(IssuedStatementRecord, EmailMessage, IdempotencyKey)> = self
            .issued
            .iter()
            .filter(|r| r.snapshot.contact.email.is_some())
            .map(|r| (r.clone(), self.message_for(&d, r), IdempotencyKey::new()))
            .collect();
        let (company, message) = (self.company(), self.message());
        let demo_items = items.clone();
        let (c2, m2) = (company.clone(), message.clone());
        self.email_job = Some(engine::write(
            &self.link,
            self.group.as_ref(),
            OrphanLabel {
                what: "orphan.statement_email",
                key: items.first().map(|(_, _, k)| k.clone()),
            },
            move |st| {
                let mut o = EmailOutcome::default();
                for (r, msg, key) in &demo_items {
                    let res = match way {
                        EmailWay::Now => print::pdf_of(r, &c2, &m2).and_then(|_| {
                            crate::company::demo::statements::email(
                                st,
                                r.id,
                                msg,
                                None,
                                &key.to_string(),
                            )
                        }),
                        EmailWay::Outbox => {
                            crate::company::demo::statements::email_later(st, r.id, msg).map(|_| ())
                        }
                    };
                    tally(&mut o, way, res);
                }
                Ok(o)
            },
            move |s| async move {
                let mut o = EmailOutcome::default();
                for (r, msg, key) in items {
                    let res = match way {
                        EmailWay::Now => match print::pdf_of(&r, &company, &message) {
                            Ok((name, pdf)) => {
                                s.customer_statements()
                                    .email(r.id, &msg, &name, &pdf, None, key)
                                    .await
                            }
                            Err(e) => Err(e),
                        },
                        EmailWay::Outbox => s
                            .customer_statements()
                            .email_later(r.id, &msg, key)
                            .await
                            .map(|_| ()),
                    };
                    tally(&mut o, way, res);
                }
                Ok(o)
            },
        ));
        self.poll_send();
    }

    fn poll_send(&mut self) {
        if let Some(rx) = &self.print_rx
            && let Ok(sent) = rx.try_recv()
        {
            self.print_rx = None;
            self.notice = Some(sent.key());
            // Only a print that reached the printer counts as sent
            // (P22 plan §5: a failed hand-off records nothing).
            if matches!(sent, Sent::Printed | Sent::Handed) {
                self.mark(SendMethod::Printed, "printed");
            } else {
                self.last = Some("print_failed");
            }
        }
        match engine::poll(&mut self.pdf_job) {
            Some(Ok((name, bytes))) => {
                self.link.push(CompanyEvent::OpenWith(
                    CommandId::new(crate::screens::document_view::KIND),
                    Box::new(crate::windows::OpenPayload::Document(
                        crate::widgets::pdf_viewer::DocSource {
                            name,
                            bytes: Arc::from(bytes),
                        },
                    )),
                ));
                if self.pdf_marks_sent {
                    self.mark(SendMethod::Pdf, "pdf_sent");
                } else {
                    self.last = Some("pdf");
                }
            }
            Some(Err(_)) => self.notice = Some("statements.print.failed"),
            None => {}
        }
        match engine::poll(&mut self.mark_job) {
            Some(Ok(n)) => {
                self.marked_count += n;
                if self.notice.is_none() {
                    self.notice = Some("statements.marked");
                }
                self.link.push(CompanyEvent::Toast("statements.marked"));
            }
            Some(Err(e)) => self.error = Some(e),
            None => {}
        }
        match engine::poll(&mut self.email_job) {
            Some(Ok(o)) => {
                self.email_outcome = Some(o);
                if o.failed == 0 {
                    self.email = None;
                }
                self.last = Some(if o.queued > 0 { "queued" } else { "emailed" });
                if o.queued > 0 {
                    crate::screens::outbox::refresh_count(&self.link, self.group.as_ref());
                }
                self.listed_for = None;
            }
            Some(Err(e)) => self.error = Some(e),
            None => {}
        }
    }

    /// The control channel's view (counts, flags, the shown figures of the
    /// demo's synthetic data; no names).
    #[must_use]
    pub fn state_json(&self) -> Value {
        let style = self.money_style;
        let mut v = json!({
            "screen": "statements",
            "loaded": self.loaded,
            "format": self.form.format.wire(),
            "shows_start": self.form.shows_start(),
            "rows": self.rows.len(),
            "selected": self.form.selected_count(),
            "amounts_due": self.rows.iter().map(|r| r.amount_due.format(style)).collect::<Vec<_>>(),
            "without_email": self.rows.iter().filter(|r| !r.has_email).count(),
            "busy": self.busy(),
            "issued": self.issued_count,
            "issued_numbers": self.issued.iter().map(|r| r.number.clone()).collect::<Vec<_>>(),
            "marked": self.marked_count,
            "email_open": self.email.is_some(),
            "email": self.email_outcome.map(|o| json!({"sent": o.sent, "queued": o.queued, "failed": o.failed, "problem": o.problem})),
            "last": self.last,
            "action": self.action.map(Action::wire),
            "notice": self.notice,
            "problems": self.problems.iter().map(|p| format!("{p:?}")).collect::<Vec<_>>(),
        });
        if let Some(p) = &self.preview {
            let b = &p.aging.buckets;
            v["preview"] = json!({
                "lines": p.lines.len(),
                "amount_due": p.amount_due.format(style),
                "difference": p.has_difference(),
                "aging": {
                    "current": b.current.format(style),
                    "days_1_30": b.days_1_30.format(style),
                    "days_31_60": b.days_31_60.format(style),
                    "days_61_90": b.days_61_90.format(style),
                    "over_90": b.over_90.format(style),
                    "total": p.aging.total.format(style),
                },
            });
        }
        v
    }
}

fn tally(o: &mut EmailOutcome, way: EmailWay, r: Result<(), ApiError>) {
    match r {
        Ok(()) => match way {
            EmailWay::Now => o.sent += 1,
            EmailWay::Outbox => o.queued += 1,
        },
        Err(e) => {
            o.failed += 1;
            o.problem = Some(crate::screens::invoice::email::send_problem(
                cl_api::invoice_refusal(&e),
            ));
        }
    }
}

impl Screen for StatementsScreen {
    fn title(&self) -> String {
        crate::i18n::t(&format!("cmd.{KIND}"))
    }

    fn ui(&mut self, ui: &mut Ui) {
        self.money_style = crate::grid_env::shell_frame(ui.ctx()).money_style;
        self.poll();
        self.draw(ui);
        if self.busy() || self.list_job.is_some() || self.preview_job.is_some() {
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
        match id.as_str() {
            KIND => {}
            "file.print" | "form.print" => self.start(Action::Print),
            "form.revert" => {
                if self.email.is_some() {
                    self.email = None;
                } else {
                    self.link
                        .push(CompanyEvent::CloseWindow(CommandId::new(KIND)));
                }
            }
            _ => return false,
        }
        true
    }

    fn open_with(&mut self, payload: crate::windows::OpenPayload) -> bool {
        match payload {
            crate::windows::OpenPayload::ForClient(c) => {
                self.for_client(c);
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
