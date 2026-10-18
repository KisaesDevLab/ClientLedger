//! Viewport properties (P10 plan §6 and Rev 2 T8): exact positions at any
//! row count, whole-row paging, an exact Ctrl+End, monotonic thumbs for up
//! to 10^9 rows, measured tables equal to naive sums, and the top row kept
//! by key across inserts above.

mod support;

use cl_grid::RowKey;
use cl_grid::core::viewport::{Heights, RowAlign, UNITS_PER_PT, Viewport};
use proptest::prelude::*;
use support::KeysModel;

const ROW: u64 = 44 * UNITS_PER_PT;

/// A model with `n` rows without storing them (keys are the index + 1).
struct Virtual(usize);

impl cl_grid::RowModel for Virtual {
    fn revision(&self) -> u64 {
        0
    }
    fn row_count(&self) -> usize {
        self.0
    }
    fn row_key(&self, index: usize) -> Option<RowKey> {
        (index < self.0).then(|| RowKey(index as u64 + 1))
    }
    fn index_of(&self, key: RowKey) -> Option<usize> {
        usize::try_from(key.0)
            .ok()
            .and_then(|k| k.checked_sub(1))
            .filter(|i| *i < self.0)
    }
    fn row_state(&self, _index: usize) -> cl_grid::RowState {
        cl_grid::RowState::empty()
    }
    fn cell(&self, _index: usize, _col: cl_grid::ColumnId) -> cl_grid::CellValue<'_> {
        cl_grid::CellValue::Empty
    }
    fn row_label(&self, _index: usize) -> String {
        String::new()
    }
}

proptest! {
    #[test]
    fn ensure_visible_shows_the_whole_row(
        n in 1usize..10_000_000,
        rows_tall in 1u64..40,
        start in 0u64..u64::MAX,
        target in 0usize..10_000_000,
    ) {
        let m = Virtual(n);
        let h = Heights::uniform(ROW, n);
        let mut v = Viewport::new(rows_tall * ROW + 17);
        v.set_pos(start, &h, &m);
        let i = target % n;
        v.ensure_visible(i, &h, &m);
        let pos = v.pos(&h);
        prop_assert!(h.top_of(i) >= pos);
        prop_assert!(h.top_of(i) + ROW <= pos + v.height());
        prop_assert!(v.visible_range(&h).contains(&i));
    }

    #[test]
    fn paging_moves_by_whole_rows(n in 50usize..1_000_000, start_row in 0usize..1_000_000, rows_tall in 2u64..30) {
        let m = Virtual(n);
        let h = Heights::uniform(ROW, n);
        let mut v = Viewport::new(rows_tall * ROW + 5);
        v.set_pos(h.top_of(start_row % n), &h, &m);
        let before = v.pos(&h);
        v.page(true, &h, &m);
        let after = v.pos(&h);
        if after < v.max_pos(&h) {
            prop_assert_eq!(after - before, rows_tall * ROW);
            prop_assert_eq!(v.offset(), 0);
        }
        v.page(false, &h, &m);
        prop_assert!(v.pos(&h) <= after);
    }

    #[test]
    fn ctrl_end_is_exact(n in 1usize..1_000_000_000, tall in 1u64..2_000) {
        let m = Virtual(n);
        let h = Heights::uniform(ROW, n);
        let mut v = Viewport::new(tall * UNITS_PER_PT);
        v.to_end(&h, &m);
        if h.total() > v.height() {
            prop_assert_eq!(v.pos(&h) + v.height(), h.total());
            let last = n - 1;
            prop_assert_eq!(v.local_top(last, &h) + i64::try_from(ROW).unwrap(), i64::try_from(v.height()).unwrap());
        } else {
            prop_assert_eq!(v.pos(&h), 0);
        }
        prop_assert_eq!(v.visible_range(&h).end, n);
    }

    #[test]
    fn thumb_is_monotonic_and_in_range(n in 1usize..1_000_000_000, a in 0u64..u64::MAX, b in 0u64..u64::MAX, track in 100u64..100_000) {
        let m = Virtual(n);
        let h = Heights::uniform(ROW, n);
        let mut v = Viewport::new(20 * ROW);
        v.set_pos(a.min(b), &h, &m);
        let (s1, l1) = v.thumb(track, 24 * UNITS_PER_PT, &h);
        v.set_pos(a.max(b), &h, &m);
        let (s2, l2) = v.thumb(track, 24 * UNITS_PER_PT, &h);
        prop_assert!(s1 <= s2);
        prop_assert_eq!(l1, l2);
        prop_assert!(s2 + l2 <= track);
        // The inverse lands within one thumb step of the position.
        let p = v.pos_for_thumb(s2, track, l2, &h);
        prop_assert!(p <= v.max_pos(&h));
    }

    #[test]
    fn measured_tables_match_naive_sums(heights in proptest::collection::vec(1u64..5_000, 0..300), probe in 0u64..2_000_000) {
        let h = Heights::measured(&heights);
        let mut sum = 0;
        for (i, x) in heights.iter().enumerate() {
            prop_assert_eq!(h.top_of(i), sum);
            prop_assert_eq!(h.height_of(i), *x);
            sum += x;
        }
        prop_assert_eq!(h.total(), sum);
        if !heights.is_empty() {
            let r = h.row_at(probe);
            let p = probe.min(sum - 1);
            prop_assert!(h.top_of(r) <= p && p < h.top_of(r) + h.height_of(r));
        }
    }

    #[test]
    fn measured_ensure_visible(heights in proptest::collection::vec(64u64..5_000, 1..300), target in 0usize..300, tall in 5_000u64..20_000) {
        let n = heights.len();
        let m = Virtual(n);
        let h = Heights::measured(&heights);
        let mut v = Viewport::new(tall);
        let i = target % n;
        v.ensure_visible(i, &h, &m);
        let pos = v.pos(&h);
        prop_assert!(h.top_of(i) >= pos);
        prop_assert!(h.top_of(i) + h.height_of(i) <= pos + v.height());
    }

    #[test]
    fn top_row_is_kept_across_inserts_and_deletes_above(top in 5usize..40, inserts in 1usize..10, deletes in 0usize..3) {
        let mut m = KeysModel::with_keys(60);
        let mut h = Heights::uniform(ROW, m.rows.len());
        let mut v = Viewport::new(10 * ROW);
        v.scroll_to(top, RowAlign::Top, &h, &m);
        let key = v.top_key().unwrap();
        for k in 0..inserts {
            m.rows.insert(0, Some(RowKey(1_000 + k as u64)));
        }
        for _ in 0..deletes {
            m.rows.remove(1);
        }
        h = Heights::uniform(ROW, m.rows.len());
        v.resync(&h, &m);
        prop_assert_eq!(v.top_key(), Some(key));
        prop_assert_eq!(v.top(), top + inserts - deletes);
    }
}

