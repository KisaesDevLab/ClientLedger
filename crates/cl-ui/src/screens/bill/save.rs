//! Saving a bill or vendor credit (P17, plan §3 WP4, §5): create or edit,
//! the closing-date password, the PR-1 unknown-save block, a changed
//! record (409 `STALE`), the engine's A/P refusals and the partial failure
//! "the bill is saved, its file isn't attached yet"; then Void, Delete,
//! Prev/Next over the saved bills and History.
//!
//! Money (plan §4): `check()` is the exact gate (the lines equal the amount
//! due to the cent, or nothing is sent); [`to_draft`] then copies the
//! checked values onto the engine draft one field at a time, with no
//! arithmetic; cl-api's `check_doc` refuses lines that don't add up once
//! more before anything leaves. Item amounts are the checked `cost × qty`
//! (D6), which the engine recomputes and refuses on a mismatch.
//!
//! Unknown outcomes (Q-PR1a/b, D2): a save whose reply was lost marks the
//! form unconfirmed and is kept in the company's lost-save registry under
//! [`super::LOST_SCOPE`]; the stored probe (a bill or credit of this vendor,
//! date, ref and total created after the send) answers Check again. Nothing
//! is retried blindly.

use std::sync::Arc;

use cl_api::{
    ApRefusal, ApWriteOptions, ApiError, AttachTo, AttachmentId, AuditAction, AuditEntity,
    BillDraft, BillLineDraft, BillListQuery, BillRecord, BillSort, BillTerms,
    ClosedPeriodAuthorization, ConflictReason, CreditListQuery, CreditSort, DraftKey,
    IdempotencyKey, LocalValidation, OrphanLabel, PageRequest, PostingOptions, RecordVersion,
    SortDir, TxnStatus, VendorCreditDraft, Versioned, ap_refusal,
};
use cl_core::ids::TransactionId;
use cl_core::{CalendarDate, Secret};
use cl_model::closing::is_locked;
use cl_model::forms::bill::{
    BillContext, BillField, BillForm, BillKind, BillProblem, CheckedBill, PaymentTerm, check,
};
use cl_model::forms::item_lines::{ItemLine, ItemLines};
use cl_model::forms::lines::{FormLine, FormLines};
use cl_model::forms::{SaveFailure, SaveIntent, SaveStart};
use cl_model::rowkey::RowKeyGen;

use super::{EnterBillScreen, Field, LastSaved, Pending, Tab, blank};
use crate::company::{CompanyEvent, Task};
use crate::forms::closing_prompt::Verb;
use crate::forms::lines_grid::{L_ACCOUNT, L_AMOUNT, L_CUSTOMER};

/// Bills are listed 50 at a time for Prev/Next.
const NAV_PAGE: u16 = 50;

/// The engine draft of a bill or a vendor credit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Draft {
    /// `POST /bills`, `PUT /bills/{id}`.
    Bill(BillDraft),
    /// `POST /vendor-credits`, `PUT /vendor-credits/{id}`.
    Credit(VendorCreditDraft),
}

/// What a save sends, for its idempotency key: the draft, the record and
/// version it edits, and whether a closing-date password goes with it
/// (never the password itself).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SaveIdentity {
    pub draft: Draft,
    pub edit: Option<(TransactionId, RecordVersion)>,
    pub password: bool,
}

/// A bill saved, whose files did not move onto it (plan §5): what Try
/// again sends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AttachRetry {
    pub id: TransactionId,
    pub kind: BillKind,
    pub draft: DraftKey,
    pub files: Vec<AttachmentId>,
}

/// A save the engine took.
#[derive(Clone, Debug)]
pub(crate) struct Saved {
    /// The record as saved (its version guards the next edit).
    pub record: Versioned<BillRecord>,
    /// The draft's files are not on it yet.
    pub attach_failed: Option<AttachRetry>,
}

/// The checked bill as the engine's draft, field by field (no arithmetic):
/// the amount due and every line amount are the owner's typed `Money` (an
/// item line's is the checked `cost × qty`). `draft` is sent on the create
/// only with `bill-lines-v1` (`lines`); otherwise the files are moved after
/// the save.
#[must_use]
pub fn to_draft(c: &CheckedBill, draft: Option<DraftKey>, lines: bool) -> Draft {
    let mut out: Vec<BillLineDraft> = c
        .expense_lines
        .iter()
        .map(|l| BillLineDraft {
            account: l.account,
            amount: l.amount,
            description: l.memo.clone(),
            tag: l.tag,
            item: None,
            quantity: None,
            unit_price: None,
            customer: l.contact,
            billable: l.billable,
        })
        .collect();
    out.extend(c.item_lines.iter().map(|l| BillLineDraft {
        account: l.account,
        amount: l.amount,
        description: l.description.clone(),
        tag: l.tag,
        item: l.item,
        quantity: l.qty,
        unit_price: l.rate,
        customer: l.contact,
        billable: l.billable,
    }));
    let draft_attachment = draft.filter(|_| lines && !c.attachments.is_empty());
    match c.kind {
        BillKind::Bill => Draft::Bill(BillDraft {
            vendor: c.vendor,
            date: c.date,
            vendor_invoice_number: c.reference.clone(),
            terms: c.terms.as_ref().map(|t| BillTerms {
                code: t.code.clone(),
                days: t.days.map(u32::from),
                discount_percent: t
                    .discount_percent
                    .and_then(|p| cl_api::Percent::parse(&percent_text(p)).ok()),
                discount_days: t.discount_days.map(u32::from),
            }),
            due_date: c.due_date,
            memo: c.memo.clone(),
            amount_due: c.amount,
            lines: out,
            received: c.bill_received.unwrap_or(true),
            draft_attachment,
        }),
        BillKind::Credit => Draft::Credit(VendorCreditDraft {
            vendor: c.vendor,
            date: c.date,
            vendor_invoice_number: c.reference.clone(),
            memo: c.memo.clone(),
            amount: c.amount,
            lines: out,
            draft_attachment,
        }),
    }
}

