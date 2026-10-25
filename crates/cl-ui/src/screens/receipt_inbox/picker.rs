//! "Add to a billâ€¦", "Add to a checkâ€¦", "Add to a bank transactionâ€¦": the
//! record an inbox file is attached to. The list is the engine's
//! transaction list (posted only, newest first, at most 50, searched by
//! the engine); a bank transaction is picked from one bank or card
//! account's records, attached under its own transaction type.

use std::sync::Arc;

use cl_api::{
    Account, AccountFilter, AttachTo, PageRequest, SortDir, TransactionFilter, TxnSort, TxnStatus,
    TxnType,
};
use cl_core::ids::{AccountId, TransactionId};
use cl_core::{CalendarDate, Money};

use crate::company::{CompanyLink, Task};
use cl_model::register::AccountClass;

/// Rows listed at most.
pub(crate) const MAX_ROWS: usize = 50;
/// The same, as the page size asked for.
const PAGE_LIMIT: u16 = 50;

/// What the file is added to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PickKind {
    /// A saved bill.
    Bill,
    /// A saved check.
    Check,
    /// Any record in a bank or card account.
    BankTxn,
}

impl PickKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            PickKind::Bill => "bill",
            PickKind::Check => "check",
            PickKind::BankTxn => "bank",
        }
    }

    /// The picker's title (i18n key).
    pub(crate) fn title_key(self) -> &'static str {
        match self {
            PickKind::Bill => "inbox.pick.bill.title",
            PickKind::Check => "inbox.pick.check.title",
            PickKind::BankTxn => "inbox.pick.bank.title",
        }
    }
}

/// One record that can take the file.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct PickRow {
    pub id: TransactionId,
    pub txn_type: TxnType,
    pub date: CalendarDate,
    pub number: Option<String>,
    pub payee: Option<String>,
    pub amount: Option<Money>,
}

impl std::fmt::Debug for PickRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PickRow")
            .field("id", &self.id)
            .field("txn_type", &self.txn_type)
            .finish_non_exhaustive()
    }
}

impl PickRow {
    /// Where the file goes for this record.
    pub(crate) fn target(&self) -> AttachTo {
        match self.txn_type {
            TxnType::Bill => AttachTo::Bill(self.id),
            TxnType::VendorCredit => AttachTo::VendorCredit(self.id),
            TxnType::Expense => AttachTo::Expense(self.id),
            t => AttachTo::Txn(t, self.id),
        }
    }
}

/// The open picker.
pub(crate) struct Picker {
    pub kind: PickKind,
    pub search: String,
    /// The bank or card account (bank transactions only).
    pub account: Option<AccountId>,
    pub accounts: Vec<(AccountId, String)>,
    accounts_task: Option<Task<Vec<Account>>>,
    pub rows: Vec<PickRow>,
    pub loaded: bool,
    pub failed: bool,
    task: Option<Task<Vec<PickRow>>>,
    /// The search the rows are for.
    searched: Option<(String, Option<AccountId>)>,
}

impl Picker {
    pub(crate) fn new(kind: PickKind) -> Picker {
        Picker {
            kind,
            search: String::new(),
            account: None,
            accounts: Vec::new(),
            accounts_task: None,
            rows: Vec::new(),
            loaded: false,
            failed: false,
            task: None,
            searched: None,
        }
    }

    pub(crate) fn busy(&self) -> bool {
        self.task.is_some() || self.accounts_task.is_some()
    }

    /// Reads the list again when the search or account changed (and the
    /// bank accounts once).
    pub(crate) fn refresh(
        &mut self,
        link: &Arc<CompanyLink>,
        group: Option<&cl_api::RequestGroup>,
    ) {
        if self.kind == PickKind::BankTxn
            && self.accounts.is_empty()
            && self.accounts_task.is_none()
        {
            self.load_accounts(link, group);
        }
        self.poll();
        let want = (self.search.trim().to_string(), self.account);
        if self.task.is_some() || self.searched.as_ref() == Some(&want) {
            return;
        }
        if self.kind == PickKind::BankTxn && self.account.is_none() {
            return;
        }
        self.searched = Some(want.clone());
        let (search, account) = want;
        let kind = self.kind;
        if let Some(mut demo) = link.demo_state() {
            let rows = super::demo::rows(&mut demo, kind, account, &search);
            drop(demo);
            self.task = Some(Task::ready(rows));
            self.poll();
            return;
        }
        let (Some(session), Some(group)) = (link.session(), group) else {
            return;
        };
        let filter = TransactionFilter {
            txn_type: match kind {
                PickKind::Bill => Some(TxnType::Bill),
                PickKind::Check => Some(TxnType::Expense),
                PickKind::BankTxn => None,
            },
            status: Some(TxnStatus::Posted),
            account_id: account,
            search: Some(search).filter(|s| !s.is_empty()),
            sort: Some((TxnSort::Date, SortDir::Desc)),
            ..TransactionFilter::default()
        };
        match group.read(&session, move |s| async move {
            let page = PageRequest::new(PAGE_LIMIT, 0)?;
            let rows = s.transactions().list(&filter, page).await?.data;
            Ok(rows
                .into_iter()
                .map(|r| PickRow {
                    id: r.id,
                    txn_type: r.txn_type,
                    date: r.txn_date,
                    number: r
                        .check_number
                        .map(|n| n.to_string())
                        .or(r.txn_number)
                        .filter(|n| !n.is_empty()),
                    payee: r.contact_name,
                    amount: r.display_total.or(r.total),
                })
                .collect())
        }) {
            Ok(p) => self.task = Some(Task::Pending(p)),
            Err(_) => self.failed = true,
        }
    }

    fn load_accounts(&mut self, link: &Arc<CompanyLink>, group: Option<&cl_api::RequestGroup>) {
        if let Some(mut demo) = link.demo_state() {
            let chart = demo
                .chart
                .get_or_insert_with(crate::company::demo::chart)
                .clone();
            drop(demo);
            self.accounts_task = Some(Task::ready(chart));
            self.poll();
            return;
        }
        let (Some(session), Some(group)) = (link.session(), group) else {
            return;
        };
        if let Ok(p) = group.read(&session, move |s| async move {
            s.accounts().list_all(&AccountFilter::default()).await
        }) {
            self.accounts_task = Some(Task::Pending(p));
        }
    }

    pub(crate) fn poll(&mut self) {
        if let Some(r) = self.accounts_task.as_mut().and_then(Task::try_take) {
            self.accounts_task = None;
            if let Ok(all) = r {
                self.accounts = bank_accounts(&all);
                if self.account.is_none() {
                    self.account = self.accounts.first().map(|a| a.0);
                }
            }
        }
        if let Some(r) = self.task.as_mut().and_then(Task::try_take) {
            self.task = None;
            match r {
                Ok(rows) => {
                    self.rows = rows;
                    self.loaded = true;
                    self.failed = false;
                }
                Err(_) => self.failed = true,
            }
        }
    }
}

/// Active bank and card accounts, as "number Â· name".
pub(crate) fn bank_accounts(all: &[Account]) -> Vec<(AccountId, String)> {
    all.iter()
        .filter(|a| {
            a.is_active
                && matches!(
                    crate::screens::register::class_of(a),
                    Some(AccountClass::Bank | AccountClass::CreditCard)
                )
        })
        .map(|a| {
            let name = match &a.fields.account_number {
                Some(n) if !n.is_empty() => format!("{n} \u{b7} {}", a.fields.name),
                _ => a.fields.name.clone(),
            };
            (a.id, name)
        })
        .collect()
}
