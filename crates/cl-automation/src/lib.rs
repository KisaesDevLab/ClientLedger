//! The opt-in control channel (Q-16): a loopback server that lets scripts and
//! tests inspect and drive the running app.
//!
//! This crate has no egui. It owns the protocol, the safe control file, the
//! tool table, the scenario runner and screenshot encoding; `cl-ui` plugs the
//! real window in on the other side of [`ControlServer::try_next`].
//!
//! The server only starts when the app is launched with `--control <path>`.
#![forbid(unsafe_code)]
// Key chords and file facts are plain bags of flags.
#![allow(clippy::struct_excessive_bools)]

pub mod actions;
pub mod client;
pub mod control_file;
pub mod inspect;
pub mod screenshot;
pub mod server;
pub mod tools;

pub use actions::{ErrorCode, Request, Response, Target};
pub use control_file::{ControlFileError, ControlInfo};
pub use server::{ControlServer, Pending};
