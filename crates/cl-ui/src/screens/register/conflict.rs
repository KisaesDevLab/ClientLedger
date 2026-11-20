//! The register's inline edit and the save-conflict dialog (P33 D4 step
//! 11). The edit's save already carries the version (and on an engine with
//! `stale-all-v1` the stamp) it read; a 409 `STALE` now reads their
//! transaction, saves again without asking when nothing editable changed
//! on their side (once per save), and otherwise opens the shared
//! [`crate::screens::conflict_dialog`] over the register:
//!
//! - **Back:** the row keeps my changes and the existing "changed
//!   elsewhere" notice with its Reload button.
//! - **Use theirs:** the row opens their transaction.
//! - **Keep mine:** their transaction, plus my changed fields, plus the
//!   Splits lines whole from one side (the amount follows the lines), saved
//!   on their version.
//! - **Deleted or voided elsewhere:** the edit closes with the existing
//!   "removed elsewhere" note and is never saved again.

use std::sync::Arc;

use cl_api::{ApiError, ConflictReason, Transaction, TxnStatus, Versioned};
use cl_core::ids::TransactionId;
use cl_model::forms::SaveIntent;
use cl_model::lan::Conflict;
use cl_model::register::EntryDraft;
use serde_json::{Value, json};

use super::RegisterScreen;
use super::edit::{Seed, SeedLine, seed};
use super::splits::{LineSeed, SplitRow};
use crate::company::Task;
use crate::forms::conflict::{OpenConflict, Read, Words, expense_line, read_of, tag_ids};
use crate::screens::conflict_dialog::{ConflictChoice, fields_from_theirs};

/// The row's fields that hold an amount.
const MONEY: &[&str] = &["payment", "deposit"];

/// The row's editable header as the dialog compares it.
#[must_use]
pub fn row_value(d: &EntryDraft, lines: Vec<Value>) -> Value {
    json!({
        "txnDate": d.date.map(cl_core::CalendarDate::to_wire),
        "refNo": d.number.trim(),
        "payee": d.payee.map(|p| p.to_string()),
        "account": d.account.map(|a| a.to_string()),
        "memo": d.memo.trim(),
        "payment": d.payment.map(cl_core::Money::to_wire),
        "deposit": d.deposit.map(cl_core::Money::to_wire),
        "headerTags": tag_ids(&d.tags),
        "lines": lines,
    })
}

/// A seed line as a Splits row.
fn row_of(l: &SeedLine) -> SplitRow {
    SplitRow {
        account: Some(l.account),
        amount: Some(l.amount),
        memo: l.memo.clone(),
        customer: l.customer,
        tag: l.tag,
        ..SplitRow::blank(cl_grid::RowKeyGen::new().next_key())
    }
}

/// Applies the header fields Keep mine takes from `theirs` onto `mine`.
fn take_theirs(mine: &mut EntryDraft, theirs: &EntryDraft, fields: &[String]) {
    for field in fields {
        match field.as_str() {
            "txnDate" => mine.date = theirs.date,
            "refNo" => mine.number.clone_from(&theirs.number),
            "payee" => mine.payee = theirs.payee,
            "account" => mine.account = theirs.account,
            "memo" => mine.memo.clone_from(&theirs.memo),
            "payment" => mine.payment = theirs.payment,
            "deposit" => mine.deposit = theirs.deposit,
            "headerTags" => mine.tags.clone_from(&theirs.tags),
            _ => {}
        }
    }
}

/// A demo or engine refusal standing for "someone else saved it first".
pub(super) fn stale_error() -> ApiError {
    ApiError::Conflict {
        reason: ConflictReason::Stale,
        current_version: None,
        code: None,
        message: cl_api::UserText::default(),
        details: cl_api::ConflictDetails::default(),
    }
}

impl RegisterScreen {
    /// The demo's version of `id` now (`None`: gone or void).
    pub(super) fn demo_txn(&self, id: TransactionId) -> Option<Versioned<Transaction>> {
        let mut demo = self.link.demo_state()?;
        let chart = demo
            .chart
            .get_or_insert_with(crate::company::demo::chart)
            .clone();
        let contacts = demo.contacts_mut().clone();
        demo.ledger
            .get(&chart, &contacts, id)
            .filter(|_| !demo.ledger.is_void(id))
    }

    /// The lines the edit opened with.
    fn opened_lines(&self) -> Vec<Value> {
        self.edit_lines.iter().map(expense_line).collect()
    }

    /// The lines on screen: the Splits panel's, else the opened ones (the
    /// one-line row keeps its amount in the header).
    fn my_lines(&self) -> Vec<Value> {
        match &self.splits {
            Some(s) => s.value().used().map(expense_line).collect(),
            None => self.opened_lines(),
        }
    }

    /// The Splits panel's lines as seeds (to put mine back after theirs).
    fn my_seeds(&self) -> Option<Vec<LineSeed>> {
        self.splits.as_ref().map(|s| {
            s.value()
                .used()
                .map(|l| (l.account, l.amount, l.memo.clone(), l.customer, l.tag))
                .collect()
        })
    }

