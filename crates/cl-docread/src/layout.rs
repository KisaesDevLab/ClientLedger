//! Page layout (WP-A, step 8; plan D3 `layout.rs`): words grouped into
//! text lines and blocks, and right-aligned numeric columns.
//!
//! Internal to the rules (WP-A may refine these types freely).

use crate::model::{Bbox, PageWords};

/// One text line: words on the same baseline, left to right.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextLine {
    /// Index into the input pages slice (not the page number).
    pub page_index: usize,
    /// Indices into that page's `words`, left to right.
    pub words: Vec<usize>,
    pub bbox: Bbox,
}

/// A block: neighbouring lines that belong together (an address, a totals
/// box).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub page_index: usize,
    /// Indices into [`Layout::lines`].
    pub lines: Vec<usize>,
    pub bbox: Bbox,
}

/// A right-aligned column of numbers (amounts, quantities).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NumericColumn {
    pub page_index: usize,
    /// The shared right edge.
    pub right: u16,
    /// Indices into [`Layout::lines`] with a number on that edge.
    pub lines: Vec<usize>,
}

/// The layout of a document.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Layout {
    pub lines: Vec<TextLine>,
    pub blocks: Vec<Block>,
    pub numeric_columns: Vec<NumericColumn>,
}

/// Group the pages' words. Stub (step 2): an empty layout.
#[must_use]
pub fn analyze(pages: &[PageWords]) -> Layout {
    let _ = pages;
    Layout::default()
}
