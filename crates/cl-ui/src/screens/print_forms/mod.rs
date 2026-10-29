//! File > Print Forms (P19, `file.print_forms`; no mockup, Q-P19e): the
//! checks waiting to print on a bank account and the forms marked Print
//! later, the PDF preview (Q-P19m) with the printer list (remembered per
//! kind, Q-P19w), Print, Save PDF (Q-P19x) and Envelopes (Q-P19aa), and
//! "Did all checks print correctly?" (Q-55, Q-P19b, Q-P19n).
//!
//! Check numbers are recorded with the engine when the owner clicks Print,
//! before anything reaches the printer (`cl_print::PrintRun`). The batch is
//! saved on this computer (`cl_settings::UnconfirmedChecks`) until the
//! owner answers; Home shows a reminder meanwhile. A first bad number sends
//! it and the later checks back to the queue (engine requeue; old numbers
//! logged as spoiled and reusable); a check in a closed period asks for the
//! closing-date password (Q-P19ac).
//!
//! Printer Setup (`file.printer_setup`) opens the same window on its setup
//! card: the printer's alignment offset in millimetres (this computer,
//! Q-P19g), the plain-paper ruler test (Q-P19p), and the bank account's
//! check style and company block (U-109, Q-P19k).
//!
//! Engine calls go through the request queue; printing runs on its own
//! thread (`job`). Nothing reaches a printer in the demo.

pub mod job;

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::Receiver;

use cl_api::{
    Account, AccountFilter, ApiError, Capabilities, CheckFormat, CheckLayout,
    ClosedPeriodAuthorization, FormFooters, OrphanLabel, PostingOptions, PrintChecks, PrintProblem,
    PrintedBatch, QueuedCheck, QueuedForm, RequestGroup, RequeuedCheck,
};
use cl_core::ids::{AccountId, TenantId, TransactionId};
use cl_core::{CalendarDate, Money, Secret};
use cl_model::{CommandId, Scope};
use cl_print::{
    AlignmentOffset, CheckOptions, CompanyInfo, Confirmation, DocKind, Document, Logo, PrintQueue,
    PrintRun,
};
use cl_settings::{
    PrintKind, PrinterPrefs, PrinterPrefsStore, UnconfirmedBatch, UnconfirmedCheck,
    UnconfirmedChecksStore,
};
use eframe::egui::{self, RichText, Ui};
use serde_json::{Value, json};

use crate::company::{CompanyLink, Task};
use crate::forms::closing_prompt::{ClosingClick, ClosingPrompt};
use crate::i18n::{t, t_with};
use crate::screens::card_charge::kit;
use crate::screens::company::form;
use crate::theme::Tokens;
use crate::widgets::pdf_viewer::{DocSource, DocViewer, ViewerState};
use crate::windows::{OpenPayload, Screen};
use job::Sent;

/// The command and window kind.
pub const KIND: &str = "file.print_forms";
/// Printer Setup opens the same window on its setup card.
pub const SETUP_KIND: &str = "file.printer_setup";

/// The Print Forms window for `kind`, if it is one.
#[must_use]
pub fn make(
    kind: CommandId,
    link: &Arc<CompanyLink>,
    open: Option<(TenantId, String)>,
    today: CalendarDate,
) -> Option<Box<dyn Screen>> {
    match kind.as_str() {
        KIND => Some(Box::new(PrintFormsScreen::new(
            Arc::clone(link),
            open,
            today,
            false,
        ))),
        SETUP_KIND => Some(Box::new(PrintFormsScreen::new(
            Arc::clone(link),
            open,
            today,
            true,
        ))),
        _ => None,
    }
}

/// The settings folder (printer choices and unanswered batches live there).
fn settings_dir(link: &CompanyLink) -> Option<PathBuf> {
    link.store()
        .path()
        .parent()
        .map(std::path::Path::to_path_buf)
}

/// True while `tenant` has printed checks waiting for "Did all checks print
/// correctly?" (Home's Needs attention line, Q-P19n).
#[must_use]
pub fn waiting_answer(link: &CompanyLink, tenant: TenantId) -> bool {
    settings_dir(link).is_some_and(|d| {
        UnconfirmedChecksStore::in_dir(&d)
            .load()
            .needs_attention(&tenant.to_string())
    })
}

/// The two lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tab {
    Checks,
    Forms,
}

/// What the preview shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PreviewKind {
    Checks,
    Forms,
    Envelopes,
    Alignment,
}

impl PreviewKind {
    fn wire(self) -> &'static str {
        match self {
            PreviewKind::Checks => "checks",
            PreviewKind::Forms => "forms",
            PreviewKind::Envelopes => "envelopes",
            PreviewKind::Alignment => "alignment",
        }
    }
}

/// The preview.
pub(crate) struct Preview {
    kind: PreviewKind,
    doc: Document,
    pdf: Arc<[u8]>,
    viewer: ViewerState,
    file_name: String,
    /// The addresses for Envelopes (name first).
    addresses: Vec<Vec<String>>,
}

/// What the closing-date password is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Locked {
    Record,
    Requeue(Vec<TransactionId>),
}

/// Read once.
#[derive(Clone, Default)]
pub(crate) struct Setup {
    banks: Vec<(AccountId, String)>,
    closing: Option<CalendarDate>,
    company: CompanyInfo,
    logo: Option<Vec<u8>>,
    footers: FormFooters,
}

/// One bank account's queue.
#[derive(Clone, Default)]
pub(crate) struct Queue {
    checks: Vec<QueuedCheck>,
    next: Option<u64>,
    layout: CheckLayout,
}

/// The pages' data, read for a preview.
pub(crate) enum Built {
    Checks(Vec<cl_print::StubData>),
    Forms(Vec<Vec<(String, Money)>>),
}

/// The Print Forms window.
#[allow(clippy::struct_excessive_bools)] // independent flags of one window
pub struct PrintFormsScreen {
    link: Arc<CompanyLink>,
    tenant: Option<(TenantId, String)>,
    group: Option<RequestGroup>,
    printer_hand_off: job::Printer,
    pub(crate) tab: Tab,
    setup_task: Option<Task<Setup>>,
    pub(crate) loaded: bool,
    setup: Setup,
    logo: Option<Logo>,
    pub(crate) logo_notice: bool,
    pub(crate) bank: Option<AccountId>,
    queue_task: Option<Task<Queue>>,
    pub(crate) checks: Vec<QueuedCheck>,
    pub(crate) picked: BTreeSet<TransactionId>,
    pub(crate) first_number: String,
    pub(crate) layout: CheckLayout,
    forms_task: Option<Task<Vec<QueuedForm>>>,
    pub(crate) forms: Vec<QueuedForm>,
    pub(crate) forms_picked: BTreeSet<TransactionId>,
    pub(crate) forms_unavailable: bool,
    pub(crate) copies: String,
    printers_rx: Option<Receiver<(Vec<String>, Option<String>)>>,
    pub(crate) printers: Vec<String>,
    pub(crate) printer: Option<String>,
    prefs_store: Option<PrinterPrefsStore>,
    prefs: PrinterPrefs,
    unconfirmed: Option<UnconfirmedChecksStore>,
    build_task: Option<Task<Built>>,
    building: Option<PreviewKind>,
    pub(crate) preview: Option<Preview>,
    run: Option<PrintRun<TransactionId>>,
    run_dates: Vec<CalendarDate>,
    run_bank: Option<AccountId>,
    record_task: Option<Task<PrintedBatch>>,
    requeue_task: Option<Task<Vec<RequeuedCheck>>>,
    requeue_ids: Vec<TransactionId>,
    mark_task: Option<Task<cl_api::MarkedPrinted>>,
    job: Option<Receiver<Sent>>,
    pub(crate) sent: Option<Sent>,
    pending_forms: Vec<TransactionId>,
    /// "Did all checks print correctly?" is showing.
    pub(crate) asking: bool,
    pub(crate) first_bad: String,
    closing_prompt: ClosingPrompt<Locked>,
    pub(crate) setup_open: bool,
    pub(crate) offset_x: String,
    pub(crate) offset_y: String,
    pub(crate) setup_style: CheckFormat,
    pub(crate) setup_block: bool,
    layout_task: Option<Task<()>>,
    save_rx: Option<Receiver<Option<PathBuf>>>,
    write_rx: Option<Receiver<bool>>,
    pub(crate) error: Option<ApiError>,
    pub(crate) problem: Option<PrintProblem>,
    pub(crate) notice: Option<&'static str>,
    preselect: Vec<TransactionId>,
    /// Reprint check: preview it once it is back in the queue.
    reprint: Option<TransactionId>,
}

