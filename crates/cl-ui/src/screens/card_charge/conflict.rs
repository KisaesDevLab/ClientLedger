//! Enter Credit Card Charges' side of the save-conflict dialog (P33 D4 step
//! 11), as Write Check's: a 409 `STALE` on an edit reads their charge,
//! saves again on it without asking when nothing editable changed on their
//! side, and opens [`crate::screens::conflict_dialog`] otherwise.
//!
//! The Expenses and Items lines are one list, taken whole from one side,
//! and the amount follows the lines (it must equal them). A void charge or
//! a shape this form shows read-only is not merged: Reload shows it.

use std::sync::Arc;

use cl_api::{ApiError, Transaction, Versioned};
use cl_core::CalendarDate;
use cl_model::forms::SaveIntent;
use cl_model::forms::card::{CardForm, CardMode};
use cl_model::forms::item_lines::ItemLines;
use cl_model::forms::lines::FormLines;
use cl_model::lan::Conflict;
use serde_json::{Value, json};

use super::CardChargeScreen;
use crate::company::Task;
use crate::forms::conflict::{GONE, OpenConflict, Read, Words, lines_value, read_of};
use crate::screens::conflict_dialog::{ConflictChoice, fields_from_theirs};

/// The editable fields of a charge or credit as this form holds them (the
/// header tags are not read back from a saved charge, so they are left
/// out: they would always look changed).
#[must_use]
pub fn card_value(f: &CardForm, lines: &FormLines, items: &ItemLines) -> Value {
    json!({
        "card": f.card.map(|a| a.to_string()),
        "kind": match f.mode {
            CardMode::Charge => "charge",
            CardMode::Credit => "credit",
        },
        "payee": f.payee.map(|c| c.to_string()),
        "txnDate": f.date.map(CalendarDate::to_wire),
        "refNo": f.ref_no.trim(),
        "amount": f.amount.map(cl_core::Money::to_wire),
        "memo": f.memo.trim(),
        "lines": lines_value(lines, Some(items)),
    })
}

/// A charge as this form holds it.
pub(crate) struct Sides {
    form: CardForm,
    lines: FormLines,
    items: ItemLines,
}

impl CardChargeScreen {
    /// P33, after a refused save (the demo's or the engine's): someone
    /// else saved it first: their version, then the conflict dialog (or a
    /// silent re-save); deleted elsewhere: never saved again.
    pub(crate) fn after_failed(&mut self) {
        let stale = self.flow.stale_intent.take();
        if self.flow.editing.is_none() {
            return;
        }
        if let Some(intent) = stale {
            self.flow.stale = false;
            self.conflict_start(intent);
        } else if matches!(self.flow.error, Some(ApiError::NotFound { .. })) {
            self.flow.error = None;
            self.flow.notice = Some(GONE);
        }
    }

    /// A save came back stale: read their version (the dialog follows).
    pub(crate) fn conflict_start(&mut self, intent: SaveIntent) {
        let Some(id) = self.flow.editing.map(|e| e.id) else {
            self.flow.stale = true;
            return;
        };
        if self.link.is_demo() {
            let r = super::records::demo_get(&self.link, id);
            self.conflict_read(r, intent);
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            self.flow.stale = true;
            return;
        };
        match group.read(
            &session,
            move |s| async move { s.transactions().get(id).await },
        ) {
            Ok(p) => self.conflict.reading = Some((Task::Pending(p), intent)),
            Err(_) => self.flow.stale = true,
        }
    }

    /// Polls the read of their version.
    pub(crate) fn poll_conflict(&mut self) {
        if let Some((r, intent)) = self.conflict.poll() {
            self.conflict_read(r, intent);
        }
    }

    /// Their charge as this form shows it; `None` when it can't be merged
    /// here (void, a shape shown read-only).
    fn theirs_of(&mut self, v: &Versioned<Transaction>) -> Option<Sides> {
        let (form, lines, items, fits) = self.form_of(v);
        (fits && v.record.status != cl_api::TxnStatus::Void).then_some(Sides { form, lines, items })
    }

    fn mine(&mut self) -> Sides {
        Sides {
            form: self.header.current().clone(),
            lines: self.lines.value().clone(),
            items: self.items.grid.value().clone(),
        }
    }

