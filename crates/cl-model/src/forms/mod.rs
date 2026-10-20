//! The form framework: one state machine every transaction form uses.
//!
//! ```text
//!            edit                 begin_save            save_ok
//!   Clean ----------> Dirty ----------------> Saving ----------> Clean
//!     ^                 ^  \                     |
//!     |   undo/revert   |   \ edit               | save_failed
//!     +-----------------+    +-- Failed <-------+
//! ```
//!
//! * Save & New (Alt+N) and Save & Close (Alt+S) are [`SaveIntent`]s; the
//!   intent is remembered across the (asynchronous) save and returned by
//!   [`FormState::save_ok`] so the shell knows what to do next.
//! * Enter in a header field is Save & New (Q-190); Enter in a grid row records
//!   the row ([`FormState::enter`]).
//! * Esc reverts the focused field, then closes when nothing is dirty, and asks
//!   first when the form is dirty ([`FormState::escape`]).
//! * Undo and redo cover unsaved edits only (Q-59).
//! * Payee prefill (Q-191) copies everything from the last transaction except
//!   amounts, as one undoable step ([`PayeePrefill`]).
//!
//! The value type `V` is the form's complete editable content. It must not be
//! used to derive balances: the engine owns accounting (non-negotiable 7).

use cl_core::OwnerMessage;

use crate::undo::UndoStack;

pub mod amount_words;
pub mod card;
pub mod check;
pub mod lines;
pub mod loan_payment;
pub mod owner_money;
pub mod personal_funds;

/// How many unsaved steps a form remembers.
pub const UNDO_LIMIT: usize = 200;

/// What the owner asked for when saving.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveIntent {
    /// Save and stay on the saved record.
    Save,
    /// Save & New (Alt+N, Enter in a header field).
    SaveAndNew,
    /// Save & Close (Alt+S).
    SaveAndClose,
}

/// What happens after a successful save.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AfterSave {
    /// Keep showing the saved record.
    Stay,
    /// Open a blank form of the same kind.
    NewForm,
    /// Close the window.
    CloseWindow,
}

impl From<SaveIntent> for AfterSave {
    fn from(i: SaveIntent) -> AfterSave {
        match i {
            SaveIntent::Save => AfterSave::Stay,
            SaveIntent::SaveAndNew => AfterSave::NewForm,
            SaveIntent::SaveAndClose => AfterSave::CloseWindow,
        }
    }
}

/// A failed save, kept so the form can show it and stay editable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveFailure {
    /// Plain-language message and fix keys (Q-119).
    pub message: OwnerMessage,
    /// Scrubbed technical detail for "Copy details".
    pub detail: String,
}

/// Where the form is in its life cycle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    /// Nothing changed since it was opened or saved.
    Clean,
    /// Unsaved edits.
    Dirty,
    /// A save is in flight; edits are locked.
    Saving(SaveIntent),
    /// The last save failed; the edits are still here.
    Failed(SaveFailure),
}

/// The answer to "can I save now?".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveStart<V> {
    /// Send this snapshot to the engine.
    Send(V),
    /// Nothing to save; just do the follow-up action.
    Skip(AfterSave),
    /// A save is already running.
    Busy,
}

/// What Esc does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EscAction {
    /// Put the focused field back to its saved value.
    RevertField,
    /// Nothing is dirty: close the form.
    Close,
    /// Dirty: ask Save / Discard / Keep editing before closing.
    ConfirmClose,
    /// A save is running; ignore.
    Ignore,
}

/// Where Enter was pressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnterContext {
    /// A header field (date, payee, number...).
    Header,
    /// A grid row (register, line items).
    GridRow,
}

/// What Enter does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnterAction {
    /// Save & New (Q-190).
    SaveAndNew,
    /// Record the grid row.
    RecordRow,
}

/// What to do when the owner tries to leave (close, switch record, quit).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeaveDecision {
    /// Safe to leave.
    Leave,
    /// Prompt: Save, Discard, or Keep editing.
    Prompt,
    /// A save is running; wait for it.
    Wait,
}

/// Forms that can start from the last transaction for the same payee
/// (Q-191). Implementations copy accounts, memo, terms, class and tags,
/// and must leave every amount for the owner (non-negotiable 4: amounts come
/// from the user, the bank file, or the source document).
pub trait PayeePrefill {
    /// Copy what is reusable from `last` into `self`, never an amount.
    fn prefill_from_last(&mut self, last: &Self);
}

