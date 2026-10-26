//! The Windows job limits for the reading worker and tesseract (A03,
//! Q-A03s; plan review M2; security review M2, M3).
//!
//! **This is the only `unsafe` code in `ClientLedger`'s own crates.**
//! `win32job` (2.0.3) sets kill-on-close and working-set limits but not
//! `JOB_OBJECT_LIMIT_PROCESS_MEMORY`, and a working-set cap is the wrong
//! tool (it lets commit grow into the page file). Rather than open a pull
//! request on a third-party crate, this module makes the one
//! `SetInformationJobObject` call itself through `windows-sys`.
//!
//! The one call sets every limit at once (it replaces the job's limits):
//!
//! * kill-on-close, as `TreeKiller` set it: the process dies with the host;
//! * a per-process commit cap: the memory watchdog's value × 1.25
//!   (`DocreadLimits`). The watchdog stays the operative limit; this
//!   catches a spike faster than its 200 ms poll, so the allocation fails
//!   and the process ends at once instead of thrashing the owner's machine;
//! * one active process: neither program starts anything, so a
//!   compromised one can't launch a second stage (`cmd`, `powershell`);
//!   breakaway is not allowed either;
//! * die on an unhandled exception: every process in the job runs with
//!   `SEM_NOGPFAULTERRORBOX`, so a crash inside `PDFium` or Leptonica does
//!   not invoke Windows Error Reporting (no report or memory dump holding
//!   document content queued, kept or uploaded; non-negotiable 3).
//!
//! The boundary is held by: `unsafe_code = "deny"` for cl-host (the
//! workspace forbids it everywhere else), `#![allow(unsafe_code)]` only
//! here, and `cargo xtask unsafe-check` (part of `cargo xtask ci`), which
//! fails on `unsafe` in any other file or on a second block here.
#![allow(unsafe_code)]

use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::System::JobObjects::{
    JOB_OBJECT_LIMIT_ACTIVE_PROCESS, JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_LIMIT_PROCESS_MEMORY,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject,
};

/// The job's limits after the call (see the module comment): kill-on-close,
/// a per-process commit cap of `bytes`, one active process, and no crash
/// reporting.
#[must_use]
pub(crate) fn limits_for(bytes: usize) -> JOBOBJECT_EXTENDED_LIMIT_INFORMATION {
    let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    // Setting the extended information replaces every limit, so
    // kill-on-close is set again here.
    info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
        | JOB_OBJECT_LIMIT_PROCESS_MEMORY
        | JOB_OBJECT_LIMIT_ACTIVE_PROCESS
        | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION;
    info.BasicLimitInformation.ActiveProcessLimit = 1;
    info.ProcessMemoryLimit = bytes;
    info
}

/// Apply [`limits_for`] to `job`. False when the limits could not be set:
/// the caller must then not run the process (fail closed, review M5).
pub(crate) fn confine_job(job: &win32job::Job, bytes: u64) -> bool {
    let Ok(bytes) = usize::try_from(bytes) else {
        return false;
    };
    let info = limits_for(bytes);
    let Ok(size) = u32::try_from(size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>()) else {
        return false;
    };
    // `win32job` hands out the raw handle as an integer.
    let handle = job.handle() as HANDLE;
    // SAFETY: `handle` is the open job handle owned by `job`, which this
    // borrow keeps alive for the whole call. `info` is a fully initialised
    // `#[repr(C)]` JOBOBJECT_EXTENDED_LIMIT_INFORMATION on this stack frame,
    // `size` is exactly its size, and the function only reads it.
    let ok = unsafe {
        SetInformationJobObject(
            handle,
            JobObjectExtendedLimitInformation,
            (&raw const info).cast(),
            size,
        )
    };
    ok != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kill_on_close_is_kept_with_the_cap() {
        let info = limits_for(1 << 30);
        let basic = info.BasicLimitInformation;
        let flags = basic.LimitFlags;
        assert_ne!(flags & JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, 0);
        assert_ne!(flags & JOB_OBJECT_LIMIT_PROCESS_MEMORY, 0);
        assert_eq!(info.ProcessMemoryLimit, 1 << 30);
        // No working-set limit (review M2).
        assert_eq!(basic.MaximumWorkingSetSize, 0);
        // Security review M3: the process can start nothing.
        assert_ne!(flags & JOB_OBJECT_LIMIT_ACTIVE_PROCESS, 0);
        assert_eq!(basic.ActiveProcessLimit, 1);
        // Security review M2: no Windows Error Reporting for a crash.
        assert_ne!(flags & JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION, 0);
        // Nothing else (no breakaway, no time or affinity limits).
        assert_eq!(
            flags,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
                | JOB_OBJECT_LIMIT_PROCESS_MEMORY
                | JOB_OBJECT_LIMIT_ACTIVE_PROCESS
                | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION
        );
    }

    #[test]
    fn a_new_job_takes_the_limits() {
        let job = win32job::Job::create().unwrap();
        // The worker tests show the limits acting (`a_worker_can_start_nothing`).
        assert!(confine_job(&job, 64 * 1024 * 1024));
    }
}
