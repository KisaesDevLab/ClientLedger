//! Bulk Entry (P15A, `banking.bulk_entry`; Banking menu and the palette, no
//! global shortcut, Q-P15Ah), on the Write Check pattern with no mockup
//! (Q-P15An).
//!
//! One window holds sections (one type and account each) shown one at a
//! time in a single grid; Save all posts every section in one atomic engine
//! call (Q-P15Aa). See `target/plans/P15A.md`.
//!
//! - The section strip: a chip per section with its row and problem
//!   counts, Add section and Remove section; Ctrl+PageUp / Ctrl+PageDown
//!   move between sections ([`sections`]).
//! - The grid: the section's columns on the P10 line grid ([`grid`]).
//! - Checks ([`validate`]): the app's own exact checks at once, then the
//!   engine's Validate a moment after a row is left; a row's engine answer
//!   counts only while the row is unchanged. Problems tint the row and
//!   caption it; F8 / Shift+F8 go to the next / previous one. **Save all
//!   is off until every row passes both.**
//!
//! Money is exact (`cl_core::Money`, cents per Q-232). The totals are sums
//! of the owner's own amounts; balances are the engine's, shown in the
//! register (non-negotiable 7).

mod draw;
mod engine;
mod grid;
mod paste;
mod save;
mod sections;
mod suggest;
mod summary;
mod validate;
mod wire;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Instant;

use cl_api::{BatchCaps, RequestGroup};
use cl_core::ids::{AccountId, TenantId};
use cl_core::{CalendarDate, MoneyStyle};
use cl_grid::{Grid, RowKey, RowKeyGen};
use cl_model::forms::FormState;
use cl_model::forms::batch::{BulkBatch, EngineChecks, Outgoing, SaveCaps, Section, SectionKind};
use cl_model::{CommandId, Scope};
use eframe::egui::Ui;
use serde_json::{Value, json};

use crate::company::{CompanyLink, Task};
use crate::forms::closing_prompt::ClosingPrompt;
use crate::grid_form::GridForm;
use crate::i18n::t;
use crate::screens::card_charge::kit::Books;
use crate::windows::Screen;

pub(crate) use engine::{BatchEngine, DemoBatchEngine};
use grid::BulkAdapter;

/// The command and window kind.
pub const KIND: &str = "banking.bulk_entry";

/// The Bulk Entry window for `kind`, if it is one.
#[must_use]
pub fn make(
    kind: CommandId,
    link: &Arc<CompanyLink>,
    open: Option<(TenantId, String)>,
    today: CalendarDate,
) -> Option<Box<dyn Screen>> {
    (kind.as_str() == KIND)
        .then(|| Box::new(BulkEntryScreen::new(Arc::clone(link), open, today)) as Box<_>)
}

/// A validate call in flight.
pub(crate) struct Checking {
    /// What was sent (row numbers → rows and hashes).
    pub out: Outgoing,
    /// The live engine's answer.
    pub task: Task<cl_api::BatchValidation>,
    /// The screen's `check_epoch` when sent: an answer from an older epoch
    /// (the lists or the closing date changed since) is dropped.
    pub epoch: u64,
}

