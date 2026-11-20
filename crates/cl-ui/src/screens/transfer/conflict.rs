//! Transfer Funds' side of the save-conflict dialog (P33 D4 step 11), as
//! Make Journal Entry's: a 409 `STALE` on an edit reads their transfer,
//! saves again on it without asking when nothing editable changed on their
//! side, and opens [`crate::screens::conflict_dialog`] otherwise. A
//! transfer has no lines; Keep mine is my form plus each of their changes
//! I didn't make (a transfer is one amount on both sides, so it stays
//! balanced whichever amount wins).

use std::sync::Arc;

use cl_api::{Transaction, Versioned};
use cl_core::CalendarDate;
use cl_model::forms::SaveIntent;
use cl_model::forms::transfer::TransferForm;
use cl_model::lan::Conflict;
use serde_json::{Value, json};

use super::{TransferScreen, form_of};
use crate::company::Task;
use crate::screens::conflict_dialog::{ConflictChoice, ConflictDialog, fields_from_theirs};
use crate::screens::register::account_text;

/// The editable fields of a transfer as this form holds them.
#[must_use]
pub fn transfer_value(f: &TransferForm) -> Value {
    json!({
        "from": f.from.map(|a| a.to_string()),
        "to": f.to.map(|a| a.to_string()),
        "txnDate": f.date.map(CalendarDate::to_wire),
        "amount": f.amount.map(cl_core::Money::to_wire),
        "memo": f.memo.trim(),
    })
}

/// A conflict waiting for the owner.
pub(crate) struct OpenConflict {
    pub dialog: ConflictDialog,
    pub theirs: Versioned<Transaction>,
    pub intent: SaveIntent,
}

impl TransferScreen {
    /// A save came back stale: read their version (the dialog follows).
    pub(crate) fn conflict_start(&mut self, intent: SaveIntent) {
        let Some(id) = self.editing.as_ref().map(|e| e.id) else {
            self.stale = true;
            return;
        };
        let link = Arc::clone(&self.link);
        if let Some(local) = self.local.as_mut() {
            let r = local.get(&link, id);
            self.conflict_read(r, intent);
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            self.stale = true;
            return;
        };
        match group.read(
            &session,
            move |s| async move { s.transactions().get(id).await },
        ) {
            Ok(p) => self.theirs_task = Some((Task::Pending(p), intent)),
            Err(_) => self.stale = true,
        }
    }

    /// Polls the read of their version.
    pub(crate) fn poll_conflict(&mut self) {
        if let Some((mut task, intent)) = self.theirs_task.take() {
            match task.try_take() {
                None => self.theirs_task = Some((task, intent)),
                Some(r) => self.conflict_read(r, intent),
            }
        }
    }

    fn conflict_read(
        &mut self,
        r: Result<Versioned<Transaction>, cl_api::ApiError>,
        intent: SaveIntent,
    ) {
        let theirs = match r {
            Ok(v) => v,
            Err(cl_api::ApiError::NotFound { .. }) => {
                self.notice = Some("lan.conflict.gone");
                return;
            }
            Err(_) => {
                self.stale = true;
                return;
            }
        };
        let (their_form, fits) = form_of(&theirs.record);
        if !fits {
            // No longer a plain transfer: Reload shows it read-only.
            self.stale = true;
            return;
        }
        let conflict = Conflict::new(
            &transfer_value(self.form.original()),
            &transfer_value(self.form.current()),
            &transfer_value(&their_form),
            "lines",
        );
        if conflict.silent_resave() {
            self.adopt(&theirs, their_form, self.form.current().clone());
            self.start_save(intent, None);
            return;
        }
        let names: Vec<(String, String)> = self
            .accounts
            .iter()
            .map(|a| (a.id.to_string(), account_text(a)))
            .collect();
        let style = self.money_style;
        let dialog = ConflictDialog::new(conflict, Box::new(|_| String::new())).with_field_text(
            Box::new(move |field, v| match field {
                "from" | "to" => v.as_str().and_then(|id| {
                    names
                        .iter()
                        .find(|(k, _)| k == id)
                        .map(|(_, text)| text.clone())
                }),
                "amount" => v
                    .as_str()
                    .and_then(|s| cl_core::Money::parse_wire(s).ok())
                    .map(|m| cl_core::report::display_amount(m.as_decimal(), style)),
                _ => None,
            }),
        );
        self.conflict = Some(OpenConflict {
            dialog,
            theirs,
            intent,
        });
    }

    /// Their record becomes the form's "as opened" (so a later conflict
    /// compares against it) and `mine` its fields, on their version.
    fn adopt(
        &mut self,
        theirs: &Versioned<Transaction>,
        their_form: TransferForm,
        mine: TransferForm,
    ) {
        self.form.load(their_form);
        if *self.form.current() != mine {
            self.form.edit_step(|f| *f = mine);
        }
        if let Some(e) = self.editing.as_mut() {
            e.version = theirs.version;
            e.old_date = theirs.record.txn_date;
        }
        self.key = None;
        self.stale = false;
        self.sync_inputs();
    }

    /// Draws the open dialog and runs the owner's choice.
    pub(crate) fn conflict_ui(&mut self, ctx: &eframe::egui::Context) {
        let Some(open) = self.conflict.as_mut() else {
            return;
        };
        let Some(choice) = open.dialog.show(ctx) else {
            return;
        };
        if let Some(open) = self.conflict.take() {
            self.run_choice(open, choice);
        }
    }

    #[allow(clippy::needless_pass_by_value)] // the dialog hands over its choice
    pub(crate) fn run_choice(&mut self, open: OpenConflict, choice: ConflictChoice) {
        match choice {
            ConflictChoice::Back => self.stale = true,
            ConflictChoice::UseTheirs => self.show(&open.theirs),
            ConflictChoice::KeepMine(_) => {
                let (their_form, _) = form_of(&open.theirs.record);
                let mut mine = self.form.current().clone();
                for field in fields_from_theirs(open.dialog.conflict()) {
                    let tf = their_form.clone();
                    match field.as_str() {
                        "from" => mine.from = tf.from,
                        "to" => mine.to = tf.to,
                        "txnDate" => mine.date = tf.date,
                        "amount" => mine.amount = tf.amount,
                        "memo" => mine.memo = tf.memo,
                        _ => {}
                    }
                }
                self.adopt(&open.theirs, their_form, mine);
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
        let v = transfer_value(&TransferForm::default());
        for k in v.as_object().expect("object").keys() {
            assert!(crate::i18n::has(&format!("lan.field.{k}")), "{k}");
        }
    }
}
