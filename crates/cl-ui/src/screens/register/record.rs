//! The register screen's side of the entry row: drawing it, checking and
//! recording the draft, the next check number (Q-P12q) and the picker
//! lists. Every write goes through the request queue (or the demo engine)
//! and is followed by `after_own_write`.

use std::sync::Arc;

use cl_api::{
    ApiError, Contact, ContactFilter, IdempotencyKey, OrphanLabel, PostingOptions, RegisterQuery,
    RegisterSort, SortDir, Tag, TagFilter, TxnType,
};
use cl_core::ids::{AccountId, TransactionId};
use cl_grid::{CellPos, ColumnId, GridEvent, NoLookups, TextKey};
use cl_model::CommandId;
use cl_model::forms::{SaveIntent, SaveStart};
use cl_model::register::{
    AccountClass, EntryContext, EntryDraft, EntryField, EntryKind, EntryProblem, RangePreset,
    SortChoice, check, infer_kind, is_view_only,
};
use eframe::egui::{self, RichText, Ui};

use super::entry_row::{
    self, Built, E_ACCOUNT, E_DATE, E_NUMBER, E_PAYEE, E_PAYMENT, Editing, EntryAdapter, PLine,
    Prepared,
};
use super::lists::RegisterLists;
use super::{RegisterScreen, view::COL_DATE};
use crate::company::Task;
use crate::grid_form::{GridForm, Routed};
use crate::i18n::t;
use crate::screens::company::form;
use crate::theme::Tokens;

/// The entry row and what it is doing.
pub(super) struct EntryState {
    pub form: GridForm<EntryDraft, EntryAdapter>,
    /// The register class the grid was built for.
    pub class: AccountClass,
    /// Editing a recorded transaction (WP6 step 2), else a new one.
    pub editing: Option<Editing>,
    /// Why the last Record was refused.
    pub problems: Vec<TextKey>,
    /// The last save's failure.
    pub error: Option<ApiError>,
    /// The save in flight.
    pub task: Option<Task<TransactionId>>,
    /// The idempotency key of the draft being retried.
    pub key: Option<(EntryDraft, IdempotencyKey)>,
    /// The next check number (Q-P12q).
    pub next_check: Option<u32>,
    next_check_task: Option<Task<Option<u32>>>,
    /// The number the row filled in itself.
    pub proposed: Option<String>,
    /// Saves from this window.
    pub saved: u32,
    /// What the last save recorded (`check`, `deposit`, ..., `update`).
    pub last_saved: Option<&'static str>,
    /// Put the keyboard in this cell next frame.
    pub focus: Option<ColumnId>,
    /// Height drawn last frame.
    pub height: f32,
    contacts_task: Option<Task<Vec<Contact>>>,
    tags_task: Option<Task<Vec<Tag>>>,
    contacts: Vec<Contact>,
    pub(super) tags: Vec<Tag>,
}

impl EntryState {
    pub fn new(lists: Arc<RegisterLists>, today: cl_core::CalendarDate) -> EntryState {
        EntryState {
            form: entry_row::new_form(AccountClass::Bank, lists, today),
            class: AccountClass::Bank,
            editing: None,
            problems: Vec::new(),
            error: None,
            task: None,
            key: None,
            next_check: None,
            next_check_task: None,
            proposed: None,
            saved: 0,
            last_saved: None,
            focus: None,
            height: entry_row::HEIGHT,
            contacts_task: None,
            tags_task: None,
            contacts: Vec::new(),
            tags: Vec::new(),
        }
    }

    /// The draft.
    pub fn draft(&self) -> &EntryDraft {
        self.form.form().current()
    }

    /// The idempotency key for `draft`: the same key while the same draft
    /// is retried, a new one once it changed.
    fn key_for(&mut self, draft: &EntryDraft) -> IdempotencyKey {
        match &self.key {
            Some((d, k)) if d == draft => k.clone(),
            _ => {
                let k = IdempotencyKey::new();
                self.key = Some((draft.clone(), k.clone()));
                k
            }
        }
    }
}

/// The entry field's column.
fn column_of(f: EntryField) -> ColumnId {
    match f {
        EntryField::Date => E_DATE,
        EntryField::Payee => E_PAYEE,
        EntryField::Account | EntryField::Tag => E_ACCOUNT,
        EntryField::Amount => E_PAYMENT,
    }
}

