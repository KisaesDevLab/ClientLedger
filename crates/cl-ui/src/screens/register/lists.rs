//! The lists behind the register's entry row and Splits pickers: accounts,
//! names (customers, vendors and others) and tags, ranked by
//! `cl_model::lookup`. A picker id is the entry's position in its list plus
//! one; it is only meaningful inside this window and is turned back into
//! the engine id before anything is sent.

use std::borrow::Cow;

use cl_api::{Account, Contact, Tag};
use cl_core::ids::{AccountId, ContactId, TagId};
use cl_grid::{ListId, LookupResult, Lookups, Suggestion};
use cl_model::lists::accounts::Chart;
use cl_model::lookup::{LookupEntry, LookupIndex};
use cl_model::register::AccountClass;

use super::account_text;
use crate::screens::lists::accounts::model::{chart_entry, kind_of};

/// Accounts.
pub const ACCOUNTS: ListId = ListId(1);
/// Customers, vendors and other names.
pub const NAMES: ListId = ListId(2);
/// Tags.
pub const TAGS: ListId = ListId(3);

/// One list: engine ids, display text and the ranking index.
struct List<I> {
    ids: Vec<I>,
    labels: Vec<String>,
    index: LookupIndex,
}

impl<I: Copy + PartialEq> List<I> {
    fn new(
        rows: Vec<(I, Option<String>, String, bool)>,
        label: impl Fn(&str, Option<&str>) -> String,
    ) -> List<I> {
        let mut ids = Vec::new();
        let mut labels = Vec::new();
        let mut entries = Vec::new();
        for (i, (id, number, name, active)) in rows.into_iter().enumerate() {
            ids.push(id);
            labels.push(label(&name, number.as_deref()));
            entries.push(LookupEntry {
                id: i as u64 + 1,
                number,
                full_name: name,
                active,
            });
        }
        List {
            ids,
            labels,
            index: LookupIndex::new(entries),
        }
    }

    fn id(&self, key: u64) -> Option<I> {
        let i = usize::try_from(key.checked_sub(1)?).ok()?;
        self.ids.get(i).copied()
    }

    fn key(&self, id: I) -> Option<u64> {
        self.ids.iter().position(|x| *x == id).map(|i| i as u64 + 1)
    }

    fn label(&self, key: u64) -> Option<&str> {
        let i = usize::try_from(key.checked_sub(1)?).ok()?;
        self.labels.get(i).map(String::as_str)
    }
}

impl<I> Default for List<I> {
    fn default() -> Self {
        List {
            ids: Vec::new(),
            labels: Vec::new(),
            index: LookupIndex::new(Vec::new()),
        }
    }
}

/// The register's pickers.
#[derive(Default)]
pub struct RegisterLists {
    accounts: List<AccountId>,
    classes: Vec<AccountClass>,
    names: List<ContactId>,
    tags: List<TagId>,
}

impl RegisterLists {
    /// Sets the chart of accounts. Each account shows its full path with
    /// its number (DESIGN.md: `5100 · Materials:Plants`) and is found by
    /// any segment of it (`Plants`, `Mulch`).
    pub fn set_accounts(&mut self, accounts: &[Account]) {
        self.classes = accounts
            .iter()
            .map(|a| {
                AccountClass::of(
                    kind_of(a.fields.account_type),
                    a.fields.detail_type.as_deref(),
                )
            })
            .collect();
        let chart = Chart::new(
            accounts.iter().map(|a| chart_entry(a, false)).collect(),
            true,
        );
        let full = |a: &Account| {
            chart
                .full_name(a.id)
                .map_or_else(|| a.fields.name.clone(), str::to_string)
        };
        let labels: Vec<String> = accounts
            .iter()
            .map(|a| chart.display_name(a.id).unwrap_or_else(|| account_text(a)))
            .collect();
        let mut list = List::new(
            accounts
                .iter()
                .map(|a| {
                    (
                        a.id,
                        a.fields.account_number.clone().filter(|n| !n.is_empty()),
                        full(a),
                        a.is_active,
                    )
                })
                .collect(),
            |name, _| name.to_string(),
        );
        list.labels = labels;
        self.accounts = list;
    }

    /// Sets the names.
    pub fn set_contacts(&mut self, contacts: &[Contact]) {
        self.names = List::new(
            contacts
                .iter()
                .map(|c| (c.id, None, c.fields.display_name.clone(), c.is_active))
                .collect(),
            |name, _| name.to_string(),
        );
    }

    /// Sets the tags.
    pub fn set_tags(&mut self, tags: &[Tag]) {
        self.tags = List::new(
            tags.iter()
                .map(|t| (t.id, None, t.fields.name.clone(), t.is_active))
                .collect(),
            |name, _| name.to_string(),
        );
    }

    /// The account behind a picker id.
    #[must_use]
    pub fn account(&self, key: u64) -> Option<AccountId> {
        self.accounts.id(key)
    }

    /// The picker id of an account.
    #[must_use]
    pub fn account_key(&self, id: AccountId) -> Option<u64> {
        self.accounts.key(id)
    }

    /// An account's display text (`5100 · Materials:Plants`).
    #[must_use]
    pub fn account_text(&self, id: AccountId) -> Option<&str> {
        self.accounts.label(self.accounts.key(id)?)
    }

