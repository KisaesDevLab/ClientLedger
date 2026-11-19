//! Send to My Accountant (P32 D4): a `.clxfer` transfer file for the
//! accountant's MyBooks Online, following `SendToAccountant.html`
//! without the PROPOSED "Send directly" panel (Q-227).
//!
//! Deviations from the mockup (Q-P32q, listed in the PR): the passphrase
//! block and sharing instructions; the names, hidden-accounts and
//! waiting-documents lines; the payroll-draft prompt; the inline backup
//! passphrase when none is set (Q-P31ad).
//!
//! - Every engine call goes through the request queue ([`engine`]) or the
//!   lifecycle queue (the transfer itself); the demo answers at once.
//! - The passphrase lives in zeroizing [`SecretText`] buffers, cleared on
//!   save, on cancel and when the window closes; it is never in `state()`,
//!   logs or settings (Q-P32f).
//! - While a transfer runs, Close Company, Restore and Delete are disabled
//!   (Q-P32u, [`transfer_running`]).

#![allow(
    clippy::doc_markdown,
    reason = "MyBooks and ClientLedger are product names"
)]

mod draw;
#[cfg(test)]
mod tests;

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

use cl_api::{ApiError, Capabilities, RequestGroup, VerificationTotals};
use cl_company::transfer::{
    Preflight, TransferError, TransferOutcome, TransferRequest, check_passphrase,
    generate_passphrase, suggested_file_name,
};
use cl_company::{CompanyError, Progress};
use cl_core::CalendarDate;
use cl_core::ids::TenantId;
use cl_model::CommandId;
use cl_settings::TransferRecord;
use eframe::egui::Ui;
use serde_json::{Value, json};

use crate::company::{CompanyLink, Task, spawn_lifecycle};
use crate::i18n::t;
use crate::screens::company::form::SecretText;
use crate::screens::invoice::engine::{self, Job};
use crate::windows::Screen;

/// The window kind.
pub const KIND: &str = "company.send_to_accountant";

/// The commands a running transfer blocks (Q-P32u; P32 UI S1): everything
/// that closes or replaces the open company, or the app.
pub const BLOCKED_WHILE_SENDING: [&str; 8] = [
    "file.close_company",
    "file.restore",
    "file.delete_company",
    "file.sign_out",
    "file.open_company",
    "file.new_company",
    "file.open_practice",
    "file.quit",
];

/// True when `id` must wait for a running transfer.
#[must_use]
pub fn blocked_now(id: &str) -> bool {
    transfer_running() && BLOCKED_WHILE_SENDING.contains(&id)
}

#[cfg(not(test))]
static RUNNING: AtomicUsize = AtomicUsize::new(0);

// Unit tests run in parallel threads: each test thread counts its own
// transfers, so a held transfer never blocks another test's Quit.
#[cfg(test)]
thread_local! {
    static RUNNING: AtomicUsize = const { AtomicUsize::new(0) };
}

fn running_add(delta: isize) {
    #[cfg(not(test))]
    let apply = |f: &dyn Fn(&AtomicUsize)| f(&RUNNING);
    #[cfg(test)]
    let apply = |f: &dyn Fn(&AtomicUsize)| RUNNING.with(|r| f(r));
    apply(&|r| {
        if delta > 0 {
            r.fetch_add(1, Ordering::SeqCst);
        } else {
            r.fetch_sub(1, Ordering::SeqCst);
        }
    });
}

/// True while a transfer file is being made (any window).
#[must_use]
pub fn transfer_running() -> bool {
    #[cfg(not(test))]
    let n = RUNNING.load(Ordering::SeqCst);
    #[cfg(test)]
    let n = RUNNING.with(|r| r.load(Ordering::SeqCst));
    n > 0
}

/// Counts a running transfer for as long as it lives.
pub(crate) struct Sending;

impl Sending {
    pub(crate) fn start() -> Sending {
        running_add(1);
        Sending
    }
}

impl Drop for Sending {
    fn drop(&mut self) {
        running_add(-1);
    }
}