/// Form state over the editable value `V`.
#[derive(Debug, Clone)]
pub struct FormState<V> {
    original: V,
    current: V,
    phase: Phase,
    undo: UndoStack<V>,
    revision: u64,
}

impl<V: Clone + PartialEq> FormState<V> {
    /// A clean form showing `initial`.
    #[must_use]
    pub fn new(initial: V) -> FormState<V> {
        FormState {
            original: initial.clone(),
            current: initial,
            phase: Phase::Clean,
            undo: UndoStack::new(UNDO_LIMIT),
            revision: 0,
        }
    }

    /// A counter that changes whenever the value on screen changes (an edit,
    /// undo, redo, save, revert or load). Grids compare it with the revision
    /// their open editor started from and reload the cell when it moved
    /// (P10 plan Rev 2 §2.5, B3).
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.revision
    }

    fn bump(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    /// The value on screen.
    #[must_use]
    pub fn current(&self) -> &V {
        &self.current
    }

    /// The last saved (or opened) value.
    #[must_use]
    pub fn original(&self) -> &V {
        &self.original
    }

    /// The life-cycle phase.
    #[must_use]
    pub fn phase(&self) -> &Phase {
        &self.phase
    }

    /// True when there are unsaved edits (also while saving and after a failure).
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.current != self.original
    }

    /// True while a save is in flight.
    #[must_use]
    pub fn is_saving(&self) -> bool {
        matches!(self.phase, Phase::Saving(_))
    }

    fn settle(&mut self) {
        self.phase = if self.is_dirty() {
            Phase::Dirty
        } else {
            Phase::Clean
        };
    }

    /// Change the value. `field` identifies the edited field so typing in it
    /// is one undo step. Returns false (and changes nothing) while saving.
    pub fn edit(&mut self, field: u64, f: impl FnOnce(&mut V)) -> bool {
        if self.is_saving() {
            return false;
        }
        self.change(Some(field), f)
    }

    /// Change the value as one undo step of its own that never merges with
    /// the step before or after, even for the same field (every grid cell
    /// commit, a row revert). Returns false (and changes nothing) while
    /// saving.
    pub fn edit_step(&mut self, f: impl FnOnce(&mut V)) -> bool {
        if self.is_saving() {
            return false;
        }
        let changed = self.change(None, f);
        self.undo.break_run();
        changed
    }

    fn change(&mut self, key: Option<u64>, f: impl FnOnce(&mut V)) -> bool {
        let before = self.current.clone();
        f(&mut self.current);
        if self.current != before {
            self.undo.record(&before, key);
            self.bump();
        }
        self.settle();
        true
    }

    /// Ctrl+Z. Returns true when something was undone.
    pub fn undo(&mut self) -> bool {
        if self.is_saving() {
            return false;
        }
        match self.undo.undo(&self.current) {
            Some(prev) => {
                self.current = prev;
                self.bump();
                self.settle();
                true
            }
            None => false,
        }
    }

    /// Ctrl+Y. Returns true when something was redone.
    pub fn redo(&mut self) -> bool {
        if self.is_saving() {
            return false;
        }
        match self.undo.redo(&self.current) {
            Some(next) => {
                self.current = next;
                self.bump();
                self.settle();
                true
            }
            None => false,
        }
    }

    /// True when Undo is available.
    #[must_use]
    pub fn can_undo(&self) -> bool {
        !self.is_saving() && self.undo.can_undo()
    }

    /// True when Redo is available.
    #[must_use]
    pub fn can_redo(&self) -> bool {
        !self.is_saving() && self.undo.can_redo()
    }

    /// Start a save.
    pub fn begin_save(&mut self, intent: SaveIntent) -> SaveStart<V> {
        if self.is_saving() {
            return SaveStart::Busy;
        }
        if !self.is_dirty() {
            return SaveStart::Skip(intent.into());
        }
        self.phase = Phase::Saving(intent);
        SaveStart::Send(self.current.clone())
    }

    /// The engine accepted the save and returned the stored record. The form
    /// becomes clean on that record and the undo history ends (Q-59).
    /// Returns what to do next; `Stay` if no save was running.
    pub fn save_ok(&mut self, stored: V) -> AfterSave {
        let after = match self.phase {
            Phase::Saving(intent) => intent.into(),
            _ => AfterSave::Stay,
        };
        self.original = stored.clone();
        self.current = stored;
        self.undo.clear();
        self.phase = Phase::Clean;
        self.bump();
        after
    }

    /// The save failed; edits stay and the form can be fixed and saved again.
    pub fn save_failed(&mut self, failure: SaveFailure) {
        if self.is_saving() {
            self.phase = Phase::Failed(failure);
        }
    }

    /// Esc. `field_dirty` is true when the focused field differs from its
    /// saved value.
    #[must_use]
    pub fn escape(&self, field_dirty: bool) -> EscAction {
        if self.is_saving() {
            EscAction::Ignore
        } else if field_dirty {
            EscAction::RevertField
        } else if self.is_dirty() {
            EscAction::ConfirmClose
        } else {
            EscAction::Close
        }
    }

    /// Enter pressed in `ctx` (Q-190).
    #[must_use]
    pub fn enter(&self, ctx: EnterContext) -> EnterAction {
        match ctx {
            EnterContext::Header => EnterAction::SaveAndNew,
            EnterContext::GridRow => EnterAction::RecordRow,
        }
    }

    /// Whether leaving (close, Prev/Next record, quit) needs a prompt.
    #[must_use]
    pub fn leave(&self) -> LeaveDecision {
        if self.is_saving() {
            LeaveDecision::Wait
        } else if self.is_dirty() {
            LeaveDecision::Prompt
        } else {
            LeaveDecision::Leave
        }
    }

    /// Throw away unsaved edits ("Clear" and "Discard").
    pub fn revert_form(&mut self) {
        if self.is_saving() {
            return;
        }
        self.current = self.original.clone();
        self.undo.clear();
        self.phase = Phase::Clean;
        self.bump();
    }

    /// Show a different record (Prev/Next, Find). Clean afterwards.
    pub fn load(&mut self, record: V) {
        self.original = record.clone();
        self.current = record;
        self.undo.clear();
        self.phase = Phase::Clean;
        self.bump();
    }
}

