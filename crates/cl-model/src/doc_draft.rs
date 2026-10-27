//! Review Documents and the compare tools, without egui (A04 plan D2, D4,
//! D6). Frozen by WP-M; every later A04 work package builds on these types:
//!
//! - **Kinds and forms** ([`DocKind`], [`DraftForm`]): which real form a
//!   draft opens in (D2). `cl-ui` maps `cl_api::DraftKind` onto [`DocKind`]
//!   (this crate may not depend on `cl-api`). Used by WP-F2.
//! - **The queue** ([`QueueStatus`], [`classify`], [`QueueRow`],
//!   [`QueueCursor`]): status in words, and J/K selection that stays on the
//!   same draft across a refresh (D6, AC1). Used by WP-F1 (the list) and
//!   WP-F2 (Page Up/Down through the queue from a draft form).
//! - **Compare** ([`PageBox`], [`TargetKey`], [`CompareTarget`],
//!   [`TargetLevel`], [`FillKind`], [`FillArm`], [`CompareFeed`]): a form's
//!   fields beside their spots on the page, and which field a click on a
//!   word fills (D4, AC2). Used by WP-C (the widget and the pop-out) and
//!   WP-F2 (the forms publish their targets). A06 reuses them for
//!   statements, so [`TargetKey`] is not tied to the bill's fields.
//!
//! Integers only: page positions are ten-thousandths of the page side, as
//! `cl_docread::model::Bbox`; all float geometry stays in `cl-ui`'s
//! `widgets/compare/overlay.rs` (plan §4).

use crate::forms::doc_read::{Confidence, ReadField, ReadTarget};
use crate::rowkey::RowKey;
use crate::text::TextKey;

// ---------------------------------------------------------------- limits

/// Drafts per queue page (`GET /document-drafts?limit=50`, D6).
pub const QUEUE_PAGE: u32 = 50;
/// Drafts the inbox follows per request batch (`SWEEP_BATCH`, D6, WP-I).
pub const FOLLOW_BATCH: usize = 10;
/// Duplicate checks the queue runs at once (D6, WP-F1).
pub const DUPLICATE_CHECKS_AT_ONCE: usize = 2;
/// Drafts whose page words a session keeps (the LRU of D4, WP-C).
pub const WORD_CACHE_DRAFTS: usize = 20;
/// Rows per register search page when looking for a match (D3, WP-F2).
pub const REGISTER_PAGE: u32 = 200;

// ------------------------------------------------------- kinds and forms

/// What a document is (the engine's `document_drafts.kind`). `cl-ui` maps
/// `cl_api::DraftKind` one to one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DocKind {
    /// A vendor bill.
    Bill,
    /// A card charge.
    CardCharge,
    /// A check already written.
    Check,
    /// An expense paid from a bank or cash account.
    CashExpense,
    /// Not known: the owner chooses (Q-A04n).
    Unknown,
    /// A vendor statement (A06, U-94): opens in Match Vendor Statement, never
    /// converts, and is not one of the chooser's kinds.
    Statement,
}

impl DocKind {
    /// Every kind, in the chooser's order (Unknown last).
    pub const ALL: [DocKind; 5] = [
        DocKind::Bill,
        DocKind::CardCharge,
        DocKind::Check,
        DocKind::CashExpense,
        DocKind::Unknown,
    ];

    /// Stable name (the engine's wire name; control channel).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            DocKind::Bill => "bill",
            DocKind::CardCharge => "card_charge",
            DocKind::Check => "check",
            DocKind::CashExpense => "cash_expense",
            DocKind::Unknown => "unknown",
            DocKind::Statement => "statement",
        }
    }

    /// The form a draft of this kind opens in; `None` for
    /// [`DocKind::Unknown`], which opens the "What is this document?"
    /// chooser with no default (Q-A04n), and for [`DocKind::Statement`],
    /// which opens Match Vendor Statement instead (A06).
    #[must_use]
    pub const fn form(self) -> Option<DraftForm> {
        match self {
            DocKind::Bill => Some(DraftForm::EnterBill),
            DocKind::CardCharge => Some(DraftForm::CardCharge),
            DocKind::Check => Some(DraftForm::WriteCheck),
            DocKind::CashExpense => Some(DraftForm::CashExpense),
            DocKind::Unknown | DocKind::Statement => None,
        }
    }
}

