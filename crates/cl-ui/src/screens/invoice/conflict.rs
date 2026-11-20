//! The sales form's side of the save-conflict dialog (P33 D4 step 11): an
//! invoice, credit memo or sales receipt, as Make Journal Entry's. A 409
//! `STALE` on an edit reads their document, saves again on it without
//! asking when nothing editable changed on their side, and opens
//! [`crate::screens::conflict_dialog`] otherwise.
//!
//! Lines are taken whole from one side; the totals and sales tax are the
//! engine's on the save, never added up here. A void or paid document is
//! not merged: Reload shows it. A saved credit memo's applications are not
//! part of an edit (Apply credit changes them), so they never merge.

use std::sync::Arc;

use cl_api::ApiError;
use cl_core::CalendarDate;
use cl_model::forms::SaveIntent;
use cl_model::forms::invoice::{InvoiceForm, InvoiceLine, LayoutPick};
use cl_model::lan::Conflict;
use serde_json::{Value, json};

use super::engine::{self, Job};
use super::{Editing, InvoiceScreen, SavedDoc};
use crate::forms::conflict::{GONE, OpenConflict, Read, Words, read_of, tag_ids};
use crate::screens::conflict_dialog::{ConflictChoice, fields_from_theirs};

/// One line as this form holds it (content only, never a row key).
fn line_value(l: &InvoiceLine) -> Value {
    json!({
        "item": l.item.map(|i| i.to_string()),
        "account": l.account.map(|a| a.to_string()),
        "description": l.description.trim(),
        "serviceDate": l.service_date.map(CalendarDate::to_wire),
        "qty": l.quantity.map(cl_core::Quantity::to_wire),
        "rate": l.rate.map(cl_core::Money::to_wire),
        "discountPercent": l.discount_percent.map(cl_core::Quantity::to_wire),
        "taxable": l.taxable,
        "tag": l.tag.map(|t| t.to_string()),
        "billable": l.billable.map(|b| json!({
            "source": b.source_line.to_string(),
            "markup": b.markup_percent.map(cl_core::Quantity::to_wire),
        })),
        "taxCode": l.tax_code.map(|c| c.to_string()),
        "unit": l.unit.map(|u| u.to_string()),
        "returnOf": l.return_of.map(|t| t.to_string()),
    })
}

/// The editable fields of a sales document as this form holds them.
#[must_use]
pub fn doc_value(f: &InvoiceForm, lines: &[InvoiceLine]) -> Value {
    json!({
        "client": f.client.map(|c| c.to_string()),
        "txnDate": f.date.map(CalendarDate::to_wire),
        "docNumber": f.number.trim(),
        "poNumber": f.po_number.trim(),
        "terms": f.terms,
        "termsDays": f.terms_days,
        "dueDate": f.due_date.map(CalendarDate::to_wire),
        "memo": f.memo.trim(),
        "customerMessage": f.customer_message.trim(),
        "headerTags": tag_ids(&f.header_tags),
        "hideZeroLines": f.hide_zero_lines,
        "printLater": f.print_later,
        "layout": match &f.layout {
            LayoutPick::Layout(id) => Some(id.clone()),
            _ => None,
        },
        "salesTax": f.sales_tax.map(|s| s.to_string()),
        "depositTo": f.deposit_to.map(|a| a.to_string()),
        "paymentMethod": f.payment_method,
        "refNo": f.ref_no.trim(),
        "lines": lines.iter().filter(|l| !l.is_blank()).map(line_value).collect::<Vec<_>>(),
    })
}

impl InvoiceScreen {
    /// A save came back stale: read their version (the dialog follows).
    pub(crate) fn conflict_start(&mut self, intent: SaveIntent) {
        let Some(id) = self.editing.as_ref().map(|e| e.id) else {
            self.stale = true;
            return;
        };
        let kind = self.kind;
        let job: Job<SavedDoc> = engine::read(
            &self.link,
            self.group.as_ref(),
            move |d| super::save::read_doc_demo(d, kind, id),
            move |s| async move { super::save::read_doc_live(&s, kind, id).await },
        );
        self.conflict_job = Some((job, intent));
        self.poll_conflict();
    }

    /// Polls the read of their version.
    pub(crate) fn poll_conflict(&mut self) {
        let Some((mut job, intent)) = self.conflict_job.take() else {
            return;
        };
        match job.take() {
            Some(r) => self.conflict_read(r, intent),
            None => self.conflict_job = Some((job, intent)),
        }
    }

    /// Their document as this form shows it; `None` when it can't be merged
    /// here (void, paid).
    fn theirs_of(&mut self, v: &SavedDoc) -> Option<(InvoiceForm, Vec<InvoiceLine>)> {
        let e = Editing::of(v.clone());
        let paid = e
            .invoice()
            .is_some_and(|r| r.invoice_status == Some(cl_api::InvoiceStatus::Paid));
        if e.void() || paid {
            return None;
        }
        Some(match v {
            SavedDoc::Invoice(r) => super::save::form_of(r, self.grid.keys()),
            SavedDoc::CreditMemo(r) => super::credit::memo_form_of(r, self.grid.keys()),
            SavedDoc::SalesReceipt(r) => super::credit::receipt_form_of(r, self.grid.keys()),
        })
    }

