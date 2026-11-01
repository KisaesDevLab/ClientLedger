//! Saving, voiding and moving between saved invoices.
//!
//! The gate is `cl_model::forms::invoice::check_invoice`; the checked
//! request maps field by field onto the engine's invoice
//! ([`super::engine::new_invoice`]); `cl_api` checks again before anything
//! is sent. An edit sends the record's `updatedAt` (409 STALE → Reload) and
//! resends each line's stored rate (a line without one is re-priced by the
//! engine on PUT, E2 note 1); only a rate the owner cleared goes blank.

use std::sync::Arc;

use cl_api::{
    ApiError, ClosedPeriodAuthorization, ConflictReason, IdempotencyKey, InvoiceListQuery,
    InvoiceRecord, InvoiceRefusal, InvoiceWriteOptions, PageRequest, PostingOptions, Versioned,
    invoice_refusal,
};
use cl_core::ids::TransactionId;
use cl_core::{Quantity, Secret};
use cl_model::CommandId;
use cl_model::closing::{is_locked, needs_password};
use cl_model::forms::SaveIntent;
use cl_model::forms::invoice::{
    BillableRef, InvoiceField, InvoiceForm, InvoiceLine, InvoiceProblem,
};

use super::engine::{self, label};
use super::{Editing, InvoiceScreen, KIND, Pending, field};
use crate::company::CompanyEvent;
use crate::forms::invoice_lines_grid::{V_DESC, V_ITEM, V_RATE};

fn options(password: Option<Secret>) -> PostingOptions {
    PostingOptions {
        closed_period: password.map(ClosedPeriodAuthorization::with_password),
    }
}

/// The saved record as the form: every line keeps its stored rate (and a
/// percent discount its percent); subtotal and discount lines keep no
/// quantity; a billable line is its source, income account and markup,
/// with the engine's quantity and display rate shown only (U-128).
pub(crate) fn form_of(
    r: &InvoiceRecord,
    keys: &mut cl_grid::RowKeyGen,
) -> (InvoiceForm, Vec<InvoiceLine>) {
    let pct =
        |p: Option<cl_api::SignedPercent>| p.and_then(|p| Quantity::parse_wire(&p.to_wire()).ok());
    let lines = r
        .lines
        .iter()
        .map(|l| match l.billable_source_line_id {
            Some(s) => InvoiceLine {
                account: Some(l.account),
                description: l.description.clone().unwrap_or_default(),
                service_date: l.service_date,
                quantity: l.quantity,
                rate: l.display_rate,
                taxable: Some(l.is_taxable),
                tag: l.tag,
                billable: Some(BillableRef {
                    source_line: s,
                    markup_percent: pct(l.markup_percent).filter(|m| !m.is_zero()),
                }),
                ..InvoiceLine::blank(keys.next_key())
            },
            None => InvoiceLine {
                item: l.item,
                account: if l.item.is_some() {
                    None
                } else {
                    Some(l.account)
                },
                description: l.description.clone().unwrap_or_default(),
                service_date: l.service_date,
                quantity: l.quantity,
                rate: l.unit_price,
                discount_percent: pct(l.discount_percent),
                taxable: Some(l.is_taxable),
                tag: l.tag,
                ..InvoiceLine::blank(keys.next_key())
            },
        })
        .collect();
    let custom = r.payment_terms.as_deref() == Some(cl_model::forms::invoice::CUSTOM_TERMS);
    let form = InvoiceForm {
        client: r.txn.contact_id,
        date: Some(r.txn.txn_date),
        number: r.txn.txn_number.clone().unwrap_or_default(),
        po_number: r.po_number.clone().unwrap_or_default(),
        terms: r.payment_terms.clone(),
        terms_days: if custom {
            r.terms_days.and_then(|d| u16::try_from(d).ok())
        } else {
            None
        },
        due_date: if r.due_date_overridden {
            r.due_date
        } else {
            None
        },
        memo: r.txn.memo.clone().unwrap_or_default(),
        customer_message: r.customer_message.clone().unwrap_or_default(),
        header_tags: r.tag_ids.clone(),
        hide_zero_lines: r.hide_zero_lines,
        print_later: r.print_later,
        layout: r
            .layout_id
            .map_or(cl_model::forms::invoice::LayoutPick::CompanyDefault, |id| {
                cl_model::forms::invoice::LayoutPick::Layout(id.to_string())
            }),
        lines: Vec::new(),
        ..InvoiceForm::default()
    };
    (form, lines)
}

