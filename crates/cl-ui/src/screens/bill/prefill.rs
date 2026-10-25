//! Vendor prefill (Q-191, Q-P13o) and the vendor's default account (Q-192).
//!
//! When the owner picks the vendor of a new, untouched bill (no line used on
//! either tab, no memo), the screen reads the vendor's newest posted bill
//! (else its newest check or expense) and copies its lines' accounts,
//! items, descriptions, customers, billable marks and tags, its memo and
//! terms. **Every amount, quantity and cost stays blank** and the amount due
//! is never touched (non-negotiable 4): the copy goes through
//! `BillForm::prefill_from_last`. With no earlier transaction the vendor's
//! default expense account (or the engine's suggestion) and default tag fill
//! the first expense line instead. The vendor's terms fill Terms unless the
//! owner chose one. Prefill never runs while a saved bill is open.

use cl_api::{BillListQuery, BillRecord, BillSort, SortDir, SuggestedAccount, TxnStatus};
use cl_core::ids::{AccountId, ContactId, TagId};
use cl_model::forms::PayeePrefill;
use cl_model::forms::bill::BillForm;
use cl_model::forms::item_lines::{ItemLine, ItemLines};
use cl_model::forms::lines::{FormLine, FormLines};
use cl_model::rowkey::RowKeyGen;

use super::{EnterBillScreen, Field, field};
use crate::company::Task;
use crate::screens::write_check::prefill::{LastTxn, copyable, last_of};

/// How many of the vendor's bills (and other transactions) are looked
/// through.
const LOOK_BACK: u16 = 10;

/// The engine's answers for one vendor.
#[derive(Clone, Default)]
pub(crate) struct PrefillData {
    /// The newest posted bill.
    pub bill: Option<BillRecord>,
    /// Else the newest check or expense.
    pub last: Option<LastTxn>,
    /// Else the engine's account suggestion.
    pub suggest: Option<SuggestedAccount>,
}

/// Counts only.
impl std::fmt::Debug for PrefillData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PrefillData")
            .field("bill", &self.bill.is_some())
            .field("last", &self.last.is_some())
            .finish_non_exhaustive()
    }
}

/// Prefill for the picked vendor.
#[derive(Default)]
pub(crate) struct PrefillState {
    pub task: Option<(ContactId, Task<PrefillData>)>,
    /// Prefill filled the form (control channel `prefill.applied`).
    pub applied: bool,
    /// `bill` (the last bill), `last` (a check or expense) or `default`.
    pub source: Option<&'static str>,
    /// Some details of the last transaction were inactive (or need
    /// `bill-lines-v1`) and were left out.
    pub dropped: bool,
    /// The owner chose the terms: a vendor change leaves them.
    pub terms_chosen: bool,
}

/// A saved bill as the form prefill copies from (amounts kept here; the
/// copy clears them). Keys come from the form's own generators.
pub(crate) fn last_form(
    r: &BillRecord,
    expense_keys: &mut RowKeyGen,
    item_keys: &mut RowKeyGen,
) -> BillForm {
    let mut f = BillForm {
        memo: r.txn.memo.clone().unwrap_or_default(),
        terms: r.payment_terms.clone(),
        terms_days: r.terms_days.and_then(|d| u16::try_from(d).ok()),
        ..BillForm::default()
    };
    let memo = r.txn.memo.as_deref().unwrap_or_default();
    // The engine stores the memo on a line without its own description.
    let desc = |d: &Option<String>| {
        d.as_deref()
            .filter(|d| *d != memo)
            .unwrap_or_default()
            .to_string()
    };
    f.expenses = FormLines {
        lines: r
            .lines
            .iter()
            .filter(|l| l.item.is_none() && l.quantity.is_none())
            .map(|l| FormLine {
                account: Some(l.account),
                amount: Some(l.amount),
                memo: desc(&l.description),
                customer: l.customer,
                billable: l.billable,
                tag: l.tag,
                ..FormLine::blank(expense_keys.next_key())
            })
            .collect(),
    };
    f.items = ItemLines {
        lines: r
            .lines
            .iter()
            .filter(|l| l.item.is_some() || l.quantity.is_some())
            .map(|l| ItemLine {
                item: l.item,
                description: desc(&l.description),
                qty: l.quantity,
                rate: l.unit_price,
                amount: Some(l.amount),
                account: Some(l.account),
                customer: l.customer,
                billable: l.billable,
                tag: l.tag,
                ..ItemLine::blank(item_keys.next_key())
            })
            .collect(),
    };
    f
}

impl EnterBillScreen {
    /// A vendor was picked: terms, the item default account, then prefill
    /// when the bill is new and untouched.
    pub(crate) fn pick_vendor(&mut self, id: ContactId) {
        self.vendor_list = false;
        self.inputs.vendor_problem = false;
        let name = self.lists.name_text(id).unwrap_or_default().to_string();
        self.inputs.vendor.clone_from(&name);
        let same = self.vendor() == Some(id);
        self.header.edit(field::VENDOR, |h| h.vendor = Some(id));
        if !same {
            self.vendor_picked(id);
        }
        self.inputs.rev = self.header.revision();
    }