fn options(password: Option<Secret>) -> PostingOptions {
    PostingOptions {
        closed_period: password.map(ClosedPeriodAuthorization::with_password),
    }
}

fn address_lines(p: &cl_api::CompanyProfileUpdate) -> Vec<String> {
    let mut out: Vec<String> = [&p.address_line1, &p.address_line2]
        .into_iter()
        .flatten()
        .filter(|l| !l.trim().is_empty())
        .cloned()
        .collect();
    let city = [p.city.as_deref(), p.state.as_deref()]
        .into_iter()
        .flatten()
        .filter(|s| !s.trim().is_empty())
        .collect::<Vec<_>>()
        .join(", ");
    let last = [city.as_str(), p.zip.as_deref().unwrap_or("")]
        .into_iter()
        .filter(|s| !s.trim().is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if !last.is_empty() {
        out.push(last);
    }
    out
}

fn banks_of(accounts: &[Account]) -> Vec<(AccountId, String)> {
    accounts
        .iter()
        .filter(|a| {
            crate::screens::register::class_of(a) == Some(cl_model::register::AccountClass::Bank)
        })
        .map(|a| (a.id, crate::account_names::shown_name(a)))
        .collect()
}

impl PrintFormsScreen {
    /// Print Forms (or Printer Setup) in the open company.
    #[must_use]
    pub fn new(
        link: Arc<CompanyLink>,
        tenant: Option<(TenantId, String)>,
        _today: CalendarDate,
        setup_open: bool,
    ) -> PrintFormsScreen {
        let group = link.group().and_then(Result::ok);
        let dir = settings_dir(&link);
        let prefs_store = dir.as_deref().map(PrinterPrefsStore::in_dir);
        let prefs = prefs_store
            .as_ref()
            .map(PrinterPrefsStore::load)
            .unwrap_or_default();
        let fake = link.is_demo();
        let mut s = PrintFormsScreen {
            printer_hand_off: job::Printer {
                root: dir
                    .as_deref()
                    .and_then(std::path::Path::parent)
                    .map(std::path::Path::to_path_buf),
                fake,
            },
            unconfirmed: dir.as_deref().map(UnconfirmedChecksStore::in_dir),
            link,
            tenant,
            group,
            tab: Tab::Checks,
            setup_task: None,
            loaded: false,
            setup: Setup::default(),
            logo: None,
            logo_notice: false,
            bank: None,
            queue_task: None,
            checks: Vec::new(),
            picked: BTreeSet::new(),
            first_number: String::new(),
            layout: CheckLayout::default(),
            forms_task: None,
            forms: Vec::new(),
            forms_picked: BTreeSet::new(),
            forms_unavailable: false,
            copies: "1".into(),
            printers_rx: Some(job::printers(fake)),
            printers: Vec::new(),
            printer: prefs.last_printer(PrintKind::Checks).map(str::to_string),
            prefs_store,
            prefs,
            build_task: None,
            building: None,
            preview: None,
            run: None,
            run_dates: Vec::new(),
            run_bank: None,
            record_task: None,
            requeue_task: None,
            requeue_ids: Vec::new(),
            mark_task: None,
            job: None,
            sent: None,
            pending_forms: Vec::new(),
            asking: false,
            first_bad: String::new(),
            closing_prompt: ClosingPrompt::default(),
            setup_open,
            offset_x: "0".into(),
            offset_y: "0".into(),
            setup_style: CheckFormat::Voucher,
            setup_block: false,
            layout_task: None,
            save_rx: None,
            write_rx: None,
            error: None,
            problem: None,
            notice: None,
            preselect: Vec::new(),
            reprint: None,
        };
        s.sync_offset_fields();
        s.start_setup();
        s.poll();
        s
    }

    fn company_key(&self) -> Option<String> {
        self.tenant.as_ref().map(|(t, _)| t.to_string())
    }

    fn start_setup(&mut self) {
        if let Some(mut d) = self.link.demo_state() {
            let chart = d
                .chart
                .get_or_insert_with(crate::company::demo::chart)
                .clone();
            let closing = crate::company::demo::journal::closing(&d);
            let name = self
                .tenant
                .as_ref()
                .map(|(_, n)| n.clone())
                .unwrap_or_default();
            let setup = Setup {
                banks: banks_of(&chart),
                closing,
                company: job::company(
                    &name,
                    vec!["410 Commerce St".into(), "Branson, MO 65616".into()],
                ),
                logo: crate::company::demo::printing::logo(&d),
                footers: crate::company::demo::printing::footers(&d),
            };
            drop(d);
            self.setup_task = Some(Task::ready(setup));
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            self.loaded = true;
            return;
        };
        let caps = self.link.capabilities();
        let logo_cap = caps.has(Capabilities::COMPANY_LOGO);
        match group.read(&session, move |s| async move {
            let accounts = s.accounts().list_all(&AccountFilter::default()).await?;
            let closing = s.company_settings().await.ok().and_then(|c| c.lock_date);
            let company = s
                .company_profile()
                .await
                .map(|p| {
                    job::company(
                        &p.record.profile.business_name,
                        address_lines(&p.record.profile),
                    )
                })
                .unwrap_or_default();
            let logo = if logo_cap {
                s.printing().logo().await.ok().flatten()
            } else {
                None
            };
            let footers = s.printing().footers().await.unwrap_or_default();
            Ok(Setup {
                banks: banks_of(&accounts),
                closing,
                company,
                logo,
                footers,
            })
        }) {
            Ok(p) => self.setup_task = Some(Task::Pending(p)),
            Err(_) => self.loaded = true,
        }
    }

    pub(crate) fn load_queue(&mut self) {
        let Some(bank) = self.bank else {
            return;
        };
        if let Some(mut d) = self.link.demo_state() {
            let checks = crate::company::demo::printing::check_queue(&mut d, bank);
            let (next, layout) = crate::company::demo::printing::layout(&mut d, bank);
            drop(d);
            self.queue_task = Some(Task::ready(Queue {
                checks,
                next,
                layout,
            }));
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        if let Ok(p) = group.read(&session, move |s| async move {
            let checks = s.printing().check_queue(Some(bank)).await?;
            let (settings, layout) = s.printing().check_layout(bank).await?;
            Ok(Queue {
                checks,
                next: settings.next_for(bank).map(u64::from),
                layout,
            })
        }) {
            self.queue_task = Some(Task::Pending(p));
        }
    }

    pub(crate) fn load_forms(&mut self) {
        if let Some(d) = self.link.demo_state() {
            let q = crate::company::demo::printing::form_queue(&d);
            drop(d);
            self.forms_task = Some(Task::ready(q));
            return;
        }
        if !self
            .link
            .capabilities()
            .has(Capabilities::FORMS_PRINT_QUEUE)
        {
            self.forms_unavailable = true;
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        if let Ok(p) = group.read(
            &session,
            |s| async move { s.printing().form_queue(None).await },
        ) {
            self.forms_task = Some(Task::Pending(p));
        }
    }

    fn sync_offset_fields(&mut self) {
        let o = self
            .printer
            .as_deref()
            .map(|p| self.prefs.offset_for(p))
            .unwrap_or_default();
        self.offset_x = format!("{:.1}", o.offset_x_mm);
        self.offset_y = format!("{:.1}", o.offset_y_mm);
    }

    fn offset(&self) -> AlignmentOffset {
        let o = self
            .printer
            .as_deref()
            .map(|p| self.prefs.offset_for(p))
            .unwrap_or_default();
        AlignmentOffset::new(o.offset_x_mm, o.offset_y_mm).unwrap_or(AlignmentOffset::NONE)
    }

    fn check_options(&self, layout: CheckLayout) -> CheckOptions {
        CheckOptions {
            style: crate::company::demo::printing::style_of(layout.format),
            offset: self.offset(),
            show_company: layout.print_company_block,
            company: self.setup.company.clone(),
            logo: self.logo.clone(),
            labels: job::check_labels(),
        }
    }

    pub(crate) fn picked_checks(&self) -> Vec<QueuedCheck> {
        self.checks
            .iter()
            .filter(|c| self.picked.contains(&c.id))
            .cloned()
            .collect()
    }

    pub(crate) fn picked_forms(&self) -> Vec<QueuedForm> {
        self.forms
            .iter()
            .filter(|f| self.forms_picked.contains(&f.id))
            .cloned()
            .collect()
    }

    fn first(&self) -> Option<u64> {
        self.first_number
            .trim()
            .parse::<u64>()
            .ok()
            .filter(|n| *n > 0)
    }

    /// Preview the ticked checks: number them from the first number, read
    /// their stubs, lay them out.
    pub(crate) fn preview_checks(&mut self) {
        self.clear_messages();
        let picked = self.picked_checks();
        let mut queue = PrintQueue::new();
        for c in &self.checks {
            queue.add(c.id, DocKind::Check);
        }
        let ids: Vec<_> = picked.iter().map(|c| c.id).collect();
        match queue.start_run(DocKind::Check, &ids, self.first()) {
            Ok(run) => {
                self.run = Some(run);
                self.run_bank = self.bank;
                self.run_dates = picked.iter().map(|c| c.date).collect();
            }
            Err(e) => {
                self.notice = Some(match e {
                    cl_print::QueueError::NoFirstNumber | cl_print::QueueError::NumberOverflow => {
                        "print.need_first_number"
                    }
                    _ => "print.nothing_picked",
                });
                return;
            }
        }
        self.building = Some(PreviewKind::Checks);
        if let Some(mut d) = self.link.demo_state() {
            let stubs = picked
                .iter()
                .map(|c| crate::company::demo::printing::stub(&mut d, c))
                .collect();
            drop(d);
            self.build_task = Some(Task::ready(Built::Checks(stubs)));
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        if let Ok(p) = group.read(&session, move |s| async move {
            let mut stubs = Vec::new();
            for c in &picked {
                let stub = if c.kind == cl_api::QueuedCheckKind::BillPayment {
                    s.bill_payments()
                        .get(c.id)
                        .await
                        .map(|r| job::stub_of_payment(&r))
                } else {
                    s.transactions()
                        .get(c.id)
                        .await
                        .map(|t| job::stub_of_check(&t.record))
                };
                stubs.push(stub.unwrap_or_default());
            }
            Ok(Built::Checks(stubs))
        }) {
            self.build_task = Some(Task::Pending(p));
        }
    }

    pub(crate) fn preview_forms(&mut self) {
        self.clear_messages();
        let picked = self.picked_forms();
        if picked.is_empty() {
            self.notice = Some("print.nothing_picked");
            return;
        }
        self.building = Some(PreviewKind::Forms);
        if self.link.is_demo() {
            let lines = picked
                .iter()
                .map(|f| vec![(t("print.demo.services"), f.total.unwrap_or(Money::ZERO))])
                .collect();
            self.build_task = Some(Task::ready(Built::Forms(lines)));
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        if let Ok(p) = group.read(&session, move |s| async move {
            let mut all = Vec::new();
            for f in &picked {
                all.push(
                    s.transactions()
                        .get(f.id)
                        .await
                        .map(|t| job::form_lines(&t.record))
                        .unwrap_or_default(),
                );
            }
            Ok(Built::Forms(all))
        }) {
            self.build_task = Some(Task::Pending(p));
        }
    }

    fn show_preview(
        &mut self,
        kind: PreviewKind,
        doc: Document,
        file_name: String,
        addresses: Vec<Vec<String>>,
    ) {
        match cl_print::render(&doc) {
            Ok(bytes) => {
                let pdf: Arc<[u8]> = Arc::from(bytes.into_boxed_slice());
                let mut viewer = ViewerState::default();
                viewer.open(DocSource {
                    name: file_name.clone(),
                    bytes: Arc::clone(&pdf),
                });
                let kind_print = if matches!(kind, PreviewKind::Checks | PreviewKind::Alignment) {
                    PrintKind::Checks
                } else {
                    PrintKind::Forms
                };
                if let Some(p) = self.prefs.last_printer(kind_print) {
                    self.printer = Some(p.to_string());
                }
                self.preview = Some(Preview {
                    kind,
                    doc,
                    pdf,
                    viewer,
                    file_name,
                    addresses,
                });
            }
            Err(e) => self.layout_failed(&e),
        }
    }

    fn layout_failed(&mut self, e: &cl_print::PrintError) {
        self.run = None;
        self.notice = Some(match e {
            cl_print::PrintError::StubMismatch => "print.problem.stub",
            cl_print::PrintError::DoesNotFit(_) | cl_print::PrintError::AmountTooLarge => {
                "print.problem.too_large"
            }
            cl_print::PrintError::NegativeAmount | cl_print::PrintError::SubCent => {
                "print.problem.amount"
            }
            _ => "print.problem.layout",
        });
    }

    fn built(&mut self, b: Built) {
        let kind = self.building.take();
        match (kind, b) {
            (Some(PreviewKind::Checks), Built::Checks(stubs)) => {
                let Some(run) = &self.run else {
                    return;
                };
                let picked = self.picked_checks();
                let data: Vec<_> = run
                    .items()
                    .iter()
                    .zip(stubs)
                    .filter_map(|(item, stub)| {
                        let c = picked.iter().find(|c| c.id == item.id)?;
                        Some(job::check_data(c, item.number, stub))
                    })
                    .collect();
                let addresses = data.iter().map(|c| c.address.clone()).collect();
                let first = data.first().map(|c| c.number.clone()).unwrap_or_default();
                match cl_print::layout_checks(&data, &self.check_options(self.layout)) {
                    Ok(doc) => {
                        let name = cl_print::pdf_file_name(&t("print.file.checks"), &first, "");
                        self.show_preview(PreviewKind::Checks, doc, name, addresses);
                    }
                    Err(e) => self.layout_failed(&e),
                }
            }
            (Some(PreviewKind::Forms), Built::Forms(lines)) => {
                let picked = self.picked_forms();
                let mut docs = Vec::new();
                for (f, l) in picked.iter().zip(&lines) {
                    let style = cl_print::FormStyle {
                        company: self.setup.company.clone(),
                        logo: self.logo.clone(),
                        footer: self.setup.footers.resolved_for(f.kind).to_string(),
                        labels: cl_print::FormLabels::default(),
                    };
                    match cl_print::layout_form(&job::form_doc(f, l), &style) {
                        Ok(d) => docs.push(d),
                        Err(e) => {
                            self.layout_failed(&e);
                            return;
                        }
                    }
                }
                let addresses = picked
                    .iter()
                    .map(|f| f.customer.clone().into_iter().collect())
                    .collect();
                let name = picked.first().map_or_else(String::new, |f| {
                    cl_print::pdf_file_name(
                        &job::kind_word(f.kind),
                        f.number.as_deref().unwrap_or(""),
                        f.customer.as_deref().unwrap_or(""),
                    )
                });
                if let Some(doc) = job::join(docs, &t("print.file.forms")) {
                    self.pending_forms = picked.iter().map(|f| f.id).collect();
                    self.show_preview(PreviewKind::Forms, doc, name, addresses);
                }
            }
            _ => {}
        }
    }

    pub(crate) fn preview_envelopes(&mut self) {
        let Some(p) = &self.preview else {
            return;
        };
        let addresses = p.addresses.clone();
        let opts = cl_print::EnvelopeOptions {
            return_address: self.setup.company.clone(),
            show_logo: self.logo.is_some(),
            logo: self.logo.clone(),
        };
        match cl_print::layout_envelopes(&addresses, &opts) {
            Ok(doc) => {
                let name = cl_print::pdf_file_name(&t("print.file.envelopes"), "", "");
                self.show_preview(PreviewKind::Envelopes, doc, name, addresses);
            }
            Err(e) => self.layout_failed(&e),
        }
    }

    pub(crate) fn preview_alignment(&mut self) {
        let opts = self.check_options(CheckLayout {
            format: self.setup_style,
            print_company_block: self.setup_block,
            own: true,
        });
        match cl_print::alignment_test(&opts) {
            Ok(doc) => {
                let name = cl_print::pdf_file_name(&t("print.file.alignment"), "", "");
                self.show_preview(PreviewKind::Alignment, doc, name, Vec::new());
            }
            Err(e) => self.layout_failed(&e),
        }
    }

    fn remember_printer(&mut self, kind: PrintKind) {
        if let Some(p) = self.printer.clone() {
            self.prefs.set_last_printer(kind, &p);
            if let Some(s) = &self.prefs_store
                && s.save(&self.prefs).is_err()
            {
                tracing::warn!("printer choices could not be saved");
            }
        }
    }

    /// Print in the preview: checks record their numbers first.
    pub(crate) fn print_clicked(&mut self) {
        let Some(kind) = self.preview.as_ref().map(|p| p.kind) else {
            return;
        };
        self.clear_messages();
        match kind {
            PreviewKind::Checks => self.record(None),
            PreviewKind::Forms => {
                let copies = self
                    .copies
                    .trim()
                    .parse::<u16>()
                    .unwrap_or(1)
                    .clamp(1, cl_host::MAX_COPIES);
                self.remember_printer(PrintKind::Forms);
                self.send_to_printer("forms", copies);
                let ids = std::mem::take(&mut self.pending_forms);
                self.mark_printed(ids);
            }
            PreviewKind::Envelopes | PreviewKind::Alignment => {
                self.remember_printer(PrintKind::Checks);
                self.send_to_printer("print", 1);
            }
        }
    }

    fn send_to_printer(&mut self, kind: &'static str, copies: u16) {
        if let Some(p) = &self.preview {
            self.job = Some(job::print(
                &self.printer_hand_off,
                p.doc.clone(),
                Arc::clone(&p.pdf),
                kind,
                self.printer.clone(),
                copies,
            ));
        }
    }

    fn record(&mut self, password: Option<Secret>) {
        let (Some(run), Some(bank)) = (self.run.as_mut(), self.run_bank) else {
            return;
        };
        let Ok(call) = run.record_print() else {
            return;
        };
        let req = PrintChecks {
            bank_account: bank,
            checks: call.check_ids,
            starting_number: call.starting_number,
            format: self.layout.format,
        };
        let demo = self
            .link
            .demo_state()
            .map(|mut d| crate::company::demo::printing::print(&mut d, &req, password.as_ref()));
        if let Some(r) = demo {
            self.recorded(r);
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        let label = OrphanLabel {
            what: "orphan.print_checks",
            key: None,
        };
        let r = group.write(&session, label, move |s| async move {
            s.printing().print_checks(&req, &options(password)).await
        });
        match r {
            Ok(p) => self.record_task = Some(Task::Pending(p)),
            Err(_) => {
                if let Some(run) = self.run.as_mut() {
                    let _ = run.record_failed();
                }
            }
        }
    }

    fn recorded(&mut self, r: Result<PrintedBatch, ApiError>) {
        let Some(run) = self.run.as_mut() else {
            return;
        };
        match r {
            Ok(_) => {
                self.closing_prompt.saved();
                let Ok(pending) = run.recorded() else {
                    return;
                };
                if let (Some(store), Some(company), Some(bank)) =
                    (&self.unconfirmed, self.company_key(), self.run_bank)
                {
                    let mut all = store.load();
                    all.add(UnconfirmedBatch {
                        company_id: company,
                        bank_account_id: bank.to_string(),
                        printed_at: chrono::Utc::now().to_rfc3339(),
                        checks: pending
                            .checks
                            .iter()
                            .map(|(id, n)| UnconfirmedCheck {
                                check_id: id.to_string(),
                                number: *n,
                            })
                            .collect(),
                    });
                    if store.save(&all).is_err() {
                        tracing::warn!("the printed batch could not be saved");
                    }
                }
                self.remember_printer(PrintKind::Checks);
                self.send_to_printer("checks", 1);
                self.asking = true;
                self.first_bad.clear();
                self.link
                    .bus()
                    .publish(crate::data_bus::Change::ledger(self.run_bank));
            }
            Err(e) => {
                let _ = run.record_failed();
                let supported = kit::closing_supported(&self.link);
                if !self
                    .closing_prompt
                    .failed(&e, Locked::Record, supported, self.setup.closing)
                {
                    self.problem = cl_api::print_problem(&e);
                    if self.problem.is_none() {
                        self.error = Some(e);
                    }
                }
            }
        }
    }

    /// "All printed" or the first bad number.
    pub(crate) fn answer(&mut self, all: bool) {
        let Some(run) = self.run.as_mut() else {
            return;
        };
        let answer = if all {
            Confirmation::AllPrinted
        } else {
            let Ok(n) = self.first_bad.trim().parse::<u64>() else {
                self.notice = Some("print.confirm.bad_number");
                return;
            };
            Confirmation::FirstBad(n)
        };
        match run.confirm(answer) {
            Ok(outcome) => {
                self.asking = false;
                let printed: Vec<TransactionId> = outcome.printed.iter().map(|p| p.id).collect();
                self.resolve(&printed);
                if let Some(n) = outcome.next_first_number {
                    self.first_number = n.to_string();
                }
                if let Some(call) = outcome.requeue {
                    self.requeue(call.check_ids, None);
                } else {
                    self.preview = None;
                    self.run = None;
                    self.notice = Some("print.confirm.done");
                    self.load_queue();
                }
            }
            Err(_) => self.notice = Some("print.confirm.bad_number"),
        }
    }

    /// Closed without an answer: the checks stay printed, Home reminds.
    pub(crate) fn later(&mut self) {
        if let Some(run) = self.run.as_mut() {
            let _ = run.dismissed();
        }
        self.asking = false;
        self.preview = None;
        self.notice = Some("print.confirm.later_note");
        self.load_queue();
    }

    fn resolve(&mut self, ids: &[TransactionId]) {
        if let (Some(store), Some(company)) = (&self.unconfirmed, self.company_key()) {
            let mut all = store.load();
            let ids: Vec<String> = ids.iter().map(ToString::to_string).collect();
            let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
            all.resolve(&company, &refs);
            if store.save(&all).is_err() {
                tracing::warn!("the printed batch could not be updated");
            }
        }
    }

    fn requeue(&mut self, ids: Vec<TransactionId>, password: Option<Secret>) {
        self.requeue_ids.clone_from(&ids);
        let dates = self.run_dates.clone();
        let demo = self.link.demo_state().map(|mut d| {
            crate::company::demo::printing::requeue(&mut d, &ids, &dates, password.as_ref())
        });
        if let Some(r) = demo {
            self.requeued(r);
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        let label = OrphanLabel {
            what: "orphan.print_checks",
            key: None,
        };
        if let Ok(p) = group.write(&session, label, move |s| async move {
            s.printing().requeue(&ids, &options(password)).await
        }) {
            self.requeue_task = Some(Task::Pending(p));
        }
    }

    fn requeued(&mut self, r: Result<Vec<RequeuedCheck>, ApiError>) {
        match r {
            Ok(_) => {
                self.closing_prompt.saved();
                let ids = std::mem::take(&mut self.requeue_ids);
                self.resolve(&ids);
                self.preview = None;
                self.run = None;
                self.notice = Some("print.confirm.requeued");
                if let Some(id) = self.reprint.take() {
                    self.preselect = vec![id];
                }
                self.link
                    .bus()
                    .publish(crate::data_bus::Change::ledger(self.run_bank));
                self.load_queue();
            }
            Err(e) => {
                let supported = kit::closing_supported(&self.link);
                let pending = Locked::Requeue(self.requeue_ids.clone());
                if !self
                    .closing_prompt
                    .failed(&e, pending, supported, self.setup.closing)
                {
                    self.problem = cl_api::print_problem(&e);
                    if self.problem.is_none() {
                        self.error = Some(e);
                    }
                }
            }
        }
    }

    fn mark_printed(&mut self, ids: Vec<TransactionId>) {
        if ids.is_empty() {
            return;
        }
        if let Some(mut d) = self.link.demo_state() {
            let r = crate::company::demo::printing::mark_printed(&mut d, &ids);
            drop(d);
            self.mark_task = Some(Task::ready(r));
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        let label = OrphanLabel {
            what: "orphan.print_checks",
            key: None,
        };
        if let Ok(p) = group.write(&session, label, move |s| async move {
            s.printing().mark_forms_printed(&ids).await
        }) {
            self.mark_task = Some(Task::Pending(p));
        }
    }

    fn submit_closing(&mut self) {
        let Some((pending, secret)) = self.closing_prompt.submit() else {
            return;
        };
        match pending {
            Locked::Record => self.record(Some(secret)),
            Locked::Requeue(ids) => self.requeue(ids, Some(secret)),
        }
    }

    /// Reopen the question for the oldest unanswered batch (Home's
    /// reminder, or after a restart).
    pub(crate) fn reopen_question(&mut self) {
        let (Some(store), Some(company)) = (&self.unconfirmed, self.company_key()) else {
            return;
        };
        let all = store.load();
        let Some(b) = all.for_company(&company).next() else {
            return;
        };
        let checks: Vec<(TransactionId, u64)> = b
            .checks
            .iter()
            .filter_map(|c| Some((TransactionId::parse(&c.check_id).ok()?, c.number)))
            .collect();
        self.run_bank = AccountId::parse(&b.bank_account_id).ok();
        self.run_dates.clear();
        self.run = Some(PrintRun::unconfirmed(checks));
        self.asking = true;
        self.first_bad.clear();
    }

    pub(crate) fn has_unanswered(&self) -> bool {
        self.company_key().is_some_and(|c| {
            self.unconfirmed
                .as_ref()
                .is_some_and(|s| s.load().needs_attention(&c))
        })
    }

    /// Printer Setup: save the offset for the chosen printer and the bank
    /// account's check style.
    pub(crate) fn save_setup(&mut self) {
        self.clear_messages();
        let (Ok(x), Ok(y)) = (
            self.offset_x.trim().parse::<f32>(),
            self.offset_y.trim().parse::<f32>(),
        ) else {
            self.notice = Some("print.setup.bad_offset");
            return;
        };
        if let Some(p) = self.printer.clone() {
            if self.prefs.set_offset(&p, x, y).is_err() {
                self.notice = Some("print.setup.bad_offset");
                return;
            }
            if let Some(s) = &self.prefs_store
                && s.save(&self.prefs).is_err()
            {
                tracing::warn!("printer choices could not be saved");
            }
        }
        let Some(bank) = self.bank else {
            self.notice = Some("print.setup.saved");
            return;
        };
        let layout = CheckLayout {
            format: self.setup_style,
            print_company_block: self.setup_block,
            own: true,
        };
        if let Some(mut d) = self.link.demo_state() {
            crate::company::demo::printing::set_layout(&mut d, bank, layout);
            drop(d);
            self.layout_task = Some(Task::ready(()));
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        let label = OrphanLabel {
            what: "orphan.print_checks",
            key: None,
        };
        if let Ok(p) = group.write(&session, label, move |s| async move {
            s.printing().set_check_layout(bank, layout).await
        }) {
            self.layout_task = Some(Task::Pending(p));
        }
    }

    pub(crate) fn save_pdf(&mut self, ctx: &egui::Context) {
        let Some(p) = &self.preview else {
            return;
        };
        let docs = dirs_documents();
        let company = self
            .tenant
            .as_ref()
            .map(|(_, n)| n.clone())
            .unwrap_or_default();
        let start = docs.map(|d| cl_print::default_save_dir(&d, &company));
        self.save_rx = Some(crate::widgets::file_picker::save_file(
            ctx,
            crate::widgets::file_picker::PickUse::SavePdf,
            start.as_deref(),
            &p.file_name,
        ));
    }

    fn clear_messages(&mut self) {
        self.error = None;
        self.problem = None;
        self.notice = None;
    }

    fn poll(&mut self) {
        if let Some(r) = self.setup_task.as_mut().and_then(Task::try_take) {
            self.setup_task = None;
            self.loaded = true;
            match r {
                Ok(s) => {
                    self.logo = None;
                    self.logo_notice = false;
                    let hidden = self.tenant.as_ref().is_some_and(|(id, _)| {
                        crate::screens::company_info::logo_hidden(&self.link, *id)
                    });
                    if let Some(bytes) = s.logo.as_ref().filter(|_| !hidden) {
                        match Logo::from_bytes(bytes) {
                            Ok(l) => self.logo = Some(l),
                            Err(_) => self.logo_notice = true,
                        }
                    }
                    let pre_bank = None::<AccountId>;
                    self.bank = pre_bank.or_else(|| s.banks.first().map(|(id, _)| *id));
                    self.setup = s;
                    self.load_queue();
                }
                Err(e) => self.error = Some(e),
            }
        }
        if let Some(r) = self.queue_task.as_mut().and_then(Task::try_take) {
            self.queue_task = None;
            match r {
                Ok(q) => {
                    self.picked = if self.preselect.is_empty() {
                        q.checks.iter().map(|c| c.id).collect()
                    } else {
                        q.checks
                            .iter()
                            .filter(|c| self.preselect.contains(&c.id))
                            .map(|c| c.id)
                            .collect()
                    };
                    if self.first_number.trim().is_empty() || self.run.is_none() {
                        self.first_number = q.next.map(|n| n.to_string()).unwrap_or_default();
                    }
                    self.layout = q.layout;
                    self.setup_style = q.layout.format;
                    self.setup_block = q.layout.print_company_block;
                    self.checks = q.checks;
                    if !self.preselect.is_empty() && !self.picked.is_empty() {
                        self.preselect.clear();
                        self.preview_checks();
                    }
                }
                Err(e) => self.error = Some(e),
            }
        }
        if let Some(r) = self.forms_task.as_mut().and_then(Task::try_take) {
            self.forms_task = None;
            match r {
                Ok(f) => {
                    self.forms_picked = f.iter().map(|x| x.id).collect();
                    self.forms = f;
                }
                Err(ApiError::Local(cl_api::LocalValidation::Unsupported)) => {
                    self.forms_unavailable = true;
                }
                Err(e) => self.error = Some(e),
            }
        }
        if let Some(r) = self.build_task.as_mut().and_then(Task::try_take) {
            self.build_task = None;
            match r {
                Ok(b) => self.built(b),
                Err(e) => {
                    self.building = None;
                    self.run = None;
                    self.error = Some(e);
                }
            }
        }
        if let Some(r) = self.record_task.as_mut().and_then(Task::try_take) {
            self.record_task = None;
            self.recorded(r);
        }
        if let Some(r) = self.requeue_task.as_mut().and_then(Task::try_take) {
            self.requeue_task = None;
            self.requeued(r);
        }
        if let Some(r) = self.mark_task.as_mut().and_then(Task::try_take) {
            self.mark_task = None;
            match r {
                Ok(_) => {
                    self.preview = None;
                    self.load_forms();
                }
                Err(e) => self.error = Some(e),
            }
        }
        if let Some(r) = self.layout_task.as_mut().and_then(Task::try_take) {
            self.layout_task = None;
            match r {
                Ok(()) => {
                    self.notice = Some("print.setup.saved");
                    self.load_queue();
                }
                Err(e) => self.error = Some(e),
            }
        }
        if let Some(rx) = &self.printers_rx
            && let Ok((list, default)) = rx.try_recv()
        {
            self.printers_rx = None;
            if self.printer.as_ref().is_none_or(|p| !list.contains(p)) {
                self.printer = default.or_else(|| list.first().cloned());
            }
            self.printers = list;
            self.sync_offset_fields();
        }
        if let Some(rx) = &self.job
            && let Ok(s) = rx.try_recv()
        {
            self.job = None;
            self.sent = Some(s);
        }
        if let Some(path) = crate::widgets::file_picker::poll(&mut self.save_rx).flatten()
            && let Some(p) = &self.preview
        {
            self.write_rx = Some(job::save(path, Arc::clone(&p.pdf)));
        }
        if let Some(rx) = &self.write_rx
            && let Ok(ok) = rx.try_recv()
        {
            self.write_rx = None;
            self.notice = Some(if ok {
                "print.saved"
            } else {
                "print.save_failed"
            });
        }
    }

    fn busy(&self) -> bool {
        self.setup_task.is_some()
            || self.queue_task.is_some()
            || self.forms_task.is_some()
            || self.build_task.is_some()
            || self.record_task.is_some()
            || self.requeue_task.is_some()
            || self.mark_task.is_some()
            || self.layout_task.is_some()
            || self.printers_rx.is_some()
            || self.job.is_some()
            || self.save_rx.is_some()
            || self.write_rx.is_some()
    }

    fn state_json(&self) -> Value {
        json!({
            "screen": "print_forms",
            "kind": KIND,
            "loaded": self.loaded,
            "tab": match self.tab { Tab::Checks => "checks", Tab::Forms => "forms" },
            "banks": self.setup.banks.len(),
            "checks": self.checks.len(),
            "picked": self.picked.len(),
            "first_number": self.first_number,
            "style": self.layout.format.as_wire(),
            "company_block": self.layout.print_company_block,
            "forms": self.forms.len(),
            "forms_picked": self.forms_picked.len(),
            "forms_unavailable": self.forms_unavailable,
            "printers": self.printers.len(),
            "printer_chosen": self.printer.is_some(),
            "preview": self.preview.as_ref().map(|p| json!({
                "kind": p.kind.wire(),
                "pages": p.doc.pages.len(),
                "viewer": p.viewer.state_json(),
            })),
            "run": self.run.as_ref().map(|r| json!({
                "state": format!("{:?}", r.state()),
                "numbers": r.items().iter().filter_map(|i| i.number).collect::<Vec<_>>(),
            })),
            "asking": self.asking,
            "sent": self.sent.map(Sent::wire),
            "unanswered": self.has_unanswered(),
            "setup_open": self.setup_open,
            "offset": [self.offset_x, self.offset_y],
            "logo": self.logo.is_some(),
            "logo_notice": self.logo_notice,
            "closing": self.closing_prompt.state_json(kit::closing_supported(&self.link)),
            "notice": self.notice,
            "problem": self.problem.map(|p| format!("{p:?}")),
            "error": self.error.is_some(),
        })
    }

    // ---- drawing -----------------------------------------------------------

    fn draw(&mut self, ui: &mut Ui) {
        let tokens = Tokens::of(ui.ctx());
        ui.label(
            RichText::new(t("cmd.file.print_forms"))
                .size(22.0)
                .strong()
                .color(tokens.text_primary),
        );
        match self.closing_prompt.ui(ui, &tokens) {
            ClosingClick::Submit => self.submit_closing(),
            ClosingClick::Cancel => self.closing_prompt.cancel(),
            ClosingClick::None => {}
        }
        if let Some(k) = self.notice {
            form::hint(ui, &tokens, &t(k));
        }
        if let Some(p) = self.problem {
            form::inline_warning(
                ui,
                &tokens,
                &t(match p {
                    PrintProblem::NumberInUse => "print.problem.number_in_use",
                    PrintProblem::QueueChanged => "print.problem.queue_changed",
                    PrintProblem::NotPrinted => "print.problem.not_printed",
                }),
            );
        }
        if let Some(e) = &self.error {
            form::inline_error(ui, &tokens, e);
        }
        if self.logo_notice {
            form::inline_warning(ui, &tokens, &t("print.logo_notice"));
        }
        if let Some(s) = self.sent {
            form::hint(ui, &tokens, &t(s.key()));
        }
        if self.asking {
            self.confirm_ui(ui, &tokens);
            return;
        }
        if !self.loaded {
            form::progress_line(ui, &tokens, &t("print.loading"));
            return;
        }
        if self.has_unanswered() && self.preview.is_none() {
            let mut open = false;
            form::card(ui, &tokens, |ui| {
                ui.label(t("print.unanswered"));
                open = ui.button(t("print.unanswered.open")).clicked();
            });
            if open {
                self.reopen_question();
            }
        }
        if self.preview.is_some() {
            self.preview_ui(ui, &tokens);
            return;
        }
        if self.setup_open {
            self.setup_ui(ui, &tokens);
            ui.add_space(10.0);
        }
        ui.horizontal(|ui| {
            if ui
                .selectable_label(self.tab == Tab::Checks, t("print.tab.checks"))
                .clicked()
            {
                self.tab = Tab::Checks;
            }
            if ui
                .selectable_label(self.tab == Tab::Forms, t("print.tab.forms"))
                .clicked()
            {
                self.tab = Tab::Forms;
                if self.forms.is_empty() && self.forms_task.is_none() {
                    self.load_forms();
                }
            }
            if !self.setup_open && ui.button(t("cmd.file.printer_setup")).clicked() {
                self.setup_open = true;
            }
        });
        ui.add_space(8.0);
        egui::ScrollArea::vertical().show(ui, |ui| match self.tab {
            Tab::Checks => self.checks_ui(ui, &tokens),
            Tab::Forms => self.forms_ui(ui, &tokens),
        });
    }

    fn bank_combo(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let shown = self
            .bank
            .and_then(|b| self.setup.banks.iter().find(|(id, _)| *id == b))
            .map_or_else(|| t("print.bank.none"), |(_, n)| n.clone());
        let l = kit::above(ui, tokens, &t("print.bank"));
        let mut pick = None;
        egui::ComboBox::from_id_salt("print_forms/bank")
            .width(240.0)
            .selected_text(shown)
            .show_ui(ui, |ui| {
                for (id, name) in &self.setup.banks {
                    if ui.selectable_label(self.bank == Some(*id), name).clicked() {
                        pick = Some(*id);
                    }
                }
            })
            .response
            .labelled_by(l.id);
        if let Some(b) = pick
            && self.bank != Some(b)
        {
            self.bank = Some(b);
            self.first_number.clear();
            self.load_queue();
        }
    }

    fn checks_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        ui.horizontal_wrapped(|ui| {
            ui.vertical(|ui| self.bank_combo(ui, tokens));
            ui.vertical(|ui| {
                let l = kit::above(ui, tokens, &t("print.first_number"));
                ui.add(egui::TextEdit::singleline(&mut self.first_number).desired_width(90.0))
                    .labelled_by(l.id);
            });
            ui.vertical(|ui| {
                kit::above(ui, tokens, &t("print.style"));
                ui.label(t(style_key(self.layout.format)));
            });
        });
        ui.add_space(8.0);
        if self.queue_task.is_some() {
            form::progress_line(ui, tokens, &t("print.loading"));
            return;
        }
        if self.checks.is_empty() {
            form::hint(ui, tokens, &t("print.checks.none"));
            return;
        }
        let style = cl_core::MoneyStyle::default();
        egui::Grid::new("print_forms/checks")
            .striped(true)
            .show(ui, |ui| {
                ui.label("");
                ui.label(RichText::new(t("print.col.date")).strong());
                ui.label(RichText::new(t("print.col.payee")).strong());
                ui.label(RichText::new(t("print.col.amount")).strong());
                ui.end_row();
                for c in &self.checks {
                    let mut on = self.picked.contains(&c.id);
                    if ui
                        .checkbox(&mut on, "")
                        .on_hover_text(t("print.pick.tip"))
                        .labelled_by(ui.scope_id())
                        .changed()
                    {
                        if on {
                            self.picked.insert(c.id);
                        } else {
                            self.picked.remove(&c.id);
                        }
                    }
                    ui.label(crate::shell::us_date(c.date));
                    ui.label(&c.payee);
                    ui.label(
                        RichText::new(c.amount.format(style))
                            .text_style(egui::TextStyle::Monospace),
                    );
                    ui.end_row();
                }
            });
        ui.add_space(10.0);
        let n = self.picked.len();
        let label = t_with("print.preview_checks", &[("n", &n.to_string())]);
        if ui
            .add_enabled(n > 0 && self.build_task.is_none(), egui::Button::new(label))
            .clicked()
        {
            self.preview_checks();
        }
    }

    fn forms_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        if self.forms_unavailable {
            form::hint(ui, tokens, &t("print.forms.unavailable"));
            return;
        }
        if self.forms_task.is_some() {
            form::progress_line(ui, tokens, &t("print.loading"));
            return;
        }
        if self.forms.is_empty() {
            form::hint(ui, tokens, &t("print.forms.none"));
            return;
        }
        let style = cl_core::MoneyStyle::default();
        egui::Grid::new("print_forms/forms")
            .striped(true)
            .show(ui, |ui| {
                ui.label("");
                ui.label(RichText::new(t("print.col.type")).strong());
                ui.label(RichText::new(t("print.col.number")).strong());
                ui.label(RichText::new(t("print.col.date")).strong());
                ui.label(RichText::new(t("print.col.customer")).strong());
                ui.label(RichText::new(t("print.col.amount")).strong());
                ui.end_row();
                for f in &self.forms {
                    let mut on = self.forms_picked.contains(&f.id);
                    if ui
                        .checkbox(&mut on, "")
                        .on_hover_text(t("print.pick.tip"))
                        .labelled_by(ui.scope_id())
                        .changed()
                    {
                        if on {
                            self.forms_picked.insert(f.id);
                        } else {
                            self.forms_picked.remove(&f.id);
                        }
                    }
                    ui.label(job::kind_word(f.kind));
                    ui.label(f.number.clone().unwrap_or_default());
                    ui.label(crate::shell::us_date(f.date));
                    ui.label(f.customer.clone().unwrap_or_default());
                    ui.label(
                        RichText::new(f.total.map(|m| m.format(style)).unwrap_or_default())
                            .text_style(egui::TextStyle::Monospace),
                    );
                    ui.end_row();
                }
            });
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            let l = ui.label(t("print.copies"));
            ui.add(egui::TextEdit::singleline(&mut self.copies).desired_width(40.0))
                .labelled_by(l.id);
        });
        let n = self.forms_picked.len();
        let label = t_with("print.preview_forms", &[("n", &n.to_string())]);
        if ui
            .add_enabled(n > 0 && self.build_task.is_none(), egui::Button::new(label))
            .clicked()
        {
            self.preview_forms();
        }
    }

    fn printer_combo(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let l = kit::above(ui, tokens, &t("print.printer"));
        let shown = self
            .printer
            .clone()
            .unwrap_or_else(|| t("print.printer.none"));
        let mut pick = None;
        egui::ComboBox::from_id_salt("print_forms/printer")
            .width(240.0)
            .selected_text(shown)
            .show_ui(ui, |ui| {
                for p in &self.printers {
                    if ui
                        .selectable_label(self.printer.as_ref() == Some(p), p)
                        .clicked()
                    {
                        pick = Some(p.clone());
                    }
                }
            })
            .response
            .labelled_by(l.id);
        if let Some(p) = pick {
            self.printer = Some(p);
            self.sync_offset_fields();
        }
    }

    fn preview_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let kind = self.preview.as_ref().map(|p| p.kind);
        let mut act = None;
        ui.horizontal_wrapped(|ui| {
            ui.vertical(|ui| self.printer_combo(ui, tokens));
            if ui.button(t("print.print")).clicked() {
                act = Some("print");
            }
            if ui.button(t("print.save_pdf")).clicked() {
                act = Some("save");
            }
            if matches!(kind, Some(PreviewKind::Checks | PreviewKind::Forms))
                && ui.button(t("print.envelopes")).clicked()
            {
                act = Some("envelopes");
            }
            if ui.button(t("print.back")).clicked() {
                act = Some("back");
            }
        });
        if kind == Some(PreviewKind::Checks) {
            form::hint(ui, tokens, &t("print.preview.checks_hint"));
        }
        if kind == Some(PreviewKind::Envelopes) {
            form::hint(ui, tokens, &t("print.preview.envelopes_hint"));
        }
        if let Some(p) = self.preview.as_mut() {
            let h = ui.available_height().max(320.0);
            ui.allocate_ui(egui::vec2(ui.available_width(), h), |ui| {
                DocViewer::show(ui, &mut p.viewer, tokens);
            });
        }
        match act {
            Some("print") => self.print_clicked(),
            Some("save") => self.save_pdf(ui.ctx()),
            Some("envelopes") => self.preview_envelopes(),
            Some("back") => {
                self.preview = None;
                if self
                    .run
                    .as_ref()
                    .is_some_and(|r| r.state() == cl_print::RunState::Ready)
                {
                    self.run = None;
                }
            }
            _ => {}
        }
    }

    fn confirm_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let numbers: Vec<u64> = self
            .run
            .as_ref()
            .map(|r| r.items().iter().filter_map(|i| i.number).collect())
            .unwrap_or_default();
        let range = match (numbers.first(), numbers.last()) {
            (Some(a), Some(b)) if a != b => format!("{a}\u{2013}{b}"),
            (Some(a), _) => a.to_string(),
            _ => String::new(),
        };
        let mut act = None;
        form::card(ui, tokens, |ui| {
            form::heading(ui, tokens, &t("print.confirm.title"));
            ui.label(t_with("print.confirm.body", &[("range", &range)]));
            ui.add_space(6.0);
            if ui.button(t("print.confirm.all")).clicked() {
                act = Some(true);
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let l = ui.label(t("print.confirm.first_bad"));
                ui.add(egui::TextEdit::singleline(&mut self.first_bad).desired_width(90.0))
                    .labelled_by(l.id);
                if ui.button(t("print.confirm.reprint")).clicked() {
                    act = Some(false);
                }
            });
            form::hint(ui, tokens, &t("print.confirm.first_bad.hint"));
            ui.add_space(6.0);
            if ui.button(t("print.confirm.later")).clicked() {
                act = None;
                self.later();
            }
        });
        if let Some(all) = act {
            self.answer(all);
        }
        if let Some(p) = self.preview.as_mut() {
            ui.add_space(8.0);
            let h = ui.available_height().max(240.0);
            ui.allocate_ui(egui::vec2(ui.available_width(), h), |ui| {
                DocViewer::show(ui, &mut p.viewer, tokens);
            });
        }
    }

    fn setup_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let mut act = None;
        form::card(ui, tokens, |ui| {
            form::heading(ui, tokens, &t("cmd.file.printer_setup"));
            ui.horizontal_wrapped(|ui| {
                ui.vertical(|ui| self.printer_combo(ui, tokens));
                ui.vertical(|ui| self.bank_combo(ui, tokens));
            });
            ui.add_space(6.0);
            form::hint(ui, tokens, &t("print.setup.offset_hint"));
            ui.horizontal(|ui| {
                let l = ui.label(t("print.setup.right"));
                ui.add(egui::TextEdit::singleline(&mut self.offset_x).desired_width(60.0))
                    .labelled_by(l.id);
                let l = ui.label(t("print.setup.down"));
                ui.add(egui::TextEdit::singleline(&mut self.offset_y).desired_width(60.0))
                    .labelled_by(l.id);
            });
            ui.add_space(6.0);
            let l = kit::above(ui, tokens, &t("print.style"));
            let mut style = self.setup_style;
            egui::ComboBox::from_id_salt("print_forms/style")
                .selected_text(t(style_key(style)))
                .show_ui(ui, |ui| {
                    for f in [
                        CheckFormat::Voucher,
                        CheckFormat::Standard,
                        CheckFormat::ZFold,
                    ] {
                        ui.selectable_value(&mut style, f, t(style_key(f)));
                    }
                })
                .response
                .labelled_by(l.id);
            self.setup_style = style;
            ui.checkbox(&mut self.setup_block, t("print.setup.company_block"));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button(t("print.setup.test")).clicked() {
                    act = Some("test");
                }
                if ui.button(t("print.setup.save")).clicked() {
                    act = Some("save");
                }
                if ui.button(t("print.setup.close")).clicked() {
                    act = Some("close");
                }
            });
        });
        match act {
            Some("test") => self.preview_alignment(),
            Some("save") => self.save_setup(),
            Some("close") => self.setup_open = false,
            _ => {}
        }
    }
}