    pub(crate) fn conflict_read(
        &mut self,
        r: Result<Versioned<Transaction>, ApiError>,
        intent: SaveIntent,
    ) {
        let theirs = match read_of(r) {
            Read::Theirs(v) => v,
            Read::Gone => {
                self.flow.notice = Some(GONE);
                return;
            }
            Read::Reload => {
                self.flow.stale = true;
                return;
            }
        };
        let Some(their) = self.theirs_of(&theirs) else {
            self.flow.stale = true;
            return;
        };
        let original = card_value(
            self.header.original(),
            self.lines.form().form().original(),
            self.items.grid.form_mut().form().original(),
        );
        let mine = self.mine();
        let conflict = Conflict::new(
            &original,
            &card_value(&mine.form, &mine.lines, &mine.items),
            &card_value(&their.form, &their.lines, &their.items),
            "lines",
        );
        if conflict.silent_resave() {
            self.adopt(&theirs, their, mine);
            self.start_save(intent, None);
            return;
        }
        let words = Words::new(
            Arc::clone(&self.books.lists),
            Some(self.items.grid.lists()),
            self.money_style,
            &["amount"],
        );
        self.conflict.open = Some(OpenConflict {
            dialog: words.dialog(conflict),
            theirs,
            intent,
        });
    }

    /// Their charge becomes the form's "as opened" and `mine` its fields,
    /// on their version.
    fn adopt(&mut self, theirs: &Versioned<Transaction>, their: Sides, mine: Sides) {
        let lists = Arc::clone(&self.books.lists);
        // Header tags are not compared; mine stay as they are.
        let mut their_form = their.form;
        their_form
            .header_tags
            .clone_from(&self.header.original().header_tags);
        self.header.load(their_form);
        if *self.header.current() != mine.form {
            self.header.edit_step(|h| *h = mine.form);
        }
        self.lines.load(their.lines, &*lists);
        if *self.lines.value() != mine.lines {
            self.lines.form_mut().edit_step(|v| *v = mine.lines);
            self.lines.lines_replaced();
        }
        self.items.grid.load(their.items);
        if *self.items.grid.value() != mine.items {
            self.items.grid.form_mut().edit_step(|v| *v = mine.items);
            self.items.grid.lines_replaced();
        }
        if let Some(e) = self.flow.editing.as_mut() {
            e.version = theirs.version;
            e.old_date = theirs.record.txn_date;
        }
        self.flow.stale = false;
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
        open: OpenConflict<Versioned<Transaction>>,
        choice: ConflictChoice,
    ) {
        match choice {
            ConflictChoice::Back => self.flow.stale = true,
            ConflictChoice::UseTheirs => self.show(&open.theirs),
            ConflictChoice::KeepMine(merge) => {
                let Some(their) = self.theirs_of(&open.theirs) else {
                    self.flow.stale = true;
                    return;
                };
                let mut mine = self.mine();
                for field in fields_from_theirs(open.dialog.conflict()) {
                    let tf = &their.form;
                    match field.as_str() {
                        "card" => mine.form.card = tf.card,
                        "kind" => mine.form.mode = tf.mode,
                        "payee" => mine.form.payee = tf.payee,
                        "txnDate" => mine.form.date = tf.date,
                        "refNo" => mine.form.ref_no.clone_from(&tf.ref_no),
                        "memo" => mine.form.memo.clone_from(&tf.memo),
                        // The amount follows the lines: mine with my lines.
                        "amount" if !merge.lines_from_mine => mine.form.amount = tf.amount,
                        _ => {}
                    }
                }
                if !merge.lines_from_mine && open.dialog.conflict().theirs_changed_lines() {
                    mine.lines = their.lines.clone();
                    mine.items = their.items.clone();
                    mine.form.amount = their.form.amount;
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
        let v = card_value(
            &CardForm::new(None, None),
            &FormLines::default(),
            &ItemLines::default(),
        );
        for k in v.as_object().expect("object").keys() {
            if k != "lines" {
                assert!(crate::i18n::has(&format!("lan.field.{k}")), "{k}");
            }
        }
    }
}