/// A percentage as the engine's strict text (`2`, `1.5`).
pub(crate) fn percent_text(p: cl_model::forms::bill::Percent) -> String {
    let w = p.to_wire();
    if w.contains('.') {
        w.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        w
    }
}

/// One of the engine's payment terms (`GET /payment-terms`) as the form
/// checks it.
pub(crate) fn term_of(p: cl_api::PaymentTerm) -> Option<PaymentTerm> {
    let discount_percent = match p.discount_percent {
        Some(pct) => Some(cl_model::forms::bill::Percent::parse_user(&pct.to_wire())?),
        None => None,
    };
    Some(PaymentTerm {
        code: p.code,
        net_days: p.net_days.and_then(|d| u16::try_from(d).ok()),
        discount_percent,
        discount_days: p.discount_days.and_then(|d| u16::try_from(d).ok()),
    })
}

fn options(password: Option<Secret>) -> PostingOptions {
    PostingOptions {
        closed_period: password.map(ClosedPeriodAuthorization::with_password),
    }
}

/// The form of a saved bill or credit: its header, its expense lines and
/// its item lines (a line with an item or a quantity), exact amounts from
/// `GET /bills/{id}`, never from a list.
pub(crate) fn form_of(
    r: &BillRecord,
    expense_keys: &mut RowKeyGen,
    item_keys: &mut RowKeyGen,
) -> (BillForm, FormLines, ItemLines) {
    let kind = match r.kind() {
        cl_api::ApKind::Bill => BillKind::Bill,
        cl_api::ApKind::VendorCredit => BillKind::Credit,
    };
    let mut f = blank(kind, r.txn.txn_date);
    f.vendor = r.txn.contact_id;
    f.reference = r.vendor_invoice_number.clone().unwrap_or_default();
    f.terms.clone_from(&r.payment_terms);
    f.terms_days = r.terms_days.and_then(|d| u16::try_from(d).ok());
    // A custom term's discount (WP8; `bill-terms-v1`).
    f.discount_percent = r
        .discount_percent
        .and_then(|p| cl_model::forms::bill::Percent::parse_user(&p.to_wire()));
    f.discount_days = r.discount_days.and_then(|d| u16::try_from(d).ok());
    f.amount_due = r.txn.total;
    f.due_date = r.due_date;
    f.discount_date = r.discount_date;
    f.memo = r.txn.memo.clone().unwrap_or_default();
    f.bill_received = r.bill_received;
    let expenses = r
        .lines
        .iter()
        .filter(|l| l.item.is_none() && l.quantity.is_none())
        .map(|l| FormLine {
            account: Some(l.account),
            amount: Some(l.amount),
            memo: l.description.clone().unwrap_or_default(),
            customer: l.customer,
            billable: l.billable,
            tag: l.tag,
            ..FormLine::blank(expense_keys.next_key())
        })
        .collect();
    let items = r
        .lines
        .iter()
        .filter(|l| l.item.is_some() || l.quantity.is_some())
        .map(|l| ItemLine {
            item: l.item,
            description: l.description.clone().unwrap_or_default(),
            qty: l.quantity,
            rate: l.unit_price,
            amount: Some(l.amount),
            account: Some(l.account),
            customer: l.customer,
            billable: l.billable,
            tag: l.tag,
            ..ItemLine::blank(item_keys.next_key())
        })
        .collect();
    (
        f,
        FormLines::from_lines(expenses, expense_keys),
        ItemLines::from_lines(items, item_keys),
    )
}

/// The plain message (i18n key) for an engine refusal of a bill or credit.
fn refusal_key(r: ApRefusal) -> Option<&'static str> {
    Some(match r {
        ApRefusal::Stale => return None,
        ApRefusal::CreditApplied => "bill.refused.credit_applied",
        ApRefusal::BillHasPayments => "bill.refused.has_payments",
        ApRefusal::LineBilled => "bill.refused.line_billed",
        ApRefusal::Deposited => "bill.refused.deposited",
        ApRefusal::DeleteLinked => "bill.refused.delete_linked",
        ApRefusal::DeleteUnsupported => "bill.refused.delete_unsupported",
        ApRefusal::LineAmountMismatch | ApRefusal::LineQtyRateIncomplete => {
            "bill.refused.line_amount"
        }
    })
}

impl EnterBillScreen {
    /// Puts the keyboard on the field a problem belongs to.
    fn focus_problem(&mut self, p: BillProblem) {
        let lists = Arc::clone(&self.lists);
        let cell = |row, col| cl_grid::CellPos { row, col };
        let in_items = |s: &Self, k| s.items.value().lines.iter().any(|l| l.key == k);
        match p.field() {
            BillField::Vendor => self.focus = Some(Field::Vendor),
            BillField::Date => self.focus = Some(Field::Date),
            BillField::Reference => self.focus = Some(Field::Reference),
            BillField::Terms | BillField::Discount => self.focus = Some(Field::Terms),
            BillField::DueDate | BillField::DiscountDate | BillField::Received => {
                self.focus = Some(Field::DueDate);
            }
            BillField::AmountDue => self.focus = Some(Field::Amount),
            BillField::Lines => match self.tab {
                Tab::Expenses => self.expenses.focus_first(),
                Tab::Items => self.items.focus_first(),
            },
            BillField::Items => {
                self.tab = Tab::Items;
                self.items.focus_first();
            }
            BillField::LineAccount(k) if in_items(self, k) => {
                self.tab = Tab::Items;
                self.items.focus(k, super::items::I_ACCOUNT);
            }
            BillField::LineAmount(k) if in_items(self, k) => {
                self.tab = Tab::Items;
                self.items.focus(k, super::items::I_AMOUNT);
            }
            BillField::LineCustomer(k) if in_items(self, k) => {
                self.tab = Tab::Items;
                self.items.focus(k, super::items::I_CUSTOMER);
            }
            BillField::LineQty(k) => {
                self.tab = Tab::Items;
                self.items.focus(k, super::items::I_QTY);
            }
            BillField::LineRate(k) => {
                self.tab = Tab::Items;
                self.items.focus(k, super::items::I_COST);
            }
            BillField::LineAccount(k) => {
                self.tab = Tab::Expenses;
                self.expenses.form_mut().focus(cell(k, L_ACCOUNT), &*lists);
            }
            BillField::LineAmount(k) => {
                self.tab = Tab::Expenses;
                self.expenses.form_mut().focus(cell(k, L_AMOUNT), &*lists);
            }
            BillField::LineCustomer(k) => {
                self.tab = Tab::Expenses;
                self.expenses.form_mut().focus(cell(k, L_CUSTOMER), &*lists);
            }
            BillField::ReadBanner => {}
        }
    }