fn style_key(f: CheckFormat) -> &'static str {
    match f {
        CheckFormat::Standard => "print.style.standard",
        CheckFormat::ZFold => "print.style.z_fold",
        CheckFormat::Voucher | CheckFormat::CheckMiddle => "print.style.voucher",
    }
}

/// The owner's Documents folder.
fn dirs_documents() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(|h| PathBuf::from(h).join("Documents"))
}

impl Screen for PrintFormsScreen {
    fn title(&self) -> String {
        t("cmd.file.print_forms")
    }

    fn ui(&mut self, ui: &mut Ui) {
        self.poll();
        self.draw(ui);
        if self.busy() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    fn scope(&self) -> Scope {
        if self.preview.as_ref().is_some_and(|p| p.viewer.has_focus()) {
            Scope::Viewer
        } else {
            Scope::Window
        }
    }

    fn command(&mut self, id: CommandId) -> bool {
        if let Some(h) = self.closing_prompt.command(id) {
            return h;
        }
        if let Some(p) = self.preview.as_mut()
            && p.viewer.command(id)
        {
            return true;
        }
        match id.as_str() {
            KIND => true,
            SETUP_KIND => {
                self.setup_open = true;
                true
            }
            "file.print" => {
                if self.preview.is_some() {
                    self.print_clicked();
                } else if self.tab == Tab::Checks {
                    self.preview_checks();
                } else {
                    self.preview_forms();
                }
                true
            }
            // Demo only: the scenario runs light, then dark.
            "dev.demo_print_reset" => {
                let Some(mut d) = self.link.demo_state() else {
                    return false;
                };
                crate::company::demo::printing::reset(&mut d);
                drop(d);
                if let (Some(store), Some(c)) = (&self.unconfirmed, self.company_key()) {
                    let mut all = store.load();
                    let ids: Vec<String> = all
                        .for_company(&c)
                        .flat_map(|b| b.checks.iter().map(|x| x.check_id.clone()))
                        .collect();
                    let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
                    all.resolve(&c, &refs);
                    let _ = store.save(&all);
                }
                self.preview = None;
                self.run = None;
                self.asking = false;
                self.sent = None;
                self.first_number.clear();
                self.clear_messages();
                self.load_queue();
                self.forms.clear();
                true
            }
            _ => false,
        }
    }

    fn open_with(&mut self, payload: OpenPayload) -> bool {
        if let OpenPayload::PrintChecks(ids) = payload {
            self.preselect = ids;
            self.tab = Tab::Checks;
            if !self.checks.is_empty() {
                self.load_queue();
            }
            return true;
        }
        if let OpenPayload::ReprintCheck(id, date) = payload {
            self.tab = Tab::Checks;
            self.run_dates = vec![date];
            self.reprint = Some(id);
            self.requeue(vec![id], None);
            return true;
        }
        false
    }

    fn state(&self) -> Value {
        self.state_json()
    }
}

#[cfg(test)]
mod tests;
