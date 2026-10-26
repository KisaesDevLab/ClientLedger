//! The supervisor: owns the bundled `PostgreSQL` server and the engine
//! sidecar, from app data paths to restarts (docs/ARCHITECTURE.md, "Startup
//! sequence").
//!
//! This crate may spawn processes (CLAUDE.md layering rules). Secrets (the
//! launch token, database passwords) travel only as [`cl_pg::Secret`] values
//! and the child environment, never as command-line arguments.
//!
//! **One `unsafe` boundary** (A03, Q-A03s): `procs/win_job_memory.rs`
//! makes the single `SetInformationJobObject` call for the Windows hard
//! memory cap. The crate denies unsafe code (the workspace's `forbid` can't
//! be relaxed in one module); `cargo xtask unsafe-check` fails on `unsafe`
//! anywhere else.
#![deny(unsafe_code)]

// Test hooks never reach an optimised build: feature unification
// (`--workspace --all-targets`) can switch `test-support` on for the apps,
// so a release build with it fails here (xtask `release-features` checks
// the normal app build too).
#[cfg(all(feature = "test-support", not(debug_assertions)))]
compile_error!(
    "the `test-support` feature is for tests only and must not be enabled in a release build"
);

mod database;
mod desktop;
mod docread_worker;
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
pub use docread_worker::{
    CouldNotRead, DocJob, DocreadConfig, DocreadHost, DocreadLimits, DocreadStats, DocreadSwept,
    JobOutcome, OcrTools, sweep_docread_leftovers,
};
pub use engine::OfflineSetting;
pub use error::{HostError, Result};
pub use health::{HealthError, HealthReport, check_health, verify_listener};
pub use instance_lock::{
    Acquired, InstanceGuard, Request, SendError, acquire, acquire_or_focus, send,
};
pub use logs::{HostLog, Level, LogPolicy, PruneReport, prune_logs, tail_lines};
pub use paths::{
    AppPaths, Os, docread_exe_name, engine_exe_name, find_docread_exe, find_engine_exe,
    find_ocr_dir, tesseract_exe_name,
};
pub use secrets::EngineSecrets;
pub use supervisor::{
    Component, Endpoint, HostConfig, HostFault, HostSnapshot, HostState, HostStatus, RestartCounts,
    RestartPolicy, Supervisor, in_memory_stores,
};
