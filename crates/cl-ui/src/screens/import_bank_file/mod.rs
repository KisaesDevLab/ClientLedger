//! Import Bank File (P25, `banking.import_file`, mockup `ImportBankFile`):
//! a four-step wizard that brings a bank's downloaded file into the Bank
//! Feed. Nothing connects to the bank.
//!
//! 1. **Choose file.** Browse picks a QFX, OFX, QIF or CSV file (at most
//!    25 MB). It is read and parsed off the egui thread by `cl-import`;
//!    errors name their line.
//! 2. **Link account.** The section (a file may hold several accounts,
//!    Q-P25i), the account it goes into — found by the local engine from
//!    the file's account number (the engine keeps the link key, Q-P25u),
//!    then suggested by its mask, else picked — the CSV columns (Q-P25w;
//!    a remembered layout only when it fits, an owner's choice always
//!    wins) and the sign: asked once with a preview when the file looks
//!    reversed, and always on a card account's first import of a format;
//!    the answer is kept with the batch (Q-196, Q-P25t).
//! 3. **Check transactions.** The engine's dry run (U-06): each row's
//!    status, the counts, the closed-period flag and the balance check. A
//!    possible duplicate can be added anyway; a row the bank changed can
//!    replace the waiting item or be added as new. Any change reruns the
//!    dry run.
//! 4. **Send to Bank Feed.** The commit, under one key made at the first
//!    Send for that exact body (a retry after a lost answer reuses it, a
//!    changed body gets a new one). The result shows the counts and
//!    "Preparing suggestions…" while the batch's preparation runs, polled
//!    off the egui thread with backoff within the Import budget (Q-P25v);
//!    the owner may close early (the rows are already in the feed). Undo
//!    this import opens once it is prepared.
//!
//! Alt+N goes to the next step (it is Save & New's key in forms) and Alt+B
//! back. Amounts are shown as the file has them, with money out negative;
//! nothing is added up here. The control channel's state carries counts,
//! steps and flags only: never amounts, names, masks or the file name.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::Receiver;

use cl_api::{
    Account, AccountFilter, AccountType, ApiError, BankImportOutcome, BankImportRefusal,
    BankImportRequest, BankLookupBy, BankLookupVia, Capabilities, ChangedAction, ConnectionMatch,
    IdempotencyKey, ImportBatchSummary, ImportPostProcessing, ImportRowStatus, ImportUndoOutcome,
    OrphanLabel, RequestGroup, bank_import_refusal,
};
use cl_core::CalendarDate;
use cl_core::ids::{AccountId, TenantId};
use cl_import::{CsvMapping, SignCheck};
use cl_model::CommandId;
use serde_json::{Value, json};

use crate::company::{CompanyLink, Task};
use crate::i18n::t;
use crate::windows::Screen;

mod draw;
pub mod flow;
#[cfg(test)]
mod tests;

use flow::{Loaded, ReadFail, RowChoice};

/// The command and window kind.
pub const KIND: &str = "banking.import_file";

/// The Import Bank File window for `kind`, if it is one.
#[must_use]
pub fn make(
    kind: CommandId,
    link: &Arc<CompanyLink>,
    open: Option<(TenantId, String)>,
    today: CalendarDate,
) -> Option<Box<dyn Screen>> {
    (kind.as_str() == KIND).then(|| {
        Box::new(ImportBankFileScreen::new(
            Arc::clone(link),
            open.map(|o| o.0),
            today,
        )) as Box<_>
    })
}

/// The wizard's steps.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Step {
    /// Choose file.
    Choose,
    /// Link account.
    Link,
    /// Check transactions.
    Check,
    /// Sent: the result.
    Done,
}

impl Step {
    fn as_str(self) -> &'static str {
        match self {
            Step::Choose => "choose",
            Step::Link => "link",
            Step::Check => "check",
            Step::Done => "done",
        }
    }
}

/// How the account was found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkedHow {
    /// Not chosen yet.
    None,
    /// By the file's keyed identity (an earlier import on this computer).
    Auto,
    /// Suggested by the mask (check it).
    Mask,
    /// Remembered by the last import of this kind.
    Owner,
}

/// Where the CSV columns came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CsvSource {
    /// Guessed from the header and the values.
    Guess,
    /// The account's latest import of this format (the company's; Q-P25w).
    Batch,
    /// A layout saved on this computer (Q-P25f).
    Local,
    /// Chosen by the owner in this window: never replaced (review DB2).
    Owner,
}

impl CsvSource {
    fn as_str(self) -> &'static str {
        match self {
            CsvSource::Guess => "guess",
            CsvSource::Batch => "batch",
            CsvSource::Local => "local",
            CsvSource::Owner => "owner",
        }
    }
}

/// How long the wizard waits for an import's preparation (the Import
/// time budget, Q-P25v).
const PREPARING_BUDGET: std::time::Duration = std::time::Duration::from_secs(120);
/// The longest pause between two checks of the preparation.
const PREPARING_MAX_DELAY: std::time::Duration = std::time::Duration::from_secs(10);
/// After the budget, the preparation is still checked this often, so Undo
/// opens when it is done (Q-P25ac).
const PREPARING_SLOW_DELAY: std::time::Duration = std::time::Duration::from_secs(30);
/// How often an earlier import still being prepared is checked again.
const EARLIER_DELAY: std::time::Duration = std::time::Duration::from_secs(2);

/// A bank or card account the file may go into.
#[derive(Clone, Debug)]
pub struct Target {
    id: AccountId,
    name: String,
    card: bool,
}

/// A body frozen at its first Send, with its key.
#[derive(Clone)]
struct Frozen {
    request: BankImportRequest,
    key: IdempotencyKey,
}

