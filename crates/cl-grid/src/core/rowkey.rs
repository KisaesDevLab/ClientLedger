//! Row keys (P10 plan Rev 2 §2.2, B5, Q-P10s). The types live in
//! `cl_model::rowkey` so line models without egui (the P13 shared form
//! lines) can carry them; the grid re-exports them unchanged.

pub use cl_model::rowkey::{RowKey, RowKeyGen};
