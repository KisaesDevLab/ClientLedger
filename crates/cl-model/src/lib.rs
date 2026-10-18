//! UI-side state without egui: the command registry and keymap, the form
//! state machine, undo for unsaved edits, date entry with the date keys and
//! date warnings, exact amount entry with the calculator, type-ahead ranking
//! for pickers, the one-tag-per-group chip rule, and the master-list rules
//! (chart of accounts hierarchy, filters and form checks).
//!
//! Nothing here draws or does I/O, so all of it is unit tested.
//!
//! Entry rules for amounts and dates live here too (P10, Q-P10b), so this
//! crate is float-free like cl-core: the float lints are denied and the xtask
//! `nofloat` scan covers `src/`.
#![forbid(unsafe_code)]
#![deny(
    clippy::float_arithmetic,
    clippy::float_cmp,
    clippy::cast_precision_loss
)]

pub mod amount_entry;
pub mod commands;
pub mod date_entry;
pub mod date_rules;
pub mod forms;
pub mod keymap;
pub mod lists;
pub mod lookup;
pub mod tags;
pub mod text;
pub mod undo;

pub use commands::{COMMANDS, Command, CommandId, Menu};
pub use keymap::{Key, Keymap, KeymapError, Mods, Scope, Shortcut};
pub use text::TextKey;