/// The Bulk Entry window.
pub struct BulkEntryScreen {
    pub(crate) link: Arc<CompanyLink>,
    pub(crate) tenant: Option<(TenantId, String)>,
    pub(crate) group: Option<RequestGroup>,
    pub(crate) today: CalendarDate,
    /// The demo engine or a test double (`None`: the live engine).
    pub(crate) local: Option<Box<dyn BatchEngine>>,
    pub(crate) books: Books,
    /// The sections and their rows (undo covers them) and the grid.
    pub(crate) grid: GridForm<BulkBatch, BulkAdapter>,
    /// The section type the grid's columns are for.
    pub(crate) shown_kind: SectionKind,
    /// Grids of the other section types, kept for their widths.
    pub(crate) spare: BTreeMap<SectionKind, Grid>,
    /// The engine's answers per row (outside undo).
    pub(crate) checks: EngineChecks,
    pub(crate) checking: Option<Checking>,
    /// Goes up whenever the engine's answers stop holding (new lists, a
    /// new closing date).
    pub(crate) check_epoch: u64,
    /// Rows the cursor has left (their problems show).
    pub(crate) touched: BTreeSet<RowKey>,
    /// The cursor's row last frame.
    pub(crate) cursor_row: Option<RowKey>,
    /// The batch revision last seen, and when it changed (auto-validate
    /// waits half a second after the last change).
    pub(crate) seen_rev: u64,
    pub(crate) changed_at: Option<Instant>,
    /// The highest check number in the books, per bank account, as read.
    pub(crate) numbers_task: Option<(AccountId, Task<Option<u64>>)>,
    pub(crate) numbers_asked: BTreeSet<AccountId>,
    /// A section to remove once the owner confirms (it has rows).
    pub(crate) confirm_remove: Option<usize>,
    /// Shown under the grid (i18n key).
    pub(crate) notice: Option<&'static str>,
    pub(crate) error: Option<cl_api::ApiError>,
    /// A section the engine refused as a whole (its index and why).
    pub(crate) section_refused: Option<(usize, cl_api::BatchGroupProblem)>,
    /// The batch revision when it was refused (an edit clears it).
    pub(crate) section_refused_rev: u64,
    /// The status bar's "rows need attention" line is up.
    pub(crate) attention_toast: bool,
    pub(crate) saved_count: u32,
    pub(crate) last_action: Option<&'static str>,
    pub(crate) money_style: MoneyStyle,
    pub(crate) bus: crate::data_bus::Subscription,
    pub(crate) lists_stale: bool,
    pub(crate) after_frame: Option<CommandId>,
    /// The date order typed dates and pasted dates are read in.
    pub(crate) date_order: cl_model::date_entry::DateOrder,
    // ------------------------------------------------ paste and import (D2)
    pub(crate) mapping: Option<paste::MappingPanel>,
    pub(crate) import_open: bool,
    pub(crate) import_path: String,
    pub(crate) import_error: Option<String>,
    pub(crate) parse_task: Option<Task<cl_api::ParsedRows>>,
    /// Rows the last paste or import added (control channel).
    pub(crate) pasted_rows: usize,
    pub(crate) suggest: suggest::BatchSuggest,
    // ------------------------------------------------------------ save (D3)
    /// Why the last Save all was refused.
    pub(crate) refused: Option<cl_model::forms::batch::Blocker>,
    /// Save all waits for the rows being checked.
    pub(crate) save_after_check: Option<cl_model::forms::SaveIntent>,
    pub(crate) save_task: Option<save::Saving>,
    /// The idempotency key for a batch content hash (kept for retries).
    pub(crate) key: Option<(u64, cl_api::IdempotencyKey)>,
    pub(crate) closing_prompt: ClosingPrompt<cl_model::forms::SaveIntent>,
    pub(crate) summary: Option<summary::Summary>,
    pub(crate) summary_open: bool,
}

/// Half a second after the last change, the changed rows go to the engine.
pub(crate) const CHECK_AFTER: std::time::Duration = std::time::Duration::from_millis(500);

impl BulkEntryScreen {
    /// A new, empty batch: one Checks section on the first bank account.
    #[must_use]
    pub fn new(
        link: Arc<CompanyLink>,
        tenant: Option<(TenantId, String)>,
        today: CalendarDate,
    ) -> BulkEntryScreen {
        let local: Option<Box<dyn BatchEngine>> = link
            .is_demo()
            .then(|| Box::new(DemoBatchEngine) as Box<dyn BatchEngine>);
        BulkEntryScreen::with_local(link, tenant, today, local)
    }

    pub(crate) fn with_local(
        link: Arc<CompanyLink>,
        tenant: Option<(TenantId, String)>,
        today: CalendarDate,
        local: Option<Box<dyn BatchEngine>>,
    ) -> BulkEntryScreen {
        let group = link.group().and_then(Result::ok);
        let books = Books::default();
        let kind = SectionKind::Checks;
        let mut keys = RowKeyGen::new();
        let mut batch = BulkBatch::new(Section::new(kind, None));
        grid::ensure_entry(&mut batch, 0, &mut keys, today, &BTreeMap::new());
        let adapter = BulkAdapter::new(Arc::clone(&books.lists), keys, today);
        // The specs are fixed; `every_section_spec_is_valid_with_its_own_headers`
        // covers every one.
        let g =
            Grid::new(grid::grid_id(kind), grid::spec(kind)).expect("the Bulk Entry spec is valid");
        let form = GridForm::new(
            FormState::new(batch),
            adapter,
            g,
            sections::grid_label(kind),
            cl_grid::GridHeight::Fill {
                reserve_below: 150.0,
                min_rows: 6,
            },
        )
        .with_widths_key(format!("{KIND}/{}", grid::grid_id(kind)));
        let mut s = BulkEntryScreen {
            bus: link.bus().subscribe(),
            link,
            tenant,
            group,
            today,
            local,
            books,
            grid: form,
            shown_kind: kind,
            spare: BTreeMap::new(),
            checks: EngineChecks::new(),
            checking: None,
            check_epoch: 0,
            touched: BTreeSet::new(),
            cursor_row: None,
            seen_rev: 0,
            changed_at: None,
            numbers_task: None,
            numbers_asked: BTreeSet::new(),
            confirm_remove: None,
            notice: None,
            error: None,
            section_refused: None,
            section_refused_rev: 0,
            attention_toast: false,
            saved_count: 0,
            last_action: None,
            money_style: MoneyStyle::default(),
            lists_stale: false,
            after_frame: None,
            date_order: cl_model::date_entry::DateOrder::Mdy,
            mapping: None,
            import_open: false,
            import_path: String::new(),
            import_error: None,
            parse_task: None,
            pasted_rows: 0,
            suggest: suggest::BatchSuggest::default(),
            refused: None,
            save_after_check: None,
            save_task: None,
            key: None,
            closing_prompt: ClosingPrompt::default(),
            summary: None,
            summary_open: false,
        };
        s.seen_rev = s.grid.form().revision();
        s.books.start(&s.link, s.group.as_ref());
        s.poll();
        s
    }

