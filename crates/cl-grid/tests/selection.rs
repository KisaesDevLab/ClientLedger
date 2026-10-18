//! T8 (selection part): selection is by `RowKey`, so random inserts and
//! deletes above and below never move it to other rows; Shift ranges are
//! anchored by key and stop at rows that are not loaded.

mod support;

use cl_grid::{RowKey, RowModel, Selection, SelectionMode};
use proptest::prelude::*;
use support::KeysModel;

#[derive(Debug, Clone)]
enum Change {
    Insert(usize),
    Delete(usize),
}

fn change() -> impl Strategy<Value = Change> {
    prop_oneof![
        (0usize..100).prop_map(Change::Insert),
        (0usize..100).prop_map(Change::Delete)
    ]
}

proptest! {
    #[test]
    fn selection_survives_inserts_and_deletes(
        picks in proptest::collection::btree_set(1u64..=30, 0..10),
        changes in proptest::collection::vec(change(), 0..30),
    ) {
        let mut m = KeysModel::with_keys(30);
        let mut sel = Selection::new();
        for k in &picks {
            sel.toggle(RowKey(*k), SelectionMode::Multi);
        }
        let mut next = 1_000;
        for c in changes {
            match c {
                Change::Insert(at) => {
                    let at = at % (m.rows.len() + 1);
                    m.rows.insert(at, Some(RowKey(next)));
                    next += 1;
                }
                Change::Delete(at) => {
                    if !m.rows.is_empty() {
                        let at = at % m.rows.len();
                        m.rows.remove(at);
                    }
                }
            }
            sel.retain_existing(&m);
        }
        // Exactly the picked keys that still exist stay selected.
        let want: Vec<RowKey> = picks
            .iter()
            .map(|k| RowKey(*k))
            .filter(|k| m.index_of(*k).is_some())
            .collect();
        prop_assert_eq!(sel.iter().collect::<Vec<_>>(), want);
    }

    #[test]
    fn shift_ranges_follow_the_model_order(a in 0usize..30, b in 0usize..30) {
        let m = KeysModel::with_keys(30);
        let mut sel = Selection::new();
        sel.toggle(m.row_key(a).unwrap(), SelectionMode::Multi);
        sel.extend_to(&m, m.row_key(b).unwrap(), SelectionMode::Multi);
        let (lo, hi) = (a.min(b), a.max(b));
        prop_assert_eq!(sel.len(), hi - lo + 1);
        for i in lo..=hi {
            prop_assert!(sel.contains(m.row_key(i).unwrap()));
        }
    }
}

proptest! {
    /// S3: extending from the same anchor replaces the previous range.
    #[test]
    fn extending_again_replaces_the_range(a in 0usize..30, b in 0usize..30, c in 0usize..30) {
        let m = KeysModel::with_keys(30);
        let mut sel = Selection::new();
        sel.anchor_if_none(m.row_key(a).unwrap());
        sel.extend_to(&m, m.row_key(b).unwrap(), SelectionMode::Multi);
        sel.extend_to(&m, m.row_key(c).unwrap(), SelectionMode::Multi);
        prop_assert_eq!(sel.len(), a.abs_diff(c) + 1);
    }
}

#[test]
fn shift_range_stops_at_rows_not_loaded() {
    let mut m = KeysModel::with_keys(10);
    m.rows[5] = None; // row 6 is still loading
    let mut sel = Selection::new();
    sel.toggle(RowKey(2), SelectionMode::Multi);
    sel.extend_to(&m, RowKey(9), SelectionMode::Multi);
    let got: Vec<u64> = sel.iter().map(|k| k.0).collect();
    assert_eq!(got, [2, 3, 4, 5]);
}

#[test]
fn modes() {
    let m = KeysModel::with_keys(5);
    let mut s = Selection::new();
    s.toggle(RowKey(1), SelectionMode::None);
    assert!(s.is_empty());
    s.toggle(RowKey(1), SelectionMode::Single);
    s.toggle(RowKey(2), SelectionMode::Single);
    assert_eq!(s.iter().collect::<Vec<_>>(), [RowKey(2)]);
    s.toggle(RowKey(2), SelectionMode::Single);
    assert!(s.is_empty());
    s.extend_to(&m, RowKey(4), SelectionMode::Single);
    assert_eq!(s.iter().collect::<Vec<_>>(), [RowKey(4)]);
    s.extend_to(&m, RowKey(5), SelectionMode::None);
    assert_eq!(s.len(), 1);
    // Without an anchor, Shift selects the target alone.
    let mut t = Selection::new();
    t.extend_to(&m, RowKey(3), SelectionMode::Multi);
    assert_eq!(t.iter().collect::<Vec<_>>(), [RowKey(3)]);
    assert_eq!(t.anchor(), Some(RowKey(3)));
    // A vanished anchor is dropped.
    let mut gone = KeysModel::with_keys(5);
    gone.rows.remove(2);
    assert!(t.retain_existing(&gone));
    assert_eq!(t.anchor(), None);
    assert!(t.is_empty());
    assert!(!t.clear());
}
