//! Bulk Entry batch state (P15A WP-B2): what goes to the engine, the
//! engine's results per row, the Save all gate, and totals.
//!
//! - [`BulkBatch::outgoing`] builds the plain groups the screen maps 1:1
//!   onto `cl_api::batch::{BatchGroup, BatchRowIn}`. Row numbers are
//!   1-based in send order and unique across the whole call (the engine
//!   requires that); [`Outgoing::sent`] remembers which row and which
//!   content hash each number stood for when it was sent.
//! - [`EngineChecks`] keeps the engine's verdict per row together with the
//!   hash it was given for. A verdict counts only while the row (and, for a
//!   journal entry, its whole group, its section's kind and account) is
//!   unchanged; anything else reads as unchecked.
//! - [`BulkBatch::can_save`] is the Save all gate: every non-blank row
//!   passes the local checks and has a current engine "ok".

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use cl_core::ids::{AccountId, ContactId, TagId};
use cl_core::{CalendarDate, Money, MoneyError};

use super::{BulkBatch, Field, Fnv, JeGroupKey, MAX_ROWS, Problem, RowDraft, RowKey, SectionKind};

// ------------------------------------------------------------ capabilities

/// The engine's `batch-*-v1` capabilities as cl-model sees them (the
/// screen copies them from `cl_api::batch::BatchCaps`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // one flag per capability
pub struct SaveCaps {
    /// `batch-atomic-v1`.
    pub atomic: bool,
    /// `batch-check-v1`.
    pub check: bool,
    /// `batch-mixed-v1`.
    pub mixed: bool,
    /// `batch-exact-v1`.
    pub exact: bool,
    /// `batch-idempotent-v1`.
    pub idempotent: bool,
    /// `batch-closing-v1`.
    pub closing: bool,
}

impl SaveCaps {
    /// Every capability (the demo company, an up-to-date engine).
    pub const ALL: SaveCaps = SaveCaps {
        atomic: true,
        check: true,
        mixed: true,
        exact: true,
        idempotent: true,
        closing: true,
    };

    /// Save all may post at all (Q-P15Ac): atomic, exact and idempotent,
    /// and grouped (`batch-mixed-v1`): the save always sends `groups[]`,
    /// even for one section, as `cl_api::Batch::save` requires.
    #[must_use]
    pub const fn can_post(self) -> bool {
        self.atomic && self.exact && self.idempotent && self.mixed
    }

    /// The engine takes rows of `kind`.
    #[must_use]
    pub const fn offers(self, kind: SectionKind) -> bool {
        match kind {
            SectionKind::Checks => self.check,
            _ => true,
        }
    }
}

// --------------------------------------------------------------- outgoing

/// One row as sent to the engine (maps 1:1 onto `BatchRowIn`; the
/// closing-date password is added by the screen on rows with `locked`).
#[derive(Clone, PartialEq, Eq)]
pub struct OutRow {
    /// 1-based, unique across the call.
    pub row_number: u32,
    /// The date.
    pub date: CalendarDate,
    /// Check number, Ref no. or Entry No. (trimmed; `None` when empty).
    pub number: Option<String>,
    /// Print later (checks only).
    pub print_later: bool,
    /// The picked name.
    pub contact: Option<ContactId>,
    /// The typed name, when none is picked.
    pub contact_name: Option<String>,
    /// The picked account.
    pub account: Option<AccountId>,
    /// The typed account, when none is picked.
    pub account_name: Option<String>,
    /// Amount (every kind but journal entries).
    pub amount: Option<Money>,
    /// Debit (journal entries).
    pub debit: Option<Money>,
    /// Credit (journal entries).
    pub credit: Option<Money>,
    /// Memo (`None` when empty).
    pub memo: Option<String>,
    /// Tag.
    pub tag: Option<TagId>,
    /// Dated on or before the closing date: needs the password.
    pub locked: bool,
}

/// Ids and flags only.
impl fmt::Debug for OutRow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OutRow")
            .field("row_number", &self.row_number)
            .field("contact", &self.contact)
            .field("account", &self.account)
            .field("tag", &self.tag)
            .field("locked", &self.locked)
            .finish_non_exhaustive()
    }
}