    /// The batch.
    #[must_use]
    pub(crate) fn batch(&self) -> &BulkBatch {
        self.grid.form().current()
    }

    /// The section the grid shows.
    #[must_use]
    pub(crate) fn selected(&self) -> usize {
        self.grid.adapter().selected
    }

    /// The engine's batch capabilities (the demo has them all).
    #[must_use]
    pub(crate) fn caps(&self) -> SaveCaps {
        if self.local.is_some() {
            SaveCaps::ALL
        } else {
            let mut c = wire::save_caps(BatchCaps::of(&self.link.capabilities()));
            // The live grouped save needs `batch-mixed-v1` even for one
            // section (lane A): without it, Save all is off.
            c.atomic &= c.mixed;
            c
        }
    }

    /// Unsaved rows.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.grid.dirty() && self.batch().filled_count() > 0
    }

    /// A save is running.
    pub(crate) fn busy(&self) -> bool {
        self.grid.form().is_saving() || self.save_task.is_some()
    }

    pub(crate) fn poll(&mut self) {
        let closing = self.books.closing;
        let lists_changed = self.books.poll();
        self.books_arrived(lists_changed, closing);
        self.poll_numbers();
        self.poll_checking();
        self.poll_save();
        self.poll_parse();
        self.maybe_suggest();
        self.poll_suggest();
    }

    /// The lists (`lists_changed`) or the closing date (it was
    /// `closing_before`) were read again.
    pub(crate) fn books_arrived(
        &mut self,
        lists_changed: bool,
        closing_before: Option<CalendarDate>,
    ) {
        if lists_changed {
            self.grid.adapter_mut().lists = Arc::clone(&self.books.lists);
            self.default_contexts();
        }
        if lists_changed || self.books.closing != closing_before {
            // A name, account or tag the engine resolved may be gone, and
            // the closed-period rows may differ: ask again.
            self.forget_engine_results();
        }
    }

    /// Sections still without an account get the first one of their kind,
    /// as part of the blank window (no dirty mark when nothing is typed).
    fn default_contexts(&mut self) {
        let missing: Vec<(usize, AccountId)> = self
            .batch()
            .sections
            .iter()
            .enumerate()
            .filter(|(_, s)| s.context.is_none())
            .filter_map(|(i, s)| {
                let class = s.kind.needs_context()?;
                sections::accounts_for(&self.books, class)
                    .first()
                    .map(|(a, _)| (i, *a))
            })
            .collect();
        if missing.is_empty() {
            return;
        }
        let mut b = self.batch().clone();
        for (i, a) in &missing {
            b.sections[*i].context = Some(*a);
        }
        if self.grid.form().is_dirty() {
            self.grid.edit_step(|v| *v = b);
        } else {
            let lists = Arc::clone(&self.books.lists);
            self.grid.load(b, &*lists);
        }
        for (_, a) in missing {
            self.fetch_numbers(a);
        }
        self.renumber_entry_rows();
    }

    /// Saves in other windows (Q-262): changed lists are read again once
    /// the batch is clean; a ledger change may use a check number.
    pub(crate) fn hear_changes(&mut self) {
        let changes = self.link.bus().take(&mut self.bus);
        if changes.lists() {
            self.lists_stale = true;
        }
        if self.lists_stale && !self.is_dirty() && !self.books.busy() {
            self.lists_stale = false;
            self.books.start(&self.link, self.group.as_ref());
        }
        let banks: Vec<AccountId> = self
            .batch()
            .sections
            .iter()
            .filter(|s| s.kind == SectionKind::Checks)
            .filter_map(|s| s.context)
            .filter(|a| changes.ledger_touches(*a))
            .collect();
        for a in banks {
            self.numbers_asked.remove(&a);
            self.fetch_numbers(a);
        }
    }

    /// Reads the highest check number in the books for `bank` (Q-242).
    pub(crate) fn fetch_numbers(&mut self, bank: AccountId) {
        if self.numbers_asked.contains(&bank) || self.numbers_task.is_some() {
            return;
        }
        self.numbers_asked.insert(bank);
        let demo = self
            .link
            .demo_state()
            .map(|mut d| crate::company::demo::checks::numbers(&mut d, bank));
        if let Some((next, highest)) = demo {
            self.numbers_task = Some((bank, Task::ready(highest_of(next, highest))));
            self.poll_numbers();
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        if let Ok(p) = group.read(&session, move |s| async move {
            let next = s
                .checks()
                .settings()
                .await
                .ok()
                .and_then(|c| c.next_for(bank));
            let mut highest: Option<u32> = None;
            for kind in [cl_api::TxnType::Expense, cl_api::TxnType::BillPayment] {
                let mut q = cl_api::RegisterQuery::new(CalendarDate::min(), CalendarDate::max());
                q.txn_type = Some(kind);
                q.sort = cl_api::RegisterSort::Number;
                q.dir = cl_api::SortDir::Desc;
                let top = s
                    .register()
                    .page(bank, &q, 1, 1)
                    .await
                    .ok()
                    .and_then(|page| page.lines.first().and_then(|l| l.check_number))
                    .and_then(|n| u32::try_from(n).ok());
                highest = highest.max(top);
            }
            Ok(highest_of(next, highest))
        }) {
            self.numbers_task = Some((bank, Task::Pending(p)));
        } else {
            self.numbers_asked.remove(&bank);
        }
    }

    fn poll_numbers(&mut self) {
        let Some((bank, mut task)) = self.numbers_task.take() else {
            return;
        };
        match task.try_take() {
            None => self.numbers_task = Some((bank, task)),
            Some(r) => {
                if let Ok(Some(n)) = r {
                    self.grid.adapter_mut().highest.insert(bank, n);
                    self.renumber_entry_rows();
                }
                // Another bank account may be waiting.
                let waiting: Vec<AccountId> = self
                    .batch()
                    .sections
                    .iter()
                    .filter(|s| s.kind == SectionKind::Checks)
                    .filter_map(|s| s.context)
                    .filter(|a| !self.numbers_asked.contains(a))
                    .collect();
                if let Some(a) = waiting.first() {
                    self.fetch_numbers(*a);
                }
            }
        }
    }

    /// The blank entry rows of Checks sections take the next number
    /// (Q-242). Part of the blank window: it adds no undo step and no
    /// dirty mark when nothing is typed.
    pub(crate) fn renumber_entry_rows(&mut self) {
        let highest = self.grid.adapter().highest.clone();
        let mut b = self.batch().clone();
        let mut changed = false;
        for i in 0..b.sections.len() {
            let s = &b.sections[i];
            let (SectionKind::Checks, Some(bank)) = (s.kind, s.context) else {
                continue;
            };
            let Some(last) = s.rows.last().filter(|r| r.is_blank() && !r.print_later) else {
                continue;
            };
            let key = last.key;
            let mut probe = b.clone();
            if let Some(r) = probe.sections[i].rows.last_mut() {
                r.number.clear();
            }
            let n = probe
                .next_check_number(bank, highest.get(&bank).copied())
                .unwrap_or_default();
            if let Some(r) = b.sections[i].rows.iter_mut().find(|r| r.key == key)
                && r.number != n
            {
                r.number = n;
                changed = true;
            }
        }
        if !changed {
            return;
        }
        let lists = Arc::clone(&self.books.lists);
        if self.grid.form().is_dirty() {
            if !self.busy() {
                // Merges with the last typing in the batch, not its own
                // undo step.
                self.grid.edit(u64::MAX, |v| *v = b);
            }
        } else {
            self.grid.load(b, &*lists);
            self.grid.reset_view(&*lists);
        }
    }

    /// The control-channel state: flags, counts and positions; amounts
    /// only as the section totals and journal differences (never a name
    /// or a memo).
    pub(crate) fn state_json(&self) -> Value {
        let b = self.batch();
        let closing = self.books.closing;
        let blocker = b.can_save(&self.checks, self.caps(), closing).err();
        let i = self.selected();
        let fmt = |m: cl_core::Money| m.format(MoneyStyle::default());
        let sections: Vec<Value> = b
            .sections
            .iter()
            .enumerate()
            .map(|(n, s)| {
                let totals = b.section_totals(n).ok();
                json!({
                    "kind": s.kind,
                    "has_account": s.context.is_some(),
                    "rows": s.filled_count(),
                    "problems": self.problem_rows_in(n).len(),
                    "total": totals.map(|t| fmt(t.amount)),
                    "debits": totals.map(|t| fmt(t.debits)),
                    "credits": totals.map(|t| fmt(t.credits)),
                })
            })
            .collect();
        let cursor = self.grid.grid().core().cursor();
        let cursor_index = cursor.and_then(|c| {
            b.sections
                .get(i)
                .and_then(|s| s.rows.iter().position(|r| r.key == c.row))
        });
        let mut v = json!({
            "screen": "batch_entry",
            "loaded": self.books.loaded,
            "sections": b.sections.len(),
            "section": i,
            "section_kind": b.sections.get(i).map(|s| s.kind),
            "section_list": sections,
            "rows": b.filled_count(),
            "problems": self.problem_rows().len(),
            "unbalanced": self.unbalanced_differences(i).iter().map(|m| fmt(m.abs())).collect::<Vec<_>>(),
            "can_save": blocker.is_none(),
            "blocker": blocker.map(validate::blocker_name),
            "checking": self.checking.is_some(),
            "locked_rows": b.locked_rows(closing).len(),
            "add_new_names": b.add_new_names,
            "dirty": self.is_dirty(),
            "saving": self.busy(),
            "saved_count": self.saved_count,
            "last_action": self.last_action,
            "notice": self.notice,
            "error": self.error.is_some(),
            "section_refused": self.section_refused.map(|(n, why)| json!({
                "section": n,
                "why": match why {
                    cl_api::BatchGroupProblem::NotAllowed => "not_allowed",
                    cl_api::BatchGroupProblem::ContextAccount => "account",
                },
            })),
            "confirm_remove": self.confirm_remove.is_some(),
            "refused": self.refused.map(validate::blocker_name),
            "summary": if self.summary_open { self.summary.as_ref().map(summary::Summary::state_json) } else { None },
            "mapping": self.mapping_json(),
            "import_open": self.import_open,
            "pasted_rows": self.pasted_rows,
            "suggest": self.suggest_json(),
            "grid": {
                "focused": self.grid.grid().has_focus(),
                "row": cursor_index,
                "column": cursor.and_then(|c| grid::field_of(c.col)),
            },
        });
        if let (Value::Object(m), Value::Object(c)) =
            (&mut v, self.closing_prompt.state_json(self.caps().closing))
        {
            m.extend(c);
        }
        v
    }
}

