//! Transfer Funds (P14 lane A, `banking.transfer`, Ctrl+T), on the Write
//! Check pattern (Q-261): the shared toolbar and title row, From (with the
//! engine's balance) → To (with its balance) · Date · Amount · Memo, Clear
//! / Save & New / Save & Close with their shortcut chips, a side panel
//! with both balances and the From account's recent transfers, Prev/Next
//! over those transfers, Void and the shared closing-date prompt.
//!
//! Money: the amount is the owner's typed `Money` (cents, Q-232);
//! `cl_model::forms::transfer::check` is the gate and
//! `BankPosting::transfer` refuses again before anything is sent. The
//! balances are the engine's; a card or other liability shows "Balance
//! owed", the stored balance with its sign turned for display (Q-241).
//! Every save and void publishes both accounts on the data bus (Q-262).

mod engine;
mod template;

use std::sync::Arc;

use cl_api::{
    Account, AccountFilter, ApiError, BankPosting, Capabilities, ClosedPeriodAuthorization,
    ConflictReason, IdempotencyKey, LocalValidation, OrphanLabel, PageRequest, PostingOptions,
    RecordVersion, RequestGroup, Transaction, TransactionFilter, TransferDraft, TxnStatus, TxnType,
    Versioned,
};
use cl_core::ids::{AccountId, TenantId, TransactionId};
use cl_core::{CalendarDate, Money, Secret};
use cl_model::amount_entry::{self, AmountRules};
use cl_model::closing::{is_locked, needs_password};
use cl_model::forms::transfer::{
    TransferCandidate, TransferForm, TransferProblem, check, transfer_accounts,
};
use cl_model::forms::{FormState, SaveIntent, SaveStart};
use cl_model::register::AccountClass;
use cl_model::{CommandId, Scope};
use eframe::egui::{self, RichText, Ui};
use serde_json::{Value, json};

use crate::company::{CompanyEvent, CompanyLink, Task};
use crate::forms::closing_prompt::{ClosingClick, ClosingPrompt};
use crate::i18n::t;
use crate::screens::card_charge::kit;
use crate::screens::company::form::{self, DateInput};
use crate::screens::register::account_text;
use crate::theme::Tokens;
use crate::windows::Screen;

pub(crate) use engine::{DemoTransfers, LocalTransfers, TransferRow};

/// The command and window kind.
pub const KIND: &str = "banking.transfer";

/// How many recent transfers the side panel lists.
const RECENT_SHOWN: usize = 3;

/// The Transfer Funds window for `kind`, if it is one.
#[must_use]
pub fn make(
    kind: CommandId,
    link: &Arc<CompanyLink>,
    open: Option<(TenantId, String)>,
    today: CalendarDate,
) -> Option<Box<dyn Screen>> {
    (kind.as_str() == KIND)
        .then(|| Box::new(TransferScreen::new(Arc::clone(link), open, today)) as Box<_>)
}

mod field {
    pub const FROM: u64 = 1;
    pub const TO: u64 = 2;
    pub const DATE: u64 = 3;
    pub const AMOUNT: u64 = 4;
    pub const MEMO: u64 = 5;
}

/// What the closing-date password is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Pending {
    Save(SaveIntent),
    Void(String),
}

/// A saved transfer being changed.
#[derive(Clone, Debug)]
pub(crate) struct Editing {
    pub id: TransactionId,
    pub version: RecordVersion,
    pub old_date: CalendarDate,
    /// Void, or not the shape this form writes.
    pub read_only: bool,
    /// The accounts the saved transfer touches: an edit or a void also
    /// tells them (Q-262), so an account the edit moved off reloads.
    pub accounts: Vec<AccountId>,
}

/// What a save sends, for its idempotency key (never the password).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SaveIdentity {
    pub posting: BankPosting,
    pub edit: Option<(TransactionId, RecordVersion)>,
    pub password: bool,
}

/// The Transfer Funds window.
pub struct TransferScreen {
    pub(crate) link: Arc<CompanyLink>,
    tenant: Option<(TenantId, String)>,
    group: Option<RequestGroup>,
    today: CalendarDate,
    pub(crate) local: Option<Box<dyn LocalTransfers>>,
    pub(crate) accounts: Vec<Account>,
    accounts_task: Option<Task<(Vec<Account>, Option<CalendarDate>)>>,
    pub(crate) loaded: bool,
    pub(crate) closing: Option<CalendarDate>,
    pub(crate) form: FormState<TransferForm>,
    date: DateInput,
    pub(crate) amount_text: String,
    amount_problem: Option<&'static str>,
    rev: u64,
    pub(crate) balances: [Option<Money>; 2],
    balance_tasks: [Option<Task<Option<Money>>>; 2],
    pub(crate) editing: Option<Editing>,
    load_task: Option<Task<Versioned<Transaction>>>,
    save_task: Option<Task<(TransactionId, RecordVersion)>>,
    intent: Option<SaveIntent>,
    pub(crate) key: Option<(SaveIdentity, IdempotencyKey)>,
    pub(crate) problems: Vec<TransferProblem>,
    pub(crate) error: Option<ApiError>,
    pub(crate) notice: Option<&'static str>,
    pub(crate) stale: bool,
    pub(crate) saved_count: u32,
    pub(crate) last_action: Option<&'static str>,
    pub(crate) last_saved: Option<TransactionId>,
    pub(crate) published: Vec<AccountId>,
    nav: Vec<TransferRow>,
    nav_task: Option<Task<Vec<TransferRow>>>,
    /// The From account's newest transfers (the side panel).
    pub(crate) recent: Vec<TransferRow>,
    recent_task: Option<Task<Vec<TransferRow>>>,
    /// The account `recent` was read for (`None`: read again).
    recent_for: Option<AccountId>,
    nav_at: Option<usize>,
    want_older: Option<bool>,
    void_open: bool,
    void_reason: String,
    void_task: Option<Task<()>>,
    pub(crate) closing_prompt: ClosingPrompt<Pending>,
    money_style: cl_core::MoneyStyle,
    bus: crate::data_bus::Subscription,
    /// Another window changed the accounts list while this form was dirty.
    pub(crate) lists_stale: bool,
    /// The field with the keyboard (Esc reverts it).
    pub(crate) focused: Option<u64>,
    /// Prev (true) or Next asked on unsaved changes: Save / Discard /
    /// Keep editing.
    pub(crate) nav_prompt: Option<bool>,
    /// Prev/Next to take once the prompt's save is done.
    after_save_nav: Option<bool>,
    /// Save as Template and the template this form came from (P15).
    pub(crate) template: crate::screens::templates::SaveFlow,
}