#[test]
fn empty_and_tiny_content() {
    let m = Virtual(0);
    let h = Heights::uniform(ROW, 0);
    let mut v = Viewport::new(10 * ROW);
    v.to_end(&h, &m);
    v.ensure_visible(3, &h, &m);
    assert_eq!((v.pos(&h), v.visible_range(&h)), (0, 0..0));
    assert_eq!(v.thumb(500, 24, &h), (0, 500));
    let m3 = Virtual(3);
    let h3 = Heights::uniform(ROW, 3);
    v.page(true, &h3, &m3);
    assert_eq!(v.pos(&h3), 0, "three rows fit: nothing to scroll");
    assert_eq!(v.visible_range(&h3), 0..3);
}

#[test]
fn alignment_and_gone_top_row() {
    let mut m = KeysModel::with_keys(100);
    let h = Heights::uniform(ROW, 100);
    let mut v = Viewport::new(10 * ROW);
    v.scroll_to(50, RowAlign::Bottom, &h, &m);
    assert_eq!(v.visible_range(&h).end, 51);
    v.scroll_to(50, RowAlign::Top, &h, &m);
    assert_eq!(v.top(), 50);
    v.scroll_by(-(i64::try_from(ROW).unwrap()) / 2, &h, &m);
    assert_eq!((v.top(), v.offset()), (49, ROW / 2));
    // The top row is deleted: the index is kept.
    m.rows.remove(49);
    let h2 = Heights::uniform(ROW, 99);
    v.resync(&h2, &m);
    assert_eq!(v.top(), 49);
    // Shrinking below the viewport resets to the start.
    m.rows.truncate(4);
    let h4 = Heights::uniform(ROW, 4);
    v.resync(&h4, &m);
    assert_eq!(v.pos(&h4), 0);
    v.set_height(2 * ROW, &h4, &m);
    assert_eq!(v.height(), 2 * ROW);
    assert_eq!(v.page_rows(&h4), 2);
    let hm = Heights::measured(&[ROW, 3 * ROW, ROW]);
    let mut vm = Viewport::new(2 * ROW);
    assert_eq!(vm.page_rows(&hm), 1);
    vm.ensure_visible(1, &hm, &Virtual(3));
    assert_eq!(
        vm.pos(&hm),
        ROW,
        "a row taller than the viewport is shown from its top"
    );
}