    fn refuse(&mut self, problems: Vec<BillProblem>) {
        if let Some(first) = problems.first().copied() {
            self.focus_problem(first);
        }
        self.save.problems = problems;
    }

    /// The context the checks run in.
    pub(crate) fn context_check(&self, form: &BillForm) -> Result<CheckedBill, Vec<BillProblem>> {
        let ctx = BillContext {
            tags: &self.tag_list,
            terms: &self.terms,
            closing_date: self.closing,
            original_date: self.editing.as_ref().map(|e| e.old_date),
            caps: self.caps(),
        };
        check(form, &ctx)
    }

    /// The problems a refused save showed, checked again against the form
    /// as it is now (each frame while any show).
    pub(crate) fn recheck_problems(&mut self) {
        if self.save.problems.is_empty() {
            return;
        }
        let now = self
            .context_check(&self.compose())
            .err()
            .unwrap_or_default();
        crate::forms::recheck(&mut self.save.problems, now);
    }

    /// Commits the amount and vendor text still being typed. False, with
    /// the keyboard put there, when the typed text is not accepted: the
    /// bill is never saved with an amount or vendor other than the one
    /// shown.
    pub(crate) fn commit_header(&mut self) -> bool {
        if self.inputs.rev != self.header.revision() {
            return true;
        }
        let (amount_shown, vendor_shown) = {
            let f = self.header.current();
            let amount = f
                .amount_due
                .map(|m| m.format(self.money_style))
                .unwrap_or_default();
            let vendor = f
                .vendor
                .and_then(|p| self.lists.name_text(p))
                .unwrap_or_default()
                .to_string();
            (amount, vendor)
        };
        if self.inputs.amount.trim() != amount_shown.trim() {
            self.amount_left();
        }
        if self.inputs.vendor.trim() != vendor_shown.trim() {
            self.resolve_vendor();
        }
        if self.inputs.amount_problem.is_some() {
            self.focus = Some(Field::Amount);
            return false;
        }
        if self.inputs.vendor_problem {
            self.focus = Some(Field::Vendor);
            return false;
        }
        true
    }