impl InvoiceScreen {
    /// Commits the dates still being typed. False when a typed date is not
    /// a date.
    fn commit_header(&mut self) -> bool {
        let today = self.today;
        let shown = |d: Option<cl_core::CalendarDate>| {
            d.map(|d| crate::screens::company::form::DateInput::new(d).text)
                .unwrap_or_default()
        };
        if self.inputs.date.text.trim() != shown(self.header.current().date) {
            self.inputs.date.reparse(today);
            let d = self.inputs.date.value;
            if d.is_none() {
                return false;
            }
            self.header.edit(field::DATE, |h| h.date = d);
        }
        if self.inputs.due.text.trim() != shown(self.header.current().due_date) {
            self.inputs.due.reparse(today);
            let d = self.inputs.due.value;
            if d.is_none() && !self.inputs.due.text.trim().is_empty() {
                return false;
            }
            self.header.edit(field::DUE, |h| h.due_date = d);
        }
        self.inputs.rev = self.header.revision();
        true
    }

    /// Checks the form and starts the save. Nothing is sent when the check
    /// doesn't pass. True when it had nothing to save.
    #[allow(clippy::needless_pass_by_value)] // the secret moves into the queued write
    pub(crate) fn start_save(&mut self, intent: SaveIntent, password: Option<Secret>) -> bool {
        if self.busy() || (self.closing_prompt.dialog_open() && password.is_none()) {
            return false;
        }
        if self.header.is_unconfirmed() {
            return false;
        }
        if let Some(note) = self.read_only() {
            self.notice = Some(note);
            return false;
        }
        if !self.commit_header() || !self.grid.commit() {
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
        let Some(req) = check_or_refuse(self) else {
            return false;
        };
        let mut new = match engine::new_invoice(&req) {
            Ok(n) => n,
            Err(e) => {
                self.error = Some(e);
                return false;
            }
        };
        // A typed number already in use saves with a warning (Q-94).
        new.warn_only = new.txn_number.is_some();
        if let Some(e) = &self.editing {
            new.release_billable_source_line_ids = released(&e.record, &req);
        }
        let closing = self.books.closing;
        let old = self.editing.as_ref().map(|e| e.old_date);
        if password.is_none() && needs_password(old, req.date, closing) {
            let supported = self.closing_supported();
            if supported {
                self.closing_prompt
                    .ask(Pending::Save(intent), supported, closing);
            } else {
                self.closing_prompt.unavailable(supported, closing);
            }
            return false;
        }
        let _ = self.grid.begin_save(intent);
        let _ = self.header.begin_save(intent);
        self.intent = Some(intent);
        let edit = self.editing.as_ref().map(|e| (e.id, e.record.clone()));
        let identity = format!(
            "{}|{:?}|{}",
            serde_json::to_string(&new).unwrap_or_default(),
            edit.as_ref()
                .map(|(id, r)| (id.to_string(), r.txn.updated_at)),
            password.is_some()
        );
        let key = self.key_for(identity);
        let pw = password.clone();
        let new2 = new.clone();
        self.save_job = Some(engine::write(
            &self.link,
            self.group.as_ref(),
            label("orphan.invoice_save", Some(key.clone())),
            |d| {
                let duplicate = crate::company::demo::invoices::number_in_use(
                    d,
                    new.txn_number.as_deref(),
                    edit.as_ref().map(|(id, _)| *id),
                );
                match &edit {
                    None => crate::company::demo::invoices::create(d, &new, password.as_ref()),
                    Some((id, r)) => crate::company::demo::invoices::update(
                        d,
                        *id,
                        r.txn.updated_at,
                        &new,
                        password.as_ref(),
                    ),
                }
                .map(|v| (v, duplicate))
            },
            {
                let edit = self.editing.as_ref().map(|e| (e.id, e.record.clone()));
                move |s| async move {
                    let opts = options(pw);
                    let saved = match edit {
                        None => s.invoices().create(&new2, &opts, key).await?,
                        Some((id, r)) => s.invoices().update(id, &r, &new2, &opts, key).await?,
                    };
                    Ok((saved.invoice, !saved.warnings.is_empty()))
                }
            },
        ));
        if self.link.is_demo() {
            self.poll();
        }
        false
    }

    /// The key for this save: the stored one when the same save goes again
    /// after an unknown outcome, a fresh one otherwise.
    fn key_for(&mut self, identity: String) -> IdempotencyKey {
        match &self.key {
            Some((d, k)) if *d == identity => k.clone(),
            _ => {
                let k = IdempotencyKey::new();
                self.key = Some((identity, k.clone()));
                k
            }
        }
    }

    /// The problems of a refused save; the first one's field gets the
    /// cursor.
    pub(crate) fn refuse(&mut self, problems: Vec<InvoiceProblem>) {
        if let Some(first) = problems.first() {
            match first.field() {
                InvoiceField::LineItem(k) => self.grid.focus(k, V_ITEM),
                InvoiceField::LineDescription(k) => self.grid.focus(k, V_DESC),
                InvoiceField::LineRate(k) | InvoiceField::LineMarkup(k) => {
                    self.grid.focus(k, V_RATE);
                }
                _ => {}
            }
        }
        self.problems = problems;
    }

    pub(crate) fn lost_event(&mut self, ev: crate::forms::lost_save::LostEvent) {
        use crate::forms::lost_save::LostEvent;
        if let LostEvent::Saved(found) = ev {
            crate::forms::publish_saved(&self.link, &self.bus, &[]);
            self.key = None;
            self.saved_count += 1;
            self.last_action = Some("save");
            let header = self.header.current().clone();
            self.header.load(header);
            if let Some(id) = found {
                self.last_saved = Some(id);
                self.listed = false;
                self.load(id);
            }
        }
    }

    pub(crate) fn saved(&mut self, v: &Versioned<InvoiceRecord>) {
        if std::mem::take(&mut self.print_after_save) {
            // Print (S6): only the print-later mark changed; no "send the
            // updated copy?" banner, and Print Forms opens once it is saved.
            self.intent = None;
            self.key = None;
            self.lost.saved();
            self.closing_prompt.saved();
            crate::forms::publish_saved(&self.link, &self.bus, &[]);
            if self.header.is_saving() {
                let h = self.header.current().clone();
                self.header.save_ok(h);
            }
            self.show(v);
            self.last_action = Some("print_queued");
            self.link.push(CompanyEvent::OpenWindow(CommandId::new(
                crate::screens::print_forms::KIND,
            )));
            return;
        }
        self.lost.saved();
        crate::forms::publish_saved(&self.link, &self.bus, &[]);
        self.closing_prompt.saved();
        let intent = self.intent.take().unwrap_or(SaveIntent::Save);
        let was_sent = self.editing.as_ref().is_some_and(|e| e.record.was_sent());
        self.saved_count += 1;
        self.last_action = Some("save");
        self.last_saved = Some(v.record.txn.id);
        self.key = None;
        self.problems.clear();
        if self.header.is_saving() {
            let h = self.header.current().clone();
            self.header.save_ok(h);
        }
        self.listed = false;
        self.show(v);
        self.last_action = Some("save");
        if self.email_after_save {
            // Email a PDF after saving: the dialog first, then what the
            // save asked for.
            self.open_email(Some(intent));
            return;
        }
        if was_sent {
            if intent == SaveIntent::Save {
                self.offer_send = true;
            } else {
                self.link
                    .push(CompanyEvent::Toast("invoice.sent.update_reminder"));
            }
        }
        self.follow_up(intent);
    }

    pub(crate) fn save_failed(&mut self, e: ApiError) {
        let intent = self.intent.take().unwrap_or(SaveIntent::Save);
        // 409 RETRY: the same save goes again with the same key.
        let retry = invoice_refusal(&e) == Some(InvoiceRefusal::Retry);
        if !crate::screens::register::keeps_key(&e) && !retry {
            self.key = None;
        }
        self.print_after_save = false;
        let failure = cl_model::forms::SaveFailure {
            message: cl_core::OwnerFacing::owner_message(&e),
            detail: cl_core::OwnerFacing::scrubbed_detail(&e),
        };
        if self.lost.lost(&e) {
            self.header.save_unknown(failure.clone());
            self.grid.save_failed(failure);
            return;
        }
        self.header.save_failed(failure.clone());
        self.grid.save_failed(failure);
        if self.closing_failed(&e, Pending::Save(intent)) {
            return;
        }
        self.engine_refusal(e);
    }

    /// Applies held deposit `source` to the open invoice, dated today; on
    /// or before the closing date the password is asked first.
    pub(crate) fn apply_deposit(&mut self, source: TransactionId, password: Option<Secret>) {
        let Some(invoice) = self.editing.as_ref().filter(|e| !e.void()).map(|e| e.id) else {
            return;
        };
        let closing = self.books.closing;
        if password.is_none() && needs_password(None, self.today, closing) {
            let supported = self.closing_supported();
            let pending = Pending::ApplyDeposit(source);
            let verb = pending.verb();
            if supported {
                self.closing_prompt.ask(pending, supported, closing);
                self.closing_prompt.set_verb(verb);
            } else {
                self.closing_prompt.unavailable(supported, closing);
            }
            return;
        }
        let today = self.today;
        self.deposit_card.send(
            &self.link,
            self.group.as_ref(),
            source,
            invoice,
            today,
            password,
        );
    }

    pub(crate) fn closing_failed(&mut self, e: &ApiError, pending: Pending) -> bool {
        let supported = self.closing_supported();
        let closing = self.books.closing;
        let verb = pending.verb();
        let took = self.closing_prompt.failed(e, pending, supported, closing);
        if took {
            self.closing_prompt.set_verb(verb);
        }
        took
    }

    /// An engine refusal in plain words.
    fn engine_refusal(&mut self, e: ApiError) {
        if matches!(
            e,
            ApiError::Conflict {
                reason: ConflictReason::Stale,
                ..
            }
        ) {
            self.stale = true;
            return;
        }
        let notice = match invoice_refusal(&e) {
            Some(InvoiceRefusal::Stale) => {
                self.stale = true;
                return;
            }
            Some(InvoiceRefusal::HasPayments) => "invoice.has_payments",
            Some(InvoiceRefusal::Void) => "invoice.read_only.void",
            Some(
                r @ (InvoiceRefusal::BillableAlreadyBilled | InvoiceRefusal::BillableUnavailable),
            ) => {
                if let Some(c) = self.header.current().client {
                    let link = Arc::clone(&self.link);
                    self.billable.fetch(&link, self.group.as_ref(), c);
                }
                if r == InvoiceRefusal::BillableAlreadyBilled {
                    "invoice.billable.already_billed"
                } else {
                    "invoice.billable.unavailable_cost"
                }
            }
            Some(InvoiceRefusal::BillableLinesMissing) => "invoice.billable.lines_missing",
            Some(InvoiceRefusal::LinesMissing) => "invoice.lines_missing",
            Some(InvoiceRefusal::BillableClientMismatch) => "invoice.billable.client_mismatch",
            Some(InvoiceRefusal::BillableDuplicate) => "invoice.billable.duplicate",
            Some(
                InvoiceRefusal::BillableFieldsReadOnly | InvoiceRefusal::BillableAccountRequired,
            ) => "invoice.billable.account_required",
            Some(InvoiceRefusal::MarkupAccountRequired) => {
                "invoice.billable.markup_account_required"
            }
            Some(InvoiceRefusal::MarkupAccountInvalid) => "invoice.billable.markup_account_invalid",
            Some(InvoiceRefusal::TotalBelowPaid) => "invoice.total_below_paid",
            Some(InvoiceRefusal::RecurringHasBillable) => "invoice.recurring_has_billable",
            Some(InvoiceRefusal::LayoutGone) => "invoice.layout.gone",
            Some(InvoiceRefusal::LayoutArchived) => "invoice.layout.archived",
            Some(InvoiceRefusal::Retry) => "invoice.retry",
            _ => {
                self.error = Some(e);
                return;
            }
        };
        self.notice = Some(notice);
    }

    pub(crate) fn follow_up(&mut self, intent: SaveIntent) {
        match intent {
            SaveIntent::SaveAndNew => self.new_form(true),
            SaveIntent::SaveAndClose => {
                self.link
                    .push(CompanyEvent::CloseWindow(CommandId::new(KIND)));
            }
            SaveIntent::Save => {}
        }
    }

    pub(crate) fn submit_closing(&mut self) {
        let Some((pending, secret)) = self.closing_prompt.submit() else {
            return;
        };
        match pending {
            Pending::Save(intent) => {
                self.start_save(intent, Some(secret));
                if self.save_job.is_none() && self.closing_prompt.dialog_open() {
                    self.closing_prompt.nothing_sent();
                }
            }
            Pending::Void(reason) => {
                self.void_reason = reason;
                self.send_void(Some(secret));
            }
            Pending::ApplyDeposit(source) => {
                self.apply_deposit(source, Some(secret));
                if !self.deposit_card.busy() && self.closing_prompt.dialog_open() {
                    self.closing_prompt.nothing_sent();
                }
            }
        }
    }

    /// Shows a saved invoice (Prev/Next, Find, after Save).
    pub(crate) fn show(&mut self, v: &Versioned<InvoiceRecord>) {
        let r = &v.record;
        let (form, lines) = form_of(r, self.grid.keys());
        self.header.load(form);
        self.grid.load(lines);
        self.editing = Some(Editing {
            id: r.txn.id,
            record: r.clone(),
            old_date: r.txn.txn_date,
        });
        self.focused = None;
        self.stale = false;
        self.key = None;
        self.problems.clear();
        self.error = None;
        self.void_open = false;
        self.offer_send = false;
        self.last_action = Some("load");
        self.sync_inputs();
        self.client_changed();
        self.list_attachments();
    }

    // ------------------------------------------------------------- void

    /// Edit > Void Transaction.
    pub(crate) fn start_void(&mut self) -> bool {
        match &self.editing {
            Some(e) if e.void() => self.notice = Some("invoice.void.already"),
            Some(e) if e.record.has_payments() => self.notice = Some("invoice.has_payments"),
            Some(_) => self.void_open = true,
            None => self.notice = Some("invoice.void.saved_only"),
        }
        true
    }

    #[allow(clippy::needless_pass_by_value)] // the secret moves into the queued write
    pub(crate) fn send_void(&mut self, password: Option<Secret>) {
        let Some(e) = self.editing.clone() else {
            self.notice = Some("invoice.void.saved_only");
            return;
        };
        let reason = self.void_reason.trim().to_string();
        if reason.is_empty() {
            self.notice = Some("invoice.void.reason_needed");
            return;
        }
        let pending = Pending::Void(reason.clone());
        let closing = self.books.closing;
        if password.is_none() && is_locked(e.old_date, closing) {
            let supported = self.closing_supported();
            if supported {
                self.closing_prompt.ask(pending.clone(), supported, closing);
                self.closing_prompt.set_verb(pending.verb());
            } else {
                self.closing_prompt.unavailable(supported, closing);
            }
            return;
        }
        let key = IdempotencyKey::new();
        let expected = e.record.txn.updated_at;
        let pw = password.clone();
        let job = engine::write(
            &self.link,
            self.group.as_ref(),
            label("orphan.invoice_void", Some(key.clone())),
            |d| crate::company::demo::invoices::void(d, e.id, password.as_ref()),
            move |s| async move {
                let opts = InvoiceWriteOptions {
                    posting: options(pw),
                    expected_updated_at: expected,
                };
                s.invoices().void(e.id, &reason, &opts, key).await
            },
        );
        self.void_job = Some((pending, job));
        if self.link.is_demo() {
            self.poll();
        }
    }

    pub(crate) fn voided(&mut self, v: &Versioned<InvoiceRecord>) {
        crate::forms::publish_saved(&self.link, &self.bus, &[]);
        self.closing_prompt.saved();
        self.void_reason.clear();
        self.listed = false;
        self.show(v);
        self.last_action = Some("void");
    }

    pub(crate) fn void_failed(&mut self, e: &ApiError, pending: Pending) {
        if self.closing_failed(e, pending) {
            return;
        }
        self.engine_refusal(e.clone());
    }

    // ------------------------------------------------------------ records

    /// Prev (`older`) or Next through the saved invoices, newest first.
    pub(crate) fn navigate(&mut self, older: bool) {
        if self.busy() || self.load_job.is_some() || self.nav_job.is_some() {
            return;
        }
        if self.is_dirty() {
            self.notice = Some("helper.unsaved");
            return;
        }
        if !self.listed {
            self.want_older = Some(older);
            self.fetch_list();
            return;
        }
        self.step(older);
    }

    pub(crate) fn step(&mut self, older: bool) {
        if self.nav_at.is_none()
            && let Some(e) = &self.editing
        {
            self.nav_at = self.nav.iter().position(|x| *x == e.id);
        }
        let target = match (self.nav_at, older) {
            (None, true) => Some(0),
            (None, false) => return,
            (Some(i), true) => Some(i + 1),
            (Some(0), false) => None,
            (Some(i), false) => Some(i - 1),
        };
        match target {
            None => self.new_form(true),
            Some(i) if i < self.nav.len() => {
                self.nav_at = Some(i);
                let id = self.nav[i];
                self.load(id);
            }
            Some(_) => self.notice = Some("invoice.nav.oldest"),
        }
    }

    fn fetch_list(&mut self) {
        self.nav_job = Some(engine::read(
            &self.link,
            self.group.as_ref(),
            |d| Ok(crate::company::demo::invoices::list(d)),
            |s| async move {
                let page = s
                    .invoices()
                    .list(&InvoiceListQuery::default(), PageRequest::new(200, 0)?)
                    .await?;
                Ok(page.data.into_iter().map(|r| r.id).collect())
            },
        ));
        if self.link.is_demo() {
            self.poll();
        }
    }

    pub(crate) fn load(&mut self, id: TransactionId) {
        self.load_job = Some(engine::read(
            &self.link,
            self.group.as_ref(),
            |d| crate::company::demo::invoices::get(d, id).ok_or(ApiError::NotFound { code: None }),
            move |s| async move { s.invoices().get(id).await },
        ));
        if self.link.is_demo() {
            self.poll();
        }
    }

    /// Reads the open invoice again (it changed elsewhere).
    pub(crate) fn reload(&mut self) {
        if let Some(id) = self.editing.as_ref().map(|e| e.id) {
            self.load(id);
        }
    }

    // ------------------------------------------------------------ commands

    /// Esc: an open card closes, then the focused field goes back to its
    /// saved value.
    pub(crate) fn revert_field(&mut self) -> bool {
        if self.email.is_open() {
            self.email.cancel();
            return true;
        }
        if self.billable.is_open() {
            self.billable.close();
            return true;
        }
        if self.void_open {
            self.void_open = false;
            return true;
        }
        let Some(f) = self.focused else { return false };
        let saved = self.header.original().clone();
        let now = self.header.current().clone();
        let differs = match f {
            field::DATE => now.date != saved.date,
            field::NUMBER => now.number != saved.number,
            field::PO => now.po_number != saved.po_number,
            field::DUE => now.due_date != saved.due_date,
            field::MEMO => now.memo != saved.memo,
            field::MESSAGE => now.customer_message != saved.customer_message,
            _ => false,
        };
        if differs {
            self.header.edit_step(|h| match f {
                field::DATE => h.date = saved.date,
                field::NUMBER => h.number.clone_from(&saved.number),
                field::PO => h.po_number.clone_from(&saved.po_number),
                field::DUE => h.due_date = saved.due_date,
                field::MEMO => h.memo.clone_from(&saved.memo),
                _ => h.customer_message.clone_from(&saved.customer_message),
            });
            self.sync_inputs();
            return true;
        }
        false
    }

    /// Keymap commands the grid and the closing dialog did not take.
    pub(crate) fn form_command(&mut self, id: CommandId) -> bool {
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
            "form.revert" => self.revert_field(),
            "form.prev_record" | "edit.find" => {
                self.navigate(true);
                true
            }
            "form.next_record" => {
                self.navigate(false);
                true
            }
            "edit.void_transaction" => self.start_void(),
            "edit.undo" => {
                if self.header.undo() {
                    self.sync_inputs();
                }
                true
            }
            "edit.redo" => {
                if self.header.redo() {
                    self.sync_inputs();
                }
                true
            }
            "invoice.add_billable" => {
                self.open_billable();
                true
            }
            "invoice.email" => {
                self.open_email(None);
                true
            }
            "file.print" | "form.print" => {
                self.print();
                true
            }
            _ => false,
        }
    }