/// One section as sent (maps 1:1 onto `BatchGroup`).
#[derive(Clone, PartialEq, Eq)]
pub struct OutGroup {
    /// The kind (`SectionKind::engine_kind` is the `txn_type`).
    pub kind: SectionKind,
    /// The context account.
    pub context: Option<AccountId>,
    /// The rows, in send order.
    pub rows: Vec<OutRow>,
}

/// Kind, account and row count only.
impl fmt::Debug for OutGroup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OutGroup")
            .field("kind", &self.kind)
            .field("context", &self.context)
            .field("rows", &self.rows.len())
            .finish()
    }
}

/// What a row number stood for when it was sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SentRow {
    /// The row.
    pub key: RowKey,
    /// Its section's index in the window.
    pub section: usize,
    /// Its group's index in the call.
    pub group: u32,
    /// [`BulkBatch::row_hash`] at send time.
    pub hash: u64,
}

/// A validate or save call's body, with its row-number map.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Outgoing {
    /// The groups (sections with at least one row sent), in strip order.
    pub groups: Vec<OutGroup>,
    /// Row number → the row it stood for.
    pub sent: BTreeMap<u32, SentRow>,
}

impl Outgoing {
    /// Nothing to send.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.sent.is_empty()
    }

    /// The window's section a group of the call stood for.
    #[must_use]
    pub fn section_of_group(&self, group: u32) -> Option<usize> {
        self.sent
            .values()
            .find(|s| s.group == group)
            .map(|s| s.section)
    }

    /// The row a row number stood for.
    #[must_use]
    pub fn key_of(&self, row_number: u32) -> Option<RowKey> {
        self.sent.get(&row_number).map(|s| s.key)
    }

    /// Every account the call touches, for the data-bus notice after a
    /// save: the context accounts, the picked line accounts, and for a row
    /// whose account was typed (journal entry rows included) the account
    /// the engine matched it to (`resolved`, by row). `None` when a typed
    /// account's match is not known: then any account may have changed.
    #[must_use]
    pub fn touched_accounts(
        &self,
        resolved: impl Fn(RowKey) -> Option<AccountId>,
    ) -> Option<BTreeSet<AccountId>> {
        let mut out = BTreeSet::new();
        for g in &self.groups {
            out.extend(g.context);
            for r in &g.rows {
                if let Some(a) = r.account {
                    out.insert(a);
                } else if r.account_name.is_some() {
                    let key = self.key_of(r.row_number)?;
                    out.insert(resolved(key)?);
                }
            }
        }
        Some(out)
    }
}

fn non_empty(s: &str) -> Option<String> {
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_string())
}

fn out_row(
    kind: SectionKind,
    row: &RowDraft,
    date: CalendarDate,
    row_number: u32,
    closing: Option<CalendarDate>,
) -> OutRow {
    let journal = kind.is_journal();
    OutRow {
        row_number,
        date,
        number: non_empty(&row.number),
        print_later: kind == SectionKind::Checks && row.print_later,
        contact: row.contact,
        contact_name: if row.contact.is_some() {
            None
        } else {
            non_empty(&row.contact_text)
        },
        account: row.account,
        account_name: if row.account.is_some() {
            None
        } else {
            non_empty(&row.account_text)
        },
        amount: if journal { None } else { row.amount },
        debit: if journal { row.debit } else { None },
        credit: if journal { row.credit } else { None },
        memo: (!row.memo.trim().is_empty()).then(|| row.memo.clone()),
        tag: row.tag,
        locked: closing.is_some_and(|c| date <= c),
    }
}

// ----------------------------------------------------------- engine state

/// What kind of problem the engine found (mirrors
/// `cl_api::batch::ProblemKind`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EngineProblemKind {
    /// "Not in your lists".
    NotInLists,
    /// An inactive or deleted account, name or tag.
    Inactive,
    /// A required cell is empty.
    Required,
    /// A value the engine refused.
    Invalid,
    /// On or before the closing date.
    ClosedPeriod,
    /// A journal entry whose debits and credits differ.
    Unbalanced,
    /// Anything else (the message says what).
    Other,
}

/// One problem the engine found on a row.
#[derive(Clone, PartialEq, Eq)]
pub struct EngineProblem {
    /// The cell (`None`: the row as a whole).
    pub field: Option<Field>,
    /// What is wrong.
    pub kind: EngineProblemKind,
    /// The engine's message (display only; never logged).
    pub message: String,
}