/// What went wrong talking to the engine (shown in the step).
#[derive(Clone, Debug, PartialEq)]
enum EngineTrouble {
    /// A refusal the screen explains.
    Refused(BankImportRefusal),
    /// The answer was lost: send again (the same key; nothing is added
    /// twice).
    Unknown,
    /// Anything else (i18n key).
    Other(&'static str),
}

/// The Import Bank File window.
pub struct ImportBankFileScreen {
    link: Arc<CompanyLink>,
    group: Option<RequestGroup>,
    tenant: Option<TenantId>,
    today: CalendarDate,
    step: Step,
    // Step 1.
    pick_rx: Option<Receiver<Vec<PathBuf>>>,
    reading: Option<Receiver<Result<Loaded, ReadFail>>>,
    loaded: Option<Loaded>,
    read_error: Option<ReadFail>,
    // Step 2.
    section: usize,
    targets: Vec<Target>,
    targets_task: Option<Task<Vec<Account>>>,
    targets_failed: bool,
    account: Option<AccountId>,
    linked: LinkedHow,
    lookup_task: Option<Task<Vec<ConnectionMatch>>>,
    history_task: Option<Task<Vec<ImportBatchSummary>>>,
    /// The account's last import of this format: its flip (Q-P25w).
    remembered_flip: Option<bool>,
    /// The sign checks for the chosen account (the card rule for a card).
    sign: SignCheck,
    flipped: bool,
    sign_confirmed: bool,
    csv_mapping: Option<CsvMapping>,
    csv_source: CsvSource,
    /// The mapping a reparse in flight applies, and where it came from.
    reparse_as: Option<(CsvMapping, CsvSource)>,
    /// A remembered layout did not fit this file and the columns were
    /// guessed again (QA-6).
    layout_changed: bool,
    // Step 3.
    choices: BTreeMap<u32, RowChoice>,
    relink: bool,
    preview_task: Option<Task<BankImportOutcome>>,
    preview: Option<BankImportOutcome>,
    preview_trouble: Option<EngineTrouble>,
    only_skipped: bool,
    // Step 4.
    frozen: Option<Frozen>,
    send_task: Option<Task<BankImportOutcome>>,
    send_trouble: Option<EngineTrouble>,
    result: Option<BankImportOutcome>,
    prep_task: Option<Task<Vec<ImportBatchSummary>>>,
    /// When the preparation began to be watched, and the next check.
    prep_started: Option<std::time::Instant>,
    prep_next: Option<std::time::Instant>,
    prep_delay: std::time::Duration,
    /// The budget ran out: the preparation goes on in the engine and is
    /// checked every 30 seconds (Q-P25ac).
    prep_gave_up: bool,
    /// Items prepared and items in all, when the engine says (Q-P25ad).
    prep_progress: Option<(u32, u32)>,
    undo_task: Option<Task<ImportUndoOutcome>>,
    undo_key: Option<IdempotencyKey>,
    undone: Option<ImportUndoOutcome>,
    undo_trouble: Option<EngineTrouble>,
    /// The undo was sent while the import was still being prepared: it
    /// stops the preparation (engine PR #234).
    undo_cancels: bool,
    /// An import sent from an earlier window and still being prepared
    /// when this one opened (P25 follow-up 6), checked until it is done.
    earlier: Option<ImportBatchSummary>,
    earlier_task: Option<Task<Vec<ImportBatchSummary>>>,
    earlier_next: Option<std::time::Instant>,
    /// Imports sent from this window (control channel).
    sent: u32,
    /// The fixed frame's sizes last frame (control channel).
    layout: crate::layout::FrameReport,
    /// The Bank Feed's count after a send or an undo, for the rail badge.
    feed_count_task: Option<Task<u64>>,
}

impl ImportBankFileScreen {
    /// A fresh wizard on step 1.
    #[must_use]
    pub fn new(
        link: Arc<CompanyLink>,
        tenant: Option<TenantId>,
        today: CalendarDate,
    ) -> ImportBankFileScreen {
        let group = link.group().and_then(Result::ok);
        let mut s = ImportBankFileScreen {
            link,
            group,
            tenant,
            today,
            step: Step::Choose,
            pick_rx: None,
            reading: None,
            loaded: None,
            read_error: None,
            section: 0,
            targets: Vec::new(),
            targets_task: None,
            targets_failed: false,
            account: None,
            linked: LinkedHow::None,
            lookup_task: None,
            history_task: None,
            remembered_flip: None,
            sign: SignCheck::default(),
            flipped: false,
            sign_confirmed: false,
            csv_mapping: None,
            csv_source: CsvSource::Guess,
            reparse_as: None,
            layout_changed: false,
            choices: BTreeMap::new(),
            relink: false,
            preview_task: None,
            preview: None,
            preview_trouble: None,
            only_skipped: false,
            frozen: None,
            send_task: None,
            send_trouble: None,
            result: None,
            prep_task: None,
            prep_started: None,
            prep_next: None,
            prep_delay: std::time::Duration::from_secs(1),
            prep_gave_up: false,
            prep_progress: None,
            undo_task: None,
            undo_key: None,
            undone: None,
            undo_trouble: None,
            undo_cancels: false,
            earlier: None,
            earlier_task: None,
            earlier_next: None,
            sent: 0,
            layout: crate::layout::FrameReport::default(),
            feed_count_task: None,
        };
        s.load_targets();
        s.check_earlier();
        s
    }

    /// Whether the engine reports the preparation's progress and lets an
    /// import be undone while it is prepared (`bank-import-progress-v1`;
    /// the demo does both).
    pub(crate) fn progress_caps(&self) -> bool {
        self.link.is_demo()
            || self
                .link
                .capabilities()
                .has(Capabilities::BANK_IMPORT_PROGRESS)
    }

