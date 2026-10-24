//! What the P14 lane C helper forms share (Record Card Charge, Loan
//! Payment, Pay Yourself or Put Money In, Paid with Personal Funds): the
//! company's lists and equity roles ([`Books`]), the engine's balances
//! ([`Balances`]), one save and void path over a checked
//! [`cl_api::BankPosting`] with the closing-date password ([`SaveFlow`]),
//! and the Write Check field widgets (label above, mono right-aligned
//! amounts, the Q-250 click-opens-list name field, the action row).
//!
//! Nothing here adds money up: the forms' figures come from `cl_model`
//! (exact) and the balances are the engine's, re-read after every save and
//! whenever another window publishes a change (Q-262).

use std::collections::BTreeMap;
use std::sync::Arc;

use cl_api::{
    Account, AccountFilter, ApiError, BankPosting, Capabilities, ClosedPeriodAuthorization,
    ConflictReason, Contact, ContactFilter, IdempotencyKey, OrphanLabel, PostingOptions,
    RecordVersion, RequestGroup, Tag, TagFilter, Transaction, TxnStatus, Versioned,
};
use cl_core::ids::{AccountId, ContactId, TagId, TransactionId};
use cl_core::{CalendarDate, Money, MoneyStyle, Secret};
use cl_grid::{LookupResult, Lookups};
use cl_model::amount_entry::{self, AmountRules};
use cl_model::closing::{is_locked, needs_password};
use cl_model::forms::SaveIntent;
use cl_model::forms::owner_money::{Entity, Role};
use cl_model::lists::tags::{TagEntry, TagList};
use cl_model::register::AccountClass;
use eframe::egui::{self, Align, Layout, RichText, Ui, Vec2};

use crate::company::demo::register::DemoPosting;
use crate::company::{CompanyLink, Task};
use crate::forms::closing_prompt::{ClosingClick, ClosingPrompt};
use crate::forms::header;
use crate::i18n::{t, t_with};
use crate::screens::company::form::{self, DateInput};
use crate::screens::register::lists::{NAMES, RegisterLists};
use crate::theme::Tokens;

/// How many names the list shows while typing.
const NAME_SUGGESTIONS: usize = 8;
/// The height of Clear / Save & New / Save & Close (DESIGN.md: 44 px).
pub(crate) const ACTION_HEIGHT: f32 = 44.0;
/// Below this width the side panel goes under the form.
pub(crate) const SIDE_BY_SIDE: f32 = 1100.0;
/// The side panel's width beside the form.
pub(crate) const PANEL_WIDTH: f32 = 270.0;

// ------------------------------------------------------------------ books

/// The open company's accounts, names, tags, closing date, entity type and
/// equity roles, read once when the form opens (and again after another
/// window changed the lists while this form was clean).
#[derive(Default)]
pub(crate) struct Books {
    pub accounts: Vec<Account>,
    pub contacts: Vec<Contact>,
    pub tags: Vec<Tag>,
    pub lists: Arc<RegisterLists>,
    pub tag_list: TagList,
    pub closing: Option<CalendarDate>,
    /// The legal form (`None`: unknown; the defaults then fall back).
    pub entity: Option<Entity>,
    /// The engine's equity role accounts (`equity-roles-v1`; empty
    /// without it, Q-P14m).
    pub roles: BTreeMap<Role, AccountId>,
    /// The roles came from the engine (or its demo stand-in).
    pub roles_supported: bool,
    /// The account list has arrived (or can't come).
    pub loaded: bool,
    accounts_task: Option<Task<Vec<Account>>>,
    contacts_task: Option<Task<Vec<Contact>>>,
    tags_task: Option<Task<Vec<Tag>>>,
    closing_task: Option<Task<Option<CalendarDate>>>,
    entity_task: Option<Task<EntityAnswer>>,
}

/// The entity type, the equity role accounts, and whether the engine gave
/// the roles.
type EntityAnswer = (Option<Entity>, BTreeMap<Role, AccountId>, bool);

/// The engine's entity type as the form model knows it.
pub(crate) fn entity_of(e: cl_api::EntityType) -> Entity {
    match e {
        cl_api::EntityType::SoleProp => Entity::SoleProp,
        cl_api::EntityType::SingleMemberLlc => Entity::SingleMemberLlc,
        cl_api::EntityType::Partnership => Entity::Partnership,
        cl_api::EntityType::SCorp => Entity::SCorp,
        cl_api::EntityType::CCorp => Entity::CCorp,
    }
}

/// The demo engine's equity roles (`company::demo::money`, its
/// `equity-roles-v1` twin) as the form model knows them: they follow the
/// demo company's entity type.
pub(crate) fn demo_roles(
    r: &crate::company::demo::money::DemoEquityRoles,
) -> BTreeMap<Role, AccountId> {
    use crate::company::demo::money::EquityRole as E;
    let pairs = [
        (E::OwnerDraw, Role::OwnerDraw),
        (E::OwnerContribution, Role::OwnerContribution),
        (E::PartnerDraws, Role::PartnerDraws),
        (E::PartnerContributions, Role::PartnerContributions),
        (E::ShareholderDistributions, Role::ShareholderDistributions),
        (E::PaidInCapital, Role::PaidInCapital),
    ];
    pairs
        .into_iter()
        .filter_map(|(e, role)| r.account(e).map(|a| (role, a)))
        .collect()
}

/// The tag list `check` needs for the header-tag defaults.
pub(crate) fn tag_list(tags: &[Tag]) -> TagList {
    TagList::new(
        Vec::new(),
        tags.iter()
            .map(|t| TagEntry {
                id: t.id,
                name: t.fields.name.clone(),
                group: t.fields.group_id,
                color: t.fields.color.clone(),
                description: t.fields.description.clone(),
                active: t.is_active,
                usage: t.usage_count,
            })
            .collect(),
    )
}

