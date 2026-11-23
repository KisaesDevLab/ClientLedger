//! Make Journal Entry's side of the save-conflict dialog (P33 D4 step 11):
//! a 409 `STALE` on an edit reads their version, then saves again on it
//! without asking when nothing editable changed on their side, and opens
//! [`crate::screens::conflict_dialog`] otherwise.
//!
//! The three versions are this form's own fields (as opened, as now, and
//! their record as this form would show it), so the comparison never
//! depends on how the engine orders or names things. Amounts are compared
//! as the engine's exact strings.

use std::collections::HashMap;
use std::sync::Arc;

use cl_api::{Transaction, Versioned};
use cl_model::forms::SaveIntent;
use cl_model::forms::journal::JournalForm;
use cl_model::forms::lines::FormLines;
use cl_model::lan::{Conflict, canonical};
use serde_json::{Value, json};

use super::{JournalScreen, field};
use crate::company::Task;
use crate::screens::conflict_dialog::{ConflictChoice, ConflictDialog};

/// The editable fields of a journal entry as this form holds them.
#[must_use]
pub fn je_value(form: &JournalForm, lines: &FormLines) -> Value {
    json!({
        "txnDate": form.date.map(cl_core::CalendarDate::to_wire),
        "number": form.number.trim(),
        "memo": form.memo.trim(),
        "adjusting": form.adjusting,
        "reverseOn": form.reverse_on.map(cl_core::CalendarDate::to_wire),
        "lines": lines.used().map(|l| json!({
            "account": l.account.map(|a| a.to_string()),
            "amount": l.amount.map(cl_core::Money::to_wire),
            "memo": l.memo.trim(),
            "name": l.customer.map(|c| c.to_string()),
            "tag": l.tag.map(|t| t.to_string()),
        })).collect::<Vec<_>>(),
    })
}

/// A conflict waiting for the owner.
pub(crate) struct OpenConflict {
    pub dialog: ConflictDialog,
    pub theirs: Versioned<Transaction>,
    pub intent: SaveIntent,
}

impl JournalScreen {
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
        let Some((opened_form, opened_lines)) = self.opened.clone() else {
            self.stale = true;
            return;
        };
        let (their_form, their_lines, fits) = self.form_of(&theirs);
        if !fits {
            // Their entry no longer fits this form: Reload shows it.
            self.stale = true;
            return;
        }
        let mine_lines = self.lines.value().clone();
        let original = je_value(&opened_form, &opened_lines);
        let mine = je_value(self.header.current(), &mine_lines);
        let their_value = je_value(&their_form, &their_lines);
        let conflict = Conflict::new(&original, &mine, &their_value, "lines");
        if conflict.silent_resave() {
            // Nothing editable changed on their side: save again on their
            // version without asking (S2).
            self.adopt_version(&theirs, their_form, their_lines);
            self.start_save(intent, None);
            return;
        }
        let texts = self.line_texts(&[&opened_lines, &mine_lines, &their_lines]);
        let dialog = ConflictDialog::new(
            conflict,
            Box::new(move |v| texts.get(&canonical(v)).cloned().unwrap_or_default()),
        );
        self.conflict = Some(OpenConflict {
            dialog,
            theirs,
            intent,
        });
    }

    /// Each line's words (account, amount, memo), by its canonical value.
    fn line_texts(&self, sides: &[&FormLines]) -> HashMap<String, String> {
        let lists = &self.books.lists;
        let style = self.money_style;
        let mut out = HashMap::new();
        for side in sides {
            for l in side.used() {
                let key = canonical(&json!({
                    "account": l.account.map(|a| a.to_string()),
                    "amount": l.amount.map(cl_core::Money::to_wire),
                    "memo": l.memo.trim(),
                    "name": l.customer.map(|c| c.to_string()),
                    "tag": l.tag.map(|t| t.to_string()),
                }));
                let account = l
                    .account
                    .and_then(|a| lists.account_text(a).map(str::to_string))
                    .unwrap_or_default();
                let amount = l.amount.map_or_else(String::new, |m| {
                    let side = if m.is_negative() { "Cr" } else { "Dr" };
                    format!(
                        "{side} {}",
                        cl_core::report::display_amount(m.abs().as_decimal(), style)
                    )
                });
                let memo = l.memo.trim();
                let text = [account.as_str(), amount.as_str(), memo]
                    .into_iter()
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join(" \u{00b7} ");
                out.insert(key, text);
            }
        }
        out
    }

    /// The form keeps its fields and takes their version; their record is
    /// the new "as opened", so a later conflict compares against it (what
    /// this form took from theirs never counts as a change of mine).
    fn adopt_version(
        &mut self,
        theirs: &Versioned<Transaction>,
        their_form: JournalForm,
        their_lines: FormLines,
    ) {
        if let Some(e) = self.editing.as_mut() {
            e.version = theirs.version;
            e.old_date = theirs.record.txn_date;
        }
        self.opened = Some((their_form, their_lines));
        self.key = None;
        self.stale = false;
    }

    /// Draws the open dialog and runs the owner's choice.
    pub(crate) fn conflict_ui(&mut self, ctx: &eframe::egui::Context) {
        let Some(open) = self.conflict.as_mut() else {
            return;
        };
        let Some(choice) = open.dialog.show(ctx) else {
            return;
        };
        let Some(open) = self.conflict.take() else {
            return;
        };
        self.run_choice(open, choice);
    }

    #[allow(clippy::needless_pass_by_value)] // the dialog hands over its choice
    pub(crate) fn run_choice(&mut self, open: OpenConflict, choice: ConflictChoice) {
        match choice {
            ConflictChoice::Back => self.stale = true,
            ConflictChoice::UseTheirs => self.show(&open.theirs),
            ConflictChoice::KeepMine(merge) => {
                // My form, plus each of their header changes I didn't make,
                // plus their lines when I left mine alone: the merge, on
                // their version.
                let (their_form, their_lines, _) = self.form_of(&open.theirs);
                for f in open.dialog.conflict().field_changes() {
                    if f.mine_changed() || !f.theirs_changed() {
                        continue;
                    }
                    let tf = their_form.clone();
                    match f.field.as_str() {
                        "txnDate" => self.header.edit(field::DATE, |h| h.date = tf.date),
                        "number" => self.header.edit(field::NUMBER, |h| h.number = tf.number),
                        "memo" => self.header.edit(field::MEMO, |h| h.memo = tf.memo),
                        "adjusting" => self
                            .header
                            .edit(field::ADJUSTING, |h| h.adjusting = tf.adjusting),
                        "reverseOn" => self
                            .header
                            .edit(field::REVERSE, |h| h.reverse_on = tf.reverse_on),
                        _ => false,
                    };
                }
                if !merge.lines_from_mine && open.dialog.conflict().theirs_changed_lines() {
                    let lists = Arc::clone(&self.books.lists);
                    self.lines.load(their_lines.clone(), &*lists);
                }
                self.sync_inputs();
                self.adopt_version(&open.theirs, their_form, their_lines);
                self.start_save(open.intent, None);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_projection_keeps_amounts_as_exact_text() {
        let form = JournalForm {
            date: cl_core::CalendarDate::from_ymd(2026, 11, 2).ok(),
            number: " 7 ".into(),
            memo: "rent".into(),
            adjusting: false,
            reverse_on: None,
            lines: FormLines::default(),
        };
        let v = je_value(&form, &FormLines::default());
        assert_eq!(v["number"], "7");
        assert_eq!(v["txnDate"], "2026-11-02");
        assert_eq!(v["lines"], json!([]));
    }
}