/// The real form a draft opens in (D2 "Kind → form"). Accept is that form's
/// Save & Close / Save & New; nothing posts from the list (Q-A04b).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DraftForm {
    /// Enter Bill.
    EnterBill,
    /// Record Card Charge (Credit hidden in draft mode, Q-A04g).
    CardCharge,
    /// Record Card Charge in "Expense paid from" mode: bank and cash
    /// accounts (Q-A04g).
    CashExpense,
    /// Write Check (Print later off).
    WriteCheck,
}

impl DraftForm {
    /// The chooser's choices and "Enter this as", in order.
    pub const ALL: [DraftForm; 4] = [
        DraftForm::EnterBill,
        DraftForm::CardCharge,
        DraftForm::WriteCheck,
        DraftForm::CashExpense,
    ];

    /// The command whose window shows the form.
    #[must_use]
    pub const fn command(self) -> &'static str {
        match self {
            DraftForm::EnterBill => "vendors.enter_bill",
            DraftForm::CardCharge | DraftForm::CashExpense => "vendors.card_charge",
            DraftForm::WriteCheck => "vendors.write_check",
        }
    }

    /// The kind a draft converts as when entered on this form.
    #[must_use]
    pub const fn kind(self) -> DocKind {
        match self {
            DraftForm::EnterBill => DocKind::Bill,
            DraftForm::CardCharge => DocKind::CardCharge,
            DraftForm::CashExpense => DocKind::CashExpense,
            DraftForm::WriteCheck => DocKind::Check,
        }
    }
}

// ----------------------------------------------------------------- queue

/// A live draft's engine status (`new`, `ready`, `failed`). Converted and
/// discarded drafts are never in the queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DraftState {
    /// Claimed, not read yet.
    New,
    /// Read, waiting for the owner.
    Ready,
    /// The reading failed.
    Failed,
}

/// The queue's lazy duplicate check of one row (D6: two at a time, cached
/// by draft id and change token).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum DuplicateCheck {
    /// Not asked yet.
    #[default]
    NotChecked,
    /// Asked; the row shows "Checking…".
    Checking,
    /// Nothing in the books looks like it.
    Clear,
    /// A transaction or bill may already be this document.
    Possible,
    /// The check couldn't be made (the row says nothing about duplicates;
    /// Accept checks again, D3).
    Failed,
}

/// A queue row's status in words (D6). Every row's AccessKit label says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum QueueStatus {
    /// Claimed, not read yet ("Waiting to be read").
    WaitingToBeRead,
    /// Read; every field sure.
    Ready,
    /// Read; some fields need a look ("Check these fields").
    CheckFields,
    /// Something in the books may already be this document.
    PossibleDuplicate,
    /// The reading failed ("Couldn't read").
    CouldntRead,
}

impl QueueStatus {
    /// Stable name (`state.queue`).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            QueueStatus::WaitingToBeRead => "waiting",
            QueueStatus::Ready => "ready",
            QueueStatus::CheckFields => "check_fields",
            QueueStatus::PossibleDuplicate => "possible_duplicate",
            QueueStatus::CouldntRead => "couldnt_read",
        }
    }

    /// The words shown (strings live in `cl-ui::i18n`).
    #[must_use]
    pub const fn key(self) -> TextKey {
        match self {
            QueueStatus::WaitingToBeRead => TextKey("doc_review.status.waiting"),
            QueueStatus::Ready => TextKey("doc_review.status.ready"),
            QueueStatus::CheckFields => TextKey("doc_review.status.check_fields"),
            QueueStatus::PossibleDuplicate => TextKey("doc_review.status.possible_duplicate"),
            QueueStatus::CouldntRead => TextKey("doc_review.status.couldnt_read"),
        }
    }
}

/// A row's status: a failed reading is "Couldn't read"; an unread draft is
/// "Waiting to be read"; a read draft is "Possible duplicate" when the
/// duplicate check found one, else "Check these fields" when
/// `needs_check` (a field below sure, or a reading check), else "Ready".
#[must_use]
pub const fn classify(state: DraftState, needs_check: bool, dup: DuplicateCheck) -> QueueStatus {
    match (state, dup) {
        (DraftState::Failed, _) => QueueStatus::CouldntRead,
        (DraftState::New, _) => QueueStatus::WaitingToBeRead,
        (DraftState::Ready, DuplicateCheck::Possible) => QueueStatus::PossibleDuplicate,
        (DraftState::Ready, _) if needs_check => QueueStatus::CheckFields,
        (DraftState::Ready, _) => QueueStatus::Ready,
    }
}