fn options(password: Option<Secret>) -> PostingOptions {
    PostingOptions {
        closed_period: password.map(ClosedPeriodAuthorization::with_password),
    }
}

/// A blank transfer.
fn blank(from: Option<AccountId>, date: CalendarDate) -> TransferForm {
    TransferForm {
        from,
        date: Some(date),
        ..TransferForm::default()
    }
}

/// The form for a saved transfer: one credit (From) and one debit (To) of
/// the same amount; anything else is shown read-only.
fn form_of(txn: &Transaction) -> (TransferForm, bool) {
    let credit: Vec<_> = txn
        .lines
        .iter()
        .filter(|l| l.credit.is_positive())
        .collect();
    let debit: Vec<_> = txn.lines.iter().filter(|l| l.debit.is_positive()).collect();
    let exact = matches!((credit.as_slice(), debit.as_slice()), ([c], [d]) if c.credit == d.debit);
    let form = TransferForm {
        from: credit.first().map(|l| l.account_id),
        to: debit.first().map(|l| l.account_id),
        date: Some(txn.txn_date),
        amount: debit.first().map(|l| l.debit),
        memo: txn.memo.clone().unwrap_or_default(),
    };
    (form, exact)
}

impl TransferScreen {
    /// A new transfer in the open company.
    #[must_use]
    pub fn new(
        link: Arc<CompanyLink>,
        tenant: Option<(TenantId, String)>,
        today: CalendarDate,
    ) -> TransferScreen {
        let local: Option<Box<dyn LocalTransfers>> = link
            .is_demo()
            .then(|| Box::new(DemoTransfers) as Box<dyn LocalTransfers>);
        TransferScreen::with_local(link, tenant, today, local)
    }

    pub(crate) fn with_local(
        link: Arc<CompanyLink>,
        tenant: Option<(TenantId, String)>,
        today: CalendarDate,
        local: Option<Box<dyn LocalTransfers>>,
    ) -> TransferScreen {
        let group = link.group().and_then(Result::ok);
        let mut s = TransferScreen {
            bus: link.bus().subscribe(),
            link,
            tenant,
            group,
            today,
            local,
            accounts: Vec::new(),
            accounts_task: None,
            loaded: false,
            closing: None,
            form: FormState::new(blank(None, today)),
            date: DateInput::new(today),
            amount_text: String::new(),
            amount_problem: None,
            rev: u64::MAX,
            balances: [None, None],
            balance_tasks: [None, None],
            editing: None,
            load_task: None,
            save_task: None,
            intent: None,
            key: None,
            problems: Vec::new(),
            error: None,
            notice: None,
            stale: false,
            saved_count: 0,
            last_action: None,
            last_saved: None,
            published: Vec::new(),
            nav: Vec::new(),
            nav_task: None,
            recent: Vec::new(),
            recent_task: None,
            recent_for: None,
            nav_at: None,
            want_older: None,
            void_open: false,
            void_reason: String::new(),
            void_task: None,
            closing_prompt: ClosingPrompt::default(),
            money_style: cl_core::MoneyStyle::default(),
            lists_stale: false,
            focused: None,
            nav_prompt: None,
            after_save_nav: None,
            template: crate::screens::templates::SaveFlow::default(),
        };
        s.start_load();
        s.poll();
        s
    }

    fn closing_supported(&self) -> bool {
        self.local.is_some() || self.link.capabilities().has(Capabilities::CLOSING_PASSWORD)
    }

    fn start_load(&mut self) {
        if let Some(mut demo) = self.link.demo_state() {
            let chart = demo
                .chart
                .get_or_insert_with(crate::company::demo::chart)
                .clone();
            let closing = demo.open.and_then(|open| {
                demo.companies
                    .iter()
                    .find(|c| c.tenant_id == open)
                    .and_then(|c| c.closed_through)
            });
            drop(demo);
            self.accounts_task = Some(Task::ready((chart, closing)));
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            self.loaded = true;
            return;
        };
        match group.read(&session, |s| async move {
            let a = s.accounts().list_all(&AccountFilter::default()).await?;
            let closing = s.company_settings().await.ok().and_then(|c| c.lock_date);
            Ok((a, closing))
        }) {
            Ok(p) => self.accounts_task = Some(Task::Pending(p)),
            Err(_) => self.loaded = true,
        }
    }

    /// The accounts money can move between (Q-P14h).
    pub(crate) fn transfer_accounts(&self) -> Vec<&Account> {
        let chart: Vec<TransferCandidate> = self
            .accounts
            .iter()
            .filter_map(|a| {
                Some(TransferCandidate {
                    id: a.id,
                    class: crate::screens::register::class_of(a)?,
                    system: a.is_system,
                    active: a.is_active,
                })
            })
            .collect();
        let ids = transfer_accounts(&chart);
        self.accounts
            .iter()
            .filter(|a| ids.contains(&a.id))
            .collect()
    }

    fn class(&self, id: AccountId) -> Option<AccountClass> {
        self.accounts
            .iter()
            .find(|a| a.id == id)
            .and_then(crate::screens::register::class_of)
    }

