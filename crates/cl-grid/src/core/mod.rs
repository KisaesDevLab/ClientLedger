//! The egui-free heart of the grid: row model traits, the integer viewport,
//! the editing state machine and key mapping (P10 plan §1, layer 2).
//!
//! Nothing here may name egui (`tests/core_is_pure.rs`) or use floating
//! point (the xtask `nofloat` scan covers `src/core`, and the float lints are
//! denied below). Positions are whole rows plus 1/64-point units in integers,
//! so the grid stays exact at any row count.
#![deny(
    clippy::float_arithmetic,
    clippy::float_cmp,
    clippy::cast_precision_loss
)]

pub mod model;
pub mod rowkey;
pub mod selection;
pub mod spec;
pub mod viewport;