/// The window for `kind`, if it is this one.
#[must_use]
pub fn make(
    kind: CommandId,
    link: &Arc<CompanyLink>,
    open: Option<(TenantId, String)>,
    today: CalendarDate,
) -> Option<Box<dyn Screen>> {
    (kind.as_str() == KIND).then(|| Box::new(SendScreen::new(Arc::clone(link), open, today)) as _)
}

/// Where the screen stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ask {
    /// Nothing asked.
    None,
    /// "Discard N unposted payroll imports first?"
    Payroll,
    /// The Save dialog is open.
    SaveDialog,
}

/// The Send to My Accountant window.
pub struct SendScreen {
    link: Arc<CompanyLink>,
    group: Option<RequestGroup>,
    today: CalendarDate,
    tenant: Option<(TenantId, String)>,
    preview: Option<VerificationTotals>,
    preview_job: Option<Job<VerificationTotals>>,
    preview_error: Option<ApiError>,
    preflight: Option<Preflight>,
    preflight_job: Option<Job<Preflight>>,
    file_name: String,
    passphrase: SecretText,
    again: SecretText,
    /// A passphrase made by "Make one for me" (never in `state()`).
    generated: bool,
    /// The generated passphrase is shown (Q-P32w: hidden by default; Copy
    /// covers sharing).
    show_generated: bool,
    /// The result or the missing-file list just arrived: scroll to it.
    scroll_result: bool,
    /// A demo transfer held open by `dev.transfer_demo_hold` (scenarios
    /// check what a running transfer blocks).
    held: Option<(TransferRequest, Option<cl_core::Secret>, Sending)>,
    hold_next: bool,
    backup_needed: bool,
    backup_ask: crate::widgets::passphrase::PassphraseAsk,
    ask: Ask,
    payroll_agreed: bool,
    save_rx: Option<Receiver<Option<PathBuf>>>,
    task: Option<Task<Result<TransferOutcome, TransferError>>>,
    progress: Arc<Mutex<Option<Progress>>>,
    outcome: Option<TransferOutcome>,
    error: Option<TransferError>,
    message: Option<&'static str>,
    history: Vec<TransferRecord>,
}

fn api_of(e: TransferError) -> ApiError {
    match e {
        TransferError::Company(CompanyError::Api(a)) => a,
        _ => ApiError::Local(cl_api::LocalValidation::InvalidValue),
    }
}

impl SendScreen {
    /// The window over the open company: reads the preview totals and
    /// what to ask before a transfer.
    #[must_use]
    pub fn new(
        link: Arc<CompanyLink>,
        tenant: Option<(TenantId, String)>,
        today: CalendarDate,
    ) -> SendScreen {
        let group = link.group().and_then(Result::ok);
        let backup_needed = crate::widgets::passphrase::needed(&link);
        let file_name = suggested_file_name(
            tenant.as_ref().map_or("Company", |(_, n)| n.as_str()),
            today,
        );
        let mut s = SendScreen {
            link,
            group,
            today,
            tenant,
            preview: None,
            preview_job: None,
            preview_error: None,
            preflight: None,
            preflight_job: None,
            file_name,
            passphrase: SecretText::default(),
            again: SecretText::default(),
            generated: false,
            show_generated: false,
            scroll_result: false,
            held: None,
            hold_next: false,
            backup_needed,
            backup_ask: crate::widgets::passphrase::PassphraseAsk::default(),
            ask: Ask::None,
            payroll_agreed: false,
            save_rx: None,
            task: None,
            progress: Arc::new(Mutex::new(None)),
            outcome: None,
            error: None,
            message: None,
            history: Vec::new(),
        };
        s.reload();
        s
    }

    fn capable(&self, cap: &str) -> bool {
        self.link.is_demo() || self.link.capabilities().has(cap)
    }

    /// The engine can make transfer files.
    fn supported(&self) -> bool {
        self.capable(Capabilities::TENANT_ARCHIVE_TRANSFER)
    }

    fn reload(&mut self) {
        self.load_history();
        let Some((tenant, _)) = self.tenant.clone() else {
            return;
        };
        let today = self.today;
        if self.capable(Capabilities::ARCHIVE_VERIFICATION_TOTALS) {
            self.preview_job = Some(engine::read(
                &self.link,
                self.group.as_ref(),
                move |d| Ok(crate::company::demo::transfer::preview(d, today)),
                move |s| async move { s.tenants().verification_totals(tenant, today, true).await },
            ));
        }
        self.preflight_job = Some(engine::read(
            &self.link,
            self.group.as_ref(),
            |d| Ok(crate::company::demo::transfer::preflight(d)),
            |s| async move { cl_company::transfer::preflight(&s).await.map_err(api_of) },
        ));
        self.poll();
    }