/// The cell and kind only, never the message.
impl fmt::Debug for EngineProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EngineProblem")
            .field("field", &self.field)
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

/// The engine's answer for one row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EngineVerdict {
    /// The row can be saved as it is.
    Ok {
        /// The name is not in the lists; it is added on save only with
        /// "Add new names to the list" (otherwise the row is blocked).
        new_contact: bool,
        /// The account the engine matched (`resolvedAccountId`; for a
        /// typed account, the one it was matched to).
        account: Option<AccountId>,
    },
    /// The row cannot be saved.
    Problems(Vec<EngineProblem>),
}

/// A row's engine state; every variant but `Unchecked` carries the
/// [`BulkBatch::row_hash`] it is about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EngineState {
    /// Not sent since its last change.
    Unchecked,
    /// Sent; waiting for the answer.
    Checking(u64),
    /// The engine accepts it.
    Ok {
        /// The hash checked.
        hash: u64,
        /// The name would be added.
        new_contact: bool,
        /// The account the engine matched.
        account: Option<AccountId>,
    },
    /// The engine refused it.
    Problems(u64, Vec<EngineProblem>),
}

impl EngineState {
    fn hash(&self) -> Option<u64> {
        match self {
            EngineState::Unchecked => None,
            EngineState::Checking(h)
            | EngineState::Ok { hash: h, .. }
            | EngineState::Problems(h, _) => Some(*h),
        }
    }
}

static UNCHECKED: EngineState = EngineState::Unchecked;

/// The engine's results per row, outside undo.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EngineChecks {
    by_row: BTreeMap<RowKey, EngineState>,
}

impl EngineChecks {
    /// No results.
    #[must_use]
    pub fn new() -> EngineChecks {
        EngineChecks::default()
    }

    /// The row's state as it applies now: a result for older content reads
    /// as [`EngineState::Unchecked`].
    #[must_use]
    pub fn state<'a>(&'a self, batch: &BulkBatch, key: RowKey) -> &'a EngineState {
        match (self.by_row.get(&key), batch.row_hash(key)) {
            (Some(s), Some(h)) if s.hash() == Some(h) => s,
            _ => &UNCHECKED,
        }
    }

    /// The account the engine matched for the row, while its "ok" holds.
    #[must_use]
    pub fn resolved_account(&self, batch: &BulkBatch, key: RowKey) -> Option<AccountId> {
        match self.state(batch, key) {
            EngineState::Ok { account, .. } => *account,
            _ => None,
        }
    }

    /// Marks every row of `out` as being checked.
    pub fn begin(&mut self, out: &Outgoing) {
        for s in out.sent.values() {
            self.by_row.insert(s.key, EngineState::Checking(s.hash));
        }
    }

    /// Applies a validate answer. A verdict lands only when the row still
    /// has the content it was sent with; sent rows without a verdict go
    /// back to unchecked. Returns how many verdicts landed.
    pub fn apply(
        &mut self,
        batch: &BulkBatch,
        out: &Outgoing,
        verdicts: impl IntoIterator<Item = (u32, EngineVerdict)>,
    ) -> usize {
        let mut answered = BTreeSet::new();
        let mut landed = 0;
        for (n, verdict) in verdicts {
            let Some(sent) = out.sent.get(&n) else {
                continue;
            };
            answered.insert(n);
            if batch.row_hash(sent.key) != Some(sent.hash) {
                continue;
            }
            let state = match verdict {
                EngineVerdict::Ok {
                    new_contact,
                    account,
                } => EngineState::Ok {
                    hash: sent.hash,
                    new_contact,
                    account,
                },
                EngineVerdict::Problems(list) => EngineState::Problems(sent.hash, list),
            };
            self.by_row.insert(sent.key, state);
            landed += 1;
        }
        for (n, sent) in &out.sent {
            if !answered.contains(n) {
                self.forget_if_checking(sent);
            }
        }
        landed
    }

    /// The call failed (transport, timeout): its rows go back to unchecked.
    pub fn fail(&mut self, out: &Outgoing) {
        for sent in out.sent.values() {
            self.forget_if_checking(sent);
        }
    }

    /// A refused save: the rows it names get the problems (when unchanged
    /// since the save was sent); other rows keep their state.
    pub fn refuse(
        &mut self,
        batch: &BulkBatch,
        out: &Outgoing,
        problems: impl IntoIterator<Item = (u32, EngineProblem)>,
    ) {
        let mut by_number: BTreeMap<u32, Vec<EngineProblem>> = BTreeMap::new();
        for (n, p) in problems {
            by_number.entry(n).or_default().push(p);
        }
        for (n, list) in by_number {
            let Some(sent) = out.sent.get(&n) else {
                continue;
            };
            if batch.row_hash(sent.key) == Some(sent.hash) {
                self.by_row
                    .insert(sent.key, EngineState::Problems(sent.hash, list));
            }
        }
    }

    /// Drops the results of rows no longer in `batch`.
    pub fn prune(&mut self, batch: &BulkBatch) {
        let keys: BTreeSet<RowKey> = batch.keys().collect();
        self.by_row.retain(|k, _| keys.contains(k));
    }

    /// Forgets everything (a new batch).
    pub fn clear(&mut self) {
        self.by_row.clear();
    }

    fn forget_if_checking(&mut self, sent: &SentRow) {
        if self.by_row.get(&sent.key) == Some(&EngineState::Checking(sent.hash)) {
            self.by_row.remove(&sent.key);
        }
    }
}

