//! The ledger grid widget (P10): virtualized one- and two-line rows, inline
//! editors, keyboard traversal and an AccessKit grid tree.
//!
//! Layers (P10 plan §1): exact entry rules live in `cl-model`; [`core`] holds
//! the egui-free model, viewport and state machine; the egui view and editors
//! are thin adapters over it. The grid makes no I/O, engine or clock calls,
//! and never computes balances or totals.
#![forbid(unsafe_code)]

pub mod core;
pub mod editors;
mod grid;
pub mod sequencer;
pub mod view;

pub use grid::{
    FULL_FILTER, Grid, GridEnv, GridHeight, GridOutput, GridSnapshot, check as draw_check,
};
pub use view::style::GridStyle;

pub use crate::core::model::{
    Align, CellInput, CellKind, CellPos, CellProblem, CellValue, Column, ColumnId, EditableModel,
    GridProblem, Line, ListId, LookupResult, Lookups, MarkKind, ModelRef, NoLookups, OptionSpec,
    RecordOutcome, RowHeight, RowModel, RowNote, RowState, Source, Suggestion, WidthSpec,
};
pub use crate::core::rowkey::{RowKey, RowKeyGen};
pub use crate::core::selection::{Selection, SelectionMode};
pub use crate::core::spec::{GridCommand, GridEvent, GridSpec, Handled, SpecError};
pub use cl_model::TextKey;
pub use cl_model::tags::TagChip;