    /// Print (Q-P20e): the saved invoice waits in File > Print Forms, where
    /// printing marks it sent ("Printed"). The engine has no route that
    /// only queues a form, so a not-yet-queued invoice is saved with Print
    /// later set — every line exactly as stored (B3), so nothing posts
    /// differently — and Print Forms opens only once that save succeeds
    /// (S6); a failed save says why and opens nothing.
    pub(crate) fn print(&mut self) {
        if self.editing.is_none() || self.is_dirty() {
            self.notice = Some("invoice.print.save_first");
            return;
        }
        if let Some(note) = self.read_only() {
            self.notice = Some(note);
            return;
        }
        if let Some(e) = &self.editing
            && !e.record.print_later
        {
            self.header.edit_step(|h| h.print_later = true);
            self.print_after_save = true;
            self.start_save(SaveIntent::Save, None);
            if self.save_job.is_none() && !self.closing_prompt.dialog_open() {
                self.print_after_save = false;
            }
            return;
        }
        self.link.push(CompanyEvent::OpenWindow(CommandId::new(
            crate::screens::print_forms::KIND,
        )));
    }
}

/// The billed costs an edit takes off the invoice (the record's minus the
/// form's): sent as `releaseBillableSourceLineIds` (U-128; leaving one out
/// is 409 `BILLABLE_LINES_MISSING`).
pub(crate) fn released(
    record: &InvoiceRecord,
    req: &cl_model::forms::invoice::InvoiceRequest,
) -> Vec<cl_core::ids::JournalLineId> {
    record
        .billable_sources()
        .into_iter()
        .filter(|s| !req.lines.iter().any(|l| l.billable_source_line == Some(*s)))
        .collect()
}

/// The check, or the problems shown (nothing sent).
fn check_or_refuse(s: &mut InvoiceScreen) -> Option<cl_model::forms::invoice::InvoiceRequest> {
    match s.compose().to_new_invoice(&s.context()) {
        Ok(r) => Some(r),
        Err(p) => {
            s.refuse(p);
            None
        }
    }
}