    fn load_history(&mut self) {
        if let Some(d) = self.link.demo_state() {
            self.history = d.transfer.history.clone();
            return;
        }
        let Some((tenant, _)) = self.tenant else {
            return;
        };
        self.history = self
            .link
            .store()
            .load()
            .ok()
            .and_then(|l| l.get(tenant).map(|c| c.transfers.clone()))
            .unwrap_or_default();
    }

    fn poll(&mut self) {
        if let Some(r) = engine::poll(&mut self.preview_job) {
            match r {
                Ok(p) => self.preview = Some(p),
                Err(e) => self.preview_error = Some(e),
            }
        }
        if let Some(Ok(p)) = engine::poll(&mut self.preflight_job) {
            self.preflight = Some(p);
        }
        if let Some(rx) = &self.save_rx {
            match rx.try_recv() {
                Ok(Some(path)) => {
                    self.save_rx = None;
                    self.ask = Ask::None;
                    self.start(path);
                }
                Ok(None) | Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.save_rx = None;
                    self.ask = Ask::None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
        }
        if let Some(task) = self.task.as_mut()
            && let Some(r) = task.poll_result()
        {
            self.task = None;
            match r {
                Ok(o) => {
                    tracing::info!("transfer file saved");
                    self.outcome = Some(o);
                    self.scroll_result = true;
                    self.clear_secrets();
                    self.payroll_agreed = false;
                    self.load_history();
                    // Waiting drafts were discarded; the counts change.
                    self.preflight_job = Some(engine::read(
                        &self.link,
                        self.group.as_ref(),
                        |d| Ok(crate::company::demo::transfer::preflight(d)),
                        |s| async move { cl_company::transfer::preflight(&s).await.map_err(api_of) },
                    ));
                }
                Err(e) => {
                    tracing::warn!(detail = %cl_core::OwnerFacing::scrubbed_detail(&e), "transfer refused");
                    self.scroll_result = matches!(e.kind(), TransferError::MissingFiles(_));
                    self.error = Some(e);
                }
            }
        }
    }

    fn clear_secrets(&mut self) {
        self.passphrase.clear();
        self.again.clear();
        self.generated = false;
        self.show_generated = false;
    }

    /// "Make one for me": both boxes filled.
    fn make_one(&mut self) {
        match generate_passphrase() {
            Ok(p) => {
                self.passphrase = SecretText::from(p.expose());
                self.again = SecretText::from(p.expose());
                self.generated = true;
                self.show_generated = false;
                self.error = None;
            }
            Err(e) => self.error = Some(e),
        }
    }

    /// Save transfer file…: the checks, then the questions, then the Save
    /// dialog.
    fn save(&mut self, ctx: Option<&eframe::egui::Context>) {
        self.error = None;
        self.message = None;
        self.outcome = None;
        if self.task.is_some() {
            return;
        }
        if !self.supported() {
            self.error = Some(TransferError::Unsupported);
            return;
        }
        let secrets = self.link.secrets();
        if let Err(e) = check_passphrase(
            &self.passphrase.to_secret(),
            &self.again.to_secret(),
            secrets.as_ref(),
        ) {
            self.error = Some(e);
            return;
        }
        if self.backup_needed {
            if !self.backup_ask.save(&self.link) {
                return;
            }
            self.backup_needed = false;
        }
        let waiting = self.preflight.as_ref().map_or(0, |p| p.unposted_payroll);
        if waiting > 0 && !self.payroll_agreed {
            self.ask = Ask::Payroll;
            return;
        }
        self.ask = Ask::SaveDialog;
        if let Some(ctx) = ctx {
            self.save_rx = Some(crate::widgets::file_picker::save_file_as(
                ctx,
                crate::widgets::file_picker::PickUse::SaveTransfer,
                Some(&self.link.backup_dir),
                &self.file_name,
                ("picker.filter.clxfer", cl_company::transfer::EXTENSION),
            ));
        } else {
            // Tests: straight into the backup folder.
            let path = self.link.backup_dir.join(&self.file_name);
            self.ask = Ask::None;
            self.start(path);
        }
    }

    fn answer_payroll(&mut self, discard: bool, ctx: Option<&eframe::egui::Context>) {
        self.ask = Ask::None;
        if discard {
            self.payroll_agreed = true;
            self.save(ctx);
        }
    }

    fn start(&mut self, path: PathBuf) {
        let Some((tenant_id, _)) = self.tenant.clone() else {
            self.error = Some(CompanyError::UnknownCompany.into());
            return;
        };
        let dest = if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case(cl_company::transfer::EXTENSION))
        {
            path
        } else {
            path.with_extension(cl_company::transfer::EXTENSION)
        };
        let req = TransferRequest {
            tenant_id,
            dest,
            passphrase: self.passphrase.to_secret(),
            passphrase_again: self.again.to_secret(),
            discard_payroll: self.payroll_agreed,
            backup_dir: self.link.backup_dir.clone(),
            as_of: self.today,
        };
        if self.link.is_demo() {
            let backup = self
                .link
                .secrets()
                .get(cl_settings::secrets::BACKUP_PASSPHRASE)
                .ok()
                .flatten();
            let demo = if self.hold_next {
                None
            } else {
                self.link.demo_state().map(|mut d| {
                    crate::company::demo::transfer::make(&mut d, &req, backup.as_ref())
                })
            };
            if self.hold_next {
                self.hold_next = false;
                self.held = Some((req, backup, Sending::start()));
                return;
            }
            if let Some(r) = demo {
                self.task = Some(Task::ready(r));
                self.poll();
                return;
            }
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            self.error = Some(CompanyError::Api(ApiError::NoCompanyOpen).into());
            return;
        };
        let store = self.link.store().clone();
        let secrets = self.link.secrets();
        let slot = Arc::clone(&self.progress);
        let sink: cl_company::ProgressSink = Arc::new(move |p| {
            if let Ok(mut s) = slot.lock() {
                *s = Some(p);
            }
        });
        let sending = Sending::start();
        self.task = spawn_lifecycle(&self.link, group, move || async move {
            let _sending = sending;
            cl_company::transfer::make_transfer(&session, &store, secrets.as_ref(), &req, &sink)
                .await
        })
        .ok();
        if self.task.is_none() {
            self.error =
                Some(CompanyError::Api(ApiError::EngineUnavailable { permanent: false }).into());
        }
    }

