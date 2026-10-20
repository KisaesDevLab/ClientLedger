//! Transfer Funds (P14 lane A, `banking.transfer`, Ctrl+T), on the Write
//! Check pattern (Q-261): From (with the engine's balance) → To (with its
//! balance) · Date · Amount · Memo, Clear / Save & New / Save & Close,
//! Prev/Next over the From account's transfers, Void and the shared
//! closing-date prompt.
//!
//! Money: the amount is the owner's typed `Money` (cents, Q-232);
//! `cl_model::forms::transfer::check` is the gate and
//! `BankPosting::transfer` refuses again before anything is sent. The
//! balances are the engine's; a card or other liability shows "Balance
//! owed", the stored balance with its sign turned for display (Q-241).
//! Every save and void publishes both accounts on the data bus (Q-262).

mod engine;

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
use eframe::egui::{self, Align, Layout, RichText, Ui, Vec2};
use serde_json::{Value, json};

use crate::company::{CompanyEvent, CompanyLink, Task};
use crate::forms::closing_prompt::{ClosingClick, ClosingPrompt};
use crate::i18n::t;
use crate::screens::company::form::{self, DateInput};
use crate::screens::register::account_text;
use crate::theme::Tokens;
use crate::windows::Screen;

pub(crate) use engine::{DemoTransfers, LocalTransfers, TransferRow};

/// The command and window kind.
pub const KIND: &str = "banking.transfer";