impl Books {
    /// Starts reading everything (the demo answers at once).
    pub(crate) fn start(&mut self, link: &CompanyLink, group: Option<&RequestGroup>) {
        if let Some(mut demo) = link.demo_state() {
            // The roles first: the demo engine may seed role accounts.
            let demo_equity = crate::company::demo::money::equity_roles(&mut demo);
            let chart = demo
                .chart
                .get_or_insert_with(crate::company::demo::chart)
                .clone();
            let contacts = demo.contacts_mut().clone();
            let tags = demo.tags_mut().clone();
            let closing = demo.open.and_then(|open| {
                demo.companies
                    .iter()
                    .find(|c| c.tenant_id == open)
                    .and_then(|c| c.closed_through)
            });
            drop(demo);
            let roles = demo_roles(&demo_equity);
            let entity = entity_of(demo_equity.entity_type);
            self.accounts_task = Some(Task::ready(chart));
            self.contacts_task = Some(Task::ready(contacts));
            self.tags_task = Some(Task::ready(tags));
            self.closing_task = Some(Task::ready(closing));
            self.entity_task = Some(Task::ready((Some(entity), roles, true)));
            return;
        }
        let (Some(session), Some(group)) = (link.session(), group) else {
            self.loaded = true;
            return;
        };
        if let Ok(p) = group.read(&session, |s| async move {
            s.accounts().list_all(&AccountFilter::default()).await
        }) {
            self.accounts_task = Some(Task::Pending(p));
        } else {
            self.loaded = true;
        }
        if let Ok(p) = group.read(&session, |s| async move {
            s.contacts().list_all(&ContactFilter::default()).await
        }) {
            self.contacts_task = Some(Task::Pending(p));
        }
        if let Ok(p) = group.read(&session, |s| async move {
            s.tags().list_all(&TagFilter::default()).await
        }) {
            self.tags_task = Some(Task::Pending(p));
        }
        if let Ok(p) = group.read(&session, |s| async move {
            Ok(s.company_settings().await?.lock_date)
        }) {
            self.closing_task = Some(Task::Pending(p));
        }
        let roles_cap = link.capabilities().has(Capabilities::EQUITY_ROLES);
        if let Ok(p) = group.read(&session, move |s| async move {
            if roles_cap {
                let r = s.equity_roles().await?;
                return Ok((r.entity_type.map(entity_of), roles_of_engine(&r), true));
            }
            let profile = s.company_profile().await?.record.profile;
            Ok((profile.entity_type.map(entity_of), BTreeMap::new(), false))
        }) {
            self.entity_task = Some(Task::Pending(p));
        }
    }

    /// Takes what arrived; true when the name, account or tag lists
    /// changed.
    pub(crate) fn poll(&mut self) -> bool {
        let mut changed = false;
        if let Some(r) = self.accounts_task.as_mut().and_then(Task::try_take) {
            self.accounts_task = None;
            self.loaded = true;
            if let Ok(a) = r {
                self.accounts = a;
                changed = true;
            }
        }
        if let Some(r) = self.contacts_task.as_mut().and_then(Task::try_take) {
            self.contacts_task = None;
            if let Ok(c) = r {
                self.contacts = c;
                changed = true;
            }
        }
        if let Some(r) = self.tags_task.as_mut().and_then(Task::try_take) {
            self.tags_task = None;
            if let Ok(tags) = r {
                self.tags = tags;
                changed = true;
            }
        }
        if let Some(r) = self.closing_task.as_mut().and_then(Task::try_take) {
            self.closing_task = None;
            if let Ok(c) = r {
                self.closing = c;
            }
        }
        if let Some(r) = self.entity_task.as_mut().and_then(Task::try_take) {
            self.entity_task = None;
            if let Ok((entity, roles, supported)) = r {
                self.entity = entity;
                self.roles = roles;
                self.roles_supported = supported;
            }
        }
        if changed {
            let mut lists = RegisterLists::default();
            lists.set_accounts(&self.accounts);
            lists.set_contacts(&self.contacts);
            lists.set_tags(&self.tags);
            self.lists = Arc::new(lists);
            self.tag_list = tag_list(&self.tags);
        }
        changed
    }

    /// Something is still being read.
    pub(crate) fn busy(&self) -> bool {
        self.accounts_task.is_some()
            || self.contacts_task.is_some()
            || self.tags_task.is_some()
            || self.closing_task.is_some()
            || self.entity_task.is_some()
    }

    /// The active accounts of the given classes, as (id, "number · name").
    pub(crate) fn accounts_of(&self, classes: &[AccountClass]) -> Vec<(AccountId, String)> {
        self.accounts
            .iter()
            .filter(|a| {
                a.is_active
                    && crate::screens::register::class_of(a).is_some_and(|c| classes.contains(&c))
            })
            .map(|a| (a.id, crate::screens::register::account_text(a)))
            .collect()
    }

    /// The class of an account.
    pub(crate) fn class(&self, id: AccountId) -> Option<AccountClass> {
        self.accounts
            .iter()
            .find(|a| a.id == id)
            .and_then(crate::screens::register::class_of)
    }

    /// A contact's display name.
    pub(crate) fn contact_name(&self, id: ContactId) -> Option<String> {
        self.contacts
            .iter()
            .find(|c| c.id == id)
            .map(|c| c.fields.display_name.clone())
    }

    /// The account with this number (demo seeds and tests).
    #[cfg(test)]
    pub(crate) fn by_number(&self, n: &str) -> Option<AccountId> {
        self.accounts
            .iter()
            .find(|a| a.fields.account_number.as_deref() == Some(n))
            .map(|a| a.id)
    }

    /// The contact with this display name (demo seeds and tests).
    #[cfg(test)]
    pub(crate) fn contact(&self, name: &str) -> Option<ContactId> {
        self.contacts
            .iter()
            .find(|c| c.fields.display_name == name)
            .map(|c| c.id)
    }
}

/// The engine's equity roles as the form model knows them.
pub(crate) fn roles_of_engine(r: &cl_api::EquityRoles) -> BTreeMap<Role, AccountId> {
    use cl_api::EquityRole as E;
    let pairs = [
        (E::OwnerDraw, Role::OwnerDraw),
        (E::OwnerContribution, Role::OwnerContribution),
        (E::PartnerDraws, Role::PartnerDraws),
        (E::PartnerContributions, Role::PartnerContributions),
        (E::ShareholderDistributions, Role::ShareholderDistributions),
        (E::PaidInCapital, Role::PaidInCapital),
    ];
    pairs
        .into_iter()
        .filter_map(|(e, role)| r.account(e).map(|a| (role, a)))
        .collect()
}

// --------------------------------------------------------------- balances

/// The engine's balances of the accounts a form shows (stored debit
/// positive; liabilities and equity are shown flipped, Q-241).
#[derive(Default)]
pub(crate) struct Balances {
    pub known: BTreeMap<AccountId, Money>,
    tasks: Vec<(AccountId, Task<Option<Money>>)>,
}

impl Balances {
    /// Reads `account`'s balance again from the engine.
    pub(crate) fn fetch(
        &mut self,
        link: &CompanyLink,
        group: Option<&RequestGroup>,
        account: AccountId,
    ) {
        self.tasks.retain(|(a, _)| *a != account);
        if let Some(mut demo) = link.demo_state() {
            let b = crate::company::demo::checks::balance(&mut demo, account);
            drop(demo);
            self.tasks.push((account, Task::ready(b)));
            self.poll();
            return;
        }
        let (Some(session), Some(group)) = (link.session(), group) else {
            return;
        };
        if let Ok(p) = group.read(&session, move |s| async move {
            Ok(s.accounts().get(account).await?.record.balance)
        }) {
            self.tasks.push((account, Task::Pending(p)));
        }
    }