    fn open_record(&self, record_type: Option<&str>, record_id: Option<&str>) {
        let (Some(kind), Some(id)) = (record_type, record_id) else {
            return;
        };
        let form = crate::screens::reports::drill::form_of(kind).or(match kind {
            "expense" => Some(crate::screens::write_check::KIND),
            _ => None,
        });
        if let (Some(form), Ok(id)) = (form, cl_core::ids::TransactionId::parse(id)) {
            self.link.push(crate::company::CompanyEvent::OpenRecord(
                CommandId::new(form),
                id,
            ));
        }
    }

    fn open_iif(&self) {
        self.link
            .push(crate::company::CompanyEvent::OpenWindow(CommandId::new(
                crate::screens::iif_export::KIND,
            )));
    }

    fn error_key(&self) -> Option<&'static str> {
        self.error
            .as_ref()
            .map(|e| cl_core::OwnerFacing::owner_message(e).key)
    }
}

impl Drop for SendScreen {
    fn drop(&mut self) {
        // Q-P32f: the buffers are wiped when the window closes.
        self.clear_secrets();
    }
}

impl Screen for SendScreen {
    fn title(&self) -> String {
        t("cmd.company.send_to_accountant")
    }

    fn dirty(&self) -> bool {
        false
    }

    fn can_save(&self) -> bool {
        false
    }

    fn ui(&mut self, ui: &mut Ui) {
        self.poll();
        if self.preview_job.is_some()
            || self.preflight_job.is_some()
            || self.task.is_some()
            || self.save_rx.is_some()
        {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
        }
        self.backup_ask.refresh(&self.link, &mut self.backup_needed);
        draw::screen(self, ui);
    }