// ------------------------------------------------------------------- gate

/// Why Save all is off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Blocker {
    /// The engine lacks `batch-atomic-v1`, `batch-exact-v1`,
    /// `batch-idempotent-v1` or `batch-mixed-v1` (Q-P15Ac,
    /// [`SaveCaps::can_post`]).
    EngineTooOld,
    /// No rows ("Add some rows first").
    Empty,
    /// More than [`MAX_ROWS`] rows.
    TooManyRows(usize),
    /// The engine does not take this kind (checks without
    /// `batch-check-v1`).
    KindUnsupported(SectionKind),
    /// A section with rows has no bank or card account (its index).
    MissingAccount(usize),
    /// Rows with problems the app found.
    LocalProblems(usize),
    /// Rows in a closed period, and the engine cannot take the password
    /// ("Enter it on its form").
    ClosedUnsupported(usize),
    /// Rows not checked by the engine yet (or being checked).
    Unchecked(usize),
    /// Rows the engine refused.
    EngineProblems(usize),
    /// Rows naming someone not in the lists while "Add new names to the
    /// list" is off.
    NotInLists(usize),
}

/// Per-section sums of the owner's own amounts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SectionTotals {
    /// Rows that are not blank.
    pub rows: usize,
    /// Sum of Amount (every kind but journal entries).
    pub amount: Money,
    /// Sum of Debit (journal entries).
    pub debits: Money,
    /// Sum of Credit (journal entries).
    pub credits: Money,
}

fn to_money(units: i128) -> Result<Money, MoneyError> {
    Money::from_ten_thousandths(units)
}

impl BulkBatch {
    /// The hash an engine result for `key` is valid for: the section's kind
    /// and account, the row's content, and for a journal entry row every
    /// row of its group (a change anywhere in an entry re-checks the whole
    /// entry). `None` when the row is not in the batch.
    #[must_use]
    pub fn row_hash(&self, key: RowKey) -> Option<u64> {
        let (i, row) = self.find(key)?;
        let section = &self.sections[i];
        let mut h = Fnv::new();
        h.byte(section.kind.hash_tag());
        h.opt(section.context.map(|a| a.to_string()).as_deref());
        row.feed(&mut h);
        if let Some(group) = section.je_group_of(row) {
            for r in section
                .filled()
                .filter(|r| JeGroupKey::of(r).as_ref() == Some(&group))
            {
                h.byte(2);
                r.feed(&mut h);
            }
        }
        Some(h.finish())
    }

    /// A hash of everything a save sends (sections with rows, their rows'
    /// content and "Add new names"); the screen keeps one idempotency key
    /// per value, so a retry of the same content replays safely.
    #[must_use]
    pub fn content_hash(&self) -> u64 {
        let mut h = Fnv::new();
        h.byte(u8::from(self.add_new_names));
        for s in &self.sections {
            let rows: Vec<&RowDraft> = s.filled().collect();
            if rows.is_empty() {
                continue;
            }
            h.byte(s.kind.hash_tag());
            h.opt(s.context.map(|a| a.to_string()).as_deref());
            h.bytes(&(rows.len() as u64).to_le_bytes());
            for r in rows {
                r.feed(&mut h);
            }
        }
        h.finish()
    }

