//! UI-side state without egui: the command registry and keymap, the form
//! state machine, undo for unsaved edits, and date-key and date-warning rules.
//!
//! Nothing here draws or does I/O, so all of it is unit tested.
#![forbid(unsafe_code)]

pub mod commands;
pub mod date_rules;
pub mod forms;
pub mod keymap;
pub mod undo;

pub use commands::{COMMANDS, Command, CommandId, Menu};
pub use keymap::{Key, Keymap, KeymapError, Mods, Scope, Shortcut};
