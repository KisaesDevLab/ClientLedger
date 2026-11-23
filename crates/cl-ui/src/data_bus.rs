//! "Data changed" notices between windows (Q-262).
//!
//! Every successful save, void or delete publishes what it changed on the
//! company's [`DataBus`]: transactions (for some accounts, or any), the
//! contact list, the chart of accounts, tags or items. Windows that show
//! that data hold a [`Subscription`] and call [`DataBus::take`] each time
//! they are drawn; a non-empty [`Changes`] tells them to read the affected
//! parts from the engine again (never to work a figure out themselves).
//!
//! A generation counter orders the notices, so a window that was not drawn
//! for a while (a background tab) catches up the next time it is shown. The
//! log keeps the newest [`KEEP`] notices; a subscriber that fell further
//! behind is told that everything changed. A notice is never sent back to
//! the subscription that published it (the publishing window already
//! reloaded what it needs).
//!
//! Notices carry ids only: no amounts, names or memo text.

use std::collections::{BTreeSet, VecDeque};
use std::sync::{Mutex, PoisonError};

use cl_core::ids::{AccountId, ContactId};

/// How many notices the bus remembers.
pub const KEEP: usize = 256;

/// One thing that changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// Transactions were posted, changed, voided or deleted. `None` when
    /// the accounts touched are not known (any register may be affected).
    Ledger(Option<Vec<AccountId>>),
    /// Customers, vendors or other names.
    Contacts,
    /// A write on something a contact owns (a sales tax exemption
    /// certificate upload) moved the contact's stamp (U-291 S2): an open
    /// contact panel takes it, so its next save is no conflict with itself.
    ContactStamp(ContactId, cl_api::RecordStamp),
    /// The chart of accounts (names, numbers, active, opening balances).
    Accounts,
    /// Tags and tag groups.
    Tags,
    /// Products and services.
    Items,
    /// Templates (saved, replaced, edited or deleted): an open Templates
    /// list reads them again.
    Templates,
    /// Bills or vendor credits of this vendor were saved, voided or deleted
    /// (P17): its vendor panel and open-bill lists read them again.
    Bills(ContactId),
    /// Bills of some vendor may have changed on another computer (the
    /// change feed names transactions, not vendors; PR-8 S1): every vendor
    /// panel checks its duplicate ref again.
    BillsAll,
    /// The company's settings or closing date changed on another computer
    /// (the change feed's companies and 	enants; PR-8 S2): an open
    /// Preferences window with nothing unsaved reads them again.
    Company,
    /// Printed checks were recorded, answered or put off (P19): Home's
    /// "Did all checks print correctly?" reminder reads its file again.
    Printing,
    /// Bank file rows were added to or taken out of the Bank Feed (P25):
    /// Home's waiting count and the Bank Feed read again.
    BankFeed,
    /// Payroll imports (P30): Home's unposted count.
    Payroll,
    /// Send to My Accountant discarded every unposted payroll import (P32
    /// QA-14): an open Import Payroll on one of them starts over.
    PayrollDiscarded,
    /// Memorized reports were saved, changed or deleted (A08 QA-16): an
    /// open report window's Memorized list reads them again.
    Reports,
    /// The company's AI settings were saved in Preferences (A10 QA-4): an
    /// open Describe the report panel reads its switches again.
    Ai,
    /// A backup passphrase was set on the Back Up screen (Q-P30h): an
    /// open Import Payroll drops its "no passphrase" message.
    BackupPassphrase,
}

impl Change {
    /// Transactions in exactly these accounts (any, when the list is empty).
    #[must_use]
    pub fn ledger<I: IntoIterator<Item = AccountId>>(accounts: I) -> Change {
        let v: Vec<AccountId> = accounts.into_iter().collect();
        Change::Ledger((!v.is_empty()).then_some(v))
    }
}

/// Which registers a set of notices touches.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum LedgerScope {
    /// No transaction changed.
    #[default]
    Untouched,
    /// Transactions in these accounts changed.
    Accounts(BTreeSet<AccountId>),
    /// Transactions changed somewhere.
    All,
}

/// Everything a subscriber missed since it last looked.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Changes {
    /// Transactions.
    pub ledger: LedgerScope,
    /// Contacts.
    pub contacts: bool,
    /// New contact stamps from writes on things they own (the newest per
    /// contact; never guessed for a subscriber that fell behind).
    pub contact_stamps: std::collections::BTreeMap<ContactId, cl_api::RecordStamp>,
    /// The chart of accounts.
    pub accounts: bool,
    /// Tags.
    pub tags: bool,
    /// Items.
    pub items: bool,
    /// Templates.
    pub templates: bool,
    /// The vendors whose bills or credits changed (P17).
    pub bill_vendors: BTreeSet<ContactId>,
    /// Bills of any vendor may have changed (a subscriber that fell behind).
    pub bills_all: bool,
    /// Printing (P19).
    pub printing: bool,
    /// The Bank Feed (P25).
    pub bank_feed: bool,
    /// Memorized reports (A08 QA-16).
    pub reports: bool,
    /// The company's AI settings (A10 QA-4).
    pub ai: bool,
    /// A backup passphrase was set (Q-P30h).
    pub backup_passphrase: bool,
    /// Every unposted payroll import was discarded (P32 QA-14).
    pub payroll_discarded: bool,
    /// The company's settings (PR-8 S2).
    pub company: bool,
}

