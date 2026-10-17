//! Undo and redo for unsaved edits in the open form (Q-59).
//!
//! Only edits made since the form was opened or last saved are undoable;
//! saved changes go through Edit / Delete / Void (all logged by the engine).
//! Snapshots are whole-value clones, which is simple and exact; forms are
//! small. Consecutive edits to the same field coalesce into one step so
//! typing a payee name is one Ctrl+Z, not one per letter.

/// Undo history over snapshots of `T`.
#[derive(Debug, Clone)]
pub struct UndoStack<T> {
    past: Vec<Step<T>>,
    future: Vec<T>,
    limit: usize,
}

#[derive(Debug, Clone)]
struct Step<T> {
    before: T,
    /// Edits with the same `Some(key)` in a row share one step.
    key: Option<u64>,
}

impl<T: Clone> UndoStack<T> {
    /// An empty history holding at most `limit` steps (oldest dropped).
    #[must_use]
    pub fn new(limit: usize) -> UndoStack<T> {
        UndoStack {
            past: Vec::new(),
            future: Vec::new(),
            limit: limit.max(1),
        }
    }

    /// Record that an edit is about to change `before`. `key` identifies the
    /// field so a run of edits to it becomes one step. Clears redo.
    pub fn record(&mut self, before: &T, key: Option<u64>) {
        self.future.clear();
        if let (Some(k), Some(last)) = (key, self.past.last())
            && last.key == Some(k)
        {
            return;
        }
        self.past.push(Step {
            before: before.clone(),
            key,
        });
        if self.past.len() > self.limit {
            self.past.remove(0);
        }
    }

    /// Step back. `current` is what is on screen now (kept for redo).
    pub fn undo(&mut self, current: &T) -> Option<T> {
        let step = self.past.pop()?;
        self.future.push(current.clone());
        Some(step.before)
    }

    /// Step forward again.
    pub fn redo(&mut self, current: &T) -> Option<T> {
        let next = self.future.pop()?;
        self.past.push(Step {
            before: current.clone(),
            key: None,
        });
        Some(next)
    }

    /// True when Ctrl+Z would do something.
    #[must_use]
    pub fn can_undo(&self) -> bool {
        !self.past.is_empty()
    }

    /// True when Ctrl+Y would do something.
    #[must_use]
    pub fn can_redo(&self) -> bool {
        !self.future.is_empty()
    }

    /// Forget everything (after a save or a revert).
    pub fn clear(&mut self) {
        self.past.clear();
        self.future.clear();
    }

    /// Stop coalescing: the next edit starts a new step even for the same field.
    pub fn break_run(&mut self) {
        if let Some(last) = self.past.last_mut() {
            last.key = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn undo_and_redo_walk_the_history() {
        let mut s = UndoStack::new(10);
        let mut v = 0;
        for next in [1, 2, 3] {
            s.record(&v, None);
            v = next;
        }
        assert_eq!(s.undo(&v), Some(2));
        assert_eq!(s.undo(&2), Some(1));
        assert!(s.can_redo());
        assert_eq!(s.redo(&1), Some(2));
        assert_eq!(s.redo(&2), Some(3));
        assert_eq!(s.redo(&3), None);
        assert_eq!(s.undo(&3), Some(2));
    }

    #[test]
    fn a_new_edit_clears_redo() {
        let mut s = UndoStack::new(10);
        s.record(&0, None);
        assert_eq!(s.undo(&1), Some(0));
        assert!(s.can_redo());
        s.record(&0, None);
        assert!(!s.can_redo());
    }

    #[test]
    fn same_field_edits_coalesce() {
        let mut s = UndoStack::new(10);
        s.record(&String::new(), Some(7));
        s.record(&"a".to_string(), Some(7));
        s.record(&"ab".to_string(), Some(7));
        s.record(&"ab".to_string(), Some(8));
        assert_eq!(s.undo(&"abc".to_string()), Some("ab".to_string()));
        assert_eq!(s.undo(&"ab".to_string()), Some(String::new()));
        assert_eq!(s.undo(&String::new()), None);
    }

    #[test]
    fn break_run_splits_a_coalesced_step() {
        let mut s = UndoStack::new(10);
        s.record(&0, Some(1));
        s.break_run();
        s.record(&5, Some(1));
        assert_eq!(s.undo(&9), Some(5));
        assert_eq!(s.undo(&5), Some(0));
    }

    #[test]
    fn history_is_bounded_and_clearable() {
        let mut s = UndoStack::new(3);
        for i in 0..10 {
            s.record(&i, None);
        }
        let mut n = 0;
        let mut cur = 10;
        while let Some(prev) = s.undo(&cur) {
            cur = prev;
            n += 1;
        }
        assert_eq!(n, 3);
        assert_eq!(cur, 7);
        s.record(&1, None);
        s.clear();
        assert!(!s.can_undo() && !s.can_redo());
    }
}