    /// Takes the balances that arrived.
    pub(crate) fn poll(&mut self) {
        let mut done = Vec::new();
        for (i, (a, task)) in self.tasks.iter_mut().enumerate() {
            if let Some(r) = task.try_take() {
                done.push(i);
                match r.ok().flatten() {
                    Some(m) => {
                        self.known.insert(*a, m);
                    }
                    None => {
                        self.known.remove(a);
                    }
                }
            }
        }
        for i in done.into_iter().rev() {
            self.tasks.remove(i);
        }
    }

    /// Still reading.
    pub(crate) fn busy(&self) -> bool {
        !self.tasks.is_empty()
    }

    /// The balance as the owner reads it: assets as stored; liabilities
    /// and equity flipped so an amount owed is positive (Q-241). A display
    /// sign only; never a derived figure.
    pub(crate) fn shown(&self, account: AccountId, class: Option<AccountClass>) -> Option<Money> {
        let b = *self.known.get(&account)?;
        match class {
            Some(
                AccountClass::CreditCard
                | AccountClass::Payable
                | AccountClass::OtherLiability
                | AccountClass::Equity,
            ) => Money::ZERO.checked_sub(b).ok(),
            _ => Some(b),
        }
    }
}

// ------------------------------------------------------------------ saving

/// What the closing-date password is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Pending {
    /// The save (with what follows it).
    Save(SaveIntent),
    /// Void the last saved record with this reason.
    Void(String),
}

/// One posting to send: the checked engine posting and the demo engine's
/// copy of the same values.
#[derive(Clone, Debug)]
pub(crate) struct Outbound {
    pub posting: BankPosting,
    pub demo: DemoPosting,
}

/// The record this window saved last (Void works on it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SavedRecord {
    pub id: TransactionId,
    pub date: CalendarDate,
    /// The record's version after the save (a later edit is guarded by it).
    pub version: RecordVersion,
}

/// A saved record open in the form (Prev/Next, the register, or the save
/// just made with Save): a save changes it, guarded by its version (a
/// fingerprint of the engine's record, the stale check), and Void voids it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Editing {
    pub id: TransactionId,
    pub version: RecordVersion,
    /// The date it has in the books (the closed-period rule looks at both
    /// dates, NN8).
    pub old_date: CalendarDate,
    /// Shown, never changed here: why (i18n key).
    pub read_only: Option<&'static str>,
    /// It is void.
    pub void: bool,
}

impl Editing {
    /// The record as read from the engine; `fits` is false when the form
    /// can't show it exactly (then it is read-only).
    pub(crate) fn of(v: &Versioned<Transaction>, fits: bool) -> Editing {
        let void = v.record.status == TxnStatus::Void;
        Editing {
            id: v.record.id,
            version: v.version,
            old_date: v.record.txn_date,
            read_only: if void {
                Some("helper.read_only.void")
            } else if fits {
                None
            } else {
                Some("helper.read_only.shape")
            },
            void,
        }
    }
}

/// What `SaveFlow::send` did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Sent {
    /// Saved at once (demo); the record.
    Saved(SavedRecord),
    /// On the queue; poll for the answer.
    Queued,
    /// Waiting for the closing-date password (nothing sent).
    Asked,
    /// Refused (nothing sent); the error is set.
    Failed,
}

/// The save's and the void's answers, when they arrived.
pub(crate) type Answers = (
    Option<Result<SavedRecord, ApiError>>,
    Option<Result<(), ApiError>>,
);

/// The save and void of one helper form.
#[derive(Default)]
pub(crate) struct SaveFlow {
    pub task: Option<Task<SavedRecord>>,
    pub void_task: Option<Task<()>>,
    pub intent: Option<SaveIntent>,
    key: Option<(SaveIdentity, IdempotencyKey)>,
    /// The saved record open in the form (`None`: a new one).
    pub editing: Option<Editing>,
    /// The open record changed somewhere else since it was read (409
    /// stale): nothing was saved; Reload reads it again.
    pub stale: bool,
    pub error: Option<ApiError>,
    /// A plain note under the toolbar (i18n key).
    pub notice: Option<&'static str>,
    pub count: u32,
    pub last: Option<SavedRecord>,
    /// `save`, `void`, `clear`.
    pub last_action: Option<&'static str>,
    pub closing_prompt: ClosingPrompt<Pending>,
    pub void_open: bool,
    pub void_reason: String,
    /// Accounts the last save touched (published, Q-262).
    pub published: Vec<AccountId>,
    /// The accounts the open record touches in the books (as read, or as
    /// this window saved it): an edit and a void tell them too.
    pub record_accounts: Vec<AccountId>,
    /// The accounts the record saved last touches (Void without an open
    /// record voids that one).
    pub last_accounts: Vec<AccountId>,
}

/// The engine takes the closing-date password.
pub(crate) fn closing_supported(link: &CompanyLink) -> bool {
    link.is_demo() || link.capabilities().has(Capabilities::CLOSING_PASSWORD)
}

fn options(password: Option<Secret>) -> PostingOptions {
    PostingOptions {
        closed_period: password.map(ClosedPeriodAuthorization::with_password),
    }
}

impl SaveFlow {
    /// A save or void is running.
    pub(crate) fn busy(&self) -> bool {
        self.task.is_some() || self.void_task.is_some()
    }

    /// Nothing typed may change while this is false.
    pub(crate) fn idle(&self) -> bool {
        !self.busy() && !self.closing_prompt.dialog_open()
    }

    /// Clears the last save's messages before a new attempt.
    pub(crate) fn reset_messages(&mut self) {
        self.error = None;
        self.notice = None;
    }

    fn key_for(&mut self, id: &SaveIdentity) -> IdempotencyKey {
        match &self.key {
            Some((d, k)) if d == id => k.clone(),
            _ => {
                let k = IdempotencyKey::new();
                self.key = Some((id.clone(), k.clone()));
                k
            }
        }
    }