impl Changes {
    /// All of it (a subscriber that fell behind the log).
    #[must_use]
    pub fn everything() -> Changes {
        Changes {
            ledger: LedgerScope::All,
            contacts: true,
            contact_stamps: std::collections::BTreeMap::new(),
            accounts: true,
            tags: true,
            items: true,
            templates: true,
            bill_vendors: BTreeSet::new(),
            bills_all: true,
            printing: true,
            bank_feed: true,
            reports: true,
            ai: true,
            backup_passphrase: true,
            // Never guessed: a subscriber that fell behind must not throw
            // away a live import on a maybe.
            payroll_discarded: false,
            company: true,
        }
    }

    /// Nothing changed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        *self == Changes::default()
    }

    /// Some transaction changed.
    #[must_use]
    pub fn ledger_any(&self) -> bool {
        self.ledger != LedgerScope::Untouched
    }

    /// A transaction in `account` may have changed.
    #[must_use]
    pub fn ledger_touches(&self, account: AccountId) -> bool {
        match &self.ledger {
            LedgerScope::Untouched => false,
            LedgerScope::Accounts(set) => set.contains(&account),
            LedgerScope::All => true,
        }
    }

    /// Bills or credits of `vendor` may have changed (P17).
    #[must_use]
    pub fn bills_touch(&self, vendor: ContactId) -> bool {
        self.bills_all || self.bill_vendors.contains(&vendor)
    }

    /// Contacts, accounts or tags changed (the pickers' lists).
    #[must_use]
    pub fn lists(&self) -> bool {
        self.contacts || self.accounts || self.tags
    }

    fn add(&mut self, c: &Change) {
        match c {
            Change::Ledger(None) => self.ledger = LedgerScope::All,
            Change::Ledger(Some(ids)) => match &mut self.ledger {
                LedgerScope::All => {}
                LedgerScope::Accounts(set) => set.extend(ids.iter().copied()),
                LedgerScope::Untouched => {
                    self.ledger = LedgerScope::Accounts(ids.iter().copied().collect());
                }
            },
            Change::Contacts => self.contacts = true,
            Change::ContactStamp(id, stamp) => {
                let held = self.contact_stamps.entry(*id).or_insert(*stamp);
                *held = (*held).max(*stamp);
            }
            Change::Accounts => self.accounts = true,
            Change::Tags => self.tags = true,
            Change::Items => self.items = true,
            Change::Templates => self.templates = true,
            Change::Printing => self.printing = true,
            // P30: Home's payroll count reads with the feed's.
            Change::BankFeed | Change::Payroll => self.bank_feed = true,
            Change::Reports => self.reports = true,
            Change::Ai => self.ai = true,
            Change::BackupPassphrase => self.backup_passphrase = true,
            Change::PayrollDiscarded => {
                self.payroll_discarded = true;
                self.bank_feed = true;
            }
            Change::Bills(vendor) => {
                self.bill_vendors.insert(*vendor);
            }
            Change::BillsAll => self.bills_all = true,
            Change::Company => self.company = true,
        }
    }
}

/// One window's place in the notices.
#[derive(Debug)]
pub struct Subscription {
    id: u64,
    seen: u64,
}

impl Subscription {
    /// A subscription to no bus (a window without a company): it never
    /// hears anything.
    #[must_use]
    pub fn detached() -> Subscription {
        Subscription { id: 0, seen: 0 }
    }
}

#[derive(Debug)]
struct Notice {
    generation: u64,
    origin: u64,
    change: Change,
}

#[derive(Debug, Default)]
struct Inner {
    generation: u64,
    next_subscriber: u64,
    log: VecDeque<Notice>,
}

/// The company's change notices.
#[derive(Debug, Default)]
pub struct DataBus {
    inner: Mutex<Inner>,
}

impl DataBus {
    /// No notices yet.
    #[must_use]
    pub fn new() -> DataBus {
        DataBus::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The newest notice's number (0 before the first).
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.lock().generation
    }

    /// Start listening: notices published from now on.
    #[must_use]
    pub fn subscribe(&self) -> Subscription {
        let mut inner = self.lock();
        inner.next_subscriber += 1;
        Subscription {
            id: inner.next_subscriber,
            seen: inner.generation,
        }
    }

    /// Publish a change made outside any window (every subscriber hears it).
    pub fn publish(&self, change: Change) -> u64 {
        self.push(0, change)
    }

    /// Publish a change `from` a window: everyone else hears it.
    pub fn publish_from(&self, from: &Subscription, change: Change) -> u64 {
        self.push(from.id, change)
    }

    fn push(&self, origin: u64, change: Change) -> u64 {
        let mut inner = self.lock();
        inner.generation += 1;
        let generation = inner.generation;
        tracing::debug!(generation, ?change, "data changed");
        inner.log.push_back(Notice {
            generation,
            origin,
            change,
        });
        while inner.log.len() > KEEP {
            inner.log.pop_front();
        }
        generation
    }

