//! Shared test models for cl-grid's integration tests.
#![allow(dead_code)] // each test binary uses a different part

use cl_grid::{CellValue, ColumnId, RowKey, RowModel, RowState};

/// Rows that are only keys; `None` is a row not loaded yet.
#[derive(Debug, Clone, Default)]
pub struct KeysModel {
    pub rows: Vec<Option<RowKey>>,
    pub revision: u64,
}

impl KeysModel {
    pub fn with_keys(n: u64) -> KeysModel {
        KeysModel {
            rows: (1..=n).map(|k| Some(RowKey(k))).collect(),
            revision: 0,
        }
    }
}

impl RowModel for KeysModel {
    fn revision(&self) -> u64 {
        self.revision
    }
    fn row_count(&self) -> usize {
        self.rows.len()
    }
    fn row_key(&self, index: usize) -> Option<RowKey> {
        self.rows.get(index).copied().flatten()
    }
    fn index_of(&self, key: RowKey) -> Option<usize> {
        self.rows.iter().position(|k| *k == Some(key))
    }
    fn row_state(&self, _index: usize) -> RowState {
        RowState::empty()
    }
    fn cell(&self, index: usize, _col: ColumnId) -> CellValue<'_> {
        if self.row_key(index).is_some() {
            CellValue::Empty
        } else {
            CellValue::Loading
        }
    }
    fn row_label(&self, _index: usize) -> String {
        String::new()
    }
}