    /// The rows auto-validate should send: non-blank, dated, and not
    /// currently checked or being checked, widened to whole journal
    /// entries.
    #[must_use]
    pub fn rows_to_check(&self, checks: &EngineChecks) -> BTreeSet<RowKey> {
        let mut out = BTreeSet::new();
        for s in &self.sections {
            let mut groups: BTreeSet<JeGroupKey> = BTreeSet::new();
            for r in s.filled().filter(|r| r.date.is_some()) {
                if matches!(checks.state(self, r.key), EngineState::Unchecked) {
                    out.insert(r.key);
                    if let Some(g) = s.je_group_of(r) {
                        groups.insert(g);
                    }
                }
            }
            for r in s.filled() {
                if s.je_group_of(r).is_some_and(|g| groups.contains(&g)) {
                    out.insert(r.key);
                }
            }
        }
        out
    }

    /// Every non-blank, dated row (the "Check all rows" button), as keys.
    #[must_use]
    pub fn all_sendable(&self) -> BTreeSet<RowKey> {
        self.filled()
            .filter(|(_, r)| r.date.is_some())
            .map(|(_, r)| r.key)
            .collect()
    }

    /// The body of a validate or save call: the non-blank, dated rows (only
    /// those in `only`, when given), grouped by section, numbered 1..n in
    /// send order across the whole call.
    #[must_use]
    pub fn outgoing(
        &self,
        only: Option<&BTreeSet<RowKey>>,
        closing: Option<CalendarDate>,
    ) -> Outgoing {
        let mut out = Outgoing::default();
        let mut next: u32 = 1;
        for (si, s) in self.sections.iter().enumerate() {
            let mut rows = Vec::new();
            let group = u32::try_from(out.groups.len()).unwrap_or(u32::MAX);
            for r in s.filled() {
                let Some(date) = r.date else { continue };
                if only.is_some_and(|keys| !keys.contains(&r.key)) {
                    continue;
                }
                let Some(hash) = self.row_hash(r.key) else {
                    continue;
                };
                rows.push(out_row(s.kind, r, date, next, closing));
                out.sent.insert(
                    next,
                    SentRow {
                        key: r.key,
                        section: si,
                        group,
                        hash,
                    },
                );
                next = next.saturating_add(1);
            }
            if !rows.is_empty() {
                out.groups.push(OutGroup {
                    kind: s.kind,
                    context: s.context,
                    rows,
                });
            }
        }
        out
    }

    /// The Save all gate (plan §1): every non-blank row passes the local
    /// checks and has a current engine "ok", and the engine can take the
    /// batch exactly, atomically and idempotently.
    ///
    /// # Errors
    /// The first [`Blocker`], in the order the owner should fix them.
    pub fn can_save(
        &self,
        checks: &EngineChecks,
        caps: SaveCaps,
        closing: Option<CalendarDate>,
    ) -> Result<(), Blocker> {
        if !caps.can_post() {
            return Err(Blocker::EngineTooOld);
        }
        let filled = self.filled_count();
        if filled == 0 {
            return Err(Blocker::Empty);
        }
        if filled > MAX_ROWS {
            return Err(Blocker::TooManyRows(filled));
        }
        let used: Vec<(usize, &super::Section)> = self
            .sections
            .iter()
            .enumerate()
            .filter(|(_, s)| s.filled_count() > 0)
            .collect();
        if let Some((_, s)) = used.iter().find(|(_, s)| !caps.offers(s.kind)) {
            return Err(Blocker::KindUnsupported(s.kind));
        }
        if let Some((i, _)) = used
            .iter()
            .find(|(_, s)| s.kind.needs_context().is_some() && s.context.is_none())
        {
            return Err(Blocker::MissingAccount(*i));
        }
        let problems = self.local_problems(closing);
        let bad: BTreeSet<RowKey> = problems
            .iter()
            .filter(|(_, p)| !p.is_lock_mark())
            .map(|(k, _)| *k)
            .collect();
        if !bad.is_empty() {
            return Err(Blocker::LocalProblems(bad.len()));
        }
        if !caps.closing {
            let locked = self.locked_rows(closing).len();
            if locked > 0 {
                return Err(Blocker::ClosedUnsupported(locked));
            }
        }
        let (mut unchecked, mut refused, mut new_names) = (0, 0, 0);
        for (_, r) in self.filled() {
            match checks.state(self, r.key) {
                EngineState::Unchecked | EngineState::Checking(_) => unchecked += 1,
                EngineState::Problems(..) => refused += 1,
                EngineState::Ok { new_contact, .. } => {
                    if *new_contact && !self.add_new_names {
                        new_names += 1;
                    }
                }
            }
        }
        if unchecked > 0 {
            return Err(Blocker::Unchecked(unchecked));
        }
        if refused > 0 {
            return Err(Blocker::EngineProblems(refused));
        }
        if new_names > 0 {
            return Err(Blocker::NotInLists(new_names));
        }
        Ok(())
    }

