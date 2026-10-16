//! The supervisor: owns the bundled `PostgreSQL` server and the engine
//! sidecar, from app data paths to restarts (docs/ARCHITECTURE.md, "Startup
//! sequence").
//!
//! This crate may spawn processes (CLAUDE.md layering rules). Secrets (the
//! launch token, database passwords) travel only as [`cl_pg::Secret`] values
//! and the child environment, never as command-line arguments.
#![forbid(unsafe_code)]

mod error;
mod instance_lock;
mod paths;

pub use error::{HostError, Result};
pub use instance_lock::{
    Acquired, InstanceGuard, Request, SendError, acquire, acquire_or_focus, send,
};
pub use paths::{AppPaths, Os};