    /// Checks the form and starts the save (Save & New, Save & Close, the
    /// close prompt's Save). Nothing is sent when the check doesn't pass,
    /// above all when the lines and the amount due disagree by any amount,
    /// or when a closed-period bill can't take the closing password yet.
    /// Returns true when there was nothing to save or it saved at once.
    pub(crate) fn start_save(&mut self, intent: SaveIntent, password: Option<Secret>) -> bool {
        if self.save.task.is_some() {
            return false;
        }
        // The last save's outcome is unknown: nothing is sent until the
        // recheck answers or the owner confirms (PR-1).
        if self.header.is_unconfirmed() || self.save.lost.pending() {
            self.save.notice = Some("bill.lost.blocked");
            return false;
        }
        if let Some(e) = self.editing.as_ref().filter(|e| e.read_only) {
            self.save.notice = Some(e.read_only_note);
            return false;
        }
        if !self.commit_header() {
            return false;
        }
        if !self.expenses.commit() || !self.items.commit() {
            return false;
        }
        self.left_lines();
        self.save.problems.clear();
        self.save.error = None;
        self.save.refusal = None;
        self.save.stale = false;
        self.save.notice = None;
        // A04 draft mode: a document is entered even when nothing was
        // typed (its checks still run).
        if !self.is_dirty() && self.draft.is_none() {
            self.follow_up(intent);
            return true;
        }
        if self.draft_locked() {
            return false;
        }
        let form = self.compose();
        let checked = match self.context_check(&form) {
            Ok(c) => c,
            Err(problems) => {
                self.refuse(problems);
                return false;
            }
        };
        // Another bill of this vendor uses the ref (WP8, Q-P17n): ask "Save
        // anyway / Go back" first; it warns, never blocks. A save waiting
        // for the check is sent once it answers.
        if password.is_none() && !self.with_panel(|p, input| p.clear_to_save(input, intent)) {
            return false;
        }
        // A change touching the closed period needs the closing-date
        // password (NN8): ask before anything is sent.
        if checked.needs_closing_password && password.is_none() {
            self.closing_prompt
                .ask(Pending::Save(intent), true, self.closing);
            return false;
        }
        // A04 draft mode: Accept is the convert, never `POST /bills`.
        if self.draft.is_some() {
            return self.accept_draft(&checked, intent, password);
        }
        let draft = to_draft(&checked, self.attach.draft, self.caps().lines);
        let lists = Arc::clone(&self.lists);
        let _ = self.expenses.form_mut().begin_save(intent, &*lists);
        let item_lists = self.items.lists();
        let _ = self.items.form_mut().begin_save(intent, &*item_lists);
        if let SaveStart::Busy = self.header.begin_save(intent) {
            return false;
        }
        self.save.intent = Some(intent);
        let edit = self.editing.as_ref().map(|e| (e.id, e.record.clone()));
        let files: Vec<AttachmentId> = form
            .attachments
            .iter()
            .filter_map(|a| AttachmentId::parse(&a.id).ok())
            .collect();
        let retry_draft = self.attach.draft.filter(|_| !files.is_empty());
        let lines_cap = self.caps().lines;
        if self.link.is_demo() {
            let r = self.demo_save(
                &draft,
                edit.as_ref(),
                password.as_ref(),
                retry_draft,
                lines_cap,
            );
            return match r {
                Ok(s) => {
                    self.saved(s);
                    true
                }
                Err(e) => {
                    self.save_failed(e);
                    false
                }
            };
        }
        let key = self.key_for(&SaveIdentity {
            draft: draft.clone(),
            edit: edit.as_ref().map(|(id, v)| (*id, v.version)),
            password: password.is_some(),
        });
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            self.save_failed(ApiError::NoCompanyOpen);
            return false;
        };
        let label = OrphanLabel {
            what: "orphan.bill_save",
            key: Some(key.clone()),
        };
        let written = group.write(&session, label, move |s| async move {
            let opts = options(password);
            let record = match (&draft, edit) {
                (Draft::Bill(d), None) => s.bills().create(d, &opts, key).await?,
                (Draft::Bill(d), Some((id, expected))) => {
                    s.bills().update(id, &expected, d, &opts, key).await?
                }
                (Draft::Credit(d), None) => s.vendor_credits().create(d, &opts, key).await?,
                (Draft::Credit(d), Some((id, expected))) => {
                    s.vendor_credits()
                        .update(id, &expected, d, &opts, key)
                        .await?
                }
            };
            // Before `bill-lines-v1` the files move after the create (two
            // calls, plan §2); a failure here leaves the bill saved.
            let id = record.record.txn.id;
            let credit = matches!(draft, Draft::Credit(_));
            let mut attach_failed = None;
            if let (Some(draft_key), false) = (retry_draft, lines_cap) {
                let moved = move_files(&s, id, credit, draft_key, &files).await;
                if moved.is_err() {
                    attach_failed = Some(AttachRetry {
                        id,
                        kind: if credit {
                            BillKind::Credit
                        } else {
                            BillKind::Bill
                        },
                        draft: draft_key,
                        files,
                    });
                }
            }
            Ok(Saved {
                record,
                attach_failed,
            })
        });
        match written {
            Ok(p) => self.save.task = Some(Task::Pending(p)),
            Err(_) => self.save_failed(ApiError::Local(LocalValidation::InvalidValue)),
        }
        false
    }

    fn key_for(&mut self, id: &SaveIdentity) -> IdempotencyKey {
        match &self.save.key {
            Some((d, k)) if d == id => k.clone(),
            _ => {
                let k = IdempotencyKey::new();
                self.save.key = Some((id.clone(), k.clone()));
                k
            }
        }
    }

    fn demo_save(
        &self,
        draft: &Draft,
        edit: Option<&(TransactionId, Versioned<BillRecord>)>,
        password: Option<&Secret>,
        retry_draft: Option<DraftKey>,
        lines_cap: bool,
    ) -> Result<Saved, ApiError> {
        use crate::company::demo::bills;
        let mut demo = self
            .link
            .demo_state()
            .ok_or(ApiError::Local(LocalValidation::InvalidValue))?;
        let id = match (draft, edit) {
            (Draft::Bill(d), None) => bills::create_bill(&mut demo, d, password)?,
            (Draft::Bill(d), Some((id, v))) => {
                bills::update_bill(&mut demo, *id, &v.version, d, password)?
            }
            (Draft::Credit(d), None) => bills::create_credit(&mut demo, d, password)?,
            (Draft::Credit(d), Some((id, v))) => {
                bills::update_credit(&mut demo, *id, &v.version, d, password)?
            }
        };
        if let (Some(key), false, None) = (retry_draft, lines_cap, edit) {
            let credit = matches!(draft, Draft::Credit(_));
            bills::attach_draft(&mut demo, id, key, credit)?;
        }
        let record = bills::get(&mut demo, id).ok_or(ApiError::NotFound { code: None })?;
        Ok(Saved {
            record,
            attach_failed: None,
        })
    }

    /// Polls the save, the record being read, the list, Void and History.
    pub(crate) fn poll_save(&mut self) {
        if let Some(r) = self.save.task.as_mut().and_then(Task::try_take) {
            self.save.task = None;
            match r {
                Ok(s) => self.saved(s),
                Err(e) => self.save_failed(e),
            }
        }
        if let Some(r) = self.save.attach_retry.as_mut().and_then(Task::try_take) {
            self.save.attach_retry = None;
            match r {
                Ok(()) => {
                    self.save.attach_failed = None;
                    self.save.notice = Some("bill.attach.moved");
                    self.load_attachments();
                }
                Err(_) => self.save.notice = Some("bill.attach.still_failed"),
            }
        }
        if let Some(ev) = self.save.lost.poll(&mut self.header) {
            self.lost_event(ev);
        }
        if let Some(r) = self.load_task.as_mut().and_then(Task::try_take) {
            self.load_task = None;
            match r {
                Ok(v) => self.show_record(&v),
                Err(e) => self.save.error = Some(e),
            }
        }
        if let Some(r) = self.nav.task.as_mut().and_then(Task::try_take) {
            self.nav.task = None;
            match r {
                Ok(rows) => {
                    self.nav.done = rows.len() < usize::from(NAV_PAGE);
                    self.nav.list.extend(rows);
                    if let Some(older) = self.nav.want_older.take() {
                        self.step(older);
                    }
                }
                Err(e) => {
                    self.nav.want_older = None;
                    self.save.error = Some(e);
                }
            }
        }
        if let Some(r) = self.void.task.as_mut().and_then(Task::try_take) {
            self.void.task = None;
            match r {
                Ok(()) => self.voided(),
                Err(e) => {
                    // An unknown outcome keeps the key: the same void goes
                    // again under it (a replay where the engine keys the
                    // route). A refusal gets a fresh one.
                    if !crate::screens::register::keeps_key(&e) {
                        self.void.key = None;
                    }
                    let reason = self.void.reason.clone();
                    let pending = if self.void.delete {
                        Pending::Delete(reason)
                    } else {
                        Pending::Void(reason)
                    };
                    self.write_failed(e, pending);
                }
            }
        }
        if let Some(r) = self.history.task.as_mut().and_then(Task::try_take) {
            self.history.task = None;
            match r {
                Ok(rows) => self.history.rows = rows,
                Err(_) => self.history.error = true,
            }
        }
    }

    pub(crate) fn closing_failed(&mut self, e: &ApiError, pending: Pending) -> bool {
        let supported = self.caps().closing;
        self.closing_prompt
            .failed(e, pending, supported, self.closing)
    }

    /// A refused void or delete: the closing dialog, a plain refusal, or
    /// the error.
    fn write_failed(&mut self, e: ApiError, pending: Pending) {
        if self.closing_failed(&e, pending) {
            return;
        }
        if let Some(key) = ap_refusal(&e).and_then(refusal_key) {
            self.save.refusal = Some(key);
        } else if matches!(ap_refusal(&e), Some(ApRefusal::Stale)) {
            self.save.stale = true;
        } else {
            self.save.error = Some(e);
        }
    }

    /// The engine took the save.
    fn saved(&mut self, s: Saved) {
        self.save.lost.saved();
        let vendor = s.record.record.txn.contact_id;
        self.publish(vendor);
        self.closing_prompt.saved();
        let intent = self.save.intent.take().unwrap_or(SaveIntent::Save);
        let kind = match s.record.record.kind() {
            cl_api::ApKind::Bill => BillKind::Bill,
            cl_api::ApKind::VendorCredit => BillKind::Credit,
        };
        self.save.count += 1;
        self.save.last = Some(LastSaved {
            kind,
            id: s.record.record.txn.id,
        });
        self.save.last_action = Some("save");
        self.save.key = None;
        self.save.problems.clear();
        self.save.error = None;
        self.save.attach_failed = s.attach_failed;
        let lists = Arc::clone(&self.lists);
        let header = self.header.current().clone();
        self.header.save_ok(header);
        let lines = self.expenses.value().clone();
        self.expenses.form_mut().save_ok(lines, &*lists);
        let items = self.items.value().clone();
        let item_lists = self.items.lists();
        self.items.form_mut().save_ok(items, &*item_lists);
        self.nav = super::Nav::default();
        // The draft's files are on the bill now (or wait for Try again).
        self.attach.draft = None;
        if let Some(e) = self
            .editing
            .as_mut()
            .filter(|e| e.id == s.record.record.txn.id)
        {
            e.old_date = s.record.record.txn.txn_date;
            e.record = s.record.clone();
        }
        match intent {
            SaveIntent::Save => {
                // Stay on the saved record with the engine's figures (due
                // date, balances).
                self.show_record(&s.record);
            }
            _ => self.follow_up(intent),
        }
        if let Some(older) = self.after_save_nav.take() {
            self.navigate(older);
        }
    }

    /// After a save (or with nothing to save): a new bill, close, or stay.
    fn follow_up(&mut self, intent: SaveIntent) {
        match intent {
            SaveIntent::SaveAndNew => self.new_bill(true),
            SaveIntent::SaveAndClose => {
                self.link
                    .push(CompanyEvent::CloseWindow(cl_model::CommandId::new(
                        super::KIND,
                    )));
            }
            SaveIntent::Save => {}
        }
    }

    /// A blank bill of the same kind. Save & New keeps the date; Clear
    /// starts from today.
    pub(crate) fn new_bill(&mut self, keep_date: bool) {
        let current = self.header.current();
        let kind = current.kind;
        let date = if keep_date {
            current.date.unwrap_or(self.today)
        } else {
            self.today
        };
        self.header.load(blank(kind, date));
        let (expenses, items) = self.blank_lines();
        self.load_lines(expenses, items);
        self.editing = None;
        self.nav.at = None;
        self.tab = super::Tab::Expenses;
        self.void = super::VoidPrompt::default();
        self.history = super::HistoryView::default();
        self.save.problems.clear();
        self.save.stale = false;
        self.save.refusal = None;
        self.nav_prompt = None;
        self.prefill = super::prefill::PrefillState::default();
        self.attach.reset();
        self.read = super::doc_read::ReadState::default();
        self.items.set_default_account(None);
        self.apply_locks();
        self.focus = Some(Field::Vendor);
        self.sync_inputs();
    }

    /// Clear: throw the unsaved edits away and start a new bill.
    pub(crate) fn clear(&mut self) {
        if self.save.task.is_some() {
            return;
        }
        self.save.error = None;
        self.save.notice = None;
        self.save.last_action = Some("clear");
        self.new_bill(false);
    }

    pub(crate) fn save_failed(&mut self, e: ApiError) {
        let intent = self.save.intent.take().unwrap_or(SaveIntent::Save);
        if !crate::screens::register::keeps_key(&e) {
            self.save.key = None;
        }
        let failure = SaveFailure {
            message: cl_core::OwnerFacing::owner_message(&e),
            detail: cl_core::OwnerFacing::scrubbed_detail(&e),
        };
        // The PR-1 rule: an unknown outcome blocks the form (and every
        // window of it) until Check again or the owner answers.
        self.save.lost.bind(
            &self.link,
            crate::forms::lost_save::LostScope::Form(super::LOST_SCOPE),
            &mut self.header,
        );
        if self.save.lost.lost(&e) {
            self.header.save_unknown(failure.clone());
            self.expenses.form_mut().save_failed(failure.clone());
            self.items.form_mut().save_failed(failure);
            return;
        }
        self.header.save_failed(failure.clone());
        self.expenses.form_mut().save_failed(failure.clone());
        self.items.form_mut().save_failed(failure);
        if self.closing_failed(&e, Pending::Save(intent)) {
            return;
        }
        match &e {
            ApiError::Conflict {
                reason: ConflictReason::Stale,
                ..
            } => self.save.stale = true,
            ApiError::Local(LocalValidation::Unsupported) => {
                // Nothing sent: a field this engine can't store yet.
                self.save.notice = Some("bill.needs_update");
            }
            _ => match ap_refusal(&e) {
                Some(ApRefusal::Stale) => self.save.stale = true,
                Some(r) => self.save.refusal = refusal_key(r),
                None => self.save.error = Some(e),
            },
        }
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
                let lists = Arc::clone(&self.lists);
                self.header.revert_form();
                self.expenses.form_mut().revert_form(&*lists);
                let item_lists = self.items.lists();
                self.items.form_mut().revert_form(&*item_lists);
                self.sync_inputs();
                self.navigate(older);
            }
            crate::windows::PromptAnswer::Save => {
                self.after_save_nav = Some(older);
                let done = self.start_save(SaveIntent::Save, None);
                if !done && self.save.task.is_none() {
                    self.after_save_nav = None;
                }
            }
        }
    }

    /// The closing-date dialog's Change: the pending change with the
    /// password.
    pub(crate) fn submit_closing(&mut self) {
        let Some((pending, secret)) = self.closing_prompt.submit() else {
            return;
        };
        let sent = match pending {
            Pending::Save(intent) => {
                self.start_save(intent, Some(secret));
                self.save.task.is_some()
                    || self
                        .draft
                        .as_ref()
                        .is_some_and(|s| s.locked() || s.closed())
                    || !self.closing_prompt.dialog_open()
            }
            Pending::Void(reason) => {
                self.send_void(&reason, false, Some(secret));
                true
            }
            Pending::Delete(reason) => {
                self.send_void(&reason, true, Some(secret));
                true
            }
        };
        if !sent {
            self.closing_prompt.nothing_sent();
        }
    }

    /// The recheck or the owner answered an unknown save (PR-1).
    pub(crate) fn lost_event(&mut self, ev: crate::forms::lost_save::LostEvent) {
        use crate::forms::lost_save::LostEvent;
        if let LostEvent::Saved(found) = ev {
            let vendor = self.vendor();
            self.publish(vendor);
            self.save.key = None;
            self.save.count += 1;
            self.save.last_action = Some("save");
            let lists = Arc::clone(&self.lists);
            let header = self.header.current().clone();
            self.header.load(header);
            let lines = self.expenses.value().clone();
            self.expenses.form_mut().load(lines, &*lists);
            let items = self.items.value().clone();
            self.items.load(items);
            if let Some(id) = found {
                // Before `bill-lines-v1` the files move in a second call
                // after the create, which the lost reply may have cut off:
                // offer Try again (the move is safe to repeat).
                let files: Vec<AttachmentId> = self
                    .compose()
                    .attachments
                    .iter()
                    .filter_map(|a| AttachmentId::parse(&a.id).ok())
                    .collect();
                if let (Some(draft), false) = (self.attach.draft, self.caps().lines)
                    && !files.is_empty()
                {
                    self.save.attach_failed = Some(AttachRetry {
                        id,
                        kind: self.kind(),
                        draft,
                        files,
                    });
                }
                self.attach.draft = None;
                self.nav = super::Nav::default();
                self.read_bill(id, Some(self.kind()));
                self.retry_attach();
            }
        }
    }

    // --------------------------------------------------------------------
    // Editing a saved bill: Prev/Next, loading, Void, Delete, History.

    /// Prev (older) or Next (newer) over the bills (or credits), by date.
    pub(crate) fn navigate(&mut self, older: bool) {
        if self.save.task.is_some() || self.load_task.is_some() {
            return;
        }
        if self.is_dirty() {
            self.nav_prompt = Some(older);
            return;
        }
        let kind = self.kind();
        if self.nav.kind != Some(kind) {
            self.nav = super::Nav {
                kind: Some(kind),
                ..super::Nav::default()
            };
        }
        if self.nav.list.is_empty() && !self.nav.done {
            self.nav.want_older = Some(older);
            self.fetch_list();
            return;
        }
        self.step(older);
    }

    fn step(&mut self, older: bool) {
        let target = match (self.nav.at, older) {
            (None, true) => Some(0),
            (None, false) => return,
            (Some(i), true) => Some(i + 1),
            (Some(0), false) => None,
            (Some(i), false) => Some(i - 1),
        };
        match target {
            None => self.new_bill(true),
            Some(i) if i < self.nav.list.len() => {
                self.nav.at = Some(i);
                let id = self.nav.list[i].0;
                self.read_bill(id, self.nav.kind);
            }
            Some(_) if !self.nav.done => {
                self.nav.want_older = Some(older);
                self.fetch_list();
            }
            Some(_) => self.save.notice = Some("bill.nav.oldest"),
        }
    }

    fn fetch_list(&mut self) {
        let credit = self.nav.kind == Some(BillKind::Credit);
        let link = Arc::clone(&self.link);
        if let Some(mut demo) = link.demo_state() {
            let all = crate::company::demo::bills::list(&mut demo, credit);
            drop(demo);
            let rows: Vec<_> = all
                .into_iter()
                .skip(self.nav.list.len())
                .take(usize::from(NAV_PAGE))
                .collect();
            self.nav.task = Some(Task::ready(rows));
            self.poll_save();
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        let offset = self.nav.list.len() as u64;
        if let Ok(p) = group.read(&session, move |s| async move {
            let page = PageRequest::new(NAV_PAGE, offset)?;
            Ok(if credit {
                let q = CreditListQuery {
                    sort: Some((CreditSort::Date, SortDir::Desc)),
                    ..CreditListQuery::default()
                };
                s.vendor_credits()
                    .list(&q, page)
                    .await?
                    .data
                    .into_iter()
                    .map(|c| (c.id, c.txn_date))
                    .collect()
            } else {
                let q = BillListQuery {
                    sort: Some((BillSort::Date, SortDir::Desc)),
                    ..BillListQuery::default()
                };
                s.bills()
                    .list(&q, page)
                    .await?
                    .data
                    .into_iter()
                    .map(|b| (b.id, b.txn_date))
                    .collect()
            })
        }) {
            self.nav.task = Some(Task::Pending(p));
        }
    }

    /// Reads a saved bill or credit into the form (`kind` unknown: a bill,
    /// else a credit).
    pub(crate) fn read_bill(&mut self, id: TransactionId, kind: Option<BillKind>) {
        let link = Arc::clone(&self.link);
        if let Some(mut demo) = link.demo_state() {
            let got = crate::company::demo::bills::get(&mut demo, id);
            drop(demo);
            match got {
                Some(v) => self.show_record(&v),
                None => self.save.error = Some(ApiError::NotFound { code: None }),
            }
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        if let Ok(p) = group.read(&session, move |s| async move {
            match kind {
                Some(BillKind::Bill) => s.bills().get(id).await,
                Some(BillKind::Credit) => s.vendor_credits().get(id).await,
                None => match s.bills().get(id).await {
                    Ok(v) => Ok(v),
                    Err(_) => s.vendor_credits().get(id).await,
                },
            }
        }) {
            self.load_task = Some(Task::Pending(p));
        }
    }

    pub(crate) fn show_record(&mut self, v: &Versioned<BillRecord>) {
        let r = &v.record;
        let caps = self.caps();
        let mut ek = std::mem::take(&mut self.expenses.form_mut().adapter_mut().keys);
        let (form, expenses, items) = form_of(r, &mut ek, self.items.keys());
        self.expenses.form_mut().adapter_mut().keys = ek;
        let credit = form.kind == BillKind::Credit;
        let locked = is_locked(r.txn.txn_date, self.closing);
        let line_detail = r.lines.iter().any(|l| {
            l.item.is_some() || l.quantity.is_some() || l.customer.is_some() || l.billable
        });
        let read_only_note = if r.txn.status == TxnStatus::Void {
            Some("bill.read_only.void")
        } else if credit && !self.ap_edit() {
            Some("bill.read_only.credit")
        } else if locked && !caps.closing {
            Some("bill.read_only.closed")
        } else if line_detail && !caps.lines {
            Some("bill.read_only.lines")
        } else {
            None
        };
        self.header.load(form);
        self.load_lines(expenses, items);
        if self.items.value().used().next().is_some() && caps.lines {
            self.tab = Tab::Items;
        } else {
            self.tab = Tab::Expenses;
        }
        self.editing = Some(super::Editing {
            id: r.txn.id,
            kind: if credit {
                BillKind::Credit
            } else {
                BillKind::Bill
            },
            record: v.clone(),
            old_date: r.txn.txn_date,
            has_payments: r.has_payments(),
            read_only: read_only_note.is_some(),
            read_only_note: read_only_note.unwrap_or("bill.read_only.void"),
        });
        self.save.problems.clear();
        self.save.error = None;
        self.save.refusal = None;
        self.save.notice = None;
        self.save.stale = false;
        self.save.last_action = Some("load");
        self.history = super::HistoryView::default();
        self.void = super::VoidPrompt::default();
        self.prefill = super::prefill::PrefillState::default();
        self.attach.reset();
        self.read = super::doc_read::ReadState::default();
        self.items
            .set_default_account(self.vendor_default_account());
        self.apply_locks();
        self.load_attachments();
        self.sync_inputs();
    }

    /// The edited bill was changed elsewhere: read it again.
    pub(crate) fn reload(&mut self) {
        if let Some((id, kind)) = self.editing.as_ref().map(|e| (e.id, e.kind)) {
            self.save.stale = false;
            self.read_bill(id, Some(kind));
        }
    }

    /// The saved bill is dated on or before the closing date.
    pub(crate) fn editing_locked(&self) -> bool {
        self.editing
            .as_ref()
            .is_some_and(|e| is_locked(e.old_date, self.closing))
    }

    /// Edit > Void (or Delete with `ap-edit-v1`): only for a saved bill
    /// with nothing applied.
    pub(crate) fn start_void(&mut self, delete: bool) -> bool {
        match &self.editing {
            Some(e) if e.record.record.txn.status == TxnStatus::Void => {
                self.save.notice = Some("bill.void.already");
            }
            Some(_) if delete && !self.ap_edit() => {
                self.save.notice = Some("bill.delete.needs_update");
            }
            Some(e) if e.has_payments => {
                self.save.refusal = Some(if e.kind == BillKind::Credit {
                    "bill.refused.credit_applied"
                } else {
                    "bill.refused.has_payments"
                });
            }
            Some(_) => {
                self.void.open = true;
                self.void.delete = delete;
                self.void.reason.clear();
            }
            None => self.save.notice = Some("bill.void.saved_only"),
        }
        true
    }

    /// Void or Delete with the typed reason (and the closing-date password
    /// when the bill is in the closed period).
    pub(crate) fn send_void(&mut self, reason: &str, delete: bool, password: Option<Secret>) {
        let Some(e) = self.editing.clone() else {
            return;
        };
        let reason = reason.trim().to_string();
        if reason.is_empty() {
            self.save.notice = Some("bill.void.reason_needed");
            return;
        }
        self.void.reason.clone_from(&reason);
        self.void.delete = delete;
        let pending = if delete {
            Pending::Delete(reason.clone())
        } else {
            Pending::Void(reason.clone())
        };
        if password.is_none() && is_locked(e.old_date, self.closing) {
            if self.caps().closing {
                self.closing_prompt.ask(pending, true, self.closing);
                self.closing_prompt
                    .set_verb(if delete { Verb::Delete } else { Verb::Void });
            } else {
                // The bill routes take no closing password on this engine:
                // nothing is sent (ap-closing-v1).
                self.save.notice = Some("bill.void.closed_needs_update");
                self.void = super::VoidPrompt::default();
            }
            return;
        }
        let link = Arc::clone(&self.link);
        if let Some(mut demo) = link.demo_state() {
            let r = if delete {
                crate::company::demo::bills::delete(&mut demo, e.id, password.as_ref())
            } else {
                crate::company::demo::bills::void(&mut demo, e.id, password.as_ref())
            };
            drop(demo);
            match r {
                Ok(()) => {
                    self.void.task = Some(Task::ready(()));
                    self.poll_save();
                }
                Err(err) => self.write_failed(err, pending),
            }
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        let identity = super::VoidIdentity {
            id: e.id,
            delete,
            reason: reason.clone(),
            password: password.is_some(),
        };
        let key = match &self.void.key {
            Some((same, k)) if *same == identity => k.clone(),
            _ => {
                let k = IdempotencyKey::new();
                self.void.key = Some((identity, k.clone()));
                k
            }
        };
        let label = OrphanLabel {
            what: if delete {
                "orphan.bill_delete"
            } else {
                "orphan.bill_void"
            },
            key: Some(key.clone()),
        };
        let opts = ApWriteOptions {
            posting: options(password),
            expected_updated_at: e.record.record.txn.updated_at,
        };
        let credit = e.kind == BillKind::Credit;
        if let Ok(p) = group.write(&session, label, move |s| async move {
            match (credit, delete) {
                (false, false) => s.bills().void(e.id, &reason, &opts, key).await,
                (true, false) => s.vendor_credits().void(e.id, &reason, &opts, key).await,
                (false, true) => s.bills().delete(e.id, Some(&reason), &opts, key).await,
                (true, true) => {
                    s.vendor_credits()
                        .delete(e.id, Some(&reason), &opts, key)
                        .await
                }
            }
        }) {
            self.void.task = Some(Task::Pending(p));
        }
    }

    fn voided(&mut self) {
        let vendor = self.vendor();
        self.publish(vendor);
        self.closing_prompt.saved();
        self.save.last_action = Some(if self.void.delete { "delete" } else { "void" });
        self.void = super::VoidPrompt::default();
        self.nav = super::Nav::default();
        self.new_bill(true);
    }

    /// The bill is saved but its files are not on it: try moving them again.
    pub(crate) fn retry_attach(&mut self) {
        let Some(r) = self.save.attach_failed.clone() else {
            return;
        };
        if self.save.attach_retry.is_some() {
            return;
        }
        let link = Arc::clone(&self.link);
        if let Some(mut demo) = link.demo_state() {
            let res = crate::company::demo::bills::attach_draft(
                &mut demo,
                r.id,
                r.draft,
                r.kind == BillKind::Credit,
            )
            .map(|_| ());
            drop(demo);
            if res.is_ok() {
                self.save.attach_retry = Some(Task::ready(()));
                self.poll_save();
            } else {
                self.save.notice = Some("bill.attach.still_failed");
            }
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        let label = OrphanLabel {
            what: "orphan.bill_attach",
            key: None,
        };
        if let Ok(p) = group.write(&session, label, move |s| async move {
            move_files(&s, r.id, r.kind == BillKind::Credit, r.draft, &r.files).await
        }) {
            self.save.attach_retry = Some(Task::Pending(p));
        }
    }

    /// History of the saved bill (when it was created, changed, voided).
    pub(crate) fn toggle_history(&mut self) {
        if self.history.open {
            self.history = super::HistoryView::default();
            return;
        }
        let Some(id) = self.editing.as_ref().map(|e| e.id) else {
            self.save.notice = Some("bill.history.saved_only");
            return;
        };
        self.history.open = true;
        let link = Arc::clone(&self.link);
        if let Some(demo) = link.demo_state() {
            let rows = crate::company::demo::bills::history(&demo, id)
                .into_iter()
                .map(|a| crate::i18n::t(action_key_of(a)))
                .collect();
            drop(demo);
            self.history.task = Some(Task::ready(rows));
            self.poll_save();
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        let uuid = *id.as_uuid();
        if let Ok(p) = group.read(&session, move |s| async move {
            let rows = s.audit().for_record(AuditEntity::Transaction, uuid).await?;
            Ok(rows
                .into_iter()
                .map(|r| {
                    let day = CalendarDate::from_naive(r.at.as_utc().date_naive())
                        .map(crate::shell::us_date)
                        .unwrap_or_default();
                    format!("{day} \u{b7} {}", crate::i18n::t(action_key(&r.action)))
                })
                .collect())
        }) {
            self.history.task = Some(Task::Pending(p));
        }
    }
}

/// Moves a saved record's draft files onto it: a bill's through
/// `attach-draft`, a credit's one by one through `link` (before
/// `bill-lines-v1`).
async fn move_files(
    s: &cl_api::Session,
    id: TransactionId,
    credit: bool,
    draft: DraftKey,
    files: &[AttachmentId],
) -> Result<(), ApiError> {
    if credit {
        for f in files {
            s.attachments()
                .link(*f, AttachTo::VendorCredit(id), IdempotencyKey::new())
                .await?;
        }
    } else {
        s.bills()
            .attach_draft(id, draft, IdempotencyKey::new())
            .await?;
    }
    Ok(())
}

fn action_key(a: &AuditAction) -> &'static str {
    match a {
        AuditAction::Create => "bill.history.create",
        AuditAction::Update => "bill.history.update",
        AuditAction::Void => "bill.history.void",
        AuditAction::Delete => "bill.history.delete",
        AuditAction::Override => "bill.history.override",
        AuditAction::Other(_) => "bill.history.other",
    }
}

fn action_key_of(a: &str) -> &'static str {
    match a {
        "create" => "bill.history.create",
        "update" => "bill.history.update",
        "void" => "bill.history.void",
        "delete" => "bill.history.delete",
        _ => "bill.history.other",
    }
}