/// True when a read field's confidence asks the owner to look ("Check
/// these fields"): anything below [`Confidence::High`].
#[must_use]
pub const fn needs_check(level: Confidence) -> bool {
    !matches!(level, Confidence::High)
}

/// One queue row: ids, kind and status only (no amounts, names or file
/// names; `Debug` is safe). `Id` is the engine draft id in `cl-ui`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueRow<Id> {
    /// The draft.
    pub id: Id,
    /// Its change token (the duplicate cache's key with `id`).
    pub updated_at: String,
    /// What it is.
    pub kind: DocKind,
    /// Its engine status.
    pub state: DraftState,
    /// A read field below sure, or a reading check.
    pub needs_check: bool,
    /// The lazy duplicate check.
    pub duplicate: DuplicateCheck,
}

impl<Id> QueueRow<Id> {
    /// The row's status in words ([`classify`]).
    #[must_use]
    pub const fn status(&self) -> QueueStatus {
        classify(self.state, self.needs_check, self.duplicate)
    }
}

/// A direction through the queue: J / Page Down, K / Page Up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Step {
    /// The next row (J, Page Down in a draft form).
    Next,
    /// The previous row (K, Page Up in a draft form).
    Prev,
}

/// The queue's selection, by draft id, so a refresh never moves it to
/// another document (D6, AC1 "J/K stable across refresh"). When the
/// selected draft leaves the list (entered, discarded, entered elsewhere)
/// the row that took its place is selected: after Accept the next document
/// is selected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueCursor<Id> {
    selected: Option<Id>,
    /// Where the selection was, for when its row leaves.
    index: usize,
}

impl<Id> Default for QueueCursor<Id> {
    fn default() -> Self {
        QueueCursor {
            selected: None,
            index: 0,
        }
    }
}

impl<Id: Clone + PartialEq> QueueCursor<Id> {
    /// Nothing selected.
    #[must_use]
    pub fn new() -> Self {
        QueueCursor::default()
    }

    /// The selected draft.
    #[must_use]
    pub fn selected(&self) -> Option<&Id> {
        self.selected.as_ref()
    }

    /// The selected row's position in `ids`.
    #[must_use]
    pub fn position(&self, ids: &[Id]) -> Option<usize> {
        let s = self.selected.as_ref()?;
        ids.iter().position(|i| i == s)
    }

    /// Selects `id` when it is in `ids`. Returns whether it is.
    pub fn select(&mut self, ids: &[Id], id: &Id) -> bool {
        match ids.iter().position(|i| i == id) {
            Some(at) => {
                self.selected = Some(id.clone());
                self.index = at;
                true
            }
            None => false,
        }
    }

    /// The list was loaded again: the same draft stays selected wherever it
    /// moved; a draft that left is replaced by the row now at its place
    /// (the last row when it was last); an empty list selects nothing. A
    /// cursor with nothing selected selects the first row.
    pub fn refresh(&mut self, ids: &[Id]) {
        if let Some(at) = self.position(ids) {
            self.index = at;
            return;
        }
        if ids.is_empty() {
            self.selected = None;
            self.index = 0;
            return;
        }
        let at = if self.selected.is_some() {
            self.index.min(ids.len() - 1)
        } else {
            0
        };
        self.index = at;
        self.selected = ids.get(at).cloned();
    }

    /// The row one `step` from the selection (Page Up/Down from a draft
    /// form), without moving; `None` at either end, or with nothing
    /// selected.
    #[must_use]
    pub fn neighbour(&self, ids: &[Id], step: Step) -> Option<Id> {
        let at = self.position(ids)?;
        let to = match step {
            Step::Next => at.checked_add(1)?,
            Step::Prev => at.checked_sub(1)?,
        };
        ids.get(to).cloned()
    }

    /// Moves one `step` (J/K) and returns the new selection. At either end
    /// the selection stays; with nothing selected J and K select the first
    /// row.
    pub fn step(&mut self, ids: &[Id], step: Step) -> Option<&Id> {
        if self.position(ids).is_none() {
            self.selected = None;
            self.refresh(ids);
            return self.selected.as_ref();
        }
        if let Some(to) = self.neighbour(ids, step) {
            self.select(ids, &to);
        }
        self.selected.as_ref()
    }
}

