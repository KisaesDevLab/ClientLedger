//! Checking the rows (plan §1): the app's own exact checks at once
//! (`cl_model::forms::batch::local_problems`), then the engine's Validate
//! half a second after the last change for the rows the cursor has left.
//! An engine answer counts only while its row (and, for a journal entry,
//! the whole entry) is unchanged (`EngineChecks`).
//!
//! A row that needs attention gets the `warn` tint and a caption; the
//! status bar says so; F8 / Shift+F8 go to the next / previous one, across
//! sections.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Instant;

use cl_api::ApiError;
use cl_core::Money;
use cl_grid::{CellPos, GridEvent, RowKey, TextKey};
use cl_model::forms::batch::{
    Blocker, EngineProblemKind, EngineState, Field, JeGroupKey, Problem, problems_by_row,
};

use super::grid::{Note, RowLook, column_of};
use super::{BulkEntryScreen, CHECK_AFTER, Checking, wire};
use crate::company::CompanyEvent;
use crate::i18n::t_with;

/// The status bar's line while rows need attention.
pub(crate) const ATTENTION: &str = "batch_entry.status.attention";

/// The words for one of the app's own problems.
#[must_use]
pub(crate) const fn problem_key(p: &Problem) -> &'static str {
    match p {
        Problem::Missing(Field::Date) => "batch_entry.problem.missing_date",
        Problem::Missing(Field::Number) => "batch_entry.problem.missing_entry_no",
        Problem::Missing(Field::Contact) => "batch_entry.problem.missing_payee",
        Problem::Missing(Field::Account) => "batch_entry.problem.missing_account",
        Problem::Missing(Field::Debit | Field::Credit) => "batch_entry.problem.missing_side",
        Problem::Missing(_) => "batch_entry.problem.missing_amount",
        Problem::NotPositive => "batch_entry.problem.not_positive",
        Problem::SubCent(_) => "batch_entry.problem.sub_cent",
        Problem::BothDebitCredit => "batch_entry.problem.both_sides",
        Problem::DuplicateNumber => "batch_entry.problem.duplicate_number",
        Problem::BadCheckNumber => "batch_entry.problem.bad_check_number",
        Problem::PrintLaterNumber => "batch_entry.problem.print_later_number",
        Problem::TooLong(_) => "batch_entry.problem.too_long",
        Problem::SameAsContext => "batch_entry.problem.same_account",
        Problem::JeUnbalanced { .. } => "batch_entry.problem.unbalanced",
        Problem::TooLarge => "batch_entry.problem.too_large",
        Problem::Closed => "batch_entry.closed",
    }
}

/// The control channel's name for a blocker.
#[must_use]
pub(crate) const fn blocker_name(b: Blocker) -> &'static str {
    match b {
        Blocker::EngineTooOld => "engine_too_old",
        Blocker::Empty => "empty",
        Blocker::TooManyRows(_) => "too_many_rows",
        Blocker::KindUnsupported(_) => "kind_unsupported",
        Blocker::MixedUnsupported => "mixed_unsupported",
        Blocker::MissingAccount(_) => "missing_account",
        Blocker::LocalProblems(_) => "local_problems",
        Blocker::ClosedUnsupported(_) => "closed_unsupported",
        Blocker::Unchecked(_) => "unchecked",
        Blocker::EngineProblems(_) => "engine_problems",
        Blocker::NotInLists(_) => "not_in_lists",
    }
}

/// Why Save all is off, in plain words.
#[must_use]
pub(crate) fn blocker_text(b: Blocker) -> String {
    let n = |k: &str, n: usize| t_with(k, &[("n", &n.to_string())]);
    match b {
        Blocker::EngineTooOld => crate::i18n::t("batch_entry.blocker.engine"),
        Blocker::Empty => crate::i18n::t("batch_entry.blocker.empty"),
        Blocker::TooManyRows(c) => n("batch_entry.blocker.too_many", c),
        Blocker::KindUnsupported(k) => t_with(
            "batch_entry.blocker.kind",
            &[("kind", &crate::i18n::t(super::sections::kind_key(k)))],
        ),
        Blocker::MixedUnsupported => crate::i18n::t("batch_entry.blocker.mixed"),
        Blocker::MissingAccount(_) => crate::i18n::t("batch_entry.blocker.missing_account"),
        Blocker::LocalProblems(c) | Blocker::EngineProblems(c) => {
            n("batch_entry.blocker.problems", c)
        }
        Blocker::NotInLists(c) => n("batch_entry.blocker.not_in_lists", c),
        Blocker::ClosedUnsupported(_) => crate::i18n::t("batch_entry.blocker.closed"),
        Blocker::Unchecked(c) => n("batch_entry.blocker.unchecked", c),
    }
}