    /// The open record is shown, not changed here: the save is refused
    /// with its note.
    pub(crate) fn read_only(&self) -> Option<&'static str> {
        self.editing.and_then(|e| e.read_only)
    }

    /// The open record is dated on or before the closing date.
    pub(crate) fn editing_locked(&self, closing: Option<CalendarDate>) -> bool {
        self.editing.is_some_and(|e| is_locked(e.old_date, closing))
    }

    /// Sends a checked posting: a new record, or a change to the open one
    /// guarded by its version. Asks for the closing-date password first
    /// when either date is on or before the closing date (NN8), then saves
    /// (the demo at once, the engine on the queue).
    pub(crate) fn send(
        &mut self,
        link: &CompanyLink,
        group: Option<&RequestGroup>,
        out: Outbound,
        intent: SaveIntent,
        password: Option<Secret>,
        closing: Option<CalendarDate>,
    ) -> Sent {
        if self.busy() {
            return Sent::Failed;
        }
        if let Some(note) = self.read_only() {
            self.notice = Some(note);
            return Sent::Failed;
        }
        self.stale = false;
        if self.editing.is_some() {
            // An edit also tells the accounts the record moved off.
            self.published.extend(self.record_accounts.iter().copied());
        }
        let date = out.posting.date();
        let old = self.editing.map(|e| e.old_date);
        let edit = self.editing.map(|e| (e.id, e.version));
        if password.is_none() && needs_password(old, date, closing) {
            let supported = closing_supported(link);
            if supported {
                self.closing_prompt
                    .ask(Pending::Save(intent), supported, closing);
            } else {
                self.closing_prompt.unavailable(supported, closing);
            }
            return Sent::Asked;
        }
        self.intent = Some(intent);
        if let Some(mut demo) = link.demo_state() {
            let r = demo_save(&mut demo, &out.demo, edit, closing, password.as_ref());
            drop(demo);
            return match r {
                Ok(saved) => Sent::Saved(saved),
                Err(e) => {
                    self.failed(link, e, closing);
                    Sent::Failed
                }
            };
        }
        let (Some(session), Some(group)) = (link.session(), group) else {
            self.failed(link, ApiError::NoCompanyOpen, closing);
            return Sent::Failed;
        };
        let key = self.key_for(&SaveIdentity {
            posting: out.posting.clone(),
            edit,
            password: password.is_some(),
        });
        let label = OrphanLabel {
            what: "orphan.helper_save",
            key: Some(key.clone()),
        };
        let posting = out.posting;
        let written = group.write(&session, label, move |s| async move {
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
            Ok(SavedRecord {
                id: v.record.id,
                date: v.record.txn_date,
                version: v.version,
            })
        });
        if let Ok(p) = written {
            self.task = Some(Task::Pending(p));
            Sent::Queued
        } else {
            self.failed(
                link,
                ApiError::Local(cl_api::LocalValidation::InvalidValue),
                closing,
            );
            Sent::Failed
        }
    }

    /// The save took: the record, the count, the key and the prompt reset.
    /// The saved record is the open one now (a second save changes it,
    /// guarded by its new version; a new form clears it).
    pub(crate) fn saved(&mut self, r: SavedRecord) -> SaveIntent {
        self.closing_prompt.saved();
        self.key = None;
        self.count += 1;
        self.last = Some(r);
        self.last_action = Some("save");
        self.error = None;
        self.stale = false;
        self.editing = Some(Editing {
            id: r.id,
            version: r.version,
            old_date: r.date,
            read_only: None,
            void: false,
        });
        self.record_accounts.clone_from(&self.published);
        self.last_accounts.clone_from(&self.published);
        self.intent.take().unwrap_or(SaveIntent::Save)
    }

    /// A new, blank form: no record open.
    pub(crate) fn new_record(&mut self) {
        self.editing = None;
        self.record_accounts.clear();
        self.stale = false;
        self.void_open = false;
        self.void_reason.clear();
    }

    /// The saved record `v` was read into the form (`fits`: the form shows
    /// it exactly); its accounts are kept for an edit's or a void's notice.
    pub(crate) fn opened_txn(&mut self, v: &Versioned<Transaction>, fits: bool) {
        self.opened(Editing::of(v, fits));
        let mut accounts: Vec<AccountId> = v.record.lines.iter().map(|l| l.account_id).collect();
        accounts.sort();
        accounts.dedup();
        self.record_accounts = accounts;
    }

    /// What a void of [`Self::void_target`] tells (Q-262): the accounts
    /// that record touches in the books, not what the form shows now.
    pub(crate) fn void_accounts(&self) -> Vec<AccountId> {
        if self.editing.is_some() {
            self.record_accounts.clone()
        } else {
            self.last_accounts.clone()
        }
    }

    /// A saved record was read into the form.
    pub(crate) fn opened(&mut self, e: Editing) {
        self.editing = Some(e);
        self.stale = false;
        self.key = None;
        self.error = None;
        self.notice = None;
        self.void_open = false;
        self.void_reason.clear();
        self.last_action = Some("load");
    }

    /// A save or void failed: a closed-period refusal or a wrong password
    /// shows in the closing-date dialog; anything else under the form.
    pub(crate) fn failed(
        &mut self,
        link: &CompanyLink,
        e: ApiError,
        closing: Option<CalendarDate>,
    ) {
        if !crate::screens::register::keeps_key(&e) {
            self.key = None;
        }
        let pending = match self.intent.take() {
            Some(i) => Pending::Save(i),
            None if self.void_open || !self.void_reason.is_empty() => {
                Pending::Void(self.void_reason.clone())
            }
            None => Pending::Save(SaveIntent::Save),
        };
        let supported = closing_supported(link);
        if self.closing_prompt.failed(&e, pending, supported, closing) {
            return;
        }
        match e {
            ApiError::Conflict {
                reason: ConflictReason::Stale,
                ..
            } => self.stale = true,
            e => self.error = Some(e),
        }
    }

    /// The answers that arrived: the save's, then the void's.
    pub(crate) fn poll(&mut self) -> Answers {
        let saved = self.task.as_mut().and_then(Task::try_take);
        if saved.is_some() {
            self.task = None;
        }
        let voided = self.void_task.as_mut().and_then(Task::try_take);
        if voided.is_some() {
            self.void_task = None;
        }
        (saved, voided)
    }

    /// What Void voids: the open record, else the one this window saved
    /// last; (id, date in the books).
    pub(crate) fn void_target(&self) -> Option<(TransactionId, CalendarDate)> {
        self.editing
            .map(|e| (e.id, e.old_date))
            .or_else(|| self.last.map(|r| (r.id, r.date)))
    }

    /// Edit > Void Transaction: the open record (or the one saved last).
    pub(crate) fn start_void(&mut self) -> bool {
        if self.editing.is_some_and(|e| e.void) {
            self.notice = Some("helper.void.already");
        } else if self.void_target().is_some() {
            self.void_open = true;
            self.void_reason.clear();
        } else {
            self.notice = Some("helper.void.saved_only");
        }
        true
    }

    /// Voids the open (or last saved) record with `reason` (and the
    /// closing-date password when it is in the closed period). True when
    /// it voided at once (demo).
    pub(crate) fn send_void(
        &mut self,
        link: &CompanyLink,
        group: Option<&RequestGroup>,
        closing: Option<CalendarDate>,
        password: Option<Secret>,
    ) -> bool {
        let Some((id, date)) = self.void_target() else {
            return false;
        };
        let reason = self.void_reason.trim().to_string();
        if reason.is_empty() {
            self.notice = Some("helper.void.reason_needed");
            return false;
        }
        if password.is_none() && is_locked(date, closing) {
            let supported = closing_supported(link);
            if supported {
                self.closing_prompt
                    .ask(Pending::Void(reason), supported, closing);
            } else {
                self.closing_prompt.unavailable(supported, closing);
            }
            return false;
        }
        if let Some(mut demo) = link.demo_state() {
            let r =
                crate::company::demo::register::check_closing(closing, &[date], password.as_ref())
                    .and_then(|()| demo.ledger.void(id));
            drop(demo);
            return match r {
                Ok(()) => true,
                Err(e) => {
                    self.failed(link, e, closing);
                    false
                }
            };
        }
        let (Some(session), Some(group)) = (link.session(), group) else {
            return false;
        };
        // A fresh key per try is safe here: void is idempotent at the
        // engine (voiding a void transaction changes nothing), so a resend
        // after a lost reply can't void twice or post anything.
        let key = IdempotencyKey::new();
        let label = OrphanLabel {
            what: "orphan.helper_void",
            key: Some(key.clone()),
        };
        if let Ok(p) = group.write(&session, label, move |s| async move {
            s.transactions()
                .void(id, &reason, &options(password), key)
                .await?;
            Ok(())
        }) {
            self.void_task = Some(Task::Pending(p));
        }
        false
    }

    /// The void took: nothing is open any more.
    pub(crate) fn voided(&mut self) {
        self.closing_prompt.saved();
        self.void_open = false;
        self.void_reason.clear();
        self.last = None;
        self.editing = None;
        self.record_accounts.clear();
        self.last_accounts.clear();
        self.stale = false;
        self.last_action = Some("void");
    }

    /// The control channel's part (no amounts).
    pub(crate) fn state_json(&self, link: &CompanyLink) -> serde_json::Value {
        let mut v = serde_json::json!({
            "saving": self.task.is_some(),
            "saved_count": self.count,
            "last_action": self.last_action,
            "notice": self.notice,
            "error": self.error.is_some(),
            "stale": self.stale,
            "void_open": self.void_open,
            "last_saved_id": self.last.map(|r| r.id.to_string()),
            "record": if self.editing.is_some() { "edit" } else { "new" },
            "editing_id": self.editing.map(|e| e.id.to_string()),
            "read_only": self.read_only(),
        });
        if let (serde_json::Value::Object(m), serde_json::Value::Object(c)) = (
            &mut v,
            self.closing_prompt.state_json(closing_supported(link)),
        ) {
            m.extend(c);
        }
        v
    }
}