// --------------------------------------------------------------- compare

/// A box on one page of the **original** document, in ten-thousandths of
/// the page's width (`l`, `r`) and height (`t`, `b`), `0..=10000`, like
/// `cl_docread::model::Bbox`. `page` counts from 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PageBox {
    /// Page, from 0.
    pub page: u16,
    /// Left edge.
    pub l: u16,
    /// Top edge.
    pub t: u16,
    /// Right edge.
    pub r: u16,
    /// Bottom edge.
    pub b: u16,
}

impl PageBox {
    /// A page side in box units.
    pub const SIDE: u16 = 10_000;
    /// The smallest box side a region re-read takes: 1% of the page side
    /// (D4 "Drag a box").
    pub const MIN_SIDE: u16 = 100;
    /// A box wider **and** taller than this (90% of the page) reads the
    /// whole page instead (D4).
    pub const WHOLE_PAGE: u16 = 9_000;

    /// A box, when `l <= r`, `t <= b` and every edge is on the page.
    #[must_use]
    pub const fn new(page: u16, l: u16, t: u16, r: u16, b: u16) -> Option<PageBox> {
        if l <= r && t <= b && r <= PageBox::SIDE && b <= PageBox::SIDE {
            Some(PageBox { page, l, t, r, b })
        } else {
            None
        }
    }

    /// Width in box units.
    #[must_use]
    pub const fn width(self) -> u16 {
        self.r.saturating_sub(self.l)
    }

    /// Height in box units.
    #[must_use]
    pub const fn height(self) -> u16 {
        self.b.saturating_sub(self.t)
    }

    /// Too small to re-read (a side under [`PageBox::MIN_SIDE`]).
    #[must_use]
    pub const fn too_small(self) -> bool {
        self.width() < PageBox::MIN_SIDE || self.height() < PageBox::MIN_SIDE
    }

    /// Nearly the whole page (both sides over [`PageBox::WHOLE_PAGE`]): a
    /// plain read instead of a region re-read.
    #[must_use]
    pub const fn whole_page(self) -> bool {
        self.width() > PageBox::WHOLE_PAGE && self.height() > PageBox::WHOLE_PAGE
    }

    /// True when `other` is on the same page and lies inside this box
    /// (edges included): which cached words a dragged box picks.
    #[must_use]
    pub const fn contains(self, other: PageBox) -> bool {
        self.page == other.page
            && other.l >= self.l
            && other.r <= self.r
            && other.t >= self.t
            && other.b <= self.b
    }
}

/// Which form field a compare target is. `field` is a stable snake-case
/// name ([`ReadTarget`] names for the expense forms; A06 adds its own);
/// `row` is set for a line's field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TargetKey {
    /// Stable field name.
    pub field: &'static str,
    /// The line, for a line's field.
    pub row: Option<RowKey>,
}

impl TargetKey {
    /// A header field.
    #[must_use]
    pub const fn field(field: &'static str) -> TargetKey {
        TargetKey { field, row: None }
    }

    /// A line's field.
    #[must_use]
    pub const fn line(field: &'static str, row: RowKey) -> TargetKey {
        TargetKey {
            field,
            row: Some(row),
        }
    }
}

impl From<ReadTarget> for TargetKey {
    fn from(t: ReadTarget) -> TargetKey {
        match t {
            ReadTarget::Vendor => TargetKey::field("vendor"),
            ReadTarget::BillDate => TargetKey::field("bill_date"),
            ReadTarget::DueDate => TargetKey::field("due_date"),
            ReadTarget::Reference => TargetKey::field("reference"),
            ReadTarget::AmountDue => TargetKey::field("amount_due"),
            ReadTarget::Line(k) => TargetKey::line("line_amount", k),
            ReadTarget::Payee => TargetKey::field("payee"),
            ReadTarget::Date => TargetKey::field("date"),
            ReadTarget::Total => TargetKey::field("total"),
            ReadTarget::CheckNumber => TargetKey::field("check_number"),
            ReadTarget::PaidFrom => TargetKey::field("paid_from"),
        }
    }
}