/// The caption of the engine's first problem on a row.
fn engine_note(list: &[cl_model::forms::batch::EngineProblem]) -> Option<Note> {
    let p = list.first()?;
    Some(match p.kind {
        EngineProblemKind::NotInLists => Note::Problem(TextKey("batch_entry.problem.not_in_lists")),
        EngineProblemKind::Inactive => Note::Problem(TextKey("batch_entry.problem.inactive")),
        EngineProblemKind::Unbalanced => Note::Problem(TextKey("batch_entry.problem.unbalanced")),
        EngineProblemKind::ClosedPeriod => Note::Problem(TextKey("batch_entry.closed")),
        EngineProblemKind::Required | EngineProblemKind::Invalid | EngineProblemKind::Other => {
            if p.message.trim().is_empty() {
                Note::Problem(TextKey("batch_entry.problem.engine"))
            } else {
                Note::Engine(p.message.clone())
            }
        }
    })
}

impl BulkEntryScreen {
    /// True when a row's problems are shown: the cursor has left it (or the
    /// grid does not have the keyboard).
    pub(crate) fn shows(&self, key: RowKey) -> bool {
        self.touched.contains(&key)
            || !(self.grid.grid().has_focus() && self.cursor_row == Some(key))
    }

    /// The rows whose own checks fail (the lock mark aside), with the
    /// problems.
    fn local_bad(&self) -> BTreeMap<RowKey, Vec<Problem>> {
        let mut by_row = problems_by_row(self.batch().local_problems(self.books.closing));
        for list in by_row.values_mut() {
            list.retain(|p| !p.is_lock_mark());
        }
        by_row.retain(|_, l| !l.is_empty());
        by_row
    }

    /// What each row looks like this frame: tint and caption.
    pub(crate) fn refresh_looks(&mut self) {
        let closing = self.books.closing;
        let local = self.local_bad();
        let locked: BTreeSet<RowKey> = self.batch().locked_rows(closing).into_iter().collect();
        let add_names = self.batch().add_new_names;
        let mut looks = BTreeMap::new();
        for (_, r) in self.batch().filled() {
            let shown = self.shows(r.key);
            let mut look = RowLook::default();
            if let Some(list) = local.get(&r.key).filter(|_| shown) {
                look.error = true;
                look.note = list.first().map(|p| Note::Problem(TextKey(problem_key(p))));
            } else {
                match self.checks.state(self.batch(), r.key) {
                    EngineState::Problems(_, list) if shown => {
                        look.error = true;
                        look.note = engine_note(list);
                    }
                    EngineState::Ok {
                        new_contact: true, ..
                    } if !add_names && shown => {
                        look.error = true;
                        look.note = Some(Note::Problem(TextKey("batch_entry.problem.new_name")));
                    }
                    _ => {}
                }
            }
            if look.note.is_none() && locked.contains(&r.key) {
                look.note = Some(Note::Caption(TextKey("batch_entry.closed")));
            }
            if look.note.is_none()
                && let Some(text) = self.suggestion_caption(r.key)
            {
                look.note = Some(Note::Suggestion(text));
            }
            if look != RowLook::default() {
                looks.insert(r.key, look);
            }
        }
        self.grid.adapter_mut().looks = looks;
        let attention = !self.problem_rows().is_empty();
        if attention != self.attention_toast {
            self.attention_toast = attention;
            self.link.push(if attention {
                CompanyEvent::Toast(ATTENTION)
            } else {
                CompanyEvent::ClearToast(ATTENTION)
            });
        }
    }