/// What a save sends, for its idempotency key: the posting, the record and
/// version it changes, and whether a closing-date password goes with it
/// (never the password itself).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SaveIdentity {
    pub posting: BankPosting,
    pub edit: Option<(TransactionId, RecordVersion)>,
    pub password: bool,
}

/// The demo engine's save: the stale check (the open record's version
/// against the record as it is now), the closed-period rule over both
/// dates, then the posting.
fn demo_save(
    demo: &mut crate::company::demo::DemoState,
    p: &DemoPosting,
    edit: Option<(TransactionId, RecordVersion)>,
    closing: Option<CalendarDate>,
    password: Option<&Secret>,
) -> Result<SavedRecord, ApiError> {
    let chart = demo
        .chart
        .get_or_insert_with(crate::company::demo::chart)
        .clone();
    let contacts = demo.contacts_mut().clone();
    let mut dates = vec![p.date];
    if let Some((id, version)) = edit {
        let now = demo
            .ledger
            .get(&chart, &contacts, id)
            .ok_or(ApiError::NotFound { code: None })?;
        if now.version != version {
            return Err(ApiError::Conflict {
                reason: ConflictReason::Stale,
                current_version: None,
                code: None,
                message: cl_api::UserText::default(),
            });
        }
        dates.push(now.record.txn_date);
    }
    crate::company::demo::register::check_closing(closing, &dates, password)?;
    let id = demo
        .ledger
        .save(&chart, p, edit.map(|(id, _)| id))
        .ok_or(ApiError::Local(cl_api::LocalValidation::InvalidValue))?;
    let version = demo
        .ledger
        .get(&chart, &contacts, id)
        .ok_or(ApiError::NotFound { code: None })?
        .version;
    Ok(SavedRecord {
        id,
        date: p.date,
        version,
    })
}

// ----------------------------------------------------------------- widgets

/// The shortcut chip text of a command ("Alt N"), from the keymap.
pub(crate) fn chip(id: &str) -> String {
    cl_model::commands::find(id)
        .and_then(|c| c.shortcut)
        .map(crate::input::shortcut_text)
        .unwrap_or_default()
        .replace('+', " ")
}

/// A label above its input; the label is the input's accessible name.
pub(crate) fn above(ui: &mut Ui, tokens: &Tokens, label: &str) -> egui::Response {
    ui.add(
        egui::Label::new(RichText::new(label).size(12.0).color(tokens.text_secondary))
            .selectable(false),
    )
}

/// An account picker with its label above. Returns the account picked
/// this frame.
pub(crate) fn account_combo(
    ui: &mut Ui,
    tokens: &Tokens,
    id_salt: &str,
    label: &str,
    accounts: &[(AccountId, String)],
    current: Option<AccountId>,
    width: f32,
) -> (Option<AccountId>, egui::Response) {
    let l = above(ui, tokens, label);
    let shown = accounts
        .iter()
        .find(|(id, _)| Some(*id) == current)
        .map(|(_, s)| s.clone())
        .unwrap_or_default();
    let mut picked = None;
    let r = egui::ComboBox::from_id_salt(id_salt)
        .width(width)
        .selected_text(shown)
        .show_ui(ui, |ui| {
            for (id, text) in accounts {
                if ui.selectable_label(Some(*id) == current, text).clicked() {
                    picked = Some(*id);
                }
            }
        })
        .response
        .labelled_by(l.id);
    (picked.filter(|p| Some(*p) != current), r)
}

/// A name field's text and list state.
#[derive(Clone, Debug, Default)]
pub(crate) struct NameInput {
    pub text: String,
    /// The names list is open (typing, a click, Ctrl+L).
    pub open: bool,
    /// The typed text is not a name from the list.
    pub problem: bool,
}

/// What happened in a name field this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NameOutcome {
    /// Nothing to apply.
    None,
    /// A name was picked (from the list or by autofill).
    Picked(ContactId),
    /// The field was emptied.
    Cleared,
}

