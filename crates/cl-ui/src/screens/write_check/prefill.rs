//! Payee prefill (Q-191) and the vendor's default account (Q-192).
//!
//! When the owner picks who the check is to on a new, untouched check (no
//! line used, no memo), the screen reads the payee's newest check, expense
//! or bill and copies its line accounts, line memos, customers, billable
//! marks and tags, and its memo. **Every amount stays blank** and the check
//! amount is never touched (non-negotiable 4): the copy goes through
//! `CheckForm::prefill_from_last`, which clears line amounts. With no
//! earlier transaction, the vendor's default expense account (or the
//! engine's suggestion) and default tag fill the first line instead.
//!
//! The address comes from the contact's billing lines, unless the owner
//! typed one. Prefill never runs while a saved check is open.

use cl_api::{SuggestedAccount, Transaction, TxnStatus, TxnType};
use cl_core::ids::{AccountId, ContactId, TagId};
use cl_model::forms::PayeePrefill;
use cl_model::forms::lines::{FormLine, FormLines};

use super::{Field, WriteCheckScreen, blank, field};
use crate::company::Task;

/// How many of the payee's transactions are looked through for the last
/// check, expense or bill.
pub(crate) const LOOK_BACK: u16 = 10;

/// One line of the last transaction (never its amount).
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct LastLine {
    pub account: AccountId,
    pub memo: String,
    pub customer: Option<ContactId>,
    pub billable: bool,
    pub tag: Option<TagId>,
}

/// What prefill copies from the payee's last transaction.
#[derive(Clone, Default, PartialEq, Eq)]
pub(crate) struct LastTxn {
    pub memo: String,
    pub lines: Vec<LastLine>,
}

/// Counts only: memos and accounts are owner data.
impl std::fmt::Debug for LastTxn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LastTxn")
            .field("lines", &self.lines.len())
            .finish_non_exhaustive()
    }
}

/// The engine's answers for one payee.
#[derive(Clone, Debug, Default)]
pub(crate) struct PrefillData {
    pub last: Option<LastTxn>,
    pub suggest: Option<SuggestedAccount>,
}

/// Prefill for the picked payee.
#[derive(Default)]
pub(crate) struct PrefillState {
    pub task: Option<(ContactId, Task<PrefillData>)>,
    /// Prefill filled the form (control channel `prefill.applied`).
    pub applied: bool,
    /// `last` (the last transaction) or `default` (the vendor default).
    pub source: Option<&'static str>,
    /// Some details of the last transaction were inactive and left out.
    pub dropped: bool,
    /// The address the screen filled in (an address the owner typed is
    /// never replaced).
    pub auto_address: Option<String>,
}

/// The kinds a check copies from: what the owner paid this payee for.
pub(crate) fn copyable(t: TxnType) -> bool {
    matches!(t, TxnType::Expense | TxnType::Bill)
}

/// The expense side of a check, expense or bill: its debit lines (the bank
/// or payables line is the credit). Amounts are not kept.
#[must_use]
pub(crate) fn last_of(t: &Transaction) -> LastTxn {
    LastTxn {
        memo: t.memo.clone().unwrap_or_default(),
        lines: t
            .lines
            .iter()
            .filter(|l| l.debit.is_positive() && l.credit.is_zero())
            .map(|l| LastLine {
                account: l.account_id,
                memo: l.description.clone().unwrap_or_default(),
                customer: l.contact_id,
                billable: l.is_billable,
                tag: l.tag_id,
            })
            .collect(),
    }
}

/// The contact's billing address as lines (name first, as printed).
fn billing_address(c: &cl_api::Contact) -> String {
    let f = &c.fields;
    let city = [
        f.billing_city.clone(),
        f.billing_state.clone().map(|st| match &f.billing_zip {
            Some(z) => format!("{st} {z}"),
            None => st,
        }),
    ]
    .into_iter()
    .flatten()
    .filter(|s| !s.trim().is_empty())
    .collect::<Vec<_>>()
    .join(", ");
    let lines: Vec<String> = [f.billing_line1.clone(), f.billing_line2.clone(), Some(city)]
        .into_iter()
        .flatten()
        .filter(|s| !s.trim().is_empty())
        .collect();
    if lines.is_empty() {
        return String::new();
    }
    std::iter::once(f.display_name.clone())
        .chain(lines)
        .collect::<Vec<_>>()
        .join("\n")
}

impl WriteCheckScreen {
    /// A payee was picked: the address, then prefill when the check is new
    /// and untouched.
    pub(crate) fn payee_picked(&mut self, id: ContactId) {
        let address = self
            .contacts
            .iter()
            .find(|c| c.id == id)
            .map(billing_address)
            .unwrap_or_default();
        let current = self.header.current().address.clone();
        let hand_typed =
            !current.trim().is_empty() && self.prefill.auto_address.as_ref() != Some(&current);
        if !hand_typed && self.editing.is_none() && current != address {
            self.header
                .edit(field::ADDRESS, |h| h.address.clone_from(&address));
            self.prefill.auto_address = Some(address);
        }
        if self.editing.is_some() || !self.compose().can_prefill() {
            return;
        }
        self.start_prefill(id);
    }

