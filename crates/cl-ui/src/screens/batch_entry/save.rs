//! Save all (Alt+S; Alt+N saves and starts a new batch) and the window's
//! commands. Save all goes only when every non-blank row passes the app's
//! checks and the engine's (`BulkBatch::can_save`); rows not checked yet
//! are checked first, and the save follows when they pass.

use std::sync::Arc;

use cl_api::{ApiError, IdempotencyKey};
use cl_core::Secret;
use cl_core::ids::AccountId;
use cl_model::CommandId;
use cl_model::forms::batch::{Blocker, BulkBatch, Outgoing, Section, SectionKind};
use cl_model::forms::{SaveIntent, SaveStart};

use super::summary::Summary;
use super::{BulkEntryScreen, KIND, grid, wire};
use crate::company::Task;

/// A save in flight on the live engine.
pub(crate) struct Saving {
    /// The batch as sent.
    pub batch: BulkBatch,
    /// Row numbers → rows.
    pub out: Outgoing,
    pub intent: SaveIntent,
    pub task: Task<cl_api::BatchSaved>,
}

impl BulkEntryScreen {
    /// Save all. True when the batch saved at once.
    pub(crate) fn save_all(&mut self, intent: SaveIntent) -> bool {
        if self.busy() {
            return false;
        }
        let lists = Arc::clone(&self.books.lists);
        if self.grid.commit_pending(&*lists).is_err() {
            return false;
        }
        // Every row's problems show now.
        let keys: Vec<_> = self.batch().filled().map(|(_, r)| r.key).collect();
        self.touched.extend(keys);
        self.cursor_row = None;
        self.refresh_looks();
        self.refused = None;
        self.notice = None;
        match self
            .batch()
            .can_save(&self.checks, self.caps(), self.books.closing)
        {
            Ok(()) => self.send_save(intent, None),
            Err(Blocker::Unchecked(_)) => {
                let before = self.saved_count;
                self.save_after_check = Some(intent);
                if self.checking.is_none() && !self.start_check(true) {
                    self.save_after_check = None;
                    let b = self
                        .batch()
                        .can_save(&self.checks, self.caps(), self.books.closing);
                    match b {
                        Ok(()) => return self.send_save(intent, None),
                        Err(b) => self.refuse_save(b),
                    }
                }
                // The demo answers at once: the save may already be done.
                self.saved_count != before
            }
            Err(b) => {
                self.refuse_save(b);
                false
            }
        }
    }

    /// The key for this content: the stored one when the same rows go
    /// again (a retry after an unknown outcome replays safely), a fresh one
    /// otherwise.
    pub(crate) fn key_for(&mut self, hash: u64) -> IdempotencyKey {
        match &self.key {
            Some((h, k)) if *h == hash => k.clone(),
            _ => {
                let k = IdempotencyKey::new();
                self.key = Some((hash, k.clone()));
                k
            }
        }
    }