/// A name field (Pay to, Purchased from, Lender, Who): autofill as typed,
/// a click opens the whole list (Q-250), Ctrl+L too; only names from the
/// list. Returns the outcome and the response.
pub(crate) fn name_field(
    ui: &mut Ui,
    tokens: &Tokens,
    label: &str,
    input: &mut NameInput,
    lists: &RegisterLists,
    current: Option<ContactId>,
    width: f32,
) -> (NameOutcome, egui::Response) {
    let l = above(ui, tokens, label);
    let r = ui
        .add(
            egui::TextEdit::singleline(&mut input.text)
                .desired_width(width - 8.0)
                .margin(crate::list_field::margin()),
        )
        .labelled_by(l.id);
    if r.changed() {
        input.open = !input.text.trim().is_empty();
        input.problem = false;
    }
    if let Some(open) = crate::list_field::mouse(ui, &r, input.open) {
        input.open = open;
    }
    let mut outcome = NameOutcome::None;
    if input.open && (r.has_focus() || r.lost_focus()) {
        let query = crate::list_field::query(ui.ctx(), r.id, &input.text);
        let limit = if query.is_empty() {
            200
        } else {
            NAME_SUGGESTIONS
        };
        if let LookupResult::Ready(found) = lists.suggest(NAMES, &query, limit) {
            let at = current.and_then(|p| found.iter().position(|s| lists.name(s.id) == Some(p)));
            if let Some(i) =
                crate::list_field::popup(ui, &r, found.iter().map(|s| s.label.as_str()), at)
                && let Some(id) = found.get(i).and_then(|s| lists.name(s.id))
            {
                outcome = NameOutcome::Picked(id);
            }
        }
    }
    if outcome == NameOutcome::None && r.lost_focus() {
        outcome = resolve_name(input, lists);
    }
    if let NameOutcome::Picked(id) = outcome {
        input.open = false;
        input.problem = false;
        input.text = lists.name_text(id).unwrap_or_default().to_string();
    }
    if input.problem {
        ui.label(
            RichText::new(t("helper.name.pick"))
                .size(12.0)
                .color(tokens.warn_text),
        );
    }
    (outcome, r)
}

/// Autofill when the owner leaves a name field: the exact name, else the
/// first name starting with the text; anything else asks for a name.
pub(crate) fn resolve_name(input: &mut NameInput, lists: &RegisterLists) -> NameOutcome {
    input.open = false;
    let typed = input.text.trim().to_string();
    if typed.is_empty() {
        input.problem = false;
        return NameOutcome::Cleared;
    }
    let found = lists
        .exact(NAMES, &typed)
        .or_else(|| match lists.suggest(NAMES, &typed, 1) {
            LookupResult::Ready(s) => s
                .into_iter()
                .next()
                .filter(|s| s.label.to_lowercase().starts_with(&typed.to_lowercase())),
            LookupResult::Loading => None,
        });
    if let Some(id) = found.and_then(|s| lists.name(s.id)) {
        NameOutcome::Picked(id)
    } else {
        input.problem = true;
        NameOutcome::Cleared
    }
}

/// What a left amount field holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Settled {
    /// An amount, or blank.
    Value(Option<Money>),
    /// Not an amount (the problem shows under the field).
    Refused,
}

/// An amount field's text and its problem.
#[derive(Clone, Debug, Default)]
pub(crate) struct AmountInput {
    pub text: String,
    /// Why the typed text is not accepted (i18n key).
    pub problem: Option<&'static str>,
}

impl AmountInput {
    /// The field showing `m`.
    pub(crate) fn show(&mut self, m: Option<Money>, style: MoneyStyle) {
        self.text = m.map(|m| m.format(style)).unwrap_or_default();
        self.problem = None;
    }

    /// The owner left the field: the amount (or blank), rounded to the
    /// cent (Q-232); `Refused` with the problem set when the text is not an
    /// amount. `rules` is `MONEY` by default.
    pub(crate) fn settle(&mut self, style: MoneyStyle, rules: AmountRules) -> Settled {
        let text = self.text.trim().to_string();
        if text.is_empty() {
            self.problem = None;
            return Settled::Value(None);
        }
        match amount_entry::parse(&text, rules) {
            Ok(p) => match p.value.as_money() {
                Some(m) => {
                    self.text = m.format(style);
                    self.problem = None;
                    Settled::Value(Some(m))
                }
                None => Settled::Refused,
            },
            Err(e) => {
                self.problem = Some(e.key().0);
                Settled::Refused
            }
        }
    }
}

/// A money input (mono, right-aligned) with its label above. Returns the
/// response; the caller settles it on `lost_focus`.
pub(crate) fn amount_field(
    ui: &mut Ui,
    tokens: &Tokens,
    label: &str,
    input: &mut AmountInput,
    width: f32,
    enabled: bool,
) -> egui::Response {
    let l = above(ui, tokens, label);
    let r = ui
        .add_enabled(
            enabled,
            egui::TextEdit::singleline(&mut input.text)
                .desired_width(width - 8.0)
                .font(egui::FontId::monospace(15.0))
                .align(egui::Align2::RIGHT_CENTER),
        )
        .labelled_by(l.id);
    if r.changed() {
        input.problem = None;
    }
    if let Some(key) = input.problem {
        ui.label(RichText::new(t(key)).size(12.0).color(tokens.warn_text));
    }
    r
}

/// A read-only figure with its label above (mono, right-aligned).
pub(crate) fn figure_field(ui: &mut Ui, tokens: &Tokens, label: &str, text: &str, width: f32) {
    let l = above(ui, tokens, label);
    let mut shown = text.to_string();
    ui.add_enabled(
        false,
        egui::TextEdit::singleline(&mut shown)
            .desired_width(width - 8.0)
            .font(egui::FontId::monospace(15.0))
            .align(egui::Align2::RIGHT_CENTER),
    )
    .labelled_by(l.id);
}

/// What a date field did this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DateChange {
    /// Nothing to apply.
    None,
    /// The date is now this (or blank).
    Set(Option<CalendarDate>),
}

/// A date input (the date keys apply) with its label above. Returns the
/// response and the date when the text changed to one or was left empty.
pub(crate) fn date_field(
    ui: &mut Ui,
    tokens: &Tokens,
    label: &str,
    input: &mut DateInput,
    today: CalendarDate,
    width: f32,
) -> (egui::Response, DateChange) {
    let l = above(ui, tokens, label);
    let before = input.text.clone();
    let r = ui
        .add(
            egui::TextEdit::singleline(&mut input.text)
                .desired_width(width - 8.0)
                .font(egui::TextStyle::Monospace),
        )
        .labelled_by(l.id);
    let mut out = DateChange::None;
    if r.changed() {
        input.typed(&before, today);
        if input.value.is_some() {
            out = DateChange::Set(input.value);
        }
    }
    if r.lost_focus() {
        input.settle();
        if input.value.is_none() {
            out = DateChange::Set(None);
        }
    }
    (r, out)
}

/// A one-line text input with its label above.
pub(crate) fn text_field(
    ui: &mut Ui,
    tokens: &Tokens,
    label: &str,
    text: &mut String,
    width: f32,
    limit: usize,
) -> egui::Response {
    let l = above(ui, tokens, label);
    ui.add(
        egui::TextEdit::singleline(text)
            .desired_width(width - 8.0)
            .char_limit(limit),
    )
    .labelled_by(l.id)
}