impl RegisterScreen {
    /// Starts loading the names and tags for the pickers.
    pub(super) fn load_lists(&mut self) {
        if let Some(mut demo) = self.link.demo_state() {
            let contacts = demo.contacts_mut().clone();
            let tags = demo.tags_mut().clone();
            drop(demo);
            self.entry.contacts_task = Some(Task::ready(contacts));
            self.entry.tags_task = Some(Task::ready(tags));
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        if let Ok(p) = group.read(&session, |s| async move {
            s.contacts().list_all(&ContactFilter::default()).await
        }) {
            self.entry.contacts_task = Some(Task::Pending(p));
        }
        if let Ok(p) = group.read(&session, |s| async move {
            s.tags().list_all(&TagFilter::default()).await
        }) {
            self.entry.tags_task = Some(Task::Pending(p));
        }
    }

    /// Builds the pickers again from the chart, names and tags.
    pub(super) fn rebuild_lists(&mut self) {
        let mut lists = RegisterLists::default();
        lists.set_accounts(&self.accounts);
        lists.set_contacts(&self.entry.contacts);
        lists.set_tags(&self.entry.tags);
        self.lists = Arc::new(lists);
        self.entry.form.adapter_mut().lists = Arc::clone(&self.lists);
        self.entry_lists_changed();
    }

    /// The entry row for the register shown: its amount headings follow the
    /// account's class. Called when the account changes.
    pub(super) fn reset_entry(&mut self) {
        let date = self.entry.draft().date.unwrap_or(self.today);
        self.entry.form = entry_row::new_form(self.class, Arc::clone(&self.lists), date);
        self.entry.class = self.class;
        self.entry.editing = None;
        self.entry.problems.clear();
        self.entry.error = None;
        self.entry.proposed = None;
        self.entry.next_check = None;
        self.entry.key = None;
        self.close_splits();
        self.fetch_next_check();
    }

    /// The engine's entry kinds for this register as the model's tags.
    pub(super) fn allowed_tags(&self) -> Vec<cl_model::register::EntryKindTag> {
        entry_row::allowed_tags(&self.allowed)
    }

    /// True when the register takes no entries at all.
    pub(super) fn view_only(&self) -> bool {
        self.model.total().is_some() && is_view_only(&self.allowed_tags())
    }

    /// The kind the draft would become now (none until the account and the
    /// amount are in). Editing keeps the transaction's own kind.
    pub(super) fn draft_kind(&self) -> Option<EntryKind> {
        if let Some(e) = &self.entry.editing {
            return Some(e.kind);
        }
        let d = self.entry.draft();
        let side = d.side()?;
        let other = self.splits_first_account().or(d.account)?;
        infer_kind(
            self.class,
            self.lists.class(other),
            side,
            &self.allowed_tags(),
        )
        .ok()
    }

    /// Reads the next check number (Q-P12q): the highest check number in
    /// the account plus one, from the engine.
    pub(super) fn fetch_next_check(&mut self) {
        let Some(account) = self.account else { return };
        if self.class != AccountClass::Bank {
            return;
        }
        if let Some(mut demo) = self.link.demo_state() {
            let chart = demo
                .chart
                .get_or_insert_with(crate::company::demo::chart)
                .clone();
            let n = demo.ledger.next_check_number(&chart, account);
            self.entry.next_check_task = Some(Task::ready(n));
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        let range = RangePreset::AllDates.resolve(self.today, &self.fiscal);
        let mut q = RegisterQuery::new(range.start, range.end);
        q.txn_type = Some(TxnType::Expense);
        q.sort = RegisterSort::Number;
        q.dir = SortDir::Desc;
        if let Ok(p) = group.read(&session, move |s| async move {
            let page = s.register().page(account, &q, 1, 1).await?;
            Ok(page
                .lines
                .first()
                .and_then(|l| l.check_number)
                .and_then(|n| u32::try_from(n).ok())
                .map(|n| n.saturating_add(1)))
        }) {
            self.entry.next_check_task = Some(Task::Pending(p));
        }
    }

    /// Polls the entry row's tasks.
    pub(super) fn poll_entry(&mut self) {
        let mut lists_changed = false;
        if let Some(r) = self.entry.contacts_task.as_mut().and_then(Task::try_take) {
            self.entry.contacts_task = None;
            if let Ok(c) = r {
                self.entry.contacts = c;
                lists_changed = true;
            }
        }
        if let Some(r) = self.entry.tags_task.as_mut().and_then(Task::try_take) {
            self.entry.tags_task = None;
            if let Ok(tags) = r {
                self.entry.tags = tags;
                lists_changed = true;
            }
        }
        if lists_changed {
            self.rebuild_lists();
        }
        if let Some(r) = self.entry.next_check_task.as_mut().and_then(Task::try_take) {
            self.entry.next_check_task = None;
            self.entry.next_check = r.ok().flatten();
        }
        if let Some(r) = self.entry.task.as_mut().and_then(Task::try_take) {
            self.entry.task = None;
            match r {
                Ok(id) => self.record_done(id),
                Err(e) => self.record_failed(e),
            }
        }
    }

    /// The Number cell: the next check number goes in when the row becomes
    /// a check and the owner hasn't typed one; it goes again when the row
    /// stops being a check.
    pub(super) fn sync_proposal(&mut self) {
        if self.entry.editing.is_some() {
            return;
        }
        let kind = self.draft_kind();
        let number = self.entry.draft().number.clone();
        if let Some(p) = &self.entry.proposed
            && *p != number
        {
            // The owner changed it: theirs now.
            self.entry.proposed = None;
        }
        if kind == Some(EntryKind::Check) {
            if number.trim().is_empty()
                && let Some(n) = self.entry.next_check
            {
                let text = n.to_string();
                let v = text.clone();
                if self.entry.form.edit_step(|d| d.number = v) {
                    self.entry.proposed = Some(text);
                }
            }
        } else if self.entry.proposed.is_some() && self.entry.form.edit_step(|d| d.number.clear()) {
            self.entry.proposed = None;
        }
    }

    /// Checks the draft and starts the save (Enter, Record, Alt+N, Alt+S).
    /// Returns true when there was nothing to save or it saved at once.
    pub(super) fn start_record(&mut self) -> bool {
        if self.entry.task.is_some() || self.view_only() {
            return false;
        }
        let Some(own) = self.account else {
            return false;
        };
        let lists = Arc::clone(&self.lists);
        if self.entry.form.commit_pending(&*lists).is_err() || !self.splits_commit() {
            return false;
        }
        let draft = self.entry.draft().clone();
        if draft.is_empty() && self.entry.editing.is_none() {
            return true;
        }
        self.entry.problems.clear();
        self.entry.error = None;
        let prepared = match self.prepare(own, &draft) {
            Ok(p) => p,
            Err((keys, col)) => {
                self.entry.problems = keys;
                self.entry.focus = col;
                return false;
            }
        };
        let update = self.entry.editing.clone();
        let built = match prepared.build(update.is_some()) {
            Ok(b) => b,
            Err(v) => {
                self.entry.problems = vec![entry_row::refusal_key(v)];
                return false;
            }
        };
        match self.entry.form.begin_save(SaveIntent::SaveAndNew, &*lists) {
            Ok(SaveStart::Send(_)) => {}
            Ok(SaveStart::Skip(_)) => {
                // Nothing changed: an edit simply ends.
                if update.is_some() && !self.splits_dirty() {
                    self.finish_edit();
                    return true;
                }
            }
            Ok(SaveStart::Busy) | Err(_) => return false,
        }
        if self.link.is_demo() {
            let saved = self.demo_save(&prepared, update.as_ref().map(|e| e.id));
            return match saved {
                Ok(id) => {
                    self.record_done(id);
                    true
                }
                Err(e) => {
                    self.record_failed(e);
                    false
                }
            };
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return false;
        };
        let key = self.entry.key_for(&draft);
        let label = OrphanLabel {
            what: "orphan.register_save",
            key: Some(key.clone()),
        };
        let edit = update.map(|e| (e.id, e.version));
        let pending = group.write(&session, label, move |s| async move {
            let opts = PostingOptions::default();
            let t = s.transactions();
            let saved = match (built, edit) {
                (Built::Bank(p), None) => t.create_bank_posting(&p, &opts, key).await?,
                (Built::Bank(p), Some((id, v))) => {
                    t.update_bank_posting(id, &v, &p, &opts, key).await?
                }
                (Built::Journal(j), None) => t.create_journal_entry(&j, &opts, key).await?,
                (Built::Journal(j), Some((id, v))) => {
                    t.update_journal_entry(id, &v, &j, &opts, key).await?
                }
            };
            Ok(saved.record.id)
        });
        match pending {
            Ok(p) => self.entry.task = Some(Task::Pending(p)),
            Err(_) => self.record_failed(ApiError::Local(cl_api::LocalValidation::InvalidValue)),
        }
        false
    }

    fn demo_save(
        &self,
        p: &Prepared,
        existing: Option<TransactionId>,
    ) -> Result<TransactionId, ApiError> {
        let mut demo = self
            .link
            .demo_state()
            .ok_or(ApiError::Local(cl_api::LocalValidation::InvalidValue))?;
        let chart = demo
            .chart
            .get_or_insert_with(crate::company::demo::chart)
            .clone();
        demo.ledger
            .save(&chart, &p.demo(), existing)
            .ok_or(ApiError::Local(cl_api::LocalValidation::InvalidValue))
    }

    /// The draft (and the Splits lines) as a posting, or the problems and
    /// the column to put the keyboard in.
    fn prepare(
        &self,
        own: AccountId,
        draft: &EntryDraft,
    ) -> Result<Prepared, (Vec<TextKey>, Option<ColumnId>)> {
        let allowed = self.allowed_tags();
        let mut d = draft.clone();
        if let Some(first) = self.splits_first_account() {
            d.account = Some(first);
        }
        let ctx = EntryContext {
            this_account: own,
            this_class: self.class,
            other_class: d.account.and_then(|a| self.lists.class(a)),
            allowed: &allowed,
        };
        let editing = self.entry.editing.clone();
        let checked = match check(&d, &ctx) {
            Ok(c) => c,
            Err(problems) => {
                let problems: Vec<EntryProblem> = problems
                    .into_iter()
                    .filter(|p| editing.is_none() || *p != EntryProblem::KindNotAllowed)
                    .collect();
                if !problems.is_empty() || editing.is_none() {
                    let col = problems.first().map(|p| column_of(p.field()));
                    return Err((problems.iter().map(|p| p.key()).collect(), col));
                }
                // Only the kind was refused, and an edit keeps its kind.
                let mut relaxed = allowed.clone();
                relaxed.extend([
                    cl_model::register::EntryKindTag::Expense,
                    cl_model::register::EntryKindTag::Deposit,
                    cl_model::register::EntryKindTag::Transfer,
                    cl_model::register::EntryKindTag::JournalEntry,
                ]);
                let ctx = EntryContext {
                    allowed: &relaxed,
                    ..ctx
                };
                check(&d, &ctx).map_err(|p| {
                    let col = p.first().map(|p| column_of(p.field()));
                    (p.iter().map(|p| p.key()).collect::<Vec<_>>(), col)
                })?
            }
        };
        let mut c = checked;
        let mut problems: Vec<(TextKey, ColumnId)> = Vec::new();
        if let Some(e) = &editing {
            if c.side != e.side {
                problems.push((TextKey("register.entry.problem.side_changed"), E_PAYMENT));
            }
            c.kind = e.kind;
        }
        let lines = match self.splits_lines(c.amount) {
            Some(Ok(lines)) => lines,
            Some(Err(key)) => return Err((vec![key], None)),
            None => vec![PLine {
                account: c.account,
                amount: c.amount,
                memo: self.edit_line_memo(),
                contact: (c.kind == EntryKind::Deposit).then_some(c.payee).flatten(),
                tag: self.edit_line_tag(),
            }],
        };
        let mut p = Prepared::new(own, &c, lines);
        p.payee_name = c
            .payee
            .and_then(|id| self.lists.name_text(id))
            .map(str::to_string);
        if let Some(e) = &editing {
            p.basis = e.basis;
            p.own_line = e.own_line.clone();
            if c.kind != EntryKind::CardCharge {
                p.number = None;
            }
        }
        match c.kind {
            EntryKind::Check if editing.is_none() => {
                if p.payee_name.is_none() {
                    problems.push((TextKey("register.entry.problem.check_payee"), E_PAYEE));
                }
                if p.number
                    .as_deref()
                    .is_some_and(|n| n.parse::<u32>().map_or(true, |n| n == 0))
                {
                    problems.push((TextKey("register.entry.problem.check_number"), E_NUMBER));
                }
            }
            EntryKind::Deposit | EntryKind::Transfer | EntryKind::JournalEntry
                if editing.is_none() && p.number.is_some() =>
            {
                problems.push((TextKey("register.entry.problem.no_number"), E_NUMBER));
            }
            _ => {}
        }
        if matches!(c.kind, EntryKind::Transfer | EntryKind::JournalEntry) && c.payee.is_some() {
            let key = if c.kind == EntryKind::JournalEntry
                && (self.class.needs_name()
                    || self
                        .lists
                        .class(c.account)
                        .is_some_and(AccountClass::needs_name))
            {
                // Q-P12p: the engine's journal lines can't carry a name yet.
                TextKey("register.entry.problem.je_name")
            } else {
                TextKey("register.entry.problem.no_name")
            };
            problems.push((key, E_PAYEE));
        }
        if c.kind == EntryKind::Transfer && p.lines.len() != 1 {
            problems.push((TextKey("register.splits.transfer"), E_ACCOUNT));
        }
        if matches!(
            c.kind,
            EntryKind::Check | EntryKind::CardCharge | EntryKind::JournalEntry
        ) && p.lines.iter().any(|l| l.contact.is_some())
        {
            problems.push((TextKey("register.splits.customer_deposits_only"), E_ACCOUNT));
        }
        if problems.is_empty() {
            Ok(p)
        } else {
            let col = problems.first().map(|(_, c)| *c);
            Err((problems.into_iter().map(|(k, _)| k).collect(), col))
        }
    }

    fn record_done(&mut self, _id: TransactionId) {
        let lists = Arc::clone(&self.lists);
        let stored = self.entry.draft().clone();
        self.entry.form.save_ok(stored, &*lists);
        self.entry.saved += 1;
        self.entry.last_saved = Some(match (&self.entry.editing, self.draft_kind()) {
            (Some(_), _) => "update",
            (None, Some(k)) => entry_row::kind_name(k),
            (None, None) => "saved",
        });
        self.entry.key = None;
        self.entry.problems.clear();
        self.entry.error = None;
        self.finish_edit();
        self.after_own_write();
        // Date order, oldest first: a new line is at (or near) the end.
        if self.prefs.sort == SortChoice::DateOldest {
            self.jump_to_end = true;
        }
        self.fetch_next_check();
    }

    fn record_failed(&mut self, e: ApiError) {
        self.entry.form.save_failed(cl_model::forms::SaveFailure {
            message: cl_core::OwnerFacing::owner_message(&e),
            detail: cl_core::OwnerFacing::scrubbed_detail(&e),
        });
        self.entry.error = Some(e);
    }

    /// Back to an empty new row, keeping the date.
    pub(super) fn finish_edit(&mut self) {
        let lists = Arc::clone(&self.lists);
        let mut fresh = self.entry.draft().clone();
        fresh.clear_keep_date();
        if fresh.date.is_none() {
            fresh.date = Some(self.today);
        }
        self.entry.form.load(fresh, &*lists);
        self.entry.editing = None;
        self.entry.proposed = None;
        self.edit_lines.clear();
        self.read_only = None;
        let a = self.entry.form.adapter_mut();
        a.lock_number = false;
        a.label = t("register.entry.row_new");
        self.close_splits();
        self.entry.focus = Some(E_DATE);
    }

    /// Puts the keyboard in the register grid (Shift+Tab from the row).
    pub(super) fn focus_register(&mut self) {
        let key = self.grid.core().cursor().map(|c| c.row).or_else(|| {
            let total = self.model.total()?;
            let last = usize::try_from(total.checked_sub(1)?).ok()?;
            cl_grid::RowModel::row_key(&self.model, last)
        });
        let Some(key) = key else { return };
        let label = String::new();
        let env = self.parts.env(
            &NoLookups,
            &label,
            cl_grid::GridHeight::Rows { min: 1, max: 1 },
        );
        self.grid.focus(
            CellPos {
                row: key,
                col: COL_DATE,
            },
            cl_grid::ModelRef::Browse(&mut self.model),
            &env,
        );
    }

    /// Commands for the entry row while it has the keyboard (Esc, undo,
    /// Ctrl+L). `None` when the row doesn't have it.
    pub(super) fn entry_command(&mut self, id: CommandId) -> Option<bool> {
        if let Some(handled) = self.splits_command(id) {
            return Some(handled);
        }
        if !self.entry.form.grid().has_focus() {
            return None;
        }
        if !matches!(
            id.as_str(),
            "form.revert" | "edit.undo" | "edit.redo" | "edit.open_list"
        ) {
            return None;
        }
        let lists = Arc::clone(&self.lists);
        match self.entry.form.command(id, &*lists) {
            Routed::Handled => Some(true),
            Routed::NotHandled if id.as_str() == "form.revert" && self.entry.editing.is_some() => {
                // Esc on an unchanged edit row: the edit ends.
                self.finish_edit();
                Some(true)
            }
            _ => None,
        }
    }

    /// The draft or Splits have unsaved edits.
    pub(super) fn entry_dirty(&self) -> bool {
        self.entry.form.dirty() || self.splits_dirty()
    }

    /// Draws the entry row (warn tint, accent top border).
    pub(super) fn entry_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let top = ui.cursor().top();
        let frame = egui::Frame::new()
            .fill(tokens.warn_tint)
            .inner_margin(egui::Margin::symmetric(12, 8));
        let r = frame.show(ui, |ui| {
            ui.set_width(ui.available_width());
            if self.view_only() {
                ui.label(RichText::new(t("register.entry.view_only")).color(tokens.text_secondary));
                return;
            }
            if self.entry.editing.is_some() {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(t("register.entry.editing"))
                            .strong()
                            .color(tokens.text_primary),
                    );
                    if ui.button(t("register.entry.cancel_edit")).clicked() {
                        self.finish_edit();
                    }
                });
            }
            self.entry_grid_ui(ui);
            self.entry_actions_ui(ui, tokens);
            self.splits_ui(ui, tokens);
            self.edit_ui(ui, tokens);
            for key in &self.entry.problems {
                ui.add(egui::Label::new(RichText::new(t(key.0)).color(tokens.warn_text)).wrap());
            }
            if let Some(e) = &self.entry.error {
                form::inline_error(ui, tokens, e);
            }
        });
        ui.painter().hline(
            r.response.rect.x_range(),
            top,
            egui::Stroke::new(2.0, tokens.accent),
        );
        self.entry.height = r.response.rect.height();
    }

    fn entry_grid_ui(&mut self, ui: &mut Ui) {
        let lists = Arc::clone(&self.lists);
        let out = self.entry.form.ui(ui, &*lists);
        if let Some(col) = self.entry.focus.take() {
            let key = self.entry.form.adapter().key;
            self.entry.form.focus(CellPos { row: key, col }, &*lists);
        }
        for e in out.events {
            match e {
                GridEvent::RowRecorded { .. } => {
                    self.start_record();
                }
                GridEvent::CellCommitted { .. } => {
                    self.entry.problems.clear();
                    self.sync_proposal();
                }
                GridEvent::LeftGrid { forward: false } => self.focus_register(),
                _ => {}
            }
        }
    }

    fn entry_actions_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        ui.horizontal(|ui| {
            if let Some(k) = self.draft_kind() {
                egui::Frame::new()
                    .fill(tokens.neutral_chip)
                    .corner_radius(egui::CornerRadius::same(9))
                    .inner_margin(egui::Margin::symmetric(8, 2))
                    .show(ui, |ui| {
                        ui.add(
                            egui::Label::new(
                                RichText::new(t(k.label().0))
                                    .size(12.0)
                                    .color(tokens.neutral_chip_text),
                            )
                            .selectable(false),
                        );
                    });
            }
            if self.entry.task.is_some() {
                ui.add(egui::Spinner::new().size(14.0));
                ui.label(RichText::new(t("register.entry.saving")).color(tokens.text_secondary));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let record = ui.add(
                    egui::Button::new(
                        RichText::new(t("register.entry.record"))
                            .color(tokens.on_accent)
                            .strong(),
                    )
                    .fill(tokens.accent)
                    .min_size(egui::vec2(88.0, 26.0)),
                );
                if record.clicked() {
                    self.start_record();
                }
                crate::widgets::kbd(ui, "Enter");
                if ui.button(t("register.entry.splits")).clicked() {
                    self.toggle_splits();
                }
            });
        });
    }

    /// The entry row's part of the control-channel state (no amounts,
    /// names or memo text).
    pub(super) fn entry_state(&self) -> serde_json::Value {
        let d = self.entry.draft();
        serde_json::json!({
            "mode": if self.entry.editing.is_some() { "edit" } else { "new" },
            "view_only": self.view_only(),
            "kind": self.draft_kind().map(entry_row::kind_name),
            "empty": d.is_empty(),
            "has_number": !d.number.trim().is_empty(),
            "number_proposed": self.entry.proposed.is_some(),
            "focused": self.entry.form.grid().has_focus(),
            "saving": self.entry.task.is_some(),
            "saved_count": self.entry.saved,
            "last_saved": self.entry.last_saved,
            "problems": self.entry.problems.len(),
            "error": self.entry.error.is_some(),
            "dirty": self.entry_dirty(),
            "splits": self.splits_state(),
            "read_only": self.read_only.is_some(),
            "edit_loading": self.edit_load.is_some(),
        })
    }
}