    pub(crate) fn conflict_read(&mut self, r: Result<SavedDoc, ApiError>, intent: SaveIntent) {
        let theirs = match read_of(r) {
            Read::Theirs(v) => v,
            Read::Gone => {
                self.notice = Some(GONE);
                return;
            }
            Read::Reload => {
                self.stale = true;
                return;
            }
        };
        let Some((their_form, their_lines)) = self.theirs_of(&theirs) else {
            self.stale = true;
            return;
        };
        let mine = (
            self.header.current().clone(),
            self.grid.value().lines.clone(),
        );
        let conflict = Conflict::new(
            &doc_value(self.header.original(), &self.grid.original().lines),
            &doc_value(&mine.0, &mine.1),
            &doc_value(&their_form, &their_lines),
            "lines",
        );
        if conflict.silent_resave() {
            self.adopt(theirs, (their_form, their_lines), mine);
            self.resave(intent);
            return;
        }
        let words = Words::new(
            Arc::clone(&self.books.lists),
            Some(Arc::new(crate::forms::item_lines_grid::ItemLists::new(
                Arc::clone(&self.books.lists),
                &self.items,
            ))),
            self.money_style,
            &[],
        );
        self.conflict.open = Some(OpenConflict {
            dialog: words.dialog(conflict),
            theirs,
            intent,
        });
    }

    /// Their document becomes the form's "as opened" and `mine` its fields,
    /// on their stamp.
    fn adopt(
        &mut self,
        theirs: SavedDoc,
        their: (InvoiceForm, Vec<InvoiceLine>),
        mine: (InvoiceForm, Vec<InvoiceLine>),
    ) {
        self.header.load(their.0);
        if *self.header.current() != mine.0 {
            self.header.edit_step(|h| *h = mine.0);
        }
        self.grid.load(their.1);
        if self.grid.value().lines != mine.1 {
            self.grid.edit(|v, _| v.lines = mine.1);
        }
        self.editing = Some(Editing::of(theirs));
        self.key = None;
        self.stale = false;
        self.sync_inputs();
    }

    /// Saves the merge; a credit memo waits for its preview, then saves.
    fn resave(&mut self, intent: SaveIntent) {
        self.start_save(intent, None);
        if self.notice == Some("credit_memo.problem.updating") {
            self.notice = None;
            self.conflict_resave = Some(intent);
        }
    }

    /// A merge waiting for the memo's preview.
    pub(crate) fn poll_resave(&mut self) {
        if self.preview_job.is_none()
            && let Some(intent) = self.conflict_resave.take()
        {
            self.resave(intent);
        }
    }

    /// Draws the open dialog and runs the owner's choice.
    pub(crate) fn conflict_ui(&mut self, ctx: &eframe::egui::Context) {
        if let Some((open, choice)) = self.conflict.ui(ctx) {
            self.run_choice(open, choice);
        }
    }

    #[allow(clippy::needless_pass_by_value)] // the dialog hands over its choice
    pub(crate) fn run_choice(&mut self, open: OpenConflict<SavedDoc>, choice: ConflictChoice) {
        match choice {
            ConflictChoice::Back => self.stale = true,
            ConflictChoice::UseTheirs => self.show(open.theirs),
            ConflictChoice::KeepMine(merge) => {
                let Some((tf, their_lines)) = self.theirs_of(&open.theirs) else {
                    self.stale = true;
                    return;
                };
                let mut mine = (
                    self.header.current().clone(),
                    self.grid.value().lines.clone(),
                );
                for field in fields_from_theirs(open.dialog.conflict()) {
                    let m = &mut mine.0;
                    match field.as_str() {
                        "client" => m.client = tf.client,
                        "txnDate" => m.date = tf.date,
                        "docNumber" => m.number.clone_from(&tf.number),
                        "poNumber" => m.po_number.clone_from(&tf.po_number),
                        "terms" => m.terms.clone_from(&tf.terms),
                        "termsDays" => m.terms_days = tf.terms_days,
                        "dueDate" => m.due_date = tf.due_date,
                        "memo" => m.memo.clone_from(&tf.memo),
                        "customerMessage" => m.customer_message.clone_from(&tf.customer_message),
                        "headerTags" => m.header_tags.clone_from(&tf.header_tags),
                        "hideZeroLines" => m.hide_zero_lines = tf.hide_zero_lines,
                        "printLater" => m.print_later = tf.print_later,
                        "layout" => m.layout = tf.layout.clone(),
                        "salesTax" => m.sales_tax = tf.sales_tax,
                        "depositTo" => m.deposit_to = tf.deposit_to,
                        "paymentMethod" => m.payment_method.clone_from(&tf.payment_method),
                        "refNo" => m.ref_no.clone_from(&tf.ref_no),
                        _ => {}
                    }
                }
                if !merge.lines_from_mine && open.dialog.conflict().theirs_changed_lines() {
                    // Their lines whole; the engine prices and taxes them.
                    let mut lines = their_lines.clone();
                    let mut v = InvoiceForm {
                        lines: std::mem::take(&mut lines),
                        ..InvoiceForm::default()
                    };
                    v.ensure_entry_line(self.grid.keys());
                    mine.1 = v.lines;
                }
                self.adopt(open.theirs, (tf, their_lines), mine);
                self.resave(open.intent);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_field_has_plain_words() {
        let v = doc_value(&InvoiceForm::default(), &[]);
        for k in v.as_object().expect("object").keys() {
            if k != "lines" {
                assert!(crate::i18n::has(&format!("lan.field.{k}")), "{k}");
            }
        }
    }
}