    /// The rows that need attention in section `i`, in grid order.
    #[must_use]
    pub(crate) fn problem_rows_in(&self, i: usize) -> Vec<RowKey> {
        let looks = &self.grid.adapter().looks;
        self.batch()
            .sections
            .get(i)
            .map(|s| {
                s.rows
                    .iter()
                    .filter(|r| looks.get(&r.key).is_some_and(|l| l.error))
                    .map(|r| r.key)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Every row that needs attention, as (section, row), in strip order.
    #[must_use]
    pub(crate) fn problem_rows(&self) -> Vec<(usize, RowKey)> {
        (0..self.batch().sections.len())
            .flat_map(|i| self.problem_rows_in(i).into_iter().map(move |k| (i, k)))
            .collect()
    }

    /// Debits minus credits of each unbalanced journal entry in section
    /// `i` (exact; the owner's own amounts).
    #[must_use]
    pub(crate) fn unbalanced_differences(&self, i: usize) -> Vec<Money> {
        let Some(s) = self.batch().sections.get(i) else {
            return Vec::new();
        };
        let mut seen: BTreeSet<JeGroupKey> = BTreeSet::new();
        let mut out = Vec::new();
        for (_, p) in s.local_problems(None) {
            if let Problem::JeUnbalanced { group, difference } = p
                && seen.insert(group)
            {
                out.push(difference);
            }
        }
        out
    }

    /// The cursor left a row: its problems show from now on.
    pub(crate) fn mark_cursor_row_left(&mut self) {
        if let Some(k) = self.cursor_row.take() {
            self.touched.insert(k);
        }
    }

    /// After the grid drew: the cursor's moves, the grid's events, and the
    /// half-second timer of the engine check.
    pub(crate) fn after_grid_events(&mut self, events: &[GridEvent]) {
        for e in events {
            if let GridEvent::RowRecorded { key } = e {
                self.touched.insert(*key);
            }
        }
        let now = self.grid.grid().core().cursor().map(|c| c.row);
        if now != self.cursor_row {
            self.mark_cursor_row_left();
            self.cursor_row = now;
        }
        if !self.grid.grid().has_focus() {
            self.mark_cursor_row_left();
            self.cursor_row = now;
        }
    }

    /// The timer: half a second after the last change, check what changed.
    pub(crate) fn after_grid(&mut self) {
        let rev = self.grid.form().revision();
        if rev != self.seen_rev {
            self.seen_rev = rev;
            self.changed_at = Some(Instant::now());
            self.checks.prune(self.grid.form().current());
        }
        if let Some(at) = self.changed_at
            && at.elapsed() >= CHECK_AFTER
            && self.checking.is_none()
            && !self.busy()
        {
            self.changed_at = None;
            self.start_check(false);
        }
    }

    /// Sends rows to the engine's Validate: those the cursor has left that
    /// pass the app's own checks and have no current answer (`all`: every
    /// row, "Check all rows"). Whole journal entries go together.
    pub(crate) fn start_check(&mut self, all: bool) -> bool {
        if self.checking.is_some() || !self.caps().exact {
            return false;
        }
        if all {
            let keys: Vec<RowKey> = self.batch().filled().map(|(_, r)| r.key).collect();
            self.touched.extend(keys);
        }
        let bad = self.local_bad();
        let b = self.batch();
        // A journal entry with any bad line waits until it is fixed.
        let mut bad_groups: BTreeSet<(usize, JeGroupKey)> = BTreeSet::new();
        for (i, s) in b.sections.iter().enumerate() {
            for r in s.filled() {
                if bad.contains_key(&r.key)
                    && let Some(g) = s.je_group_of(r)
                {
                    bad_groups.insert((i, g));
                }
            }
        }
        let candidates = if all {
            b.all_sendable()
                .into_iter()
                .filter(|k| matches!(self.checks.state(b, *k), EngineState::Unchecked))
                .collect()
        } else {
            b.rows_to_check(&self.checks)
        };
        let send: BTreeSet<RowKey> = candidates
            .into_iter()
            .filter(|k| !bad.contains_key(k) && self.shows(*k))
            .filter(|k| {
                b.find(*k).is_none_or(|(i, r)| {
                    b.sections[i]
                        .je_group_of(r)
                        .is_none_or(|g| !bad_groups.contains(&(i, g)))
                })
            })
            .collect();
        if send.is_empty() {
            return false;
        }
        let out = b.outgoing(Some(&send), self.books.closing);
        if out.is_empty() {
            return false;
        }
        let Ok(groups) = wire::groups(&out, None) else {
            return false;
        };
        self.checks.begin(&out);
        let link = Arc::clone(&self.link);
        if let Some(local) = self.local.as_mut() {
            let answer = local.validate(&link, &groups);
            self.checked(&out, answer);
            return true;
        }
        if let Some(task) = super::engine::live::validate(&self.link, self.group.as_ref(), groups) {
            self.checking = Some(Checking { out, task });
            true
        } else {
            self.checks.fail(&out);
            false
        }
    }

    pub(crate) fn poll_checking(&mut self) {
        let Some(mut c) = self.checking.take() else {
            return;
        };
        match c.task.try_take() {
            None => self.checking = Some(c),
            Some(r) => self.checked(&c.out, r),
        }
    }

    /// A validate answer arrived (or failed: the rows go back to unchecked
    /// and the error shows).
    fn checked(
        &mut self,
        out: &cl_model::forms::batch::Outgoing,
        r: Result<cl_api::BatchValidation, ApiError>,
    ) {
        match r {
            Ok(v) => {
                let verdicts = wire::verdicts(&v);
                let batch = self.batch().clone();
                self.checks.apply(&batch, out, verdicts);
                if matches!(
                    self.error,
                    Some(ApiError::Local(cl_api::LocalValidation::BatchUnavailable))
                ) {
                    self.error = None;
                }
            }
            Err(e) => {
                self.checks.fail(out);
                self.error = Some(e);
            }
        }
        self.after_check();
    }

    /// F8 (`forward`) / Shift+F8: the next / previous row that needs
    /// attention after the cursor, across sections (wrapping); the cursor
    /// goes to its first problem cell.
    pub(crate) fn go_to_problem(&mut self, forward: bool) {
        let all = self.problem_rows();
        if all.is_empty() {
            self.notice = Some("batch_entry.no_problems");
            return;
        }
        self.notice = None;
        let here = self.selected();
        let pos_of = |(i, k): (usize, RowKey)| -> (usize, usize) {
            let idx = self.batch().sections[i]
                .rows
                .iter()
                .position(|r| r.key == k)
                .unwrap_or(0);
            (i, idx)
        };
        let cursor = self
            .grid
            .grid()
            .core()
            .cursor()
            .and_then(|c| {
                self.batch()
                    .sections
                    .get(here)
                    .and_then(|s| s.rows.iter().position(|r| r.key == c.row))
            })
            .map(|idx| (here, idx));
        let ordered: Vec<((usize, usize), (usize, RowKey))> =
            all.into_iter().map(|p| (pos_of(p), p)).collect();
        let target = match (cursor, forward) {
            (Some(c), true) => ordered.iter().find(|(p, _)| *p > c).or(ordered.first()),
            (Some(c), false) => ordered
                .iter()
                .rev()
                .find(|(p, _)| *p < c)
                .or(ordered.last()),
            (None, true) => ordered.first(),
            (None, false) => ordered.last(),
        };
        let Some(&(_, (_, key))) = target else {
            return;
        };
        self.focus_row(key);
        self.last_action = Some("go_to_problem");
    }

    /// Shows row `key` (its section) with the cursor on its first problem
    /// cell.
    pub(crate) fn focus_row(&mut self, key: RowKey) {
        let Some((section, _)) = self.batch().find(key) else {
            return;
        };
        self.select_section(section);
        let kind = self.batch().sections[section].kind;
        let field = self
            .first_field(key)
            .filter(|f| kind.columns().contains(f))
            .unwrap_or(Field::Date);
        let lists = Arc::clone(&self.books.lists);
        self.grid.focus(
            CellPos {
                row: key,
                col: column_of(field),
            },
            &*lists,
        );
        self.cursor_row = Some(key);
        self.touched.insert(key);
    }

    /// The cell a row's first problem is about, when one is named.
    pub(crate) fn first_field(&self, key: RowKey) -> Option<Field> {
        let local = self.local_bad();
        local
            .get(&key)
            .and_then(|l| l.iter().find_map(Problem::field))
            .or_else(|| match self.checks.state(self.batch(), key) {
                EngineState::Problems(_, l) => l.iter().find_map(|p| p.field),
                EngineState::Ok { .. } => Some(Field::Contact),
                _ => None,
            })
    }

    /// After a check: a save waiting for it goes on when nothing blocks it.
    pub(crate) fn after_check(&mut self) {
        if let Some(intent) = self.save_after_check.take() {
            match self
                .batch()
                .can_save(&self.checks, self.caps(), self.books.closing)
            {
                Ok(()) => {
                    self.save_all(intent);
                }
                Err(Blocker::Unchecked(_)) if self.checking.is_some() => {
                    self.save_after_check = Some(intent);
                }
                Err(b) => self.refuse_save(b),
            }
        }
    }
}