/// Clear · Save & New · Save & Close (right-aligned, 44 px). Returns the
/// command clicked.
pub(crate) fn actions_row(ui: &mut Ui, can_save: bool, idle: bool) -> Option<&'static str> {
    let mut clicked = None;
    let row = Vec2::new(ui.available_width(), ACTION_HEIGHT);
    ui.allocate_ui_with_layout(row, Layout::right_to_left(Align::Center), |ui| {
        let close = ui
            .add_enabled_ui(can_save, |ui| {
                crate::widgets::action_button_with_chip(
                    ui,
                    &t("helper.save_close"),
                    &chip("form.save_close"),
                    true,
                    ACTION_HEIGHT,
                )
            })
            .inner;
        if close.clicked() {
            clicked = Some("form.save_close");
        }
        let new = ui
            .add_enabled_ui(can_save, |ui| {
                crate::widgets::action_button_with_chip(
                    ui,
                    &t("helper.save_new"),
                    &chip("form.save_new"),
                    false,
                    ACTION_HEIGHT,
                )
            })
            .inner;
        if new.clicked() {
            clicked = Some("form.save_new");
        }
        if ui
            .add_enabled(
                idle,
                egui::Button::new(t("helper.clear")).min_size(Vec2::new(0.0, ACTION_HEIGHT)),
            )
            .clicked()
        {
            clicked = Some("form.clear");
        }
    });
    clicked
}

/// What the owner did in the messages area.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MessageClick {
    /// Nothing.
    None,
    /// The closing-date dialog's Change.
    SubmitClosing,
    /// Void with the typed reason.
    Void,
    /// Read the open record again (it changed elsewhere).
    Reload,
}

/// The closing-date dialog, the notice, the open record's notes (read-only,
/// closed period, changed elsewhere), the error and the Void card.
pub(crate) fn messages(
    ui: &mut Ui,
    tokens: &Tokens,
    flow: &mut SaveFlow,
    closing: Option<CalendarDate>,
) -> MessageClick {
    let mut click = MessageClick::None;
    match flow.closing_prompt.ui(ui, tokens) {
        ClosingClick::Submit => click = MessageClick::SubmitClosing,
        ClosingClick::Cancel => flow.closing_prompt.cancel(),
        ClosingClick::None => {}
    }
    if let Some(key) = flow.notice {
        form::hint(ui, tokens, &t(key));
    }
    if let Some(key) = flow.read_only() {
        form::hint(ui, tokens, &t(key));
    } else if flow.editing_locked(closing) {
        form::hint(ui, tokens, &t("closing.edit_note"));
    }
    if flow.stale {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(t("helper.stale")).color(tokens.warn_text));
            if ui.button(t("helper.reload")).clicked() {
                click = MessageClick::Reload;
            }
        });
    }
    if flow.void_open {
        let mut send = false;
        let mut cancel = false;
        form::card(ui, tokens, |ui| {
            form::heading(ui, tokens, &t("helper.void.title"));
            form::hint(ui, tokens, &t("helper.void.explain"));
            let r = form::field(ui, &t("helper.void.reason"), &mut flow.void_reason);
            send |= crate::forms::enter::submitted(&r);
            ui.horizontal(|ui| {
                send |= ui.button(t("helper.void.confirm")).clicked();
                cancel = ui.button(t("helper.void.cancel")).clicked();
            });
        });
        if cancel {
            flow.void_open = false;
            flow.void_reason.clear();
        } else if send {
            click = MessageClick::Void;
        }
    }
    click
}

/// The error and the saving line under the form.
pub(crate) fn status(ui: &mut Ui, tokens: &Tokens, flow: &SaveFlow) {
    if let Some(e) = &flow.error {
        form::inline_error(ui, tokens, e);
    }
    if flow.busy() {
        form::progress_line(ui, tokens, &t("helper.saving"));
    }
}

/// A problem list under the form (plain language, warn colour).
pub(crate) fn problems(ui: &mut Ui, tokens: &Tokens, keys: &[&'static str]) {
    for key in keys {
        ui.add(egui::Label::new(RichText::new(t(key)).color(tokens.warn_text)).wrap());
    }
}

/// A side-panel card with a heading.
pub(crate) fn panel_card(ui: &mut Ui, tokens: &Tokens, heading: &str, add: impl FnOnce(&mut Ui)) {
    form::card(ui, tokens, |ui| {
        ui.label(
            RichText::new(heading)
                .size(12.0)
                .strong()
                .color(tokens.text_secondary),
        );
        add(ui);
    });
}

/// "label  figure" with the figure in mono (a balance).
pub(crate) fn balance_line(ui: &mut Ui, tokens: &Tokens, label: &str, figure: Option<String>) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).size(12.0).color(tokens.text_secondary));
        ui.label(
            RichText::new(figure.unwrap_or_else(|| t("helper.balance.unknown")))
                .text_style(egui::TextStyle::Monospace)
                .color(tokens.text_primary),
        );
    });
}

/// The text of `key` with `figure` (an amount) for `{var}` in the
/// monospace font and the rest in the body font; one label.
pub(crate) fn with_figure(
    ui: &Ui,
    key: &str,
    var: &str,
    figure: &str,
    size: f32,
    color: egui::Color32,
) -> egui::text::LayoutJob {
    const MARK: &str = "\u{1}";
    let text = t_with(key, &[(var, MARK)]);
    let body = egui::TextFormat {
        font_id: egui::FontId::proportional(size),
        color,
        ..Default::default()
    };
    let mono = egui::TextFormat {
        font_id: egui::FontId::new(size, egui::TextStyle::Monospace.resolve(ui.style()).family),
        color,
        ..Default::default()
    };
    let mut job = egui::text::LayoutJob::default();
    for (i, part) in text.split(MARK).enumerate() {
        if i > 0 {
            job.append(figure, 0.0, mono.clone());
        }
        if !part.is_empty() {
            job.append(part, 0.0, body.clone());
        }
    }
    job
}

/// Which part `form_frame` is drawing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Column {
    /// The toolbar and the messages (fixed at the top).
    Top,
    /// What shows instead of the form when it can't be filled in (no
    /// account of the kind yet); no actions then.
    Empty,
    /// The form.
    Form,
    /// The side panel.
    Panel,
    /// Clear · Save & New · Save & Close (fixed at the bottom).
    Actions,
}