    /// Sends the whole batch in one call. Rows dated on or before the
    /// closing date first ask for the closing-date password, once for all
    /// of them (Q-P15Ae); it goes on each such row and is dropped after
    /// the call.
    pub(crate) fn send_save(&mut self, intent: SaveIntent, password: Option<Secret>) -> bool {
        let closing = self.books.closing;
        let locked = !self.batch().locked_rows(closing).is_empty();
        if locked && password.is_none() {
            let supported = self.caps().closing;
            if supported {
                self.closing_prompt.ask(intent, supported, closing);
            } else {
                self.closing_prompt.unavailable(supported, closing);
            }
            return false;
        }
        let lists = Arc::clone(&self.books.lists);
        let batch = match self.grid.begin_save(intent, &*lists) {
            Ok(SaveStart::Send(v)) => v,
            Ok(SaveStart::Skip(_) | SaveStart::Busy) | Err(_) => {
                self.closing_prompt.nothing_sent();
                return false;
            }
        };
        let out = batch.outgoing(None, closing);
        let groups = match wire::groups(&out, password.as_ref()) {
            Ok(g) => g,
            Err(v) => {
                self.closing_prompt.nothing_sent();
                self.save_failed(&out, intent, ApiError::Local(v));
                return false;
            }
        };
        drop(password);
        let key = self.key_for(batch.content_hash());
        let add_names = batch.add_new_names;
        self.last_action = Some("saving");
        let link = Arc::clone(&self.link);
        if let Some(local) = self.local.as_mut() {
            let r = local.save(&link, &groups, &key, add_names);
            return match r {
                Ok(saved) => {
                    self.saved(&batch, &out, intent, &saved);
                    true
                }
                Err(e) => {
                    self.save_failed(&out, intent, e);
                    false
                }
            };
        }
        match super::engine::live::save(&self.link, self.group.as_ref(), groups, key, add_names) {
            Some(task) => {
                self.save_task = Some(Saving {
                    batch,
                    out,
                    intent,
                    task,
                });
            }
            None => self.save_failed(&out, intent, ApiError::NoCompanyOpen),
        }
        false
    }

    pub(crate) fn poll_save(&mut self) {
        let Some(mut s) = self.save_task.take() else {
            return;
        };
        match s.task.try_take() {
            None => self.save_task = Some(s),
            Some(Ok(saved)) => self.saved(&s.batch, &s.out, s.intent, &saved),
            Some(Err(e)) => self.save_failed(&s.out, s.intent, e),
        }
    }

    /// The closing-date dialog's Change: send with the password.
    pub(crate) fn submit_closing(&mut self) {
        let Some((intent, secret)) = self.closing_prompt.submit() else {
            return;
        };
        self.send_save(intent, Some(secret));
    }

    /// Everything posted: the other windows hear it (Q-262), the window
    /// shows the summary and starts a fresh batch.
    fn saved(
        &mut self,
        batch: &BulkBatch,
        out: &Outgoing,
        intent: SaveIntent,
        saved: &cl_api::BatchSaved,
    ) {
        self.closing_prompt.saved();
        let touched: Vec<AccountId> = out.touched_accounts().into_iter().collect();
        crate::forms::publish_saved(&self.link, &self.bus, &touched);
        if saved.created_contacts > 0 {
            self.link
                .bus()
                .publish_from(&self.bus, crate::data_bus::Change::Contacts);
        }
        let rows = batch.filled_count();
        self.saved_count = self
            .saved_count
            .saturating_add(u32::try_from(rows).unwrap_or(u32::MAX));
        self.summary = Some(Summary::of(batch, saved.created_contacts));
        self.summary_open = intent != SaveIntent::SaveAndNew;
        self.key = None;
        self.refused = None;
        self.error = None;
        self.notice = None;
        self.last_action = Some("save");
        let lists = Arc::clone(&self.books.lists);
        let _ = self.grid.save_ok(batch.clone(), &*lists);
        self.new_batch();
    }

    fn save_failed(&mut self, out: &Outgoing, intent: SaveIntent, e: ApiError) {
        let failure = cl_model::forms::SaveFailure {
            message: cl_core::OwnerFacing::owner_message(&e),
            detail: cl_core::OwnerFacing::scrubbed_detail(&e),
        };
        self.grid.save_failed(failure);
        self.last_action = Some("save_failed");
        let supported = self.caps().closing;
        if self
            .closing_prompt
            .failed(&e, intent, supported, self.books.closing)
        {
            return;
        }
        if !crate::screens::register::keeps_key(&e) {
            self.key = None;
        }
        match e {
            ApiError::BatchRows(rows) => {
                let batch = self.batch().clone();
                self.checks.refuse(&batch, out, wire::refused(&rows));
                let keys: Vec<_> = rows
                    .iter()
                    .filter_map(|r| out.key_of(r.row_number))
                    .collect();
                self.touched.extend(keys);
                self.refresh_looks();
                self.notice = Some("batch_entry.save.rows_refused");
                if !self.problem_rows().is_empty() {
                    self.go_to_problem(true);
                }
            }
            e if crate::screens::register::keeps_key(&e) => {
                // The engine may or may not have it: the same key replays.
                self.notice = Some("batch_entry.save.unconfirmed");
                self.error = Some(e);
            }
            e => self.error = Some(e),
        }
    }