    /// Reads the imports again for an earlier one still being prepared
    /// (P25 follow-up 6): the newest not undone that is still scheduled,
    /// or, once one was found, that one.
    fn check_earlier(&mut self) {
        self.earlier_next = None;
        let link = Arc::clone(&self.link);
        if let Some(demo) = link.demo_state() {
            let batches = demo.bank_feed.summaries();
            drop(demo);
            self.take_earlier(batches);
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        self.earlier_task = group
            .read(&session, |s| async move {
                s.bank_import().batches(None, None).await
            })
            .ok()
            .map(Task::Pending);
    }

    fn take_earlier(&mut self, batches: Vec<ImportBatchSummary>) {
        let found = match self.earlier.as_ref() {
            Some(e) => batches.into_iter().find(|b| b.id == e.id),
            None => batches.into_iter().find(|b| {
                b.undone_at.is_none() && b.post_processing == Some(ImportPostProcessing::Scheduled)
            }),
        };
        // Done or failed stays on show; undone elsewhere (or gone) goes.
        self.earlier = found.filter(|b| b.undone_at.is_none());
        if self
            .earlier
            .as_ref()
            .is_some_and(|b| b.post_processing == Some(ImportPostProcessing::Scheduled))
        {
            self.earlier_next = Some(std::time::Instant::now() + EARLIER_DELAY);
        }
    }

    fn poll_earlier(&mut self) {
        if let Some(r) = self.earlier_task.as_mut().and_then(Task::try_take) {
            self.earlier_task = None;
            if let Ok(batches) = r {
                self.take_earlier(batches);
            }
        }
        if self.earlier_task.is_none()
            && self
                .earlier_next
                .is_some_and(|n| std::time::Instant::now() >= n)
        {
            self.check_earlier();
        }
    }

    /// The earlier import still being prepared, or just prepared, for the
    /// note on steps 1 and 2.
    pub(crate) fn earlier(&self) -> Option<&ImportBatchSummary> {
        self.earlier.as_ref()
    }

    // ---- Accounts ---------------------------------------------------------

    fn load_targets(&mut self) {
        self.targets_failed = false;
        if self.link.is_demo() {
            self.targets_task = Some(Task::ready(crate::company::demo::chart()));
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            self.targets_failed = true;
            return;
        };
        self.targets_task = group
            .read(&session, |s| async move {
                s.accounts().list_all(&AccountFilter::default()).await
            })
            .ok()
            .map(Task::Pending);
        self.targets_failed = self.targets_task.is_none();
    }

    fn take_targets(&mut self, accounts: Vec<Account>) {
        let mut v: Vec<Target> = accounts
            .into_iter()
            .filter(|a| a.is_active)
            .filter_map(|a| {
                let detail = a.fields.detail_type.as_deref();
                let card = match (a.fields.account_type, detail) {
                    (AccountType::Asset, Some("bank")) => false,
                    (AccountType::Liability, Some("credit_card")) => true,
                    _ => return None,
                };
                let shown = crate::account_names::shown_name(&a);
                let name = match a.fields.account_number.as_deref() {
                    Some(n) if !n.trim().is_empty() => format!("{} \u{b7} {shown}", n.trim()),
                    _ => shown,
                };
                Some(Target {
                    id: a.id,
                    name,
                    card,
                })
            })
            .collect();
        v.sort_by_key(|t| t.name.to_lowercase());
        self.targets = v;
    }

    // ---- Step 1: the file -------------------------------------------------

    /// Opens the system file window (answered by `dialog_answer` under the
    /// control channel).
    pub(crate) fn browse(&mut self, ctx: &eframe::egui::Context) {
        use crate::widgets::file_picker::{BANK_FILES, PickUse, pick_files};
        self.pick_rx = Some(pick_files(ctx, PickUse::BankFile, &[BANK_FILES], false));
    }

    /// Reads `path` on a worker thread.
    pub(crate) fn read(&mut self, path: PathBuf, ctx: Option<&eframe::egui::Context>) {
        let today = self.today;
        let (tx, rx) = std::sync::mpsc::channel();
        let repaint = ctx.cloned();
        let spawned = std::thread::Builder::new()
            .name("cl-bank-file".into())
            .spawn(move || {
                let _ = tx.send(flow::read_file(&path, today, None));
                if let Some(c) = repaint {
                    c.request_repaint();
                }
            });
        if spawned.is_ok() {
            self.reading = Some(rx);
            self.read_error = None;
        } else {
            self.read_error = Some(ReadFail::Open);
        }
    }

    /// Reads the loaded CSV again with `mapping` from `source`. A
    /// remembered mapping is used only when it fits the file (review DB2).
    fn reparse(&mut self, mapping: CsvMapping, source: CsvSource) {
        let Some(l) = self.loaded.as_ref() else {
            return;
        };
        let (name, bytes, today) = (l.file_name.clone(), Arc::clone(&l.bytes), self.today);
        let remembered = matches!(source, CsvSource::Batch | CsvSource::Local);
        let (tx, rx) = std::sync::mpsc::channel();
        let m = mapping.clone();
        if std::thread::Builder::new()
            .name("cl-bank-file".into())
            .spawn(move || {
                let _ = tx.send(flow::parse_bytes(name, bytes, today, Some(m), remembered));
            })
            .is_ok()
        {
            self.reading = Some(rx);
            self.reparse_as = Some((mapping, source));
        }
    }

    fn take_loaded(&mut self, r: Result<Loaded, ReadFail>) {
        match r {
            Ok(l) => {
                let keep_section = self
                    .loaded
                    .as_ref()
                    .is_some_and(|old| old.sha256 == l.sha256);
                self.csv_mapping.clone_from(&l.parsed.csv_mapping);
                // The mapping asked for was used (a remembered one that did
                // not fit was not): its source holds; else it is a guess.
                let asked = self.reparse_as.take();
                if keep_section {
                    match asked {
                        Some((m, source)) if l.parsed.csv_mapping.as_ref() == Some(&m) => {
                            self.csv_source = source;
                        }
                        // QA-6: a remembered layout that did not fit.
                        Some((_, CsvSource::Batch | CsvSource::Local)) => {
                            self.layout_changed = true;
                        }
                        _ => {}
                    }
                } else {
                    // QA-8: another file; an answer still on its way for
                    // the one before is dropped, never applied to this one.
                    self.lookup_task = None;
                    self.history_task = None;
                    self.csv_source = CsvSource::Guess;
                    self.section = l
                        .parsed
                        .sections
                        .iter()
                        .position(|s| !s.rows.is_empty())
                        .unwrap_or(0);
                    self.reset_link();
                }
                self.loaded = Some(l);
                self.read_error = None;
                self.forget_preview();
                if keep_section {
                    // A new CSV mapping: same file, same account; the
                    // history is not asked again, so an owner's choice is
                    // never put back (review DB2). New rows, new checks.
                    self.refresh_sign();
                } else {
                    self.start_lookup();
                }
            }
            Err(e) => {
                self.reparse_as = None;
                self.read_error = Some(e);
            }
        }
    }

    fn reset_link(&mut self) {
        self.layout_changed = false;
        self.account = None;
        self.linked = LinkedHow::None;
        self.remembered_flip = None;
        self.sign_confirmed = false;
        self.relink = false;
        self.refresh_sign();
    }

    /// Whether the chosen account is a credit card account.
    fn card_target(&self) -> bool {
        self.account
            .is_some_and(|a| self.targets.iter().any(|t| t.id == a && t.card))
    }

    /// Runs the sign checks for the chosen account (once per change, not
    /// per frame): a card account gets the card rule even for a CSV
    /// (review DB1). Until the owner answered or a batch told the sign,
    /// the suggestion sets it.
    fn refresh_sign(&mut self) {
        let card = self.card_target();
        self.sign = self
            .current_section()
            .map(|s| flow::sign_check(s, card))
            .unwrap_or_default();
        // QA-4: a remembered answer this file's evidence contradicts is
        // not applied; the evidence is the default until the owner answers.
        let flipped = match self.remembered_flip {
            Some(f) if !flow::sign_disagrees(&self.sign, f) => f,
            _ if self.sign_confirmed => self.flipped,
            _ => self.sign.suggest_flip,
        };
        if flipped != self.flipped {
            self.flipped = flipped;
            self.forget_preview();
        }
    }

    fn forget_preview(&mut self) {
        self.preview = None;
        self.preview_task = None;
        self.preview_trouble = None;
        self.choices.clear();
    }

    pub(crate) fn current_section(&self) -> Option<&cl_import::Section> {
        self.loaded.as_ref()?.parsed.sections.get(self.section)
    }

    fn identity(&self) -> Option<cl_api::BankIdentity> {
        flow::identity(self.current_section()?)
    }

    // ---- Step 2: the account ---------------------------------------------

    /// Picks section `i` of the file.
    pub(crate) fn pick_section(&mut self, i: usize) {
        if self.section == i {
            return;
        }
        self.section = i;
        self.reset_link();
        self.forget_preview();
        self.start_lookup();
    }

    /// Finds the account the file's account number was linked to: the
    /// engine matches it with the company's link key (Q-P25u), else its
    /// last 4 characters suggest one (the engine does both in one call).
    fn start_lookup(&mut self) {
        let Some(id) = self.identity() else {
            return;
        };
        let link = Arc::clone(&self.link);
        if let Some(demo) = link.demo_state() {
            let found = demo.bank_feed.lookup(&BankLookupBy::Account(id));
            drop(demo);
            self.lookup_task = Some(Task::ready(found));
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        self.lookup_task = group
            .read(&session, move |s| async move {
                s.bank_import().lookup(&BankLookupBy::Account(id)).await
            })
            .ok()
            .map(Task::Pending);
    }

    fn take_lookup(&mut self, found: &[ConnectionMatch]) {
        if self.account.is_some() {
            return;
        }
        let Some(m) = found
            .iter()
            .find(|m| self.targets.is_empty() || self.targets.iter().any(|t| t.id == m.account))
        else {
            return;
        };
        self.account = Some(m.account);
        self.linked = match m.via {
            BankLookupVia::Hash => LinkedHow::Auto,
            BankLookupVia::Mask => LinkedHow::Mask,
        };
        self.refresh_sign();
        self.start_history();
    }

    /// Picks the account the file goes into.
    pub(crate) fn pick_account(&mut self, id: AccountId) {
        if self.account == Some(id) {
            return;
        }
        self.account = Some(id);
        self.linked = LinkedHow::Owner;
        self.layout_changed = false;
        self.remembered_flip = None;
        self.sign_confirmed = false;
        self.relink = false;
        self.forget_preview();
        self.refresh_sign();
        self.start_history();
    }

    /// The account's latest import of this format: its sign and columns
    /// win over the guesses (Q-P25w).
    fn start_history(&mut self) {
        let (Some(account), Some(l)) = (self.account, self.loaded.as_ref()) else {
            return;
        };
        let format = flow::wire_format(l.parsed.format);
        let link = Arc::clone(&self.link);
        if let Some(demo) = link.demo_state() {
            let latest = demo.bank_feed.latest(account, format);
            drop(demo);
            match latest {
                Some((flip, mapping)) => self.remember(flip, mapping),
                None => self.apply_local_profile(),
            }
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        self.history_task = group
            .read(&session, move |s| async move {
                s.bank_import().batches(Some(account), Some(format)).await
            })
            .ok()
            .map(Task::Pending);
    }

    /// The account's latest import of this format told the sign and,
    /// for a CSV, the columns. The columns are tried only when the owner
    /// has not chosen them here, and used only when they fit (DB2).
    fn remember(&mut self, flip: bool, mapping: Option<Value>) {
        self.remembered_flip = Some(flip);
        // QA-4: asked again when this file's evidence says the other way.
        self.sign_confirmed = false;
        self.refresh_sign();
        self.sign_confirmed = !self.sign_differs();
        let stored: Option<CsvMapping> = mapping.and_then(|m| serde_json::from_value(m).ok());
        match stored {
            Some(m) => self.try_stored_mapping(m, CsvSource::Batch),
            None => self.apply_local_profile(),
        }
    }

    fn try_stored_mapping(&mut self, m: CsvMapping, source: CsvSource) {
        let is_csv = self
            .loaded
            .as_ref()
            .is_some_and(|l| l.parsed.csv_mapping.is_some());
        if !is_csv || self.csv_source == CsvSource::Owner {
            return;
        }
        if self.csv_mapping.as_ref() == Some(&m) {
            self.csv_source = source;
        } else {
            self.reparse(m, source);
        }
    }

    /// Sets the sign: `true` flips every amount of the file.
    pub(crate) fn set_flipped(&mut self, flipped: bool) {
        if self.flipped != flipped {
            self.flipped = flipped;
            self.forget_preview();
        }
        self.sign_confirmed = true;
    }

    /// Applies a changed CSV mapping (the file is read again).
    pub(crate) fn set_mapping(&mut self, m: CsvMapping) {
        if self.csv_mapping.as_ref() != Some(&m) {
            self.csv_mapping = Some(m.clone());
            self.csv_source = CsvSource::Owner;
            self.layout_changed = false;
            self.reparse(m, CsvSource::Owner);
        }
    }

    /// The remembered flip and this file's evidence disagree (QA-4).
    pub(crate) fn sign_differs(&self) -> bool {
        self.remembered_flip
            .is_some_and(|f| flow::sign_disagrees(&self.sign, f))
    }

    /// The file's account kind and the chosen account's differ (QA-5).
    pub(crate) fn type_mismatch(&self) -> Option<&'static str> {
        self.account?;
        flow::type_mismatch(self.current_section()?.kind, self.card_target())
    }

    pub(crate) fn sign_needs_confirm(&self) -> bool {
        self.current_section().is_some()
            && flow::sign_needs_confirm(&self.sign, self.card_target(), self.remembered_flip)
            && !self.sign_confirmed
    }

    // ---- Step 3: the dry run ----------------------------------------------

    fn request(&self) -> Option<BankImportRequest> {
        let l = self.loaded.as_ref()?;
        let section = l.parsed.sections.get(self.section)?;
        Some(flow::build_request(
            l,
            section,
            self.account?,
            self.flipped,
            self.csv_mapping.as_ref(),
            self.relink,
            &self.choices,
        ))
    }

    /// Runs the dry run for the current choices.
    pub(crate) fn run_preview(&mut self) {
        self.preview_trouble = None;
        let Some(req) = self.request() else {
            return;
        };
        let link = Arc::clone(&self.link);
        if let Some(mut demo) = link.demo_state() {
            let now = crate::company::demo::DemoState::now();
            let lock = crate::company::demo::home::closing_date(&demo);
            let o = demo.bank_feed.import(&req, false, now, lock);
            drop(demo);
            match o {
                Ok(o) => {
                    self.preview_task = Some(Task::ready(o));
                    self.poll();
                }
                Err(r) => self.preview_trouble = Some(EngineTrouble::Refused(r)),
            }
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            self.preview_trouble = Some(EngineTrouble::Other("bankimport.engine_down"));
            return;
        };
        self.preview_task = group
            .read(&session, move |s| async move {
                s.bank_import().preview(&req).await
            })
            .ok()
            .map(Task::Pending);
    }

    /// Sets a row's choice and runs the dry run again.
    pub(crate) fn choose(&mut self, line: u32, choice: RowChoice) {
        if choice == RowChoice::default() {
            self.choices.remove(&line);
        } else {
            self.choices.insert(line, choice);
        }
        self.run_preview();
    }

    // ---- Step 4: the commit -----------------------------------------------

    /// Sends the batch: the key is the one made for this exact body, or a
    /// new one when anything changed since.
    pub(crate) fn send(&mut self) {
        self.send_trouble = None;
        let Some(req) = self.request() else {
            return;
        };
        let key = self.key_for(&req);
        let link = Arc::clone(&self.link);
        if let Some(mut demo) = link.demo_state() {
            let now = crate::company::demo::DemoState::now();
            let lock = crate::company::demo::home::closing_date(&demo);
            let o = demo.bank_feed.import(&req, true, now, lock);
            drop(demo);
            match o {
                Ok(o) => {
                    self.send_task = Some(Task::ready(o));
                    self.poll();
                }
                Err(r) => self.send_trouble = Some(EngineTrouble::Refused(r)),
            }
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            self.send_trouble = Some(EngineTrouble::Other("bankimport.engine_down"));
            return;
        };
        let label = OrphanLabel {
            what: "orphan.bank_import",
            key: Some(key.clone()),
        };
        self.send_task = group
            .write(&session, label, move |s| async move {
                s.bank_import().commit(&req, key).await
            })
            .ok()
            .map(Task::Pending);
    }

    /// The key of `req`: the one made at its first Send, or a new one when
    /// the body changed since (plan §3 step 4).
    fn key_for(&mut self, req: &BankImportRequest) -> IdempotencyKey {
        match &self.frozen {
            Some(f) if f.request == *req => f.key.clone(),
            _ => {
                let key = IdempotencyKey::new();
                self.frozen = Some(Frozen {
                    request: req.clone(),
                    key: key.clone(),
                });
                key
            }
        }
    }

    /// The local CSV profile key (Q-P25f fallback): the company, the
    /// account and the header names, hashed (no owner text is stored).
    fn profile_key(&self) -> Option<String> {
        let l = self.loaded.as_ref()?;
        let headers = l.parsed.csv_headers.as_ref()?.join("\u{1f}");
        Some(format!(
            "{}/{}/{}",
            self.tenant.map(|t| t.to_string()).unwrap_or_default(),
            self.account.map(|a| a.to_string()).unwrap_or_default(),
            &flow::sha256_hex(headers.as_bytes())[..16]
        ))
    }

    fn profiles(&self) -> Option<cl_settings::CsvProfilesStore> {
        self.link
            .store()
            .path()
            .parent()
            .map(cl_settings::CsvProfilesStore::in_dir)
    }

    /// No batch of this account told the columns: a layout saved on this
    /// computer does (Q-P25f), when it fits. This is the one CSV profile
    /// mechanism: the key is the company, the account and the header.
    fn apply_local_profile(&mut self) {
        let (Some(key), Some(store)) = (self.profile_key(), self.profiles()) else {
            return;
        };
        let stored: Option<CsvMapping> = store
            .load()
            .get(&key)
            .cloned()
            .and_then(|m| serde_json::from_value(m).ok());
        if let Some(m) = stored {
            self.try_stored_mapping(m, CsvSource::Local);
        }
    }

    fn forget_local_layouts(&self) {
        if let Some(store) = self.profiles()
            && store.save(&cl_settings::CsvProfiles::default()).is_err()
        {
            tracing::warn!("the bank CSV layouts on this computer could not be cleared");
        }
    }

    fn save_local_profile(&self) {
        let (Some(key), Some(store), Some(m)) = (
            self.profile_key(),
            self.profiles(),
            self.csv_mapping.as_ref(),
        ) else {
            return;
        };
        let Ok(v) = serde_json::to_value(m) else {
            return;
        };
        let mut p = store.load();
        p.put(&key, v);
        if store.save(&p).is_err() {
            tracing::warn!("the bank CSV columns could not be saved on this computer");
        }
    }

    fn after_send(&mut self, o: BankImportOutcome) {
        self.save_local_profile();
        self.sent += 1;
        self.frozen = None;
        self.step = Step::Done;
        let now = std::time::Instant::now();
        self.prep_started = Some(now);
        self.prep_delay = std::time::Duration::from_secs(1);
        self.prep_next = Some(now + self.prep_delay);
        self.prep_gave_up = false;
        self.prep_progress = None;
        if o.counts.inserted > 0 || o.rows.iter().any(|r| r.replaced) {
            self.link.bus().publish(crate::data_bus::Change::BankFeed);
            self.recount_feed();
        }
        self.result = Some(o);
    }

    /// Whether the import just sent is still being prepared.
    fn preparing(&self) -> bool {
        self.result
            .as_ref()
            .is_some_and(|r| r.post_processing == Some(ImportPostProcessing::Scheduled))
    }

    /// Checks the preparation of the import just sent (Q-P25v): off the
    /// egui thread, after 1, 2, 4, 8 then every 10 seconds, until it is
    /// done or failed or the 120 s Import budget is spent.
    fn poll_preparing(&mut self) {
        let now = std::time::Instant::now();
        let (Some(started), Some(next)) = (self.prep_started, self.prep_next) else {
            return;
        };
        if !self.preparing() || self.prep_task.is_some() || now < next {
            return;
        }
        // Q-P25ac: past the budget the owner is told it goes on, and it is
        // still checked (slowly) so Undo opens once it is done.
        if now.duration_since(started) > PREPARING_BUDGET {
            self.prep_gave_up = true;
        }
        self.prep_delay = if self.prep_gave_up {
            PREPARING_SLOW_DELAY
        } else {
            (self.prep_delay * 2).min(PREPARING_MAX_DELAY)
        };
        self.prep_next = Some(now + self.prep_delay);
        let Some(batch) = self.result.as_ref().and_then(|r| r.batch) else {
            return;
        };
        let link = Arc::clone(&self.link);
        if let Some(demo) = link.demo_state() {
            let p = demo.bank_feed.post_processing(batch);
            let progress = demo.bank_feed.progress(batch);
            drop(demo);
            if let (Some(p), Some(res)) = (p, self.result.as_mut()) {
                res.post_processing = Some(p);
                self.prep_progress = progress;
            }
            return;
        }
        let (Some(session), Some(group), Some(account)) =
            (self.link.session(), self.group.as_ref(), self.account)
        else {
            return;
        };
        self.prep_task = group
            .read(&session, move |s| async move {
                s.bank_import().batches(Some(account), None).await
            })
            .ok()
            .map(Task::Pending);
    }

    /// Undoes the import just sent; with `pending_only`, rows already used
    /// stay.
    pub(crate) fn undo(&mut self, pending_only: bool) {
        self.undo_trouble = None;
        let Some(batch) = self.result.as_ref().and_then(|r| r.batch) else {
            return;
        };
        let link = Arc::clone(&self.link);
        if let Some(mut demo) = link.demo_state() {
            self.undo_cancels = self.preparing();
            let u = demo.bank_feed.undo(batch);
            drop(demo);
            match u {
                Ok(u) => self.take_undo(u),
                Err(r) => self.undo_trouble = Some(EngineTrouble::Refused(r)),
            }
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        self.undo_cancels = self.preparing();
        let key = self
            .undo_key
            .get_or_insert_with(IdempotencyKey::new)
            .clone();
        let label = OrphanLabel {
            what: "orphan.bank_import_undo",
            key: Some(key.clone()),
        };
        self.undo_task = group
            .write(&session, label, move |s| async move {
                s.bank_import().undo(batch, pending_only, key).await
            })
            .ok()
            .map(Task::Pending);
    }

    fn take_undo(&mut self, u: ImportUndoOutcome) {
        self.undo_key = None;
        self.undone = Some(u);
        // PR #234: an undo while the import was prepared stopped it; the
        // engine reports `cancelled` from now on, so the checks end here.
        if std::mem::take(&mut self.undo_cancels)
            && let Some(res) = self.result.as_mut()
            && res.post_processing == Some(ImportPostProcessing::Scheduled)
        {
            res.post_processing = Some(ImportPostProcessing::Cancelled);
        }
        self.link.bus().publish(crate::data_bus::Change::BankFeed);
        self.recount_feed();
    }

    /// Reads the Bank Feed's waiting count again for the rail's badge
    /// (UI review 11): Home, which reads it otherwise, is not in front.
    fn recount_feed(&mut self) {
        let link = Arc::clone(&self.link);
        if let Some(mut demo) = link.demo_state() {
            let n = crate::company::demo::home::feed_waiting(&mut demo);
            drop(demo);
            self.feed_count_task = Some(Task::ready(n));
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        let today = self.today;
        self.feed_count_task = group
            .read(&session, move |s| async move {
                s.home().feed_pending_count(today).await
            })
            .ok()
            .map(Task::Pending);
    }

    /// Back to step 1 for another file (or the next section of this one).
    pub(crate) fn start_over(&mut self, next_section: bool) {
        self.step = if next_section {
            Step::Link
        } else {
            Step::Choose
        };
        self.result = None;
        self.undone = None;
        self.undo_trouble = None;
        self.undo_cancels = false;
        self.send_trouble = None;
        self.frozen = None;
        self.prep_started = None;
        self.prep_next = None;
        self.prep_task = None;
        self.prep_gave_up = false;
        self.prep_progress = None;
        self.forget_preview();
        if next_section {
            let next = self.section + 1;
            self.section = usize::MAX;
            self.pick_section(next);
        } else {
            self.loaded = None;
            self.read_error = None;
            self.reset_link();
        }
    }

    // ---- Steps -------------------------------------------------------------

    /// Whether Next is open on the current step.
    #[must_use]
    pub(crate) fn can_next(&self) -> bool {
        match self.step {
            Step::Choose => self.loaded.is_some() && self.reading.is_none(),
            Step::Link => {
                self.account.is_some()
                    && !self.sign_needs_confirm()
                    && self.reading.is_none()
                    && self.current_section().is_some_and(|s| !s.rows.is_empty())
            }
            // UI review 8: Send opens only when it would add or replace
            // something.
            Step::Check => {
                self.unanswered_changed() == 0
                    && self.preview.as_ref().is_some_and(|p| {
                        flow::rows_to_send(p, &self.choices) > 0 || self.replacing() > 0
                    })
                    && self.preview_task.is_none()
                    && self.send_task.is_none()
                    && self
                        .preview
                        .as_ref()
                        .is_some_and(|p| p.identity_linked_elsewhere.is_none() || self.relink)
            }
            Step::Done => false,
        }
    }

    /// Changed rows the owner has not answered yet (Q-P25af): Send stays
    /// closed until each is replaced, added or skipped.
    pub(crate) fn unanswered_changed(&self) -> usize {
        self.preview.as_ref().map_or(0, |p| {
            p.rows
                .iter()
                .filter(|r| {
                    r.status == ImportRowStatus::Changed
                        && !changed_answered(self.choices.get(&r.line).copied().unwrap_or_default())
                })
                .count()
        })
    }

    /// Changed rows set to replace their waiting item (the dry run says
    /// the item is still waiting).
    pub(crate) fn replacing(&self) -> usize {
        let Some(p) = self.preview.as_ref() else {
            return 0;
        };
        p.rows
            .iter()
            .filter(|r| {
                r.status == ImportRowStatus::Changed
                    && r.existing_pending == Some(true)
                    && self.choices.get(&r.line).and_then(|c| c.changed)
                        == Some(ChangedAction::Replace)
            })
            .count()
    }

    /// Next step (Alt+N).
    pub(crate) fn next(&mut self) {
        if !self.can_next() {
            return;
        }
        match self.step {
            Step::Choose => self.step = Step::Link,
            Step::Link => {
                self.step = Step::Check;
                self.run_preview();
            }
            Step::Check => self.send(),
            Step::Done => {}
        }
    }

    /// Previous step (Alt+B).
    pub(crate) fn back(&mut self) {
        self.step = match self.step {
            Step::Choose | Step::Link => Step::Choose,
            Step::Check => Step::Link,
            Step::Done => return,
        };
    }

    /// Like [`Self::trouble`], for Undo: a view-only role gets its own
    /// text (Q-P25ab).
    fn undo_trouble_of(e: &ApiError) -> EngineTrouble {
        match e {
            ApiError::Forbidden { .. } => EngineTrouble::Other("bankimport.undo.forbidden"),
            _ => Self::trouble(e),
        }
    }

    fn trouble(e: &ApiError) -> EngineTrouble {
        if let Some(r) = bank_import_refusal(e) {
            return EngineTrouble::Refused(r);
        }
        match e {
            ApiError::OutcomeUnknown { .. } | ApiError::Timeout => EngineTrouble::Unknown,
            ApiError::Local(cl_api::LocalValidation::BatchFileSize) => {
                EngineTrouble::Other("bankimport.too_big")
            }
            ApiError::Local(cl_api::LocalValidation::Unsupported) => {
                EngineTrouble::Other("bankimport.unsupported")
            }
            ApiError::Forbidden { .. } => EngineTrouble::Other("bankimport.forbidden"),
            _ => EngineTrouble::Other("bankimport.engine_failed"),
        }
    }

    /// Takes every finished job.
    pub(crate) fn poll(&mut self) {
        if let Some(paths) = crate::widgets::file_picker::poll(&mut self.pick_rx)
            && let Some(p) = paths.into_iter().next()
        {
            self.read(p, None);
        }
        if let Some(rx) = self.reading.as_ref() {
            match rx.try_recv() {
                Ok(r) => {
                    self.reading = None;
                    self.take_loaded(r);
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.reading = None;
                    self.read_error = Some(ReadFail::Open);
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
        }
        if let Some(r) = self.targets_task.as_mut().and_then(Task::try_take) {
            self.targets_task = None;
            match r {
                Ok(a) => self.take_targets(a),
                Err(_) => self.targets_failed = true,
            }
        }
        if self.targets_task.is_none()
            && let Some(r) = self.lookup_task.as_mut().and_then(Task::try_take)
        {
            self.lookup_task = None;
            if let Ok(found) = r {
                self.take_lookup(&found);
            }
        }
        if let Some(r) = self.history_task.as_mut().and_then(Task::try_take) {
            self.history_task = None;
            let format = self
                .loaded
                .as_ref()
                .map(|l| flow::wire_format(l.parsed.format));
            let latest = r.ok().and_then(|batches| {
                batches
                    .into_iter()
                    .find(|b| b.undone_at.is_none() && b.format == format && b.inserted > 0)
            });
            match latest {
                Some(b) => self.remember(b.amounts_flipped, b.csv_mapping),
                None => self.apply_local_profile(),
            }
        }
        if let Some(r) = self.preview_task.as_mut().and_then(Task::try_take) {
            self.preview_task = None;
            match r {
                Ok(o) => self.preview = Some(o),
                Err(e) => self.preview_trouble = Some(Self::trouble(&e)),
            }
        }
        if let Some(r) = self.send_task.as_mut().and_then(Task::try_take) {
            self.send_task = None;
            match r {
                Ok(o) => self.after_send(o),
                Err(e) => self.send_trouble = Some(Self::trouble(&e)),
            }
        }
        if let Some(r) = self.prep_task.as_mut().and_then(Task::try_take) {
            self.prep_task = None;
            // A check read before an undo landed never brings back
            // "preparing".
            if let (Ok(batches), Some(res)) = (r, self.result.as_mut())
                && let Some(b) = batches.iter().find(|b| Some(b.id) == res.batch)
                && res.post_processing != Some(ImportPostProcessing::Cancelled)
            {
                res.post_processing = b.post_processing;
                self.prep_progress = b.progress;
            }
        }
        self.poll_preparing();
        self.poll_earlier();
        if let Some(r) = self.feed_count_task.as_mut().and_then(Task::try_take) {
            self.feed_count_task = None;
            if let Ok(n) = r {
                self.link.push(crate::company::CompanyEvent::BankFeedCount(
                    u32::try_from(n).unwrap_or(u32::MAX),
                ));
            }
        }
        if let Some(r) = self.undo_task.as_mut().and_then(Task::try_take) {
            self.undo_task = None;
            match r {
                Ok(u) => self.take_undo(u),
                Err(e) => self.undo_trouble = Some(Self::undo_trouble_of(&e)),
            }
        }
    }

    fn busy(&self) -> bool {
        self.pick_rx.is_some()
            || self.reading.is_some()
            || self.targets_task.is_some()
            || self.lookup_task.is_some()
            || self.history_task.is_some()
            || self.preview_task.is_some()
            || self.send_task.is_some()
            || self.prep_task.is_some()
            || self.undo_task.is_some()
            || self.feed_count_task.is_some()
            || self.earlier_task.is_some()
            || (self.preparing() && !self.prep_gave_up)
    }
}

impl Screen for ImportBankFileScreen {
    fn title(&self) -> String {
        t(&format!("cmd.{KIND}"))
    }

    fn fixed_layout(&self) -> bool {
        true
    }

    /// A chosen file not yet sent: closing asks first (Q-P25ae).
    fn dirty(&self) -> bool {
        self.loaded.is_some() && self.step != Step::Done
    }

    /// Nothing here is saved; the prompt asks only whether to close (P26
    /// QA-12).
    fn can_save(&self) -> bool {
        false
    }

    fn ui(&mut self, ui: &mut eframe::egui::Ui) {
        self.poll();
        if self.busy() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        } else if self.preparing() || self.earlier_next.is_some() {
            // Q-P25ac: still checked after the wait gave up.
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_secs(1));
        }
        self.draw(ui);
    }

    /// Form keys, so Alt+N (next step) and Alt+B (back) reach the wizard.
    fn scope(&self) -> cl_model::Scope {
        cl_model::Scope::Form
    }

    fn command(&mut self, id: CommandId) -> bool {
        match id.as_str() {
            "form.save_new" => {
                self.next();
                true
            }
            "form.step_back" => {
                self.back();
                true
            }
            // The scenario repeats in dark in one app: an empty demo feed.
            // Q-P25aa: after a restore the company's feed and links are
            // the same, nothing of this computer's is; the wizard starts
            // over as in a freshly opened window.
            "dev.demo_bank_import_restore" => {
                if !self.link.is_demo() {
                    return false;
                }
                self.forget_local_layouts();
                self.start_over(false);
                true
            }
            // PR #234: the demo prepares the next imports slowly, so Undo
            // and Close can be tried while it runs.
            "dev.demo_bank_import_slow" => {
                let link = Arc::clone(&self.link);
                let Some(mut demo) = link.demo_state() else {
                    return false;
                };
                demo.bank_feed.set_slow(true);
                true
            }
            // Q-P25aa: a second computer has no CSV layouts of its own.
            "dev.bank_import_forget_local" => {
                self.forget_local_layouts();
                true
            }
            "dev.demo_bank_import_reset" => {
                let link = Arc::clone(&self.link);
                let Some(mut demo) = link.demo_state() else {
                    return false;
                };
                demo.bank_feed.reset();
                drop(demo);
                self.earlier = None;
                self.earlier_task = None;
                self.earlier_next = None;

                self.link.bus().publish(crate::data_bus::Change::BankFeed);
                self.recount_feed();
                self.forget_local_layouts();
                self.start_over(false);
                self.sent = 0;
                true
            }
            // Q-P25ae: Esc goes back one step; on step 1 (and after the
            // send) the window closes, asking first while a file is chosen
            // and not sent (`dirty`).
            "form.revert" => match self.step {
                Step::Link | Step::Check => {
                    self.back();
                    true
                }
                Step::Choose | Step::Done => false,
            },
            _ => false,
        }
    }

    fn state(&self) -> Value {
        // Steps, counts, lines and flags only: never amounts, names,
        // masks, descriptions or the file name.
        let section = self.current_section();
        let preview = self
            .preview
            .as_ref()
            .map(|p| outcome_state(p, &self.choices));
        let earlier = self.earlier.as_ref().map(|b| {
            json!({
                "post_processing": b.post_processing.map(post_processing_name),
                "progress": b.progress.map(|(d, a)| json!([d, a])),
            })
        });
        let result = self
            .result
            .as_ref()
            .map(|p| outcome_state(p, &self.choices));
        let mut state = json!({
            "screen": "import_bank_file",

            "step": self.step.as_str(),
            "can_next": self.can_next(),
            "reading": self.reading.is_some(),
            "file": self.loaded.as_ref().map(|l| json!({
                "format": flow::wire_format(l.parsed.format).as_wire(),
                "sections": l.parsed.sections.len(),
                "has_identity": l.parsed.sections.get(self.section).is_some_and(|s| s.account.is_some()),
            })),
            "read_error": self.read_error.as_ref().map(|e| {
                let (key, line) = e.key();
                json!({"key": key, "line": line})
            }),
            "section": self.loaded.as_ref().map(|_| self.section),
            "rows": section.map(|s| s.rows.len()),
            "problems": section.map(|s| s.problems.iter().map(|p| p.line).collect::<Vec<_>>()),
            "skipped": section.map(|s| s.skipped.len()),
            "csv": self.csv_mapping.is_some(),
            "csv_source": self.csv_mapping.as_ref().map(|_| self.csv_source.as_str()),
            "csv_date_order": self.csv_mapping.as_ref().map(|m| match m.date_order {
                cl_import::DateOrder::Mdy => "mdy",
                cl_import::DateOrder::Dmy => "dmy",
                cl_import::DateOrder::Ymd => "ymd",
            }),
            "card_account": self.card_target(),
            "accounts": self.targets.len(),
            "account_chosen": self.account.is_some(),
            "linked": match self.linked {
                LinkedHow::None => "none",
                LinkedHow::Auto => "auto",
                LinkedHow::Mask => "mask",
                LinkedHow::Owner => "owner",
            },
            "flipped": self.flipped,
            "sign_needs_confirm": self.sign_needs_confirm(),
            "remembered_flip": self.remembered_flip.is_some(),
            "sign_differs": self.sign_differs(),
            "type_mismatch": self.type_mismatch().is_some(),
            "layout_changed": self.layout_changed,
            "previewing": self.preview_task.is_some(),
            "preview": preview,
            "preview_trouble": trouble_state(self.preview_trouble.as_ref()),
            "only_skipped": self.only_skipped,
            "to_send": self.preview.as_ref().map(|p| flow::rows_to_send(p, &self.choices)),
            "changed_unanswered": self.unanswered_changed(),
            "sending": self.send_task.is_some(),
            "send_trouble": trouble_state(self.send_trouble.as_ref()),
            "has_key": self.frozen.is_some(),
            "result": result,
            "preparing": self.preparing(),
            "prep_gave_up": self.prep_gave_up,
            "prep_progress": self.prep_progress.map(|(d, a)| json!([d, a])),
            "undone": self.undone.map(|u| json!({"removed": u.removed, "kept": u.kept, "restored": u.restored})),
            "undo_trouble": trouble_state(self.undo_trouble.as_ref()),
            "sent": self.sent,
            "layout": self.layout.json(),
        });
        // Added outside the macro (its recursion limit).
        if let Some(m) = state.as_object_mut() {
            m.insert("progress_caps".into(), json!(self.progress_caps()));
            m.insert("earlier".into(), earlier.unwrap_or(Value::Null));
        }
        state
    }
}

fn outcome_state(o: &BankImportOutcome, choices: &BTreeMap<u32, RowChoice>) -> Value {
    let lines = |st: ImportRowStatus| {
        o.rows
            .iter()
            .filter(|r| r.status == st)
            .map(|r| r.line)
            .collect::<Vec<_>>()
    };
    json!({
        "in_file": o.counts.in_file,
        "new": o.counts.new,
        "duplicate": o.counts.duplicate,
        "possible_duplicate": o.counts.possible_duplicate,
        "changed": o.counts.changed,
        "closed_period": o.counts.closed_period,
        "inserted": o.counts.inserted,
        "possible_lines": lines(ImportRowStatus::PossibleDuplicate),
        "changed_lines": lines(ImportRowStatus::Changed),
        "chosen": choices.len(),
        "balance_check": o.balance_check.as_ref().map(|b| b.difference.is_zero()),
        "linked_elsewhere": o.identity_linked_elsewhere.is_some(),
        "same_file_before": o.same_file_imported_at.is_some(),
        "batch": o.batch.is_some(),
        "post_processing": o.post_processing.map(post_processing_name),
        "replayed": o.replayed,
    })
}

fn post_processing_name(p: ImportPostProcessing) -> &'static str {
    match p {
        ImportPostProcessing::Scheduled => "scheduled",
        ImportPostProcessing::Done => "done",
        ImportPostProcessing::Failed => "failed",
        ImportPostProcessing::Cancelled => "cancelled",
    }
}

fn trouble_state(t: Option<&EngineTrouble>) -> Value {
    match t {
        None => Value::Null,
        Some(EngineTrouble::Unknown) => json!("unknown"),
        Some(EngineTrouble::Other(k)) => json!(k),
        Some(EngineTrouble::Refused(r)) => json!(flow::refusal_key(r)),
    }
}

/// `ChangedAction` as the owner's two buttons.
pub(crate) fn changed_choice(action: ChangedAction) -> RowChoice {
    RowChoice {
        include: None,
        changed: Some(action),
    }
}

/// Q-P25af: a changed row left out (neither replace nor add).
pub(crate) fn changed_skip() -> RowChoice {
    RowChoice {
        include: Some(false),
        changed: None,
    }
}

/// Whether the owner has answered a changed row (Q-P25af: no default).
pub(crate) fn changed_answered(c: RowChoice) -> bool {
    c.changed.is_some() || c.include == Some(false)
}
