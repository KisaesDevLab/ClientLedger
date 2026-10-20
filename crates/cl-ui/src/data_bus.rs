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

use cl_core::ids::AccountId;

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
    /// The chart of accounts (names, numbers, active, opening balances).
    Accounts,
    /// Tags and tag groups.
    Tags,
    /// Products and services.
    Items,
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
    /// The chart of accounts.
    pub accounts: bool,
    /// Tags.
    pub tags: bool,
    /// Items.
    pub items: bool,
}

impl Changes {
    /// All of it (a subscriber that fell behind the log).
    #[must_use]
    pub fn everything() -> Changes {
        Changes {
            ledger: LedgerScope::All,
            contacts: true,
            accounts: true,
            tags: true,
            items: true,
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
            Change::Accounts => self.accounts = true,
            Change::Tags => self.tags = true,
            Change::Items => self.items = true,
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