/// The highest check number in the books from the engine's proposed next
/// number and the highest one on a check.
fn highest_of(next: Option<u32>, highest: Option<u32>) -> Option<u64> {
    let from_next = next.and_then(|n| n.checked_sub(1)).filter(|n| *n > 0);
    from_next.max(highest).map(u64::from)
}

impl Screen for BulkEntryScreen {
    fn title(&self) -> String {
        t("cmd.banking.bulk_entry")
    }

    fn ui(&mut self, ui: &mut Ui) {
        let frame = crate::grid_env::shell_frame(ui.ctx());
        self.money_style = frame.money_style;
        self.date_order = frame.date_order;
        self.poll();
        self.hear_changes();
        self.sync_shown_section();
        self.refresh_looks();
        self.draw(ui);
        self.after_grid();
        if let Some(cmd) = self.after_frame.take() {
            self.command(cmd);
        }
        let busy = self.books.busy()
            || self.checking.is_some()
            || self.numbers_task.is_some()
            || self.changed_at.is_some()
            || self.parse_task.is_some()
            || !self.suggest.asking.is_empty()
            || self.busy();
        if busy {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    fn scope(&self) -> Scope {
        Scope::Form
    }

    fn dirty(&self) -> bool {
        self.is_dirty()
    }

    fn save(&mut self) -> bool {
        self.save_all(cl_model::forms::SaveIntent::Save)
    }

    fn command(&mut self, id: CommandId) -> bool {
        if let Some(h) = self.closing_prompt.command(id) {
            return h;
        }
        self.form_command(id)
    }

    fn state(&self) -> Value {
        self.state_json()
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod flow_tests;