    /// The sums of one section's own amounts (non-blank rows).
    ///
    /// # Errors
    /// [`MoneyError`] when a sum leaves the money range.
    pub fn section_totals(&self, index: usize) -> Result<SectionTotals, MoneyError> {
        let Some(s) = self.sections.get(index) else {
            return Ok(SectionTotals {
                rows: 0,
                amount: Money::ZERO,
                debits: Money::ZERO,
                credits: Money::ZERO,
            });
        };
        let units = |f: fn(&RowDraft) -> Option<Money>| -> i128 {
            s.filled()
                .filter_map(f)
                .map(Money::to_ten_thousandths)
                .sum()
        };
        Ok(SectionTotals {
            rows: s.filled_count(),
            amount: to_money(units(|r| r.amount))?,
            debits: to_money(units(|r| r.debit))?,
            credits: to_money(units(|r| r.credit))?,
        })
    }

    /// The owner's own amounts per context account and kind (Checks on 1010,
    /// Deposits on 1010, ...): the summary's totals. Journal entries have no
    /// context account and are in [`BulkBatch::section_totals`]. Never a
    /// balance (non-negotiable 7).
    ///
    /// # Errors
    /// [`MoneyError`] when a sum leaves the money range.
    pub fn totals(&self) -> Result<BTreeMap<(AccountId, SectionKind), Money>, MoneyError> {
        let mut units: BTreeMap<(AccountId, SectionKind), i128> = BTreeMap::new();
        for s in &self.sections {
            let (Some(ctx), false) = (s.context, s.kind.is_journal()) else {
                continue;
            };
            for r in s.filled() {
                if let Some(a) = r.amount {
                    *units.entry((ctx, s.kind)).or_default() += a.to_ten_thousandths();
                }
            }
        }
        units
            .into_iter()
            .map(|(k, u)| to_money(u).map(|m| (k, m)))
            .collect()
    }

    /// Transactions per kind: non-blank rows, and for journal entries
    /// the entries (lines with the same date and Entry No. make one; a
    /// line without both counts on its own).
    #[must_use]
    pub fn count_by_kind(&self) -> BTreeMap<SectionKind, usize> {
        let mut out = BTreeMap::new();
        for s in &self.sections {
            let n = if s.kind.is_journal() {
                let mut groups = BTreeSet::new();
                let mut loose = 0;
                for r in s.filled() {
                    match s.je_group_of(r) {
                        Some(g) => {
                            groups.insert(g);
                        }
                        None => loose += 1,
                    }
                }
                groups.len() + loose
            } else {
                s.filled_count()
            };
            if n > 0 {
                *out.entry(s.kind).or_default() += n;
            }
        }
        out
    }

    /// Rows with a problem the app found, other than the lock mark.
    #[must_use]
    pub fn problem_rows(&self, closing: Option<CalendarDate>) -> BTreeSet<RowKey> {
        self.local_problems(closing)
            .into_iter()
            .filter(|(_, p)| !p.is_lock_mark())
            .map(|(k, _)| k)
            .collect()
    }
}

/// The local problems of the rows, grouped by row (for drawing).
#[must_use]
pub fn problems_by_row(list: Vec<(RowKey, Problem)>) -> BTreeMap<RowKey, Vec<Problem>> {
    let mut out: BTreeMap<RowKey, Vec<Problem>> = BTreeMap::new();
    for (k, p) in list {
        out.entry(k).or_default().push(p);
    }
    out
}