/// The height of Clear / Save & New / Save & Close.
const ACTION_HEIGHT: f32 = 44.0;

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
    key: Option<(BankPosting, IdempotencyKey)>,
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
    nav_at: Option<usize>,
    want_older: Option<bool>,
    void_open: bool,
    void_reason: String,
    void_task: Option<Task<()>>,
    pub(crate) closing_prompt: ClosingPrompt<Pending>,
    money_style: cl_core::MoneyStyle,
    bus: crate::data_bus::Subscription,
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
            nav_at: None,
            want_older: None,
            void_open: false,
            void_reason: String::new(),
            void_task: None,
            closing_prompt: ClosingPrompt::default(),
            money_style: cl_core::MoneyStyle::default(),
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

    /// Saves elsewhere (Q-262): the balances of From and To come again.
    fn hear_changes(&mut self) {
        let changes = self.link.bus().take(&mut self.bus);
        let f = self.form.current();
        if [f.from, f.to]
            .into_iter()
            .flatten()
            .any(|a| changes.ledger_touches(a))
        {
            self.fetch_balances();
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
        let key = match &self.key {
            Some((p, k)) if *p == posting => k.clone(),
            _ => {
                let k = IdempotencyKey::new();
                self.key = Some((posting.clone(), k.clone()));
                k
            }
        };
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

    fn closing_failed(&mut self, e: &ApiError, pending: Pending) -> bool {
        let supported = self.closing_supported();
        self.closing_prompt
            .failed(e, pending, supported, self.closing)
    }

    fn saved(&mut self, id: TransactionId, version: RecordVersion) {
        crate::forms::publish_saved(&self.link, &self.bus, &self.published);
        self.closing_prompt.saved();
        let intent = self.intent.take().unwrap_or(SaveIntent::Save);
        self.saved_count += 1;
        self.last_action = Some("save");
        self.last_saved = Some(id);
        self.key = None;
        let f = self.form.current().clone();
        self.form.save_ok(f.clone());
        if let Some(e) = self.editing.as_mut() {
            e.version = version;
            e.old_date = f.date.unwrap_or(e.old_date);
        } else if intent == SaveIntent::Save {
            self.editing = Some(Editing {
                id,
                version,
                old_date: f.date.unwrap_or(self.today),
                read_only: false,
            });
        }
        self.nav.clear();
        self.follow_up(intent);
        self.fetch_balances();
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

    fn save_failed(&mut self, e: ApiError) {
        let intent = self.intent.take().unwrap_or(SaveIntent::Save);
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
        self.nav_at = None;
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

    fn navigate(&mut self, older: bool) {
        if self.save_task.is_some() || self.load_task.is_some() {
            return;
        }
        if self.is_dirty() {
            self.notice = Some("transfer.nav.unsaved");
            return;
        }
        if self.nav.is_empty() {
            self.want_older = Some(older);
            self.fetch_list();
            return;
        }
        self.step(older);
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

    fn fetch_list(&mut self) {
        let Some(account) = self.form.current().from else {
            return;
        };
        let link = Arc::clone(&self.link);
        if let Some(local) = self.local.as_mut() {
            self.nav_task = Some(Task::ready(local.list(&link, account)));
            self.poll();
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        if let Ok(p) = group.read(&session, move |s| async move {
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
                })
                .collect())
        }) {
            self.nav_task = Some(Task::Pending(p));
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
        self.editing = Some(Editing {
            id: v.record.id,
            version: v.version,
            old_date: v.record.txn_date,
            read_only: !exact || v.record.status == TxnStatus::Void,
        });
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
        self.new_transfer(true);
    }

    // ------------------------------------------------------------ drawing

    /// The balance text: "Balance 1,234.00" or, for a card or other
    /// liability, "Balance owed" with the stored sign turned (Q-241).
    fn balance_text(&self, i: usize, account: Option<AccountId>) -> String {
        let Some(m) = self.balances[i] else {
            return String::new();
        };
        let owed = account
            .and_then(|a| self.class(a))
            .is_some_and(|c| matches!(c, AccountClass::CreditCard | AccountClass::OtherLiability));
        if owed {
            let shown = m.checked_neg().unwrap_or(m);
            format!(
                "{} {}",
                t("transfer.balance_owed"),
                shown.format(self.money_style)
            )
        } else {
            format!("{} {}", t("transfer.balance"), m.format(self.money_style))
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
        ui.vertical(|ui| {
            let l = ui.add(
                egui::Label::new(
                    RichText::new(t(if from { "transfer.from" } else { "transfer.to" }))
                        .size(12.0)
                        .color(tokens.text_secondary),
                )
                .selectable(false),
            );
            ui.add_enabled_ui(enabled, |ui| {
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
                    .labelled_by(l.id);
            });
            let text = self.balance_text(usize::from(!from), current);
            ui.label(RichText::new(text).text_style(egui::TextStyle::Monospace));
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
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(!working, egui::Button::new(t("transfer.prev")))
                .clicked()
            {
                self.navigate(true);
            }
            if ui
                .add_enabled(!working, egui::Button::new(t("transfer.next")))
                .clicked()
            {
                self.navigate(false);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if self.is_dirty() {
                    ui.label(
                        RichText::new(t("transfer.unsaved"))
                            .size(12.0)
                            .color(tokens.text_tertiary),
                    );
                }
            });
        });
        ui.separator();
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
        let read_only = self.editing.as_ref().is_some_and(|e| e.read_only);
        if read_only {
            form::hint(ui, &tokens, &t("transfer.read_only"));
        } else if self
            .editing
            .as_ref()
            .is_some_and(|e| is_locked(e.old_date, self.closing))
        {
            form::hint(ui, &tokens, &t("closing.edit_note"));
        }
        let enabled = !working && !self.closing_prompt.dialog_open() && !read_only;
        form::card(ui, &tokens, |ui| {
            ui.horizontal_top(|ui| {
                self.account_ui(ui, &tokens, true, enabled);
                ui.label(RichText::new("\u{2192}").size(20.0));
                self.account_ui(ui, &tokens, false, enabled);
            });
            ui.add_space(8.0);
            ui.add_enabled_ui(enabled, |ui| {
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        let l = ui.label(RichText::new(t("transfer.date")).size(12.0));
                        let before = self.date.text.clone();
                        let r = ui
                            .add(
                                egui::TextEdit::singleline(&mut self.date.text)
                                    .desired_width(110.0),
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
                    });
                    ui.vertical(|ui| {
                        let l = ui.label(RichText::new(t("transfer.amount")).size(12.0));
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
                        if let Some(k) = self.amount_problem {
                            ui.label(RichText::new(t(k)).size(12.0).color(tokens.warn_text));
                        }
                    });
                    ui.vertical(|ui| {
                        let l = ui.label(RichText::new(t("transfer.memo")).size(12.0));
                        let mut memo = self.form.current().memo.clone();
                        let r = ui
                            .add(egui::TextEdit::singleline(&mut memo).desired_width(260.0))
                            .labelled_by(l.id);
                        if r.changed() {
                            self.form.edit(field::MEMO, |f| f.memo = memo);
                            self.rev = self.form.revision();
                        }
                    });
                });
            });
        });
        for p in &self.problems {
            ui.label(RichText::new(t(p.key().0)).color(tokens.warn_text));
        }
        if self.stale {
            ui.label(RichText::new(t("transfer.changed_elsewhere")).color(tokens.warn_text));
        }
        if let Some(e) = &self.error {
            form::inline_error(ui, &tokens, e);
        }
        ui.add_space(12.0);
        let mut clicked = None;
        let idle = !working && !self.closing_prompt.dialog_open();
        ui.allocate_ui_with_layout(
            Vec2::new(ui.available_width(), ACTION_HEIGHT),
            Layout::right_to_left(Align::Center),
            |ui| {
                let can = idle && !read_only;
                if ui
                    .add_enabled(
                        can,
                        egui::Button::new(t("transfer.save_close"))
                            .min_size(Vec2::new(0.0, ACTION_HEIGHT)),
                    )
                    .clicked()
                {
                    clicked = Some("form.save_close");
                }
                if ui
                    .add_enabled(
                        can,
                        egui::Button::new(t("transfer.save_new"))
                            .min_size(Vec2::new(0.0, ACTION_HEIGHT)),
                    )
                    .clicked()
                {
                    clicked = Some("form.save_new");
                }
                if ui
                    .add_enabled(
                        idle,
                        egui::Button::new(t("transfer.clear"))
                            .min_size(Vec2::new(0.0, ACTION_HEIGHT)),
                    )
                    .clicked()
                {
                    clicked = Some("form.clear");
                }
            },
        );
        if let Some(c) = clicked {
            self.command(CommandId::new(c));
        }
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
        let busy = self.accounts_task.is_some()
            || self.save_task.is_some()
            || self.load_task.is_some()
            || self.nav_task.is_some()
            || self.void_task.is_some()
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
            "form.revert" if self.void_open => {
                self.void_open = false;
                true
            }
            _ => false,
        }
    }

    fn open_record(&mut self, txn: TransactionId) -> bool {
        self.load(txn);
        self.editing.as_ref().is_some_and(|e| e.id == txn) || self.load_task.is_some()
    }

    fn state(&self) -> Value {
        self.state_json()
    }
}

#[cfg(test)]
mod tests;