    /// The vendor changed (picked, or cleared with `None` elsewhere).
    fn vendor_picked(&mut self, id: ContactId) {
        if self.editing.is_none() && !self.prefill.terms_chosen {
            let terms = self
                .contacts
                .iter()
                .find(|c| c.id == id)
                .and_then(|c| c.fields.default_payment_terms.clone());
            self.header.edit(field::VENDOR, |h| h.terms = terms);
        }
        self.items
            .set_default_account(self.vendor_default_account());
        self.refresh_terms_dates();
        if self.editing.is_some() || !self.compose().can_prefill() {
            return;
        }
        self.start_prefill(id);
    }

    fn start_prefill(&mut self, id: ContactId) {
        self.prefill.task = None;
        let link = std::sync::Arc::clone(&self.link);
        if let Some(mut demo) = link.demo_state() {
            let bill = crate::company::demo::bills::newest_for_vendor(&mut demo, id);
            let last = if bill.is_none() {
                crate::company::demo::checks::contact_transactions(
                    &mut demo,
                    id,
                    usize::from(LOOK_BACK),
                )
                .into_iter()
                .find(|t| t.txn_type == cl_api::TxnType::Expense && t.status != TxnStatus::Void)
                .and_then(|t| crate::company::demo::checks::transaction(&mut demo, t.id))
                .map(|t| last_of(&t))
            } else {
                None
            };
            let suggest = if bill.is_none() && last.is_none() {
                crate::company::demo::checks::suggest_account(&mut demo, id)
            } else {
                None
            };
            drop(demo);
            let data = PrefillData {
                bill,
                last,
                suggest,
            };
            self.prefill.task = Some((id, Task::ready(data)));
            self.poll_prefill();
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        if let Ok(p) = group.read(&session, move |s| async move {
            let q = BillListQuery {
                vendor: Some(id),
                sort: Some((BillSort::Date, SortDir::Desc)),
                ..BillListQuery::default()
            };
            let page = cl_api::PageRequest::new(LOOK_BACK, 0)?;
            let pick = s
                .bills()
                .list(&q, page)
                .await?
                .data
                .into_iter()
                .find(|b| b.status == TxnStatus::Posted);
            let bill = match pick {
                Some(b) => Some(s.bills().get(b.id).await?.record),
                None => None,
            };
            let mut last = None;
            if bill.is_none() {
                let page = cl_api::PageRequest::new(LOOK_BACK, 0)?;
                let rows = s.contacts().transactions(id, page).await?;
                let pick = rows.data.into_iter().find(|t| {
                    copyable(t.txn_type)
                        && t.txn_type != cl_api::TxnType::Bill
                        && t.status != TxnStatus::Void
                });
                if let Some(t) = pick {
                    last = Some(last_of(&s.transactions().get(t.id).await?.record));
                }
            }
            let suggest = if bill.is_none() && last.is_none() {
                s.contacts().suggest_account(id).await.ok().flatten()
            } else {
                None
            };
            Ok(PrefillData {
                bill,
                last,
                suggest,
            })
        }) {
            self.prefill.task = Some((id, Task::Pending(p)));
        }
    }

    /// Applies prefill once its answers are in (only if the form is still
    /// untouched and the vendor is the same).
    pub(crate) fn poll_prefill(&mut self) {
        let Some((id, task)) = self.prefill.task.as_mut() else {
            return;
        };
        let id = *id;
        let Some(r) = task.try_take() else { return };
        self.prefill.task = None;
        let Ok(data) = r else { return };
        if self.editing.is_some() || self.vendor() != Some(id) || !self.compose().can_prefill() {
            return;
        }
        if let Some(bill) = data.bill.filter(|b| !b.lines.is_empty()) {
            self.apply_last_bill(&bill);
        } else if let Some(last) = data.last.filter(|l| !l.lines.is_empty()) {
            self.apply_last_txn(&last);
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

    /// Leaves out what can't be saved: inactive accounts (the line), tags
    /// and customers (the detail), and customers and Billable without
    /// `bill-lines-v1`. True when something was left out.
    fn keep_active(&self, last: &mut BillForm) -> bool {
        let lines_cap = self.caps().lines;
        let mut dropped = false;
        let before = last.expenses.lines.len() + last.items.lines.len();
        last.expenses
            .lines
            .retain(|l| l.account.is_none_or(|a| self.account_active(a)));
        last.items
            .lines
            .retain(|l| lines_cap && l.account.is_none_or(|a| self.account_active(a)));
        dropped |= last.expenses.lines.len() + last.items.lines.len() != before;
        for l in &mut last.expenses.lines {
            if l.tag.is_some_and(|t| !self.tag_active(t)) {
                l.tag = None;
                dropped = true;
            }
            if l.customer
                .is_some_and(|c| !lines_cap || !self.contact_active(c))
                || (l.billable && !lines_cap)
            {
                l.customer = None;
                l.billable = false;
                dropped = true;
            }
        }
        for l in &mut last.items.lines {
            if l.tag.is_some_and(|t| !self.tag_active(t)) {
                l.tag = None;
                dropped = true;
            }
            if l.customer.is_some_and(|c| !self.contact_active(c)) {
                l.customer = None;
                l.billable = false;
                dropped = true;
            }
        }
        dropped
    }

    /// Puts a prefilled form's memo, terms and lines into the screen.
    fn take_prefill(&mut self, f: BillForm, source: &'static str, dropped: bool) {
        let BillForm {
            memo,
            terms,
            terms_days,
            header_tags,
            mut expenses,
            mut items,
            ..
        } = f;
        let keep_terms = self.prefill.terms_chosen;
        self.header.edit(field::VENDOR, |h| {
            h.memo = memo;
            if !keep_terms && terms.is_some() {
                h.terms = terms;
                h.terms_days = terms_days;
            }
            h.header_tags = header_tags;
        });
        expenses.ensure_entry(&mut self.expenses.form_mut().adapter_mut().keys);
        items.ensure_entry(self.items.keys());
        self.expenses.form_mut().edit_step(|v| *v = expenses);
        self.items.form_mut().edit_step(|v| *v = items);
        self.prefill.applied = true;
        self.prefill.source = Some(source);
        self.prefill.dropped = dropped;
        self.focus = Some(Field::Amount);
        self.refresh_terms_dates();
        self.inputs.rev = u64::MAX;
    }

    /// Copies the vendor's newest bill (Q-191): never an amount.
    fn apply_last_bill(&mut self, bill: &BillRecord) {
        let mut ek = std::mem::take(&mut self.expenses.form_mut().adapter_mut().keys);
        let mut last = last_form(bill, &mut ek, self.items.keys());
        self.expenses.form_mut().adapter_mut().keys = ek;
        // The header tags stay as they are on this bill.
        last.header_tags
            .clone_from(&self.header.current().header_tags);
        let dropped = self.keep_active(&mut last);
        let mut f = self.compose();
        f.prefill_from_last(&last);
        self.take_prefill(f, "bill", dropped);
    }

    /// Copies the vendor's newest check or expense: accounts, memos,
    /// customers, billable and tags; never an amount.
    fn apply_last_txn(&mut self, last: &LastTxn) {
        let keys = &mut self.expenses.form_mut().adapter_mut().keys;
        let mut from = BillForm {
            memo: last.memo.clone(),
            ..BillForm::default()
        };
        from.expenses = FormLines {
            lines: last
                .lines
                .iter()
                .map(|l| FormLine {
                    account: Some(l.account),
                    memo: l.memo.clone(),
                    customer: l.customer,
                    billable: l.billable,
                    tag: l.tag,
                    ..FormLine::blank(keys.next_key())
                })
                .collect(),
        };
        from.header_tags
            .clone_from(&self.header.current().header_tags);
        from.terms = self.header.current().terms.clone();
        from.terms_days = self.header.current().terms_days;
        let dropped = self.keep_active(&mut from);
        let mut f = self.compose();
        f.prefill_from_last(&from);
        self.take_prefill(f, "last", dropped);
    }

    /// No earlier transaction: the vendor's default account and tag fill an
    /// untouched first line (Q-192, Q-178).
    fn apply_default(&mut self, id: ContactId, suggest: Option<SuggestedAccount>) {
        let contact = self.contacts.iter().find(|c| c.id == id);
        let account = contact
            .and_then(|c| c.fields.default_expense_account_id)
            .or_else(|| suggest.map(|s| s.account))
            .filter(|a| self.account_active(*a));
        let tag = contact
            .and_then(|c| c.fields.default_tag_id)
            .filter(|t| self.tag_active(*t));
        let Some(account) = account else { return };
        let mut f = self.compose();
        let keys = &mut self.expenses.form_mut().adapter_mut().keys;
        if f.apply_vendor_default(account, tag, keys) {
            let lines = f.expenses;
            self.expenses.form_mut().edit_step(|v| *v = lines);
            self.prefill.applied = true;
            self.prefill.source = Some("default");
        }
    }

    /// The vendor's address as lines (read-only, Q-P17f).
    pub(crate) fn vendor_address(&self) -> String {
        let Some(c) = self
            .vendor()
            .and_then(|v| self.contacts.iter().find(|c| c.id == v))
        else {
            return String::new();
        };
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
        std::iter::once(f.display_name.clone())
            .chain(
                [f.billing_line1.clone(), f.billing_line2.clone(), Some(city)]
                    .into_iter()
                    .flatten()
                    .filter(|s| !s.trim().is_empty()),
            )
            .collect::<Vec<_>>()
            .join("\n")
    }
}