    /// Reads the balances of From and To.
    fn fetch_balances(&mut self) {
        let f = self.form.current();
        let ids = [f.from, f.to];
        for (i, id) in ids.into_iter().enumerate() {
            let Some(account) = id else {
                self.balances[i] = None;
                continue;
            };
            let link = Arc::clone(&self.link);
            if let Some(local) = self.local.as_mut() {
                self.balances[i] = local.balance(&link, account);
                continue;
            }
            let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
                continue;
            };
            if let Ok(p) = group.read(&session, move |s| async move {
                Ok(s.accounts().get(account).await?.record.balance)
            }) {
                self.balance_tasks[i] = Some(Task::Pending(p));
            }
        }
    }

    pub(crate) fn poll(&mut self) {
        if let Some(r) = self.accounts_task.as_mut().and_then(Task::try_take) {
            self.accounts_task = None;
            self.loaded = true;
            if let Ok((accounts, closing)) = r {
                self.accounts = accounts;
                self.closing = closing;
                if self.form.current().from.is_none() {
                    let first = self
                        .transfer_accounts()
                        .into_iter()
                        .find(|a| crate::screens::register::class_of(a) == Some(AccountClass::Bank))
                        .map(|a| a.id);
                    let mut f = self.form.current().clone();
                    f.from = first;
                    self.form.load(f);
                }
                self.fetch_balances();
            }
        }
        for i in 0..2 {
            if let Some(r) = self.balance_tasks[i].as_mut().and_then(Task::try_take) {
                self.balance_tasks[i] = None;
                self.balances[i] = r.ok().flatten();
            }
        }
        if let Some(r) = self.save_task.as_mut().and_then(Task::try_take) {
            self.save_task = None;
            match r {
                Ok((id, v)) => self.saved(id, v),
                Err(e) => self.save_failed(e),
            }
        }
        if let Some(r) = self.load_task.as_mut().and_then(Task::try_take) {
            self.load_task = None;
            match r {
                Ok(v) => self.show(&v),
                Err(e) => self.error = Some(e),
            }
        }
        if self.loaded && self.recent_task.is_none() && self.recent_for != self.form.current().from
        {
            self.fetch_recent();
        }
        if let Some(r) = self.recent_task.as_mut().and_then(Task::try_take) {
            self.recent_task = None;
            self.recent = r.unwrap_or_default();
            self.recent.retain(|r| !r.void);
            self.recent.truncate(RECENT_SHOWN);
        }
        if let Some(r) = self.nav_task.as_mut().and_then(Task::try_take) {
            self.nav_task = None;
            self.nav = r.unwrap_or_default();
            if let Some(older) = self.want_older.take() {
                self.step(older);
            }
        }
        if let Some(r) = self.void_task.as_mut().and_then(Task::try_take) {
            self.void_task = None;
            match r {
                Ok(()) => self.voided(),
                Err(e) => {
                    let reason = self.void_reason.clone();
                    if !self.closing_failed(&e, Pending::Void(reason)) {
                        self.error = Some(e);
                    }
                }
            }
        }
    }

    /// Saves elsewhere (Q-262): the balances of From and To come again; a
    /// changed accounts list is read again once the form is clean (new
    /// accounts appear in the pickers).
    pub(crate) fn hear_changes(&mut self) {
        let changes = self.link.bus().take(&mut self.bus);
        if changes.lists() {
            self.lists_stale = true;
        }
        if self.lists_stale && !self.is_dirty() && self.accounts_task.is_none() {
            self.lists_stale = false;
            self.start_load();
            self.poll();
        }
        let f = self.form.current();
        if [f.from, f.to]
            .into_iter()
            .flatten()
            .any(|a| changes.ledger_touches(a))
        {
            self.fetch_balances();
            self.recent_for = None;
        }
    }

    /// Unsaved edits.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.form.is_dirty()
    }

    fn sync_inputs(&mut self) {
        let f = self.form.current();
        if let Some(d) = f.date {
            self.date = DateInput::new(d);
        }
        self.amount_text = f
            .amount
            .map(|m| m.format(self.money_style))
            .unwrap_or_default();
        self.amount_problem = None;
        self.rev = self.form.revision();
    }

    /// Picks From (`true`) or To.
    pub(crate) fn set_account(&mut self, from: bool, account: AccountId) {
        if from {
            self.form.edit(field::FROM, |f| f.from = Some(account));
        } else {
            self.form.edit(field::TO, |f| f.to = Some(account));
        }
        self.rev = self.form.revision();
        self.fetch_balances();
    }

    /// Commits the typed amount (cents, Q-232). False when it isn't one.
    pub(crate) fn amount_left(&mut self) -> bool {
        let text = self.amount_text.trim().to_string();
        if text.is_empty() {
            self.form.edit(field::AMOUNT, |f| f.amount = None);
            self.rev = self.form.revision();
            return true;
        }
        match amount_entry::parse(&text, AmountRules::MONEY) {
            Ok(p) => {
                let Some(m) = p.value.as_money() else {
                    return false;
                };
                if self.form.current().amount != Some(m) {
                    self.form.edit(field::AMOUNT, |f| f.amount = Some(m));
                }
                self.amount_text = m.format(self.money_style);
                self.amount_problem = None;
                self.rev = self.form.revision();
                true
            }
            Err(e) => {
                self.amount_problem = Some(e.key().0);
                false
            }
        }
    }

    /// Checks and saves. Nothing is sent unless the check passes.
    pub(crate) fn start_save(&mut self, intent: SaveIntent, password: Option<Secret>) -> bool {
        if self.save_task.is_some() {
            return false;
        }
        if self.editing.as_ref().is_some_and(|e| e.read_only) {
            self.notice = Some("transfer.read_only");
            return false;
        }
        if self.rev == self.form.revision() && !self.amount_left() {
            return false;
        }
        self.problems.clear();
        self.error = None;
        self.notice = None;
        self.stale = false;
        if !self.is_dirty() {
            self.follow_up(intent);
            return true;
        }
        let c = match check(self.form.current()) {
            Ok(c) => c,
            Err(p) => {
                self.problems = p;
                return false;
            }
        };
        let draft = TransferDraft {
            from: c.from,
            to: c.to,
            date: c.date,
            amount: c.amount,
            memo: c.memo.clone(),
        };
        let posting = match BankPosting::transfer(draft.clone()) {
            Ok(p) => p,
            Err(LocalValidation::SameAccount) => {
                self.problems = vec![TransferProblem::SameAccount];
                return false;
            }
            Err(v) => {
                self.error = Some(ApiError::Local(v));
                return false;
            }
        };
        let old = self.editing.as_ref().map(|e| e.old_date);
        if password.is_none() && needs_password(old, c.date, self.closing) {
            let supported = self.closing_supported();
            if supported {
                self.closing_prompt
                    .ask(Pending::Save(intent), supported, self.closing);
            } else {
                self.closing_prompt.unavailable(supported, self.closing);
            }
            return false;
        }
        if let SaveStart::Busy = self.form.begin_save(intent) {
            return false;
        }
        self.intent = Some(intent);
        self.published = vec![c.from, c.to];
        if let Some(e) = &self.editing {
            self.published.extend(e.accounts.iter().copied());
        }
        let edit = self.editing.as_ref().map(|e| (e.id, e.version));
        let link = Arc::clone(&self.link);
        if let Some(local) = self.local.as_mut() {
            return match local.save(&link, &draft, edit, password.as_ref()) {
                Ok((id, v)) => {
                    self.saved(id, v);
                    true
                }
                Err(e) => {
                    self.save_failed(e);
                    false
                }
            };
        }
        let key = self.key_for(&SaveIdentity {
            posting: posting.clone(),
            edit,
            password: password.is_some(),
        });
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            self.save_failed(ApiError::NoCompanyOpen);
            return false;
        };
        let label = OrphanLabel {
            what: "orphan.transfer_save",
            key: Some(key.clone()),
        };
        match group.write(&session, label, move |s| async move {
            let opts = options(password);
            let v = match edit {
                None => {
                    s.transactions()
                        .create_bank_posting(&posting, &opts, key)
                        .await?
                }
                Some((id, version)) => {
                    s.transactions()
                        .update_bank_posting(id, &version, &posting, &opts, key)
                        .await?
                }
            };
            Ok((v.record.id, v.version))
        }) {
            Ok(p) => self.save_task = Some(Task::Pending(p)),
            Err(_) => self.save_failed(ApiError::Local(LocalValidation::InvalidValue)),
        }
        false
    }

    /// The key for this save: the stored one when the same save (same
    /// posting, same record version, same password-or-not) goes again
    /// after an unknown outcome, a fresh one otherwise.
    pub(crate) fn key_for(&mut self, id: &SaveIdentity) -> IdempotencyKey {
        match &self.key {
            Some((d, k)) if d == id => k.clone(),
            _ => {
                let k = IdempotencyKey::new();
                self.key = Some((id.clone(), k.clone()));
                k
            }
        }
    }

    fn closing_failed(&mut self, e: &ApiError, pending: Pending) -> bool {
        let supported = self.closing_supported();
        self.closing_prompt
            .failed(e, pending, supported, self.closing)
    }

    fn saved(&mut self, id: TransactionId, version: RecordVersion) {
        crate::forms::publish_saved(&self.link, &self.bus, &self.published);
        self.template
            .form_saved(&self.link, self.group.as_ref(), id);
        self.closing_prompt.saved();
        let intent = self.intent.take().unwrap_or(SaveIntent::Save);
        self.saved_count += 1;
        self.last_action = Some("save");
        self.last_saved = Some(id);
        self.key = None;
        let f = self.form.current().clone();
        self.form.save_ok(f.clone());
        let now: Vec<AccountId> = [f.from, f.to].into_iter().flatten().collect();
        if let Some(e) = self.editing.as_mut() {
            e.version = version;
            e.old_date = f.date.unwrap_or(e.old_date);
            e.accounts = now;
        } else if intent == SaveIntent::Save {
            self.editing = Some(Editing {
                id,
                version,
                old_date: f.date.unwrap_or(self.today),
                read_only: false,
                accounts: now,
            });
        }
        self.nav.clear();
        self.recent_for = None;
        self.follow_up(intent);
        self.fetch_balances();
        // Prev/Next asked before the save (the unsaved-changes prompt).
        if let Some(older) = self.after_save_nav.take() {
            self.navigate(older);
        }
    }

    fn follow_up(&mut self, intent: SaveIntent) {
        match intent {
            SaveIntent::SaveAndNew => self.new_transfer(true),
            SaveIntent::SaveAndClose => {
                self.link
                    .push(CompanyEvent::CloseWindow(CommandId::new(KIND)));
            }
            SaveIntent::Save => {}
        }
    }

    pub(crate) fn save_failed(&mut self, e: ApiError) {
        let intent = self.intent.take().unwrap_or(SaveIntent::Save);
        self.after_save_nav = None;
        if !crate::screens::register::keeps_key(&e) {
            self.key = None;
        }
        self.form.save_failed(cl_model::forms::SaveFailure {
            message: cl_core::OwnerFacing::owner_message(&e),
            detail: cl_core::OwnerFacing::scrubbed_detail(&e),
        });
        if self.closing_failed(&e, Pending::Save(intent)) {
            return;
        }
        match &e {
            ApiError::Conflict {
                reason: ConflictReason::Stale,
                ..
            } => self.stale = true,
            _ => self.error = Some(e),
        }
    }

    /// A blank transfer: Save & New keeps From and the date.
    fn new_transfer(&mut self, keep: bool) {
        let f = self.form.current();
        let from = f.from;
        let date = if keep {
            f.date.unwrap_or(self.today)
        } else {
            self.today
        };
        self.form.load(blank(from, date));
        self.editing = None;
        self.template.forget();
        self.nav_at = None;
        self.nav_prompt = None;
        self.focused = None;
        self.void_open = false;
        self.problems.clear();
        self.stale = false;
        self.sync_inputs();
        self.fetch_balances();
    }

    fn clear(&mut self) {
        if self.save_task.is_some() {
            return;
        }
        self.error = None;
        self.notice = None;
        self.last_action = Some("clear");
        self.new_transfer(false);
    }

    fn submit_closing(&mut self) {
        let Some((pending, secret)) = self.closing_prompt.submit() else {
            return;
        };
        match pending {
            Pending::Save(intent) => {
                self.start_save(intent, Some(secret));
                if self.save_task.is_none() && self.closing_prompt.dialog_open() {
                    self.closing_prompt.nothing_sent();
                }
            }
            Pending::Void(reason) => self.send_void(&reason, Some(secret)),
        }
    }

    // ------------------------------------------------------------ records

    pub(crate) fn navigate(&mut self, older: bool) {
        if self.save_task.is_some() || self.load_task.is_some() {
            return;
        }
        if self.is_dirty() {
            // The standard question: Save / Discard changes / Keep editing.
            self.nav_prompt = Some(older);
            return;
        }
        if self.nav.is_empty() {
            self.want_older = Some(older);
            self.fetch_list();
            return;
        }
        self.step(older);
    }

    /// Prev/Next on unsaved changes: the answer to Save / Discard changes /
    /// Keep editing.
    pub(crate) fn answer_nav(&mut self, answer: crate::windows::PromptAnswer) {
        let Some(older) = self.nav_prompt.take() else {
            return;
        };
        match answer {
            crate::windows::PromptAnswer::Keep => {}
            crate::windows::PromptAnswer::Discard => {
                self.form.revert_form();
                self.sync_inputs();
                self.navigate(older);
            }
            crate::windows::PromptAnswer::Save => {
                self.after_save_nav = Some(older);
                let done = self.start_save(SaveIntent::Save, None);
                // Refused or waiting on a question: the move waits for the
                // save (an engine save moves on when it finishes).
                if !done && self.save_task.is_none() {
                    self.after_save_nav = None;
                }
            }
        }
    }

    /// Esc: an open question or Void card closes, then the focused field
    /// goes back to its saved value; with nothing to revert the shell
    /// closes the window (asking first when the form is dirty).
    pub(crate) fn revert_field(&mut self) -> bool {
        if self.nav_prompt.is_some() {
            self.answer_nav(crate::windows::PromptAnswer::Keep);
            return true;
        }
        if self.void_open {
            self.void_open = false;
            return true;
        }
        let Some(f) = self.focused else { return false };
        let saved = self.form.original().clone();
        let now = self.form.current();
        let differs = match f {
            field::FROM => now.from != saved.from,
            field::TO => now.to != saved.to,
            field::DATE => now.date != saved.date,
            field::AMOUNT => now.amount != saved.amount,
            field::MEMO => now.memo != saved.memo,
            _ => false,
        };
        // An amount typed but not yet taken also goes back.
        let typed = f == field::AMOUNT
            && self.amount_text.trim()
                != saved
                    .amount
                    .map(|m| m.format(self.money_style))
                    .unwrap_or_default();
        if !differs && !typed {
            return false;
        }
        if differs {
            self.form.edit_step(|v| match f {
                field::FROM => v.from = saved.from,
                field::TO => v.to = saved.to,
                field::DATE => v.date = saved.date,
                field::AMOUNT => v.amount = saved.amount,
                _ => v.memo.clone_from(&saved.memo),
            });
        }
        self.sync_inputs();
        if matches!(f, field::FROM | field::TO) {
            self.fetch_balances();
        }
        true
    }

    fn track(&mut self, r: &egui::Response, f: u64) {
        if r.has_focus() {
            self.focused = Some(f);
        } else if self.focused == Some(f) {
            self.focused = None;
        }
    }

    fn step(&mut self, older: bool) {
        let target = match (self.nav_at, older) {
            (None, true) => Some(0),
            (None, false) => return,
            (Some(i), true) => Some(i + 1),
            (Some(0), false) => None,
            (Some(i), false) => Some(i - 1),
        };
        match target {
            None => self.new_transfer(true),
            Some(i) if i < self.nav.len() => {
                self.nav_at = Some(i);
                let id = self.nav[i].id;
                self.load(id);
            }
            Some(_) => self.notice = Some("transfer.nav.oldest"),
        }
    }

    /// The transfers touching `account`, newest first.
    fn list_task(&mut self, account: AccountId) -> Option<Task<Vec<TransferRow>>> {
        let link = Arc::clone(&self.link);
        if let Some(local) = self.local.as_mut() {
            return Some(Task::ready(local.list(&link, account)));
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return None;
        };
        group
            .read(&session, move |s| async move {
                let filter = TransactionFilter {
                    txn_type: Some(TxnType::Transfer),
                    account_id: Some(account),
                    sort: Some((cl_api::TxnSort::Date, cl_api::SortDir::Desc)),
                    ..TransactionFilter::default()
                };
                let page = s
                    .transactions()
                    .list(&filter, PageRequest::new(50, 0)?)
                    .await?;
                Ok(page
                    .data
                    .into_iter()
                    .map(|r| TransferRow {
                        id: r.id,
                        date: r.txn_date,
                        amount: r.display_total.or(r.total),
                        void: r.status == TxnStatus::Void,
                    })
                    .collect())
            })
            .ok()
            .map(Task::Pending)
    }

    fn fetch_list(&mut self) {
        let Some(account) = self.form.current().from else {
            return;
        };
        self.nav_task = self.list_task(account);
        self.poll();
    }

    /// The side panel's recent transfers of the From account.
    fn fetch_recent(&mut self) {
        let from = self.form.current().from;
        self.recent_for = from;
        match from {
            Some(account) => self.recent_task = self.list_task(account),
            None => self.recent.clear(),
        }
    }

    fn load(&mut self, id: TransactionId) {
        let link = Arc::clone(&self.link);
        if let Some(local) = self.local.as_mut() {
            match local.get(&link, id) {
                Ok(v) => self.show(&v),
                Err(e) => self.error = Some(e),
            }
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        if let Ok(p) = group.read(
            &session,
            move |s| async move { s.transactions().get(id).await },
        ) {
            self.load_task = Some(Task::Pending(p));
        }
    }

    fn show(&mut self, v: &Versioned<Transaction>) {
        let (form, exact) = form_of(&v.record);
        self.form.load(form);
        self.template.forget();
        let mut accounts: Vec<AccountId> = v.record.lines.iter().map(|l| l.account_id).collect();
        accounts.sort();
        accounts.dedup();
        self.editing = Some(Editing {
            id: v.record.id,
            version: v.version,
            old_date: v.record.txn_date,
            read_only: !exact || v.record.status == TxnStatus::Void,
            accounts,
        });
        self.focused = None;
        self.last_action = Some("load");
        self.problems.clear();
        self.error = None;
        self.sync_inputs();
        self.fetch_balances();
    }

    fn send_void(&mut self, reason: &str, password: Option<Secret>) {
        let Some(e) = self.editing.clone() else {
            self.notice = Some("transfer.void.saved_only");
            return;
        };
        let reason = reason.trim().to_string();
        if reason.is_empty() {
            self.notice = Some("transfer.void.reason_needed");
            return;
        }
        self.void_reason.clone_from(&reason);
        if password.is_none() && is_locked(e.old_date, self.closing) {
            let supported = self.closing_supported();
            if supported {
                self.closing_prompt
                    .ask(Pending::Void(reason), supported, self.closing);
            } else {
                self.closing_prompt.unavailable(supported, self.closing);
            }
            return;
        }
        let f = self.form.current();
        self.published = [f.from, f.to].into_iter().flatten().collect();
        self.published.extend(e.accounts.iter().copied());
        let link = Arc::clone(&self.link);
        if let Some(local) = self.local.as_mut() {
            match local.void(&link, e.id, password.as_ref()) {
                Ok(()) => self.voided(),
                Err(err) => {
                    if !self.closing_failed(&err, Pending::Void(reason)) {
                        self.error = Some(err);
                    }
                }
            }
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        // A fresh key per try is safe: void is idempotent at the engine.
        let key = IdempotencyKey::new();
        let label = OrphanLabel {
            what: "orphan.transfer_void",
            key: Some(key.clone()),
        };
        if let Ok(p) = group.write(&session, label, move |s| async move {
            s.transactions()
                .void(e.id, &reason, &options(password), key)
                .await?;
            Ok(())
        }) {
            self.void_task = Some(Task::Pending(p));
        }
    }

    fn voided(&mut self) {
        crate::forms::publish_saved(&self.link, &self.bus, &self.published);
        self.closing_prompt.saved();
        self.void_open = false;
        self.void_reason.clear();
        self.last_action = Some("void");
        self.nav.clear();
        self.recent_for = None;
        self.new_transfer(true);
    }

    // ------------------------------------------------------------ drawing

    /// The balance label and figure: "Balance" and the stored balance, or
    /// for a card or other liability "Balance owed" with the stored sign
    /// turned (Q-241). `None` until the engine's balance arrives.
    fn balance_parts(&self, i: usize, account: Option<AccountId>) -> Option<(String, String)> {
        let m = self.balances[i]?;
        let owed = account
            .and_then(|a| self.class(a))
            .is_some_and(|c| matches!(c, AccountClass::CreditCard | AccountClass::OtherLiability));
        if owed {
            let shown = m.checked_neg().unwrap_or(m);
            Some((t("transfer.balance_owed"), shown.format(self.money_style)))
        } else {
            Some((t("transfer.balance"), m.format(self.money_style)))
        }
    }

    fn account_ui(&mut self, ui: &mut Ui, tokens: &Tokens, from: bool, enabled: bool) {
        let options: Vec<(AccountId, String)> = self
            .transfer_accounts()
            .into_iter()
            .map(|a| (a.id, account_text(a)))
            .collect();
        let f = self.form.current();
        let current = if from { f.from } else { f.to };
        let shown = options
            .iter()
            .find(|(id, _)| Some(*id) == current)
            .map(|(_, s)| s.clone())
            .unwrap_or_default();
        let mut picked = None;
        kit::wrap_item(ui, ("transfer/account", from), |ui| {
            let l = kit::above(
                ui,
                tokens,
                &t(if from { "transfer.from" } else { "transfer.to" }),
            );
            let r = ui.add_enabled_ui(enabled, |ui| {
                // Q-250: a click opens the list.
                egui::ComboBox::from_id_salt(if from { "transfer/from" } else { "transfer/to" })
                    .width(260.0)
                    .selected_text(shown)
                    .show_ui(ui, |ui| {
                        for (id, text) in &options {
                            if ui.selectable_label(Some(*id) == current, text).clicked() {
                                picked = Some(*id);
                            }
                        }
                    })
                    .response
                    .labelled_by(l.id)
            });
            self.track(&r.inner, if from { field::FROM } else { field::TO });
            if let Some((label, figure)) = self.balance_parts(usize::from(!from), current) {
                kit::balance_line(ui, tokens, &label, Some(figure));
            }
        });
        if let Some(id) = picked {
            self.set_account(from, id);
        }
    }

    fn draw(&mut self, ui: &mut Ui) {
        let tokens = Tokens::of(ui.ctx());
        if !self.loaded {
            form::progress_line(ui, &tokens, &t("transfer.loading"));
            return;
        }
        let working = self.save_task.is_some();
        if let Some(cmd) = kit::toolbar(ui, &tokens, !working, self.is_dirty(), |ui| {
            self.template_slot(ui);
        }) {
            self.command(CommandId::new(cmd));
        }
        match self.closing_prompt.ui(ui, &tokens) {
            ClosingClick::Submit => self.submit_closing(),
            ClosingClick::Cancel => self.closing_prompt.cancel(),
            ClosingClick::None => {}
        }
        if let Some(k) = self.notice {
            form::hint(ui, &tokens, &t(k));
        }
        if self.void_open {
            let mut send = false;
            let mut cancel = false;
            form::card(ui, &tokens, |ui| {
                form::heading(ui, &tokens, &t("transfer.void.title"));
                form::field(ui, &t("transfer.void.reason"), &mut self.void_reason);
                ui.horizontal(|ui| {
                    send = ui.button(t("transfer.void.confirm")).clicked();
                    cancel = ui.button(t("transfer.void.cancel")).clicked();
                });
            });
            if cancel {
                self.void_open = false;
            } else if send {
                let r = self.void_reason.clone();
                self.send_void(&r, None);
            }
        }
        if self.transfer_accounts().len() < 2 {
            form::hint(ui, &tokens, &t("transfer.no_accounts"));
            return;
        }
        kit::two_columns(ui, "transfer/scroll", |ui, col| match col {
            kit::Column::Form => self.form_ui(ui, &tokens),
            kit::Column::Panel => self.panel_ui(ui, &tokens),
        });
        if self.nav_prompt.is_some()
            && let Some(a) =
                crate::forms::nav_prompt::ui(ui, "transfer", &t("cmd.banking.transfer"))
        {
            self.answer_nav(a);
        }
    }

    fn form_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let working = self.save_task.is_some();
        kit::title_only(ui, tokens, &t("cmd.banking.transfer"));
        ui.add_space(6.0);
        let read_only = self.editing.as_ref().is_some_and(|e| e.read_only);
        if read_only {
            form::hint(ui, tokens, &t("transfer.read_only"));
        } else if self
            .editing
            .as_ref()
            .is_some_and(|e| is_locked(e.old_date, self.closing))
        {
            form::hint(ui, tokens, &t("closing.edit_note"));
        }
        let enabled = !working && !self.closing_prompt.dialog_open() && !read_only;
        // A read-only transfer keeps readable text (contrast >= 4.5:1).
        crate::forms::header::locked_scope(ui, true, read_only, |ui| {
            form::card(ui, tokens, |ui| {
                crate::forms::header::wrap_row(ui, |ui| {
                    self.account_ui(ui, tokens, true, enabled);
                    kit::wrap_item(ui, "transfer/arrow", |ui| {
                        ui.add_space(22.0);
                        ui.label(RichText::new("\u{2192}").size(20.0));
                    });
                    self.account_ui(ui, tokens, false, enabled);
                });
                ui.add_space(8.0);
                ui.add_enabled_ui(enabled, |ui| {
                    crate::forms::header::wrap_row(ui, |ui| {
                        kit::wrap_item(ui, "transfer/date", |ui| {
                            let l = kit::above(ui, tokens, &t("transfer.date"));
                            let before = self.date.text.clone();
                            let r = ui
                                .add(
                                    egui::TextEdit::singleline(&mut self.date.text)
                                        .desired_width(110.0)
                                        .font(egui::TextStyle::Monospace),
                                )
                                .labelled_by(l.id);
                            if r.changed() {
                                self.date.typed(&before, self.today);
                                let v = self.date.value;
                                if v.is_some() {
                                    self.form.edit(field::DATE, |f| f.date = v);
                                    self.rev = self.form.revision();
                                }
                            }
                            if r.lost_focus() {
                                self.date.settle();
                            }
                            self.track(&r, field::DATE);
                        });
                        kit::wrap_item(ui, "transfer/amount", |ui| {
                            let l = kit::above(ui, tokens, &t("transfer.amount"));
                            let r = ui
                                .add(
                                    egui::TextEdit::singleline(&mut self.amount_text)
                                        .desired_width(130.0)
                                        .font(egui::TextStyle::Monospace)
                                        .align(egui::Align2::RIGHT_CENTER),
                                )
                                .labelled_by(l.id);
                            if r.lost_focus() {
                                self.amount_left();
                            }
                            self.track(&r, field::AMOUNT);
                            if let Some(k) = self.amount_problem {
                                ui.label(RichText::new(t(k)).size(12.0).color(tokens.warn_text));
                            }
                        });
                        kit::wrap_item(ui, "transfer/memo", |ui| {
                            let l = kit::above(ui, tokens, &t("transfer.memo"));
                            let mut memo = self.form.current().memo.clone();
                            let r = ui
                                .add(egui::TextEdit::singleline(&mut memo).desired_width(260.0))
                                .labelled_by(l.id);
                            if r.changed() {
                                self.form.edit(field::MEMO, |f| f.memo = memo);
                                self.rev = self.form.revision();
                            }
                            self.track(&r, field::MEMO);
                        });
                    });
                });
            });
        });
        let keys: Vec<&'static str> = self.problems.iter().map(|p| p.key().0).collect();
        kit::problems(ui, tokens, &keys);
        if self.stale {
            ui.label(RichText::new(t("transfer.changed_elsewhere")).color(tokens.warn_text));
        }
        if let Some(e) = &self.error {
            form::inline_error(ui, tokens, e);
        }
        ui.add_space(12.0);
        let idle = !working && !self.closing_prompt.dialog_open();
        if let Some(c) = kit::actions_row(ui, idle && !read_only, idle) {
            self.command(CommandId::new(c));
        }
    }

    /// The side panel: both accounts with the engine's balances, and the
    /// From account's newest transfers (as the engine lists them).
    fn panel_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let f = self.form.current().clone();
        kit::panel_card(ui, tokens, &t("transfer.panel.balances"), |ui| {
            for (i, account, key) in [(0, f.from, "transfer.from"), (1, f.to, "transfer.to")] {
                ui.add_space(4.0);
                ui.label(
                    RichText::new(t(key))
                        .size(12.0)
                        .color(tokens.text_secondary),
                );
                let Some(a) = account else {
                    form::hint(ui, tokens, &t("transfer.panel.not_chosen"));
                    continue;
                };
                let name = self
                    .accounts
                    .iter()
                    .find(|x| x.id == a)
                    .map(account_text)
                    .unwrap_or_default();
                ui.add(egui::Label::new(RichText::new(name).color(tokens.text_primary)).wrap());
                match self.balance_parts(i, Some(a)) {
                    Some((label, figure)) => kit::balance_line(ui, tokens, &label, Some(figure)),
                    None => kit::balance_line(ui, tokens, &t("transfer.balance"), None),
                }
            }
        });
        ui.add_space(12.0);
        kit::panel_card(ui, tokens, &t("transfer.panel.recent"), |ui| {
            if self.recent.is_empty() {
                form::hint(ui, tokens, &t("transfer.panel.recent.none"));
            }
            for r in &self.recent {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!(
                            "{:02}/{:02}/{}",
                            r.date.month(),
                            r.date.day(),
                            r.date.year()
                        ))
                        .size(12.0)
                        .color(tokens.text_secondary),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // The engine's amount as listed; nothing added up.
                        let amount = r
                            .amount
                            .map(|m| m.format(self.money_style))
                            .unwrap_or_default();
                        ui.label(
                            RichText::new(amount)
                                .text_style(egui::TextStyle::Monospace)
                                .color(tokens.text_primary),
                        );
                    });
                });
            }
        });
    }

    /// The control-channel state: flags, ids and the typed amount the
    /// scenario checks.
    fn state_json(&self) -> Value {
        let f = self.form.current();
        let mut v = json!({
            "screen": "transfer",
            "company_open": self.tenant.is_some(),
            "loaded": self.loaded,
            "mode": if self.editing.is_some() { "edit" } else { "new" },
            "read_only": self.editing.as_ref().is_some_and(|e| e.read_only),
            "from_id": f.from.map(|a| a.to_string()),
            "to_id": f.to.map(|a| a.to_string()),
            "date": f.date.map(CalendarDate::to_wire),
            "amount": f.amount.map(|m| m.format(cl_core::MoneyStyle::default())),
            "balances_shown": [self.balances[0].is_some(), self.balances[1].is_some()],
            "recent_shown": self.recent.len(),
            "dirty": self.is_dirty(),
            "saving": self.save_task.is_some(),
            "saved_count": self.saved_count,
            "txn_id": self.last_saved.map(|id| id.to_string()),
            "save": {
                "refused": self.problems.first().map(|p| p.name()),
                "error": self.error.is_some(),
                "stale": self.stale,
            },
            "published": self.published.iter().map(ToString::to_string).collect::<Vec<_>>(),
            "last_action": self.last_action,
            "notice": self.notice,
            "void_open": self.void_open,
            "nav_at": self.nav_at,
            "nav_prompt": self.nav_prompt.is_some(),
            "template": self.template.state_json(),
        });
        if let (Value::Object(m), Value::Object(c)) = (
            &mut v,
            self.closing_prompt.state_json(self.closing_supported()),
        ) {
            m.extend(c);
        }
        v
    }
}

