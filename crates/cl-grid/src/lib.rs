//! The ledger grid widget (P10): virtualized one- and two-line rows, inline
//! editors, keyboard traversal and an AccessKit grid tree.
//!
//! Layers (P10 plan §1): exact entry rules live in `cl-model`; [`core`] holds
//! the egui-free model, viewport and state machine; the egui view and editors
//! are thin adapters over it. The grid makes no I/O, engine or clock calls,
//! and never computes balances or totals.
#![forbid(unsafe_code)]

pub mod core;