    fn start_prefill(&mut self, id: ContactId) {
        self.prefill.task = None;
        let link = std::sync::Arc::clone(&self.link);
        if let Some(mut demo) = link.demo_state() {
            let last = crate::company::demo::checks::contact_transactions(
                &mut demo,
                id,
                usize::from(LOOK_BACK),
            )
            .into_iter()
            .find(|t| copyable(t.txn_type) && t.status != TxnStatus::Void)
            .and_then(|t| crate::company::demo::checks::transaction(&mut demo, t.id))
            .map(|t| last_of(&t));
            let suggest = if last.is_none() {
                crate::company::demo::checks::suggest_account(&mut demo, id)
            } else {
                None
            };
            drop(demo);
            self.prefill.task = Some((id, Task::ready(PrefillData { last, suggest })));
            self.poll_prefill();
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        if let Ok(p) = group.read(&session, move |s| async move {
            let page = cl_api::PageRequest::new(LOOK_BACK, 0)?;
            let rows = s.contacts().transactions(id, page).await?;
            let pick = rows
                .data
                .into_iter()
                .find(|t| copyable(t.txn_type) && t.status != TxnStatus::Void);
            let last = match pick {
                Some(t) => Some(last_of(&s.transactions().get(t.id).await?.record)),
                None => None,
            };
            let suggest = if last.is_none() {
                s.contacts().suggest_account(id).await.ok().flatten()
            } else {
                None
            };
            Ok(PrefillData { last, suggest })
        }) {
            self.prefill.task = Some((id, Task::Pending(p)));
        }
    }

    /// Applies prefill once its answers are in (only if the form is still
    /// untouched and the payee is the same).
    pub(crate) fn poll_prefill(&mut self) {
        let Some((id, task)) = self.prefill.task.as_mut() else {
            return;
        };
        let id = *id;
        let Some(r) = task.try_take() else { return };
        self.prefill.task = None;
        let Ok(data) = r else { return };
        if self.editing.is_some()
            || self.header.current().payee != Some(id)
            || !self.compose().can_prefill()
        {
            return;
        }
        if let Some(last) = data.last.filter(|l| !l.lines.is_empty()) {
            self.apply_last(&last);
        } else {
            self.apply_default(id, data.suggest);
        }
    }

    fn account_active(&self, a: AccountId) -> bool {
        self.accounts.iter().any(|x| x.id == a && x.is_active)
    }

    fn tag_active(&self, t: TagId) -> bool {
        self.tags.iter().any(|x| x.id == t && x.is_active)
    }

    fn contact_active(&self, c: ContactId) -> bool {
        self.contacts.iter().any(|x| x.id == c && x.is_active)
    }

    /// Copies the last transaction (Q-191): accounts, memos, customers,
    /// billable and tags; never an amount. Inactive details are left out.
    fn apply_last(&mut self, last: &LastTxn) {
        let mut dropped = false;
        // Without `line-billable-v1` a line customer could never be saved:
        // it is not copied.
        let customers = self.line_billable();
        let keys = &mut self.lines.form_mut().adapter_mut().keys;
        let mut seeds = Vec::new();
        for l in &last.lines {
            let key = keys.next_key();
            seeds.push(FormLine {
                account: Some(l.account),
                memo: l.memo.clone(),
                customer: l.customer.filter(|_| customers),
                billable: l.billable && customers,
                tag: l.tag,
                ..FormLine::blank(key)
            });
        }
        let lines = FormLines { lines: seeds };
        let mut seeds = Vec::new();
        for mut l in lines.lines {
            if l.account.is_some_and(|a| !self.account_active(a)) {
                dropped = true;
                continue;
            }
            if l.tag.is_some_and(|t| !self.tag_active(t)) {
                dropped = true;
                l.tag = None;
            }
            if l.customer.is_some_and(|c| !self.contact_active(c)) {
                dropped = true;
                l.customer = None;
                l.billable = false;
            }
            seeds.push(l);
        }
        let mut f = self.compose();
        let mut from = blank(f.bank, f.date.unwrap_or(self.today), f.number.clone());
        from.memo.clone_from(&last.memo);
        // The address and header tags stay as they are on this check.
        from.address.clone_from(&f.address);
        from.header_tags.clone_from(&f.header_tags);
        from.lines = FormLines { lines: seeds };
        f.prefill_from_last(&from);
        let (memo, lines) = (f.memo, f.lines);
        self.header.edit(field::PAYEE, |h| h.memo = memo);
        self.lines.form_mut().edit_step(|v| *v = lines);
        let mut v = self.lines.value().clone();
        let keys = &mut self.lines.form_mut().adapter_mut().keys;
        v.ensure_entry(keys);
        if &v != self.lines.value() {
            self.lines.form_mut().edit_step(|x| *x = v);
        }
        self.prefill.applied = true;
        self.prefill.source = Some("last");
        self.prefill.dropped = dropped;
        self.focus = Some(Field::Amount);
        self.inputs.rev = u64::MAX;
    }

    /// No earlier transaction: the vendor's default account and tag fill
    /// an untouched first line (Q-192, Q-178).
    fn apply_default(&mut self, id: ContactId, suggest: Option<SuggestedAccount>) {
        let contact = self.contacts.iter().find(|c| c.id == id);
        let account = suggest
            .map(|s| s.account)
            .or_else(|| contact.and_then(|c| c.fields.default_expense_account_id))
            .filter(|a| self.account_active(*a));
        let tag = contact
            .and_then(|c| c.fields.default_tag_id)
            .filter(|t| self.tag_active(*t));
        let Some(account) = account else { return };
        let mut f = self.compose();
        let keys = &mut self.lines.form_mut().adapter_mut().keys;
        if f.apply_vendor_default(account, tag, keys) {
            let lines = f.lines;
            self.lines.form_mut().edit_step(|v| *v = lines);
            self.prefill.applied = true;
            self.prefill.source = Some("default");
        }
    }
}