impl Screen for TransferScreen {
    fn title(&self) -> String {
        t("cmd.banking.transfer")
    }

    fn ui(&mut self, ui: &mut Ui) {
        let style = crate::grid_env::shell_frame(ui.ctx()).money_style;
        if style != self.money_style {
            self.money_style = style;
            self.rev = u64::MAX;
        }
        self.poll();
        self.hear_changes();
        if self.rev != self.form.revision() {
            self.sync_inputs();
        }
        self.draw(ui);
        // A refused save's problems follow the owner's fixes.
        self.recheck_problems();
        let busy = self.accounts_task.is_some()
            || self.save_task.is_some()
            || self.load_task.is_some()
            || self.nav_task.is_some()
            || self.void_task.is_some()
            || self.recent_task.is_some()
            || self.balance_tasks.iter().any(Option::is_some);
        if busy {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    fn scope(&self) -> Scope {
        Scope::Form
    }

    fn dirty(&self) -> bool {
        self.is_dirty()
    }

    fn save(&mut self) -> bool {
        self.start_save(SaveIntent::Save, None)
    }

    fn command(&mut self, id: CommandId) -> bool {
        if let Some(h) = self.closing_prompt.command(id) {
            return h;
        }
        match id.as_str() {
            KIND => true,
            "form.save_new" => {
                self.start_save(SaveIntent::SaveAndNew, None);
                true
            }
            "form.save_close" => {
                self.start_save(SaveIntent::SaveAndClose, None);
                true
            }
            "form.clear" => {
                self.clear();
                true
            }
            "form.prev_record" => {
                self.navigate(true);
                true
            }
            "form.next_record" => {
                self.navigate(false);
                true
            }
            "edit.void_transaction" => {
                if self.editing.is_some() {
                    self.void_open = true;
                } else {
                    self.notice = Some("transfer.void.saved_only");
                }
                true
            }
            "edit.undo" => {
                let d = self.form.undo();
                if d {
                    self.sync_inputs();
                }
                d
            }
            "edit.redo" => {
                let d = self.form.redo();
                if d {
                    self.sync_inputs();
                }
                d
            }
            "form.revert" => self.revert_field(),
            "form.save_as_template" => {
                self.save_as_template();
                true
            }

            _ => false,
        }
    }

    fn open_template(&mut self, t: crate::forms::template::TemplateOpen) -> bool {
        self.fill_template(&t)
    }

    fn open_record(&mut self, txn: TransactionId) -> bool {
        self.load(txn);
        self.editing.as_ref().is_some_and(|e| e.id == txn) || self.load_task.is_some()
    }

    fn state(&self) -> Value {
        self.state_json()
    }
}

impl TransferScreen {
    /// A refused save's problems, checked again against the form as it is
    /// now (a fixed problem goes away at once, P13).
    pub(crate) fn recheck_problems(&mut self) {
        if self.problems.is_empty() {
            return;
        }
        let now = check(self.form.current()).err().unwrap_or_default();
        crate::forms::recheck(&mut self.problems, now);
    }
}

#[cfg(test)]
mod tests;