/// A form window (Q-265): the toolbar and messages stay at the top, the
/// actions at the bottom, and between them the form and the side panel
/// (beside it when wide, under it when narrow) scroll. `empty` draws
/// [`Column::Empty`] as the body instead, with no actions. `draw` draws one
/// part.
pub(crate) fn form_frame(
    ui: &mut Ui,
    id_salt: &str,
    empty: bool,
    mut draw: impl FnMut(&mut Ui, Column),
) -> crate::layout::FrameReport {
    use crate::layout::{Body, FixedFrame, FrameOpts};
    let wide = ui.available_width() >= SIDE_BY_SIDE;
    let form_width = |ui: &Ui| {
        let spacing = 2.0 * ui.spacing().item_spacing.x;
        ui.available_width() - PANEL_WIDTH - 20.0 - spacing
    };
    let mut frame = FixedFrame::begin(ui, id_salt, FrameOpts::default());
    frame.top(|ui| draw(ui, Column::Top));
    if empty {
        frame.body(Body::Scroll, |ui| draw(ui, Column::Empty));
        return frame.end();
    }
    frame.body(Body::Scroll, |ui| {
        if wide {
            ui.horizontal_top(|ui| {
                let form_width = form_width(ui);
                ui.vertical(|ui| {
                    ui.set_width(form_width);
                    draw(ui, Column::Form);
                });
                ui.add_space(20.0);
                ui.vertical(|ui| {
                    ui.set_width(PANEL_WIDTH);
                    draw(ui, Column::Panel);
                });
            });
        } else {
            draw(ui, Column::Form);
            ui.add_space(12.0);
            draw(ui, Column::Panel);
        }
    });
    frame.bottom(|ui| {
        // Under the form's column, as in the mockups.
        if wide {
            let w = form_width(ui);
            ui.allocate_ui(Vec2::new(w, ACTION_HEIGHT), |ui| draw(ui, Column::Actions));
        } else {
            draw(ui, Column::Actions);
        }
    });
    frame.end()
}

/// The tooltip key that explains the entity type's words for owner money
/// (draw, contribution, distribution, paid-in capital).
pub(crate) fn jargon_of(entity: Option<Entity>) -> &'static str {
    match entity {
        Some(Entity::Partnership) => "jargon.partner_money",
        Some(Entity::SCorp | Entity::CCorp) => "jargon.shareholder_money",
        Some(Entity::SoleProp | Entity::SingleMemberLlc) | None => "jargon.owner_money",
    }
}

/// ◀ Prev · Next ▶ over the saved records (their shortcuts in the
/// tooltips). Returns the command clicked.
pub(crate) fn nav_buttons(ui: &mut Ui, enabled: bool) -> Option<&'static str> {
    let tip = |cmd: &str| chip(cmd).replace(' ', "+");
    header::nav_buttons(
        ui,
        enabled,
        (&t("helper.prev"), &tip("form.prev_record")),
        (&t("helper.next"), &tip("form.next_record")),
    )
    .map(header::Nav::command)
}

/// The toolbar (Write Check layout): Prev and Next, then what `add`
/// draws (starting with its own separator), then "Unsaved changes" at the
/// right; it wraps when narrow. Returns the record command clicked.
pub(crate) fn toolbar(
    ui: &mut Ui,
    tokens: &Tokens,
    idle: bool,
    dirty: bool,
    add: impl FnOnce(&mut Ui),
) -> Option<&'static str> {
    header::toolbar(ui, tokens, dirty, &t("helper.unsaved"), |ui| {
        let clicked = nav_buttons(ui, idle);
        add(ui);
        clicked
    })
}

/// The title row (Write Check layout): `content` on the left and the
/// window title on the right, or the title above when they don't fit.
pub(crate) fn title_row<R>(
    ui: &mut Ui,
    tokens: &Tokens,
    salt: &str,
    title: &str,
    content: impl FnOnce(&mut Ui) -> R,
) -> R {
    header::title_row(ui, tokens, salt, title, content)
}

/// The title alone at the right (Write Check layout).
pub(crate) fn title_only(ui: &mut Ui, tokens: &Tokens, title: &str) {
    header::title_only(ui, tokens, title);
}

/// One group of a wrapping row, moved whole to the next line when it
/// doesn't fit (see [`header::wrap_item`]).
pub(crate) fn wrap_item<R>(
    ui: &mut Ui,
    salt: impl std::hash::Hash + std::fmt::Debug,
    add: impl FnOnce(&mut Ui) -> R,
) -> R {
    header::wrap_item(ui, salt, add)
}

/// Keeps `focused` on `field` while its input has the keyboard (Esc
/// reverts the field that had it).
pub(crate) fn track(focused: &mut Option<u64>, r: &egui::Response, field: u64) {
    if r.has_focus() {
        *focused = Some(field);
    } else if *focused == Some(field) {
        *focused = None;
    }
}

/// What the owner did in the header tags row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TagEdit {
    /// Took a tag off.
    Remove(TagId),
    /// Put a tag on (the same group's other tag comes off).
    Add(TagId),
}

/// Header tags (Q-178): chips with a remove click, and an Add list of the
/// active tags. The tags default onto lines without their own tag.
pub(crate) fn tags_row(
    ui: &mut Ui,
    tokens: &Tokens,
    id_salt: &str,
    tags: &[Tag],
    header: &[TagId],
) -> Option<TagEdit> {
    let mut edit = None;
    ui.horizontal_wrapped(|ui| {
        let l = above(ui, tokens, &t("helper.tags"));
        for id in header {
            let name = tags
                .iter()
                .find(|g| g.id == *id)
                .map(|g| g.fields.name.clone())
                .unwrap_or_default();
            let r = ui
                .add(
                    egui::Button::new(
                        RichText::new(t_with("check.tags.remove", &[("tag", &name)])).size(12.0),
                    )
                    .fill(tokens.neutral_chip),
                )
                .on_hover_text(t("check.tags.remove.tip"));
            if r.clicked() {
                edit = Some(TagEdit::Remove(*id));
            }
        }
        egui::ComboBox::from_id_salt(id_salt)
            .selected_text(t("check.tags.add"))
            .show_ui(ui, |ui| {
                for g in tags
                    .iter()
                    .filter(|g| g.is_active && !header.contains(&g.id))
                {
                    if ui.selectable_label(false, &g.fields.name).clicked() {
                        edit = Some(TagEdit::Add(g.id));
                    }
                }
            })
            .response
            .labelled_by(l.id);
    });
    edit
}

/// `header` after `edit`: one tag per group.
pub(crate) fn apply_tag_edit(tags: &[Tag], header: &mut Vec<TagId>, edit: TagEdit) {
    match edit {
        TagEdit::Remove(id) => header.retain(|g| *g != id),
        TagEdit::Add(id) => {
            let group = tags
                .iter()
                .find(|g| g.id == id)
                .and_then(|g| g.fields.group_id);
            let same_group: Vec<TagId> = tags
                .iter()
                .filter(|g| group.is_some() && g.fields.group_id == group)
                .map(|g| g.id)
                .collect();
            header.retain(|g| !same_group.contains(g));
            if !header.contains(&id) {
                header.push(id);
            }
        }
    }
}