impl<V: Clone + PartialEq + PayeePrefill> FormState<V> {
    /// Fill the form from the last transaction for the payee, as one undoable
    /// step (Q-191).
    pub fn apply_prefill(&mut self, field: u64, last: &V) -> bool {
        self.edit(field, |v| v.prefill_from_last(last))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq, Default)]
    struct Check {
        payee: String,
        account: String,
        memo: String,
        /// Minor units; a real form uses `cl_core::Money`.
        amount_cents: i64,
    }

    impl PayeePrefill for Check {
        fn prefill_from_last(&mut self, last: &Check) {
            self.account.clone_from(&last.account);
            self.memo.clone_from(&last.memo);
        }
    }

    fn failure() -> SaveFailure {
        SaveFailure {
            message: OwnerMessage::plain("error.test"),
            detail: "ApiError::Test".into(),
        }
    }

    fn typed(f: &mut FormState<Check>, text: &str) {
        f.edit(1, |c| c.payee = text.to_string());
    }

    #[test]
    fn clean_to_dirty_and_back_by_editing_back() {
        let mut f = FormState::new(Check::default());
        assert_eq!(f.phase(), &Phase::Clean);
        typed(&mut f, "Acme");
        assert_eq!(f.phase(), &Phase::Dirty);
        typed(&mut f, "");
        assert_eq!(f.phase(), &Phase::Clean, "back to the saved value is clean");
    }

    #[test]
    fn save_and_new_flow() {
        let mut f = FormState::new(Check::default());
        typed(&mut f, "Acme");
        let SaveStart::Send(snapshot) = f.begin_save(SaveIntent::SaveAndNew) else {
            panic!("a dirty form sends");
        };
        assert_eq!(snapshot.payee, "Acme");
        assert!(f.is_saving());
        assert_eq!(f.begin_save(SaveIntent::Save), SaveStart::Busy);
        assert!(
            !f.edit(1, |c| c.payee = "X".into()),
            "edits locked while saving"
        );
        let after = f.save_ok(snapshot);
        assert_eq!(after, AfterSave::NewForm);
        assert_eq!(f.phase(), &Phase::Clean);
        assert!(!f.can_undo(), "undo ends at save");
    }

    #[test]
    fn save_and_close_and_plain_save_intents() {
        for (intent, want) in [
            (SaveIntent::SaveAndClose, AfterSave::CloseWindow),
            (SaveIntent::Save, AfterSave::Stay),
        ] {
            let mut f = FormState::new(Check::default());
            typed(&mut f, "A");
            let SaveStart::Send(s) = f.begin_save(intent) else {
                panic!()
            };
            assert_eq!(f.save_ok(s), want);
        }
    }

    #[test]
    fn saving_a_clean_form_skips_to_the_follow_up() {
        let mut f = FormState::new(Check::default());
        assert_eq!(
            f.begin_save(SaveIntent::SaveAndNew),
            SaveStart::Skip(AfterSave::NewForm)
        );
        assert_eq!(f.phase(), &Phase::Clean);
    }

    #[test]
    fn failed_save_keeps_edits_and_allows_retry() {
        let mut f = FormState::new(Check::default());
        typed(&mut f, "Acme");
        let SaveStart::Send(_) = f.begin_save(SaveIntent::SaveAndClose) else {
            panic!()
        };
        f.save_failed(failure());
        assert!(matches!(f.phase(), Phase::Failed(_)));
        assert!(f.is_dirty());
        assert_eq!(f.current().payee, "Acme");
        // Editing clears the failure state back to Dirty.
        typed(&mut f, "Acme Co");
        assert_eq!(f.phase(), &Phase::Dirty);
        let SaveStart::Send(s) = f.begin_save(SaveIntent::SaveAndClose) else {
            panic!()
        };
        assert_eq!(f.save_ok(s), AfterSave::CloseWindow);
    }

    #[test]
    fn failure_without_a_running_save_is_ignored() {
        let mut f = FormState::new(Check::default());
        f.save_failed(failure());
        assert_eq!(f.phase(), &Phase::Clean);
    }

    #[test]
    fn retry_after_failure_can_save_without_new_edits() {
        let mut f = FormState::new(Check::default());
        typed(&mut f, "Acme");
        let _ = f.begin_save(SaveIntent::Save);
        f.save_failed(failure());
        assert!(matches!(f.begin_save(SaveIntent::Save), SaveStart::Send(_)));
    }

    #[test]
    fn enter_is_save_and_new_in_headers_and_record_in_grids() {
        let f = FormState::new(Check::default());
        assert_eq!(f.enter(EnterContext::Header), EnterAction::SaveAndNew);
        assert_eq!(f.enter(EnterContext::GridRow), EnterAction::RecordRow);
    }

    #[test]
    fn escape_chain() {
        let mut f = FormState::new(Check::default());
        assert_eq!(f.escape(false), EscAction::Close);
        typed(&mut f, "Acme");
        assert_eq!(f.escape(true), EscAction::RevertField, "field first");
        assert_eq!(f.escape(false), EscAction::ConfirmClose, "then the prompt");
        f.revert_form();
        assert_eq!(f.escape(false), EscAction::Close);
        typed(&mut f, "Acme");
        let _ = f.begin_save(SaveIntent::Save);
        assert_eq!(f.escape(true), EscAction::Ignore);
    }

    #[test]
    fn leave_decisions() {
        let mut f = FormState::new(Check::default());
        assert_eq!(f.leave(), LeaveDecision::Leave);
        typed(&mut f, "x");
        assert_eq!(f.leave(), LeaveDecision::Prompt);
        let _ = f.begin_save(SaveIntent::Save);
        assert_eq!(f.leave(), LeaveDecision::Wait);
    }

    #[test]
    fn undo_redo_cover_unsaved_edits_only() {
        let mut f = FormState::new(Check::default());
        f.edit(1, |c| c.payee = "A".into());
        f.edit(2, |c| c.memo = "m".into());
        assert!(f.undo());
        assert_eq!(f.current().memo, "");
        assert_eq!(f.current().payee, "A");
        assert!(f.redo());
        assert_eq!(f.current().memo, "m");
        assert!(f.undo() && f.undo());
        assert_eq!(f.phase(), &Phase::Clean);
        assert!(!f.undo());
        f.edit(1, |c| c.payee = "B".into());
        let SaveStart::Send(s) = f.begin_save(SaveIntent::Save) else {
            panic!()
        };
        f.save_ok(s);
        assert!(!f.undo(), "a saved change is not undoable (Q-59)");
    }

    #[test]
    fn typing_in_one_field_is_one_undo_step() {
        let mut f = FormState::new(Check::default());
        for t in ["A", "Ac", "Acm", "Acme"] {
            typed(&mut f, t);
        }
        assert!(f.undo());
        assert_eq!(f.current().payee, "");
        assert!(!f.can_undo());
    }

    #[test]
    fn no_op_edit_is_not_an_undo_step() {
        let mut f = FormState::new(Check::default());
        f.edit(1, |_| {});
        assert!(!f.can_undo());
    }

    #[test]
    fn prefill_copies_everything_but_amounts_as_one_step() {
        let last = Check {
            payee: "Acme".into(),
            account: "Office Supplies".into(),
            memo: "Monthly".into(),
            amount_cents: 12_345,
        };
        let mut f = FormState::new(Check {
            payee: "Acme".into(),
            ..Check::default()
        });
        assert!(f.apply_prefill(9, &last));
        assert_eq!(f.current().account, "Office Supplies");
        assert_eq!(f.current().memo, "Monthly");
        assert_eq!(f.current().amount_cents, 0, "amounts are never prefilled");
        assert!(f.undo());
        assert_eq!(f.current().account, "");
    }

    #[test]
    fn revert_and_load() {
        let mut f = FormState::new(Check::default());
        typed(&mut f, "x");
        f.revert_form();
        assert_eq!(f.current(), &Check::default());
        assert!(!f.can_undo());
        typed(&mut f, "y");
        f.load(Check {
            payee: "next".into(),
            ..Check::default()
        });
        assert_eq!(f.phase(), &Phase::Clean);
        assert_eq!(f.original().payee, "next");
    }

    #[test]
    fn edit_steps_never_merge() {
        let mut f = FormState::new(Check::default());
        // The same cell committed twice (Tab out, Shift+Tab back, retype).
        f.edit_step(|c| c.amount_cents = 100);
        f.edit_step(|c| c.amount_cents = 200);
        assert!(f.undo());
        assert_eq!(f.current().amount_cents, 100, "first typed value (T5)");
        assert!(f.undo());
        assert_eq!(f.current().amount_cents, 0);
        assert!(f.redo());
        assert_eq!(f.current().amount_cents, 100);
        // A typed field edit right after a step does not merge into it.
        let mut g = FormState::new(Check::default());
        g.edit_step(|c| c.memo = "a".into());
        g.edit(7, |c| c.memo = "ab".into());
        g.edit(7, |c| c.memo = "abc".into());
        assert!(g.undo());
        assert_eq!(g.current().memo, "a");
        // Steps that change nothing record nothing.
        let mut h = FormState::new(Check::default());
        assert!(h.edit_step(|_| {}));
        assert!(!h.can_undo());
    }

    #[test]
    fn revision_moves_on_every_change() {
        let mut f = FormState::new(Check::default());
        let r0 = f.revision();
        f.edit_step(|_| {});
        assert_eq!(f.revision(), r0, "no change, no revision");
        f.edit_step(|c| c.amount_cents = 5);
        let r1 = f.revision();
        assert_ne!(r1, r0);
        f.edit(1, |c| c.memo = "m".into());
        let r2 = f.revision();
        assert_ne!(r2, r1);
        f.undo();
        let r3 = f.revision();
        assert_ne!(r3, r2);
        f.redo();
        let r4 = f.revision();
        assert_ne!(r4, r3);
        f.revert_form();
        let r5 = f.revision();
        assert_ne!(r5, r4);
        f.load(Check::default());
        let r6 = f.revision();
        assert_ne!(r6, r5);
        f.edit_step(|c| c.amount_cents = 9);
        let _ = f.begin_save(SaveIntent::Save);
        assert!(!f.edit_step(|c| c.amount_cents = 10), "locked while saving");
        let r7 = f.revision();
        f.save_ok(Check::default());
        assert_ne!(f.revision(), r7);
        assert!(!f.undo(), "no undo after save");
    }
}
