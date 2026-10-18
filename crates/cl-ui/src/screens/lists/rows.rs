//! A read-only grid model of text rows, shared by the smaller lists (items,
//! tags, terms, payment methods, the CSV preview). Each row has an id, one
//! text per column and a spoken label; row keys stay with their ids across
//! reloads so the cursor follows a record. Amounts arrive already formatted
//! from exact values (prices from `Money`); nothing here adds them up.

use std::borrow::Cow;
use std::collections::HashMap;
use std::hash::Hash;

use cl_grid::{
    Align, CellKind, CellValue, Column, ColumnId, GridSpec, Line, RowHeight, RowKey, RowKeyGen,
    RowModel, RowState, SelectionMode, TextKey, WidthSpec,
};

/// A display column.
#[must_use]
pub fn column(id: ColumnId, header: &'static str, w: u16, flex: u8) -> Column {
    Column {
        id,
        header: TextKey(header),
        placeholder: None,
        help: None,
        kind: CellKind::Display,
        line: Line::First,
        align: Align::Left,
        width: WidthSpec {
            min: w.min(70),
            preferred: w,
            flex,
        },
        editable: false,
        mono: false,
    }
}

/// A right-aligned, monospaced display column (prices, counts).
#[must_use]
pub fn figure_column(id: ColumnId, header: &'static str, w: u16) -> Column {
    Column {
        align: Align::Right,
        mono: true,
        ..column(id, header, w, 0)
    }
}

/// A read-only list: one line per row, one row selected.
#[must_use]
pub fn list_spec(cols: Vec<Column>) -> GridSpec {
    let mut s = GridSpec::browse(cols);
    s.height = RowHeight::Uniform { two_lines: false };
    s.selection = SelectionMode::Single;
    s
}

/// One row: its id, a text per column (in the spec's order) and the label
/// a screen reader says for the row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextRow<Id> {
    /// The record.
    pub id: Id,
    /// Text per column.
    pub cells: Vec<String>,
    /// Spoken row label.
    pub label: String,
}

/// The rows a grid shows.
#[derive(Debug)]
pub struct TextRows<Id> {
    cols: Vec<ColumnId>,
    rows: Vec<TextRow<Id>>,
    keys: HashMap<Id, RowKey>,
    keygen: RowKeyGen,
    row_keys: Vec<RowKey>,
    by_key: HashMap<RowKey, usize>,
    revision: u64,
}

impl<Id: Copy + Eq + Hash> TextRows<Id> {
    /// An empty model over `cols` (the spec's column ids, in order).
    #[must_use]
    pub fn new(cols: Vec<ColumnId>) -> TextRows<Id> {
        TextRows {
            cols,
            rows: Vec::new(),
            keys: HashMap::new(),
            keygen: RowKeyGen::default(),
            row_keys: Vec::new(),
            by_key: HashMap::new(),
            revision: 0,
        }
    }

    /// Replaces the rows (keys stay with their ids).
    pub fn set(&mut self, rows: Vec<TextRow<Id>>) {
        self.row_keys.clear();
        self.by_key.clear();
        for (i, r) in rows.iter().enumerate() {
            let key = *self
                .keys
                .entry(r.id)
                .or_insert_with(|| self.keygen.next_key());
            self.row_keys.push(key);
            self.by_key.insert(key, i);
        }
        self.rows = rows;
        self.revision += 1;
    }

    /// Changes the columns (same rows; the cell texts must match).
    pub fn set_columns(&mut self, cols: Vec<ColumnId>) {
        self.cols = cols;
        self.revision += 1;
    }

    /// The id on a row key.
    #[must_use]
    pub fn id_at(&self, key: RowKey) -> Option<Id> {
        self.by_key.get(&key).map(|i| self.rows[*i].id)
    }

    /// The row key of `id`, when shown.
    #[must_use]
    pub fn key_of(&self, id: Id) -> Option<RowKey> {
        let key = *self.keys.get(&id)?;
        self.by_key.contains_key(&key).then_some(key)
    }

    /// The first row whose first column starts with `text` (any case).
    #[must_use]
    pub fn find_prefix(&self, text: &str) -> Option<RowKey> {
        let q = text.to_lowercase();
        if q.is_empty() {
            return None;
        }
        self.rows
            .iter()
            .position(|r| {
                r.cells
                    .first()
                    .is_some_and(|c| c.trim_start().to_lowercase().starts_with(&q))
            })
            .map(|i| self.row_keys[i])
    }

    /// Rows shown.
    #[must_use]
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// No rows.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// The rows.
    #[must_use]
    pub fn rows(&self) -> &[TextRow<Id>] {
        &self.rows
    }
}

impl<Id: Copy + Eq + Hash> RowModel for TextRows<Id> {
    fn revision(&self) -> u64 {
        self.revision
    }
    fn row_count(&self) -> usize {
        self.rows.len()
    }
    fn row_key(&self, index: usize) -> Option<RowKey> {
        self.row_keys.get(index).copied()
    }
    fn index_of(&self, key: RowKey) -> Option<usize> {
        self.by_key.get(&key).copied()
    }
    fn row_state(&self, _index: usize) -> RowState {
        RowState::empty()
    }
    fn cell(&self, index: usize, col: ColumnId) -> CellValue<'_> {
        let Some(row) = self.rows.get(index) else {
            return CellValue::Empty;
        };
        let Some(c) = self.cols.iter().position(|c| *c == col) else {
            return CellValue::Empty;
        };
        match row.cells.get(c) {
            Some(s) if !s.is_empty() => CellValue::Text(Cow::Borrowed(s)),
            _ => CellValue::Empty,
        }
    }
    fn row_label(&self, index: usize) -> String {
        self.rows
            .get(index)
            .map(|r| r.label.clone())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: u32, name: &str) -> TextRow<u32> {
        TextRow {
            id,
            cells: vec![name.to_string(), String::new()],
            label: format!("{name} label"),
        }
    }

    #[test]
    fn keys_follow_ids_and_cells_resolve_by_column() {
        let a = ColumnId(1);
        let b = ColumnId(2);
        let mut m = TextRows::new(vec![a, b]);
        assert!(m.is_empty());
        m.set(vec![row(7, "Mowing"), row(9, "Aeration")]);
        let k7 = m.key_of(7).expect("key");
        m.set(vec![row(9, "Aeration"), row(7, "Mowing")]);
        assert_eq!(m.key_of(7), Some(k7));
        assert_eq!(m.id_at(k7), Some(7));
        assert_eq!(m.index_of(k7), Some(1));
        assert_eq!(m.len(), 2);
        assert_eq!(m.find_prefix("mo"), Some(k7));
        assert_eq!(m.find_prefix(""), None);
        assert!(matches!(m.cell(1, a), CellValue::Text(t) if t == "Mowing"));
        assert!(matches!(m.cell(1, b), CellValue::Empty));
        assert!(matches!(m.cell(5, a), CellValue::Empty));
        assert!(matches!(m.cell(0, ColumnId(3)), CellValue::Empty));
        assert_eq!(m.row_label(1), "Mowing label");
        assert_eq!(m.row_label(9), "");
        let rev = m.revision();
        m.set(vec![row(9, "Aeration")]);
        assert_eq!(m.key_of(7), None);
        assert!(m.revision() > rev);
        m.set_columns(vec![a]);
        assert_eq!(m.rows().len(), 1);
        let spec = list_spec(vec![column(a, "x", 100, 1), figure_column(b, "y", 80)]);
        assert_eq!(spec.columns.len(), 2);
    }
}