    /// A fresh batch: one section of the first section's type and account.
    pub(crate) fn new_batch(&mut self) {
        let first = self.batch().sections.first().map_or_else(
            || Section::new(SectionKind::Checks, None),
            |s| Section::new(s.kind, s.context),
        );
        let today = self.today;
        let lists = Arc::clone(&self.books.lists);
        let a = self.grid.adapter_mut();
        let highest = a.highest.clone();
        let mut b = BulkBatch::new(first);
        grid::ensure_entry(&mut b, 0, &mut a.keys, today, &highest);
        a.selected = 0;
        a.suggested.clear();
        a.accepted.clear();
        self.grid.load(b, &*lists);
        self.checks.clear();
        self.touched.clear();
        self.cursor_row = None;
        self.changed_at = None;
        self.seen_rev = self.grid.form().revision();
        self.reset_suggestions();
        self.sync_shown_section();
        self.grid.reset_view(&*lists);
        // A save may have used check numbers.
        let banks: Vec<AccountId> = self.grid.adapter().highest.keys().copied().collect();
        self.numbers_asked.retain(|a| !banks.contains(a));
        for bank in banks {
            self.fetch_numbers(bank);
        }
        self.renumber_entry_rows();
    }

    /// Save all was refused: why, and the cursor to the first row that
    /// needs attention.
    pub(crate) fn refuse_save(&mut self, b: Blocker) {
        self.refused = Some(b);
        self.last_action = Some("save_refused");
        self.refresh_looks();
        if matches!(
            b,
            Blocker::LocalProblems(_) | Blocker::EngineProblems(_) | Blocker::NotInLists(_)
        ) && !self.problem_rows().is_empty()
        {
            self.go_to_problem(true);
        }
    }

    /// Keymap commands.
    pub(crate) fn form_command(&mut self, id: CommandId) -> bool {
        if self.grid.grid().has_focus()
            && matches!(
                id.as_str(),
                "form.revert"
                    | "edit.undo"
                    | "edit.redo"
                    | "edit.open_list"
                    | "form.insert_line"
                    | "form.delete_line"
            )
        {
            let lists = Arc::clone(&self.books.lists);
            if self.grid.command(id, &*lists) == crate::grid_form::Routed::Handled {
                return true;
            }
        }
        if self.summary_open
            && matches!(
                id.as_str(),
                "form.save_close" | "form.save_new" | "form.revert"
            )
        {
            // The summary's New batch (the batch is already fresh).
            self.summary_open = false;
            return true;
        }
        match id.as_str() {
            KIND => true,
            "form.clear" => {
                if !self.busy() {
                    self.new_batch();
                    self.refused = None;
                    self.error = None;
                    self.notice = None;
                    self.mapping = None;
                    self.last_action = Some("clear");
                }
                true
            }
            "form.save_close" => {
                self.save_all(SaveIntent::Save);
                true
            }
            "form.save_new" => {
                self.save_all(SaveIntent::SaveAndNew);
                true
            }
            "batch.next_problem" => {
                self.go_to_problem(true);
                true
            }
            "batch.prev_problem" => {
                self.go_to_problem(false);
                true
            }
            "batch.next_section" => {
                self.step_section(true);
                true
            }
            "batch.prev_section" => {
                self.step_section(false);
                true
            }
            "edit.undo" | "edit.redo" => {
                let lists = Arc::clone(&self.books.lists);
                self.grid.command(id, &*lists) == crate::grid_form::Routed::Handled
            }
            "form.revert" => {
                if self.confirm_remove.take().is_some() {
                    return true;
                }
                false
            }
            _ => false,
        }
    }
}
