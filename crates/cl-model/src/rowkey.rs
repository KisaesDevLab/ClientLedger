//! Row keys (P10 plan Rev 2 §2.2, B5, Q-P10s).
//!
//! A [`RowKey`] names one row for as long as the form is open: across
//! inserts, deletes, sorts, reloads and undo. Screens allocate keys from a
//! [`RowKeyGen`] and keep them **inside their line values**, so undo and redo
//! restore a line together with its key. Keys are never reused and are never
//! engine line ids (the engine recreates those on every update).

/// A stable row identity, assigned by the screen. The default, `RowKey(0)`,
/// is never handed out by [`RowKeyGen`], so it reads as "unassigned".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct RowKey(pub u64);

/// A monotonic key source: [`RowKeyGen::next`] never returns a key twice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowKeyGen(u64);

impl Default for RowKeyGen {
    fn default() -> Self {
        RowKeyGen::new()
    }
}

impl RowKeyGen {
    /// Starts at key 1 (0 is never handed out, so a zeroed value is visibly
    /// unassigned).
    #[must_use]
    pub fn new() -> RowKeyGen {
        RowKeyGen(1)
    }

    /// A generator that only hands out keys above every key in `existing`
    /// (after loading lines that already carry keys).
    pub fn after<I: IntoIterator<Item = RowKey>>(existing: I) -> RowKeyGen {
        let max = existing.into_iter().map(|k| k.0).max().unwrap_or(0);
        RowKeyGen(max.saturating_add(1).max(1))
    }

    /// The next unused key.
    pub fn next_key(&mut self) -> RowKey {
        let k = RowKey(self.0);
        self.0 = self.0.saturating_add(1);
        k
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_never_repeat() {
        let mut g = RowKeyGen::new();
        let a = g.next_key();
        let b = g.next_key();
        assert_eq!((a, b), (RowKey(1), RowKey(2)));
        let mut h = RowKeyGen::after([RowKey(7), RowKey(3)]);
        assert_eq!(h.next_key(), RowKey(8));
        assert_eq!(RowKeyGen::after([]).next_key(), RowKey(1));
        assert_eq!(RowKeyGen::default(), RowKeyGen::new());
    }
}
