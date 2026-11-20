//! Write Check's side of the save-conflict dialog (P33 D4 step 11), as
//! Make Journal Entry's: a 409 `STALE` on an edit reads their check, saves
//! again on it without asking when nothing editable changed on their side,
//! and opens [`crate::screens::conflict_dialog`] otherwise.
//!
//! The Expenses and Items lines are one list, taken whole from one side.
//! The check amount must equal the lines, so it follows the lines: their
//! amount with their lines, mine with mine (never their amount over my
//! re-split lines). A void check, a bill payment or a shape this form can't
//! show is not merged: the form says it changed elsewhere (Reload).

use std::sync::Arc;

use cl_api::{ApiError, CheckRecord, Versioned};
use cl_core::CalendarDate;
use cl_model::forms::SaveIntent;
use cl_model::forms::check::CheckForm;
use cl_model::forms::item_lines::ItemLines;
use cl_model::forms::lines::FormLines;
use cl_model::lan::Conflict;
use serde_json::{Value, json};

use super::WriteCheckScreen;
use super::save::form_of;
use crate::company::Task;
use crate::forms::conflict::{GONE, OpenConflict, Read, Words, lines_value, read_of, tag_ids};
use crate::screens::conflict_dialog::{ConflictChoice, fields_from_theirs};

/// The editable fields of a check as this form holds them.
#[must_use]
pub fn check_value(f: &CheckForm, lines: &FormLines, items: &ItemLines) -> Value {
    json!({
        "bank": f.bank.map(|a| a.to_string()),
        "checkNumber": f.number.text().map(|t| t.trim().to_string()),
        "printLater": f.print_later,
        "txnDate": f.date.map(CalendarDate::to_wire),
        "payee": f.payee.map(|c| c.to_string()),
        "payeeName": f.payee_name.trim(),
        "address": f.address.trim(),
        "amount": f.amount.map(|m| m.to_wire()),
        "memo": f.memo.trim(),
        "headerTags": tag_ids(&f.header_tags),
        "lines": lines_value(lines, Some(items)),
    })
}

/// A check read again, as this form would show it.
pub(crate) struct Theirs {
    form: CheckForm,
    lines: FormLines,
    items: ItemLines,
}

impl WriteCheckScreen {
    /// A save came back stale: read their version (the dialog follows).
    pub(crate) fn conflict_start(&mut self, intent: SaveIntent) {
        let Some(id) = self.editing.as_ref().map(|e| e.id) else {
            self.save.stale = true;
            return;
        };
        if self.link.is_demo() {
            let r = self.link.demo_state().map_or(
                Err(ApiError::NotFound { code: None }),
                |mut d| {
                    crate::company::demo::checks::get(&mut d, id)
                        .ok_or(ApiError::NotFound { code: None })
                },
            );
            self.conflict_read(r, intent);
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            self.save.stale = true;
            return;
        };
        match group.read(&session, move |s| async move { s.checks().get(id).await }) {
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

    /// Their check as this form shows it; `None` when it can't be merged
    /// here (void, a bill payment, a shape this form shows read-only).
    fn theirs_of(&mut self, v: &Versioned<CheckRecord>) -> Option<Theirs> {
        let bank = self.editing.as_ref().map(|e| e.bank)?;
        let (form, lines, items, exact) =
            form_of(&v.record, bank, &mut self.lines.form_mut().adapter_mut().keys);
        if self.read_only_note_of(&v.record, exact).is_some() {
            return None;
        }
        Some(Theirs { form, lines, items })
    }

    pub(crate) fn conflict_read(&mut self, r: Result<Versioned<CheckRecord>, ApiError>, intent: SaveIntent) {
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
        let original = check_value(
            self.header.original(),
            self.lines.form().form().original(),
            self.items.grid.form_mut().form().original(),
        );
        let mine = self.mine();
        let conflict = Conflict::new(
            &original,
            &check_value(&mine.form, &mine.lines, &mine.items),
            &check_value(&their.form, &their.lines, &their.items),
            "lines",
        );
        if conflict.silent_resave() {
            // Nothing editable changed on their side: save again on their
            // version without asking (S2).
            self.adopt(&theirs, their, mine);
            self.start_save(intent, None);
            return;
        }
        let words = Words::new(
            Arc::clone(&self.lists),
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

    /// The form as it is now.
    fn mine(&mut self) -> Theirs {
        Theirs {
            form: self.header.current().clone(),
            lines: self.lines.value().clone(),
            items: self.items.grid.value().clone(),
        }
    }

    /// Their check becomes the form's "as opened" (so a later conflict
    /// compares against it) and `mine` its fields, on their versions.
    fn adopt(&mut self, theirs: &Versioned<CheckRecord>, their: Theirs, mine: Theirs) {
        let lists = Arc::clone(&self.lists);
        self.header.load(their.form);
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
        if let Some(e) = self.editing.as_mut() {
            e.version = theirs.version;
            e.txn_version = theirs.transaction_version();
            e.old_date = theirs.record.txn.txn_date;
            e.print_status = theirs.record.print_status.clone();
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

    pub(crate) fn run_choice(
        &mut self,
        open: OpenConflict<Versioned<CheckRecord>>,
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
                    match field.as_str() {
                        "bank" => mine.form.bank = tf.bank,
                        "checkNumber" | "printLater" => {
                            mine.form.number = tf.number.clone();
                            mine.form.print_later = tf.print_later;
                        }
                        "txnDate" => mine.form.date = tf.date,
                        "payee" => mine.form.payee = tf.payee,
                        "payeeName" => mine.form.payee_name.clone_from(&tf.payee_name),
                        "address" => mine.form.address.clone_from(&tf.address),
                        "memo" => mine.form.memo.clone_from(&tf.memo),
                        "headerTags" => mine.form.header_tags.clone_from(&tf.header_tags),
                        // The amount follows the lines: mine with my lines.
                        "amount" if !merge.lines_from_mine => mine.form.amount = tf.amount,
                        _ => {}
                    }
                }
                if !merge.lines_from_mine && open.dialog.conflict().theirs_changed_lines() {
                    // Their lines whole, with their amount (they add up).
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
        let f = super::super::blank(
            None,
            CalendarDate::from_ymd(2026, 11, 4).expect("date"),
            cl_model::forms::check::NumberField::Typed(String::new()),
        );
        let v = check_value(&f, &FormLines::default(), &ItemLines::default());
        for k in v.as_object().expect("object").keys() {
            if k != "lines" {
                assert!(crate::i18n::has(&format!("lan.field.{k}")), "{k}");
            }
        }
    }
}
