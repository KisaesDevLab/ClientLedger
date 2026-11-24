//! MCP bookkeeper (A11, ADD-06): an allow-list bridge between an MCP client
//! and the local engine, running in the desktop host process.
//!
//! Tools have fixed endpoints and closed schemas; their request bodies come
//! from [`forms`], the same mapping the desktop forms use, so an MCP save and
//! a human save send identical JSON. The engine enforces every rule again
//! (plan §1), so a bridge bug cannot widen access.
//!
//! No egui here (layering): `cl-ui` calls [`forms`]; the host owns transports.
//! Money is exact: `cl_core::Money` and `rust_decimal`, never floats.
#![forbid(unsafe_code)]
#![deny(
    clippy::float_arithmetic,
    clippy::float_cmp,
    clippy::cast_precision_loss
)]

pub mod applier;
pub mod bridge;
pub mod claude_desktop;
pub mod forms;
pub mod policy;
pub mod protocol;
pub mod relay;
pub mod tools;
pub mod undo;
