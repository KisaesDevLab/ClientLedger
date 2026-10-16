//! The supervisor: owns the bundled `PostgreSQL` server and the engine
//! sidecar, from app data paths to restarts (docs/ARCHITECTURE.md, "Startup
//! sequence").
//!
//! This crate may spawn processes (CLAUDE.md layering rules). Secrets (the
//! launch token, database passwords) travel only as [`cl_pg::Secret`] values
//! and the child environment, never as command-line arguments.
#![forbid(unsafe_code)]

mod database;
mod desktop;
mod engine;
mod error;
mod health;
mod http;
mod instance_lock;
mod logs;
mod paths;
mod procs;
mod secrets;
mod supervisor;

pub use cl_pg::Secret;
pub use database::{
    APP_DATABASE, APP_ROLE, DatabaseFactory, DatabaseParts, DatabaseService, DbEndpoint, PgService,
};
pub use desktop::{AdminAccount, CompanySeed, DesktopError, DesktopInfo, Initialized};
pub use engine::OfflineSetting;
pub use error::{HostError, Result};
pub use health::{HealthError, HealthReport, check_health, verify_listener};
pub use instance_lock::{
    Acquired, InstanceGuard, Request, SendError, acquire, acquire_or_focus, send,
};
pub use logs::{HostLog, Level, LogPolicy, PruneReport, prune_logs, tail_lines};
pub use paths::{AppPaths, Os, engine_exe_name, find_engine_exe};
pub use secrets::EngineSecrets;
pub use supervisor::{
    Component, Endpoint, HostConfig, HostFault, HostSnapshot, HostState, HostStatus, RestartCounts,
    RestartPolicy, Supervisor, in_memory_stores,
};