    /// What changed since `sub` last looked (its own notices left out),
    /// and moves it up to now.
    #[must_use]
    pub fn take(&self, sub: &mut Subscription) -> Changes {
        let inner = self.lock();
        if sub.id == 0 || sub.seen >= inner.generation {
            sub.seen = sub.seen.max(inner.generation);
            return Changes::default();
        }
        let oldest = inner.log.front().map_or(u64::MAX, |n| n.generation);
        let mut out = Changes::default();
        if sub.seen + 1 < oldest {
            // Fell behind the log: anything may have changed.
            out = Changes::everything();
        } else {
            for n in inner.log.iter().filter(|n| n.generation > sub.seen) {
                if n.origin != 0 && n.origin == sub.id {
                    continue;
                }
                out.add(&n.change);
            }
        }
        sub.seen = inner.generation;
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(n: u8) -> AccountId {
        AccountId::parse(&format!("00000000-0000-4000-8000-0000000000{n:02x}")).expect("id")
    }

    #[test]
    fn a_contact_stamp_keeps_the_newest_and_never_lists_again() {
        let bus = DataBus::new();
        let mut sub = bus.subscribe();
        let id = ContactId::parse("2a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01").expect("id");
        let older = cl_api::RecordStamp::parse("2026-11-20T10:00:00.000Z").expect("stamp");
        let newer = cl_api::RecordStamp::parse("2026-11-20T10:00:01.000Z").expect("stamp");
        bus.publish(Change::ContactStamp(id, newer));
        bus.publish(Change::ContactStamp(id, older));
        let got = bus.take(&mut sub);
        assert_eq!(got.contact_stamps.get(&id), Some(&newer));
        assert!(!got.contacts, "a stamp alone reads no list again");
    }

    #[test]
    fn a_new_subscriber_hears_only_later_notices() {
        let bus = DataBus::new();
        bus.publish(Change::Contacts);
        let mut sub = bus.subscribe();
        assert!(bus.take(&mut sub).is_empty());
        bus.publish(Change::Tags);
        let c = bus.take(&mut sub);
        assert!(c.tags && !c.contacts && !c.ledger_any());
        assert!(bus.take(&mut sub).is_empty(), "heard once");
    }

    #[test]
    fn ledger_notices_merge_by_account_and_any_wins() {
        let bus = DataBus::new();
        let mut sub = bus.subscribe();
        bus.publish(Change::ledger([account(1)]));
        bus.publish(Change::ledger([account(2)]));
        let c = bus.take(&mut sub);
        assert!(c.ledger_touches(account(1)) && c.ledger_touches(account(2)));
        assert!(!c.ledger_touches(account(3)));
        assert!(!c.lists());
        bus.publish(Change::ledger([account(1)]));
        bus.publish(Change::ledger(Vec::new()));
        let c = bus.take(&mut sub);
        assert_eq!(c.ledger, LedgerScope::All);
        assert!(c.ledger_touches(account(3)));
    }

    #[test]
    fn bill_notices_name_their_vendors() {
        let vendor = |n: u8| {
            ContactId::parse(&format!("00000000-0000-4000-8000-0000000001{n:02x}")).expect("id")
        };
        let bus = DataBus::new();
        let mut sub = bus.subscribe();
        bus.publish(Change::Bills(vendor(1)));
        let c = bus.take(&mut sub);
        assert!(c.bills_touch(vendor(1)) && !c.bills_touch(vendor(2)));
        assert!(!c.ledger_any() && !c.is_empty());
        assert!(Changes::everything().bills_touch(vendor(2)));
    }

    #[test]
    fn the_publisher_does_not_hear_itself() {
        let bus = DataBus::new();
        let mut a = bus.subscribe();
        let mut b = bus.subscribe();
        bus.publish_from(&a, Change::Accounts);
        assert!(bus.take(&mut a).is_empty());
        assert!(bus.take(&mut b).accounts);
        // A later notice from someone else still reaches `a`.
        bus.publish_from(&b, Change::Items);
        assert!(bus.take(&mut a).items);
        assert!(bus.take(&mut b).is_empty());
    }

    #[test]
    fn a_subscriber_behind_the_log_hears_everything() {
        let bus = DataBus::new();
        let mut sub = bus.subscribe();
        for _ in 0..=KEEP {
            bus.publish(Change::Tags);
        }
        assert_eq!(bus.take(&mut sub), Changes::everything());
        assert_eq!(bus.generation(), u64::try_from(KEEP).expect("small") + 1);
        // Exactly at the edge of the log: only what was published.
        let mut edge = bus.subscribe();
        for _ in 0..KEEP {
            bus.publish(Change::Contacts);
        }
        let c = bus.take(&mut edge);
        assert!(c.contacts && !c.tags);
    }

    #[test]
    fn a_detached_subscription_never_hears() {
        let bus = DataBus::new();
        let mut d = Subscription::detached();
        bus.publish(Change::Contacts);
        assert!(bus.take(&mut d).is_empty());
    }
}