    /// The register class of an account in the chart (`None` when it is
    /// not there any more).
    #[must_use]
    pub fn class(&self, id: AccountId) -> Option<AccountClass> {
        let i = self.accounts.ids.iter().position(|x| *x == id)?;
        self.classes.get(i).copied()
    }

    /// The name behind a picker id.
    #[must_use]
    pub fn name(&self, key: u64) -> Option<ContactId> {
        self.names.id(key)
    }

    /// The picker id of a name.
    #[must_use]
    pub fn name_key(&self, id: ContactId) -> Option<u64> {
        self.names.key(id)
    }

    /// A name's display text.
    #[must_use]
    pub fn name_text(&self, id: ContactId) -> Option<&str> {
        self.names.label(self.names.key(id)?)
    }

    /// The tag behind a picker id.
    #[must_use]
    pub fn tag(&self, key: u64) -> Option<TagId> {
        self.tags.id(key)
    }

    /// The picker id of a tag.
    #[must_use]
    pub fn tag_key(&self, id: TagId) -> Option<u64> {
        self.tags.key(id)
    }

    fn list(&self, list: ListId) -> Option<(&LookupIndex, &[String])> {
        match list {
            ACCOUNTS => Some((&self.accounts.index, &self.accounts.labels)),
            NAMES => Some((&self.names.index, &self.names.labels)),
            TAGS => Some((&self.tags.index, &self.tags.labels)),
            _ => None,
        }
    }
}

fn suggestion(labels: &[String], id: u64) -> Option<Suggestion> {
    let i = usize::try_from(id.checked_sub(1)?).ok()?;
    labels.get(i).map(|label| Suggestion {
        id,
        label: label.clone(),
        group: None,
    })
}

impl Lookups for RegisterLists {
    fn suggest(&self, list: ListId, query: &str, limit: usize) -> LookupResult {
        let Some((index, labels)) = self.list(list) else {
            return LookupResult::Ready(Vec::new());
        };
        LookupResult::Ready(
            index
                .rank(query, limit)
                .into_iter()
                .filter_map(|r| suggestion(labels, r.id))
                .collect(),
        )
    }

    fn exact(&self, list: ListId, query: &str) -> Option<Suggestion> {
        let (index, labels) = self.list(list)?;
        let e = index.exact(query)?;
        suggestion(labels, e.id)
    }

    fn display(&self, list: ListId, id: u64) -> Option<Cow<'_, str>> {
        let (_, labels) = self.list(list)?;
        let i = usize::try_from(id.checked_sub(1)?).ok()?;
        labels.get(i).map(|s| Cow::Borrowed(s.as_str()))
    }

    fn all(&self, list: ListId) -> LookupResult {
        self.suggest(list, "", 200)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picker_ids_round_trip_and_rank() {
        let chart = crate::company::demo::chart();
        let mut l = RegisterLists::default();
        l.set_accounts(&chart);
        let fuel = chart
            .iter()
            .find(|a| a.fields.account_number.as_deref() == Some("6100"))
            .expect("6100");
        let key = l.account_key(fuel.id).expect("key");
        assert_eq!(l.account(key), Some(fuel.id));
        assert_eq!(l.account(0), None);
        assert_eq!(l.class(fuel.id), Some(AccountClass::IncomeOrExpense));
        let LookupResult::Ready(s) = l.suggest(ACCOUNTS, "6100", 5) else {
            panic!("ready");
        };
        assert_eq!(s[0].id, key);
        assert_eq!(s[0].label, "6100 \u{b7} Fuel");
        assert_eq!(l.exact(ACCOUNTS, "6100").map(|s| s.id), Some(key));
        assert_eq!(
            l.display(ACCOUNTS, key).as_deref(),
            Some("6100 \u{b7} Fuel")
        );
        let LookupResult::Ready(none) = l.suggest(TAGS, "x", 5) else {
            panic!("ready");
        };
        assert_eq!(none.len(), 0);
    }

    #[test]
    fn accounts_show_their_full_path_and_list_by_number() {
        let chart = crate::company::demo::chart();
        let mut l = RegisterLists::default();
        l.set_accounts(&chart);
        let plants = chart
            .iter()
            .find(|a| a.fields.account_number.as_deref() == Some("5100"))
            .expect("5100");
        let key = l.account_key(plants.id).expect("key");
        assert_eq!(
            l.display(ACCOUNTS, key).as_deref(),
            Some("5100 \u{b7} Materials:Plants")
        );
        assert_eq!(
            l.account_text(plants.id),
            Some("5100 \u{b7} Materials:Plants")
        );
        // A later segment of the path still finds it.
        let LookupResult::Ready(s) = l.suggest(ACCOUNTS, "Plants", 5) else {
            panic!("ready");
        };
        assert_eq!(s.first().map(|s| s.id), Some(key));
        // Ctrl+L: the whole list by account number.
        let LookupResult::Ready(all) = l.all(ACCOUNTS) else {
            panic!("ready");
        };
        let numbers: Vec<u32> = all
            .iter()
            .filter_map(|s| s.label.split(' ').next()?.parse().ok())
            .collect();
        assert!(numbers.len() > 10);
        assert!(numbers.windows(2).all(|w| w[0] < w[1]), "{numbers:?}");
    }
}
