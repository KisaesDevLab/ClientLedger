//! Enter Bills' side of the save-conflict dialog (P33 D4 step 11), for
//! bills and vendor credits alike, as Write Check's: a 409 `STALE` on an
//! edit reads their bill, saves again on it without asking when nothing
//! editable changed on their side, and opens
//! [`crate::screens::conflict_dialog`] otherwise.
//!
//! The Expenses and Items lines are one list, taken whole from one side,
//! and the amount due follows the lines (it must equal them). Due and
//! discount dates come from the engine's terms when the owner didn't type
//! them. A void or read-only bill is not merged: Reload shows it.

use std::sync::Arc;

use cl_api::{ApiError, BillRecord, Versioned};
use cl_core::CalendarDate;
use cl_model::forms::SaveIntent;
use cl_model::forms::bill::{BillForm, BillKind};
use cl_model::forms::item_lines::ItemLines;
use cl_model::forms::lines::FormLines;
use cl_model::lan::Conflict;
use serde_json::{Value, json};

use super::EnterBillScreen;
use super::save::form_of;
use crate::company::Task;
use crate::forms::conflict::{GONE, OpenConflict, Read, Words, lines_value, read_of};
use crate::screens::conflict_dialog::{ConflictChoice, fields_from_theirs};

/// The editable fields of a bill or credit as this form holds them (header
/// tags are not read back from a saved bill, so they are left out).
#[must_use]
pub fn bill_value(f: &BillForm, lines: &FormLines, items: &ItemLines) -> Value {
    json!({
        "vendor": f.vendor.map(|c| c.to_string()),
        "txnDate": f.date.map(CalendarDate::to_wire),
        "refNo": f.reference.trim(),
        "terms": f.terms,
        "termsDays": f.terms_days,
        "discountPercent": f.discount_percent.map(cl_model::forms::bill::Percent::to_wire),
        "discountDays": f.discount_days,
        "amount": f.amount_due.map(cl_core::Money::to_wire),
        "dueDate": f.due_date.map(CalendarDate::to_wire),
        "discountDate": f.discount_date.map(CalendarDate::to_wire),
        "memo": f.memo.trim(),
        "lines": lines_value(lines, Some(items)),
    })
}

/// A bill as this form holds it.
pub(crate) struct Sides {
    form: BillForm,
    lines: FormLines,
    items: ItemLines,
}

impl EnterBillScreen {
    /// A save came back stale: read their version (the dialog follows).
    pub(crate) fn conflict_start(&mut self, intent: SaveIntent) {
        let Some((id, kind)) = self.editing.as_ref().map(|e| (e.id, e.kind)) else {
            self.save.stale = true;
            return;
        };
        if self.link.is_demo() {
            let got =
                self.link
                    .demo_state()
                    .map_or(Err(ApiError::NotFound { code: None }), |mut d| {
                        crate::company::demo::bills::get(&mut d, id)
                            .ok_or(ApiError::NotFound { code: None })
                    });
            self.conflict_read(got, intent);
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            self.save.stale = true;
            return;
        };
        match group.read(&session, move |s| async move {
            match kind {
                BillKind::Bill => s.bills().get(id).await,
                BillKind::Credit => s.vendor_credits().get(id).await,
            }
        }) {
            Ok(p) => self.conflict.reading = Some((Task::Pending(p), intent)),
            Err(_) => self.save.stale = true,
        }
    }

    /// Polls the read of their version.
    pub(crate) fn poll_conflict(&mut self) {
        if let Some((r, intent)) = self.conflict.poll() {
            self.conflict_read(r, intent);
        }
    }

    /// Their bill as this form shows it; `None` when it can't be merged here
    /// (void, or shown read-only).
    fn theirs_of(&mut self, v: &Versioned<BillRecord>) -> Option<Sides> {
        if self.read_only_note_of(&v.record).is_some() {
            return None;
        }
        let mut ek = std::mem::take(&mut self.expenses.form_mut().adapter_mut().keys);
        let (form, lines, items) = form_of(&v.record, &mut ek, self.items.keys());
        self.expenses.form_mut().adapter_mut().keys = ek;
        Some(Sides { form, lines, items })
    }

    fn mine(&mut self) -> Sides {
        Sides {
            form: self.header.current().clone(),
            lines: self.expenses.value().clone(),
            items: self.items.value().clone(),
        }
    }