    fn command(&mut self, id: CommandId) -> bool {
        use crate::company::demo::transfer::DemoTransferFailure as F;
        if !self.link.is_demo() {
            return false;
        }
        match id.as_str() {
            "dev.transfer_demo_hold" => {
                self.hold_next = true;
                return true;
            }
            "dev.transfer_demo_release" => {
                let Some((req, backup, sending)) = self.held.take() else {
                    return false;
                };
                let r = self.link.demo_state().map(|mut d| {
                    crate::company::demo::transfer::make(&mut d, &req, backup.as_ref())
                });
                drop(sending);
                if let Some(r) = r {
                    self.task = Some(Task::ready(r));
                    self.poll();
                }
                return true;
            }
            "dev.transfer_demo_ask_backup_passphrase" => {
                self.backup_needed = true;
                return true;
            }
            _ => {}
        }
        let handled = self.link.demo_state().is_some_and(|mut d| {
            match id.as_str() {
                "dev.transfer_demo_busy" => d.transfer.fail_next = F::Busy,
                "dev.transfer_demo_missing_files" => d.transfer.fail_next = F::MissingFiles,
                "dev.transfer_demo_post" => d.transfer.posted_since_preview = true,
                "dev.transfer_demo_payroll_draft" => d.transfer.payroll_drafts += 1,
                "dev.transfer_demo_waiting_documents" => d.transfer.waiting_documents = 3,
                _ => return false,
            }
            true
        });
        if handled {
            self.reload_preflight_only();
        }
        handled
    }

    fn state(&self) -> Value {
        let missing = match self.error.as_ref().map(TransferError::kind) {
            Some(TransferError::MissingFiles(f)) => f.len(),
            _ => 0,
        };
        json!({
            "screen": "send_to_accountant",
            "supported": self.supported(),
            "preview_loaded": self.preview.is_some(),
            "total_assets": self.preview.as_ref().map(|p| p.total_assets.to_wire()),
            "unposted_payroll": self.preflight.as_ref().map(|p| p.unposted_payroll),
            "waiting_documents": self.preflight.as_ref().map(|p| p.waiting_documents),
            "payroll_prompt": self.ask == Ask::Payroll,
            "save_dialog": self.ask == Ask::SaveDialog,
            "backup_passphrase_needed": self.backup_needed,
            "passphrase_filled": !self.passphrase.is_empty(),
            "generated": self.generated,
            "shown": self.show_generated,
            "running": self.task.is_some() || self.held.is_some(),
            "done": self.outcome.is_some(),
            "totals_differ": self.totals_differ(),
            "error": self.error_key(),
            "missing_files": missing,
            "discarded_before_error": self.error.as_ref().and_then(TransferError::discarded).map(|d| d.count),
            "history": self.history.len(),
            "blocked": transfer_running(),
        })
    }
}

impl SendScreen {
    fn reload_preflight_only(&mut self) {
        self.preflight_job = Some(engine::read(
            &self.link,
            self.group.as_ref(),
            |d| Ok(crate::company::demo::transfer::preflight(d)),
            |s| async move { cl_company::transfer::preflight(&s).await.map_err(api_of) },
        ));
        self.poll();
    }
    /// Q-P32l: the books in the file differ from the preview shown before
    /// (money and counts; the audit entries the backup and the payroll
    /// discard add are not a change, review B1).
    fn totals_differ(&self) -> bool {
        match (
            self.preview.as_ref(),
            self.outcome.as_ref().and_then(|o| o.totals.as_ref()),
        ) {
            (Some(p), Some(f)) => p.books_differ(f),
            _ => false,
        }
    }
}

/// Review S7: "N payroll imports were already discarded; the backup made
/// at <time> still holds them."
pub(super) fn discard_note(d: &cl_company::transfer::Discarded) -> String {
    let local = d.backup_at.as_utc().with_timezone(&chrono::Local);
    crate::i18n::t_count(
        "send.error.discarded",
        usize::try_from(d.count).unwrap_or(usize::MAX),
        &[
            ("n", &d.count.to_string()),
            ("time", &local.format("%m/%d/%Y %I:%M %p").to_string()),
        ],
    )
}
