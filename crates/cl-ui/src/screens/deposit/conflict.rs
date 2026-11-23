//! Make Deposits' side of the save-conflict dialog (P33 D4 step 11), as
//! Make Journal Entry's: a 409 `STALE` on an edit reads their deposit,
//! saves again on it without asking when nothing editable changed on their
//! side, and opens [`crate::screens::conflict_dialog`] otherwise.
//!
//! The other-deposit lines are taken whole from one side; the deposit total
//! is the engine's (bank debit of the parts), never added up here. A void,
//! grouped or odd-shaped deposit is not merged: Reload shows it.

use std::sync::Arc;

use cl_api::{ApiError, Transaction, Versioned};
use cl_core::CalendarDate;
use cl_model::forms::SaveIntent;
use cl_model::forms::deposit::DepositForm;
use cl_model::forms::lines::FormLines;
use cl_model::lan::Conflict;
use serde_json::{Value, json};

use super::DepositScreen;
use super::save::form_of;
use crate::company::Task;
use crate::forms::conflict::{GONE, OpenConflict, Read, Words, lines_value, read_of};
use crate::screens::conflict_dialog::{ConflictChoice, fields_from_theirs};

/// The editable fields of a deposit as this form holds them.
#[must_use]
pub fn deposit_value(f: &DepositForm, lines: &FormLines) -> Value {
    json!({
        "depositTo": f.account.map(|a| a.to_string()),
        "txnDate": f.date.map(CalendarDate::to_wire),
        "memo": f.memo.trim(),
        "payments": f.picked.iter().map(|p| json!({
            "source": p.source.to_string(),
            "amount": p.amount.to_wire(),
        })).collect::<Vec<_>>(),
        "cashBackAccount": f.cash_back.account.map(|a| a.to_string()),
        "cashBackAmount": f.cash_back.amount.map(cl_core::Money::to_wire),
        "cashBackMemo": f.cash_back.memo.trim(),
        "lines": lines_value(lines, None),
    })
}

impl DepositScreen {
    /// A save came back stale: read their version (the dialog follows).
    pub(crate) fn conflict_start(&mut self, intent: SaveIntent) {
        let Some(id) = self.editing.as_ref().map(|e| e.id) else {
            self.save.stale = true;
            return;
        };
        let link = Arc::clone(&self.link);
        if let Some(local) = self.local.as_mut() {
            let r = local.get(&link, id);
            self.conflict_read(r, intent);
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            self.save.stale = true;
            return;
        };
        match group.read(
            &session,
            move |s| async move { s.transactions().get(id).await },
        ) {
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

    /// Their deposit as this form shows it; `None` when it can't be merged
    /// here (void, grouped, a shape shown read-only).
    fn theirs_of(&mut self, v: &Versioned<Transaction>) -> Option<(DepositForm, FormLines)> {
        let r = self.record_of(&v.record).ok()?;
        if r.txn.status == cl_api::TxnStatus::Void || r.is_grouped() || !r.exact {
            return None;
        }
        Some(form_of(&r, &mut self.lines.form_mut().adapter_mut().keys))
    }

    pub(crate) fn conflict_read(
        &mut self,
        r: Result<Versioned<Transaction>, ApiError>,
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
        let Some((their_form, their_lines)) = self.theirs_of(&theirs) else {
            self.save.stale = true;
            return;
        };
        let mine = (self.header.current().clone(), self.lines.value().clone());
        let conflict = Conflict::new(
            &deposit_value(self.header.original(), self.lines.form().form().original()),
            &deposit_value(&mine.0, &mine.1),
            &deposit_value(&their_form, &their_lines),
            "lines",
        );
        if conflict.silent_resave() {
            self.adopt(&theirs, (their_form, their_lines), mine);
            self.start_save(intent, None);
            return;
        }
        let words = Words::new(
            Arc::clone(&self.lists),
            None,
            self.money_style,
            &["cashBackAmount"],
        );
        self.conflict.open = Some(OpenConflict {
            dialog: words.dialog(conflict),
            theirs,
            intent,
        });
    }

    /// Their deposit becomes the form's "as opened" and `mine` its fields,
    /// on their version.
    fn adopt(
        &mut self,
        theirs: &Versioned<Transaction>,
        their: (DepositForm, FormLines),
        mine: (DepositForm, FormLines),
    ) {
        let lists = Arc::clone(&self.lists);
        self.header.load(their.0);
        if *self.header.current() != mine.0 {
            self.header.edit_step(|h| *h = mine.0);
        }
        self.lines.load(their.1, &*lists);
        if *self.lines.value() != mine.1 {
            self.lines.form_mut().edit_step(|v| *v = mine.1);
            self.lines.lines_replaced();
        }
        if let Ok(r) = self.record_of(&theirs.record) {
            self.record = Some(r);
        }
        if let Some(e) = self.editing.as_mut() {
            e.version = theirs.version;
            e.old_date = theirs.record.txn_date;
        }
        self.save.key.clear();
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
        open: OpenConflict<Versioned<Transaction>>,
        choice: ConflictChoice,
    ) {
        match choice {
            ConflictChoice::Back => self.save.stale = true,
            ConflictChoice::UseTheirs => self.show_txn(&open.theirs),
            ConflictChoice::KeepMine(merge) => {
                let Some((tf, their_lines)) = self.theirs_of(&open.theirs) else {
                    self.save.stale = true;
                    return;
                };
                let mut mine = (self.header.current().clone(), self.lines.value().clone());
                for field in fields_from_theirs(open.dialog.conflict()) {
                    match field.as_str() {
                        "depositTo" => mine.0.account = tf.account,
                        "txnDate" => mine.0.date = tf.date,
                        "memo" => mine.0.memo.clone_from(&tf.memo),
                        "payments" => mine.0.picked.clone_from(&tf.picked),
                        "cashBackAccount" => mine.0.cash_back.account = tf.cash_back.account,
                        "cashBackAmount" => mine.0.cash_back.amount = tf.cash_back.amount,
                        "cashBackMemo" => mine.0.cash_back.memo.clone_from(&tf.cash_back.memo),
                        _ => {}
                    }
                }
                if !merge.lines_from_mine && open.dialog.conflict().theirs_changed_lines() {
                    mine.1 = their_lines.clone();
                }
                self.adopt(&open.theirs, (tf, their_lines), mine);
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
        let f = super::super::blank(None, CalendarDate::from_ymd(2026, 11, 4).expect("date"));
        let v = deposit_value(&f, &FormLines::default());
        for k in v.as_object().expect("object").keys() {
            if k != "lines" {
                assert!(crate::i18n::has(&format!("lan.field.{k}")), "{k}");
            }
        }
    }
}
