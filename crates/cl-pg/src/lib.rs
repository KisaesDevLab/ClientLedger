//! Bundled `PostgreSQL` 16 manager.
//!
//! `cl-pg` finds the pinned binaries fetched by `cargo xtask pg-fetch`, creates
//! the database cluster, keeps its superuser password in the OS keychain,
//! starts and stops the server on a free loopback port, and takes whole-cluster
//! cold backups for disaster recovery. Company backups use the engine's tenant
//! export instead (Q-05).
//!
//! Security properties (see `docs/ARCHITECTURE.md`):
//! * the server listens on `127.0.0.1` only and never on a Unix socket;
//! * the only login method is `scram-sha-256`; the password is random, 256 bits;
//! * the password never appears in logs, error text, command lines, or files
//!   inside the data directory.
//!
//! This crate is allowed to spawn processes and to touch the data directory
//! (CLAUDE.md layering rules).
#![forbid(unsafe_code)]

mod binaries;
mod cluster;
mod error;
mod keychain;
mod paths;
pub mod ports;
mod proc;
mod secret;

pub use binaries::PgBinaries;
pub use cluster::{Cluster, is_initialized};
pub use error::PgError;
pub use keychain::{
    EncryptedFileStore, KeyringStore, MemoryStore, PasswordStore, StoreChoice, StoreNotice,
    default_store,
};
pub use paths::{CloudService, DataDirIssue, check_data_dir};
pub use secret::Secret;

/// Convenience alias used across the crate.
pub type Result<T> = std::result::Result<T, PgError>;