    /// The edit's save came back stale: read their transaction.
    pub(super) fn txn_conflict_start(&mut self) {
        let Some(id) = self.entry.editing.as_ref().map(|e| e.id) else {
            return;
        };
        self.txn_conflict_for = Some(id);
        if self.link.is_demo() {
            let got = self.demo_txn(id).ok_or(ApiError::NotFound { code: None });
            self.txn_conflict_read(got);
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        if let Ok(p) = group.read(&session, move |s| async move {
            let v = s.transactions().get(id).await?;
            if matches!(v.record.status, TxnStatus::Void | TxnStatus::Deleted) {
                return Err(ApiError::NotFound { code: None });
            }
            Ok(v)
        }) {
            self.txn_conflict.reading = Some((Task::Pending(p), SaveIntent::SaveAndNew));
        }
    }

    /// Polls the read of their transaction.
    pub(super) fn poll_txn_conflict(&mut self) {
        if let Some((r, _)) = self.txn_conflict.poll() {
            self.txn_conflict_read(r);
        }
    }

    /// Their transaction as the row would open it (`None`: not a shape the
    /// row edits any more).
    fn their_seed(&self, v: &Versioned<Transaction>) -> Option<Seed> {
        let own = self.account?;
        let s = seed(v, own, self.class);
        matches!(s, Seed::Edit { .. }).then_some(s)
    }

    pub(super) fn txn_conflict_read(&mut self, r: Result<Versioned<Transaction>, ApiError>) {
        let editing = self.entry.editing.as_ref().map(|e| e.id);
        if editing.is_none() || editing != self.txn_conflict_for {
            // Another edit, or none: the read is for nothing.
            return;
        }
        let theirs = match read_of(r) {
            Read::Theirs(v) => v,
            Read::Gone => {
                // Never saved again: the edit closes with the plain note.
                self.finish_edit();
                self.entry.problems = vec![cl_grid::TextKey("register.edit.removed_elsewhere")];
                return;
            }
            // The stale notice and its Reload button stay.
            Read::Reload => return,
        };
        let Some(Seed::Edit {
            editing,
            draft,
            lines,
        }) = self.their_seed(&theirs)
        else {
            return;
        };
        let their_lines: Vec<Value> = lines.iter().map(|l| expense_line(&row_of(l))).collect();
        let conflict = Conflict::new(
            &row_value(self.entry.form.form().original(), self.opened_lines()),
            &row_value(self.entry.draft(), self.my_lines()),
            &row_value(&draft, their_lines),
            "lines",
        );
        if conflict.silent_resave() {
            if std::mem::take(&mut self.silent_resaved) {
                // A second stale save in a row: the notice stays, no loop.
                return;
            }
            // Nothing editable changed on their side: my row, saved on
            // their version.
            self.silent_resaved = true;
            self.entry.editing = Some(editing);
            self.entry.error = None;
            self.start_record();
            return;
        }
        let words = Words::new(
            Arc::clone(&self.lists),
            None,
            self.parts.frame.money_style,
            MONEY,
        );
        self.txn_conflict.open = Some(OpenConflict {
            dialog: words.dialog(conflict),
            theirs,
            intent: SaveIntent::SaveAndNew,
        });
    }

    /// Draws the open dialog and runs the owner's choice.
    pub(super) fn txn_conflict_ui(&mut self, ctx: &eframe::egui::Context) {
        if let Some((open, choice)) = self.txn_conflict.ui(ctx) {
            self.run_txn_choice(open, choice);
        }
    }

    #[allow(clippy::needless_pass_by_value)] // the dialog hands over its choice
    pub(super) fn run_txn_choice(
        &mut self,
        open: OpenConflict<Versioned<Transaction>>,
        choice: ConflictChoice,
    ) {
        let editing = self.entry.editing.as_ref().map(|e| e.id);
        if editing.is_none() || editing != self.txn_conflict_for {
            return;
        }
        self.silent_resaved = false;
        let splits = self.splits.is_some();
        match choice {
            // The row keeps my changes; the stale notice offers Reload.
            ConflictChoice::Back => {}
            ConflictChoice::UseTheirs => {
                if let Some(s) = self.their_seed(&open.theirs) {
                    self.entry.error = None;
                    self.open_seed(s, splits);
                }
            }
            ConflictChoice::KeepMine(merge) => {
                let Some(s) = self.their_seed(&open.theirs) else {
                    return;
                };
                let Seed::Edit { draft: theirs, .. } = &s else {
                    return;
                };
                let theirs = theirs.clone();
                let mut mine = self.entry.draft().clone();
                let my_seeds = self.my_seeds();
                take_theirs(
                    &mut mine,
                    &theirs,
                    &fields_from_theirs(open.dialog.conflict()),
                );
                // With Splits the amount follows the lines it goes with.
                if splits && !merge.lines_from_mine {
                    mine.payment = theirs.payment;
                    mine.deposit = theirs.deposit;
                }
                self.entry.error = None;
                self.open_seed(s, splits);
                if let Some(rows) = my_seeds.filter(|_| merge.lines_from_mine) {
                    self.open_splits(rows);
                }
                if *self.entry.draft() != mine {
                    self.entry.form.edit_step(|d| *d = mine);
                }
                self.start_record();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_field_has_plain_words() {
        for k in row_value(&EntryDraft::default(), Vec::new())
            .as_object()
            .expect("object")
            .keys()
            .filter(|k| *k != "lines")
        {
            assert!(crate::i18n::has(&format!("lan.field.{k}")), "{k}");
        }
    }

    #[test]
    fn keep_mine_takes_every_field_it_is_given() {
        let theirs = EntryDraft {
            number: "12".into(),
            memo: "theirs".into(),
            payment: cl_core::Money::parse_wire("3.00").ok(),
            ..EntryDraft::default()
        };
        let fields: Vec<String> = row_value(&theirs, Vec::new())
            .as_object()
            .expect("object")
            .keys()
            .cloned()
            .collect();
        let mut mine = EntryDraft::default();
        take_theirs(&mut mine, &theirs, &fields);
        assert_eq!(row_value(&mine, Vec::new()), row_value(&theirs, Vec::new()));
    }
}
