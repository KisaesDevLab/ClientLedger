//! The supervisor: owns the bundled `PostgreSQL` server and the engine
//! sidecar, from app data paths to restarts (docs/ARCHITECTURE.md, "Startup
//! sequence").
//!
//! This crate may spawn processes (CLAUDE.md layering rules). Secrets (the
//! launch token, database passwords) travel only as [`cl_pg::Secret`] values
//! and the child environment, never as command-line arguments.
//!
//! **Two `unsafe` boundaries**, both Windows API calls:
//! `procs/win_job_memory.rs` makes the single `SetInformationJobObject`
//! call for the Windows hard memory cap (A03, Q-A03s), and `win_print.rs`
//! draws printed layouts through GDI (Q-P19ae). The crate denies unsafe
//! code (the workspace's `forbid` can't be relaxed in one module); those two
//! modules allow it, and `cargo xtask unsafe-check` fails on `unsafe`
//! anywhere else or on a block without a `// SAFETY:` comment.
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
mod os_print;
mod paths;
mod procs;
mod secrets;
mod supervisor;
#[cfg(windows)]
mod win_print;

pub use cl_pg::Secret;
pub use database::{
    APP_DATABASE, APP_ROLE, DatabaseFactory, DatabaseParts, DatabaseService, DbEndpoint, PgService,
};
pub use desktop::{AdminAccount, CompanySeed, DesktopError, DesktopInfo, Initialized};
pub use docread_worker::{
    CouldNotRead, DocJob, DocreadConfig, DocreadHost, DocreadLimits, DocreadStats, DocreadSwept,
    JobOutcome, OcrTools, RegionJob, RegionOutcome, sweep_docread_leftovers,
};
pub use engine::OfflineSetting;
pub use error::{HostError, Result, is_master_secret_message};
pub use health::{HealthError, HealthReport, check_health, verify_listener};
pub use instance_lock::{
    Acquired, InstanceGuard, Request, SendError, acquire, acquire_or_focus, send,
};
pub use logs::{HostLog, Level, LogPolicy, PruneReport, prune_logs, tail_lines};
#[cfg(windows)]
pub use os_print::WindowsPrinter;
pub use os_print::{
    CommandOutput, CommandRunner, CommandSpec, DeviceMap, DirectPrintError, DirectPrinter,
    MAX_COPIES, MAX_PRINTER_NAME, OsPrint, PrintOutcome, SystemRunner, sweep_print_dir,
    valid_printer_name, write_temp_pdf,
};
pub use paths::{
    AppPaths, Os, docread_exe_name, engine_exe_name, find_docread_exe, find_engine_exe,
    find_ocr_dir, tesseract_exe_name,
};
pub use secrets::EngineSecrets;
pub use supervisor::{
    Component, Endpoint, HostConfig, HostFault, HostSnapshot, HostState, HostStatus, RestartCounts,
    RestartPolicy, Supervisor, in_memory_stores,
};