/// How a target's highlight is drawn (D4): `accent` sure, `warn` plus a
/// "?" glyph check this, dashed `border.strong` not found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TargetLevel {
    /// Read and sure.
    Sure,
    /// Read, not sure: look at it.
    CheckThis,
    /// Not found on the page (no spot).
    NotFound,
}

impl TargetLevel {
    /// The level of a field from its read state and whether it has a spot:
    /// no spot is [`TargetLevel::NotFound`]; a field read as
    /// [`Confidence::High`] is [`TargetLevel::Sure`]; any other read level
    /// is [`TargetLevel::CheckThis`]. A field the owner accepted, edited
    /// or filled by a click is sure.
    #[must_use]
    pub const fn of(read: Option<ReadField>, has_spot: bool) -> TargetLevel {
        if !has_spot {
            return TargetLevel::NotFound;
        }
        match read {
            Some(ReadField::Read { level }) if needs_check(level) => TargetLevel::CheckThis,
            _ => TargetLevel::Sure,
        }
    }

    /// Stable name (`state.doc_viewer.highlight`).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            TargetLevel::Sure => "sure",
            TargetLevel::CheckThis => "check_this",
            TargetLevel::NotFound => "not_found",
        }
    }
}

/// What a click on a word may put into a field (D4 "Click to fill").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FillKind {
    /// One amount: exactly one word that parses alone, never two digit
    /// words joined (M4).
    Money,
    /// A date: words may join, only through the date parser.
    Date,
    /// Text: words join with single spaces.
    Text,
}

/// One form field beside its spot on the page (D4). `label` is the field's
/// label text (not owner data); the value is never carried here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompareTarget {
    /// The field.
    pub key: TargetKey,
    /// The field's label as shown ("Amount due").
    pub label: String,
    /// Where the value was read, when it was.
    pub spot: Option<PageBox>,
    /// How sure the reading was.
    pub level: TargetLevel,
    /// What a click on a word fills; `None` for fields filled from a list
    /// (vendor, accounts), which a click never fills.
    pub fill: Option<FillKind>,
}

/// The field a click on a word fills: the last focused fillable target
/// (D4). It stays armed while focus moves to the document (docked or
/// popped out) or to anything that isn't a target, so the owner can click
/// a field, then the page.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FillArm {
    armed: Option<(TargetKey, FillKind)>,
}

impl FillArm {
    /// Nothing armed.
    #[must_use]
    pub fn new() -> FillArm {
        FillArm::default()
    }

    /// Focus moved to `key` (`None`: the viewer or any widget that isn't a
    /// target). A fillable target arms; anything else keeps the arm.
    pub fn focus(&mut self, key: Option<TargetKey>, targets: &[CompareTarget]) {
        let Some(key) = key else {
            return;
        };
        if let Some(fill) = targets.iter().find(|t| t.key == key).and_then(|t| t.fill) {
            self.armed = Some((key, fill));
        }
    }

    /// The armed field and what it takes.
    #[must_use]
    pub fn armed(&self) -> Option<(TargetKey, FillKind)> {
        self.armed
    }

    /// Drops the arm when its target is gone (a deleted line, another
    /// draft) or no longer fillable.
    pub fn retain(&mut self, targets: &[CompareTarget]) {
        if let Some((key, _)) = self.armed
            && !targets.iter().any(|t| t.key == key && t.fill.is_some())
        {
            self.armed = None;
        }
    }

    /// Disarms (the form closed, "Clear and type it myself").
    pub fn clear(&mut self) {
        self.armed = None;
    }
}

/// What a form shows the compare tools each frame (D4, S7): its targets,
/// the armed and focused fields, and the Highlights switch. The pop-out
/// reads it through `Screen::compare()` beside `Screen::document()`;
/// `generation` changes whenever any of it does, so a viewer redraws only
/// then. No values, names or document text.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CompareFeed {
    /// Every target, in field order.
    pub targets: Vec<CompareTarget>,
    /// The field a click on a word fills.
    pub armed: Option<TargetKey>,
    /// The field with the keyboard (its box is outlined and its page shown).
    pub focused: Option<TargetKey>,
    /// Highlights on (`viewer.toggle_highlights`).
    pub highlights: bool,
    /// Bumped on every change.
    pub generation: u64,
}

#[cfg(test)]
mod tests;