    pub(crate) fn conflict_read(
        &mut self,
        r: Result<Versioned<BillRecord>, ApiError>,
        intent: SaveIntent,
    ) {
        let theirs = match read_of(r) {
            Read::Theirs(v) => v,
            Read::Gone => {
                self.save.notice = Some(GONE);
                return;
            }
            Read::Reload => {
                self.save.stale = true;
                return;
            }
        };
        let Some(their) = self.theirs_of(&theirs) else {
            self.save.stale = true;
            return;
        };
        let original = bill_value(
            self.header.original(),
            self.expenses.form().form().original(),
            self.items.form_mut().form().original(),
        );
        let mine = self.mine();
        let conflict = Conflict::new(
            &original,
            &bill_value(&mine.form, &mine.lines, &mine.items),
            &bill_value(&their.form, &their.lines, &their.items),
            "lines",
        );
        if conflict.silent_resave() {
            self.adopt(&theirs, their, mine);
            self.start_save(intent, None);
            return;
        }
        let words = Words::new(
            Arc::clone(&self.lists),
            Some(self.items.lists()),
            self.money_style,
            &["amount"],
        );
        self.conflict.open = Some(OpenConflict {
            dialog: words.dialog(conflict),
            theirs,
            intent,
        });
    }

    /// Their bill becomes the form's "as opened" and `mine` its fields, on
    /// their version.
    fn adopt(&mut self, theirs: &Versioned<BillRecord>, their: Sides, mine: Sides) {
        let lists = Arc::clone(&self.lists);
        let mut their_form = their.form;
        // Header tags are not compared; the form's own stay.
        their_form
            .header_tags
            .clone_from(&self.header.original().header_tags);
        self.header.load(their_form);
        if *self.header.current() != mine.form {
            self.header.edit_step(|h| *h = mine.form);
        }
        self.expenses.load(their.lines, &*lists);
        if *self.expenses.value() != mine.lines {
            self.expenses.form_mut().edit_step(|v| *v = mine.lines);
            self.expenses.lines_replaced();
        }
        self.items.load(their.items);
        if *self.items.value() != mine.items {
            self.items.form_mut().edit_step(|v| *v = mine.items);
            self.items.lines_replaced();
        }
        if let Some(e) = self.editing.as_mut() {
            e.record = theirs.clone();
            e.old_date = theirs.record.txn.txn_date;
            e.has_payments = theirs.record.has_payments();
        }
        self.save.key = None;
        self.save.stale = false;
        self.sync_inputs();
    }

    /// Draws the open dialog and runs the owner's choice.
    pub(crate) fn conflict_ui(&mut self, ctx: &eframe::egui::Context) {
        if let Some((open, choice)) = self.conflict.ui(ctx) {
            self.run_choice(open, choice);
        }
    }

    #[allow(clippy::needless_pass_by_value)] // the dialog hands over its choice
    pub(crate) fn run_choice(
        &mut self,
        open: OpenConflict<Versioned<BillRecord>>,
        choice: ConflictChoice,
    ) {
        match choice {
            ConflictChoice::Back => self.save.stale = true,
            ConflictChoice::UseTheirs => self.show_record(&open.theirs),
            ConflictChoice::KeepMine(merge) => {
                let Some(their) = self.theirs_of(&open.theirs) else {
                    self.save.stale = true;
                    return;
                };
                let mut mine = self.mine();
                for field in fields_from_theirs(open.dialog.conflict()) {
                    let tf = &their.form;
                    let m = &mut mine.form;
                    match field.as_str() {
                        "vendor" => m.vendor = tf.vendor,
                        "txnDate" => m.date = tf.date,
                        "refNo" => m.reference.clone_from(&tf.reference),
                        "terms" => m.terms.clone_from(&tf.terms),
                        "termsDays" => m.terms_days = tf.terms_days,
                        "discountPercent" => m.discount_percent = tf.discount_percent,
                        "discountDays" => m.discount_days = tf.discount_days,
                        "dueDate" => {
                            m.due_date = tf.due_date;
                            m.due_overridden = tf.due_overridden;
                        }
                        "discountDate" => {
                            m.discount_date = tf.discount_date;
                            m.discount_overridden = tf.discount_overridden;
                        }
                        "memo" => m.memo.clone_from(&tf.memo),
                        // The amount follows the lines: mine with my lines.
                        "amount" if !merge.lines_from_mine => m.amount_due = tf.amount_due,
                        _ => {}
                    }
                }
                if !merge.lines_from_mine && open.dialog.conflict().theirs_changed_lines() {
                    mine.lines = their.lines.clone();
                    mine.items = their.items.clone();
                    mine.form.amount_due = their.form.amount_due;
                    mine.form.amount_from_lines = their.form.amount_from_lines;
                }
                self.adopt(&open.theirs, their, mine);
                self.start_save(open.intent, None);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_field_has_plain_words() {
        let f = super::super::blank(
            BillKind::Bill,
            CalendarDate::from_ymd(2026, 11, 4).expect("date"),
        );
        let v = bill_value(&f, &FormLines::default(), &ItemLines::default());
        for k in v.as_object().expect("object").keys() {
            if k != "lines" {
                assert!(crate::i18n::has(&format!("lan.field.{k}")), "{k}");
            }
        }
    }
}
